// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon States Language interpreter: what an execution
//! COMPUTES from the definition it was started on, read back through
//! `DescribeExecution` over this service's own wire protocol.
//!
//! The rule under test: a definition built only of states and members this emulator
//! can evaluate inside its own process is EXECUTED to the outcome the definition
//! dictates — the terminal status, the `output` and the `error`/`cause` follow from
//! the states, the data-flow members, the context object and the intrinsic functions
//! the definition uses — and only a state or member whose semantics need something
//! outside this process (a `Task` whose `Resource` names no integration call this
//! emulator makes, a Distributed Map's `ResultWriter` whose `WriterConfig` asks for a
//! dataset this emulator does not write, an `ItemReader` whose
//! `InputType` this emulator does not read) ends
//! the execution `FAILED` with ASL's `States.Runtime` and a cause that says what was
//! not run. The integration calls that ARE made are
//! `tests/all/task_integrations.rs`, the datasets an `ItemReader` DOES read are
//! `tests/all/map_item_reader.rs`, and the results a `ResultWriter` DOES write are
//! `tests/all/map_result_writer.rs`.
//!
//! Each table below spans one class of that rule: the recorded Distributed Map that
//! this suite's issue was filed about is ONE row of the first one.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{MAX_CAUSE_BYTES, MAX_ERROR_BYTES};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// What a row expects the execution to end as.
enum Expected {
    /// `SUCCEEDED`, with this JSON output.
    Output(&'static str),
    /// `FAILED`, with this `error` name.
    Failure(&'static str),
}

/// One row of the interpreter table.
struct RunCase {
    case: &'static str,
    definition: &'static str,
    input: &'static str,
    expected: Expected,
}

/// Start every row's execution, wait once for the outcomes the clock owes, and
/// assert each row's status, output and error.
async fn check_rows(emulator: &Emulator, prefix: &str, cases: &[RunCase]) {
    let mut started = Vec::with_capacity(cases.len());
    for (index, case) in cases.iter().enumerate() {
        let machine = create_machine(emulator, &format!("{prefix}-{index}"), case.definition).await;
        started.push(start_execution(emulator, &machine, "only", case.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (case, execution) in cases.iter().zip(started) {
        let described = describe_execution(emulator, &execution).await;
        let label = case.case;
        let status = described.get("status").and_then(Value::as_str);
        match case.expected {
            Expected::Output(expected) => {
                assert_eq!(
                    status,
                    Some("SUCCEEDED"),
                    "case {label:?}: expected SUCCEEDED, got {described:?}"
                );
                let expected: Value =
                    serde_json::from_str(expected).expect("the expected output is JSON");
                let actual: Value = described
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str(text).expect("the output is JSON"))
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: expected an output in {described:?}")
                    });
                assert_eq!(
                    actual, expected,
                    "case {label:?}: expected output {expected}, got {actual}"
                );
                assert!(
                    described
                        .get("outputDetails")
                        .and_then(|details| details.get("included"))
                        .and_then(Value::as_bool)
                        == Some(true),
                    "case {label:?}: a succeeded execution reports its output details, got \
                     {described:?}"
                );
            }
            Expected::Failure(error) => {
                assert_eq!(
                    status,
                    Some("FAILED"),
                    "case {label:?}: expected FAILED, got {described:?}"
                );
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(error),
                    "case {label:?}: expected error {error}, got {described:?}"
                );
                assert!(
                    described.get("output").is_none_or(Value::is_null),
                    "case {label:?}: a failed execution reports no output, got {described:?}"
                );
                let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
                assert!(
                    !error.starts_with("States.") || !cause.is_empty(),
                    "case {label:?}: a failure this emulator raised must say what happened, got \
                     {described:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Iteration: Map (inline and Distributed) and Parallel
// ---------------------------------------------------------------------------

/// A `Map` state iterates the items it selected and a `Parallel` state runs every
/// branch, so the state's output is computed from what the iterations produced.
///
/// Rows vary along the mode (inline and `"Mode": "DISTRIBUTED"`), where the items
/// come from (`ItemsPath`, the state's whole input, a literal `Items`), how many
/// there are (none, one, several), what each iteration does to its item, and how a
/// failed iteration is answered (propagated, tolerated, caught).
#[tokio::test]
async fn iteration_states_compute_their_output_from_the_items_they_ran() {
    let cases = [
        RunCase {
            // This is the shape of the recorded Distributed Map this suite's issue
            // was filed about: the items go through a `Pass` unchanged.
            case: "a Distributed Map over an ItemsPath array",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED","ExecutionType":"STANDARD"},"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: r#"{"items":[{"id":"a"},{"id":"b"}]}"#,
            expected: Expected::Output(r#"[{"id":"a"},{"id":"b"}]"#),
        },
        RunCase {
            case: "an inline Map over the whole input",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: r#"[1,2,3]"#,
            expected: Expected::Output("[1,2,3]"),
        },
        RunCase {
            case: "a Map over an empty array runs no iteration",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: r#"{"items":[]}"#,
            expected: Expected::Output("[]"),
        },
        RunCase {
            case: "a Map whose ItemSelector reads the item and its index",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemSelector":{"id.$":"$$.Map.Item.Value.id","at.$":"$$.Map.Item.Index","batch.$":"$.batch"},"ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED"},"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: r#"{"batch":"b1","items":[{"id":"a"},{"id":"b"}]}"#,
            expected: Expected::Output(
                r#"[{"id":"a","at":0,"batch":"b1"},{"id":"b","at":1,"batch":"b1"}]"#,
            ),
        },
        RunCase {
            case: "a Map with MaxConcurrency 1 still iterates every item",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","MaxConcurrency":1,"ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","Result":"seen","End":true}}},"End":true}}}"#,
            input: r#"["x","y","z"]"#,
            expected: Expected::Output(r#"["seen","seen","seen"]"#),
        },
        RunCase {
            case: "a Map nested inside a Map",
            definition: r#"{"StartAt":"Outer","States":{"Outer":{"Type":"Map","ItemProcessor":{"StartAt":"Inner","States":{"Inner":{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}},"End":true}}}"#,
            input: "[[1,2],[3]]",
            expected: Expected::Output("[[1,2],[3]]"),
        },
        RunCase {
            case: "a Map state's ResultSelector and ResultPath place its result",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"ResultSelector":{"count.$":"States.ArrayLength($)"},"ResultPath":"$.summary","End":true}}}"#,
            input: r#"{"items":[1,2,3]}"#,
            expected: Expected::Output(r#"{"items":[1,2,3],"summary":{"count":3}}"#),
        },
        RunCase {
            case: "an inline Map reports the failure of an iteration",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"F","States":{"F":{"Type":"Fail","Error":"ItemBroke","Cause":"the item could not be handled"}}},"End":true}}}"#,
            input: "[1]",
            expected: Expected::Failure("ItemBroke"),
        },
        RunCase {
            case: "a Distributed Map tolerating one failed item succeeds",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ToleratedFailureCount":1,"ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED"},"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.ok","BooleanEquals":false,"Next":"F"}],"Default":"P"},"P":{"Type":"Pass","End":true},"F":{"Type":"Fail","Error":"ItemBroke"}}},"End":true}}}"#,
            input: r#"[{"ok":true},{"ok":false}]"#,
            expected: Expected::Output(r#"[{"ok":true}]"#),
        },
        RunCase {
            case: "a Distributed Map past its tolerated failures reports the threshold error",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ToleratedFailureCount":1,"ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED"},"StartAt":"F","States":{"F":{"Type":"Fail","Error":"ItemBroke"}}},"End":true}}}"#,
            input: "[1,2]",
            expected: Expected::Failure("States.ExceedToleratedFailureThreshold"),
        },
        RunCase {
            case: "a Catch on a Map routes the error output onward",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"StartAt":"F","States":{"F":{"Type":"Fail","Error":"ItemBroke","Cause":"nope"}}},"Catch":[{"ErrorEquals":["ItemBroke"],"ResultPath":"$.failure","Next":"Report"}],"End":true},"Report":{"Type":"Pass","OutputPath":"$.failure.Error","End":true}}}"#,
            input: r#"{"items":[1]}"#,
            expected: Expected::Output(r#""ItemBroke""#),
        },
        RunCase {
            case: "a Parallel state collects its branches' outputs in order",
            definition: r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Branches":[{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"first","End":true}}},{"StartAt":"B","States":{"B":{"Type":"Pass","Result":"second","End":true}}}],"End":true}}}"#,
            input: "{}",
            expected: Expected::Output(r#"["first","second"]"#),
        },
        RunCase {
            case: "a Parallel state fails when a branch fails",
            definition: r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Branches":[{"StartAt":"A","States":{"A":{"Type":"Pass","End":true}}},{"StartAt":"B","States":{"B":{"Type":"Fail","Error":"BranchBroke"}}}],"End":true}}}"#,
            input: "{}",
            expected: Expected::Failure("BranchBroke"),
        },
        RunCase {
            case: "a Map state given something that is not an array",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: r#"{"items":{"not":"an array"}}"#,
            expected: Expected::Failure("States.Runtime"),
        },
    ];

    let emulator = Emulator::new();
    check_rows(&emulator, "iterate", &cases).await;
}

// ---------------------------------------------------------------------------
// Data flow, the context object and the intrinsic functions
// ---------------------------------------------------------------------------

/// The data-flow members decide what each state reads and writes, so the execution's
/// output is what they computed.
///
/// Rows vary along which member is used (`InputPath`, `Parameters`, `ResultPath`,
/// `OutputPath`, each also in its `null` form), the state type it is used on, and
/// which intrinsic function the payload template calls.
#[tokio::test]
async fn the_data_flow_members_decide_what_a_state_reads_and_writes() {
    let cases = [
        RunCase {
            case: "InputPath narrows the state's input",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","InputPath":"$.inner","End":true}}}"#,
            input: r#"{"inner":{"keep":1},"drop":2}"#,
            expected: Expected::Output(r#"{"keep":1}"#),
        },
        RunCase {
            case: "a null InputPath discards the input",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","InputPath":null,"End":true}}}"#,
            input: r#"{"drop":2}"#,
            expected: Expected::Output("{}"),
        },
        RunCase {
            case: "Parameters builds the result from paths and literals",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"who.$":"$.name","nested":{"always":true}},"End":true}}}"#,
            input: r#"{"name":"ada"}"#,
            expected: Expected::Output(r#"{"who":"ada","nested":{"always":true}}"#),
        },
        RunCase {
            case: "a null ResultPath keeps the state's raw input",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"ignored","ResultPath":null,"End":true}}}"#,
            input: r#"{"kept":1}"#,
            expected: Expected::Output(r#"{"kept":1}"#),
        },
        RunCase {
            case: "a ResultPath creates the member it names",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":7,"ResultPath":"$.deep.count","End":true}}}"#,
            input: "{}",
            expected: Expected::Output(r#"{"deep":{"count":7}}"#),
        },
        RunCase {
            case: "a bracketed ResultPath overwrites an existing member",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"new","ResultPath":"$['slot']","End":true}}}"#,
            input: r#"{"slot":"old","other":1}"#,
            expected: Expected::Output(r#"{"slot":"new","other":1}"#),
        },
        RunCase {
            case: "a ResultPath into something that is not an object conflicts with it",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":1,"ResultPath":"$.slot.deeper","End":true}}}"#,
            input: r#"{"slot":"a string"}"#,
            expected: Expected::Failure("States.ReferencePathConflict"),
        },
        RunCase {
            case: "OutputPath narrows the state's output",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","OutputPath":"$.pick","End":true}}}"#,
            input: r#"{"pick":[1,2],"drop":3}"#,
            expected: Expected::Output("[1,2]"),
        },
        RunCase {
            case: "a null OutputPath discards everything",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","OutputPath":null,"End":true}}}"#,
            input: r#"{"drop":1}"#,
            expected: Expected::Output("{}"),
        },
        RunCase {
            case: "a Succeed state's OutputPath decides the execution's output",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Succeed","OutputPath":"$.answer"}}}"#,
            input: r#"{"answer":42}"#,
            expected: Expected::Output("42"),
        },
        RunCase {
            case: "a path that selects nothing fails",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","InputPath":"$.missing","End":true}}}"#,
            input: "{}",
            expected: Expected::Failure("States.Runtime"),
        },
        RunCase {
            case: "a Pass carrying both Parameters and Result reports its Result",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"built.$":"$.name"},"Result":{"declared":true},"End":true}}}"#,
            input: r#"{"name":"ada"}"#,
            expected: Expected::Output(r#"{"declared":true}"#),
        },
        // The intrinsic functions below are the ones this interpreter evaluates; each
        // is from the Amazon States Language's intrinsic-function list.
        RunCase {
            case: "States.Format renders its template",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"greeting.$":"States.Format('hello, {} and {}', $.first, $.second)"},"End":true}}}"#,
            input: r#"{"first":"ada","second":"grace"}"#,
            expected: Expected::Output(r#"{"greeting":"hello, ada and grace"}"#),
        },
        RunCase {
            case: "States.Array, States.ArrayLength and States.ArrayGetItem",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"made.$":"States.Array($.a, 2, 'three')","size.$":"States.ArrayLength($.list)","first.$":"States.ArrayGetItem($.list, 0)"},"End":true}}}"#,
            input: r#"{"a":1,"list":[9,8,7]}"#,
            expected: Expected::Output(r#"{"made":[1,2,"three"],"size":3,"first":9}"#),
        },
        RunCase {
            case: "States.StringSplit, States.MathAdd and States.ArrayRange",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"parts.$":"States.StringSplit($.csv, ',')","sum.$":"States.MathAdd($.n, 3)","range.$":"States.ArrayRange(1, 3, 1)"},"End":true}}}"#,
            input: r#"{"csv":"a,b,c","n":4}"#,
            expected: Expected::Output(r#"{"parts":["a","b","c"],"sum":7,"range":[1,2,3]}"#),
        },
        RunCase {
            case: "States.StringToJson and States.JsonToString round-trip a document",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"parsed.$":"States.StringToJson($.text)","rendered.$":"States.JsonToString($.doc)"},"End":true}}}"#,
            input: r#"{"text":"{\"k\":1}","doc":{"k":1}}"#,
            expected: Expected::Output(r#"{"parsed":{"k":1},"rendered":"{\"k\":1}"}"#),
        },
        RunCase {
            case: "States.Base64Encode and States.Base64Decode round-trip text",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"encoded.$":"States.Base64Encode($.text)","decoded.$":"States.Base64Decode($.encoded)"},"End":true}}}"#,
            input: r#"{"text":"Data to encode","encoded":"RGF0YSB0byBlbmNvZGU="}"#,
            expected: Expected::Output(
                r#"{"encoded":"RGF0YSB0byBlbmNvZGU=","decoded":"Data to encode"}"#,
            ),
        },
        RunCase {
            // Every intrinsic the language documents is evaluated (the digests
            // `States.Hash` computes are `tests/all/intrinsic_functions.rs`); a name
            // the language does not define is not one, and is reported rather than
            // approximated.
            case: "an intrinsic name the language does not define is not faked",
            definition: r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"product.$":"States.MathMultiply($.n, 2)"},"End":true}}}"#,
            input: r#"{"n":3}"#,
            expected: Expected::Failure("States.Runtime"),
        },
        RunCase {
            case: "a Fail state reads its error and cause from the input",
            definition: r#"{"StartAt":"F","States":{"F":{"Type":"Fail","ErrorPath":"$.why","CausePath":"$.detail"}}}"#,
            input: r#"{"why":"FromInput","detail":"because the input said so"}"#,
            expected: Expected::Failure("FromInput"),
        },
        RunCase {
            case: "a Wait state reads its duration from the input",
            definition: r#"{"StartAt":"W","States":{"W":{"Type":"Wait","SecondsPath":"$.seconds","End":true}}}"#,
            input: r#"{"seconds":0}"#,
            expected: Expected::Output(r#"{"seconds":0}"#),
        },
    ];

    let emulator = Emulator::new();
    check_rows(&emulator, "flow", &cases).await;
}

