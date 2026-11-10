// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue interactive session slice: `CreateSession`, the
//! `GetSession` that reads back what it stored, and the `GetDashboardUrl` that addresses a
//! stored session.
//!
//! RULE: a session create provisions ONE record per `Id` in the Region it was served in,
//! holding the whole request (`Command`, `Role`, `Description`, the capacity, the worker
//! configuration, `SecurityConfiguration`, `GlueVersion`, `SessionType`, `DefaultArguments`,
//! `Connections`) plus the timeouts the model documents defaults for, and answers with the
//! session it wrote; it starts in `PROVISIONING` and the clock owes it `READY`, and then
//! `TIMEOUT` when its own documented timeout falls due. Every later read of that id in that
//! Region reports that record, an id already taken is refused instead of overwritten, and an
//! id nothing created is a miss.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip rows differ along what the rule generalises over: which optional members
//! the create carries, how the capacity is expressed (a DPU figure, a worker configuration,
//! or neither), which timeouts are named, the session type, and the size of the id, the
//! description and the argument map — each published bound included.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    dpu_per_worker, messages, ComAmazonawsGlueAwsglueState, DEFAULT_SESSION_TIMEOUT_MINUTES,
    DEFAULT_SESSION_TYPE, DEFAULT_SESSION_WORKERS, DEFAULT_SESSION_WORKER_TYPE,
    MIN_SESSION_GLUE_VERSION, SESSION_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::far_future;
use crate::wire::{
    assert_echoed, call, call_in_region, emulator, emulator_on, error_code, error_message, ok,
    REGION,
};

/// The longest `Id` the model accepts: `NameString`, "len 1..255"
/// (`smithy_query operation=CreateSession`).
const MAX_ID_LENGTH: usize = 255;

/// The longest `Description` the model accepts: `DescriptionString`, "len 0..2048"
/// (`smithy_query operation=CreateSession`).
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// The most `DefaultArguments` pairs the model accepts: `OrchestrationArgumentsMap`,
/// "len 0..75" (`smithy_query operation=CreateSession`).
const MAX_DEFAULT_ARGUMENTS: usize = 75;

/// The `SessionStatus` values this suite observes, of the modelled enum (`PROVISIONING`,
/// `READY`, `FAILED`, `TIMEOUT`, `STOPPING`, `STOPPED`, from
/// `smithy_query shape=SessionStatus`).
const STATUS_PROVISIONING: &str = "PROVISIONING";
const STATUS_READY: &str = "READY";
const STATUS_TIMEOUT: &str = "TIMEOUT";

/// An IAM role ARN of the form the modelled `OrchestrationRoleArn` pattern requires
/// (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/GlueSessionRole";

/// Seconds in one minute: both documented session timeouts are stated in minutes.
const SECONDS_PER_MINUTE: u64 = 60;

/// The members of a create that the modelled `Session` shape reports back, as a body a
/// round-trip assertion can be made against. `Timeout`, `Tags` and `RequestOrigin` are not
/// among them: the shape carries no such members.
///
/// Three of them are filled in by the SERVICE when the create named nothing for them, so
/// what must come back is the create's value where it sent one and the service's otherwise:
/// the arguments map (the empty map), the session type ([`DEFAULT_SESSION_TYPE`]) and, for a
/// create that expressed no capacity at all, the worker configuration a session is
/// provisioned with.
fn reported_members(request: &Value) -> Value {
    let mut expected = json!({});
    for member in [
        "Id",
        "Description",
        "Role",
        "Command",
        "Connections",
        "SecurityConfiguration",
        "GlueVersion",
    ] {
        expected[member] = request.get(member).cloned().unwrap_or(Value::Null);
    }
    expected["DefaultArguments"] = request
        .get("DefaultArguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    expected["SessionType"] = request
        .get("SessionType")
        .cloned()
        .unwrap_or_else(|| json!(DEFAULT_SESSION_TYPE));
    let named_capacity = ["WorkerType", "NumberOfWorkers", "MaxCapacity"]
        .iter()
        .any(|member| request.get(member).is_some());
    if named_capacity {
        for member in ["WorkerType", "NumberOfWorkers"] {
            expected[member] = request.get(member).cloned().unwrap_or(Value::Null);
        }
    } else {
        expected["WorkerType"] = json!(DEFAULT_SESSION_WORKER_TYPE);
        expected["NumberOfWorkers"] = json!(DEFAULT_SESSION_WORKERS);
    }
    expected
}

/// The DPU figure the worker configuration a session that named no capacity is provisioned
/// with amounts to, through the model's own per-worker figures.
fn default_session_capacity() -> f64 {
    dpu_per_worker(DEFAULT_SESSION_WORKER_TYPE).expect("the default worker type maps to DPUs")
        * f64::from(DEFAULT_SESSION_WORKERS)
}

/// Create a session and return the `Session` its answer reports.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    let created = ok(router, "CreateSession", body).await;
    created
        .get("Session")
        .cloned()
        .unwrap_or_else(|| panic!("CreateSession must report a Session, got {created}"))
}

