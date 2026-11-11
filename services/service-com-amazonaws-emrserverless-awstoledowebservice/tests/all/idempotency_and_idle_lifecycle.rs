// SPDX-License-Identifier: CC0-1.0

//! Idempotency, inventory, the job run lifecycle, the dashboard and the idle stop.
//!
//! Every case goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each one exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes. Six rules are
//! under test, each asserted over a SET of inputs rather than over one recorded request:
//!
//! 1. An `@idempotencyToken` on an `@idempotent` create names ONE request: replayed with
//!    the same parameters it reports the resource it already created, and replayed with
//!    different ones it is refused with the modelled `ConflictException` (HTTP 409)
//!    without creating or touching anything.
//! 2. A soft delete keeps the resource readable in its terminal state and drops it out
//!    of the inventory a list reports when the request names no state filter; naming the
//!    terminal state explicitly still reports it.
//! 3. An `@idempotent` cancel of a resource that is ALREADY in the state the cancel
//!    leads to succeeds and changes nothing; one that finished some other way is refused
//!    with a message stating the condition rather than the state it found.
//! 4. A job run is not finished the instant it is submitted: it passes through the
//!    non-terminal `JobRunState` values, so a client that polls or filters for in-flight
//!    runs observes it in them.
//! 5. `GetDashboardForJobRun` answers the modelled 200 with an `https` `url`, freshly
//!    minted per call, and refuses what did not resolve in the class that part of the
//!    request belongs to: the application it is scoped to as a 404 not-found, the job run
//!    the URL would be minted for as a 400 validation failure.
//! 6. `autoStopConfiguration` is ENFORCED: an application that has been idle past the
//!    idle timeout it names stops itself, with no `StopApplication` call.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::{
    idle_span, ComAmazonawsEmrserverlessAwstoledowebserviceState, JOB_RUN_STAGES,
    JOB_RUN_STAGE_TRANSITION,
};

use super::application_lifecycle::{
    call, cancel_job_run, create_application, created_application, delete_application, emulator,
    emulator_with_engine, get_application, get_job_run, list_applications, list_job_runs,
    minimal_create, minimal_job_run, start_application, started_application, stop_application,
    wait_for_job_run_to_settle, wait_for_transition, Answer, EXECUTION_ROLE,
};

/// The `JobRunState` values a run can be in while it is still in flight, written out
/// here rather than read from `crate::state` so a wrong catalog in `src/` cannot make
/// this test pass. Source: the `JobRunState` enum (`SUBMITTED`, `PENDING`, `SCHEDULED`,
/// `RUNNING`, `SUCCESS`, `FAILED`, `CANCELLING`, `CANCELLED`, `QUEUED`) minus the three
/// a finished run carries.
const NON_TERMINAL_JOB_RUN_STATES: &[&str] = &[
    "SUBMITTED",
    "PENDING",
    "SCHEDULED",
    "QUEUED",
    "RUNNING",
    "CANCELLING",
];

/// The query that asks `ListJobRuns` for the runs that are still in flight.
const NON_TERMINAL_STATES_QUERY: &str =
    "?states=SUBMITTED&states=PENDING&states=SCHEDULED&states=QUEUED&states=RUNNING";

/// The `ValidationException` message a cancel that cannot act on the run answers with:
/// it states the condition, so it is the same sentence whatever state the run is in.
const NOT_CANCELLABLE: &str = "Job run is not in a cancellable state";

/// The `ResourceNotFoundException` phrases, one per part of the request that can fail to
/// resolve.
const APPLICATION_NOT_FOUND: &str = "Application does not exist";
const JOB_RUN_ATTEMPT_NOT_FOUND: &str = "Job Run with given attempt doesn't exist";

/// The `ValidationException` sentence an ACCESS-GRANT read — one that mints a URL for a
/// job run rather than reporting the run — states for a job-run id that resolves to
/// nothing: the class's fixed text, terminated by a period.
const JOB_RUN_ID_INVALID: &str = "Job Run with given ID doesn't exist.";

/// The two `CreateApplication` refusals, one per member that can fail.
const UNSUPPORTED_RELEASE_LABEL: &str =
    "Type is not supported for the release label or release label does not exist";

