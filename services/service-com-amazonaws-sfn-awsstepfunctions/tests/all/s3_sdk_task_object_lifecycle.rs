// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the S3 calls a `Task` state's AWS SDK integration MAKES, and
//! for what a diagnosis about such a task's result NAMES it by. Driven over both
//! services' own wire protocols through the production `MultiServiceRouter` with Step
//! Functions, S3 and IAM mounted on ONE engine.
//!
//! The rules under test:
//!
//! 1. an `arn:aws:states:::aws-sdk:s3:<action>` task is CALLED for every operation
//!    whose answer this emulator reads as that operation's own output members, which
//!    includes an operation that answers NO BODY at all — a HEAD, a write, a delete —
//!    whose output members are the headers its answer carried; so one state machine
//!    writes, heads, reads, deletes and reads again, each state's result being the
//!    answer S3's own handler gave;
//! 2. what a state wrote for a payload member is a JSON VALUE of the document the
//!    state built, so the bytes the invoked operation receives for it are that value's
//!    own TEXT — a string arrives as the JSON string literal it is in that document —
//!    and the object S3 then holds is read back as exactly those bytes;
//! 3. a result larger than a state's output may carry fails the execution
//!    `States.DataLimitExceeded` with the service's quota sentence naming the task's
//!    work as this state's own history events name it: the KIND of resource the call
//!    was made on (`aws-sdk:<service>`), not the whole `Resource` ARN and not the
//!    state's name.
//!
//! Each table's rows differ along what its rule generalises over: which operation the
//! `Resource` names and whether the object is there (rule 1), what kind of JSON value
//! the payload member carries (rule 2), and which service the oversize result came
//! from (rule 3).
//!
//! The harness is `crate::map_result_writer`'s `Chain`, as
//! `crate::s3_sdk_task_integration` uses it: `tests/all.rs` is this binary's crate
//! root, so this suite drives the same three services through it.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{DATA_LIMIT_ERROR, MAX_PAYLOAD_BYTES};

use crate::map_result_writer::{every_action, Chain, BUCKET};

/// What the service states about a result larger than a state's output may carry:
/// RECORDED text, with the work that produced it named in it.
fn too_large(produced_by: &str) -> String {
    format!(
        "The state/task '{produced_by}' returned a result with a size exceeding the maximum \
         number of bytes service limit."
    )
}

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

/// The `output` of one described execution, as the document it holds.
fn output_of(described: &Value, case: &str) -> Value {
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| {
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("case {case:?}: the output is JSON: {error}"))
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1: every call whose answer is read as that operation's output is made
// ---------------------------------------------------------------------------

/// One row of the per-operation table: the case, the action the `Resource` names, its
/// parameters, and the output members the state's result must carry.
type CallRow = (&'static str, &'static str, Value, &'static [&'static str]);

