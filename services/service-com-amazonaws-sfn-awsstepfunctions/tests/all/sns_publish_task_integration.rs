// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `arn:aws:states:::sns:publish` optimized `Task`
//! integration, driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions, SNS and SQS mounted on ONE engine — the
//! shape `core/router/src/lib.rs` mounts them in.
//!
//! The rule under test: a `Task` state whose `Resource` names an optimized integration
//! performs that service's own API operation, with the document the state built as the
//! operation's REQUEST and the operation's response as the state's RESULT — whatever
//! protocol that service's model declares. SNS's is a FORM protocol, so the request is
//! the action and the API version beside one field per LEAF of the input: a member that
//! is a map goes on the wire as the numbered `entry` fields that protocol names a map's
//! pairs by. So the publish really happens — the message reaches the queue subscribed
//! to the topic, attributes included — and a publish SNS REFUSES fails the task under
//! SNS's own error name, which the state's `Catch` routes on.
//!
//! The rows differ along what the rule generalises over: the integration FAMILY naming
//! the service, how many attributes the map carries and of which data types, what the
//! attribute names and the message hold (including the form's own delimiters), and
//! whether the publish carries a subject at all.
//!
//! The router, the two wire protocols, the form encoding and the one-`Task` definition
//! are `tests/all/form_protocol_integration.rs`'s: that suite holds the same rule over
//! the other `awsQuery` service a `Task` state calls, and `tests/all.rs` is one crate
//! root, so this suite drives its harness rather than standing up a second copy.

use serde_json::{json, Value};

use crate::form_protocol_integration::{element, form_value, task, Chain};
use crate::state_machine_lifecycle::{string, REGION};

/// SNS's signing name and awsQuery API version
/// (`smithy_query(query="service_info",model_path="services/
/// service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
/// com-amazonaws-sns-amazonsimplenotificationservice.json")`: `sigv4Name` `sns`,
/// version `2010-03-31`).
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// One awsQuery form call to SNS, the way a client of that service makes it, that must
/// succeed: its XML body.
async fn sns(chain: &Chain, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = chain
        .query(SNS_SIGNING_NAME, SNS_API_VERSION, action, params)
        .await;
    assert_eq!(
        status, 200,
        "case {action}: expected 200 from SNS itself, got {status} with body {body:?}"
    );
    body
}

/// A queue an SNS topic may deliver to, with the queue policy AWS documents for an SQS
/// subscriber: its url and the ARN a subscription names it by.
async fn queue(chain: &Chain, name: &str, topic_arn: &str) -> (String, String) {
    let created = chain.sqs("CreateQueue", &json!({ "QueueName": name })).await;
    let url = string(&created, "QueueUrl");
    let attributes = chain
        .sqs(
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn for {name} in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string();
    chain
        .sqs(
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
    (url, arn)
}

/// A topic called `name`, and a queue subscribed to it: the topic's ARN and the url the
/// delivery is then observed on.
async fn topic_with_subscriber(chain: &Chain, name: &str) -> (String, String) {
    let topic = element(&sns(chain, "CreateTopic", &[("Name", name)]).await, "TopicArn");
    let (url, queue_arn) = queue(chain, &format!("{name}-queue"), &topic).await;
    let subscription = element(
        &sns(
            chain,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &queue_arn),
            ],
        )
        .await,
        "SubscriptionArn",
    );
    assert!(
        subscription.contains(':'),
        "an SQS subscription is confirmed at once, got {subscription:?}"
    );
    (topic, url)
}

/// Every message waiting on the queue at `url`, as the body text each carries.
async fn drain(chain: &Chain, url: &str) -> Vec<String> {
    let received = chain
        .sqs(
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A one-state definition whose `Task` names `resource`, sends `parameters` and
/// `Catch`es every error into a state that keeps what failed.
fn caught_task(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Publish",
        "States": {
            "Publish": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Next": "Denied",
                    "ResultPath": "$.errorInfo",
                }],
                "End": true,
            },
            "Denied": { "Type": "Pass", "End": true },
        },
    })
}

/// The output of a terminal execution, as the document it carries.
fn output(case: &str, described: &Value) -> Value {
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

/// One row of the publish table: the parameters the `Task` sends beside the topic, and
/// the attributes the subscribed queue must then see.
struct Published {
    case: &'static str,
    /// Which integration family the `Resource` names the service by.
    resource: &'static str,
    message: &'static str,
    subject: Option<&'static str>,
    /// `(name, data type, value)` per attribute the publish carries.
    attributes: &'static [(&'static str, &'static str, &'static str)],
}

