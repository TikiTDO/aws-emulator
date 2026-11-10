// SPDX-License-Identifier: CC0-1.0

//! The KEY an Amazon SQS queue target is encrypted with decides whether a rule's
//! delivery to that queue is enqueued at all, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter` with Amazon SQS and KMS mounted on one engine — so
//! the key is read where its own handlers wrote it and the outcome is asserted
//! where a client sees it: on the target queue, on a healthy sibling target, and
//! on the target's dead-letter queue.
//!
//! The RULE asserted here: a queue carrying `KmsMasterKeyId` has every message
//! sent to it encrypted under that key, and Amazon SQS performs that encryption by
//! calling `kms:GenerateDataKey` for the sender — so the KEY's own policy is a
//! second document in force over such a delivery, beside the queue's own `Policy`
//! (`tests/all/queue_target_policy_authorization.rs`), and a key policy that does
//! not admit the EventBridge service principal is a send that does not happen
//! however freely the queue's policy grants `sqs:SendMessage`. Nothing reaches the
//! queue; the delivery FAILED, so the event goes to the target's dead-letter queue
//! under the condition `NO_PERMISSIONS` carrying the two nested refusals the call
//! produced — KMS's denial of the `kms:GenerateDataKey` inside Amazon SQS's
//! `KMS.AccessDeniedException` — while another target of the same rule still
//! receives. A key policy written afterwards lets the next delivery through with
//! nothing else changed.
//!
//! It is the SAME rule the topic branch is asserted under
//! (`tests/all/encrypted_topic_target_key_policy.rs`), applied to the other target
//! kind whose service encrypts what it is handed, so the rows differ along the same
//! axes: what the key's policy states — whether the queue names a key at all,
//! whether a statement about the service principal exists, which action and
//! principal it is about, which `Effect` it states and which condition it is
//! scoped by. The `kms:ViaService` rows are the ones that are NOT shared: the
//! service the use arrives through is Amazon SQS here.
//!
//! The harness (the router with EventBridge, Amazon SQS, KMS, IAM and CloudWatch on
//! one engine, the queue fixtures and the rule fixture) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, NO_PERMISSIONS, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_rule, publish, rpc, sqs, Emulator, REGION,
};

/// The rule every test here publishes through.
const RULE: &str = "encrypted-queue-rule";

/// The attribute Amazon SQS reports an encrypted queue's key under
/// (`GetQueueAttributes`'s `KmsMasterKeyId`).
const KMS_MASTER_KEY_ID: &str = "KmsMasterKeyId";

/// The dead-letter attributes EventBridge stamps on an undeliverable event — the
/// EXACT set a refusal that is not retried carries.
const DEAD_LETTER_ATTRIBUTES: [&str; 4] =
    ["ERROR_CODE", "ERROR_MESSAGE", "RULE_ARN", "TARGET_ARN"];

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// What the two nested refusals read with every request id normalized away, which
/// is the whole of the requirement: the innermost sentence is KMS's for a denial
/// that can report nothing about the resource, wrapped in the metadata block of
/// the refused `kms:GenerateDataKey` and then in Amazon SQS's own
/// `KMS.AccessDeniedException`.
fn expected_message() -> String {
    format!(
        "User: {SERVICE_PRINCIPAL} is not authorized to perform: kms:GenerateDataKey on this \
         resource because the resource does not exist in this Region, no resource-based \
         policies allow access, or a resource-based policy explicitly denies access (Service: \
         AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: \
         null) (Service: AmazonSQS; Status Code: 400; Error Code: KMS.AccessDeniedException; \
         Request ID: <id>; Proxy: null)"
    )
}

/// `message` with each request id it reports replaced by `<id>`, asserting on the
/// way that each one has the 8-4-4-4-12 form an AWS request id carries: the ids
/// identify the two calls that were refused, so they differ per delivery and are
/// no part of the sentence.
fn normalized(case: &str, message: &str) -> String {
    const MARKER: &str = "Request ID: ";

    let mut rest = message;
    let mut out = String::with_capacity(message.len());
    let mut found = 0usize;
    while let Some(at) = rest.find(MARKER) {
        let (before, after) = rest.split_at(at + MARKER.len());
        out.push_str(before);
        let end = after
            .find(';')
            .unwrap_or_else(|| panic!("case {case:?}: an unterminated request id in {message:?}"));
        let id = &after[..end];
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {id:?}"
        );
        out.push_str("<id>");
        rest = &after[end..];
        found += 1;
    }
    out.push_str(rest);
    assert_eq!(
        found, 2,
        "case {case:?}: expected one request id per nested refusal, got {message:?}"
    );
    out
}

