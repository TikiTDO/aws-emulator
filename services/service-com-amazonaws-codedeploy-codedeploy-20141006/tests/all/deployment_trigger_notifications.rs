// SPDX-License-Identifier: CC0-1.0

//! What a deployment group's `triggerConfigurations` DO: the Amazon SNS
//! notifications a deployment's lifecycle events publish, read where a client
//! reads them — off the Amazon SQS queue subscribed to the topic the trigger
//! names.
//!
//! The RULE asserted here: a trigger configuration names "the Amazon Resource
//! Name (ARN) of the Amazon Simple Notification Service topic through which
//! notifications about deployment or instance events are sent" and "the event type
//! or types for which notifications are triggered"
//! (`smithy_query(query="shape",shape="TriggerConfig")`), so every lifecycle event
//! a deployment REACHES is published once, to each trigger of its deployment group
//! whose `triggerEvents` name that event and to no other topic, carrying the
//! deployment it is about and the CONFIGURATION's own `triggerName` as
//! `eventTriggerName`; and the write that stores a trigger list publishes the
//! one-time verification notification that tells each named topic the trigger was
//! configured on it.
//!
//! The rows span what the rule generalises over: which lifecycle event the
//! deployment reaches (its start, success, failure, the stop a caller asks for and
//! the rollback that stop creates), which events a trigger selects (one, several,
//! none, and one the deployment never reaches), how many triggers and topics a
//! group carries, and whether the `triggerTargetArn` names an Amazon SNS topic at
//! all. Every case drives both halves: the topic configured for the event that
//! happened, and the topic configured for an event that did not.
//!
//! Everything goes through the front door a client uses: CodeDeploy's awsJson
//! envelope, SNS's `awsQuery` form and SQS's `awsJson1_0` target, all on the one
//! shared engine the production router mounts them on.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{s3_revision, terminal_deployment, xml_field, Emulator};

/// The Region every call below is signed for, which is therefore the Region the
/// deployments run in and the Region the topics live in.
const REGION: &str = "us-east-1";

/// The service role every deployment group here is created with; CodeDeploy only
/// stores it.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// How long a case waits for a notification the clock's own transitions owe, and
/// how often it looks. Generous: the publish follows the transition that owed it,
/// and nothing here asserts on how long either took.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);
/// See [`DELIVERY_DEADLINE`].
const DELIVERY_INTERVAL: Duration = Duration::from_millis(25);

/// One SNS `awsQuery` call that must succeed, answering its XML body.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = emulator.sns_query(REGION, action, params).await;
    assert_eq!(
        status, 200,
        "setup: SNS {action} must succeed, got {status} with {body}"
    );
    body
}

/// One SQS call that must succeed, answering its parsed body.
async fn sqs(emulator: &Emulator, operation: &str, body: Value) -> Value {
    let (status, parsed) = emulator.sqs_json(REGION, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: SQS {operation} must succeed, got {status} with {parsed:?}"
    );
    parsed
}

/// The queue policy AWS documents for a queue subscribed to a topic: the
/// `sns.amazonaws.com` service principal, allowed `sqs:SendMessage` on that queue.
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

/// A topic named `name` with one SQS queue subscribed to it: the topic's ARN (what
/// a `triggerTargetArn` names) and the queue's URL (where a notification is read).
async fn given_topic(emulator: &Emulator, name: &str) -> (String, String) {
    let created = sqs(
        emulator,
        "CreateQueue",
        json!({"QueueName": format!("{name}-inbox")}),
    )
    .await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue({name}) answered no QueueUrl: {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the queue {name} reported no QueueArn: {attributes}"))
        .to_string();
    sqs(
        emulator,
        "SetQueueAttributes",
        json!({"QueueUrl": queue_url, "Attributes": {"Policy": grant_to_sns(&queue_arn)}}),
    )
    .await;

    let topic = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = xml_field(&topic, "TopicArn")
        .unwrap_or_else(|| panic!("setup: CreateTopic({name}) answered no TopicArn: {topic}"));
    sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn.as_str()),
        ],
    )
    .await;
    (topic_arn, queue_url)
}

/// What one queue has received: the JSON notification documents, and the bodies of
/// the notifications that are not JSON documents — the verification notification
/// is the one that is not.
#[derive(Debug, Default)]
struct Inbox {
    documents: Vec<Value>,
    plain: Vec<String>,
}

