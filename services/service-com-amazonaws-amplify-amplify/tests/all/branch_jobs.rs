// SPDX-License-Identifier: CC0-1.0

//! `StartJob` / `GetJob`: a job is a persisted resource of the branch it runs on.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `StartJob` on an existing branch records a NEW job: the branch mints its `jobId`
//!    (`^[0-9]+$`, counted per branch), the job carries the commit members the request
//!    states and the empty value of each it omits, and it starts in the non-terminal
//!    `PENDING` status with no end time. `GetJob` answers that persisted record.
//! 2. `RETRY` is the one `jobType` that re-runs an EXISTING job, so it is refused with
//!    the modelled `BadRequestException` (HTTP 400) when the request carries no
//!    `jobId`, answers the modelled `NotFoundException` (HTTP 404) when the `jobId`
//!    names no job of that branch, and otherwise re-runs that same job id.
//! 3. A started job is asynchronous: the clock owes it a terminal status, and a read
//!    after that instant reports the settled record — the same record on every later
//!    read.
//! 4. A branch's job history is BOUNDED: starting a job past the retention cap retires
//!    the oldest FINISHED job, a branch whose every retained job is still running
//!    refuses one more with the modelled `LimitExceededException`, and
//!    `totalNumberOfJobs` still counts the jobs the branch has started.
//! 5. A job is addressed through its branch and its app, so an unknown app, an unknown
//!    branch, or a job id belonging to another branch is the modelled 404.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    JOB_SETTLE_MILLIS, MAX_JOBS_PER_BRANCH, NO_BUILD_ENVIRONMENT_REASON,
};

use crate::app_branch_lifecycle::{
    list_member, string_member, timestamp_member, Emulator, Reply,
};

/// The values `JobType` accepts, from the model
/// (`smithy_query(query="shape",shape="JobType")`: `RELEASE | RETRY | MANUAL |
/// WEB_HOOK`). Every row of every table below is one of these.
const JOB_TYPES: &[&str] = &["RELEASE", "RETRY", "MANUAL", "WEB_HOOK"];

/// The `jobType` that re-runs an existing job, and so the one the model requires a
/// `jobId` with.
const RETRY: &str = "RETRY";

/// The status `StartJob` answers with: `JobStatus` is modelled `CREATED | PENDING |
/// PROVISIONING | RUNNING | FAILED | SUCCEED | CANCELLING | CANCELLED`
/// (`smithy_query(query="shape",shape="JobStatus")`), and a job that has not run yet
/// is `PENDING`.
const PENDING: &str = "PENDING";

/// The terminal status a job of this emulator settles to, which is the modelled
/// failure state: no build environment exists here, so no build ran.
const FAILED: &str = "FAILED";

/// How long a test waits before expecting the clock-owed status change: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 250)
}

/// An app with one branch, and the branch's ARN (which every `jobArn` extends).
///
/// The app is CONNECTED TO A REPOSITORY, because every case here starts a job: a build
/// of the app's repository is what `StartJob` runs, so an app with no repository
/// provider refuses it (see `start_job_repository_precondition.rs`).
async fn app_with_branch(emulator: &Emulator, name: &str, branch: &str) -> (String, String) {
    let app_id = emulator.app_id_of_new_repository_app(name).await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let branch_arn = string_member(&created.member("branch"), "branchArn");
    (app_id, branch_arn)
}

/// `StartJob` over the wire.
async fn start_job(emulator: &Emulator, app_id: &str, branch: &str, request: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/jobs"),
            Some(request),
        )
        .await
}

/// `GetJob` over the wire.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
}

/// Job `job_id`'s record once the clock has settled it out of the non-terminal status a
/// start leaves it in — read again until it has, or a generous deadline passes.
///
/// The span a job settles over is the service's own, so a case waits for the CONDITION
/// rather than for a fixed time: the same read on a loaded or a faster host answers the
/// same way.
async fn settled_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let job = get_job(emulator, app_id, branch, job_id).await.member("job");
        if string_member(&optional_member(&job, "summary"), "status") != PENDING {
            return job;
        }
        assert!(
            Instant::now() < deadline,
            "job {job_id:?} on {branch:?} never settled: last read {job:?}"
        );
        sleep(Duration::from_millis(25));
    }
}

