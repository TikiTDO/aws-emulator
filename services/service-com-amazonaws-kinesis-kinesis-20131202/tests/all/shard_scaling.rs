// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's SHARD COUNT, end to end over the wire: `CreateStream` ->
//! `UpdateShardCount` -> `ListShards` / `DescribeStreamSummary` / `GetRecords`.
//!
//! RULE: an accepted shard-count change re-tiles the stream's whole hash key space onto
//! the target number of contiguous equal slices — closing every shard that was open and
//! laying down one new open shard per slice, each naming the shards it was produced from —
//! answers the count the stream had beside the count it is being taken to, and leaves the
//! stream `UPDATING` until the transition back to `ACTIVE` falls due, during which the
//! stream still serves reads and writes. It is served only for a stream in the provisioned
//! capacity mode (`ValidationException` otherwise), only while that stream is `ACTIVE`
//! (`ResourceInUseException` otherwise), and only inside the documented scaling limits:
//! no more than double and no less than half the current open count
//! (`InvalidArgumentException` otherwise), and no more than ten changes per rolling 24-hour
//! period per stream (`LimitExceededException` otherwise).
//!
//! The recorded flows this was derived from scale ONE stream once, double and then halve
//! one stream, and scale an on-demand stream (which fails); each is one row of the tables
//! below. The rows differ along what the rule generalises over: how many shards the stream
//! holds when the call arrives, how many it is taken to (every target inside the documented
//! bounds, including both bounds, the unchanged count and a target that is no multiple of
//! 25%), the capacity mode of the stream, the identifier it is addressed by, the status it
//! is in when the call arrives, and how many times it has already been scaled.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    scale_down_below_half_message, scale_up_over_double_message, stream_not_found_message,
    stream_on_demand_message, MAX_RESHARDS_PER_WINDOW, RESHARD_DELAY, STREAM_ACTIVE,
    stream_identifier_mismatch_message, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
    STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED, STREAM_UPDATING,
};

use crate::stream_tags::{
    error_code, error_message, past_activation, stream_arn, Emulator, REGION,
};

/// The only `ScalingType` the model declares
/// (`smithy_query(query="shape",shape="ScalingType")` -> values `UNIFORM_SCALING`).
const UNIFORM_SCALING: &str = "UNIFORM_SCALING";

/// How long a case waits before treating a scaled stream as ready: past the instant the
/// change armed the `UPDATING` -> `ACTIVE` transition for.
pub(crate) fn past_reshard() -> Duration {
    RESHARD_DELAY + Duration::from_millis(400)
}

/// An `UpdateShardCount` body addressing `name` by its `StreamName`.
fn scale_to(name: &str, target: u32) -> Value {
    json!({
        "StreamName": name,
        "TargetShardCount": target,
        "ScalingType": UNIFORM_SCALING,
    })
}

/// The shard map `ListShards` reports for `name`, in map order. Every stream a case here
/// holds fits in one page, so the listing never has to be followed.
pub(crate) async fn shard_map(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let listed = emulator
        .ok(case, "ListShards", &json!({ "StreamName": name }))
        .await;
    listed
        .get("Shards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: ListShards must carry Shards, got {listed}"))
}

/// The `StreamStatus` and `OpenShardCount` `DescribeStreamSummary` reports for `name`.
pub(crate) async fn summary(emulator: &Emulator, case: &str, name: &str) -> (String, u64) {
    let described = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
    let summary = &described["StreamDescriptionSummary"];
    let status = summary
        .get("StreamStatus")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the summary must carry StreamStatus, got {described}"))
        .to_string();
    let open = summary
        .get("OpenShardCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must carry OpenShardCount, got {described}")
        });
    (status, open)
}

/// A shard's id.
pub(crate) fn shard_id(case: &str, shard: &Value) -> String {
    shard
        .get("ShardId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a shard must carry a ShardId, got {shard}"))
        .to_string()
}

/// A shard's `HashKeyRange`, as the pair of integers it names.
pub(crate) fn hash_key_range(case: &str, shard: &Value) -> (u128, u128) {
    let bound = |member: &str| -> u128 {
        shard
            .get("HashKeyRange")
            .and_then(|range| range.get(member))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: a shard must carry {member}, got {shard}"))
            .parse::<u128>()
            .unwrap_or_else(|error| {
                panic!("case {case}: {member} must be a 128-bit integer, got {shard}: {error}")
            })
    };
    (bound("StartingHashKey"), bound("EndingHashKey"))
}

