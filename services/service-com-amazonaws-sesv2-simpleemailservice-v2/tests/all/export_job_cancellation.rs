// SPDX-License-Identifier: CC0-1.0

//! Cancelling an export job (`CancelExportJob`), end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised; a case reaches the engine's own state only to move the instants a
//! run's clock-owed transitions fall due at.
//!
//! The rules these cases pin:
//!
//! * a cancellation stops the stored job's run: a job whose run has not ended reaches
//!   the terminal `CANCELLED` status, which is what every later read of that job
//!   reports, and it never completes — so it reports no completion instant, writes no
//!   export file and hands out no download URL;
//! * `CANCELLED` is terminal: once a run is stopped, no transition the clock would
//!   otherwise have owed it moves it on;
//! * "Call GetExportJob with that ID until JobStatus is COMPLETED, FAILED, or
//!   CANCELLED" — a job already in a status its run never leaves has nothing left to
//!   stop, so the cancellation is refused with the modelled `BadRequestException` and
//!   that job is left exactly as it stood;
//! * a `JobId` the account holds no export job for is the modelled `NotFoundException`;
//! * reading a job never cancels it, and never moves its run on.
//!
//! The tables below span what the rules generalise over: both modelled source types,
//! both modelled data formats, each run state a cancellation can meet, every terminal
//! status a refusal reports, and several forms of unknown job id.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An export job belongs to the account, so one Region is enough.
const HOME: &str = "us-east-1";

/// `2026-01-01T00:00:00Z`, the UTC day a case's data source covers, as whole epoch
/// seconds.
const DAY_START: i64 = 1_767_225_600;

/// One second short of the next day, so the interval covers exactly one UTC day.
const DAY_END: i64 = DAY_START + 86_399;

/// `MetricNamespace$VDM` — "the metrics namespace - e.g., VDM".
const VDM: &str = "VDM";

/// Long enough that a transition held this far out cannot fall due while a case runs.
const HELD: Duration = Duration::from_secs(3600);

/// Just enough to put a due instant in the past.
const ELAPSED: Duration = Duration::from_secs(1);

