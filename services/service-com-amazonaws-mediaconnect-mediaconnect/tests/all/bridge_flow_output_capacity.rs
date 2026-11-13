// SPDX-License-Identifier: CC0-1.0

//! How many flow outputs a bridge takes, end to end over HTTP: CreateBridge / CreateFlow /
//! AddFlowSources / UpdateFlowSource / UpdateBridge -> DescribeBridge.
//!
//! The rule under test: a bridge holds no more outputs than its own type declares. A cloud
//! flow source naming a bridge (`GatewayBridgeSource$BridgeArn`, "the ARN of the bridge
//! feeding this flow") gives that bridge exactly one output — the `BridgeFlowOutput` the
//! service materialises for it — and for a ground-to-cloud bridge those are precisely the
//! outputs its REQUIRED `IngressGatewayBridge$MaxOutputs` ("the maximum number of outputs on
//! the ingress bridge") bounds, a bridge of that direction taking no output a request
//! configures at all. A cloud-to-ground bridge's `EgressGatewayBridge` declares no output
//! count, so the only bound on it is the one this crate applies to every bridge
//! (`mc::MAX_BRIDGE_OUTPUTS`). So a request that would leave a bridge holding more outputs
//! than it declares is refused with `BadRequestException` and stores nothing — whether it is
//! a write that points a flow source at the bridge, or an UpdateBridge that moves the
//! declaration under the outputs the bridge already holds.
//!
//! The rows vary along what the rule generalises over: the maximum the bridge DECLARES (none,
//! one, several, a negative declaration, and no declaration at all on a cloud-to-ground
//! bridge), WHICH call points the flow source at the bridge (CreateFlow's singular `Source`,
//! CreateFlow's `Sources` list, AddFlowSources, UpdateFlowSource), how many sources ONE call
//! points at the same bridge, how many outputs the bridge already holds when the call
//! arrives, and — for the declaration side — whether the new maximum is under, at or over
//! what the bridge holds.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::{flow_source, network_source};
use crate::flow_lifecycle::Emulator;
use crate::flow_sources::{add_flow_sources, create_flow_from, described_sources, failover_active};
use crate::update_flow_source::update_flow_source;

/// `PUT /v1/bridges/{BridgeArn}` — the UpdateBridge binding
/// (`smithy_query(query="operation",operation="UpdateBridge")`).
pub(crate) async fn update_bridge(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/bridges/{arn}"), Some(body))
        .await
}

/// `DELETE /v1/flows/{FlowArn}` — the DeleteFlow binding
/// (`smithy_query(query="operation",operation="DeleteFlow")`).
async fn delete_flow(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await
}

/// A source of a flow that takes the bridge `bridge_arn` as its source, as a request
/// configures one.
pub(crate) fn bridge_source(name: &str, bridge_arn: &str) -> Value {
    json!({ "name": name, "gatewayBridgeSource": { "bridgeArn": bridge_arn } })
}

/// A source that takes no bridge, for a flow that needs one before the case's own.
fn plain_source(name: &str) -> Value {
    json!({ "name": name, "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" })
}

/// The flow outputs `bridge_arn` reports — the `Bridge$Outputs` entries carrying a
/// `flowOutput`, which is every output a ground-to-cloud bridge has.
pub(crate) async fn reported_flow_outputs(
    emulator: &Emulator,
    case: &str,
    bridge_arn: &str,
) -> Vec<Value> {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|output| output.get("flowOutput").is_some_and(|output| !output.is_null()))
        .collect()
}

/// `Bridge$IngressGatewayBridge.MaxOutputs` as DescribeBridge reports it.
async fn reported_max_outputs(emulator: &Emulator, case: &str, bridge_arn: &str) -> Option<i64> {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["ingressGatewayBridge"]["maxOutputs"].as_i64()
}

