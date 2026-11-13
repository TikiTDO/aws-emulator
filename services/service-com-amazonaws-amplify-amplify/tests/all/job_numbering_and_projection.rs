// SPDX-License-Identifier: CC0-1.0

//! How this service NUMBERS a branch's jobs and PROJECTS a job summary.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A job summary reports a member only where the job HAS a value for it, and the two
//!    ways it names the job are not the same string: `jobId` is the branch's bare job
//!    number and the last segment of `jobArn` is that number in a ten-digit field. A job
//!    nothing has started holds a number and no work, so it reports neither commit
//!    information, nor a start or end time, nor a job type — and its status is the
//!    `PENDING` a client polls a deployment through. Asserted wherever a summary is
//!    answered: `GetJob`, `ListJobs`, `StopJob` and `DeleteJob`.
//! 2. Stopping a job NOTHING HAS STARTED is complete when the call answers: that call's
//!    own response carries the terminal `CANCELLED`, and the branch is deployable again
//!    by the very next call, which is what `StopJob` is the documented way out for.
//! 3. Every `CreateDeployment` call spends the branch's next job number and records a
//!    job for it — including a call the serialization rule refuses — so the refusal's
//!    message names the number THAT call spent, which is by then the branch's last job.
//! 4. A branch's job history therefore carries one job per `CreateDeployment` call made
//!    on it, which `ListJobs` reports newest first with each job's own status.
//! 5. The ten-digit field of `jobArn` holds the job number whatever its width and
//!    whichever call made the job — a reserved deployment, or a job `StartJob` started.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// How wide the job-number field of a `jobArn` is.
///
/// `JobArn` is modelled as free text and `JobId` as `pattern ^[0-9]+$`
/// (`smithy_query(query="shape",shape="JobSummary")`), so the form is recorded rather
/// than modelled: Amplify writes the job number into a fixed-width field of leading
/// zeroes in the ARN while `jobId` reports it bare.
const JOB_ARN_NUMBER_DIGITS: usize = 10;

/// The members a job NOTHING HAS RUN reports no value for.
///
/// Source: `JobSummary`'s member list
/// (`smithy_query(query="shape",shape="JobSummary")`) minus the three members such a job
/// does have a value for — `jobArn`, `jobId` and `status`. `commitId`, `commitMessage`
/// and `commitTime` describe a commit "from a third-party repository provider", which a
/// manually deployed app has none of; `startTime` and `endTime` are the job's execution,
/// which has not happened; `jobType` is settled by the call that STARTS a job; and
/// `sourceUrl`/`sourceUrlType` describe the files a started deployment deployed.
const UNREPORTED_WITHOUT_WORK: &[&str] = &[
    "commitId",
    "commitMessage",
    "commitTime",
    "startTime",
    "endTime",
    "jobType",
    "sourceUrl",
    "sourceUrlType",
];

/// The status a job carries while nothing has finished it.
const PENDING: &str = "PENDING";

/// The status a stopped job has reached the end of its life in.
const CANCELLED: &str = "CANCELLED";

/// `CreateDeployment` over the wire.
async fn create_deployment(
    emulator: &Emulator,
    app_id: &str,
    branch: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/deployments"),
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

/// `ListJobs` over the wire.
async fn list_jobs(emulator: &Emulator, app_id: &str, branch: &str) -> Reply {
    emulator
        .call("GET", &format!("/apps/{app_id}/branches/{branch}/jobs"), None)
        .await
}

/// `StopJob` over the wire.
async fn stop_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}/stop"),
            None,
        )
        .await
}

/// `DeleteJob` over the wire.
async fn delete_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
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

