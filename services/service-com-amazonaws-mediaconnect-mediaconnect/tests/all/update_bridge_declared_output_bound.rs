// SPDX-License-Identifier: CC0-1.0

//! UpdateBridge against the outputs a bridge already holds, end to end over HTTP:
//! CreateBridge -> CreateFlow (a flow source naming the bridge) -> UpdateBridge ->
//! DescribeBridge.
//!
//! The rule under test: an operation that MOVES a bound a bridge's own record declares
//! re-evaluates that bound against what the record already holds and refuses the request
//! — storing nothing at all — when the stored record would no longer satisfy it. For
//! `IngressGatewayBridge$MaxOutputs` ("the maximum number of outputs on the ingress
//! bridge", `smithy_query(query="shape",shape="IngressGatewayBridge")`) what it bounds are
//! the outputs a ground-to-cloud bridge holds — the `BridgeFlowOutput`s the service
//! materialises for the cloud flow sources that name the bridge, a bridge of that
//! direction taking no output a request configures — so an UpdateBridge may not declare
//! fewer of them than the bridge is holding, exactly as it may not move the failover state
//! under the sources the bridge is holding.
//!
//! `bridge_flow_output_capacity` tables the same bound from the ADD side (how many flow
//! outputs a bridge takes) and, at one held count, the declaration side. What this suite
//! adds is the rest of what the rule generalises over on the declaration side: how many
//! outputs the bridge HOLDS when the update arrives (none, one, two), a negative
//! declaration (the member targets `Integer`, which the model puts no range on), and which
//! OTHER settings the same request carries (none, the wrapper's `MaxBitrate`, the bridge's
//! failover config, nothing at all) — because a refused update may write none of them
//! either. Its helpers are that suite's, so there is one spelling of each call.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_flow_output_capacity::{
    assert_refused, bridge_source, reported_flow_outputs, update_bridge,
};
use crate::bridge_lifecycle::{create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::network_source;
use crate::flow_lifecycle::Emulator;
use crate::flow_sources::create_flow_from;

/// The bitrate every row's bridge is created with, so a row that carries no bitrate of its
/// own knows what the record must still report.
const CREATED_BITRATE: i64 = 20_000_000;

/// The whole bridge DescribeBridge reports, so a row can compare the record before the
/// update with the record after it, member by member.
async fn described_bridge(emulator: &Emulator, case: &str, bridge_arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    described["bridge"].clone()
}

/// A refusal of this bound states the bound it refused BY: the room the declaration would
/// leave, the outputs the bridge holds, and that outputs are what is being counted — so a
/// row cannot pass on one of this handler's other validation refusals (the source bound,
/// the primary source).
fn assert_names_the_output_bound(case: &str, message: &str, capacity: i64, held: usize) {
    for (what, needle) in [
        ("the room the declaration would leave", capacity.to_string()),
        ("the outputs the bridge holds", held.to_string()),
    ] {
        assert!(
            message.contains(&needle),
            "case {case:?}: the refusal names {what} ({needle}), got {message:?}"
        );
    }
    assert!(
        message.contains("output"),
        "case {case:?}: the refusal says outputs are what it counted, got {message:?}"
    );
}

/// One row: a ground-to-cloud bridge declaring `declared` outputs and holding `held` of
/// them, the UpdateBridge request applied to it, and whether the rule admits it.
struct BoundRow {
    case: &'static str,
    /// `IngressGatewayBridge$MaxOutputs` the create declares.
    declared: i64,
    /// How many cloud flows this row points at the bridge, one source each, so the bridge
    /// holds exactly that many flow outputs when the update arrives.
    held: usize,
    /// The whole UpdateBridge body, bridge ARN aside.
    update: Value,
    /// Whether the rule admits the update while the bridge holds `held` outputs.
    accepted: bool,
    /// The room the declaration this row carries would leave, which a refusal states. A
    /// non-positive declaration is room for nothing.
    refused_capacity: i64,
    /// `IngressGatewayBridge$MaxOutputs` the bridge declares afterwards. On a refused
    /// update this is `declared`, because a refusal stores nothing.
    declared_after: i64,
    /// `IngressGatewayBridge$MaxBitrate` the bridge declares afterwards: the wrapper's
    /// other member, which a refused update may not write either.
    bitrate_after: i64,
}

fn bound_rows() -> Vec<BoundRow> {
    vec![
        BoundRow {
            case: "no outputs held: a declaration of no room at all is admitted",
            declared: 2,
            held: 0,
            update: json!({ "ingressGatewayBridge": { "maxOutputs": 0 } }),
            accepted: true,
            refused_capacity: 0,
            declared_after: 0,
            bitrate_after: CREATED_BITRATE,
        },
        BoundRow {
            case: "one output held, a declaration under it",
            declared: 2,
            held: 1,
            update: json!({ "ingressGatewayBridge": { "maxOutputs": 0 } }),
            accepted: false,
            refused_capacity: 0,
            declared_after: 2,
            bitrate_after: CREATED_BITRATE,
        },
        BoundRow {
            case: "one output held, a declaration exactly at it",
            declared: 2,
            held: 1,
            update: json!({ "ingressGatewayBridge": { "maxOutputs": 1 } }),
            accepted: true,
            refused_capacity: 1,
            declared_after: 1,
            bitrate_after: CREATED_BITRATE,
        },
        BoundRow {
            case: "two outputs held, a negative declaration: room for nothing",
            declared: 3,
            held: 2,
            update: json!({ "ingressGatewayBridge": { "maxOutputs": -1 } }),
            accepted: false,
            refused_capacity: 0,
            declared_after: 3,
            bitrate_after: CREATED_BITRATE,
        },
        BoundRow {
            case: "two outputs held, a declaration exactly at them beside the wrapper's \
                   other member: both are written",
            declared: 3,
            held: 2,
            update: json!({
                "ingressGatewayBridge": { "maxBitrate": 123_000_000, "maxOutputs": 2 },
            }),
            accepted: true,
            refused_capacity: 2,
            declared_after: 2,
            bitrate_after: 123_000_000,
        },
        BoundRow {
            case: "two outputs held, a declaration under them beside the wrapper's other \
                   member and a failover config: none of the three is written",
            declared: 3,
            held: 2,
            update: json!({
                "ingressGatewayBridge": { "maxBitrate": 123_000_000, "maxOutputs": 1 },
                "sourceFailoverConfig": { "state": mc::STATE_ENABLED, "failoverMode": "MERGE" },
            }),
            accepted: false,
            refused_capacity: 1,
            declared_after: 3,
            bitrate_after: CREATED_BITRATE,
        },
        BoundRow {
            case: "two outputs held, the bitrate alone: the declaration is not moved, so \
                   the outputs held are no reason to refuse",
            declared: 3,
            held: 2,
            update: json!({ "ingressGatewayBridge": { "maxBitrate": 44_000_000 } }),
            accepted: true,
            refused_capacity: 3,
            declared_after: 3,
            bitrate_after: 44_000_000,
        },
        BoundRow {
            case: "two outputs held, a request that moves no bridge setting at all",
            declared: 3,
            held: 2,
            update: json!({}),
            accepted: true,
            refused_capacity: 3,
            declared_after: 3,
            bitrate_after: CREATED_BITRATE,
        },
    ]
}

/// Every row: UpdateBridge admits a declaration the outputs the bridge holds fit under and
/// refuses one they do not, and a refused update leaves the whole stored bridge as it was.
#[tokio::test]
async fn update_bridge_weighs_the_declared_output_maximum_against_the_outputs_held() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-declared-bound", &["net-1"]).await;

    for (index, row) in bound_rows().into_iter().enumerate() {
        let BoundRow {
            case,
            declared,
            held,
            update,
            accepted,
            refused_capacity,
            declared_after,
            bitrate_after,
        } = row;

        // A fresh bridge per row, so what one row stores cannot decide the next.
        let (status, code, created) = create_bridge(
            &emulator,
            json!({
                "name": format!("br-declared-bound-{index}"),
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": CREATED_BITRATE, "maxOutputs": declared },
                "sources": [network_source("net-a", 5_000 + index as i64, "rtp")],
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the bridge under test must be created, got {status} {code:?}: \
             {created}"
        );
        let bridge_arn = created["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        // Each cloud flow whose source names the bridge gives it one flow output, which is
        // the only kind of output a ground-to-cloud bridge holds.
        for offset in 0..held {
            create_flow_from(
                &emulator,
                case,
                json!({
                    "name": format!("flow-declared-bound-{index}-{offset}"),
                    "source": bridge_source("from-bridge", &bridge_arn),
                }),
            )
            .await;
        }

        let before = described_bridge(&emulator, case, &bridge_arn).await;
        assert_eq!(
            reported_flow_outputs(&emulator, case, &bridge_arn).await.len(),
            held,
            "case {case:?}: the bridge holds one output per flow source naming it, got {:?}",
            before["outputs"]
        );

        let (status, code, answered) = update_bridge(&emulator, &bridge_arn, update).await;
        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: a declaration the outputs held fit under expects 202, got \
                 {status} {code:?}: {answered}"
            );
        } else {
            assert_refused(case, status, code.as_deref(), &answered);
            assert_names_the_output_bound(
                case,
                answered["message"].as_str().unwrap_or_default(),
                refused_capacity,
                held,
            );
        }

        let after = described_bridge(&emulator, case, &bridge_arn).await;
        assert_eq!(
            after["ingressGatewayBridge"]["maxOutputs"].as_i64(),
            Some(declared_after),
            "case {case:?}: expected the stored maxOutputs {declared_after}, got {:?}",
            after["ingressGatewayBridge"]
        );
        assert_eq!(
            after["ingressGatewayBridge"]["maxBitrate"].as_i64(),
            Some(bitrate_after),
            "case {case:?}: expected the stored maxBitrate {bitrate_after}, got {:?}",
            after["ingressGatewayBridge"]
        );
        assert_eq!(
            reported_flow_outputs(&emulator, case, &bridge_arn).await.len(),
            held,
            "case {case:?}: no update moves the outputs the bridge holds, got {:?}",
            after["outputs"]
        );
        if !accepted {
            // A refused update is not a partial one: nothing the request carried reached
            // the record, so the whole bridge reads back exactly as it was — the failover
            // config the same request carried included.
            assert_eq!(
                after, before,
                "case {case:?}: a refused update writes nothing, got {after} after {before}"
            );
        }
    }
}
