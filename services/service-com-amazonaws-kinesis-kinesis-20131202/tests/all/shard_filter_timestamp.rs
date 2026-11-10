// SPDX-License-Identifier: CC0-1.0

//! The `Timestamp` a `ListShards` `ShardFilter` needs, end to end over the wire.
//!
//! RULE: `ShardFilter$Timestamp` "can only be used if FROM_TIMESTAMP or AT_TIMESTAMP shard
//! types are specified" (`smithy_query(query="shape",shape="ShardFilter")`), and it is what
//! those two types SELECT by — so a filter of either type that carries no `Timestamp` names
//! no selection and is refused `InvalidArgumentException` (HTTP 400) with ONE fixed
//! sentence. The sentence states the RULE and so has no variable part at all: it names both
//! types the member belongs to, whichever of them the request sent, and it names neither
//! the stream, nor the account, nor the value that was missing. The same filter WITH a
//! `Timestamp` is served, and a filter whose type selects by something else is unaffected
//! by the rule.
//!
//! The recorded flow this was derived from sends ONE `ListShards(ShardFilter={"Type":
//! "AT_TIMESTAMP"})` against one multi-shard stream, which is one row of the tables below.
//! The rows differ along what the rule generalises over: which of the two timestamp types
//! the filter names, whether the filter carries a `Timestamp` at all, the identifier the
//! request addresses the stream by, the shard count and capacity mode of that stream, and
//! the filter types the rule does NOT reach.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.ListShards` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// The one sentence a filter that needs a `Timestamp` and carries none is refused with.
/// Fixed text in full: it names BOTH types the member belongs to, so it is the same
/// sentence for either of them, and it carries nothing taken from the request or the store.
const TIMESTAMP_REQUIRED_MESSAGE: &str =
    "Timestamp must be provided when the ShardFilter Type is AT_TIMESTAMP or FROM_TIMESTAMP.";

/// The two `ShardFilterType` values that select by `Timestamp`, as the member's own rule
/// names them (`smithy_query(query="shape",shape="ShardFilter")` -> `Timestamp`).
const TIMESTAMP_FILTER_TYPES: [&str; 2] = ["AT_TIMESTAMP", "FROM_TIMESTAMP"];

/// The `ShardFilterType` values that select by something other than a `Timestamp`, from the
/// same shape (`smithy_query(query="shape",shape="ShardFilterType")`): the rule does not
/// reach them, so a request carrying one and no `Timestamp` is served.
const TIMESTAMPLESS_FILTER_TYPES: [&str; 3] = ["AT_TRIM_HORIZON", "FROM_TRIM_HORIZON", "AT_LATEST"];

/// How a case addresses the stream: "you must use either the StreamARN or the StreamName
/// parameter, or both" (`ListShards`), so all three are rows of the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Name,
    Arn,
    Both,
}

/// A `ListShards` body addressing `name` the way `by` says, carrying `filter`.
fn listing_body(name: &str, by: By, filter: Value) -> Value {
    let mut body = json!({ "ShardFilter": filter });
    if by != By::Arn {
        body["StreamName"] = json!(name);
    }
    if by != By::Name {
        body["StreamARN"] = json!(stream_arn(REGION, name));
    }
    body
}

/// The instant `at` as the awsJson1_1 wire carries an `@timestamp`: epoch seconds.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Create a stream and wait out the transition the create armed.
async fn active_stream(emulator: &Emulator, case: &str, name: &str, shard_count: Option<i64>) {
    let body = match shard_count {
        Some(count) => json!({
            "StreamName": name,
            "ShardCount": count,
            "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
        }),
        None => json!({
            "StreamName": name,
            "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
        }),
    };
    emulator.ok(case, "CreateStream", &body).await;
    tokio::time::sleep(past_activation()).await;
}

/// The shard ids a listing reported.
fn shard_ids(case: &str, answer: &Value) -> Vec<String> {
    answer
        .get("Shards")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: ListShards must report Shards, got {answer}"))
        .iter()
        .map(|shard| {
            shard
                .get("ShardId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a shard must carry a ShardId, got {shard}"))
                .to_string()
        })
        .collect()
}

/// One stream a row is judged against, so the refusal is shown not to depend on the stream.
#[derive(Debug, Clone, Copy)]
struct Stream {
    name: &'static str,
    /// `None` is the on-demand mode, whose shard count the service picks.
    shard_count: Option<i64>,
}

const STREAMS: [Stream; 3] = [
    Stream {
        name: "filter-one-shard",
        shard_count: Some(1),
    },
    Stream {
        name: "filter-many-shards",
        shard_count: Some(4),
    },
    Stream {
        name: "filter-on-demand",
        shard_count: None,
    },
];

