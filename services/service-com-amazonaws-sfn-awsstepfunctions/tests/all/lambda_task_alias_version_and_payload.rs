// SPDX-License-Identifier: CC0-1.0

//! What the OPTIMIZED Lambda integration (`arn:aws:states:::lambda:invoke`) reports
//! about the invocation it made, and which `Parameters` can name one at all, driven over
//! both services' own wire protocols through the production `MultiServiceRouter` with
//! Step Functions and Lambda mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in, reached through [`crate::lambda_task_control_flow::Chain`] so there is
//! one setup of the two services in this target — so the invocation a task makes is
//! really served by the function this suite created.
//!
//! RULE 1: a task's SDK response envelope reports each header the answer carried under
//! the field name the invoked operation BINDS it to, so `ExecutedVersion` is readable
//! both as the operation's own member and as `X-Amz-Executed-Version` — and its value is
//! the version the invocation resolved to, which for an alias-qualified invoke is the
//! numeric version that alias points at rather than the unpublished one.
//!
//! RULE 2: the `Payload` parameter of a Lambda task carries the JSON document the
//! function is invoked with, so parameters whose `Payload` is a bare JSON scalar name no
//! invocation the call can be built from: the task fails on its own parameters, with the
//! Amazon States Language's runtime error and the diagnosis the service words for
//! parameters it could not start a task with.
//!
//! The rows differ along what each rule generalises over: for the first, every
//! documented form a request names a qualified function in (the alias in the name, in
//! the ARN, as the `Qualifier`, the published version itself, and no qualifier at all);
//! for the second, the JSON type the `Payload` parameter is given, whether a path or the
//! definition itself gave it, the null document, and parameters that name no `Payload`
//! at all.

use serde_json::{json, Value};

use crate::lambda_task_control_flow::Chain;

/// The `Resource` of the OPTIMIZED Lambda integration: one of this service's own
/// integration tokens, whose request is the `Parameters` the state builds.
const OPTIMIZED_RESOURCE: &str = "arn:aws:states:::lambda:invoke";

/// The error the Amazon States Language reports for a state whose own expression or
/// parameters could not be used at run time.
const RUNTIME_ERROR: &str = "States.Runtime";

/// The alias every row below invokes the function through.
const ALIAS: &str = "live";

/// The handler each row invokes: it echoes the event it was handed, so what a usable
/// `Payload` resolved to is readable in the task's own result.
const ECHO_SOURCE: &str = r#"
def handler(event, context):
    return {"echo": event}
"#;

/// A one-`Task` definition whose work is the optimized Lambda integration with
/// `parameters`.
fn invoke_definition(parameters: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": {
                "Type": "Task",
                "Resource": OPTIMIZED_RESOURCE,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
}

/// The document an execution's `output` carries, for a row that reads the task's result.
fn output_of(described: &Value) -> Value {
    serde_json::from_str(
        described
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("null"),
    )
    .unwrap_or_else(|error| panic!("the output is a JSON document, got {error} in {described}"))
}

/// The `ExecutedVersion` the envelope reports, and the same value under the response
/// header the operation binds that member to.
#[tokio::test]
async fn an_alias_qualified_invoke_reports_the_version_the_alias_resolved_to() {
    let chain = Chain::new();
    let name = "echo";
    let arn = chain.given_function_of(name, ECHO_SOURCE, None).await;
    let version = chain.published(name).await;
    chain.aliased(name, ALIAS, &version).await;

    // Each row names the SAME function through a different documented form of
    // `FunctionName`/`Qualifier`, with the version that form resolves to.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the alias in the name",
            json!({ "FunctionName": format!("{name}:{ALIAS}"), "Payload.$": "$" }),
            version.clone(),
        ),
        (
            "the alias in the ARN",
            json!({ "FunctionName": format!("{arn}:{ALIAS}"), "Payload.$": "$" }),
            version.clone(),
        ),
        (
            "the alias as the Qualifier",
            json!({ "FunctionName": name, "Qualifier": ALIAS, "Payload.$": "$" }),
            version.clone(),
        ),
        (
            "the published version itself",
            json!({ "FunctionName": format!("{name}:{version}"), "Payload.$": "$" }),
            version.clone(),
        ),
        (
            "no qualifier at all",
            json!({ "FunctionName": name, "Payload.$": "$" }),
            "$LATEST".to_string(),
        ),
    ];

    for (index, (case, parameters, expected)) in rows.into_iter().enumerate() {
        let described = chain
            .ran(
                case,
                &format!("alias-{index}"),
                &invoke_definition(parameters),
                &json!({ "n": index }),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: the invocation must succeed, got {described}"
        );
        let output = output_of(&described);
        assert_eq!(
            output.get("ExecutedVersion").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected ExecutedVersion {expected:?}, got {output}"
        );
        assert_eq!(
            output
                .pointer("/SdkHttpMetadata/HttpHeaders/X-Amz-Executed-Version")
                .and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the X-Amz-Executed-Version header {expected:?}, got {output}"
        );
    }
}

