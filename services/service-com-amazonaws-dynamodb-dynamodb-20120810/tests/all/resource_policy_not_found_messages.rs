// SPDX-License-Identifier: CC0-1.0

//! How the resource-policy surface WORDS `PolicyNotFoundException`.
//!
//! One rule, asserted over a set of inputs rather than one example: every
//! `PolicyNotFoundException` a resource-policy operation raises opens with the same
//! fixed sentence and then says, from the request and from stored state, which of the
//! two situations the modelled error covers it is —
//!
//! * an UNCONDITIONED request for a policy that is not there is followed by the
//!   `ResourceArn` the request sent, spelled as the request spelled it;
//! * a request whose `ExpectedRevisionId` the target does not satisfy is followed by
//!   the revision id that was asked for and what the target carries instead: the
//!   revision id of the policy attached to it, or that it carries no policy at all.
//!
//! The expected strings are spelled out here rather than taken from the crate, so the
//! wording itself is what is pinned. The rows differ along the operation, the shape and
//! size of the `ExpectedRevisionId` (including the published 255-character bound), the
//! table the ARN names, and whether a policy is attached at all.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the
//! harness in [`super::table_items`].

use serde_json::json;

use super::table_items::{call, create_active_table, emulator, ok};
use crate::emulator::{error_code, message};

/// The ARN prefix every table of this emulator carries, in the region the harness's
/// credential scope names.
const TABLE_ARN_PREFIX: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/";

/// The fixed opening of every `PolicyNotFoundException` message, written out so this
/// suite asserts the wording and not the emulator's own copy of it.
const PREFIX: &str = "Resource-based policy not found for the provided ResourceArn: ";

/// The documented maximum length of a `PolicyRevisionId`
/// (`smithy_query(query="shape",shape="PolicyRevisionId")` -> length 1..255), so a row
/// can sit on the published bound.
const MAX_POLICY_REVISION_ID_LENGTH: usize = 255;

/// The ARN of one of this emulator's tables.
fn table_arn(name: &str) -> String {
    format!("{TABLE_ARN_PREFIX}{name}")
}

/// Wait out the span a policy write takes to be applied, after which the eventually
/// consistent reader reports it — the span the store itself owes it
/// (`crate::state::STATUS_TRANSITION`), plus room for the round trips.
async fn wait_for_policy_application() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    )
    .await;
}

/// A resource-policy document this emulator accepts, denying `action` on `resource_arn`
/// so that a different action yields a different document — and so a different
/// revision id.
fn policy(resource_arn: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": action,
            "Resource": resource_arn,
        }],
    })
    .to_string()
}

/// A table's `(attribute, key type, attribute type)` rows, as `create_active_table`
/// takes them.
type KeySchema = &'static [(&'static str, &'static str, &'static str)];

/// The refusal an unconditioned read of an absent policy earns, over a set of tables:
/// the fixed sentence plus the `ResourceArn` that was sent.
#[tokio::test]
async fn an_unconditioned_policy_read_names_the_requested_resource_arn() {
    let router = emulator();
    // The rows differ along the table name's length and character classes (a DynamoDB
    // table name is `[A-Za-z0-9_.-]{3,255}`, so the shortest and the longest name are
    // both rows), which is the request-derived part of the message, and along the
    // table's key schema.
    let tables: [(String, KeySchema); 4] = [
        ("Unconditioned".to_string(), &[("pk", "HASH", "S")]),
        (
            "abc".to_string(),
            &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        ),
        (
            "a.table-with_every.class-0".to_string(),
            &[("pk", "HASH", "B")],
        ),
        ("x".repeat(255), &[("pk", "HASH", "S")]),
    ];

    for (name, keys) in tables {
        let case = format!("a table named {} characters long", name.len());
        create_active_table(&router, &name, keys).await;
        let arn = table_arn(&name);

        let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "PolicyNotFoundException"),
            "case {case}: expected 400 PolicyNotFoundException, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            format!("{PREFIX}{arn}"),
            "case {case}: expected the fixed sentence and the requested ResourceArn, got {:?}",
            message(&body)
        );

        // The same wording once a policy that WAS there is gone again: the situation is
        // "no policy", whatever the table's history.
        ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": arn, "Policy": policy(&arn, "dynamodb:GetItem")}),
        )
        .await;
        ok(
            &router,
            "DeleteResourcePolicy",
            json!({"ResourceArn": arn}),
        )
        .await;
        // Both writes are asynchronous, so the reader is asked once the removal has been
        // applied.
        wait_for_policy_application().await;
        let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "PolicyNotFoundException"),
            "case {case} after the policy was deleted: expected 400 PolicyNotFoundException, \
             got {status} {body}"
        );
        assert_eq!(
            message(&body),
            format!("{PREFIX}{arn}"),
            "case {case} after the policy was deleted: expected the fixed sentence and the \
             requested ResourceArn, got {:?}",
            message(&body)
        );
    }
}

