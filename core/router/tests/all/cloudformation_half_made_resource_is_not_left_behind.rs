// SPDX-License-Identifier: CC0-1.0

//! What a resource whose create was refused PART-WAY leaves behind in the service that owns
//! it: nothing.
//!
//! The rule under test: **a resource provider that physically created a resource and was then
//! refused one of the follow-up calls the declaration takes to install fails that resource
//! WITHOUT leaving it standing — the provider takes away what it had already made. So
//! whichever follow-up call was refused, and however the create's own failure setting
//! disposes of the STACK ("ROLLBACK — Roll back the stack creation. This is the default",
//! "DO_NOTHING … is equivalent to specifying true for the DisableRollback parameter", and
//! `DELETE`, the three documented `CreateStackInput$OnFailure` behaviours), the owning
//! service reports the half-made resource as ABSENT once the create has settled, while a
//! sibling the create had finished is there exactly as long as the stack keeps it.**
//!
//! For `AWS::S3::Bucket` that half-made resource is a bucket: `CreateBucket` is only the
//! first of the calls such a declaration takes, and each sub-resource configuration is a
//! call of its own on the bucket that by then exists, so a refusal of one of those leaves a
//! bucket nothing recorded as created. `HeadBucket` is where a client reads the answer —
//! HTTP 404 for a bucket Amazon S3 does not hold.
//!
//! The rows differ along what the rule generalises over:
//!
//! * WHICH follow-up call is refused, and so which error the failure carries:
//!   `PutBucketNotificationConfiguration` refusing a destination it cannot validate ("after
//!   Amazon S3 receives this request, it first verifies that any Amazon Simple Notification
//!   Service (Amazon SNS) or Amazon Simple Queue Service (Amazon SQS) destination exists, and
//!   that the bucket owner has permission to publish to it"), and `PutBucketReplication`
//!   refusing a bucket that is not versioning-enabled ("For information about enabling
//!   versioning on a bucket, see Using Versioning") — a different call, with a different
//!   message, reached only when the bucket already exists;
//! * WHICH destination kind fails that validation, and so which other service decides it: an
//!   Amazon SQS queue, an Amazon SNS topic, each existing and carrying no resource policy, so
//!   each grants Amazon S3 nothing;
//! * WHAT the create asks for on failure (the default rollback, `DisableRollback=true`,
//!   `OnFailure=DO_NOTHING`, `OnFailure=DELETE`), which decides the stack's terminal status
//!   and the fate of the SIBLING bucket the create had finished — the control that keeps the
//!   404 on the half-made bucket from being vacuous.
//!
//! Each row also reads the failed resource's own event, so the 404 is known to come from a
//! bucket that WAS created and then refused rather than from a declaration CloudFormation
//! never deployed: the reason CloudFormation quotes is the owning service's own, and the
//! sibling's events say whether it was created and whether it was removed again.
//!
//! Three crates have to agree for the rule to hold — CloudFormation deploys and disposes,
//! Amazon S3 refuses the follow-up call and then answers the read, and Amazon SQS or Amazon
//! SNS owns the destination that decides the refusal — so the suite lives here rather than in
//! any one of them. The sibling suite
//! `cloudformation_rollback_deletes_the_buckets_it_created` pins the same rule for the
//! default rollback and the destination that does not exist at all; what this one adds is the
//! other follow-up call and the other failure dispositions.
//!
//! Nothing here waits on the wall clock: a stack's terminal status is reached by looking
//! again until it holds, and every read is made once it does.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, members_of, opt_element};

/// The Region every request here is signed for, and so the Region the stacks, their buckets
/// and their destinations belong to.
const REGION: &str = "us-east-1";

/// How long this suite keeps LOOKING for a stack's terminal status, and how often. Generous:
/// the deadline is a safety net, never a measurement — what is asserted is the status, and
/// the loop stops the instant the stack reports one.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The statuses a `CreateStack` settles in, whichever of the documented failure behaviours
/// it ran under: there is nothing more to wait for once a stack reports one of them.
const TERMINAL_STATUSES: &[&str] = &[
    "CREATE_COMPLETE",
    "CREATE_FAILED",
    "ROLLBACK_COMPLETE",
    "ROLLBACK_FAILED",
    "DELETE_COMPLETE",
    "DELETE_FAILED",
];

/// The resource statuses the rows read off a bucket's own events.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const CREATE_FAILED: &str = "CREATE_FAILED";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The logical ids the template declares: the bucket the create finishes, and the one whose
/// own create is refused part-way.
const FINISHED: &str = "BucketA";
const HALF_MADE: &str = "BucketB";

