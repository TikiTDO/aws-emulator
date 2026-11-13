// SPDX-License-Identifier: CC0-1.0

//! What a Submit/Wait/CheckStatus/`Choice` POLLER reports when the field its router
//! branches on is not in the payload the polled task answered, driven over both
//! services' own wire protocols through the production `MultiServiceRouter` with Step
//! Functions and Lambda mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so the function really runs and the `Choice` state reads the
//! payload that run produced.
//!
//! RULE: a reference path a `Choice` rule's condition reads and that selects no value
//! in the state's input fails the execution with `States.Runtime`, and the cause names
//! the path AS THE DEFINITION WROTE IT beside the service's choice-condition sentence,
//! under the state/entering-event prefix every diagnosis a state raises is reported
//! under — so an operator reading the failure learns WHICH field the router needed and
//! WHERE the run stopped, whichever operand of whichever rule named it. The Amazon
//! States Language is not in the Smithy model, so the wording is the recorded one;
//! `DescribeExecution` models the member it arrives in as `SensitiveCause`.
//!
//! The rows differ along what the rule generalises over the POLLER's own dimensions —
//! the operand and path forms a `Choice` reads over a STATIC input are tabled by
//! `choice_and_wait_evaluation`, so they are not re-tabled here: which poll of the loop
//! stopped reporting the field, so the number of times the `Choice` is entered, and
//! with it the event id the cause names, differ from row to row; which integration form
//! the polled task was written in (a bare function ARN, the optimized
//! `arn:aws:states:::lambda:invoke`), and with it whether the path reads the payload
//! itself or the member of the invocation envelope the payload arrives in; and which
//! operand of the rule names the path the payload lacks (the `Variable`, a `*Path`
//! operand beside a `Variable` the payload does resolve).
//!
//! The event id in the cause is not a figure written into the table: each row reads the
//! entering event of its own `Choice` state out of `GetExecutionHistory` and requires
//! the cause to name THAT id, so the two readings of one execution cannot drift.
//!
//! The harness (`Chain`, which mounts the two services over one engine) is the one
//! `lambda_task_control_flow` builds, the choice-condition sentence is the one
//! `choice_and_wait_evaluation` states and `string` is `state_machine_lifecycle`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use crate::choice_and_wait_evaluation::CHOICE_CONDITION_DIAGNOSIS;
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::{string, ROLE_ARN};

/// The error a path a state could not resolve fails the execution with, written out as
/// the recorded wording rather than read from the service, so the check does not restate
/// the constant the implementation carries.
const RUNTIME_ERROR: &str = "States.Runtime";

/// The name the poller's `Choice` state carries, which the cause has to name.
const ROUTER: &str = "JobComplete";

/// The state the `Choice` routes to once a rule matches, which every row's definition
/// declares because a rule names it.
const DONE: &str = "JobDone";

/// The event a `Choice` state is entered by.
const CHOICE_ENTERED: &str = "ChoiceStateEntered";

/// The job poller's function: one report of a job whose payload carries the status
/// members only while the job is still being REPORTED on.
///
/// `reports` is how many of the run's polls still answer with those members, and
/// `polls` is the count the state machine feeds back round the loop, so which poll
/// stops reporting them is decided by the data the execution was started with rather
/// than by anything the emulator holds. A poll past that count answers the job's id and
/// the counters alone — the shape an upstream task that stopped returning a field
/// leaves its caller with.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    polls = int(event.get("polls", 0)) + 1
    reports = int(event.get("reports", 0))
    answer = {"id": event.get("id", "job"), "polls": polls, "reports": reports}
    if polls <= reports:
        answer["status"] = "RUNNING"
        answer["job"] = {"state": "RUNNING"}
    return answer
"#;

/// Which form the polled task's `Resource` is written in.
#[derive(Clone, Copy, Debug)]
enum Integration {
    /// The function's own ARN, whose unit of work is one invocation of it and whose
    /// result is the payload the function answered.
    FunctionArn,
    /// The optimized integration, which names the function in its `Parameters` and
    /// whose result is the invocation's own envelope, with the payload under `Payload`.
    Optimized,
}

/// One row: the `Choice` rules the poller routes on, how many polls still report the
/// status members, and the path the cause must name.
struct Row {
    case: &'static str,
    /// The `Choices` of the poller's router state.
    rules: Value,
    /// How many of the run's polls answer with the status members.
    reports: u64,
    /// How often the router state is entered before the run fails, which is the poll
    /// that stopped reporting the field.
    entries: usize,
    /// The path the cause must name, as the definition wrote it.
    named: &'static str,
    /// A path the same state also reads and that resolves, which the cause must NOT
    /// name — `None` where the state reads only the failing one.
    not_named: Option<&'static str>,
    /// Which form the polled task is written in.
    integration: Integration,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the member the first poll of the loop stopped reporting",
            rules: json!([
                { "Variable": "$.status", "StringEquals": "SUCCEEDED", "Next": DONE },
                { "Variable": "$.status", "StringEquals": "FAILED", "Next": DONE },
            ]),
            reports: 1,
            entries: 1,
            named: "$.status",
            not_named: None,
            integration: Integration::FunctionArn,
        },
        Row {
            case: "the member a LATER poll of the same loop stopped reporting",
            rules: json!([
                { "Variable": "$.status", "StringEquals": "SUCCEEDED", "Next": DONE },
            ]),
            reports: 3,
            entries: 3,
            named: "$.status",
            not_named: None,
            integration: Integration::FunctionArn,
        },
        Row {
            case: "a *Path operand beside a Variable that resolves",
            rules: json!([
                { "Variable": "$.polls", "NumericEqualsPath": "$.target", "Next": DONE },
            ]),
            reports: 1,
            entries: 1,
            named: "$.target",
            not_named: Some("$.polls"),
            integration: Integration::FunctionArn,
        },
        Row {
            case: "a member of the optimized integration's own result envelope",
            rules: json!([
                { "Variable": "$.Payload.status", "StringEquals": "SUCCEEDED", "Next": DONE },
            ]),
            reports: 1,
            entries: 1,
            named: "$.Payload.status",
            not_named: None,
            integration: Integration::Optimized,
        },
        Row {
            case: "the envelope member a LATER poll of the optimized task stopped reporting",
            rules: json!([
                { "Variable": "$.Payload.status", "StringEquals": "SUCCEEDED", "Next": DONE },
            ]),
            reports: 2,
            entries: 2,
            named: "$.Payload.status",
            not_named: None,
            integration: Integration::Optimized,
        },
    ]
}

