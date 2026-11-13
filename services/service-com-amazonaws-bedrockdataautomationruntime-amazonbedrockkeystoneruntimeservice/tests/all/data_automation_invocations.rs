// SPDX-License-Identifier: CC0-1.0

//! The asynchronous data-automation invocation slice, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same awsJson1_1 path a real SDK takes: `POST /` with
//! `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.<Op>`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `InvokeDataAutomationAsync` mints and PERSISTS one invocation per accepted
//!   submission and answers with that record's own ARN; `clientToken` is accepted for
//!   request validation and deduplicates nothing, so a repeated token mints a second,
//!   independently tracked job.
//! * `GetDataAutomationStatus` computes its answer from that record, and the status
//!   progression is owed by the clock, so repeated reads never disagree.
//! * The three tagging operations edit and report the stored tag map of the invocation
//!   their `resourceARN` names.
//! * An ARN naming no stored invocation is `ResourceNotFoundException`, answered with
//!   HTTP 400 rather than the 404 the shape's advisory `@httpError` declares; an ARN
//!   outside the modelled pattern or length is `ValidationException` (400).
//! * A submission must name EXACTLY one extraction target — a data-automation project or
//!   an inline blueprint. Both members are optional in the model, so the rule is the
//!   service's, and both invoke operations refuse a submission naming neither, or both,
//!   with `ValidationException` (400).
//! * Every tag key must satisfy the modelled `TagKey` pattern, including the negative
//!   lookahead the framework's regex engine cannot compile, so the reserved `aws:`
//!   namespace and the character class are both enforced by the three operations that
//!   accept a tag key.
//! * A refusal the framework's boundary walk raises is stated in this service's own
//!   wording: the `ValidationException` envelope, a clause that names the member and the
//!   constraint without echoing the offending value, and the declared pattern printed
//!   without its anchors.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

use bda::state::{AutomationJobStatus, InputAsset, Invocation, CREATED_FOR, IN_PROGRESS_FOR};

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for. The handler reads it out of the SigV4
/// credential scope, so it is the region in every ARN these tests expect.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str = "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn` (pattern from
/// `smithy_query` shape `DataAutomationProjectArn`).
///
/// Every accepted submission below names either this project or an inline blueprint:
/// one of the two is what tells the service WHAT to extract, and a submission carrying
/// neither is refused (see
/// `a_submission_naming_neither_a_project_nor_a_blueprint_is_refused`).
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices";

/// The modelled `AutomationJobStatus` values, as `AUTOMATION_JOB_STATUS_VALUES` lists
/// them.
const JOB_STATUS_VALUES: &[&str] = &[
    "ClientError",
    "Created",
    "InProgress",
    "ServiceError",
    "Success",
];

/// One row of the submission table: the case name, the request body, the output prefix
/// the submission names, and the tags the resource must then carry.
type SubmissionCase = (
    &'static str,
    Value,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// One row of the tag-edit script: the case name, the operation to call, its request
/// body, and the whole tag map the resource must report afterwards.
type TagEditCase = (
    &'static str,
    &'static str,
    Value,
    Vec<(&'static str, &'static str)>,
);

/// One ARN-keyed operation under test: its name, and how it carries the ARN.
type ArnKeyedOperation = (&'static str, fn(&str) -> Value);

/// A router serving only this service over one shared engine — what the production
/// router does for this crate, minus every other service.
///
/// [`PROJECT_ARN`] is arranged in the store of the service that OWNS data-automation
/// projects first: a submission's project reference is resolved against it (see
/// `control_plane_reference_resolution`), so a submission naming a project nobody created
/// is refused before any rule this suite is about can be reached.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "data-automation-invoices");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }
}

/// Call one operation on `router` the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Submit one invocation and return its `invocationArn`.
async fn submit(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let answer = call(router, "InvokeDataAutomationAsync", body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
        .to_string()
}

/// The smallest legal submission body, parameterised by the output prefix.
///
/// "Legal" includes the cross-member rule the model cannot express: a submission must
/// name a data-automation project or an inline blueprint, so the project is part of the
/// minimum even though the model marks `dataAutomationConfiguration` optional.
fn minimal_submission(output_s3_uri: &str) -> Value {
    json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
        "outputConfiguration": {"s3Uri": output_s3_uri},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    })
}

