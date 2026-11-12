// SPDX-License-Identifier: CC0-1.0

//! Concurrently submitted statements, driven over the wire through the shared
//! harness in [`super::emulator`]: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! The rule under test: a submission's execution belongs to the SUBMISSION and
//! not to the request that made it. Every statement the service accepts is
//! recorded under the `Id` it answers with, runs to a terminal status on its
//! own, and the statements of one database run one at a time and in submission
//! order — so submissions that contend for the same row serialize instead of
//! interleaving, each one reaches a decision, and none is refused or left
//! without a status because another one was running. It is asserted over a
//! TABLE of contention shapes rather than one case: the rows differ along how
//! many submissions contend (two, three), what each one does (a
//! read-modify-write transaction, one that does real work over a filler table
//! first, a single statement), and whether the callers asked to wait.

use std::sync::Arc;

use serde_json::{json, Value};

use super::emulator::{
    error_code, id_of, list, wait_for_terminal, Emulator, QUICK_SQL, TERMINAL_STATUSES,
    WAIT_SECONDS,
};

use service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_ACTIVE_STATEMENTS;

/// Rows the filler table holds, so a transaction that aggregates over it does
/// real work between its read and its write.
const FILLER_ROWS: usize = 200;

/// The connection every submission below is made with: one documented
/// authorization combination (a provisioned cluster as a database user).
fn connection() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// Create the contended row's table and the filler table, waiting for each
/// statement, so the contention below starts from a known state.
async fn seed(emulator: &Emulator) {
    for sql in [
        "CREATE TABLE contended (id INT, n INT, v VARCHAR(16))".to_string(),
        "INSERT INTO contended (id, n, v) VALUES (1, 0, 'seed')".to_string(),
        "CREATE TABLE filler (id INT, v VARCHAR(16))".to_string(),
        format!(
            "INSERT INTO filler (id, v) VALUES {}",
            (0..FILLER_ROWS)
                .map(|index| format!("({index}, 'row{index}')"))
                .collect::<Vec<String>>()
                .join(",")
        ),
    ] {
        let mut request = connection();
        request["Sql"] = json!(sql);
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let response = emulator.ok("ExecuteStatement", &request).await;
        assert_eq!(
            response["Status"], "FINISHED",
            "seeding [{sql}] must FINISH, got {response}"
        );
    }
}

