// SPDX-License-Identifier: CC0-1.0

//! What a resource-based policy's statement `Condition` decides, and what an eventually
//! consistent read of that policy reports.
//!
//! Two rules, each asserted over a SET of inputs rather than one example:
//!
//! 1. a statement's `Condition` is decided against the condition keys the request itself
//!    carries values for, so the statement applies to the requests its condition selects
//!    and to no others: a `Deny` whose condition the request does not satisfy refuses
//!    nothing, while one whose condition the request satisfies refuses it — and a clause
//!    over a key the request carries no value for cannot be decided at all, which keeps a
//!    refusal (the fail-closed reading of an undecided statement). The rows differ along
//!    the condition operator, the set qualifier, the number of values the clause lists,
//!    the negation, the condition key, the partition the request addresses and the
//!    operation that addresses it;
//! 2. `GetResourcePolicy` "follows an eventually consistent model": inside the span the
//!    newest write takes to be applied it reports the revision the resource carried
//!    BEFORE that write — "your previous policy, if there was one, or … the
//!    PolicyNotFoundException" after an attach (the `PutResourcePolicy` doc) and "the
//!    policy that was present before the deletion request" after a delete (the
//!    `GetResourcePolicy` doc) — and the stored revision once that span has elapsed. The
//!    rows differ along the kind of write (a first attach, a revision of an attached
//!    policy, an attach of the identical document, a removal, a removal against a
//!    resource with no policy).
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the
//! harness in [`super::table_items`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::table_items::{call, create_active_table, emulator, ok};
use crate::emulator::error_code;

/// The ARN prefix every table of this emulator carries, in the region the harness's
/// credential scope names.
const TABLE_ARN_PREFIX: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/";

/// The documented bound on a `PolicyRevisionId`
/// (`smithy_query(query="shape",shape="PolicyRevisionId")` -> string, length 1..255).
const MAX_POLICY_REVISION_ID_LENGTH: usize = 255;

/// The partitions every table in this suite holds one item in, so a row can address one
/// the policy selects and one it does not.
const PARTITIONS: [&str; 3] = ["secret", "private", "public"];

/// The ARN of one of this emulator's tables.
fn table_arn(name: &str) -> String {
    format!("{TABLE_ARN_PREFIX}{name}")
}

/// Wait out the span a policy write takes to be applied, after which both the data plane
/// and the reader answer from it — the span the store itself owes it
/// (`crate::state::STATUS_TRANSITION`), so a change to that span does not invalidate this
/// wait, plus room for the round trips.
async fn wait_for_policy_application() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    )
    .await;
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A table holding one item per entry of [`PARTITIONS`].
async fn seeded_table(router: &MultiServiceRouter, name: &str) -> String {
    create_active_table(router, name, &[("pk", "HASH", "S")]).await;
    for partition in PARTITIONS {
        ok(
            router,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"pk": {"S": partition}, "v": {"S": format!("{partition}-1")}},
            }),
        )
        .await;
    }
    table_arn(name)
}

/// A document denying `actions` on `resource_arn` to every principal, gated on
/// `condition`.
fn conditioned_deny(resource_arn: &str, actions: Value, condition: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": actions,
            "Resource": resource_arn,
            "Condition": condition,
        }],
    })
    .to_string()
}

/// Attach `document` and wait until it is the revision in force.
async fn attach_and_apply(router: &MultiServiceRouter, resource_arn: &str, document: &str) {
    ok(
        router,
        "PutResourcePolicy",
        json!({
            "ResourceArn": resource_arn,
            "Policy": document,
            // A conditioned deny takes nobody's policy-management access away in every
            // request context, so the confirmation is not required; it is passed so a row
            // that denies `dynamodb:*` unconditionally would be accepted on the same path.
            "ConfirmRemoveSelfResourceAccess": true,
        }),
    )
    .await;
    wait_for_policy_application().await;
}

/// One document row: what the request states, and what a read of the stored policy
/// reports.
struct DocumentCase {
    case: &'static str,
    /// The document as the request states it.
    stated: String,
    /// The document a read reports, in the canonical form the store holds it in.
    reported: String,
}