/// A member that may legitimately be absent, as the value a client sees: JSON `null`
/// for a member the body does not carry.
fn optional_member(record: &Value, member: &str) -> Value {
    record.get(member).cloned().unwrap_or(Value::Null)
}

/// Assert `reply` is the modelled 404 whose message names `entity` and `identifier`.
fn assert_not_found(case: &str, reply: &Reply, entity: &str, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected the x-amzn-errortype header to name NotFoundException, got {:?}",
        reply.error_code
    );
    let expected = format!("{entity} {identifier} not found.");
    assert_eq!(
        reply.message().as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected the request-derived message {expected:?}, got {:?}",
        reply.message()
    );
}

/// One `StartJob` request, plus the commit members the rule says the recorded job must
/// carry. `None` for a commit time means "the request stated none", which the job
/// records as the instant it was started.
struct StartCase {
    case: &'static str,
    request: Value,
    commit_id: String,
    commit_message: String,
    commit_time: Option<f64>,
}

/// The `StartJob` table: one row per `jobType` that starts a NEW job, crossed with the
/// optional members a request may carry — none of them, a commit id and message, an
/// explicit commit time, a `jobReason` the model gives no place to report, the empty
/// commit id the `length 0..255` minimum admits, and the modelled length MAXIMA of
/// `commitId` (255) and `commitMessage` (10000)
/// (`smithy_query(query="operation",operation="StartJob")`).
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "MANUAL, no optional members",
            request: json!({ "jobType": "MANUAL" }),
            commit_id: String::new(),
            commit_message: String::new(),
            commit_time: None,
        },
        StartCase {
            case: "RELEASE with commit id and message",
            request: json!({
                "jobType": "RELEASE",
                "commitId": "0123456789abcdef0123456789abcdef01234567",
                "commitMessage": "ship the release",
            }),
            commit_id: "0123456789abcdef0123456789abcdef01234567".to_string(),
            commit_message: "ship the release".to_string(),
            commit_time: None,
        },
        StartCase {
            case: "WEB_HOOK with an explicit commit time",
            request: json!({
                "jobType": "WEB_HOOK",
                "commitId": "deadbeef",
                "commitMessage": "webhook push",
                "commitTime": 1_700_000_000,
            }),
            commit_id: "deadbeef".to_string(),
            commit_message: "webhook push".to_string(),
            commit_time: Some(1_700_000_000.0),
        },
        StartCase {
            case: "MANUAL with a jobReason the model reports nowhere",
            request: json!({ "jobType": "MANUAL", "jobReason": "operator asked for a rebuild" }),
            commit_id: String::new(),
            commit_message: String::new(),
            commit_time: None,
        },
        StartCase {
            case: "RELEASE with the empty commit id the length minimum admits",
            request: json!({ "jobType": "RELEASE", "commitId": "", "commitMessage": "no commit" }),
            commit_id: String::new(),
            commit_message: "no commit".to_string(),
            commit_time: None,
        },
        StartCase {
            case: "WEB_HOOK at the modelled commit length maxima",
            request: json!({
                "jobType": "WEB_HOOK",
                "commitId": "c".repeat(255),
                "commitMessage": "m".repeat(10_000),
            }),
            commit_id: "c".repeat(255),
            commit_message: "m".repeat(10_000),
            commit_time: None,
        },
    ]
}