/// The state an optimized poller's router falls back to: the envelope the invocation
/// answered, narrowed to the payload itself, so the next poll is handed the job report
/// rather than the envelope around it — the unwrapping a poller written in that form
/// carries.
const UNWRAP: &str = "UnwrapPayload";

/// The poller one row runs: a submit task, the wait between polls, the polled task, and
/// the router state that branches on what the poll answered — with the router's
/// `Default` re-entering the wait, which is what makes the loop a loop.
fn definition(row: &Row, function_arn: &str) -> Value {
    let mut states = json!({
        "SubmitJob": { "Type": "Task", "Resource": function_arn, "Next": "WaitForJob" },
        "WaitForJob": { "Type": "Wait", "Seconds": 1, "Next": "CheckStatus" },
        DONE: { "Type": "Succeed" },
    });
    let members = states.as_object_mut().expect("the states are an object");
    let (polled, fallback) = match row.integration {
        Integration::FunctionArn => (
            json!({ "Type": "Task", "Resource": function_arn, "Next": ROUTER }),
            "WaitForJob",
        ),
        Integration::Optimized => {
            members.insert(
                UNWRAP.to_string(),
                json!({ "Type": "Pass", "InputPath": "$.Payload", "Next": "WaitForJob" }),
            );
            (
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::lambda:invoke",
                    "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
                    "Next": ROUTER,
                }),
                UNWRAP,
            )
        }
    };
    members.insert("CheckStatus".to_string(), polled);
    members.insert(
        ROUTER.to_string(),
        json!({
            "Type": "Choice",
            "Choices": row.rules.clone(),
            "Default": fallback,
        }),
    );
    json!({ "StartAt": "SubmitJob", "States": states })
}

/// The cause the service words for the path `path` that a `Choice` rule in state `name`,
/// entered at event `entered`, could not resolve.
fn expected_cause(name: &str, entered: i64, path: &str) -> String {
    format!(
        "An error occurred while executing the state '{name}' (entered at the event id \
         #{entered}). Invalid path '{path}': {CHOICE_CONDITION_DIAGNOSIS}"
    )
}

/// The ids of every event of `event_type` naming the state `name`, in history order.
fn entered_event_ids(events: &[Value], event_type: &str, name: &str) -> Vec<i64> {
    events
        .iter()
        .filter(|event| {
            event.get("type").and_then(Value::as_str) == Some(event_type)
                && event
                    .get("stateEnteredEventDetails")
                    .and_then(|details| details.get("name"))
                    .and_then(Value::as_str)
                    == Some(name)
        })
        .map(|event| {
            event
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("expected an id on {event:?}"))
        })
        .collect()
}

/// A `Choice` rule reading a path the polled task's payload does not carry fails the
/// execution with a cause naming that path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pollers_router_names_the_path_its_rule_could_not_resolve() {
    let chain = Chain::new();
    let function_arn = chain
        .given_function_of("job-reporter", HANDLER_SOURCE, None)
        .await;
    let rows = rows();

    // Every row is started first and settled afterwards, so the loops run alongside one
    // another rather than one after the next.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let created = chain
            .sfn(
                case,
                "CreateStateMachine",
                &json!({
                    "name": format!("poller-{index}"),
                    "definition": definition(row, &function_arn).to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let execution = chain
            .sfn(
                case,
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": format!("poll-{index}"),
                    "input": json!({ "id": format!("job-{index}"), "reports": row.reports })
                        .to_string(),
                }),
            )
            .await;
        started.push(string(&execution, "executionArn"));
    }

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = chain.settled(case, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described}"
        );

        let answered = chain
            .sfn(
                case,
                "GetExecutionHistory",
                &json!({ "executionArn": execution }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an events list in {answered}"))
            .clone();
        let entries = entered_event_ids(&events, CHOICE_ENTERED, ROUTER);
        // The field went missing at the row's own poll, so the loop reached the router
        // state exactly that many times before the run failed.
        assert_eq!(
            entries.len(),
            row.entries,
            "case {case:?}: expected the router state to be entered {} time(s), got {entries:?} \
             in {events:?}",
            row.entries
        );
        let entered = *entries
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected the router state to be entered"));

        let expected = expected_cause(ROUTER, entered, row.named);
        let cause = string(&described, "cause");
        assert_eq!(
            cause, expected,
            "case {case:?}: expected the cause {expected:?}, got {cause:?}"
        );
        if let Some(resolved) = row.not_named {
            assert!(
                !cause.contains(resolved),
                "case {case:?}: expected the cause to name only the path that failed, not \
                 {resolved:?}, got {cause:?}"
            );
        }
    }
}
