// SPDX-License-Identifier: CC0-1.0

//! A stream's RECORD DISTRIBUTION STRATEGY, end to end over the wire: `CreateStream` ->
//! `UpdateStreamRecordDistributionStrategy` -> `DescribeStreamSummary` / `PutRecord`.
//!
//! RULE: the record distribution strategy is a stream-level setting of an ON-DEMAND stream
//! only — "This operation is only supported for data streams that use the on-demand capacity
//! mode. Provisioned capacity mode streams do not support the record distribution strategy
//! setting" (`UpdateStreamRecordDistributionStrategy`), and "This field is only present for
//! streams that use the on-demand capacity mode"
//! (`StreamDescriptionSummary$RecordDistributionStrategy`). So:
//!
//! * `UpdateStreamRecordDistributionStrategy` STORES the strategy its request names on the
//!   on-demand stream its `StreamARN` addresses, replacing whatever that stream carried, and
//!   answers 200 with the modelled empty `Unit` body; the change "takes effect immediately
//!   without downtime", so the stream stays `ACTIVE`;
//! * a read of an on-demand stream reports the STORED strategy, and reports
//!   `USER_PARTITION_KEY` — "This is the default strategy" — for one nothing has set a
//!   strategy on; a read of a provisioned stream reports the member not at all;
//! * the request is refused for a provisioned stream, which carries no such setting, and the
//!   member stays absent;
//! * the stored strategy is what places every LATER write: under `USER_PARTITION_KEY` the
//!   producer's partition key decides the shard, so every record of one key lands on the one
//!   open shard whose hash key range covers that key's hash; under `AUTO` the service places
//!   the record itself and "ignores any partition key and ExplicitHashKey that a producer
//!   provides", so the records of one key spread over the open shards. "New records that
//!   arrive after the change are distributed according to the new strategy. Records already
//!   in the stream keep their original shard assignments and are not redistributed."
//!
//! The recorded flows this was derived from set `AUTO` on ONE on-demand stream, call the
//! operation once against an ARN nothing created, and call it once against one provisioned
//! stream — each one row of the tables below. The rows differ along what the rule
//! generalises over: the strategy the create named (none, each value the enum admits), the
//! strategy the update names and how many times it switches, the capacity mode and shard
//! count of the stream, the lifecycle state the request lands in, the identifier's region
//! and account, and the form and length of the partition key a write carries.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness and the ARN builder are [`crate::stream_tags`]', reused rather than
//! copied.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    partition_key_hash, stream_not_active_message, stream_not_found_message,
    stream_provisioned_message, RegionalName, ON_DEMAND_INITIAL_SHARDS, RECORD_DISTRIBUTION_AUTO,
    RECORD_DISTRIBUTION_USER_PARTITION_KEY, SHARD_ITERATOR_TYPE_TRIM_HORIZON, STREAM_ACTIVE,
    STREAM_CREATING, STREAM_DELETING, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::stream_tags::{error_code, error_message, stream_arn, Emulator, REGION};

/// Every value `RecordDistributionStrategy` admits
/// (`smithy_query(query="shape",shape="com.amazonaws.kinesis#RecordDistributionStrategy")` ->
/// `AUTO | USER_PARTITION_KEY`).
const EVERY_STRATEGY: [&str; 2] = [RECORD_DISTRIBUTION_AUTO, RECORD_DISTRIBUTION_USER_PARTITION_KEY];

/// The `@length` maximum of `PartitionKey`
/// (`smithy_query(query="operation",operation="PutRecord")` -> `PartitionKey` len 1..256).
const PARTITION_KEY_MAX_LENGTH: usize = 256;

/// How many writes of one partition key a placement case makes: enough that a strategy
/// spreading them over the open shards cannot put them all on one by chance.
const WRITES_PER_KEY: usize = 8;