/// Receive everything waiting on the queue at `url` and fold it into `inbox`.
///
/// A received message stays invisible for the queue's visibility timeout, so
/// repeated calls accumulate rather than re-read.
async fn drain_into(emulator: &Emulator, url: &str, inbox: &mut Inbox) {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    for message in received["Messages"].as_array().cloned().unwrap_or_default() {
        let body = message["Body"].as_str().unwrap_or_default();
        // The SNS envelope carries the published notification in `Message`.
        let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("a delivered message is not an SNS envelope ({error}): {body}")
        });
        let notification = envelope["Message"].as_str().unwrap_or_default().to_string();
        match serde_json::from_str::<Value>(&notification) {
            Ok(document) if document.is_object() => inbox.documents.push(document),
            _ => inbox.plain.push(notification),
        }
    }
}

/// Poll the queue at `url` until it holds at least `documents` JSON notifications
/// and `plain` notifications that are not, answering everything it received.
async fn awaited_inbox(
    emulator: &Emulator,
    case: &str,
    url: &str,
    documents: usize,
    plain: usize,
) -> Inbox {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut inbox = Inbox::default();
    loop {
        drain_into(emulator, url, &mut inbox).await;
        if inbox.documents.len() >= documents && inbox.plain.len() >= plain {
            return inbox;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {documents} notification document(s) and {plain} plain \
             notification(s) on {url}, got {inbox:?}"
        );
        tokio::time::sleep(DELIVERY_INTERVAL).await;
    }
}

/// `(eventTriggerName, status, deploymentId)` of every document in `inbox`,
/// sorted, so a comparison does not depend on the order SNS fanned the publishes
/// out in.
fn reported(inbox: &Inbox) -> Vec<(String, String, String)> {
    let mut found: Vec<(String, String, String)> = inbox
        .documents
        .iter()
        .map(|document| {
            (
                text(document, "eventTriggerName"),
                text(document, "status"),
                text(document, "deploymentId"),
            )
        })
        .collect();
    found.sort();
    found
}

/// One text member of a notification document, or `<missing>` so a failed
/// assertion names what was there instead.
fn text(document: &Value, member: &str) -> String {
    document[member].as_str().unwrap_or("<missing>").to_string()
}

/// One trigger configuration a row carries: its name, the topic it names (a key
/// into the row's topics, or a literal ARN for the case that names no topic) and
/// the events it selects.
struct Trigger {
    name: &'static str,
    /// `Some(key)` names one of the row's two topics; `None` carries `arn` as the
    /// `triggerTargetArn` verbatim.
    topic: Option<&'static str>,
    arn: &'static str,
    events: &'static [&'static str],
}

/// Create the application and the deployment group of a row, with its triggers
/// resolved against the topics the case created.
async fn given_group(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extras: &Value,
    triggers: &[Trigger],
    topics: &[(&str, String)],
) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let configurations: Vec<Value> = triggers
        .iter()
        .map(|trigger| {
            let target = match trigger.topic {
                Some(key) => topics
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, arn)| arn.clone())
                    .unwrap_or_else(|| panic!("the row names no topic {key:?}")),
                None => trigger.arn.to_string(),
            };
            json!({
                "triggerName": trigger.name,
                "triggerTargetArn": target,
                "triggerEvents": trigger.events,
            })
        })
        .collect();
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": "dg",
        "serviceRoleArn": SERVICE_ROLE,
        "triggerConfigurations": configurations,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
}

