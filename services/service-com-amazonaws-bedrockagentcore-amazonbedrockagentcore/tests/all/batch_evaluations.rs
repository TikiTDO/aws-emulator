// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore batch-evaluation surface —
//! `StartBatchEvaluation`, `GetBatchEvaluation` and `ListBatchEvaluations`.
//!
//! The rules under test:
//!
//! * a started batch evaluation is a RESOURCE of this service, so every member of the
//!   starting request reads back through `GetBatchEvaluation` — whatever shape of job was
//!   asked for — under an id and an ARN this emulator minted for it;
//! * `BatchEvaluationStatus` is read off the CLOCK: `PENDING` until the job starts
//!   running, `IN_PROGRESS` while it is and `COMPLETED` once it has run to its end, so
//!   reading the job advances nothing and `updatedAt` names the instant of the transition
//!   that was last due;
//! * no rating is reported for a job: a rating is an evaluator's output and no judge
//!   model runs in this emulator, so `evaluationResults`, `insightResults`,
//!   `failureAnalysisResult`, `userIntentResult`, `executionSummaryResult` and
//!   `errorDetails` are absent from every read;
//! * `clientToken` is an `@idempotencyToken`: a replayed token answers with the job it
//!   already started, while two starts carrying different tokens are two jobs even under
//!   one `batchEvaluationName`;
//! * a `batchEvaluationId` no job is held for is refused 404 `ResourceNotFoundException`
//!   with the sentence this service words every missed lookup with;
//! * `ListBatchEvaluations` reports exactly the jobs this emulator holds, cut from ONE
//!   total order (newest first), one `maxResults`-sized page at a time with an opaque
//!   `nextToken` resuming after it: whatever page size a caller asks for, following the
//!   token to the end yields every held job exactly once in the same order, each summary
//!   agreeing with a read of the same job, while a `maxResults` outside its published
//!   range or a `nextToken` this listing never issued is refused 400
//!   `ValidationException`.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the routes, status codes, `restJson1` bodies and modelled error codes are
//! exercised and not just the handler functions.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// A `clientToken` of the modelled length (33..256) and pattern.
const CLIENT_TOKEN: &str = "batchevalclienttoken00000000000001";
/// A second `clientToken`, so a case can tell a replay from a fresh call.
const OTHER_CLIENT_TOKEN: &str = "batchevalclienttoken00000000000002";

/// An online evaluation configuration, in the form `OnlineEvaluationConfigArn` publishes
/// as its pattern (source: smithy_query query="shape" shape="DataSourceConfig").
const ONLINE_EVALUATION_CONFIG_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:online-evaluation-config/quality-0a1b2c3d4e";

/// A KMS key, in the form `KmsKeyArn` publishes as its pattern (source: smithy_query
/// query="operation" operation="StartBatchEvaluation").
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// The emulator, with request authentication off so a test signs nothing, plus the shared
/// engine so a case can age a stored job's clock without sleeping.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `StartBatchEvaluation`: `POST /evaluations/batch-evaluate`.
async fn start_batch_evaluation(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        "/evaluations/batch-evaluate",
        Some(body),
    )
    .await
}

/// `GetBatchEvaluation`: `GET /evaluations/batch-evaluate/{batchEvaluationId}`. The
/// `@httpLabel` needs no escaping — `BatchEvaluationId` admits only letters, digits, `-`
/// and `_`.
async fn get_batch_evaluation(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/evaluations/batch-evaluate/{id}"),
        None,
    )
    .await
}

/// One member of a JSON object, or `Value::Null` when it is absent.
fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

/// One member of a JSON object as a string, or `""`.
fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