/// The event every notification configuration below subscribes to. Which event it is does
/// not enter the rule: a destination is validated when the configuration is INSTALLED,
/// before any object changes.
const SUBSCRIBED_EVENT: &str = "s3:ObjectCreated:*";

/// The `<Message>` Amazon S3 answers a `NotificationConfiguration` whose destination it could
/// not validate with — the fixed sentence of that refusal, which CloudFormation quotes in the
/// failed resource's `ResourceStatusReason`.
const UNABLE_TO_VALIDATE: &str = "Unable to validate the following destination configurations";

/// The `<Message>` Amazon S3 answers a `PutBucketReplication` on a bucket whose versioning is
/// not enabled with.
const VERSIONING_REQUIRED: &str =
    "Versioning must be 'Enabled' on the bucket to apply a replication configuration";

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

/// One awsQuery call of `service`, as its status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// One CloudFormation call (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
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

/// A queue called `name`, and the `QueueArn` Amazon SQS reports for it. It carries no
/// `Policy`, so it grants Amazon S3 nothing.
async fn given_ungranted_queue(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let reported = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["All"] }),
    )
    .await;
    assert!(
        reported["Attributes"]["Policy"].is_null(),
        "case {case:?}: the queue must carry no Policy, got {reported}"
    );
    reported["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {reported}"))
        .to_string()
}

/// A topic called `name`, and the `TopicArn` Amazon SNS minted for it. Its created policy
/// grants Amazon S3 nothing.
async fn given_ungranted_topic(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = query(router, "sns", "2010-03-31", "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SNS CreateTopic, got {status} with {body}"
    );
    element(&body, "TopicArn")
}

/// Which follow-up call of the bucket's declaration is refused, which is what fails the
/// resource after `CreateBucket` has already made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// `PutBucketNotificationConfiguration`, naming a queue that exists and grants Amazon S3
    /// nothing.
    QueueDestinationWithoutAGrant,
    /// `PutBucketNotificationConfiguration`, naming a topic that exists and grants Amazon S3
    /// nothing.
    TopicDestinationWithoutAGrant,
    /// `PutBucketReplication`, on a bucket whose declaration enables no versioning.
    ReplicationWithoutVersioning,
}

impl Refused {
    /// The destination this failure needs in the service that owns it, created as the
    /// ordinary resource it is — and the empty string for a failure that needs none.
    async fn destination(self, router: &MultiServiceRouter, case: &str, suffix: &str) -> String {
        match self {
            Self::QueueDestinationWithoutAGrant => {
                given_ungranted_queue(router, case, &format!("half-made-queue-{suffix}")).await
            }
            Self::TopicDestinationWithoutAGrant => {
                given_ungranted_topic(router, case, &format!("half-made-topic-{suffix}")).await
            }
            Self::ReplicationWithoutVersioning => String::new(),
        }
    }

    /// The properties the half-made bucket declares beside its name, which are what take the
    /// deployment to the refused call.
    ///
    /// The member names are `AWS::S3::Bucket`'s own: a queue destination is a
    /// `QueueConfigurations` entry carrying `Queue` and `Event`, a topic destination a
    /// `TopicConfigurations` entry carrying `Topic` and `Event`, and a replication is a
    /// `ReplicationConfiguration` carrying `Role` and `Rules`.
    fn declared(self, destination: &str, bucket: &str) -> Value {
        match self {
            Self::QueueDestinationWithoutAGrant => json!({
                "NotificationConfiguration": {
                    "QueueConfigurations": [
                        { "Queue": destination, "Event": SUBSCRIBED_EVENT },
                    ],
                },
            }),
            Self::TopicDestinationWithoutAGrant => json!({
                "NotificationConfiguration": {
                    "TopicConfigurations": [
                        { "Topic": destination, "Event": SUBSCRIBED_EVENT },
                    ],
                },
            }),
            // `Role` and each rule's `Status` and `Destination.Bucket` are the members
            // `ReplicationConfiguration` requires; the declaration carries no
            // `VersioningConfiguration`, which is what Amazon S3 refuses the call for.
            Self::ReplicationWithoutVersioning => json!({
                "ReplicationConfiguration": {
                    "Role": format!(
                        "arn:aws:iam::{}:role/{bucket}-replication",
                        engine::emulator_account_id()
                    ),
                    "Rules": [{
                        "Status": "Enabled",
                        "Prefix": "logs/",
                        "Destination": { "Bucket": format!("arn:aws:s3:::{bucket}-replica") },
                    }],
                },
            }),
        }
    }