/// A branch of `app_id`, and the branch ARN every `jobArn` of it extends.
async fn branch_arn_of_new_branch(emulator: &Emulator, app_id: &str, branch: &str) -> String {
    let created = emulator
        .create_branch(app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "branch {branch:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    string_member(&created.member("branch"), "branchArn")
}

/// The `summary` of a `GetJob` answer.
fn read_summary(reply: &Reply) -> Value {
    reply
        .member("job")
        .get("summary")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Rule 1, applied to one answered summary: the two names of the job, the status, and
/// the members a job with no work reports nothing for.
fn assert_summary_of_a_job_without_work(
    case: &str,
    answered_by: &str,
    summary: &Value,
    branch_arn: &str,
    job_id: &str,
    status: &str,
) {
    assert_eq!(
        string_member(summary, "jobId"),
        job_id,
        "case {case:?}: {answered_by} must report the bare job number, got {summary:?}"
    );
    let padded = format!("{job_id:0>width$}", width = JOB_ARN_NUMBER_DIGITS);
    assert_eq!(
        string_member(summary, "jobArn"),
        format!("{branch_arn}/jobs/{padded}"),
        "case {case:?}: {answered_by} must address the job with its number in a \
         {JOB_ARN_NUMBER_DIGITS}-digit field, got {summary:?}"
    );
    assert_eq!(
        string_member(summary, "status"),
        status,
        "case {case:?}: {answered_by} must report the status {status}, got {summary:?}"
    );
    for member in UNREPORTED_WITHOUT_WORK {
        assert!(
            summary.get(member).is_none(),
            "case {case:?}: {answered_by} must leave out {member}, which a job nothing \
             ran has no value for, got {summary:?}"
        );
    }
}

/// The ways a `CreateDeployment` request can describe what it is about to upload: the
/// rule under test is about the JOB a call creates, which is the same whatever the call
/// asked to upload.
fn request_forms() -> Vec<(&'static str, Value)> {
    // `MD5Hash` is modelled `length 0..32` (`smithy_query(query="shape",shape="FileMap")`)
    // and nothing in these rules reads it.
    let hash = "d41d8cd98f00b204e9800998ecf8427e";
    vec![
        ("no fileMap member", json!({})),
        ("fileMap naming no file", json!({ "fileMap": {} })),
        (
            "one named file",
            json!({ "fileMap": { "index.html": hash } }),
        ),
        (
            "several named files",
            json!({ "fileMap": { "index.html": hash, "assets/app.js": hash } }),
        ),
    ]
}

/// Rules 1 and 2: a job nothing started reports only what it has, wherever it is
/// answered — and stopping it is complete in the stop's own answer.
#[tokio::test]
async fn a_job_nothing_started_reports_only_what_it_has() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("job-projection").await;

    for (index, (case, request)) in request_forms().into_iter().enumerate() {
        // One row per BRANCH: a branch serializes its deployments, and this rule is
        // about one job's answers.
        let branch = format!("row-{index}");
        let branch_arn = branch_arn_of_new_branch(&emulator, &app_id, &branch).await;

        let reserved = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            reserved.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
        );
        let job_id = string_member(&reserved.body, "jobId");
        assert_eq!(
            job_id, "1",
            "case {case:?}: the first call on a branch spends job number 1, got {job_id:?}"
        );

        let read = get_job(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the call's job must be readable, got {read:?}"
        );
        assert_summary_of_a_job_without_work(
            case,
            "GetJob",
            &read_summary(&read),
            &branch_arn,
            &job_id,
            PENDING,
        );
        assert!(
            list_member(&read.member("job"), "steps").is_empty(),
            "case {case:?}: a job that has run nothing carries no step, got {read:?}"
        );

        let listed = list_jobs(&emulator, &app_id, &branch).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListJobs, got {listed:?}"
        );
        let summaries = list_member(&listed.body, "jobSummaries");
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: one call made, so the branch holds one job, got {listed:?}"
        );
        assert_eq!(
            summaries[0],
            read_summary(&read),
            "case {case:?}: a listed job must be the record GetJob answers, got {listed:?}"
        );

        // Rule 2: the stop's OWN answer is terminal.
        let stopped = stop_job(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopJob, got {stopped:?}"
        );
        assert_summary_of_a_job_without_work(
            case,
            "StopJob",
            &stopped.member("jobSummary"),
            &branch_arn,
            &job_id,
            CANCELLED,
        );

        // ... and the job stays there: a later read is the same record, not a status
        // some clock is still owed.
        let after = get_job(&emulator, &app_id, &branch, &job_id).await;
        assert_summary_of_a_job_without_work(
            case,
            "GetJob after StopJob",
            &read_summary(&after),
            &branch_arn,
            &job_id,
            CANCELLED,
        );

        // A stop that has been answered leaves the branch deployable, by the very next
        // call: that is what the serialization refusal tells a caller to do.
        let again = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a stopped last job must leave the branch deployable, got {again:?}"
        );
        assert_eq!(
            string_member(&again.body, "jobId"),
            "2",
            "case {case:?}: the next call spends the next number, got {again:?}"
        );

        // `DeleteJob` answers a summary too, and answers it the same way.
        let deleted = delete_job(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteJob, got {deleted:?}"
        );
        assert_summary_of_a_job_without_work(
            case,
            "DeleteJob",
            &deleted.member("jobSummary"),
            &branch_arn,
            &job_id,
            CANCELLED,
        );
    }
}

