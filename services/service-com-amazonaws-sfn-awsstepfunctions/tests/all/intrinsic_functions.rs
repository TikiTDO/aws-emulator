// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon States Language's INTRINSIC FUNCTIONS, driven
//! through `StartExecution` and read back through `DescribeExecution` over this
//! service's own wire protocol.
//!
//! The rules under test, one table each:
//!
//! * `States.Hash(data, algorithm)` returns the lowercase hex digest of its input
//!   under the named algorithm, for each of the five the language names;
//! * `States.JsonMerge` merges the two documents SHALLOWLY and refuses the deep mode
//!   the language does not support;
//! * inside an intrinsic's single-quoted string literal a backslash makes the next
//!   character DATA, so a `States.Format` template's `\{`, `\}`, `\'` and `\\` stand
//!   for those characters and consume no argument — only a bare `{}` is a
//!   placeholder;
//! * `States.ArrayRange` fails rather than returning a result longer than its
//!   published 1,000-value limit;
//! * `States.MathRandom(start, end)` draws from the HALF-OPEN range `[start, end)`,
//!   so the end bound is never a result.
//!
//! Every failure a row expects is the ASL `States.Runtime` an intrinsic invocation
//! that could not be evaluated carries, which `tests/all/terminal_runtime_errors.rs`
//! pins as terminal.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`,
//! `describe_execution`) is `state_machine_lifecycle`'s; `tests/all.rs` is this
//! binary's crate root, so it is reached through `crate::`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    MAX_ARRAY_RANGE_VALUES, MAX_HASH_CHARS,
};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// A one-state definition whose `Pass` state builds `parameters`, which is where an
/// intrinsic invocation is evaluated.
fn pass_definition(parameters: Value) -> String {
    json!({
        "StartAt": "A",
        "States": { "A": { "Type": "Pass", "Parameters": parameters, "End": true } },
    })
    .to_string()
}

/// A definition whose single built member is `invocation`, under the key `out`.
fn one_member_definition(invocation: &str) -> String {
    pass_definition(json!({ "out.$": invocation }))
}

