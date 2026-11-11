// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `error` and the `cause` a FAILED execution reports, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test, one per suite below, each the service's own FIXED sentence for
//! a class of failure with only the definition's and the execution's own values
//! substituted into it:
//!
//! 1. a document a state produced that is larger than a state's output may carry fails
//!    the execution `States.DataLimitExceeded` with the quota sentence naming what
//!    produced it, and the limit is a boundary a document may sit exactly ON: a state's
//!    output of exactly the maximum carries on, and only one byte more fails;
//! 2. a `Map` state whose items are not an array fails `States.Runtime` with the
//!    reference-path sentence naming the path that was supposed to select them;
//! 3. a `Fail` state's `ErrorPath`/`CausePath` resolving to a value that is not a
//!    string fails `States.Runtime` naming the member, what it must be and the value;
//! 4. an intrinsic-function invocation that could not be evaluated fails
//!    `States.Runtime` naming the INVOCATION as the definition wrote it, with the
//!    per-function detail after it;
//! 5. a `ResultPath` whose own segments conflict with the document it names a place in
//!    fails `States.ReferencePathConflict` naming the step that could not be applied
//!    and the document it was applied to;
//! 6. an error NAME longer than the modelled `SensitiveError` (0..256) is TRIMMED where
//!    `DescribeExecution` reports it, while the synchronous response carries the whole
//!    of what the state raised.
//!
//! Every suite's rows differ along what its rule generalises over — which state type
//! produced the document, which path named the items, which member read the value, which
//! intrinsic failed, which segment conflicted — rather than repeating one shape.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds, the Express one is
//! `sync_execution`'s, and the history reader is `execution_history`'s; `tests/all.rs`
//! is this binary's crate root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    DATA_LIMIT_ERROR, MAX_ERROR_BYTES, MAX_PAYLOAD_BYTES, REFERENCE_PATH_CONFLICT_ERROR,
    RUNTIME_ERROR,
};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
    PAST_THE_EXECUTION_DELAY,
};
use crate::sync_execution::create_express;

/// What the service states about a result larger than a state's output may carry:
/// RECORDED text, with the state or the invoked resource that produced it named in it.
fn too_large(produced_by: &str) -> String {
    format!(
        "The state/task '{produced_by}' returned a result with a size exceeding the maximum \
         number of bytes service limit."
    )
}

/// The prefix every diagnosis the interpreter raises about a state's own expression
/// carries: the state, and the id of the history event it was entered at.
fn executing(state: &str, entered: i64) -> String {
    format!("An error occurred while executing the state '{state}' (entered at the event id #{entered}). ")
}

/// A one-state definition: the state `name`, of the shape `state`.
fn one_state(name: &str, state: Value) -> String {
    json!({ "StartAt": name, "States": { name: state } }).to_string()
}

/// The `status`, `error` and `cause` of one described execution.
fn outcome(described: &Value) -> (&str, &str, &str) {
    (
        described.get("status").and_then(Value::as_str).unwrap_or(""),
        described.get("error").and_then(Value::as_str).unwrap_or(""),
        described.get("cause").and_then(Value::as_str).unwrap_or(""),
    )
}

// ---------------------------------------------------------------------------
// The data-limit sentence, and the boundary it is applied at
// ---------------------------------------------------------------------------

/// A string that, as the one member of a one-member object, makes that document exactly
/// `bytes` bytes of compact JSON: `{"v":"<padding>"}` costs eight bytes around the
/// padding.
fn padded_document_member(bytes: usize) -> String {
    "p".repeat(bytes - 8)
}

