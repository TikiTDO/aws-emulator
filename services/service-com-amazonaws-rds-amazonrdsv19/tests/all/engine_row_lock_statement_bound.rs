// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for WHOSE budget a statement on the ROW-LOCKING path of a PostgreSQL
//! DB instance's endpoint runs under.
//!
//! The rule under test: the row query this endpoint evaluates to identify the rows a
//! locking statement will take runs under the budget of the session that SUBMITTED the
//! statement — never under a budget another session left behind — so a statement whose
//! predicate waits past its own `statement_timeout` is cancelled with PostgreSQL's own
//! `canceling statement due to statement timeout` (SQLSTATE `57014`) even when the
//! session that ran last on that endpoint had its timeout disabled.
//!
//! The rows span the statement forms whose row query the endpoint evaluates ahead of the
//! statement itself and which then go on to run: an `UPDATE` and a `DELETE`. The explicit
//! locking-clause form is left to
//! [`crate::engine_server_surface::a_waiting_function_in_a_locking_statements_predicate_stops_at_the_deadline`],
//! which is where what this endpoint answers for a locking statement whose rows it could
//! not identify in time belongs; that sibling test also covers the single-session case of
//! this rule, and this one adds the budget a statement must not inherit.
//!
//! The client is the one the sibling endpoint suites drive
//! ([`crate::db_instance_engine_endpoint`]), written against the PostgreSQL wire itself,
//! and everything runs in ONE process: the instance's listener is opened by the
//! `CreateDBInstance` call below.

use std::time::{Duration, Instant};

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Transport, DATABASE, MASTER_PASSWORD,
    MASTER_USER,
};
use crate::db_instances::emulator;

/// The session timeout every statement below is run under: long enough that reaching the
/// engine is not a race, far shorter than [`PREDICATE_WAIT`].
const STATEMENT_TIMEOUT_MS: u64 = 250;

/// How long each statement's own predicate asks to wait — an order of magnitude past
/// [`STATEMENT_TIMEOUT_MS`], so a statement that served that wait cannot be mistaken for
/// one the timeout cancelled.
const PREDICATE_WAIT: &str = "5";

/// The longest a cancelled statement may take to come back: well under [`PREDICATE_WAIT`],
/// well over [`STATEMENT_TIMEOUT_MS`].
const CANCELLED_WITHIN: Duration = Duration::from_millis(2_500);

/// One session on the instance's endpoint, logged in as its master user.
async fn session(case: &str, address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the master user must be admitted, got {refusal:?}")
    })
}

/// Run one statement that must succeed.
async fn must_run(case: &str, session: &mut PostgresSession, sql: &str) {
    let answer = session.query(sql).await;
    assert!(
        answer.is_ok(),
        "case {case:?}: {sql:?} must run, got {answer:?}"
    );
}

/// A statement whose row predicate waits far past its OWN session's `statement_timeout` is
/// cancelled on that timeout, even when the session that ran last on the endpoint had the
/// timeout disabled.
#[tokio::test]
async fn a_locking_statement_does_not_inherit_another_sessions_budget() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-rowlock-bound-postgres",
        "postgres",
        &[("EngineVersion", "16.4".to_string())],
    )
    .await;

    let mut seeding = session("seed", &address, port).await;
    must_run(
        "seed",
        &mut seeding,
        "CREATE TABLE s2r_bounded (id integer PRIMARY KEY, n integer NOT NULL)",
    )
    .await;
    must_run(
        "seed",
        &mut seeding,
        "INSERT INTO s2r_bounded (id, n) VALUES (1, 10), (2, 20), (3, 30)",
    )
    .await;

    // The rows span the statement forms whose row query the endpoint evaluates and which
    // then run: the locks they take are the engine's own and implicit.
    let forms = [
        (
            "an UPDATE whose predicate waits",
            format!("UPDATE s2r_bounded SET n = n + 1 WHERE pg_sleep({PREDICATE_WAIT}) IS NULL"),
        ),
        (
            "a DELETE whose predicate waits",
            format!("DELETE FROM s2r_bounded WHERE pg_sleep({PREDICATE_WAIT}) IS NULL"),
        ),
    ];

    for (case, sql) in forms {
        // ANOTHER session runs last with its timeout disabled, so the budget most recently
        // in force on this endpoint is the long one. The statement below must still be
        // cancelled on its OWN session's timeout.
        let mut unbounded = session(case, &address, port).await;
        must_run(case, &mut unbounded, "SET statement_timeout = 0").await;
        must_run(case, &mut unbounded, "SELECT 1").await;

        let mut bounded = session(case, &address, port).await;
        must_run(
            case,
            &mut bounded,
            &format!("SET statement_timeout = {STATEMENT_TIMEOUT_MS}"),
        )
        .await;

        let started = Instant::now();
        let answer = bounded.query(&sql).await;
        let elapsed = started.elapsed();

        let refusal = answer.err().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a statement waiting {PREDICATE_WAIT}s under a \
                 {STATEMENT_TIMEOUT_MS}ms statement_timeout must be cancelled, but \
                 {sql:?} succeeded after {elapsed:?}"
            )
        });
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            ("57014", "canceling statement due to statement timeout"),
            "case {case:?}: the cancellation must be PostgreSQL's own, got {refusal:?}"
        );
        assert!(
            elapsed < CANCELLED_WITHIN,
            "case {case:?}: the statement must be cancelled on its own session's budget \
             rather than after its predicate's wait, got {elapsed:?}"
        );

        // The endpoint is still serving: the cancellation ended one statement, not the
        // instance's engine — and a cancelled statement changed no row.
        must_run(case, &mut bounded, "SET statement_timeout = 0").await;
        let rows = bounded
            .query("SELECT count(*) FROM s2r_bounded")
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}: the endpoint must still serve, got {refusal:?}")
            });
        assert_eq!(
            rows.only(),
            Some("3"),
            "case {case:?}: a cancelled statement changes no row, got {rows:?}"
        );
    }
}