/// The longest a case waits for a transition the clock already owes to fall due. Generous on
/// purpose: what a case waits for is the CONDITION, re-read until it holds.
const READY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a case waits between two reads of a condition that does not hold yet.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A century: long enough that a transition armed this far out never falls due inside a test
/// run, which is what pins a stream in a non-terminal lifecycle state without waiting.
const A_LONG_TIME: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// The `StreamDescriptionSummary` `DescribeStreamSummary` reports for the stream named
/// `name`.
async fn summary_of(emulator: &Emulator, case: &str, name: &str) -> Value {
    emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await
        .get("StreamDescriptionSummary")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `RecordDistributionStrategy` a read of the stream named `name` reports, `None` when
/// the summary carries no such member at all.
async fn reported_strategy(emulator: &Emulator, case: &str, name: &str) -> Option<String> {
    summary_of(emulator, case, name)
        .await
        .get("RecordDistributionStrategy")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Re-read the stream named `name` until it reports `ACTIVE`, which is what a transition the
/// clock owes falling due looks like from outside. No fixed wait: the condition is what is
/// waited for, under a deadline a loaded host still meets.
async fn await_active(emulator: &Emulator, case: &str, name: &str) {
    let started = Instant::now();
    loop {
        let status = summary_of(emulator, case, name)
            .await
            .get("StreamStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == STREAM_ACTIVE {
            return;
        }
        assert!(
            started.elapsed() < READY_DEADLINE,
            "case {case}: the stream {name} never reached {STREAM_ACTIVE}, last reported \
             {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create an on-demand stream named `name`, naming `created_with` as its strategy when a row
/// names one, and wait out the transition the create armed.
async fn on_demand_stream(
    emulator: &Emulator,
    case: &str,
    name: &str,
    created_with: Option<&str>,
) {
    // An on-demand stream's shard count is the service's to manage, so the create names
    // none.
    let mut body = json!({
        "StreamName": name,
        "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
    });
    if let Some(strategy) = created_with {
        body["RecordDistributionStrategy"] = json!(strategy);
    }
    emulator.ok(case, "CreateStream", &body).await;
    await_active(emulator, case, name).await;
}

/// Set the strategy of the stream `arn` addresses, asserting the modelled 200 with the empty
/// `Unit` body.
async fn set_strategy(emulator: &Emulator, case: &str, arn: &str, strategy: &str) {
    let (status, answered) = emulator
        .call(
            "UpdateStreamRecordDistributionStrategy",
            &json!({ "StreamARN": arn, "RecordDistributionStrategy": strategy }),
        )
        .await;
    assert_eq!(
        (status, &answered),
        (200, &json!({})),
        "case {case}: setting {strategy} on an ACTIVE on-demand stream answers 200 and the \
         modelled empty body, got {status} with body {answered}"
    );
}

/// One row of the round-trip table: the strategy the create named, and the strategies the
/// updates name in turn.
struct Row {
    label: &'static str,
    /// `CreateStreamInput$RecordDistributionStrategy`, `None` on a row whose create names
    /// none.
    created_with: Option<&'static str>,
    /// The strategies the updates name, in order.
    switches: &'static [&'static str],
}

/// The table the store-and-report rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: a stream whose create named no strategy, switched to AUTO.
        Row {
            label: "record-distribution-default-then-auto",
            created_with: None,
            switches: &[RECORD_DISTRIBUTION_AUTO],
        },
        // Naming the value the stream already carries by default changes nothing.
        Row {
            label: "record-distribution-default-then-the-default",
            created_with: None,
            switches: &[RECORD_DISTRIBUTION_USER_PARTITION_KEY],
        },
        // Off a strategy the create named, in each direction.
        Row {
            label: "record-distribution-auto-at-create-back-to-the-key",
            created_with: Some(RECORD_DISTRIBUTION_AUTO),
            switches: &[RECORD_DISTRIBUTION_USER_PARTITION_KEY],
        },
        Row {
            label: "record-distribution-key-at-create-to-auto",
            created_with: Some(RECORD_DISTRIBUTION_USER_PARTITION_KEY),
            switches: &[RECORD_DISTRIBUTION_AUTO],
        },
        // Switched repeatedly, and twice onto the value it already carries.
        Row {
            label: "record-distribution-switched-both-ways-twice",
            created_with: None,
            switches: &[
                RECORD_DISTRIBUTION_AUTO,
                RECORD_DISTRIBUTION_AUTO,
                RECORD_DISTRIBUTION_USER_PARTITION_KEY,
                RECORD_DISTRIBUTION_AUTO,
                RECORD_DISTRIBUTION_USER_PARTITION_KEY,
            ],
        },
    ]
}

/// Whatever the update stored is what a later read of the stream reports, for every value
/// the strategy admits and however many times it is switched — and a stream nothing set a
/// strategy on reports the documented default.
#[tokio::test]
async fn an_update_stores_the_strategy_every_later_read_reports() {
    let emulator = Emulator::new();
    for row in rows() {
        let case = row.label;
        on_demand_stream(&emulator, case, case, row.created_with).await;
        let arn = stream_arn(REGION, case);

        // Before any update: the strategy the create named, else the documented default —
        // "USER_PARTITION_KEY ... This is the default strategy".
        let expected = row
            .created_with
            .unwrap_or(RECORD_DISTRIBUTION_USER_PARTITION_KEY);
        let reported = reported_strategy(&emulator, case, case).await;
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case}: an on-demand stream reports the strategy its create named, else \
             the default {expected}, got {reported:?}"
        );

        for strategy in row.switches {
            set_strategy(&emulator, case, &arn, strategy).await;

            let reported = reported_strategy(&emulator, case, case).await;
            assert_eq!(
                reported.as_deref(),
                Some(*strategy),
                "case {case}: expected the stored strategy {strategy}, got {reported:?}"
            );

            // "the change takes effect immediately without downtime, data loss, or
            // disruption to producer or consumer applications": nothing is owed afterwards,
            // so the stream is still ACTIVE the moment the update is served.
            let status = summary_of(&emulator, case, case)
                .await
                .get("StreamStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                status, STREAM_ACTIVE,
                "case {case}: a strategy switch leaves the stream {STREAM_ACTIVE}, got \
                 {status}"
            );
        }
    }
}

