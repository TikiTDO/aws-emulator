// SPDX-License-Identifier: CC0-1.0

//! A bucket's notification configuration naming an Amazon SNS TOPIC, end to end over the
//! assembled router: what the configuring call does with that destination, and what an object
//! change on the configured bucket then delivers to the topic's own subscribers.
//!
//! The rule under test: **a notification destination is reached as the Amazon S3 service
//! principal, which holds no identity-based policy, so the destination's OWN resource policy
//! is the only document in force over it — `PutBucketNotificationConfiguration` resolves every
//! destination before it records anything and refuses the whole container (HTTP 400
//! `InvalidArgument`, one fixed sentence, with the numbered argument pair naming the
//! destination and the reason) when that document does not allow S3 to publish this bucket's
//! notifications to it, while a destination it does allow is sent the test notification, is
//! recorded, and is afterwards published every object change the configuration names.**
//!
//! Three crates have to agree for that to hold — S3 validates the destination and publishes to
//! it, SNS holds the topic's policy and fans a publication out to its subscriptions, SQS holds
//! the subscribed queue and its own policy — so the suite lives here: a crate-local suite can
//! only ever see one side of it. The S3 crate's `notification_destination_validation` and
//! `object_event_notifications` suites pin the queue and function destination kinds, and
//! `cloudformation_sns_topic_policy_admits_s3_notifications` pins the deployed form of a topic
//! policy; what only this suite reaches is the TOPIC destination's whole path — the grant S3
//! reads, the test publication, and an object event travelling through the topic onto a
//! subscribed queue, where a client observes it.
//!
//! The grant rows differ along what the rule generalises over: whether the topic carries a
//! document at all, which principal it names, which action and which resource, how the grant is
//! scoped (by the publishing bucket's ARN, by the owning account, by another bucket) and in
//! which order an explicit `Deny` about it stands. The delivery rows differ along the object
//! change made (a write, a delete, a tag set), how the configuration names it (the event value
//! itself, the type wildcard, another type's wildcard), the key's own form (plain, prefixed,
//! carrying a space) and the key-name filter the configuration carries.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// The Region every request here is signed for, and so the Region the bucket, the topic and
/// the queue belong to.
const REGION: &str = "us-west-2";

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The service principal Amazon S3 publishes notifications under, and the one Amazon SNS
/// delivers to a subscribed queue under — each the identity the destination's policy has to
/// grant for that delivery to be made.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The action S3 performs on a topic destination, and the one SNS performs on a subscribed
/// queue.
const PUBLISH_ACTION: &str = "sns:Publish";
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";

/// The condition keys a grant to a service principal is narrowed by: the resource whose
/// activity caused the call, and the account that owns it.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The code, status and message S3 answers a container whose destination it could not
/// validate with, and the per-destination detail it carries for a topic that exists and whose
/// policy grants Amazon S3 nothing.
const INVALID_ARGUMENT: &str = "InvalidArgument";
const INVALID_ARGUMENT_STATUS: u16 = 400;
const UNABLE_TO_VALIDATE: &str = "Unable to validate the following destination configurations";
const TOPIC_UNPERMITTED: &str = "Permissions on the destination topic do not allow S3 to \
                                 publish notifications from this bucket";

/// The `Event` of the notification a validated destination is sent while the configuration is
/// being resolved, and the `eventSource` of every record S3 publishes about an object.
const TEST_EVENT: &str = "s3:TestEvent";
const EVENT_SOURCE: &str = "aws:s3";

/// The `Id` every configuration below carries, which a delivered record reports as its
/// `configurationId`.
const CONFIGURATION_ID: &str = "fan-out";

/// The bytes every write below stores, so a record's `size` has one expected value.
const OBJECT_BODY: &str = "notification-payload";

/// How long a wait for an asynchronous delivery lasts before giving up, how long a row that
/// must see NOTHING keeps looking, and how often either looks.
const DELIVERY_BUDGET: Duration = Duration::from_secs(20);
const QUIET_BUDGET: Duration = Duration::from_millis(1_500);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as the binary assembles it — so which service
/// claims each call, S3's and SNS's own in-process calls included, is decided by the production
/// mount rather than by a hand-picked set.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200, as its
/// parsed body.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Amazon SNS awsQuery call (API version 2010-03-31) that must answer 200, as its XML body.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sns"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SNS {action}, got {status} with {text}"
    );
    text
}