/// The ids `ListApplications` reports for `query`.
async fn listed_ids(router: &MultiServiceRouter, query: &str) -> BTreeSet<String> {
    list_applications(router, query)
        .await
        .body
        .get("applications")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The ids `ListJobRuns` reports for `query`.
async fn listed_run_ids(
    router: &MultiServiceRouter,
    application_id: &str,
    query: &str,
) -> BTreeSet<String> {
    list_job_runs(router, application_id, query)
        .await
        .body
        .get("jobRuns")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Submit a job run and return its id.
async fn submitted_job_run(
    router: &MultiServiceRouter,
    application_id: &str,
    token: &str,
) -> String {
    let answer = super::application_lifecycle::start_job_run(
        router,
        application_id,
        minimal_job_run(token),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {token:?}: StartJobRun expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `GET /applications/{id}/jobruns/{run}/dashboard` with whatever query a case adds.
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

/// Rule 1: `CreateApplication.clientToken` is `@idempotencyToken` on an `@idempotent`
/// operation, so the token names ONE request. A replay of that request reports the same
/// `applicationId`; a replay carrying ANY different parameter is a conflict (HTTP 409
/// `ConflictException`) and neither mints a second application nor changes the first.
///
/// The rows differ along WHICH member the second request changes and along the kind of
/// member it is — a string, an enum value, a map's value, a map's size, a nested
/// configuration's number and a nested list's properties — because the rule generalises
/// over exactly that: the request either is the one the token already created, or it is
/// not.
#[tokio::test]
async fn a_client_token_replayed_with_different_parameters_is_a_conflict() {
    const TOKEN: &str = "tok-idem";

    let first = json!({
        "clientToken": TOKEN,
        "releaseLabel": "emr-7.1.0",
        "type": "SPARK",
        "name": "first",
        "tags": { "team": "data", "env": "test" },
        "architecture": "X86_64",
        "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 30 },
        "runtimeConfiguration": [
            { "classification": "spark-defaults", "properties": { "spark.executor.cores": "2" } },
        ],
    });

    // (case, the second request, whether it is the same request as the first)
    let mut cases: Vec<(&str, Value, bool)> = vec![
        ("the identical request", first.clone(), true),
        (
            "the identical request again, members in another order",
            json!({
                "runtimeConfiguration": [
                    {
                        "properties": { "spark.executor.cores": "2" },
                        "classification": "spark-defaults",
                    },
                ],
                "autoStopConfiguration": { "idleTimeoutMinutes": 30, "enabled": true },
                "architecture": "X86_64",
                "tags": { "env": "test", "team": "data" },
                "name": "first",
                "type": "SPARK",
                "releaseLabel": "emr-7.1.0",
                "clientToken": TOKEN,
            }),
            true,
        ),
    ];
    // One row per member a second request can differ in.
    for (case, member, value) in [
        ("a different name", "name", json!("second")),
        ("a different release label", "releaseLabel", json!("emr-7.0.0")),
        ("a different engine type", "type", json!("HIVE")),
        ("a different architecture", "architecture", json!("ARM64")),
        (
            "a different value for one tag",
            "tags",
            json!({ "team": "ops", "env": "test" }),
        ),
        (
            "one tag more",
            "tags",
            json!({ "team": "data", "env": "test", "owner": "nobody" }),
        ),
        (
            "a different idle timeout",
            "autoStopConfiguration",
            json!({ "enabled": true, "idleTimeoutMinutes": 31 }),
        ),
        (
            "a different runtime configuration property",
            "runtimeConfiguration",
            json!([
                { "classification": "spark-defaults", "properties": { "spark.executor.cores": "4" } },
            ]),
        ),
        (
            "a member the first request did not carry",
            "maximumCapacity",
            json!({ "cpu": "400vCPU", "memory": "3000GB" }),
        ),
    ] {
        let mut request = first.clone();
        request[member] = value;
        cases.push((case, request, false));
    }
    // And one row that DROPS a member the first request carried.
    let mut without_name = first.clone();
    without_name
        .as_object_mut()
        .expect("the request is an object")
        .remove("name");
    cases.push(("a member the first request did carry, dropped", without_name, false));

    for (case, second, is_same_request) in cases {
        let router = emulator();
        let created = create_application(&router, first.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first CreateApplication expected 200, got {} with {:?}",
            created.status, created.body
        );
        let application_id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let replayed = create_application(&router, second).await;
        if is_same_request {
            assert_eq!(
                replayed.status, 200,
                "case {case:?}: a replay of the SAME request expected 200, got {} with {:?}",
                replayed.status, replayed.body
            );
            assert_eq!(
                replayed.at("/applicationId"),
                json!(application_id),
                "case {case:?}: a replay must name the application the token created, got {:?}",
                replayed.at("/applicationId")
            );
        } else {
            assert_eq!(
                replayed.status, 409,
                "case {case:?}: a replay with different parameters expected 409, got {} \
                 with {:?}",
                replayed.status, replayed.body
            );
            assert_eq!(
                replayed.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                replayed.body
            );
            assert_eq!(
                replayed.at("/applicationId"),
                Value::Null,
                "case {case:?}: a refused create must name no application, got {:?}",
                replayed.body
            );
        }

        // Either way exactly one application exists, and it is the one the first
        // request asked for.
        let held = listed_ids(&router, "").await;
        assert_eq!(
            held,
            BTreeSet::from([application_id.clone()]),
            "case {case:?}: the token must have created exactly one application, got {held:?}"
        );
        let fetched = get_application(&router, &application_id).await;
        assert_eq!(
            fetched.at("/application/name"),
            json!("first"),
            "case {case:?}: the first application must be untouched, got {:?}",
            fetched.at("/application/name")
        );
        assert_eq!(
            fetched.at("/application/releaseLabel"),
            json!("emr-7.1.0"),
            "case {case:?}: the first application must be untouched, got {:?}",
            fetched.at("/application/releaseLabel")
        );
    }
}

/// Rule 2: `DeleteApplication` is a SOFT delete — the application stays readable in the
/// terminal `TERMINATED` state — but it leaves the inventory: `ListApplications` with no
/// `states` filter does not report it, while a filter naming `TERMINATED` does.
///
/// The rows differ along the state the application was deleted from, along how many
/// applications are live beside it, and along the page size the listing is asked for,
/// because the rule generalises over all of those: a deleted application is absent from
/// the unfiltered listing however the listing is taken.
#[tokio::test]
async fn a_deleted_application_leaves_the_unfiltered_listing() {
    for (case, delete_from_stopped, live_count) in [
        ("deleted from CREATED, nothing else held", false, 0),
        ("deleted from CREATED, two live beside it", false, 2),
        ("deleted from STOPPED, nothing else held", true, 0),
        ("deleted from STOPPED, one live beside it", true, 1),
    ] {
        let router = emulator();
        let mut live: BTreeSet<String> = BTreeSet::new();
        for index in 0..live_count {
            live.insert(created_application(&router, &format!("tok-live-{index}")).await);
        }

        let doomed = created_application(&router, "tok-doomed").await;
        if delete_from_stopped {
            start_application(&router, &doomed).await;
            wait_for_transition();
            stop_application(&router, &doomed).await;
            wait_for_transition();
        }
        let deleted = delete_application(&router, &doomed).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteApplication expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        // The record is retained: a read still answers, in the terminal state.
        let read_back = get_application(&router, &doomed).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: GetApplication after the delete expected 200, got {} with {:?}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.at("/application/state"),
            json!("TERMINATED"),
            "case {case:?}: the deleted application reports TERMINATED, got {:?}",
            read_back.at("/application/state")
        );

        // ... and it is out of the inventory, whatever page size the listing uses.
        for query in ["", "?maxResults=1", "?maxResults=50"] {
            let listed = listed_ids(&router, query).await;
            assert!(
                !listed.contains(&doomed),
                "case {case:?}: with {query:?} the deleted application must be absent, \
                 got {listed:?}"
            );
            let expected_live: BTreeSet<String> = live
                .iter()
                .take(if query == "?maxResults=1" {
                    1
                } else {
                    live.len()
                })
                .cloned()
                .collect();
            assert_eq!(
                listed, expected_live,
                "case {case:?}: with {query:?} the listing must report the live \
                 applications, got {listed:?}"
            );
        }

        // Naming the terminal state explicitly is how a caller asks for it.
        for query in ["?states=TERMINATED", "?states=CREATED&states=TERMINATED"] {
            let listed = listed_ids(&router, query).await;
            assert!(
                listed.contains(&doomed),
                "case {case:?}: with {query:?} the deleted application must be listed, \
                 got {listed:?}"
            );
        }
        // A filter naming only states it is not in still excludes it.
        let listed = listed_ids(&router, "?states=STARTED").await;
        assert!(
            !listed.contains(&doomed),
            "case {case:?}: a filter naming another state must not report it, got {listed:?}"
        );
    }
}

/// Rule 3: `CancelJobRun` is `@idempotent`. A run that is ALREADY `CANCELLED` is the
/// success the operation promises — HTTP 200 echoing `applicationId` and `jobRunId`,
/// with the state left where it settled — however many times the cancel is repeated. A
/// run that finished some OTHER way is refused with the modelled `ValidationException`
/// (HTTP 400), stating the condition rather than the state it found.
#[tokio::test]
async fn cancel_job_run_is_idempotent_on_a_cancelled_run_and_refuses_another_terminal_one() {
    // (case, whether the run is cancelled first, the state it ends in, expected status)
    let cases = [
        ("a run cancelled by the caller", true, "CANCELLED", 200u16),
        ("a run that finished on its own", false, "FAILED", 400u16),
    ];

    for (case, cancel_first, terminal_state, expected_status) in cases {
        let router = emulator();
        let application_id = started_application(&router, "tok-cancel-idem").await;
        let job_run_id = submitted_job_run(&router, &application_id, "job-1").await;

        if cancel_first {
            let cancelled = cancel_job_run(&router, &application_id, &job_run_id, "").await;
            assert_eq!(
                cancelled.status, 200,
                "case {case:?}: the first CancelJobRun expected 200, got {} with {:?}",
                cancelled.status, cancelled.body
            );
        }
        wait_for_job_run_to_settle();
        assert_eq!(
            get_job_run(&router, &application_id, &job_run_id)
                .await
                .at("/jobRun/state"),
            json!(terminal_state),
            "case {case:?}: the run must have settled on {terminal_state}"
        );

        // Repeated twice: an idempotent success stays a success, and a refusal stays a
        // refusal — neither moves the state.
        for attempt in 1..=2 {
            let again = cancel_job_run(&router, &application_id, &job_run_id, "").await;
            assert_eq!(
                again.status, expected_status,
                "case {case:?} (repeat {attempt}): CancelJobRun expected \
                 {expected_status}, got {} with {:?}",
                again.status, again.body
            );
            if expected_status == 200 {
                assert_eq!(
                    (again.at("/applicationId"), again.at("/jobRunId")),
                    (json!(application_id.clone()), json!(job_run_id.clone())),
                    "case {case:?} (repeat {attempt}): the output must echo both ids, \
                     got {:?}",
                    again.body
                );
            } else {
                assert_eq!(
                    again.error_code(),
                    "ValidationException",
                    "case {case:?} (repeat {attempt}): expected ValidationException, \
                     got {:?}",
                    again.body
                );
                assert_eq!(
                    again.body["message"].as_str().unwrap_or_default(),
                    NOT_CANCELLABLE,
                    "case {case:?} (repeat {attempt}): expected the condition, got {:?}",
                    again.body
                );
                assert!(
                    !again.body.to_string().contains(terminal_state),
                    "case {case:?} (repeat {attempt}): the refusal must not quote the \
                     state it found, got {:?}",
                    again.body
                );
            }
            assert_eq!(
                get_job_run(&router, &application_id, &job_run_id)
                    .await
                    .at("/jobRun/state"),
                json!(terminal_state),
                "case {case:?} (repeat {attempt}): the state must not move"
            );
        }
    }
}

/// A `shutdownGracePeriodInSeconds` the model puts no bound on is accepted and holds the
/// run in `CANCELLING` for the wait the caller asked for — including values no clock can
/// count out, which must be answered rather than crashed on.
///
/// The rows span the member's whole range: absent, zero, a small value, and `Integer`'s
/// maximum (`CancelJobRunRequest$shutdownGracePeriodInSeconds` is an integer with no
/// `@range`, per `smithy_query(query="operation",operation="CancelJobRun")`).
#[tokio::test]
async fn cancel_job_run_accepts_any_shutdown_grace_period_the_model_admits() {
    let max = i32::MAX.to_string();
    // (case, the query, whether the cancel is still in its wait after the emulator's own
    //  transition span, whether a zero wait lets it settle at once)
    let cases: Vec<(&str, String, bool, bool)> = vec![
        ("no grace period", String::new(), false, false),
        (
            "a zero grace period",
            "?shutdownGracePeriodInSeconds=0".to_string(),
            false,
            true,
        ),
        (
            "a grace period longer than the emulator's own span",
            "?shutdownGracePeriodInSeconds=600".to_string(),
            true,
            false,
        ),
        (
            "the largest value the member's type can carry",
            format!("?shutdownGracePeriodInSeconds={max}"),
            true,
            false,
        ),
    ];

    for (case, query, still_cancelling, settles_at_once) in cases {
        let router = emulator();
        let application_id = started_application(&router, "tok-grace").await;
        let job_run_id = submitted_job_run(&router, &application_id, "job-grace").await;

        let cancelled = cancel_job_run(&router, &application_id, &job_run_id, &query).await;
        assert_eq!(
            cancelled.status, 200,
            "case {case:?}: CancelJobRun expected 200, got {} with {:?}",
            cancelled.status, cancelled.body
        );
        assert_eq!(
            (cancelled.at("/applicationId"), cancelled.at("/jobRunId")),
            (json!(application_id.clone()), json!(job_run_id.clone())),
            "case {case:?}: the output must echo both ids, got {:?}",
            cancelled.body
        );
        // A wait the caller asked for is the wait that applies: with a span to wait out
        // the run reports `CANCELLING`, and with none it may already have settled.
        let immediately = get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state");
        let admissible: &[&str] = if settles_at_once {
            &["CANCELLING", "CANCELLED"]
        } else {
            &["CANCELLING"]
        };
        assert!(
            admissible.contains(&immediately.as_str().unwrap_or_default()),
            "case {case:?}: expected one of {admissible:?} straight after the cancel, \
             got {immediately:?}"
        );

        // Past the emulator's own transition span: a run whose caller asked for a
        // longer wait is still cancelling, and one that asked for less has settled.
        wait_for_job_run_to_settle();
        let settled = get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state");
        let expected = if still_cancelling {
            "CANCELLING"
        } else {
            "CANCELLED"
        };
        assert_eq!(
            settled,
            json!(expected),
            "case {case:?}: expected {expected} once the emulator's span has passed, \
             got {settled:?}"
        );
    }
}

/// Rule 4: a submitted job run is NOT finished the instant the submit returns. It passes
/// through the non-terminal `JobRunState` values in order, so a client polling it, or
/// filtering `ListJobRuns` for the runs in flight, observes it there; only afterwards
/// does it settle.
///
/// The emulator executes no Spark or Hive workload, so the stages it passes through are
/// the control plane's own (`SUBMITTED` through `SCHEDULED`) and the state it settles on
/// is the modelled failure — `RUNNING` and `SUCCESS` are never reported for work nothing
/// ran.
#[tokio::test]
async fn a_submitted_job_run_is_observable_in_the_non_terminal_states() {
    let router = emulator();
    let application_id = started_application(&router, "tok-inflight").await;
    let job_run_id = submitted_job_run(&router, &application_id, "job-inflight").await;

    // The run is in flight the moment the submit returns, and the filter that asks for
    // the in-flight runs reports it.
    let first = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        first.at("/jobRun/state"),
        json!("SUBMITTED"),
        "a job run is SUBMITTED when it is accepted, got {:?}",
        first.at("/jobRun/state")
    );
    let in_flight = listed_run_ids(&router, &application_id, NON_TERMINAL_STATES_QUERY).await;
    assert!(
        in_flight.contains(&job_run_id),
        "the run just submitted must be listed among the non-terminal ones, got {in_flight:?}"
    );

    // Sample the state across the whole lifecycle. Every sample before the run settles
    // must be a non-terminal state, and the states must not go backwards.
    let stages = u32::try_from(JOB_RUN_STAGES.len()).unwrap_or(1);
    let whole_lifecycle = JOB_RUN_STAGE_TRANSITION * stages + Duration::from_millis(500);
    let started = Instant::now();
    let mut observed: Vec<(Duration, String)> = Vec::new();
    while started.elapsed() < whole_lifecycle {
        let state = get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        observed.push((started.elapsed(), state));
        std::thread::sleep(JOB_RUN_STAGE_TRANSITION / 3);
    }

    let distinct_non_terminal: BTreeSet<&String> = observed
        .iter()
        .filter(|(_, state)| NON_TERMINAL_JOB_RUN_STATES.contains(&state.as_str()))
        .map(|(_, state)| state)
        .collect();
    assert!(
        distinct_non_terminal.len() >= 2,
        "a submitted run must be observable in more than one non-terminal state, \
         got {observed:?}"
    );
    for (at, state) in &observed {
        assert!(
            NON_TERMINAL_JOB_RUN_STATES.contains(&state.as_str()) || state == "FAILED",
            "case {at:?}: a run nothing executed is either in flight or FAILED, got \
             {state:?} in {observed:?}"
        );
        assert!(
            state != "RUNNING" && state != "SUCCESS",
            "case {at:?}: nothing ran, so the run is never reported {state:?} \
             ({observed:?})"
        );
    }
    // Once a sample is terminal, every later sample is too: the lifecycle moves one way.
    let settled_at = observed.iter().position(|(_, state)| state == "FAILED");
    let settled_at = settled_at.unwrap_or_else(|| {
        panic!("the run must settle inside its lifecycle, got {observed:?}");
    });
    assert!(
        observed[settled_at..].iter().all(|(_, state)| state == "FAILED"),
        "a settled run must stay settled, got {observed:?}"
    );
    assert!(
        settled_at > 0,
        "the run must be observed in flight before it settles, got {observed:?}"
    );

    // And the in-flight filter no longer reports it, while the terminal one does.
    let in_flight = listed_run_ids(&router, &application_id, NON_TERMINAL_STATES_QUERY).await;
    assert!(
        !in_flight.contains(&job_run_id),
        "a settled run must leave the non-terminal listing, got {in_flight:?}"
    );
    let finished = listed_run_ids(&router, &application_id, "?states=FAILED").await;
    assert!(
        finished.contains(&job_run_id),
        "a settled run must be listed under the state it settled on, got {finished:?}"
    );

    // A second run submitted while the first has already settled is in flight on its
    // own: the lifecycle is per run, not per application.
    let second = submitted_job_run(&router, &application_id, "job-inflight-2").await;
    let in_flight = listed_run_ids(&router, &application_id, NON_TERMINAL_STATES_QUERY).await;
    assert_eq!(
        in_flight,
        BTreeSet::from([second.clone()]),
        "only the newly submitted run is in flight, got {in_flight:?}"
    );
}

/// Rule 5: `GetDashboardForJobRun` "Creates and returns a URL that you can use to access
/// the application UIs for a job run", so it answers the modelled 200 with an `https`
/// `url` inside `Url`'s `@length(1..2048)` — freshly minted per call, because "the URL
/// is valid for one hour after you generate it. To access the application UI after that
/// hour elapses, you must invoke the API again to generate a new URL."
///
/// The rows differ along the query the request carries (no query, the run's own
/// `attempt`, `accessSystemProfileLogs` both ways) and along whether the run is still in
/// flight or has settled — the operation serves a live UI for a running job and a
/// persistent one for a completed job, so both answer a URL.
#[tokio::test]
async fn get_dashboard_for_job_run_mints_an_https_url_per_call() {
    let router = emulator();
    let application_id = started_application(&router, "tok-dashboard").await;
    let job_run_id = submitted_job_run(&router, &application_id, "job-dashboard").await;

    let queries = [
        ("no query", ""),
        ("the run's own attempt", "?attempt=1"),
        ("system profile logs asked for", "?accessSystemProfileLogs=true"),
        ("system profile logs declined", "?accessSystemProfileLogs=false"),
        (
            "both members",
            "?attempt=1&accessSystemProfileLogs=true",
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for settled in [false, true] {
        if settled {
            wait_for_job_run_to_settle();
        }
        for (case, query) in queries {
            let case = format!("{case} ({} run)", if settled { "settled" } else { "in-flight" });
            let answer = get_dashboard(&router, &application_id, &job_run_id, query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: GetDashboardForJobRun expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let url = answer.at("/url").as_str().unwrap_or_default().to_string();
            assert!(
                url.starts_with("https://"),
                "case {case:?}: the dashboard url must be https, got {url:?}"
            );
            assert!(
                (1..=2048).contains(&url.len()),
                "case {case:?}: the url must satisfy Url's length 1..2048, got {} \
                 characters",
                url.len()
            );
            assert!(
                url.contains(&application_id) && url.contains(&job_run_id),
                "case {case:?}: the url must name the run it is for, got {url:?}"
            );
            minted.push(url);
        }
    }
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every call generates a new url, got {minted:?}"
    );

    // A refusal states WHICH part of the request did not resolve, and in which class:
    // the application a grant is scoped to is a resource that was not found (404), while
    // the job run the grant is minted FOR is an identifier the request got wrong (400).
    for (case, application, run, query, status, shape, expected) in [
        (
            "an unknown application",
            "doesnotexist".to_string(),
            job_run_id.clone(),
            "",
            404,
            "ResourceNotFoundException",
            APPLICATION_NOT_FOUND,
        ),
        (
            "an unknown job run",
            application_id.clone(),
            "doesnotexist".to_string(),
            "",
            400,
            "ValidationException",
            JOB_RUN_ID_INVALID,
        ),
        (
            "an attempt the run does not have",
            application_id.clone(),
            job_run_id.clone(),
            "?attempt=999",
            404,
            "ResourceNotFoundException",
            JOB_RUN_ATTEMPT_NOT_FOUND,
        ),
    ] {
        let answer = get_dashboard(&router, &application, &run, query).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            shape,
            "case {case:?}: expected {shape}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: expected the phrase for the part that was not found, got {:?}",
            answer.body
        );
    }
}

/// Rule 6: `AutoStopConfig` is enforced, not just stored. "Enables the application to
/// automatically stop after a certain amount of time being idle. Defaults to true", and
/// `idleTimeoutMinutes` is "the amount of idle time in minutes after which your
/// application will automatically stop" (`@range(min: 1, max: 10080)`, "Defaults to 15
/// minutes"): a `STARTED` application with nothing running on it reaches `STOPPED` by
/// itself once that span has passed, with no `StopApplication` call.
///
/// The rows span the configuration — both published bounds of `idleTimeoutMinutes`, the
/// documented default when the member is absent, and the stop switched off — and each is
/// checked on BOTH sides of its threshold. The idle span is advanced by rewinding the
/// application's recorded last activity in the shared store, so the test asserts the rule
/// over configurations whose span no test could sit through, and every observation still
/// goes through `GetApplication` over HTTP.
#[tokio::test]
async fn an_idle_application_stops_itself_once_its_idle_timeout_has_passed() {
    // (case, the autoStopConfiguration the create carries, the idle minutes that apply,
    //  whether the application stops at all)
    let cases: Vec<(&str, Option<Value>, u64, bool)> = vec![
        (
            "the documented default, with no configuration at all",
            None,
            15,
            true,
        ),
        (
            "the documented default idle timeout, enabled explicitly",
            Some(json!({ "enabled": true })),
            15,
            true,
        ),
        (
            "the published minimum idle timeout",
            Some(json!({ "enabled": true, "idleTimeoutMinutes": 1 })),
            1,
            true,
        ),
        (
            "the published maximum idle timeout",
            Some(json!({ "enabled": true, "idleTimeoutMinutes": 10080 })),
            10080,
            true,
        ),
        (
            "an idle timeout between the bounds",
            Some(json!({ "idleTimeoutMinutes": 120 })),
            120,
            true,
        ),
        (
            "the automatic stop switched off",
            Some(json!({ "enabled": false, "idleTimeoutMinutes": 1 })),
            1,
            false,
        ),
    ];

    for (case, configuration, idle_minutes, stops) in cases {
        let (router, engine) = emulator_with_engine();
        let mut request = minimal_create("tok-autostop");
        if let Some(configuration) = configuration {
            request["autoStopConfiguration"] = configuration;
        }
        let created = create_application(&router, request).await;
        let application_id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        wait_for_transition();
        start_application(&router, &application_id).await;
        wait_for_transition();
        assert_eq!(
            get_application(&router, &application_id)
                .await
                .at("/application/state"),
            json!("STARTED"),
            "case {case:?}: the application must be STARTED before it can stop itself"
        );

        // Seconds short of the span those idle minutes are served as: still running.
        // (Short of it by seconds whatever the configuration asks for, so every row
        // exercises the boundary rather than a point well inside it.)
        rewind_last_activity(
            &engine,
            &application_id,
            idle_span(idle_minutes) - Duration::from_secs(3),
        );
        assert_eq!(
            get_application(&router, &application_id)
                .await
                .at("/application/state"),
            json!("STARTED"),
            "case {case:?}: inside its idle timeout the application must stay STARTED"
        );

        // Past it: stopped by the service itself, if it is configured to.
        rewind_last_activity(&engine, &application_id, Duration::from_secs(4));
        let observed = get_application(&router, &application_id).await;
        let expected = if stops { "STOPPED" } else { "STARTED" };
        assert_eq!(
            observed.at("/application/state"),
            json!(expected),
            "case {case:?}: past its idle timeout the application must be {expected}, got {:?}",
            observed.at("/application/state")
        );
        if stops {
            let details = observed.at("/application/stateDetails");
            assert!(
                details
                    .as_str()
                    .is_some_and(|text| text.to_ascii_lowercase().contains("idle")),
                "case {case:?}: the automatic stop must say why, got {details:?}"
            );
            // It stays stopped, and the listing reports the state it is in.
            assert_eq!(
                get_application(&router, &application_id)
                    .await
                    .at("/application/state"),
                json!("STOPPED"),
                "case {case:?}: a second read must report STOPPED too"
            );
            let listed = listed_ids(&router, "?states=STOPPED").await;
            assert!(
                listed.contains(&application_id),
                "case {case:?}: the stopped application must be listed under STOPPED, \
                 got {listed:?}"
            );
            // And it starts again on request.
            start_application(&router, &application_id).await;
            wait_for_transition();
            assert_eq!(
                get_application(&router, &application_id)
                    .await
                    .at("/application/state"),
                json!("STARTED"),
                "case {case:?}: a start after an automatic stop must bring it back"
            );
        }
    }
}

/// An application with a job run that has not finished is NOT idle, however long ago
/// anything last reached it: the automatic stop waits for the work to finish.
#[tokio::test]
async fn an_application_running_work_is_not_idle() {
    let (router, engine) = emulator_with_engine();
    let mut request = minimal_create("tok-busy");
    request["autoStopConfiguration"] = json!({ "enabled": true, "idleTimeoutMinutes": 1 });
    let created = create_application(&router, request).await;
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_transition();
    start_application(&router, &application_id).await;
    wait_for_transition();
    let job_run_id = submitted_job_run(&router, &application_id, "job-busy").await;

    // Well past the idle timeout, with the run still in flight.
    rewind_last_activity(&engine, &application_id, Duration::from_secs(3600));
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STARTED"),
        "an application with a job run in flight is not idle and must stay STARTED"
    );
    assert!(
        NON_TERMINAL_JOB_RUN_STATES.contains(
            &get_job_run(&router, &application_id, &job_run_id)
                .await
                .at("/jobRun/state")
                .as_str()
                .unwrap_or_default()
        ),
        "the job run must still be in flight for this case to mean anything"
    );

    // Once the run finishes, the idle span runs from the instant it finished: the
    // application is still STARTED straight afterwards, and stops once that span has
    // passed.
    wait_for_job_run_to_settle();
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STARTED"),
        "the idle span starts when the work finished, not before it"
    );
    rewind_last_activity(&engine, &application_id, Duration::from_secs(120));
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STOPPED"),
        "once idle past its timeout the application stops itself"
    );
}