/// The update is scoped to the ONE stream its ARN names: another on-demand stream of the
/// same account and region keeps the strategy it had.
#[tokio::test]
async fn an_update_changes_only_the_stream_it_names() {
    let emulator = Emulator::new();
    let case = "record-distribution-scoped";
    let updated = "record-distribution-scoped-updated";
    let untouched = "record-distribution-scoped-untouched";
    for name in [updated, untouched] {
        on_demand_stream(&emulator, case, name, None).await;
    }

    set_strategy(
        &emulator,
        case,
        &stream_arn(REGION, updated),
        RECORD_DISTRIBUTION_AUTO,
    )
    .await;

    assert_eq!(
        reported_strategy(&emulator, case, updated).await.as_deref(),
        Some(RECORD_DISTRIBUTION_AUTO),
        "case {case}: the stream the update named carries the new strategy"
    );
    assert_eq!(
        reported_strategy(&emulator, case, untouched).await.as_deref(),
        Some(RECORD_DISTRIBUTION_USER_PARTITION_KEY),
        "case {case}: the stream the update did not name keeps the default strategy"
    );
}

/// "Provisioned capacity mode streams do not support the record distribution strategy
/// setting": a provisioned stream reports no such member, every value the strategy admits is
/// refused on it, and the member stays absent — while the SAME stream taken to the on-demand
/// mode carries the setting from then on.
#[tokio::test]
async fn the_setting_belongs_to_an_on_demand_stream_only() {
    let emulator = Emulator::new();
    // (case, the ShardCount the provisioned create names)
    let provisioned: [(&str, i64); 3] = [
        ("record-distribution-provisioned-one-shard", 1),
        ("record-distribution-provisioned-three-shards", 3),
        ("record-distribution-provisioned-eight-shards", 8),
    ];

    for (name, shard_count) in provisioned {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": shard_count }),
            )
            .await;
        await_active(&emulator, name, name).await;

        let reported = reported_strategy(&emulator, name, name).await;
        assert_eq!(
            reported, None,
            "case {name}: a provisioned stream reports no RecordDistributionStrategy, got \
             {reported:?}"
        );

        for strategy in EVERY_STRATEGY {
            let (status, response) = emulator
                .call(
                    "UpdateStreamRecordDistributionStrategy",
                    &json!({
                        "StreamARN": stream_arn(REGION, name),
                        "RecordDistributionStrategy": strategy,
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {name}/{strategy}: a provisioned stream does not support the setting, \
                 got {status} with body {response}"
            );
            assert_eq!(
                error_message(&response),
                stream_provisioned_message(name),
                "case {name}/{strategy}: expected the provisioned-mode sentence this service \
                 states, got {response}"
            );
            let reported = reported_strategy(&emulator, name, name).await;
            assert_eq!(
                reported, None,
                "case {name}/{strategy}: a refused update stores nothing, got {reported:?}"
            );
        }
    }

    // The mode is what decides, not the stream: taken to the on-demand mode, the very same
    // stream carries the setting, and taken back it carries none again.
    let (name, _) = provisioned[0];
    let arn = stream_arn(REGION, name);
    for mode in [STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED] {
        emulator
            .ok(
                name,
                "UpdateStreamMode",
                &json!({ "StreamARN": arn, "StreamModeDetails": { "StreamMode": mode } }),
            )
            .await;
        await_active(&emulator, name, name).await;
        let expected = (mode == STREAM_MODE_ON_DEMAND)
            .then_some(RECORD_DISTRIBUTION_USER_PARTITION_KEY);
        let reported = reported_strategy(&emulator, name, name).await;
        assert_eq!(
            reported.as_deref(),
            expected,
            "case {name}/{mode}: expected the member reported as {expected:?}, got {reported:?}"
        );
        if mode == STREAM_MODE_ON_DEMAND {
            set_strategy(&emulator, name, &arn, RECORD_DISTRIBUTION_AUTO).await;
            assert_eq!(
                reported_strategy(&emulator, name, name).await.as_deref(),
                Some(RECORD_DISTRIBUTION_AUTO),
                "case {name}/{mode}: the stream now takes the setting its mode supports"
            );
        }
    }
}

/// An ARN this account and region hold no stream under names nothing to update, a request
/// naming no ARN at all names no stream, and a stream that is not `ACTIVE` is not available
/// for the change.
#[tokio::test]
async fn a_request_naming_no_usable_stream_is_refused() {
    let emulator = Emulator::new();
    let served = "record-distribution-refusals";
    on_demand_stream(&emulator, served, served, None).await;

    // (case, the ARN the request names, the error code, the sentence it carries)
    let rows: Vec<(&str, Option<String>, &str, String)> = vec![
        (
            // `StreamARN` is `@required` on this operation, so a request that names none is
            // refused by the boundary walk before the handler runs.
            "a request naming no StreamARN",
            None,
            "ValidationException",
            "1 validation error detected: Value null at 'streamARN' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a stream nothing created",
            Some(stream_arn(REGION, "record-distribution-never-created")),
            "ResourceNotFoundException",
            stream_not_found_message(&stream_arn(REGION, "record-distribution-never-created")),
        ),
        (
            "a stream of another region",
            Some(stream_arn("eu-west-1", served)),
            "ResourceNotFoundException",
            stream_not_found_message(&stream_arn("eu-west-1", served)),
        ),
        (
            // `StreamARN` is modelled `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`, so an ARN
            // naming another account's stream is well formed and names no stream of this
            // one.
            "a stream of another account",
            Some(format!(
                "arn:aws:kinesis:{REGION}:123456789012:stream/{served}"
            )),
            "ResourceNotFoundException",
            stream_not_found_message(&format!(
                "arn:aws:kinesis:{REGION}:123456789012:stream/{served}"
            )),
        ),
    ];

    for (case, arn, expected_code, expected_message) in &rows {
        let mut body = json!({ "RecordDistributionStrategy": RECORD_DISTRIBUTION_AUTO });
        if let Some(arn) = arn {
            body["StreamARN"] = json!(arn);
        }
        let (status, response) = emulator
            .call("UpdateStreamRecordDistributionStrategy", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {case}: expected {expected_code}, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            expected_message,
            "case {case}: expected the sentence this service states, got {response}"
        );
    }

    // "For successful operation, the resource must be in the ACTIVE state"
    // (`ResourceInUseException`): a stream whose create has not fallen due, and one whose
    // removal has been accepted. Each transition is armed a century out in the store, so the
    // state the case tests is the state the request lands in whatever the host's speed.
    //
    // (case, the stream's name, the state the request lands in)
    let not_active: [(&str, &str); 2] = [
        ("record-distribution-creating", STREAM_CREATING),
        ("record-distribution-deleting", STREAM_DELETING),
    ];
    for (name, expected_status) in not_active {
        on_demand_stream(&emulator, name, name, None).await;
        {
            let store = emulator.store();
            let mut held = store.lock();
            let stream = held
                .streams
                .get_mut(&RegionalName::new(REGION, name))
                .unwrap_or_else(|| panic!("case {name}: the create stored the stream"));
            let far_out = SystemTime::now() + A_LONG_TIME;
            match expected_status {
                STREAM_CREATING => stream.active_at = far_out,
                _ => stream.deleted_at = Some(far_out),
            }
        }

        let status_now = summary_of(&emulator, name, name)
            .await
            .get("StreamStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            status_now, expected_status,
            "case {name}: the stream is in {expected_status} when the update lands, got \
             {status_now}"
        );

        let (status, response) = emulator
            .call(
                "UpdateStreamRecordDistributionStrategy",
                &json!({
                    "StreamARN": stream_arn(REGION, name),
                    "RecordDistributionStrategy": RECORD_DISTRIBUTION_AUTO,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceInUseException"),
            "case {name}: an update of a {expected_status} stream is refused, got {status} \
             with body {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_not_active_message(name, expected_status),
            "case {name}: expected the not-active sentence this service states, got {response}"
        );
    }

    // The stream this account does hold is untouched by the refusals.
    assert_eq!(
        reported_strategy(&emulator, served, served).await.as_deref(),
        Some(RECORD_DISTRIBUTION_USER_PARTITION_KEY),
        "case {served}: a refused update stores nothing"
    );
}

/// One shard of the stream's map: its id and the hash key range it covers.
struct Shard {
    id: String,
    starting_hash_key: u128,
    ending_hash_key: u128,
}

/// The stream's open shard map, as `ListShards` reports it.
async fn shards_of(emulator: &Emulator, case: &str, name: &str) -> Vec<Shard> {
    let listed = emulator
        .ok(case, "ListShards", &json!({ "StreamName": name }))
        .await;
    let hash_key = |shard: &Value, member: &str| {
        shard
            .get("HashKeyRange")
            .and_then(|range| range.get(member))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u128>().ok())
            .unwrap_or_else(|| panic!("case {case}: a shard reports its {member}, got {shard}"))
    };
    listed["Shards"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListShards reported no Shards, got {listed}"))
        .iter()
        .map(|shard| Shard {
            id: shard["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a shard carries no ShardId"))
                .to_string(),
            starting_hash_key: hash_key(shard, "StartingHashKey"),
            ending_hash_key: hash_key(shard, "EndingHashKey"),
        })
        .collect()
}

/// The shard of `shards` that OWNS `partition_key` under the default strategy: the one whose
/// hash key range covers the key's 128-bit hash — "Amazon Kinesis Data Streams uses the
/// partition key as input to a hash function that maps the partition key and associated data
/// to a specific shard" (`PutRecordInput$PartitionKey`). Computed here from the shard map the
/// wire reported, so the case does not take the emulator's word for the placement.
fn owning_shard(case: &str, shards: &[Shard], partition_key: &str) -> String {
    let hash = partition_key_hash(partition_key);
    shards
        .iter()
        .find(|shard| shard.starting_hash_key <= hash && hash <= shard.ending_hash_key)
        .map(|shard| shard.id.clone())
        .unwrap_or_else(|| panic!("case {case}: the shard map covers every hash key"))
}

/// Write one record of `partition_key`, answering the shard id and sequence number
/// `PutRecord` reports.
async fn write_one(
    emulator: &Emulator,
    case: &str,
    name: &str,
    partition_key: &str,
) -> (String, String) {
    let answer = emulator
        .ok(
            case,
            "PutRecord",
            &json!({
                "StreamName": name,
                "PartitionKey": partition_key,
                "Data": aws_smithy_types::base64::encode(case),
            }),
        )
        .await;
    (
        answer["ShardId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: PutRecord answered no ShardId, got {answer}"))
            .to_string(),
        answer["SequenceNumber"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: PutRecord answered no SequenceNumber, got {answer}")
            })
            .to_string(),
    )
}

