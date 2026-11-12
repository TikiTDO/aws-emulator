// SPDX-License-Identifier: CC0-1.0

//! What a TAGGING operation answers for a `resourceArn` that is not an ARN, end to end over
//! the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — through the harness this crate's
//! `collection_lifecycle` suite owns, so the boundary validator, the protocol codec and the
//! handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: `resourceArn` is "the Amazon Resource Name (ARN) of the resource"
//! (`smithy_query(query="shape", shape="TagResourceRequest")`, and the same sentence on
//! `UntagResourceRequest` and `ListTagsForResourceRequest`), so a value that is not an ARN
//! — not the six fields `arn:<partition>:<service>:<region>:<account-id>:<resource>`, with
//! the literal `arn` first and a partition, a service namespace and a resource that each
//! name something — addresses no resource of any service and nothing can be looked up under
//! it: it is MALFORMED INPUT, refused with `ValidationException` and `Resource ARN: <the
//! value the request carried> is invalid` before any lookup. A value that IS an ARN is
//! looked up, and refused with `ResourceNotFoundException` when it addresses no live
//! resource — the two conditions are not interchangeable, since the first is a caller bug
//! no retry can turn into a resource while the second is a state a client may poll or read
//! as already deleted.
//!
//! The table spans what the rule generalises over — which tagging operation takes the
//! value, and each way a value can fail to be an ARN (no prefix, the wrong prefix, too few
//! fields, an empty partition, service or resource, a bare word, leading space, multi-byte
//! text, a value at the member's documented maximum length) — rather than the one probe
//! string the issue quoted. Beside it stands the ARN table the refusal must NOT claim: an
//! ARN that is well formed is answered by the state, whether it addresses a live resource
//! (served) or none (not found).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{call, emulator, text, ACCOUNT, CLIENT_ERROR};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// `Arn`'s documented maximum length (`smithy_query(query="shape", shape="Arn")` ->
/// `length {min: 1, max: 1011}`): a value this long is inside the member's own bound, so
/// the handler — not the boundary validator — is what answers it.
const ARN_MAX_LENGTH: usize = 1011;

/// Every operation whose input is a `resourceArn`, with the rest of a well-formed request
/// for it. One table, so no case can be asserted of the writes but not of the read.
fn tagging_plane() -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({})),
        (
            "TagResource",
            json!({ "tags": [{ "key": "team", "value": "search" }] }),
        ),
        ("UntagResource", json!({ "tagKeys": ["team"] })),
    ]
}

/// The `message` an error body carries, whichever casing the envelope used.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Send `operation` against `value` and answer `(status, error code, body)`.
async fn against(
    router: &MultiServiceRouter,
    operation: &str,
    extra: &Value,
    value: &str,
) -> (u16, String, Value) {
    let mut request = extra.clone();
    request["resourceArn"] = json!(value);
    call(router, operation, request).await
}