/// The same rule reaches the OTHER kind of work an application holds: an interactive
/// session that has not ended keeps the application out of the idle state, and the idle
/// span starts again when the session is over.
///
/// `StartSession` is served by applications on EMR 7.13.0 and later with interactive
/// sessions enabled, and a session's own `idleTimeoutMinutes` is what ends it — so this
/// case starts a session with the smallest timeout the model admits and advances the
/// clock-owed state through the store, exactly as the application rows do.
#[tokio::test]
async fn an_application_holding_a_live_session_is_not_idle() {
    let (router, engine) = emulator_with_engine();
    let created = create_application(
        &router,
        json!({
            "clientToken": "tok-session-idle",
            // The earliest release whose applications serve interactive sessions, with
            // `InteractiveConfiguration$sessionEnabled` switched on.
            "releaseLabel": "emr-7.13.0",
            "type": "Spark",
            "interactiveConfiguration": { "sessionEnabled": true },
            "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 1 },
        }),
    )
    .await;
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_transition();
    start_application(&router, &application_id).await;
    wait_for_transition();

    let started = call(
        &router,
        "POST",
        &format!("/applications/{application_id}/sessions"),
        Some(json!({
            "clientToken": "ses-idle",
            "executionRoleArn": EXECUTION_ROLE,
            "idleTimeoutMinutes": 1,
        })),
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
    wait_for_transition();

    // Well past the application's idle timeout, with the session still live.
    rewind_last_activity(&engine, &application_id, Duration::from_secs(3600));
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STARTED"),
        "an application holding a live session is not idle and must stay STARTED"
    );

    // The session's own idle timeout ends it; from then the application is idle, and
    // once its own span has passed it stops itself. The session's termination is moved
    // exactly to due, so the application has been idle only since the session ended.
    rewind_session_idle_since(&engine, &application_id, &session_id, idle_span(1));
    let session = call(
        &router,
        "GET",
        &format!("/applications/{application_id}/sessions/{session_id}"),
        None,
    )
    .await;
    assert_eq!(
        session.at("/session/state"),
        json!("TERMINATED"),
        "the session's own idle timeout must have ended it, got {:?}",
        session.at("/session/state")
    );
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STARTED"),
        "the application's idle span starts when the session ended, not before it"
    );
    rewind_last_activity(&engine, &application_id, Duration::from_secs(120));
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STOPPED"),
        "with no work left and its idle timeout passed the application stops itself"
    );
}

