// SPDX-License-Identifier: CC0-1.0

//! Amazon EC2 Auto Scaling PUBLISHES the notifications a group's notification
//! configurations ask for, onto the Amazon SNS topics they name.
//!
//! The RULE under test: `PutNotificationConfiguration` stores, for one group, the event
//! types one topic is to be told about — replacing whatever that topic was configured with
//! before, and leaving the group's other topics alone — and a group configured for an event
//! type publishes that event onto every topic configured for it and onto no other topic. So
//! storing a configuration confirms it on its topic as the
//! `autoscaling:TEST_NOTIFICATION` event, every instance the group LAUNCHES is published to
//! the topics configured for `autoscaling:EC2_INSTANCE_LAUNCH` and every instance it
//! TERMINATES to the topics configured for `autoscaling:EC2_INSTANCE_TERMINATE`, each
//! naming the exact instance that moved and the scaling activity that moved it; a topic
//! configured for other types receives nothing, and the stored configurations are what
//! `DescribeNotificationConfigurations` reports back.
//!
//! The rows span what the rule generalises over: which event types a configuration names
//! (one, several, the whole catalog, a set replaced by a later call), how many topics one
//! group is configured with, which door moved the capacity (the create itself, a later
//! `SetDesiredCapacity` up or down), how many instances one change moves, and whether the
//! topic the assertion reads was configured for the event at all.
//!
//! Amazon SNS and Amazon SQS are mounted BESIDE Auto Scaling and Amazon EC2 on the SAME
//! engine, so a publication travels the real in-process callback, through the real topic,
//! to a real queue subscribed to it: nothing here reads the publisher's own state for the
//! delivery.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::support::{
    borrowed, ec2_query, form_encode, json_rpc, p, query, seed_subnets, tag_value, tag_values,
    REGION,
};

/// The three event types this suite configures and reads, spelled out here rather than
/// imported, so a wrong constant in the implementation cannot make a test pass.
const TEST_NOTIFICATION: &str = "autoscaling:TEST_NOTIFICATION";
const INSTANCE_LAUNCH: &str = "autoscaling:EC2_INSTANCE_LAUNCH";
const INSTANCE_TERMINATE: &str = "autoscaling:EC2_INSTANCE_TERMINATE";

/// The `Service` every notification of this service names itself by.
const NOTIFICATION_SERVICE: &str = "AWS Auto Scaling";

/// The API version an Amazon SNS awsQuery client sends.
const SNS_API_VERSION: &str = "2010-03-31";

/// The launch template every group below launches from.
const TEMPLATE: &str = "notification-template";

/// How often a poll re-reads the subscribed queue.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: the lifecycle span the service owes a
/// transition, with ample margin for a loaded test machine.
fn poll_budget() -> Duration {
    autoscaling::state::LIFECYCLE_TRANSITION + Duration::from_secs(5)
}

/// How long a row waits before asserting that NOTHING was delivered: long enough that a
/// delivery that was going to happen would have.
fn quiet_budget() -> Duration {
    autoscaling::state::LIFECYCLE_TRANSITION + Duration::from_secs(1)
}

/// One emulator: Auto Scaling, Amazon EC2, Amazon SNS and Amazon SQS over ONE shared engine
/// — the four services a published notification travels through, mounted the way the
/// production router mounts them.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One awsQuery form call against Amazon SNS, which must succeed.
async fn sns(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={SNS_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: SNS {action} expected 200, got {status}: {text}"
    );
    text
}

/// One Amazon SQS call, which must succeed.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = json_rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SQS {operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// A topic of this emulator with a queue of this emulator subscribed to it, the queue
/// carrying the policy AWS documents for a queue an SNS topic delivers to. Answers the
/// topic's ARN and the queue's URL.
async fn given_topic_with_subscriber(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let created = sns(router, case, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = tag_value(&created, "TopicArn");
    assert!(
        !topic_arn.is_empty(),
        "case {case:?}: CreateTopic must report the topic's ARN, got {created}"
    );

    let queue = sqs(router, case, "CreateQueue", json!({ "QueueName": name })).await;
    let url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {queue}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    // A delivery Amazon SNS is not granted is withheld by the queue, so the queue grants
    // the SNS service principal the send AWS documents for a subscribed queue.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    sns(
        router,
        case,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    (topic_arn, url)
}

/// Every notification waiting on the queue at `url` right now: the document Auto Scaling
/// published, read out of the SNS envelope the queue carries it in.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let envelope: Value =
                serde_json::from_str(message["Body"].as_str().unwrap_or_default())
                    .unwrap_or(Value::Null);
            serde_json::from_str(envelope["Message"].as_str().unwrap_or_default())
                .unwrap_or(envelope)
        })
        .collect()
}