/// Check `arn` against the modelled `InvocationArn` shape — pattern
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$`
/// and `@length(1..128)`, from `smithy_query` shape `InvocationArn`. Checked
/// structurally so the test needs no regex engine.
fn assert_invocation_arn(case: &str, arn: &str) {
    let prefix = format!(
        "arn:aws:bedrock:{REGION}:{}:data-automation-invocation/",
        emulator_server::EMULATOR_ACCOUNT_ID
    );
    let Some(id) = arn.strip_prefix(prefix.as_str()) else {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}");
    };
    assert!(
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {case:?}: the resource id must match [a-zA-Z0-9-_]+, got {id:?}"
    );
    assert!(
        !arn.is_empty() && arn.len() <= 128,
        "case {case:?}: InvocationArn is @length(1..128), got {} chars in {arn:?}",
        arn.len()
    );
}

/// A timestamp from a response as seconds since the epoch. The member carries
/// `timestampFormat: date-time`, but accept the epoch-seconds form too so the assertion
/// is about the instant, not the encoding.
fn epoch_seconds(case: &str, value: &Value) -> f64 {
    match value {
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| panic!("case {case:?}: {text:?} is not an RFC 3339 timestamp: {e}"))
        .as_secs_f64(),
        Value::Number(number) => number
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {number} is not a number of seconds")),
        other => panic!("case {case:?}: expected a timestamp, got {other}"),
    }
}

/// The tags `ListTagsForResource` reports, as (key, value) pairs in the order answered.
async fn list_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} / {}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no tags array in {}", answer.body))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Rank of a status in the progression, for the monotonicity assertion.
fn progression_rank(status: &str) -> usize {
    match status {
        "Created" => 0,
        "InProgress" => 1,
        "Success" => 2,
        other => panic!("{other:?} is not part of the success progression"),
    }
}

/// A fixed submission instant, so the clock table below is deterministic.
fn fixed_submission_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// An invocation submitted at [`fixed_submission_time`], for the pure clock tests.
fn fixed_invocation() -> Invocation {
    Invocation::new(
        "arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/fixture".to_string(),
        "fixture".to_string(),
        "s3://in-bucket/assets/doc.pdf".to_string(),
        "s3://out-bucket/results/".to_string(),
        PROFILE_ARN.to_string(),
        fixed_submission_time(),
    )
}

#[test]
fn advance_to_materialises_only_the_transitions_the_clock_owes() {
    let submitted = fixed_submission_time();
    let in_progress_due = CREATED_FOR;
    let success_due = CREATED_FOR + IN_PROGRESS_FOR;

    // Every row is "how long after the submission the job is observed" -> the status the
    // clock owes it by then, and whether a completion time is owed with it.
    let rows: &[(&str, Duration, AutomationJobStatus, bool)] = &[
        ("observed at the submission instant", Duration::ZERO, AutomationJobStatus::Created, false),
        ("one tick before InProgress is due", in_progress_due - Duration::from_millis(1), AutomationJobStatus::Created, false),
        ("exactly when InProgress is due", in_progress_due, AutomationJobStatus::InProgress, false),
        ("one tick before Success is due", success_due - Duration::from_millis(1), AutomationJobStatus::InProgress, false),
        ("exactly when Success is due", success_due, AutomationJobStatus::Success, true),
        ("long after Success is due", success_due + Duration::from_secs(3_600), AutomationJobStatus::Success, true),
    ];

    for (case, after, want_status, want_completion) in rows {
        let mut invocation = fixed_invocation();
        invocation.advance_to(submitted + *after, InputAsset::Available);
        assert_eq!(
            invocation.status, *want_status,
            "case {case:?}: expected {want_status:?} {after:?} after submission, got {:?}",
            invocation.status
        );
        assert_eq!(
            invocation.job_completion_time.is_some(),
            *want_completion,
            "case {case:?}: completion time presence wrong, got {:?}",
            invocation.job_completion_time
        );
        if *want_completion {
            // The instant stored is the instant the transition became DUE, so it does
            // not depend on when a caller happened to look.
            assert_eq!(
                invocation.job_completion_time,
                Some(submitted + success_due),
                "case {case:?}: the completion instant must be when Success fell due, got {:?}",
                invocation.job_completion_time
            );
        }
    }

    // Idempotence of the materialisation: once a transition is recorded, re-applying the
    // clock — even at an earlier instant — never moves the job back.
    let mut invocation = fixed_invocation();
    invocation.advance_to(submitted + success_due, InputAsset::Available);
    let completed_at = invocation.job_completion_time;
    for case in ["re-read at the submission instant", "re-read far in the future"] {
        let now = if case.contains("submission") {
            submitted
        } else {
            submitted + Duration::from_secs(86_400)
        };
        invocation.advance_to(now, InputAsset::Available);
        assert_eq!(
            invocation.status,
            AutomationJobStatus::Success,
            "case {case:?}: a recorded transition must not be undone"
        );
        assert_eq!(
            invocation.job_completion_time, completed_at,
            "case {case:?}: the completion instant must stay put"
        );
    }
}

#[test]
fn job_duration_in_seconds_is_derived_from_the_two_stored_timestamps() {
    let submitted = fixed_submission_time();

    // Whole seconds between submission and completion, truncated — the duration is never
    // stored, so each row only sets the two timestamps.
    let rows: &[(&str, Option<Duration>, Option<i32>)] = &[
        ("still running", None, None),
        ("completed inside the first second", Some(Duration::from_millis(600)), Some(0)),
        ("completed just past one second", Some(Duration::from_millis(1_500)), Some(1)),
        ("completed after a minute and a half", Some(Duration::from_secs(90)), Some(90)),
        ("completed after a day", Some(Duration::from_secs(86_400)), Some(86_400)),
    ];

    for (case, elapsed, want) in rows {
        let mut invocation = fixed_invocation();
        invocation.job_completion_time = elapsed.map(|elapsed| submitted + elapsed);
        assert_eq!(
            invocation.job_duration_in_seconds(),
            *want,
            "case {case:?}: expected {want:?} seconds for an elapsed {elapsed:?}, got {:?}",
            invocation.job_duration_in_seconds()
        );
    }
}

#[tokio::test]
async fn each_submission_is_persisted_and_reported_back_from_state() {
    let router = service();

    // One row per shape of submission the model allows: the bare required members, and
    // then each optional member group added. What differs between rows is what the
    // operation must persist, which is what the two readers below report.
    let rows: Vec<SubmissionCase> = vec![
        (
            "the required members plus the project that says what to extract",
            minimal_submission("s3://out-bucket/plain/"),
            "s3://out-bucket/plain/",
            vec![],
        ),
        (
            "a project configuration and one tag",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/scan.tiff"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/project/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {
                    "dataAutomationProjectArn": PROJECT_ARN,
                    "stage": "LIVE",
                },
                "tags": [{"key": "env", "value": "dev"}],
            }),
            "s3://out-bucket/project/",
            vec![("env", "dev")],
        ),
        (
            "blueprints, encryption, notification and three tags",
            json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/clip.mp4"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/deep/nested/prefix/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "blueprints": [{
                    "blueprintArn": "arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-video",
                    "stage": "LIVE",
                }],
                "encryptionConfiguration": {"kmsKeyId": "alias/data-automation"},
                "notificationConfiguration": {"eventBridgeConfiguration": {"eventBridgeEnabled": true}},
                "tags": [
                    {"key": "team", "value": "search"},
                    {"key": "env", "value": "prod"},
                    {"key": "cost-center", "value": "4711"},
                ],
            }),
            "s3://out-bucket/deep/nested/prefix/",
            vec![("cost-center", "4711"), ("env", "prod"), ("team", "search")],
        ),
        (
            "an output uri that is a bucket root",
            minimal_submission("s3://out-bucket"),
            "s3://out-bucket",
            vec![],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body, want_output_prefix, want_tags) in &rows {
        let before = SystemTime::now();
        let arn = submit(&router, case, body.clone()).await;
        let after = SystemTime::now();
        assert_invocation_arn(case, &arn);
        minted.push(arn.clone());

        let answer = call(&router, "GetDataAutomationStatus", json!({"invocationArn": arn})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["outputConfiguration"]["s3Uri"].as_str(),
            Some(crate::support::job_metadata_uri(want_output_prefix, &arn).as_str()),
            "case {case:?}: the reported outputConfiguration must be this job's own metadata \
             location under the submitted prefix, got {}",
            answer.body
        );
        let status = answer.body["status"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: {status:?} is not a modelled AutomationJobStatus"
        );

        // The submission time is the instant this call was accepted, so it falls inside
        // the window the call itself spanned.
        let submitted = epoch_seconds(case, &answer.body["jobSubmissionTime"]);
        let low = before
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs_f64();
        let high = after
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs_f64();
        assert!(
            submitted >= low - 1.0 && submitted <= high + 1.0,
            "case {case:?}: jobSubmissionTime {submitted} is outside the submit window {low}..{high}"
        );

        let tags = list_tags(&router, case, &arn).await;
        let want: Vec<(String, String)> = want_tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags, want,
            "case {case:?}: ListTagsForResource must report exactly the submitted tags"
        );
    }

    // Distinct submissions are distinct jobs: no two share an ARN.
    let mut sorted = minted.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        minted.len(),
        "each submission must mint its own invocationArn, got {minted:?}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_mints_a_second_independent_job() {
    let router = service();

    // `clientToken` is `@idempotencyToken` on an `@idempotent` operation, which is a
    // statement about the TOKEN rather than a dedup key this operation applies: the token
    // is accepted for request validation and every accepted submission mints its own job.
    // Rows differ in the token's shape, which the modelled pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){1,256}$` allows.
    let tokens = ["tok-1", "a1", "Zz9", "many-dashes-in-here"];

    let mut minted: Vec<String> = Vec::new();
    for token in tokens {
        let body = json!({
            "clientToken": token,
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/first/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        });
        // The identical request twice, under the identical token: two jobs.
        let first = submit(&router, token, body.clone()).await;
        let second = submit(&router, token, body).await;
        assert_ne!(
            first, second,
            "case {token:?}: a repeated clientToken must mint a second invocation, got \
             {first:?} twice"
        );

        // And a third submission under the same token, carrying DIFFERENT parameters, is
        // a third job — the token replays nothing, so each ARN resolves to the job that
        // minted it and reports the prefix ITS submission named.
        let third_prefix = "s3://out-bucket/second/";
        let third = submit(
            &router,
            token,
            json!({
                "clientToken": token,
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/other.pdf"},
                "outputConfiguration": {"s3Uri": third_prefix},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            }),
        )
        .await;

        for (arn, prefix) in [
            (&first, "s3://out-bucket/first/"),
            (&second, "s3://out-bucket/first/"),
            (&third, third_prefix),
        ] {
            let answer = call(
                &router,
                "GetDataAutomationStatus",
                json!({"invocationArn": arn}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {token:?}: every minted ARN must resolve, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["outputConfiguration"]["s3Uri"].as_str(),
                Some(crate::support::job_metadata_uri(prefix, arn).as_str()),
                "case {token:?}: {arn:?} must report its own submission's output location, \
                 got {}",
                answer.body
            );
        }
        minted.extend([first, second, third]);
    }

    // No token at all: the SDK would have filled one in, so two bare calls are two jobs.
    minted.push(submit(&router, "no token (a)", minimal_submission("s3://out-bucket/a/")).await);
    minted.push(submit(&router, "no token (b)", minimal_submission("s3://out-bucket/b/")).await);

    // Every submission above is its own job: no two of them share an ARN, whether they
    // carried one token, another token, or none.
    let submissions = minted.len();
    minted.sort();
    minted.dedup();
    assert_eq!(
        minted.len(),
        submissions,
        "each submission must mint its own invocationArn, got {minted:?}"
    );
}

