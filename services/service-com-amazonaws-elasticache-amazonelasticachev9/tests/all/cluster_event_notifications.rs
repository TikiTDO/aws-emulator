// SPDX-License-Identifier: CC0-1.0

//! The NOTIFICATION a cache cluster's events are delivered as, end to end with
//! Amazon SNS mounted beside ElastiCache on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so every assertion is made where a
//! client sees it: on the topic the cluster's notification configuration names.
//!
//! The RULE asserted here: `NotificationConfiguration` is "a notification topic
//! and its status. Notification topics are used for publishing ElastiCache
//! events to subscribers using Amazon Simple Notification Service (SNS)", and
//! `NotificationTopicStatus` is "the status of the Amazon SNS notification topic
//! … notifications are sent only if the status is `active`. Valid values: active
//! | inactive". So while a cluster's configuration is ACTIVE, every cluster event
//! this service records is also published to that topic — as the published
//! `ElastiCache:<Event>` name of the occurrence keyed to the cluster it happened
//! to — and a configuration moved to `inactive` keeps its `TopicArn`, reports
//! that status, and is published nothing until it is moved back.
//!
//! The tables span the class rather than one recorded request: the first varies
//! the OPERATION that makes the event happen (the completion of a provisioning, a
//! reboot, a node addition, a node removal, a scaling, a snapshot, and the
//! changes applied at the call itself, plus a delete), the second varies the
//! notification configuration's STATUS against one event, and the third the
//! SPELLING a status may be asked for in.

use serde_json::Value;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;
use service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;

use crate::cache_cluster_lifecycle::{create_cluster, describe_cluster};
use crate::harness::{params, Emulator, Reply, PAST_TRANSITION, REGION};

/// The cache node type every cluster below is created with, and the one a
/// scaling case moves a cluster ONTO — both from the catalogue
/// `ListAllowedNodeTypeModifications` publishes.
const NODE_TYPE: &str = "cache.t3.micro";
/// See [`NODE_TYPE`].
const LARGER_NODE_TYPE: &str = "cache.t3.small";

/// How long the one case that waits on the DELIVERIES alone polls for, as a
/// number of polls of [`WAIT_POLL_INTERVAL`] — far longer than the cluster span
/// this service publishes, so the wait is bounded without being tight.
const WAIT_POLLS: usize = 60;
/// See [`WAIT_POLLS`].
const WAIT_POLL_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(100);

/// The topic `name`, in [`REGION`], with one confirmed subscription on it so a
/// publish onto it is observable: Amazon SNS "delivers the message to each
/// endpoint that is subscribed to the topic", and a topic with no subscription
/// holds no delivery to read back.
async fn given_topic(emulator: &Emulator, name: &str) -> String {
    let client = emulator.sns(REGION);
    let created = client
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {name}: CreateTopic failed: {error:?}"));
    let arn = created.topic_arn().unwrap_or_default().to_string();
    let queue = format!(
        "arn:aws:sqs:{REGION}:{}:{name}-inbox",
        ::engine::emulator_account_id()
    );
    client
        .subscribe()
        .topic_arn(&arn)
        .protocol("sqs")
        .endpoint(queue)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {name}: Subscribe failed: {error:?}"));
    arn
}

/// Every document Amazon SNS has been handed for `topic_arn`, in the order it
/// fanned them out — read out of the store of the service that OWNS topics and
/// their deliveries, so nothing here can pass on a publish ElastiCache never
/// made.
fn documents_on(emulator: &Emulator, topic_arn: &str) -> Vec<Value> {
    let state = emulator
        .engine()
        .state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>();
    let store = state.lock();
    let mut found = Vec::new();
    for messages in store.delivered.values() {
        for published in messages {
            if published.topic_arn != topic_arn {
                continue;
            }
            let parsed =
                serde_json::from_str::<Value>(&published.message).unwrap_or_else(|error| {
                    panic!(
                        "a published notification is not JSON ({error}): {:?}",
                        published.message
                    )
                });
            found.push(parsed);
        }
    }
    found
}