/// The statement every key policy carries beside what a row states: the account
/// root's own authority over the key, which is what `CreateKey` writes and what
/// the lockout safety check of `PutKeyPolicy` requires of a replacement.
fn root_statement() -> Value {
    json!({
        "Sid": "Enable IAM policies",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy document carrying the root statement and whatever `statements`
/// adds to it.
fn key_policy(statements: Vec<Value>) -> String {
    let mut all = vec![root_statement()];
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Id": "key-default", "Statement": all }).to_string()
}

/// A statement about the EventBridge service principal: `effect` of `actions`,
/// scoped by `condition` when the row states one.
fn about_events(effect: &str, actions: Value, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": { "Service": SERVICE_PRINCIPAL },
        "Action": actions,
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// One KMS call (`awsJson1_1`, `TrentService.<Operation>`) that must answer 200.
async fn kms(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "kms", "TrentService", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} answered {status}: {answered}"
    );
    answered
}

/// A customer managed key carrying `policy` when a row states one, answered as its
/// ARN.
async fn given_key(emulator: &Emulator, case: &str, policy: Option<String>) -> String {
    let created = kms(emulator, case, "CreateKey", &json!({})).await;
    let arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no KeyMetadata.Arn in {created}"))
        .to_string();
    if let Some(policy) = policy {
        kms(
            emulator,
            case,
            "PutKeyPolicy",
            &json!({ "KeyId": arn, "PolicyName": "default", "Policy": policy }),
        )
        .await;
    }
    arn
}

/// A queue called `name` whose own policy grants this service the send and which
/// is encrypted with `key` when the row names one; answers its url and ARN.
async fn given_encrypted_queue(
    emulator: &Emulator,
    case: &str,
    name: &str,
    key: Option<&str>,
) -> (String, String) {
    // The queue's OWN policy grants the send in every row: what is under test is
    // the key, so nothing about the queue's policy may withhold the delivery.
    let (url, arn) = given_queue(emulator, name).await;
    if let Some(key) = key {
        let (status, written) = sqs(
            emulator,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { KMS_MASTER_KEY_ID: key } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SetQueueAttributes answered {status}: {written}"
        );
    }
    (url, arn)
}

/// What a row's key is.
enum Key {
    /// The queue names no key at all.
    None,
    /// A key this account holds, carrying the policy the row states — `None`
    /// leaving the one `CreateKey` writes.
    Own(Option<Vec<Value>>),
}

/// One row: the case, the key the queue is encrypted with, and whether
/// EventBridge sends to it.
struct Row {
    case: &'static str,
    key: Key,
    delivered: bool,
}

/// The key's own policy decides each delivery to an encrypted queue target, over a
/// table of key policies.
#[tokio::test]
async fn an_encrypted_queue_target_receives_only_where_its_key_allows() {
    let rows = vec![
        Row {
            case: "a queue on no customer managed key",
            key: Key::None,
            delivered: true,
        },
        Row {
            case: "a key carrying only the policy CreateKey writes",
            key: Key::Own(None),
            delivered: false,
        },
        Row {
            case: "a key granting the service principal the data-key actions",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                None,
            )])),
            delivered: true,
        },
        Row {
            case: "a key granting the service principal every action of the service",
            key: Key::Own(Some(vec![about_events("Allow", json!("kms:*"), None)])),
            delivered: true,
        },
        Row {
            case: "a key granting the service principal another action only",
            key: Key::Own(Some(vec![about_events("Allow", json!("kms:Decrypt"), None)])),
            delivered: false,
        },
        Row {
            case: "a key whose grant to the service principal is denied beside it",
            key: Key::Own(Some(vec![
                about_events("Allow", json!("kms:GenerateDataKey*"), None),
                about_events("Deny", json!("kms:GenerateDataKey"), None),
            ])),
            delivered: false,
        },
        Row {
            case: "a key granting another service principal",
            key: Key::Own(Some(vec![json!({
                "Effect": "Allow",
                "Principal": { "Service": "s3.amazonaws.com" },
                "Action": "kms:GenerateDataKey*",
                "Resource": "*",
            })])),
            delivered: false,
        },
        Row {
            case: "a grant scoped by kms:ViaService to Amazon SQS in this Region",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!("kms:GenerateDataKey*"),
                Some(json!({
                    "StringEquals": { "kms:ViaService": format!("sqs.{REGION}.amazonaws.com") },
                })),
            )])),
            delivered: true,
        },
        Row {
            case: "a grant scoped by kms:ViaService to another service",
            key: Key::Own(Some(vec![about_events(
                "Allow",
                json!("kms:GenerateDataKey*"),
                Some(json!({
                    "StringEquals": { "kms:ViaService": format!("sns.{REGION}.amazonaws.com") },
                })),
            )])),
            delivered: false,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("encrypted-queue-dlq-{index}")).await;
        // A second target of the same rule, on no key at all: a refusal the key's
        // policy decided stops THAT target's delivery and no other.
        let (sibling_url, sibling_arn) =
            given_queue(&emulator, &format!("encrypted-queue-sibling-{index}")).await;
        let key = match row.key {
            Key::None => None,
            Key::Own(statements) => {
                Some(given_key(&emulator, case, statements.map(key_policy)).await)
            }
        };
        let (target_url, target_arn) = given_encrypted_queue(
            &emulator,
            case,
            &format!("encrypted-queue-{index}"),
            key.as_deref(),
        )
        .await;
        given_rule(
            &emulator,
            RULE,
            json!([
                {
                    "Id": "encrypted",
                    "Arn": target_arn,
                    // No retry at all: a refusal the key's policy decided is
                    // permanent, so the dead letter carries no retry attributes.
                    "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                },
                { "Id": "sibling", "Arn": sibling_arn },
            ]),
        )
        .await;

        publish(&emulator).await;

        let received = drain(&emulator, &target_url).await;
        let expected_messages = usize::from(row.delivered);
        assert_eq!(
            received.len(),
            expected_messages,
            "case {case:?}: expected {expected_messages} message(s) on the encrypted queue, got \
             {received:?}"
        );
        let sibling = drain(&emulator, &sibling_url).await;
        assert_eq!(
            sibling.len(),
            1,
            "case {case:?}: the sibling target on no key receives whatever the key decides, got \
             {sibling:?}"
        );

        let dead_lettered = drain(&emulator, &dead_letter_url).await;
        let expected_dead_letters = usize::from(!row.delivered);
        assert_eq!(
            dead_lettered.len(),
            expected_dead_letters,
            "case {case:?}: expected {expected_dead_letters} dead letter(s), got \
             {dead_lettered:?}"
        );
        for message in &dead_lettered {
            let attributes = message["MessageAttributes"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: no MessageAttributes in {message}"));
            let mut names: Vec<&str> = attributes.keys().map(String::as_str).collect();
            names.sort_unstable();
            let mut expected = DEAD_LETTER_ATTRIBUTES.to_vec();
            expected.sort_unstable();
            assert_eq!(
                names, expected,
                "case {case:?}: expected exactly {expected:?} on the dead letter, got {message}"
            );
            assert_eq!(
                attributes["ERROR_CODE"]["StringValue"],
                json!(NO_PERMISSIONS),
                "case {case:?}: expected the dead letter to name the permission that stopped \
                 the send, got {message}"
            );
            assert_eq!(
                attributes["RULE_ARN"]["StringValue"],
                json!(rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE)),
                "case {case:?}: expected the dead letter to name the publishing rule, got \
                 {message}"
            );
            assert_eq!(
                attributes["TARGET_ARN"]["StringValue"],
                json!(target_arn),
                "case {case:?}: expected the dead letter to name the queue, got {message}"
            );
            let reported = attributes["ERROR_MESSAGE"]["StringValue"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                normalized(case, reported),
                expected_message(),
                "case {case:?}: expected KMS's own refusal of the data key, got {reported:?}"
            );
        }
    }
}

