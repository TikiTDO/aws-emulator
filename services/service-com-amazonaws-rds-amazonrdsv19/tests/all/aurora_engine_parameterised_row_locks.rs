// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a ROW-LOCKING clause carried by a statement whose predicate values
//! are BOUND rather than written into its text — the form every driver sends for
//! `with_for_update()` over an extended-protocol statement.
//!
//! The rule under test: a row-level locking clause (`FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR
//! SHARE`, `FOR KEY SHARE`, with `NOWAIT` or `SKIP LOCKED`) on a `SELECT` over one base
//! relation is SERVED — the rows the statement's own predicate resolves to are returned and
//! locked for the transaction — however many values the statement binds and wherever it
//! binds them; the only refusal the clause itself produces is the engine's own
//! `lock_not_available` (SQLSTATE `55P03`) for a row another transaction already holds, and a
//! statement that is faulty for some OTHER reason is refused with the very condition it
//! would be refused with without the clause. The endpoint reports no limit of its own for a
//! locking clause.
//!
//! The rows of each table span what the rule generalises over: the strength of the clause,
//! the wait policy, the NUMBER of bound values (none, one, two), WHERE in the statement they
//! are bound (the predicate, the projection), the TYPE of the value bound (an integer, a
//! text value), how many rows the predicate resolves to (none, one, several), and the class
//! of fault a statement can carry.
//!
//! The client is the one the sibling endpoint suites drive
//! ([`crate::db_instance_engine_endpoint`]), written against the PostgreSQL wire itself, so
//! the exchange under test is `Parse`/`Bind`/`Describe`/`Execute` — what a stock driver
//! sends — and everything runs in ONE process: the cluster's listener is opened by the
//! `CreateDBCluster` call below.

use crate::db_clusters::describe_one;
use crate::db_instance_engine_endpoint::{postgres_login, PostgresSession, Refusal, Transport};
use crate::db_instances::{call, emulator, expect_ok, text, wait_for_settle};

/// The master user every cluster below is created with, and its password: a test credential,
/// and the only place in this suite it appears.
const MASTER_USER: &str = "sim2real";
const MASTER_PASSWORD: &str = "Sim2RealPassw0rd";

/// The `DatabaseName` the clusters are created with, which every session here lands in.
const DATABASE: &str = "lockingtruth";

/// The relation every statement below locks rows of.
const RELATION: &str = "s2r_held";

/// `lock_not_available`: what PostgreSQL answers a statement that asked for a row another
/// transaction holds and asked not to wait for it. Source: the condition name in
/// PostgreSQL's "Appendix A. PostgreSQL Error Codes".
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// `program_limit_exceeded`: the class a resource limit of the SERVER is reported under. No
/// locking clause may ever be refused under it — the clause's own refusal is
/// [`LOCK_NOT_AVAILABLE`]. Source: the same appendix.
const PROGRAM_LIMIT_EXCEEDED: &str = "54000";

/// The table every case starts from: three rows, keyed by an integer primary key and also
/// carrying a text column, so a predicate can bind either type.
const SEED: [&str; 2] = [
    "CREATE TABLE s2r_held (id integer PRIMARY KEY, n integer NOT NULL, label text NOT NULL)",
    "INSERT INTO s2r_held (id, n, label) VALUES (1, 10, 'alpha'), (2, 20, 'beta'), (3, 30, 'gamma')",
];

/// Create an Aurora PostgreSQL cluster, wait for it to settle, and answer with the endpoint
/// it publishes.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> (String, u16) {
    expect_ok(
        identifier,
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", MASTER_USER),
                ("MasterUserPassword", MASTER_PASSWORD),
                ("DatabaseName", DATABASE),
                ("BackupRetentionPeriod", "1"),
            ],
        )
        .await,
    );
    // A client connects once the cluster reports `available`, which the clock makes due.
    wait_for_settle();
    let settled = describe_one(router, identifier, identifier).await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, "available",
        "case {identifier:?}: the cluster must settle to available before a client connects, \
         got {status:?}"
    );
    let address = text(&settled, "Endpoint");
    let port = text(&settled, "Port");
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {identifier:?}: expected a port, got {port:?}"));
    (address, port)
}

/// One session on that endpoint, logged in as the cluster's master user.
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

