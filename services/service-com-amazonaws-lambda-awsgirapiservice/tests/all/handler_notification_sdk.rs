// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS calls a RUNNING handler makes with the AWS SDK its runtime supplies, end to
//! end over the wire: Lambda, Amazon SNS, Amazon SQS, IAM and the token service on ONE engine,
//! a `python3.x` function whose code does `import boto3`, and everything it published,
//! subscribed and tagged read back where a client reads it — through Amazon SNS's own reads and
//! off the queue the subscription delivers to.
//!
//! The RULE pinned here: a managed runtime ships the AWS SDK for the handler's language on its
//! import path with a client for every service the runtime's call seam binds, WHICHEVER
//! protocol that service speaks, so a handler can call it with no dependency of its own and
//! what the call answers is THAT service's own answer — a success carrying the operation's
//! output members and leaving the state that service then reports, or that service's modelled
//! refusal raised as `botocore.exceptions.ClientError` carrying its error code. Amazon SNS's
//! model declares `awsQuery` rather than one of the JSON protocols, so a publication a handler
//! makes is the message the topic holds, and a subscription it creates is the subscription the
//! topic reports.
//!
//! The rows differ along what the rule generalises over: the SHAPE of the input each call
//! carries (a scalar, a map of text, a list of structures, a map of structures carrying BYTES)
//! and of the output it answers (a scalar the service mints, a map, a list of structures, and
//! the empty output of an operation that answers none), and whether the call is served,
//! refused by the service, or one the SDK this runtime supplies makes no request for. Each
//! served row asserts both halves — what the handler read back, which it returns as the
//! invocation's payload, and what Amazon SNS itself holds afterwards — so a call that answered
//! without doing the work, or did the work without answering, fails here.
//!
//! A last publication runs with the grant WITHDRAWN from the function's execution role, so the
//! call a handler makes is judged against the policies in force on that role and not served as
//! the account's own.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]), and it makes THREE
//! invocations in all — the served calls, the refused calls, and the withdrawn-grant
//! publication — because an invocation of its own costs this target a share of the run budget
//! every other invocation suite draws on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoding, the XML element reader and the IAM call every suite in this
// target writes a role with, owned by the suite that first needed them rather than copied.
use crate::event_source_mapping_role_access::{element, form_encode};
use crate::event_source_mappings::{await_function_active, call, router_on};
// The one .zip writer this target packages a running handler with.
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The execution role the function runs as, the function's own name, and the inline identity
/// policy the role carries the grant in.
const ROLE_NAME: &str = "lambda-notification-publisher";
const FUNCTION: &str = "notification-caller";
const INLINE_POLICY: &str = "inline";

/// The account every ARN below names, which is the account this emulator serves.
const ACCOUNT: &str = "000000000000";

/// The topic the SUITE creates for the handler to publish to and subscribe the queue to, and
/// the topic the HANDLER creates itself.
const SUITE_TOPIC: &str = "suite-orders";
const HANDLER_TOPIC: &str = "handler-orders";

/// The queue the subscription delivers to, which is where a publication is observed.
const QUEUE: &str = "notification-sink";

/// A topic ARN no topic answers to, which is what the refused publication names.
const ABSENT_TOPIC: &str = "arn:aws:sns:us-east-1:000000000000:no-such-topic";

/// The message and subject the handler publishes, and the attribute bytes it carries.
const MESSAGE: &str = r#"{"orderId":"A-1"}"#;
const SUBJECT: &str = "an order";
const ATTRIBUTE_BYTES: &[u8] = &[0, 1, 2, 250];

/// The tag the handler writes on the topic it created.
const TAG_KEY: &str = "team";
const TAG_VALUE: &str = "orders";

/// The display name the handler asks for when it creates its topic, which is a topic attribute
/// Amazon SNS reports back.
const DISPLAY_NAME: &str = "handler orders";

/// The error code Amazon SNS refuses a call naming a resource it does not hold with
/// (`NotFoundException`'s `awsQueryError` code), and the one it refuses an unauthorized call
/// with (`AuthorizationErrorException`'s).
const NOT_FOUND: &str = "NotFound";
const AUTHORIZATION_ERROR: &str = "AuthorizationError";

/// An operation of Amazon SNS the SDK this runtime supplies makes no request for, so a handler
/// naming it is told the call was not made, naming it — with the operation name the method
/// stands for.
const UNBOUND_OPERATION: &str = "create_platform_endpoint";
const UNBOUND_OPERATION_NAME: &str = "CreatePlatformEndpoint";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on Amazon SNS.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// How long a round trip through another service is waited for: the condition is looked at
/// again until it holds or this budget is spent, so nothing here is timed.
const CLOCK_BUDGET: Duration = Duration::from_secs(30);