#[tokio::test]
async fn status_reads_agree_with_each_other_and_reach_success() {
    let router = service();
    let case = "a single submission polled to completion";
    let arn = submit(&router, case, minimal_submission("s3://out-bucket/poll/")).await;

    let read = |arn: String| {
        let router = &router;
        async move {
            let answer = call(
                router,
                "GetDataAutomationStatus",
                json!({"invocationArn": arn}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
                answer.status, answer.body
            );
            answer.body
        }
    };

    // The first read happens straight after the submit. That the job is NOT born
    // finished is pinned deterministically by
    // `advance_to_materialises_only_the_transitions_the_clock_owes`, with injected
    // instants; over the wire the assertion is the invariant that holds whatever the
    // machine's load did to the 300 ms window — a job without a terminal status reports
    // neither a completion time nor a duration.
    let first = read(arn.clone()).await;
    let first_status = first["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no status in {first}"))
        .to_string();
    assert!(
        JOB_STATUS_VALUES.contains(&first_status.as_str()),
        "case {case:?}: {first_status:?} is not a modelled AutomationJobStatus"
    );
    if first_status != "Success" {
        assert!(
            first.get("jobCompletionTime").is_none_or(Value::is_null),
            "case {case:?}: a job in {first_status:?} has no completion time, got {first}"
        );
        assert!(
            first.get("jobDurationInSeconds").is_none_or(Value::is_null),
            "case {case:?}: a job in {first_status:?} has no duration, got {first}"
        );
    }

    // Poll until the clock owes Success. The observed statuses may skip a step, but they
    // must never go backwards.
    let mut observed = vec![first_status];
    let deadline = SystemTime::now() + Duration::from_secs(10);
    let mut final_body = first;
    while SystemTime::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        final_body = read(arn.clone()).await;
        let status = final_body["status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no status in {final_body}"))
            .to_string();
        if observed.last() != Some(&status) {
            observed.push(status.clone());
        }
        if status == "Success" {
            break;
        }
    }
    assert!(
        observed
            .windows(2)
            .all(|pair| progression_rank(&pair[0]) < progression_rank(&pair[1])),
        "case {case:?}: the status must only move forwards, observed {observed:?}"
    );
    assert_eq!(
        final_body["status"].as_str(),
        Some("Success"),
        "case {case:?}: the job must reach Success, observed {observed:?}"
    );

    // Once terminal, the record is fixed: every later read answers identically.
    let submitted = epoch_seconds(case, &final_body["jobSubmissionTime"]);
    let completed = epoch_seconds(case, &final_body["jobCompletionTime"]);
    let duration = final_body["jobDurationInSeconds"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no jobDurationInSeconds in {final_body}"));
    assert!(
        completed > submitted,
        "case {case:?}: completion {completed} must follow submission {submitted}"
    );
    #[allow(clippy::cast_possible_truncation)]
    let want_duration = (completed - submitted).floor() as i64;
    assert_eq!(
        duration, want_duration,
        "case {case:?}: jobDurationInSeconds must be the whole seconds between {submitted} and {completed}"
    );
    for round in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let again = read(arn.clone()).await;
        assert_eq!(
            again["status"], final_body["status"],
            "case {case:?} re-read {round}: a read must not change what a later caller sees"
        );
        assert_eq!(
            again["jobSubmissionTime"], final_body["jobSubmissionTime"],
            "case {case:?} re-read {round}: the submission time must be stable"
        );
        assert_eq!(
            again["jobCompletionTime"], final_body["jobCompletionTime"],
            "case {case:?} re-read {round}: the completion time must be stable"
        );
        assert_eq!(
            again["jobDurationInSeconds"], final_body["jobDurationInSeconds"],
            "case {case:?} re-read {round}: the derived duration must be stable"
        );
    }
}