/// Every value that is NOT an ARN, each row a different way of missing the form.
fn malformed_values() -> Vec<(&'static str, String)> {
    vec![
        ("a bare word", "not-a-real-arn".to_string()),
        (
            "no `arn:` prefix at all",
            format!("aoss:{REGION}:{ACCOUNT}:collection/abc123"),
        ),
        (
            "the prefix in upper case",
            format!("ARN:aws:aoss:{REGION}:{ACCOUNT}:collection/abc123"),
        ),
        (
            "a leading space before the prefix",
            format!(" arn:aws:aoss:{REGION}:{ACCOUNT}:collection/abc123"),
        ),
        ("the prefix alone", "arn".to_string()),
        (
            "five fields, the resource field missing",
            format!("arn:aws:aoss:{REGION}:{ACCOUNT}"),
        ),
        (
            "an empty partition field",
            format!("arn::aoss:{REGION}:{ACCOUNT}:collection/abc123"),
        ),
        (
            "an empty service field",
            format!("arn:aws::{REGION}:{ACCOUNT}:collection/abc123"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:aoss:{REGION}:{ACCOUNT}:"),
        ),
        (
            "multi-byte text that is no ARN",
            "ベクトル検索 🌐 — naïve".to_string(),
        ),
        (
            "a value at the member's maximum length",
            "x".repeat(ARN_MAX_LENGTH),
        ),
    ]
}

#[tokio::test]
async fn every_tagging_operation_refuses_a_resource_arn_that_is_not_an_arn_as_malformed_input() {
    let router = emulator();

    // A live resource exists throughout, so no refusal below can be the store being empty.
    let group = create_group(
        &router,
        REGION,
        json!({ "name": "arn-syntax-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let live = text(&group, "arn");

    for (operation, extra) in tagging_plane() {
        for (case, value) in malformed_values() {
            let (status, code, answer) = against(&router, operation, &extra, &value).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ValidationException"),
                "case {operation}({case}): expected {CLIENT_ERROR} ValidationException, got \
                 {status} {code} with {answer}"
            );
            let said = message(&answer);
            assert_eq!(
                said,
                format!("Resource ARN: {value} is invalid"),
                "case {operation}({case}): the refusal must name the value the request \
                 carried, got {said:?}"
            );
        }

        // The same operation, against an ARN, is answered by the STATE and not by the
        // syntax refusal: the live resource is served and an absent one is not found.
        let (status, code, answer) = against(&router, operation, &extra, &live).await;
        assert_eq!(
            status, 200,
            "case {operation}(a live collection group): expected 200, got {status} {code} \
             with {answer}"
        );
    }
}

#[tokio::test]
async fn an_arn_is_answered_by_the_state_and_never_by_the_syntax_refusal() {
    let router = emulator();

    // Every row IS an ARN — including ones whose region and account are empty, as a global
    // or account-less resource's ARN is, and ones naming another service or a resource half
    // this service does not address — so each is looked up and reported as not found, never
    // as malformed.
    let absent_id = "0".repeat(24);
    let arns: Vec<(&str, String, String)> = vec![
        (
            "a collection id nothing carries",
            format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection/{absent_id}"),
            format!("Collection ID {absent_id} is not found"),
        ),
        (
            "a collection group id nothing carries",
            format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection-group/{absent_id}"),
            format!("Collection Group ID {absent_id} is not found"),
        ),
        (
            "another partition's collection",
            format!("arn:aws-us-gov:aoss:{REGION}:{ACCOUNT}:collection/{absent_id}"),
            format!("Collection ID {absent_id} is not found"),
        ),
        (
            "an ARN with no region and no account",
            "arn:aws:s3:::a-bucket".to_string(),
            "Resource arn:aws:s3:::a-bucket is not found".to_string(),
        ),
        (
            "an ARN of another service whose resource half reads like one of ours",
            format!("arn:aws:es:{REGION}:{ACCOUNT}:collection/{absent_id}"),
            format!("Resource arn:aws:es:{REGION}:{ACCOUNT}:collection/{absent_id} is not found"),
        ),
    ];

    for (operation, extra) in tagging_plane() {
        for (case, arn, expected) in &arns {
            let (status, code, answer) = against(&router, operation, &extra, arn).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {operation}({case}): expected {CLIENT_ERROR} ResourceNotFoundException, \
                 got {status} {code} with {answer}"
            );
            let said = message(&answer);
            assert_eq!(
                &said, expected,
                "case {operation}({case}): expected the refusal to say {expected:?}, got \
                 {said:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_tag_lifecycle_of_a_resource_addressed_by_its_own_arn_round_trips() {
    let router = emulator();

    // The ARN a create HANDS the caller is the one the tagging plane addresses the resource
    // by: the write is reported in the order written, and a removal leaves the rest.
    let group = create_group(
        &router,
        REGION,
        json!({ "name": "arn-syntax-lifecycle", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let arn = text(&group, "arn");

    let written = json!([
        { "key": "env", "value": "probe" },
        { "key": "team", "value": "search" },
    ]);
    let (status, code, answer) = call(
        &router,
        "TagResource",
        json!({ "resourceArn": &arn, "tags": written }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case TagResource(a live collection group): expected 200, got {status} {code} with \
         {answer}"
    );
    assert_eq!(
        answer,
        json!({}),
        "case TagResource(a live collection group): the response models no members, got \
         {answer}"
    );

    let (status, code, answer) = call(
        &router,
        "ListTagsForResource",
        json!({ "resourceArn": &arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource(after the write): expected 200, got {status} {code} with \
         {answer}"
    );
    assert_eq!(
        answer.get("tags").cloned().unwrap_or(Value::Null),
        written,
        "case ListTagsForResource(after the write): expected the tags as written, got {answer}"
    );

    let (status, code, answer) = call(
        &router,
        "UntagResource",
        json!({ "resourceArn": &arn, "tagKeys": ["team"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UntagResource(one key): expected 200, got {status} {code} with {answer}"
    );
    let (status, code, answer) = call(
        &router,
        "ListTagsForResource",
        json!({ "resourceArn": &arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource(after the removal): expected 200, got {status} {code} with \
         {answer}"
    );
    assert_eq!(
        answer.get("tags").cloned().unwrap_or(Value::Null),
        json!([{ "key": "env", "value": "probe" }]),
        "case ListTagsForResource(after the removal): expected the remaining tag, got {answer}"
    );
}
