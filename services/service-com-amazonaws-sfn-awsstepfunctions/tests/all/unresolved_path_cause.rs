// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an execution reports when a reference path a state's
//! member names resolves to NOTHING, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rule under test: a path a state could not resolve fails the execution with
//! `States.Runtime`, and the cause names the STATE that was being executed, the id of
//! the history event that state was entered at, and then what could not be resolved —
//! "An error occurred while executing the state '<name>' (entered at the event id
//! #<id>). <diagnosis>" — so a definition carrying many states that read the same path
//! still says which one failed and where in the history it failed. The diagnosis is the
//! path AS THE DEFINITION WROTE IT followed by the path that was RESOLVED, rendered in
//! BRACKET notation ("Invalid path '<path>' : No results for path: $['a'][0]"), for
//! every member of a state; the same path followed by the service's own
//! choice-condition sentence, with NO space before the colon, for a path a `Choice`
//! rule's condition reads; the parameter sentence naming the MEMBER for one of a pair
//! of alternatives a `Wait` state reads its figure through; and the field the path was
//! written for beside the WHOLE document it was looked for in, for a member of a
//! payload template.
//!
//! The rows differ along what the rule generalises over: which MEMBER names the path (a
//! `Choice` rule's `Variable`, the same rule's `*Path` operand, `OutputPath`, a `Wait`
//! state's `SecondsPath` and `TimestampPath`, a `Fail` state's `CausePath`, a `Map`
//! state's `ItemsPath`, a `Parameters` member), the state's TYPE, the path's own form (a
//! member, a nested member, a bracketed member, an array index, a path into the context
//! object), the state's POSITION in the definition — which is what makes the event id
//! differ from row to row — and whether the failing state is a top-level state or one
//! inside a `Map`'s `ItemProcessor`.
//!
//! The event id in the cause is not a figure written into the table: each row reads the
//! entered event of its failing state out of `GetExecutionHistory` and requires the
//! cause to name THAT id, so the two readings of one execution cannot drift.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so both are
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// The name every row's failing state carries.
const FAILING: &str = "JobComplete";

/// Which diagnosis the service words for an unresolvable path, which depends on the
/// member the path was written in.
enum Diagnosis {
    /// The path as written and the path that was resolved, which every member of a
    /// state reports.
    Path,
    /// The path and the service's choice-condition sentence, for a path a `Choice`
    /// rule's condition reads.
    ChoiceCondition,
    /// The member named as the parameter it is, for a member the definition carries one
    /// of a pair of alternatives of (a `Wait` state's `SecondsPath`, say).
    Member(&'static str),
    /// The field the path was written for and the WHOLE document it was looked for in,
    /// for a `"<name>.$"` member of a payload template.
    Field {
        field: &'static str,
        /// The document the path was resolved against, as the execution carries it.
        searched: &'static str,
    },
}

/// One row: the state that fails, how many `Pass` states run before it, and the path
/// it could not resolve.
struct Row {
    case: &'static str,
    /// The failing state itself, which every row reaches as its last state.
    failing: Value,
    /// How many `Pass` states the execution walks before the failing one, so the event
    /// id the cause names differs from row to row.
    before: usize,
    /// The execution input, which carries none of the paths below.
    input: &'static str,
    /// The path the cause must name.
    path: &'static str,
    /// The type prefix of the event the failing state is entered by.
    entered_event: &'static str,
    /// Whether the failing state transitions to a `Done` state, which the definition
    /// then has to declare: a state no state reaches is not a definition this service
    /// accepts.
    transitions_to_done: bool,
    /// The diagnosis the service words for this row's member.
    diagnosis: Diagnosis,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Choice rule's Variable, with no Default to fall back to",
            failing: json!({
                "Type": "Choice",
                "Choices": [{ "Variable": "$.status", "StringEquals": "done", "Next": "Done" }],
            }),
            before: 0,
            input: r#"{"other":1}"#,
            path: "$.status",
            entered_event: "ChoiceStateEntered",
            transitions_to_done: true,
            diagnosis: Diagnosis::ChoiceCondition,
        },
        Row {
            case: "a Choice rule's *Path operand, a nested member the input stops short of",
            failing: json!({
                "Type": "Choice",
                "Choices": [{
                    "Variable": "$.count",
                    "NumericEqualsPath": "$.expected.count",
                    "Next": "Done",
                }],
                "Default": "Done",
            }),
            before: 1,
            input: r#"{"count":1,"expected":{}}"#,
            path: "$.expected.count",
            entered_event: "ChoiceStateEntered",
            transitions_to_done: true,
            diagnosis: Diagnosis::ChoiceCondition,
        },
        Row {
            case: "an OutputPath over what the state produced",
            failing: json!({ "Type": "Pass", "OutputPath": "$['chosen']", "End": true }),
            before: 2,
            input: r#"{"present":true}"#,
            path: "$['chosen']",
            entered_event: "PassStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Path,
        },
        Row {
            case: "a Wait state's SecondsPath",
            failing: json!({ "Type": "Wait", "SecondsPath": "$.delay", "End": true }),
            before: 0,
            input: r#"{"no-delay-here":true}"#,
            path: "$.delay",
            entered_event: "WaitStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Member("SecondsPath"),
        },
        Row {
            case: "a Wait state's TimestampPath, the other member of that pair",
            failing: json!({ "Type": "Wait", "TimestampPath": "$.due[0]", "End": true }),
            before: 2,
            input: r#"{"due":[]}"#,
            path: "$.due[0]",
            entered_event: "WaitStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Member("TimestampPath"),
        },
        Row {
            case: "an array index the state's output stops short of",
            failing: json!({ "Type": "Pass", "OutputPath": "$.items[2]", "End": true }),
            before: 1,
            input: r#"{"items":[1,2]}"#,
            path: "$.items[2]",
            entered_event: "PassStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Path,
        },
        Row {
            case: "a member of a Parameters payload template",
            failing: json!({
                "Type": "Pass",
                "Parameters": { "picked.$": "$.absent" },
                "End": true,
            }),
            before: 0,
            input: r#"{"present":true}"#,
            path: "$.absent",
            entered_event: "PassStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Field {
                field: "picked.$",
                searched: r#"{"present":true}"#,
            },
        },
        Row {
            case: "a Fail state's CausePath",
            failing: json!({ "Type": "Fail", "Error": "Boom", "CausePath": "$.why" }),
            before: 3,
            input: r#"{"other":1}"#,
            path: "$.why",
            entered_event: "FailStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Path,
        },
        Row {
            case: "a Map state's ItemsPath",
            failing: json!({
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "End": true,
            }),
            before: 1,
            input: r#"{"nothing":[]}"#,
            path: "$.items",
            entered_event: "MapStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Path,
        },
        Row {
            case: "a path into the context object the execution carries no value for",
            failing: json!({ "Type": "Pass", "OutputPath": "$$.Map.Item.Value", "End": true }),
            before: 0,
            input: "{}",
            path: "$$.Map.Item.Value",
            entered_event: "PassStateEntered",
            transitions_to_done: false,
            diagnosis: Diagnosis::Path,
        },
    ]
}