// 1. A timestamp filter carrying no Timestamp is refused with the one fixed sentence
#[tokio::test]
async fn a_timestamp_filter_with_no_timestamp_is_refused_with_the_rule_it_states() {
    let emulator = Emulator::new();
    for stream in STREAMS {
        active_stream(&emulator, stream.name, stream.name, stream.shard_count).await;
    }

    for stream in STREAMS {
        for filter_type in TIMESTAMP_FILTER_TYPES {
            for by in [By::Name, By::Arn, By::Both] {
                let case = format!("{} / {filter_type} / {by:?}", stream.name);
                let body = listing_body(stream.name, by, json!({ "Type": filter_type }));
                let (status, response) = emulator.call("ListShards", &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected ListShards to refuse with 400, got {status} \
                     with body {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "InvalidArgumentException",
                    "case {case:?}: expected InvalidArgumentException, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    TIMESTAMP_REQUIRED_MESSAGE,
                    "case {case:?}: expected message {TIMESTAMP_REQUIRED_MESSAGE:?}, got {:?}",
                    error_message(&response)
                );
            }
        }
    }
}

// 2. The same filter WITH a Timestamp is served, so the refusal is about the missing member
#[tokio::test]
async fn the_same_filter_with_a_timestamp_resolves_the_streams_shards() {
    let emulator = Emulator::new();
    for stream in STREAMS {
        active_stream(&emulator, stream.name, stream.name, stream.shard_count).await;
    }

    let now = SystemTime::now();
    // Instants on either side of the streams' creation, so the rows differ in what the
    // filter selects as well as in carrying a timestamp at all. `before_creation` is whether
    // the instant falls before the streams existed, which is what decides the selection:
    // `AT_TIMESTAMP` "includes all shards whose start timestamp is less than or equal to the
    // given timestamp and end timestamp is greater than or equal to the given timestamp or
    // still open", so no shard of a stream created later spans it, while `FROM_TIMESTAMP` is
    // "corrected to TRIM_HORIZON of the data stream if FROM_TIMESTAMP is less than the
    // TRIM_HORIZON value" and so still reports every open shard.
    let instants = [
        ("the epoch", epoch_seconds(UNIX_EPOCH), true),
        (
            "an hour ago",
            epoch_seconds(now - Duration::from_secs(3_600)),
            true,
        ),
        ("now", epoch_seconds(now), false),
    ];

    for stream in STREAMS {
        let all = shard_ids(
            stream.name,
            &emulator
                .ok(
                    stream.name,
                    "ListShards",
                    &json!({ "StreamName": stream.name }),
                )
                .await,
        );
        for filter_type in TIMESTAMP_FILTER_TYPES {
            for (label, timestamp, before_creation) in instants {
                let case = format!("{} / {filter_type} / {label}", stream.name);
                let body = listing_body(
                    stream.name,
                    By::Name,
                    json!({ "Type": filter_type, "Timestamp": timestamp }),
                );
                let (status, response) = emulator.call("ListShards", &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a filter carrying a Timestamp must be served, got \
                     {status} with body {response}"
                );
                let expected: Vec<String> = if filter_type == "AT_TIMESTAMP" && before_creation {
                    Vec::new()
                } else {
                    all.clone()
                };
                let reported = shard_ids(&case, &response);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the shards {expected:?}, got {reported:?}"
                );
            }
        }
    }
}

// 3. A filter type the rule does not reach is served with no Timestamp at all
#[tokio::test]
async fn a_filter_that_selects_by_something_else_needs_no_timestamp() {
    let emulator = Emulator::new();
    let stream = "filter-timestampless";
    active_stream(&emulator, stream, stream, Some(2)).await;
    let all = shard_ids(
        stream,
        &emulator
            .ok(stream, "ListShards", &json!({ "StreamName": stream }))
            .await,
    );

    for filter_type in TIMESTAMPLESS_FILTER_TYPES {
        let body = listing_body(stream, By::Name, json!({ "Type": filter_type }));
        let (status, response) = emulator.call("ListShards", &body).await;
        assert_eq!(
            status, 200,
            "case {filter_type:?}: a filter that selects by something other than a \
             Timestamp must be served, got {status} with body {response}"
        );
        let reported = shard_ids(filter_type, &response);
        assert!(
            reported.iter().all(|shard| all.contains(shard)),
            "case {filter_type:?}: every selected shard must be one of the stream's \
             {all:?}, got {reported:?}"
        );
    }

    // `AFTER_SHARD_ID` selects by a `ShardId` rather than a `Timestamp`, so the rule does
    // not reach it either: the refusal it carries for a missing member is its own.
    let (status, response) = emulator
        .call(
            "ListShards",
            &listing_body(stream, By::Name, json!({ "Type": "AFTER_SHARD_ID" })),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"AFTER_SHARD_ID\": a filter naming no ShardId must be refused, got {status} \
         with body {response}"
    );
    assert_ne!(
        error_message(&response),
        TIMESTAMP_REQUIRED_MESSAGE,
        "case \"AFTER_SHARD_ID\": the Timestamp rule must not be stated for a filter type \
         it does not reach, got {:?}",
        error_message(&response)
    );
}

// 4. A refused listing changes nothing a later caller sees
#[tokio::test]
async fn a_refused_listing_leaves_the_shard_map_as_it_was() {
    let emulator = Emulator::new();
    let stream = "filter-read-only";
    active_stream(&emulator, stream, stream, Some(3)).await;
    let before = shard_ids(
        stream,
        &emulator
            .ok(stream, "ListShards", &json!({ "StreamName": stream }))
            .await,
    );

    for filter_type in TIMESTAMP_FILTER_TYPES {
        let (status, _) = emulator
            .call(
                "ListShards",
                &listing_body(stream, By::Name, json!({ "Type": filter_type })),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {filter_type:?}: the refusal is the premise of this case, got {status}"
        );
    }

    let after = shard_ids(
        stream,
        &emulator
            .ok(stream, "ListShards", &json!({ "StreamName": stream }))
            .await,
    );
    assert_eq!(
        after, before,
        "case \"after two refusals\": expected the shard map {before:?}, got {after:?}"
    );
}
