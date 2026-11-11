// SPDX-License-Identifier: CC0-1.0

//! The stream-state PRECONDITION of the resource-policy surface, end to end over the wire:
//! `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` against a stream that
//! is not available for the operation.
//!
//! RULE: a resource-policy operation addressing a Kinesis data stream — the stream itself, or
//! a consumer registered with it — requires that stream to be in a state the operation is
//! available in, which is `ACTIVE` or the `UPDATING` a change still in flight leaves it in;
//! any other state is refused with the modelled `ResourceInUseException` ("The resource is not
//! available for this operation. For successful operation, the resource must be in the ACTIVE
//! state") carrying ONE sentence that names the stream by the `StreamName` it is stored under,
//! the account that holds it and the state it is in instead. The gate stands BEFORE the policy
//! store is read or written, so a refused call reports neither the presence nor the absence of
//! a policy, and leaves the stored slot exactly as it found it.
//!
//! The recorded flows these were derived from call each of the three operations once, on ONE
//! stream in `CREATING`, addressed by its stream ARN — each one row of the tables below. The
//! rows differ along what the rule generalises over: the OPERATION called, the lifecycle STATE
//! the stream is in (the two states that refuse, and the two that serve), the ARN FORM the
//! request addresses the resource by (the stream's own ARN and a consumer's), and whether the
//! stream carries a policy when the call arrives.
//!
//! The wire harness, the ARN builders and the policy helpers are [`crate::stream_tags`]' and
//! [`crate::resource_policies`]', reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING, STREAM_UPDATING,
};

use crate::resource_policies::{account, assert_refused, delete, get, policy_for, put, reported_policy};
use crate::resource_policy_absence::EMPTY_POLICY_DOCUMENT;
use crate::stream_tags::{past_activation, stream_arn, Emulator, REGION};

/// The modelled error every one of these refusals is carried in.
const IN_USE: &str = "ResourceInUseException";

/// The three operations the precondition stands in front of, with the body each one addresses
/// `arn` with. Every body is well formed, so what refuses the call is the stream's state and
/// nothing else.
fn body_for(operation: &str, arn: &str) -> Value {
    match operation {
        "GetResourcePolicy" | "DeleteResourcePolicy" => json!({ "ResourceARN": arn }),
        "PutResourcePolicy" => json!({
            "ResourceARN": arn,
            "Policy": policy_for(arn, "Readers", "kinesis:GetRecords"),
        }),
        other => panic!("case {other}: this suite builds no body for that operation"),
    }
}

/// The operations the rule covers, in the order a client uses them.
const POLICY_OPERATIONS: &[&str] = &["PutResourcePolicy", "GetResourcePolicy", "DeleteResourcePolicy"];

/// The sentence this service states for a stream that is not available for a resource-policy
/// operation, written out here rather than borrowed from the crate so the wording itself is
/// what the cases pin.
fn not_ready_sentence(stream_name: &str, status: &str) -> String {
    format!(
        "Stream {stream_name} under account {} not {STREAM_ACTIVE} or {STREAM_UPDATING}, \
         instead in state {status}",
        account()
    )
}

/// Create a provisioned stream of one shard named `name` without waiting for the transition
/// the create armed: the stream is `CREATING` when this returns.
async fn creating_stream(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
}

/// Call `operation` against `arn` and assert it was refused with the one sentence for `status`.
async fn assert_refused_with_the_sentence(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    arn: &str,
    stream_name: &str,
    status: &str,
) {
    let (http_status, response) = emulator.call(operation, &body_for(operation, arn)).await;
    assert_refused(
        case,
        http_status,
        &response,
        IN_USE,
        &not_ready_sentence(stream_name, status),
    );
}

/// What the store holds in the policy slot of `arn`: `Some(document)` for a resource carrying
/// one, `None` for a resource carrying none. Read from the very store the handlers write, so a
/// refused call's effect on it — there must be none — is what a case can assert.
fn stored_policy(emulator: &Emulator, case: &str, arn: &str) -> Option<String> {
    emulator
        .store()
        .lock()
        .resource_policy_by_arn(std::time::SystemTime::now(), REGION, arn)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: this account and region must still hold {arn}"))
}

/// A stream whose create has not fallen due yet refuses every one of the three operations,
/// with the one sentence naming the stored name, this account and `CREATING` — and the refused
/// attach writes nothing, so the stream carries no policy once it is ready.
#[tokio::test]
async fn a_creating_stream_refuses_every_resource_policy_operation() {
    let emulator = Emulator::new();

    for operation in POLICY_OPERATIONS {
        let name = format!("creating-{}", operation.to_lowercase());
        // Created and called straight away, so the stream is still inside the transition its
        // create armed when the call arrives.
        creating_stream(&emulator, &name).await;
        let arn = stream_arn(REGION, &name);
        assert_refused_with_the_sentence(
            &emulator,
            &name,
            operation,
            &arn,
            &name,
            STREAM_CREATING,
        )
        .await;
        assert_eq!(
            stored_policy(&emulator, &name, &arn),
            None,
            "case {name}: a refused {operation} must leave the stream carrying no policy"
        );
    }

    // ... and once every one of those streams is ready, the read is served and reports the
    // empty document: nothing was attached behind the refusals.
    tokio::time::sleep(past_activation()).await;
    for operation in POLICY_OPERATIONS {
        let name = format!("creating-{}", operation.to_lowercase());
        let arn = stream_arn(REGION, &name);
        let (status, read) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(&name, status, &read),
            EMPTY_POLICY_DOCUMENT,
            "case {name}: expected the empty document for a stream nothing attached a policy \
             to, got {read}"
        );
    }
}