/// Poll the queue at `url` — and ONLY the queue — until it has delivered `wanted`
/// notifications carrying `event`, or the budget runs out.
///
/// Nothing in this loop calls Auto Scaling, which is the point: a client that watches a
/// group through its notifications makes no further call to the service that publishes.
async fn collect(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    event: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + poll_budget();
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(
            drain(router, case, url)
                .await
                .into_iter()
                .filter(|notification| notification["Event"] == json!(event)),
        );
        if collected.len() >= wanted || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create the launch template and the subnet every group of this suite launches into, and
/// answer the subnet id.
async fn given_launch_source(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> String {
    let subnet = seed_subnets(engine, 1)
        .first()
        .expect("one seeded subnet")
        .id
        .clone();
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", TEMPLATE),
            p("LaunchTemplateData.ImageId", "ami-0abcdef1234567890"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate expected 200, got {status}: {body}"
    );
    subnet
}

/// Create one group with `desired` capacity, and answer its ARN.
async fn given_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    subnet: &str,
    desired: i32,
) -> String {
    let params = vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "4"),
        p("DesiredCapacity", desired.to_string()),
        p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        p("VPCZoneIdentifier", subnet),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup expected 200, got {status}: {body}"
    );
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}"
    );
    let arn = tag_value(&body, "AutoScalingGroupARN");
    assert!(
        !arn.is_empty(),
        "case {case:?}: the group must report its ARN, got {body}"
    );
    arn
}

/// Configure the group `group` to notify `topic_arn` about `events`.
async fn put_notification_configuration(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    topic_arn: &str,
    events: &[&str],
) -> (u16, String) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("TopicARN", topic_arn),
    ];
    for (index, event) in events.iter().enumerate() {
        params.push(p(
            &format!("NotificationTypes.member.{}", index + 1),
            *event,
        ));
    }
    let answered = query(
        router,
        "PutNotificationConfiguration",
        &borrowed(&params),
    )
    .await;
    let _ = case;
    answered
}

/// The instance ids the group `name` reports right now.
async fn group_instance_ids(router: &MultiServiceRouter, case: &str, name: &str) -> BTreeSet<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceId").into_iter().collect()
}

