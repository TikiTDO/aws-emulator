// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CancelJob`: the control plane cancels a job in the
//! SHARED jobs store (`service_common::iot_jobs`) and, with it, the executions of that
//! job in the SHARED job-execution store (`service_common::iot_job_executions`) the IoT
//! Jobs data plane serves.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /jobs/{jobId}/cancel` signed for the `iot` signing name — after
//! creating the job through `PUT /jobs/{jobId}`, and reads the result back through
//! `GET /jobs/{jobId}` (`DescribeJob`) and `GET /things/{thingName}/jobs/{jobId}`
//! (`DescribeJobExecution`), so what the cancel claims and what a later reader sees
//! cannot disagree.
//!
//! The rules under test:
//!
//! * a cancel moves the job's own lifecycle to the terminal `CANCELED` status, recording
//!   whether it was forced and the `reasonCode` / `comment` it stated, and answers the
//!   stored identity and description;
//! * it cancels exactly the executions its `force` flag reaches — `CancelJob$force`:
//!   "If true job executions with status IN_PROGRESS and QUEUED are canceled, otherwise
//!   only job executions with status QUEUED are canceled" — and leaves every execution
//!   that has already finished exactly as it stands;
//! * a `jobId` no record holds is the modelled `ResourceNotFoundException` (404);
//! * a job already in a status it never leaves (`CANCELED`, or a snapshot job every
//!   target has completed) is the modelled `InvalidRequestException` (400), and that
//!   refusal writes nothing.
//!
//! Each table spans what its rule generalises over — the `force` flag, how many devices
//! the job targets, which `JobExecutionStatus` each of them is in, whether the cancel
//! states a reason, whether the job carries a description, and which terminal status the
//! job reached — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{
    JobExecutionRecord, JobExecutionStore, STATUS_CANCELED, STATUS_IN_PROGRESS, STATUS_QUEUED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// The `JobStatus` of a job that is rolling out to its targets.
const JOB_IN_PROGRESS: &str = "IN_PROGRESS";
/// The `JobStatus` of a job whose rollout was cancelled.
const JOB_CANCELED: &str = "CANCELED";
/// The `JobStatus` of a snapshot job every target has finished.
const JOB_COMPLETED: &str = "COMPLETED";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("/message").unwrap_or_default()
    }

    /// The `job` member of a `DescribeJob` answer.
    fn job(&self) -> &Value {
        self.body.get("job").unwrap_or(&Value::Null)
    }

    /// One member of the `execution` a `DescribeJobExecution` answer reports.
    fn execution_member(&self, name: &str) -> &Value {
        &self.body["execution"][name]
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can seed the SHARED job-execution store with a status
/// only the device-facing plane would otherwise write.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_job(&self, job_id: &str, request: &Value) -> Reply {
        self.call("PUT", &format!("/jobs/{job_id}"), Some(request))
            .await
    }

    /// `CancelJob` for one job, with the `force` query and the body members the row asks
    /// for.
    async fn cancel_job(&self, job_id: &str, force: bool, body: &Value) -> Reply {
        let query = if force { "?force=true" } else { "" };
        self.call("PUT", &format!("/jobs/{job_id}/cancel{query}"), Some(body))
            .await
    }

    async fn describe_job(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/jobs/{job_id}"), None).await
    }

    async fn describe_execution(&self, thing_name: &str, job_id: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}/jobs/{job_id}"), None)
            .await
    }

    /// Mutate the stored execution of (thing, job) in the SHARED store, so a case can put
    /// a record in a state only the device-facing plane reaches.
    fn with_stored_execution<F>(&self, thing_name: &str, job_id: &str, mutate: F)
    where
        F: FnOnce(&mut JobExecutionRecord),
    {
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        let index = store
            .execution_index(thing_name, job_id, None)
            .unwrap_or_else(|| panic!("{thing_name} must hold an execution of {job_id}"));
        mutate(
            store
                .record_mut(thing_name, index)
                .expect("the index came from this store"),
        );
    }
}

/// The ARN a `targets` entry names a device by.
fn target_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// The job ARN this plane mints for `job_id` — `Job$jobArn` is "an ARN identifying the
/// job with format arn:aws:iot:region:account:job/jobId".
fn job_arn(job_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:job/{job_id}")
}