/// The document a read reports is the one the request stated, in the canonical form: the
/// same members, in the order they were written, with every value's own type kept — and an
/// element the IAM grammar states as "either a single string or an array of strings" as
/// the one string it lists. Nothing is added to it and nothing is dropped from it.
#[tokio::test]
async fn the_stored_document_is_the_one_the_request_stated() {
    let router = emulator();
    create_active_table(&router, "Documents", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Documents");
    // The members a statement may state an element of either way, from the IAM policy
    // grammar reference: `Action`/`NotAction`, `Resource`/`NotResource`, a `Principal`
    // kind's values, and each key of each operator of a `Condition`.
    let cases = vec![
        DocumentCase {
            case: "a document stated with insignificant whitespace",
            stated: format!(
                "{{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {{\n      \
                 \"Effect\": \"Deny\",\n      \"Principal\": {{\"AWS\": \"*\"}},\n      \
                 \"Action\": \"dynamodb:GetItem\",\n      \"Resource\": \"{arn}\"\n    }}\n  \
                 ]\n}}"
            ),
            reported: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:GetItem",
                    "Resource": arn,
                }],
            })
            .to_string(),
        },
        DocumentCase {
            case: "one-element lists in every element that admits a list",
            stated: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "OneOfEach",
                    "Effect": "Deny",
                    "Principal": {"AWS": ["*"]},
                    "Action": ["dynamodb:GetItem"],
                    "Resource": [arn],
                    "Condition": {
                        "ForAllValues:StringEquals": {"dynamodb:LeadingKeys": ["secret"]},
                    },
                }],
            })
            .to_string(),
            reported: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "OneOfEach",
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:GetItem",
                    "Resource": arn,
                    "Condition": {
                        "ForAllValues:StringEquals": {"dynamodb:LeadingKeys": "secret"},
                    },
                }],
            })
            .to_string(),
        },
        DocumentCase {
            case: "lists of two values, and the negated elements",
            stated: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": ["arn:aws:iam::000000000000:role/a",
                                             "arn:aws:iam::000000000000:role/b"]},
                    "NotAction": ["dynamodb:GetItem", "dynamodb:Query"],
                    "NotResource": [arn, format!("{arn}/index/by-rank")],
                    "Condition": {
                        "ForAnyValue:StringEquals": {"dynamodb:LeadingKeys": ["a", "b"]},
                    },
                }],
            })
            .to_string(),
            // Every element lists two values, so each keeps the array it was stated as.
            reported: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": ["arn:aws:iam::000000000000:role/a",
                                             "arn:aws:iam::000000000000:role/b"]},
                    "NotAction": ["dynamodb:GetItem", "dynamodb:Query"],
                    "NotResource": [arn, format!("{arn}/index/by-rank")],
                    "Condition": {
                        "ForAnyValue:StringEquals": {"dynamodb:LeadingKeys": ["a", "b"]},
                    },
                }],
            })
            .to_string(),
        },
        DocumentCase {
            case: "a statement stated as one object rather than a list",
            stated: json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:DeleteItem",
                    "Resource": arn,
                },
            })
            .to_string(),
            reported: json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:DeleteItem",
                    "Resource": arn,
                },
            })
            .to_string(),
        },
        DocumentCase {
            case: "a document that states no Version",
            // The `Version` such a document is READ under is no member of it, so the
            // read reports the document without one.
            stated: json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:Scan",
                    "Resource": arn,
                }],
            })
            .to_string(),
            reported: json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "dynamodb:Scan",
                    "Resource": arn,
                }],
            })
            .to_string(),
        },
    ];

    for DocumentCase {
        case,
        stated,
        reported,
    } in cases
    {
        attach_and_apply(&router, &arn, &stated).await;
        let read = read_policy(&router, &arn).await;
        assert_eq!(
            read.as_ref().map(|(document, _)| document.as_str()),
            Some(reported.as_str()),
            "case {case}: expected the document the request stated, got {read:?}"
        );
    }
}

/// One condition row: the clause attached, and the partitions a `GetItem` of which it
/// then refuses.
struct ConditionCase {
    case: &'static str,
    condition: Value,
    denied: &'static [&'static str],
}

