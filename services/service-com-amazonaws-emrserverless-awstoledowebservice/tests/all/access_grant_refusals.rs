// SPDX-License-Identifier: CC0-1.0

//! How EMR Serverless's ACCESS-GRANT reads refuse a target that does not resolve.
//!
//! An access-grant read is one that MINTS something for a resource inside an
//! application rather than reporting that resource: `GetDashboardForJobRun` "creates and
//! returns a URL that you can use to access the application UIs for a job run", and
//! `GetSessionEndpoint` "returns the session endpoint URL and a time-limited
//! authentication token for the specified session"
//! (`smithy_query(query="operation",operation="GetDashboardForJobRun,GetSessionEndpoint")`).
//!
//! The rule under test: on those reads, an identifier naming no resource the grant could
//! be minted for is refused as INPUT VALIDATION — HTTP 400 with the modelled
//! `ValidationException` shape, both in `__type` and in the `x-amzn-errortype` identity a
//! client branches on — carrying the fixed sentence for that identifier's class and no
//! part of the request. The resource REPORTS of the same resources keep the not-found
//! class (HTTP 404 `ResourceNotFoundException`), so the two classes are asserted side by
//! side here.
//!
//! Every case goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each one exercises routing, deserialization, the handler and
//! serialization — the same path an SDK client takes.

use smithy_server_runtime::MultiServiceRouter;

use super::application_lifecycle::{
    call, emulator, get_job_run, minimal_job_run, start_job_run, started_application,
    wait_for_job_run_to_settle, Answer,
};
use super::session_lifecycle::{minimal_session, session_application, start_session};

/// The sentence the job-run access-grant path states for a job-run id that resolves to
/// nothing. Written out here rather than read from `crate::state`, so a wrong sentence in
/// `src/` cannot make this suite pass. Fixed text for the class, terminated by a period.
const JOB_RUN_ID_INVALID: &str = "Job Run with given ID doesn't exist.";

/// The same, for the session access-grant path: it names the INPUT MEMBER whose value
/// resolved to no session, and carries neither that value nor the application's.
const SESSION_ID_INVALID: &str = "SessionId is invalid.";

/// What the job-run REPORTS say for the same unresolvable id — the not-found class of
/// this service, whose phrase carries no terminating period.
const JOB_RUN_NOT_FOUND: &str = "Job Run with given ID doesn't exist";

/// The same, for an application that does not exist.
const APPLICATION_NOT_FOUND: &str = "Application does not exist";

/// Identifiers that name nothing, differing along what the rule generalises over: the
/// length (1, 12, 32 and the modelled maximum 64 — `ApplicationId`, `JobRunId` and
/// `SessionId` are all `@length(min: 1, max: 64)`, `@pattern ^[0-9a-z]+$`) and the
/// characters (digits only, letters only, both).
fn unresolvable_ids() -> Vec<String> {
    vec![
        "7".to_string(),
        "doesnotexist".to_string(),
        "00fakeidnonexistent0000000000000".to_string(),
        "a".repeat(64),
        "0".repeat(63) + "z",
    ]
}

async fn get_dashboard(
    router: &MultiServiceRouter,
    application_id: &str,
    job_run_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{application_id}/jobruns/{job_run_id}/dashboard{query}"),
        None,
    )
    .await
}

async fn get_session_endpoint(
    router: &MultiServiceRouter,
    application_id: &str,
    session_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{application_id}/sessions/{session_id}/endpoint"),
        None,
    )
    .await
}

/// One expected refusal: the request to make and the class and sentence it must carry.
struct Refusal {
    case: String,
    method: &'static str,
    uri: String,
    status: u16,
    shape: &'static str,
    message: &'static str,
}

/// Assert `refusal` is answered with the status, error shape and sentence it names, and
/// that nothing of the request's identifiers reached the body.
async fn assert_refused(router: &MultiServiceRouter, refusal: &Refusal, submitted_id: &str) {
    let Refusal {
        case,
        method,
        uri,
        status,
        shape,
        message,
    } = refusal;
    let answer = call(router, method, uri, None).await;
    assert_eq!(
        answer.status, *status,
        "case {case:?}: expected status {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        *shape,
        "case {case:?}: expected the {shape:?} shape in __type, got {:?} with {:?}",
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.resolved_error_shape(),
        Some(*shape),
        "case {case:?}: expected x-amzn-errortype to resolve to {shape:?}, got {:?}",
        answer.resolved_error_shape()
    );
    assert_eq!(
        answer.at("/message").as_str().unwrap_or_default(),
        *message,
        "case {case:?}: expected the fixed sentence {message:?}, got {:?}",
        answer.body
    );
    assert!(
        !answer.body.to_string().contains(submitted_id),
        "case {case:?}: the submitted identifier {submitted_id:?} must not be echoed, got {:?}",
        answer.body
    );
}

