// SPDX-License-Identifier: CC0-1.0

//! Cloud Control API over the topics Amazon SNS holds.
//!
//! The rule every case here holds the service to: **a resource of a type another service of
//! this emulator owns is resolved, enumerated, provisioned, patched and removed in THAT
//! service's own state, whichever API created it — "you can use this action to return
//! information about an existing resource in your account and Amazon Web Services Region,
//! whether those resources were provisioned using Cloud Control API" (`GetResource`) — and
//! it answers to the identifier the type's schema declares, which for a type whose schema
//! declares that identifier read-only is the one the OWNING service assigns.** For
//! `AWS::SNS::Topic` that identifier is the topic's ARN (`primaryIdentifier`
//! `/properties/TopicArn`, a `readOnlyProperty`), so a create through Cloud Control API
//! reports the ARN Amazon SNS assigned and the topic is one `sns:GetTopicAttributes`
//! answers for.
//!
//! The rows differ along what the rule generalises over: which API the topic came into
//! being through, which reference addresses it (the assigned ARN, its name), the kind of
//! topic it is and which properties it carries.
//!
//! `sqs_queue_read_through` and `native_resource_resolution` hold the same rule for the
//! `AWS::SQS::*` and `AWS::Logs::*` types, so what is asserted across the suites is where a
//! resource LIVES rather than anything about one owning service.

use serde_json::{json, Value};

use super::emulator::{
    descriptions, element, map_entry, text, Emulator, SIGNING_REGION, STATUS_FAILED,
};

/// The type whose resources live in the Amazon SNS service's own store.
const TOPIC: &str = "AWS::SNS::Topic";

/// The ARN of the topic called `name`, built by the core's own ARN builder so a case pins
/// the topic Cloud Control API resolved to the one Amazon SNS holds rather than to a string
/// this suite spells.
fn topic_arn(name: &str) -> String {
    engine::emulator_arn("sns", SIGNING_REGION, name)
}

/// The value Amazon SNS reports for the topic attribute `name` in a `GetTopicAttributes`
/// envelope, or `""` when it reports none: the attribute map is one entry per attribute,
/// read by the harness's one reader of an `awsQuery` answer.
fn attribute(body: &str, name: &str) -> String {
    map_entry(body, name)
}

/// The identifiers `ListResources` reports for the topics.
async fn inventory(emulator: &Emulator) -> Vec<String> {
    let listed = emulator.list(TOPIC, None, None).await;
    descriptions(&listed)
        .iter()
        .map(|description| text(description, "Identifier"))
        .collect()
}

/// One `GetTopicAttributes` call, required to be answered.
async fn attributes(emulator: &Emulator, case: &str, arn: &str) -> String {
    let (status, body) = emulator
        .sns("GetTopicAttributes", &[("TopicArn", arn)])
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: sns:GetTopicAttributes expected HTTP 200, got {status} with body \
         {body:?}"
    );
    body
}

/// One row of the create table: what the case is, the topic's name, the desired state the
/// create carries, and the attributes the owning service must then report for it.
type CreateRow<'a> = (&'a str, &'a str, Value, Vec<(&'a str, &'a str)>);