/// A statement `Condition` decides WHICH requests the statement is about: the ones whose
/// context satisfies it.
///
/// The condition keys a data-plane call of this service carries values for include the
/// partition it addresses (`dynamodb:LeadingKeys`), which is what the fine-grained
/// per-partition pattern is written over.
#[tokio::test]
async fn a_conditioned_deny_refuses_only_the_requests_its_condition_selects() {
    let router = emulator();
    let arn = seeded_table(&router, "Conditions").await;

    // The rows differ along the operator (equality, inequality, pattern), the set
    // qualifier, how many values the clause lists, and the condition key itself — the
    // operators and keys are the ones the IAM policy reference publishes, and include
    // values and spellings no recorded case used.
    let cases = vec![
        ConditionCase {
            case: "ForAllValues:StringEquals on one leading key",
            condition: json!({"ForAllValues:StringEquals": {"dynamodb:LeadingKeys": "secret"}}),
            denied: &["secret"],
        },
        ConditionCase {
            case: "ForAllValues:StringEquals on two leading keys",
            condition: json!({
                "ForAllValues:StringEquals": {"dynamodb:LeadingKeys": ["secret", "private"]},
            }),
            denied: &["secret", "private"],
        },
        ConditionCase {
            case: "ForAnyValue:StringEquals on one leading key",
            condition: json!({"ForAnyValue:StringEquals": {"dynamodb:LeadingKeys": ["private"]}}),
            denied: &["private"],
        },
        ConditionCase {
            case: "an unqualified StringEquals on the leading key",
            condition: json!({"StringEquals": {"dynamodb:LeadingKeys": "public"}}),
            denied: &["public"],
        },
        ConditionCase {
            case: "a negated StringNotEquals on the leading key",
            condition: json!({"StringNotEquals": {"dynamodb:LeadingKeys": "secret"}}),
            denied: &["private", "public"],
        },
        ConditionCase {
            case: "StringLike over a pattern of leading keys",
            condition: json!({"StringLike": {"dynamodb:LeadingKeys": "p*"}}),
            denied: &["private", "public"],
        },
        ConditionCase {
            case: "StringEquals on a key this request carries no value for",
            // Undecidable, so the deny is never weakened into permission: every
            // partition stays refused.
            condition: json!({"StringEquals": {"aws:SourceVpc": "vpc-0"}}),
            denied: &PARTITIONS,
        },
    ];

    for ConditionCase {
        case,
        condition,
        denied,
    } in cases
    {
        // Each row revises the policy the row before it attached, so one span per row is
        // waited out rather than one per write.
        attach_and_apply(
            &router,
            &arn,
            &conditioned_deny(&arn, json!("dynamodb:GetItem"), condition),
        )
        .await;
        let denied: BTreeSet<&str> = denied.iter().copied().collect();

        for partition in PARTITIONS {
            let (status, body) = call(
                &router,
                "GetItem",
                &json!({"TableName": "Conditions", "Key": {"pk": {"S": partition}}}),
            )
            .await;
            if denied.contains(partition) {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "AccessDeniedException"),
                    "case {case}, a read of {partition:?}: expected the condition to select it, \
                     got {status} {body}"
                );
                assert_eq!(
                    message_of(&body),
                    format!(
                        "User: arn:aws:iam::000000000000:user/emulator is not authorized to \
                         perform: dynamodb:GetItem on resource: {arn} with an explicit deny in a \
                         resource-based policy"
                    ),
                    "case {case}, a read of {partition:?}: expected the explicit-deny sentence, \
                     got {:?}",
                    message_of(&body)
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case}, a read of {partition:?}: the condition does not select it, so \
                     the read is served, got {status} {body}"
                );
                assert_eq!(
                    body["Item"]["v"],
                    json!({"S": format!("{partition}-1")}),
                    "case {case}, a read of {partition:?}: the stored item is reported, got {body}"
                );
            }
        }
    }
}

/// One operation row: the request, the partition it addresses, and whether the
/// conditioned deny refuses it.
struct AddressedCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    denied: bool,
}

