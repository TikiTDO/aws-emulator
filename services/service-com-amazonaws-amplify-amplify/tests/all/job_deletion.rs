// SPDX-License-Identifier: CC0-1.0

//! `DeleteJob`: deleting a job from a branch of an Amplify app.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over:
//!
//! 1. "Deletes a job for a branch of an Amplify app"
//!    (`smithy_query(query="doc",shape="DeleteJob")`): the call answers the deleted
//!    job's `jobSummary` — the record a read answered an instant earlier — and the job
//!    then belongs to the branch no longer: `GetJob` is the modelled 404, `ListJobs`
//!    lists it no more, and a second delete is that same 404. The rule takes no view of
//!    the job's status, so the table has one row per status a job of this service is in:
//!    created, in flight, failed, cancelled and succeeded.
//! 2. A job is deleted THROUGH its branch and its app, so an unknown app, an unknown
//!    branch or a job id belonging to another branch is the modelled 404, and nothing is
//!    deleted on the way.
//! 3. `Branch$totalNumberOfJobs` counts the jobs a branch has STARTED, so deleting one
//!    out of its history does not rewrite that count.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    JOB_SETTLE_MILLIS, JOB_STATUS_CANCELLED, JOB_STATUS_FAILED,
    JOB_STATUS_SUCCEED,
};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// The non-terminal status a job carries before the clock settles it: the one `StartJob`
/// answers with, and the one the job of a reserved deployment holds until something
/// starts, stops or expires it.
const PENDING: &str = "PENDING";

/// How long a case waits before expecting the status change the clock already owed.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 250)
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

/// The `jobId`s `ListJobs` reports for a branch.
async fn listed_job_ids(emulator: &Emulator, app_id: &str, branch: &str) -> Vec<String> {
    let listed = emulator
        .call("GET", &format!("/apps/{app_id}/branches/{branch}/jobs"), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "ListJobs on {branch:?}: expected HTTP 200, got {listed:?}"
    );
    list_member(&listed.body, "jobSummaries")
        .iter()
        .map(|summary| string_member(summary, "jobId"))
        .collect()
}

/// A `.zip` carrying each named path with the content given, as a real archive a manual
/// deployment can be.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for (path, content) in files {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        std::io::Write::write_all(&mut writer, content).expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// What state a row's job is in when it is deleted.
#[derive(Debug, Clone, Copy)]
enum JobState {
    /// A deployment `CreateDeployment` reserved and nothing started.
    Reserved,
    /// A build `StartJob` started and the clock has not settled yet.
    InFlight,
    /// A build the clock settled (this emulator runs no build, so it ends failed).
    Settled,
    /// A job a `StopJob` stopped and the clock settled.
    Stopped,
    /// A manual deployment that published its content.
    Deployed,
}

/// An app with one branch, and a job on that branch in the state the row asks for.
async fn job_in_state(
    emulator: &Emulator,
    case: &str,
    branch: &str,
    state: JobState,
) -> (String, String) {
    let app_id = match state {
        JobState::InFlight | JobState::Settled | JobState::Stopped => {
            // `StartJob` builds the app's connected repository, so it is served only
            // for an app that has one.
            emulator.app_id_of_new_repository_app(case).await
        }
        JobState::Reserved | JobState::Deployed => emulator.app_id_of_new_app(case).await,
    };
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );

    let job_id = match state {
        JobState::InFlight | JobState::Settled | JobState::Stopped => {
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/jobs"),
                    Some(&json!({ "jobType": "RELEASE" })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartJob, got {started:?}"
            );
            let job_id = string_member(&started.member("jobSummary"), "jobId");
            if matches!(state, JobState::Stopped) {
                let stopped = emulator
                    .call(
                        "DELETE",
                        &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}/stop"),
                        None,
                    )
                    .await;
                assert_eq!(
                    stopped.status, 200,
                    "case {case:?}: expected HTTP 200 from StopJob, got {stopped:?}"
                );
            }
            if matches!(state, JobState::Settled | JobState::Stopped) {
                sleep(past_the_settle_span());
            }
            job_id
        }
        JobState::Reserved | JobState::Deployed => {
            let reserved = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/deployments"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                reserved.status, 200,
                "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
            );
            let job_id = string_member(&reserved.body, "jobId");
            if matches!(state, JobState::Deployed) {
                let upload = http::Request::builder()
                    .method("PUT")
                    .uri(string_member(&reserved.body, "zipUploadUrl"))
                    .body(bytes::Bytes::from(zip_of(&[(
                        "index.html",
                        b"<html>deployed</html>" as &[u8],
                    )])))
                    .expect("an upload destination is a well-formed URL");
                assert_eq!(
                    emulator.dispatch(upload).await.status().as_u16(),
                    200,
                    "case {case:?}: the destination must accept the upload"
                );
                let started = emulator
                    .call(
                        "POST",
                        &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
                        Some(&json!({ "jobId": job_id })),
                    )
                    .await;
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: expected HTTP 200 from StartDeployment, got {started:?}"
                );
                sleep(past_the_settle_span());
            }
            job_id
        }
    };
    (app_id, job_id)
}