    /// The owning service's own sentence the failed resource's reason has to quote, which is
    /// what says WHICH call was refused.
    fn reported(self) -> &'static str {
        match self {
            Self::QueueDestinationWithoutAGrant | Self::TopicDestinationWithoutAGrant => {
                UNABLE_TO_VALIDATE
            }
            Self::ReplicationWithoutVersioning => VERSIONING_REQUIRED,
        }
    }
}

/// What the create asks for when a resource of it fails — the three documented `OnFailure`
/// behaviours, with the `DisableRollback` spelling of the middle one beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnFailure {
    /// Nothing stated, so the documented default: roll the stack creation back.
    DefaultRollback,
    /// `DisableRollback=true`.
    DisableRollback,
    /// `OnFailure=DO_NOTHING`, which that parameter's own documentation calls equivalent to
    /// the one above.
    DoNothing,
    /// `OnFailure=DELETE`.
    Delete,
}

impl OnFailure {
    /// The `CreateStack` members this behaviour is asked for with.
    fn members(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::DefaultRollback => &[],
            Self::DisableRollback => &[("DisableRollback", "true")],
            Self::DoNothing => &[("OnFailure", "DO_NOTHING")],
            Self::Delete => &[("OnFailure", "DELETE")],
        }
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    refused: Refused,
    on_failure: OnFailure,
    /// The status the stack reports once it has settled.
    settles_in: &'static str,
    /// Whether the SIBLING bucket — the one the create finished — is still there afterwards.
    keeps_the_sibling: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a notification naming a queue that grants Amazon S3 nothing, rolled back",
        refused: Refused::QueueDestinationWithoutAGrant,
        on_failure: OnFailure::DefaultRollback,
        settles_in: "ROLLBACK_COMPLETE",
        keeps_the_sibling: false,
    },
    Row {
        case: "a notification naming a topic that grants Amazon S3 nothing, rolled back",
        refused: Refused::TopicDestinationWithoutAGrant,
        on_failure: OnFailure::DefaultRollback,
        settles_in: "ROLLBACK_COMPLETE",
        keeps_the_sibling: false,
    },
    Row {
        case: "a replication configuration on a bucket with no versioning, rolled back",
        refused: Refused::ReplicationWithoutVersioning,
        on_failure: OnFailure::DefaultRollback,
        settles_in: "ROLLBACK_COMPLETE",
        keeps_the_sibling: false,
    },
    Row {
        case: "a notification naming an ungranted queue, with rollback disabled",
        refused: Refused::QueueDestinationWithoutAGrant,
        on_failure: OnFailure::DisableRollback,
        settles_in: "CREATE_FAILED",
        keeps_the_sibling: true,
    },
    Row {
        case: "a replication configuration with no versioning, asked to do nothing on failure",
        refused: Refused::ReplicationWithoutVersioning,
        on_failure: OnFailure::DoNothing,
        settles_in: "CREATE_FAILED",
        keeps_the_sibling: true,
    },
    Row {
        case: "a notification naming an ungranted topic, asked to delete the stack on failure",
        refused: Refused::TopicDestinationWithoutAGrant,
        on_failure: OnFailure::Delete,
        settles_in: "DELETE_COMPLETE",
        keeps_the_sibling: false,
    },
];

/// The template a row deploys: one plain bucket the create finishes, and one whose
/// declaration takes the refused follow-up call — so the rule is asserted both for the
/// resource that failed and for a sibling the create had already completed.
fn template(row: &Row, finished: &str, half_made: &str, destination: &str) -> String {
    let mut properties = serde_json::Map::new();
    properties.insert("BucketName".to_string(), json!(half_made));
    let declared = row.refused.declared(destination, half_made);
    for (name, value) in declared.as_object().expect("the declaration is an object") {
        properties.insert(name.clone(), value.clone());
    }
    json!({
        "Resources": {
            FINISHED: {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": finished },
            },
            HALF_MADE: {
                "Type": "AWS::S3::Bucket",
                "Properties": Value::Object(properties),
            },
        }
    })
    .to_string()
}

/// Look at the stack until it reports a terminal status, and answer that status together
/// with the body it was read from.
///
/// The CONDITION is what is waited for, not a span: each look is a `DescribeStacks`, which
/// is also what materialises the transitions the stack's own clock owes. The stack is
/// addressed by its ID, which resolves a stack a `DELETE` disposition took away too.
async fn settled(router: &MultiServiceRouter, case: &str, stack_id: &str) -> (String, String) {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_id)]).await;
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

