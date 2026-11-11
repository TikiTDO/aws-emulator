// SPDX-License-Identifier: CC0-1.0

//! The states a submitted statement really passes through, and the SESSION a
//! caller's consecutive statements share — both driven over the wire.
//!
//! Two rules:
//!
//! 1. a statement's status follows its own execution through the lifecycle the
//!    model declares: `SUBMITTED` while it waits for its turn, `PICKED` once
//!    that turn is its, `STARTED` while the engine is running it, and then one
//!    of the three terminal states. So a long statement is observable WHILE it
//!    runs, which is what makes it cancellable: cancelling it answers that it
//!    was stopped, the statement lands `ABORTED` carrying the backend's reason,
//!    and cancelling anything that has already stopped is refused naming the
//!    state it is in.
//! 2. a statement submitted with a `SessionId` runs on the session's own
//!    connection, so what an earlier statement of that session built — a
//!    temporary table, a session setting — is there for the next one.

use serde_json::{json, Value};

use super::emulator::{
    error_code, id_of, wait_for_terminal, wait_until_started, Emulator, RUNNING_STATUSES,
    SLOW_SQL, WAIT_SECONDS,
};

/// How long the session suite keeps its session alive: longer than the
/// statements it runs in it take, so what ends the session is the caller and
/// never the clock.
const SESSION_KEEP_ALIVE_SECONDS: i32 = 90;

/// Submit `sql` against one database without waiting for it, and answer the `Id`
/// the service recorded it under.
async fn submit(emulator: &Emulator, database: &str, sql: &str) -> String {
    let response = emulator
        .ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": database,
                "DbUser": "awsuser",
                "Sql": sql,
            }),
        )
        .await;
    let answered = response["Status"].as_str().unwrap_or_default().to_string();
    assert!(
        RUNNING_STATUSES.contains(&answered.as_str()) || answered == "FINISHED",
        "a submit answers with the state the statement has reached, got {response}"
    );
    id_of(&response)
}

/// A statement that takes real time is observable as STARTED while the engine
/// runs it, and that is the state in which a caller can stop it: the cancel
/// answers that it stopped the query, and the statement lands ABORTED carrying
/// the backend's own reason for stopping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_statement_is_started_and_cancelling_it_aborts_it() {
    let emulator = Emulator::new();
    let id = submit(&emulator, "dev", SLOW_SQL).await;
    let running = wait_until_started(&emulator, "a long statement", &id).await;
    assert_eq!(
        running["Duration"].as_i64().unwrap_or_default(),
        0,
        "a statement that has not stopped has not reported how long it took: {running}"
    );

    let cancelled = emulator.ok("CancelStatement", &json!({"Id": id})).await;
    assert_eq!(
        cancelled["Status"],
        json!(true),
        "cancelling a running statement answers that it was stopped: {cancelled}"
    );

    let aborted = wait_for_terminal(&emulator, "a cancelled statement", &id).await;
    assert_eq!(
        aborted["Status"], "ABORTED",
        "a cancelled statement lands ABORTED: {aborted}"
    );
    let error = aborted["Error"].as_str().unwrap_or_default();
    assert!(
        error.ends_with("cancelled on user's request"),
        "a cancelled statement reports that the user's request is what stopped it, got {aborted}"
    );
    let query_id = aborted["RedshiftQueryId"].as_i64().unwrap_or_default();
    assert!(
        error.contains(&query_id.to_string()),
        "the reason names the query that was stopped ({query_id}), got {aborted}"
    );
    assert!(
        aborted["HasResultSet"].as_bool() != Some(true),
        "a cancelled statement has no result set to fetch: {aborted}"
    );
}