// ---------------------------------------------------------------------------
// What is NOT run
// ---------------------------------------------------------------------------

/// A state or member whose semantics need something outside this process is not run
/// and not faked: the execution ends `FAILED` with `States.Runtime` and a cause that
/// names what was not run.
///
/// Rows vary along what reaches outside: a service integration, an S3 dataset, a
/// query language this interpreter does not evaluate.
#[tokio::test]
async fn what_cannot_be_run_in_process_fails_rather_than_being_faked() {
    let cases = [
        RunCase {
            case: "a Task state whose Resource names no integration call",
            definition: r#"{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/t","End":true}}}"#,
            input: "{}",
            expected: Expected::Failure("States.Runtime"),
        },
        RunCase {
            // The datasets this emulator DOES read are read from S3 for real
            // (`tests/all/map_item_reader.rs`); an `InputType` outside those is still
            // not read, and says so.
            case: "a Distributed Map's ItemReader whose InputType is not read",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemReader":{"Resource":"arn:aws:states:::s3:getObject","ReaderConfig":{"InputType":"PARQUET"},"Parameters":{"Bucket":"b","Key":"k"}},"ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED"},"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: "{}",
            expected: Expected::Failure("States.Runtime"),
        },
        RunCase {
            // The results this emulator DOES write are written to S3 for real
            // (`tests/all/map_result_writer.rs`); a `WriterConfig` asking for another
            // dataset — one JSON value per line — is still not written, and says so.
            case: "a Distributed Map's ResultWriter whose OutputType is not written",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ResultWriter":{"Resource":"arn:aws:states:::s3:putObject","WriterConfig":{"OutputType":"JSONL"},"Parameters":{"Bucket":"b"}},"ItemProcessor":{"ProcessorConfig":{"Mode":"DISTRIBUTED"},"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#,
            input: "[1]",
            expected: Expected::Failure("States.Runtime"),
        },
        RunCase {
            // Both query languages are evaluated now, so a JSONata definition RUNS —
            // the rows that pin what each language computes live in
            // `tests/all/jsonata_states.rs`.
            //
            // "A States.Runtime error isn't retriable, and will always cause the
            // execution to fail. A retry or catch on States.ALL won't catch
            // States.Runtime errors."
            case: "a Catch on States.ALL does not catch States.Runtime",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:dynamodb:us-west-2:000000000000:table/t","End":true}}},"Catch":[{"ErrorEquals":["States.ALL"],"Next":"Report"}],"End":true},"Report":{"Type":"Pass","Result":"caught","End":true}}}"#,
            input: "[1]",
            expected: Expected::Failure("States.Runtime"),
        },
    ];

    let emulator = Emulator::new();
    check_rows(&emulator, "unrun", &cases).await;
}

