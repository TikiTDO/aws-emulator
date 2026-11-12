// SPDX-License-Identifier: CC0-1.0

//! What the `RevisionId` of a table's resource-based policy IS, end to end.
//!
//! The rule pinned here: a revision id is a token the SERVICE hands out for the revision
//! it just made, not a value derived from the document. `PolicyRevisionId` is a
//! 1..255-character string with no pattern
//! (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#PolicyRevisionId")`) and
//! all a caller does with one is compare it — hold it and pass it back as an
//! `ExpectedRevisionId` — so each revision of a policy gets its own id, the ids of one
//! policy's revisions are ordered, and the id changes when the policy is replaced even if
//! the document that replaces it says exactly the same thing in different words. The one
//! documented exception is the idempotent re-attach: "PutResourcePolicy is an idempotent
//! operation; running it multiple times on the same resource using the same policy
//! document will return the same revision id".
//!
//! The rows differ along what the rule generalises over: whether the next document is the
//! same text, the same meaning in different text, or a different policy altogether;
//! whether the revision is read back by `GetResourcePolicy` or reported by the write that
//! made it; and which of the two resources a policy attaches to carries it (a table and
//! that table's stream), since each carries a revision of its own and replacing one
//! leaves the other where it was.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The modelled bounds on `PolicyRevisionId` (`@length min 1 max 255`).
const REVISION_ID_BOUNDS: (usize, usize) = (1, 255);

/// One policy document granting `action` to this emulator's account on `resource`.
fn document(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "grant",
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::000000000000:root"},
            "Action": [action],
            "Resource": [resource],
        }],
    })
    .to_string()
}

/// Create a table with a stream, so both resources a policy attaches to exist, and
/// report its ARN.
async fn policy_table(router: &MultiServiceRouter, name: &str) -> String {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
        }),
    )
    .await;
    let described = describe_when_active(router, name, name).await;
    described["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The stream ARN the table reports, which is the OTHER resource a policy attaches to.
async fn stream_arn(router: &MultiServiceRouter, name: &str) -> String {
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    described["Table"]["LatestStreamArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Attach `policy` to `arn` and report the `RevisionId` the put reported.
async fn put_policy(router: &MultiServiceRouter, case: &str, arn: &str, policy: &str) -> String {
    let answered = ok(
        router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": policy}),
    )
    .await;
    revision_id(case, "PutResourcePolicy", &answered)
}

/// The `RevisionId` a response carries, held to the form the member takes: a non-empty
/// decimal token inside the modelled length bounds, and never a digest of the document
/// (which would be 64 hexadecimal characters).
fn revision_id(case: &str, operation: &str, body: &Value) -> String {
    let reported = body["RevisionId"].as_str().unwrap_or_default().to_string();
    let (min, max) = REVISION_ID_BOUNDS;
    assert!(
        reported.len() >= min && reported.len() <= max,
        "case {case:?} ({operation}): a revision id is {min}..{max} characters, got \
         {reported:?} ({} characters)",
        reported.len()
    );
    assert!(
        reported.chars().all(|character| character.is_ascii_digit())
            && !reported.starts_with('0'),
        "case {case:?} ({operation}): a revision id is a decimal token, got {reported:?}"
    );
    reported
}

#[tokio::test]
async fn every_revision_of_a_policy_gets_its_own_revision_id() {
    let router = emulator();
    let arn = policy_table(&router, "PolicyRevisions").await;

    // Rows differ in HOW the next document relates to the one attached: the same text (the
    // idempotent re-attach, which keeps the revision), the same grant written differently,
    // a different action, and a different resource. Each row states whether the revision
    // must be the one before it.
    let same = document("dynamodb:GetItem", &format!("{arn}*"));
    let cases: [(&str, String, bool); 5] = [
        ("the same document re-attached", same.clone(), true),
        ("the same document re-attached twice", same.clone(), true),
        (
            "the same grant in a different document",
            document("dynamodb:GetItem", &arn),
            false,
        ),
        (
            "a different action",
            document("dynamodb:Query", &arn),
            false,
        ),
        (
            "a different resource",
            document("dynamodb:Query", &format!("{arn}/index/*")),
            false,
        ),
    ];

    let mut current = put_policy(&router, "the first policy attached", &arn, &same).await;
    let mut seen: Vec<String> = vec![current.clone()];
    for (case, policy, idempotent) in cases {
        let reported = put_policy(&router, case, &arn, &policy).await;
        if idempotent {
            assert_eq!(
                reported, current,
                "case {case:?}: the same document re-attached reports the revision it \
                 already had ({current})"
            );
        } else {
            assert!(
                !seen.contains(&reported),
                "case {case:?}: a new revision gets an id of its own, got {reported} among \
                 {seen:?}"
            );
            assert!(
                reported.parse::<u128>().unwrap_or_default()
                    > current.parse::<u128>().unwrap_or_default(),
                "case {case:?}: a revision sorts after the one it replaced, got {reported} \
                 after {current}"
            );
            seen.push(reported.clone());
            current = reported.clone();
        }
        // The read reports the revision the write made, every time it is asked.
        for round in 0..2 {
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"ResourceArn": &arn}),
            )
            .await;
            let found = revision_id(case, "GetResourcePolicy", &read);
            assert_eq!(
                found, current,
                "case {case:?} (read {round}): the attached revision is reported as the \
                 write reported it ({current})"
            );
        }
    }
}