/// The sequence numbers `shard` serves from its oldest retained record.
async fn sequence_numbers_of(
    emulator: &Emulator,
    case: &str,
    name: &str,
    shard: &str,
) -> Vec<String> {
    let iterator = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": name,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator"))
        .to_string();
    let answer = emulator
        .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
        .await;
    answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: GetRecords answered no Records, got {answer}"))
        .iter()
        .map(|record| {
            record["SequenceNumber"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The stored strategy is what places every LATER write, and a switch redistributes nothing
/// already stored.
#[tokio::test]
async fn the_stored_strategy_places_every_later_write_and_moves_nothing_stored() {
    let emulator = Emulator::new();
    let name = "record-distribution-placement";
    on_demand_stream(&emulator, name, name, None).await;
    let shards = shards_of(&emulator, name, name).await;
    assert_eq!(
        shards.len(),
        ON_DEMAND_INITIAL_SHARDS as usize,
        "case {name}: an on-demand create allocates {ON_DEMAND_INITIAL_SHARDS} shards, got {}",
        shards.len()
    );

    // The partition keys the rule is checked over: rows differing in form and in length,
    // including both modelled bounds of `PartitionKey` (len 1..256).
    let long_key = "k".repeat(PARTITION_KEY_MAX_LENGTH);
    let keys: Vec<&str> = vec![
        "a",
        "a-hot-partition-key",
        "1123581321345589",
        "κλειδί-διαμερισμού",
        "key with spaces/and.punctuation",
        long_key.as_str(),
    ];

    // Under the default strategy every write of one key lands on the shard that key's hash
    // belongs to, whatever the key looks like.
    let mut before_the_switch: Vec<(&str, String, Vec<String>)> = Vec::new();
    for key in &keys {
        let case = format!("{name}/user-partition-key/{}", key.len());
        let owning = owning_shard(&case, &shards, key);
        let mut stored = Vec::new();
        for _ in 0..WRITES_PER_KEY {
            let (shard, sequence_number) = write_one(&emulator, &case, name, key).await;
            assert_eq!(
                shard, owning,
                "case {case}: the producer's partition key places the record on the shard \
                 whose hash key range covers it, got {shard}"
            );
            stored.push(sequence_number);
        }
        before_the_switch.push((key, owning, stored));
    }

    // Switched to AUTO, the service places the records itself and the producer's key is
    // ignored, so one key's writes no longer share a shard.
    set_strategy(
        &emulator,
        name,
        &stream_arn(REGION, name),
        RECORD_DISTRIBUTION_AUTO,
    )
    .await;
    for key in &keys {
        let case = format!("{name}/auto/{}", key.len());
        let mut placed = Vec::new();
        for _ in 0..WRITES_PER_KEY {
            let (shard, _) = write_one(&emulator, &case, name, key).await;
            assert!(
                shards.iter().any(|known| known.id == shard),
                "case {case}: the record is placed on a shard of the stream's map, got {shard}"
            );
            placed.push(shard);
        }
        let mut distinct = placed.clone();
        distinct.sort();
        distinct.dedup();
        assert!(
            distinct.len() > 1,
            "case {case}: under AUTO the records of one key spread over more than one shard, \
             got {placed:?}"
        );
    }

    // "Records already in the stream keep their original shard assignments and are not
    // redistributed": every record written before the switch is still served from the shard
    // the write assigned it, and from no other.
    for (key, owning, stored) in &before_the_switch {
        let case = format!("{name}/kept/{}", key.len());
        for shard in &shards {
            let served = sequence_numbers_of(&emulator, &case, name, &shard.id).await;
            let found: Vec<&String> = stored
                .iter()
                .filter(|sequence_number| served.contains(sequence_number))
                .collect();
            let expected = if &shard.id == owning { stored.len() } else { 0 };
            assert_eq!(
                found.len(),
                expected,
                "case {case}: shard {} serves {expected} of the records written before the \
                 switch, got {}",
                shard.id,
                found.len()
            );
        }
    }

    // And switched back, the producer's key places the record again — on the same shard as
    // before the switch.
    set_strategy(
        &emulator,
        name,
        &stream_arn(REGION, name),
        RECORD_DISTRIBUTION_USER_PARTITION_KEY,
    )
    .await;
    for (key, owning, _) in &before_the_switch {
        let case = format!("{name}/back/{}", key.len());
        for _ in 0..WRITES_PER_KEY {
            let (shard, _) = write_one(&emulator, &case, name, key).await;
            assert_eq!(
                &shard, owning,
                "case {case}: the key places the record on its own shard again, got {shard}"
            );
        }
    }
}