/// Start one execution per row, wait once for the outcomes the clock owes them all,
/// and return each row's `DescribeExecution`.
async fn run_all(emulator: &Emulator, prefix: &str, rows: &[(String, String)]) -> Vec<Value> {
    let mut started = Vec::with_capacity(rows.len());
    for (index, (definition, input)) in rows.iter().enumerate() {
        let machine = create_machine(emulator, &format!("{prefix}-{index}"), definition).await;
        started.push(start_execution(emulator, &machine, "only", input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let mut described = Vec::with_capacity(started.len());
    for execution in started {
        described.push(describe_execution(emulator, &execution).await);
    }
    described
}

/// The `output` document of an execution that must have SUCCEEDED.
fn succeeded_output(case: &str, described: &Value) -> Value {
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

/// Assert that an execution FAILED with the ASL runtime error, carrying a cause that
/// names `names`.
fn failed_at_runtime(case: &str, described: &Value, names: &str) {
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some("States.Runtime")),
        "case {case:?}: expected FAILED with States.Runtime, got {described:?}"
    );
    let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
    assert!(
        cause.contains(names),
        "case {case:?}: expected the cause to name {names:?}, got {cause:?}"
    );
}

// ---------------------------------------------------------------------------
// States.Hash
// ---------------------------------------------------------------------------

/// `States.Hash` returns the hex digest of its input under each algorithm the
/// language names.
///
/// The expected digests of the rows below are the PUBLISHED test vectors of those
/// algorithms — the `"abc"` and empty-input examples of RFC 1321 (MD5), RFC 3174
/// (SHA-1) and FIPS 180-4 (SHA-256/384/512) — so the assertion is independent of
/// what this emulator computes them with. The algorithm list itself is the one
/// `States.Hash` documents ("MD5, SHA-1, SHA-256, SHA-384 and SHA-512"), and the
/// final rows vary the input instead of the algorithm: a longer input, and one at
/// the documented input bound, each hashed independently in the test.
#[tokio::test]
async fn states_hash_returns_the_published_digest_of_its_input() {
    let emulator = Emulator::new();

    // (case, the algorithm, the data, the expected lowercase hex digest)
    let mut cases: Vec<(String, &str, String, String)> = vec![
        (
            "MD5 of abc".to_string(),
            "MD5",
            "abc".to_string(),
            "900150983cd24fb0d6963f7d28e17f72".to_string(),
        ),
        (
            "SHA-1 of abc".to_string(),
            "SHA-1",
            "abc".to_string(),
            "a9993e364706816aba3e25717850c26c9cd0d89d".to_string(),
        ),
        (
            "SHA-256 of abc".to_string(),
            "SHA-256",
            "abc".to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(),
        ),
        (
            "SHA-384 of abc".to_string(),
            "SHA-384",
            "abc".to_string(),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc235\
             8baeca134c825a7"
                .to_string(),
        ),
        (
            "SHA-512 of abc".to_string(),
            "SHA-512",
            "abc".to_string(),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a83\
             6ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
                .to_string(),
        ),
        (
            "MD5 of the empty input".to_string(),
            "MD5",
            String::new(),
            "d41d8cd98f00b204e9800998ecf8427e".to_string(),
        ),
        (
            "SHA-256 of the empty input".to_string(),
            "SHA-256",
            String::new(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        ),
        (
            "SHA-256 of a multi-byte input".to_string(),
            "SHA-256",
            "héllo wörld".to_string(),
            {
                use sha2::Digest as _;
                hex::encode(sha2::Sha256::digest("héllo wörld".as_bytes()))
            },
        ),
    ];
    // At the documented input bound, which is counted in CHARACTERS.
    let at_the_bound = "a".repeat(MAX_HASH_CHARS);
    cases.push((
        format!("SHA-512 of an input of {MAX_HASH_CHARS} characters"),
        "SHA-512",
        at_the_bound.clone(),
        {
            use sha2::Digest as _;
            hex::encode(sha2::Sha512::digest(at_the_bound.as_bytes()))
        },
    ));

    let rows: Vec<(String, String)> = cases
        .iter()
        .map(|(_, algorithm, data, _)| {
            (
                one_member_definition(&format!("States.Hash($.data, '{algorithm}')")),
                json!({ "data": data }).to_string(),
            )
        })
        .collect();

    let described = run_all(&emulator, "hash", &rows).await;
    for ((case, _, _, digest), described) in cases.iter().zip(described) {
        let output = succeeded_output(case, &described);
        assert_eq!(
            output,
            json!({ "out": digest }),
            "case {case:?}: expected the digest {digest:?}, got {output}"
        );
    }
}

/// What `States.Hash` refuses: an algorithm outside the five the language names, and
/// an input past its documented character bound.
#[tokio::test]
async fn states_hash_refuses_an_unnamed_algorithm_and_an_oversized_input() {
    let emulator = Emulator::new();

    // (case, the invocation, the execution input, the text the cause must name)
    let cases: Vec<(String, String, Value, &str)> = vec![
        (
            "an algorithm the language does not name".to_string(),
            "States.Hash($.data, 'SHA-224')".to_string(),
            json!({ "data": "abc" }),
            "Invalid arguments in States.Hash",
        ),
        (
            "an algorithm whose name differs in case".to_string(),
            "States.Hash($.data, 'sha-256')".to_string(),
            json!({ "data": "abc" }),
            "Invalid arguments in States.Hash",
        ),
        (
            "data that is not a string".to_string(),
            "States.Hash($.data, 'MD5')".to_string(),
            json!({ "data": 17 }),
            "Invalid arguments in States.Hash",
        ),
        (
            format!("an input of {} characters", MAX_HASH_CHARS + 1),
            "States.Hash($.data, 'MD5')".to_string(),
            json!({ "data": "a".repeat(MAX_HASH_CHARS + 1) }),
            "more than the",
        ),
    ];

    let rows: Vec<(String, String)> = cases
        .iter()
        .map(|(_, invocation, input, _)| (one_member_definition(invocation), input.to_string()))
        .collect();

    let described = run_all(&emulator, "hash-refused", &rows).await;
    for ((case, _, _, names), described) in cases.iter().zip(described) {
        failed_at_runtime(case, &described, names);
    }
}

// ---------------------------------------------------------------------------
// States.JsonMerge
// ---------------------------------------------------------------------------

/// `States.JsonMerge` merges SHALLOWLY — a key the second document carries replaces
/// the first's whole value for it — and refuses the deep mode the language does not
/// support, which fails the execution rather than merging anyway.
///
/// Rows vary along what the merge has to resolve (disjoint keys, a colliding scalar,
/// a colliding object, an empty document) and along the third argument.
#[tokio::test]
async fn states_json_merge_merges_shallowly_and_refuses_a_deep_merge() {
    // (case, the invocation, the two documents, the merged document or None to fail)
    let cases: Vec<(&str, &str, Value, Option<Value>)> = vec![
        (
            "disjoint keys",
            "States.JsonMerge($.a, $.b, false)",
            json!({ "a": { "x": 1 }, "b": { "y": 2 } }),
            Some(json!({ "x": 1, "y": 2 })),
        ),
        (
            "a colliding scalar takes the second document's value",
            "States.JsonMerge($.a, $.b, false)",
            json!({ "a": { "x": 1 }, "b": { "x": 2 } }),
            Some(json!({ "x": 2 })),
        ),
        (
            "a colliding object is REPLACED, not merged into",
            "States.JsonMerge($.a, $.b, false)",
            json!({ "a": { "x": { "k": 1 }, "y": 1 }, "b": { "x": { "j": 2 } } }),
            Some(json!({ "x": { "j": 2 }, "y": 1 })),
        ),
        (
            "an empty second document changes nothing",
            "States.JsonMerge($.a, $.b, false)",
            json!({ "a": { "x": 1 }, "b": {} }),
            Some(json!({ "x": 1 })),
        ),
        (
            "the deep mode the language does not support",
            "States.JsonMerge($.a, $.b, true)",
            json!({ "a": { "x": { "k": 1 } }, "b": { "x": { "j": 2 } } }),
            None,
        ),
        (
            "a document that is not a JSON object",
            "States.JsonMerge($.a, $.b, false)",
            json!({ "a": { "x": 1 }, "b": 7 }),
            None,
        ),
    ];

    let rows: Vec<(String, String)> = cases
        .iter()
        .map(|(_, invocation, input, _)| (one_member_definition(invocation), input.to_string()))
        .collect();

    let emulator = Emulator::new();
    let described = run_all(&emulator, "merge", &rows).await;
    for ((case, _, _, expected), described) in cases.iter().zip(described) {
        match expected {
            Some(merged) => {
                let output = succeeded_output(case, &described);
                assert_eq!(
                    output,
                    json!({ "out": merged }),
                    "case {case:?}: expected the merge {merged}, got {output}"
                );
            }
            None => failed_at_runtime(case, &described, "States.JsonMerge"),
        }
    }
}

// ---------------------------------------------------------------------------
// States.Format
// ---------------------------------------------------------------------------

/// Only a BARE `{}` is a `States.Format` placeholder: inside the invocation's own
/// single-quoted literal, a backslash makes the character after it data.
///
/// Rows vary along which character is escaped (`{`, `}`, `'`, `\`), along how many
/// placeholders the template carries (none, one, two), along where the template
/// comes from (a literal, a nested invocation's result, a path) and along whether
/// the argument count matches the placeholder count.
#[tokio::test]
async fn states_format_treats_only_bare_braces_as_placeholders() {
    // (case, the invocation, the rendered string, or None when the row must fail)
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "escaped braces are literal and consume no argument",
            r"States.Format('x\{\} {} \'q\' {}', $.s, $.n)",
            Some("x{} hi 'q' 7"),
        ),
        (
            "bare placeholders take the arguments in order",
            r"States.Format('{} and {}', $.s, $.n)",
            Some("hi and 7"),
        ),
        (
            "a template with no placeholder takes no argument",
            r"States.Format('plain text')",
            Some("plain text"),
        ),
        (
            "a template of nothing but escaped braces",
            r"States.Format('\{\}')",
            Some("{}"),
        ),
        (
            "an escaped backslash is one backslash",
            r"States.Format('a\\b {}', $.s)",
            Some(r"a\b hi"),
        ),
        (
            "an escaped quote is a quote",
            r"States.Format('\'{}\'', $.s)",
            Some("'hi'"),
        ),
        (
            "a nested invocation's escaped braces are applied once",
            r"States.Format('{}', States.Format('\{\}'))",
            Some("{}"),
        ),
        (
            "a template read from the input renders its bare placeholders",
            r"States.Format($.template, $.n)",
            Some("count: 7"),
        ),
        (
            "fewer arguments than placeholders fails",
            r"States.Format('{} {}', $.s)",
            None,
        ),
        (
            "a placeholder whose escaped twin supplied no argument still needs one",
            r"States.Format('\{\} {}')",
            None,
        ),
    ];

    let input = json!({ "s": "hi", "n": 7, "template": "count: {}" }).to_string();
    let rows: Vec<(String, String)> = cases
        .iter()
        .map(|(_, invocation, _)| (one_member_definition(invocation), input.clone()))
        .collect();

    let emulator = Emulator::new();
    let described = run_all(&emulator, "format", &rows).await;
    for ((case, _, expected), described) in cases.iter().zip(described) {
        match expected {
            Some(rendered) => {
                let output = succeeded_output(case, &described);
                assert_eq!(
                    output,
                    json!({ "out": rendered }),
                    "case {case:?}: expected {rendered:?}, got {output}"
                );
            }
            None => failed_at_runtime(case, &described, "States.Format"),
        }
    }
}

// ---------------------------------------------------------------------------
// States.ArrayRange
// ---------------------------------------------------------------------------

/// The result one `States.ArrayRange` row must build.
struct Built {
    length: usize,
    /// The first value of the built array.
    first: i64,
    /// Its last value.
    last: i64,
}

/// One row of the `States.ArrayRange` table: the invocation, and what it must build
/// — `None` where the execution must fail instead.
struct RangeCase {
    case: String,
    invocation: String,
    expected: Option<Built>,
}

/// `States.ArrayRange` builds up to its published limit of values and FAILS the
/// execution past it: the limit is on how many values the RESULT carries, whatever
/// the bounds and the step that produced them.
///
/// Rows sit either side of that bound, over a positive and a negative step, plus a
/// small range well inside it.
#[tokio::test]
async fn states_array_range_fails_past_its_published_limit() {
    let limit = i64::try_from(MAX_ARRAY_RANGE_VALUES).expect("the limit fits");

    let cases = [
        RangeCase {
            case: "a small range".to_string(),
            invocation: "States.ArrayRange(1, 5, 1)".to_string(),
            expected: Some(Built {
                length: 5,
                first: 1,
                last: 5,
            }),
        },
        RangeCase {
            case: format!("a range of exactly {MAX_ARRAY_RANGE_VALUES} values"),
            invocation: format!("States.ArrayRange(1, {limit}, 1)"),
            expected: Some(Built {
                length: MAX_ARRAY_RANGE_VALUES,
                first: 1,
                last: limit,
            }),
        },
        RangeCase {
            case: format!("a range of {} values", MAX_ARRAY_RANGE_VALUES + 1),
            invocation: format!("States.ArrayRange(1, {}, 1)", limit + 1),
            expected: None,
        },
        RangeCase {
            case: "a step-2 range at the limit".to_string(),
            invocation: format!("States.ArrayRange(0, {}, 2)", (limit - 1) * 2),
            expected: Some(Built {
                length: MAX_ARRAY_RANGE_VALUES,
                first: 0,
                last: (limit - 1) * 2,
            }),
        },
        RangeCase {
            case: "a step-2 range one value past the limit".to_string(),
            invocation: format!("States.ArrayRange(0, {}, 2)", limit * 2),
            expected: None,
        },
        RangeCase {
            case: "a descending range at the limit".to_string(),
            invocation: format!("States.ArrayRange({limit}, 1, -1)"),
            expected: Some(Built {
                length: MAX_ARRAY_RANGE_VALUES,
                first: limit,
                last: 1,
            }),
        },
        RangeCase {
            case: "a descending range one value past the limit".to_string(),
            invocation: format!("States.ArrayRange({}, 1, -1)", limit + 1),
            expected: None,
        },
    ];

    let rows: Vec<(String, String)> = cases
        .iter()
        .map(|row| (one_member_definition(&row.invocation), "{}".to_string()))
        .collect();

    let emulator = Emulator::new();
    let described = run_all(&emulator, "range", &rows).await;
    for (row, described) in cases.iter().zip(described) {
        let case = &row.case;
        match &row.expected {
            Some(Built {
                length,
                first,
                last,
            }) => {
                let output = succeeded_output(case, &described);
                let values = output
                    .get("out")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("case {case:?}: expected an array, got {output}"));
                assert_eq!(
                    (
                        values.len(),
                        values.first().and_then(Value::as_i64),
                        values.last().and_then(Value::as_i64),
                    ),
                    (*length, Some(*first), Some(*last)),
                    "case {case:?}: expected {length} values from {first} to {last}, got {} \
                     values {:?}..{:?}",
                    values.len(),
                    values.first(),
                    values.last()
                );
            }
            None => failed_at_runtime(case, &described, "States.ArrayRange"),
        }
    }
}

