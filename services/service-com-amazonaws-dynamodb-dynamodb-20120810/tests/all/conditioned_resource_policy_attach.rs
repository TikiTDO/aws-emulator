// SPDX-License-Identifier: CC0-1.0

//! What a statement `Condition` does to the ATTACH path of a resource-based policy, and
//! how the resource-policy surface names a resource it cannot find.
//!
//! Two rules, each asserted over a set of inputs rather than one example:
//!
//! * `ConfirmRemoveSelfResourceAccess` is required only for a document that would deny
//!   the caller the policy-management actions in EVERY request context. A statement
//!   carrying a `Condition` applies only to the requests whose context satisfies it, so
//!   it leaves the caller able to change the policy again and needs no confirmation: the
//!   document is stored verbatim, reported back by `GetResourcePolicy`, and removed by
//!   `DeleteResourcePolicy`, which reports the revision id it removed. A document whose
//!   deny of those actions carries no condition still needs the confirmation.
//! * a resource-policy operation that cannot find the resource names the TABLE it could
//!   not find, by the bare name inside the `ResourceArn` the request carried.
//!
//! The rows differ along the condition operator and context key, the action pattern, the
//! `Principal` spelling, the statement count and order, the operation, and the length and
//! character classes of the table name. The expected wording is spelled out here rather
//! than taken from the crate, so the wording itself is what is pinned.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the
//! harness in [`super::table_items`].

use serde_json::{json, Value};

use super::table_items::{call, create_active_table, emulator, ok};
use crate::emulator::{error_code, message};

/// The ARN prefix every table of this emulator carries, in the region the harness's
/// credential scope names.
const TABLE_ARN_PREFIX: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/";

/// The account id every generated emulator presents.
const ACCOUNT_ID: &str = "000000000000";

/// The refusal a document that takes the caller's own policy-management access away
/// earns when the request does not confirm it.
const LOCK_OUT_MESSAGE: &str =
    "The new resource policy will not allow you to update the resource policy in the future.";

/// The documented maximum length of a `PolicyRevisionId`
/// (`smithy_query(query="shape",shape="PolicyRevisionId")` -> length 1..255).
const MAX_POLICY_REVISION_ID_LENGTH: usize = 255;

/// The ARN of one of this emulator's tables.
fn table_arn(name: &str) -> String {
    format!("{TABLE_ARN_PREFIX}{name}")
}

/// A policy document of the statements given, in the version IAM reads them under.
fn document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One `Deny` statement: who it is about, what it denies, and the `Condition` it is
/// gated on, if any.
fn deny(principal: Value, action: &str, resource_arn: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Deny",
        "Principal": principal,
        "Action": action,
        "Resource": resource_arn,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The revision id a successful write reports, held to the member's modelled bound.
fn revision_id(case: &str, body: &Value) -> String {
    let revision_id = body["RevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a RevisionId, got {body}"))
        .to_string();
    assert!(
        (1..=MAX_POLICY_REVISION_ID_LENGTH).contains(&revision_id.chars().count()),
        "case {case}: a RevisionId is 1..{MAX_POLICY_REVISION_ID_LENGTH} characters, got \
         {revision_id:?}"
    );
    revision_id
}

/// Wait out the span a policy write takes to be applied, after which the eventually
/// consistent reader reports it — the span the store itself owes it
/// (`crate::state::STATUS_TRANSITION`), so a change to that span does not invalidate this
/// wait, plus room for the round trips.
async fn wait_for_policy_application() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    )
    .await;
}

/// A `Condition` block, and what makes it a different row.
struct ConditionCase {
    case: &'static str,
    condition: Value,
}

