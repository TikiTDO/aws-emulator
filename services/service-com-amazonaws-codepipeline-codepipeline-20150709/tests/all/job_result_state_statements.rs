// SPDX-License-Identifier: CC0-1.0

//! What a job-result refusal STATES, over the wire the service speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: every operation a job worker reports a result through refuses a
//! job that cannot take that result with `InvalidJobStateException`, and WHICH statement
//! the refusal carries is decided by the job's own status against the status the result
//! reports — the job is already in it ("already set to the desired value"), the job has
//! reached some other terminal status ("has already terminated"), or no worker has
//! claimed it ("has not been acknowledged"). The statement names the `jobId` the REQUEST
//! named and the condition; the `JobStatus` the store holds is not disclosed in it. The
//! rule is the service's, not one plane's: the custom-action plane
//! (`PutJobSuccessResult`, `PutJobFailureResult`) and the partner plane
//! (`PutThirdPartyJobSuccessResult`, `PutThirdPartyJobFailureResult`) model the same
//! error for the same three conditions, so they make the same statement.
//! `JobNotFoundException` stays what an id no job was ever issued under is answered with.
//!
//! The rows span what the rule generalises over: every value the model's `JobStatus`
//! enum declares, against each of the two results a worker can report, on the partner
//! plane — where the job's own state is the only thing the refusal can be about — plus
//! the custom plane's statement for the same condition, so a divergence between the two
//! planes fails here.
//!
//! The custom-action plane's jobs come from the execution plane (the suite
//! `tests/all/terminal_state_refusals.rs` drives that end to end); the partner plane's
//! queue has no execution-plane writer in this crate, so these jobs are queued through
//! `ComAmazonawsCodepipelineCodepipeline20150709State::enqueue_third_party_job`, its one
//! authoring seam and the same store the handlers read. Every call goes through the
//! router.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeId, JOB_STATUS_VALUES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    invalid_state_message, ComAmazonawsCodepipelineCodepipeline20150709State as CodePipelineState,
    ThirdPartyJobRecord, JOB_STATUS_FAILED, JOB_STATUS_IN_PROGRESS, JOB_STATUS_SUCCEEDED,
    TERMINAL_JOB_STATUSES,
};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The status every refusal here carries: each of these errors is modelled HTTP 400.
const REFUSAL_STATUS: u16 = 400;

/// The id the seeded job is filed under, in the UUID form `ThirdPartyJob$jobId` models.
const JOB_ID: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// The `clientToken` the seeded job was handed out with.
const CLIENT_TOKEN: &str = "worker-token";

/// The `clientId` half of the pair the job was handed to.
const CLIENT_ID: &str = "7f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b";

/// A `jobId` no job was ever issued under: the id the OTHER error is reserved for.
const UNISSUED_JOB_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The third terminal value of the model's `JobStatus` enum, beside `Succeeded` and
/// `Failed`: a job whose worker's window to report ran out.
const JOB_STATUS_TIMED_OUT: &str = "TimedOut";

/// When a seeded job's status last changed: a fixed past instant, so a refusal that
/// recorded anything would be visible.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

/// The identity of the `ThirdParty` action type whose queue the seeded job waits in.
fn partner_action_type() -> ActionTypeId {
    ActionTypeId {
        category: "Build".to_string(),
        // `ActionOwner` (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`,
        // `Custom`, `ThirdParty`): the store queues a third-party job only for a
        // partner action type.
        owner: "ThirdParty".to_string(),
        provider: "PartnerBuild".to_string(),
        version: "1".to_string(),
    }
}

/// One emulator: the shared engine, the service mounted on it, and the router.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// Queue one partner job in the `JobStatus` a case needs, as the action-execution
    /// plane does when an execution reaches a partner action.
    fn seed_job(&self, status: &str) {
        let mut job = ThirdPartyJobRecord::new(JOB_ID, CLIENT_ID);
        job.client_token = CLIENT_TOKEN.to_string();
        job.status = status.to_string();
        job.last_status_change = seeded_at();
        let queued = self
            .engine
            .state::<CodePipelineState>()
            .lock()
            .enqueue_third_party_job(&partner_action_type(), job);
        assert!(
            queued,
            "seeding the job: the store must queue a job for {:?}",
            partner_action_type()
        );
    }

    /// The stored job, as the next caller would read it.
    fn job(&self) -> ThirdPartyJobRecord {
        self.engine
            .state::<CodePipelineState>()
            .lock()
            .third_party_jobs
            .values()
            .flatten()
            .find(|job| job.job_id == JOB_ID)
            .cloned()
            .unwrap_or_else(|| panic!("job {JOB_ID} must still be in the store"))
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("{operation}: body is not JSON ({error})"))
        };
        (status, parsed)
    }

    /// The `(status, __type, message)` of a refusal.
    async fn refusal(&self, operation: &str, body: &Value) -> (u16, String, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        let message = response["message"].as_str().unwrap_or_default().to_string();
        (status, code, message)
    }

    async fn ok(&self, operation: &str, body: &Value) {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
    }
}

/// The partner plane's two result operations, with the `JobStatus` each one reports.
const PARTNER_RESULTS: [(&str, &str); 2] = [
    ("PutThirdPartyJobSuccessResult", JOB_STATUS_SUCCEEDED),
    ("PutThirdPartyJobFailureResult", JOB_STATUS_FAILED),
];