// ---------------------------------------------------------------------------
// Choice: every comparator the language defines
// ---------------------------------------------------------------------------

/// The definition a comparator row runs: the rule's branch answers `"rule"`, the
/// `Default` answers `"default"`, so what the execution outputs says which was taken.
pub(crate) fn choice_definition(rule: Value) -> String {
    json!({
        "StartAt": "C",
        "States": {
            "C": { "Type": "Choice", "Choices": [rule], "Default": "Else" },
            "Then": { "Type": "Pass", "Result": "rule", "End": true },
            "Else": { "Type": "Pass", "Result": "default", "End": true },
        },
    })
    .to_string()
}

/// Every comparator the Amazon States Language defines for a `Choice` state is
/// evaluated against the state's input.
///
/// The catalog of comparators is the Amazon States Language's own list (the
/// `Choices` field's "comparison operators": the `String*`, `Numeric*`, `Boolean*`
/// and `Timestamp*` families, the `Is*` type and presence tests, the `*Path` form of
/// each, and the `And`/`Or`/`Not` combinators). Rows cover both the matching and the
/// non-matching side of each family, including a comparator the issue did not cite.
#[tokio::test]
async fn every_choice_comparator_the_language_defines_is_evaluated() {
    // (case, rule, input, the branch that must be taken)
    let cases: Vec<(&str, Value, Value, &str)> = vec![
        (
            "StringEquals that matches",
            json!({"Variable":"$.s","StringEquals":"go","Next":"Then"}),
            json!({"s":"go"}),
            "rule",
        ),
        (
            "StringEquals that does not match",
            json!({"Variable":"$.s","StringEquals":"go","Next":"Then"}),
            json!({"s":"stop"}),
            "default",
        ),
        (
            "StringEqualsPath against another member",
            json!({"Variable":"$.s","StringEqualsPath":"$.other","Next":"Then"}),
            json!({"s":"same","other":"same"}),
            "rule",
        ),
        (
            "StringLessThan",
            json!({"Variable":"$.s","StringLessThan":"b","Next":"Then"}),
            json!({"s":"a"}),
            "rule",
        ),
        (
            "StringLessThanEquals at the bound",
            json!({"Variable":"$.s","StringLessThanEquals":"a","Next":"Then"}),
            json!({"s":"a"}),
            "rule",
        ),
        (
            "StringGreaterThan",
            json!({"Variable":"$.s","StringGreaterThan":"a","Next":"Then"}),
            json!({"s":"b"}),
            "rule",
        ),
        (
            "StringGreaterThanEquals past the bound",
            json!({"Variable":"$.s","StringGreaterThanEquals":"b","Next":"Then"}),
            json!({"s":"a"}),
            "default",
        ),
        (
            "StringMatches with a trailing wildcard",
            json!({"Variable":"$.s","StringMatches":"log-*","Next":"Then"}),
            json!({"s":"log-2026"}),
            "rule",
        ),
        (
            "StringMatches with a wildcard in the middle",
            json!({"Variable":"$.s","StringMatches":"a*c","Next":"Then"}),
            json!({"s":"abbbc"}),
            "rule",
        ),
        (
            "StringMatches that does not match",
            json!({"Variable":"$.s","StringMatches":"log-*","Next":"Then"}),
            json!({"s":"metric-1"}),
            "default",
        ),
        (
            // A leftmost-first scan that never reconsidered would answer `false`
            // here, so these two rows pin the backtracking the glob needs.
            "StringMatches with a leading wildcard whose literal recurs",
            json!({"Variable":"$.s","StringMatches":"*ab","Next":"Then"}),
            json!({"s":"abab"}),
            "rule",
        ),
        (
            "StringMatches with a middle wildcard whose literal recurs",
            json!({"Variable":"$.s","StringMatches":"a*bc","Next":"Then"}),
            json!({"s":"abcbc"}),
            "rule",
        ),
        (
            "StringMatches with an escaped wildcard matches the literal character",
            json!({"Variable":"$.s","StringMatches":"a\\*b","Next":"Then"}),
            json!({"s":"a*b"}),
            "rule",
        ),
        (
            "StringMatches with an escaped wildcard does not match any character",
            json!({"Variable":"$.s","StringMatches":"a\\*b","Next":"Then"}),
            json!({"s":"axb"}),
            "default",
        ),
        (
            "StringMatches of only wildcards matches anything",
            json!({"Variable":"$.s","StringMatches":"**","Next":"Then"}),
            json!({"s":"whatever it is"}),
            "rule",
        ),
        (
            "StringMatches with no wildcard is an exact comparison",
            json!({"Variable":"$.s","StringMatches":"exact","Next":"Then"}),
            json!({"s":"exactly"}),
            "default",
        ),
        (
            "StringMatches of an empty pattern matches only the empty string",
            json!({"Variable":"$.s","StringMatches":"","Next":"Then"}),
            json!({"s":""}),
            "rule",
        ),
        (
            "NumericEquals",
            json!({"Variable":"$.n","NumericEquals":3,"Next":"Then"}),
            json!({"n":3}),
            "rule",
        ),
        (
            "NumericLessThan",
            json!({"Variable":"$.n","NumericLessThan":3.5,"Next":"Then"}),
            json!({"n":3}),
            "rule",
        ),
        (
            "NumericLessThanEquals at the bound",
            json!({"Variable":"$.n","NumericLessThanEquals":3,"Next":"Then"}),
            json!({"n":3}),
            "rule",
        ),
        (
            "NumericGreaterThan at the bound",
            json!({"Variable":"$.n","NumericGreaterThan":3,"Next":"Then"}),
            json!({"n":3}),
            "default",
        ),
        (
            "NumericGreaterThanEqualsPath",
            json!({"Variable":"$.n","NumericGreaterThanEqualsPath":"$.limit","Next":"Then"}),
            json!({"n":10,"limit":10}),
            "rule",
        ),
        (
            "BooleanEquals",
            json!({"Variable":"$.b","BooleanEquals":true,"Next":"Then"}),
            json!({"b":true}),
            "rule",
        ),
        (
            "BooleanEqualsPath",
            json!({"Variable":"$.b","BooleanEqualsPath":"$.expected","Next":"Then"}),
            json!({"b":false,"expected":true}),
            "default",
        ),
        (
            "TimestampEquals",
            json!({"Variable":"$.t","TimestampEquals":"2026-01-01T00:00:00Z","Next":"Then"}),
            json!({"t":"2026-01-01T00:00:00Z"}),
            "rule",
        ),
        (
            "TimestampLessThan",
            json!({"Variable":"$.t","TimestampLessThan":"2026-01-02T00:00:00Z","Next":"Then"}),
            json!({"t":"2026-01-01T00:00:00Z"}),
            "rule",
        ),
        (
            "TimestampGreaterThan",
            json!({"Variable":"$.t","TimestampGreaterThan":"2026-01-02T00:00:00Z","Next":"Then"}),
            json!({"t":"2026-01-01T00:00:00Z"}),
            "default",
        ),
        (
            "TimestampGreaterThanEqualsPath",
            json!({"Variable":"$.t","TimestampGreaterThanEqualsPath":"$.since","Next":"Then"}),
            json!({"t":"2026-01-03T00:00:00Z","since":"2026-01-02T00:00:00Z"}),
            "rule",
        ),
        (
            "TimestampLessThanEquals at the bound",
            json!({"Variable":"$.t","TimestampLessThanEquals":"2026-01-01T00:00:00Z","Next":"Then"}),
            json!({"t":"2026-01-01T00:00:00Z"}),
            "rule",
        ),
        (
            "IsPresent on a member the input carries",
            json!({"Variable":"$.maybe","IsPresent":true,"Next":"Then"}),
            json!({"maybe":null}),
            "rule",
        ),
        (
            "IsPresent on a member the input does not carry",
            json!({"Variable":"$.maybe","IsPresent":true,"Next":"Then"}),
            json!({"other":1}),
            "default",
        ),
        (
            "IsNull on a null member",
            json!({"Variable":"$.maybe","IsNull":true,"Next":"Then"}),
            json!({"maybe":null}),
            "rule",
        ),
        (
            "IsNumeric on a number",
            json!({"Variable":"$.v","IsNumeric":true,"Next":"Then"}),
            json!({"v":1.5}),
            "rule",
        ),
        (
            "IsString on a number",
            json!({"Variable":"$.v","IsString":true,"Next":"Then"}),
            json!({"v":1.5}),
            "default",
        ),
        (
            "IsBoolean on a boolean",
            json!({"Variable":"$.v","IsBoolean":true,"Next":"Then"}),
            json!({"v":false}),
            "rule",
        ),
        (
            "IsTimestamp on an ISO 8601 string",
            json!({"Variable":"$.v","IsTimestamp":true,"Next":"Then"}),
            json!({"v":"2026-01-01T00:00:00Z"}),
            "rule",
        ),
        (
            "IsTimestamp on a string that is not one",
            json!({"Variable":"$.v","IsTimestamp":true,"Next":"Then"}),
            json!({"v":"yesterday"}),
            "default",
        ),
        (
            "And over two conditions",
            json!({"And":[
                {"Variable":"$.n","NumericGreaterThan":1},
                {"Variable":"$.s","StringEquals":"go"}
            ],"Next":"Then"}),
            json!({"n":2,"s":"go"}),
            "rule",
        ),
        (
            "And where one condition fails",
            json!({"And":[
                {"Variable":"$.n","NumericGreaterThan":1},
                {"Variable":"$.s","StringEquals":"go"}
            ],"Next":"Then"}),
            json!({"n":0,"s":"go"}),
            "default",
        ),
        (
            "Or over two conditions",
            json!({"Or":[
                {"Variable":"$.n","NumericGreaterThan":10},
                {"Variable":"$.s","StringEquals":"go"}
            ],"Next":"Then"}),
            json!({"n":0,"s":"go"}),
            "rule",
        ),
        (
            "Not around a condition",
            json!({"Not":{"Variable":"$.s","StringEquals":"stop"},"Next":"Then"}),
            json!({"s":"go"}),
            "rule",
        ),
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(cases.len());
    for (index, (_, rule, input, _)) in cases.iter().enumerate() {
        let definition = choice_definition(rule.clone());
        let machine = create_machine(&emulator, &format!("choice-{index}"), &definition).await;
        started.push(start_execution(&emulator, &machine, "only", &input.to_string()).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, _, expected), execution) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: a Choice state over a value it can compare succeeds, got {described:?}"
        );
        let output = described.get("output").and_then(Value::as_str).unwrap_or("");
        assert_eq!(
            output,
            format!("\"{expected}\""),
            "case {case:?}: expected the {expected:?} branch, got {described:?}"
        );
    }
}