/// Create a deployment of the row's group, answering its id.
async fn create_deployment(emulator: &Emulator, application: &str) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": "dg",
                "revision": s3_revision("trigger-bucket", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a deployment's lifecycle event is published to every trigger of its
/// deployment group that selects that event, naming the deployment and the
/// trigger's own `triggerName`, and to no trigger that selects another event.
#[tokio::test]
async fn a_lifecycle_event_is_published_to_the_triggers_that_select_it() {
    /// One row: the group it deploys to, the triggers on it, the terminal status
    /// the deployment reaches, and what each topic must hold.
    struct Row {
        case: &'static str,
        /// The application's compute platform and the group members that decide
        /// the deployment's outcome: a group that resolves at least one target
        /// ends `Succeeded`, one that resolves none ends `Failed` (see
        /// `codedeploy_deployment_outcome`).
        platform: &'static str,
        extras: Value,
        triggers: Vec<Trigger>,
        outcome: &'static str,
        /// Per topic: the `(eventTriggerName, status)` pairs it must hold, and how
        /// many verification notifications it must have received.
        expected_alpha: Vec<(&'static str, &'static str)>,
        expected_beta: Vec<(&'static str, &'static str)>,
        verifications_alpha: usize,
        verifications_beta: usize,
    }

    let rows = vec![
        Row {
            case: "a successful deployment publishes to the success topic only",
            platform: "Lambda",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "on-success",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
                Trigger {
                    name: "on-failure",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentFailure"],
                },
            ],
            outcome: "Succeeded",
            expected_alpha: vec![("on-success", "succeeded")],
            expected_beta: Vec::new(),
            verifications_alpha: 1,
            verifications_beta: 1,
        },
        Row {
            case: "a failed deployment publishes to the failure topic only",
            platform: "Server",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "on-success",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
                Trigger {
                    name: "on-failure",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentFailure"],
                },
            ],
            outcome: "Failed",
            expected_alpha: Vec::new(),
            expected_beta: vec![("on-failure", "failed")],
            verifications_alpha: 1,
            verifications_beta: 1,
        },
        Row {
            case: "one trigger selecting several events publishes each of them",
            platform: "ECS",
            extras: json!({"ecsServices": [{"clusterName": "cluster", "serviceName": "web"}]}),
            triggers: vec![Trigger {
                name: "whole-lifecycle",
                topic: Some("alpha"),
                arn: "",
                events: &["DeploymentStart", "DeploymentSuccess", "DeploymentFailure"],
            }],
            outcome: "Succeeded",
            expected_alpha: vec![
                ("whole-lifecycle", "in_progress"),
                ("whole-lifecycle", "succeeded"),
            ],
            expected_beta: Vec::new(),
            verifications_alpha: 1,
            verifications_beta: 0,
        },
        Row {
            case: "two triggers on one topic each publish under their own name",
            platform: "Lambda",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "pipeline-gate",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
                Trigger {
                    name: "release-notes",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
            ],
            outcome: "Succeeded",
            expected_alpha: vec![
                ("pipeline-gate", "succeeded"),
                ("release-notes", "succeeded"),
            ],
            expected_beta: Vec::new(),
            verifications_alpha: 2,
            verifications_beta: 0,
        },
        Row {
            case: "a trigger selecting events the deployment never reaches publishes none",
            platform: "Lambda",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "on-ready",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentReady", "InstanceSuccess"],
                },
                Trigger {
                    name: "on-success",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
            ],
            outcome: "Succeeded",
            expected_alpha: Vec::new(),
            expected_beta: vec![("on-success", "succeeded")],
            verifications_alpha: 1,
            verifications_beta: 1,
        },
        Row {
            case: "a trigger selecting no event at all publishes none",
            platform: "Lambda",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "no-events",
                    topic: Some("alpha"),
                    arn: "",
                    events: &[],
                },
                Trigger {
                    name: "on-success",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
            ],
            outcome: "Succeeded",
            expected_alpha: Vec::new(),
            expected_beta: vec![("on-success", "succeeded")],
            verifications_alpha: 1,
            verifications_beta: 1,
        },
        Row {
            case: "a trigger whose target is no SNS topic publishes nothing, the rest still do",
            platform: "Lambda",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "not-a-topic",
                    topic: None,
                    arn: "arn:aws:sqs:us-east-1:123456789012:not-a-topic",
                    events: &["DeploymentSuccess"],
                },
                Trigger {
                    name: "on-success",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
            ],
            outcome: "Succeeded",
            expected_alpha: Vec::new(),
            expected_beta: vec![("on-success", "succeeded")],
            verifications_alpha: 0,
            verifications_beta: 1,
        },
        Row {
            case: "the start event is published on its own",
            platform: "Server",
            extras: json!({}),
            triggers: vec![
                Trigger {
                    name: "on-start",
                    topic: Some("alpha"),
                    arn: "",
                    events: &["DeploymentStart"],
                },
                Trigger {
                    name: "on-success",
                    topic: Some("beta"),
                    arn: "",
                    events: &["DeploymentSuccess"],
                },
            ],
            outcome: "Failed",
            expected_alpha: vec![("on-start", "in_progress")],
            expected_beta: Vec::new(),
            verifications_alpha: 1,
            verifications_beta: 1,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::with_sns_and_sqs();
        let (alpha_arn, alpha_queue) = given_topic(&emulator, "alpha").await;
        let (beta_arn, beta_queue) = given_topic(&emulator, "beta").await;
        let topics = [("alpha", alpha_arn), ("beta", beta_arn)];

        let application = "trigger-notifications";
        given_group(
            &emulator,
            application,
            row.platform,
            &row.extras,
            &row.triggers,
            &topics,
        )
        .await;

        // The verification notification each stored trigger owes its topic, at the
        // write that configured it there.
        let verified_alpha =
            awaited_inbox(&emulator, case, &alpha_queue, 0, row.verifications_alpha).await;
        let verified_beta =
            awaited_inbox(&emulator, case, &beta_queue, 0, row.verifications_beta).await;
        for (topic, inbox, expected) in [
            ("alpha", &verified_alpha, row.verifications_alpha),
            ("beta", &verified_beta, row.verifications_beta),
        ] {
            assert_eq!(
                inbox.plain.len(),
                expected,
                "case {case:?}: expected {expected} verification notification(s) on {topic}, \
                 got {inbox:?}"
            );
            assert!(
                inbox.documents.is_empty(),
                "case {case:?}: nothing but the verification notification is owed on {topic} \
                 before a deployment, got {inbox:?}"
            );
            for notification in &inbox.plain {
                assert!(
                    notification.contains("dg"),
                    "case {case:?}: the verification notification on {topic} must name the \
                     deployment group, got {notification:?}"
                );
            }
        }

        let deployment_id = create_deployment(&emulator, application).await;
        let info = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some(row.outcome),
            "case {case:?}: expected the terminal status {}, got {info:?}",
            row.outcome
        );

        let alpha = awaited_inbox(
            &emulator,
            case,
            &alpha_queue,
            row.expected_alpha.len(),
            0,
        )
        .await;
        let beta =
            awaited_inbox(&emulator, case, &beta_queue, row.expected_beta.len(), 0).await;
        for (topic, inbox, expected) in [
            ("alpha", &alpha, &row.expected_alpha),
            ("beta", &beta, &row.expected_beta),
        ] {
            let mut wanted: Vec<(String, String, String)> = expected
                .iter()
                .map(|(trigger, status)| {
                    (
                        (*trigger).to_string(),
                        (*status).to_string(),
                        deployment_id.clone(),
                    )
                })
                .collect();
            wanted.sort();
            assert_eq!(
                reported(inbox),
                wanted,
                "case {case:?}: expected {wanted:?} on {topic}, got {inbox:?}"
            );
            for document in &inbox.documents {
                assert_eq!(
                    (
                        text(document, "applicationName"),
                        text(document, "deploymentGroupName"),
                        text(document, "region"),
                        text(document, "accountId"),
                    ),
                    (
                        application.to_string(),
                        "dg".to_string(),
                        REGION.to_string(),
                        ::engine::emulator_account_id().to_string(),
                    ),
                    "case {case:?}: the notification on {topic} must name the deployment's own \
                     application, group, Region and account, got {document:?}"
                );
            }
        }
    }
}

