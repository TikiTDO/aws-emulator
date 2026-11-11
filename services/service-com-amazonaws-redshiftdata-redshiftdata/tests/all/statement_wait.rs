// SPDX-License-Identifier: CC0-1.0

//! What a caller's `WaitTimeSeconds` decides for a BATCH submit, driven over the
//! wire through the shared harness in [`super::emulator`]: every call below is a
//! real `POST /` with `X-Amz-Target: RedshiftData.<Operation>`, so what is
//! asserted is the emulator's HTTP behaviour and not a handler's return value.
//!
//! The rule is the one [`super::caller_patience`] asserts for
//! `ExecuteStatement`, here for `BatchExecuteStatement` — the other operation
//! carrying `WaitTimeSeconds`, and the one whose submission is recorded as ONE
//! statement with its statements underneath it: the wait bounds the RESPONSE and
//! never the execution, so a batch reaches the terminal status its execution
//! decided whatever its caller asked to wait for, a caller whose wait runs out
//! first is answered with the status the batch has reached by then, and a caller
//! whose wait outlasts the service's own execution bound is answered with the
//! verdict itself.
//!
//! It is asserted over a TABLE rather than one case: the rows differ along the
//! patience the submit asks for (none at all, the shortest the model admits, the
//! longest it admits) and along what the batch does (statements the engine runs
//! to completion, a batch holding one that runs until the engine's own bound
//! stops it). The table's shape and its verdict assertion are the harness's, so
//! the two submit operations are driven from one shape.

use serde_json::json;

use super::emulator::{
    assert_verdict, list, wait_for_terminal, Emulator, PatienceRow, Verdict, QUICK_SQL,
    SHORTEST_MODELLED_WAIT_SECONDS, UNBOUNDED_SQL, WAIT_SECONDS,
};

/// Statements the engine runs to completion as soon as it picks the batch up,
/// each one reading what the previous one wrote.
const QUICK_SQLS: &[&str] = &[
    "CREATE TABLE t (id INTEGER)",
    "INSERT INTO t (id) VALUES (1), (2)",
    "SELECT id FROM t ORDER BY id",
];

/// A batch whose second statement runs until a bound of the engine stops it, so
/// the batch's own verdict is that interruption.
const INTERRUPTED_SQLS: &[&str] = &[QUICK_SQL, UNBOUNDED_SQL];

/// A batch caller's `WaitTimeSeconds` bounds its own response and nothing else:
/// the batch still reaches the verdict its execution produced, and a later read
/// reports it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_callers_wait_bounds_its_own_response_and_not_the_execution() {
    let emulator = Emulator::new();

    // The row that asks for a long wait comes last, so the executions of the
    // rows before it are already in flight while that request waits rather than
    // being started one after another.
    let rows = [
        PatienceRow {
            label: "no wait at all, for a batch the engine completes",
            sqls: QUICK_SQLS,
            wait: None,
            // Nobody waited, so whether this batch's execution has already
            // reported when the response is written is the engine's business.
            answered: &["SUBMITTED", "PICKED", "STARTED", "FINISHED"],
            verdict: Verdict::Finished,
        },
        PatienceRow {
            label: "the shortest wait the model admits, for a batch the engine's bound stops",
            sqls: INTERRUPTED_SQLS,
            wait: Some(SHORTEST_MODELLED_WAIT_SECONDS),
            // This wait does not cover the service's execution bound, so what it
            // reports is the status reached by the time it expires: the
            // non-terminal one, or the verdict if a bound of the engine happened
            // to stop the batch inside the wait. Which one it is must not depend
            // on the machine, so both are admitted here and the verdict itself
            // is pinned by the read below.
            answered: &["SUBMITTED", "PICKED", "STARTED", "FAILED"],
            verdict: Verdict::Interrupted,
        },
        PatienceRow {
            label: "the shortest wait the model admits, for a batch the engine completes",
            sqls: QUICK_SQLS,
            wait: Some(SHORTEST_MODELLED_WAIT_SECONDS),
            answered: &["SUBMITTED", "PICKED", "STARTED", "FINISHED"],
            verdict: Verdict::Finished,
        },
        PatienceRow {
            label: "a wait that outlasts the service's own execution bound",
            sqls: INTERRUPTED_SQLS,
            wait: Some(WAIT_SECONDS),
            // This wait covers the engine's bound, so the verdict is in this
            // very response.
            answered: &["FAILED"],
            verdict: Verdict::Interrupted,
        },
    ];

    let mut submitted: Vec<(&PatienceRow, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // Each row runs on a database of its own: the submissions of ONE
        // database run one at a time, so rows sharing one would queue behind
        // each other instead of running side by side.
        let request = json!({
            "ClusterIdentifier": "my-cluster",
            "Database": format!("dev{index}"),
            "DbUser": "awsuser",
            "Sqls": row.sqls,
        });
        let id = emulator
            .submit_with_patience("BatchExecuteStatement", &request, row)
            .await;
        submitted.push((row, id));
    }

    for (row, id) in submitted {
        let described = wait_for_terminal(&emulator, row.label, &id).await;

        // The record read back is this row's own batch: ONE statement carrying
        // the statements it submitted, in order.
        let texts: Vec<String> = list(&described, "SubStatements")
            .iter()
            .map(|sub| sub["QueryString"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = row.sqls.iter().map(|sql| (*sql).to_string()).collect();
        assert_eq!(
            texts, expected,
            "case {}: the batch read back must carry the statements this row submitted, got \
             {described}",
            row.label
        );

        assert_verdict(row, &described);
    }

    // And the service is still serving after every one of those batches: a
    // caller that stopped waiting left nothing behind.
    let id = emulator.run(QUICK_SQL).await;
    let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "a statement submitted after the rows above must still run, got {described}"
    );
}