/// A `Task` state whose `Resource` is `arn:aws:states:::sns:publish` publishes to the
/// topic its parameters name: the message — attributes and all — reaches the queue
/// subscribed to that topic, and the task's result is the publish's own answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_sns_publish_task_delivers_to_the_subscribed_queue() {
    let chain = Chain::new();
    let (topic, url) = topic_with_subscriber(&chain, "orders").await;

    let rows: &[Published] = &[
        Published {
            case: "no attributes at all",
            resource: "arn:aws:states:::sns:publish",
            message: "a plain notification",
            subject: None,
            attributes: &[],
        },
        Published {
            case: "one string attribute",
            resource: "arn:aws:states:::sns:publish",
            message: r#"{"orderId":"A-1"}"#,
            subject: Some("an order"),
            attributes: &[("kind", "String", "created")],
        },
        Published {
            // Two pairs: the map's entries are numbered, so a second one must not
            // overwrite the first.
            case: "two attributes of different data types",
            resource: "arn:aws:states:::sns:publish",
            message: "two attributes",
            subject: None,
            attributes: &[("kind", "String", "created"), ("count", "Number", "17")],
        },
        Published {
            // `String.Array` is one of the logical types `MessageAttributeValue$DataType`
            // documents (String, String.Array, Number, Binary), and the issue's own case
            // named none of them.
            case: "a string-array attribute",
            resource: "arn:aws:states:::sns:publish",
            message: "an array attribute",
            subject: None,
            attributes: &[("tags", "String.Array", r#"["a","b"]"#)],
        },
        Published {
            // An attribute name and a value carrying the form's own delimiters: each
            // is one field, not several.
            case: "names and values carrying the form's delimiters",
            resource: "arn:aws:states:::sns:publish",
            message: "a&b=c+d%e",
            subject: Some("a&subject"),
            attributes: &[("order.id", "String", "a&b=c+d%e")],
        },
        Published {
            // The AWS SDK family names the same operation of the same service, so it
            // makes the same publish.
            case: "the same publish through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:sns:publish",
            message: "through the SDK family",
            subject: None,
            attributes: &[("kind", "String", "sdk")],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let mut parameters = json!({ "TopicArn": topic, "Message": row.message });
        if let Some(subject) = row.subject {
            parameters["Subject"] = json!(subject);
        }
        if !row.attributes.is_empty() {
            let mut attributes = serde_json::Map::new();
            for (name, data_type, value) in row.attributes {
                attributes.insert(
                    (*name).to_string(),
                    json!({ "DataType": data_type, "StringValue": value }),
                );
            }
            parameters["MessageAttributes"] = Value::Object(attributes);
        }
        let described = chain
            .run(
                &format!("publish-{index}"),
                &task(row.resource, parameters),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );

        // The task's result is the publish's own answer: `Publish` answers a
        // `MessageId`, and the optimized family reports it inside the SDK response.
        let produced = output(case, &described);
        let message_id = produced
            .get("MessageId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: the result carries the MessageId SNS answered, got {produced}"
        );
        assert_eq!(
            produced.get("SdkHttpMetadata").is_some(),
            !row.resource.contains("aws-sdk"),
            "case {case:?}: the SDK response envelope belongs to the optimized family \
             alone, got {produced}"
        );

        // The far end: exactly one notification on the subscribed queue, carrying what
        // was published.
        let delivered = drain(&chain, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected exactly one delivery, got {delivered:?}"
        );
        let envelope: Value = serde_json::from_str(&delivered[0]).unwrap_or_else(|error| {
            panic!("case {case:?}: the notification is JSON ({error}): {:?}", delivered[0])
        });
        assert_eq!(
            envelope.get("Message").and_then(Value::as_str),
            Some(row.message),
            "case {case:?}: the queue receives the message that was published, got {envelope}"
        );
        assert_eq!(
            envelope.get("MessageId").and_then(Value::as_str),
            Some(message_id),
            "case {case:?}: the delivered notification is the publish the task reported, \
             got {envelope}"
        );
        assert_eq!(
            envelope.get("Subject").and_then(Value::as_str),
            row.subject,
            "case {case:?}: the subject is the one published, got {envelope}"
        );
        for (name, data_type, value) in row.attributes {
            let carried = &envelope["MessageAttributes"][*name];
            assert_eq!(
                carried.get("Type").and_then(Value::as_str),
                Some(*data_type),
                "case {case:?}: attribute {name:?} keeps its data type, got {envelope}"
            );
            assert_eq!(
                carried.get("Value").and_then(Value::as_str),
                Some(*value),
                "case {case:?}: attribute {name:?} keeps its value, got {envelope}"
            );
        }
        if row.attributes.is_empty() {
            assert!(
                envelope.get("MessageAttributes").is_none(),
                "case {case:?}: a publish with no attributes carries none, got {envelope}"
            );
        }
    }
}

/// A publish SNS REFUSES fails the task under SNS's OWN error name, with that
/// service's own sentence as the cause — so the state's `Catch` routes on it and
/// nothing reaches the subscribed queue.
///
/// The rows differ along which refusal the publish meets: a topic that does not exist,
/// and a parameter the operation rejects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_publish_fails_the_task_under_the_sns_error() {
    let chain = Chain::new();
    let (_topic, url) = topic_with_subscriber(&chain, "refusals").await;
    let absent = format!("arn:aws:sns:{REGION}:000000000000:no-such-topic");

    // (case, the parameters the Task sends, the error name a Catch sees, a fragment of
    //  SNS's own answer the cause must carry)
    let rows: &[(&str, Value, &str, &str)] = &[
        (
            "a topic that does not exist",
            json!({ "TopicArn": absent, "Message": "nobody is listening" }),
            "SNS.NotFoundException",
            "NotFound",
        ),
        (
            "a destination that is not a topic ARN",
            json!({ "TopicArn": "not-an-arn", "Message": "malformed" }),
            "SNS.InvalidParameterException",
            "InvalidParameter",
        ),
    ];

    for (index, (case, parameters, error, fragment)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("refused-{index}"),
                &caught_task("arn:aws:states:::sns:publish", parameters.clone()),
            )
            .await;
        // The Catch handled it, so the execution itself SUCCEEDS.
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: the Catch routes the failure, got {described:?}"
        );
        let caught = output(case, &described);
        assert_eq!(
            caught["errorInfo"].get("Error").and_then(Value::as_str),
            Some(*error),
            "case {case:?}: expected the error {error:?}, got {caught}"
        );
        let cause = caught["errorInfo"]
            .get("Cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            cause.contains(fragment),
            "case {case:?}: the cause carries SNS's own code {fragment:?}, got {cause:?}"
        );
        assert!(
            cause.contains("AmazonSNS"),
            "case {case:?}: the cause names the SNS client the refusal came back \
             through, got {cause:?}"
        );
        let delivered = drain(&chain, &url).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: a refused publish delivers nothing, got {delivered:?}"
        );
    }
}