/// The condition is decided against the partition the CALL addresses, whichever operation
/// addresses it — and a call that addresses no partition of its own carries no value for
/// the key, which cannot be decided and so stays refused.
#[tokio::test]
async fn the_addressed_partition_decides_each_operation() {
    let router = emulator();
    let table = "AddressedPartitions";
    let arn = seeded_table(&router, table).await;
    attach_and_apply(
        &router,
        &arn,
        &conditioned_deny(
            &arn,
            json!("dynamodb:*"),
            json!({"ForAllValues:StringEquals": {"dynamodb:LeadingKeys": "secret"}}),
        ),
    )
    .await;

    let cases = vec![
        AddressedCase {
            case: "a read of the selected partition",
            operation: "GetItem",
            request: json!({"TableName": table, "Key": {"pk": {"S": "secret"}}}),
            denied: true,
        },
        AddressedCase {
            case: "a read of another partition",
            operation: "GetItem",
            request: json!({"TableName": table, "Key": {"pk": {"S": "public"}}}),
            denied: false,
        },
        AddressedCase {
            case: "a query of the selected partition",
            operation: "Query",
            request: json!({
                "TableName": table,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "secret"}},
            }),
            denied: true,
        },
        AddressedCase {
            case: "a query of another partition",
            operation: "Query",
            request: json!({
                "TableName": table,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "private"}},
            }),
            denied: false,
        },
        AddressedCase {
            case: "a write to the selected partition",
            operation: "PutItem",
            request: json!({
                "TableName": table,
                "Item": {"pk": {"S": "secret"}, "v": {"S": "secret-2"}},
            }),
            denied: true,
        },
        AddressedCase {
            case: "a write to another partition",
            operation: "PutItem",
            request: json!({
                "TableName": table,
                "Item": {"pk": {"S": "public"}, "v": {"S": "public-2"}},
            }),
            denied: false,
        },
        AddressedCase {
            case: "a delete of the selected partition's item",
            operation: "DeleteItem",
            request: json!({"TableName": table, "Key": {"pk": {"S": "secret"}}}),
            denied: true,
        },
        AddressedCase {
            case: "a delete of another partition's item",
            operation: "DeleteItem",
            request: json!({"TableName": table, "Key": {"pk": {"S": "private"}}}),
            denied: false,
        },
        AddressedCase {
            // A batch that reaches ONLY the selected partition carries one value for the
            // key, which the clause's set covers.
            case: "a batch read of the selected partition alone",
            operation: "BatchGetItem",
            request: json!({
                "RequestItems": {table: {"Keys": [{"pk": {"S": "secret"}}]}},
            }),
            denied: true,
        },
        AddressedCase {
            // "ForAllValues — tests whether the value of every member of the request set
            // is a subset of the condition key set" (the IAM policy reference), so a
            // batch that also reaches a partition the clause does not list makes the
            // condition false and the statement does not apply to it.
            case: "a batch read spanning the selected partition and another",
            operation: "BatchGetItem",
            request: json!({
                "RequestItems": {
                    table: {"Keys": [{"pk": {"S": "secret"}}, {"pk": {"S": "public"}}]},
                },
            }),
            denied: false,
        },
        AddressedCase {
            case: "a table walk, which addresses no partition of its own",
            operation: "Scan",
            request: json!({"TableName": table}),
            denied: true,
        },
    ];

    for AddressedCase {
        case,
        operation,
        request,
        denied,
    } in cases
    {
        let (status, body) = call(&router, operation, &request).await;
        if denied {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {case} ({operation}): expected the conditioned deny to refuse it, got \
                 {status} {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case} ({operation}): the condition does not select it, so it is served, \
                 got {status} {body}"
            );
        }
    }
}

/// What one write leaves for the reader to report, inside its application span and once
/// it has elapsed.
struct WriteCase {
    case: &'static str,
    /// The document to attach, or `None` for a removal.
    document: Option<String>,
    /// The document the reader reports INSIDE the span: the one the resource carried
    /// before this write, or `None` for `PolicyNotFoundException`.
    inside_span: Option<String>,
    /// The document the reader reports once the span has elapsed.
    past_span: Option<String>,
}