/// One targeted device of a row: the thing the job targets, the `JobExecutionStatus` its
/// execution is in when the cancel arrives, and the status it must be in afterwards.
struct Device {
    thing_name: &'static str,
    before: &'static str,
    after: &'static str,
}

/// One row of the cancel table.
struct CancelCase {
    case: &'static str,
    job_id: &'static str,
    description: Option<&'static str>,
    target_selection: Option<&'static str>,
    force: bool,
    reason_code: Option<&'static str>,
    comment: Option<&'static str>,
    devices: Vec<Device>,
}

/// Create `job_id` targeting every device of `case`, then put each device's execution in
/// the status the row says it is in when the cancel arrives.
async fn seed(emulator: &Emulator, case: &CancelCase) {
    let targets: Vec<String> = case
        .devices
        .iter()
        .map(|device| target_arn(device.thing_name))
        .collect();
    let mut request = json!({
        "targets": targets,
        "document": r#"{"operation":"reboot"}"#,
    });
    if let Some(description) = case.description {
        request["description"] = json!(description);
    }
    if let Some(target_selection) = case.target_selection {
        request["targetSelection"] = json!(target_selection);
    }
    let created = emulator.create_job(case.job_id, &request).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the job must be creatable, got {} {:?}",
        case.case, created.status, created.body
    );
    for device in &case.devices {
        if device.before == STATUS_QUEUED {
            continue;
        }
        let status = device.before.to_string();
        emulator.with_stored_execution(device.thing_name, case.job_id, |record| {
            record.status = status;
        });
    }
}