/// The wire error code of a refusal (`__type` on `restJson1`).
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// `BatchEvaluationId`'s modelled pattern,
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, checked without a regex dependency:
/// the discriminator is the last ten characters after the final `-`, and everything
/// before it is an identifier starting with a letter.
fn is_modelled_batch_evaluation_id(id: &str) -> bool {
    let Some((prefix, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        && (1..=100).contains(&prefix.len())
        && prefix.starts_with(|c: char| c.is_ascii_alphabetic())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A batch evaluation ARN as this service spells one: the partition, this service's
/// namespace, the region the request was signed for, the twelve-digit account and the
/// `batch-evaluate/<batchEvaluationId>` resource segment.
fn is_batch_evaluation_arn(arn: &str, id: &str) -> bool {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    segments.len() == 6
        && segments[0] == "arn"
        && segments[1].starts_with("aws")
        && segments[2] == "bedrock-agentcore"
        && segments[3] == "us-east-1"
        && segments[4].len() == 12
        && segments[4].bytes().all(|b| b.is_ascii_digit())
        && segments[5] == format!("batch-evaluate/{id}")
}

/// Move a stored job's start instant `by` further into the past, so a case can reach the
/// far side of a lifecycle span without sleeping for it. This is the test reaching into
/// the store the handlers share, not an emulator behaviour.
fn age_batch_evaluation(engine: &engine::InMemoryEngine, id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let job = store
        .batch_evaluations
        .get_mut(id)
        .expect("the batch evaluation that was just started");
    job.created_at -= by;
}

/// How many batch evaluation jobs this emulator holds — the check that a replayed start
/// wrote nothing new.
fn batch_evaluation_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.batch_evaluations.len()
}

/// A `cloudWatchLogs` data source naming the services whose traces are evaluated.
fn cloudwatch_source(service_names: &[&str], log_group_names: &[&str]) -> Value {
    json!({"cloudWatchLogs": {
        "serviceNames": service_names,
        "logGroupNames": log_group_names,
    }})
}

/// The smallest `StartBatchEvaluation` request the model admits: every `@required`
/// member and nothing else.
fn minimal_request(name: &str) -> Value {
    json!({
        "batchEvaluationName": name,
        "dataSourceConfig": cloudwatch_source(&["checkout-agent"], &["/aws/bedrock-agentcore/checkout"]),
    })
}

/// `name`, `evaluators` and `insights` as a request carries them, for a table row that
/// asks for `count` of each.
fn evaluators(count: usize) -> Value {
    // `EvaluatorId`'s three pattern alternatives: a built-in, a third-party and a custom
    // evaluator minted by the control plane (source: smithy_query query="shape"
    // shape="Evaluator").
    let ids = [
        "Builtin.Correctness",
        "Builtin.Helpfulness",
        "ThirdParty.Ragas.Faithfulness",
        "quality-0a1b2c3d4e",
        "Builtin.Completeness",
        "Builtin.Coherence",
        "Builtin.Relevance",
        "Builtin.Groundedness",
        "Builtin.ToolUse",
        "Builtin.Goal.Accuracy",
    ];
    Value::Array(
        ids.iter()
            .take(count)
            .map(|id| json!({"evaluatorId": id}))
            .collect(),
    )
}

/// `insights` as a request carries them, up to the modelled maximum of ten.
fn insights(count: usize) -> Value {
    // `InsightId`'s two pattern alternatives: a built-in insight and a custom one
    // (source: smithy_query query="shape" shape="Insight").
    let ids = [
        "Builtin.FailureAnalysis",
        "Builtin.UserIntent",
        "Builtin.ExecutionSummary",
        "custom-1a2b3c4d5e",
        "Builtin.ToolErrors",
        "Builtin.LatencyOutliers",
        "Builtin.Hallucination",
        "Builtin.Refusals",
        "Builtin.Escalations",
        "Builtin.Loops",
    ];
    Value::Array(
        ids.iter()
            .take(count)
            .map(|id| json!({"insightId": id}))
            .collect(),
    )
}

/// The round-trip table: a row per shape of batch evaluation a caller can ask for. Rows
/// differ along the name's length (`BatchEvaluationName` is
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, so one character and 48 are its bounds), the
/// description (absent and both bounds of `BatchEvaluationDescription` len 1..200), the
/// member of the `DataSourceConfig` union and how much of it is filled in, the number of
/// evaluators and insights (none, one, the empty list the model admits, and the documented
/// maximum of ten), whether an output configuration, tags, a KMS key and per-session
/// evaluation metadata were supplied.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    let longest_name = "a".repeat(48);
    let longest_description = "d".repeat(200);
    vec![
        ("required-members-only", minimal_request("checkout_flow")),
        (
            "shortest-name-shortest-description-one-evaluator",
            json!({
                "batchEvaluationName": "a",
                "description": "d",
                "dataSourceConfig": cloudwatch_source(&["agent-a"], &["/aws/a"]),
                "evaluators": evaluators(1),
            }),
        ),
        (
            "longest-name-longest-description-ten-evaluators-ten-insights",
            json!({
                "batchEvaluationName": longest_name,
                "description": longest_description,
                "dataSourceConfig": cloudwatch_source(
                    &["checkout-agent"],
                    &["/aws/bedrock-agentcore/checkout", "/aws/bedrock-agentcore/search"],
                ),
                "evaluators": evaluators(10),
                "insights": insights(10),
            }),
        ),
        (
            "empty-insight-list-and-prefix-filtered-cloudwatch-source",
            json!({
                "batchEvaluationName": "support_triage",
                "dataSourceConfig": {"cloudWatchLogs": {
                    "serviceNames": ["support-agent"],
                    "logGroupNamePrefixes": ["/aws/bedrock-agentcore/support"],
                    "filterConfig": {
                        "sessionIds": ["session-01", "session-02"],
                        "timeRange": {"startTime": 1_767_225_600, "endTime": 1_767_312_000},
                        "sessionTraceIds": [{
                            "sessionId": "session-01",
                            "traceIds": ["4bf92f3577b34da6a3ce929d0e0e4736"],
                        }],
                    },
                }},
                "insights": insights(0),
                "evaluators": evaluators(2),
            }),
        ),
        (
            "online-evaluation-config-source-output-tags-kms-and-metadata",
            json!({
                "batchEvaluationName": "nightly_regression",
                "description": "re-score yesterday's sessions",
                "dataSourceConfig": {"onlineEvaluationConfigSource": {
                    "onlineEvaluationConfigArn": ONLINE_EVALUATION_CONFIG_ARN,
                    "timeRange": {"startTime": 1_767_225_600, "endTime": 1_767_312_000},
                }},
                "evaluators": evaluators(3),
                "insights": insights(1),
                "outputConfig": {"cloudWatchConfig": {
                    // `resultDestination` is an enum of DEDICATED_LOG_GROUP |
                    // SOURCE_LOG_GROUP (source: smithy_query query="shape"
                    // shape="CloudWatchOutputConfig").
                    "resultDestination": "DEDICATED_LOG_GROUP",
                    "logGroupName": "/aws/bedrock-agentcore/evaluations",
                    "logStreamName": "nightly",
                    "metricsNamespace": "AgentCore/Evaluations",
                }},
                "tags": {"team": "quality", "env": "staging"},
                "kmsKeyArn": KMS_KEY_ARN,
                "evaluationMetadata": {"sessionMetadata": [{
                    "sessionId": "session-01",
                    "testScenarioId": "refund-happy-path",
                    "groundTruth": {"inline": {
                        "assertions": [{"text": "the refund amount is stated"}],
                        "expectedTrajectory": {"toolNames": ["lookupOrder", "issueRefund"]},
                    }},
                    "metadata": {"locale": "en-US"},
                }]},
            }),
        ),
    ]
}