/// One document denying `action`, so a row can revise the policy with a different
/// document.
fn deny_of(resource_arn: &str, action: &str) -> String {
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

/// The revision id of the policy the reader reports, or `None` when it reports none.
async fn read_policy(router: &MultiServiceRouter, arn: &str) -> Option<(String, String)> {
    let (status, body) = call(router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    if status == 400 {
        assert_eq!(
            error_code(&body).as_str(),
            "PolicyNotFoundException",
            "case a read of {arn}: a resource with no policy in force is PolicyNotFound, got \
             {body}"
        );
        return None;
    }
    assert_eq!(status, 200, "case a read of {arn}: got {status} {body}");
    let document = body["Policy"]
        .as_str()
        .unwrap_or_else(|| panic!("case a read of {arn}: a 200 reports a Policy, got {body}"))
        .to_string();
    let revision_id = body["RevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case a read of {arn}: a 200 reports a RevisionId, got {body}"))
        .to_string();
    Some((document, revision_id))
}

/// `GetResourcePolicy` reports the revision in force: inside the span of the newest write
/// the one the resource carried before it, and the stored one once that span has elapsed.
/// Every successful write mints a revision id of its own, and re-attaching the identical
/// document keeps the one it already has.
#[tokio::test]
async fn the_policy_reader_reports_the_revision_in_force() {
    let router = emulator();
    create_active_table(&router, "Lagging", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Lagging");
    let first = deny_of(&arn, "dynamodb:GetItem");
    let second = deny_of(&arn, "dynamodb:PutItem");

    let cases = vec![
        WriteCase {
            case: "a first attach, over a resource with no policy",
            document: Some(first.clone()),
            inside_span: None,
            past_span: Some(first.clone()),
        },
        WriteCase {
            case: "an attach of the identical document",
            document: Some(first.clone()),
            inside_span: Some(first.clone()),
            past_span: Some(first.clone()),
        },
        WriteCase {
            case: "a revision of the attached policy",
            document: Some(second.clone()),
            inside_span: Some(first.clone()),
            past_span: Some(second.clone()),
        },
        WriteCase {
            case: "a removal of the attached policy",
            document: None,
            inside_span: Some(second.clone()),
            past_span: None,
        },
        WriteCase {
            case: "a removal against a resource with no policy",
            document: None,
            inside_span: None,
            past_span: None,
        },
    ];

    // The revision ids the writes mint, in order, so each can be held against the one it
    // replaced.
    let mut minted: Vec<String> = Vec::new();
    for WriteCase {
        case,
        document,
        inside_span,
        past_span,
    } in cases
    {
        let reported = match &document {
            Some(document) => {
                let written = ok(
                    &router,
                    "PutResourcePolicy",
                    json!({
                        "ResourceArn": arn,
                        "Policy": document,
                        "ConfirmRemoveSelfResourceAccess": true,
                    }),
                )
                .await;
                written["RevisionId"].as_str().map(str::to_string)
            }
            None => {
                let removed = ok(
                    &router,
                    "DeleteResourcePolicy",
                    json!({"ResourceArn": arn}),
                )
                .await;
                removed["RevisionId"].as_str().map(str::to_string)
            }
        };

        // Inside the span the reader answers from the revision the resource carried
        // before this write.
        let lagging = read_policy(&router, &arn).await;
        assert_eq!(
            lagging.as_ref().map(|(document, _)| document.clone()),
            inside_span,
            "case {case}: inside the application span the reader reports the revision that was \
             there before the write, got {lagging:?}"
        );

        wait_for_policy_application().await;
        let settled = read_policy(&router, &arn).await;
        assert_eq!(
            settled.as_ref().map(|(document, _)| document.clone()),
            past_span,
            "case {case}: once the span has elapsed the reader reports the stored revision, got \
             {settled:?}"
        );

        match (&reported, &settled) {
            // A write that attached a document: the reader reports the revision id the
            // write returned, and that id is a `PolicyRevisionId`.
            (Some(reported), Some((_, settled_revision))) => {
                assert_eq!(
                    reported, settled_revision,
                    "case {case}: the reader reports the revision id the write returned, got \
                     {settled_revision:?}"
                );
                assert!(
                    (1..=MAX_POLICY_REVISION_ID_LENGTH).contains(&reported.chars().count()),
                    "case {case}: a RevisionId is 1..{MAX_POLICY_REVISION_ID_LENGTH} characters, \
                     got {reported:?}"
                );
            }
            // A removal reports the revision it removed, which is the one that was in
            // force before it.
            (Some(reported), None) => assert!(
                minted.contains(reported),
                "case {case}: the removal reports a revision this suite attached, got \
                 {reported:?} against {minted:?}"
            ),
            // "This value will be empty if you make a request against a resource without
            // a policy" (`DeleteResourcePolicyOutput$RevisionId`).
            (None, _) => assert!(
                document.is_none(),
                "case {case}: an attach always reports a revision id"
            ),
        }
        if let Some(reported) = reported {
            if !minted.contains(&reported) {
                minted.push(reported);
            }
        }
    }

    // Each attach of a DIFFERENT document minted a revision id of its own, and the
    // identical re-attach reported the one it already had — so the ids are as many as the
    // documents attached.
    assert_eq!(
        minted.len(),
        2,
        "case the whole lifecycle: one revision id per attached document, got {minted:?}"
    );
    assert_ne!(
        minted.first(),
        minted.last(),
        "case the whole lifecycle: a different document gets a different revision id, got \
         {minted:?}"
    );
}