/// One conditioned-request row: the operation, the `ExpectedRevisionId` it sends and
/// whether the table carries a policy when it is sent.
struct ConditionedCase {
    case: &'static str,
    operation: &'static str,
    expected_revision_id: String,
    attached: bool,
}

/// The refusal a conditioned request earns, over both operations that accept
/// `ExpectedRevisionId` and both situations the modelled error covers: the requested
/// revision id, and either the revision id the target carries or that it carries none.
#[tokio::test]
async fn a_conditioned_request_names_the_requested_and_the_actual_revision() {
    let router = emulator();
    let cases = [
        ConditionedCase {
            case: "a put against a stale revision",
            operation: "PutResourcePolicy",
            expected_revision_id: "stale-revision-id-does-not-match".to_string(),
            attached: true,
        },
        ConditionedCase {
            case: "a put against a revision id of digits only",
            operation: "PutResourcePolicy",
            expected_revision_id: "0".repeat(32),
            attached: true,
        },
        ConditionedCase {
            case: "a put at the maximum revision-id length",
            operation: "PutResourcePolicy",
            expected_revision_id: "r".repeat(MAX_POLICY_REVISION_ID_LENGTH),
            attached: true,
        },
        ConditionedCase {
            // "To conditionally attach a policy when no policy exists for the resource,
            // specify NO_POLICY for the revision ID" — so against a table that DOES
            // carry one, this is a revision that does not match.
            case: "a put conditioned on no policy against a table that carries one",
            operation: "PutResourcePolicy",
            expected_revision_id: "NO_POLICY".to_string(),
            attached: true,
        },
        ConditionedCase {
            case: "a put against a table with no policy",
            operation: "PutResourcePolicy",
            expected_revision_id: "stale-revision-id-does-not-match".to_string(),
            attached: false,
        },
        ConditionedCase {
            case: "a delete against a stale revision",
            operation: "DeleteResourcePolicy",
            expected_revision_id: "0".repeat(32),
            attached: true,
        },
        ConditionedCase {
            case: "a delete at the maximum revision-id length",
            operation: "DeleteResourcePolicy",
            expected_revision_id: "r".repeat(MAX_POLICY_REVISION_ID_LENGTH),
            attached: true,
        },
        ConditionedCase {
            case: "a delete against a table with no policy",
            operation: "DeleteResourcePolicy",
            expected_revision_id: "stale-revision-id-does-not-match".to_string(),
            attached: false,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let ConditionedCase {
            case,
            operation,
            expected_revision_id,
            attached,
        } = row;
        // One table per row, so what a row asserts never depends on the row before it.
        let table = format!("Conditioned{index}");
        create_active_table(&router, &table, &[("pk", "HASH", "S")]).await;
        let arn = table_arn(&table);
        let document = policy(&arn, "dynamodb:GetItem");

        let current = if *attached {
            let put = ok(
                &router,
                "PutResourcePolicy",
                json!({"ResourceArn": arn, "Policy": document}),
            )
            .await;
            // The attach is asynchronous, so the reader below is asked once it is applied.
            wait_for_policy_application().await;
            Some(
                put["RevisionId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: the attach reports a revision id"))
                    .to_string(),
            )
        } else {
            None
        };

        let mut request = json!({
            "ResourceArn": arn,
            "ExpectedRevisionId": expected_revision_id,
        });
        if *operation == "PutResourcePolicy" {
            request["Policy"] = json!(policy(&arn, "dynamodb:PutItem"));
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "PolicyNotFoundException"),
            "case {case}: expected 400 PolicyNotFoundException, got {status} {body}"
        );
        let expected_message = match &current {
            Some(current) => format!(
                "{PREFIX}Requested update for policy with revision id {expected_revision_id}, \
                 but the policy associated to target table {table} has revision id {current}."
            ),
            None => format!(
                "{PREFIX}Requested update for policy with revision id {expected_revision_id}, \
                 but table {table} has no associated policy."
            ),
        };
        assert_eq!(
            message(&body),
            expected_message,
            "case {case}: expected {expected_message:?}, got {:?}",
            message(&body)
        );

        // The refused request changed nothing, so the revision id the message reported
        // is still the one a reader sees.
        let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
        match &current {
            Some(current) => assert_eq!(
                (status, body["RevisionId"].as_str()),
                (200, Some(current.as_str())),
                "case {case}: the refusal left the attached policy alone, got {status} {body}"
            ),
            None => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "PolicyNotFoundException"),
                "case {case}: the refusal attached nothing, got {status} {body}"
            ),
        }
    }

    // One character past the published bound is a different rule: the value is not a
    // `PolicyRevisionId` at all, so it is refused as input and never reaches the
    // precondition this suite is about.
    let arn = table_arn("Conditioned0");
    let (status, body) = call(
        &router,
        "PutResourcePolicy",
        &json!({
            "ResourceArn": arn,
            "Policy": policy(&arn, "dynamodb:PutItem"),
            "ExpectedRevisionId": "r".repeat(MAX_POLICY_REVISION_ID_LENGTH + 1),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case an ExpectedRevisionId one character past {MAX_POLICY_REVISION_ID_LENGTH}: expected \
         400 ValidationException, got {status} {body}"
    );
}

/// The two situations are told APART: for one table, one revision id and one operation,
/// the "no associated policy" refusal and the "has revision id" refusal are different
/// messages — which is the decision a client doing optimistic concurrency makes.
#[tokio::test]
async fn the_two_policy_not_found_situations_do_not_share_a_message() {
    let router = emulator();
    create_active_table(&router, "Distinguished", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Distinguished");
    let stale = "0".repeat(32);
    let request = json!({
        "ResourceArn": arn,
        "Policy": policy(&arn, "dynamodb:PutItem"),
        "ExpectedRevisionId": stale,
    });

    let (_, absent) = call(&router, "PutResourcePolicy", &request).await;
    ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": policy(&arn, "dynamodb:GetItem")}),
    )
    .await;
    let (_, mismatched) = call(&router, "PutResourcePolicy", &request).await;

    assert_ne!(
        message(&absent),
        message(&mismatched),
        "case the same stale revision id before and after a policy is attached: the two \
         situations must not share a message, got {:?}",
        message(&absent)
    );
    for body in [&absent, &mismatched] {
        assert!(
            message(body).starts_with(PREFIX),
            "case either situation: the message opens with the fixed sentence, got {:?}",
            message(body)
        );
    }
    assert!(
        message(&absent).ends_with("has no associated policy."),
        "case no policy attached: the message says the table carries none, got {:?}",
        message(&absent)
    );
    assert!(
        message(&mismatched).contains("has revision id "),
        "case a policy attached: the message names the revision id it carries, got {:?}",
        message(&mismatched)
    );
}