/// How long between two looks.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How many messages the handler's publications put on the subscribed queue: the one the
/// `Publish` row sent and one per entry of the `PublishBatch` row.
const PUBLISHED_MESSAGES: usize = 3;

/// Lambda, Amazon SNS, Amazon SQS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the topic it publishes to is the topic this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. Nothing here is signed: the suite's own calls present the account's own published
/// credential.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call to Amazon SNS, as the status it answered and the XML it carried.
async fn sns_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The same, for a call that must be served.
async fn sns(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, text) = sns_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from SNS {action}, got {status} / {text}"
    );
    text
}

/// One `awsJson1_0` call to Amazon SQS that must be served.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One awsQuery call to IAM that must be served.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) {
    let (status, text) =
        crate::event_source_mapping_role_access::iam(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} / {text}"
    );
}

/// The handler every invocation runs: it makes the Amazon SNS calls the event names, in order,
/// and reports what each one answered — so the invocation's payload IS what the SDK gave the
/// guest.
///
/// A call the event marks as carrying BYTES is given a binary message attribute, because the
/// bytes a handler passes are what an SDK carries for a blob member and no JSON event can hold
/// them.
const HANDLER_SOURCE: &str = r#"
import base64

import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("sns")
    results = []
    for asked in event["calls"]:
        parameters = dict(asked["parameters"])
        carried = asked.get("binary_attribute")
        if carried:
            parameters["MessageAttributes"] = {
                "kind": {"DataType": "String", "StringValue": "created"},
                "raw": {
                    "DataType": "Binary",
                    "BinaryValue": base64.b64decode(carried),
                },
            }
        try:
            answered = getattr(client, asked["method"])(**parameters)
        except botocore.exceptions.ClientError as error:
            results.append(
                {
                    "ok": False,
                    "code": error.response["Error"]["Code"],
                    "message": error.response["Error"].get("Message"),
                }
            )
            continue
        except botocore.exceptions.SdkCallNotMade as error:
            results.append({"ok": False, "not_made": str(error)})
            continue
        results.append(
            {
                "ok": True,
                "status": answered["ResponseMetadata"]["HTTPStatusCode"],
                "answered": {
                    name: value
                    for name, value in answered.items()
                    if name != "ResponseMetadata"
                },
            }
        )
    return {"results": results}
"#;

/// One served row: the client method the handler calls, the input it carries, the output
/// members whose value is known, and the ones Amazon SNS itself mints.
struct Row {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    /// Whether the call carries the binary message attribute above.
    binary_attribute: bool,
    /// Each known output member: where it sits in the answered document, and its value.
    answered: Vec<(&'static [&'static str], Value)>,
    /// Each output member Amazon SNS mints, which must be there and carry something.
    minted: Vec<&'static [&'static str]>,
}

/// One refused row: the call, and what the handler must have been told.
struct Refused {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    /// The error code the service refuses the call with, or `None` for a call the SDK makes no
    /// request for at all.
    code: Option<&'static str>,
    /// Text the detail of a call that was not made has to name.
    names: Option<&'static str>,
}

/// The ARN of a topic of this account in this Region.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT}:{name}")
}

/// The ARN of the execution role, as IAM records it.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}")
}