/// A state's output of exactly [`MAX_PAYLOAD_BYTES`] bytes is inside the limit and the
/// execution carries on; one byte more fails with the quota sentence naming the state.
///
/// The rows sit either side of the published bound and well inside it, and they differ
/// in WHICH state produced the document: a `Pass` state's own result, a `Parallel`
/// branch's, and the `Output` of a state written in the other query language.
#[tokio::test]
async fn a_states_output_is_bounded_at_the_byte_and_reports_the_quota_sentence() {
    let emulator = Emulator::new();

    /// The state whose output is measured, in each row's definition.
    const PRODUCER: &str = "Big";

    // (case, the definition, the state the cause must name, whether it must succeed)
    let rows: Vec<(String, String, &str, bool)> = vec![
        (
            "a Pass state's result exactly at the bound".to_string(),
            one_state(
                PRODUCER,
                json!({
                    "Type": "Pass",
                    "Result": { "v": padded_document_member(MAX_PAYLOAD_BYTES) },
                    "End": true,
                }),
            ),
            PRODUCER,
            true,
        ),
        (
            "a Pass state's result one byte past the bound".to_string(),
            one_state(
                PRODUCER,
                json!({
                    "Type": "Pass",
                    "Result": { "v": padded_document_member(MAX_PAYLOAD_BYTES + 1) },
                    "End": true,
                }),
            ),
            PRODUCER,
            false,
        ),
        (
            "a Parallel branch's own state, one byte past the bound".to_string(),
            one_state(
                "Fan",
                json!({
                    "Type": "Parallel",
                    "Branches": [{
                        "StartAt": PRODUCER,
                        "States": {
                            PRODUCER: {
                                "Type": "Pass",
                                "Result": {
                                    "v": padded_document_member(MAX_PAYLOAD_BYTES + 1),
                                },
                                "End": true,
                            },
                        },
                    }],
                    "End": true,
                }),
            ),
            PRODUCER,
            false,
        ),
        (
            "a JSONata state's Output, one byte past the bound".to_string(),
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": PRODUCER,
                "States": {
                    PRODUCER: {
                        "Type": "Pass",
                        "Output": { "v": "{% $join([1..2049].($states.input.pad), '') %}" },
                        "End": true,
                    },
                },
            })
            .to_string(),
            PRODUCER,
            false,
        ),
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, definition, _, _)) in rows.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("size-{index}"), definition).await;
        // The JSONata row builds its document out of its own input, so every row is
        // started with a padding member of its own to read.
        let input = json!({ "pad": "p".repeat(128) }).to_string();
        started.push(start_execution(&emulator, &machine, "only", &input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, producer, succeeds), execution) in rows.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        let (status, error, cause) = outcome(&described);
        if *succeeds {
            assert_eq!(
                status, "SUCCEEDED",
                "case {case:?}: a document exactly at the bound carries on, got {described:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error, cause),
            ("FAILED", DATA_LIMIT_ERROR, too_large(producer).as_str()),
            "case {case:?}: expected the quota sentence naming {producer:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A Map state whose items are not an array
// ---------------------------------------------------------------------------

/// A `Map` state whose items are not an array fails `States.Runtime` with the
/// reference-path sentence naming the path that was to select them — the state's
/// `ItemsPath`, or the `$` the language defaults it to — and quotes none of the data.
///
/// Rows differ in the path (the default `$`, a member, a nested member, a bracketed
/// member) and in the TYPE the path selected (an object, a number, a string, null).
#[tokio::test]
async fn a_map_given_something_that_is_not_an_array_names_the_reference_path() {
    let emulator = Emulator::new();

    /// The `Map` state every row declares.
    fn map_over(items_path: Option<&str>) -> String {
        let mut state = json!({
            "Type": "Map",
            "ItemProcessor": {
                "StartAt": "P",
                "States": { "P": { "Type": "Pass", "End": true } },
            },
            "End": true,
        });
        if let Some(path) = items_path {
            state["ItemsPath"] = json!(path);
        }
        one_state("Each", state)
    }

    // (case, the ItemsPath the definition declares, the input, the path the cause names)
    let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
        (
            "the whole input, which is an object",
            None,
            r#"{"not":"an array"}"#,
            "$",
        ),
        ("the whole input, which is a number", None, "5", "$"),
        (
            "an ItemsPath that selects a number",
            Some("$.Number"),
            r#"{"Number":7}"#,
            "$.Number",
        ),
        (
            "an ItemsPath that selects a string",
            Some("$.deep.text"),
            r#"{"deep":{"text":"items"}}"#,
            "$.deep.text",
        ),
        (
            "a bracketed ItemsPath that selects null",
            Some("$['slot']"),
            r#"{"slot":null}"#,
            "$['slot']",
        ),
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, items_path, input, _)) in rows.iter().enumerate() {
        let machine =
            create_machine(&emulator, &format!("map-items-{index}"), &map_over(*items_path)).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, input, path), execution) in rows.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        let (status, error, cause) = outcome(&described);
        assert_eq!(
            (status, error, cause),
            (
                "FAILED",
                RUNTIME_ERROR,
                format!("Reference path \"{path}\" must point to array.").as_str()
            ),
            "case {case:?}: expected the reference-path sentence naming {path:?}, got \
             {described:?}"
        );
        // The data itself is not quoted back: the message names the path, not the value.
        assert!(
            !cause.contains(input.trim_matches(['{', '}'])),
            "case {case:?}: expected the cause to quote none of the input, got {cause:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A Fail state's ErrorPath and CausePath
// ---------------------------------------------------------------------------

/// A `Fail` state's `ErrorPath` or `CausePath` resolving to a value that is not a string
/// fails `States.Runtime`, naming the member the definition wrote, what it must carry
/// and the value it found, under the state and the event id it was entered at.
///
/// Rows differ in the member (`ErrorPath`, `CausePath`) and in the TYPE the path
/// selected (a number, an array, an object, null, a boolean).
#[tokio::test]
async fn a_fail_states_path_member_must_resolve_to_a_string() {
    let emulator = Emulator::new();

    // (case, the member, the input, the value the cause must quote)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("CausePath selecting a number", "CausePath", r#"{"v":5}"#, "5"),
        (
            "CausePath selecting an array",
            "CausePath",
            r#"{"v":[1,2]}"#,
            "[1,2]",
        ),
        (
            "ErrorPath selecting an object",
            "ErrorPath",
            r#"{"v":{"a":1}}"#,
            r#"{"a":1}"#,
        ),
        ("ErrorPath selecting null", "ErrorPath", r#"{"v":null}"#, "null"),
        (
            "CausePath selecting a boolean",
            "CausePath",
            r#"{"v":true}"#,
            "true",
        ),
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, member, input, _)) in rows.iter().enumerate() {
        let mut state = json!({ "Type": "Fail" });
        state[*member] = json!("$.v");
        // A `Fail` state that reads one member from its input declares the other as a
        // literal, so the state is the same shape in every row.
        let other = match *member {
            "CausePath" => "Error",
            _ => "Cause",
        };
        state[other] = json!("declared");
        let definition = one_state("F", state);
        let machine = create_machine(&emulator, &format!("fail-path-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, member, _, value), execution) in rows.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        let (status, error, cause) = outcome(&described);
        let expected = format!(
            "{}The value referenced by {member} must be a string, but the value was: {value}",
            executing("F", 2)
        );
        assert_eq!(
            (status, error, cause),
            ("FAILED", RUNTIME_ERROR, expected.as_str()),
            "case {case:?}: expected the cause {expected:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An intrinsic-function invocation that could not be evaluated
// ---------------------------------------------------------------------------

/// An intrinsic-function invocation that could not be evaluated fails `States.Runtime`
/// with the invocation AS THE DEFINITION WROTE IT named in the cause, and the detail the
/// function itself reports after it: the service's own "Invalid arguments in <function>"
/// for arguments the function does not take, and the function's own sentence for a
/// `*Path` argument that selected no value.
///
/// Rows differ in the intrinsic (`States.ArrayGetItem`, `States.Format`,
/// `States.MathAdd`, `States.ArrayPartition`), in which argument is wrong, and in
/// whether the invocation is written in a `Parameters` member or in a `ResultSelector`.
#[tokio::test]
async fn an_intrinsic_that_could_not_be_evaluated_names_the_invocation() {
    let emulator = Emulator::new();

    /// The detail the service reports for arguments a function does not take.
    fn invalid_arguments(function: &str) -> String {
        format!("Invalid arguments in {function}")
    }

    // (case, the invocation, the input, the detail the cause must carry)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "an array index below zero",
            "States.ArrayGetItem($.grades, -1)",
            r#"{"grades":[1,2,3]}"#,
            invalid_arguments("States.ArrayGetItem"),
        ),
        (
            "an array index past the end of the array",
            "States.ArrayGetItem($.grades, 9)",
            r#"{"grades":[1,2,3]}"#,
            invalid_arguments("States.ArrayGetItem"),
        ),
        (
            "an argument of the wrong type",
            "States.MathAdd($.grades, 1)",
            r#"{"grades":[1,2,3]}"#,
            invalid_arguments("States.MathAdd"),
        ),
        (
            "a chunk size the function does not take",
            "States.ArrayPartition($.grades, 0)",
            r#"{"grades":[1,2,3]}"#,
            invalid_arguments("States.ArrayPartition"),
        ),
        (
            "a JSONPath argument the input carries no value for",
            "States.Format('{}-{}', $.a, $.missing)",
            r#"{"a":"a"}"#,
            "The function 'States.Format('{}-{}', $.a, $.missing)' had the following error: The \
             JsonPath argument for the field '$.missing' could not be found in the input \
             '{\"a\":\"a\"}'"
                .to_string(),
        ),
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, invocation, input, _)) in rows.iter().enumerate() {
        // The odd rows write the invocation NESTED inside another one, whose argument it
        // is: the invocation the cause names is then the inner one, which is the one
        // that failed.
        let written = match index % 2 {
            0 => (*invocation).to_string(),
            _ => format!("States.Array({invocation})"),
        };
        let definition = one_state(
            "Fmt",
            json!({
                "Type": "Pass",
                "Parameters": { "v.$": written },
                "End": true,
            }),
        );
        let machine = create_machine(&emulator, &format!("intrinsic-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, invocation, _, detail), execution) in rows.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        let (status, error, cause) = outcome(&described);
        assert_eq!(
            (status, error),
            ("FAILED", RUNTIME_ERROR),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        // A failure of the function itself is the whole of what such a cause says; every
        // other one is reported as this invocation failing to evaluate.
        let expected = match detail.starts_with("The function '") {
            true => format!("{}{detail}", executing("Fmt", 2)),
            false => format!(
                "{}There was an error while evaluating the intrinsic function: {invocation}. \
                 {detail}",
                executing("Fmt", 2)
            ),
        };
        assert_eq!(
            cause, expected,
            "case {case:?}: expected the cause {expected:?}, got {cause:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A ResultPath that conflicts with the document it names a place in
// ---------------------------------------------------------------------------

/// A `ResultPath` whose segments conflict with the document the state's result is placed
/// into fails `States.ReferencePathConflict`, naming the step that could not be applied
/// and the document it was applied to — and a `Catch` written for that name recovers the
/// execution, since the failure is not the interpreter's own terminal one.
///
/// Rows differ in the conflicting segment (a member of an array, a member of a scalar, a
/// member of a nested scalar, an index of an object) and in the document it was applied
/// to.
#[tokio::test]
async fn a_result_path_that_conflicts_reports_the_step_it_could_not_apply() {
    let emulator = Emulator::new();

    // (case, the ResultPath, the input, the step the cause names, the document it names)
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a member of the array the state was entered with",
            "$.notExist.notExist",
            "[1,2]",
            "notExist",
            "[1,2]",
        ),
        (
            "a member of a string the input carries",
            "$.slot.deeper",
            r#"{"slot":"text"}"#,
            "deeper",
            "\"text\"",
        ),
        (
            "a member of a number the input carries",
            "$.n.deeper",
            r#"{"n":1}"#,
            "deeper",
            "1",
        ),
        (
            "an index of the object the state was entered with",
            "$[0]",
            r#"{"a":1}"#,
            "0",
            r#"{"a":1}"#,
        ),
        (
            "an index past the end of the array the path reached",
            "$.items[5]",
            r#"{"items":[1,2]}"#,
            "5",
            "[1,2]",
        ),
    ];

    let mut started = Vec::with_capacity(rows.len());
    for (index, (_, result_path, input, _, _)) in rows.iter().enumerate() {
        // The state that places the result is a `Pass`, which the language does not let
        // declare a `Catch` of its own: a catcher for its failure is written on the
        // `Parallel` state around it, which is where the branch's failure arrives.
        let placing = json!({
            "Type": "Pass",
            "Result": 1,
            "ResultPath": result_path,
            "End": true,
        });
        let definition = json!({
            "StartAt": "Fan",
            "States": {
                "Fan": {
                    "Type": "Parallel",
                    "Branches": [{ "StartAt": "Place", "States": { "Place": placing } }],
                    "Catch": [{
                        "ErrorEquals": [REFERENCE_PATH_CONFLICT_ERROR],
                        "ResultPath": null,
                        "Next": "Recovered",
                    }],
                    "End": true,
                },
                "Recovered": { "Type": "Pass", "Result": { "caught": true }, "End": true },
            },
        })
        .to_string();
        let machine =
            create_machine(&emulator, &format!("result-path-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);

        // The same state with NOTHING to answer for the failure, so what the execution
        // reports is asserted too.
        let uncaught = json!({
            "StartAt": "Place",
            "States": {
                "Place": {
                    "Type": "Pass",
                    "Result": 1,
                    "ResultPath": result_path,
                    "End": true,
                },
            },
        })
        .to_string();
        let machine =
            create_machine(&emulator, &format!("result-path-bare-{index}"), &uncaught).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let mut answers = started.into_iter();
    for (case, _, _, step, document) in rows {
        let caught = describe_execution(&emulator, &answers.next().expect("one per row")).await;
        assert_eq!(
            outcome(&caught).0,
            "SUCCEEDED",
            "case {case:?}: a Catch naming {REFERENCE_PATH_CONFLICT_ERROR} recovers the \
             execution, got {caught:?}"
        );

        let described = describe_execution(&emulator, &answers.next().expect("one per row")).await;
        let (status, error, cause) = outcome(&described);
        let expected = format!("Unable to apply step \"{step}\" to input {document}");
        assert_eq!(
            (status, error, cause),
            ("FAILED", REFERENCE_PATH_CONFLICT_ERROR, expected.as_str()),
            "case {case:?}: expected the cause {expected:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An error name longer than the member it is reported through
// ---------------------------------------------------------------------------

/// An error NAME the run computed that is longer than the modelled `SensitiveError`
/// (0..256) is TRIMMED where `DescribeExecution` and the execution's history report it —
/// its first `256 - 3` characters followed by `...` — while the SYNCHRONOUS response
/// carries the whole of what the state raised.
#[tokio::test]
async fn an_over_long_error_name_is_trimmed_where_it_is_reported() {
    let emulator = Emulator::new();

    /// What marks a trimmed name, counted inside the maximum.
    const ELLIPSIS: &str = "...";

    let from_input = one_state("F", json!({ "Type": "Fail", "ErrorPath": "$.why" }));
    let raised = "e".repeat(MAX_ERROR_BYTES + 44);
    let trimmed = format!(
        "{}{ELLIPSIS}",
        "e".repeat(MAX_ERROR_BYTES - ELLIPSIS.len())
    );
    let input = json!({ "why": raised }).to_string();

    // The STANDARD execution reports the trimmed name, through both readings of it.
    let machine = create_machine(&emulator, "long-error", &from_input).await;
    let execution = start_execution(&emulator, &machine, "only", &input).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        outcome(&described).1,
        trimmed.as_str(),
        "expected DescribeExecution to report the trimmed name, got {described:?}"
    );
    let events = crate::execution_history::history(&emulator, &execution, json!({})).await;
    let failed = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("ExecutionFailed"))
        .unwrap_or_else(|| panic!("expected an ExecutionFailed event in {events:?}"));
    assert_eq!(
        failed
            .get("executionFailedEventDetails")
            .and_then(|details| details.get("error"))
            .and_then(Value::as_str),
        Some(trimmed.as_str()),
        "expected the history to report the trimmed name, got {failed:?}"
    );

    // The SYNCHRONOUS response carries what the state raised, untrimmed.
    let express = create_express(&emulator, "long-error-sync", &from_input).await;
    let answered = emulator
        .ok(
            "StartSyncExecution",
            &json!({ "stateMachineArn": express, "name": "only", "input": input }),
        )
        .await;
    assert_eq!(
        string(&answered, "error"),
        raised,
        "expected the synchronous response to carry the untrimmed name, got {answered:?}"
    );

    // A name the execution's data wrote outside ASCII is trimmed to the BYTES the
    // member carries, at a character boundary: the maximum is a size in bytes, and a
    // trimmed name is still text.
    let wide = "é".repeat(MAX_ERROR_BYTES);
    let wide_input = json!({ "why": wide }).to_string();
    let machine = create_machine(&emulator, "long-error-wide", &from_input).await;
    let execution = start_execution(&emulator, &machine, "only", &wide_input).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(&emulator, &execution).await;
    let reported = outcome(&described).1;
    assert!(
        reported.len() <= MAX_ERROR_BYTES
            && reported.ends_with(ELLIPSIS)
            && wide.starts_with(reported.trim_end_matches(ELLIPSIS)),
        "expected a prefix of the raised name within {MAX_ERROR_BYTES} bytes, got {} bytes: \
         {reported:?}",
        reported.len()
    );
}
