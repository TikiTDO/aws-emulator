// SPDX-License-Identifier: CC0-1.0

//! The Kinesis data stream, end to end over the wire: `CreateStream` ->
//! `DescribeStream` / `DescribeStreamSummary`.
//!
//! RULE: `CreateStream` PERSISTS a stream under the name its request carries, scoped to
//! the region that request was signed for, and answers the modelled empty body; the
//! stream it stores carries a shard map that partitions the 128-bit hash key space into
//! as many contiguous, equal, non-overlapping slices as the stream's capacity mode gives
//! it shards (the `ShardCount` a provisioned stream names, the service-managed initial
//! count of an on-demand one), the documented default retention window, the `NONE`
//! encryption a stream nothing has encrypted reports, and every optional member the
//! request supplied. It reports `CREATING` until the transition the create armed falls
//! due and `ACTIVE` from then on. `DescribeStream` and `DescribeStreamSummary` project
//! that STORED record onto their output shapes, resolving the stream by `StreamName`, by
//! `StreamARN` or by both, so what a create wrote is what a read reports and a member the
//! create did not name stays absent. A name the region already holds a stream under is
//! refused with the modelled `ResourceInUseException` and the stored stream is left
//! untouched, while the same name in another region is another stream; a read naming a
//! stream this account and region do not hold is `ResourceNotFoundException`, and one
//! naming no stream at all is `InvalidArgumentException`.
//!
//! The recorded flow this was derived from provisions ONE stream
//! (`create_stream(StreamName=…, ShardCount=1)`) and is one row of the tables below. The
//! rows differ along what the rule generalises over: the capacity mode, the shard count
//! (both published bounds of the shard map and a count past one `DescribeStream` page),
//! which optional members the create names and at which modelled bound, the form and
//! length of `StreamName` inside its `@pattern`/`@length`, the identifier a read resolves
//! by, and the region the request is signed for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_identifier_mismatch_message, stream_not_found_message, ACTIVATION_DELAY,
    DEFAULT_MAX_RECORD_SIZE_KI_B, DEFAULT_RETENTION_PERIOD_HOURS, ENCRYPTION_TYPE_NONE,
    MAX_DESCRIBE_STREAM_SHARD_LIMIT, MAX_SHARDS_PER_REGION, MAX_SHARDS_TOTAL, MAX_STREAM_TAGS,
    ON_DEMAND_INITIAL_SHARDS, SHARD_WRITE_MI_BPS, STREAM_ACTIVE, STREAM_CREATING,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `@length` maximum of `StreamName`, in characters
/// (`smithy_query(query="operation",operation="CreateStream")` -> `StreamName` len
/// 1..128).
const STREAM_NAME_MAX_LENGTH: usize = 128;

/// The published bounds of `MaxRecordSizeInKiB` (same call -> range 1024..10240).
const MAX_RECORD_SIZE_BOUNDS: (i64, i64) = (1024, 10240);

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// [`Self::call_in`] in the default test region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation` in `region`, asserting a 200 and answering the body.
    async fn ok_in(&self, case: &str, region: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// [`Self::ok_in`] in the default test region.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.ok_in(case, REGION, operation, body).await
    }
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of `value`, absent when the body carries it as null.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|member| !member.is_null())
}

/// The ARN a stream named `name` in `region` must report: the canonical emulator ARN,
/// which also has to satisfy the modelled `StreamARN` pattern
/// `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`.
fn expected_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:kinesis:{region}:{}:stream/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `StreamName` of `length` characters inside the modelled `^[a-zA-Z0-9_.-]+$`.
fn name_of_length(length: usize) -> String {
    std::iter::repeat_n('s', length).collect()
}

