// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the error names a `Retry` and a `Catch` may NOT take over,
//! driven over this service's own wire protocol.
//!
//! The rule under test: a `States.Runtime` failure is TERMINAL. It matches no `Retry`
//! retrier and no `Catch` catcher — not one naming `States.ALL`, and not one naming
//! `States.Runtime` itself — so the state that raised it runs exactly ONCE, no
//! catcher's target state is ever entered, and the execution reports `FAILED` with
//! error `States.Runtime` and a cause naming what could not be run. Every OTHER error
//! name is retried and caught exactly as the state declares.
//!
//! The table spans the constructs that raise `States.Runtime` — an intrinsic
//! invocation that cannot be evaluated, an intrinsic name the language does not define,
//! a transition to a state the definition does not declare, a path the service does
//! not follow, an item source that is not an array — against the controls in the same
//! shape, whose errors ARE retriable and catchable: a `Fail` state's own error name
//! and a `ResultPath` that conflicts with the document it names a place in.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`) is the one
//! `state_machine_lifecycle` already builds and the history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so both are
//! reached through `crate::`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// How many attempts the retrier of every row below declares, past the first.
const MAX_ATTEMPTS: u64 = 2;

/// How long the retrier of every row below waits between attempts.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Long enough that the outcome of a row that spent every retry its retrier declares
/// has become due: the waits the attempts added, plus the settle delay.
fn past_the_retries() -> Duration {
    PAST_THE_EXECUTION_DELAY.saturating_add(RETRY_INTERVAL.saturating_mul(
        u32::try_from(MAX_ATTEMPTS).expect("the attempt count fits"),
    ))
}

/// One row: the state the branch fails at, and what the execution must report.
struct FailureCase {
    case: &'static str,
    /// The state the branch's `Tally` transitions to, which is what fails.
    failing: Value,
    /// The execution input.
    input: &'static str,
    /// The error name the execution must report, or `None` when the failure is
    /// retried and caught and the execution recovers.
    terminal_error: Option<&'static str>,
    /// Text the `cause` must carry, for a terminal row.
    cause_names: &'static str,
    /// The error name a caught row delivers into `$.err`.
    caught_error: &'static str,
}

fn cases() -> Vec<FailureCase> {
    vec![
        FailureCase {
            case: "an intrinsic invocation that cannot be evaluated",
            failing: json!({
                "Type": "Pass",
                "Parameters": { "v.$": "States.MathAdd($.num, 1)" },
                "Next": "Boom",
            }),
            input: r#"{"num":"one"}"#,
            terminal_error: Some("States.Runtime"),
            cause_names: "States.MathAdd",
            caught_error: "",
        },
        FailureCase {
            case: "an intrinsic name the language does not define",
            failing: json!({
                "Type": "Pass",
                "Parameters": { "v.$": "States.MathMultiply($.num, 2)" },
                "Next": "Boom",
            }),
            input: r#"{"num":3}"#,
            terminal_error: Some("States.Runtime"),
            cause_names: "States.MathMultiply",
            caught_error: "",
        },
        FailureCase {
            case: "a path longer than this service evaluates",
            failing: json!({
                "Type": "Pass",
                "InputPath": format!("$.{}", "p".repeat(2_000)),
                "Next": "Boom",
            }),
            input: r#"{"num":1}"#,
            terminal_error: Some("States.Runtime"),
            cause_names: "longer than",
            caught_error: "",
        },
        FailureCase {
            case: "a path this service does not follow",
            failing: json!({ "Type": "Pass", "InputPath": "$..a..b..c", "Next": "Boom" }),
            input: r#"{"a":{"b":{"c":1}}}"#,
            terminal_error: Some("States.Runtime"),
            cause_names: "$..a..b..c",
            caught_error: "",
        },
        FailureCase {
            case: "a Map state whose item source is not an array",
            failing: json!({
                "Type": "Map",
                "ItemsPath": "$.num",
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "Next": "Boom",
            }),
            input: r#"{"num":1}"#,
            terminal_error: Some("States.Runtime"),
            cause_names: "must point to array",
            caught_error: "",
        },
        FailureCase {
            case: "a Fail state's own error name, which IS retried and caught",
            failing: json!({ "Type": "Pass", "Next": "Boom" }),
            input: r#"{"num":1}"#,
            terminal_error: None,
            cause_names: "",
            caught_error: "Tally.Boom",
        },
        FailureCase {
            case: "a ResultPath that conflicts with the document, which IS retried and caught",
            failing: json!({
                "Type": "Pass",
                "Result": 1,
                "ResultPath": "$.num.deeper",
                "Next": "Boom",
            }),
            input: r#"{"num":1}"#,
            terminal_error: None,
            cause_names: "",
            caught_error: "States.ReferencePathConflict",
        },
    ]
}