/// The `Payload` parameter of a Lambda task carries the JSON DOCUMENT the function is
/// invoked with, and only an object or an array is one.
#[tokio::test]
async fn a_payload_parameter_that_is_not_a_document_fails_the_task() {
    let chain = Chain::new();
    let name = "scalars";
    chain.given_function_of(name, ECHO_SOURCE, None).await;

    // Most rows resolve `Payload` out of the execution input, so the rows differ along
    // the JSON type that parameter is given; a row whose type a path cannot reach writes
    // the parameter as a literal instead. The input is a JSON text of its own, which is
    // what `StartExecution` takes.
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        ("an object", json!({ "Payload.$": "$" }), r#"{"n": 1}"#, true),
        ("an array", json!({ "Payload.$": "$" }), "[1, 2, 3]", true),
        (
            "a nested array",
            json!({ "Payload.$": "$" }),
            r#"[["a"], {"b": 2}]"#,
            true,
        ),
        ("a bare number", json!({ "Payload.$": "$" }), "42", false),
        (
            "a bare negative number",
            json!({ "Payload.$": "$" }),
            "-1.5",
            false,
        ),
        (
            "a bare string",
            json!({ "Payload.$": "$" }),
            r#""text""#,
            false,
        ),
        ("a bare boolean", json!({ "Payload.$": "$" }), "true", false),
        (
            "a literal object the definition wrote",
            json!({ "Payload": { "fixed": true } }),
            "42",
            true,
        ),
        (
            "a literal scalar the definition wrote",
            json!({ "Payload": 7 }),
            r#"{"n": 1}"#,
            false,
        ),
        (
            // The null document names no value, which is what a request carrying no
            // payload member at all names: a function invoked with no event.
            "the null document the definition wrote",
            json!({ "Payload": Value::Null }),
            r#"{"n": 1}"#,
            true,
        ),
        (
            "no Payload parameter at all",
            json!({}),
            r#"{"n": 1}"#,
            true,
        ),
    ];

    for (index, (case, payload, input, usable)) in rows.into_iter().enumerate() {
        let mut parameters = json!({ "FunctionName": name });
        for (member, value) in payload.as_object().expect("each row names parameters") {
            parameters[member] = value.clone();
        }
        let started: Value =
            serde_json::from_str(input).expect("each row's input is a JSON text");
        let described = chain
            .ran(
                case,
                &format!("payload-{index}"),
                &invoke_definition(parameters),
                &started,
            )
            .await;
        let status = described.get("status").and_then(Value::as_str);
        if usable {
            assert_eq!(
                status,
                Some("SUCCEEDED"),
                "case {case:?}: the invocation must be made, got {described}"
            );
            continue;
        }
        assert_eq!(
            status,
            Some("FAILED"),
            "case {case:?}: these parameters name no invocation, got {described}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(RUNTIME_ERROR),
            "case {case:?}: expected the runtime error, got {described}"
        );
        let cause = described
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // The state, the parameter that could not be used and the value it was given are
        // each the definition's or the execution's own, so each is named.
        for named in ["Work", "Payload", "could not be used to start the Task"] {
            assert!(
                cause.contains(named),
                "case {case:?}: the cause must name {named:?}, got {cause:?}"
            );
        }
    }

    // What a usable `Payload` resolved to reaches the function UNCHANGED: the handler
    // echoes the event it was handed, so the task's own result carries it back.
    let input = json!([1, { "deep": [true, null] }]);
    let described = chain
        .ran(
            "a document the path selected whole",
            "unchanged",
            &invoke_definition(json!({ "FunctionName": name, "Payload.$": "$" })),
            &input,
        )
        .await;
    let output = output_of(&described);
    assert_eq!(
        output.pointer("/Payload/echo"),
        Some(&input),
        "case \"a document the path selected whole\": expected {input} to reach the function \
         unchanged, got {output}"
    );
}