/// One resource event, as the three members these assertions read off it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    logical_id: String,
    status: String,
    reason: String,
}

/// Every event of the stack, in the order `DescribeStackEvents` reports them.
///
/// Each member's three elements are read from the MEMBER that carries them, so an event
/// omitting one — a resource transition with no reason — contributes the empty string for it
/// rather than borrowing the next event's value.
async fn events(router: &MultiServiceRouter, case: &str, stack_id: &str) -> Vec<Event> {
    let (status, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
         {body}"
    );
    let first =
        |member: &str, name: &str| elements(member, name).into_iter().next().unwrap_or_default();
    members_of(&body)
        .into_iter()
        .map(|member| Event {
            logical_id: first(member, "LogicalResourceId"),
            status: first(member, "ResourceStatus"),
            reason: first(member, "ResourceStatusReason"),
        })
        .collect()
}

/// The statuses one logical id of the stack reported, in the order the events carry them.
fn statuses_of(all: &[Event], logical_id: &str) -> Vec<String> {
    all.iter()
        .filter(|event| event.logical_id == logical_id)
        .map(|event| event.status.clone())
        .collect()
}

#[tokio::test]
async fn a_resource_refused_part_way_through_its_create_is_not_left_behind() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // One emulator per row, so a bucket, queue or topic name one row spells cannot
        // decide another row's deployment.
        let router = router();

        let destination = row
            .refused
            .destination(&router, case, &index.to_string())
            .await;
        let stack = format!("half-made-{index}");
        let finished = format!("half-made-finished-{index}");
        let half_made = format!("half-made-refused-{index}");
        let body = template(row, &finished, &half_made, &destination);

        let mut members: Vec<(&str, &str)> = vec![("StackName", &stack), ("TemplateBody", &body)];
        members.extend_from_slice(row.on_failure.members());
        let (status, created) = cloudformation(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );
        let stack_id = opt_element(&created, "StackId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a StackId from CreateStack, got {created}")
        });

        let (reported, described) = settled(&router, case, &stack_id).await;
        assert_eq!(
            reported, row.settles_in,
            "case {case:?}: expected the stack to settle in {:?}, got {reported:?} in \
             {described}",
            row.settles_in
        );

        // The failed resource's own event says the create got as far as the follow-up call
        // and the owning service refused THAT: one CREATE_FAILED, quoting that service's
        // sentence.
        let all = events(&router, case, &stack_id).await;
        let failed: Vec<&Event> = all
            .iter()
            .filter(|event| event.logical_id == HALF_MADE && event.status == CREATE_FAILED)
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one {CREATE_FAILED} event for {HALF_MADE}, got \
             {failed:?}"
        );
        let expected = row.refused.reported();
        assert!(
            failed[0].reason.contains(expected),
            "case {case:?}: expected the failure reason to quote {expected:?}, got {:?}",
            failed[0].reason
        );

        // The sibling was created, and its events say whether it was taken away again —
        // which is the stack-level disposition, not the provider's own cleanup.
        let sibling = statuses_of(&all, FINISHED);
        assert!(
            sibling.iter().any(|status| status == CREATE_COMPLETE),
            "case {case:?}: expected {FINISHED} to report {CREATE_COMPLETE}, got {sibling:?}"
        );
        assert_eq!(
            sibling.iter().any(|status| status == DELETE_COMPLETE),
            !row.keeps_the_sibling,
            "case {case:?}: expected {FINISHED} to report {DELETE_COMPLETE} only where the \
             stack does not keep it (kept: {}), got {sibling:?}",
            row.keeps_the_sibling
        );

        // The rule, read where a client reads it: the half-made bucket is gone whatever the
        // stack did with itself, and the finished one is there exactly as long as the stack
        // keeps it.
        let answered = head_bucket(&router, &half_made).await;
        assert_eq!(
            answered, 404,
            "case {case:?}: after {reported}, HeadBucket on the refused bucket \
             {half_made:?} must answer 404, got {answered}"
        );
        let answered = head_bucket(&router, &finished).await;
        let expected = if row.keeps_the_sibling { 200 } else { 404 };
        assert_eq!(
            answered, expected,
            "case {case:?}: after {reported}, HeadBucket on the finished bucket \
             {finished:?} must answer {expected}, got {answered}"
        );
    }
}