/// A `Choice` state no rule matched, and that declares no `Default`, ends the
/// execution under the interpreter's own `States.Runtime`.
#[tokio::test]
async fn a_choice_with_no_match_and_no_default_reports_no_choice_matched() {
    let cases = [RunCase {
        case: "no rule matches and there is no Default",
        definition: r#"{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.s","StringEquals":"go","Next":"Then"}]},"Then":{"Type":"Succeed"}}}"#,
        input: r#"{"s":"stop"}"#,
        expected: Expected::Failure("States.Runtime"),
    }];

    let emulator = Emulator::new();
    check_rows(&emulator, "nochoice", &cases).await;
}

// ---------------------------------------------------------------------------
// The context object
// ---------------------------------------------------------------------------

/// A definition reading the context object (`$$`) reads THIS execution's own
/// identity: the ARNs and names the start call fixed, not a placeholder.
#[tokio::test]
async fn the_context_object_reports_this_execution() {
    let emulator = Emulator::new();
    let definition = r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"executionArn.$":"$$.Execution.Id","executionName.$":"$$.Execution.Name","machineArn.$":"$$.StateMachine.Id","machineName.$":"$$.StateMachine.Name","stateName.$":"$$.State.Name","input.$":"$$.Execution.Input"},"End":true}}}"#;
    let machine = create_machine(&emulator, "context", definition).await;
    let execution = start_execution(&emulator, &machine, "ctx-run", r#"{"seed":1}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let described = describe_execution(&emulator, &execution).await;
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case context: expected an output in {described:?}"));
    let expected = json!({
        "executionArn": execution,
        "executionName": "ctx-run",
        "machineArn": machine,
        "machineName": "context",
        "stateName": "A",
        "input": {"seed": 1},
    });
    assert_eq!(
        output, expected,
        "case context: expected the run's own identity {expected}, got {output}"
    );
}

