// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE SENTENCE a refused `RedriveAllowPolicy` document is answered
//! with, driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`)
//! through the shared harness in [`super::support`].
//!
//! The rule under test: **every refusal of a `RedriveAllowPolicy` document states the rule
//! that document broke in the sentence Amazon SQS words for THAT rule — never a restatement
//! of the rule in other words — with the document the request supplied quoted back
//! byte-for-byte as the template's only variable part, so a client that tells these refusals
//! apart by their text (they share one error code and one `Reason` prefix) is sent to the one
//! member it has to change.**
//!
//! The refusals of this one attribute are what makes the wording load-bearing: the missing
//! list, the list supplied under the wrong permission type and the list past the published
//! ceiling all arrive as `InvalidParameterValue` at HTTP 400 with the same
//! `Value … for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the
//! redrive allow policy.` opening, so the clause that follows it is the only thing that says
//! which of the three happened.
//!
//! The table spans what the rule generalises over rather than one example: the RULE broken
//! (the list's presence, its coupling to the permission type, its arity, the permission
//! type's own value), the FORM the document is written in (compact, spaced, pretty-printed,
//! members in either order), the permission type it names, the number of ARNs it lists (none,
//! one, the published ceiling, one past it, far past it), and both paths that write a queue
//! attribute — with the documents the rules ADMIT carried alongside, so a service that
//! refused everything with the right sentence would fail the table too.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{all_attributes, call, create, create_ok, emulator, error_code, expected_arn};

/// The attribute whose refusals are under test.
const REDRIVE_ALLOW_POLICY: &str = "RedriveAllowPolicy";

/// The class a refused parameter VALUE carries: the recorded 400 response of each document
/// refusal of this attribute names it, and the model declares no `InvalidParameterValue` for
/// `SetQueueAttributes`, so the pairing is the service's recorded behaviour
/// (`smithy_query(query="operation", operation="SetQueueAttributes",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`
/// lists `InvalidAttributeName` and `InvalidAttributeValue` only).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The class a refused attribute VALUE carries — the modelled `InvalidAttributeValue` of
/// `CreateQueue`/`SetQueueAttributes`, which is how the refusal of a value INSIDE the
/// document is answered.
const INVALID_ATTRIBUTE_VALUE: &str = "InvalidAttributeValue";

/// The sentence Amazon SQS answers a `byQueue` permission naming no source queue with,
/// spelled out here rather than imported so a drifting constant in the service cannot make a
/// row pass. Source: the recorded 400 response of this refusal, whose text is wholly fixed.
const BY_QUEUE_NEEDS_SOURCES: &str =
    "Amazon SQS can't create the redrive allow policy. When you specify the byQueue permission \
     type, you must also specify one or more sourceQueueArns values.";

/// The sentence for the other half of the same coupling: a list supplied under a permission
/// type that takes none, worded for the member the caller has to set.
const SOURCES_NEED_BY_QUEUE: &str =
    "Amazon SQS can't create the redrive allow policy. To specify one or more source queue \
     ARNs, you must set redrivePermission to byQueue.";

/// The sentence for a list longer than the member admits, stating the published ceiling.
/// Source: the `sourceQueueArns` member's own documentation — "You can specify up to 10
/// source queue ARNs" (`smithy_query(query="doc",
/// shape="com.amazonaws.sqs#SetQueueAttributesRequest", member="Attributes")`).
const SOURCE_LIMIT: &str =
    "Amazon SQS can't create the redrive allow policy. The maximum number of source queue ARNs \
     you can specify is 10.";

/// The sentence for a `redrivePermission` naming none of the three permission types the
/// member publishes: a document the attribute cannot be read in at all. The contraction is
/// written with the character the service answers with.
const UNSUPPORTED_FORMAT: &str =
    "Amazon SQS can't create the redrive allow policy, as it\u{2019}s in an unsupported format.";

