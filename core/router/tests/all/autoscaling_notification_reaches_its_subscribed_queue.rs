// SPDX-License-Identifier: CC0-1.0

//! A group's Amazon SNS notification configuration reaches the queue subscribed to its
//! topic — through the WHOLE mounted service set, the one `core/router/src/lib.rs`
//! assembles for the binary.
//!
//! The RULE under test: **an Auto Scaling group configured with
//! `PutNotificationConfiguration` publishes each event type that configuration names onto
//! the topic it names and onto no other topic — the configuration itself as
//! `autoscaling:TEST_NOTIFICATION`, every instance the group launches as
//! `autoscaling:EC2_INSTANCE_LAUNCH` and every instance it terminates as
//! `autoscaling:EC2_INSTANCE_TERMINATE`, each naming the exact instance that moved — so a
//! queue subscribed to that topic (and granting the SNS service principal the send)
//! receives one message per event.**
//!
//! Four services have to agree for that to hold — Auto Scaling publishes, Amazon EC2 holds
//! the instances it names, SNS fans out, SQS holds what arrived — and the publish Auto
//! Scaling makes is an in-process call addressed the way a client's own `sns`-scoped call
//! is addressed, so WHICH mounted model claims it is only decidable with every service
//! mounted. A crate-local suite mounts the four it needs and can therefore never see a
//! fifth service claiming that call, nor the key resolving to none:
//! `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/tests/all/sns_notifications.rs`
//! pins the configuration's own rules — the upsert, the selection, the refusals — over
//! tables of event types, capacities and topics; what only this suite can reach is the same
//! path over the production service set.
//!
//! The rows differ along what the rule generalises over: which event the capacity change
//! publishes (a launch, a termination) and the Region the whole wiring lives in.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    given_topic_with_subscriber, router, Grant,
};
use crate::query_wire::{element, elements, form_encode};

/// The three event types this suite configures and reads, spelled out here rather than
/// imported, so a wrong constant in the implementation cannot make the test pass.
const TEST_NOTIFICATION: &str = "autoscaling:TEST_NOTIFICATION";
const INSTANCE_LAUNCH: &str = "autoscaling:EC2_INSTANCE_LAUNCH";
const INSTANCE_TERMINATE: &str = "autoscaling:EC2_INSTANCE_TERMINATE";

/// The launch template every group here launches from.
const TEMPLATE: &str = "router-notification-template";

/// How many times a leg polls the queue before calling a notification undelivered, and how
/// long it waits between polls: a group's instance lifecycle step is clock-owed, so the
/// poll is driven off what has ARRIVED rather than off a fixed sleep.
const POLLS: usize = 60;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One form-wire (`awsQuery` / `ec2Query`) call against the service whose signing name is
/// `signing_name`, which must answer 200.
async fn form_call(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, text) =
        form_call_answering(router, region, signing_name, version, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {action}, got {status} with {text}"
    );
    text
}

/// The same call, with whatever status it answered.
async fn form_call_answering(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Auto Scaling call that must answer 200.
async fn autoscaling(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    form_call(
        router,
        case,
        region,
        "autoscaling",
        "2011-01-01",
        action,
        params,
    )
    .await
}

/// One Amazon EC2 call that must answer 200.
async fn ec2(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    form_call(router, case, region, "ec2", "2016-11-15", action, params).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sqs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Take every notification waiting on the queue at `url`, read out of the SNS envelope the
/// queue carries it in, deleting what it hands over so a later leg reads only what arrived
/// since.
async fn drain(router: &MultiServiceRouter, case: &str, region: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        region,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut notifications = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: the body is not the SNS envelope ({error}): {body}")
        });
        let inner = envelope["Message"].as_str().unwrap_or_default();
        notifications.push(serde_json::from_str(inner).unwrap_or_else(|error| {
            panic!("case {case:?}: the message is not a notification document ({error}): {inner}")
        }));
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                case,
                region,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    notifications
}