/// A started batch evaluation is reported from what was stored, and `GetBatchEvaluation`
/// reports the same record back — for every shape of request the model admits.
#[tokio::test]
async fn a_started_batch_evaluation_round_trips_through_get_batch_evaluation() {
    let (router, _engine) = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, request) in round_trip_cases() {
        let (status, started) = start_batch_evaluation(&router, request.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );

        let id = text(&started, "batchEvaluationId").to_string();
        assert!(
            is_modelled_batch_evaluation_id(&id),
            "case {case:?}: expected a batchEvaluationId matching ^[a-zA-Z][a-zA-Z0-9-_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {id:?}"
        );
        let requested_name = request
            .get("batchEvaluationName")
            .and_then(Value::as_str)
            .expect("every case names its job");
        assert!(
            id.starts_with(&format!("{requested_name}-")),
            "case {case:?}: expected the id to be minted from the name {requested_name:?}, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a batchEvaluationId no earlier start minted, got {id:?}"
        );
        let arn = text(&started, "batchEvaluationArn").to_string();
        assert!(
            is_batch_evaluation_arn(&arn, &id),
            "case {case:?}: expected an ARN addressing {id:?} in the signed region, got {arn:?}"
        );
        assert_eq!(
            text(&started, "batchEvaluationName"),
            requested_name,
            "case {case:?}: expected the name the request asked for, got {started:?}"
        );
        assert_eq!(
            text(&started, "status"),
            "PENDING",
            "case {case:?}: expected a just-started job to be PENDING, got {started:?}"
        );
        let created_at = member(&started, "createdAt").clone();
        assert!(
            created_at.is_number(),
            "case {case:?}: expected createdAt as epoch seconds (smithy.api#Timestamp on restJson1), got {created_at:?}"
        );
        // Every optional member the start echoes is the one the request asked for.
        for name in ["description", "evaluators", "insights", "outputConfig", "tags", "kmsKeyArn"] {
            assert_eq!(
                member(&started, name),
                request.get(name).unwrap_or(&Value::Null),
                "case {case:?}: expected StartBatchEvaluation to echo {name} as the request asked for it, got {started:?}"
            );
        }

        // The read reports the record that was just stored.
        let (status, read) = get_batch_evaluation(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBatchEvaluation, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationId"),
            id,
            "case {case:?}: expected the id the start minted, got {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationArn"),
            arn,
            "case {case:?}: expected the ARN the start minted, got {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationName"),
            requested_name,
            "case {case:?}: expected the stored name, got {read:?}"
        );
        assert_eq!(
            member(&read, "createdAt"),
            &created_at,
            "case {case:?}: expected the createdAt the start reported, got {read:?}"
        );
        assert_eq!(
            text(&read, "status"),
            "PENDING",
            "case {case:?}: expected the read to report the job as PENDING, got {read:?}"
        );
        for name in [
            "description",
            "evaluators",
            "insights",
            "dataSourceConfig",
            "outputConfig",
            "kmsKeyArn",
        ] {
            assert_eq!(
                member(&read, name),
                request.get(name).unwrap_or(&Value::Null),
                "case {case:?}: expected GetBatchEvaluation to report {name} as the request asked for it, got {read:?}"
            );
        }
        // No judge model and no insight analysis run here, so no rating, cluster or
        // failure is reported for any job.
        for name in [
            "evaluationResults",
            "insightResults",
            "failureAnalysisResult",
            "userIntentResult",
            "executionSummaryResult",
            "errorDetails",
        ] {
            assert_eq!(
                member(&read, name),
                &Value::Null,
                "case {case:?}: expected no {name} for a job nothing scored, got {read:?}"
            );
        }
    }
}

