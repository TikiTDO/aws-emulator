// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a job run COMING TO REST: that the state a run ends in is the one
//! its own script's execution came to, whichever call performed that execution and however
//! many reads it takes to observe it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the scripts
//! are staged through Amazon S3's own `PutObject`, exactly as in
//! [`crate::job_script_execution`].
//!
//! The rule under test: a run's script is executed by whichever call finds that execution
//! owed — the `StartJobRun` that began the run, or a later read of the job's runs — and the
//! terminal `JobRunState` that execution decides falls due one owed span after it answered.
//! So a client observes the terminal state by READING THE RUN AGAIN until it is there, and
//! never by assuming one call did all of it: the first read of a run whose script a busy
//! process had not started yet reports it still in flight, which is a correct answer and not
//! a finished run.
//!
//! That is why this suite, and the script suite beside it, poll for the CONDITION
//! ([`crate::job_runs::run_at_rest`] and [`crate::job_runs::runs_at_rest`]) with the owed
//! span made due through the store on every turn rather than slept out — the one thing that
//! keeps what these tests observe independent of how fast or how loaded the host is.
//!
//! The rows differ along WHICH call must perform the execution the run comes to rest on (the
//! start, for a first attempt; a later read, for the retry a failed attempt bought), along
//! the outcome the script came to (exited cleanly, raised, was never readable) and along how
//! many attempts the chain therefore holds.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    FIRST_JOB_RUN_ATTEMPT, JOB_RUN_SCRIPT_UNREADABLE_LEAD, JOB_RUN_STATE_FAILED,
    JOB_RUN_STATE_SUCCEEDED,
};

use crate::job_runs::{at_rest, run_at_rest, runs_at_rest};
use crate::job_script_execution::{
    create_bucket, emulator_with_s3, object_body, stage_script, BUCKET, ROLE_ARN, SCRIPTS,
};
use crate::wire::ok;

/// A script that exits cleanly, having computed something and written nothing.
const EXITS_CLEANLY: &str = r#"
rows = [row for row in range(8) if row % 2 == 0]
assert sum(rows) == 12, rows
"#;

/// A script that writes the object its arguments name, so the run's own effect is readable.
const WRITES_ITS_TOKEN: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
boto3.client("s3").put_object(
    Bucket=named["--bucket"],
    Key=named["--key"],
    Body=named["--token"].encode("utf-8"),
)
"#;

/// A script that raises, so the attempt fails and the job's retry budget buys another — an
/// execution only a LATER call can perform, because the attempt that bought it had to come
/// to rest first.
const RAISES: &str = r#"
raise RuntimeError("the partition could not be read")
"#;

/// One row: a job whose script does something, the retry budget it carries, and what the
/// chain of attempts must come to rest as.
#[derive(Debug)]
struct Row {
    /// What makes the row recognisable in an assertion.
    case: &'static str,
    /// The job (and script object) name.
    name: &'static str,
    /// The script to stage, or `None` for a job whose `ScriptLocation` names an object the
    /// store does not serve.
    script: Option<&'static str>,
    /// `Job$MaxRetries`: how many further attempts a failed one buys.
    max_retries: i64,
    /// The `JobRunState` every attempt of the chain must come to rest in.
    state: &'static str,
    /// How many attempts the chain must hold once it is at rest.
    attempts: usize,
    /// Whether the token object the script writes must be present afterwards.
    writes_token: bool,
}

/// The key the row's script writes its token under.
fn token_key(row: &Row) -> String {
    format!("rest/{}.txt", row.name)
}

/// The token the row's run passes, which is also the body the object must carry.
fn token(row: &Row) -> String {
    format!("token-for-{}", row.name)
}