#[tokio::test]
async fn a_revision_id_is_what_a_conditional_write_is_held_to() {
    let router = emulator();
    let arn = policy_table(&router, "PolicyConditions").await;
    let first = document("dynamodb:GetItem", &arn);
    let second = document("dynamodb:Query", &arn);

    // The revision a client holds is the whole guard: a put that names the current one is
    // served, one that names a revision that has been replaced is refused, and the refusal
    // names both revisions so a stale writer can tell what it is holding.
    let stale = put_policy(&router, "the first policy attached", &arn, &first).await;
    let current = put_policy(&router, "the revision that replaces it", &arn, &second).await;
    assert_ne!(
        stale, current,
        "case the replacing revision: a replaced policy's id is never reused"
    );

    let (status, refused) = call(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": &arn, "Policy": &first, "ExpectedRevisionId": &stale}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "PolicyNotFoundException"),
        "case a stale revision: expected a 400 PolicyNotFoundException, got {status} with \
         {refused}"
    );
    let stated = message(&refused);
    assert!(
        stated.contains(&stale) && stated.contains(&current),
        "case a stale revision: the refusal names the revision asked for and the one \
         attached, got {stated:?}"
    );

    let served = ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": &arn, "Policy": &first, "ExpectedRevisionId": &current}),
    )
    .await;
    let after = revision_id("the current revision", "PutResourcePolicy", &served);
    assert!(
        after.parse::<u128>().unwrap_or_default() > current.parse::<u128>().unwrap_or_default(),
        "case the current revision: the accepted write makes a later revision, got {after} \
         after {current}"
    );

    // The delete reports the revision it removed — "the revision ID of the policy that was
    // deleted" — and the resource then carries none at all.
    let deleted = ok(
        &router,
        "DeleteResourcePolicy",
        json!({"ResourceArn": &arn}),
    )
    .await;
    assert_eq!(
        revision_id("the removed policy", "DeleteResourcePolicy", &deleted),
        after,
        "case the removed policy: the delete reports the revision it removed ({after})"
    );
    let (status, read) = call(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": &arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (400, "PolicyNotFoundException"),
        "case the removed policy: the resource carries no policy to read, got {status} with \
         {read}"
    );
}

#[tokio::test]
async fn each_resource_carries_its_own_policy_revision() {
    let router = emulator();
    let table = policy_table(&router, "PolicyPerResource").await;
    let stream = stream_arn(&router, "PolicyPerResource").await;
    assert!(
        !stream.is_empty(),
        "case the stream resource: the table reports a LatestStreamArn to attach a policy to"
    );

    // The SAME grant attached to the two resources a policy attaches to: each carries its
    // own revision, so replacing one leaves the other exactly where it was, and a read of
    // either reports that resource's own revision.
    let stream_policy = document("dynamodb:GetRecords", &stream);
    let table_policy = document("dynamodb:GetItem", &table);
    let on_table = put_policy(&router, "a policy on the table", &table, &table_policy).await;
    let on_stream = put_policy(&router, "a policy on the stream", &stream, &stream_policy).await;

    let replaced = put_policy(
        &router,
        "the stream's policy replaced",
        &stream,
        &document("dynamodb:DescribeStream", &stream),
    )
    .await;
    assert_ne!(
        replaced, on_stream,
        "case the stream's policy replaced: the new revision has an id of its own"
    );
    let read = ok(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": &table}),
    )
    .await;
    assert_eq!(
        revision_id("the table after the stream was written", "GetResourcePolicy", &read),
        on_table,
        "case the table after the stream was written: the table's revision is untouched \
         ({on_table})"
    );
}