// ---------------------------------------------------------------------------
// What the clock owes: a Map's waits and a retried state's interval
// ---------------------------------------------------------------------------

/// The `Wait` states a `Map` iteration passes lengthen the run, and a `Retry`'s
/// interval lengthens it again, so the outcome is not due until they have passed:
/// the iterations and the retry really ran rather than being skipped.
#[tokio::test]
async fn waits_inside_an_iteration_and_a_retry_interval_hold_the_outcome() {
    let emulator = Emulator::new();

    // Two items, each waiting a minute, running concurrently: the Map's span is one
    // item's wait, which is still far ahead of the settle delay.
    let waiting = r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":60,"End":true}}},"End":true}}}"#;
    // One item whose iteration fails, retried once a second later: the failure is
    // not due until the retry's interval has passed.
    let retried = r#"{"StartAt":"M","States":{"M":{"Type":"Map","Retry":[{"ErrorEquals":["ItemBroke"],"MaxAttempts":1,"IntervalSeconds":1}],"ItemProcessor":{"StartAt":"F","States":{"F":{"Type":"Fail","Error":"ItemBroke","Cause":"the item could not be handled"}}},"End":true}}}"#;

    let waiting_machine = create_machine(&emulator, "owed-wait", waiting).await;
    let retried_machine = create_machine(&emulator, "owed-retry", retried).await;
    let waiting_run = start_execution(&emulator, &waiting_machine, "only", "[1,2]").await;
    let retried_run = start_execution(&emulator, &retried_machine, "only", "[1]").await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    for (case, execution) in [("a Map of Waits", &waiting_run), ("a retried Map", &retried_run)] {
        let described = describe_execution(&emulator, execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: the outcome is not due yet, got {described:?}"
        );
    }

    // Past the retry's own interval, but nowhere near the minute the Map waits.
    std::thread::sleep(Duration::from_millis(900));
    let retried_described = describe_execution(&emulator, &retried_run).await;
    assert_eq!(
        retried_described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case \"a retried Map\": the failure is due once the retry interval has passed, got \
         {retried_described:?}"
    );
    let waiting_described = describe_execution(&emulator, &waiting_run).await;
    assert_eq!(
        waiting_described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case \"a Map of Waits\": an iteration's own Wait still holds the outcome, got \
         {waiting_described:?}"
    );
}