/// `BatchEvaluationStatus` is read off the clock, and reading a job advances nothing: a
/// row per point of the lifecycle, each asserted on two consecutive reads.
#[tokio::test]
async fn a_batch_evaluation_s_status_follows_the_clock() {
    // How far past the start instant each row reads the job, and what the lifecycle says
    // it is then: the span is 3 s per step, so the bounds of each step and one point past
    // the last are all exercised.
    let cases: [(&str, u64, &str, u64); 6] = [
        ("just started", 0, "PENDING", 0),
        ("one second in, still pending", 1, "PENDING", 0),
        ("at the first span, running", 3, "IN_PROGRESS", 3),
        ("inside the run", 5, "IN_PROGRESS", 3),
        ("at the second span, finished", 6, "COMPLETED", 6),
        ("long past the run", 600, "COMPLETED", 6),
    ];

    for (case, aged_seconds, expected_status, expected_update_offset) in cases {
        let (router, engine) = emulator();
        let (status, started) =
            start_batch_evaluation(&router, minimal_request("clock_probe")).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );
        let id = text(&started, "batchEvaluationId").to_string();
        let created_at = member(&started, "createdAt").as_f64().unwrap_or_default();

        age_batch_evaluation(&engine, &id, Duration::from_secs(aged_seconds));

        for read_number in 1..=2 {
            let (status, read) = get_batch_evaluation(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from read {read_number}, got {status} with {read:?}"
            );
            assert_eq!(
                text(&read, "status"),
                expected_status,
                "case {case:?}: expected status {expected_status} on read {read_number} at {aged_seconds}s past the start, got {read:?}"
            );
            // `createdAt` is the instant the job was started, whatever the clock has
            // since made due; the aged record's start instant is the reported one.
            let reported_created = member(&read, "createdAt").as_f64().unwrap_or_default();
            let expected_created = created_at - aged_seconds as f64;
            assert!(
                (reported_created - expected_created).abs() < 1.0,
                "case {case:?}: expected createdAt near {expected_created} on read {read_number}, got {reported_created}"
            );
            let reported_updated = member(&read, "updatedAt").as_f64().unwrap_or_default();
            let expected_updated = expected_created + expected_update_offset as f64;
            assert!(
                (reported_updated - expected_updated).abs() < 1.0,
                "case {case:?}: expected updatedAt near {expected_updated} (the last transition the clock made due) on read {read_number}, got {reported_updated}"
            );
        }
    }
}