/// Condition blocks spanning the operator and context-key classes an IAM condition can
/// be written in — a string comparison, a negated one, a set operator, a date, a
/// boolean, an address and a two-key block — taken from the condition operators and
/// global context keys the IAM policy reference publishes, and including keys the
/// recorded cases did not use.
fn condition_cases(resource_arn: &str) -> Vec<ConditionCase> {
    vec![
        ConditionCase {
            case: "StringEquals on the requested region",
            condition: json!({"StringEquals": {"aws:RequestedRegion": "eu-west-1"}}),
        },
        ConditionCase {
            case: "StringNotEquals on the calling principal",
            condition: json!({
                "StringNotEquals": {
                    "aws:PrincipalArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/operator"),
                },
            }),
        },
        ConditionCase {
            case: "ForAllValues:StringEquals on the leading keys",
            condition: json!({
                "ForAllValues:StringEquals": {"dynamodb:LeadingKeys": ["secret", "private"]},
            }),
        },
        ConditionCase {
            case: "DateLessThan on the current time",
            condition: json!({"DateLessThan": {"aws:CurrentTime": "2030-01-01T00:00:00Z"}}),
        },
        ConditionCase {
            case: "Bool on secure transport",
            condition: json!({"Bool": {"aws:SecureTransport": "false"}}),
        },
        ConditionCase {
            case: "IpAddress on the source address",
            condition: json!({"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}}),
        },
        ConditionCase {
            case: "two operators in one block",
            condition: json!({
                "StringEquals": {"aws:RequestedRegion": "us-east-1"},
                "ArnEquals": {"aws:SourceArn": resource_arn},
            }),
        },
    ]
}

/// A deny of the policy-management actions that is GATED on a condition is attached
/// without `ConfirmRemoveSelfResourceAccess`, round trips through the reader, and is
/// removed with its revision id reported.
#[tokio::test]
async fn a_conditioned_policy_management_deny_attaches_without_the_confirmation() {
    let router = emulator();
    create_active_table(&router, "Conditioned", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Conditioned");

    // The actions a document can take the caller's own access away with — each
    // policy-management action by name and the wildcards that cover them — against every
    // spelling a `Principal` element names this account by.
    let actions = [
        "dynamodb:PutResourcePolicy",
        "dynamodb:DeleteResourcePolicy",
        "dynamodb:GetResourcePolicy",
        "dynamodb:*",
        "*",
    ];
    let principals = [
        json!("*"),
        json!({"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")}),
        json!({"AWS": ACCOUNT_ID}),
    ];

    for ConditionCase { case, condition } in condition_cases(&arn) {
        for action in actions {
            for principal in &principals {
                let case = format!("{case}, a Deny of {action:?} for {principal}");
                let conditioned = document(vec![deny(
                    principal.clone(),
                    action,
                    &arn,
                    Some(condition.clone()),
                )]);

                let (status, body) = call(
                    &router,
                    "PutResourcePolicy",
                    &json!({"ResourceArn": arn, "Policy": conditioned}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: a conditioned deny needs no confirmation, got {status} {body}"
                );
                let attached = revision_id(&case, &body);

                // "If you issue a GetResourcePolicy request immediately after a
                // PutResourcePolicy request, DynamoDB might return your previous policy,
                // if there was one, or return the PolicyNotFoundException" (the
                // `PutResourcePolicy` doc): the attach is not applied yet, and this
                // resource carried no policy before it.
                let (status, body) =
                    call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "PolicyNotFoundException"),
                    "case {case}: the attach is not readable yet, got {status} {body}"
                );

                // The revision the delete removed is what it reports, which is the
                // revision the attach minted.
                let removed = ok(
                    &router,
                    "DeleteResourcePolicy",
                    json!({"ResourceArn": arn}),
                )
                .await;
                assert_eq!(
                    removed["RevisionId"],
                    json!(attached),
                    "case {case}: the delete reports the revision id it removed, got {removed}"
                );

                // "If you issue a GetResourcePolicy request immediately after a
                // DeleteResourcePolicy request, DynamoDB might return the policy that was
                // present before the deletion request" (the `GetResourcePolicy` doc), so
                // the document round trips through the reader exactly as it was attached.
                let read = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
                assert_eq!(
                    read["Policy"],
                    json!(conditioned),
                    "case {case}: the document is reported as it was attached, got {read}"
                );
                assert_eq!(
                    read["RevisionId"],
                    json!(attached),
                    "case {case}: the reader reports the revision id the attach returned, got \
                     {read}"
                );

                // The SAME statement without the condition denies those actions in every
                // request context, so it still needs the confirmation.
                let unconditioned = document(vec![deny(principal.clone(), action, &arn, None)]);
                let (status, body) = call(
                    &router,
                    "PutResourcePolicy",
                    &json!({"ResourceArn": arn, "Policy": unconditioned}),
                )
                .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "AccessDeniedException"),
                    "case {case} with the Condition removed: expected 400 \
                     AccessDeniedException, got {status} {body}"
                );
                assert_eq!(
                    message(&body),
                    LOCK_OUT_MESSAGE,
                    "case {case} with the Condition removed: got {:?}",
                    message(&body)
                );
            }
        }
    }
}

/// One multi-statement row: the statements to attach, and whether the document takes the
/// caller's policy-management access away in every request context.
struct DocumentCase {
    case: &'static str,
    statements: Vec<Value>,
    needs_confirmation: bool,
}

