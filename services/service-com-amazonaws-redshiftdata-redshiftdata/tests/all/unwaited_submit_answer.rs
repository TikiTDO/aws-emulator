// SPDX-License-Identifier: CC0-1.0

//! What a submit operation ANSWERS WITH, driven over the wire through the shared
//! harness in [`super::emulator`]: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! The rule under test: a submit that asks for no wait does not wait for the
//! execution at all, so it answers with the submission as it was ACCEPTED —
//! `SUBMITTED`, "the query was submitted, but not yet processed" — while a
//! submit that names `WaitTimeSeconds` answers with the status the submission
//! has reached by the time that wait is up ("If the SQL statement does not
//! complete within the specified time, the response returns the current
//! status"). The answer to a submission therefore never depends on how far the
//! execution the service hands over has got while the response is being
//! written, and the execution still runs to a terminal status of its own.
//!
//! It is asserted over a TABLE rather than one case: the rows differ along the
//! operation that submits (one statement, a batch), how many statements the
//! submission carries, what those statements do (work the engine finishes at
//! once, work that takes real time, work the engine refuses) and whether the
//! submit also opens a session. Each row is submitted repeatedly, so a row whose
//! answer depended on the execution would have to win the same race every time.

use serde_json::{json, Value};

use super::emulator::{
    id_of, wait_for_terminal, Emulator, QUICK_SQL, SLOW_SQL, TERMINAL_STATUSES, WAIT_SECONDS,
};

/// How many times each row below is submitted. A handful: enough that an answer
/// read from an independently scheduled execution would show up, bounded so the
/// submissions of all the rows together stay inside the service's in-flight
/// bound.
const SUBMISSIONS_PER_ROW: usize = 4;

/// One row: what is submitted, and how.
struct SubmitRow {
    label: &'static str,
    /// The statements submitted, in submission order.
    sqls: &'static [&'static str],
    /// Whether they are submitted as a BATCH (`BatchExecuteStatement`) rather
    /// than as one statement (`ExecuteStatement`, which carries exactly one).
    batch: bool,
    /// Whether the submit also asks to keep a session alive.
    session: bool,
}

impl SubmitRow {
    /// The operation that submits this row's statements.
    fn operation(&self) -> &'static str {
        if self.batch {
            "BatchExecuteStatement"
        } else {
            "ExecuteStatement"
        }
    }

    /// The request body, against `database`.
    fn request(&self, database: &str) -> Value {
        let mut request = json!({
            "ClusterIdentifier": "my-cluster",
            "Database": database,
            "DbUser": "awsuser",
        });
        if self.batch {
            request["Sqls"] = json!(self.sqls);
        } else {
            assert_eq!(
                self.sqls.len(),
                1,
                "case {}: ExecuteStatement carries exactly one statement, got {:?}",
                self.label,
                self.sqls
            );
            request["Sql"] = json!(self.sqls[0]);
        }
        if self.session {
            request["SessionKeepAliveSeconds"] = json!(300);
        }
        request
    }
}

/// A statement the engine refuses, so its submission reaches a terminal status
/// as fast as a submission can: the answer to the submit is still the accepted
/// submission and never that outcome.
const REFUSED_SQL: &str = "SELECT * FROM a_table_that_was_never_created";

/// Statements of a batch. Each one is a read, so the row can be submitted
/// repeatedly against one database and the batch's verdict stays the engine's
/// own rather than depending on what an earlier submission of the same row
/// left behind.
const BATCH_SQLS: &[&str] = &["SELECT 1", "SELECT 2", "SELECT 3"];