/// The definition one row runs: a `Parallel` state whose single branch records that
/// it ran (`Tally`), then fails at the row's state, under a retrier that names
/// `States.ALL` and two catchers — one naming `States.Runtime` itself and one naming
/// `States.ALL`.
fn definition(row: &FailureCase) -> String {
    let branch_states = json!({
        "Tally": { "Type": "Pass", "Next": "Failing" },
        "Failing": row.failing,
        "Boom": { "Type": "Fail", "Error": "Tally.Boom", "Cause": "the branch said so" },
    });
    json!({
        "StartAt": "Attempt",
        "States": {
            "Attempt": {
                "Type": "Parallel",
                "Branches": [{ "StartAt": "Tally", "States": branch_states }],
                "Retry": [{
                    "ErrorEquals": ["States.ALL"],
                    "IntervalSeconds": RETRY_INTERVAL.as_secs(),
                    "MaxAttempts": MAX_ATTEMPTS,
                    "BackoffRate": 1.0,
                }],
                "Catch": [
                    {
                        "ErrorEquals": ["States.Runtime"],
                        "ResultPath": "$.err",
                        "Next": "CaughtRuntime",
                    },
                    {
                        "ErrorEquals": ["States.ALL"],
                        "ResultPath": "$.err",
                        "Next": "CaughtAll",
                    },
                ],
                "End": true,
            },
            "CaughtRuntime": { "Type": "Pass", "End": true },
            "CaughtAll": { "Type": "Pass", "End": true },
        },
    })
    .to_string()
}

/// How many times the state `name` was ENTERED, over every state type's own entered
/// event.
fn entries(events: &[Value], name: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type.ends_with("StateEntered"))
                && event
                    .get("stateEnteredEventDetails")
                    .and_then(|details| details.get("name"))
                    .and_then(Value::as_str)
                    == Some(name)
        })
        .count()
}

/// A `States.Runtime` failure bypasses every retrier and every catcher and fails the
/// execution; the controls in the same shape are retried and caught.
#[tokio::test]
async fn a_states_runtime_failure_is_neither_retried_nor_caught() {
    let emulator = Emulator::new();
    let rows = cases();

    // Every row is started first and read afterwards, so one wait covers the outcomes
    // the clock owed all of them — the retried ones included.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("runtime-{index}");
        let machine = create_machine(&emulator, &name, &definition(row)).await;
        started.push(start_execution(&emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(past_the_retries());

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        let events = history(&emulator, &execution, json!({})).await;
        let entered = entries(&events, "Tally");

        match row.terminal_error {
            Some(error) => {
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                        described.get("output"),
                    ),
                    (Some("FAILED"), Some(error), None),
                    "case {case:?}: expected the execution to fail with {error}, got \
                     {described:?}"
                );
                let cause = string(&described, "cause");
                assert!(
                    cause.contains(row.cause_names),
                    "case {case:?}: expected the cause to name {:?}, got {cause:?}",
                    row.cause_names
                );
                // Non-retriability is what makes the branch's work happen once: a
                // retried branch would have entered its first state again.
                assert_eq!(
                    entered, 1,
                    "case {case:?}: expected the branch to run exactly once, got \
                     {entered} entries of its first state in {events:?}"
                );
                // Non-catchability is what keeps the catchers' targets out of the run.
                for target in ["CaughtRuntime", "CaughtAll"] {
                    assert_eq!(
                        entries(&events, target),
                        0,
                        "case {case:?}: expected {target} never to be entered, got \
                         {events:?}"
                    );
                }
            }
            None => {
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error"),
                    ),
                    (Some("SUCCEEDED"), None),
                    "case {case:?}: expected the catcher to recover the execution, \
                     got {described:?}"
                );
                let output: Value = serde_json::from_str(&string(&described, "output"))
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: expected JSON output, got {error}")
                    });
                assert_eq!(
                    output
                        .get("err")
                        .and_then(|err| err.get("Error"))
                        .and_then(Value::as_str),
                    Some(row.caught_error),
                    "case {case:?}: expected the catcher to deliver the error, got \
                     {output:?}"
                );
                // The retrier really re-ran the branch: one attempt, plus every one
                // its `MaxAttempts` declares.
                let expected = usize::try_from(MAX_ATTEMPTS).expect("the count fits") + 1;
                assert_eq!(
                    entered, expected,
                    "case {case:?}: expected the branch to run {expected} times, got \
                     {entered} in {events:?}"
                );
                assert_eq!(
                    (
                        entries(&events, "CaughtRuntime"),
                        entries(&events, "CaughtAll"),
                    ),
                    (0, 1),
                    "case {case:?}: expected the States.ALL catcher to take it, got \
                     {events:?}"
                );
            }
        }
    }
}