/// The request body one of those operations takes for `job_id`.
fn partner_body(operation: &str, job_id: &str) -> Value {
    match operation {
        "PutThirdPartyJobFailureResult" => json!({
            "jobId": job_id,
            "clientToken": CLIENT_TOKEN,
            "failureDetails": {"type": "JobFailed", "message": "the partner build broke"},
        }),
        _ => json!({
            "jobId": job_id,
            "clientToken": CLIENT_TOKEN,
            "executionDetails": {"summary": "the partner worker ran it"},
        }),
    }
}

/// RULE: on the partner plane, a job that is not one a worker currently holds takes no
/// result, and the refusal's statement is the one its own status against the reported
/// status makes true — with no `JobStatus` disclosed.
///
/// The rows are every value the model's `JobStatus` enum declares, against each result a
/// worker can report.
#[tokio::test]
async fn a_partner_job_that_takes_no_result_is_refused_with_the_statement_its_state_makes() {
    // Every value of the model's enum, from this service's own generated catalog
    // (`ops::types::JOB_STATUS_VALUES`): `Created`, `Dispatched`, `Failed`,
    // `InProgress`, `Queued`, `Succeeded`, `TimedOut`.
    assert_eq!(
        JOB_STATUS_VALUES.len(),
        7,
        "the table spans every modelled JobStatus, got {JOB_STATUS_VALUES:?}"
    );

    for status in JOB_STATUS_VALUES {
        for (operation, reported) in PARTNER_RESULTS {
            let case = format!("a {operation} for a job that is {status}");
            let emulator = Emulator::new();
            emulator.seed_job(status);

            if *status == JOB_STATUS_IN_PROGRESS {
                // The positive control: the job a worker holds takes the result, so the
                // rows below are refusals of STATE and not of the request's shape.
                emulator.ok(operation, &partner_body(operation, JOB_ID)).await;
                assert_eq!(
                    emulator.job().status,
                    reported,
                    "case {case:?}: a held job takes the result and reaches {reported}"
                );
                continue;
            }

            let (http_status, code, message) = emulator
                .refusal(operation, &partner_body(operation, JOB_ID))
                .await;
            assert_eq!(
                (http_status, code.as_str()),
                (REFUSAL_STATUS, "InvalidJobStateException"),
                "case {case:?}: unexpected refusal ({message})"
            );
            assert_eq!(
                message,
                invalid_state_message(JOB_ID, status, reported),
                "case {case:?}: unexpected refusal statement"
            );
            // The statement is about the CONDITION: a job already in the reported status
            // cannot be updated with a different result, a job in another terminal
            // status has terminated, and one nobody claimed has not been acknowledged —
            // and none of them names the status the store holds.
            let expected_fragment = if status == &reported {
                "already set to the desired value"
            } else if TERMINAL_JOB_STATUSES.contains(status) {
                "has already terminated"
            } else {
                "has not been acknowledged"
            };
            assert!(
                message.contains(expected_fragment),
                "case {case:?}: expected the statement to read {expected_fragment:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(status),
                "case {case:?}: the statement must not disclose the stored JobStatus, got \
                 {message:?}"
            );

            // The refused call recorded nothing: the job is where it was.
            let job = emulator.job();
            assert_eq!(
                (job.status.as_str(), job.last_status_change),
                (*status, seeded_at()),
                "case {case:?}: a refused result must leave the job as it was, got {job:?}"
            );

            // An id no job was ever issued under is the OTHER error, from the same call.
            let (http_status, code, _) = emulator
                .refusal(operation, &partner_body(operation, UNISSUED_JOB_ID))
                .await;
            assert_eq!(
                (http_status, code.as_str()),
                (REFUSAL_STATUS, "JobNotFoundException"),
                "case {case:?}: an id no job was issued under is not a state error"
            );
        }
    }
}

/// RULE, across the two planes: a job a worker carried to a terminal status is refused a
/// later result with the SAME statement whichever plane the worker reports on — the
/// condition is the service's, so neither plane words it for itself.
#[tokio::test]
async fn both_job_planes_state_a_terminated_job_the_same_way() {
    // (case, the status the job terminated in, the result reported afterwards)
    let rows: [(&str, &str, &str); 3] = [
        (
            "a failure reported for a job that succeeded",
            JOB_STATUS_SUCCEEDED,
            "PutThirdPartyJobFailureResult",
        ),
        (
            "a success reported for a job that failed",
            JOB_STATUS_FAILED,
            "PutThirdPartyJobSuccessResult",
        ),
        (
            "a success reported for a job that timed out",
            JOB_STATUS_TIMED_OUT,
            "PutThirdPartyJobSuccessResult",
        ),
    ];

    for (case, terminal_status, operation) in rows {
        let emulator = Emulator::new();
        emulator.seed_job(terminal_status);
        let (_, _, message) = emulator
            .refusal(operation, &partner_body(operation, JOB_ID))
            .await;
        // The statement the custom-action plane makes for the same condition, which
        // `tests/all/terminal_state_refusals.rs` pins against a job the execution plane
        // drove to a terminal status there.
        let custom_plane = format!("Job with id {JOB_ID} has already terminated");
        assert_eq!(
            message, custom_plane,
            "case {case:?}: the partner plane must state a terminated job the way the \
             custom-action plane does"
        );
    }
}