/// The rows of a single-column answer to a parameterised statement, as text.
async fn bound_column(
    case: &str,
    session: &mut PostgresSession,
    sql: &str,
    params: &[&str],
) -> Result<Vec<String>, Refusal> {
    session
        .query_with_params(sql, params)
        .await
        .map(|answer| {
            answer
                .rows
                .iter()
                .map(|row| row.first().cloned().flatten().unwrap_or_default())
                .collect()
        })
        .inspect_err(|refusal| {
            assert_ne!(
                refusal.code, PROGRAM_LIMIT_EXCEEDED,
                "case {case:?}: {sql:?} bound to {params:?} must never be refused as a limit \
                 of this endpoint, got {refusal:?}"
            );
        })
}

/// What a second session reads when it asks for one row of the relation and will not wait.
async fn nowait_on(case: &str, session: &mut PostgresSession, id: &str) -> Result<Vec<String>, Refusal> {
    let sql = format!("SELECT n FROM {RELATION} WHERE id = $1 FOR UPDATE NOWAIT");
    bound_column(case, session, &sql, &[id]).await
}

// ---------------------------------------------------------------------------
// A parameterised locking SELECT is served, and the rows it selected are held.
// ---------------------------------------------------------------------------

/// One row: a parameterised locking statement, the rows it must answer with, and the rows of
/// the relation it must then be holding.
struct LockingCase {
    case: &'static str,
    /// The statement, with its placeholders as a driver writes them.
    sql: &'static str,
    /// The values bound to it, in order.
    params: &'static [&'static str],
    /// The single-column answer it must produce, as text.
    rows: &'static [&'static str],
    /// The `id` of every row it must then hold, so a second session asking for one of them
    /// without waiting is refused.
    holds: &'static [&'static str],
}

/// The rows span the clause strengths PostgreSQL defines, both forms of its wait policy, the
/// number and placement of bound values, the type of the value bound, and how many rows the
/// predicate resolves to.
///
/// Source for the clause forms: the `FOR UPDATE`/`FOR SHARE` section of PostgreSQL's
/// `SELECT` reference, which lists `FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE` and `FOR
/// KEY SHARE`, each optionally followed by `NOWAIT` or `SKIP LOCKED`.
const LOCKING_CASES: &[LockingCase] = &[
    LockingCase {
        case: "one bound integer, FOR UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR UPDATE NOWAIT",
        params: &["2"],
        rows: &["20"],
        holds: &["2"],
    },
    LockingCase {
        case: "one bound integer, FOR UPDATE",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR UPDATE",
        params: &["1"],
        rows: &["10"],
        holds: &["1"],
    },
    LockingCase {
        case: "one bound integer, FOR NO KEY UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR NO KEY UPDATE NOWAIT",
        params: &["3"],
        rows: &["30"],
        holds: &["3"],
    },
    LockingCase {
        case: "one bound integer, FOR SHARE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR SHARE NOWAIT",
        params: &["1"],
        rows: &["10"],
        holds: &["1"],
    },
    LockingCase {
        case: "one bound integer, FOR KEY SHARE",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR KEY SHARE",
        params: &["2"],
        rows: &["20"],
        holds: &["2"],
    },
    LockingCase {
        case: "one bound TEXT value in the predicate, FOR UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE label = $1 FOR UPDATE NOWAIT",
        params: &["beta"],
        rows: &["20"],
        holds: &["2"],
    },
    LockingCase {
        case: "two bound integers spanning several rows, FOR UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id >= $1 AND id <= $2 ORDER BY id FOR UPDATE NOWAIT",
        params: &["1", "2"],
        rows: &["10", "20"],
        holds: &["1", "2"],
    },
    LockingCase {
        case: "a bound value in the projection and another in the predicate, FOR UPDATE",
        sql: "SELECT n + $1 FROM s2r_held WHERE id = $2 FOR UPDATE",
        params: &["5", "3"],
        rows: &["35"],
        holds: &["3"],
    },
    LockingCase {
        case: "no bound value at all, FOR UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id = 1 FOR UPDATE NOWAIT",
        params: &[],
        rows: &["10"],
        holds: &["1"],
    },
    LockingCase {
        case: "a bound value the predicate resolves to no row for, FOR UPDATE NOWAIT",
        sql: "SELECT n FROM s2r_held WHERE id = $1 FOR UPDATE NOWAIT",
        params: &["99"],
        rows: &[],
        holds: &[],
    },
    LockingCase {
        case: "a bound bound over every free row, FOR UPDATE SKIP LOCKED",
        sql: "SELECT n FROM s2r_held WHERE id <= $1 ORDER BY id FOR UPDATE SKIP LOCKED",
        params: &["3"],
        rows: &["10", "20", "30"],
        holds: &["1", "2", "3"],
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parameterised_locking_select_is_served_and_holds_the_rows_it_selected() {
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-bound-locks-served").await;
    let mut setup = session("setup", &address, port).await;
    for statement in SEED {
        must_run("setup", &mut setup, statement).await;
    }

    for row in LOCKING_CASES {
        let case = row.case;
        // Each case starts from the seeded relation, whatever the one before it left.
        must_run(case, &mut setup, &format!("DELETE FROM {RELATION}")).await;
        must_run(case, &mut setup, SEED[1]).await;

        let mut holder = session(case, &address, port).await;
        must_run(case, &mut holder, "BEGIN").await;
        let served = bound_column(case, &mut holder, row.sql, row.params).await;
        let answered = served.unwrap_or_else(|refusal| {
            panic!(
                "case {case:?}: {:?} bound to {:?} must be served over an uncontended \
                 relation, got {refusal:?}",
                row.sql, row.params
            )
        });
        let expected: Vec<String> = row.rows.iter().map(|value| (*value).to_string()).collect();
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the rows {expected:?}, got {answered:?}"
        );

        // The lock really was taken: a SECOND session asking for one of those rows without
        // waiting reads the engine's own condition for a row another transaction holds, and
        // the rows the statement did NOT select are untouched.
        let mut other = session(case, &address, port).await;
        for id in row.holds {
            let refusal = nowait_on(case, &mut other, id)
                .await
                .err()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: row {id} was selected by {:?}, so another session \
                         must not be able to lock it",
                        row.sql
                    )
                });
            assert_eq!(
                (refusal.code.as_str(), refusal.message.as_str()),
                (
                    LOCK_NOT_AVAILABLE,
                    format!("could not obtain lock on row in relation \"{RELATION}\"").as_str()
                ),
                "case {case:?}: row {id} must be refused with the engine's own lock \
                 condition, got {refusal:?}"
            );
        }
        for id in ["1", "2", "3"] {
            if row.holds.contains(&id) {
                continue;
            }
            let free = nowait_on(case, &mut other, id).await;
            assert!(
                free.is_ok(),
                "case {case:?}: row {id} was not selected by {:?}, so another session must be \
                 able to lock it, got {free:?}",
                row.sql
            );
            must_run(case, &mut other, "ROLLBACK").await;
        }

        // The hold lasts exactly as long as the transaction: once it ends, every row is free.
        must_run(case, &mut holder, "ROLLBACK").await;
        for id in row.holds {
            let after = nowait_on(case, &mut other, id).await;
            assert!(
                after.is_ok(),
                "case {case:?}: once the holder ended, row {id} must be lockable again, got \
                 {after:?}"
            );
        }
        must_run(case, &mut other, "ROLLBACK").await;
    }
}