/// RULE: a cancel moves the job to the terminal `CANCELED` status with what it stated
/// about the cancel, and cancels exactly the executions its `force` flag reaches —
/// `QUEUED` always, `IN_PROGRESS` only when forced — leaving a finished execution alone.
#[tokio::test]
async fn a_cancel_moves_the_job_and_the_executions_its_force_flag_reaches() {
    let emulator = Emulator::new();

    let cases = vec![
        CancelCase {
            case: "one queued device, nothing stated",
            job_id: "cancel-queued",
            description: None,
            target_selection: None,
            force: false,
            reason_code: None,
            comment: None,
            devices: vec![Device {
                thing_name: "queued-a",
                before: STATUS_QUEUED,
                after: STATUS_CANCELED,
            }],
        },
        CancelCase {
            case: "an unforced cancel leaves the device that started alone",
            job_id: "cancel-unforced",
            description: Some("reboot the fleet"),
            target_selection: Some("SNAPSHOT"),
            force: false,
            reason_code: None,
            comment: None,
            devices: vec![
                Device {
                    thing_name: "running-b",
                    before: STATUS_IN_PROGRESS,
                    after: STATUS_IN_PROGRESS,
                },
                Device {
                    thing_name: "queued-b",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
            ],
        },
        CancelCase {
            case: "a forced cancel reaches the device that started",
            job_id: "cancel-forced",
            description: Some("install the new firmware"),
            target_selection: Some("CONTINUOUS"),
            force: true,
            reason_code: Some("FLEET_RECALL"),
            comment: Some("the rollout was recalled"),
            devices: vec![
                Device {
                    thing_name: "running-c",
                    before: STATUS_IN_PROGRESS,
                    after: STATUS_CANCELED,
                },
                Device {
                    thing_name: "queued-c",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
            ],
        },
        CancelCase {
            case: "finished executions are untouched by a forced cancel",
            job_id: "cancel-finished",
            description: None,
            target_selection: Some("SNAPSHOT"),
            force: true,
            reason_code: Some("SUPERSEDED"),
            comment: None,
            devices: vec![
                Device {
                    thing_name: "done-d",
                    before: "SUCCEEDED",
                    after: "SUCCEEDED",
                },
                Device {
                    thing_name: "failed-d",
                    before: "FAILED",
                    after: "FAILED",
                },
                Device {
                    thing_name: "rejected-d",
                    before: "REJECTED",
                    after: "REJECTED",
                },
                Device {
                    thing_name: "queued-d",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
            ],
        },
        CancelCase {
            case: "three queued devices and a comment only",
            job_id: "cancel-fanout",
            description: Some("stop the fan-out"),
            target_selection: None,
            force: false,
            reason_code: None,
            comment: Some("no longer needed"),
            devices: vec![
                Device {
                    thing_name: "queued-e",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
                Device {
                    thing_name: "queued-f",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
                Device {
                    thing_name: "queued-g",
                    before: STATUS_QUEUED,
                    after: STATUS_CANCELED,
                },
            ],
        },
    ];

    for case in &cases {
        seed(&emulator, case).await;

        let mut body = json!({});
        if let Some(reason_code) = case.reason_code {
            body["reasonCode"] = json!(reason_code);
        }
        if let Some(comment) = case.comment {
            body["comment"] = json!(comment);
        }
        let canceled = emulator.cancel_job(case.job_id, case.force, &body).await;
        assert_eq!(
            canceled.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, canceled.status, canceled.body
        );
        // The cancel answers the stored identity and description, not an echo of the
        // request's own members.
        assert_eq!(
            (
                canceled.text("/jobId"),
                canceled.text("/jobArn"),
                canceled.text("/description")
            ),
            (
                Some(case.job_id.to_string()),
                Some(job_arn(case.job_id)),
                case.description.map(str::to_string)
            ),
            "case {:?}: expected the stored job identity and description, got {:?}",
            case.case,
            canceled.body
        );

        // The JOB is cancelled, and says so with what the cancel stated about it.
        let read = emulator.describe_job(case.job_id).await;
        assert_eq!(
            (read.status, read.text("/job/status")),
            (200, Some(JOB_CANCELED.to_string())),
            "case {:?}: expected a {JOB_CANCELED} job, got {} {:?}",
            case.case,
            read.status,
            read.job()
        );
        assert_eq!(
            read.body
                .pointer("/job/forceCanceled")
                .and_then(Value::as_bool),
            Some(case.force),
            "case {:?}: expected forceCanceled {}, got {:?}",
            case.case,
            case.force,
            read.job()
        );
        assert_eq!(
            (read.text("/job/reasonCode"), read.text("/job/comment")),
            (
                case.reason_code.map(str::to_string),
                case.comment.map(str::to_string)
            ),
            "case {:?}: expected the stated reasonCode and comment, got {:?}",
            case.case,
            read.job()
        );

        // Every device's execution is in the status the rule leaves it in, and only an
        // execution this cancel moved carries the force flag.
        for device in &case.devices {
            let execution = emulator
                .describe_execution(device.thing_name, case.job_id)
                .await;
            assert_eq!(
                (
                    execution.status,
                    execution.execution_member("status").as_str()
                ),
                (200, Some(device.after)),
                "case {:?}: expected {} on {}, got {} {:?}",
                case.case,
                device.after,
                device.thing_name,
                execution.status,
                execution.body
            );
            let moved = device.before != device.after;
            assert_eq!(
                execution.execution_member("forceCanceled").as_bool(),
                moved.then_some(case.force),
                "case {:?}: expected forceCanceled {:?} on {}, got {:?}",
                case.case,
                moved.then_some(case.force),
                device.thing_name,
                execution.body
            );
        }

        // The per-status counts the job reports are counted from those very executions.
        let canceled_things = i64::try_from(
            case.devices
                .iter()
                .filter(|device| device.after == STATUS_CANCELED)
                .count(),
        )
        .expect("a handful of devices");
        assert_eq!(
            read.body
                .pointer("/job/jobProcessDetails/numberOfCanceledThings")
                .and_then(Value::as_i64),
            Some(canceled_things),
            "case {:?}: expected {canceled_things} canceled thing(s), got {:?}",
            case.case,
            read.job()
        );
        let still_running = case
            .devices
            .iter()
            .filter(|device| device.after == STATUS_IN_PROGRESS)
            .count();
        assert_eq!(
            read.body
                .pointer("/job/jobProcessDetails/numberOfInProgressThings")
                .and_then(Value::as_i64),
            (still_running > 0)
                .then(|| i64::try_from(still_running).expect("a handful of devices")),
            "case {:?}: expected {still_running} in-progress thing(s), got {:?}",
            case.case,
            read.job()
        );
        assert!(
            read.body
                .pointer("/job/jobProcessDetails/numberOfQueuedThings")
                .is_none(),
            "case {:?}: a cancelled job leaves no device queued, got {:?}",
            case.case,
            read.job()
        );
    }
}

/// RULE: a `jobId` no job was created under is the modelled `ResourceNotFoundException`
/// (HTTP 404) — never a 200 cancelling a job that does not exist, and never a 501.
#[tokio::test]
async fn an_unknown_job_id_is_a_404() {
    let emulator = Emulator::new();

    let case = CancelCase {
        case: "a job that exists",
        job_id: "job-that-exists",
        description: None,
        target_selection: None,
        force: false,
        reason_code: None,
        comment: None,
        devices: vec![Device {
            thing_name: "device-a",
            before: STATUS_QUEUED,
            after: STATUS_CANCELED,
        }],
    };
    seed(&emulator, &case).await;

    // An id nobody used, one differing only in case, one differing in a separator, and
    // one that is a prefix of the created id: none of them names a job.
    let unknown = [
        "job-that-exists-not",
        "JOB-THAT-EXISTS",
        "job_that_exists",
        "job",
    ];
    for job_id in unknown {
        let reply = emulator.cancel_job(job_id, false, &json!({})).await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {job_id:?}: expected a 404 ResourceNotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("Job {job_id} cannot be found."),
            "case {job_id:?}: expected the refusal to name the job, got {:?}",
            reply.body
        );
    }

    // The job that DOES exist is still rolling out: a refused cancel of another id wrote
    // nothing anywhere.
    let read = emulator.describe_job(case.job_id).await;
    assert_eq!(
        read.text("/job/status"),
        Some(JOB_IN_PROGRESS.to_string()),
        "case \"a job that exists\": a refused cancel of another id must not move it, \
         got {:?}",
        read.job()
    );
}

/// RULE: a job already in a status it never leaves has no rollout left to stop, so the
/// cancel is the modelled `InvalidRequestException` (HTTP 400) naming the job and that
/// status, and the refusal writes nothing.
#[tokio::test]
async fn a_job_in_a_terminal_status_cannot_be_canceled() {
    let emulator = Emulator::new();

    // (case, the job, the terminal status it reaches, and how it got there)
    let cancelled = CancelCase {
        case: "a job that was already cancelled",
        job_id: "already-cancelled",
        description: None,
        target_selection: None,
        force: false,
        reason_code: Some("RECALLED"),
        comment: Some("cancelled once"),
        devices: vec![Device {
            thing_name: "device-h",
            before: STATUS_QUEUED,
            after: STATUS_CANCELED,
        }],
    };
    // A SNAPSHOT job "will be complete after all those things specified as targets have
    // completed the job", so a snapshot job whose only execution succeeded is COMPLETED.
    let completed = CancelCase {
        case: "a snapshot job every target completed",
        job_id: "already-completed",
        description: Some("done rolling out"),
        target_selection: Some("SNAPSHOT"),
        force: true,
        reason_code: None,
        comment: None,
        devices: vec![Device {
            thing_name: "device-i",
            before: "SUCCEEDED",
            after: "SUCCEEDED",
        }],
    };

    seed(&emulator, &cancelled).await;
    let first = emulator
        .cancel_job(
            cancelled.job_id,
            cancelled.force,
            &json!({ "reasonCode": "RECALLED", "comment": "cancelled once" }),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case {:?}: the first cancel must succeed, got {} {:?}",
        cancelled.case, first.status, first.body
    );
    seed(&emulator, &completed).await;

    for (case, job_id, status) in [
        (cancelled.case, cancelled.job_id, JOB_CANCELED),
        (completed.case, completed.job_id, JOB_COMPLETED),
    ] {
        let before = emulator.describe_job(job_id).await;
        assert_eq!(
            before.text("/job/status"),
            Some(status.to_string()),
            "case {case:?}: the job must be in {status} before the cancel, got {:?}",
            before.job()
        );

        // Both a forced and an unforced cancel of a terminal job are refused: `force`
        // decides which EXECUTIONS a cancel reaches, never whether a terminal job moves.
        for force in [false, true] {
            let refused = emulator
                .cancel_job(job_id, force, &json!({ "reasonCode": "AGAIN" }))
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} (force={force}): expected a 400 InvalidRequestException, \
                 got {} {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.message(),
                format!("Job {job_id} in status {status} cannot be canceled."),
                "case {case:?} (force={force}): expected the refusal to name the job and \
                 its status, got {:?}",
                refused.body
            );
        }

        // The refusal wrote nothing: the job reads back exactly as it did before it.
        let after = emulator.describe_job(job_id).await;
        assert_eq!(
            (after.status, after.body.clone()),
            (before.status, before.body.clone()),
            "case {case:?}: a refused cancel must change nothing, got {:?}",
            after.body
        );
    }
}
