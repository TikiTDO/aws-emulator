// SPDX-License-Identifier: CC0-1.0

//! An object write on a bucket that notifies an Amazon SNS topic really RUNS the function
//! subscribed to that topic — the three-hop S3 -> SNS -> Lambda chain, end to end over the
//! assembled router, with the negative control beside it.
//!
//! The rule under test: **an object change matching a bucket's `TopicConfiguration` is
//! published to that topic, and SNS then delivers the publication to every confirmed
//! `lambda`-protocol subscription of the topic by invoking the endpoint function
//! asynchronously as the service principal `sns.amazonaws.com` ("lambda – delivery of
//! JSON-encoded message to an Lambda function", `SubscribeInput$Protocol`) — so that
//! invocation is admitted only by the FUNCTION's own resource-based policy, the document
//! `GetPolicy` reports, evaluated for that principal, for the invoking action and for the
//! publishing topic as `aws:SourceArn`. A function whose policy states no such grant is never
//! invoked, while the object write and the publication it causes are accepted either way.**
//!
//! Four crates have to agree for that to hold (S3 evaluates the configuration and publishes
//! to the topic, SNS fans the publication out to its `lambda` subscription, Lambda holds the
//! grant and runs the handler, Amazon SQS holds what the handler forwarded), so the suite
//! lives here: a crate-local suite can only ever see one side of it. Its siblings each reach
//! one hop — [`crate::s3_object_events_fan_out_through_sns`] pins the S3 -> SNS hop onto a
//! subscribed QUEUE, and [`crate::sns_lambda_invoke_permission_revocation`] pins the
//! SNS -> Lambda hop for a publish a client makes itself — and what only this suite reaches is
//! the two hops CHAINED: the event that starts at an object write arriving in a handler, which
//! is what tells a missing grant apart from a pipeline that delivers nothing at all.
//!
//! The rows differ along what the rule generalises over — every part of the policy decision:
//! whether the function carries a document at all, which principal its statement names, which
//! action it names (the invoking action, a wildcard over the service's actions, another action
//! of the same service), and how it is scoped by `aws:SourceArn` (unscoped, to the publishing
//! topic, to another topic). The denied rows sit between granted ones, so a round that
//! observes nothing is read against rounds of the same chain that observe a delivery.
//!
//! Its CloudFormation, SNS, Amazon SQS and IAM callers, its execution role and its queue
//! reader are the SNS -> Lambda boundary suite's
//! ([`crate::sns_lambda_subscription_permission_boundary`]), so there is one harness rather
//! than a copy per reading; what this file owns is the assembled router (the boundary suite's
//! mounts no S3), S3's REST surface, and the Lambda control-plane calls that move a grant
//! across every part of the statement rather than one fixed one.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, drain, execution_role, sns, sqs, ACCOUNT, CREATE_COMPLETE, DELIVERY_BUDGET,
    LOOK_INTERVAL, ROLE_NAME,
};

/// The Region every request here is signed for: the one the shared harness and the shared
/// stack reader sign for, so the resources this suite reaches through them are the ones it
/// creates.
const REGION: &str = "us-east-1";

/// The names this suite's wiring is created under.
const BUCKET: &str = "s3-sns-lambda-chain-source";
const TOPIC: &str = "s3-sns-lambda-chain-topic";
const OTHER_TOPIC: &str = "s3-sns-lambda-chain-other-topic";
const STACK: &str = "s3-sns-lambda-chain";
const FUNCTION: &str = "s3-sns-lambda-chain-forwarder";
const SINK_QUEUE: &str = "s3-sns-lambda-chain-sink";

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `Id` the bucket's topic configuration carries.
const CONFIGURATION_ID: &str = "chain";

/// The statement id every grant below is added and removed under: a client reconciling a
/// grant addresses the statement by id, which is what `RemovePermission` takes.
const STATEMENT_ID: &str = "sns-invoke";

/// The service principals in play: the one Amazon S3 publishes a bucket's notifications
/// under, and the one SNS delivers to a subscribed endpoint under.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// A service principal that is NOT the one performing the delivery, which a row names to
/// check that the statement's `Principal` is read.
const OTHER_PRINCIPAL: &str = "events.amazonaws.com";

/// The action S3 performs on a topic destination, and the one a delivery to a subscribed
/// function performs on that function.
const PUBLISH_ACTION: &str = "sns:Publish";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A wildcard over every action of the function's own service, and an action of that service
/// that is NOT the invocation — the two other action forms a row names.
const WILDCARD_ACTION: &str = "lambda:*";
const OTHER_ACTION: &str = "lambda:GetFunction";