/// Whether the shard is still open: "shards that are in the OPEN state have an ending
/// sequence number of null" (`SequenceNumberRange$EndingSequenceNumber`).
pub(crate) fn is_open(case: &str, shard: &Value) -> bool {
    let range = shard
        .get("SequenceNumberRange")
        .unwrap_or_else(|| panic!("case {case}: a shard must carry a SequenceNumberRange, got {shard}"));
    range
        .get("EndingSequenceNumber")
        .is_none_or(Value::is_null)
}

/// A shard's lineage, as `Shard$ParentShardId` and `Shard$AdjacentParentShardId` name it.
pub(crate) fn parents(shard: &Value) -> Vec<String> {
    ["ParentShardId", "AdjacentParentShardId"]
        .into_iter()
        .filter_map(|member| shard.get(member).and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Assert that `open` covers the WHOLE 128-bit hash key space in `expected` contiguous
/// slices of equal size: "uniform scaling creates shards of equal size" (`ScalingType`),
/// and a shard's range "is a set of ordered contiguous positive integers"
/// (`Shard$HashKeyRange`).
fn assert_uniform_tiling(case: &str, open: &[Value], expected: usize) {
    assert_eq!(
        open.len(),
        expected,
        "case {case}: expected {expected} open shards, got {}: {open:?}",
        open.len()
    );
    // The width is kept as `end - start` (one less than the slice's size) so a single
    // shard covering the whole space is representable.
    let mut widths: Vec<u128> = Vec::new();
    let mut next = 0u128;
    for shard in open {
        let (start, end) = hash_key_range(case, shard);
        assert_eq!(
            start, next,
            "case {case}: expected shard {} to start at {next}, got {start}",
            shard_id(case, shard)
        );
        assert!(
            end >= start,
            "case {case}: shard {} must end no lower than it starts, got {start}..{end}",
            shard_id(case, shard)
        );
        widths.push(end - start);
        next = end.wrapping_add(1);
    }
    assert_eq!(
        next, 0,
        "case {case}: the slices must cover the key space up to its maximum, the last one \
         ends at {}",
        next.wrapping_sub(1)
    );
    let smallest = widths.iter().min().copied().unwrap_or_default();
    let largest = widths.iter().max().copied().unwrap_or_default();
    assert!(
        largest - smallest <= 1,
        "case {case}: the slices must be of equal size, got widths between {smallest} and \
         {largest}"
    );
}

/// Every target inside the documented bounds, for streams of one to five shards: "you
/// cannot ... scale up to more than double your current shard count for a stream[, or]
/// scale down below half your current shard count for a stream" (`UpdateShardCount`).
///
/// The sweep is exhaustive over that bounded domain rather than sampled, so it spans both
/// published bounds, the unchanged count and every target between them — including targets
/// that are no multiple of 25% of the current count.
fn scaling_cases() -> Vec<(u32, u32)> {
    let mut cases = Vec::new();
    for current in 1..=5u32 {
        for target in current.div_ceil(2)..=current * 2 {
            cases.push((current, target));
        }
    }
    cases
}

/// An accepted change re-tiles the key space onto the target count, reports the count the
/// stream had, and hands the stream back `ACTIVE` once the transition it armed falls due.
#[tokio::test]
async fn update_shard_count_retiles_the_key_space() {
    let emulator = Emulator::new();
    let cases = scaling_cases();

    // Every stream is created first and waited out ONCE: the rule is about the call, not
    // about how long a create takes.
    for (current, target) in &cases {
        let name = format!("scale-{current}-to-{target}");
        emulator
            .ok(
                &name,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "ShardCount": current,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
                }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (current, target) in &cases {
        let case = format!("scale-{current}-to-{target}");
        let before = shard_map(&emulator, &case, &case).await;
        let answer = emulator
            .ok(&case, "UpdateShardCount", &scale_to(&case, *target))
            .await;

        assert_eq!(
            answer.get("StreamName").and_then(Value::as_str),
            Some(case.as_str()),
            "case {case}: expected the answer to name the stream, got {answer}"
        );
        assert_eq!(
            answer.get("StreamARN").and_then(Value::as_str),
            Some(stream_arn(REGION, &case).as_str()),
            "case {case}: expected the answer to carry the stream's ARN, got {answer}"
        );
        assert_eq!(
            answer.get("CurrentShardCount").and_then(Value::as_u64),
            Some(u64::from(*current)),
            "case {case}: expected CurrentShardCount {current}, got {answer}"
        );
        assert_eq!(
            answer.get("TargetShardCount").and_then(Value::as_u64),
            Some(u64::from(*target)),
            "case {case}: expected TargetShardCount {target}, got {answer}"
        );

        // "Upon receiving the request, Kinesis Data Streams returns immediately and sets
        // the status of the stream to UPDATING", and the stream still serves its shard map
        // while it is in that status.
        let (status, _) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             call, got {status}"
        );
        let open: Vec<Value> = shard_map(&emulator, &case, &case)
            .await
            .into_iter()
            .filter(|shard| is_open(&case, shard))
            .collect();
        assert_uniform_tiling(&case, &open, usize::try_from(*target).unwrap_or_default());

        // A target the map already tiles the key space by is reached by no split and no
        // merge, so the shards a reader already holds positions in stay open.
        let closed: Vec<String> = shard_map(&emulator, &case, &case)
            .await
            .into_iter()
            .filter(|shard| !is_open(&case, shard))
            .map(|shard| shard_id(&case, &shard))
            .collect();
        if current == target {
            assert!(
                closed.is_empty(),
                "case {case}: a change to the count the stream already has must split and \
                 merge nothing, got the closed shards {closed:?}"
            );
            let kept: Vec<String> = open.iter().map(|shard| shard_id(&case, shard)).collect();
            let was: Vec<String> = before
                .iter()
                .map(|shard| shard_id(&case, shard))
                .collect();
            assert_eq!(
                kept, was,
                "case {case}: expected the shard map to be left as it was, got {kept:?}"
            );
        } else {
            let was: Vec<String> = before
                .iter()
                .map(|shard| shard_id(&case, shard))
                .collect();
            assert_eq!(
                closed, was,
                "case {case}: expected every shard that was open to be closed, got {closed:?}"
            );
            // Each new shard is produced from the shards that covered its own hash key
            // range: the one holding its lowest key, and the one holding its highest when
            // that is another shard (`ParentShardId` / `AdjacentParentShardId`).
            for shard in &open {
                let (start, end) = hash_key_range(&case, shard);
                let covering = |key: u128| -> String {
                    before
                        .iter()
                        .find(|parent| {
                            let (from, to) = hash_key_range(&case, parent);
                            (from..=to).contains(&key)
                        })
                        .map(|parent| shard_id(&case, parent))
                        .unwrap_or_else(|| {
                            panic!("case {case}: no shard of the map held the key {key}")
                        })
                };
                let lowest = covering(start);
                let highest = covering(end);
                let expected: Vec<String> = if lowest == highest {
                    vec![lowest]
                } else {
                    vec![lowest, highest]
                };
                assert_eq!(
                    parents(shard),
                    expected,
                    "case {case}: expected shard {} to name the shards it was produced \
                     from",
                    shard_id(&case, shard)
                );
            }
        }
    }

    // "After the update is complete, Kinesis Data Streams sets the status of the stream
    // back to ACTIVE", with the target count open.
    tokio::time::sleep(past_reshard()).await;
    for (current, target) in &cases {
        let case = format!("scale-{current}-to-{target}");
        let (status, open) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to be {STREAM_ACTIVE} once the change fell \
             due, got {status}"
        );
        assert_eq!(
            open,
            u64::from(*target),
            "case {case}: expected {target} open shards once the change fell due, got {open}"
        );
    }
}

/// What the stream a refusal case addresses looks like when the call arrives.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// A provisioned stream of this many shards, `ACTIVE`.
    Active(u32),
    /// An on-demand stream, whose shard count the service manages itself.
    OnDemand,
    /// A stream whose create has not fallen due yet.
    Creating,
    /// A stream whose previous shard-count change has not fallen due yet.
    Updating,
    /// A stream a delete has been accepted for.
    Deleting,
    /// No stream at all.
    Absent,
}