/// Cancelling a statement that has already stopped is refused, naming the state
/// it is in — whichever of the terminal states that is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_stopped_statement_is_refused_naming_its_state() {
    let emulator = Emulator::new();

    // One statement per terminal state, each reached the way a statement really
    // reaches it: one that ran, one the engine refused, one a caller stopped.
    let finished = submit(&emulator, "finished-db", "SELECT 1").await;
    wait_for_terminal(&emulator, "a statement that ran", &finished).await;
    let failed = submit(&emulator, "failed-db", "SELECT * FROM absent_table").await;
    wait_for_terminal(&emulator, "a statement the engine refused", &failed).await;
    let aborted = submit(&emulator, "aborted-db", SLOW_SQL).await;
    wait_until_started(&emulator, "a statement to stop", &aborted).await;
    emulator
        .ok("CancelStatement", &json!({"Id": aborted}))
        .await;
    wait_for_terminal(&emulator, "a statement a caller stopped", &aborted).await;

    for (id, state) in [
        (finished, "FINISHED"),
        (failed, "FAILED"),
        (aborted, "ABORTED"),
    ] {
        let (status, refused) = emulator.call("CancelStatement", &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "case {state}: cancelling a stopped query is refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {state}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Message"],
            json!(format!(
                "Could not cancel a query that is already in {state} state with ID: {id}"
            )),
            "case {state}: expected the sentence the service refuses a stopped query with, got \
             {refused}"
        );
    }
}

/// A statement submitted with a `SessionId` runs on the session's connection,
/// so what an earlier statement of that session built is there for it.
///
/// The rows differ in WHAT the first statement of the session leaves behind — a
/// temporary table, rows in it, a view over it — so the rule is asserted over
/// the session's state and not over one object.
#[tokio::test]
async fn a_sessions_statements_share_what_each_of_them_builds() {
    let emulator = Emulator::new();

    // The first statement asks to keep a session alive, which is what opens one.
    let opened = emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": "CREATE TEMPORARY TABLE sess_tmp (id INTEGER, label VARCHAR(16))",
            "SessionKeepAliveSeconds": SESSION_KEEP_ALIVE_SECONDS,
        }))
        .await;
    assert_eq!(
        opened["Status"], "FINISHED",
        "the statement that opens a session must run: {opened}"
    );
    let session = opened["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("a statement that asked to keep a session alive reports its \
                                   SessionId: {opened}"))
        .to_string();

    // Every later statement names only the session: the warehouse and the
    // database it runs against are the session's.
    let steps = [
        (
            "an insert into the session's temporary table",
            "INSERT INTO sess_tmp VALUES (1, 'one'), (2, 'two')",
        ),
        (
            "a view over it, built in the same session",
            "CREATE TEMPORARY VIEW sess_view AS SELECT count(*) AS rows_held FROM sess_tmp",
        ),
        (
            "a statement that reads what the earlier ones left",
            "SELECT rows_held FROM sess_view",
        ),
    ];
    let mut last = Value::Null;
    for (label, sql) in steps {
        let response = emulator
            .execute_and_wait(&json!({
                "SessionId": session,
                "Sql": sql,
                "SessionKeepAliveSeconds": SESSION_KEEP_ALIVE_SECONDS,
            }))
            .await;
        assert_eq!(
            response["Status"], "FINISHED",
            "case {label}: a statement of a live session must run on what that session holds, \
             got {response}"
        );
        assert_eq!(
            response["SessionId"], json!(session),
            "case {label}: the statement runs in the session it named, got {response}"
        );
        last = emulator
            .ok("DescribeStatement", &json!({"Id": response["Id"]}))
            .await;
    }

    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": last["Id"]}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": 2}]]),
        "the session's last statement must see the rows its earlier statements inserted, got \
         {fetched}"
    );

    // The session is the connection, so the database it was opened with is the
    // one its statements ran against.
    let listed = emulator
        .ok("ListSessions", &json!({"ClusterIdentifier": "my-cluster"}))
        .await;
    let sessions = listed["Sessions"].as_array().cloned().unwrap_or_default();
    assert!(
        sessions
            .iter()
            .any(|held| held["SessionId"] == json!(session) && held["Database"] == json!("dev")),
        "the session the statements ran in is the one this service holds, got {listed}"
    );
}

/// Every statement here is submitted with the modelled ceiling on patience where
/// it waits at all, so a statement that was waited for has really stopped.
const _: () = assert!(
    WAIT_SECONDS > 0,
    "the suites above wait for the statements they wait for"
);
