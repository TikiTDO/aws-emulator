// SPDX-License-Identifier: CC0-1.0

//! Every response this service sends names the request it answered, and each HTTP ATTEMPT
//! is a request of its own.
//!
//! RULE: a response carries the header this protocol reports the request id in
//! (`x-amzn-RequestId` for `awsJson1_0`, matched case-insensitively), freshly minted per
//! answer, on a success exactly as on a failure, and the VALUE is spelled the way THIS
//! service spells its request ids: one unbroken fixed-width run of uppercase letters and
//! digits, which is what every sample response in the DynamoDB API reference shows — not
//! the hyphenated 8-4-4-4-12 UUID most AWS services report. It is a property of the
//! RESPONSE rather than of any modelled shape — no output and no error shape of this model
//! declares it — and it is what an SDK exposes as the call's `ResponseMetadata.RequestId`
//! and `ServiceError::request_id()`. Two deliveries of ONE call (what an SDK retry sends)
//! are two requests, so they are answered with two DIFFERENT ids: the id is what tells a
//! replay from a first attempt in a server-side record.
//!
//! What the rule does not change, and this suite pins too: the status, the error code and
//! every byte of the body stay exactly what they were — the id travels in the header alone.
//!
//! The rows span what the rule generalises over: which operation answered, whether the
//! answer is a success or a failure, and which LAYER produced it — a handler's success, a
//! handler's modelled error, the constraint walk ahead of the handler, and the router's own
//! routing failure for a request that names no modelled operation.

use std::collections::BTreeSet;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{describe_when_active, emulator, ok, REGION};

/// The header AWS names the request a response answered in. Written out here rather than
/// read from the crate, so the assertion does not merely compare the implementation with
/// itself.
const REQUEST_ID_HEADER: &str = "x-amzn-RequestId";

/// The width of one of this service's request ids, and the alphabet it is written in —
/// read off the `x-amzn-RequestId` of the sample responses in the DynamoDB API reference
/// and written out here, so the assertion does not merely compare the implementation with
/// itself.
const REQUEST_ID_WIDTH: usize = 52;

/// Whether `id` is spelled the way this service spells a request id: exactly
/// [`REQUEST_ID_WIDTH`] characters, every one of them an uppercase letter or a digit. The
/// hyphenated UUID other AWS services report fails on both counts, which is the difference
/// this suite exists to pin.
fn names_a_request_id(id: &str) -> bool {
    id.chars().count() == REQUEST_ID_WIDTH
        && id
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

/// One delivery of one request: its status, the request id the response named, and its
/// body as text.
///
/// `target` is the whole `X-Amz-Target` value, so a row can address an operation the model
/// does not declare.
async fn deliver(
    router: &MultiServiceRouter,
    target: &str,
    body: &Value,
) -> (u16, Option<String>, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/dynamodb/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, request_id, text)
}

#[tokio::test]
async fn every_response_names_the_request_it_answered_and_every_attempt_is_its_own() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "RequestIds",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "RequestIds", "RequestIds").await;

    // (case, X-Amz-Target, body, the status it must answer with)
    let rows: Vec<(&str, &str, Value, u16)> = vec![
        (
            "a handler's success",
            "DynamoDB_20120810.PutItem",
            json!({"TableName": "RequestIds", "Item": {"pk": {"S": "a"}}}),
            200,
        ),
        (
            "a read that found nothing, which is still a success",
            "DynamoDB_20120810.GetItem",
            json!({"TableName": "RequestIds", "Key": {"pk": {"S": "absent"}}}),
            200,
        ),
        (
            "a control-plane success",
            "DynamoDB_20120810.DescribeTable",
            json!({"TableName": "RequestIds"}),
            200,
        ),
        (
            "a handler's modelled error",
            "DynamoDB_20120810.DescribeTable",
            json!({"TableName": "NoSuchTable"}),
            400,
        ),
        (
            "a refusal from the constraint walk ahead of the handler",
            "DynamoDB_20120810.Query",
            json!({"TableName": "RequestIds", "Limit": 0, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "a"}}}),
            400,
        ),
        (
            "a write that mutates an existing item",
            "DynamoDB_20120810.UpdateItem",
            json!({"TableName": "RequestIds", "Key": {"pk": {"S": "a"}},
                   "UpdateExpression": "SET #n = :v",
                   "ExpressionAttributeNames": {"#n": "note"},
                   "ExpressionAttributeValues": {":v": {"S": "written"}}}),
            200,
        ),
        (
            "a success answering a request with no resource named in it",
            "DynamoDB_20120810.ListTables",
            json!({}),
            200,
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, target, body, expected) in &rows {
        let (status, request_id, text) = deliver(&router, target, body).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {text}"
        );
        let id = request_id.unwrap_or_else(|| {
            panic!("case {case:?}: every response names the request it answered, got none")
        });
        assert!(
            names_a_request_id(&id),
            "case {case:?}: expected {REQUEST_ID_WIDTH} uppercase letters and digits, which is \
             how this service spells a request id, got {id:?}"
        );
        assert!(
            !text.contains(&id),
            "case {case:?}: the id travels in the header alone, got {text}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every response names its OWN request, and {id} was already used"
        );
    }

    // A request that names NO modelled operation is answered by the router rather than by
    // an operation, and names its request too.
    let (status, request_id, text) = deliver(
        &router,
        "DynamoDB_20120810.NoSuchOperation",
        &json!({"TableName": "RequestIds"}),
    )
    .await;
    assert!(
        status >= 400,
        "case a request naming no modelled operation: expected a failure, got {status} {text}"
    );
    let id = request_id.expect("a routing failure names the request it answered");
    assert!(
        names_a_request_id(&id) && minted.insert(id.clone()),
        "case a request naming no modelled operation: expected its own request id, got {id:?}"
    );

    // The SAME call delivered twice — what an SDK retry sends — is two requests, so it is
    // answered with two different ids. The body of both is the same byte for byte: the
    // retry is served exactly as the first delivery was.
    let retried = json!({"TableName": "RequestIds", "Key": {"pk": {"S": "a"}}});
    let mut answers = Vec::new();
    for attempt in 0..2 {
        let (status, request_id, text) =
            deliver(&router, "DynamoDB_20120810.GetItem", &retried).await;
        assert_eq!(
            status, 200,
            "case retry delivery {attempt}: expected HTTP 200, got {status} {text}"
        );
        answers.push((
            request_id.unwrap_or_else(|| {
                panic!("case retry delivery {attempt}: expected a request id, got none")
            }),
            text,
        ));
    }
    assert_ne!(
        answers[0].0, answers[1].0,
        "case an SDK-retried call: each attempt is a separate request and is answered as \
         one, got {:?} for both",
        answers[0].0
    );
    for (attempt, (id, _)) in answers.iter().enumerate() {
        assert!(
            names_a_request_id(id),
            "case retry delivery {attempt}: expected {REQUEST_ID_WIDTH} uppercase letters and \
             digits, which is how this service spells a request id, got {id:?}"
        );
    }
    assert_eq!(
        answers[0].1, answers[1].1,
        "case an SDK-retried call: the id is the only thing that differs between the two \
         answers, got {:?} and {:?}",
        answers[0].1, answers[1].1
    );
}