/// One refused call: the stream it addresses, the request it makes and the modelled
/// refusal it must answer.
struct Refusal {
    case: &'static str,
    subject: Subject,
    /// The request body, built from the name the subject was set up under.
    body: fn(&str) -> Value,
    code: &'static str,
    /// What the message must name, so a caller can tell which rule refused the call.
    says: fn(&str) -> String,
}

/// A refused change leaves the stored shard map exactly as it was.
#[tokio::test]
async fn update_shard_count_refuses_what_the_limits_forbid() {
    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "on-demand",
            subject: Subject::OnDemand,
            body: |name| scale_to(name, 4),
            code: "ValidationException",
            says: |name| stream_on_demand_message(name),
        },
        Refusal {
            case: "more-than-double",
            subject: Subject::Active(2),
            body: |name| scale_to(name, 5),
            code: "InvalidArgumentException",
            says: |_| scale_up_over_double_message(2, 5),
        },
        Refusal {
            case: "below-half",
            subject: Subject::Active(4),
            body: |name| scale_to(name, 1),
            code: "InvalidArgumentException",
            says: |_| scale_down_below_half_message(4, 1),
        },
        Refusal {
            // A target no bigger than double the stream's open count, and still more shards
            // than the account's published quota leaves this region: the ONE shard ceiling
            // a scaling action is refused past (`shard_quota.rs` covers it in full).
            case: "past-the-shard-quota",
            subject: Subject::Active(400),
            body: |name| scale_to(name, 800),
            code: "LimitExceededException",
            says: |_| {
                format!(
                    "This request would exceed the shard limit for the account {} in {REGION}.",
                    engine::EMULATOR_ACCOUNT_ID
                )
            },
        },
        Refusal {
            case: "still-creating",
            subject: Subject::Creating,
            body: |name| scale_to(name, 2),
            code: "ResourceInUseException",
            says: |_| "CREATING".to_string(),
        },
        Refusal {
            case: "already-updating",
            subject: Subject::Updating,
            body: |name| scale_to(name, 2),
            code: "ResourceInUseException",
            says: |_| STREAM_UPDATING.to_string(),
        },
        Refusal {
            case: "being-deleted",
            subject: Subject::Deleting,
            body: |name| scale_to(name, 4),
            code: "ResourceInUseException",
            says: |_| "DELETING".to_string(),
        },
        Refusal {
            case: "no-such-stream",
            subject: Subject::Absent,
            body: |name| scale_to(name, 2),
            code: "ResourceNotFoundException",
            says: |name| stream_not_found_message(name),
        },
        Refusal {
            case: "no-identifier",
            subject: Subject::Absent,
            body: |_| json!({ "TargetShardCount": 2, "ScalingType": UNIFORM_SCALING }),
            code: "InvalidArgumentException",
            says: |_| STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        Refusal {
            case: "two-streams",
            subject: Subject::Active(2),
            body: |name| {
                json!({
                    "StreamARN": stream_arn(REGION, name),
                    "StreamName": format!("{name}-other"),
                    "TargetShardCount": 4,
                    "ScalingType": UNIFORM_SCALING,
                })
            },
            code: "InvalidArgumentException",
            says: |name| {
                stream_identifier_mismatch_message(
                    &format!("{name}-other"),
                    &stream_arn(REGION, name),
                )
            },
        },
    ];

    let emulator = Emulator::new();
    // Every stream whose status the case needs settled is created first and waited out
    // ONCE; the ones whose status is still in motion are armed inside the loop below, so
    // the call arrives while the transition is still due.
    for refusal in &cases {
        let shard_count = match refusal.subject {
            Subject::Active(shards) => Some(shards),
            Subject::Updating | Subject::Deleting => Some(2),
            Subject::OnDemand | Subject::Creating | Subject::Absent => None,
        };
        let mut body = json!({ "StreamName": refusal.case });
        match (refusal.subject, shard_count) {
            (Subject::OnDemand, _) => {
                body["StreamModeDetails"] = json!({ "StreamMode": STREAM_MODE_ON_DEMAND });
            }
            (_, Some(shards)) => {
                body["ShardCount"] = json!(shards);
                body["StreamModeDetails"] = json!({ "StreamMode": STREAM_MODE_PROVISIONED });
            }
            (_, None) => continue,
        }
        emulator.ok(refusal.case, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for refusal in &cases {
        let case = refusal.case;
        match refusal.subject {
            Subject::Creating => {
                emulator
                    .ok(
                        case,
                        "CreateStream",
                        &json!({ "StreamName": case, "ShardCount": 1 }),
                    )
                    .await;
            }
            Subject::Updating => {
                emulator
                    .ok(case, "UpdateShardCount", &scale_to(case, 4))
                    .await;
            }
            Subject::Deleting => {
                emulator
                    .ok(case, "DeleteStream", &json!({ "StreamName": case }))
                    .await;
            }
            Subject::Active(_) | Subject::OnDemand | Subject::Absent => {}
        }

        let before = match refusal.subject {
            // A stream nothing holds, or one whose map no read serves while its create or
            // its removal is still due, has no map to compare against.
            Subject::Absent | Subject::Creating | Subject::Deleting => None,
            _ => Some(shard_map(&emulator, case, case).await),
        };

        let (status, response) = emulator
            .call("UpdateShardCount", &(refusal.body)(case))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            refusal.code,
            "case {case}: expected {}, got {response}",
            refusal.code
        );
        let says = (refusal.says)(case);
        let message = error_message(&response);
        assert!(
            message.contains(&says),
            "case {case}: expected the message to name {says:?}, got {message:?}"
        );
        if let Some(before) = before {
            assert_eq!(
                shard_map(&emulator, case, case).await.len(),
                before.len(),
                "case {case}: a refused change must leave the shard map as it was, which \
                 held {} shards",
                before.len()
            );
        }
    }
}