/// A definition that declares `TimeoutSeconds` and runs past it ends `TIMED_OUT`,
/// rather than succeeding late — reporting that status and nothing else, because what
/// ran out is the execution's own allowance rather than anything one state did.
#[tokio::test]
async fn an_execution_past_its_definition_timeout_times_out() {
    let emulator = Emulator::new();
    let definition = r#"{"TimeoutSeconds":1,"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":3600,"End":true}}}"#;
    let machine = create_machine(&emulator, "timeout", definition).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let before = describe_execution(&emulator, &execution).await;
    assert_eq!(
        before.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case timeout: the timeout is not due with the start, got {before:?}"
    );

    std::thread::sleep(Duration::from_millis(900));
    let after = describe_execution(&emulator, &execution).await;
    assert_eq!(
        after.get("status").and_then(Value::as_str),
        Some("TIMED_OUT"),
        "case timeout: an execution past its TimeoutSeconds times out, got {after:?}"
    );
    assert!(
        after.get("error").is_none_or(Value::is_null),
        "case timeout: a machine-level timeout names no error, got {after:?}"
    );
    assert!(
        after.get("cause").is_none_or(Value::is_null),
        "case timeout: a machine-level timeout names no cause, got {after:?}"
    );
    assert!(
        after.get("output").is_none_or(Value::is_null),
        "case timeout: a timed-out execution reports no output, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// What bounds a run: hostile, malformed and oversized definitions
// ---------------------------------------------------------------------------

/// A definition is a CALLER's input, bounded only in bytes, so every dimension of a
/// run carries its own ceiling: a definition that asks for more than one of them ends
/// in a modelled `States.*` failure, and the emulator keeps serving.
///
/// Rows vary along WHICH ceiling is asked past — the bytes and the nesting of the
/// document a state produces, the nesting of `Map` iterations, the nesting and the
/// length of an intrinsic invocation, the segment count of a reference path — and
/// along the malformed forms of each parser's own syntax.
#[tokio::test]
async fn a_definition_past_what_the_interpreter_bounds_fails_rather_than_running() {
    /// `levels` `Map` states nested one inside the next, around a `Pass`.
    fn nested_maps(levels: usize) -> String {
        let mut definition =
            r#"{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}"#.to_string();
        for _ in 0..levels {
            definition = format!(
                r#"{{"StartAt":"M","States":{{"M":{{"Type":"Map","ItemProcessor":{definition},"End":true}}}}}}"#
            );
        }
        definition
    }

    /// An array nested `levels` deep, which a `Map` nested that deep iterates.
    fn nested_items(levels: usize) -> String {
        format!("{}1{}", "[".repeat(levels), "]".repeat(levels))
    }

    // (case, definition, input, the error the execution must report)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "an intrinsic invocation nested past the depth this service evaluates",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Parameters":{{"x.$":"{}1{}"}},"End":true}}}}}}"#,
                "States.Array(".repeat(20),
                ")".repeat(20)
            ),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "an intrinsic invocation longer than this service evaluates",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Parameters":{{"x.$":"States.Format('{}')"}},"End":true}}}}}}"#,
                "p".repeat(5_000)
            ),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "an intrinsic invocation that is never closed",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"x.$":"States.Array(1, 2"},"End":true}}}"#
                .to_string(),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "an intrinsic invocation with an unterminated quoted string",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"x.$":"States.Format('still going)"},"End":true}}}"#
                .to_string(),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "an intrinsic name this service does not carry",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"x.$":"States.Teleport($.anywhere)"},"End":true}}}"#
                .to_string(),
            r#"{"anywhere":1}"#.to_string(),
            "States.Runtime",
        ),
        (
            "a reference path with more segments than this service follows",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Result":1,"ResultPath":"${}","End":true}}}}}}"#,
                ".a".repeat(80)
            ),
            "{}".to_string(),
            "States.ResultPathMatchFailure",
        ),
        (
            "a reference path with an unclosed subscript",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":1,"ResultPath":"$.a[0","End":true}}}"#
                .to_string(),
            "{}".to_string(),
            "States.ResultPathMatchFailure",
        ),
        (
            "a reference path that is not a path at all",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":1,"ResultPath":"a.b","End":true}}}"#
                .to_string(),
            "{}".to_string(),
            "States.ResultPathMatchFailure",
        ),
        (
            // The state the cycle ends at is reached from the `Choice` beside it, so
            // the only problem this definition carries is the one under test: a
            // state nothing reaches is an error of its own.
            "a definition that doubles its document on every transition",
            r#"{"StartAt":"G","States":{"G":{"Type":"Pass","Parameters":{"a.$":"$","b.$":"$"},"Next":"C"},"C":{"Type":"Choice","Choices":[{"Variable":"$.stop","IsPresent":true,"Next":"Done"}],"Default":"G"},"Done":{"Type":"Succeed"}}}"#
                .to_string(),
            format!(r#"{{"seed":"{}"}}"#, "s".repeat(1_024)),
            "States.DataLimitExceeded",
        ),
        (
            "a definition that nests its document one level deeper on every transition",
            r#"{"StartAt":"W","States":{"W":{"Type":"Pass","Parameters":{"a.$":"$"},"Next":"C"},"C":{"Type":"Choice","Choices":[{"Variable":"$.stop","IsPresent":true,"Next":"Done"}],"Default":"W"},"Done":{"Type":"Succeed"}}}"#
                .to_string(),
            "{}".to_string(),
            "States.DataLimitExceeded",
        ),
        (
            "Map iterations nested deeper than this emulator runs",
            nested_maps(20),
            nested_items(20),
            "States.Runtime",
        ),
        (
            "a path longer than this service evaluates",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","InputPath":"$.{}","End":true}}}}}}"#,
                "p".repeat(2_000)
            ),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "a path with more descendant segments than this service evaluates",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","InputPath":"$..a..b..c","End":true}}}"#
                .to_string(),
            r#"{"a":{"b":{"c":1}}}"#.to_string(),
            "States.Runtime",
        ),
        (
            "a payload template whose members together outgrow a state's output",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"a.$":"$","b.$":"$","c.$":"$"},"End":true}}}"#
                .to_string(),
            format!(r#"{{"blob":"{}"}}"#, "b".repeat(100_000)),
            "States.DataLimitExceeded",
        ),
        (
            "an intrinsic handed more arguments bytes than a state's output carries",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"x.$":"States.Array($, $, $, $)"},"End":true}}}"#
                .to_string(),
            format!(r#"{{"blob":"{}"}}"#, "b".repeat(80_000)),
            "States.Runtime",
        ),
        (
            "a Choice state with more rules than this service evaluates",
            format!(
                r#"{{"StartAt":"C","States":{{"C":{{"Type":"Choice","Choices":[{}],"Default":"D"}},"D":{{"Type":"Succeed"}}}}}}"#,
                (0..200)
                    .map(|index| format!(
                        r#"{{"Variable":"$.n","NumericEquals":{index},"Next":"D"}}"#
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            r#"{"n":1000}"#.to_string(),
            "States.Runtime",
        ),
        (
            "a payload template with more paths than this service evaluates",
            format!(
                r#"{{"StartAt":"A","States":{{"A":{{"Type":"Pass","Parameters":{{{}}},"End":true}}}}}}"#,
                (0..200)
                    .map(|index| format!(r#""m{index}.$":"$.n""#))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            r#"{"n":1}"#.to_string(),
            "States.ParameterPathFailure",
        ),
        (
            // A ceiling this emulator sets is a FAILURE, never an answer about the
            // data: a `Choice` must not fall through to its `Default` because the
            // comparator was not evaluated.
            "a StringMatches pattern longer than this service matches",
            format!(
                r#"{{"StartAt":"C","States":{{"C":{{"Type":"Choice","Choices":[{{"Variable":"$.s","StringMatches":"{}*","Next":"D"}}],"Default":"D"}},"D":{{"Type":"Succeed"}}}}}}"#,
                "p".repeat(300)
            ),
            r#"{"s":"anything"}"#.to_string(),
            "States.Runtime",
        ),
        (
            "a path this service refuses under an IsPresent test",
            r#"{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$..deep","IsPresent":true,"Next":"D"}],"Default":"D"},"D":{"Type":"Succeed"}}}"#
                .to_string(),
            r#"{"a":{"deep":1}}"#.to_string(),
            "States.Runtime",
        ),
        (
            "a Parallel state with more branches than this service runs",
            format!(
                r#"{{"StartAt":"P","States":{{"P":{{"Type":"Parallel","Branches":[{}],"End":true}}}}}}"#,
                (0..100)
                    .map(|index| format!(
                        r#"{{"StartAt":"B{index}","States":{{"B{index}":{{"Type":"Pass","End":true}}}}}}"#
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            "{}".to_string(),
            "States.Runtime",
        ),
        (
            "Parallel branches that together outgrow a state's output",
            format!(
                r#"{{"StartAt":"P","States":{{"P":{{"Type":"Parallel","Branches":[{}],"End":true}}}}}}"#,
                (0..8)
                    .map(|index| format!(
                        r#"{{"StartAt":"B{index}","States":{{"B{index}":{{"Type":"Pass","End":true}}}}}}"#
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            format!(r#"{{"blob":"{}"}}"#, "b".repeat(60_000)),
            "States.DataLimitExceeded",
        ),
        (
            "a choice rule nesting conditions deeper than this service evaluates",
            format!(
                r#"{{"StartAt":"C","States":{{"C":{{"Type":"Choice","Choices":[{{{}"Variable":"$.s","StringEquals":"go"{},"Next":"D"}}],"Default":"D"}},"D":{{"Type":"Succeed"}}}}}}"#,
                r#""Not":{"#.repeat(20),
                "}".repeat(20)
            ),
            r#"{"s":"go"}"#.to_string(),
            "States.Runtime",
        ),
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(cases.len());
    for (index, (_, definition, input, _)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("bounded-{index}"), definition).await;
        started.push(start_execution(&emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, _, expected), execution) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected error {expected}, got {described:?}"
        );
        let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
        assert!(
            !cause.is_empty(),
            "case {case:?}: a refusal must say what was not run, got {described:?}"
        );
    }

    // And the emulator is still serving: a definition it can run still runs.
    let healthy = create_machine(
        &emulator,
        "bounded-healthy",
        r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"fine","End":true}}}"#,
    )
    .await;
    let run = start_execution(&emulator, &healthy, "only", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(&emulator, &run).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"still serving\": the next execution still runs, got {described:?}"
    );
}

/// Numbers a definition chose reach arithmetic, so the values at the edge of what a
/// JSON number carries are answered rather than crashing the handler.
///
/// Rows vary along which arithmetic is driven to its edge: a retrier's interval and
/// backoff (whose product overflows, or is the zero-times-infinity that is NaN), and
/// an intrinsic's range (whose span is past what an integer carries).
#[tokio::test]
async fn numbers_at_the_edge_of_what_a_definition_may_carry_are_answered() {
    let emulator = Emulator::new();

    // A retrier whose interval is 0 and whose backoff overflows: the product is not a
    // number, and the retry delay it asks for is the longest wait this service waits.
    let overflowing_retry = r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Retry":[{"ErrorEquals":["BranchBroke"],"IntervalSeconds":0,"BackoffRate":1e200,"MaxAttempts":5}],"Branches":[{"StartAt":"F","States":{"F":{"Type":"Fail","Error":"BranchBroke"}}}],"End":true}}}"#;
    let retry_machine = create_machine(&emulator, "edge-retry", overflowing_retry).await;
    let retry_run = start_execution(&emulator, &retry_machine, "only", "{}").await;

    // A range whose span is larger than an integer carries.
    let huge_range = r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"drawn.$":"States.MathRandom(0, 1e30)"},"End":true}}}"#;
    let range_machine = create_machine(&emulator, "edge-random", huge_range).await;
    let range_run = start_execution(&emulator, &range_machine, "only", "{}").await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The retried execution is answered — and it is still RUNNING, because the delay
    // its retrier asked for is a year rather than a non-number.
    let described = describe_execution(&emulator, &retry_run).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case \"an overflowing retry\": the retry's delay holds the outcome, got {described:?}"
    );

    let described = describe_execution(&emulator, &range_run).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"a range past what an integer carries\": expected SUCCEEDED, got {described:?}"
    );
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case \"a huge range\": expected an output in {described:?}"));
    let drawn = output.get("drawn").and_then(Value::as_i64);
    assert!(
        drawn.is_some_and(|drawn| drawn >= 0),
        "case \"a range past what an integer carries\": expected a non-negative draw, got {output}"
    );
}