/// The published event names delivered on `topic_arn`, each paired with the
/// cluster the notification names.
fn events_on(emulator: &Emulator, topic_arn: &str) -> Vec<(String, String)> {
    documents_on(emulator, topic_arn)
        .into_iter()
        .flat_map(|document| {
            let object = document
                .as_object()
                .unwrap_or_else(|| panic!("a notification must be a JSON object, got {document}"))
                .clone();
            object
                .into_iter()
                .map(|(event, named)| {
                    let cluster = named
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("a notification must name the cluster as text, got {named}")
                        })
                        .to_string();
                    (event, cluster)
                })
                .collect::<Vec<(String, String)>>()
        })
        .collect()
}

/// Assert one `awsQuery` call was served.
fn assert_served(case: &str, action: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected {action} served, got {} with body {}",
        reply.status, reply.body
    );
}

/// Assert `event` was delivered to `topic_arn` naming `cluster`, and nothing
/// else about the deliveries.
fn assert_delivered(case: &str, emulator: &Emulator, topic_arn: &str, event: &str, cluster: &str) {
    let delivered = events_on(emulator, topic_arn);
    assert!(
        delivered
            .iter()
            .any(|(named, about)| named == event && about == cluster),
        "case {case}: expected {event} for {cluster} on the cluster's topic, got {delivered:?}"
    );
}

/// Assert NO notification naming `event` reached `topic_arn`.
fn assert_not_delivered(case: &str, emulator: &Emulator, topic_arn: &str, event: &str) {
    let delivered = events_on(emulator, topic_arn);
    assert!(
        !delivered.iter().any(|(named, _)| named == event),
        "case {case}: expected no {event} notification, got {delivered:?}"
    );
}