/// The error code Lambda answers a `GetPolicy` of a function holding no policy at all, which
/// is what the first round's function holds.
const POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// The `eventSource` and `eventName` every record S3 publishes about one of these writes
/// carries.
const EVENT_SOURCE: &str = "aws:s3";
const EVENT_NAME: &str = "ObjectCreated:Put";

/// The bytes every write below stores.
const OBJECT_BODY: &str = "the object the chain carries";

/// How long a round that must observe NOTHING keeps looking before concluding nothing came,
/// and how long the final reading keeps collecting: well past the span a granted round of the
/// same chain takes to leave its forward, and in the range the sibling suites' own quiet
/// windows sit in ([`crate::sns_lambda_subscription_permission_boundary`] waits 3 s,
/// [`crate::sns_lambda_invoke_permission_revocation`] 5 s).
const QUIET_BUDGET: Duration = Duration::from_secs(5);

/// The handler the stack deploys: for every SNS record it is handed it reads the document the
/// publication carried and forwards one body per S3 record in it to the sink queue, so a
/// client observes the whole chain where it observes any delivery.
///
/// The publication a destination is sent while a configuration is being VALIDATED carries no
/// `Records` at all (it is the *Event message structure*'s test message), so the records are
/// read with a default: a handler run for it forwards nothing rather than failing.
const FORWARDING_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName=os.environ["SINK_QUEUE"])["QueueUrl"]
    forwarded = 0
    for record in event["Records"]:
        published = json.loads(record["Sns"]["Message"])
        for inner in published.get("Records", []):
            client.send_message(
                QueueUrl=url,
                MessageBody=json.dumps(
                    {
                        "eventSource": inner["eventSource"],
                        "eventName": inner["eventName"],
                        "bucket": inner["s3"]["bucket"]["name"],
                        "key": inner["s3"]["object"]["key"],
                    }
                ),
            )
            forwarded += 1
    return {"forwarded": forwarded}
"#;

