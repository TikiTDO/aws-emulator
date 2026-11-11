// SPDX-License-Identifier: CC0-1.0

//! End-to-end and seam tests for HOW LONG the call that performs a job run's script waits
//! for it, and therefore for what a run whose script really finished reports.
//!
//! The rule under test, in one sentence: a run's terminal state is what its own execution
//! came to, so the wait that decides a script was "terminated for taking too long" has to
//! cover every span the ENGINE may take — the execution environment it builds before a line
//! of the script is read as much as the script's own wall clock — because the environment is
//! the host's work and no script controls it.
//!
//! What goes wrong when it does not: a wait of the script's own ceiling and the stop grace
//! alone runs out while the interpreter is still being stood up on a busy host, and the run
//! is then reported in the state `Job$Timeout` documents for a script that ran past nothing.
//! That is an outcome no execution produced, and it is the same reading the engine would have
//! answered the real one with.
//!
//! The first test pins the wait against the engine's own span over the `Timeout` values a job
//! may carry; the second drives the rule end to end, where a script whose MODULES are
//! imported before it runs — work the engine does in the phase it bounds on its own span —
//! still reports what the script itself came to.

use serde_json::Value;
use service_com_amazonaws_glue_awsglue::state::{
    job_script_budget, job_script_execution_wait, JOB_RUN_STATE_SUCCEEDED,
    JOB_SCRIPT_ABANDON_GRACE, MAX_TIMEOUT_MINUTES,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::INIT_PHASE_LIMIT;
use serde_json::json;

use crate::job_script_execution::{
    create_bucket, emulator_with_s3, run_to_rest, stage_script, ROLE_ARN, SCRIPTS,
};
use crate::wire::ok;

/// The `Timeout` values the wait is pinned over: the shortest the model admits
/// (`Timeout`'s `@range(min: 1)`), one in between, the longest it documents
/// ([`MAX_TIMEOUT_MINUTES`], the seven days `Job$Timeout` admits), and one outside that range
/// altogether, which bounds nothing and so leaves the emulator's own ceiling in charge.
const TIMEOUT_ROWS: &[(&str, i32)] = &[
    ("the shortest Timeout the model admits", 1),
    ("an hour", 60),
    ("the longest Timeout the model documents", MAX_TIMEOUT_MINUTES),
    ("a Timeout outside the range the model allows", 0),
];

#[test]
fn the_wait_for_one_execution_covers_the_whole_span_the_engine_may_take() {
    for (case, minutes) in TIMEOUT_ROWS {
        let waited = job_script_execution_wait(*minutes);
        // The span the ENGINE may take for the same budget: the phase it builds the execution
        // environment in, which it bounds on its own span, and then the script's own wall
        // clock.
        let script = job_script_budget(*minutes).wall_clock;
        let engine_span = INIT_PHASE_LIMIT.saturating_add(script);
        assert!(
            waited > engine_span,
            "case {case:?}: the wait must cover the engine's own span for the budget \
             ({engine_span:?} = {INIT_PHASE_LIMIT:?} standing the environment up plus \
             {script:?} of script) before a run may be reported as terminated, and it is \
             {waited:?}"
        );
        assert!(
            waited <= engine_span.saturating_add(JOB_SCRIPT_ABANDON_GRACE),
            "case {case:?}: the wait must stay inside the engine's span plus the grace a \
             script is given to stop ({JOB_SCRIPT_ABANDON_GRACE:?}), so a call is not held \
             open past it, and it is {waited:?}"
        );
    }
}

/// A script whose modules are imported before a line of it runs — the phase the engine
/// stands the environment up in — and which then exits cleanly, having computed something.
///
/// The imports are standard-library modules the engine serves from its own frozen library,
/// so what they cost is the HOST's work of building the environment rather than the script's.
const IMPORTS_THEN_EXITS_CLEANLY: &str = r#"
import base64
import datetime
import decimal
import hashlib
import json
import re
import uuid

digest = hashlib.sha256(b"partition").hexdigest()
encoded = base64.b64encode(digest.encode("utf-8")).decode("ascii")
parsed = json.loads(json.dumps({"rows": [decimal.Decimal(1).as_tuple().exponent]}))
matched = re.findall(r"[0-9a-f]+", digest)
assert parsed["rows"] == [0], parsed
assert matched, digest
assert isinstance(uuid.uuid4().hex, str)
assert datetime.datetime(2026, 1, 1).year == 2026
"#;

#[tokio::test]
async fn a_run_whose_script_finished_is_not_reported_as_terminated_for_taking_too_long() {
    let case = "a script whose modules are imported before it runs";
    let name = "script-imports-then-exits";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}{name}.py"),
        IMPORTS_THEN_EXITS_CLEANLY,
    )
    .await;
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": {
                "Name": "pythonshell",
                "PythonVersion": "3.9",
                "ScriptLocation": location,
            },
            "MaxCapacity": 0.0625,
            // The shortest Timeout the model admits, so the span the script itself is given
            // is this emulator's own ceiling and nothing longer.
            "Timeout": 1,
        }),
    )
    .await;

    let run = run_to_rest(&router, &engine, name, &json!({})).await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: the script exited cleanly, so the run must come to rest in \
         {JOB_RUN_STATE_SUCCEEDED} rather than in a state no execution produced, got {run}"
    );
    assert!(
        run.get("ErrorMessage").is_none(),
        "case {case:?}: a run whose script failed at nothing carries no ErrorMessage, got {run}"
    );
    // And it really ran: a run that came to rest reports when it completed.
    assert!(
        run.get("CompletedOn").is_some(),
        "case {case:?}: a run that came to rest reports its CompletedOn, got {run}"
    );
}