/// One `CreateStream` request, and what the stream it creates must report.
struct CreateCase {
    /// What an assertion calls this row.
    label: &'static str,
    stream_name: String,
    shard_count: Option<i64>,
    /// `StreamModeDetails.StreamMode`, absent on a row that names none.
    stream_mode: Option<&'static str>,
    tags: Option<Value>,
    max_record_size_in_ki_b: Option<i64>,
    warm_throughput_mi_bps: Option<i64>,
    record_distribution_strategy: Option<&'static str>,
    /// The `StreamMode` the stored stream must report.
    expected_mode: &'static str,
    /// The shards the stored stream must hold.
    expected_shards: usize,
}

impl CreateCase {
    /// The `CreateStreamInput` body this row sends.
    fn body(&self) -> Value {
        let mut body = json!({ "StreamName": self.stream_name });
        if let Some(shard_count) = self.shard_count {
            body["ShardCount"] = json!(shard_count);
        }
        if let Some(stream_mode) = self.stream_mode {
            body["StreamModeDetails"] = json!({ "StreamMode": stream_mode });
        }
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        if let Some(size) = self.max_record_size_in_ki_b {
            body["MaxRecordSizeInKiB"] = json!(size);
        }
        if let Some(throughput) = self.warm_throughput_mi_bps {
            body["WarmThroughputMiBps"] = json!(throughput);
        }
        if let Some(strategy) = self.record_distribution_strategy {
            body["RecordDistributionStrategy"] = json!(strategy);
        }
        body
    }
}

/// The table the create/read rule is checked over. The rows differ along the capacity
/// mode, the shard count (one shard, a handful, the shard map that runs past one
/// `DescribeStream` page), the optional members named and the bounds they sit at, and the
/// form and length of `StreamName`.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "provisioned-by-default-one-shard",
            // The modelled `@length` minimum of `StreamName`.
            stream_name: name_of_length(1),
            shard_count: Some(1),
            stream_mode: None,
            tags: None,
            max_record_size_in_ki_b: None,
            warm_throughput_mi_bps: None,
            record_distribution_strategy: None,
            expected_mode: STREAM_MODE_PROVISIONED,
            expected_shards: 1,
        },
        CreateCase {
            label: "explicit-provisioned-three-shards-punctuated-name",
            stream_name: "sim.2real_stream-a.1".to_string(),
            shard_count: Some(3),
            stream_mode: Some(STREAM_MODE_PROVISIONED),
            tags: Some(json!({ "Team": "iot", "Env": "test" })),
            max_record_size_in_ki_b: Some(MAX_RECORD_SIZE_BOUNDS.0),
            warm_throughput_mi_bps: None,
            record_distribution_strategy: None,
            expected_mode: STREAM_MODE_PROVISIONED,
            expected_shards: 3,
        },
        CreateCase {
            label: "on-demand-service-managed-shards-longest-name",
            stream_name: name_of_length(STREAM_NAME_MAX_LENGTH),
            shard_count: None,
            stream_mode: Some(STREAM_MODE_ON_DEMAND),
            tags: Some(json!({ "Team": "iot" })),
            max_record_size_in_ki_b: Some(MAX_RECORD_SIZE_BOUNDS.1),
            warm_throughput_mi_bps: Some(0),
            // Only an on-demand stream carries a record distribution strategy.
            record_distribution_strategy: Some("AUTO"),
            expected_mode: STREAM_MODE_ON_DEMAND,
            expected_shards: ON_DEMAND_INITIAL_SHARDS as usize,
        },
        CreateCase {
            label: "provisioned-seven-shards-warm-throughput",
            stream_name: "sim2real-stream-b".to_string(),
            shard_count: Some(7),
            stream_mode: Some(STREAM_MODE_PROVISIONED),
            tags: None,
            max_record_size_in_ki_b: None,
            warm_throughput_mi_bps: Some(12),
            record_distribution_strategy: None,
            expected_mode: STREAM_MODE_PROVISIONED,
            expected_shards: 7,
        },
        CreateCase {
            label: "provisioned-shard-map-past-one-describe-page",
            stream_name: "sim2real-stream-wide".to_string(),
            shard_count: Some(i64::from(MAX_DESCRIBE_STREAM_SHARD_LIMIT) + 1),
            stream_mode: None,
            tags: None,
            max_record_size_in_ki_b: None,
            warm_throughput_mi_bps: None,
            record_distribution_strategy: None,
            expected_mode: STREAM_MODE_PROVISIONED,
            expected_shards: MAX_DESCRIBE_STREAM_SHARD_LIMIT as usize + 1,
        },
    ]
}