/// Each delivery is decided against the key policy in force AT THAT INSTANT: a
/// grant written after a refused delivery lets the next one through, with nothing
/// about the queue, the rule or the target changed in between — which is the
/// signal a client repairing a mis-scoped key policy watches for.
#[tokio::test]
async fn a_key_policy_written_after_a_refusal_lets_the_next_delivery_through() {
    let case = "a key policy granting the service principal after a refused delivery";
    let emulator = emulator();
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "repaired-queue-dlq").await;
    let key = given_key(&emulator, case, None).await;
    let (target_url, target_arn) =
        given_encrypted_queue(&emulator, case, "repaired-queue", Some(&key)).await;
    given_rule(
        &emulator,
        RULE,
        json!([{
            "Id": "encrypted",
            "Arn": target_arn,
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        }]),
    )
    .await;

    publish(&emulator).await;
    let withheld = drain(&emulator, &target_url).await;
    assert!(
        withheld.is_empty(),
        "case {case:?}: the key's policy admits this service nothing, so nothing may be \
         enqueued; got {withheld:?}"
    );
    let dead_lettered = drain(&emulator, &dead_letter_url).await;
    assert_eq!(
        dead_lettered.len(),
        1,
        "case {case:?}: expected the refused delivery on the dead-letter queue, got \
         {dead_lettered:?}"
    );

    kms(
        &emulator,
        case,
        "PutKeyPolicy",
        &json!({
            "KeyId": key,
            "PolicyName": "default",
            "Policy": key_policy(vec![about_events(
                "Allow",
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                None,
            )]),
        }),
    )
    .await;

    publish(&emulator).await;
    let received = drain(&emulator, &target_url).await;
    assert_eq!(
        received.len(),
        1,
        "case {case:?}: expected the message once the key policy granted the data key, got \
         {received:?}"
    );
    let after = drain(&emulator, &dead_letter_url).await;
    assert!(
        after.is_empty(),
        "case {case:?}: nothing may be dead-lettered once the delivery goes through, got \
         {after:?}"
    );
}