/// The attribute name of a publish goes on the wire as the form field `Name` of its
/// numbered entry, so a topic whose subscriber asks for RAW delivery receives the
/// attribute under the name it was published with.
///
/// Raw delivery is the only form in which the queue's OWN message attributes are the
/// published ones, so it is what pins that the name and the type survived the form
/// round trip rather than the notification envelope's rendering of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_raw_subscriber_receives_the_published_attributes_as_its_own() {
    let chain = Chain::new();
    let topic = element(
        &sns(&chain, "CreateTopic", &[("Name", "raw")]).await,
        "TopicArn",
    );
    let (url, queue_arn) = queue(&chain, "raw-queue", &topic).await;
    let subscription = element(
        &sns(
            &chain,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &queue_arn),
            ],
        )
        .await,
        "SubscriptionArn",
    );
    sns(
        &chain,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "RawMessageDelivery"),
            ("AttributeValue", "true"),
        ],
    )
    .await;

    // (case, the attribute's name, its data type, its value)
    let rows: &[(&str, &str, &str, &str)] = &[
        ("a plain name", "kind", "String", "created"),
        ("a name carrying a period", "order.id", "String", "A-1"),
        ("a number", "count", "Number", "17"),
    ];
    for (index, (case, name, data_type, value)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("raw-{index}"),
                &task(
                    "arn:aws:states:::sns:publish",
                    json!({
                        "TopicArn": topic,
                        "Message": "a raw delivery",
                        "MessageAttributes": {
                            *name: { "DataType": data_type, "StringValue": value },
                        },
                    }),
                ),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let received = chain
            .sqs(
                "ReceiveMessage",
                &json!({
                    "QueueUrl": url,
                    "MaxNumberOfMessages": 10,
                    "MessageAttributeNames": ["All"],
                }),
            )
            .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            messages.len(),
            1,
            "case {case:?}: expected exactly one delivery, got {messages:?}"
        );
        let carried = &messages[0]["MessageAttributes"][*name];
        assert_eq!(
            carried.get("DataType").and_then(Value::as_str),
            Some(*data_type),
            "case {case:?}: the queue's own attribute keeps the published type, got \
             {:?}",
            messages[0]
        );
        assert_eq!(
            carried.get("StringValue").and_then(Value::as_str),
            Some(*value),
            "case {case:?}: the queue's own attribute keeps the published value, got \
             {:?}",
            messages[0]
        );
    }
}

/// The form encoding this suite sends its own SNS calls with is the one the task's call
/// uses, so a value carrying the form's delimiters is one field either way.
#[test]
fn a_form_value_escapes_every_character_outside_the_unreserved_set() {
    // (case, the value, the encoding expected of it)
    let rows: &[(&str, &str, &str)] = &[
        ("the unreserved set is left alone", "a-b_c.d~e9", "a-b_c.d~e9"),
        ("the form's own delimiters", "a&b=c+d%e", "a%26b%3Dc%2Bd%25e"),
        ("an ARN's punctuation", "arn:aws:sns:r:1:t", "arn%3Aaws%3Asns%3Ar%3A1%3At"),
    ];
    for (case, value, expected) in rows {
        let encoded = form_value(value);
        assert_eq!(
            encoded, *expected,
            "case {case:?}: expected {expected:?}, got {encoded:?}"
        );
    }
}