/// The definition one row runs: `before` `Pass` states, then the failing one, and the
/// `Done` state the failing one transitions to where it names one.
fn definition(row: &Row) -> String {
    let mut states = serde_json::Map::new();
    let mut start_at = FAILING.to_string();
    for index in 0..row.before {
        let name = format!("Step{index}");
        let next = if index + 1 == row.before {
            FAILING.to_string()
        } else {
            format!("Step{}", index + 1)
        };
        if index == 0 {
            start_at = name.clone();
        }
        states.insert(name, json!({ "Type": "Pass", "Next": next }));
    }
    states.insert(FAILING.to_string(), row.failing.clone());
    if row.transitions_to_done {
        states.insert("Done".to_string(), json!({ "Type": "Pass", "End": true }));
    }
    json!({ "StartAt": start_at, "States": Value::Object(states) }).to_string()
}

/// The cause the service words for a path that selected no value in state `name`,
/// entered at event `entered`.
///
/// The `diagnosis` selects what follows: a path a `Choice` rule's condition reads is
/// reported with the condition referencing a value that is not there and no space before
/// its colon, a member of a pair of alternatives is reported as that parameter not
/// referencing an input value, a payload-template member is reported with the field it
/// builds and the whole document the path was looked for in, and every other member of a
/// state is reported with the path that was RESOLVED, in bracket notation.
fn expected_cause(name: &str, entered: i64, path: &str, diagnosis: &Diagnosis) -> String {
    let prefix =
        format!("An error occurred while executing the state '{name}' (entered at the event id #{entered}). ");
    let diagnosis = match diagnosis {
        Diagnosis::Path => format!(
            "Invalid path '{path}' : No results for path: {}",
            bracketed(path)
        ),
        Diagnosis::ChoiceCondition => {
            format!("Invalid path '{path}': {CHOICE_CONDITION_DIAGNOSIS}")
        }
        Diagnosis::Member(member) => {
            format!("The {member} parameter does not reference an input value: {path}")
        }
        Diagnosis::Field { field, searched } => format!(
            "The JSONPath '{path}' specified for the field '{field}' could not be found in the \
             input '{searched}'"
        ),
    };
    format!("{prefix}{diagnosis}")
}

