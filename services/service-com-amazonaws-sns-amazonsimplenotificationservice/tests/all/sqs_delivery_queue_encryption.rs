// SPDX-License-Identifier: CC0-1.0

//! A notification reaches a subscribed SQS queue that is encrypted at rest with a customer
//! managed key only where that KEY's own policy admits this service, end to end over the wire
//! through the real `MultiServiceRouter` with SQS and KMS mounted on the same engine — so a
//! withheld delivery is observed where a client observes it: the queue stays empty while the
//! publishing call still answers with a message id.
//!
//! The RULE asserted here: a queue carrying `KmsMasterKeyId` has every message written to it
//! encrypted under that key, and the queue's own service generates that data key for the
//! WRITER, which on this path is the service principal `sns.amazonaws.com` rather than the
//! queue's owner. So the key's own policy is a second document in force over the delivery,
//! beside the queue's `Policy`: a key whose policy admits that principal nothing — the policy
//! `CreateKey` writes for the account administrator alone, among others — takes no
//! notification, and one whose policy grants it the data-key operations takes every one. The
//! withhold is not a refusal of `Publish`: the call answers with a message id either way, and
//! a client that adds the grant sees the next notification arrive while the ones withheld
//! before it never do.
//!
//! The rows differ along what the rule generalises over: whether the queue is on a customer
//! managed key at all, which principal the key policy grants to, which of the KMS operations
//! it grants, which resource it grants them on, how the grant is scoped by condition (the
//! service the use arrives through), and whether a `Deny` of one of those operations stands
//! beside the grant.
//!
//! Every queue here carries the queue policy an SNS subscriber needs
//! ([`grant_to_sns`]) — which policies grant the SEND is `sqs_delivery_authorization.rs`, and
//! this suite holds that half constant so what each row varies is the KEY's policy.

use serde_json::{json, Value};

use crate::sqs_subscription_delivery::{
    drain, element, emulator, grant_to_sns, kms, queue_arn, set_queue_policy, sns, sqs, Emulator,
};

/// The Region every call in this suite is signed for, which is the Region the queues, the
/// keys and the topic are minted in.
const REGION: &str = "us-east-1";

/// The account of an ARN this emulator minted: the identity a key policy's administrative
/// statement names.
fn account_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account in {arn}"))
        .to_string()
}