/// The one row the contended table holds, as `(n, v, row count)`.
async fn contended_row(emulator: &Emulator, case: &str) -> (i64, String, i64) {
    let mut request = connection();
    request["Sql"] = json!("SELECT n, v, (SELECT count(*) FROM contended) FROM contended");
    request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let read = emulator.ok("ExecuteStatement", &request).await;
    assert_eq!(
        read["Status"], "FINISHED",
        "case {case}: reading the contended row must FINISH, got {read}"
    );
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": id_of(&read)}))
        .await;
    let records = list(&fetched, "Records");
    assert_eq!(
        records.len(),
        1,
        "case {case}: the contended table holds one row, got {fetched}"
    );
    let cells = records[0]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a record is a list of fields, got {fetched}"));
    let number = |index: usize| {
        cells[index]["longValue"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: field {index} is no number: {fetched}"))
    };
    (
        number(0),
        cells[1]["stringValue"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        number(2),
    )
}

/// One row of the contention table: what is submitted concurrently, and how.
struct ContentionCase {
    label: &'static str,
    /// The statements of each submission, in the order they are submitted. One
    /// entry per concurrent submission; an entry holding one statement is
    /// submitted as a single statement rather than as a batch.
    submissions: Vec<Vec<String>>,
    /// Whether each caller asks to wait for its own submission.
    waited: bool,
}

/// A read-modify-write transaction that derives the contended counter from the
/// value it read, labelled with `writer` — the shape a lost update shows up in.
/// `work` aggregates over the filler table that many times between the read and
/// the write, so the transactions really overlap.
fn read_modify_write(writer: &str, work: usize) -> Vec<String> {
    let mut sqls = vec!["SELECT n FROM contended WHERE id = 1".to_string()];
    for _ in 0..work {
        sqls.push("SELECT count(*), sum(a.id * b.id) FROM filler a, filler b".to_string());
    }
    sqls.push(format!(
        "UPDATE contended SET n = (SELECT max(n) FROM contended) + 1, v = '{writer}' \
         WHERE id = 1"
    ));
    sqls
}

/// A single statement that increments the contended counter on its own.
fn single_increment(writer: &str) -> Vec<String> {
    vec![format!(
        "UPDATE contended SET n = (SELECT max(n) FROM contended) + 1, v = '{writer}' \
         WHERE id = 1"
    )]
}

/// Every concurrently submitted transaction reaches a terminal status, the
/// contended counter ends equal to the number that committed, the surviving
/// value belongs to a committed writer, and the row count is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_submissions_on_one_row_each_reach_a_decision() {
    let cases = vec![
        ContentionCase {
            label: "two read-modify-write transactions, no work beside the row",
            submissions: vec![read_modify_write("w1", 0), read_modify_write("w2", 0)],
            waited: false,
        },
        ContentionCase {
            label: "two read-modify-write transactions doing real work",
            submissions: vec![read_modify_write("w1", 4), read_modify_write("w2", 4)],
            waited: false,
        },
        ContentionCase {
            label: "three transactions, the middle one doing more work",
            submissions: vec![
                read_modify_write("w1", 1),
                read_modify_write("w2", 6),
                read_modify_write("w3", 1),
            ],
            waited: false,
        },
        ContentionCase {
            label: "two transactions whose callers each asked to wait",
            submissions: vec![read_modify_write("w1", 2), read_modify_write("w2", 2)],
            waited: true,
        },
        ContentionCase {
            label: "single statements, each its own transaction",
            submissions: vec![single_increment("w1"), single_increment("w2")],
            waited: false,
        },
    ];

    for case in cases {
        // One emulator per row, so a row never reads the counter another left
        // behind.
        let emulator = Arc::new(Emulator::new());
        seed(&emulator).await;

        // Submitted concurrently: every submission is in flight before any of
        // them is polled.
        let mut inflight = Vec::new();
        for sqls in &case.submissions {
            let mut request = connection();
            let operation = if sqls.len() == 1 {
                request["Sql"] = json!(sqls[0]);
                "ExecuteStatement"
            } else {
                request["Sqls"] = json!(sqls);
                "BatchExecuteStatement"
            };
            if case.waited {
                request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
            }
            let emulator = Arc::clone(&emulator);
            inflight.push(tokio::spawn(async move {
                emulator.call(operation, &request).await
            }));
        }

        let mut submitted = Vec::new();
        for answer in inflight {
            let (status, body) = answer.await.expect("the submitting task");
            assert_eq!(
                status, 200,
                "case {}: every concurrent submission must be accepted, got {status} {body}",
                case.label
            );
            submitted.push(id_of(&body));
        }

        // Every submission reaches a terminal status, and at least one of them
        // commits: contention is resolved by serializing the submissions, never
        // by leaving one of them without a decision.
        let mut committed = 0_i64;
        let mut winners: Vec<String> = Vec::new();
        for (index, id) in submitted.iter().enumerate() {
            let described = wait_for_terminal(&emulator, case.label, id).await;
            let status = described["Status"].as_str().unwrap_or_default().to_string();
            if status == "FINISHED" {
                committed += 1;
                winners.push(format!("w{}", index + 1));
            } else {
                // A transaction that did not commit says why, in the engine's
                // own words.
                assert!(
                    described["Error"]
                        .as_str()
                        .is_some_and(|error| !error.is_empty()),
                    "case {}: a {status} transaction must report why: {described}",
                    case.label
                );
            }
        }
        assert!(
            committed >= 1,
            "case {}: at least one of {} concurrent transactions must commit, {committed} did",
            case.label,
            submitted.len()
        );

        let (counter, survivor, rows) = contended_row(&emulator, case.label).await;
        assert_eq!(
            counter, committed,
            "case {}: the contended counter must equal the number of transactions that \
             committed ({committed}), got {counter} — lower is a lost update, higher is a \
             rolled-back write that survived",
            case.label
        );
        assert!(
            winners.contains(&survivor),
            "case {}: the surviving value must belong to a committed writer {winners:?}, got \
             {survivor:?}",
            case.label
        );
        assert_eq!(
            rows, 1,
            "case {}: the contended table must still hold exactly one row, got {rows}",
            case.label
        );
    }
}

/// The submissions in flight are BOUNDED, and a submission the bound refuses
/// reserves nothing: it takes no turn on the database, so the submissions after
/// it still run.
///
/// The in-flight window is held open by this test rather than by how long the
/// statements take: the turn on the database is taken first
/// ([`Emulator::hold_turn`]), so every submission below waits for it and the
/// bound is asserted over submissions that are all certainly still in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_submissions_in_flight_are_bounded_and_a_refusal_takes_no_turn() {
    let emulator = Emulator::new();

    // The turn on the database every submission below is made to, held by this
    // test: the submissions of one database run one at a time and in submission
    // order, so none of them is picked, started or finished while this guard is
    // alive.
    let held = emulator.hold_turn("my-cluster", "dev");

    // Fill the bound.
    let mut queued = Vec::new();
    for index in 0..MAX_ACTIVE_STATEMENTS {
        let mut request = connection();
        request["Sql"] = json!(QUICK_SQL);
        let response = emulator.ok("ExecuteStatement", &request).await;
        assert_eq!(
            response["Status"], "SUBMITTED",
            "submission {index} must be in flight, got {response}"
        );
        queued.push(id_of(&response));
    }

    // One past the bound is refused with the modelled capacity error.
    let mut past_bound = connection();
    past_bound["Sql"] = json!("SELECT 1");
    let (status, refused) = emulator.call("ExecuteStatement", &past_bound).await;
    assert_eq!(
        status, 400,
        "one submission past the in-flight bound must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ActiveStatementsExceededException",
        "expected the modelled capacity error, got {refused}"
    );

    // Cancelling the queued submissions stops them: none of them had stopped on
    // its own, each reaches a terminal status, so the bound admits submissions
    // again.
    for id in &queued {
        let cancelled = emulator.ok("CancelStatement", &json!({"Id": id})).await;
        assert_eq!(
            cancelled["Status"],
            json!(true),
            "cancelling the in-flight {id} must stop it, got {cancelled}"
        );
        let described = wait_for_terminal(&emulator, "the cancelled queue", id).await;
        assert!(
            TERMINAL_STATUSES.contains(&described["Status"].as_str().unwrap_or_default()),
            "a cancelled submission must reach a terminal status, got {described}"
        );
    }

    // And a submission made after the refusal runs to completion once the turn
    // is handed on: the refused one left no turn taken on the database, and
    // nothing of the queue it was refused behind is still holding one.
    drop(held);
    past_bound["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let after = emulator.ok("ExecuteStatement", &past_bound).await;
    let described = wait_for_terminal(&emulator, "after the refusal", &id_of(&after)).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "a submission after the refusal must still run on that database, got {described}"
    );
}
