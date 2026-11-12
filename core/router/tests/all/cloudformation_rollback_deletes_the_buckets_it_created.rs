// SPDX-License-Identifier: CC0-1.0

//! What a ROLLED-BACK stack leaves behind in the service that owns its resources: nothing.
//!
//! The rule under test: **when a stack's create fails, CloudFormation rolls it back —
//! "ROLLBACK — Roll back the stack creation. This is the default"
//! (`CreateStackInput$OnFailure`) — and the rollback takes away every resource that create
//! had MADE in the owning service, including the one whose own create failed part-way: a
//! provider that created a resource and could not finish configuring it does not leave it
//! standing. So once the stack reports `ROLLBACK_COMPLETE`, Amazon S3 answers `HeadBucket`
//! on each bucket the failed create declared with HTTP 404.**
//!
//! The failure the rows use is the one Amazon S3 states for a notification destination it
//! cannot validate: "after Amazon S3 receives this request, it first verifies that any
//! Amazon Simple Notification Service (Amazon SNS) or Amazon Simple Queue Service (Amazon
//! SQS) destination exists, and that the bucket owner has permission to publish to it"
//! (`PutBucketNotificationConfiguration`'s own documentation), and a configuration naming
//! one that fails either is refused with `InvalidArgument`. That refusal arrives AFTER
//! `CreateBucket` has already made the bucket, which is exactly the half-made resource the
//! rule is about — and it takes three crates to observe (CloudFormation deploys, Amazon S3
//! refuses and then answers the read, Amazon SQS owns the destination that decides it), so
//! the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: WHY the destination fails
//! validation (the queue does not exist, the queue exists and its policy grants Amazon S3
//! nothing, the topic does not exist), which destination kind the configuration names, and —
//! as the control that keeps the 404s from being vacuous — a destination that validates, for
//! which the stack completes and both buckets are there to read.
//!
//! Nothing here waits on the wall clock: a stack's terminal status is reached by looking
//! again until it holds, and the bucket reads are made once it does.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The Region every request here is signed for, and so the Region the stacks and their
/// buckets belong to.
const REGION: &str = "us-east-1";

/// How long this suite keeps LOOKING for a stack's terminal status, and how often. Generous:
/// the deadline is a safety net, never a measurement — what is asserted is the status, and
/// the loop stops the instant the stack reaches one.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The statuses a `CreateStack` settles in, whichever way it went: there is nothing more to
/// wait for once a stack reports one of them.
const TERMINAL_STATUSES: &[&str] = &[
    "CREATE_COMPLETE",
    "CREATE_FAILED",
    "ROLLBACK_COMPLETE",
    "ROLLBACK_FAILED",
    "DELETE_COMPLETE",
    "DELETE_FAILED",
];

/// The service principal Amazon S3 publishes a bucket's notifications under, and the action
/// a queue's policy has to allow it for a destination to validate.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The event every notification configuration below subscribes to. Which event it is does
/// not enter the rule: the destination is validated when the configuration is INSTALLED,
/// before any object changes.
const SUBSCRIBED_EVENT: &str = "s3:ObjectCreated:*";

/// The whole emulator over one engine, exactly as the binary assembles it, so which service
/// claims each call — CloudFormation's and Amazon S3's own in-process calls included — is
/// decided by the production mount.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One CloudFormation call over its awsQuery wire, as its status and body.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {text}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// `HEAD /<bucket>` — Amazon S3's `HeadBucket`, as a CLIENT's request: the read a caller
/// holding only the bucket name makes. Answers the status alone, which is all that read
/// reports.
async fn head_bucket(router: &MultiServiceRouter, bucket: &str) -> u16 {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("HEAD")
        .uri(format!("https://{host}/{bucket}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .body(Bytes::new())
        .expect("the test request is well formed");
    router.dispatch(request).await.status().as_u16()
}

/// A queue called `name`: its url and the ARN it reports.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let reported = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = reported["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {reported}"))
        .to_string();
    (url, arn)
}

/// The account field of an ARN this emulator minted.
fn account_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account field in {arn}"))
        .to_string()
}

/// Grant the Amazon S3 service principal `sqs:SendMessage` on the queue at `url`, which is
/// what lets a bucket's notification destination validate.
async fn grant_s3(router: &MultiServiceRouter, case: &str, url: &str, queue_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": S3_PRINCIPAL },
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
}

/// Which destination the row's bucket notification names, which is what decides whether the
/// create goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// A queue ARN of this account that names no queue Amazon SQS holds.
    AbsentQueue,
    /// A queue that exists and carries no policy, so it grants Amazon S3 nothing.
    UngrantedQueue,
    /// A queue that exists and whose policy allows the Amazon S3 principal to publish.
    GrantedQueue,
    /// A topic ARN of this account that names no topic Amazon SNS holds.
    AbsentTopic,
}