/// Rule 1: a deleted job is answered and then gone, whatever status it was in.
#[tokio::test]
async fn a_deleted_job_is_answered_and_leaves_its_branchs_history() {
    let emulator = Emulator::new();

    // One row per status a job of this service is in, with the branch the row drives it
    // on and the status the row's job must be in when it is deleted.
    let rows: Vec<(&str, &str, JobState, &str)> = vec![
        (
            "a reserved deployment nothing started",
            "trunk",
            JobState::Reserved,
            PENDING,
        ),
        (
            "a build still in flight",
            "inflight",
            JobState::InFlight,
            PENDING,
        ),
        (
            "a build the clock settled",
            "settled",
            JobState::Settled,
            JOB_STATUS_FAILED,
        ),
        (
            "a job a StopJob stopped",
            "stopped",
            JobState::Stopped,
            JOB_STATUS_CANCELLED,
        ),
        (
            "a deployment that published its content",
            "deployed",
            JobState::Deployed,
            JOB_STATUS_SUCCEED,
        ),
    ];

    for (case, branch, state, expected_status) in rows {
        let (app_id, job_id) = job_in_state(&emulator, case, branch, state).await;

        let read = get_job(&emulator, &app_id, branch, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the job must be readable before it is deleted, got {read:?}"
        );
        let before = read
            .member("job")
            .get("summary")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string_member(&before, "status"),
            expected_status,
            "case {case:?}: the row must reach {expected_status}, got {before:?}"
        );
        assert!(
            listed_job_ids(&emulator, &app_id, branch)
                .await
                .iter()
                .any(|listed| listed == &job_id),
            "case {case:?}: ListJobs must carry the job before it is deleted"
        );

        let deleted = delete_job(&emulator, &app_id, branch, &job_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteJob, got {deleted:?}"
        );
        assert_eq!(
            deleted.member("jobSummary"),
            before,
            "case {case:?}: DeleteJob answers the summary the job was deleted in, got {:?}",
            deleted.member("jobSummary")
        );

        // Gone: the read, the listing and a second delete all agree.
        let absent = get_job(&emulator, &app_id, branch, &job_id).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a deleted job is no longer readable, got {absent:?}"
        );
        assert_eq!(
            absent.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {absent:?}"
        );
        assert_eq!(
            absent.message().as_deref(),
            Some(format!("Job {job_id} not found.").as_str()),
            "case {case:?}: the 404 names the job the request addressed, got {:?}",
            absent.message()
        );
        assert!(
            !listed_job_ids(&emulator, &app_id, branch)
                .await
                .contains(&job_id),
            "case {case:?}: ListJobs must not carry a deleted job"
        );
        let again = delete_job(&emulator, &app_id, branch, &job_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a job can be deleted once, got {again:?}"
        );
        assert_eq!(
            again.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {again:?}"
        );

        // Rule 3: the count of jobs the branch has STARTED is not rewritten by deleting
        // one out of its history — a reserved deployment nothing started counts none.
        let record = emulator.get_branch(&app_id, branch).await.member("branch");
        let expected_started = if matches!(state, JobState::Reserved) { "0" } else { "1" };
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            expected_started,
            "case {case:?}: deleting a job does not un-start it, got {record:?}"
        );
    }
}