/// The published ceiling on the ARNs one permission lists, spelled out rather than imported,
/// from the same member documentation [`SOURCE_LIMIT`] cites.
const PUBLISHED_ARN_LIMIT: usize = 10;

/// The refused-parameter-value template this service answers a refused VALUE in: the document
/// the request supplied, the parameter it was supplied for, and the one rule it broke.
fn parameter_refusal(supplied: &str, reason: &str) -> String {
    format!("Value {supplied} for parameter RedriveAllowPolicy is invalid. Reason: {reason}")
}

/// The refused-attribute-value template, which names the attribute and the rule rather than
/// the document: what was refused is one MEMBER of it.
fn attribute_refusal(reason: &str) -> String {
    format!("Invalid value for the parameter RedriveAllowPolicy. Reason: {reason}")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// `count` source queue ARNs of this region, each naming a different queue.
fn source_arns(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| expected_arn(&format!("wording-source-{index}")))
        .collect()
}

/// `count` source queue ARNs as the JSON list a document carries them in.
fn arn_list(count: usize) -> String {
    serde_json::to_string(&source_arns(count)).expect("the ARN list serializes")
}

/// Write `document` onto the queue at `url`, answering `(status, body)`.
async fn set_allow_policy(
    router: &MultiServiceRouter,
    url: &str,
    document: &str,
) -> (u16, Value) {
    call(
        router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {REDRIVE_ALLOW_POLICY: document}}),
    )
    .await
}

/// One row: the document a write submits, and the refusal it must draw.
struct WordingRow {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// The document the write submits, exactly as it goes on the wire.
    document: String,
    /// The refusal the document must draw, as `(code, reason sentence)`, or `None` for a
    /// document both write paths must accept.
    refusal: Option<(&'static str, &'static str)>,
}