/// Each S3 operation the AWS SDK integration calls answers that operation's own output
/// members, read off the answer S3's own handler gave.
///
/// The rows differ along which operation the `Resource` names — a write, a HEAD of a
/// bucket, a read, a delete — and, for the delete, whether the object was there at all:
/// `DeleteObject` "removes an object" and answers the same 204 either way, so the state
/// succeeds for both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_sdk_s3_call_answers_that_operations_own_output() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkLifecycleRole", &every_action()).await;
    chain.object("lifecycle/held", "held").await;

    // The members named are the ones the invoked operation binds to its answer's
    // headers or payload (`smithy_query(query="operation",operation="PutObject,
    // HeadBucket,GetObject,DeleteObject",model_path="services/
    // service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`),
    // and that S3's own handler answers for a general purpose bucket in one account.
    let rows: &[CallRow] = &[
        (
            "a write",
            "putObject",
            json!({ "Bucket": BUCKET, "Key": "lifecycle/written", "Body": "stored" }),
            &["ETag"],
        ),
        (
            "a HEAD of the bucket",
            "headBucket",
            json!({ "Bucket": BUCKET }),
            &["BucketRegion", "BucketArn"],
        ),
        (
            "a read",
            "getObject",
            json!({ "Bucket": BUCKET, "Key": "lifecycle/held" }),
            &["Body", "ETag"],
        ),
        (
            "a delete of an object that is there",
            "deleteObject",
            json!({ "Bucket": BUCKET, "Key": "lifecycle/held" }),
            &[],
        ),
        (
            "a delete of an object that is not",
            "deleteObject",
            json!({ "Bucket": BUCKET, "Key": "lifecycle/absent" }),
            &[],
        ),
    ];

    for (index, (case, action, parameters, members)) in rows.iter().enumerate() {
        let definition = calling(
            &format!("arn:aws:states:::aws-sdk:s3:{action}"),
            parameters.clone(),
        );
        let machine = chain
            .machine(&format!("sdk-lifecycle-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let output = output_of(&described, case);
        for member in *members {
            assert!(
                output.get(*member).is_some(),
                "case {case:?}: the result carries the answer's {member:?}, got {output:?}"
            );
        }
        // An operation whose answer carried no output member at all reports none: a
        // delete of an unversioned object carries neither a version nor a charge, and
        // nothing is invented for either.
        if members.is_empty() {
            assert_eq!(
                output,
                json!({}),
                "case {case:?}: expected the answer's own members and no others, got {output:?}"
            );
        }
    }
}

/// One state machine writes an object, heads its bucket, reads it, deletes it and reads
/// it again, the last read being caught: every state's result is the answer S3's own
/// handler gave, and the object is really gone afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_s3_round_trip_runs_every_call_and_catches_the_read_after_the_delete() {
    /// The object this round trip writes, reads, deletes and reads again.
    const KEY: &str = "round-trip/object";

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkRoundTripRole", &every_action()).await;
    let definition = json!({
        "StartAt": "Put",
        "States": {
            "Put": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:putObject",
                "Parameters": { "Bucket": BUCKET, "Key": KEY, "Body.$": "$.body" },
                "ResultPath": "$.put",
                "Next": "Head",
            },
            "Head": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:headBucket",
                "Parameters": { "Bucket": BUCKET },
                "ResultPath": "$.head",
                "Next": "Get",
            },
            "Get": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:getObject",
                "Parameters": { "Bucket": BUCKET, "Key": KEY },
                "ResultSelector": { "content.$": "$.Body" },
                "ResultPath": "$.get",
                "Next": "Delete",
            },
            "Delete": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:deleteObject",
                "Parameters": { "Bucket": BUCKET, "Key": KEY },
                "ResultPath": "$.delete",
                "Next": "GetAgain",
            },
            "GetAgain": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:s3:getObject",
                "Parameters": { "Bucket": BUCKET, "Key": KEY },
                "ResultPath": "$.gone",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "ResultPath": "$.caught",
                    "Next": "Done",
                }],
                "End": true,
            },
            "Done": { "Type": "Pass", "End": true },
        },
    });
    let machine = chain.machine("sdk-round-trip", &definition, &role_arn).await;
    let case = "a write, a head, a read, a delete and the read after it";
    let (_execution, described) = chain
        .ran(&machine, &json!({ "body": { "id": 7 } }), case)
        .await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    let output = output_of(&described, case);
    // The read before the delete answers the bytes the write stored, which are the JSON
    // text of the value the state wrote for the payload member.
    assert_eq!(
        output.pointer("/get/content").and_then(Value::as_str),
        Some("{\"id\":7}"),
        "case {case:?}: the read answers the object the write stored, got {output:?}"
    );
    assert!(
        output.pointer("/head/BucketRegion").is_some(),
        "case {case:?}: the HEAD answers the bucket's own Region, got {output:?}"
    );
    assert!(
        output.pointer("/put/ETag").is_some(),
        "case {case:?}: the write answers the entity tag its answer carried, got {output:?}"
    );
    // The read AFTER the delete is the error S3's own client raises for an object that
    // is not there, which the state's `Catch` answered for.
    let caught = output
        .pointer("/caught/Error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a caught error in {output:?}"));
    assert_eq!(
        caught, "S3.NoSuchKeyException",
        "case {case:?}: the read after the delete is refused by S3 itself, got {output:?}"
    );
    assert!(
        output.pointer("/gone").is_none(),
        "case {case:?}: the state that failed placed no result, got {output:?}"
    );
    assert_eq!(
        chain.stored(KEY).await,
        None,
        "case {case:?}: the delete really removed the object"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a payload member carries the TEXT of the JSON value the state wrote
// ---------------------------------------------------------------------------

/// The object a write stores holds the TEXT of the JSON value the state wrote for the
/// payload member, and a read answers exactly those bytes.
///
/// The rows differ along what kind of JSON value the member carries: a string, a
/// document, an array and a number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_payload_member_carries_the_text_of_the_value_the_state_wrote() {
    // (case, the value the state writes for `Body`, the text the object then holds)
    let rows: &[(&str, Value, &str)] = &[
        ("a string", json!("plain text"), "\"plain text\""),
        (
            "a string that is itself a document's text",
            json!("{\"id\": 7}"),
            "\"{\\\"id\\\": 7}\"",
        ),
        (
            "a document",
            json!({ "id": 7, "ok": true }),
            "{\"id\":7,\"ok\":true}",
        ),
        ("an array", json!([1, "two"]), "[1,\"two\"]"),
        ("a number", json!(7), "7"),
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkPayloadRole", &every_action()).await;
    for (index, (case, body, expected)) in rows.iter().enumerate() {
        let key = format!("payload/written-{index}");
        let definition = json!({
            "StartAt": "Put",
            "States": {
                "Put": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:s3:putObject",
                    "Parameters": { "Bucket": BUCKET, "Key": key, "Body.$": "$.body" },
                    "Next": "Get",
                },
                "Get": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:s3:getObject",
                    "Parameters": { "Bucket": BUCKET, "Key": key },
                    "End": true,
                },
            },
        });
        let machine = chain
            .machine(&format!("sdk-payload-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({ "body": body }), case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        // What S3 holds, read through S3's own `GetObject` rather than through the task.
        assert_eq!(
            chain.stored(&key).await.as_deref(),
            Some(*expected),
            "case {case:?}: the object holds the text of the value the state wrote"
        );
        // And the task's own read answers those same bytes as the text they are.
        let output = output_of(&described, case);
        assert_eq!(
            output.get("Body").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: the read answers the object's bytes as text, got {output:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an oversize task result names the task's work by its resource KIND
// ---------------------------------------------------------------------------

/// A `Task` state's result past what a state's output may carry fails the execution
/// with the quota sentence naming the KIND of resource the call was made on, which is
/// what that state's own history events name its work by.
///
/// The rows differ along which service the oversize result came from, so the name is
/// read off the `Resource` rather than written for one service: an object S3 answers,
/// and a state machine's own definition Step Functions answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversize_task_result_names_the_kind_of_resource_it_came_from() {
    // A body past the bound a state's output carries, by enough that no envelope
    // around it can bring the document back inside.
    let oversize = "p".repeat(MAX_PAYLOAD_BYTES + 1024);

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SdkOversizeRole", &every_action()).await;
    chain.object("oversize/object", &oversize).await;
    // A state machine whose own definition is past the bound, so DESCRIBING it is a
    // result past it too. Its `Comment` is where the language lets a definition carry
    // text of any length.
    let described_machine = chain
        .machine(
            "oversize-definition",
            &json!({
                "Comment": oversize,
                "StartAt": "Done",
                "States": { "Done": { "Type": "Succeed" } },
            }),
            &role_arn,
        )
        .await;

    // (case, the Resource, its parameters, the work the cause must name)
    let rows: &[(&str, &str, Value, &str)] = &[
        (
            "an object S3 answers",
            "arn:aws:states:::aws-sdk:s3:getObject",
            json!({ "Bucket": BUCKET, "Key": "oversize/object" }),
            "aws-sdk:s3",
        ),
        (
            "a definition Step Functions answers",
            "arn:aws:states:::aws-sdk:sfn:describeStateMachine",
            json!({ "StateMachineArn": described_machine }),
            "aws-sdk:sfn",
        ),
    ];

    for (index, (case, resource, parameters, produced_by)) in rows.iter().enumerate() {
        let definition = calling(resource, parameters.clone());
        let machine = chain
            .machine(&format!("sdk-oversize-{index}"), &definition, &role_arn)
            .await;
        let (_execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
                described.get("cause").and_then(Value::as_str),
            ),
            (
                Some("FAILED"),
                Some(DATA_LIMIT_ERROR),
                Some(too_large(produced_by).as_str()),
            ),
            "case {case:?}: expected the quota sentence naming {produced_by:?}, got {described:?}"
        );
        assert!(
            described.get("output").is_none(),
            "case {case:?}: an execution that failed puts out nothing, got {described:?}"
        );
    }
}