#[tokio::test]
async fn every_arn_keyed_operation_refuses_unknown_and_malformed_arns() {
    let router = service();
    let account = emulator_server::EMULATOR_ACCOUNT_ID;

    // The four operations that resolve a caller-supplied invocation ARN, with the member
    // each one carries it in.
    let operations: &[ArnKeyedOperation] = &[
        ("GetDataAutomationStatus", |arn| {
            json!({"invocationArn": arn})
        }),
        ("ListTagsForResource", |arn| json!({"resourceARN": arn})),
        ("TagResource", |arn| {
            json!({"resourceARN": arn, "tags": [{"key": "env", "value": "dev"}]})
        }),
        ("UntagResource", |arn| {
            json!({"resourceARN": arn, "tagKeys": ["env"]})
        }),
    ];

    // Well formed under the modelled pattern, but naming nothing this emulator minted.
    // Rows differ in region, partition and resource-id shape.
    let unknown = [
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-invocation/does-not-exist"),
        format!("arn:aws:bedrock:eu-west-1:{account}:data-automation-invocation/0"),
        format!("arn:aws-us-gov:bedrock:us-gov-west-1:{account}:data-automation-invocation/a_B-9"),
        format!("arn:aws-cn:bedrock:cn-north-1:{account}:data-automation-invocation/{}", "x".repeat(40)),
    ];
    // Outside the modelled pattern or length: refused by the boundary, before any
    // lookup. Rows differ in which part of the ARN is wrong.
    let malformed = [
        "not-an-arn".to_string(),
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-project/p"),
        format!("arn:aws:bedrock:{REGION}:12345:data-automation-invocation/x"),
        format!("arn:aws:bedrock:{REGION}:{account}:data-automation-invocation/"),
        format!("arn:aws:s3:{REGION}:{account}:data-automation-invocation/x"),
    ];

    for (operation, build) in operations {
        for arn in &unknown {
            let case = format!("{operation} on unknown {arn}");
            let answer = call(&router, operation, build(arn)).await;
            // 400, not the 404 the `ResourceNotFoundException` shape's advisory
            // `@httpError` declares: on this awsJson1_1 service the status says only
            // "the caller is at fault" and the body's `__type` says which fault.
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                // Spelled out rather than read off the crate, so a wording change fails
                // here: this is the service's own fixed not-found string.
                Some("Data Automation Invocation ARN not found."),
                "case {case:?}: the refusal must carry this service's own not-found \
                 message and nothing from the request, got {}",
                answer.body
            );
        }
        for arn in &malformed {
            let case = format!("{operation} on malformed {arn}");
            let answer = call(&router, operation, build(arn)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            // This service's own wording for a boundary refusal: the AWS
            // `ValidationException` envelope, a clause naming the member and the
            // constraint, the declared pattern printed WITHOUT its anchors, and no echo
            // of the offending value.
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("validation error")
                    && (message.contains("'resourceARN'") || message.contains("'invocationArn'")),
                "case {case:?}: the clause must sit in the validation envelope and name \
                 the member, got {message:?}"
            );
            assert!(
                !message.contains("Value '"),
                "case {case:?}: the clause must not echo the offending value, got \
                 {message:?}"
            );
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: the refusal must not echo the submitted value, got \
                 {message:?}"
            );
            let Some((_, pattern)) =
                message.split_once("Member must satisfy regular expression pattern: ")
            else {
                panic!("case {case:?}: no pattern clause in {message:?}");
            };
            assert!(
                !pattern.starts_with('^'),
                "case {case:?}: the pattern must be printed without its anchors, got \
                 {pattern:?}"
            );
        }
    }
}