/// A replayed `clientToken` answers with the job it already started, and two starts that
/// carry different tokens are two jobs even under one name.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_job_it_already_started() {
    let (router, engine) = emulator();

    let mut request = minimal_request("duplicate_name");
    request["clientToken"] = json!(CLIENT_TOKEN);
    let (status, first) = start_batch_evaluation(&router, request.clone()).await;
    assert_eq!(
        status, 202,
        "first start: expected HTTP 202, got {status} with {first:?}"
    );
    let first_id = text(&first, "batchEvaluationId").to_string();

    // The SAME token: the service ignores the request rather than starting a second job.
    let (status, replayed) = start_batch_evaluation(&router, request.clone()).await;
    assert_eq!(
        status, 202,
        "replayed token: expected HTTP 202, got {status} with {replayed:?}"
    );
    assert_eq!(
        text(&replayed, "batchEvaluationId"),
        first_id,
        "replayed token: expected the job the first start made, got {replayed:?}"
    );
    assert_eq!(
        batch_evaluation_count(&engine),
        1,
        "replayed token: expected the replay to have started no second job"
    );

    // A DIFFERENT token under the same name: its own job, with its own id, and both read
    // back.
    request["clientToken"] = json!(OTHER_CLIENT_TOKEN);
    let (status, second) = start_batch_evaluation(&router, request).await;
    assert_eq!(
        status, 202,
        "second token: expected HTTP 202, got {status} with {second:?}"
    );
    let second_id = text(&second, "batchEvaluationId").to_string();
    assert_ne!(
        second_id, first_id,
        "second token: expected a distinct batchEvaluationId, got {second_id:?}"
    );
    assert_eq!(
        batch_evaluation_count(&engine),
        2,
        "second token: expected two jobs to be held"
    );
    for (case, id) in [("first job", &first_id), ("second job", &second_id)] {
        let (status, read) = get_batch_evaluation(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading {id:?}, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "batchEvaluationName"),
            "duplicate_name",
            "case {case:?}: expected the name both starts asked for, got {read:?}"
        );
    }
}