/// A stream whose removal has been accepted refuses every one of the three operations with the
/// same sentence, naming the state it reports — even though it DOES carry a policy, which is
/// what shows the gate precedes the policy lookup: the refusal is the state's, not an absence,
/// and the document the stream carries is still there afterwards.
#[tokio::test]
async fn a_deleting_stream_refuses_every_resource_policy_operation() {
    let emulator = Emulator::new();

    let names: Vec<String> = POLICY_OPERATIONS
        .iter()
        .map(|operation| format!("deleting-{}", operation.to_lowercase()))
        .collect();
    for name in &names {
        creating_stream(&emulator, name).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (operation, name) in POLICY_OPERATIONS.iter().zip(&names) {
        let arn = stream_arn(REGION, name);
        let attached = policy_for(&arn, "Attached", "kinesis:DescribeStreamSummary");
        let (status, response) = put(&emulator, REGION, &arn, &attached).await;
        assert_eq!(
            status, 200,
            "case {name}: the attach on the ACTIVE stream must be served, got {status} with \
             body {response}"
        );

        // The removal is armed and the call arrives before it falls due, so the stream is
        // `DELETING` rather than gone.
        emulator
            .ok(name, "DeleteStream", &json!({ "StreamName": name }))
            .await;
        assert_refused_with_the_sentence(
            &emulator,
            name,
            operation,
            &arn,
            name,
            STREAM_DELETING,
        )
        .await;
        assert_eq!(
            stored_policy(&emulator, name, &arn).as_deref(),
            Some(attached.as_str()),
            "case {name}: a refused {operation} must leave the attached document alone"
        );
    }
}

/// A CONSUMER's policy is addressed through the stream that holds it, so the same gate stands
/// in front of a `ConsumerARN`: the refusal names the STREAM it is registered with, by the
/// stored stream name rather than by the ARN the request carried.
#[tokio::test]
async fn a_consumer_arn_is_gated_by_the_stream_that_holds_it() {
    let emulator = Emulator::new();
    let name = "consumer-arn-gate";
    creating_stream(&emulator, name).await;
    tokio::time::sleep(past_activation()).await;
    let stream = stream_arn(REGION, name);

    let registered = emulator
        .ok(
            name,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream, "ConsumerName": "gated-consumer" }),
        )
        .await;
    let consumer = registered
        .get("Consumer")
        .and_then(|consumer| consumer.get("ConsumerARN"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the registration must carry a ConsumerARN, got {registered}"))
        .to_string();

    // While the stream serves, the consumer's own policy round-trips.
    let attached = policy_for(&consumer, "ConsumerReaders", "kinesis:SubscribeToShard");
    let (status, response) = put(&emulator, REGION, &consumer, &attached).await;
    assert_eq!(
        status, 200,
        "the attach on the consumer of an ACTIVE stream must be served, got {status} with body \
         {response}"
    );

    // Its stream's removal is accepted, and from then on every one of the three operations is
    // refused for the consumer too, with the sentence the STREAM's state states. The removal
    // carries `EnforceConsumerDeletion`, since the stream has a consumer registered with it.
    emulator
        .ok(
            name,
            "DeleteStream",
            &json!({ "StreamName": name, "EnforceConsumerDeletion": true }),
        )
        .await;
    for operation in POLICY_OPERATIONS {
        let case = format!("{operation} on a consumer of a deleting stream");
        let (http_status, response) = emulator
            .call(operation, &body_for(operation, &consumer))
            .await;
        assert_refused(
            &case,
            http_status,
            &response,
            IN_USE,
            &not_ready_sentence(name, STREAM_DELETING),
        );
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(&consumer),
            "case {case}: the sentence names the stored stream, not the ARN the request used, \
             got {message}"
        );
    }
}

/// The states that SERVE: `ACTIVE`, and the `UPDATING` a change still in flight leaves the
/// stream in ("You can continue to read and write data to your stream while its status is
/// UPDATING"). All three operations are served in both, and the document round-trips byte for
/// byte through each.
#[tokio::test]
async fn an_active_or_updating_stream_serves_every_resource_policy_operation() {
    let emulator = Emulator::new();
    let name = "serving-states";
    // Two shards, so the shard-count change below has somewhere to go.
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 2 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);

    for round in [STREAM_ACTIVE, STREAM_UPDATING] {
        if round == STREAM_UPDATING {
            // A shard-count change leaves the stream `UPDATING` until the re-tiling it armed
            // falls due.
            emulator
                .ok(
                    name,
                    "UpdateShardCount",
                    &json!({
                        "StreamName": name,
                        "TargetShardCount": 4,
                        "ScalingType": "UNIFORM_SCALING",
                    }),
                )
                .await;
        }

        // The state the case claims to be in is the state the stream reports.
        let summary = emulator
            .ok(round, "DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        let reported = summary
            .get("StreamDescriptionSummary")
            .and_then(|description| description.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported, round,
            "case {round}: expected the stream to be in that state when the calls arrive, got \
             {reported}"
        );

        // Attached, reported byte for byte, and detached — every one served.
        let attached = policy_for(&arn, round, "kinesis:GetRecords");
        let (status, response) = put(&emulator, REGION, &arn, &attached).await;
        assert_eq!(
            status, 200,
            "case {round}: expected the attach to be served, got {status} with body {response}"
        );
        let (status, read) = get(&emulator, REGION, &arn).await;
        assert_eq!(
            reported_policy(round, status, &read),
            attached,
            "case {round}: expected the document the attach carried, got {read}"
        );
        let (status, response) = delete(&emulator, REGION, &arn).await;
        assert_eq!(
            status, 200,
            "case {round}: expected the detach to be served, got {status} with body {response}"
        );
    }
}