/// Assert `shards` is the slice of a stream's shard map starting at `offset`: the ids run
/// in order, every hash key range begins where the previous one ended, the first range of
/// the map starts at 0 and the last ends at the top of the 128-bit space, and every shard
/// is open.
fn assert_shard_page(label: &str, shards: &[Value], offset: usize, map_len: usize) {
    let mut previous_end: Option<u128> = None;
    for (index, shard) in shards.iter().enumerate() {
        let position = offset + index;
        let shard_id = shard
            .get("ShardId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            shard_id,
            format!("shardId-{position:012}"),
            "case {label}: shard {position} must carry the id its position names, got \
             {shard_id:?}"
        );

        let range = shard.get("HashKeyRange").cloned().unwrap_or(Value::Null);
        let start: u128 = range
            .get("StartingHashKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|error| {
                panic!("case {label}: shard {position} StartingHashKey: {error} in {range}")
            });
        let end: u128 = range
            .get("EndingHashKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .parse()
            .unwrap_or_else(|error| {
                panic!("case {label}: shard {position} EndingHashKey: {error} in {range}")
            });
        assert!(
            start <= end,
            "case {label}: shard {position} must cover a non-empty range, got {start}..{end}"
        );
        match previous_end {
            None if position == 0 => assert_eq!(
                start, 0,
                "case {label}: the first shard of the map must start at 0, got {start}"
            ),
            None => {}
            Some(previous) => assert_eq!(
                start,
                previous + 1,
                "case {label}: shard {position} must start where shard {} ended, got {start} \
                 after {previous}",
                position - 1
            ),
        }
        previous_end = Some(end);
        if position + 1 == map_len {
            assert_eq!(
                end,
                u128::MAX,
                "case {label}: the last shard of the map must end at the top of the hash key \
                 space, got {end}"
            );
        }

        let sequence = shard
            .get("SequenceNumberRange")
            .cloned()
            .unwrap_or(Value::Null);
        let starting = sequence
            .get("StartingSequenceNumber")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            starting.parse::<u128>().is_ok() && !starting.starts_with('0'),
            "case {label}: shard {position} must carry a decimal StartingSequenceNumber, got \
             {starting:?}"
        );
        assert!(
            member(&sequence, "EndingSequenceNumber").is_none(),
            "case {label}: an open shard must report no EndingSequenceNumber, got {sequence}"
        );
    }
}