#[tokio::test]
async fn a_topic_created_through_cloud_control_is_the_topic_amazon_sns_holds() {
    let emulator = Emulator::with_sns();
    // The rows differ along the kind of topic and the properties the desired state carries:
    // a standard topic with a display name and a tag, and a FIFO one — whose name carries
    // the suffix that kind of topic's name must end with — with the deduplication property
    // only a FIFO topic may hold.
    let rows: [CreateRow; 2] = [
        (
            "a standard topic",
            "orders",
            json!({
                "TopicName": "orders",
                "DisplayName": "Orders",
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
            vec![("DisplayName", "Orders")],
        ),
        (
            "a FIFO topic",
            "orders.fifo",
            json!({
                "TopicName": "orders.fifo",
                "FifoTopic": true,
                "ContentBasedDeduplication": true,
            }),
            vec![("FifoTopic", "true"), ("ContentBasedDeduplication", "true")],
        ),
    ];
    for (case, name, desired, reported) in rows {
        let identifier = emulator.provision(TOPIC, &desired).await;
        assert_eq!(
            identifier,
            topic_arn(name),
            "case {case:?}: expected the ARN Amazon SNS assigns, got {identifier:?}"
        );

        // The service that OWNS the type holds the topic and answers for it in its own
        // terms, under the very ARN Cloud Control API reported.
        let body = attributes(&emulator, case, &identifier).await;
        assert_eq!(
            attribute(&body, "TopicArn"),
            identifier,
            "case {case:?}: the owning service must hold the topic under its own ARN, got \
             {body:?}"
        );
        for (name, value) in reported {
            assert_eq!(
                attribute(&body, name),
                value,
                "case {case:?} ({name}): expected {value:?}, got {:?}",
                attribute(&body, name)
            );
        }

        // And Cloud Control API reads back the model that service holds, whichever
        // reference the read addressed the topic by — the assigned ARN, or the topic's name.
        let mut expected = desired.clone();
        expected["TopicArn"] = json!(identifier);
        for (reference, addressed) in [("the assigned ARN", identifier.as_str()), ("the name", name)]
        {
            let model = emulator.properties(TOPIC, addressed).await;
            assert_eq!(
                model, expected,
                "case {case:?} ({reference}): expected {expected:?}, got {model:?}"
            );
        }
        assert!(
            inventory(&emulator).await.contains(&identifier),
            "case {case:?}: the topic must be in the type's inventory"
        );
    }

    // The tag the standard topic was created with is a tag the owning service holds.
    let (status, body) = emulator
        .sns(
            "ListTagsForResource",
            &[("ResourceArn", &topic_arn("orders"))],
        )
        .await;
    assert_eq!(
        status, 200,
        "sns:ListTagsForResource expected HTTP 200, got {status} with body {body:?}"
    );
    assert_eq!(
        (element(&body, "Key"), element(&body, "Value")),
        ("team".to_string(), "platform".to_string()),
        "the owning service must hold the tag the create carried, got {body:?}"
    );
}

#[tokio::test]
async fn a_topic_amazon_sns_holds_is_resolved_and_removed_through_cloud_control() {
    let emulator = Emulator::with_sns();
    // Created through the OWNING service's own API: nothing of this topic is ever named to
    // Cloud Control API before the reads below.
    let (status, created) = emulator.sns("CreateTopic", &[("Name", "native-topic")]).await;
    assert_eq!(
        status, 200,
        "sns:CreateTopic expected HTTP 200, got {status} with body {created:?}"
    );
    let arn = element(&created, "TopicArn");
    assert_eq!(
        arn,
        topic_arn("native-topic"),
        "sns:CreateTopic must report the topic's own ARN, got {created:?}"
    );

    let expected = json!({ "TopicArn": arn, "TopicName": "native-topic" });
    for (case, reference) in [("the ARN", arn.as_str()), ("the name", "native-topic")] {
        let model = emulator.properties(TOPIC, reference).await;
        assert_eq!(
            model, expected,
            "case {case:?}: expected {expected:?}, got {model:?}"
        );
    }
    assert_eq!(
        inventory(&emulator).await,
        vec![arn.clone()],
        "the type's inventory must be the one topic the owning service holds"
    );

    // A delete through Cloud Control API is a removal from that service's store: it reports
    // the topic as gone, in its own terms.
    emulator.delete_and_settle(TOPIC, &arn).await;
    let (status, body) = emulator
        .sns("GetTopicAttributes", &[("TopicArn", &arn)])
        .await;
    assert_eq!(
        status, 404,
        "sns:GetTopicAttributes after the delete: expected HTTP 404, got {status} with body \
         {body:?}"
    );
    assert_eq!(
        element(&body, "Code"),
        "NotFound",
        "sns:GetTopicAttributes after the delete: unexpected error code, got {body:?}"
    );
    assert_eq!(
        element(&body, "Message"),
        "Topic does not exist",
        "sns:GetTopicAttributes after the delete: unexpected wording, got {body:?}"
    );
    let (status, body) = emulator.get(TOPIC, &arn).await;
    assert_eq!(
        status, 404,
        "GetResource after the delete: expected HTTP 404, got {status} with body {body:?}"
    );
    assert!(
        inventory(&emulator).await.is_empty(),
        "the deleted topic must be listed nowhere"
    );
}

#[tokio::test]
async fn a_patch_through_cloud_control_is_a_write_to_the_topic_amazon_sns_holds() {
    let emulator = Emulator::with_sns();
    let identifier = emulator
        .provision(
            TOPIC,
            &json!({ "TopicName": "patched", "DisplayName": "Before" }),
        )
        .await;

    emulator
        .update_and_settle(
            TOPIC,
            &identifier,
            &json!([{ "op": "replace", "path": "/DisplayName", "value": "After" }]),
        )
        .await;
    let body = attributes(&emulator, "the applied patch", &identifier).await;
    assert_eq!(
        attribute(&body, "DisplayName"),
        "After",
        "the patch must be a write to the topic the owning service holds, got {body:?}"
    );

    // A patch asking for a property of the type no handler here applies is not applied at
    // all: the request reaches the owning handler's verdict, and the topic keeps the model
    // it had.
    let (status, accepted) = emulator
        .update(
            TOPIC,
            &identifier,
            &json!([{
                "op": "add",
                "path": "/Subscription",
                "value": [{ "Protocol": "sqs", "Endpoint": "arn:aws:sqs:us-east-1:000000000000:q" }],
            }]),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "the refused patch: expected the request to be accepted, got {status} with body \
         {accepted:?}"
    );
    let event = emulator
        .terminal(&text(&super::emulator::progress(&accepted), "RequestToken"))
        .await;
    assert_eq!(
        text(&event, "OperationStatus"),
        STATUS_FAILED,
        "the refused patch: expected the request to fail, got {event:?}"
    );
    assert_eq!(
        text(&event, "ErrorCode"),
        "InvalidRequest",
        "the refused patch: expected the owning handler's verdict, got {event:?}"
    );
    assert_eq!(
        text(&event, "StatusMessage"),
        format!("Subscription is a property of {TOPIC} this emulator's handler does not apply"),
        "the refused patch: unexpected wording, got {event:?}"
    );
    let body = attributes(&emulator, "after the refused patch", &identifier).await;
    assert_eq!(
        attribute(&body, "DisplayName"),
        "After",
        "the refused patch must leave the topic as it was, got {body:?}"
    );
}