/// The statement every key policy below carries: the one `CreateKey` writes by default, which
/// "gives the Amazon Web Services account that owns the KMS key permission to use it in all
/// cryptographic operations" — and grants no service principal anything.
fn administers(account: &str) -> Value {
    json!({
        "Sid": "AccountAdministers",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy document: the administrative statement, plus whatever `statements` the row
/// adds to it.
fn key_policy(account: &str, statements: Vec<Value>) -> String {
    let mut all = vec![administers(account)];
    all.extend(statements);
    json!({"Version": "2012-10-17", "Statement": all}).to_string()
}

/// The statement AWS documents for a queue subscribed to an SNS topic: the service principal
/// `sns.amazonaws.com`, allowed the data-key operations a delivery into the encrypted queue
/// performs, with `actions` as the row spells them and `resource` as the row scopes them.
fn grant_to_sns_on(actions: Value, resource: &str) -> Value {
    json!({
        "Sid": "SnsEncryptsIntoTheQueue",
        "Effect": "Allow",
        "Principal": {"Service": "sns.amazonaws.com"},
        "Action": actions,
        "Resource": resource,
    })
}

/// A key this emulator's KMS minted, with its policy replaced by `policy` when the row states
/// one: its ARN, which is what a queue's `KmsMasterKeyId` names it by.
async fn given_key(emulator: &Emulator, policy: Option<fn(&str, &str) -> String>) -> String {
    let created = kms(emulator, "CreateKey", &json!({})).await;
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey: no KeyMetadata.Arn in {created}"))
        .to_string();
    if let Some(policy) = policy {
        let account = account_of(&key_arn);
        kms(
            emulator,
            "PutKeyPolicy",
            &json!({
                "KeyId": key_arn,
                "PolicyName": "default",
                "Policy": policy(&account, &key_arn),
            }),
        )
        .await;
    }
    key_arn
}

/// A queue called `name` encrypted with the key `master_key_id` names (or with its own
/// service's key when the row names none), carrying the queue policy an SNS subscriber needs:
/// its url and the ARN a subscription names it by.
async fn given_encrypted_queue(
    emulator: &Emulator,
    name: &str,
    master_key_id: Option<&str>,
) -> (String, String) {
    let attributes = match master_key_id {
        Some(key) => json!({"KmsMasterKeyId": key}),
        None => json!({}),
    };
    let created = sqs(
        emulator,
        "CreateQueue",
        &json!({"QueueName": name, "Attributes": attributes}),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let arn = queue_arn(emulator, &url).await;
    set_queue_policy(emulator, &url, &grant_to_sns(&arn)).await;
    (url, arn)
}

/// Subscribe the queue `queue_arn` names to `topic` with raw delivery, so what the queue holds
/// is the published body itself.
async fn subscribe_raw(emulator: &Emulator, topic: &str, queue_arn: &str) {
    let subscription = element(
        &sns(
            emulator,
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue_arn),
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "true"),
            ],
        )
        .await,
        "SubscriptionArn",
    );
    assert!(
        subscription.contains(':'),
        "an SQS subscription is confirmed at once, got {subscription:?}"
    );
}

/// Publish `body` to `topic`, and report the `MessageId` the call answered with: a withheld
/// delivery is still an accepted publish, so every row reads one.
async fn publish(emulator: &Emulator, topic: &str, body: &str) -> String {
    let answered = sns(
        emulator,
        "Publish",
        &[("TopicArn", topic), ("Message", body)],
    )
    .await;
    element(&answered, "MessageId")
}

#[tokio::test]
async fn an_encrypted_queue_receives_a_notification_only_where_its_key_admits_this_service() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "encrypted-fan-out")]).await,
        "TopicArn",
    );

    // (case, the key policy built from (account, key ARN) — `None` for a queue on no customer
    //  managed key at all, how many messages the queue must hold after one publish)
    type PolicyOf = fn(&str, &str) -> String;
    let rows: &[(&str, Option<PolicyOf>, usize)] = &[
        (
            "no customer managed key at all, so no key policy is in force",
            None,
            1,
        ),
        (
            "the grant AWS documents for a subscribed queue",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![grant_to_sns_on(
                        json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                        "*",
                    )],
                )
            }),
            1,
        ),
        (
            "the administrator's statement alone, which names no service principal",
            Some(|account, _| key_policy(account, Vec::new())),
            0,
        ),
        (
            "the data-key grant without the decrypt the documented statement carries",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![grant_to_sns_on(json!("kms:GenerateDataKey*"), "*")],
                )
            }),
            0,
        ),
        (
            "the documented grant made to ANOTHER service's principal",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![json!({
                        "Sid": "AnotherServiceEncrypts",
                        "Effect": "Allow",
                        "Principal": {"Service": "events.amazonaws.com"},
                        "Action": ["kms:GenerateDataKey*", "kms:Decrypt"],
                        "Resource": "*",
                    })],
                )
            }),
            0,
        ),
        (
            "every KMS action granted to this service's principal",
            Some(|account, _| key_policy(account, vec![grant_to_sns_on(json!("kms:*"), "*")])),
            1,
        ),
        (
            "the documented grant beside an explicit deny of the decrypt",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![
                        grant_to_sns_on(json!(["kms:GenerateDataKey*", "kms:Decrypt"]), "*"),
                        json!({
                            "Sid": "ButNotTheDecrypt",
                            "Effect": "Deny",
                            "Principal": {"Service": "sns.amazonaws.com"},
                            "Action": "kms:Decrypt",
                            "Resource": "*",
                        }),
                    ],
                )
            }),
            0,
        ),
        (
            "the documented grant on the key's own ARN as the resource",
            Some(|account, key_arn| {
                key_policy(
                    account,
                    vec![grant_to_sns_on(
                        json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                        key_arn,
                    )],
                )
            }),
            1,
        ),
        (
            "the documented grant on ANOTHER key's ARN as the resource",
            Some(|account, _key_arn| {
                let elsewhere = format!(
                    "arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"
                );
                key_policy(
                    account,
                    vec![grant_to_sns_on(
                        json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                        &elsewhere,
                    )],
                )
            }),
            0,
        ),
        (
            "the documented grant scoped to the use arriving through the queue's service",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![json!({
                        "Sid": "SnsEncryptsIntoTheQueue",
                        "Effect": "Allow",
                        "Principal": {"Service": "sns.amazonaws.com"},
                        "Action": ["kms:GenerateDataKey*", "kms:Decrypt"],
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": {
                                "kms:ViaService": format!("sqs.{REGION}.amazonaws.com"),
                            },
                        },
                    })],
                )
            }),
            1,
        ),
        (
            "the documented grant scoped to a use arriving through another service",
            Some(|account, _| {
                key_policy(
                    account,
                    vec![json!({
                        "Sid": "SnsEncryptsElsewhere",
                        "Effect": "Allow",
                        "Principal": {"Service": "sns.amazonaws.com"},
                        "Action": ["kms:GenerateDataKey*", "kms:Decrypt"],
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": {
                                "kms:ViaService": format!("dynamodb.{REGION}.amazonaws.com"),
                            },
                        },
                    })],
                )
            }),
            0,
        ),
    ];

    for (index, (case, policy, expected)) in rows.iter().enumerate() {
        let key = match policy {
            Some(_) => Some(given_key(&emulator, *policy).await),
            None => None,
        };
        let (url, arn) = given_encrypted_queue(
            &emulator,
            &format!("encrypted-subscriber-{index}"),
            key.as_deref(),
        )
        .await;
        subscribe_raw(&emulator, &topic, &arn).await;

        let body = format!("marker-{index}");
        let message_id = publish(&emulator, &topic, &body).await;
        assert!(
            !message_id.is_empty(),
            "case {case:?}: a withheld delivery is still an accepted publish, so the call must \
             answer with a MessageId, got {message_id:?}"
        );

        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            *expected,
            "case {case:?}: expected {expected} delivered message(s), got {delivered:?}"
        );
        if let Some((carried, _)) = delivered.first() {
            assert_eq!(
                carried, &body,
                "case {case:?}: the delivered body must be the published one, got {carried:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_withheld_delivery_resumes_once_the_key_policy_grants_this_service() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "withheld-then-granted")]).await,
        "TopicArn",
    );
    // A key whose policy admits the account's administrator alone: it grants this service's
    // principal nothing until a statement names it.
    let key = given_key(
        &emulator,
        Some(|account, _| key_policy(account, Vec::new())),
    )
    .await;
    let (url, arn) = given_encrypted_queue(&emulator, "withheld-then-granted", Some(&key)).await;
    subscribe_raw(&emulator, &topic, &arn).await;

    // The control the queue and its key work at all: a client of the account sends into the
    // same queue and receives what it sent.
    sqs(
        &emulator,
        "SendMessage",
        &json!({"QueueUrl": url, "MessageBody": "the owner's own send"}),
    )
    .await;
    let received = drain(&emulator, &url).await;
    assert_eq!(
        received
            .iter()
            .map(|(body, _)| body.as_str())
            .collect::<Vec<_>>(),
        vec!["the owner's own send"],
        "the queue and its key must serve the account's own send, got {received:?}"
    );

    let withheld_id = publish(&emulator, &topic, "withheld").await;
    assert!(
        !withheld_id.is_empty(),
        "the publish must be accepted while the delivery is withheld, got {withheld_id:?}"
    );
    let drained = drain(&emulator, &url).await;
    assert!(
        drained.is_empty(),
        "a key policy naming no service principal must leave the queue empty, got {drained:?}"
    );

    // The grant AWS documents for the subscriber, added to the same key.
    kms(
        &emulator,
        "PutKeyPolicy",
        &json!({
            "KeyId": key,
            "PolicyName": "default",
            "Policy": key_policy(
                &account_of(&key),
                vec![grant_to_sns_on(json!(["kms:GenerateDataKey*", "kms:Decrypt"]), "*")],
            ),
        }),
    )
    .await;

    publish(&emulator, &topic, "delivered").await;
    let drained = drain(&emulator, &url).await;
    assert_eq!(
        drained
            .iter()
            .map(|(body, _)| body.as_str())
            .collect::<Vec<_>>(),
        vec!["delivered"],
        "once the key policy grants this service, the message published after it must arrive \
         and the one withheld before it must not, got {drained:?}"
    );
}