#[tokio::test]
async fn tag_and_untag_edit_the_stored_tag_map() {
    let router = service();
    let case = "tag edits on one invocation";
    let arn = submit(
        &router,
        case,
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/tags/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            "tags": [{"key": "env", "value": "dev"}],
        }),
    )
    .await;

    // Each row is one edit applied to the tag map built by the rows before it, and the
    // whole map the resource must then report.
    let rows: Vec<TagEditCase> = vec![
        (
            "a new key is added and an existing key takes the new value",
            "TagResource",
            json!({"resourceARN": arn, "tags": [
                {"key": "team", "value": "search"},
                {"key": "env", "value": "prod"},
            ]}),
            vec![("env", "prod"), ("team", "search")],
        ),
        (
            "tagging with an empty list changes nothing",
            "TagResource",
            json!({"resourceARN": arn, "tags": []}),
            vec![("env", "prod"), ("team", "search")],
        ),
        (
            "an empty tag value is a value, not an absence",
            "TagResource",
            json!({"resourceARN": arn, "tags": [{"key": "note", "value": ""}]}),
            vec![("env", "prod"), ("note", ""), ("team", "search")],
        ),
        (
            "untagging removes exactly the named keys",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["team", "note"]}),
            vec![("env", "prod")],
        ),
        (
            "untagging a key the resource does not carry is a no-op success",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["never-set"]}),
            vec![("env", "prod")],
        ),
        (
            "untagging every key leaves an empty tag list",
            "UntagResource",
            json!({"resourceARN": arn, "tagKeys": ["env"]}),
            vec![],
        ),
        (
            "a key removed can be set again",
            "TagResource",
            json!({"resourceARN": arn, "tags": [{"key": "env", "value": "staging"}]}),
            vec![("env", "staging")],
        ),
    ];

    for (row, operation, body, want) in &rows {
        let answer = call(&router, operation, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected 200 from {operation}, got {} / {}",
            answer.status, answer.body
        );
        let tags = list_tags(&router, row, &arn).await;
        let expected: Vec<(String, String)> = want
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags, expected,
            "case {row:?}: ListTagsForResource must report the edited map"
        );
        let mut sorted = tags.clone();
        sorted.sort();
        assert_eq!(
            tags, sorted,
            "case {row:?}: tags must be reported in a stable key order"
        );
    }
}

#[tokio::test]
async fn tagging_past_the_modelled_tag_bound_is_refused_and_changes_nothing() {
    let router = service();
    let case = "a resource already carrying the maximum number of tags";

    // The modelled `TagList` bound is `@length(min: 0, max: 200)` (smithy_query shape
    // `TagList`), so a resource can hold 200 tags and no more.
    let full: Vec<Value> = (0..200)
        .map(|i| json!({"key": format!("key{i:03}"), "value": format!("value{i:03}")}))
        .collect();
    let arn = submit(
        &router,
        case,
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/quota/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            "tags": full,
        }),
    )
    .await;
    assert_eq!(
        list_tags(&router, case, &arn).await.len(),
        200,
        "case {case:?}: the submission must persist all 200 tags"
    );

    // Overwriting an existing key keeps the count at the bound, so it is allowed.
    let overwrite = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": "key000", "value": "rewritten"}]}),
    )
    .await;
    assert_eq!(
        overwrite.status, 200,
        "case {case:?}: overwriting a key does not grow the map, got {} / {}",
        overwrite.status, overwrite.body
    );
    let tags = list_tags(&router, case, &arn).await;
    assert_eq!(tags.len(), 200, "case {case:?}: the count must stay at 200");
    assert!(
        tags.contains(&("key000".to_string(), "rewritten".to_string())),
        "case {case:?}: the overwrite must be visible"
    );

    // One more distinct key would take it past the bound.
    let over = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": "one-too-many", "value": "x"}]}),
    )
    .await;
    assert_eq!(
        over.status, 402,
        "case {case:?}: expected 402, got {} / {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {}",
        over.body
    );
    let after = list_tags(&router, case, &arn).await;
    assert_eq!(
        after, tags,
        "case {case:?}: a refused TagResource must leave the stored tags untouched"
    );
}

#[test]
fn the_invocation_store_is_bounded_and_retires_finished_jobs_first() {
    use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState as Store;

    // `now` is an hour past the fixture epoch: a record submitted back then has long
    // since finished, while one submitted AT `now` is still `Created`.
    let epoch = fixed_submission_time();
    let now = epoch + Duration::from_secs(3_600);

    /// One invocation record with index `i`, submitted at `submitted`, carrying its own
    /// `clientToken` — the shape `InvokeDataAutomationAsync` admits.
    fn record(i: usize, submitted: SystemTime) -> Invocation {
        let arn =
            format!("arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/job{i:05}");
        let mut invocation = Invocation::new(
            arn,
            format!("job{i:05}"),
            "s3://in-bucket/assets/doc.pdf".to_string(),
            "s3://out-bucket/results/".to_string(),
            PROFILE_ARN.to_string(),
            submitted,
        );
        invocation.client_token = Some(format!("token{i:05}"));
        invocation
    }

    /// Fill `store` to capacity through the ONE admission path. The first `running`
    /// records are submitted at `now`, so they are still pending; the rest are submitted
    /// in the past in index order, so index `running` is the longest-submitted finished
    /// record. Returns the ARNs in index order.
    fn fill(store: &mut Store, running: usize, epoch: SystemTime, now: SystemTime) -> Vec<String> {
        let mut arns = Vec::new();
        for i in 0..bda::state::MAX_INVOCATIONS {
            let submitted = if i < running {
                now
            } else {
                epoch + Duration::from_millis(i as u64)
            };
            let invocation = record(i, submitted);
            let arn = invocation.invocation_arn.clone();
            assert!(
                store.admit_invocation(invocation, now),
                "filling below capacity must always find a slot (record {i})"
            );
            arns.push(arn);
        }
        arns
    }

    // Rows differ in how many of the records at capacity are still running: the policy
    // must retire the longest-submitted FINISHED record, and refuse only when there is
    // no finished record to retire.
    let rows: &[(&str, usize, bool)] = &[
        ("every record finished", 0, true),
        ("the first record still running", 1, true),
        ("all but one still running", bda::state::MAX_INVOCATIONS - 1, true),
        ("every record still running", bda::state::MAX_INVOCATIONS, false),
    ];

    for (case, running, want_room) in rows {
        let mut store = Store::default();
        let arns = fill(&mut store, *running, epoch, now);
        assert_eq!(
            store.invocation_count(),
            bda::state::MAX_INVOCATIONS,
            "case {case:?}: the fixture must start at capacity"
        );

        // One more submission, at capacity: it is admitted only if a finished record can
        // be retired to make room for it.
        let newcomer = record(bda::state::MAX_INVOCATIONS, now);
        let newcomer_arn = newcomer.invocation_arn.clone();
        let admitted = store.admit_invocation(newcomer, now);
        assert_eq!(
            admitted, *want_room,
            "case {case:?}: expected admit_invocation to answer {want_room}"
        );

        // Whatever the answer, the store may not exceed the cap.
        assert!(
            store.invocation_count() <= bda::state::MAX_INVOCATIONS,
            "case {case:?}: invocations must stay bounded, got {}",
            store.invocation_count()
        );

        if *want_room {
            assert_eq!(
                store.invocation_count(),
                bda::state::MAX_INVOCATIONS,
                "case {case:?}: exactly one record must be retired to seat the newcomer"
            );
            assert!(
                store.invocation(&newcomer_arn).is_some(),
                "case {case:?}: the admitted record must be retained"
            );
            // The retired record is the longest-submitted finished one — the first
            // record that was not left running.
            let retired = &arns[*running];
            assert!(
                store.invocation(retired).is_none(),
                "case {case:?}: {retired:?} was the longest-submitted finished record"
            );
        } else {
            assert_eq!(
                store.invocation_count(),
                bda::state::MAX_INVOCATIONS,
                "case {case:?}: a job still running must never be dropped"
            );
            assert!(
                store.invocation(&newcomer_arn).is_none(),
                "case {case:?}: a refused submission must insert nothing"
            );
        }
    }
}