#[tokio::test]
async fn a_created_stream_is_reported_by_name_and_by_arn() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let label = case.label;
        let created = emulator.ok(label, "CreateStream", &case.body()).await;
        assert!(
            created.is_null() || created == json!({}),
            "case {label}: CreateStream models no output members, got {created}"
        );

        let arn = expected_arn(REGION, &case.stream_name);
        let summary = emulator
            .ok(
                label,
                "DescribeStreamSummary",
                &json!({ "StreamName": case.stream_name }),
            )
            .await;
        let summary = summary
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null);

        assert_eq!(
            member(&summary, "StreamName").and_then(Value::as_str),
            Some(case.stream_name.as_str()),
            "case {label}: the summary must report the name the create carried, got {summary}"
        );
        assert_eq!(
            member(&summary, "StreamARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {label}: the summary must report the stream's ARN, got {summary}"
        );
        let status = member(&summary, "StreamStatus")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            status == STREAM_CREATING || status == STREAM_ACTIVE,
            "case {label}: a created stream is CREATING until its transition is due and \
             ACTIVE after, got {status:?}"
        );
        assert_eq!(
            member(&summary, "RetentionPeriodHours").and_then(Value::as_i64),
            Some(i64::from(DEFAULT_RETENTION_PERIOD_HOURS)),
            "case {label}: a new stream carries the default retention window, got {summary}"
        );
        assert_eq!(
            summary
                .get("StreamModeDetails")
                .and_then(|details| details.get("StreamMode"))
                .and_then(Value::as_str),
            Some(case.expected_mode),
            "case {label}: expected the {} capacity mode, got {summary}",
            case.expected_mode
        );
        assert_eq!(
            member(&summary, "OpenShardCount").and_then(Value::as_i64),
            Some(case.expected_shards as i64),
            "case {label}: expected {} open shards, got {summary}",
            case.expected_shards
        );
        assert_eq!(
            member(&summary, "EncryptionType").and_then(Value::as_str),
            Some(ENCRYPTION_TYPE_NONE),
            "case {label}: a stream nothing encrypted reports NONE, got {summary}"
        );
        assert_eq!(
            member(&summary, "ConsumerCount").and_then(Value::as_i64),
            Some(0),
            "case {label}: no consumer is registered with a new stream, got {summary}"
        );
        assert!(
            member(&summary, "KeyId").is_none() && member(&summary, "StreamId").is_none(),
            "case {label}: a member nothing set must stay absent, got {summary}"
        );
        assert_eq!(
            member(&summary, "MaxRecordSizeInKiB").and_then(Value::as_i64),
            // Every stream is configured for a maximum record size: the value the create
            // named, or the default record size a create that names none falls back to.
            Some(
                case.max_record_size_in_ki_b
                    .unwrap_or(i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B))
            ),
            "case {label}: MaxRecordSizeInKiB must be the maximum the stream is configured \
             for, got {summary}"
        );
        assert_eq!(
            member(&summary, "RecordDistributionStrategy").and_then(Value::as_str),
            case.record_distribution_strategy,
            "case {label}: RecordDistributionStrategy must be exactly what the create named, \
             got {summary}"
        );
        // "Represents the warm throughput configuration on the stream. This is only present
        // for On-Demand Kinesis Data Streams" (`WarmThroughputObject`): an on-demand stream
        // reports the target its create named, or the throughput its open shards provide, and
        // a provisioned stream reports no such configuration at all.
        let expected_warm = (case.expected_mode == STREAM_MODE_ON_DEMAND).then(|| {
            case.warm_throughput_mi_bps
                .unwrap_or(case.expected_shards as i64 * i64::from(SHARD_WRITE_MI_BPS))
        });
        assert_eq!(
            summary
                .get("WarmThroughput")
                .and_then(|warm| warm.get("TargetMiBps"))
                .and_then(Value::as_i64),
            expected_warm,
            "case {label}: expected a warm throughput target of {expected_warm:?}, got {summary}"
        );
        assert_eq!(
            member(&summary, "EnhancedMonitoring"),
            Some(&json!([{ "ShardLevelMetrics": [] }])),
            "case {label}: no shard-level metric is enabled on a new stream, got {summary}"
        );

        // The same record, read by ARN rather than by name, and through the other reader.
        let described = emulator
            .ok(label, "DescribeStream", &json!({ "StreamARN": arn }))
            .await;
        let description = described
            .get("StreamDescription")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&description, "StreamName").and_then(Value::as_str),
            Some(case.stream_name.as_str()),
            "case {label}: a read by ARN must resolve the same stream, got {description}"
        );
        assert_eq!(
            member(&description, "RetentionPeriodHours").and_then(Value::as_i64),
            member(&summary, "RetentionPeriodHours").and_then(Value::as_i64),
            "case {label}: both readers project ONE stored record, got {description}"
        );

        let page_len = case
            .expected_shards
            .min(MAX_DESCRIBE_STREAM_SHARD_LIMIT as usize);
        let shards = member(&description, "Shards")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            shards.len(),
            page_len,
            "case {label}: expected {page_len} shards in one page, got {}",
            shards.len()
        );
        assert_eq!(
            member(&description, "HasMoreShards").and_then(Value::as_bool),
            Some(case.expected_shards > page_len),
            "case {label}: HasMoreShards must say whether the map runs past this page, got \
             {description}"
        );
        assert_shard_page(label, &shards, 0, case.expected_shards);
    }
}