/// Rules 3 and 4: every call spends a number and records its job, the refusal names the
/// number it spent, and the branch's history is one job per call.
#[tokio::test]
async fn every_create_deployment_call_spends_a_job_number() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("job-numbering").await;

    // One row per request form, each with one more refused call than the row before it:
    // what the rule turns on is how many calls the serialization rule refuses before the
    // branch is freed, and the form of the request is not part of it.
    for (refusals, (case, request)) in request_forms().into_iter().enumerate() {
        let refusals = refusals + 1;
        let branch = format!("refused-{refusals}");
        let branch_arn = branch_arn_of_new_branch(&emulator, &app_id, &branch).await;

        let first = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200 from the first CreateDeployment, got {first:?}"
        );
        assert_eq!(
            string_member(&first.body, "jobId"),
            "1",
            "case {case:?}: the first call spends job number 1, got {first:?}"
        );

        for spent in 2..=(refusals + 1) {
            let refused = create_deployment(&emulator, &app_id, &branch, &request).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a deployment ordered against an unfinished last job is \
                 refused with the modelled 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                refused.error_code
            );
            let expected = format!(
                "The last job(deployment) {spent} was not finished, please wait until it \
                 finish or use StopJob operation to cancel it before creating a new deployment."
            );
            assert_eq!(
                refused.message().as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                refused.message()
            );
            assert!(
                refused.body.get("jobId").is_none()
                    && refused.body.get("zipUploadUrl").is_none(),
                "case {case:?}: a refused call hands back no destination, got {refused:?}"
            );

            // The number it spent IS a job of the branch — which is what makes the
            // message actionable, since it names the job to stop.
            let read = get_job(&emulator, &app_id, &branch, &spent.to_string()).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the job the refusal names must be readable, got {read:?}"
            );
            assert_summary_of_a_job_without_work(
                case,
                "GetJob of a refused call's job",
                &read_summary(&read),
                &branch_arn,
                &spent.to_string(),
                PENDING,
            );
        }

        // Rule 4: one job per call made, newest first, each in its own status.
        let calls = refusals + 1;
        let expected: Vec<(String, &str)> = (1..=calls)
            .rev()
            .map(|number| (number.to_string(), PENDING))
            .collect();
        assert_history(&emulator, &app_id, &branch, case, &expected).await;

        // Stopping the branch's LAST job frees the branch; the earlier unfinished ones
        // stay exactly as they are, and the next call spends the next number.
        let last = calls.to_string();
        let stopped = stop_job(&emulator, &app_id, &branch, &last).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopJob, got {stopped:?}"
        );
        let next = create_deployment(&emulator, &app_id, &branch, &request).await;
        assert_eq!(
            next.status, 200,
            "case {case:?}: a stopped last job leaves the branch deployable, got {next:?}"
        );
        assert_eq!(
            string_member(&next.body, "jobId"),
            (calls + 1).to_string(),
            "case {case:?}: the accepted call spends the next number, got {next:?}"
        );

        let mut expected: Vec<(String, &str)> = vec![((calls + 1).to_string(), PENDING)];
        expected.push((last, CANCELLED));
        expected.extend(
            (1..calls)
                .rev()
                .map(|number| (number.to_string(), PENDING)),
        );
        assert_history(&emulator, &app_id, &branch, case, &expected).await;
    }
}