/// A `batchEvaluationId` no job is held for is refused 404 `ResourceNotFoundException`,
/// naming the resource and the identifier that did not resolve.
#[tokio::test]
async fn a_batch_evaluation_id_no_job_is_held_for_is_refused() {
    let (router, _engine) = emulator();

    // One job exists, so the refusals below are about the id and not about an empty
    // store.
    let (status, started) = start_batch_evaluation(&router, minimal_request("held_job")).await;
    assert_eq!(
        status, 202,
        "expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
    );
    let held = text(&started, "batchEvaluationId").to_string();

    // Each row is an id the model admits that addresses no job: a plausible one, one
    // differing from the held job only in its discriminator, and the longest the pattern
    // allows.
    let unknown = [
        ("never started", "nightly_regression-0a1b2c3d4e".to_string()),
        (
            "same name, other discriminator",
            format!("{}-ffffffffff", held.rsplit_once('-').expect("a minted id").0),
        ),
        ("longest admissible id", format!("{}-0a1b2c3d4e", "j".repeat(89))),
    ];

    for (case, id) in unknown {
        let (status, body) = get_batch_evaluation(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {id:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled error code for {id:?}, got {body:?}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Batch Evaluation not found: {id}"),
            "case {case:?}: expected the service's not-found sentence for {id:?}, got {body:?}"
        );
    }

    // The job that IS held still reads back, so the refusals above removed nothing.
    let (status, read) = get_batch_evaluation(&router, &held).await;
    assert_eq!(
        status, 200,
        "expected the held job to still read back, got {status} with {read:?}"
    );
}

/// `ListBatchEvaluations`: `GET /evaluations/batch-evaluate`, with `maxResults` and
/// `nextToken` carried as `@httpQuery` members.
async fn list_batch_evaluations(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        "/evaluations/batch-evaluate".to_string()
    } else {
        format!("/evaluations/batch-evaluate?{query}")
    };
    send(router, "GET", &path, None).await
}

/// The summaries one `ListBatchEvaluations` page carries.
fn page(body: &Value) -> &Vec<Value> {
    member(body, "batchEvaluations")
        .as_array()
        .unwrap_or_else(|| panic!("expected batchEvaluations to be a list, got {body:?}"))
}

/// Every batch evaluation the emulator lists, following `nextToken` until the last page,
/// with `max_results` asked for on each request.
async fn list_every_batch_evaluation(
    router: &MultiServiceRouter,
    max_results: i64,
) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more request than there can be pages, so a token that never clears is caught
    // here instead of hanging the suite.
    for _ in 0..64 {
        let query = match token.as_deref() {
            None => format!("maxResults={max_results}"),
            Some(next) => format!("maxResults={max_results}&nextToken={next}"),
        };
        let (status, body) = list_batch_evaluations(router, &query).await;
        assert_eq!(
            status, 200,
            "paging with maxResults={max_results}: expected HTTP 200, got {status} with {body:?}"
        );
        let returned = page(&body);
        assert!(
            returned.len() as i64 <= max_results,
            "paging with maxResults={max_results}: expected at most {max_results} summaries on a page, got {}",
            returned.len()
        );
        summaries.extend(returned.iter().cloned());
        match member(&body, "nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return summaries,
        }
    }
    panic!("paging with maxResults={max_results} never reached a last page");
}

/// `ListBatchEvaluations` cuts every page from one total order: whatever `maxResults` a
/// caller asks for, following `nextToken` to the end yields every held job exactly once in
/// the same order, and the last page carries no token.
#[tokio::test]
async fn list_batch_evaluations_pages_the_whole_list_in_one_stable_order() {
    let (router, _engine) = emulator();

    // Seven jobs, so a page size that divides the list and one that does not are both
    // exercised.
    let mut started_order: Vec<String> = Vec::new();
    for index in 0..7 {
        let (status, started) =
            start_batch_evaluation(&router, minimal_request(&format!("paged_job_{index}"))).await;
        assert_eq!(
            status, 202,
            "starting paged_job_{index}: expected HTTP 202, got {status} with {started:?}"
        );
        started_order.push(text(&started, "batchEvaluationId").to_string());
    }
    let mut newest_first = started_order.clone();
    newest_first.reverse();

    // `maxResults` is range 1..100 (source: smithy_query query="operation"
    // operation="ListBatchEvaluations"): both published bounds, a size that divides the
    // list, one that does not, and one larger than it.
    for max_results in [1_i64, 2, 3, 7, 8, 100] {
        let summaries = list_every_batch_evaluation(&router, max_results).await;
        let order: Vec<String> = summaries
            .iter()
            .map(|summary| text(summary, "batchEvaluationId").to_string())
            .collect();
        assert_eq!(
            order, newest_first,
            "case maxResults={max_results}: expected every held job exactly once, newest first, got {order:?}"
        );
    }

    // A request that names no `maxResults` is served as much of the list as one page can
    // carry, and that page closes the listing.
    let (status, body) = list_batch_evaluations(&router, "").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 with no maxResults, got {status} with {body:?}"
    );
    let order: Vec<String> = page(&body)
        .iter()
        .map(|summary| text(summary, "batchEvaluationId").to_string())
        .collect();
    assert_eq!(
        order, newest_first,
        "expected the default page to carry the whole list, newest first, got {order:?}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        &Value::Null,
        "expected no nextToken on a page that carries the whole list, got {body:?}"
    );
}