/// Create one ground-to-cloud (ingress) bridge declaring `max_outputs` outputs, and answer
/// its ARN. The bridge carries the one source CreateBridge requires, of the kind its own
/// direction ingests ("the content originates at your premises").
async fn ingress_bridge(
    emulator: &Emulator,
    case: &str,
    name: &str,
    placement: &str,
    max_outputs: i64,
) -> String {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": max_outputs },
            "sources": [network_source("net-a", 5_000, "rtp")],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the bridge under test must be created, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create one cloud-to-ground (egress) bridge — which declares no output count of its own —
/// and answer its ARN.
async fn egress_bridge(
    emulator: &Emulator,
    case: &str,
    name: &str,
    placement: &str,
    content_from: &str,
) -> String {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement,
            "egressGatewayBridge": { "maxBitrate": 80_000_000 },
            "sources": [flow_source("flow-src", content_from)],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the bridge under test must be created, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A refusal of this service states what was unacceptable inside its validation envelope.
pub(crate) fn assert_refused(case: &str, status: u16, code: Option<&str>, answered: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: a request past the bridge's declared maximum expects 400 \
         BadRequestException, got {status} {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: the refusal states what was wrong, got {message:?}"
    );
}

/// One row of the declaration table: the bridge a create describes, how many flows are
/// pointed at it one at a time, and how many of those writes the rule admits.
struct CapacityRow {
    case: &'static str,
    /// `IngressGatewayBridge$MaxOutputs` the create declares, or `None` for a
    /// cloud-to-ground bridge, which declares no output count at all.
    declared: Option<i64>,
    /// How many flows this row points at the bridge, one source each.
    driven: usize,
    /// How many of those writes the rule admits; the rest are refused.
    admitted: usize,
}

/// A bridge holds no more flow outputs than its own type declares: the writes up to the
/// declared maximum are accepted and reported, and the next one is refused and stores
/// nothing — until a flow gives its output back, when the same write is accepted again.
#[tokio::test]
async fn a_bridge_takes_no_more_flow_outputs_than_its_own_type_declares() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-flow-capacity", &["net-1"]).await;
    // The cloud flow a cloud-to-ground bridge's content comes from, which its create
    // resolves against the store.
    let content_from = cloud_flow(&emulator, "flow-capacity-content").await;

    // The declaration's range, from the model's own member: no room at all, room for one,
    // room for several, a negative declaration the `Integer` member alone does not keep out,
    // and no declaration at all.
    let rows = [
        CapacityRow {
            case: "a ground-to-cloud bridge declaring room for none",
            declared: Some(0),
            driven: 1,
            admitted: 0,
        },
        CapacityRow {
            case: "a ground-to-cloud bridge declaring room for one",
            declared: Some(1),
            driven: 2,
            admitted: 1,
        },
        CapacityRow {
            case: "a ground-to-cloud bridge declaring room for two",
            declared: Some(2),
            driven: 3,
            admitted: 2,
        },
        CapacityRow {
            case: "a ground-to-cloud bridge declaring room for three",
            declared: Some(3),
            driven: 4,
            admitted: 3,
        },
        CapacityRow {
            case: "a negative declaration is room for nothing",
            declared: Some(-1),
            driven: 1,
            admitted: 0,
        },
        CapacityRow {
            case: "a cloud-to-ground bridge declares no output count of its own",
            declared: None,
            driven: 4,
            admitted: 4,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let CapacityRow {
            case,
            declared,
            driven,
            admitted,
        } = row;
        let bridge_arn = match declared {
            Some(max_outputs) => {
                ingress_bridge(
                    &emulator,
                    case,
                    &format!("br-capacity-{index}"),
                    &placement,
                    *max_outputs,
                )
                .await
            }
            None => {
                egress_bridge(
                    &emulator,
                    case,
                    &format!("br-capacity-{index}"),
                    &placement,
                    &content_from,
                )
                .await
            }
        };

        let mut accepted_flows: Vec<String> = Vec::new();
        for attempt in 0..*driven {
            let body = json!({
                "name": format!("flow-capacity-{index}-{attempt}"),
                "source": bridge_source("from-bridge", &bridge_arn),
            });
            let (status, code, answered) = emulator.create_flow(body).await;
            let outputs = reported_flow_outputs(&emulator, case, &bridge_arn).await;
            if attempt < *admitted {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: write {attempt} is within the declared maximum and expects \
                     201, got {status} {code:?}: {answered}"
                );
                accepted_flows.push(
                    answered["flow"]["flowArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
                assert_eq!(
                    outputs.len(),
                    attempt + 1,
                    "case {case:?}: after write {attempt} the bridge reports one flow output per \
                     flow source naming it, got {outputs:?}"
                );
            } else {
                assert_refused(case, status, code.as_deref(), &answered);
                assert_eq!(
                    outputs.len(),
                    *admitted,
                    "case {case:?}: a refused write leaves the bridge's outputs as they were, \
                     got {outputs:?}"
                );
            }
        }

        // The refusal is the CAPACITY, not a blanket refusal: a flow that gives its output
        // back leaves room for the very write that was refused.
        if *admitted > 0 && driven > admitted {
            let released = accepted_flows
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: a write was admitted above"))
                .clone();
            let (status, code, deleted) = delete_flow(&emulator, &released).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the flow holding an output must be deleted, got {status} \
                 {code:?}: {deleted}"
            );
            let (status, code, answered) = emulator
                .create_flow(json!({
                    "name": format!("flow-capacity-{index}-retry"),
                    "source": bridge_source("from-bridge", &bridge_arn),
                }))
                .await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the write refused above is accepted once the bridge has room \
                 again, got {status} {code:?}: {answered}"
            );
            let outputs = reported_flow_outputs(&emulator, case, &bridge_arn).await;
            assert_eq!(
                outputs.len(),
                *admitted,
                "case {case:?}: the bridge is back at its declared maximum, got {outputs:?}"
            );
        }
    }
}