// ---------------------------------------------------------------------------
// The clause's own refusal, and what happens once the holder commits.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parameterised_nowait_is_refused_for_a_contended_row_and_granted_once_it_commits() {
    let case = "a contended row, asked for by a bound NOWAIT";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-bound-locks-contended").await;
    let mut holder = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut holder, statement).await;
    }

    // One transaction claims row 1 and writes it, and keeps the transaction open.
    must_run(case, &mut holder, "BEGIN").await;
    must_run(
        case,
        &mut holder,
        &format!("UPDATE {RELATION} SET n = 111 WHERE id = 1"),
    )
    .await;

    let mut client = session(case, &address, port).await;
    must_run(case, &mut client, "BEGIN").await;

    // The UNCONTENDED neighbour is served and reads its committed value.
    let neighbour = nowait_on(case, &mut client, "2").await;
    assert_eq!(
        neighbour,
        Ok(vec!["20".to_string()]),
        "case {case:?}: an uncontended row must be locked and returned, got {neighbour:?}"
    );

    // The CONTENDED row is refused — with the engine's lock condition, not a limit.
    let contended = nowait_on(case, &mut client, "1").await.err().unwrap_or_else(|| {
        panic!("case {case:?}: a row the other transaction holds must not be locked")
    });
    assert_eq!(
        (contended.code.as_str(), contended.message.as_str()),
        (
            LOCK_NOT_AVAILABLE,
            format!("could not obtain lock on row in relation \"{RELATION}\"").as_str()
        ),
        "case {case:?}: expected the engine's own lock condition, got {contended:?}"
    );

    // A PLAIN read of the contended row still reads its committed value: the other
    // transaction's write is its own until it commits.
    let plain = bound_column(
        case,
        &mut client,
        &format!("SELECT n FROM {RELATION} WHERE id = $1"),
        &["1"],
    )
    .await;
    assert_eq!(
        plain,
        Ok(vec!["10".to_string()]),
        "case {case:?}: a plain read must see the committed value, got {plain:?}"
    );

    // Once the holder COMMITS, the same bound NOWAIT is granted and reads what it wrote.
    must_run(case, &mut client, "ROLLBACK").await;
    must_run(case, &mut holder, "COMMIT").await;
    must_run(case, &mut client, "BEGIN").await;
    let granted = nowait_on(case, &mut client, "1").await;
    assert_eq!(
        granted,
        Ok(vec!["111".to_string()]),
        "case {case:?}: once the holder committed, the NOWAIT must be granted and read the \
         committed value, got {granted:?}"
    );
    must_run(case, &mut client, "ROLLBACK").await;
}