#[tokio::test]
async fn start_job_records_the_job_and_get_job_answers_the_record() {
    let emulator = Emulator::new();

    for (row, case) in start_cases().into_iter().enumerate() {
        let label = case.case;
        // A fresh branch per row, so the minted id is this branch's FIRST job id.
        let branch = format!("branch-{row}");
        let (app_id, branch_arn) =
            app_with_branch(&emulator, &format!("app-for-row-{row}"), &branch).await;

        let started = start_job(&emulator, &app_id, &branch, &case.request).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: expected HTTP 200 from StartJob, got {started:?}"
        );
        let summary = started.member("jobSummary");

        // The branch mints the id: digits only, and the first job of a branch is 1.
        let job_id = string_member(&summary, "jobId");
        assert_eq!(
            job_id, "1",
            "case {label:?}: the first job of a branch must be job 1, got {job_id:?}"
        );
        assert!(
            !job_id.is_empty() && job_id.chars().all(|digit| digit.is_ascii_digit()),
            "case {label:?}: jobId must satisfy the modelled pattern ^[0-9]+$, got {job_id:?}"
        );
        assert_eq!(
            string_member(&summary, "jobArn"),
            format!("{branch_arn}/jobs/{job_id:0>10}"),
            "case {label:?}: the jobArn must address the job inside its branch, with the \
             job number in the ten-digit field every job ARN carries, got {summary:?}"
        );

        // The job type is the request's, and the commit members are the request's or
        // the empty value of each the request omitted.
        assert_eq!(
            string_member(&summary, "jobType"),
            string_member(&case.request, "jobType"),
            "case {label:?}: the job must carry the requested jobType, got {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "commitId"),
            case.commit_id,
            "case {label:?}: unexpected commitId in {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "commitMessage"),
            case.commit_message,
            "case {label:?}: unexpected commitMessage (len {}) in the answer",
            string_member(&summary, "commitMessage").len()
        );
        let start_time = timestamp_member(&summary, "startTime");
        let commit_time = timestamp_member(&summary, "commitTime");
        match case.commit_time {
            Some(stated) => assert!(
                (commit_time - stated).abs() < f64::EPSILON,
                "case {label:?}: expected the stated commitTime {stated}, got {commit_time}"
            ),
            None => assert!(
                (commit_time - start_time).abs() < f64::EPSILON,
                "case {label:?}: a job started for no stated commit time records the instant \
                 it started ({start_time}), got {commit_time}"
            ),
        }
        assert!(
            start_time > 0.0,
            "case {label:?}: startTime must be the instant the job started, got {start_time}"
        );

        // A started job has not run: non-terminal status, no end time.
        assert_eq!(
            string_member(&summary, "status"),
            PENDING,
            "case {label:?}: a started job must be non-terminal, got {summary:?}"
        );
        assert!(
            optional_member(&summary, "endTime").is_null(),
            "case {label:?}: a job that has not ended carries no endTime, got {:?}",
            optional_member(&summary, "endTime")
        );
        // `jobReason` has no member on `JobSummary`, so nothing claims to echo it.
        assert!(
            summary.get("jobReason").is_none(),
            "case {label:?}: JobSummary models no jobReason, got {summary:?}"
        );

        // The read answers the PERSISTED record: the same summary, and no steps,
        // because nothing has run yet.
        let read = get_job(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetJob, got {read:?}"
        );
        let job = read.member("job");
        assert_eq!(
            job.get("summary").cloned().unwrap_or(Value::Null),
            summary,
            "case {label:?}: GetJob must answer the job StartJob recorded, got {job:?}"
        );
        assert!(
            list_member(&job, "steps").is_empty(),
            "case {label:?}: a job that has not run has no execution steps, got {job:?}"
        );
    }
}