/// Which call points the flow source at the bridge: the four paths that write a
/// `GatewayBridgeSource` onto a flow source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WritePath {
    /// CreateFlow's own `Source` — "the settings for the source of the flow".
    CreateFlowSource,
    /// CreateFlow's `Sources` list, carrying `sources` entries naming the bridge.
    CreateFlowSources,
    /// AddFlowSources, appending a source to a flow that already holds one.
    AddFlowSources,
    /// UpdateFlowSource, pointing a source the flow already holds at the bridge.
    UpdateFlowSource,
}

/// One row of the write-path table: which call writes the source, and how many sources
/// that one call points at the bridge.
struct WriteRow {
    case: &'static str,
    path: WritePath,
    /// How many sources this one call points at the bridge.
    pointing: usize,
}

/// Every path that points a flow source at a bridge is admitted by the bridge's declared
/// maximum, and a refused one stores nothing — including a single call pointing a whole
/// batch of sources at one bridge, which asks for one output per source.
#[tokio::test]
async fn every_path_that_points_a_flow_source_at_a_bridge_is_admitted_by_the_declared_maximum() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-flow-paths", &["net-1"]).await;

    let rows = [
        WriteRow {
            case: "CreateFlow's own Source, on a bridge already at its maximum",
            path: WritePath::CreateFlowSource,
            pointing: 1,
        },
        WriteRow {
            case: "CreateFlow's Sources list, on a bridge already at its maximum",
            path: WritePath::CreateFlowSources,
            pointing: 1,
        },
        WriteRow {
            case: "AddFlowSources, on a bridge already at its maximum",
            path: WritePath::AddFlowSources,
            pointing: 1,
        },
        WriteRow {
            case: "UpdateFlowSource, on a bridge already at its maximum",
            path: WritePath::UpdateFlowSource,
            pointing: 1,
        },
        WriteRow {
            case: "CreateFlow's Sources list pointing a PAIR of sources at one bridge",
            path: WritePath::CreateFlowSources,
            pointing: 2,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let WriteRow {
            case,
            path,
            pointing,
        } = row;
        // A bridge declaring room for ONE output. A row whose single call points a pair of
        // sources at it finds it EMPTY, so what refuses that call is the batch itself; every
        // other row finds it already holding the one output it declares.
        let bridge_arn = ingress_bridge(
            &emulator,
            case,
            &format!("br-paths-{index}"),
            &placement,
            1,
        )
        .await;
        let fills_it_first = *pointing == 1;
        if fills_it_first {
            create_flow_from(
                &emulator,
                case,
                json!({
                    "name": format!("flow-paths-holder-{index}"),
                    "source": bridge_source("from-bridge", &bridge_arn),
                }),
            )
            .await;
        }
        let held_before = reported_flow_outputs(&emulator, case, &bridge_arn).await;
        assert_eq!(
            held_before.len(),
            usize::from(fills_it_first),
            "case {case:?}: the bridge starts where the row says it does, got {held_before:?}"
        );

        let pointed: Vec<Value> = (0..*pointing)
            .map(|offset| bridge_source(&format!("from-bridge-{offset}"), &bridge_arn))
            .collect();
        let (status, code, answered) = match path {
            WritePath::CreateFlowSource => {
                emulator
                    .create_flow(json!({
                        "name": format!("flow-paths-{index}"),
                        "source": pointed[0].clone(),
                    }))
                    .await
            }
            WritePath::CreateFlowSources => {
                let mut body = json!({
                    "name": format!("flow-paths-{index}"),
                    "sources": pointed,
                });
                // "If the state is active, the flow can have one or two sources"
                // (`FailoverConfig$State`), so a flow carrying a PAIR needs failover active.
                if *pointing > 1 {
                    body["sourceFailoverConfig"] = failover_active();
                }
                emulator.create_flow(body).await
            }
            WritePath::AddFlowSources => {
                let flow_arn = create_flow_from(
                    &emulator,
                    case,
                    json!({
                        "name": format!("flow-paths-{index}"),
                        "sourceFailoverConfig": failover_active(),
                        "source": plain_source("plain"),
                    }),
                )
                .await;
                let answered = add_flow_sources(&emulator, &flow_arn, json!(pointed)).await;
                // A refused add appends nothing: the flow still holds the one source it was
                // created with.
                let sources = described_sources(&emulator, case, &flow_arn).await;
                assert_eq!(
                    sources.len(),
                    1,
                    "case {case:?}: a refused add leaves the flow's sources as they were, got \
                     {sources:?}"
                );
                answered
            }
            WritePath::UpdateFlowSource => {
                let flow_arn = create_flow_from(
                    &emulator,
                    case,
                    json!({
                        "name": format!("flow-paths-{index}"),
                        "source": plain_source("plain"),
                    }),
                )
                .await;
                let source_arn = described_sources(&emulator, case, &flow_arn).await[0]
                    ["sourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let answered = update_flow_source(
                    &emulator,
                    &flow_arn,
                    &source_arn,
                    json!({ "gatewayBridgeSource": { "bridgeArn": bridge_arn } }),
                )
                .await;
                // A refused update moves nothing: the source is still fed by no bridge.
                let sources = described_sources(&emulator, case, &flow_arn).await;
                assert!(
                    sources[0]["gatewayBridgeSource"].is_null(),
                    "case {case:?}: a refused update leaves the source as it was, got \
                     {sources:?}"
                );
                answered
            }
        };
        assert_refused(case, status, code.as_deref(), &answered);
        let held_after = reported_flow_outputs(&emulator, case, &bridge_arn).await;
        assert_eq!(
            held_after, held_before,
            "case {case:?}: a refused write leaves the bridge's outputs as they were, got \
             {held_after:?}"
        );
    }
}