// ---------------------------------------------------------------------------
// What the clock owes: a Map's waits and a retried state's interval
// ---------------------------------------------------------------------------

/// A retried state reads the attempt it is on through `$$.State.RetryCount`, so the
/// value a retried `ItemSelector` sees is the attempt the interpreter really ran.
#[tokio::test]
async fn a_retried_state_reads_the_attempt_it_is_on() {
    let emulator = Emulator::new();
    // The first attempt fails (the item carries no `ok`), and the retry's
    // `ItemSelector` supplies it from the attempt count, so the second succeeds and
    // the output says which attempt produced it.
    let definition = r#"{"StartAt":"M","States":{"M":{"Type":"Map","Retry":[{"ErrorEquals":["NeedAnotherTry"],"MaxAttempts":2,"IntervalSeconds":0}],"ItemSelector":{"attempt.$":"$$.State.RetryCount","value.$":"$$.Map.Item.Value"},"ItemProcessor":{"StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.attempt","NumericEquals":0,"Next":"F"}],"Default":"P"},"P":{"Type":"Pass","End":true},"F":{"Type":"Fail","Error":"NeedAnotherTry"}}},"End":true}}}"#;
    let machine = create_machine(&emulator, "retry-count", definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"["a"]"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"a retried attempt\": the second attempt succeeds, got {described:?}"
    );
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case \"a retried attempt\": expected an output in {described:?}"));
    assert_eq!(
        output,
        json!([{ "attempt": 1, "value": "a" }]),
        "case \"a retried attempt\": expected the attempt the interpreter ran, got {output}"
    );
}

