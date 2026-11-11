// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a JSONata expression that MISEVALUATED in a typed field
//! of a state is reported as, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rule under test: a FIELD of a state whose value the Amazon States Language
//! requires to be of one JSON type — a `Choice` rule's boolean `Condition`, a `Wait`'s
//! numeric `Seconds`, a `Map`'s array of `Items`, a `Fail`'s string `Error` — fails the
//! execution with `States.QueryEvaluationError` when the expression written in it
//! produces a value of another type, or produces nothing at all. The cause names the
//! expression as the definition wrote it, the FIELD of the definition it is written in
//! (indexed where the field is an element of an array, so one rule of many can be
//! found), and then either the type the field expected beside the type that was
//! produced, or that the expression returned nothing. The same bytes are what the
//! history reports: an `EvaluationFailed` event carrying that error, that cause and
//! that field as its location, and the `ExecutionFailed` event the execution ends on.
//!
//! Rows vary along the two dimensions the rule generalises over: WHICH field carries
//! the expression (and, for a `Choice`, which rule of the array) and WHAT the
//! expression produced. A field whose expression produces the type the language
//! requires is the first row, so the table also pins that nothing above refuses a
//! field that evaluated.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`,
//! `describe_execution`, `string`) is the one `state_machine_lifecycle` builds and the
//! history reader is `execution_history`'s; `tests/all.rs` is this binary's crate
//! root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EVENT_EVALUATION_FAILED, EVENT_EXECUTION_FAILED,
};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// The ASL error name an expression that could not be evaluated fails under
/// ("States.QueryEvaluationError — the JSONata expression evaluation failed").
const QUERY_EVALUATION_ERROR: &str = "States.QueryEvaluationError";

/// The id of the history event the FIRST state of a definition is entered at: the
/// events are numbered from one, and an execution opens with its own
/// `ExecutionStarted`.
const ENTERED_AT: i64 = 2;

/// The whole cause the service reports for the expression `expression`, written in the
/// field `field` of the state `state` entered at the event [`ENTERED_AT`], that
/// misevaluated for the reason `because`.
///
/// The sentence is fixed text with the state, the event id, the expression, the field
/// and the diagnosis substituted in, so the test states it once and every row says
/// only what its own values are.
fn diagnosis(state: &str, expression: &str, field: &str, because: &str) -> String {
    format!(
        "An error occurred while executing the state '{state}' (entered at the event id \
         #{ENTERED_AT}). The JSONata expression '{expression}' specified for the field \
         '{field}' {because}"
    )
}

/// What the diagnosis says of an expression that produced a value of the wrong type:
/// the type the field requires, the type it produced and the VALUE it produced.
fn unexpected_type(expected: &str, produced: &str, value: &str) -> String {
    format!(
        "returned an unexpected result type. Expected '{expected}', but was '{produced}' for \
         value: {value}"
    )
}

/// What it says of an expression that produced nothing at all.
const RETURNED_NOTHING: &str = "returned nothing (undefined).";

/// One row: a JSONata definition, the input it runs over, and what the field that
/// carries the expression must be reported as.
struct Row {
    case: &'static str,
    definition: String,
    input: &'static str,
    /// The state the expression is written in, which is also the definition's
    /// `StartAt`, and the states the run must have entered.
    state: &'static str,
    /// What the run must end as: the cause of the failure the field reports, the field
    /// that failure names, and the states the run entered before it — or `None` for a
    /// field that evaluated, which fails nothing.
    expected: Option<Failed>,
    /// The output a run that did NOT fail must carry.
    output: Option<&'static str>,
    /// Every state the run must have entered, in order.
    entered: &'static [&'static str],
}

/// What a row that fails must report.
struct Failed {
    cause: String,
    location: &'static str,
}