/// One row: what the notification destination is, and the status the stack must settle in.
struct Row {
    case: &'static str,
    destination: Destination,
    /// The status the stack reports once it has settled.
    settles_in: &'static str,
    /// Whether the buckets the stack declared are still there afterwards.
    buckets_remain: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a notification naming a queue Amazon SQS does not hold",
        destination: Destination::AbsentQueue,
        settles_in: "ROLLBACK_COMPLETE",
        buckets_remain: false,
    },
    Row {
        case: "a notification naming a queue whose policy grants Amazon S3 nothing",
        destination: Destination::UngrantedQueue,
        settles_in: "ROLLBACK_COMPLETE",
        buckets_remain: false,
    },
    Row {
        case: "a notification naming a topic Amazon SNS does not hold",
        destination: Destination::AbsentTopic,
        settles_in: "ROLLBACK_COMPLETE",
        buckets_remain: false,
    },
    Row {
        case: "a notification naming a queue that grants Amazon S3 the publish",
        destination: Destination::GrantedQueue,
        settles_in: "CREATE_COMPLETE",
        buckets_remain: true,
    },
];

/// The `NotificationConfiguration` property the row's second bucket declares, naming the
/// destination its row wired.
///
/// The member names are `AWS::S3::Bucket`'s own: a queue destination is a
/// `QueueConfigurations` entry carrying `Queue` and `Event`, and a topic destination a
/// `TopicConfigurations` entry carrying `Topic` and `Event`.
fn notification_configuration(destination: Destination, arn: &str) -> Value {
    match destination {
        Destination::AbsentTopic => json!({
            "TopicConfigurations": [{ "Topic": arn, "Event": SUBSCRIBED_EVENT }],
        }),
        _ => json!({
            "QueueConfigurations": [{ "Queue": arn, "Event": SUBSCRIBED_EVENT }],
        }),
    }
}

/// The template a row deploys: one plain bucket, and one whose `NotificationConfiguration`
/// names the row's destination — so the rule is asserted both for the resource that failed
/// and for a sibling the create had already finished.
fn template(destination: Destination, plain: &str, notified: &str, arn: &str) -> String {
    json!({
        "Resources": {
            "BucketA": {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": plain },
            },
            "BucketB": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": notified,
                    "NotificationConfiguration": notification_configuration(destination, arn),
                },
            },
        }
    })
    .to_string()
}

/// Look at the stack until it reports a terminal status, and answer that status together
/// with the body it was read from.
///
/// The CONDITION is what is waited for, not a span: each look is a `DescribeStacks`, which
/// is also what materialises the transitions the stack's own clock owes.
async fn settled(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (status, described) = cloudformation(router, "DescribeStacks", &[("StackName", stack)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if TERMINAL_STATUSES.contains(&reported.as_str()) {
            return (reported, described);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack never reached a terminal status; it reports \
             {reported:?} in {described}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_rolled_back_stack_leaves_none_of_the_buckets_it_created_behind() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // One emulator per row, so a bucket or queue name one row spells cannot decide
        // another row's deployment.
        let router = router();

        // The destination the row's notification names, wired as the row asks.
        let (url, queue_arn) = given_queue(&router, case, &format!("rollback-queue-{index}")).await;
        let account = account_of(&queue_arn);
        let destination_arn = match row.destination {
            Destination::AbsentQueue => {
                format!("arn:aws:sqs:{REGION}:{account}:rollback-no-such-queue-{index}")
            }
            Destination::AbsentTopic => {
                format!("arn:aws:sns:{REGION}:{account}:rollback-no-such-topic-{index}")
            }
            Destination::UngrantedQueue => queue_arn.clone(),
            Destination::GrantedQueue => {
                grant_s3(&router, case, &url, &queue_arn).await;
                queue_arn.clone()
            }
        };

        let stack = format!("rollback-leaves-nothing-{index}");
        let plain = format!("rollback-plain-{index}");
        let notified = format!("rollback-notified-{index}");
        let body = template(row.destination, &plain, &notified, &destination_arn);
        let (status, created) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", &stack), ("TemplateBody", &body)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );

        let (reported, described) = settled(&router, case, &stack).await;
        assert_eq!(
            reported, row.settles_in,
            "case {case:?}: expected the stack to settle in {:?}, got {reported:?} in \
             {described}",
            row.settles_in
        );

        // The rule, read where a client reads it: on the bucket.
        for bucket in [&plain, &notified] {
            let answered = head_bucket(&router, bucket).await;
            let expected = if row.buckets_remain { 200 } else { 404 };
            assert_eq!(
                answered, expected,
                "case {case:?}: after {reported}, HeadBucket on {bucket:?} must answer \
                 {expected}, got {answered}"
            );
        }
    }
}