/// A `Map` whose iteration is a bare `Pass` outputs exactly the items it iterated,
/// in order: the invariant holds over the item count and over what each item is.
#[tokio::test]
async fn a_map_of_passes_mirrors_its_items() {
    let emulator = Emulator::new();
    let definition = r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}}}"#;
    let machine = create_machine(&emulator, "mirror", definition).await;

    // Rows vary along the item count (none, one, several) and along what an item is:
    // every JSON kind a state's payload may carry.
    let cases: Vec<Value> = vec![
        json!([]),
        json!([1]),
        json!(["a", "b", "c"]),
        json!([null, true, 2.5, "s", [1], {"k": "v"}]),
        json!([{"id":"a"},{"id":"b"},{"id":"c"},{"id":"d"},{"id":"e"}]),
        // Order must be kept, not sorted.
        json!([3, 1, 2]),
    ];

    let mut started = Vec::with_capacity(cases.len());
    for (index, items) in cases.iter().enumerate() {
        started.push(
            start_execution(
                &emulator,
                &machine,
                &format!("mirror-{index}"),
                &items.to_string(),
            )
            .await,
        );
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (items, execution) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        let actual: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {items}: expected an output in {described:?}"));
        assert_eq!(
            &actual, items,
            "case {items}: a Map of Passes outputs its items unchanged, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the run computes stays inside what the service reports it through
// ---------------------------------------------------------------------------

/// An `error` and a `cause` the run computed from the execution's own data stay
/// inside the modelled `SensitiveError` (0..256) and `SensitiveCause` (0..32768)
/// shapes they are reported through: an over-long error NAME is TRIMMED to the
/// maximum — its first `256 - 3` characters followed by `...` — and an over-long cause
/// keeps the error name and is clipped.
///
/// Rows sit either side of each published bound, which is the member's own modelled
/// length, and one far past it.
#[tokio::test]
async fn a_computed_error_and_cause_stay_within_what_the_service_reports() {
    let emulator = Emulator::new();
    let from_input = r#"{"StartAt":"F","States":{"F":{"Type":"Fail","ErrorPath":"$.why"}}}"#;
    let machine = create_machine(&emulator, "bounded-error", from_input).await;

    /// What marks an error name the maximum cut, counted inside that maximum.
    const ELLIPSIS: &str = "...";

    // (case, the error name's length, the error the execution must report)
    let cases: Vec<(&str, usize, String)> = vec![
        (
            "an error name at the modelled bound",
            MAX_ERROR_BYTES,
            "e".repeat(MAX_ERROR_BYTES),
        ),
        (
            "an error name one byte past the bound",
            MAX_ERROR_BYTES + 1,
            format!("{}{ELLIPSIS}", "e".repeat(MAX_ERROR_BYTES - ELLIPSIS.len())),
        ),
        (
            "an error name far past the bound",
            MAX_ERROR_BYTES * 4,
            format!("{}{ELLIPSIS}", "e".repeat(MAX_ERROR_BYTES - ELLIPSIS.len())),
        ),
    ];
    let mut started = Vec::with_capacity(cases.len());
    for (index, (_, length, _)) in cases.iter().enumerate() {
        let input = json!({ "why": "e".repeat(*length) }).to_string();
        started.push(
            start_execution(&emulator, &machine, &format!("error-{index}"), &input).await,
        );
    }

    // A cause the interpreter wrote quotes the document a path could not be found in,
    // so a large input is what pushes it past its own bound.
    let absent_member = r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"v.$":"$.absent"},"End":true}}}"#;
    let bulky = create_machine(&emulator, "bounded-cause", absent_member).await;
    let bulky_input = json!({ "blob": "b".repeat(MAX_CAUSE_BYTES * 2) }).to_string();
    let bulky_run = start_execution(&emulator, &bulky, "only", &bulky_input).await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for ((case, _, expected), execution) in cases.iter().zip(started) {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected error {expected:?}, got {described:?}"
        );
        let reported = described.get("error").and_then(Value::as_str).unwrap_or("");
        assert!(
            reported.len() <= MAX_ERROR_BYTES,
            "case {case:?}: expected an error name within {MAX_ERROR_BYTES} bytes, got {} bytes",
            reported.len()
        );
    }

    let described = describe_execution(&emulator, &bulky_run).await;
    let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
    assert_eq!(
        described.get("error").and_then(Value::as_str),
        Some("States.Runtime"),
        "case \"an over-long cause\": the error name is kept, got {described:?}"
    );
    assert!(
        !cause.is_empty() && cause.len() <= MAX_CAUSE_BYTES,
        "case \"an over-long cause\": expected a cause within {MAX_CAUSE_BYTES} bytes, got {} \
         bytes",
        cause.len()
    );
}