/// RULE: the stop a caller asks for and the rollback deployment that stop creates
/// are lifecycle events of the stopped deployment like any other — each published
/// to the triggers that select it, and to no other topic.
#[tokio::test]
async fn a_stop_and_its_rollback_are_published_to_the_triggers_that_select_them() {
    let emulator = Emulator::with_sns_and_sqs();
    let (stop_arn, stop_queue) = given_topic(&emulator, "stops").await;
    let (rollback_arn, rollback_queue) = given_topic(&emulator, "rollbacks").await;
    let topics = [("alpha", stop_arn), ("beta", rollback_arn)];
    let case = "a stopped deployment with a rollback";

    // A blue/green group that TERMINATES its original instances holds every
    // deployment to it open for the termination wait it configured, so a
    // deployment of it is still in flight when the stop arrives — no case here
    // depends on how fast the host runs.
    let extras = json!({
        "deploymentStyle": {"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"},
        "blueGreenDeploymentConfiguration": {
            "terminateBlueInstancesOnDeploymentSuccess": {
                "action": "TERMINATE",
                "terminationWaitTimeInMinutes": 60,
            }
        },
        "ec2TagFilters": [{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}],
    });
    let triggers = vec![
        Trigger {
            name: "on-stop",
            topic: Some("alpha"),
            arn: "",
            events: &["DeploymentStop"],
        },
        Trigger {
            name: "on-rollback",
            topic: Some("beta"),
            arn: "",
            events: &["DeploymentRollback"],
        },
    ];
    let application = "trigger-stops";
    given_group(
        &emulator,
        application,
        "Server",
        &extras,
        &triggers,
        &topics,
    )
    .await;
    // The verification notification each topic is owed, received before the
    // deployments so the documents below are the only thing left to arrive.
    awaited_inbox(&emulator, case, &stop_queue, 0, 1).await;
    awaited_inbox(&emulator, case, &rollback_queue, 0, 1).await;

    // A rollback redeploys "the revision deployed before" this deployment, so the
    // group needs an earlier deployment for one to exist at all.
    create_deployment(&emulator, application).await;
    let deployment_id = create_deployment(&emulator, application).await;
    let stopped = emulator
        .ok(
            "StopDeployment",
            json!({"deploymentId": deployment_id, "autoRollbackEnabled": true}),
        )
        .await;
    assert_eq!(
        stopped["status"].as_str(),
        Some("Succeeded"),
        "case {case:?}: the stop must have taken effect, got {stopped:?}"
    );

    for (topic, queue, trigger) in [
        ("stops", &stop_queue, "on-stop"),
        ("rollbacks", &rollback_queue, "on-rollback"),
    ] {
        let inbox = awaited_inbox(&emulator, case, queue, 1, 0).await;
        assert_eq!(
            reported(&inbox),
            vec![(
                trigger.to_string(),
                "stopped".to_string(),
                deployment_id.clone(),
            )],
            "case {case:?}: expected the stopped deployment on {topic} under {trigger}, got \
             {inbox:?}"
        );
    }
}