/// Every summary a listing reports is the job that was started, and it agrees with a
/// `GetBatchEvaluation` of the same job — for every shape of request the model admits.
#[tokio::test]
async fn a_listed_batch_evaluation_summary_agrees_with_the_read() {
    let (router, _engine) = emulator();

    let cases = round_trip_cases();
    let mut expected_ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let (status, started) = start_batch_evaluation(&router, request.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );
        expected_ids.push(text(&started, "batchEvaluationId").to_string());
    }

    let summaries = list_every_batch_evaluation(&router, 100).await;
    assert_eq!(
        summaries.len(),
        cases.len(),
        "expected one summary per started job, got {summaries:?}"
    );

    for ((case, request), id) in cases.iter().zip(&expected_ids) {
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "batchEvaluationId") == id.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: expected {id:?} to be listed, got {summaries:?}"));
        let (status, read) = get_batch_evaluation(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading {id:?}, got {status} with {read:?}"
        );
        // The members `BatchEvaluationSummary` shares with the full report are the same
        // values (source: smithy_query query="shape" shape="BatchEvaluationSummary").
        for name in [
            "batchEvaluationId",
            "batchEvaluationArn",
            "batchEvaluationName",
            "status",
            "createdAt",
            "updatedAt",
            "description",
            "evaluators",
            "insights",
            "kmsKeyArn",
        ] {
            assert_eq!(
                member(summary, name),
                member(&read, name),
                "case {case:?}: expected the listed {name} to agree with the read of {id:?}, got {summary:?}"
            );
        }
        // What the request asked for is what the summary carries.
        for name in ["description", "evaluators", "insights", "kmsKeyArn"] {
            assert_eq!(
                member(summary, name),
                request.get(name).unwrap_or(&Value::Null),
                "case {case:?}: expected the listed {name} to be the one the start asked for, got {summary:?}"
            );
        }
        assert_eq!(
            text(summary, "batchEvaluationName"),
            request
                .get("batchEvaluationName")
                .and_then(Value::as_str)
                .expect("every case names its job"),
            "case {case:?}: expected the name the start asked for, got {summary:?}"
        );
        // No judge model runs here, so no summary carries a rating or a failure.
        for name in ["evaluationResults", "errorDetails"] {
            assert_eq!(
                member(summary, name),
                &Value::Null,
                "case {case:?}: expected no {name} for a job nothing scored, got {summary:?}"
            );
        }
    }
}

/// A listing is a READ: an emulator holding no job lists none rather than failing, two
/// listings report the same page, and nothing of the listing is stored.
#[tokio::test]
async fn listing_batch_evaluations_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();

    let (status, empty) = list_batch_evaluations(&router, "maxResults=100").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from a listing with nothing to list, got {status} with {empty:?}"
    );
    assert!(
        page(&empty).is_empty(),
        "expected no summaries before anything was started, got {empty:?}"
    );
    assert_eq!(
        member(&empty, "nextToken"),
        &Value::Null,
        "expected no nextToken on an empty listing, got {empty:?}"
    );

    let (_, started) = start_batch_evaluation(&router, minimal_request("read_only_check")).await;
    let id = text(&started, "batchEvaluationId").to_string();
    let (_, first) = list_batch_evaluations(&router, "maxResults=100").await;
    let (_, again) = list_batch_evaluations(&router, "maxResults=100").await;
    assert_eq!(
        first, again,
        "expected a second listing to report the same page, got {again:?}"
    );
    assert_eq!(
        batch_evaluation_count(&engine),
        1,
        "expected listing to store nothing of its own"
    );
    assert_eq!(
        text(&first["batchEvaluations"][0], "batchEvaluationId"),
        id,
        "expected the started job to be the one listed, got {first:?}"
    );
}