/// The `Session` the `GetSession` answer for `id` reports.
async fn get(router: &MultiServiceRouter, id: &str) -> Value {
    let read = ok(router, "GetSession", &json!({ "Id": id })).await;
    read.get("Session")
        .cloned()
        .unwrap_or_else(|| panic!("GetSession must report a Session, got {read}"))
}

/// Pull a stored session's instants back by `elapsed`, so a suite can observe the status the
/// clock owes it without sleeping for the span.
fn rewind(engine: &Arc<engine::InMemoryEngine>, id: &str, elapsed: Duration) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    let back = |at: std::time::SystemTime| at.checked_sub(elapsed).expect("an instant in range");
    entry.created_on = back(entry.created_on);
    entry.last_active_on = back(entry.last_active_on);
    entry.settles_at = entry.settles_at.map(back);
}

/// Put the instant the clock owes a stored session its next status at `at`.
///
/// A row that means "the clock has not reached the due instant yet" holds that instant far
/// enough ahead that the wall clock cannot cross it while the row's calls are made: the
/// status turns on the stored due instant, so positioning the instant is what fixes which
/// side of it a call lands on, and a margin measured against the clock is not.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, id: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    if entry.settles_at.is_some() {
        entry.settles_at = Some(at);
    }
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateSession` persists is what `GetSession` reports: the create answers with the
/// session it stored, every member the request carried comes back from the store unchanged,
/// the documented timeout defaults are filled in, and the capacity is the figure the request
/// named or the one its worker configuration maps to.
#[tokio::test]
async fn created_sessions_are_read_back_from_the_store() {
    let router = emulator();

    let long_id = "i".repeat(MAX_ID_LENGTH);
    let long_description = "d".repeat(MAX_DESCRIPTION_LENGTH);
    let full_arguments: Value = (0..MAX_DEFAULT_ARGUMENTS)
        .map(|index| (format!("arg-{index}"), json!(format!("value-{index}"))))
        .collect();

    // (case, the create body, the IdleTimeout it must report, the MaxCapacity it must report)
    let defaulted_capacity = Some(default_session_capacity());
    let cases: Vec<(&str, Value, i64, Option<f64>)> = vec![
        (
            // The shape a notebook client sends: the three required members alone.
            "required members only",
            json!({
                "Id": "plain-session",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
        (
            "every optional member",
            json!({
                "Id": "fully-specified",
                "Role": ROLE,
                "Command": { "Name": "glueetl", "PythonVersion": "3" },
                "Description": "Interactive exploration of one table.",
                "Timeout": 60,
                "IdleTimeout": 30,
                "DefaultArguments": { "--enable-metrics": "true", "library-set": "analytics" },
                "Connections": { "Connections": ["warehouse", "lake"] },
                "MaxCapacity": 5.0,
                "SecurityConfiguration": "sessions-cmk",
                "GlueVersion": "4.0",
                "SessionType": "LIVY",
                "Tags": { "team": "data" },
                "RequestOrigin": "notebook",
            }),
            30,
            Some(5.0),
        ),
        (
            // "Default for Spark ETL jobs is value of Timeout."
            "a timeout with no idle timeout",
            json!({
                "Id": "timeout-only",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "Timeout": 120,
            }),
            120,
            defaulted_capacity,
        ),
        (
            // Both timeouts at the modelled floor ("range 1..").
            "timeouts at the modelled floor",
            json!({
                "Id": "shortest-timeouts",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "Timeout": 1,
                "IdleTimeout": 1,
            }),
            1,
            defaulted_capacity,
        ),
        (
            // "each worker maps to 2 DPU" for `G.2X`, so three of them are six DPUs.
            "workers instead of a capacity figure",
            json!({
                "Id": "worker-capacity",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "WorkerType": "G.2X",
                "NumberOfWorkers": 3,
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            Some(6.0),
        ),
        (
            // "each worker maps to 0.25 DPU" for `G.025X`.
            "a fractional per-worker capacity",
            json!({
                "Id": "fractional-capacity",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "WorkerType": "G.025X",
                "NumberOfWorkers": 1,
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            Some(0.25),
        ),
        (
            // The model gives `Standard` no per-worker DPU figure, so none is invented.
            "a worker type the model gives no DPU figure for",
            json!({
                "Id": "standard-workers",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "WorkerType": "Standard",
                "NumberOfWorkers": 2,
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            None,
        ),
        (
            // The other value of the modelled `SessionType` enum, and a streaming command.
            "the other session type",
            json!({
                "Id": "spark-connect",
                "Role": ROLE,
                "Command": { "Name": "gluestreaming", "PythonVersion": "3.9" },
                "SessionType": "SPARK_CONNECT",
                "GlueVersion": "5.1",
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
        (
            "an id at the modelled length bound",
            json!({
                "Id": long_id,
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
        (
            "a description at the modelled length bound",
            json!({
                "Id": "long-description",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "Description": long_description,
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
        (
            "the maximum number of default arguments",
            json!({
                "Id": "many-arguments",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "DefaultArguments": full_arguments,
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
        (
            "an empty argument map and an empty connection list",
            json!({
                "Id": "empty-collections",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "DefaultArguments": {},
                "Connections": { "Connections": [] },
            }),
            i64::from(DEFAULT_SESSION_TIMEOUT_MINUTES),
            defaulted_capacity,
        ),
    ];

    for (case, request, idle_timeout, max_capacity) in &cases {
        let id = request["Id"].as_str().expect("the case carries an Id");

        let created = create(&router, request).await;
        // "Returns the session object in the response": the record the create wrote.
        assert_eq!(
            created["Id"].as_str(),
            Some(id),
            "case {case}: the create must report the stored id, got {created}"
        );
        // "The period in which a remote Spark runtime environment is running" starts with
        // that environment being provisioned.
        assert_eq!(
            created["Status"].as_str(),
            Some(STATUS_PROVISIONING),
            "case {case}: expected {STATUS_PROVISIONING} straight after the create, got \
             {created}"
        );

        let read = get(&router, id).await;
        assert_eq!(
            read, created,
            "case {case}: the read must report the session the create wrote, got {read}"
        );

        // Every member of the request the modelled `Session` shape reports comes back
        // unchanged.
        assert_echoed(case, &reported_members(request), &read);

        // "The time and date when the session was created."
        assert!(
            read["CreatedOn"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: expected a CreatedOn instant, got {read}"
        );
        // "The number of minutes when idle before the session times out", which for a
        // create that named none is the value of `Timeout`.
        assert_eq!(
            read["IdleTimeout"].as_i64(),
            Some(*idle_timeout),
            "case {case}: expected an IdleTimeout of {idle_timeout}, got {read}"
        );
        // The capacity the request named, else the DPUs its worker configuration maps to.
        assert_eq!(
            read["MaxCapacity"].as_f64(),
            *max_capacity,
            "case {case}: expected a MaxCapacity of {max_capacity:?}, got {read}"
        );
        // "The code execution progress of the session": no statement of it has run code.
        assert_eq!(
            read["Progress"].as_f64(),
            Some(0.0),
            "case {case}: expected no progress on a fresh session, got {read}"
        );
        // A session that is still running has not completed, no error was displayed during
        // it, and no usage profile is associated with it.
        for member in ["CompletedOn", "ErrorMessage", "ProfileName"] {
            assert!(
                read.get(member).is_none_or(Value::is_null),
                "case {case}: a running session reports no {member}, got {read}"
            );
        }
        // Nothing has been accounted to a session that has not run: "the total time the
        // session ran for" and "the DPUs consumed by the session" are both zero, which is a
        // figure a client can read rather than an absent member.
        for member in ["ExecutionTime", "DPUSeconds"] {
            assert_eq!(
                read[member].as_f64(),
                Some(0.0),
                "case {case}: a fresh session has consumed no {member}, got {read}"
            );
        }
        // The modelled `Session` shape carries neither member, so what the create sent
        // under them is not part of the session a read reports.
        for member in ["Timeout", "Tags"] {
            assert!(
                read.get(member).is_none_or(Value::is_null),
                "case {case}: the Session shape carries no {member}, got {read}"
            );
        }
    }

    // Each create is its own record: after every one has landed, each id still reports its
    // OWN role and command, so no create overwrote another.
    for (case, request, _, _) in &cases {
        let id = request["Id"].as_str().expect("the case carries an Id");
        let read = get(&router, id).await;
        assert_echoed(case, &reported_members(request), &read);
    }
}

/// A session id is the CLIENT's, and addresses one session per Region: the second create
/// under an id already taken is refused with the modelled `AlreadyExistsException`, and the
/// stored session is left as the first create wrote it.
#[tokio::test]
async fn a_duplicate_session_id_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same id, a different command",
            json!({ "Id": "dup-one", "Role": ROLE, "Command": { "Name": "glueetl" } }),
            json!({ "Id": "dup-one", "Role": ROLE, "Command": { "Name": "gluestreaming" } }),
        ),
        (
            "same id, a description and a capacity added",
            json!({ "Id": "dup-two", "Role": ROLE, "Command": { "Name": "glueetl" } }),
            json!({
                "Id": "dup-two",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "Description": "Added by the second create.",
                "MaxCapacity": 9.0,
            }),
        ),
        (
            "same id and the identical request",
            json!({
                "Id": "dup-three",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "IdleTimeout": 15,
            }),
            json!({
                "Id": "dup-three",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "IdleTimeout": 15,
            }),
        ),
    ];

    for (case, first, second) in &cases {
        let id = first["Id"].as_str().expect("the case carries an Id");
        let stored = create(&router, first).await;

        let (status, body) = call(&router, "CreateSession", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate id must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_already_exists(id),
            "case {case}: the refusal must name the id that collided, got {body}"
        );

        let read = get(&router, id).await;
        assert_eq!(
            read, stored,
            "case {case}: the refused create must leave the stored session untouched, got \
             {read}"
        );
    }
}

/// An id no session of this Region has is a miss: the read is refused with the modelled
/// `EntityNotFoundException` naming it, never answered with an invented session. A session is
/// served only through the endpoint that created it, so another Region's endpoint is a miss
/// too.
#[tokio::test]
async fn an_id_no_session_has_is_a_miss() {
    let router = emulator();
    create(
        &router,
        &json!({ "Id": "held", "Role": ROLE, "Command": { "Name": "glueetl" } }),
    )
    .await;

    for (case, id) in [
        ("an id nothing created", "no-such-session"),
        ("a uuid-shaped id", "11111111-2222-3333-4444-555555555555"),
        ("an id carrying dots", "team.session.42"),
        ("a prefix of a stored id", "hel"),
        ("a stored id in another case", "HELD"),
    ] {
        let (status, body) = call(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            status, 400,
            "case {case}: an unknown session must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_not_found(id),
            "case {case}: the refusal must name the id the request addressed, got {body}"
        );
    }

    // The same id through another Region's endpoint: the session is not there either.
    for region in ["eu-west-1", "us-west-2"] {
        let (status, body) =
            call_in_region(&router, region, "GetSession", &json!({ "Id": "held" })).await;
        assert_eq!(
            status, 400,
            "case {region}: a session is served only by the endpoint that created it, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::session_not_found("held"),
            "case {region}: expected the miss to name the id, got {body}"
        );
    }

    // Creating the same id through ANOTHER Region's endpoint makes a SECOND session, and
    // neither Region's read reports the other's record.
    let (status, elsewhere) = call_in_region(
        &router,
        "eu-west-1",
        "CreateSession",
        &json!({
            "Id": "held",
            "Role": ROLE,
            "Command": { "Name": "gluestreaming" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the id is free in another Region, got {status} with {elsewhere}"
    );
    assert_eq!(
        elsewhere["Session"]["Command"]["Name"].as_str(),
        Some("gluestreaming"),
        "the second create must report its own command, got {elsewhere}"
    );
    let home = get(&router, "held").await;
    assert_eq!(
        home["Command"]["Name"].as_str(),
        Some("glueetl"),
        "the first Region's session must keep its own command, got {home}"
    );
    let (status, away) =
        call_in_region(&router, "eu-west-1", "GetSession", &json!({ "Id": "held" })).await;
    assert_eq!(
        status, 200,
        "the second Region now holds the id, got {status} with {away}"
    );
    assert_eq!(
        away["Session"]["Command"]["Name"].as_str(),
        Some("gluestreaming"),
        "the second Region's read must report its own session, got {away}"
    );
}

// ---------------------------------------------------------------------------
// The statuses the clock owes a session
// ---------------------------------------------------------------------------

/// A session leaves `PROVISIONING` for `READY` when the instant the clock owed it has
/// passed, and not before — and the change is materialised by whichever read happens next,
/// so nothing sleeps and two reads of one session agree.
///
/// The status turns on the stored DUE instant, so each row positions that instant: a row
/// that reads a session still provisioning holds it ahead of the clock, and a row that reads
/// a provisioned one leaves it at or behind the clock, where the clock only moves further.
#[tokio::test]
async fn a_session_reaches_ready_once_the_clock_owes_it() {
    // (case, how long ago the session was created, whether its due instant is held ahead of
    //  the clock, the status a read must report)
    let rows: &[(&str, Duration, bool, &str)] = &[
        ("the instant it was created", Duration::ZERO, false, STATUS_PROVISIONING),
        (
            "halfway through the span, the due instant still ahead",
            SESSION_SETTLES_AFTER / 2,
            true,
            STATUS_PROVISIONING,
        ),
        (
            // A session as old as the span still reports the status its DUE instant says,
            // not one computed from how long ago it was created.
            "a hair short of the span, the due instant still ahead",
            SESSION_SETTLES_AFTER - Duration::from_millis(1),
            true,
            STATUS_PROVISIONING,
        ),
        ("exactly at the due instant", SESSION_SETTLES_AFTER, false, STATUS_READY),
        ("well past the due instant", SESSION_SETTLES_AFTER * 4, false, STATUS_READY),
    ];

    for (case, elapsed, due_held_ahead, expected) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        create(
            &router,
            &json!({
                "Id": "settling",
                "Role": ROLE,
                "Command": { "Name": "glueetl" },
                "Description": "Provisioning in flight.",
            }),
        )
        .await;
        rewind(&engine, "settling", *elapsed);
        if *due_held_ahead {
            set_due_instant(&engine, "settling", far_future());
        }

        let read = get(&router, "settling").await;
        assert_eq!(
            read["Status"].as_str(),
            Some(*expected),
            "case {case}: expected {expected} {elapsed:?} after the create, got {read}"
        );

        // Reading again reports the same record: the status comes from the DUE instant the
        // create recorded, not from the instant somebody read it.
        let again = get(&router, "settling").await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one session must agree, got {again} then {read}"
        );
        assert_eq!(
            read["Description"].as_str(),
            Some("Provisioning in flight."),
            "case {case}: the stored session must survive the status change, got {read}"
        );
    }
}

/// A session that has been idle past its own timeout is reported as `TIMEOUT`, completed at
/// the instant the timeout fell due — the earlier of the two timeouts the model documents,
/// `IdleTimeout` minutes after it was last active and `Timeout` minutes after it was created.
/// Until then it stays `READY`.
#[tokio::test]
async fn an_idle_session_times_out_when_its_own_timeout_falls_due() {
    // (case, the timeouts the create names, how long ago it was created, the status a read
    // must report, the minutes it ran for when it has completed)
    let rows: &[(&str, Value, u64, &str, Option<u64>)] = &[
        (
            "an idle timeout not yet reached",
            json!({ "IdleTimeout": 10 }),
            5,
            STATUS_READY,
            None,
        ),
        (
            "an idle timeout exactly reached",
            json!({ "IdleTimeout": 10 }),
            10,
            STATUS_TIMEOUT,
            Some(10),
        ),
        (
            "an idle timeout well past",
            json!({ "IdleTimeout": 2 }),
            30,
            STATUS_TIMEOUT,
            Some(2),
        ),
        (
            // "Default for Spark ETL jobs is value of Timeout": the idle leg falls due with
            // the timeout the create named.
            "no idle timeout, a short timeout",
            json!({ "Timeout": 3 }),
            4,
            STATUS_TIMEOUT,
            Some(3),
        ),
        (
            // The EARLIER of the two deadlines is the one that ends the session.
            "a timeout shorter than the idle timeout",
            json!({ "Timeout": 4, "IdleTimeout": 60 }),
            5,
            STATUS_TIMEOUT,
            Some(4),
        ),
        (
            "an idle timeout shorter than the timeout",
            json!({ "Timeout": 60, "IdleTimeout": 4 }),
            5,
            STATUS_TIMEOUT,
            Some(4),
        ),
        (
            // The documented default of 48 hours is far beyond an hour of idling.
            "the documented default timeouts",
            json!({}),
            60,
            STATUS_READY,
            None,
        ),
    ];

    for (case, timeouts, idle_minutes, expected, ran_for) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());

        let mut body = timeouts.clone();
        body["Id"] = json!("idling");
        body["Role"] = json!(ROLE);
        body["Command"] = json!({ "Name": "glueetl" });
        // A capacity, so the DPUs a completed session consumed are reportable.
        body["MaxCapacity"] = json!(2.0);
        create(&router, &body).await;
        rewind(
            &engine,
            "idling",
            Duration::from_secs(idle_minutes * SECONDS_PER_MINUTE),
        );

        let read = get(&router, "idling").await;
        assert_eq!(
            read["Status"].as_str(),
            Some(*expected),
            "case {case}: expected {expected} after {idle_minutes} idle minutes, got {read}"
        );

        match ran_for {
            Some(minutes) => {
                let seconds = (minutes * SECONDS_PER_MINUTE) as f64;
                // "The total time the session ran for": from its creation to the instant the
                // timeout fell due, not to the instant somebody read it.
                assert_eq!(
                    read["ExecutionTime"].as_f64(),
                    Some(seconds),
                    "case {case}: expected an ExecutionTime of {seconds} seconds, got {read}"
                );
                // "The DPUs consumed by the session (formula: ExecutionTime * MaxCapacity)."
                assert_eq!(
                    read["DPUSeconds"].as_f64(),
                    Some(seconds * 2.0),
                    "case {case}: expected the documented DPUSeconds formula, got {read}"
                );
                assert!(
                    read["CompletedOn"].as_f64().is_some_and(|at| at > 0.0),
                    "case {case}: a completed session reports when it completed, got {read}"
                );
            }
            None => {
                // A session still running has not completed, so nothing has been accounted
                // to it: the accounting members report zero rather than nothing.
                assert!(
                    read.get("CompletedOn").is_none_or(Value::is_null),
                    "case {case}: a running session reports no CompletedOn, got {read}"
                );
                for member in ["ExecutionTime", "DPUSeconds"] {
                    assert_eq!(
                        read[member].as_f64(),
                        Some(0.0),
                        "case {case}: a running session has consumed no {member}, got {read}"
                    );
                }
            }
        }

        // The timeout is a status the CLOCK owed: reading again reports the same record.
        let again = get(&router, "idling").await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one session must agree, got {again} then {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Glue version a session may run on
// ---------------------------------------------------------------------------

/// "The GlueVersion must be greater than 2.0": a create naming a version that is not is
/// refused with the modelled `InvalidInputException` and stores nothing, while a version
/// above the floor — and one the restriction cannot be compared against at all — is stored.
#[tokio::test]
async fn a_glue_version_at_or_below_the_floor_is_refused() {
    let router = emulator();

    // (case, the GlueVersion, whether the create is accepted)
    let rows: &[(&str, &str, bool)] = &[
        ("exactly at the published floor", "2.0", false),
        ("below the floor", "1.0", false),
        ("just above the floor", "2.1", true),
        ("one minor version above the floor", "3.0", true),
        ("the current default version", "5.1", true),
        (
            // The modelled pattern `^(\w+\.)+\w+$` is wider than a decimal, so this version
            // supports no comparison against the floor and the restriction does not refuse
            // it.
            "a version no comparison can be made against",
            "4.0.1",
            true,
        ),
    ];

    for (case, glue_version, accepted) in rows {
        let id = format!("version-{glue_version}");
        let body = json!({
            "Id": id,
            "Role": ROLE,
            "Command": { "Name": "glueetl" },
            "GlueVersion": glue_version,
        });
        let (status, response) = call(&router, "CreateSession", &body).await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: {glue_version} clears the floor, got {status} with {response}"
            );
            let read = get(&router, &id).await;
            assert_eq!(
                read["GlueVersion"].as_str(),
                Some(*glue_version),
                "case {case}: the stored session must report the version it was created \
                 with, got {read}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case}: {glue_version} does not clear the floor, got {status} with \
             {response}"
        );
        assert!(
            error_code(&response).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {response}",
            error_code(&response)
        );
        assert_eq!(
            error_message(&response),
            messages::session_glue_version_too_low(glue_version),
            "case {case}: the refusal must name the version and the bound, got {response}"
        );
        assert!(
            error_message(&response).contains(&format!("{MIN_SESSION_GLUE_VERSION:.1}")),
            "case {case}: the refusal must state the published bound, got {response}"
        );
        // Nothing was stored for the refused create.
        let (status, body) = call(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create stores nothing, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The dashboard of a stored session
// ---------------------------------------------------------------------------

/// `GetDashboardUrl` for a `SESSION` resolves the session in the Region it is asked in: one
/// this endpoint holds is answered with a URL that names it, and an id it holds no session
/// under is still the modelled `EntityNotFoundException`.
#[tokio::test]
async fn the_dashboard_url_of_a_stored_session_names_it() {
    let router = emulator();

    let ids = ["dashboard-session", "another.session-1"];
    for id in ids {
        create(
            &router,
            &json!({ "Id": id, "Role": ROLE, "Command": { "Name": "glueetl" } }),
        )
        .await;
    }

    let mut urls: Vec<String> = Vec::new();
    for id in ids {
        let answer = ok(
            &router,
            "GetDashboardUrl",
            &json!({ "ResourceId": id, "ResourceType": "SESSION" }),
        )
        .await;
        let url = answer["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {id}: expected a Url, got {answer}"))
            .to_string();
        assert!(
            url.starts_with("https://"),
            "case {id}: expected an https endpoint, got {url}"
        );
        assert!(
            url.contains(id),
            "case {id}: the URL must address the session it was asked for, got {url}"
        );
        assert!(
            url.contains(REGION),
            "case {id}: the URL must be scoped to the Region it was served in, got {url}"
        );

        // Two calls for one session answer one URL: it is read off the request and the
        // Region, not minted per call.
        let again = ok(
            &router,
            "GetDashboardUrl",
            &json!({ "ResourceId": id, "ResourceType": "SESSION", "RequestOrigin": "console" }),
        )
        .await;
        assert_eq!(
            again["Url"].as_str(),
            Some(url.as_str()),
            "case {id}: two calls must answer one URL, got {again}"
        );
        urls.push(url);
    }
    assert_ne!(
        urls[0], urls[1],
        "two sessions must not share one dashboard URL, got {urls:?}"
    );

    // An id this endpoint holds no session under is still a miss, whether or not it holds
    // other sessions — and so is a stored id asked for through another Region's endpoint.
    for (case, region, id) in [
        ("an id nothing created", REGION, "no-such-session"),
        ("a stored id in another Region", "eu-west-1", ids[0]),
    ] {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetDashboardUrl",
            &json!({ "ResourceId": id, "ResourceType": "SESSION" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an unheld session must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert!(
            body.get("Url").is_none_or(Value::is_null),
            "case {case}: a refused call hands out no URL, got {body}"
        );
    }
}