/// RULE: `UpdateDeploymentGroup` stores the trigger list the request names, so the
/// triggers it stored owe their topics the same one-time verification notification
/// `CreateDeploymentGroup`'s do, and a later deployment publishes to the UPDATED
/// list rather than to the one the group was created with.
#[tokio::test]
async fn an_updated_trigger_list_is_the_one_that_publishes() {
    let emulator = Emulator::with_sns_and_sqs();
    let (created_arn, created_queue) = given_topic(&emulator, "created").await;
    let (updated_arn, updated_queue) = given_topic(&emulator, "updated").await;
    let topics = [("alpha", created_arn), ("beta", updated_arn.clone())];
    let case = "an updated trigger list";

    let application = "trigger-updates";
    given_group(
        &emulator,
        application,
        "Lambda",
        &json!({}),
        &[Trigger {
            name: "on-success",
            topic: Some("alpha"),
            arn: "",
            events: &["DeploymentSuccess"],
        }],
        &topics,
    )
    .await;
    let verified = awaited_inbox(&emulator, case, &created_queue, 0, 1).await;
    assert_eq!(
        verified.plain.len(),
        1,
        "case {case:?}: the created trigger owes its topic one verification notification, got \
         {verified:?}"
    );

    emulator
        .ok(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": "dg",
                "triggerConfigurations": [{
                    "triggerName": "after-update",
                    "triggerTargetArn": updated_arn,
                    "triggerEvents": ["DeploymentSuccess"],
                }],
            }),
        )
        .await;
    let verified = awaited_inbox(&emulator, case, &updated_queue, 0, 1).await;
    assert_eq!(
        verified.plain.len(),
        1,
        "case {case:?}: the trigger the update stored owes its topic one verification \
         notification, got {verified:?}"
    );

    let deployment_id = create_deployment(&emulator, application).await;
    let info = terminal_deployment(&emulator, &deployment_id, case).await;
    assert_eq!(
        info["status"].as_str(),
        Some("Succeeded"),
        "case {case:?}: expected the deployment to succeed, got {info:?}"
    );

    let after_update = awaited_inbox(&emulator, case, &updated_queue, 1, 0).await;
    assert_eq!(
        reported(&after_update),
        vec![(
            "after-update".to_string(),
            "succeeded".to_string(),
            deployment_id.clone(),
        )],
        "case {case:?}: the updated trigger must publish the outcome, got {after_update:?}"
    );
    let replaced = awaited_inbox(&emulator, case, &created_queue, 0, 0).await;
    assert!(
        replaced.documents.is_empty(),
        "case {case:?}: the trigger the update replaced must publish nothing, got {replaced:?}"
    );
}