// ---------------------------------------------------------------------------
// States.MathRandom
// ---------------------------------------------------------------------------

/// How many draws one execution of the table below makes.
const DRAWS_PER_EXECUTION: usize = 20;

/// How many executions each row of the table below runs, so that every row's draws
/// are made at distinct instants rather than in one evaluation.
const EXECUTIONS_PER_ROW: usize = 3;

/// `States.MathRandom(start, end)` draws from the HALF-OPEN range `[start, end)`: the
/// end bound is never a result, so a draw used to index an array of `end - start`
/// items is always in bounds.
///
/// Rows vary along the width of the range (one value, two, ten), along its sign, and
/// along whether the invocation supplies the optional seed.
#[tokio::test]
async fn states_math_random_draws_from_a_half_open_range() {
    // (case, the invocation, the inclusive start, the exclusive end)
    let cases: Vec<(&str, &str, i64, i64)> = vec![
        ("a range of two values", "States.MathRandom(1, 3)", 1, 3),
        (
            "a range of exactly one value",
            "States.MathRandom(7, 8)",
            7,
            8,
        ),
        ("a range of ten values", "States.MathRandom(0, 10)", 0, 10),
        (
            "a range of negative values",
            "States.MathRandom(-5, -1)",
            -5,
            -1,
        ),
        (
            "a seeded range",
            "States.MathRandom(1, 3, 42)",
            1,
            3,
        ),
    ];

    // One execution draws many times, so the exclusive bound is tested over a sample
    // rather than over a single draw.
    let rows: Vec<(String, String)> = cases
        .iter()
        .flat_map(|(_, invocation, _, _)| {
            let members: serde_json::Map<String, Value> = (0..DRAWS_PER_EXECUTION)
                .map(|index| (format!("d{index}.$"), Value::String((*invocation).to_string())))
                .collect();
            let definition = pass_definition(Value::Object(members));
            (0..EXECUTIONS_PER_ROW).map(move |_| (definition.clone(), "{}".to_string()))
        })
        .collect();

    let emulator = Emulator::new();
    let described = run_all(&emulator, "random", &rows).await;
    for (index, (case, _, start, end)) in cases.iter().enumerate() {
        let mut drawn: Vec<i64> = Vec::with_capacity(DRAWS_PER_EXECUTION * EXECUTIONS_PER_ROW);
        for run in 0..EXECUTIONS_PER_ROW {
            let output = succeeded_output(case, &described[index * EXECUTIONS_PER_ROW + run]);
            for member in 0..DRAWS_PER_EXECUTION {
                let value = output
                    .get(format!("d{member}"))
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a whole-number draw in {output}")
                    });
                drawn.push(value);
            }
        }
        assert!(
            drawn.iter().all(|value| value >= start && value < end),
            "case {case:?}: expected every draw in [{start}, {end}), got {drawn:?}"
        );
    }
}