/// A request that RESTATES the bridge a flow source already names asks for no further
/// output, so it is never refused for the output that source already has — even on a bridge
/// declaring room for exactly one.
#[tokio::test]
async fn restating_the_bridge_a_source_already_names_asks_for_no_further_output() {
    let emulator = Emulator::new();
    let case = "UpdateFlowSource restating the bridge the source already names";
    let placement = gateway_with_networks(&emulator, "gw-flow-restate", &["net-1"]).await;
    let bridge_arn = ingress_bridge(&emulator, case, "br-restate", &placement, 1).await;
    let flow_arn = create_flow_from(
        &emulator,
        case,
        json!({
            "name": "flow-restate",
            "source": bridge_source("from-bridge", &bridge_arn),
        }),
    )
    .await;
    let before = reported_flow_outputs(&emulator, case, &bridge_arn).await;
    assert_eq!(
        before.len(),
        1,
        "case {case:?}: the bridge holds the one output its flow source asked for, got \
         {before:?}"
    );
    let source_arn = described_sources(&emulator, case, &flow_arn).await[0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (attempt, body) in [
        json!({ "gatewayBridgeSource": { "bridgeArn": bridge_arn } }),
        json!({ "description": "the same bridge, other settings moved" }),
    ]
    .iter()
    .enumerate()
    {
        let (status, code, answered) =
            update_flow_source(&emulator, &flow_arn, &source_arn, body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: update {attempt} asks for no further output and expects 202, got \
             {status} {code:?}: {answered}"
        );
        let after = reported_flow_outputs(&emulator, case, &bridge_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: update {attempt} leaves the bridge with the one output it had, got \
             {after:?}"
        );
    }
}

/// One row of the declaration-moving table: the maximum an UpdateBridge declares, and
/// whether the rule admits it while the bridge holds two flow outputs.
struct DeclarationRow {
    case: &'static str,
    /// The `IngressGatewayBridge` the update carries.
    update: Value,
    /// Whether the rule admits it while the bridge holds two flow outputs.
    accepted: bool,
    /// The maximum the bridge declares afterwards: unchanged on a refused update.
    declared_after: i64,
}

/// UpdateBridge moves the DECLARATION rather than the output count, so it cannot declare
/// fewer outputs than the bridge already holds — and once the bridge holds fewer, the same
/// update is accepted.
#[tokio::test]
async fn update_bridge_cannot_declare_fewer_outputs_than_the_bridge_holds() {
    let emulator = Emulator::new();
    let case = "a ground-to-cloud bridge holding two flow outputs";
    let placement = gateway_with_networks(&emulator, "gw-flow-declaration", &["net-1"]).await;
    let bridge_arn = ingress_bridge(&emulator, case, "br-declaration", &placement, 3).await;
    let mut flows = Vec::new();
    for offset in 0..2 {
        flows.push(
            create_flow_from(
                &emulator,
                case,
                json!({
                    "name": format!("flow-declaration-{offset}"),
                    "source": bridge_source("from-bridge", &bridge_arn),
                }),
            )
            .await,
        );
    }
    let held = reported_flow_outputs(&emulator, case, &bridge_arn).await;
    assert_eq!(
        held.len(),
        2,
        "case {case:?}: the bridge holds one output per flow source naming it, got {held:?}"
    );

    // The rows span the new maximum against what the bridge holds: under it, at it, over it,
    // none at all, and an update that does not touch the declaration.
    let rows = [
        DeclarationRow {
            case: "a maximum under the outputs the bridge holds",
            update: json!({ "maxOutputs": 1 }),
            accepted: false,
            declared_after: 3,
        },
        DeclarationRow {
            case: "no room at all while the bridge holds outputs",
            update: json!({ "maxOutputs": 0 }),
            accepted: false,
            declared_after: 3,
        },
        DeclarationRow {
            case: "an update that moves the bitrate and not the maximum",
            update: json!({ "maxBitrate": 30_000_000 }),
            accepted: true,
            declared_after: 3,
        },
        DeclarationRow {
            case: "a maximum exactly at the outputs the bridge holds",
            update: json!({ "maxOutputs": 2 }),
            accepted: true,
            declared_after: 2,
        },
        DeclarationRow {
            case: "a maximum over the outputs the bridge holds",
            update: json!({ "maxOutputs": 5 }),
            accepted: true,
            declared_after: 5,
        },
    ];

    for row in &rows {
        let DeclarationRow {
            case,
            update,
            accepted,
            declared_after,
        } = row;
        let (status, code, answered) = update_bridge(
            &emulator,
            &bridge_arn,
            json!({ "ingressGatewayBridge": update }),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: an update the bridge's outputs fit under expects 202, got \
                 {status} {code:?}: {answered}"
            );
        } else {
            assert_refused(case, status, code.as_deref(), &answered);
        }
        assert_eq!(
            reported_max_outputs(&emulator, case, &bridge_arn).await,
            Some(*declared_after),
            "case {case:?}: the stored declaration is what the rule says, got {answered}"
        );
        let after = reported_flow_outputs(&emulator, case, &bridge_arn).await;
        assert_eq!(
            after.len(),
            2,
            "case {case:?}: no update moves the outputs the bridge holds, got {after:?}"
        );
    }

    // And once a flow gives its output back, the maximum the bridge refused above is
    // accepted — the bound is on what the bridge HOLDS, not on the number itself.
    let case = "the maximum refused above, once the bridge holds one output";
    let (status, code, deleted) = delete_flow(&emulator, &flows[0]).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the flow holding an output must be deleted, got {status} {code:?}: \
         {deleted}"
    );
    let (status, code, answered) = update_bridge(
        &emulator,
        &bridge_arn,
        json!({ "ingressGatewayBridge": { "maxOutputs": 1 } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202, got {status} {code:?}: {answered}"
    );
    assert_eq!(
        reported_max_outputs(&emulator, case, &bridge_arn).await,
        Some(1),
        "case {case:?}: the bridge declares the maximum the update stored"
    );
    // The declaration it now carries bounds the next write, read back through the flow-source
    // path: the bridge is full at its new maximum.
    let (status, code, answered) = emulator
        .create_flow(json!({
            "name": "flow-declaration-past-new-maximum",
            "source": bridge_source("from-bridge", &bridge_arn),
        }))
        .await;
    assert_refused(case, status, code.as_deref(), &answered);
    let after = reported_flow_outputs(&emulator, case, &bridge_arn).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: the bridge holds the one output its remaining flow source asked for, \
         got {after:?}"
    );
}