/// The documented rate ceiling: "you cannot ... scale more than ten times per rolling
/// 24-hour period per stream".
#[tokio::test]
async fn update_shard_count_refuses_past_the_rolling_window() {
    let emulator = Emulator::new();
    let name = "rolling-window";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // Alternating between one and two shards stays inside the half-to-double bounds, so
    // the ONLY limit these calls can reach is the rolling-window one.
    for change in 1..=MAX_RESHARDS_PER_WINDOW {
        let target = if change % 2 == 1 { 2 } else { 1 };
        let case = format!("change-{change}");
        let answer = emulator
            .ok(&case, "UpdateShardCount", &scale_to(name, target))
            .await;
        assert_eq!(
            answer.get("TargetShardCount").and_then(Value::as_u64),
            Some(u64::from(target)),
            "case {case}: expected the change to be accepted, got {answer}"
        );
        tokio::time::sleep(past_reshard()).await;
    }

    let (status, response) = emulator
        .call("UpdateShardCount", &scale_to(name, 2))
        .await;
    assert_eq!(
        status, 400,
        "case past-the-window: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case past-the-window: expected the rate ceiling to refuse the change, got {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(&MAX_RESHARDS_PER_WINDOW.to_string()),
        "case past-the-window: expected the message to name the ceiling, got {message:?}"
    );
}