/// A document is judged by whether ANY of its statements denies the caller the
/// policy-management actions unconditionally — whatever the order of its statements, and
/// whatever its conditioned statements say.
#[tokio::test]
async fn a_document_is_judged_by_its_unconditioned_statements() {
    let router = emulator();
    create_active_table(&router, "Documents", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Documents");
    let condition = json!({"StringEquals": {"aws:RequestedRegion": "eu-west-1"}});
    let conditioned_management = deny(
        json!("*"),
        "dynamodb:PutResourcePolicy",
        &arn,
        Some(condition.clone()),
    );
    let unconditioned_management = deny(json!("*"), "dynamodb:DeleteResourcePolicy", &arn, None);
    let unconditioned_data = deny(json!("*"), "dynamodb:GetItem", &arn, None);
    let conditioned_wildcard = deny(json!("*"), "*", &arn, Some(condition));

    let cases = vec![
        DocumentCase {
            case: "one conditioned management deny",
            statements: vec![conditioned_management.clone()],
            needs_confirmation: false,
        },
        DocumentCase {
            case: "a conditioned management deny beside an unconditioned data deny",
            statements: vec![conditioned_management.clone(), unconditioned_data.clone()],
            needs_confirmation: false,
        },
        DocumentCase {
            case: "a conditioned wildcard deny beside an unconditioned data deny",
            statements: vec![unconditioned_data.clone(), conditioned_wildcard],
            needs_confirmation: false,
        },
        DocumentCase {
            case: "a conditioned management deny before an unconditioned one",
            statements: vec![
                conditioned_management.clone(),
                unconditioned_management.clone(),
            ],
            needs_confirmation: true,
        },
        DocumentCase {
            case: "a conditioned management deny after an unconditioned one",
            statements: vec![unconditioned_management.clone(), conditioned_management],
            needs_confirmation: true,
        },
        DocumentCase {
            case: "an unconditioned management deny beside an unconditioned data deny",
            statements: vec![unconditioned_data, unconditioned_management],
            needs_confirmation: true,
        },
    ];

    for DocumentCase {
        case,
        statements,
        needs_confirmation,
    } in cases
    {
        let policy = document(statements);
        let (status, body) = call(
            &router,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": policy}),
        )
        .await;
        if needs_confirmation {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected 400 AccessDeniedException, got {status} {body}"
            );
            assert_eq!(
                message(&body),
                LOCK_OUT_MESSAGE,
                "case {case}: got {:?}",
                message(&body)
            );
            // Refused, so nothing was attached and the next row starts from no policy:
            // the reader is asked once the previous row's removal has been applied, which
            // the end of the loop waits for.
            let (status, body) =
                call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "PolicyNotFoundException"),
                "case {case}: the refused attach stored nothing, got {status} {body}"
            );
            // The same document WITH the confirmation is accepted: the member is what
            // makes the choice deliberate.
            let confirmed = ok(
                &router,
                "PutResourcePolicy",
                json!({
                    "ResourceArn": arn,
                    "Policy": policy,
                    "ConfirmRemoveSelfResourceAccess": true,
                }),
            )
            .await;
            revision_id(case, &confirmed);
        } else {
            assert_eq!(
                status, 200,
                "case {case}: no statement locks the caller out, got {status} {body}"
            );
            let attached = revision_id(case, &body);
            // The attach is asynchronous, so the reader is asked once it is applied.
            wait_for_policy_application().await;
            let read = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
            assert_eq!(
                read["Policy"],
                json!(policy),
                "case {case}: the document is reported as it was attached, got {read}"
            );
            assert_eq!(
                read["RevisionId"],
                json!(attached),
                "case {case}: the reader reports the attach's revision id, got {read}"
            );
        }
        ok(
            &router,
            "DeleteResourcePolicy",
            json!({"ResourceArn": arn}),
        )
        .await;
        // The removal is asynchronous too, so the next row starts from a resource the
        // reader reports no policy for.
        wait_for_policy_application().await;
    }
}

/// Every resource-policy operation names a resource it cannot find as the TABLE it is,
/// by the bare name inside the `ResourceArn` the request carried.
#[tokio::test]
async fn the_resource_policy_surface_names_the_missing_table_by_name() {
    let router = emulator();
    // The rows differ along the length and the character classes of the name, which is
    // the request-derived part of the message: a DynamoDB table name is
    // `[A-Za-z0-9_.-]{3,255}`, so the shortest and the longest name are both rows.
    let names = [
        "Missing".to_string(),
        "abc".to_string(),
        "a.table-with_every.class-0".to_string(),
        "z".repeat(255),
    ];
    // A table that DOES exist, so the rows refuse for the one reason under test.
    create_active_table(&router, "Present", &[("pk", "HASH", "S")]).await;
    let present = table_arn("Present");
    let policy = document(vec![deny(
        json!("*"),
        "dynamodb:GetItem",
        &present,
        None,
    )]);

    for name in names {
        let arn = table_arn(&name);
        let requests = [
            ("GetResourcePolicy", json!({"ResourceArn": arn})),
            (
                "PutResourcePolicy",
                json!({"ResourceArn": arn, "Policy": policy}),
            ),
            ("DeleteResourcePolicy", json!({"ResourceArn": arn})),
        ];
        for (operation, request) in requests {
            let case = format!("{operation} against a table name {} characters long", name.len());
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: expected 400 ResourceNotFoundException, got {status} {body}"
            );
            assert_eq!(
                message(&body),
                format!("Requested resource not found: Table: {name} not found"),
                "case {case}: expected the table named by the bare name in the ARN, got {:?}",
                message(&body)
            );
        }
    }

    // A STREAM ARN of a table that is not there is the same miss: the table is what is
    // absent, so it is what the refusal names.
    let arn = format!("{}/stream/2026-01-01T00:00:00.000", table_arn("Missing"));
    let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case a stream ARN of an absent table: expected 400 ResourceNotFoundException, got \
         {status} {body}"
    );
    assert_eq!(
        message(&body),
        "Requested resource not found: Table: Missing not found",
        "case a stream ARN of an absent table: expected the absent table to be named, got {:?}",
        message(&body)
    );
}