/// Every document rule of this attribute, each written in a form the request chose itself, so
/// a refusal is judged on its sentence and on the document it quotes back and not on one
/// spelling.
fn rows() -> Vec<WordingRow> {
    vec![
        // The list's presence under the permission that admits exactly what it names.
        WordingRow {
            case: "byQueue naming no list at all",
            document: r#"{"redrivePermission": "byQueue"}"#.to_string(),
            refusal: Some((INVALID_PARAMETER_VALUE, BY_QUEUE_NEEDS_SOURCES)),
        },
        WordingRow {
            case: "byQueue naming no list, written compactly",
            document: r#"{"redrivePermission":"byQueue"}"#.to_string(),
            refusal: Some((INVALID_PARAMETER_VALUE, BY_QUEUE_NEEDS_SOURCES)),
        },
        WordingRow {
            case: "byQueue naming no list, pretty-printed over three lines",
            document: "{\n    \"redrivePermission\" : \"byQueue\"\n}".to_string(),
            refusal: Some((INVALID_PARAMETER_VALUE, BY_QUEUE_NEEDS_SOURCES)),
        },
        WordingRow {
            case: "byQueue listing nothing",
            document: r#"{"redrivePermission": "byQueue", "sourceQueueArns": []}"#.to_string(),
            refusal: Some((INVALID_PARAMETER_VALUE, BY_QUEUE_NEEDS_SOURCES)),
        },
        WordingRow {
            case: "byQueue listing nothing, the list written first",
            document: r#"{"sourceQueueArns": [], "redrivePermission": "byQueue"}"#.to_string(),
            refusal: Some((INVALID_PARAMETER_VALUE, BY_QUEUE_NEEDS_SOURCES)),
        },
        // The other half of the coupling: the list belongs to that permission alone.
        WordingRow {
            case: "one ARN listed under allowAll",
            document: format!(
                r#"{{"redrivePermission": "allowAll", "sourceQueueArns": {}}}"#,
                arn_list(1)
            ),
            refusal: Some((INVALID_PARAMETER_VALUE, SOURCES_NEED_BY_QUEUE)),
        },
        WordingRow {
            case: "three ARNs listed under denyAll",
            document: format!(
                r#"{{"sourceQueueArns": {}, "redrivePermission": "denyAll"}}"#,
                arn_list(3)
            ),
            refusal: Some((INVALID_PARAMETER_VALUE, SOURCES_NEED_BY_QUEUE)),
        },
        WordingRow {
            case: "a list on a document naming no permission type",
            document: format!(r#"{{"sourceQueueArns": {}}}"#, arn_list(1)),
            refusal: Some((INVALID_PARAMETER_VALUE, SOURCES_NEED_BY_QUEUE)),
        },
        // The list's arity, one past the published ceiling and far past it.
        WordingRow {
            case: "byQueue listing one ARN past the published ceiling",
            document: format!(
                r#"{{"redrivePermission": "byQueue", "sourceQueueArns": {}}}"#,
                arn_list(PUBLISHED_ARN_LIMIT + 1)
            ),
            refusal: Some((INVALID_PARAMETER_VALUE, SOURCE_LIMIT)),
        },
        WordingRow {
            case: "byQueue listing far more ARNs than the published ceiling",
            document: format!(
                r#"{{"redrivePermission":"byQueue","sourceQueueArns":{}}}"#,
                arn_list(PUBLISHED_ARN_LIMIT * 3)
            ),
            refusal: Some((INVALID_PARAMETER_VALUE, SOURCE_LIMIT)),
        },
        // The permission type's own value, refused as a value INSIDE the document.
        WordingRow {
            case: "a permission type the member does not publish",
            document: r#"{"redrivePermission": "allowSome"}"#.to_string(),
            refusal: Some((INVALID_ATTRIBUTE_VALUE, UNSUPPORTED_FORMAT)),
        },
        WordingRow {
            case: "a published permission type spelled in the wrong case",
            document: r#"{"redrivePermission": "byqueue"}"#.to_string(),
            refusal: Some((INVALID_ATTRIBUTE_VALUE, UNSUPPORTED_FORMAT)),
        },
        WordingRow {
            case: "a permission type that is not text at all",
            document: r#"{"redrivePermission": 7}"#.to_string(),
            refusal: Some((INVALID_ATTRIBUTE_VALUE, UNSUPPORTED_FORMAT)),
        },
        // The documents the rules admit: the refusals above must not swallow them.
        WordingRow {
            case: "byQueue listing one source queue",
            document: format!(
                r#"{{"redrivePermission": "byQueue", "sourceQueueArns": {}}}"#,
                arn_list(1)
            ),
            refusal: None,
        },
        WordingRow {
            case: "byQueue listing exactly the published ceiling",
            document: format!(
                r#"{{"redrivePermission": "byQueue", "sourceQueueArns": {}}}"#,
                arn_list(PUBLISHED_ARN_LIMIT)
            ),
            refusal: None,
        },
        WordingRow {
            case: "allowAll, the documented default, carrying no list",
            document: r#"{"redrivePermission": "allowAll"}"#.to_string(),
            refusal: None,
        },
        WordingRow {
            case: "denyAll carrying no list",
            document: r#"{"redrivePermission":"denyAll"}"#.to_string(),
            refusal: None,
        },
    ]
}

/// Each refusal of a `RedriveAllowPolicy` document states the rule THAT document broke, in
/// the sentence Amazon SQS words for it, with the document quoted back exactly as sent —
/// through both paths that write a queue attribute, leaving the stored permission untouched.
#[tokio::test]
async fn a_refused_redrive_allow_policy_states_its_own_rule_in_amazon_sqs_wording() {
    for (index, row) in rows().into_iter().enumerate() {
        let WordingRow {
            case,
            document,
            refusal,
        } = row;
        let router = emulator();
        let expected = refusal.map(|(code, reason)| {
            let message = if code == INVALID_ATTRIBUTE_VALUE {
                attribute_refusal(reason)
            } else {
                parameter_refusal(&document, reason)
            };
            (code, message)
        });

        // 1. The SetQueueAttributes path, onto a queue already carrying a permission: a
        // refused document must leave THAT permission in force.
        let held = r#"{"redrivePermission":"denyAll"}"#;
        let url = create_ok(
            &router,
            &format!("wording-{index}"),
            &json!({REDRIVE_ALLOW_POLICY: held}),
        )
        .await;
        let (status, parsed) = set_allow_policy(&router, &url, &document).await;
        let stored = all_attributes(&router, &url)
            .await
            .get(REDRIVE_ALLOW_POLICY)
            .cloned();
        match &expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case} on SetQueueAttributes: expected the document accepted, got \
                     {status} with {parsed}"
                );
                let reported: Option<Value> = stored
                    .as_deref()
                    .map(|stored| serde_json::from_str(stored).unwrap_or(Value::Null));
                assert_eq!(
                    reported,
                    serde_json::from_str::<Value>(&document).ok(),
                    "case {case}: expected the accepted document reported back, got {stored:?}"
                );
            }
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (400, *code, message.clone()),
                    "case {case} on SetQueueAttributes: expected the refusal to state its own \
                     rule in Amazon SQS's wording, got {status} with {parsed}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(held),
                    "case {case}: a refused document must leave the stored permission untouched, \
                     got {stored:?}"
                );
            }
        }

        // 2. The CreateQueue path, which judges the same document the same way and so must
        // answer it with the same sentence.
        let created = format!("wording-created-{index}");
        let (status, parsed) =
            create(&router, &created, &json!({REDRIVE_ALLOW_POLICY: document})).await;
        match &expected {
            None => assert_eq!(
                status, 200,
                "case {case} on CreateQueue: expected the document accepted, got {status} with \
                 {parsed}"
            ),
            Some((code, message)) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (400, *code, message.clone()),
                    "case {case} on CreateQueue: expected the refusal to state its own rule in \
                     Amazon SQS's wording, got {status} with {parsed}"
                );
                let (status, parsed) =
                    call(&router, "GetQueueUrl", &json!({"QueueName": created})).await;
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "QueueDoesNotExist"),
                    "case {case}: a refused create must leave no queue behind, got {status} with \
                     {parsed}"
                );
            }
        }
    }
}