#[tokio::test]
async fn job_ids_are_counted_per_branch_and_the_branch_reports_its_jobs() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_repository_app("counting-app").await;

    // Two branches of the SAME app: each counts its own jobs from 1, and starting a
    // job on one may not disturb the other.
    for branch in ["main", "feature-login"] {
        let created = emulator
            .create_branch(&app_id, &json!({ "branchName": branch }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {branch:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        // A branch that has never been built reports no active job and zero jobs.
        let record = emulator.get_branch(&app_id, branch).await.member("branch");
        assert!(
            optional_member(&record, "activeJobId").is_null(),
            "case {branch:?}: a new branch has no active job, got {record:?}"
        );
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            "0",
            "case {branch:?}: a new branch has run no jobs, got {record:?}"
        );
    }

    for branch in ["main", "feature-login"] {
        // Each row starts one more job than the row before it, so the ids span a
        // sequence rather than a single value.
        for expected in ["1", "2", "3"] {
            let started = start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" }))
                .await;
            assert_eq!(
                started.status, 200,
                "case {branch:?}/{expected}: expected HTTP 200 from StartJob, got {started:?}"
            );
            assert_eq!(
                string_member(&started.member("jobSummary"), "jobId"),
                expected,
                "case {branch:?}: job ids count this branch's jobs, got {:?}",
                started.member("jobSummary")
            );

            // The branch's own view of its jobs follows the jobs it holds.
            let record = emulator.get_branch(&app_id, branch).await.member("branch");
            assert_eq!(
                string_member(&record, "activeJobId"),
                expected,
                "case {branch:?}: the job just started is the active one, got {record:?}"
            );
            assert_eq!(
                string_member(&record, "totalNumberOfJobs"),
                expected,
                "case {branch:?}: totalNumberOfJobs counts the branch's jobs, got {record:?}"
            );
        }
    }

    // Every job of every branch is still readable through its own branch, and only
    // through its own branch.
    for (branch, other) in [("main", "feature-login"), ("feature-login", "main")] {
        for job_id in ["1", "2", "3"] {
            assert_eq!(
                get_job(&emulator, &app_id, branch, job_id).await.status,
                200,
                "case {branch:?}/{job_id}: a branch's own job must be readable"
            );
        }
        // Job 4 exists on neither branch: the id is absent, not merely elsewhere.
        assert_not_found(
            &format!("{branch}: job 4"),
            &get_job(&emulator, &app_id, branch, "4").await,
            "Job",
            "4",
        );
        assert_eq!(
            string_member(
                &emulator.get_branch(&app_id, other).await.member("branch"),
                "totalNumberOfJobs"
            ),
            "3",
            "case {branch:?}: reading one branch's jobs must not disturb another's"
        );
    }
}

#[tokio::test]
async fn retry_requires_an_existing_job_id_and_re_runs_that_job() {
    let emulator = Emulator::new();
    let branch = "main";

    // Row per `jobType`: without a `jobId`, only RETRY is refused — and it is refused
    // with the modelled 400 naming the member and the condition, not with a 501, a
    // 404, or a silently started job.
    for job_type in JOB_TYPES {
        let (app_id, _) =
            app_with_branch(&emulator, &format!("app-for-{job_type}"), branch).await;
        let reply = start_job(&emulator, &app_id, branch, &json!({ "jobType": job_type })).await;
        if *job_type == RETRY {
            assert_eq!(
                reply.status, 400,
                "case {job_type:?}: a RETRY without a jobId must be refused, got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("BadRequestException"),
                "case {job_type:?}: expected the modelled BadRequestException, got {:?}",
                reply.error_code
            );
            let message = reply.message().unwrap_or_default();
            assert!(
                message.contains("jobId") && message.contains(RETRY),
                "case {job_type:?}: the refusal must name the member and the condition, \
                 got {message:?}"
            );
            // Nothing was recorded: the branch still has no job 1.
            assert_not_found(
                &format!("{job_type}: nothing recorded"),
                &get_job(&emulator, &app_id, branch, "1").await,
                "Job",
                "1",
            );
        } else {
            assert_eq!(
                reply.status, 200,
                "case {job_type:?}: a new job needs no jobId, got {reply:?}"
            );
        }
    }

    // A RETRY whose `jobId` names no job of this branch is the modelled 404, whatever
    // the id looks like inside the modelled `pattern ^[0-9]+$`.
    let (app_id, branch_arn) = app_with_branch(&emulator, "retry-app", branch).await;
    let first = start_job(&emulator, &app_id, branch, &json!({
        "jobType": "RELEASE",
        "commitId": "abc123",
        "commitMessage": "the commit the retry rebuilds",
    }))
    .await;
    assert_eq!(
        first.status, 200,
        "case \"seed job\": expected HTTP 200 from StartJob, got {first:?}"
    );
    let seeded = first.member("jobSummary");

    for absent in ["0", "2", "999", &"9".repeat(255)] {
        assert_not_found(
            &format!("RETRY of absent job {absent}"),
            &start_job(
                &emulator,
                &app_id,
                branch,
                &json!({ "jobType": RETRY, "jobId": absent }),
            )
            .await,
            "Job",
            absent,
        );
    }

    // Wait for the clock to settle the seeded job, so the retry has a terminal job to
    // re-run.
    let settled = settled_job(&emulator, &app_id, branch, "1").await;
    assert_eq!(
        string_member(&optional_member(&settled, "summary"), "status"),
        FAILED,
        "case \"seed job\": the clock must have settled the job, got {settled:?}"
    );

    // The RETRY re-runs THAT job: same id, same commit (the request restated none),
    // a start time at or after the first run's, no end time, no steps — and the branch
    // still holds exactly one job, because a retry is not a new one.
    let retried = start_job(
        &emulator,
        &app_id,
        branch,
        &json!({ "jobType": RETRY, "jobId": "1" }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "case \"RETRY of job 1\": expected HTTP 200, got {retried:?}"
    );
    let summary = retried.member("jobSummary");
    assert_eq!(
        string_member(&summary, "jobId"),
        "1",
        "case \"RETRY of job 1\": a retry re-runs the job it names, got {summary:?}"
    );
    assert_eq!(
        string_member(&summary, "jobArn"),
        format!("{branch_arn}/jobs/0000000001"),
        "case \"RETRY of job 1\": the retried job keeps its ARN, got {summary:?}"
    );
    assert_eq!(
        string_member(&summary, "jobType"),
        RETRY,
        "case \"RETRY of job 1\": the job records the type it was started with"
    );
    assert_eq!(
        string_member(&summary, "commitId"),
        string_member(&seeded, "commitId"),
        "case \"RETRY of job 1\": a retry that restates no commit rebuilds the same one"
    );
    assert_eq!(
        string_member(&summary, "commitMessage"),
        string_member(&seeded, "commitMessage"),
        "case \"RETRY of job 1\": a retry that restates no commit keeps its message"
    );
    assert_eq!(
        string_member(&summary, "status"),
        PENDING,
        "case \"RETRY of job 1\": a re-run job is non-terminal again, got {summary:?}"
    );
    assert!(
        optional_member(&summary, "endTime").is_null(),
        "case \"RETRY of job 1\": the previous run's end time is cleared, got {summary:?}"
    );
    assert!(
        timestamp_member(&summary, "startTime") >= timestamp_member(&seeded, "startTime"),
        "case \"RETRY of job 1\": the re-run starts at or after the first run, got {summary:?}"
    );

    let record = emulator.get_branch(&app_id, branch).await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "1",
        "case \"RETRY of job 1\": a retry re-runs a job rather than adding one, got {record:?}"
    );
    let read_back = get_job(&emulator, &app_id, branch, "1").await.member("job");
    assert_eq!(
        read_back.get("summary").cloned().unwrap_or(Value::Null),
        summary,
        "case \"RETRY of job 1\": GetJob must answer the re-run job, got {read_back:?}"
    );
    assert!(
        list_member(&read_back, "steps").is_empty(),
        "case \"RETRY of job 1\": the previous run's steps are cleared, got {read_back:?}"
    );

    // A retry whose `jobId` is stated as the empty string names no job either: the
    // member's modelled length minimum is 0, so it reaches the handler as "no id".
    let empty = start_job(
        &emulator,
        &app_id,
        branch,
        &json!({ "jobType": RETRY, "jobId": "" }),
    )
    .await;
    assert_eq!(
        empty.status, 400,
        "case \"RETRY with an empty jobId\": expected the refusal, got {empty:?}"
    );
}

#[tokio::test]
async fn the_clock_settles_a_started_job_and_every_later_read_agrees() {
    let emulator = Emulator::new();
    let branch = "main";

    // One row per job type that starts a new job: the settling is a property of a
    // started job, not of one particular type.
    for job_type in JOB_TYPES.iter().filter(|job_type| **job_type != RETRY) {
        let (app_id, _) =
            app_with_branch(&emulator, &format!("settle-{job_type}"), branch).await;
        let started = start_job(&emulator, &app_id, branch, &json!({ "jobType": job_type })).await;
        assert_eq!(
            started.status, 200,
            "case {job_type:?}: expected HTTP 200 from StartJob, got {started:?}"
        );
        let start_time = timestamp_member(&started.member("jobSummary"), "startTime");

        // Before the span elapses the job is still the one `StartJob` answered: a read
        // does not settle it early.
        let pending = get_job(&emulator, &app_id, branch, "1").await.member("job");
        assert_eq!(
            string_member(
                &pending.get("summary").cloned().unwrap_or(Value::Null),
                "status"
            ),
            PENDING,
            "case {job_type:?}: the span is non-zero, so a read straight after StartJob \
             still sees PENDING, got {pending:?}"
        );

        sleep(past_the_settle_span());

        let settled = get_job(&emulator, &app_id, branch, "1").await.member("job");
        let summary = settled.get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            FAILED,
            "case {job_type:?}: the clock owes the job a terminal status, got {settled:?}"
        );
        let end_time = timestamp_member(&summary, "endTime");
        assert!(
            end_time >= start_time,
            "case {job_type:?}: a job cannot end before it started ({start_time}), got {end_time}"
        );

        // The reason the job failed reaches the wire on the one member that can carry
        // it: a build nothing executed is not reported as a success.
        let steps = list_member(&settled, "steps").clone();
        assert_eq!(
            steps.len(),
            1,
            "case {job_type:?}: the settled job carries the step its failure is on, \
             got {settled:?}"
        );
        assert_eq!(
            string_member(&steps[0], "status"),
            FAILED,
            "case {job_type:?}: the step must carry the job's status, got {:?}",
            steps[0]
        );
        assert_eq!(
            string_member(&steps[0], "statusReason"),
            NO_BUILD_ENVIRONMENT_REASON,
            "case {job_type:?}: the step must say why the job failed, got {:?}",
            steps[0]
        );
        assert!(
            !string_member(&steps[0], "stepName").is_empty(),
            "case {job_type:?}: Step$stepName is @required, got {:?}",
            steps[0]
        );

        // The settling happened ONCE: a later read reports the identical record, so a
        // read changes nothing a later caller sees.
        let again = get_job(&emulator, &app_id, branch, "1").await.member("job");
        assert_eq!(
            again, settled,
            "case {job_type:?}: a settled job reads back identically, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_branch_keeps_a_bounded_job_history() {
    let emulator = Emulator::new();
    let branch = "main";
    let (app_id, _) = app_with_branch(&emulator, "bounded-history-app", branch).await;

    // Fill the branch's retained history. Every one of these is accepted: the bound is
    // on what the branch KEEPS, not on what a caller may start.
    for sequence in 1..=MAX_JOBS_PER_BRANCH {
        let started = start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" })).await;
        assert_eq!(
            started.status, 200,
            "case \"job {sequence}\": expected HTTP 200 from StartJob, got {started:?}"
        );
        assert_eq!(
            string_member(&started.member("jobSummary"), "jobId"),
            sequence.to_string(),
            "case \"job {sequence}\": job ids count the branch's jobs"
        );
    }

    // At the cap with every retained job still RUNNING there is nothing to retire, so
    // one more start is refused with the modelled quota error rather than growing the
    // store without limit.
    let refused = start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" })).await;
    assert_eq!(
        refused.status, 429,
        "case \"over the cap, all running\": expected the modelled quota refusal, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"over the cap, all running\": expected LimitExceededException, got {:?}",
        refused.error_code
    );
    let message = refused.message().unwrap_or_default();
    assert!(
        message.contains(&MAX_JOBS_PER_BRANCH.to_string()) && message.contains(branch),
        "case \"over the cap, all running\": the refusal must name the limit and the \
         branch, got {message:?}"
    );

    // Once the clock has finished those jobs, the oldest FINISHED job is retired to
    // make room, so a caller is never blocked for good.
    sleep(past_the_settle_span());
    let accepted = start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" })).await;
    let expected_id = (MAX_JOBS_PER_BRANCH + 1).to_string();
    assert_eq!(
        accepted.status, 200,
        "case \"over the cap, oldest finished\": expected HTTP 200, got {accepted:?}"
    );
    assert_eq!(
        string_member(&accepted.member("jobSummary"), "jobId"),
        expected_id,
        "case \"over the cap, oldest finished\": the new job keeps the next minted id"
    );

    // Job 1 was the oldest finished job, so it is the one retired; the rest of the
    // history is intact, and the newest job is readable.
    assert_not_found(
        "the retired job",
        &get_job(&emulator, &app_id, branch, "1").await,
        "Job",
        "1",
    );
    for still_held in ["2", expected_id.as_str()] {
        assert_eq!(
            get_job(&emulator, &app_id, branch, still_held).await.status,
            200,
            "case \"retained job {still_held}\": the rest of the history must survive"
        );
    }

    // `totalNumberOfJobs` counts the jobs the branch has STARTED, so retiring history
    // does not rewrite it.
    let record = emulator.get_branch(&app_id, branch).await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        expected_id,
        "case \"bounded history\": totalNumberOfJobs counts started jobs, got {record:?}"
    );
    assert_eq!(
        string_member(&record, "activeJobId"),
        expected_id,
        "case \"bounded history\": the job just started is the active one, got {record:?}"
    );

    // A RETRY adds no entry, so it is accepted at the cap and re-runs the job it names.
    let retried = start_job(
        &emulator,
        &app_id,
        branch,
        &json!({ "jobType": RETRY, "jobId": "2" }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "case \"RETRY at the cap\": a retry adds no job, so it is not over quota, got {retried:?}"
    );
    assert_eq!(
        string_member(&retried.member("jobSummary"), "jobId"),
        "2",
        "case \"RETRY at the cap\": the retry re-runs the job it names"
    );
    assert_eq!(
        string_member(
            &emulator.get_branch(&app_id, branch).await.member("branch"),
            "totalNumberOfJobs"
        ),
        expected_id,
        "case \"RETRY at the cap\": a retry starts no new job, so the count is unchanged"
    );
}

#[tokio::test]
async fn a_job_is_reachable_only_through_its_own_app_and_branch() {
    let emulator = Emulator::new();
    let branch = "main";
    let (app_id, _) = app_with_branch(&emulator, "reachability-app", branch).await;
    assert_eq!(
        start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" }))
            .await
            .status,
        200,
        "case \"seed job\": expected HTTP 200 from StartJob"
    );

    // An app that does not exist: both operations report the APP absent, whatever the
    // identifier looks like inside `AppId`'s modelled pattern.
    for absent_app in ["d0", "dnosuchapp1", "d9999999999999999999"] {
        assert_not_found(
            &format!("StartJob on absent app {absent_app}"),
            &start_job(
                &emulator,
                absent_app,
                branch,
                &json!({ "jobType": "MANUAL" }),
            )
            .await,
            "App",
            absent_app,
        );
        assert_not_found(
            &format!("GetJob on absent app {absent_app}"),
            &get_job(&emulator, absent_app, branch, "1").await,
            "App",
            absent_app,
        );
    }

    // An existing app and a branch it does not carry: the BRANCH is what is absent,
    // and no branch is created on the way.
    for absent_branch in ["develop", "release-1.2.3", &"b".repeat(255)] {
        assert_not_found(
            &format!("StartJob on absent branch {absent_branch}"),
            &start_job(
                &emulator,
                &app_id,
                absent_branch,
                &json!({ "jobType": "MANUAL" }),
            )
            .await,
            "Branch",
            absent_branch,
        );
        assert_not_found(
            &format!("GetJob on absent branch {absent_branch}"),
            &get_job(&emulator, &app_id, absent_branch, "1").await,
            "Branch",
            absent_branch,
        );
    }

    // Another app's branch of the same name holds its own jobs, so job 1 of this app
    // is not reachable there.
    let (other_app, _) = app_with_branch(&emulator, "other-app", branch).await;
    assert_not_found(
        "GetJob of another app's job id",
        &get_job(&emulator, &other_app, branch, "1").await,
        "Job",
        "1",
    );

    // Deleting the branch takes its jobs with it: a job exists only as part of its
    // branch.
    assert_eq!(
        emulator.delete_branch(&app_id, branch).await.status,
        200,
        "case \"DeleteBranch\": expected HTTP 200"
    );
    assert_not_found(
        "GetJob after DeleteBranch",
        &get_job(&emulator, &app_id, branch, "1").await,
        "Branch",
        branch,
    );
    let recreated = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "case \"CreateBranch again\": expected HTTP 200, got {recreated:?}"
    );
    assert_not_found(
        "GetJob after the branch was re-created",
        &get_job(&emulator, &app_id, branch, "1").await,
        "Job",
        "1",
    );
    assert_eq!(
        string_member(
            &start_job(&emulator, &app_id, branch, &json!({ "jobType": "MANUAL" }))
                .await
                .member("jobSummary"),
            "jobId"
        ),
        "1",
        "case \"CreateBranch again\": a re-created branch counts its jobs from 1"
    );
}