/// Records written before a change stay readable in the shards it closed, and the reader
/// that drains one is told the children to read on from; writes after it land in the new
/// shards.
#[tokio::test]
async fn records_survive_a_shard_count_change() {
    let emulator = Emulator::new();
    let case = "reshard-records";
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": case, "ShardCount": 2 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // One record per partition key, so the writes spread over the shard map rather than
    // landing in one shard.
    let keys = ["alpha", "beta", "gamma", "delta"];
    for key in keys {
        emulator
            .ok(
                case,
                "PutRecord",
                &json!({
                    "StreamName": case,
                    "PartitionKey": key,
                    "Data": "cGF5bG9hZA==",
                }),
            )
            .await;
    }
    let before: Vec<String> = shard_map(&emulator, case, case)
        .await
        .iter()
        .map(|shard| shard_id(case, shard))
        .collect();

    emulator
        .ok(case, "UpdateShardCount", &scale_to(case, 4))
        .await;
    tokio::time::sleep(past_reshard()).await;

    // Every record written before the change is still readable, in the shard it was
    // written to.
    let mut read = 0usize;
    for parent in &before {
        let iterator = emulator
            .ok(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": case,
                    "ShardId": parent,
                    "ShardIteratorType": "TRIM_HORIZON",
                }),
            )
            .await;
        let iterator = iterator
            .get("ShardIterator")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected an iterator for {parent}"))
            .to_string();
        let page = emulator
            .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
            .await;
        read += page
            .get("Records")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        // "If the shard has been closed, the shard iterator can't return more data and
        // GetRecords returns null in NextShardIterator", and the children of the closed
        // shard are what the reader reads on from.
        assert!(
            page.get("NextShardIterator").is_none_or(Value::is_null),
            "case {case}: a drained closed shard hands out no further iterator, got {page}"
        );
        let children = page
            .get("ChildShards")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected the children of {parent}, got {page}")
            });
        assert!(
            !children.is_empty()
                && children.iter().all(|child| child
                    .get("ParentShards")
                    .and_then(Value::as_array)
                    .is_some_and(|names| names.iter().any(|name| name.as_str()
                        == Some(parent.as_str())))),
            "case {case}: expected every child to name {parent} as a parent, got {children:?}"
        );
    }
    assert_eq!(
        read,
        keys.len(),
        "case {case}: expected every record written before the change to stay readable, \
         read {read}"
    );

    // A write after the change lands in one of the shards it laid down.
    let open: Vec<String> = shard_map(&emulator, case, case)
        .await
        .iter()
        .filter(|shard| is_open(case, shard))
        .map(|shard| shard_id(case, shard))
        .collect();
    let written = emulator
        .ok(
            case,
            "PutRecord",
            &json!({
                "StreamName": case,
                "PartitionKey": "epsilon",
                "Data": "cGF5bG9hZA==",
            }),
        )
        .await;
    let landed = written
        .get("ShardId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: PutRecord must name the shard, got {written}"))
        .to_string();
    assert!(
        open.contains(&landed),
        "case {case}: expected the write to land in one of {open:?}, got {landed}"
    );
}