/// Rule 7 of the create's validation, asserted on the exact phrases: a refusal names the
/// member that failed — the engine type when it names no engine this service serves, the
/// release label when the engine is real but the release does not offer it.
///
/// The rows differ along which member is wrong and along the shape of the wrong value: a
/// release before the service existed, a release far before it, a label carrying no
/// readable version at all, and engine types of several lengths.
#[tokio::test]
async fn a_create_refusal_names_the_member_that_failed() {
    let router = emulator();
    // (case, type, releaseLabel, the exact message)
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "a release one step before EMR Serverless existed",
            "SPARK",
            "emr-6.5.0",
            UNSUPPORTED_RELEASE_LABEL.to_string(),
        ),
        (
            "a release far before it",
            "HIVE",
            "emr-5.36.0",
            UNSUPPORTED_RELEASE_LABEL.to_string(),
        ),
        (
            "a release label that names no release at all",
            "SPARK",
            "emr-0.0.0-does-not-exist",
            UNSUPPORTED_RELEASE_LABEL.to_string(),
        ),
        (
            "a release label with no version in it",
            "SPARK",
            "latest",
            UNSUPPORTED_RELEASE_LABEL.to_string(),
        ),
        (
            "an engine type nothing serves",
            "NOT_A_REAL_ENGINE",
            "emr-7.0.0",
            "Type NOT_A_REAL_ENGINE is not supported for release label".to_string(),
        ),
        (
            "an engine type of one character",
            "X",
            "emr-7.0.0",
            "Type X is not supported for release label".to_string(),
        ),
        (
            "an engine type that is a near miss",
            "Sparks",
            "emr-7.0.0",
            "Type Sparks is not supported for release label".to_string(),
        ),
    ];

    for (index, (case, engine_type, release_label, expected)) in cases.into_iter().enumerate() {
        let answer = create_application(
            &router,
            json!({
                "clientToken": format!("tok-member-{index}"),
                "releaseLabel": release_label,
                "type": engine_type,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // Nothing was minted by any of them.
    let held = listed_ids(&router, "").await;
    assert!(
        held.is_empty(),
        "a refused create must mint nothing, got {held:?}"
    );

    // And the pair the service does serve is accepted.
    let accepted = create_application(
        &router,
        json!({
            "clientToken": "tok-member-ok",
            "releaseLabel": "emr-6.6.0",
            "type": "SPARK",
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the earliest release that serves Spark must be accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
}

/// Move an application's recorded last activity `span` into the past, through the
/// store the handlers share, so an idle span longer than any test can sit through has
/// elapsed by the time the next request reads it.
fn rewind_last_activity(engine: &engine::InMemoryEngine, application_id: &str, span: Duration) {
    let state = engine.state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>();
    let mut store = state.lock();
    let record = store
        .applications
        .get_mut(application_id)
        .expect("the application the test created is in the store");
    record.last_activity_at -= span;
}

/// Move a session's recorded idle instant — and the termination its own idle timeout
/// owes it — `span` into the past, so the span the session's `idleTimeoutMinutes`
/// bounds has elapsed by the time the next request reads it.
fn rewind_session_idle_since(
    engine: &engine::InMemoryEngine,
    application_id: &str,
    session_id: &str,
    span: Duration,
) {
    let state = engine.state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>();
    let mut store = state.lock();
    let record = store
        .applications
        .get_mut(application_id)
        .expect("the application the test created is in the store");
    let session = record
        .sessions
        .get_mut(session_id)
        .expect("the session the test started is in the store");
    if let Some(pending) = session.pending.as_mut() {
        pending.due -= span;
    }
}