/// A run comes to rest in the state its own execution came to, whichever call performs that
/// execution, and a chain of attempts comes to rest with every attempt finished.
#[tokio::test]
async fn a_run_comes_to_rest_whichever_call_performs_its_script() {
    let rows: [Row; 4] = [
        Row {
            case: "a clean script, whose execution the start performs",
            name: "rest-clean",
            script: Some(EXITS_CLEANLY),
            max_retries: 0,
            state: JOB_RUN_STATE_SUCCEEDED,
            attempts: 1,
            writes_token: false,
        },
        Row {
            case: "a script that writes its object, so the run's own effect is readable",
            name: "rest-writes",
            script: Some(WRITES_ITS_TOKEN),
            max_retries: 0,
            state: JOB_RUN_STATE_SUCCEEDED,
            attempts: 1,
            writes_token: true,
        },
        Row {
            // The second attempt's script can only be executed by a call that found the
            // first one finished, so this row comes to rest over several reads.
            case: "a failing script with a retry, whose further attempt a later read performs",
            name: "rest-retried",
            script: Some(RAISES),
            max_retries: 1,
            state: JOB_RUN_STATE_FAILED,
            attempts: 2,
            writes_token: false,
        },
        Row {
            case: "a job pointing at a script the object store does not serve",
            name: "rest-missing",
            script: None,
            max_retries: 0,
            state: JOB_RUN_STATE_FAILED,
            attempts: 1,
            writes_token: false,
        },
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows {
        let key = format!("{SCRIPTS}{}.py", row.name);
        let location = match row.script {
            Some(source) => stage_script(&router, &key, source).await,
            // Nothing is staged, so the location names an object the store does not serve.
            None => format!("s3://{BUCKET}/{key}"),
        };
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": row.name,
                "Role": ROLE_ARN,
                "Command": {
                    "Name": "pythonshell",
                    "PythonVersion": "3.9",
                    "ScriptLocation": location,
                },
                "MaxCapacity": 0.0625,
                "Timeout": 5,
                "MaxRetries": row.max_retries,
                "ExecutionProperty": { "MaxConcurrentRuns": 10 },
            }),
        )
        .await;

        let started = ok(
            &router,
            "StartJobRun",
            &json!({
                "JobName": row.name,
                "Arguments": {
                    "--bucket": BUCKET,
                    "--key": token_key(row),
                    "--token": token(row),
                },
            }),
        )
        .await;
        let run_id = started
            .get("JobRunId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartJobRun must answer a JobRunId, got {started}",
                    row.case
                )
            })
            .to_string();

        // The run the start minted comes to rest in what its script came to, read back
        // until it is there rather than depended on having finished by the first read.
        let first = run_at_rest(&router, &engine, row.case, row.name, &run_id).await;
        assert_eq!(
            first.get("JobRunState").and_then(Value::as_str),
            Some(row.state),
            "case {:?}: the first attempt must come to rest in {}, got {first}",
            row.case,
            row.state
        );
        assert_eq!(
            first.get("Attempt").and_then(Value::as_i64),
            Some(i64::from(FIRST_JOB_RUN_ATTEMPT)),
            "case {:?}: the run the start minted is the first attempt, got {first}",
            row.case
        );
        assert!(
            first.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {:?}: a run that has come to rest reports when it finished, got {first}",
            row.case
        );
        // A run that failed carries the reason its execution answered with, and one that
        // failed at nothing carries none at all.
        let reason = first.get("ErrorMessage").and_then(Value::as_str);
        if row.state == JOB_RUN_STATE_SUCCEEDED {
            assert!(
                reason.is_none(),
                "case {:?}: a run that failed at nothing carries no ErrorMessage, got {first}",
                row.case
            );
        } else {
            assert!(
                reason.is_some_and(|message| !message.is_empty()),
                "case {:?}: a failed run carries the reason its execution answered with, got \
                 {first}",
                row.case
            );
        }
        if row.script.is_none() {
            assert!(
                reason.is_some_and(|message| message.starts_with(JOB_RUN_SCRIPT_UNREADABLE_LEAD)),
                "case {:?}: a run whose script could not be read must say so, got {first}",
                row.case
            );
        }

        // And the whole chain comes to rest: every attempt the budget bought is finished,
        // each of them in the state its own execution came to.
        let chain = runs_at_rest(&router, &engine, row.case, row.name).await;
        assert_eq!(
            chain.len(),
            row.attempts,
            "case {:?}: expected {} attempt(s) at rest, got {chain:?}",
            row.case,
            row.attempts
        );
        for attempt in &chain {
            assert!(
                at_rest(attempt),
                "case {:?}: every attempt of a chain at rest has finished, got {attempt}",
                row.case
            );
            assert_eq!(
                attempt.get("JobRunState").and_then(Value::as_str),
                Some(row.state),
                "case {:?}: every attempt must come to rest in {}, got {attempt}",
                row.case,
                row.state
            );
        }

        // The script's own effect is there exactly when the script that makes it ran.
        let written = object_body(&router, &token_key(row)).await;
        if row.writes_token {
            assert_eq!(
                written.as_deref(),
                Some(token(row).as_str()),
                "case {:?}: the object the script wrote must carry the token it was passed, \
                 got {written:?}",
                row.case
            );
        } else {
            assert!(
                written.is_none(),
                "case {:?}: no object may appear for a script that writes none, got {written:?}",
                row.case
            );
        }
    }
}