/// A listed `BatchEvaluationStatus` is the one the clock implies, and the listing writes
/// nothing to make the transition: a row per point of the lifecycle.
#[tokio::test]
async fn a_listed_batch_evaluation_status_follows_the_clock() {
    let span = agentcore::state::BATCH_EVALUATION_TRANSITION_SPAN;
    // How far past the start instant each row lists the job, and what the lifecycle says
    // it is then: both sides of each span boundary, and one point well past the last.
    let cases: [(&str, Duration, &str); 5] = [
        ("just started", Duration::ZERO, "PENDING"),
        ("one second short of running", span - Duration::from_secs(1), "PENDING"),
        ("at the first span", span, "IN_PROGRESS"),
        ("at the second span", span * 2, "COMPLETED"),
        ("long past the run", span * 200, "COMPLETED"),
    ];

    for (case, aged_by, expected_status) in cases {
        let (router, engine) = emulator();
        let (status, started) =
            start_batch_evaluation(&router, minimal_request("clock_listing")).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from StartBatchEvaluation, got {status} with {started:?}"
        );
        let id = text(&started, "batchEvaluationId").to_string();
        age_batch_evaluation(&engine, &id, aged_by);

        // Two listings in a row: the status is read off the clock, so neither advances
        // the job.
        for listing_number in 1..=2 {
            let (status, body) = list_batch_evaluations(&router, "maxResults=100").await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from listing {listing_number}, got {status} with {body:?}"
            );
            assert_eq!(
                text(&body["batchEvaluations"][0], "status"),
                expected_status,
                "case {case:?}: expected status {expected_status} on listing {listing_number}, got {body:?}"
            );
        }
        // And the read of the same job says the same thing.
        let (_, read) = get_batch_evaluation(&router, &id).await;
        assert_eq!(
            text(&read, "status"),
            expected_status,
            "case {case:?}: expected the read to agree with the listing, got {read:?}"
        );
    }
}

/// A `maxResults` outside the range the model publishes, and a `nextToken` this listing
/// never issued, are refused 400 `ValidationException`; the published bounds themselves
/// are served.
#[tokio::test]
async fn a_batch_evaluation_listing_outside_its_modelled_input_is_refused_with_400() {
    let (router, engine) = emulator();
    let (status, started) = start_batch_evaluation(&router, minimal_request("bounds")).await;
    assert_eq!(
        status, 202,
        "expected the job under the bounds cases to be started, got {status} with {started:?}"
    );

    // `maxResults` is range 1..100 (source: smithy_query query="operation"
    // operation="ListBatchEvaluations"): one case on each side of it, and a negative one.
    let refused: [(&str, &str); 5] = [
        ("max-results-below-the-range", "maxResults=0"),
        ("max-results-above-the-range", "maxResults=101"),
        ("negative-max-results", "maxResults=-1"),
        ("next-token-this-listing-never-issued", "nextToken=not-a-token"),
        // A token another paginated listing of this service minted does not resume this
        // one: `nextToken` carries the list it belongs to.
        ("next-token-of-another-listing", "nextToken=YWItdGVzdHM6MA=="),
    ];
    for (case, query) in refused {
        let (status, body) = list_batch_evaluations(&router, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    for (case, query) in [
        ("lowest-max-results", "maxResults=1"),
        ("highest-max-results", "maxResults=100"),
    ] {
        let (status, body) = list_batch_evaluations(&router, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a published bound to be served, got {status} with {body:?}"
        );
        assert_eq!(
            page(&body).len(),
            1,
            "case {case:?}: expected the one started job on the page, got {body:?}"
        );
    }

    // A refused listing stored nothing and removed nothing.
    assert_eq!(
        batch_evaluation_count(&engine),
        1,
        "expected the refusals above to have changed nothing"
    );
}