/// `ListJobs` reports exactly `expected` — `(jobId, status)` newest first.
async fn assert_history(
    emulator: &Emulator,
    app_id: &str,
    branch: &str,
    case: &str,
    expected: &[(String, &str)],
) {
    let listed = list_jobs(emulator, app_id, branch).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListJobs, got {listed:?}"
    );
    let reported: Vec<(String, String)> = list_member(&listed.body, "jobSummaries")
        .iter()
        .map(|summary| {
            (
                string_member(summary, "jobId"),
                string_member(summary, "status"),
            )
        })
        .collect();
    let wanted: Vec<(String, String)> = expected
        .iter()
        .map(|(job_id, status)| (job_id.clone(), (*status).to_string()))
        .collect();
    assert_eq!(
        reported, wanted,
        "case {case:?}: expected the branch's whole history newest first, got {listed:?}"
    );
}

/// Rule 5: the job number reaches the ARN's field whatever its width, and whichever call
/// made the job.
#[tokio::test]
async fn a_job_arn_carries_the_job_number_at_every_width() {
    let emulator = Emulator::new();
    // A repository app, because the second half of this case starts a BUILD, which is
    // served only for an app connected to a repository provider.
    let app_id = emulator
        .app_id_of_new_repository_app("job-arn-widths")
        .await;
    let branch_arn = branch_arn_of_new_branch(&emulator, &app_id, "main").await;

    // Past the first decade of job numbers, so the field is asserted at more than one
    // width of number.
    for number in 1..=12u64 {
        let reserved = create_deployment(&emulator, &app_id, "main", &json!({})).await;
        assert_eq!(
            reserved.status, 200,
            "case \"job {number}\": expected HTTP 200 from CreateDeployment, got {reserved:?}"
        );
        let job_id = string_member(&reserved.body, "jobId");
        assert_eq!(
            job_id,
            number.to_string(),
            "case \"job {number}\": a branch's numbers are a sequence, got {reserved:?}"
        );
        let read = get_job(&emulator, &app_id, "main", &job_id).await;
        assert_summary_of_a_job_without_work(
            &format!("job {number}"),
            "GetJob",
            &read_summary(&read),
            &branch_arn,
            &job_id,
            PENDING,
        );
        // Stopped, so the branch takes the next deployment.
        let stopped = stop_job(&emulator, &app_id, "main", &job_id).await;
        assert_eq!(
            stopped.status, 200,
            "case \"job {number}\": expected HTTP 200 from StopJob, got {stopped:?}"
        );
    }

    // A job that RAN is addressed the same way — and it reports the members a started
    // job does have values for, so the ARN rule is not the projection's.
    let started = start_job(&emulator, &app_id, "main", &json!({ "jobType": "RELEASE" })).await;
    assert_eq!(
        started.status, 200,
        "case \"a started build\": expected HTTP 200 from StartJob, got {started:?}"
    );
    let summary = started.member("jobSummary");
    let job_id = string_member(&summary, "jobId");
    assert_eq!(
        job_id, "13",
        "case \"a started build\": the build takes the branch's next number, got {summary:?}"
    );
    assert_eq!(
        string_member(&summary, "jobArn"),
        format!("{branch_arn}/jobs/0000000013"),
        "case \"a started build\": the ARN carries the padded number, got {summary:?}"
    );
    assert_eq!(
        string_member(&summary, "jobType"),
        "RELEASE",
        "case \"a started build\": a job that was started reports its type, got {summary:?}"
    );
    assert!(
        summary.get("startTime").is_some(),
        "case \"a started build\": a job that was started reports its start, got {summary:?}"
    );
}