/// One response of S3's REST surface: its status, the request-correlation header a client
/// traces the call by, and the body as text.
struct Answer {
    status: u16,
    request_id: String,
    body: String,
}

impl Answer {
    /// The text of the first `<tag>` element the body carries, or the empty string.
    fn element(&self, tag: &str) -> String {
        elements(&self.body, tag).into_iter().next().unwrap_or_default()
    }
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        request_id: response
            .headers()
            .get("x-amz-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// `PUT /<bucket>` — `CreateBucket`, in the Region every call here is signed for.
async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}"),
        &format!(
            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `key` as a REQUEST addresses it: the one character the rows carry that a URI cannot hold
/// literally, percent-encoded. What S3 stores is the decoded key.
fn request_key(key: &str) -> String {
    key.replace(' ', "%20")
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> Answer {
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}/{}", request_key(key)),
        OBJECT_BODY,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`, whose modelled success is 204.
async fn delete_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> Answer {
    let answer = s3(
        router,
        "DELETE",
        &format!("/{bucket}/{}", request_key(key)),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case {case:?}: DeleteObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// `PUT /<bucket>/<key>?tagging` — `PutObjectTagging`, which "sets the supplied tag-set to an
/// object".
async fn put_object_tagging(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
) -> Answer {
    let body = format!(
        "<Tagging xmlns=\"{S3_XMLNS}\"><TagSet><Tag><Key>reviewed</Key><Value>yes</Value>\
         </Tag></TagSet></Tagging>"
    );
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}/{}?tagging", request_key(key)),
        &body,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutObjectTagging({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// `PUT /<bucket>?notification` with ONE `TopicConfiguration` naming `topic_arn` for `events`,
/// filtered by `filter_rules`.
///
/// The elements are the `xmlName`s the model puts on `NotificationConfiguration` and on the
/// topic configuration's ARN member (`smithy_query(query="shape",shape="TopicConfiguration")`:
/// `TopicArn` is bound as `Topic`).
async fn put_topic_notification(
    router: &MultiServiceRouter,
    bucket: &str,
    topic_arn: &str,
    events: &[&str],
    filter_rules: &[(&str, &str)],
) -> Answer {
    let mut body = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\"><TopicConfiguration>\
         <Id>{CONFIGURATION_ID}</Id><Topic>{topic_arn}</Topic>"
    );
    for event in events {
        body.push_str(&format!("<Event>{event}</Event>"));
    }
    if !filter_rules.is_empty() {
        body.push_str("<Filter><S3Key>");
        for (name, value) in filter_rules {
            body.push_str(&format!(
                "<FilterRule><Name>{name}</Name><Value>{value}</Value></FilterRule>"
            ));
        }
        body.push_str("</S3Key></Filter>");
    }
    body.push_str("</TopicConfiguration></NotificationConfiguration>");
    s3(router, "PUT", &format!("/{bucket}?notification"), &body).await
}

/// `GET /<bucket>?notification` — `GetBucketNotificationConfiguration`, which reports what the
/// put recorded.
async fn get_notification(router: &MultiServiceRouter, case: &str, bucket: &str) -> Answer {
    let answer = s3(router, "GET", &format!("/{bucket}?notification"), "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetBucketNotificationConfiguration({bucket:?}) must succeed, got {} \
         with {}",
        answer.status, answer.body
    );
    answer
}

/// Everything waiting on the queue at `url` right now, each body parsed as the JSON it carries
/// and DELETED as it is read — so nothing one stage received comes back into view for the next.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut bodies = Vec::with_capacity(messages.len());
    for message in &messages {
        let handle = message["ReceiptHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ReceiptHandle, got {message}"));
        sqs(
            router,
            case,
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
        )
        .await;
        let body = message["Body"].as_str().unwrap_or_default();
        bodies.push(serde_json::from_str(body).unwrap_or(Value::Null));
    }
    bodies
}

/// The first message the queue receives, waiting up to [`DELIVERY_BUDGET`] for the delivery, or
/// `None` when none arrives in that time.
async fn delivered(router: &MultiServiceRouter, case: &str, url: &str) -> Option<Value> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let mut messages = drain(router, case, url).await;
        if !messages.is_empty() {
            return Some(messages.remove(0));
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Whatever the queue receives over [`QUIET_BUDGET`], which is how a row asserts that NOTHING
/// was delivered: a delivery is made on a task of its own as soon as the operation that owed it
/// was answered, so the window is waited out ONCE and then read, rather than polled — what a
/// delivery made inside it left is still on the queue when the single read comes.
async fn nothing_delivered(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    tokio::time::sleep(QUIET_BUDGET).await;
    drain(router, case, url).await
}

/// The message one SNS envelope on the queue carries, parsed as the document S3 published.
fn published_document(case: &str, envelope: &Value) -> Value {
    assert_eq!(
        envelope["Type"].as_str(),
        Some("Notification"),
        "case {case:?}: what arrived must be an Amazon SNS notification envelope, got {envelope}"
    );
    let message = envelope["Message"].as_str().unwrap_or_default();
    serde_json::from_str(message).unwrap_or_else(|error| {
        panic!("case {case:?}: the envelope's message must be the document S3 published ({error}): {message}")
    })
}

/// Everything a row's documents can name: the bucket that publishes, the topic it publishes to,
/// and the queue subscribed to that topic.
struct Wiring {
    bucket: String,
    topic_arn: String,
    queue_url: String,
}

/// The wiring every row shares: a queue whose own policy admits the Amazon SNS delivery, a
/// topic that queue is subscribed to, and a bucket in this Region — with the topic carrying
/// whatever policy it was created with and nothing more.
async fn wiring(router: &MultiServiceRouter, case: &str, name: &str) -> Wiring {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let reported = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = reported["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {reported}"))
        .to_string();
    // "You must ensure that the Amazon SQS queue policy allows the Amazon SNS topic to send
    // messages to the queue": the subscribed queue's own document, which is not what these rows
    // vary — they vary the TOPIC's.
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": {
                "Policy": document(json!([statement(
                    "Allow",
                    json!({ "Service": SNS_PRINCIPAL }),
                    json!(SEND_MESSAGE_ACTION),
                    &queue_arn,
                    None,
                )])),
            },
        }),
    )
    .await;

    let created = sns(router, case, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = element(&created, "TopicArn");
    sns(
        router,
        case,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;

    let bucket = name.to_string();
    create_bucket(router, case, &bucket).await;
    Wiring {
        bucket,
        topic_arn,
        queue_url,
    }
}

/// Write `policy` as the `Policy` attribute of the topic — "the JSON serialization of the
/// topic's access control policy" (`GetTopicAttributes`), which is the document S3 reads to
/// decide whether it may publish this bucket's notifications to it.
async fn set_topic_policy(router: &MultiServiceRouter, case: &str, topic_arn: &str, policy: &str) {
    sns(
        router,
        case,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", policy),
        ],
    )
    .await;
}

/// A one-or-many statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A statement about `action` on `resource` for `principal`, scoped by `condition` when the row
/// states one.
fn statement(
    effect: &str,
    principal: Value,
    action: Value,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Sid": "AllowS3Publish",
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The ARN of an S3 bucket: `arn:aws:s3:::<bucket>`, with no Region or account, because a
/// bucket name is globally unique. It is the value `aws:SourceArn` carries for a notification
/// this bucket published.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The grant S3 needs on a topic, scoped by `condition` when a row gives one.
fn publish_grant(wiring: &Wiring, condition: Option<Value>) -> String {
    document(json!([statement(
        "Allow",
        json!({ "Service": S3_PRINCIPAL }),
        json!(PUBLISH_ACTION),
        &wiring.topic_arn,
        condition,
    )]))
}

/// One row of the grant table: the document the topic carries, and whether the identical
/// configuration naming that topic is recorded.
struct GrantRow {
    case: &'static str,
    /// `None` leaves the topic with the policy it was created with.
    policy: Option<fn(&Wiring) -> String>,
    accepted: bool,
}

/// The grant table, over the class the rule generalises over.
fn grant_rows() -> Vec<GrantRow> {
    vec![
        GrantRow {
            case: "a topic left with the policy it was created with",
            policy: None,
            accepted: false,
        },
        GrantRow {
            case: "a grant to the Amazon S3 principal scoped to this bucket by aws:SourceArn",
            policy: Some(|wiring| {
                publish_grant(
                    wiring,
                    Some(json!({ "ArnLike": { SOURCE_ARN_KEY: bucket_arn(&wiring.bucket) } })),
                )
            }),
            accepted: true,
        },
        GrantRow {
            case: "an unscoped grant to the Amazon S3 principal",
            policy: Some(|wiring| publish_grant(wiring, None)),
            accepted: true,
        },
        GrantRow {
            case: "a grant scoped to this account by aws:SourceAccount",
            policy: Some(|wiring| {
                publish_grant(
                    wiring,
                    Some(json!({
                        "StringEquals": { SOURCE_ACCOUNT_KEY: engine::emulator_account_id() },
                    })),
                )
            }),
            accepted: true,
        },
        GrantRow {
            case: "a grant scoped by aws:SourceArn to another bucket",
            policy: Some(|wiring| {
                publish_grant(
                    wiring,
                    Some(json!({
                        "ArnLike": {
                            SOURCE_ARN_KEY: bucket_arn(&format!("{}-elsewhere", wiring.bucket)),
                        },
                    })),
                )
            }),
            accepted: false,
        },
        GrantRow {
            case: "a grant scoped by aws:SourceAccount to another account",
            policy: Some(|wiring| {
                publish_grant(
                    wiring,
                    Some(json!({ "StringEquals": { SOURCE_ACCOUNT_KEY: "210987654321" } })),
                )
            }),
            accepted: false,
        },
        GrantRow {
            case: "a grant of another action of the same service",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": S3_PRINCIPAL }),
                    json!("sns:Subscribe"),
                    &wiring.topic_arn,
                    None,
                )]))
            }),
            accepted: false,
        },
        GrantRow {
            case: "a grant of every action of the service to the Amazon S3 principal",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": S3_PRINCIPAL }),
                    json!("sns:*"),
                    &wiring.topic_arn,
                    None,
                )]))
            }),
            accepted: true,
        },
        GrantRow {
            case: "a grant of the publish action to another service principal",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": "events.amazonaws.com" }),
                    json!(PUBLISH_ACTION),
                    &wiring.topic_arn,
                    None,
                )]))
            }),
            accepted: false,
        },
        GrantRow {
            case: "a grant whose Resource names another topic",
            policy: Some(|wiring| {
                document(json!([statement(
                    "Allow",
                    json!({ "Service": S3_PRINCIPAL }),
                    json!(PUBLISH_ACTION),
                    &format!("{}-elsewhere", wiring.topic_arn),
                    None,
                )]))
            }),
            accepted: false,
        },
        GrantRow {
            case: "a grant with an explicit deny of the Amazon S3 principal written after it",
            policy: Some(|wiring| {
                document(json!([
                    statement(
                        "Allow",
                        json!({ "Service": S3_PRINCIPAL }),
                        json!(PUBLISH_ACTION),
                        &wiring.topic_arn,
                        None,
                    ),
                    statement(
                        "Deny",
                        json!({ "Service": S3_PRINCIPAL }),
                        json!(PUBLISH_ACTION),
                        &wiring.topic_arn,
                        None,
                    ),
                ]))
            }),
            accepted: false,
        },
    ]
}

/// The topic's own document decides the configuring call: a container naming a topic it does not
/// admit is refused whole and records nothing, and one it admits is sent the test notification —
/// which the topic fans out to its subscription — and is recorded.
#[tokio::test]
async fn a_topic_configuration_is_recorded_only_where_the_topic_policy_lets_s3_publish() {
    for (index, row) in grant_rows().iter().enumerate() {
        let case = row.case;
        let router = router();
        let wiring = wiring(&router, case, &format!("s3-topic-grant-{index}")).await;
        if let Some(policy) = row.policy {
            set_topic_policy(&router, case, &wiring.topic_arn, &policy(&wiring)).await;
        }

        let answer = put_topic_notification(
            &router,
            &wiring.bucket,
            &wiring.topic_arn,
            &["s3:ObjectCreated:*"],
            &[],
        )
        .await;

        if !row.accepted {
            assert_eq!(
                answer.status, INVALID_ARGUMENT_STATUS,
                "case {case:?}: the configuration must be refused, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.element("Code"),
                INVALID_ARGUMENT,
                "case {case:?}: expected the refusal to carry {INVALID_ARGUMENT}, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message"),
                UNABLE_TO_VALIDATE,
                "case {case:?}: expected the refusal's message to be {UNABLE_TO_VALIDATE:?}, \
                 got {}",
                answer.body
            );
            assert_eq!(
                (
                    answer.element("ArgumentName1"),
                    answer.element("ArgumentValue1")
                ),
                (wiring.topic_arn.clone(), TOPIC_UNPERMITTED.to_string()),
                "case {case:?}: expected the refusal to name the destination and its reason, \
                 got {}",
                answer.body
            );
            // Refused WHOLE: nothing was recorded, and nothing was published to the topic.
            let read = get_notification(&router, case, &wiring.bucket).await;
            assert_eq!(
                opt_element(&read.body, "Topic"),
                None,
                "case {case:?}: a refused put records nothing, so the bucket must report no \
                 topic configuration, got {}",
                read.body
            );
            let quiet = nothing_delivered(&router, case, &wiring.queue_url).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: a refused put publishes nothing, so the subscribed queue must \
                 stay empty, got {quiet:?}"
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {}",
            answer.status, answer.body
        );
        let read = get_notification(&router, case, &wiring.bucket).await;
        assert_eq!(
            read.element("Topic"),
            wiring.topic_arn,
            "case {case:?}: expected the stored configuration to name {}, got {}",
            wiring.topic_arn,
            read.body
        );
        // The test notification S3 "sends … to verify that the destination exists and that the
        // bucket owner has permission to publish to it": published to the TOPIC, so what proves
        // it is the subscribed queue receiving it.
        let envelope = delivered(&router, case, &wiring.queue_url)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a validated topic destination must be sent the test \
                     notification, and the topic must fan it out to its subscription; nothing \
                     reached the queue"
                )
            });
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(wiring.topic_arn.as_str()),
            "case {case:?}: the notification must have come through the configured topic, got \
             {envelope}"
        );
        let published = published_document(case, &envelope);
        assert_eq!(
            published["Event"].as_str(),
            Some(TEST_EVENT),
            "case {case:?}: the first notification a validated destination receives must be \
             the test event, got {published}"
        );
        assert_eq!(
            published["Bucket"].as_str(),
            Some(wiring.bucket.as_str()),
            "case {case:?}: the test notification must name the bucket it was configured on, \
             got {published}"
        );
        assert_eq!(
            published["RequestId"].as_str(),
            Some(answer.request_id.as_str()),
            "case {case:?}: the test notification must name the call that configured it, got \
             {published}"
        );
    }
}

/// Which call a delivery row makes on the object once the bucket is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// `PutObject` — `s3:ObjectCreated:Put`.
    Write,
    /// `DeleteObject` on a plain bucket — `s3:ObjectRemoved:Delete`.
    Delete,
    /// `PutObjectTagging` — `s3:ObjectTagging:Put`.
    Tag,
}

impl Change {
    /// Whether the key has to hold an object BEFORE the configuration is installed, so that
    /// seeding it does not itself notify.
    fn needs_a_seeded_object(self) -> bool {
        !matches!(self, Self::Write)
    }

    /// Whether the change leaves an object at the key, which is what decides whether the
    /// delivered record reports a size and an entity tag.
    fn leaves_an_object(self) -> bool {
        !matches!(self, Self::Delete)
    }
}

/// One row of the delivery table.
struct DeliveryRow {
    case: &'static str,
    /// The `Events` the topic configuration names.
    configured_events: &'static [&'static str],
    /// The `Filter`'s key-name rules, as `(Name, Value)` pairs.
    filter_rules: &'static [(&'static str, &'static str)],
    change: Change,
    key: &'static str,
    /// The key as a delivered record spells it — "URL encoded" (Amazon S3 User Guide, *Event
    /// message structure*), with a space as a plus.
    published_key: &'static str,
    /// The `eventName` the subscribed queue must receive, or `None` when this change is not one
    /// the configuration asked for.
    expected_event_name: Option<&'static str>,
}

const DELIVERY_ROWS: &[DeliveryRow] = &[
    DeliveryRow {
        case: "a write, named by its own event value",
        configured_events: &["s3:ObjectCreated:Put"],
        filter_rules: &[],
        change: Change::Write,
        key: "report.json",
        published_key: "report.json",
        expected_event_name: Some("ObjectCreated:Put"),
    },
    DeliveryRow {
        case: "a write under a prefixed key, named by the type wildcard",
        configured_events: &["s3:ObjectCreated:*"],
        filter_rules: &[],
        change: Change::Write,
        key: "uploads/report.json",
        published_key: "uploads/report.json",
        expected_event_name: Some("ObjectCreated:Put"),
    },
    DeliveryRow {
        case: "a write whose key carries a space",
        configured_events: &["s3:ObjectCreated:*"],
        filter_rules: &[],
        change: Change::Write,
        key: "red flower.jpg",
        published_key: "red+flower.jpg",
        expected_event_name: Some("ObjectCreated:Put"),
    },
    DeliveryRow {
        case: "a write a prefix-and-suffix filter admits",
        configured_events: &["s3:ObjectCreated:*"],
        filter_rules: &[("prefix", "uploads/"), ("suffix", ".json")],
        change: Change::Write,
        key: "uploads/report.json",
        published_key: "uploads/report.json",
        expected_event_name: Some("ObjectCreated:Put"),
    },
    DeliveryRow {
        case: "a write the filter's prefix rule excludes",
        configured_events: &["s3:ObjectCreated:*"],
        filter_rules: &[("prefix", "uploads/")],
        change: Change::Write,
        key: "archive/report.json",
        published_key: "archive/report.json",
        expected_event_name: None,
    },
    DeliveryRow {
        case: "a write the filter's suffix rule excludes",
        configured_events: &["s3:ObjectCreated:*"],
        filter_rules: &[("prefix", "uploads/"), ("suffix", ".json")],
        change: Change::Write,
        key: "uploads/report.csv",
        published_key: "uploads/report.csv",
        expected_event_name: None,
    },
    DeliveryRow {
        case: "a delete, named by the removal type wildcard",
        configured_events: &["s3:ObjectRemoved:*"],
        filter_rules: &[],
        change: Change::Delete,
        key: "gone.txt",
        published_key: "gone.txt",
        expected_event_name: Some("ObjectRemoved:Delete"),
    },
    DeliveryRow {
        case: "a tag set put, named by its own event value",
        configured_events: &["s3:ObjectTagging:Put"],
        filter_rules: &[],
        change: Change::Tag,
        key: "tagged.json",
        published_key: "tagged.json",
        expected_event_name: Some("ObjectTagging:Put"),
    },
    DeliveryRow {
        case: "a write a configuration naming another event type does not ask for",
        configured_events: &["s3:ObjectRemoved:*"],
        filter_rules: &[],
        change: Change::Write,
        key: "report.json",
        published_key: "report.json",
        expected_event_name: None,
    },
];

/// An object change the configuration names is published to the configured topic, which fans it
/// out to the topic's subscription — and a change it does not name reaches nothing.
#[tokio::test]
async fn an_object_change_reaches_the_queue_subscribed_to_the_configured_topic() {
    for (index, row) in DELIVERY_ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        let wiring = wiring(&router, case, &format!("s3-topic-event-{index}")).await;
        set_topic_policy(
            &router,
            case,
            &wiring.topic_arn,
            &publish_grant(
                &wiring,
                Some(json!({ "ArnLike": { SOURCE_ARN_KEY: bucket_arn(&wiring.bucket) } })),
            ),
        )
        .await;

        // Seeded BEFORE the configuration is installed, so the write that seeds the key a
        // delete or a tagging needs notifies nothing and only the row's own change does.
        if row.change.needs_a_seeded_object() {
            put_object(&router, case, &wiring.bucket, row.key).await;
        }
        let configured = put_topic_notification(
            &router,
            &wiring.bucket,
            &wiring.topic_arn,
            row.configured_events,
            row.filter_rules,
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: configuring the granted topic must succeed, got {} with {}",
            configured.status, configured.body
        );
        // The validation's own test notification, taken here so the row below reads only what
        // its own object change delivered.
        let envelope = delivered(&router, case, &wiring.queue_url)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: no test notification reached the queue"));
        assert_eq!(
            published_document(case, &envelope)["Event"].as_str(),
            Some(TEST_EVENT),
            "case {case:?}: the first notification on the queue must be the test event, got \
             {envelope}"
        );

        let answer = match row.change {
            Change::Write => put_object(&router, case, &wiring.bucket, row.key).await,
            Change::Delete => delete_object(&router, case, &wiring.bucket, row.key).await,
            Change::Tag => put_object_tagging(&router, case, &wiring.bucket, row.key).await,
        };

        let Some(event_name) = row.expected_event_name else {
            let quiet = nothing_delivered(&router, case, &wiring.queue_url).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: this configuration does not name the change, so the subscribed \
                 queue must stay empty, got {quiet:?}"
            );
            continue;
        };

        let envelope = delivered(&router, case, &wiring.queue_url)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the change must be published to the configured topic and \
                     fanned out to its subscription; nothing reached the queue"
                )
            });
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(wiring.topic_arn.as_str()),
            "case {case:?}: the notification must have come through the configured topic, got \
             {envelope}"
        );
        let published = published_document(case, &envelope);
        let record = &published["Records"][0];
        assert_eq!(
            record["eventName"].as_str(),
            Some(event_name),
            "case {case:?}: expected eventName {event_name:?}, got {published}"
        );
        assert_eq!(
            record["eventSource"].as_str(),
            Some(EVENT_SOURCE),
            "case {case:?}: expected eventSource {EVENT_SOURCE:?}, got {published}"
        );
        assert_eq!(
            record["awsRegion"].as_str(),
            Some(REGION),
            "case {case:?}: expected awsRegion {REGION:?}, got {published}"
        );
        assert_eq!(
            record["s3"]["configurationId"].as_str(),
            Some(CONFIGURATION_ID),
            "case {case:?}: expected configurationId {CONFIGURATION_ID:?}, got {published}"
        );
        assert_eq!(
            record["s3"]["bucket"]["name"].as_str(),
            Some(wiring.bucket.as_str()),
            "case {case:?}: expected the record's bucket {:?}, got {published}",
            wiring.bucket
        );
        assert_eq!(
            record["s3"]["bucket"]["arn"].as_str(),
            Some(bucket_arn(&wiring.bucket).as_str()),
            "case {case:?}: expected the record's bucket ARN, got {published}"
        );
        assert_eq!(
            record["s3"]["object"]["key"].as_str(),
            Some(row.published_key),
            "case {case:?}: expected the record's key {:?}, got {published}",
            row.published_key
        );
        // The record names the CALL it came of, by the id that call's own response carried.
        assert_eq!(
            record["responseElements"]["x-amz-request-id"].as_str(),
            Some(answer.request_id.as_str()),
            "case {case:?}: expected the call's own x-amz-request-id {:?}, got {published}",
            answer.request_id
        );
        if row.change.leaves_an_object() {
            assert_eq!(
                record["s3"]["object"]["size"].as_i64(),
                Some(OBJECT_BODY.len() as i64),
                "case {case:?}: expected the stored size, got {published}"
            );
            // "The object etag … does not include the surrounding quotes": the digest alone,
            // as the write that stored the object reported it.
            let reported = record["s3"]["object"]["eTag"].as_str().unwrap_or_default();
            assert!(
                !reported.is_empty() && !reported.contains('"'),
                "case {case:?}: expected an unquoted eTag, got {published}"
            );
        } else {
            assert!(
                record["s3"]["object"]["size"].is_null()
                    && record["s3"]["object"]["eTag"].is_null(),
                "case {case:?}: a removal's record must carry no size or eTag, got {published}"
            );
        }
        // The queue is reached only through the topic, so the one delivery is the only one.
        let extra = nothing_delivered(&router, case, &wiring.queue_url).await;
        assert!(
            extra.is_empty(),
            "case {case:?}: one change owes one notification, got {extra:?} after it"
        );
    }
}