#[tokio::test]
async fn a_new_stream_is_creating_and_then_becomes_active() {
    let emulator = Emulator::new();
    let case = "creating-then-active";
    let name = "sim2real-stream-transition";

    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;

    let status = |body: &Value| {
        body.get("StreamDescriptionSummary")
            .and_then(|summary| summary.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    let immediately = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    assert_eq!(
        status(&immediately),
        STREAM_CREATING,
        "case {case}: a caller reading straight after the create still sees CREATING, got \
         {immediately}"
    );

    // The transition is owed at a stored instant, so waiting past it is all it takes.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(400)).await;

    let settled = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    assert_eq!(
        status(&settled),
        STREAM_ACTIVE,
        "case {case}: once the transition is due the stream is ACTIVE, got {settled}"
    );
    // A read reports the transition; it does not undo or re-arm it.
    let again = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    assert_eq!(
        status(&again),
        STREAM_ACTIVE,
        "case {case}: a later read must agree with the first, got {again}"
    );
}

#[tokio::test]
async fn a_duplicate_stream_name_is_refused_and_leaves_the_stored_stream_untouched() {
    // Each row: the first create's body, then a second create under the SAME name whose
    // every other member differs.
    let cases: Vec<(&str, Value, Value, i64)> = vec![
        (
            "provisioned-over-provisioned",
            json!({ "StreamName": "sim2real-stream-dup-a", "ShardCount": 2 }),
            json!({ "StreamName": "sim2real-stream-dup-a", "ShardCount": 5 }),
            2,
        ),
        (
            "on-demand-over-provisioned",
            json!({ "StreamName": "sim2real-stream-dup-b", "ShardCount": 1 }),
            json!({
                "StreamName": "sim2real-stream-dup-b",
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
            1,
        ),
        (
            "provisioned-over-on-demand",
            json!({
                "StreamName": "sim2real-stream-dup-c",
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
            json!({ "StreamName": "sim2real-stream-dup-c", "ShardCount": 3 }),
            i64::from(ON_DEMAND_INITIAL_SHARDS),
        ),
    ];

    for (label, first, second, expected_shards) in cases {
        let emulator = Emulator::new();
        emulator.ok(label, "CreateStream", &first).await;

        let (status, response) = emulator.call("CreateStream", &second).await;
        assert_eq!(
            status, 400,
            "case {label}: a duplicate name is a client error, got {status} with body \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceInUseException",
            "case {label}: expected the modelled conflict, got {response}"
        );

        let name = first
            .get("StreamName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let summary = emulator
            .ok(label, "DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        let summary = summary
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&summary, "OpenShardCount").and_then(Value::as_i64),
            Some(expected_shards),
            "case {label}: the refused create must leave the stored stream untouched, got \
             {summary}"
        );
    }
}

#[tokio::test]
async fn the_same_stream_name_in_two_regions_is_two_streams() {
    let emulator = Emulator::new();
    let name = "sim2real-stream-regional";
    // Each row: the region, and the shard count that region's stream is created with.
    let cases = [("us-east-1", 1), ("us-west-2", 3), ("eu-central-1", 2)];

    for (region, shard_count) in cases {
        emulator
            .ok_in(
                region,
                region,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": shard_count }),
            )
            .await;
    }

    for (region, shard_count) in cases {
        let summary = emulator
            .ok_in(
                region,
                region,
                "DescribeStreamSummary",
                &json!({ "StreamName": name }),
            )
            .await;
        let summary = summary
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&summary, "OpenShardCount").and_then(Value::as_i64),
            Some(i64::from(shard_count)),
            "case {region}: each region holds its own stream under the shared name, got \
             {summary}"
        );
        assert_eq!(
            member(&summary, "StreamARN").and_then(Value::as_str),
            Some(expected_arn(region, name).as_str()),
            "case {region}: the ARN carries the region that created the stream, got {summary}"
        );
    }

    // Another region's ARN does not resolve through this region's endpoint.
    let (status, response) = emulator
        .call_in(
            "us-east-1",
            "DescribeStream",
            &json!({ "StreamARN": expected_arn("us-west-2", name) }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case cross-region-arn: a stream of another region does not resolve here, got \
         {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case cross-region-arn: expected the modelled not-found, got {response}"
    );
}

#[tokio::test]
async fn a_read_naming_an_unknown_or_no_stream_is_refused() {
    let emulator = Emulator::new();
    let held = "sim2real-stream-held";
    emulator
        .ok(
            "seed",
            "CreateStream",
            &json!({ "StreamName": held, "ShardCount": 1 }),
        )
        .await;

    // Each row: the request body, the error code it must answer, and the message.
    let not_found_cases: Vec<(&str, Value, String)> = vec![
        (
            "unknown-name",
            json!({ "StreamName": "sim2real-stream-absent" }),
            stream_not_found_message("sim2real-stream-absent"),
        ),
        (
            "unknown-arn",
            json!({ "StreamARN": expected_arn(REGION, "sim2real-stream-absent") }),
            stream_not_found_message(&expected_arn(REGION, "sim2real-stream-absent")),
        ),
    ];

    for (label, body, message) in not_found_cases {
        for operation in ["DescribeStream", "DescribeStreamSummary"] {
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label}/{operation}: expected the modelled client error, got {status} \
                 with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {label}/{operation}: expected the modelled not-found, got {response}"
            );
            assert_eq!(
                error_message(&response),
                message,
                "case {label}/{operation}: the refusal names the identifier at fault, got \
                 {response}"
            );
        }
    }

    // A pair whose two halves DISAGREE is reconciled from the request itself, before any
    // stream is looked up: whether or not either half names a held stream, the refusal is the
    // modelled invalid-argument naming both the name sent and the one the ARN carries.
    let other = "sim2real-stream-held-too";
    emulator
        .ok(
            "seed",
            "CreateStream",
            &json!({ "StreamName": other, "ShardCount": 1 }),
        )
        .await;
    let mismatched: Vec<(&str, &str, String)> = vec![
        ("both-held", held, expected_arn(REGION, other)),
        (
            "neither-held",
            "sim2real-stream-absent",
            expected_arn(REGION, "sim2real-stream-other"),
        ),
        (
            "an-arn-of-another-region",
            held,
            expected_arn("us-west-2", other),
        ),
    ];
    for (label, name, arn) in mismatched {
        let (status, response) = emulator
            .call(
                "DescribeStreamSummary",
                &json!({ "StreamName": name, "StreamARN": arn }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {label}: a pair whose halves disagree names no stream, got {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_identifier_mismatch_message(name, &arn),
            "case {label}: the refusal names both halves of the pair, got {response}"
        );
    }

    // And a request that names no stream at all: `StreamId` is modelled "Not Implemented.
    // Reserved for future use", so it names no stream either.
    let unspecified_cases: Vec<(&str, Value)> = vec![
        ("no-member", json!({})),
        (
            "stream-id-only",
            json!({ "StreamId": "abcdefghij0123456789-abc" }),
        ),
    ];
    for (label, body) in unspecified_cases {
        for operation in ["DescribeStream", "DescribeStreamSummary"] {
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label}/{operation}: expected a client error, got {status} with body \
                 {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidArgumentException",
                "case {label}/{operation}: expected the modelled refusal, got {response}"
            );
            assert_eq!(
                error_message(&response),
                STREAM_IDENTIFIER_REQUIRED_MESSAGE,
                "case {label}/{operation}: the refusal states the rule, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_outside_the_documented_rules_is_refused_and_stores_nothing() {
    // Each row: the create body, and the modelled error code it must answer.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "on-demand-with-a-shard-count",
            json!({
                "StreamName": "sim2real-stream-bad-a",
                "ShardCount": 2,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
            "InvalidArgumentException",
        ),
        (
            "provisioned-without-a-shard-count",
            json!({
                "StreamName": "sim2real-stream-bad-b",
                "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
            }),
            "InvalidArgumentException",
        ),
        (
            "default-mode-without-a-shard-count",
            json!({ "StreamName": "sim2real-stream-bad-c" }),
            "InvalidArgumentException",
        ),
        (
            "provisioned-with-a-record-distribution-strategy",
            json!({
                "StreamName": "sim2real-stream-bad-d",
                "ShardCount": 1,
                "RecordDistributionStrategy": "USER_PARTITION_KEY",
            }),
            "InvalidArgumentException",
        ),
        (
            "more-shards-than-the-account-quota-authorizes",
            json!({
                "StreamName": "sim2real-stream-bad-e",
                "ShardCount": i64::from(MAX_SHARDS_PER_REGION) + 1,
            }),
            "LimitExceededException",
        ),
        (
            "more-tags-than-the-documented-ceiling",
            json!({
                "StreamName": "sim2real-stream-bad-f",
                "ShardCount": 1,
                "Tags": (0..=MAX_STREAM_TAGS)
                    .map(|index| (format!("key-{index}"), json!("value")))
                    .collect::<serde_json::Map<String, Value>>(),
            }),
            "InvalidArgumentException",
        ),
    ];

    let emulator = Emulator::new();
    for (label, body, expected_code) in cases {
        let (status, response) = emulator.call("CreateStream", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {label}: expected {expected_code}, got {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {label}: a refusal has to say what is wrong, got {response}"
        );

        // Nothing was stored: the name is still free.
        let name = body
            .get("StreamName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (status, response) = emulator
            .call("DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {label}: a refused create stores nothing, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label}: a refused create stores nothing, got {response}"
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_whatever_region_a_caller_names() {
    // The ceilings a create is refused past. The per-region shard quota is reached by
    // creating streams of half of it; the ceiling that no request can partition is then
    // reached by doing the same in region after region.
    let emulator = Emulator::new();
    let shards_per_stream = MAX_SHARDS_PER_REGION / 2;
    let streams_per_region = (MAX_SHARDS_PER_REGION / shards_per_stream) as usize;
    assert!(
        streams_per_region >= 1,
        "case bounds: the per-region shard quota must admit at least one full stream, got \
         {MAX_SHARDS_PER_REGION} against {shards_per_stream}"
    );

    // One region, filled to its shard quota, then refused.
    for index in 0..streams_per_region {
        emulator
            .ok(
                "region-quota/fill",
                "CreateStream",
                &json!({
                    "StreamName": format!("sim2real-stream-quota-{index}"),
                    "ShardCount": shards_per_stream,
                }),
            )
            .await;
    }
    let (status, response) = emulator
        .call(
            "CreateStream",
            &json!({
                "StreamName": "sim2real-stream-quota-past",
                "ShardCount": shards_per_stream,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case region-quota/past: a create past the region's shard quota is refused, got \
         {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case region-quota/past: expected the modelled quota refusal, got {response}"
    );

    // The quota is partitioned by the region the CALLER names, so the ceilings that no
    // request can partition are what bound the store: naming region after region is
    // refused once the total shard ceiling is reached.
    let mut created_shards = shards_per_stream * streams_per_region as u32;
    let mut refused_in_another_region = false;
    for index in 0..64 {
        let region = format!("ap-southeast-{index}");
        let (status, response) = emulator
            .call_in(
                &region,
                "CreateStream",
                &json!({
                    "StreamName": "sim2real-stream-spill",
                    "ShardCount": shards_per_stream,
                }),
            )
            .await;
        if status == 200 {
            created_shards += shards_per_stream;
            assert!(
                created_shards <= MAX_SHARDS_TOTAL,
                "case total-ceiling: the store must never hold more than {MAX_SHARDS_TOTAL} \
                 shards, got {created_shards} after {region}"
            );
            continue;
        }
        assert_eq!(
            error_code(&response),
            "LimitExceededException",
            "case total-ceiling: expected the modelled quota refusal in {region}, got \
             {response}"
        );
        refused_in_another_region = true;
        break;
    }
    assert!(
        refused_in_another_region,
        "case total-ceiling: a caller that varies the region must still be refused once the \
         store's own ceiling is reached, created {created_shards} shards"
    );
}

#[tokio::test]
async fn the_shard_map_pages_and_partitions_the_whole_hash_key_space() {
    let emulator = Emulator::new();
    let case = "shard-map-paging";
    let name = "sim2real-stream-paged";
    // A map of more shards than one `DescribeStream` page carries, so the paging the rule
    // names is exercised; the account's shard quota admits it in one stream.
    let map_len = MAX_DESCRIBE_STREAM_SHARD_LIMIT as usize * 5 / 2;
    let shard_count = i64::try_from(map_len).unwrap_or(i64::MAX);

    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": shard_count }),
        )
        .await;

    // Each row: the `Limit` a page asks for, and the page size it may actually get —
    // "If you specify a value greater than 100, at most 100 results are returned".
    let page_sizes = [
        (Some(1_i64), 1_usize),
        (Some(37), 37),
        (None, MAX_DESCRIBE_STREAM_SHARD_LIMIT as usize),
        (Some(10_000), MAX_DESCRIBE_STREAM_SHARD_LIMIT as usize),
    ];

    for (limit, expected_page) in page_sizes {
        let label = format!("{case}/limit-{limit:?}");
        let mut seen = 0_usize;
        let mut exclusive_start: Option<String> = None;

        loop {
            let mut body = json!({ "StreamName": name });
            if let Some(limit) = limit {
                body["Limit"] = json!(limit);
            }
            if let Some(start) = &exclusive_start {
                body["ExclusiveStartShardId"] = json!(start);
            }
            let described = emulator.ok(&label, "DescribeStream", &body).await;
            let description = described
                .get("StreamDescription")
                .cloned()
                .unwrap_or(Value::Null);
            let shards = member(&description, "Shards")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                shards.len(),
                expected_page.min(map_len - seen),
                "case {label}: a page carries the clamped limit's worth of shards, got {}",
                shards.len()
            );
            assert_shard_page(&label, &shards, seen, map_len);

            seen += shards.len();
            let has_more = member(&description, "HasMoreShards")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            assert_eq!(
                has_more,
                seen < map_len,
                "case {label}: HasMoreShards must say whether shards remain after {seen} of \
                 {map_len}, got {has_more}"
            );
            if !has_more {
                break;
            }
            exclusive_start = shards
                .last()
                .and_then(|shard| shard.get("ShardId"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }

        assert_eq!(
            seen, map_len,
            "case {label}: paging must walk the whole shard map, saw {seen} of {map_len}"
        );
    }

    // An `ExclusiveStartShardId` that is not a shard of this stream is refused rather
    // than silently answered with the first page.
    let (status, response) = emulator
        .call(
            "DescribeStream",
            &json!({ "StreamName": name, "ExclusiveStartShardId": "shardId-999999999999" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}/unknown-start: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidArgumentException",
        "case {case}/unknown-start: expected the modelled refusal, got {response}"
    );
}