/// One answered API request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the answered document as text.
    fn text(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The download URL the destination carries, if any.
    fn s3_url(&self) -> Option<String> {
        self.body
            .get("ExportDestination")
            .and_then(|destination| destination.get("S3Url"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The job ids a listing reports, in the order it reports them.
    fn ids(&self) -> Vec<String> {
        self.body
            .get("ExportJobs")
            .and_then(Value::as_array)
            .map(|jobs| {
                jobs.iter()
                    .map(|job| {
                        job.get("JobId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// How far a job's run has got when a case acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    /// The run has not started: both transitions are still ahead.
    Created,
    /// The run started and has not finished.
    Processing,
    /// The run finished.
    Completed,
}

impl Run {
    /// The modelled `JobStatus` a job in this state reports.
    fn status(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
        }
    }
}

/// One job a case creates: the data source it draws on and the format it is written
/// in, which together span the two modelled source types and the two modelled data
/// formats.
struct JobRow {
    /// What the case is about, named in every assertion.
    case: &'static str,
    /// The `ExportDataSource` the create supplies.
    data_source: fn() -> Value,
    /// `ExportDestination$DataFormat` the create asks for.
    data_format: &'static str,
    /// The `ExportSourceType` the data source selects, which every read reports.
    source_type: &'static str,
}

/// The job shapes every case runs over. Source: `ExportDataSource`'s two members and
/// `DataFormat`'s two values
/// (`crate::ops::types::EXPORT_SOURCE_TYPE_VALUES` / `DATA_FORMAT_VALUES`).
const JOB_ROWS: &[JobRow] = &[
    JobRow {
        case: "metrics data source, CSV",
        data_source: metrics_source,
        data_format: "CSV",
        source_type: "METRICS_DATA",
    },
    JobRow {
        case: "metrics data source, JSON",
        data_source: metrics_source,
        data_format: "JSON",
        source_type: "METRICS_DATA",
    },
    JobRow {
        case: "message insights data source, CSV",
        data_source: insights_source,
        data_format: "CSV",
        source_type: "MESSAGE_INSIGHTS",
    },
    JobRow {
        case: "message insights data source, JSON",
        data_source: insights_source,
        data_format: "JSON",
        source_type: "MESSAGE_INSIGHTS",
    },
];

/// A metrics data source over one UTC day, split by one ISP and reporting one metric.
fn metrics_source() -> Value {
    json!({
        "MetricsDataSource": {
            "Dimensions": {"ISP": ["gmail"]},
            "Namespace": VDM,
            "Metrics": [{"Name": "SEND", "Aggregation": "VOLUME"}],
            "StartDate": DAY_START,
            "EndDate": DAY_END,
        }
    })
}

/// A Message Insights data source over the same UTC day, with no filters.
fn insights_source() -> Value {
    json!({"MessageInsightsDataSource": {"StartDate": DAY_START, "EndDate": DAY_END}})
}

/// The emulated service behind its router, plus the shared engine so a case can move
/// the instants a run's transitions fall due at.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Send one API request.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            );
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let response = self
            .router
            .dispatch(request.body(payload).expect("build request"))
            .await;
        let status = response.status().as_u16();
        let error_type = response
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
            error_type,
            body,
        }
    }

    /// Create one export job as the row describes and answer the `JobId` it minted.
    async fn create(&self, row: &JobRow) -> String {
        let reply = self
            .call(
                "POST",
                "/v2/email/export-jobs",
                Some(json!({
                    "ExportDataSource": (row.data_source)(),
                    "ExportDestination": {"DataFormat": row.data_format},
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the create, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let job_id = reply.text("JobId");
        assert!(
            !job_id.is_empty(),
            "case {:?}: the create must answer a JobId, got {:?}",
            row.case,
            reply.body
        );
        job_id
    }

    /// Read one export job.
    async fn get(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/export-jobs/{job_id}"), None)
            .await
    }

    /// Cancel one export job.
    async fn cancel(&self, job_id: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/export-jobs/{job_id}/cancel"),
            None,
        )
        .await
    }

    /// List the export jobs matching `body`.
    async fn list(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/list-export-jobs", Some(body))
            .await
    }

    /// Put `job_id`'s run where `run` says by moving the instants its transitions fall
    /// due at. The handler stores a due instant per transition rather than sleeping, so
    /// a case drives a run by moving those instants instead of waiting on the wall
    /// clock.
    fn drive(&self, job_id: &str, run: Run) {
        let now = SystemTime::now();
        let (processing, completion) = match run {
            Run::Created => (now + HELD, now + HELD + HELD),
            Run::Processing => (now - ELAPSED, now + HELD),
            Run::Completed => (now - ELAPSED - ELAPSED, now - ELAPSED),
        };
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .export_jobs
            .get_mut(job_id)
            .expect("the created job is stored");
        record.processing_due_at = processing;
        record.completion_due_at = completion;
    }

    /// Whether the stored job has written its export file.
    fn has_a_file(&self, job_id: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .export_jobs
            .get(job_id)
            .is_some_and(|record| record.file.is_some())
    }

    /// Put `job_id`'s run in `run` and assert a read reports the status that goes with
    /// it, so a case acts on a job it has established the state of.
    async fn establish(&self, case: &str, job_id: &str, run: Run) {
        self.drive(job_id, run);
        let reply = self.get(job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from the read, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("JobStatus"),
            run.status(),
            "case {case:?}: expected the run to stand at {}, got {:?}",
            run.status(),
            reply.body
        );
    }
}

/// How this surface words the refusal of a cancellation of a run that has ended, with
/// the status the job is in as its one variable part.
fn not_cancellable(status: &str) -> String {
    format!("Cannot cancel an export job that has a <{status}> status.")
}

#[tokio::test]
async fn a_cancellation_stops_a_run_that_has_not_ended() {
    // Every job shape, met in each run state a cancellation can act on.
    for row in JOB_ROWS {
        for run in [Run::Created, Run::Processing] {
            let case = format!("{} from {}", row.case, run.status());
            let ses = Ses::new();
            let job_id = ses.create(row).await;
            ses.establish(&case, &job_id, run).await;

            let reply = ses.cancel(&job_id).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 from the cancellation, got {} {:?}",
                reply.status, reply.body
            );

            // The cancellation is reported by the stored job: every later read of it
            // reports the terminal status, and the run it stopped never completed, so
            // there is no completion instant, no statistics and no file to download.
            let read = ses.get(&job_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected 200 from the read, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("JobStatus"),
                "CANCELLED",
                "case {case:?}: expected the cancelled run's status, got {:?}",
                read.body
            );
            assert_eq!(
                read.text("JobId"),
                job_id,
                "case {case:?}: expected the job's own id, got {:?}",
                read.body
            );
            assert_eq!(
                read.text("ExportSourceType"),
                row.source_type,
                "case {case:?}: a cancellation leaves the job's source type, got {:?}",
                read.body
            );
            assert_eq!(
                read.body.get("CompletedTimestamp"),
                None,
                "case {case:?}: a stopped run never completed, got {:?}",
                read.body
            );
            assert_eq!(
                read.body.get("Statistics"),
                None,
                "case {case:?}: a stopped run counted no records, got {:?}",
                read.body
            );
            assert_eq!(
                read.s3_url(),
                None,
                "case {case:?}: a stopped run wrote no file to download, got {:?}",
                read.body
            );
            assert!(
                !ses.has_a_file(&job_id),
                "case {case:?}: a stopped run must write no export file"
            );

            // The listing reports the job under the status it now carries, and no
            // longer under the one its run had reached.
            let cancelled = ses.list(json!({"JobStatus": "CANCELLED"})).await;
            assert_eq!(
                cancelled.ids(),
                vec![job_id.clone()],
                "case {case:?}: expected the cancelled job in the CANCELLED listing, got {:?}",
                cancelled.body
            );
            let before = ses.list(json!({"JobStatus": run.status()})).await;
            assert_eq!(
                before.ids(),
                Vec::<String>::new(),
                "case {case:?}: expected no job left at {}, got {:?}",
                run.status(),
                before.body
            );
        }
    }
}

#[tokio::test]
async fn a_cancelled_run_is_never_moved_on_by_the_clock() {
    // Each run state a cancellation can act on, with every transition the clock would
    // otherwise have owed the run made due afterwards.
    for row in JOB_ROWS {
        for run in [Run::Created, Run::Processing] {
            let case = format!("{} from {}", row.case, run.status());
            let ses = Ses::new();
            let job_id = ses.create(row).await;
            ses.establish(&case, &job_id, run).await;
            let reply = ses.cancel(&job_id).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 from the cancellation, got {} {:?}",
                reply.status, reply.body
            );

            // Both of the run's transitions are now in the past, so a run that had not
            // been stopped would have completed by the next read.
            ses.drive(&job_id, Run::Completed);
            let read = ses.get(&job_id).await;
            assert_eq!(
                read.text("JobStatus"),
                "CANCELLED",
                "case {case:?}: a stopped run is owed no transition, got {:?}",
                read.body
            );
            assert!(
                !ses.has_a_file(&job_id),
                "case {case:?}: a stopped run must write no export file"
            );
            assert_eq!(
                read.s3_url(),
                None,
                "case {case:?}: a stopped run has no file to download, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn cancelling_a_run_that_has_already_ended_is_refused() {
    // The two statuses a run of this emulator never leaves: the one a finished run
    // reaches, and the one a cancellation puts it in.
    for row in JOB_ROWS {
        let ses = Ses::new();

        let completed_case = format!("{}, already COMPLETED", row.case);
        let completed = ses.create(row).await;
        ses.establish(&completed_case, &completed, Run::Completed)
            .await;

        let cancelled_case = format!("{}, already CANCELLED", row.case);
        let cancelled = ses.create(row).await;
        ses.establish(&cancelled_case, &cancelled, Run::Processing)
            .await;
        let stopped = ses.cancel(&cancelled).await;
        assert_eq!(
            stopped.status, 200,
            "case {cancelled_case:?}: expected 200 from the first cancellation, got {} {:?}",
            stopped.status, stopped.body
        );

        for (case, job_id, status) in [
            (&completed_case, &completed, "COMPLETED"),
            (&cancelled_case, &cancelled, "CANCELLED"),
        ] {
            let reply = ses.cancel(job_id).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected 400 for a run that has ended, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case:?}: expected the modelled input error, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.message(),
                not_cancellable(status),
                "case {case:?}: expected the refusal to name the status, got {:?}",
                reply.message()
            );

            // The refused request left the job exactly as it stood.
            let read = ses.get(job_id).await;
            assert_eq!(
                read.text("JobStatus"),
                status,
                "case {case:?}: a refused cancellation changes nothing, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_job_id_the_account_holds_no_job_for_is_not_found() {
    let ses = Ses::new();
    // One stored job, so the refusal is about the id asked for rather than an empty
    // account; then ids of several forms that no stored job carries.
    let stored = ses.create(&JOB_ROWS[0]).await;
    let unknown = [
        "x",
        "ef5f5e16-0d39-4e07-9cd9-1b50ee4a4f3a",
        "EXPORT-JOB-0000000001",
        &format!("{stored}-suffix"),
        &"j".repeat(256),
    ];
    for job_id in unknown {
        let reply = ses.cancel(job_id).await;
        assert_eq!(
            reply.status, 404,
            "case {job_id:?}: expected 404 for an id no job carries, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NotFoundException",
            "case {job_id:?}: expected the modelled missing-resource error, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("Export job {job_id} does not exist."),
            "case {job_id:?}: expected the refusal to name the requested id, got {:?}",
            reply.message()
        );
    }

    // The stored job is untouched by every refused request.
    let read = ses.get(&stored).await;
    assert_eq!(
        read.text("JobStatus"),
        "CREATED",
        "case \"the stored job\": a refused cancellation changes nothing, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn reading_a_job_never_cancels_it() {
    // A read is reporting only: whatever a case reads the job through, and however
    // often, the job stays cancellable and its run stays where the clock left it.
    for row in JOB_ROWS {
        let case = row.case;
        let ses = Ses::new();
        let job_id = ses.create(row).await;
        ses.establish(case, &job_id, Run::Processing).await;

        for round in 0..3 {
            let read = ses.get(&job_id).await;
            assert_eq!(
                read.text("JobStatus"),
                "PROCESSING",
                "case {case:?}: read {round} must leave the run where it stood, got {:?}",
                read.body
            );
            let listed = ses.list(json!({})).await;
            assert_eq!(
                listed.ids(),
                vec![job_id.clone()],
                "case {case:?}: read {round} must report the stored job, got {:?}",
                listed.body
            );
        }

        let reply = ses.cancel(&job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a job no read cancelled is still cancellable, got {} {:?}",
            reply.status, reply.body
        );
    }
}