/// A `Choice` state written in JSONata whose rules are `conditions`, routing to one
/// target per rule and to `Fallback` by default.
fn choice(conditions: &[&str]) -> String {
    let rules: Vec<Value> = conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            json!({ "Condition": condition, "Next": format!("Target{index}") })
        })
        .collect();
    let mut states = json!({
        "Route": { "Type": "Choice", "Choices": rules, "Default": "Fallback" },
        "Fallback": { "Type": "Pass", "Output": { "took": "fallback" }, "End": true },
    });
    for index in 0..conditions.len() {
        states[format!("Target{index}")] = json!({
            "Type": "Pass",
            "Output": { "took": format!("rule {index}") },
            "End": true,
        });
    }
    json!({ "QueryLanguage": "JSONata", "StartAt": "Route", "States": states }).to_string()
}

/// A one-state JSONata definition: the state `name`, of the shape `state`.
fn one_state(name: &str, state: Value) -> String {
    json!({
        "QueryLanguage": "JSONata",
        "StartAt": name,
        "States": { name: state },
    })
    .to_string()
}

/// Every typed field of a JSONata state holds its expression to the type the language
/// requires there, and reports one that misevaluated as the service reports it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_jsonata_field_reports_the_expression_the_field_and_the_types() {
    let rows = vec![
        Row {
            case: "a Condition that produces the boolean the field requires",
            definition: choice(&["{% $states.input.label = 'yes' %}"]),
            input: r#"{"label":"yes"}"#,
            state: "Route",
            expected: None,
            output: Some(r#"{"took":"rule 0"}"#),
            entered: &["Route", "Target0"],
        },
        Row {
            case: "a Condition that produces a string",
            definition: choice(&["{% $states.input.label %}"]),
            input: r#"{"label":"hello"}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.label",
                    "Choices[0]/Condition",
                    &unexpected_type("boolean", "string", "\"hello\""),
                ),
                location: "Choices[0]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            case: "a Condition that produces a number",
            definition: choice(&["{% $states.input.label %}"]),
            input: r#"{"label":1}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.label",
                    "Choices[0]/Condition",
                    &unexpected_type("boolean", "number", "1"),
                ),
                location: "Choices[0]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            case: "a Condition that produces null",
            definition: choice(&["{% $states.input.label %}"]),
            input: r#"{"label":null}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.label",
                    "Choices[0]/Condition",
                    &unexpected_type("boolean", "null", "null"),
                ),
                location: "Choices[0]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            // The SECOND rule is the one that misevaluates: the first decides false,
            // so the index the field names is the offending rule's own position.
            case: "the second rule's Condition, which produces an object",
            definition: choice(&[
                "{% $states.input.n > 100 %}",
                "{% $states.input.shape %}",
            ]),
            input: r#"{"n":1,"shape":{"a":1}}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.shape",
                    "Choices[1]/Condition",
                    &unexpected_type("boolean", "object", r#"{"a":1}"#),
                ),
                location: "Choices[1]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            case: "the third rule's Condition, which produces an array",
            definition: choice(&[
                "{% $states.input.n > 100 %}",
                "{% $states.input.n > 50 %}",
                "{% [$states.input.n] %}",
            ]),
            input: r#"{"n":1}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "[$states.input.n]",
                    "Choices[2]/Condition",
                    &unexpected_type("boolean", "array", "[1]"),
                ),
                location: "Choices[2]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            case: "a Condition that resolves to nothing",
            definition: choice(&["{% $states.input.label %}"]),
            input: r#"{"other":"no label at all"}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.label",
                    "Choices[0]/Condition",
                    RETURNED_NOTHING,
                ),
                location: "Choices[0]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            // The expression is the DEFINITION'S text and the diagnosis echoes it, so
            // one that writes the diagnosis' own clause into itself still reports the
            // field it is really written in.
            case: "a Condition whose own text carries the diagnosis' field clause",
            definition: choice(&[
                "{% $states.input.x & \"' specified for the field 'nope'\" %}",
            ]),
            input: r#"{"x":"a"}"#,
            state: "Route",
            expected: Some(Failed {
                cause: diagnosis(
                    "Route",
                    "$states.input.x & \"' specified for the field 'nope'\"",
                    "Choices[0]/Condition",
                    &unexpected_type("boolean", "string", "\"a' specified for the field 'nope'\""),
                ),
                location: "Choices[0]/Condition",
            }),
            output: None,
            entered: &["Route"],
        },
        Row {
            case: "a Wait state's Seconds, which produces a string",
            definition: one_state(
                "Hold",
                json!({ "Type": "Wait", "Seconds": "{% $states.input.delay %}", "End": true }),
            ),
            input: r#"{"delay":"soon"}"#,
            state: "Hold",
            expected: Some(Failed {
                cause: diagnosis(
                    "Hold",
                    "$states.input.delay",
                    "Seconds",
                    &unexpected_type("number", "string", "\"soon\""),
                ),
                location: "Seconds",
            }),
            output: None,
            entered: &["Hold"],
        },
        Row {
            case: "a Wait state's Seconds, which resolves to nothing",
            definition: one_state(
                "Hold",
                json!({ "Type": "Wait", "Seconds": "{% $states.input.delay %}", "End": true }),
            ),
            input: r#"{"other":1}"#,
            state: "Hold",
            expected: Some(Failed {
                cause: diagnosis("Hold", "$states.input.delay", "Seconds", RETURNED_NOTHING),
                location: "Seconds",
            }),
            output: None,
            entered: &["Hold"],
        },
        Row {
            case: "a Map state's Items, which produces an object",
            definition: one_state(
                "Each",
                json!({
                    "Type": "Map",
                    "Items": "{% $states.input.values %}",
                    "ItemProcessor": {
                        "StartAt": "P",
                        "States": { "P": { "Type": "Pass", "End": true } }
                    },
                    "End": true,
                }),
            ),
            input: r#"{"values":{"not":"an array"}}"#,
            state: "Each",
            expected: Some(Failed {
                cause: diagnosis(
                    "Each",
                    "$states.input.values",
                    "Items",
                    &unexpected_type("array", "object", r#"{"not":"an array"}"#),
                ),
                location: "Items",
            }),
            output: None,
            entered: &["Each"],
        },
        Row {
            case: "a Fail state's Error, which produces a number",
            definition: one_state(
                "Boom",
                json!({ "Type": "Fail", "Error": "{% $states.input.code %}" }),
            ),
            input: r#"{"code":7}"#,
            state: "Boom",
            expected: Some(Failed {
                cause: diagnosis(
                    "Boom",
                    "$states.input.code",
                    "Error",
                    &unexpected_type("string", "number", "7"),
                ),
                location: "Error",
            }),
            output: None,
            entered: &["Boom"],
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("typed-{index}"), &row.definition).await;
        started.push(start_execution(&emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(&emulator, &execution).await;
        let status = described.get("status").and_then(Value::as_str);
        let events = history(&emulator, &execution, json!({})).await;
        let entered: Vec<String> = events
            .iter()
            .filter_map(|event| {
                let kind = event.get("type").and_then(Value::as_str)?;
                kind.ends_with("StateEntered").then_some(())?;
                Some(
                    event
                        .get("stateEnteredEventDetails")
                        .and_then(|details| details.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            entered, row.entered,
            "case {case:?}: expected the run to have entered {:?}, got {entered:?}",
            row.entered
        );

        let Some(expected) = row.expected.as_ref() else {
            assert_eq!(
                status,
                Some("SUCCEEDED"),
                "case {case:?}: expected the field that evaluated to run on, got {described:?}"
            );
            let output: Option<Value> = described
                .get("output")
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).expect("the output is JSON"));
            let wanted: Option<Value> = row
                .output
                .map(|text| serde_json::from_str(text).expect("the expected output is JSON"));
            assert_eq!(
                output, wanted,
                "case {case:?}: expected output {wanted:?}, got {output:?}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| event.get("type").and_then(Value::as_str)
                        == Some(EVENT_EVALUATION_FAILED)),
                "case {case:?}: expected no evaluation failure, got {events:?}"
            );
            continue;
        };

        assert_eq!(
            status,
            Some("FAILED"),
            "case {case:?}: expected the misevaluated field to fail the execution, got \
             {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(QUERY_EVALUATION_ERROR),
            "case {case:?}: expected {QUERY_EVALUATION_ERROR}, got {described:?}"
        );
        assert_eq!(
            described.get("cause").and_then(Value::as_str),
            Some(expected.cause.as_str()),
            "case {case:?}: expected cause {:?}, got {:?}",
            expected.cause,
            described.get("cause")
        );
        assert!(
            described.get("output").is_none_or(Value::is_null),
            "case {case:?}: a failed execution carries no output, got {described:?}"
        );

        // The history says the same thing: the field that misevaluated is reported on
        // an event of its own, and the execution ends carrying the same bytes.
        let evaluation = events
            .iter()
            .find(|event| {
                event.get("type").and_then(Value::as_str) == Some(EVENT_EVALUATION_FAILED)
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an {EVENT_EVALUATION_FAILED} event in {events:?}")
            });
        assert_eq!(
            evaluation.get("evaluationFailedEventDetails"),
            Some(&json!({
                "error": QUERY_EVALUATION_ERROR,
                "cause": expected.cause,
                "location": expected.location,
                "state": row.state,
            })),
            "case {case:?}: expected the field and the cause on the event, got {evaluation:?}"
        );
        let failed = events
            .iter()
            .find(|event| {
                event.get("type").and_then(Value::as_str) == Some(EVENT_EXECUTION_FAILED)
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an {EVENT_EXECUTION_FAILED} event in {events:?}")
            });
        assert_eq!(
            failed.get("executionFailedEventDetails"),
            Some(&json!({
                "error": QUERY_EVALUATION_ERROR,
                "cause": expected.cause,
            })),
            "case {case:?}: expected the execution to end on the same cause, got {failed:?}"
        );
    }
}

/// A misevaluated field inside a NESTED state list is reported once, for the state the
/// expression is written in: the failure is carried to the state that encloses it
/// unchanged, so the event the inner state wrote is the only one.
///
/// Rows vary along the concurrent state the expression is nested inside, which is the
/// dimension the nesting generalises over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_misevaluated_field_inside_a_nested_state_list_is_reported_once() {
    let inner = json!({
        "StartAt": "Route",
        "States": {
            "Route": {
                "Type": "Choice",
                "Choices": [{ "Condition": "{% $states.input.label %}", "Next": "Done" }],
                "Default": "Done",
            },
            "Done": { "Type": "Pass", "End": true }
        }
    });
    // (case, the state that encloses the inner list, the input the run is given)
    let rows: [(&str, Value, &str); 2] = [
        (
            "a Choice inside a Parallel branch",
            json!({
                "Type": "Parallel",
                "Branches": [inner.clone()],
                "End": true,
            }),
            r#"{"label":"hello"}"#,
        ),
        (
            "a Choice inside a Map iteration",
            json!({
                "Type": "Map",
                "Items": "{% $states.input.each %}",
                "ItemProcessor": inner.clone(),
                "End": true,
            }),
            r#"{"each":[{"label":"hello"}]}"#,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, enclosing, input)) in rows.into_iter().enumerate() {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "Around",
            "States": { "Around": enclosing },
        })
        .to_string();
        let machine = create_machine(&emulator, &format!("nested-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the nested failure to end the execution, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(QUERY_EVALUATION_ERROR),
            "case {case:?}: expected {QUERY_EVALUATION_ERROR}, got {described:?}"
        );
        let events = history(&emulator, &execution, json!({})).await;
        let reported: Vec<&Value> = events
            .iter()
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some(EVENT_EVALUATION_FAILED)
            })
            .collect();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one {EVENT_EVALUATION_FAILED}, got {reported:?}"
        );
        // And it names the INNER state, where the expression is written, with the
        // same cause the execution reports.
        assert_eq!(
            reported[0]
                .get("evaluationFailedEventDetails")
                .and_then(|details| details.get("state"))
                .and_then(Value::as_str),
            Some("Route"),
            "case {case:?}: expected the state the expression is written in, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0]
                .get("evaluationFailedEventDetails")
                .and_then(|details| details.get("cause")),
            described.get("cause"),
            "case {case:?}: expected the event and the execution to carry one cause, got {:?}",
            reported[0]
        );
    }
}