/// Drain until `wanted` notifications carrying `event` have arrived, or the polls run out.
async fn drain_until(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    event: &str,
    wanted: usize,
) -> Vec<Value> {
    let mut arrived: Vec<Value> = Vec::new();
    for _ in 0..POLLS {
        arrived.extend(
            drain(router, case, region, url)
                .await
                .into_iter()
                .filter(|notification| notification["Event"] == json!(event)),
        );
        if arrived.len() >= wanted {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    arrived
}

/// The instance ids the group `name` reports right now.
async fn group_instances(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> Vec<String> {
    let described = autoscaling(
        router,
        case,
        region,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    elements(&described, "InstanceId")
}

#[tokio::test]
async fn an_asg_notification_reaches_the_queue_subscribed_to_its_topic() {
    // (case, the Region the wiring lives in, how the queue grants SNS the send, the capacity
    //  the configured group is moved to, the event that move publishes, how many of them)
    let rows: &[(&str, &str, Grant, i32, &str, usize)] = &[
        (
            "a launch, with the grant scoped to the publishing topic",
            "us-west-2",
            Grant::ScopedToTopic,
            3,
            INSTANCE_LAUNCH,
            2,
        ),
        (
            "a termination, with an unscoped grant in another Region",
            "eu-west-1",
            Grant::Unscoped,
            0,
            INSTANCE_TERMINATE,
            1,
        ),
    ];

    for (case, region, grant, moved_to, event, expected) in rows {
        let router = router();
        let (topic, url) =
            given_topic_with_subscriber(&router, case, region, "asg-notifications", *grant).await;
        // A second topic of the SAME group, configured for nothing this group's capacity
        // does: it is told about its own configuration and about no instance.
        let (other_topic, other_url) =
            given_topic_with_subscriber(&router, case, region, "asg-test-only", *grant).await;

        // What the group launches from: a launch template of the account's own Amazon EC2.
        ec2(
            &router,
            case,
            region,
            "CreateLaunchTemplate",
            &[
                ("LaunchTemplateName", TEMPLATE),
                ("LaunchTemplateData.ImageId", "ami-0abcdef1234567890"),
                ("LaunchTemplateData.InstanceType", "t3.micro"),
            ],
        )
        .await;

        let group = "notified-group";
        let zone = format!("{region}a");
        autoscaling(
            &router,
            case,
            region,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", group),
                ("MinSize", "0"),
                ("MaxSize", "3"),
                ("DesiredCapacity", "1"),
                ("LaunchTemplate.LaunchTemplateName", TEMPLATE),
                ("AvailabilityZones.member.1", &zone),
            ],
        )
        .await;
        let arn = element(
            &autoscaling(
                &router,
                case,
                region,
                "DescribeAutoScalingGroups",
                &[("AutoScalingGroupNames.member.1", group)],
            )
            .await,
            "AutoScalingGroupARN",
        );

        // The configuration: this topic is told about the test notification and about both
        // instance lifecycle events; the other about the test notification alone.
        autoscaling(
            &router,
            case,
            region,
            "PutNotificationConfiguration",
            &[
                ("AutoScalingGroupName", group),
                ("TopicARN", &topic),
                ("NotificationTypes.member.1", TEST_NOTIFICATION),
                ("NotificationTypes.member.2", INSTANCE_LAUNCH),
                ("NotificationTypes.member.3", INSTANCE_TERMINATE),
            ],
        )
        .await;
        autoscaling(
            &router,
            case,
            region,
            "PutNotificationConfiguration",
            &[
                ("AutoScalingGroupName", group),
                ("TopicARN", &other_topic),
                ("NotificationTypes.member.1", TEST_NOTIFICATION),
            ],
        )
        .await;

        // The configuration confirms itself on each topic that asked for the test.
        for (topic_arn, queue_url) in [(&topic, &url), (&other_topic, &other_url)] {
            let confirmations = drain_until(
                &router,
                case,
                region,
                queue_url,
                TEST_NOTIFICATION,
                1,
            )
            .await;
            assert_eq!(
                confirmations.len(),
                1,
                "case {case:?}: {topic_arn} must receive one {TEST_NOTIFICATION}, got \
                 {confirmations:?}"
            );
            assert_eq!(
                confirmations[0]["AutoScalingGroupName"],
                json!(group),
                "case {case:?}: the confirmation must name the group, got {}",
                confirmations[0]
            );
            assert_eq!(
                confirmations[0]["AutoScalingGroupARN"],
                json!(arn),
                "case {case:?}: the confirmation must name the group's ARN, got {}",
                confirmations[0]
            );
        }

        // The group's own read is what says which instances it holds, and so which instance
        // id a notification must name.
        let held = group_instances(&router, case, region, group).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the created group must hold the one instance it asked for, got \
             {held:?}"
        );
        // The create's own launch is published too, so it is taken off the queue before the
        // row's own move: what is left is what that move published.
        drain_until(&router, case, region, &url, INSTANCE_LAUNCH, 1).await;

        autoscaling(
            &router,
            case,
            region,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", group),
                ("DesiredCapacity", &moved_to.to_string()),
            ],
        )
        .await;
        let published = drain_until(&router, case, region, &url, event, *expected).await;
        assert_eq!(
            published.len(),
            *expected,
            "case {case:?}: expected {expected} {event} notifications on {topic}, got \
             {published:?}"
        );
        for notification in &published {
            let instance_id = notification["EC2InstanceId"].as_str().unwrap_or_default();
            assert!(
                instance_id.starts_with("i-"),
                "case {case:?}: the notification must name the instance that moved, got \
                 {notification}"
            );
            assert!(
                !notification["ActivityId"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "case {case:?}: the notification must name the activity that moved it, got \
                 {notification}"
            );
            if *event == INSTANCE_TERMINATE {
                assert!(
                    held.contains(&instance_id.to_string()),
                    "case {case:?}: a termination must name an instance the group held; named \
                     {instance_id}, held {held:?}"
                );
            }
        }

        // The topic configured for the test notification alone is told about no instance.
        let withheld: Vec<Value> = drain(&router, case, region, &other_url)
            .await
            .into_iter()
            .filter(|notification| notification["Event"] != json!(TEST_NOTIFICATION))
            .collect();
        assert!(
            withheld.is_empty(),
            "case {case:?}: {other_topic} asked about no instance and must receive none, got \
             {withheld:?}"
        );
    }
}