#[test]
fn a_stream_of_submissions_cannot_grow_the_store_past_its_bound() {
    use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState as Store;

    // The adversarial pattern the bound exists for: a stream of submissions, each its own
    // job — which is what every submission is, under a fresh `clientToken`, a repeated one
    // or none at all. The store must stay at or below the cap however long the stream
    // runs, and the records it retains must be the newest ones.
    let epoch = fixed_submission_time();
    let submissions = bda::state::MAX_INVOCATIONS + 250;
    let mut store = Store::default();
    let mut arns = Vec::new();

    for i in 0..submissions {
        // Each record is submitted far enough in the past to have finished, so the
        // retirement policy always has a candidate; `now` keeps pace with the stream.
        let submitted = epoch + Duration::from_millis(i as u64);
        let now = submitted + CREATED_FOR + IN_PROGRESS_FOR;
        let arn = format!(
            "arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/stream{i:06}"
        );
        let mut invocation = Invocation::new(
            arn.clone(),
            format!("stream{i:06}"),
            "s3://in-bucket/assets/doc.pdf".to_string(),
            "s3://out-bucket/results/".to_string(),
            PROFILE_ARN.to_string(),
            submitted,
        );
        // Every submission in the stream carries the SAME token, which deduplicates
        // nothing: each one is admitted as its own record.
        invocation.client_token = Some("one-token-for-the-whole-stream".to_string());
        assert!(
            store.admit_invocation(invocation, now),
            "submission {i}: a finished record was available to retire"
        );
        arns.push(arn);

        assert!(
            store.invocation_count() <= bda::state::MAX_INVOCATIONS,
            "submission {i}: invocations grew past the bound, got {}",
            store.invocation_count()
        );
    }

    assert_eq!(
        store.invocation_count(),
        bda::state::MAX_INVOCATIONS,
        "the store must sit exactly at its bound after {submissions} submissions, got {}",
        store.invocation_count()
    );
    // The records retained are the newest `MAX_INVOCATIONS` of the stream: the longest
    // submitted finished record is the one retirement takes.
    let retained = submissions - bda::state::MAX_INVOCATIONS;
    for (i, arn) in arns.iter().enumerate() {
        let stored = store.invocation(arn).is_some();
        assert_eq!(
            stored,
            i >= retained,
            "submission {i}: expected retained={}, got {stored}",
            i >= retained
        );
    }
}

/// One submission shape under the cross-member rule, exercised through BOTH invoke
/// operations.
struct ExtractionTargetCase {
    /// How the row reads in an assertion message.
    name: &'static str,
    /// The `dataAutomationConfiguration` member to send, or `Value::Null` to omit it.
    configuration: Value,
    /// The `blueprints` member to send, or `Value::Null` to omit it.
    blueprints: Value,
    /// The `inputConfiguration` the SYNCHRONOUS row sends: `SyncInputConfiguration`
    /// (`smithy_query` shape) accepts either an `s3Uri` or inline `bytes`, and the rule
    /// under test is independent of which.
    sync_input: Value,
    /// `None` when this shape is accepted; otherwise the `ValidationException` message
    /// both invoke operations answer with.
    refusal: Option<&'static str>,
}

/// An `InvokeDataAutomationAsync` body carrying `case`'s extraction members.
fn async_submission(case: &ExtractionTargetCase) -> Value {
    let mut body = json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
        "outputConfiguration": {"s3Uri": "s3://out-bucket/extraction-rule/"},
        "dataAutomationProfileArn": PROFILE_ARN,
    });
    if !case.configuration.is_null() {
        body["dataAutomationConfiguration"] = case.configuration.clone();
    }
    if !case.blueprints.is_null() {
        body["blueprints"] = case.blueprints.clone();
    }
    body
}

/// An `InvokeDataAutomation` body carrying `case`'s extraction members.
fn sync_submission(case: &ExtractionTargetCase) -> Value {
    let mut body = json!({
        "inputConfiguration": case.sync_input.clone(),
        "dataAutomationProfileArn": PROFILE_ARN,
    });
    if !case.configuration.is_null() {
        body["dataAutomationConfiguration"] = case.configuration.clone();
    }
    if !case.blueprints.is_null() {
        body["blueprints"] = case.blueprints.clone();
    }
    body
}

/// One public blueprint ARN, inside the modelled `BlueprintArn` pattern's
/// `bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}` alternative (`smithy_query`
/// shape `BlueprintArn`).
fn public_blueprint(suffix: &str) -> Value {
    json!({"blueprintArn": format!("arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-{suffix}")})
}