/// Rule 2: a job is deleted only through its own app and branch.
#[tokio::test]
async fn a_job_is_deletable_only_through_its_own_app_and_branch() {
    let emulator = Emulator::new();
    let (app_id, job_id) =
        job_in_state(&emulator, "delete-reachability", "main", JobState::Settled).await;

    // A second branch of the same app, with no job of its own: the first branch's job id
    // names nothing there.
    let other = "feature-x";
    assert_eq!(
        emulator
            .create_branch(&app_id, &json!({ "branchName": other }))
            .await
            .status,
        200,
        "case \"the second branch\": expected HTTP 200 from CreateBranch"
    );

    // Each row: the app, the branch and the job id the request addresses, and the entity
    // plus identifier its 404 must name.
    let rows: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "an app that does not exist",
            "dnosuchapp1".to_string(),
            "main".to_string(),
            job_id.clone(),
            "App",
            "dnosuchapp1".to_string(),
        ),
        (
            "a branch the app does not carry",
            app_id.clone(),
            "no-such-branch".to_string(),
            job_id.clone(),
            "Branch",
            "no-such-branch".to_string(),
        ),
        (
            "a job id of another branch",
            app_id.clone(),
            other.to_string(),
            job_id.clone(),
            "Job",
            job_id.clone(),
        ),
        (
            "a job id nothing was started under",
            app_id.clone(),
            "main".to_string(),
            "4242".to_string(),
            "Job",
            "4242".to_string(),
        ),
    ];

    for (case, app, branch, job, entity, identifier) in rows {
        let refused = delete_job(&emulator, &app, &branch, &job).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.message().as_deref(),
            Some(format!("{entity} {identifier} not found.").as_str()),
            "case {case:?}: the 404 must name the absent {entity}, got {:?}",
            refused.message()
        );
    }

    // Nothing was deleted on the way: the job is still readable where it lives.
    assert_eq!(
        get_job(&emulator, &app_id, "main", &job_id).await.status,
        200,
        "case \"the addressed job\": a refused delete must leave it alone"
    );
}

/// Rule 1's other half for a RESERVED deployment: with its job deleted there is nothing
/// left to start, so the reservation is spent and `StartDeployment` refuses it.
#[tokio::test]
async fn deleting_a_reserved_deployments_job_spends_the_reservation() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("delete-reservation").await;
    assert_eq!(
        emulator
            .create_branch(&app_id, &json!({ "branchName": "main" }))
            .await
            .status,
        200,
        "case \"the branch\": expected HTTP 200 from CreateBranch"
    );
    let reserved = emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/main/deployments"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        reserved.status, 200,
        "case \"the reservation\": expected HTTP 200 from CreateDeployment, got {reserved:?}"
    );
    let job_id = string_member(&reserved.body, "jobId");
    let upload = http::Request::builder()
        .method("PUT")
        .uri(string_member(&reserved.body, "zipUploadUrl"))
        .body(bytes::Bytes::from(zip_of(&[(
            "index.html",
            b"<html>never deployed</html>" as &[u8],
        )])))
        .expect("an upload destination is a well-formed URL");
    assert_eq!(
        emulator.dispatch(upload).await.status().as_u16(),
        200,
        "case \"the reservation\": the destination must accept the upload"
    );

    assert_eq!(
        delete_job(&emulator, &app_id, "main", &job_id).await.status,
        200,
        "case \"the reservation\": expected HTTP 200 from DeleteJob"
    );

    let started = emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/main/deployments/start"),
            Some(&json!({ "jobId": job_id })),
        )
        .await;
    assert_eq!(
        started.status, 404,
        "case \"the reservation\": a deleted job cannot be started, got {started:?}"
    );
    assert_eq!(
        started.error_code.as_deref(),
        Some("NotFoundException"),
        "case \"the reservation\": expected the modelled NotFoundException, got {started:?}"
    );

    // The branch is free again: its last job is gone, so a further deployment reserves.
    let again = emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/main/deployments"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        again.status, 200,
        "case \"after the delete\": the branch takes a further deployment, got {again:?}"
    );
    assert_ne!(
        string_member(&again.body, "jobId"),
        job_id,
        "case \"after the delete\": the new reservation gets its own id, got {again:?}"
    );
}