/// The identity policy that grants the role every Amazon SNS action this suite's handler
/// calls, on every resource of the account.
fn grant_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "sns:*",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The trust policy a Lambda execution role carries.
fn trust_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The queue policy AWS documents for a queue subscribed to a topic: the service principal
/// `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue.
fn grant_to_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// The served rows, in the order one invocation makes them: each later row stands on what the
/// earlier ones did, which is what makes the publication reach the queue the subscribe row
/// attached.
fn rows(queue_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a create carrying a map of attributes and a list of tags",
            method: "create_topic",
            parameters: json!({
                "Name": HANDLER_TOPIC,
                "Attributes": { "DisplayName": DISPLAY_NAME },
                "Tags": [{ "Key": TAG_KEY, "Value": TAG_VALUE }],
            }),
            binary_attribute: false,
            answered: vec![(&["TopicArn"], json!(topic_arn(HANDLER_TOPIC)))],
            minted: Vec::new(),
        },
        Row {
            case: "a read answering a map: the attributes of the topic just created",
            method: "get_topic_attributes",
            parameters: json!({ "TopicArn": topic_arn(HANDLER_TOPIC) }),
            binary_attribute: false,
            answered: vec![
                (
                    &["Attributes", "TopicArn"],
                    json!(topic_arn(HANDLER_TOPIC)),
                ),
                (&["Attributes", "DisplayName"], json!(DISPLAY_NAME)),
            ],
            minted: Vec::new(),
        },
        Row {
            case: "a subscribe carrying a boolean and a map of attributes",
            method: "subscribe",
            parameters: json!({
                "TopicArn": topic_arn(SUITE_TOPIC),
                "Protocol": "sqs",
                "Endpoint": queue_arn,
                "Attributes": { "RawMessageDelivery": "false" },
                "ReturnSubscriptionArn": true,
            }),
            binary_attribute: false,
            answered: Vec::new(),
            minted: vec![&["SubscriptionArn"]],
        },
        Row {
            case: "a read answering a list of structures: the subscriptions of that topic",
            method: "list_subscriptions_by_topic",
            parameters: json!({ "TopicArn": topic_arn(SUITE_TOPIC) }),
            binary_attribute: false,
            answered: vec![
                (
                    &["Subscriptions", "0", "Endpoint"],
                    json!(queue_arn),
                ),
                (&["Subscriptions", "0", "Protocol"], json!("sqs")),
                (
                    &["Subscriptions", "0", "TopicArn"],
                    json!(topic_arn(SUITE_TOPIC)),
                ),
            ],
            minted: vec![&["Subscriptions", "0", "SubscriptionArn"]],
        },
        Row {
            case: "a publication carrying BYTES in a map of message attributes",
            method: "publish",
            parameters: json!({
                "TopicArn": topic_arn(SUITE_TOPIC),
                "Message": MESSAGE,
                "Subject": SUBJECT,
            }),
            binary_attribute: true,
            answered: Vec::new(),
            minted: vec![&["MessageId"]],
        },
        Row {
            case: "a batched publication carrying a list of structures",
            method: "publish_batch",
            parameters: json!({
                "TopicArn": topic_arn(SUITE_TOPIC),
                "PublishBatchRequestEntries": [
                    { "Id": "one", "Message": "batched one" },
                    { "Id": "two", "Message": "batched two" },
                ],
            }),
            binary_attribute: false,
            answered: vec![
                (&["Successful", "0", "Id"], json!("one")),
                (&["Successful", "1", "Id"], json!("two")),
                (&["Failed"], json!([])),
            ],
            minted: vec![
                &["Successful", "0", "MessageId"],
                &["Successful", "1", "MessageId"],
            ],
        },
        Row {
            case: "a write answering no output member of its own: tags on the topic",
            method: "tag_resource",
            parameters: json!({
                "ResourceArn": topic_arn(SUITE_TOPIC),
                "Tags": [{ "Key": TAG_KEY, "Value": TAG_VALUE }],
            }),
            binary_attribute: false,
            answered: Vec::new(),
            minted: Vec::new(),
        },
        Row {
            case: "a read answering the tags that write left",
            method: "list_tags_for_resource",
            parameters: json!({ "ResourceArn": topic_arn(SUITE_TOPIC) }),
            binary_attribute: false,
            answered: vec![(
                &["Tags"],
                json!([{ "Key": TAG_KEY, "Value": TAG_VALUE }]),
            )],
            minted: Vec::new(),
        },
    ]
}

/// The refused rows, which one further invocation makes.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a publication to a topic no topic answers to",
            method: "publish",
            parameters: json!({ "TopicArn": ABSENT_TOPIC, "Message": MESSAGE }),
            code: Some(NOT_FOUND),
            names: None,
        },
        Refused {
            case: "a subscribe naming a topic no topic answers to",
            method: "subscribe",
            parameters: json!({
                "TopicArn": ABSENT_TOPIC,
                "Protocol": "sqs",
                "Endpoint": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{QUEUE}"),
            }),
            code: Some(NOT_FOUND),
            names: None,
        },
        Refused {
            case: "an operation the SDK this runtime supplies binds no request for",
            method: UNBOUND_OPERATION,
            parameters: json!({}),
            code: None,
            names: Some(UNBOUND_OPERATION_NAME),
        },
        Refused {
            case: "a parameter the binding does not carry",
            method: "publish",
            parameters: json!({
                "TopicArn": topic_arn(SUITE_TOPIC),
                "Message": MESSAGE,
                "Nonsense": "x",
            }),
            code: None,
            names: Some("Nonsense"),
        },
    ]
}

/// The value at `place` of `document`, where a numeric step indexes an array.
fn at<'a>(document: &'a Value, place: &[&str]) -> &'a Value {
    let mut read = document;
    for step in place {
        read = match step.parse::<usize>() {
            Ok(index) => &read[index],
            Err(_not_an_index) => &read[*step],
        };
    }
    read
}