/// The `ValidationException` message both invoke operations answer when a submission
/// names NEITHER a data-automation project nor an inline blueprint. Spelled out rather
/// than read off the crate, so the test pins the wire text.
const MISSING_PROJECT_OR_BLUEPRINTS: &str =
    "At least one of project or inline blueprints must be provided in the request.";

/// The `ValidationException` message both invoke operations answer when a submission
/// names BOTH.
const BOTH_PROJECT_AND_BLUEPRINTS: &str =
    "Only one of project or inline blueprints can be provided in the request.";

#[tokio::test]
async fn a_submission_must_name_exactly_one_of_a_project_or_blueprints() {
    // A data-automation submission has to say WHAT to extract, and it says so in exactly
    // ONE of two optional members: `dataAutomationConfiguration`'s project ARN, or an
    // inline `blueprints` list. No Smithy trait expresses "exactly one of", so the rule
    // is the service's and both invoke operations apply it identically: naming neither,
    // or naming both, is `ValidationException` (HTTP 400); naming exactly one is past the
    // rule.
    //
    // The rows differ along what the rule generalises over — which of the two members is
    // present, how many blueprints there are, whether the blueprint carries its optional
    // version/stage, which form the synchronous input takes, and whether an unrelated
    // optional member is present — not along renamed values.
    let router = service();

    let rows: Vec<ExtractionTargetCase> = vec![
        ExtractionTargetCase {
            name: "neither member present",
            configuration: Value::Null,
            blueprints: Value::Null,
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/doc.pdf"}),
            refusal: Some(MISSING_PROJECT_OR_BLUEPRINTS),
        },
        ExtractionTargetCase {
            name: "neither member present, inline input bytes",
            configuration: Value::Null,
            blueprints: Value::Null,
            // Base64 of `{"a":1}`: `SyncInputConfiguration.bytes` is an unconstrained
            // blob, so any payload exercises the same rule.
            sync_input: json!({"bytes": "eyJhIjoxfQ=="}),
            refusal: Some(MISSING_PROJECT_OR_BLUEPRINTS),
        },
        ExtractionTargetCase {
            name: "a project only",
            configuration: json!({"dataAutomationProjectArn": PROJECT_ARN}),
            blueprints: Value::Null,
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/scan.tiff"}),
            refusal: None,
        },
        ExtractionTargetCase {
            name: "a project with its optional stage",
            configuration: json!({"dataAutomationProjectArn": PROJECT_ARN, "stage": "DEVELOPMENT"}),
            blueprints: Value::Null,
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/doc.pdf"}),
            refusal: None,
        },
        ExtractionTargetCase {
            name: "one inline blueprint only",
            configuration: Value::Null,
            blueprints: json!([public_blueprint("video")]),
            sync_input: json!({"bytes": "eyJhIjoxfQ=="}),
            refusal: None,
        },
        ExtractionTargetCase {
            name: "three inline blueprints only",
            configuration: Value::Null,
            blueprints: json!([
                public_blueprint("video"),
                public_blueprint("audio"),
                public_blueprint("document"),
            ]),
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/clip.mp4"}),
            refusal: None,
        },
        ExtractionTargetCase {
            name: "one blueprint carrying its optional version and stage",
            configuration: Value::Null,
            blueprints: json!([{
                "blueprintArn": "arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-image",
                "version": "1",
                "stage": "LIVE",
            }]),
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/photo.png"}),
            refusal: None,
        },
        ExtractionTargetCase {
            name: "both a project and one blueprint",
            configuration: json!({"dataAutomationProjectArn": PROJECT_ARN}),
            blueprints: json!([public_blueprint("document")]),
            sync_input: json!({"s3Uri": "s3://in-bucket/assets/doc.pdf"}),
            refusal: Some(BOTH_PROJECT_AND_BLUEPRINTS),
        },
        ExtractionTargetCase {
            name: "both a project with a stage and three blueprints",
            configuration: json!({"dataAutomationProjectArn": PROJECT_ARN, "stage": "LIVE"}),
            blueprints: json!([
                public_blueprint("video"),
                public_blueprint("audio"),
                public_blueprint("document"),
            ]),
            sync_input: json!({"bytes": "eyJhIjoxfQ=="}),
            refusal: Some(BOTH_PROJECT_AND_BLUEPRINTS),
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for case in &rows {
        // The ASYNCHRONOUS operation: refused with 400 `ValidationException`, or
        // accepted with the invocation it minted.
        let answer = call(&router, "InvokeDataAutomationAsync", async_submission(case)).await;
        match case.refusal {
            None => {
                assert_eq!(
                    (answer.status, answer.error_code()),
                    (200, ""),
                    "case {:?}: a submission naming exactly one extraction target must be \
                     accepted by InvokeDataAutomationAsync, got {} / {}",
                    case.name,
                    answer.status,
                    answer.body
                );
                minted.push(
                    answer.body["invocationArn"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {:?}: no invocationArn in {}", case.name, answer.body)
                        })
                        .to_string(),
                );
            }
            Some(message) => assert_eq!(
                (answer.status, answer.error_code(), answer.body["message"].as_str()),
                (400, "ValidationException", Some(message)),
                "case {:?}: InvokeDataAutomationAsync must answer 400 ValidationException \
                 {message:?}, got {} / {}",
                case.name,
                answer.status,
                answer.body
            ),
        }

        // The SYNCHRONOUS operation applies the same rule, so a refused shape is the
        // same 400 there. A shape that passes the rule reaches the part of the operation
        // that is still scaffolded, which is the 501 — never a 400.
        let answer = call(&router, "InvokeDataAutomation", sync_submission(case)).await;
        let expected = match case.refusal {
            None => (501, "NotImplemented", None),
            Some(message) => (400, "ValidationException", Some(message)),
        };
        assert_eq!(
            (
                answer.status,
                answer.error_code(),
                case.refusal.and(answer.body["message"].as_str())
            ),
            expected,
            "case {:?}: InvokeDataAutomation must answer {expected:?}, got {} / {}",
            case.name,
            answer.status,
            answer.body
        );
    }

    // Only the accepted rows minted anything: each answered with its own ARN, and every
    // one of them resolves through the reader. A refused submission left no record, so
    // the count of distinct ARNs is exactly the count of accepted rows.
    let accepted = rows.iter().filter(|case| case.refusal.is_none()).count();
    assert_eq!(
        minted.len(),
        accepted,
        "every accepted submission must mint one invocation, got {minted:?}"
    );
    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "two submissions must never share an invocationArn, got {minted:?}"
    );
    for arn in &minted {
        let answer = call(&router, "GetDataAutomationStatus", json!({"invocationArn": arn})).await;
        assert_eq!(
            answer.status, 200,
            "the invocation {arn:?} minted by an accepted submission must read back, got \
             {} / {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn every_operation_that_accepts_a_tag_key_applies_the_modelled_tag_key_pattern() {
    // The modelled `TagKey` pattern is `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$`
    // (`smithy_query` shape `TagKey`). Its negative lookahead cannot be compiled by the
    // boundary walk's regex engine, and an uncompilable `@pattern` is treated as matching
    // every value — which drops the character class with it — so the three operations
    // that accept a tag key apply the whole pattern themselves.
    //
    // Rows span both halves of the pattern: the reserved prefix, and characters inside
    // and outside the class (Unicode letters, numbers and separators, which the class
    // admits; combining marks, symbols and ASCII punctuation outside it, which it does
    // not). The class is spanned category by category — including the marks that carry
    // `Other_Alphabetic` — by `tag_key_unicode_class`.
    let router = service();
    let arn = submit(
        &router,
        "tag key pattern",
        minimal_submission("s3://out-bucket/tag-keys/"),
    )
    .await;

    // (case, tag key, whether the pattern admits it).
    let keys: [(&str, &str, bool); 12] = [
        ("a plain ASCII key", "env", true),
        ("every punctuation character the class admits", "a_b.c:d/e=f+g-h@i", true),
        ("a Unicode letter key (\\p{L})", "ümgebung", true),
        ("a Unicode number key (\\p{N})", "stage٣", true),
        ("a separator inside the key (\\p{Z})", "cost center", true),
        ("the empty-ish single character", "x", true),
        (
            // A symbol (`So`) is in none of the three categories the class names.
            "an emoji, outside the class",
            "env\u{1f680}",
            false,
        ),
        (
            // …and so is a plain combining mark.
            "a plain combining mark, outside the class",
            "e\u{301}nv",
            false,
        ),
        ("the reserved prefix exactly", "aws:", false),
        ("a key under the reserved prefix", "aws:cloudformation:stack-name", false),
        ("a character outside the class", "bad!key", false),
        ("another character outside the class", "bad#key", false),
    ];

    for (case, key, admitted) in keys {
        // TagResource carries the key in `tags`; UntagResource carries it in `tagKeys`.
        // Both must agree, because both are bound by the same `TagKey` shape.
        let rows: [(&str, Value); 2] = [
            (
                "TagResource",
                json!({"resourceARN": arn, "tags": [{"key": key, "value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"resourceARN": arn, "tagKeys": [key]}),
            ),
        ];
        for (operation, body) in rows {
            let answer = call(&router, operation, body).await;
            if admitted {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} ({key:?}) on {operation}: the modelled TagKey pattern \
                     admits this key, got {} / {}",
                    answer.status, answer.body
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_code()),
                    (400, "ValidationException"),
                    "case {case:?} ({key:?}) on {operation}: the modelled TagKey pattern \
                     rejects this key, got {} / {}",
                    answer.status,
                    answer.body
                );
                // The refusal reads in this service's own wording — the same wording the
                // boundary walk's refusals get, since one logical failure must not read
                // two ways: the member is named, the offending value is not echoed, and
                // the declared pattern is printed without its anchors.
                let message = answer.body["message"].as_str().unwrap_or_default();
                assert!(
                    message.starts_with("1 validation error detected: Value at '")
                        && !message.contains("Value '"),
                    "case {case:?} ({key:?}) on {operation}: the refusal must name the \
                     member without echoing the value, got {message:?}"
                );
                let Some((_, pattern)) =
                    message.split_once("Member must satisfy regular expression pattern: ")
                else {
                    panic!("case {case:?} ({key:?}) on {operation}: no pattern clause in {message:?}");
                };
                assert!(
                    !pattern.starts_with('^'),
                    "case {case:?} ({key:?}) on {operation}: the pattern must be printed \
                     without its anchors, got {pattern:?}"
                );
            }
        }

        // And the submission that creates a resource is bound by the same shape.
        let mut body = minimal_submission("s3://out-bucket/tag-keys/");
        body["tags"] = json!([{"key": key, "value": "v"}]);
        let answer = call(&router, "InvokeDataAutomationAsync", body).await;
        let expected = if admitted {
            (200, "")
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            (answer.status, answer.error_code()),
            expected,
            "case {case:?} ({key:?}) on InvokeDataAutomationAsync: expected \
             {expected:?}, got {} / {}",
            answer.status,
            answer.body
        );
    }

    // A refused tag key changed nothing: the resource still carries only what the
    // admitted rows left on it, and `aws:`-prefixed keys never reached the tag map.
    let tags = list_tags(&router, "tag key pattern", &arn).await;
    assert!(
        tags.iter().all(|(key, _)| !key.starts_with("aws:")),
        "no reserved tag key may be stored, got {tags:?}"
    );
}

#[tokio::test]
async fn the_synchronous_invoke_data_automation_is_still_a_stub() {
    // `InvokeDataAutomation` returns the analysis of the asset itself
    // (`semanticModality` plus `outputSegments`), which no engine in this emulator can
    // produce. It therefore keeps the scaffold's 501 rather than answering with
    // synthesised output; this pins that it is a 501 and not an empty 200. The
    // submission names a project, so it is past the cross-member rule the operation DOES
    // enforce — a 501 here is about the missing analysis, not about a bad request.
    let router = service();
    let answer = call(
        &router,
        "InvokeDataAutomation",
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 501,
        "expected 501 NotImplemented from the synchronous invoke, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotImplemented",
        "expected the scaffold's NotImplemented, got {}",
        answer.body
    );
}