/// `path` in the BRACKET notation the service renders a RESOLVED path in: every member
/// name quoted, every index bare, and the first `$` of a path into the context object
/// dropped, since that `$` names the object the rest of the path was resolved against.
///
/// Written out here rather than read from the service, so the test states the notation
/// instead of restating the code that produces it.
fn bracketed(path: &str) -> String {
    let mut rest = path.strip_prefix('$').unwrap_or(path);
    rest = rest.strip_prefix('$').unwrap_or(rest);
    let mut rendered = String::from("$");
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('.') {
            let end = tail.find(['.', '[']).unwrap_or(tail.len());
            let (name, tail) = tail.split_at(end);
            rendered.push_str(&format!("['{name}']"));
            rest = tail;
            continue;
        }
        let tail = rest
            .strip_prefix('[')
            .unwrap_or_else(|| panic!("the row's path {path:?} is a reference path"));
        let end = tail
            .find(']')
            .unwrap_or_else(|| panic!("the row's path {path:?} closes every subscript"));
        let (inside, tail) = tail.split_at(end);
        let name = inside.trim_matches('\'').trim_matches('"');
        match name.parse::<usize>() {
            Ok(index) => rendered.push_str(&format!("[{index}]")),
            Err(_) => rendered.push_str(&format!("['{name}']")),
        }
        rest = &tail[1..];
    }
    rendered
}

/// What the service states about a path a `Choice` rule's condition could not resolve.
const CHOICE_CONDITION_DIAGNOSIS: &str =
    "The choice state's condition path references an invalid value.";

/// The id of the one event of `event_type` naming the state `name`.
fn entered_event_id(events: &[Value], case: &str, event_type: &str, name: &str) -> i64 {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event.get("type").and_then(Value::as_str) == Some(event_type)
                && event
                    .get("stateEnteredEventDetails")
                    .and_then(|details| details.get("name"))
                    .and_then(Value::as_str)
                    == Some(name)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {event_type} for {name}, got {events:?}"
    );
    matching[0]
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an id on {:?}", matching[0]))
}

/// A path that selected no value fails the execution with a cause naming the state and
/// the event id it was entered at.
#[tokio::test]
async fn an_unresolved_path_names_the_state_and_the_event_it_was_entered_at() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every row is started first and read afterwards, so one wait covers every
    // outcome the clock owed.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("path-cause-{index}"), &definition(row))
            .await;
        started.push(start_execution(&emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let events = history(&emulator, &execution, json!({})).await;
        let entered = entered_event_id(&events, case, row.entered_event, FAILING);
        // The id is the one the history really holds for this state, and the position
        // of the state in the definition is what makes it differ row by row.
        let expected_entered = 2 + 2 * i64::try_from(row.before).expect("the count fits");
        assert_eq!(
            entered, expected_entered,
            "case {case:?}: expected the state to be entered at event #{expected_entered}, got \
             #{entered} in {events:?}"
        );
        let expected = expected_cause(FAILING, entered, row.path, &row.diagnosis);
        let cause = string(&described, "cause");
        assert_eq!(
            cause, expected,
            "case {case:?}: expected the cause {expected:?}, got {cause:?}"
        );
    }
}

/// The state the cause names is the state that FAILED, including one inside a `Map`'s
/// `ItemProcessor`: the enclosing `Map` state does not take the diagnosis over.
#[tokio::test]
async fn a_nested_state_names_itself_rather_than_the_map_around_it() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "Fan",
        "States": {
            "Fan": {
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemProcessor": {
                    "StartAt": FAILING,
                    "States": {
                        FAILING: {
                            "Type": "Choice",
                            "Choices": [{
                                "Variable": "$.status",
                                "StringEquals": "done",
                                "Next": "Done",
                            }],
                        },
                        "Done": { "Type": "Pass", "End": true },
                    },
                },
                "End": true,
            },
        },
    })
    .to_string();
    let case = "a Choice state inside an ItemProcessor";
    let machine = create_machine(&emulator, "path-cause-nested", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"items":[{"n":1}]}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(RUNTIME_ERROR)),
        "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
    );
    let events = history(&emulator, &execution, json!({})).await;
    let entered = entered_event_id(&events, case, "ChoiceStateEntered", FAILING);
    let expected = expected_cause(FAILING, entered, "$.status", &Diagnosis::ChoiceCondition);
    let cause = string(&described, "cause");
    assert_eq!(
        cause, expected,
        "case {case:?}: expected the cause {expected:?}, got {cause:?}"
    );
    // And the enclosing Map state is named by none of it: the state that could not
    // evaluate its own path is the one the cause identifies.
    assert!(
        !cause.contains("'Fan'"),
        "case {case:?}: expected the cause to name the failing state alone, got {cause:?}"
    );
}