/// Invoke the function with `event` and answer the payload, asserting the invocation itself
/// answered 200 with no function error — the handler reports a refusal in its own payload, so a
/// function error here is the crash the runtime must not produce.
async fn invoked(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal, None,
        "case {case:?}: expected no FunctionError, got {signal:?} with {}",
        answered.text()
    );
    answered.json()
}

/// Every message the queue at `url` holds once `expected` of them have arrived, looked at again
/// until they have or the budget is spent — so what is waited for is the CONDITION and not a
/// span. Each message read is kept, because one read answers what is visible at that instant.
async fn delivered(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    expected: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + CLOCK_BUDGET;
    let mut held: Vec<Value> = Vec::new();
    loop {
        let received = sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "MessageAttributeNames": ["All"],
            }),
        )
        .await;
        held.extend(
            received["Messages"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        if held.len() >= expected {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {expected} message(s) on the subscribed queue, got {}",
            held.len()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a handler reaches Amazon SNS through the SDK its runtime supplies, so what its call
/// answers is that service's own answer and what it publishes, subscribes and tags is what that
/// service then reports.
#[tokio::test]
async fn a_running_handler_publishes_and_subscribes_through_its_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The role the function runs as, written with IAM's own operations, so the session the
    // runtime mints speaks for the record `iam:CreateRole` wrote.
    iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_document()),
        ],
    )
    .await;
    iam(
        &router,
        "AttachRolePolicy",
        &[("RoleName", ROLE_NAME), ("PolicyArn", BASIC_EXECUTION_POLICY)],
    )
    .await;
    iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &grant_document()),
        ],
    )
    .await;

    // The topic the handler publishes to, and the queue a subscription delivers to — each
    // created where a client creates one.
    let suite_topic = element(&sns(&router, "CreateTopic", &[("Name", SUITE_TOPIC)]).await, "TopicArn");
    assert_eq!(
        suite_topic,
        topic_arn(SUITE_TOPIC),
        "setup: expected the topic ARN of this account and Region, got {suite_topic}"
    );
    let created = sqs(&router, "CreateQueue", &json!({ "QueueName": QUEUE })).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        &router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueArn in {attributes}"))
        .to_string();
    sqs(
        &router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": { "Policy": grant_to_sns(&queue_arn) },
        }),
    )
    .await;

    // The function itself, running as that role.
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": role_arn(),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        created.text()
    );
    await_function_active(&router, "setup", FUNCTION).await;

    // ONE invocation makes every served call, in order.
    let rows = rows(&queue_arn);
    let asked: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "method": row.method,
                "parameters": row.parameters,
                "binary_attribute": if row.binary_attribute {
                    Value::String(
                        base64::engine::general_purpose::STANDARD.encode(ATTRIBUTE_BYTES),
                    )
                } else {
                    Value::Null
                },
            })
        })
        .collect();
    let payload = invoked(
        &router,
        "the served calls",
        &json!({ "calls": asked }),
    )
    .await;
    let results = payload["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        rows.len(),
        "case \"the served calls\": expected one result per call, got {payload}"
    );

    for (row, result) in rows.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(true),
            "case {case:?}: expected the handler's call to be served, got {result}"
        );
        assert_eq!(
            result["status"],
            json!(200),
            "case {case:?}: expected the SDK to report HTTP 200, got {result}"
        );
        for (place, expected) in &row.answered {
            let read = at(&result["answered"], place);
            assert_eq!(
                read, expected,
                "case {case:?}: expected the output member {place:?} to be {expected}, got \
                 {result}"
            );
        }
        for place in &row.minted {
            let read = at(&result["answered"], place);
            assert!(
                read.as_str().is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected the service to mint the output member {place:?}, got \
                 {result}"
            );
        }
    }

    // The other half of each served row: what Amazon SNS itself holds, read where a client
    // reads it.
    let handler_topic_attributes =
        sns(&router, "GetTopicAttributes", &[("TopicArn", &topic_arn(HANDLER_TOPIC))]).await;
    assert!(
        handler_topic_attributes.contains(DISPLAY_NAME),
        "case \"a create carrying a map of attributes and a list of tags\": expected the \
         service to report the display name the handler asked for, got \
         {handler_topic_attributes}"
    );
    let handler_topic_tags =
        sns(&router, "ListTagsForResource", &[("ResourceArn", &topic_arn(HANDLER_TOPIC))]).await;
    assert!(
        handler_topic_tags.contains(TAG_VALUE),
        "case \"a create carrying a map of attributes and a list of tags\": expected the \
         service to report the tag the create carried, got {handler_topic_tags}"
    );
    let listed = sns(
        &router,
        "ListSubscriptionsByTopic",
        &[("TopicArn", &suite_topic)],
    )
    .await;
    assert!(
        listed.contains(&queue_arn),
        "case \"a subscribe carrying a boolean and a map of attributes\": expected the service \
         to report the subscription the handler created, got {listed}"
    );
    let suite_topic_tags = sns(&router, "ListTagsForResource", &[("ResourceArn", &suite_topic)]).await;
    assert!(
        suite_topic_tags.contains(TAG_KEY) && suite_topic_tags.contains(TAG_VALUE),
        "case \"a write answering no output member of its own: tags on the topic\": expected \
         the service to report the tag the handler wrote, got {suite_topic_tags}"
    );

    // And the publications themselves, read off the queue the subscription delivers to: one
    // message for the publish row and one per entry of the batch.
    let drained = delivered(
        &router,
        "a publication carrying BYTES in a map of message attributes",
        &queue_url,
        PUBLISHED_MESSAGES,
    )
    .await;
    let bodies: Vec<String> = drained
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect();
    let envelopes: Vec<Value> = bodies
        .iter()
        .map(|body| serde_json::from_str::<Value>(body).unwrap_or(Value::Null))
        .collect();
    let published = envelopes
        .iter()
        .find(|envelope| envelope["Message"] == json!(MESSAGE))
        .unwrap_or_else(|| panic!("expected the published message on the queue, got {bodies:?}"));
    assert_eq!(
        published["Subject"],
        json!(SUBJECT),
        "case \"a publication carrying BYTES in a map of message attributes\": expected the \
         subject as published, got {published}"
    );
    assert_eq!(
        published["MessageAttributes"]["kind"],
        json!({ "Type": "String", "Value": "created" }),
        "case \"a publication carrying BYTES in a map of message attributes\": expected the \
         text attribute as published, got {published}"
    );
    assert_eq!(
        published["MessageAttributes"]["raw"],
        json!({
            "Type": "Binary",
            "Value": base64::engine::general_purpose::STANDARD.encode(ATTRIBUTE_BYTES),
        }),
        "case \"a publication carrying BYTES in a map of message attributes\": expected the \
         bytes the handler passed to travel as the blob they are, got {published}"
    );
    for entry in ["batched one", "batched two"] {
        assert!(
            envelopes
                .iter()
                .any(|envelope| envelope["Message"] == json!(entry)),
            "case \"a batched publication carrying a list of structures\": expected the entry \
             {entry:?} on the queue, got {bodies:?}"
        );
    }

    // A second invocation makes the calls that are refused.
    let refused = refused_rows();
    let asked: Vec<Value> = refused
        .iter()
        .map(|row| {
            json!({
                "method": row.method,
                "parameters": row.parameters,
                "binary_attribute": Value::Null,
            })
        })
        .collect();
    let payload = invoked(
        &router,
        "the refused calls",
        &json!({ "calls": asked }),
    )
    .await;
    let results = payload["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        refused.len(),
        "case \"the refused calls\": expected one result per call, got {payload}"
    );
    for (row, result) in refused.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(false),
            "case {case:?}: expected the call to be refused, got {result}"
        );
        if let Some(code) = row.code {
            assert_eq!(
                result["code"],
                json!(code),
                "case {case:?}: expected the error code {code:?}, got {result}"
            );
            assert!(
                result["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected the service's own sentence beside the code, got \
                 {result}"
            );
        }
        if let Some(named) = row.names {
            let detail = result["not_made"].as_str().unwrap_or_default();
            assert!(
                detail.contains(named),
                "case {case:?}: expected the call-not-made detail to name {named:?}, got \
                 {result}"
            );
        }
    }

    // A third invocation publishes with the grant WITHDRAWN from the role: the call a handler
    // makes is the role's, so the service refuses it.
    iam(
        &router,
        "DeleteRolePolicy",
        &[("RoleName", ROLE_NAME), ("PolicyName", INLINE_POLICY)],
    )
    .await;
    let case = "a publication the execution role is no longer granted";
    let payload = invoked(
        &router,
        case,
        &json!({
            "calls": [{
                "method": "publish",
                "parameters": { "TopicArn": suite_topic, "Message": MESSAGE },
                "binary_attribute": Value::Null,
            }],
        }),
    )
    .await;
    let denied = &payload["results"][0];
    assert_eq!(
        denied["ok"],
        json!(false),
        "case {case:?}: expected the publication to be refused, got {payload}"
    );
    assert_eq!(
        denied["code"],
        json!(AUTHORIZATION_ERROR),
        "case {case:?}: expected the service's own authorization refusal, got {payload}"
    );
}