/// No two of this attribute's document rules are reported with one sentence, and none of them
/// is reported with a restatement of another rule: the sentence is the only thing telling the
/// refusals of one error code and one `Reason` prefix apart.
#[tokio::test]
async fn each_redrive_allow_policy_document_rule_has_its_own_sentence() {
    let sentences: Vec<(&str, &str)> = vec![
        ("the list's presence under byQueue", BY_QUEUE_NEEDS_SOURCES),
        ("the list's coupling to byQueue", SOURCES_NEED_BY_QUEUE),
        ("the list's published ceiling", SOURCE_LIMIT),
        ("the permission type's own value", UNSUPPORTED_FORMAT),
    ];
    for (position, (rule, sentence)) in sentences.iter().enumerate() {
        for (other_rule, other) in sentences.iter().skip(position + 1) {
            assert_ne!(
                sentence, other,
                "case {rule} against {other_rule}: two rules must not share one sentence"
            );
        }
    }

    // The three refusals of one code and one prefix really do share that prefix, which is why
    // the clause after it has to differ.
    let shared = "Amazon SQS can't create the redrive allow policy. ";
    for (rule, sentence) in [
        ("the list's presence under byQueue", BY_QUEUE_NEEDS_SOURCES),
        ("the list's coupling to byQueue", SOURCES_NEED_BY_QUEUE),
        ("the list's published ceiling", SOURCE_LIMIT),
    ] {
        assert!(
            sentence.starts_with(shared),
            "case {rule}: expected the shared opening, got {sentence:?}"
        );
        assert!(
            sentence.len() > shared.len(),
            "case {rule}: expected a clause after the shared opening, got {sentence:?}"
        );
    }
}