/// The rule, over the table of unresolvable identifiers: an access-grant read refuses one
/// as a 400 `ValidationException`, while the reports of the same resources keep their 404
/// `ResourceNotFoundException`.
#[tokio::test]
async fn an_access_grant_read_refuses_an_unresolvable_target_as_input_validation() {
    let router = emulator();

    // One application with a job run, and one that serves a session, so each row below
    // differs only in the identifier it submits.
    let job_application = started_application(&router, "tok-grant-jobs").await;
    let submitted = start_job_run(
        &router,
        &job_application,
        minimal_job_run("tok-grant-job-run"),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (session_application_id, _) = session_application(&router, "tok-grant-sessions").await;
    let started = start_session(
        &router,
        &session_application_id,
        minimal_session("tok-grant-session"),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSession expected 200, got {} with {:?}",
        started.status, started.body
    );
    let session_id = started
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    for unknown in unresolvable_ids() {
        let rows = vec![
            // The grant target itself does not resolve: input validation.
            Refusal {
                case: format!("GetDashboardForJobRun(unknown job run {unknown})"),
                method: "GET",
                uri: format!("/applications/{job_application}/jobruns/{unknown}/dashboard"),
                status: 400,
                shape: "ValidationException",
                message: JOB_RUN_ID_INVALID,
            },
            Refusal {
                case: format!("GetSessionEndpoint(unknown session {unknown})"),
                method: "GET",
                uri: format!(
                    "/applications/{session_application_id}/sessions/{unknown}/endpoint"
                ),
                status: 400,
                shape: "ValidationException",
                message: SESSION_ID_INVALID,
            },
            // A session id names one session service-wide, so the reference is resolved
            // as a whole: an application that does not exist means the session asked for
            // is not there to mint a grant for either.
            Refusal {
                case: format!("GetSessionEndpoint(unknown application {unknown})"),
                method: "GET",
                uri: format!("/applications/{unknown}/sessions/{session_id}/endpoint"),
                status: 400,
                shape: "ValidationException",
                message: SESSION_ID_INVALID,
            },
            Refusal {
                case: format!("GetSessionEndpoint(neither id resolves, {unknown})"),
                method: "GET",
                uri: format!("/applications/{unknown}/sessions/{unknown}/endpoint"),
                status: 400,
                shape: "ValidationException",
                message: SESSION_ID_INVALID,
            },
            // The other side of the rule: a REPORT of the same resource keeps the
            // not-found class, and so does the application a job-run grant is scoped to.
            Refusal {
                case: format!("GetJobRun(unknown job run {unknown})"),
                method: "GET",
                uri: format!("/applications/{job_application}/jobruns/{unknown}"),
                status: 404,
                shape: "ResourceNotFoundException",
                message: JOB_RUN_NOT_FOUND,
            },
            Refusal {
                case: format!("GetDashboardForJobRun(unknown application {unknown})"),
                method: "GET",
                uri: format!("/applications/{unknown}/jobruns/{job_run_id}/dashboard"),
                status: 404,
                shape: "ResourceNotFoundException",
                message: APPLICATION_NOT_FOUND,
            },
        ];
        for row in &rows {
            assert_refused(&router, row, &unknown).await;
        }
    }

    // The sentence the grant path states differs from the report's only by the period the
    // validation class terminates its fixed text with — asserted so neither constant can
    // drift onto the other.
    assert_eq!(
        JOB_RUN_ID_INVALID,
        format!("{JOB_RUN_NOT_FOUND}."),
        "the validation sentence is the class's fixed text terminated by a period"
    );

    // And a target that DOES resolve is still served, so the refusal is the lookup and
    // not the operation.
    let dashboard = get_dashboard(&router, &job_application, &job_run_id, "").await;
    assert_eq!(
        dashboard.status, 200,
        "GetDashboardForJobRun on the stored run expected 200, got {} with {:?}",
        dashboard.status, dashboard.body
    );
    assert!(
        dashboard
            .at("/url")
            .as_str()
            .is_some_and(|url| url.starts_with("https://")),
        "GetDashboardForJobRun on the stored run must mint a url, got {:?}",
        dashboard.body
    );
    let endpoint = get_session_endpoint(&router, &session_application_id, &session_id).await;
    assert_eq!(
        endpoint.status, 200,
        "GetSessionEndpoint on the stored session expected 200, got {} with {:?}",
        endpoint.status, endpoint.body
    );
    assert!(
        endpoint.at("/authToken").as_str().is_some(),
        "GetSessionEndpoint on the stored session must mint a token, got {:?}",
        endpoint.body
    );
}

/// The grant reads change nothing a later caller sees when they refuse: a refused call
/// leaves the resources that DO exist exactly as they were.
#[tokio::test]
async fn a_refused_access_grant_read_leaves_the_stored_resources_alone() {
    let router = emulator();
    let application_id = started_application(&router, "tok-grant-readonly").await;
    let submitted = start_job_run(
        &router,
        &application_id,
        minimal_job_run("tok-grant-readonly-run"),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    // Read once the run has settled, so the clock owes it no further change and two
    // reports taken at different instants are comparable.
    wait_for_job_run_to_settle();

    let before = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        before.status, 200,
        "GetJobRun expected 200, got {} with {:?}",
        before.status, before.body
    );
    for unknown in unresolvable_ids() {
        let refused = get_dashboard(&router, &application_id, &unknown, "").await;
        assert_eq!(
            refused.status, 400,
            "case {unknown:?}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
    }
    let after = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        after.body, before.body,
        "a refused dashboard call must leave the stored run as it found it, got {:?} then {:?}",
        before.body, after.body
    );
}