/// The table every case below is drawn from.
fn rows() -> Vec<SubmitRow> {
    vec![
        SubmitRow {
            label: "one statement the engine finishes at once",
            sqls: &[QUICK_SQL],
            batch: false,
            session: false,
        },
        SubmitRow {
            label: "one statement that takes real time",
            sqls: &[SLOW_SQL],
            batch: false,
            session: false,
        },
        SubmitRow {
            label: "one statement the engine refuses",
            sqls: &[REFUSED_SQL],
            batch: false,
            session: false,
        },
        SubmitRow {
            label: "one statement, submitted with a session of its own",
            sqls: &[QUICK_SQL],
            batch: false,
            session: true,
        },
        SubmitRow {
            label: "a batch of one statement",
            sqls: &[QUICK_SQL],
            batch: true,
            session: false,
        },
        SubmitRow {
            label: "a batch of three statements",
            sqls: BATCH_SQLS,
            batch: true,
            session: false,
        },
    ]
}

/// A submit that asks for no wait is answered with the submission as accepted:
/// SUBMITTED, whatever the statements it carries do and whichever operation
/// submitted them — and the execution it handed over still reaches a terminal
/// status of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_that_asks_for_no_wait_answers_with_the_accepted_submission() {
    let emulator = Emulator::new();
    let mut submitted: Vec<(&'static str, String)> = Vec::new();

    for (index, row) in rows().iter().enumerate() {
        // Each row runs on a database of its own, so the rows' executions do not
        // queue behind each other.
        let request = row.request(&format!("dev{index}"));
        for attempt in 0..SUBMISSIONS_PER_ROW {
            let response = emulator.ok(row.operation(), &request).await;
            assert_eq!(
                response["Status"], "SUBMITTED",
                "case {} ({}, submission {attempt}): a submit that asks for no wait must answer \
                 with the submission as accepted, got {response}",
                row.label,
                row.operation()
            );
            assert!(
                response["Id"].is_string(),
                "case {} (submission {attempt}): an accepted submission is answered with its Id, \
                 got {response}",
                row.label
            );
            assert!(
                response["HasResultSet"].is_null(),
                "case {} (submission {attempt}): whether a submission that has not run has a \
                 result set is not known yet, got {response}",
                row.label
            );
            assert!(
                response["CreatedAt"].is_number(),
                "case {} (submission {attempt}): an accepted submission reports when it was \
                 created, got {response}",
                row.label
            );
            submitted.push((row.label, id_of(&response)));
        }
    }

    // The execution each of those submissions handed over is the SUBMISSION's,
    // so every one of them reaches a terminal status on its own — which is what
    // the answer above deliberately does not report.
    for (label, id) in &submitted {
        let described = wait_for_terminal(&emulator, label, id).await;
        let status = described["Status"].as_str().unwrap_or_default();
        assert!(
            TERMINAL_STATUSES.contains(&status),
            "case {label}: a submission nobody waited for must still reach a terminal status, \
             got {described}"
        );
        assert!(
            described["Duration"].as_i64().unwrap_or_default() > 0,
            "case {label}: a submission that reached {status} reports how long its execution \
             took, got {described}"
        );
    }
}

/// And the other half of the rule: a submit that names a wait covering the
/// service's own execution bound is answered with the status the submission
/// reached, for the same rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_that_waits_is_answered_with_the_status_it_reached() {
    let emulator = Emulator::new();

    for (index, row) in rows().iter().enumerate() {
        let mut request = row.request(&format!("dev{index}"));
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let response = emulator.ok(row.operation(), &request).await;
        let answered = response["Status"].as_str().unwrap_or_default();
        assert!(
            TERMINAL_STATUSES.contains(&answered),
            "case {} ({}): a submit whose wait covers the execution must be answered with the \
             status it reached, got {response}",
            row.label,
            row.operation()
        );
        // The same record, read back: the submit's answer is the statement's
        // state and not a status assembled for that one response.
        let described = emulator
            .ok("DescribeStatement", &json!({"Id": id_of(&response)}))
            .await;
        assert_eq!(
            described["Status"], response["Status"],
            "case {}: the status the submit answered with must be the one the record reports, \
             got {described} after {response}",
            row.label
        );
    }
}