/// What a row of the delivery table does to the cluster once it is available.
#[derive(Debug, Clone, Copy)]
enum Work {
    /// Nothing beyond the create, whose completion every row asserts.
    None,
    /// Reboot the cluster's first cache node.
    Reboot,
    /// Modify the cluster to `nodes` cache nodes, removing `remove` if it names
    /// one.
    Resize {
        nodes: &'static str,
        remove: Option<&'static str>,
    },
    /// Move the cluster onto another cache node type.
    Scale,
    /// Take a snapshot of the cluster.
    Snapshot,
    /// Attach the cluster to another VPC security group.
    SecurityGroups,
    /// Associate the cluster with another cache parameter group of its family.
    ParameterGroup { family: &'static str },
    /// Delete the cluster.
    Delete,
}

/// One row of the delivery table: a cluster, the work done on it, and the
/// published event that work owes the cluster's topic.
struct DeliveryCase {
    case: &'static str,
    engine: &'static str,
    /// The `NumCacheNodes` the cluster is created with.
    nodes: &'static str,
    work: Work,
    expected: &'static str,
}

/// Do `work` to the cluster `id`, asserting every call it makes was served.
async fn perform(emulator: &Emulator, case: &str, id: &str, work: Work) {
    match work {
        Work::None => {}
        Work::Reboot => {
            let rebooted = emulator
                .call(
                    "RebootCacheCluster",
                    &params(&[
                        ("CacheClusterId", id),
                        ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
                    ]),
                )
                .await;
            assert_served(case, "RebootCacheCluster", &rebooted);
        }
        Work::Resize { nodes, remove } => {
            let mut pairs = params(&[
                ("CacheClusterId", id),
                ("NumCacheNodes", nodes),
                ("ApplyImmediately", "true"),
            ]);
            if let Some(node) = remove {
                pairs.extend(params(&[("CacheNodeIdsToRemove.CacheNodeId.1", node)]));
            }
            let resized = emulator.call("ModifyCacheCluster", &pairs).await;
            assert_served(case, "ModifyCacheCluster", &resized);
        }
        Work::Scale => {
            let scaled = emulator
                .call(
                    "ModifyCacheCluster",
                    &params(&[
                        ("CacheClusterId", id),
                        ("CacheNodeType", LARGER_NODE_TYPE),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_served(case, "ModifyCacheCluster", &scaled);
        }
        Work::Snapshot => {
            let name = format!("{id}-snapshot");
            let taken = emulator
                .call(
                    "CreateSnapshot",
                    &params(&[("CacheClusterId", id), ("SnapshotName", &name)]),
                )
                .await;
            assert_served(case, "CreateSnapshot", &taken);
        }
        Work::SecurityGroups => {
            let group = emulator.security_group_named(&format!("{id}-group")).await;
            let attached = emulator
                .call(
                    "ModifyCacheCluster",
                    &params(&[
                        ("CacheClusterId", id),
                        ("SecurityGroupIds.SecurityGroupId.1", &group),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_served(case, "ModifyCacheCluster", &attached);
        }
        Work::ParameterGroup { family } => {
            let name = format!("{id}-parameters");
            let created = emulator
                .call(
                    "CreateCacheParameterGroup",
                    &params(&[
                        ("CacheParameterGroupName", &name),
                        ("CacheParameterGroupFamily", family),
                        ("Description", "the group this cluster is moved onto"),
                    ]),
                )
                .await;
            assert_served(case, "CreateCacheParameterGroup", &created);
            let associated = emulator
                .call(
                    "ModifyCacheCluster",
                    &params(&[
                        ("CacheClusterId", id),
                        ("CacheParameterGroupName", &name),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_served(case, "ModifyCacheCluster", &associated);
        }
        Work::Delete => {
            let deleted = emulator
                .call("DeleteCacheCluster", &params(&[("CacheClusterId", id)]))
                .await;
            assert_served(case, "DeleteCacheCluster", &deleted);
        }
    }
}

/// Every cluster event of a cluster whose notification configuration is active is
/// published to its topic, whichever operation made it happen.
///
/// Each row also asserts the completion of the cluster's own PROVISIONING, which
/// every row's create owes: that is the event a client waiting on
/// `CacheClusterStatus` is told about.
#[tokio::test]
async fn every_cluster_event_is_published_to_the_clusters_topic() {
    assert!(
        PAST_TRANSITION > state::CLUSTER_TRANSITION,
        "this suite waits {PAST_TRANSITION:?}, which must out-last the cluster span ({:?}) this \
         service publishes",
        state::CLUSTER_TRANSITION
    );
    let rows: [DeliveryCase; 8] = [
        DeliveryCase {
            case: "the provisioning alone",
            engine: "redis",
            nodes: "1",
            work: Work::None,
            expected: "ElastiCache:CacheClusterProvisioningComplete",
        },
        DeliveryCase {
            case: "a reboot",
            engine: "redis",
            nodes: "1",
            work: Work::Reboot,
            expected: "ElastiCache:CacheNodesRebooted",
        },
        DeliveryCase {
            case: "a node added",
            engine: "memcached",
            nodes: "1",
            work: Work::Resize {
                nodes: "2",
                remove: None,
            },
            expected: "ElastiCache:AddCacheNodeComplete",
        },
        DeliveryCase {
            case: "a node removed",
            engine: "memcached",
            nodes: "2",
            work: Work::Resize {
                nodes: "1",
                remove: Some("0002"),
            },
            expected: "ElastiCache:RemoveCacheNodeComplete",
        },
        DeliveryCase {
            case: "a scaling",
            engine: "redis",
            nodes: "1",
            work: Work::Scale,
            expected: "ElastiCache:CacheClusterScalingComplete",
        },
        DeliveryCase {
            case: "a snapshot",
            engine: "redis",
            nodes: "1",
            work: Work::Snapshot,
            expected: "ElastiCache:SnapshotComplete",
        },
        DeliveryCase {
            case: "the security groups changed",
            engine: "redis",
            nodes: "1",
            work: Work::SecurityGroups,
            expected: "ElastiCache:CacheClusterSecurityGroupModified",
        },
        DeliveryCase {
            case: "a delete",
            engine: "redis",
            nodes: "1",
            work: Work::Delete,
            expected: "ElastiCache:DeleteCacheClusterComplete",
        },
    ];

    let emulator = Emulator::with_sns();
    for (index, row) in rows.iter().enumerate() {
        let id = format!("notified-{index}");
        let topic = given_topic(&emulator, &format!("{id}-events")).await;
        let created = create_cluster(
            &emulator,
            &id,
            &[
                ("Engine", row.engine),
                ("CacheNodeType", NODE_TYPE),
                ("NumCacheNodes", row.nodes),
                ("NotificationTopicArn", &topic),
            ],
        )
        .await;
        assert_served(row.case, "CreateCacheCluster", &created);

        // The clock owes the cluster `available`; the call that materialises
        // that completion is also the call that delivers the notification it
        // raises.
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_cluster(&emulator, &id).await;
        assert_served(row.case, "DescribeCacheClusters", &described);
        assert_delivered(
            row.case,
            &emulator,
            &topic,
            "ElastiCache:CacheClusterProvisioningComplete",
            &id,
        );

        perform(&emulator, row.case, &id, row.work).await;
        // A completion the clock owes needs the span to pass and a later call to
        // materialise it; a change applied at the call has already been
        // delivered, and this reads the cluster back either way.
        std::thread::sleep(PAST_TRANSITION);
        let events = emulator
            .call(
                "DescribeEvents",
                &params(&[("SourceIdentifier", &id), ("SourceType", "cache-cluster")]),
            )
            .await;
        assert_served(row.case, "DescribeEvents", &events);
        assert_delivered(row.case, &emulator, &topic, row.expected, &id);
    }
}

/// A cluster configured with a topic in one of the two parameter-group families
/// of the engine it runs: the parameter-group row of the delivery table needs a
/// family, which is the engine's own.
#[tokio::test]
async fn a_parameter_group_change_is_published_to_the_clusters_topic() {
    // (case, engine, the family a group of that engine is created in)
    let rows: [(&str, &str, &str); 2] = [
        ("a redis cluster", "redis", "redis7"),
        ("a memcached cluster", "memcached", "memcached1.6"),
    ];
    let emulator = Emulator::with_sns();
    for (index, &(case, engine, family)) in rows.iter().enumerate() {
        let id = format!("parameters-{index}");
        let topic = given_topic(&emulator, &format!("{id}-events")).await;
        let created = create_cluster(
            &emulator,
            &id,
            &[
                ("Engine", engine),
                ("CacheNodeType", NODE_TYPE),
                ("NotificationTopicArn", &topic),
            ],
        )
        .await;
        assert_served(case, "CreateCacheCluster", &created);
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_cluster(&emulator, &id).await;
        assert_served(case, "DescribeCacheClusters", &described);

        perform(&emulator, case, &id, Work::ParameterGroup { family }).await;
        assert_delivered(
            case,
            &emulator,
            &topic,
            "ElastiCache:CacheClusterParametersChanged",
            &id,
        );
    }
}

/// The STATUS of the notification configuration is what decides delivery, and a
/// status change never detaches the topic: the configuration reports the same
/// `TopicArn` throughout, with the status asked for, and delivery resumes on that
/// same topic.
#[tokio::test]
async fn the_topic_status_gates_delivery_without_detaching_the_topic() {
    // (case, the status asked for, what the configuration then reports, whether a
    // reboot's completion is delivered)
    let rows: [(&str, Option<&str>, &str, bool); 4] = [
        ("as created", None, "active", true),
        ("paused", Some("inactive"), "inactive", false),
        ("paused again", Some("inactive"), "inactive", false),
        ("resumed", Some("active"), "active", true),
    ];
    let emulator = Emulator::with_sns();
    let id = "gated";
    let topic = given_topic(&emulator, "gated-events").await;
    let created = create_cluster(
        &emulator,
        id,
        &[
            ("Engine", "redis"),
            ("CacheNodeType", NODE_TYPE),
            ("NotificationTopicArn", &topic),
        ],
    )
    .await;
    assert_served("setup", "CreateCacheCluster", &created);
    std::thread::sleep(PAST_TRANSITION);

    for (round, &(case, status, reported, delivered)) in rows.iter().enumerate() {
        if let Some(status) = status {
            let modified = emulator
                .call(
                    "ModifyCacheCluster",
                    &params(&[
                        ("CacheClusterId", id),
                        ("NotificationTopicStatus", status),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_served(case, "ModifyCacheCluster", &modified);
        }
        // The topic BINDING survives every status change, and the status reads
        // back as the one asked for.
        let described = describe_cluster(&emulator, id).await;
        assert_served(case, "DescribeCacheClusters", &described);
        for (member, expected) in [
            ("NotificationConfiguration/TopicArn", topic.as_str()),
            ("NotificationConfiguration/TopicStatus", reported),
        ] {
            assert_eq!(
                described.text(member),
                Some(expected),
                "case {case}: expected {member} to report {expected:?}, got {:?} in {}",
                described.text(member),
                described.body
            );
        }

        // One cluster event per round, with the same topic configured: the
        // status alone decides whether it is delivered.
        let before = events_on(&emulator, &topic).len();
        perform(&emulator, case, id, Work::Reboot).await;
        std::thread::sleep(PAST_TRANSITION);
        let settled = describe_cluster(&emulator, id).await;
        assert_served(case, "DescribeCacheClusters", &settled);
        let after = events_on(&emulator, &topic);
        if delivered {
            assert!(
                after.len() > before,
                "case {case}: expected round {round}'s reboot delivered, got {after:?}"
            );
            assert_delivered(
                case,
                &emulator,
                &topic,
                "ElastiCache:CacheNodesRebooted",
                id,
            );
        } else {
            assert_eq!(
                after.len(),
                before,
                "case {case}: expected no delivery while the configuration is inactive, got \
                 {after:?}"
            );
        }
        // The event itself is recorded whatever the status says: pausing
        // notifications pauses the DELIVERY, not the log.
        let events = emulator
            .call(
                "DescribeEvents",
                &params(&[("SourceIdentifier", id), ("SourceType", "cache-cluster")]),
            )
            .await;
        assert_served(case, "DescribeEvents", &events);
        assert!(
            events
                .all("Event/Message")
                .iter()
                .any(|message| message.contains("rebooting")),
            "case {case}: expected the reboot recorded in the event log, got {}",
            events.body
        );
    }
}

/// A cluster with NO notification configuration is published nothing, and reports
/// no configuration at all.
#[tokio::test]
async fn a_cluster_with_no_configured_topic_is_published_nothing() {
    let emulator = Emulator::with_sns();
    // The topic exists and has a subscriber; what it has not been given is a
    // cluster that names it.
    let topic = given_topic(&emulator, "unconfigured-events").await;
    let id = "unconfigured";
    let created = create_cluster(
        &emulator,
        id,
        &[("Engine", "redis"), ("CacheNodeType", NODE_TYPE)],
    )
    .await;
    assert_served("no topic", "CreateCacheCluster", &created);
    std::thread::sleep(PAST_TRANSITION);
    perform(&emulator, "no topic", id, Work::Reboot).await;
    std::thread::sleep(PAST_TRANSITION);
    let described = describe_cluster(&emulator, id).await;
    assert_served("no topic", "DescribeCacheClusters", &described);
    assert!(
        described
            .all("NotificationConfiguration/TopicArn")
            .is_empty(),
        "case no topic: expected no NotificationConfiguration at all, got {}",
        described.body
    );
    for event in [
        "ElastiCache:CacheClusterProvisioningComplete",
        "ElastiCache:CacheNodesRebooted",
    ] {
        assert_not_delivered("no topic", &emulator, &topic, event);
    }
}

/// An SQS subscriber of the cluster's topic receives the SNS notification
/// ENVELOPE for each cluster event — the delivery a client actually reads, end to
/// end: ElastiCache publishes, Amazon SNS fans out to the subscribed queue, and
/// the message sits on the queue.
///
/// The envelope's identifying members are Amazon SNS's own (`Type`, `TopicArn`,
/// `Message`), and the body inside it is this service's: the published
/// `ElastiCache:<Event>` name keyed to the cluster.
#[tokio::test]
async fn an_sqs_subscriber_receives_the_notification_envelope() {
    let emulator = Emulator::with_sns();
    let id = "subscribed";
    let (topic, queue_url) = given_subscribed_queue(&emulator, id).await;

    let created = create_cluster(
        &emulator,
        id,
        &[
            ("Engine", "redis"),
            ("CacheNodeType", NODE_TYPE),
            ("NotificationTopicArn", &topic),
        ],
    )
    .await;
    assert_served("subscriber", "CreateCacheCluster", &created);
    std::thread::sleep(PAST_TRANSITION);
    let described = describe_cluster(&emulator, id).await;
    assert_served("subscriber", "DescribeCacheClusters", &described);

    perform(&emulator, "subscriber", id, Work::Reboot).await;
    std::thread::sleep(PAST_TRANSITION);
    let settled = describe_cluster(&emulator, id).await;
    assert_served("subscriber", "DescribeCacheClusters", &settled);

    let received = received_events(&emulator, &queue_url, &topic).await;
    // Both completions of this scenario, each as the published event name keyed
    // to the cluster it happened to.
    for event in [
        "ElastiCache:CacheClusterProvisioningComplete",
        "ElastiCache:CacheNodesRebooted",
    ] {
        assert!(
            received
                .iter()
                .any(|(named, about)| named == event && about == id),
            "case {event}: expected the subscribed queue to hold the notification for \
             {id}, got {received:?}"
        );
    }
}

/// A queue subscribed to a fresh topic, with the queue policy that lets Amazon
/// SNS deliver to it: "you must ensure that the Amazon SQS queue policy allows
/// the Amazon SNS topic to send messages to the queue".
///
/// Answers `(the topic's ARN, the queue's URL)`.
async fn given_subscribed_queue(emulator: &Emulator, label: &str) -> (String, String) {
    let sqs = emulator.sqs(REGION);
    let created = sqs
        .create_queue()
        .queue_name(format!("{label}-inbox"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {label}: CreateQueue failed: {error:?}"));
    let queue_url = created
        .queue_url()
        .unwrap_or_else(|| panic!("setup {label}: CreateQueue reported no queue URL"))
        .to_string();
    let attributes = sqs
        .get_queue_attributes()
        .queue_url(&queue_url)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::QueueArn)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {label}: GetQueueAttributes failed: {error:?}"));
    let queue_arn = attributes
        .attributes()
        .and_then(|held| held.get(&::aws_sdk_sqs::types::QueueAttributeName::QueueArn))
        .unwrap_or_else(|| panic!("setup {label}: the queue reported no ARN"))
        .to_string();

    let sns = emulator.sns(REGION);
    let topic = sns
        .create_topic()
        .name(format!("{label}-events"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {label}: CreateTopic failed: {error:?}"))
        .topic_arn()
        .unwrap_or_else(|| panic!("setup {label}: CreateTopic reported no ARN"))
        .to_string();
    let policy = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic } },
        }],
    })
    .to_string();
    sqs.set_queue_attributes()
        .queue_url(&queue_url)
        .attributes(::aws_sdk_sqs::types::QueueAttributeName::Policy, policy)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {label}: SetQueueAttributes failed: {error:?}"));
    sns.subscribe()
        .topic_arn(&topic)
        .protocol("sqs")
        .endpoint(&queue_arn)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {label}: Subscribe failed: {error:?}"));
    (topic, queue_url)
}

/// The published events sitting on the queue at `queue_url`, each paired with the
/// cluster its notification names — read off the SNS ENVELOPE each message
/// carries, whose `Type` and `TopicArn` are asserted here because a client
/// branches on them.
async fn received_events(
    emulator: &Emulator,
    queue_url: &str,
    topic_arn: &str,
) -> Vec<(String, String)> {
    let received = emulator
        .sqs(REGION)
        .receive_message()
        .queue_url(queue_url)
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap_or_else(|error| panic!("ReceiveMessage failed: {error:?}"));
    let mut found = Vec::new();
    for message in received.messages() {
        let body = message.body().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("a delivered message is not JSON ({error}): {body:?}"));
        assert_eq!(
            envelope.get("Type").and_then(Value::as_str),
            Some("Notification"),
            "expected an SNS Notification envelope, got {envelope}"
        );
        assert_eq!(
            envelope.get("TopicArn").and_then(Value::as_str),
            Some(topic_arn),
            "expected the envelope to name the configured topic, got {envelope}"
        );
        let inner = envelope
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected the envelope to carry a Message, got {envelope}"));
        let document: Value = serde_json::from_str(inner).unwrap_or_else(|error| {
            panic!("the notification body is not JSON ({error}): {inner:?}")
        });
        for (event, named) in document
            .as_object()
            .unwrap_or_else(|| panic!("the notification body must be an object, got {document}"))
        {
            found.push((
                event.clone(),
                named.as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    found
}

/// A client that waits on its SUBSCRIBER rather than on ElastiCache is still
/// told: the completion the clock owes a configured cluster is published when it
/// falls due, with no further ElastiCache call to materialise it.
#[tokio::test]
async fn a_completion_is_published_without_another_elasticache_call() {
    let emulator = Emulator::with_sns();
    let id = "unpolled";
    let topic = given_topic(&emulator, "unpolled-events").await;
    let created = create_cluster(
        &emulator,
        id,
        &[
            ("Engine", "redis"),
            ("CacheNodeType", NODE_TYPE),
            ("NotificationTopicArn", &topic),
        ],
    )
    .await;
    assert_served("unpolled", "CreateCacheCluster", &created);
    // Nothing but the clock and the topic: no describe, no poll of this service.
    await_event(
        &emulator,
        &topic,
        "ElastiCache:CacheClusterProvisioningComplete",
        id,
    )
    .await;

    // And the same for a completion a later call starts: the reboot answers
    // while the nodes are still rebooting, and the notification arrives on its
    // own.
    perform(&emulator, "unpolled", id, Work::Reboot).await;
    await_event(&emulator, &topic, "ElastiCache:CacheNodesRebooted", id).await;
}

/// Wait for `event` to be delivered to `topic_arn` for `cluster`, polling only
/// the DELIVERIES — nothing of the service under test — so what the clock owes is
/// published by the service rather than by the poll.
async fn await_event(emulator: &Emulator, topic_arn: &str, event: &str, cluster: &str) {
    for _ in 0..WAIT_POLLS {
        if events_on(emulator, topic_arn)
            .iter()
            .any(|(named, about)| named == event && about == cluster)
        {
            return;
        }
        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
    panic!(
        "case {event}: expected the notification for {cluster} within {:?}, got {:?}",
        WAIT_POLL_INTERVAL * u32::try_from(WAIT_POLLS).unwrap_or(1),
        events_on(emulator, topic_arn)
    );
}

/// `NotificationTopicStatus`'s "valid values: active | inactive" are the only
/// spellings a modify accepts, and a value outside them is refused as the bad
/// parameter value it is — leaving the stored configuration alone.
#[tokio::test]
async fn a_notification_topic_status_outside_the_documented_values_is_refused() {
    // (case, the status asked for, whether it is one of the documented values)
    let rows: [(&str, &str, bool); 6] = [
        ("active", "active", true),
        ("inactive", "inactive", true),
        ("the wrong case", "ACTIVE", false),
        ("a status of another resource", "available", false),
        ("a sentence", "turn it off", false),
        ("nothing at all", "", false),
    ];
    let emulator = Emulator::with_sns();
    let id = "status-spelling";
    let topic = given_topic(&emulator, "status-spelling-events").await;
    let created = create_cluster(
        &emulator,
        id,
        &[
            ("Engine", "redis"),
            ("CacheNodeType", NODE_TYPE),
            ("NotificationTopicArn", &topic),
        ],
    )
    .await;
    assert_served("setup", "CreateCacheCluster", &created);
    std::thread::sleep(PAST_TRANSITION);

    for (case, status, accepted) in rows {
        let modified = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", id),
                    ("NotificationTopicStatus", status),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        if accepted {
            assert_served(case, "ModifyCacheCluster", &modified);
            continue;
        }
        assert_eq!(
            modified.status, 400,
            "case {case}: expected {status:?} refused 400, got {} with body {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.error_code(),
            "InvalidParameterValue",
            "case {case}: expected the parameter-value code, got {}",
            modified.body
        );
        assert!(
            modified.error_message().contains("NotificationTopicStatus"),
            "case {case}: expected the refusal to name the member, got {:?}",
            modified.error_message()
        );
        // The refused call wrote nothing: the configuration is the one the
        // cluster already had.
        let described = describe_cluster(&emulator, id).await;
        assert_eq!(
            described.text("NotificationConfiguration/TopicArn"),
            Some(topic.as_str()),
            "case {case}: expected the stored topic untouched, got {}",
            described.body
        );
    }
}