// ---------------------------------------------------------------------------
// A faulty statement is refused with its OWN condition, clause or no clause.
// ---------------------------------------------------------------------------

/// One row: a statement that is faulty for a reason of its own, written both with and
/// without a locking clause.
struct FaultyCase {
    case: &'static str,
    /// The statement without a locking clause.
    plain: &'static str,
    /// The same statement with one.
    locking: &'static str,
    /// The values bound to both.
    params: &'static [&'static str],
}

/// The rows span the classes of fault a single-relation `SELECT` can carry: a relation the
/// database has not got, a column it has not got, a function it has not got, and a bind
/// message that does not match the statement's placeholders.
const FAULTY_CASES: &[FaultyCase] = &[
    FaultyCase {
        case: "a relation the database has not got",
        plain: "SELECT n FROM s2r_absent WHERE id = $1",
        locking: "SELECT n FROM s2r_absent WHERE id = $1 FOR UPDATE NOWAIT",
        params: &["1"],
    },
    FaultyCase {
        case: "a column the relation has not got",
        plain: "SELECT absent FROM s2r_held WHERE id = $1",
        locking: "SELECT absent FROM s2r_held WHERE id = $1 FOR UPDATE NOWAIT",
        params: &["1"],
    },
    FaultyCase {
        case: "a column the relation has not got, in the predicate",
        plain: "SELECT n FROM s2r_held WHERE absent = $1",
        locking: "SELECT n FROM s2r_held WHERE absent = $1 FOR SHARE NOWAIT",
        params: &["1"],
    },
    FaultyCase {
        case: "a function the engine has not got",
        plain: "SELECT no_such_function(n) FROM s2r_held WHERE id = $1",
        locking: "SELECT no_such_function(n) FROM s2r_held WHERE id = $1 FOR UPDATE",
        params: &["1"],
    },
    FaultyCase {
        case: "more values bound than the statement has placeholders",
        plain: "SELECT n FROM s2r_held WHERE id = $1",
        locking: "SELECT n FROM s2r_held WHERE id = $1 FOR UPDATE NOWAIT",
        params: &["1", "2"],
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locking_clause_does_not_change_which_condition_a_faulty_statement_is_refused_with() {
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-bound-locks-faults").await;
    let mut client = session("setup", &address, port).await;
    for statement in SEED {
        must_run("setup", &mut client, statement).await;
    }

    for row in FAULTY_CASES {
        let case = row.case;
        let plain = client
            .query_with_params(row.plain, row.params)
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {:?} must be refused, it was served", row.plain)
            });
        let locking = client
            .query_with_params(row.locking, row.params)
            .await
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {:?} must be refused, it was served",
                    row.locking
                )
            });
        assert_ne!(
            locking.code, PROGRAM_LIMIT_EXCEEDED,
            "case {case:?}: a locking clause must not be refused as a limit of this \
             endpoint, got {locking:?}"
        );
        assert_eq!(
            locking, plain,
            "case {case:?}: the locking form must be refused with the same condition as the \
             plain form {plain:?}, got {locking:?}"
        );
        // The session goes on serving after each refusal.
        let still = bound_column(
            case,
            &mut client,
            &format!("SELECT n FROM {RELATION} WHERE id = $1"),
            &["1"],
        )
        .await;
        assert_eq!(
            still,
            Ok(vec!["10".to_string()]),
            "case {case:?}: the endpoint must still serve after the refusals, got {still:?}"
        );
    }
}