/// Ask for `desired` capacity on the group `name`.
async fn set_desired_capacity(router: &MultiServiceRouter, case: &str, name: &str, desired: i32) {
    let (status, body) = query(
        router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", name),
            ("DesiredCapacity", &desired.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetDesiredCapacity expected 200, got {status}: {body}"
    );
}

/// The instance ids a set of notifications name.
fn named_instances(notifications: &[Value]) -> BTreeSet<String> {
    notifications
        .iter()
        .filter_map(|notification| notification["EC2InstanceId"].as_str())
        .map(str::to_string)
        .collect()
}

/// Assert one notification is a well-formed report of `event` about the group `group`, and
/// answer the instance id it names (empty for a notification about no instance).
fn assert_notification(case: &str, notification: &Value, event: &str, group: &str, arn: &str) {
    assert_eq!(
        notification["Event"],
        json!(event),
        "case {case:?}: expected Event {event}, got {notification}"
    );
    assert_eq!(
        notification["Service"],
        json!(NOTIFICATION_SERVICE),
        "case {case:?}: expected Service {NOTIFICATION_SERVICE}, got {notification}"
    );
    assert_eq!(
        notification["AutoScalingGroupName"],
        json!(group),
        "case {case:?}: expected the group {group}, got {notification}"
    );
    assert_eq!(
        notification["AutoScalingGroupARN"],
        json!(arn),
        "case {case:?}: expected the group's ARN {arn}, got {notification}"
    );
    assert_eq!(
        notification["AccountId"],
        json!(engine::emulator_account_id()),
        "case {case:?}: expected the account every emulated ARN carries, got {notification}"
    );
    assert!(
        !notification["Time"].as_str().unwrap_or_default().is_empty(),
        "case {case:?}: the notification reports when it happened, got {notification}"
    );
    assert!(
        !notification["RequestId"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: the notification reports the request behind it, got {notification}"
    );
}

#[tokio::test]
async fn a_stored_configuration_is_reported_back_and_replaced_per_topic() {
    let (router, engine) = emulator();
    let subnet = given_launch_source(&router, &engine).await;
    let case = "the configurations of one group";
    let group = "configured-group";
    given_group(&router, case, group, &subnet, 0).await;
    let other_group = "other-group";
    given_group(&router, case, other_group, &subnet, 0).await;

    let (first_topic, _) = given_topic_with_subscriber(&router, case, "first-topic").await;
    let (second_topic, _) = given_topic_with_subscriber(&router, case, "second-topic").await;
    let (other_topic, _) = given_topic_with_subscriber(&router, case, "other-group-topic").await;

    // (case, the topic configured, the event types it is configured with, what the group
    //  must report for that topic afterwards) — the rows differ in how many types one call
    //  names, whether a later call REPLACES a topic's types or configures another topic,
    //  and whether the whole catalog is named.
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "one event type",
            &first_topic,
            &[INSTANCE_LAUNCH],
            &[INSTANCE_LAUNCH],
        ),
        (
            "a second topic of the same group, with two types",
            &second_topic,
            &[INSTANCE_LAUNCH, INSTANCE_TERMINATE],
            &[INSTANCE_LAUNCH, INSTANCE_TERMINATE],
        ),
        (
            "the same topic again, which REPLACES its types rather than adding to them",
            &first_topic,
            &[TEST_NOTIFICATION],
            &[TEST_NOTIFICATION],
        ),
        (
            "a type named twice, which configures one",
            &first_topic,
            &[INSTANCE_TERMINATE, INSTANCE_TERMINATE],
            &[INSTANCE_TERMINATE],
        ),
        (
            "every type of the service's own catalog",
            &first_topic,
            autoscaling::state::AUTO_SCALING_NOTIFICATION_TYPES,
            autoscaling::state::AUTO_SCALING_NOTIFICATION_TYPES,
        ),
    ];

    for (row, topic, configured, expected) in rows {
        let (status, body) =
            put_notification_configuration(&router, row, group, topic, configured).await;
        assert_eq!(
            status, 200,
            "case {row:?}: PutNotificationConfiguration expected 200, got {status}: {body}"
        );
        let (status, body) = query(
            &router,
            "DescribeNotificationConfigurations",
            &[("AutoScalingGroupNames.member.1", group)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: DescribeNotificationConfigurations expected 200, got {status}: {body}"
        );
        let reported: Vec<(String, String)> = tag_values(&body, "TopicARN")
            .into_iter()
            .zip(tag_values(&body, "NotificationType"))
            .collect();
        let for_topic: Vec<String> = reported
            .iter()
            .filter(|(reported_topic, _)| reported_topic == *topic)
            .map(|(_, notification_type)| notification_type.clone())
            .collect();
        let mut wanted: Vec<String> = expected.iter().map(|event| (*event).to_string()).collect();
        wanted.sort();
        wanted.dedup();
        assert_eq!(
            for_topic, wanted,
            "case {row:?}: expected {wanted:?} for {topic}, got {for_topic:?} from {reported:?}"
        );
        assert!(
            reported
                .iter()
                .all(|(_, notification_type)| !notification_type.is_empty()),
            "case {row:?}: every reported configuration names its event type, got {reported:?}"
        );
    }

    // A configuration belongs to ONE group: the other group reports only its own.
    put_notification_configuration(&router, case, other_group, &other_topic, &[TEST_NOTIFICATION])
        .await;
    let (status, body) = query(
        &router,
        "DescribeNotificationConfigurations",
        &[("AutoScalingGroupNames.member.1", other_group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNotificationConfigurations expected 200, got {status}: {body}"
    );
    assert_eq!(
        tag_values(&body, "TopicARN"),
        vec![other_topic.clone()],
        "case {case:?}: a group reports only the topics configured for it, got {body}"
    );

    // Absent `AutoScalingGroupNames` reads every group of the account, and a name no group
    // answers to selects nothing rather than refusing the read.
    let (status, body) = query(&router, "DescribeNotificationConfigurations", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an unnarrowed read expected 200, got {status}: {body}"
    );
    let groups: BTreeSet<String> = tag_values(&body, "AutoScalingGroupName").into_iter().collect();
    assert_eq!(
        groups,
        BTreeSet::from([group.to_string(), other_group.to_string()]),
        "case {case:?}: an unnarrowed read reports every configured group, got {body}"
    );
    let (status, body) = query(
        &router,
        "DescribeNotificationConfigurations",
        &[("AutoScalingGroupNames.member.1", "no-such-group")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a read naming no stored group expected 200, got {status}: {body}"
    );
    assert!(
        tag_values(&body, "TopicARN").is_empty(),
        "case {case:?}: a read naming no stored group reports nothing, got {body}"
    );

    // One page at a time: the cursor the service issues reads the next one.
    let (status, first_page) = query(
        &router,
        "DescribeNotificationConfigurations",
        &[("MaxRecords", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a one-record page expected 200, got {status}: {first_page}"
    );
    assert_eq!(
        tag_values(&first_page, "TopicARN").len(),
        1,
        "case {case:?}: a one-record page holds one configuration, got {first_page}"
    );
    let next_token = tag_value(&first_page, "NextToken");
    assert!(
        !next_token.is_empty(),
        "case {case:?}: a page that does not end the list carries a cursor, got {first_page}"
    );
    let (status, second_page) = query(
        &router,
        "DescribeNotificationConfigurations",
        &[("MaxRecords", "1"), ("NextToken", &next_token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the next page expected 200, got {status}: {second_page}"
    );
    assert_ne!(
        tag_values(&second_page, "NotificationType"),
        tag_values(&first_page, "NotificationType"),
        "case {case:?}: the next page reports the next configuration, got {second_page}"
    );
}

#[tokio::test]
async fn storing_a_configuration_confirms_it_on_the_topic_that_asked_for_the_test() {
    let (router, engine) = emulator();
    let subnet = given_launch_source(&router, &engine).await;

    // (case, the event types configured, whether the topic is owed the test notification)
    // — the rows differ in whether the configuration SELECTS the test notification, how
    // many other types it names, and whether it names the whole catalog.
    let rows: &[(&str, &[&str], bool)] = &[
        (
            "a configuration naming the test notification alone",
            &[TEST_NOTIFICATION],
            true,
        ),
        (
            "a configuration naming it beside the lifecycle events",
            &[INSTANCE_LAUNCH, TEST_NOTIFICATION, INSTANCE_TERMINATE],
            true,
        ),
        (
            "a configuration naming every type of the service's own catalog",
            autoscaling::state::AUTO_SCALING_NOTIFICATION_TYPES,
            true,
        ),
        (
            "a configuration naming only the lifecycle events, which asked for no test",
            &[INSTANCE_LAUNCH, INSTANCE_TERMINATE],
            false,
        ),
    ];

    for (index, (case, configured, owed)) in rows.iter().enumerate() {
        let group = format!("confirmed-group-{index}");
        let arn = given_group(&router, case, &group, &subnet, 0).await;
        let (topic, url) =
            given_topic_with_subscriber(&router, case, &format!("confirmed-topic-{index}")).await;
        let (unconfigured_topic, unconfigured_url) =
            given_topic_with_subscriber(&router, case, &format!("unconfigured-topic-{index}"))
                .await;

        let (status, body) =
            put_notification_configuration(&router, case, &group, &topic, configured).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutNotificationConfiguration expected 200, got {status}: {body}"
        );

        if *owed {
            let delivered = collect(&router, case, &url, TEST_NOTIFICATION, 1).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: the configured topic must receive one {TEST_NOTIFICATION}, got \
                 {delivered:?}"
            );
            assert_notification(case, &delivered[0], TEST_NOTIFICATION, &group, &arn);
        } else {
            tokio::time::sleep(quiet_budget()).await;
            let delivered = drain(&router, case, &url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: {topic} asked for no test notification and must receive none, \
                 got {delivered:?}"
            );
        }

        // A topic nobody configured receives nothing — which is only distinguishable from
        // "nothing is ever published" because the topics of the rows above do receive
        // their own.
        tokio::time::sleep(quiet_budget()).await;
        let withheld = drain(&router, case, &unconfigured_url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: {unconfigured_topic} was configured for nothing and must receive \
             nothing, got {withheld:?}"
        );
    }
}

#[tokio::test]
async fn an_instance_the_group_moves_is_published_to_the_topics_configured_for_it() {
    // (case, the capacity the configured group is first asked for, the capacity it is then
    //  moved to, the event the row asserts, how many notifications that move publishes)
    let rows: &[(&str, i32, i32, &str, usize)] = &[
        ("a scale-out of one instance", 1, 1, INSTANCE_LAUNCH, 1),
        ("a scale-out of two instances at once", 2, 2, INSTANCE_LAUNCH, 2),
        ("a scale-out of two more on top of one", 1, 3, INSTANCE_LAUNCH, 2),
        ("a scale-in of one instance", 2, 1, INSTANCE_TERMINATE, 1),
        ("a scale-in to no instances at all", 2, 0, INSTANCE_TERMINATE, 2),
    ];

    for (index, (case, first_capacity, moved_to, event, expected)) in rows.iter().enumerate() {
        let (router, engine) = emulator();
        let subnet = given_launch_source(&router, &engine).await;
        let group = format!("moved-group-{index}");
        let (topic, url) =
            given_topic_with_subscriber(&router, case, &format!("moved-topic-{index}")).await;
        // A second topic of the SAME group, configured for the test notification only: it
        // is told about the configuration and about no instance the group moves.
        let (other_topic, other_url) =
            given_topic_with_subscriber(&router, case, &format!("test-only-topic-{index}")).await;

        let arn = given_group(&router, case, &group, &subnet, 0).await;
        let (status, body) = put_notification_configuration(
            &router,
            case,
            &group,
            &topic,
            &[INSTANCE_LAUNCH, INSTANCE_TERMINATE],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutNotificationConfiguration expected 200, got {status}: {body}"
        );
        put_notification_configuration(&router, case, &group, &other_topic, &[TEST_NOTIFICATION])
            .await;
        // The confirmation is the configuration's own, not a capacity change's: take it off
        // the second queue, so what is left there is what the capacity changes published.
        collect(&router, case, &other_url, TEST_NOTIFICATION, 1).await;

        // The first move: every instance it launches is published, and the notifications
        // name exactly the instances the group then reports.
        set_desired_capacity(&router, case, &group, *first_capacity).await;
        let launched = collect(
            &router,
            case,
            &url,
            INSTANCE_LAUNCH,
            usize::try_from(*first_capacity).expect("a capacity fits in a usize"),
        )
        .await;
        assert_eq!(
            launched.len(),
            usize::try_from(*first_capacity).expect("a capacity fits in a usize"),
            "case {case:?}: every instance the scale-out launched must be published, got \
             {launched:?}"
        );
        let first_reported = group_instance_ids(&router, case, &group).await;
        assert_eq!(
            named_instances(&launched),
            first_reported,
            "case {case:?}: the notifications must name the instances the group reports"
        );
        for notification in &launched {
            assert_notification(case, notification, INSTANCE_LAUNCH, &group, &arn);
            assert_activity(case, notification);
        }

        // The second move, when the row makes one: the same rule, in the other direction.
        if moved_to != first_capacity {
            set_desired_capacity(&router, case, &group, *moved_to).await;
            let published = collect(&router, case, &url, event, *expected).await;
            assert_eq!(
                published.len(),
                *expected,
                "case {case:?}: expected {expected} {event} notifications, got {published:?}"
            );
            for notification in &published {
                assert_notification(case, notification, event, &group, &arn);
                assert_activity(case, notification);
            }
            let named = named_instances(&published);
            if *event == INSTANCE_LAUNCH {
                let now_reported = group_instance_ids(&router, case, &group).await;
                assert!(
                    named.is_subset(&now_reported),
                    "case {case:?}: a launch names an instance the group holds; named {named:?}, \
                     reported {now_reported:?}"
                );
                assert!(
                    named.is_disjoint(&first_reported),
                    "case {case:?}: a launch names an instance the group did NOT hold before; \
                     named {named:?}, held {first_reported:?}"
                );
            } else {
                assert!(
                    named.is_subset(&first_reported),
                    "case {case:?}: a termination names an instance the group held; named \
                     {named:?}, held {first_reported:?}"
                );
                let left = group_instance_ids(&router, case, &group).await;
                assert!(
                    left.is_disjoint(&named),
                    "case {case:?}: an instance named by a termination has left the group; still \
                     reported {left:?}, named {named:?}"
                );
            }
        }

        // The topic configured for the test notification alone is told about no instance.
        tokio::time::sleep(quiet_budget()).await;
        let withheld: Vec<Value> = drain(&router, case, &other_url)
            .await
            .into_iter()
            .filter(|notification| notification["Event"] != json!(TEST_NOTIFICATION))
            .collect();
        assert!(
            withheld.is_empty(),
            "case {case:?}: {other_topic} is configured for {TEST_NOTIFICATION} only and must \
             receive no instance notification, got {withheld:?}"
        );
    }
}

/// Assert one lifecycle notification reports the scaling activity that moved the instance.
fn assert_activity(case: &str, notification: &Value) {
    let instance_id = notification["EC2InstanceId"].as_str().unwrap_or_default();
    assert!(
        instance_id.starts_with("i-"),
        "case {case:?}: the notification names the instance that moved, got {notification}"
    );
    assert!(
        !notification["ActivityId"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: the notification names the activity that moved it, got {notification}"
    );
    assert!(
        notification["Description"]
            .as_str()
            .unwrap_or_default()
            .contains(instance_id),
        "case {case:?}: the activity's description names the instance, got {notification}"
    );
    assert!(
        !notification["Cause"].as_str().unwrap_or_default().is_empty(),
        "case {case:?}: the activity's cause is reported, got {notification}"
    );
    assert!(
        !notification["StartTime"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: the activity's start is reported, got {notification}"
    );
}

#[tokio::test]
async fn a_configuration_outside_the_rules_is_refused() {
    let (router, engine) = emulator();
    let subnet = given_launch_source(&router, &engine).await;
    let case = "the refusals";
    let group = "refusing-group";
    given_group(&router, case, group, &subnet, 0).await;
    let (topic, _) = given_topic_with_subscriber(&router, case, "refusing-topic").await;

    // A name no group answers to: a configuration belongs to a group.
    let (status, body) =
        put_notification_configuration(&router, case, "no-such-group", &topic, &[TEST_NOTIFICATION])
            .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a configuration of an absent group is refused, got {status}: {body}"
    );
    assert!(
        body.contains("AutoScalingGroup name not found"),
        "case {case:?}: the refusal names the absent group as the cause, got {body}"
    );

    // An event type outside this service's own catalog, which `DescribeAutoScalingNotificationTypes`
    // publishes: the one value in the row the service does not notify about.
    for unknown in ["autoscaling:EC2_INSTANCE_RESTART", "EC2_INSTANCE_LAUNCH", ""] {
        let (status, body) =
            put_notification_configuration(&router, case, group, &topic, &[unknown]).await;
        assert_eq!(
            status, 400,
            "case {case:?}/{unknown:?}: a type outside the catalog is refused, got {status}: {body}"
        );
    }

    // The documented ceiling on the topics ONE group may notify, and the topic that stays
    // inside it.
    let limit = autoscaling::state::MAX_NOTIFICATION_TOPICS_PER_GROUP;
    for index in 0..limit {
        let topic_arn = format!(
            "arn:aws:sns:{REGION}:{}:limit-topic-{index}",
            engine::emulator_account_id()
        );
        let (status, body) =
            put_notification_configuration(&router, case, group, &topic_arn, &[TEST_NOTIFICATION])
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: topic {index} of {limit} is inside the limit, got {status}: {body}"
        );
    }
    // Re-configuring one of them is not a new topic, so it stays inside the limit.
    let held = format!(
        "arn:aws:sns:{REGION}:{}:limit-topic-0",
        engine::emulator_account_id()
    );
    let (status, body) =
        put_notification_configuration(&router, case, group, &held, &[INSTANCE_LAUNCH]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: re-configuring a held topic is no new topic, got {status}: {body}"
    );
    let one_too_many = format!(
        "arn:aws:sns:{REGION}:{}:limit-topic-{limit}",
        engine::emulator_account_id()
    );
    let (status, body) =
        put_notification_configuration(&router, case, group, &one_too_many, &[TEST_NOTIFICATION])
            .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the topic past the limit is refused, got {status}: {body}"
    );
    assert!(
        body.contains("LimitExceeded"),
        "case {case:?}: the refusal past the limit is the modelled limit error, got {body}"
    );
}
