// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `Task` state whose `Resource` names an S3 operation through
//! the AWS SDK integration family, driven over both services' own wire protocols
//! through the production `MultiServiceRouter` with Step Functions, S3 and IAM mounted
//! on ONE engine.
//!
//! The rule under test: `arn:aws:states:::aws-sdk:s3:<action>` is CALLED on S3's own
//! handler, and the state's result is that operation's own output — an object's content
//! arrives as the TEXT its bytes are, under the `Body` member the operation binds its
//! payload to, so a state reads a JSON object with `States.StringToJson` exactly as it
//! does on the real service — while a refusal is reported as the error that service's
//! own client raises. An operation whose answer this emulator does not read as that
//! operation's output members is not run, and nothing is synthesised for it.
//!
//! The rows span what the rule generalises over: what the object holds (a JSON
//! document, plain text, an empty object), which operation the `Resource` names (a
//! read, a write, one that is not read as output members) and whether the object is
//! there at all.
//!
//! The harness is `crate::map_result_writer`'s: `tests/all.rs` is this binary's crate
//! root, so this suite drives the same three services through it.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;

use crate::map_result_writer::{every_action, Chain, BUCKET};

/// A definition whose one `Task` state makes `resource`'s call with `parameters` and
/// ends with whatever it answered.
fn calling(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
}

/// One row of the read table: what the object holds, and what the state's result must
/// carry under `Body`.
type ReadRow = (&'static str, &'static str, &'static str);

/// An AWS SDK `s3:getObject` task answers the object's own bytes as the TEXT of its
/// `Body` member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_sdk_s3_read_answers_the_objects_bytes_as_text() {
    // (case, what the object holds, the text the result's `Body` must carry)
    let rows: &[ReadRow] = &[
        (
            "a JSON document",
            "{\"id\": 7, \"ok\": true}",
            "{\"id\": 7, \"ok\": true}",
        ),
        ("a JSON array", "[1, 2, 3]", "[1, 2, 3]"),
        ("plain text", "not json at all", "not json at all"),
        (
            "a document with its own spacing kept",
            "{\n  \"id\": 7\n}\n",
            "{\n  \"id\": 7\n}\n",
        ),
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkReadRole", &every_action()).await;
    for (index, (case, held, expected)) in rows.iter().enumerate() {
        let key = format!("sdk/read-{index}");
        chain.object(&key, held).await;
        let definition = calling(
            "arn:aws:states:::aws-sdk:s3:getObject",
            json!({ "Bucket": BUCKET, "Key": key }),
        );
        let machine = chain
            .machine(&format!("sdk-read-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let output: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
        assert_eq!(
            output.get("Body").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: the result carries the object's bytes as text, got {output:?}"
        );
        assert!(
            output.get("ETag").and_then(Value::as_str).is_some(),
            "case {case:?}: the result carries the output members the answer's headers \
             carried, got {output:?}"
        );
    }
}

/// A state that reads a JSON object can parse it with `States.StringToJson`, which is
/// what the `Body` member being a STRING is for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_object_parses_with_string_to_json() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkParseRole", &every_action()).await;
    chain
        .object("sdk/parsed.json", "{\"id\": 7, \"ok\": true}")
        .await;
    let definition = json!({
        "StartAt": "Read",
        "States": {
            "Read": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:getObject",
                "Parameters": { "Bucket": BUCKET, "Key": "sdk/parsed.json" },
                "ResultSelector": { "body.$": "$.Body" },
                "ResultPath": "$.fetched",
                "Next": "Parse",
            },
            "Parse": {
                "Type": "Pass",
                "Parameters": { "item.$": "States.StringToJson($.fetched.body)" },
                "End": true,
            },
        },
    });
    let machine = chain.machine("sdk-parse", &definition, &role_arn).await;
    let (_execution, described) = chain
        .ran(&machine, &json!({}), "a JSON object read and parsed")
        .await;
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("expected an output in {described:?}"));
    assert_eq!(
        output,
        json!({ "item": { "id": 7, "ok": true } }),
        "the object's text parses to the document it holds, got {output:?}"
    );
}

/// An AWS SDK `s3:putObject` task really stores the object, and the state's result
/// carries the output members the answer carried.
///
/// What the state wrote for `Body` is a JSON VALUE of the document the state built, so
/// the object holds that value's own TEXT: a string is stored as the JSON string
/// literal it is in that document, and a document as its own compact text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_sdk_s3_write_stores_the_object() {
    // (case, the body the task writes, the text the object then holds)
    let rows: &[(&str, Value, &str)] = &[
        ("text", json!("just text"), "\"just text\""),
        ("a document", json!({ "id": 7 }), "{\"id\":7}"),
        ("an array", json!([1, 2]), "[1,2]"),
        ("a number", json!(7), "7"),
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkWriteRole", &every_action()).await;
    for (index, (case, body, expected)) in rows.iter().enumerate() {
        let key = format!("sdk/written-{index}");
        let definition = calling(
            "arn:aws:states:::aws-sdk:s3:putObject",
            json!({ "Bucket": BUCKET, "Key": key, "Body": body }),
        );
        let machine = chain
            .machine(&format!("sdk-write-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        assert_eq!(
            chain.stored(&key).await.as_deref(),
            Some(*expected),
            "case {case:?}: the object S3 holds is the one the task wrote"
        );
    }
}

/// What an S3 call this emulator does not serve as a `Task` state's work reports.
///
/// The rows differ along why: an object that is not there, which S3 itself refuses, and
/// an operation whose restXml answer is not read as that operation's output members,
/// which is not run at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_s3_call_that_cannot_be_served_says_so() {
    // (case, the Resource, its parameters, the error the state must report)
    let rows: &[(&str, &str, Value, &str)] = &[
        (
            "an object that is not there",
            "arn:aws:states:::aws-sdk:s3:getObject",
            json!({ "Bucket": BUCKET, "Key": "sdk/absent" }),
            "S3.NoSuchKeyException",
        ),
        (
            "a listing, whose answer is not read as output members",
            "arn:aws:states:::aws-sdk:s3:listObjectsV2",
            json!({ "Bucket": BUCKET }),
            RUNTIME_ERROR,
        ),
        (
            "the optimized family, which S3 publishes no integration for",
            "arn:aws:states:::s3:getObject",
            json!({ "Bucket": BUCKET, "Key": "sdk/absent" }),
            RUNTIME_ERROR,
        ),
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkRefusalRole", &every_action()).await;
    for (index, (case, resource, parameters, expected)) in rows.iter().enumerate() {
        let definition = calling(resource, parameters.clone());
        let machine = chain
            .machine(&format!("sdk-refused-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(*expected)),
            "case {case:?}: expected {expected}, got {described:?}"
        );
        assert!(
            described
                .get("cause")
                .and_then(Value::as_str)
                .is_some_and(|cause| !cause.is_empty()),
            "case {case:?}: the failure says what happened, got {described:?}"
        );
    }
}