/// The whole emulator over one engine, exactly as the binary assembles it — so which service
/// claims each call, S3's publish to the topic and SNS's invocation of the function included,
/// is decided by the production mount rather than by a hand-picked set.
fn build_router() -> MultiServiceRouter {
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
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One response of S3's REST surface: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
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
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// What one Lambda call answered: its status, its body as JSON, and the wire error code of a
/// refusal (`restJson1` carries it in `__type` or in the `x-amzn-errortype` header).
struct Answered {
    status: u16,
    body: Value,
    error_code: String,
}

/// One Lambda `restJson1` call.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answered {
    let payload = match &body {
        Some(body) => Bytes::from(body.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let from_header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(from_header)
        .unwrap_or_default();
    Answered {
        status,
        body: parsed,
        error_code,
    }
}

/// The stack this suite deploys: the forwarding function only. The bucket, its notification
/// configuration, the topic, the subscription and every grant are made through the APIs a
/// client makes them through, afterwards.
fn template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Forwarder": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Timeout": 30,
                    "Environment": { "Variables": { "SINK_QUEUE": SINK_QUEUE } },
                    "Code": { "ZipFile": FORWARDING_SOURCE },
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Forwarder", "Arn"] } },
        },
    })
    .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active` — the
/// state an invocation needs — and answer the state it reported then. The span a created
/// function spends coming up is that service's own, so nothing here sleeps a fixed span on
/// its behalf.
async fn wait_until_active(router: &MultiServiceRouter) -> String {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        let state = reported.body["State"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == "Active" || Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// `PUT /<bucket>` — `CreateBucket`, in the Region every call here is signed for.
async fn create_bucket(router: &MultiServiceRouter) {
    let answer = s3(router, "PUT", &format!("/{BUCKET}"), "").await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({BUCKET:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`, which is what starts the chain.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str) {
    let answer = s3(router, "PUT", &format!("/{BUCKET}/{key}"), OBJECT_BODY).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutObject({BUCKET:?}, {key:?}) must succeed whether or not the \
         delivery it causes is admitted, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>?notification` with ONE `TopicConfiguration` naming `topic_arn` for the
/// object-created class.
///
/// The elements are the `xmlName`s the model puts on `NotificationConfiguration` and on the
/// topic configuration's ARN member (`smithy_query(query="shape",shape="TopicConfiguration")`:
/// `TopicArn` is bound as `Topic`).
async fn put_topic_notification(router: &MultiServiceRouter, topic_arn: &str) {
    let body = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\"><TopicConfiguration>\
         <Id>{CONFIGURATION_ID}</Id><Topic>{topic_arn}</Topic>\
         <Event>s3:ObjectCreated:*</Event>\
         </TopicConfiguration></NotificationConfiguration>"
    );
    let answer = s3(router, "PUT", &format!("/{BUCKET}?notification"), &body).await;
    assert_eq!(
        answer.status, 200,
        "setup: PutBucketNotificationConfiguration({BUCKET:?}) must record the topic \
         destination, got {} with {}",
        answer.status, answer.body
    );
}

/// The grant S3 needs on the topic destination, written as the topic's `Policy` attribute —
/// "the JSON serialization of the topic's access control policy" (`GetTopicAttributes`), the
/// document S3 reads before it records a configuration naming that topic. It is the hop this
/// suite does NOT vary.
async fn grant_s3_publish(router: &MultiServiceRouter, topic_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowS3Publish",
            "Effect": "Allow",
            "Principal": { "Service": S3_PRINCIPAL },
            "Action": PUBLISH_ACTION,
            "Resource": topic_arn,
        }],
    })
    .to_string();
    sns(
        router,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn),
            ("AttributeName", "Policy"),
            ("AttributeValue", &policy),
        ],
    )
    .await;
}

/// Which topic a statement's `aws:SourceArn` scopes the grant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// No condition at all, so the grant holds for every source.
    Unscoped,
    /// The topic the bucket publishes to.
    PublishingTopic,
    /// Another topic of the same account, which this chain never publishes to.
    AnotherTopic,
}

/// One statement a row puts on the function's policy through `AddPermission`.
#[derive(Debug, Clone, Copy)]
struct Grant {
    principal: &'static str,
    action: &'static str,
    scope: Scope,
}

/// One round: the grant the function's policy carries when the round writes its object, and
/// whether the function must therefore be invoked for that write.
struct Row {
    case: &'static str,
    /// `None` leaves the function carrying no policy at all.
    grant: Option<Grant>,
    invoked: bool,
}

/// The rounds, in order, over one bucket, one topic and one subscription: each round brings
/// the function's policy to what it names and then writes one object. They differ along every
/// part of the decision the rule names — presence, principal, action and source scope — and a
/// denied round always sits between granted ones.
const ROWS: &[Row] = &[
    Row {
        case: "the subscribed function carries no resource-based policy at all",
        grant: None,
        invoked: false,
    },
    Row {
        case: "the invoking action granted to this service for every source",
        grant: Some(Grant {
            principal: SNS_PRINCIPAL,
            action: INVOKE_ACTION,
            scope: Scope::Unscoped,
        }),
        invoked: true,
    },
    Row {
        case: "the same grant scoped by aws:SourceArn to another topic",
        grant: Some(Grant {
            principal: SNS_PRINCIPAL,
            action: INVOKE_ACTION,
            scope: Scope::AnotherTopic,
        }),
        invoked: false,
    },
    Row {
        case: "the same grant scoped by aws:SourceArn to the publishing topic",
        grant: Some(Grant {
            principal: SNS_PRINCIPAL,
            action: INVOKE_ACTION,
            scope: Scope::PublishingTopic,
        }),
        invoked: true,
    },
    Row {
        case: "the invoking action granted to another service principal",
        grant: Some(Grant {
            principal: OTHER_PRINCIPAL,
            action: INVOKE_ACTION,
            scope: Scope::PublishingTopic,
        }),
        invoked: false,
    },
    Row {
        case: "another action of the same service granted to this one",
        grant: Some(Grant {
            principal: SNS_PRINCIPAL,
            action: OTHER_ACTION,
            scope: Scope::PublishingTopic,
        }),
        invoked: false,
    },
    Row {
        case: "a wildcard over the service's actions granted to this one",
        grant: Some(Grant {
            principal: SNS_PRINCIPAL,
            action: WILDCARD_ACTION,
            scope: Scope::PublishingTopic,
        }),
        invoked: true,
    },
];

/// Add `grant` to the function's policy through `AddPermission`, the call a client grants a
/// fan-out's invocation with.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    grant: Grant,
    topic: &str,
    other_topic: &str,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": grant.action,
        "Principal": grant.principal,
    });
    match grant.scope {
        Scope::Unscoped => {}
        Scope::PublishingTopic => body["SourceArn"] = json!(topic),
        Scope::AnotherTopic => body["SourceArn"] = json!(other_topic),
    }
    let answered = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(body),
    )
    .await;
    assert_eq!(
        answered.status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission on {FUNCTION}, got {} with {}",
        answered.status, answered.body
    );
}

/// Take the statement away again through `RemovePermission`, so the next round's policy is the
/// one that round names and nothing it inherited.
async fn remove_permission(router: &MultiServiceRouter, case: &str) {
    let answered = lambda(
        router,
        "DELETE",
        &format!("/2015-03-31/functions/{FUNCTION}/policy/{STATEMENT_ID}"),
        None,
    )
    .await;
    assert_eq!(
        answered.status, 204,
        "case {case:?}: expected HTTP 204 from RemovePermission on {FUNCTION}, got {} with {}",
        answered.status, answered.body
    );
}

/// The statements `GetPolicy` reports for the function, each as its `(Sid, Action, Principal)`
/// — and `None` when it reports no policy at all. What the document says is what the next
/// write is decided by, so every round reads it back rather than assuming the calls above.
async fn policy_statements(
    router: &MultiServiceRouter,
    case: &str,
) -> Option<Vec<(String, String, String)>> {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        None,
    )
    .await;
    if answered.status == 404 {
        assert!(
            answered.error_code.contains(POLICY_ABSENT_CODE),
            "case {case:?}: expected GetPolicy of {FUNCTION} to refuse with \
             {POLICY_ABSENT_CODE}, got {:?} with {}",
            answered.error_code,
            answered.body
        );
        return None;
    }
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 or 404 from GetPolicy of {FUNCTION}, got {} with {}",
        answered.status, answered.body
    );
    let document: Value = answered.body["Policy"]
        .as_str()
        .and_then(|policy| serde_json::from_str(policy).ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetPolicy of {FUNCTION} must report a policy document, got {}",
                answered.body
            )
        });
    let text = |statement: &Value, member: &str| -> String {
        match &statement[member] {
            Value::String(held) => held.clone(),
            Value::Object(held) => held
                .values()
                .next()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    };
    Some(
        document["Statement"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|statement| {
                (
                    text(statement, "Sid"),
                    text(statement, "Action"),
                    text(statement, "Principal"),
                )
            })
            .collect(),
    )
}

/// The forwards the sink queue has handed out so far whose `key` is `key`, polled until one
/// arrives (`expected`) or until the round's budget runs out.
///
/// `seen` accumulates every body the queue ever handed out, across rounds, so a delivery that
/// arrives after the round that published it has already been read is still there for the
/// final reading to catch.
async fn forwards_of(
    router: &MultiServiceRouter,
    sink: &str,
    seen: &mut Vec<Value>,
    key: &str,
    expected: bool,
) -> Vec<Value> {
    let budget = if expected { DELIVERY_BUDGET } else { QUIET_BUDGET };
    let deadline = Instant::now() + budget;
    loop {
        seen.extend(
            drain(router, sink)
                .await
                .iter()
                .map(|body| serde_json::from_str(body).unwrap_or(Value::Null)),
        );
        if !carrying(seen, key).is_empty() || Instant::now() >= deadline {
            return carrying(seen, key);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The forwards among `seen` that report `key`.
fn carrying(seen: &[Value], key: &str) -> Vec<Value> {
    seen.iter()
        .filter(|forward| forward["key"].as_str() == Some(key))
        .cloned()
        .collect()
}

/// The key the round at `index` writes.
fn round_key(index: usize) -> String {
    format!("uploads/round-{index}.txt")
}

/// Every round, over one bucket notifying one topic with one subscribed function: an object
/// write reaches the handler exactly when that function's own policy admits the invocation,
/// and the record the handler was handed is the one the write produced.
#[tokio::test]
async fn an_object_write_reaches_the_topics_subscribed_function_only_when_it_is_permitted() {
    let router = build_router();

    // The execution role the template names, and the sink the handler forwards to. The sink
    // is created with a visibility window past the whole suite, so a body one round read
    // cannot come back visible and be counted again in a later one.
    execution_role(&router).await;
    let created_sink = sqs(
        &router,
        "CreateQueue",
        &json!({
            "QueueName": SINK_QUEUE,
            "Attributes": { "VisibilityTimeout": "300" },
        }),
    )
    .await;
    let sink = created_sink["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue must answer a QueueUrl, got {created_sink}"))
        .to_string();

    cloudformation(
        &router,
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template())],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "setup: the stack holding the subscribed function must deploy, got {status} with \
         {described}"
    );
    let function_arn = outputs_of(&described)
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack must publish FunctionArn, got {described}"));
    let state = wait_until_active(&router).await;
    assert_eq!(
        state, "Active",
        "setup: expected Lambda to report {FUNCTION} Active, got {state:?}"
    );

    // The two topics: the one the bucket notifies, and another one a row scopes its grant to.
    let created = sns(&router, "CreateTopic", &[("Name", TOPIC)]).await;
    let topic = element(&created, "TopicArn");
    assert!(
        !topic.is_empty(),
        "setup: CreateTopic must answer a TopicArn, got {created}"
    );
    let created = sns(&router, "CreateTopic", &[("Name", OTHER_TOPIC)]).await;
    let other_topic = element(&created, "TopicArn");
    assert!(
        !other_topic.is_empty(),
        "setup: CreateTopic must answer a TopicArn, got {created}"
    );

    let subscribed = sns(
        &router,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "lambda"),
            ("Endpoint", &function_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    let subscription = element(&subscribed, "SubscriptionArn");
    assert!(
        subscription.starts_with(&topic),
        "setup: subscribing {function_arn} must answer a confirmed subscription of the topic, \
         got {subscribed}"
    );

    // The bucket, the grant S3 needs on the topic, and the configuration that makes the write
    // publish — all installed while the function still carries no grant, so the publication
    // the configuring call itself makes cannot reach the handler either.
    create_bucket(&router).await;
    grant_s3_publish(&router, &topic).await;
    put_topic_notification(&router, &topic).await;

    let mut seen: Vec<Value> = Vec::new();
    let mut held = false;
    let mut granted_keys: Vec<String> = Vec::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        if held {
            remove_permission(&router, case).await;
            held = false;
        }
        if let Some(grant) = row.grant {
            add_permission(&router, case, grant, &topic, &other_topic).await;
            held = true;
        }

        let statements = policy_statements(&router, case).await;
        let wanted = row.grant.map(|grant| {
            vec![(
                STATEMENT_ID.to_string(),
                grant.action.to_string(),
                grant.principal.to_string(),
            )]
        });
        assert_eq!(
            statements, wanted,
            "case {case:?}: expected the function to hold {wanted:?} after the grant was \
             reconciled, got {statements:?}"
        );

        let key = round_key(index);
        put_object(&router, case, &key).await;

        let forwarded = forwards_of(&router, &sink, &mut seen, &key, row.invoked).await;
        if !row.invoked {
            assert!(
                forwarded.is_empty(),
                "case {case:?}: the function's policy does not admit this service's \
                 invocation, so the write of {key:?} must reach no handler, got {forwarded:?}"
            );
            continue;
        }
        granted_keys.push(key.clone());
        assert_eq!(
            forwarded.len(),
            1,
            "case {case:?}: expected exactly one forward for the write of {key:?}, got \
             {forwarded:?}"
        );
        let forward = &forwarded[0];
        assert_eq!(
            forward["eventSource"].as_str(),
            Some(EVENT_SOURCE),
            "case {case:?}: expected the handler to be handed the record S3 published, got \
             {forward}"
        );
        assert_eq!(
            forward["eventName"].as_str(),
            Some(EVENT_NAME),
            "case {case:?}: expected the eventName of a put, got {forward}"
        );
        assert_eq!(
            forward["bucket"].as_str(),
            Some(BUCKET),
            "case {case:?}: expected the record to name the written bucket, got {forward}"
        );
    }

    // One last reading, kept open past the span a delivery takes: nothing arrived for a round
    // whose grant was absent, mis-principalled, mis-actioned or scoped elsewhere, and nothing
    // arrived twice for a round whose grant admitted it.
    let deadline = Instant::now() + QUIET_BUDGET;
    while Instant::now() < deadline {
        seen.extend(
            drain(&router, &sink)
                .await
                .iter()
                .map(|body| serde_json::from_str(body).unwrap_or(Value::Null)),
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let mut overall: Vec<String> = seen
        .iter()
        .map(|forward| forward["key"].as_str().unwrap_or_default().to_string())
        .collect();
    overall.sort();
    granted_keys.sort();
    assert_eq!(
        overall, granted_keys,
        "expected exactly one forward per admitted round and none for any other, got \
         {overall:?} (every body seen: {seen:?})"
    );
}
