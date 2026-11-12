// SPDX-License-Identifier: CC0-1.0

//! The DELIVERY half of a cache cluster's events: the Amazon SNS topic a
//! cluster's notification configuration names, and the publish every cluster
//! event owes that topic while the configuration is active.
//!
//! `NotificationConfiguration` is "a notification topic and its status.
//! Notification topics are used for publishing ElastiCache events to subscribers
//! using Amazon Simple Notification Service (SNS)", and
//! `NotificationTopicStatus` adds that "notifications are sent only if the status
//! is `active`". So a cluster configured with a topic has every event this
//! service records about it published to that topic, and a client subscribed to
//! the topic learns what happened without reading [`super::EventRecord`] through
//! `DescribeEvents`.
//!
//! The delivery is REAL: the publish is an `SNS:Publish` into the emulated Amazon
//! SNS over the in-process callback, so a queue subscribed to the topic receives
//! the notification through Amazon SNS's own fan-out. Nothing here opens a socket
//! or writes to a subscriber directly — the ARN a caller supplied is not a URL
//! this code dials, it is a topic the publish is routed to INSIDE this process,
//! so the topic has to be one this emulator itself holds or Amazon SNS answers
//! that it does not exist.
//!
//! It is also ASYNCHRONOUS, the way the real service's is: the call that records
//! the event queues the notification it owes ([`ClusterNotification`]) and the
//! publishes are made once the store's lock is released, from
//! [`deliver_cluster_notifications`] — which every call this service serves runs
//! (see `crate::service::query_error_envelope`). A completion the CLOCK owes is
//! published when it falls due ([`publish_when_due`]), because the client these
//! notifications exist for waits on its subscriber rather than calling this
//! service again.
//!
//! Split out of [`super`], which holds the clusters and the event log the
//! notifications are about.

use super::*;

/// The service namespace of the topic a notification is published to: the ARN
/// field [`topic_region`] holds a configured topic to, and the endpoint prefix
/// the in-process publish is addressed at.
pub const SNS_ARN_VENDOR: &str = "sns";

/// The first word of every published ElastiCache event name: the names are
/// spelled `ElastiCache:<Event>`, which is the form the service's published
/// event list uses and the key a notification's document carries.
pub const EVENT_PREFIX: &str = "ElastiCache:";

/// The event the completion of a cluster's provisioning raises.
///
/// This and the names below are the ElastiCache event list's own entries for the
/// cluster-level occurrences this emulator materialises; each is spelled with
/// [`EVENT_PREFIX`], and the value one occurrence carries is the cluster the
/// event happened to.
pub const EVENT_CACHE_CLUSTER_PROVISIONING_COMPLETE: &str =
    "ElastiCache:CacheClusterProvisioningComplete";

/// The event the completion of a `RebootCacheCluster` raises: "one or more cache
/// nodes of a cluster have been rebooted".
pub const EVENT_CACHE_NODES_REBOOTED: &str = "ElastiCache:CacheNodesRebooted";

/// The event the completion of a modify that ADDS cache nodes raises.
pub const EVENT_ADD_CACHE_NODE_COMPLETE: &str = "ElastiCache:AddCacheNodeComplete";

/// The event the completion of a modify that REMOVES cache nodes raises.
pub const EVENT_REMOVE_CACHE_NODE_COMPLETE: &str = "ElastiCache:RemoveCacheNodeComplete";

/// The event the completion of a modify that moves the cluster onto another cache
/// node type raises — the scaling of a cluster.
pub const EVENT_CACHE_CLUSTER_SCALING_COMPLETE: &str = "ElastiCache:CacheClusterScalingComplete";

/// The event a change to the parameters a cluster runs raises.
pub const EVENT_CACHE_CLUSTER_PARAMETERS_CHANGED: &str = "ElastiCache:CacheClusterParametersChanged";

/// The event a change to the security groups a cluster is attached to raises.
pub const EVENT_CACHE_CLUSTER_SECURITY_GROUP_MODIFIED: &str =
    "ElastiCache:CacheClusterSecurityGroupModified";

/// The event a cluster's deletion raises.
pub const EVENT_DELETE_CACHE_CLUSTER_COMPLETE: &str = "ElastiCache:DeleteCacheClusterComplete";

/// The event the completion of a snapshot of a cluster raises.
pub const EVENT_SNAPSHOT_COMPLETE: &str = "ElastiCache:SnapshotComplete";

/// How many notifications one store holds waiting to be published.
///
/// A bound on what one client's calls can queue: the queue is drained by every
/// call this service serves, so it is normally only as long as one request's own
/// events. This is the backstop for a store nothing drains, and the OLDEST
/// notification is dropped rather than the newest, so a queue at the bound still
/// delivers what just happened.
const MAX_PENDING_NOTIFICATIONS: usize = 500;

/// How many notifications ONE request publishes. Each publish is an in-process
/// call into Amazon SNS that fans out again to the topic's subscribers, so the
/// work one request can start is capped; what is left stays queued for the next
/// call.
const MAX_NOTIFICATIONS_PER_REQUEST: usize = 50;

/// One notification a recorded cluster event owes, waiting to be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterNotification {
    /// The topic the cluster's `NotificationConfiguration$TopicArn` named.
    pub topic_arn: String,
    /// The document published onto it; see [`notification_document`].
    pub message: String,
}

/// The Region of the Amazon SNS topic `arn` names, or `None` when it names no
/// topic of that service.
///
/// A caller-supplied ARN's Region ends up in the authority of the endpoint the
/// in-process publish is addressed at, so it is read as a Region LABEL through
/// the one place this tree states that grammar
/// (`service_common::arn::region_of`): the only thing an ARN can decide is which
/// Region of this emulator the publish is made in, never what that address is.
pub fn topic_region(arn: &str) -> Option<&str> {
    ::service_common::arn::region_of(arn, SNS_ARN_VENDOR)
}

/// The document a subscriber receives for one cluster event: the published event
/// name keyed to the cluster it happened to.
///
/// The model declares no shape for the notification, so the two parts are the
/// ones the published event list fixes — the `ElastiCache:<Event>` name — and the
/// one the occurrence supplies: the identifier of the cluster the event is about.
pub fn notification_document(event: &str, cache_cluster_id: &str) -> String {
    ::serde_json::json!({ event: cache_cluster_id }).to_string()
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// Record one cache-cluster event: the entry `DescribeEvents` reports, and —
    /// while the cluster's notification configuration is active — the
    /// notification its topic is owed.
    ///
    /// The ONE place a cluster event becomes real, so no path can record an event
    /// a configured topic is never told about. `event` is the published
    /// `ElastiCache:<Event>` name of the occurrence, or `None` for an occurrence
    /// this service records in its log but the published event list names no
    /// event for (the start of a change, as opposed to its completion).
    pub fn record_cluster_event(
        &mut self,
        cache_cluster_id: &str,
        event: Option<&str>,
        message: impl Into<String>,
        at: SystemTime,
    ) {
        self.record_event(SOURCE_TYPE_CACHE_CLUSTER, cache_cluster_id, message, at);
        if let Some(event) = event {
            self.queue_cluster_notification(cache_cluster_id, event);
        }
    }

    /// Queue the notification the cluster `cache_cluster_id`'s topic is owed for
    /// `event`, if that cluster has a topic configured and ACTIVE.
    ///
    /// "Notifications are sent only if the status is `active`": a cluster whose
    /// notification configuration is `inactive` keeps the binding and is told
    /// nothing, and a cluster with no configuration has nowhere to be told.
    pub fn queue_cluster_notification(&mut self, cache_cluster_id: &str, event: &str) {
        let Some(cluster) = self.cache_clusters.get(cache_cluster_id) else {
            return;
        };
        if cluster.notification_topic_status != NOTIFICATION_STATUS_ACTIVE {
            return;
        }
        let Some(topic_arn) = cluster.notification_topic_arn.clone() else {
            return;
        };
        // A value that is not the ARN of an SNS topic in a Region names nothing
        // this emulator can publish to, so it is skipped here rather than
        // attempted.
        if topic_region(&topic_arn).is_none() {
            return;
        }
        let message = notification_document(event, &cluster.cache_cluster_id);
        self.push_cluster_notification(ClusterNotification { topic_arn, message });
    }

    /// Queue `notification`, holding the queue to [`MAX_PENDING_NOTIFICATIONS`].
    fn push_cluster_notification(&mut self, notification: ClusterNotification) {
        if self.pending_cluster_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            // The oldest is dropped, so a queue at its bound still carries the
            // event that just happened.
            ::tracing::warn!(
                topic = %notification.topic_arn,
                "this store holds as many unpublished cluster notifications as it keeps, so the \
                 oldest is dropped",
            );
            self.pending_cluster_notifications.remove(0);
        }
        self.pending_cluster_notifications.push(notification);
    }

    /// Take at most `limit` notifications off the queue, oldest first.
    ///
    /// Taken, not read: a notification this hands back is gone from the queue, so
    /// no later call publishes it a second time.
    fn take_pending_notifications(&mut self, limit: usize) -> Vec<ClusterNotification> {
        let taken = limit.min(self.pending_cluster_notifications.len());
        self.pending_cluster_notifications.drain(..taken).collect()
    }
}

/// Publish the notifications this store's clusters owe, and arrange for the ones
/// a clock-owed completion will raise.
///
/// Every operation this service serves calls this once it has answered (see
/// `crate::service::query_error_envelope`), so an event is delivered by the same
/// call that recorded it.
///
/// The store's lock is taken for the take alone and NEVER held across an await:
/// each publish is an in-process call into Amazon SNS, which must not run under
/// this service's lock.
///
/// A publish Amazon SNS refuses — most often a topic that does not exist, or one
/// no SNS is mounted to hold — is LOGGED, not raised: ElastiCache does not fail a
/// control-plane call because a notification could not be delivered, and this
/// runs after the response has been rendered, so it cannot change what the call
/// answered. Nothing is requeued: delivery is at-most-once.
pub async fn deliver_cluster_notifications<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = service_state(&**engine) else {
        // No store: the handler that answered this request has already reported
        // the store's absence with its own modelled fault.
        return;
    };
    let owed = state
        .lock()
        .take_pending_notifications(MAX_NOTIFICATIONS_PER_REQUEST);
    publish(ctx.callback(), &owed).await;
    // The completion the clock still owes a configured cluster is published when
    // it falls due, rather than when somebody next calls; see
    // [`publish_when_due`].
    publish_when_due(::std::sync::Arc::clone(engine), ctx.callback().clone());
}

/// Publish `notifications`, each to the topic it names.
///
/// The client is built once per Region and reused, so a set of notifications
/// spanning one Region builds one client.
async fn publish(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    notifications: &[ClusterNotification],
) {
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for notification in notifications {
        // Re-read from the ARN rather than carried beside it, so the Region a
        // publish is addressed in is always one that came out of the ARN.
        let Some(region) = topic_region(&notification.topic_arn) else {
            continue;
        };
        if client
            .as_ref()
            .is_none_or(|(held, _)| held.as_str() != region)
        {
            client = Some((region.to_string(), sns_client(callback, region)));
        }
        let Some((_, sns)) = client.as_ref() else {
            continue;
        };
        let published = sns
            .publish()
            .topic_arn(&notification.topic_arn)
            .message(&notification.message)
            .send()
            .await;
        if let Err(failure) = published {
            ::tracing::warn!(
                topic = %notification.topic_arn,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "a cluster event could not be published to the cluster's notification topic",
            );
        }
    }
}

/// How many of these settle-and-publish tasks may be in flight at once.
///
/// One request starts at most one, and a slot is given back however the task
/// ends; a request that finds none free leaves the completion to the next call
/// that reads the cluster, which materialises and publishes it the same way.
const MAX_TASKS_IN_FLIGHT: usize = 64;

/// How long past the due instant the task below settles, so the completion is
/// already due when it takes the lock rather than a millisecond short of it.
const SETTLE_SLACK: Duration = Duration::from_millis(50);

/// How many times one task waits for a due instant: a completion settled while
/// it waited leaves nothing to wait for, and a cluster whose next completion
/// falls further out is waited for again, at most this many times, so the task
/// cannot be kept alive without end.
const MAX_SETTLE_WAITS: usize = 4;

/// The slots [`MAX_TASKS_IN_FLIGHT`] bounds.
static TASKS_IN_FLIGHT: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// Materialise the completion the clock owes a cluster whose notification
/// configuration is active once it is due, and publish what that raised.
///
/// Returns at once: the settle runs in a task of its own, because the call that
/// started the work (a create, a reboot, an immediate modify, a snapshot) has to
/// answer while that work is still going on. It exists for the client driven by
/// these NOTIFICATIONS — such a client waits on its subscriber rather than
/// calling this service again, so without it the completion's notification would
/// wait for a status read that never comes.
///
/// It materialises exactly the transitions the clock has already made due,
/// through the same one settle path a status read uses, so what this writes is
/// what a read at the same instant would have written; a completion a reader
/// already settled leaves nothing to publish, because the queue is TAKEN and each
/// notification is therefore published exactly once whichever got there first.
///
/// Its whole life is bounded: one of [`MAX_TASKS_IN_FLIGHT`] slots, at most
/// [`MAX_SETTLE_WAITS`] waits, and each wait is the span the store itself put on
/// the completion — never a duration a request chose.
fn publish_when_due<Eng: Send + Sync + 'static>(
    engine: ::std::sync::Arc<Eng>,
    callback: ::smithy_server_runtime::EmbeddedEmulator,
) {
    // Nothing is owed to a topic, which is every store with no cluster waiting on
    // a completion its configuration is told about: no task, no slot, no wait.
    if next_notified_completion(&*engine).is_none() {
        return;
    }
    let Some(slot) = TaskSlot::claim() else {
        ::tracing::warn!(
            "{MAX_TASKS_IN_FLIGHT} cluster notification tasks are already in flight; this \
             completion is published by the next call that reads the cluster"
        );
        return;
    };
    ::tokio::spawn(async move {
        let _slot = slot;
        for _ in 0..MAX_SETTLE_WAITS {
            let Some(due) = next_notified_completion(&*engine) else {
                break;
            };
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or_default()
                .saturating_add(SETTLE_SLACK);
            ::tokio::time::sleep(wait).await;
            let owed = match service_state(&*engine) {
                None => Vec::new(),
                Some(state) => {
                    let mut store = state.lock();
                    store.settle_all(SystemTime::now());
                    store.take_pending_notifications(MAX_NOTIFICATIONS_PER_REQUEST)
                }
            };
            publish(&callback, &owed).await;
        }
    });
}

/// When the next completion a cluster's ACTIVE notification configuration is told
/// about falls due, or `None` when no cluster of this store is waiting on one.
fn next_notified_completion<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<SystemTime> {
    let state = service_state(engine)?;
    let store = state.lock();
    store
        .cache_clusters
        .values()
        .filter(|cluster| {
            cluster.pending_event.is_some()
                && cluster.notification_topic_status == NOTIFICATION_STATUS_ACTIVE
                && cluster
                    .notification_topic_arn
                    .as_deref()
                    .is_some_and(|arn| topic_region(arn).is_some())
        })
        .filter_map(|cluster| cluster.pending_status.as_ref().map(|pending| pending.due))
        .min()
}

/// ONE of the [`MAX_TASKS_IN_FLIGHT`] slots, held for as long as the task that
/// took it runs.
///
/// The slot is given back when this is DROPPED, so a task that ends early — or
/// panics — cannot leak it.
struct TaskSlot;

impl TaskSlot {
    /// Take one slot, or `None` when they are all taken: the count is only ever
    /// raised from below the ceiling, so two callers cannot both take the last
    /// one.
    fn claim() -> Option<Self> {
        TASKS_IN_FLIGHT
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |held| (held < MAX_TASKS_IN_FLIGHT).then_some(held + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TaskSlot {
    fn drop(&mut self) {
        TASKS_IN_FLIGHT.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// The Amazon SNS client a notification is published through: the IN-PROCESS
/// callback into this same emulator.
///
/// Nothing here opens a socket or leaves the host: the callback is the
/// framework's seam for calling another emulated service, and the `http_client`
/// it hands over dispatches the request inside this process to whichever mounted
/// service claims it. The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up, and `region` is
/// the label [`topic_region`] read out of the topic's own ARN, so a caller cannot
/// steer that authority.
///
/// The credentials are the emulator's own root credentials, which only sign this
/// operation's in-process calls; they are never put in a response or a log.
fn sns_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_ARN_VENDOR}.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every published event name this service raises, for the tables below.
    const EVENTS: [&str; 9] = [
        EVENT_CACHE_CLUSTER_PROVISIONING_COMPLETE,
        EVENT_CACHE_NODES_REBOOTED,
        EVENT_ADD_CACHE_NODE_COMPLETE,
        EVENT_REMOVE_CACHE_NODE_COMPLETE,
        EVENT_CACHE_CLUSTER_SCALING_COMPLETE,
        EVENT_CACHE_CLUSTER_PARAMETERS_CHANGED,
        EVENT_CACHE_CLUSTER_SECURITY_GROUP_MODIFIED,
        EVENT_DELETE_CACHE_CLUSTER_COMPLETE,
        EVENT_SNAPSHOT_COMPLETE,
    ];

    /// The document names the event and the cluster it happened to, for every
    /// event this service raises and whatever the cluster is called.
    #[test]
    fn a_document_names_its_event_and_its_cluster() {
        for event in EVENTS {
            assert!(
                event.starts_with(EVENT_PREFIX),
                "case {event:?}: expected a published name spelled {EVENT_PREFIX}<Event>"
            );
            for cluster in ["c", "my-cluster", "cluster-with-a-rather-long-identifier-0001"] {
                let document = notification_document(event, cluster);
                let parsed: ::serde_json::Value =
                    ::serde_json::from_str(&document).unwrap_or_else(|error| {
                        panic!("case {event:?}/{cluster:?}: expected JSON, got {document:?}: {error}")
                    });
                let named = parsed.get(event).and_then(::serde_json::Value::as_str);
                assert_eq!(
                    named,
                    Some(cluster),
                    "case {event:?}/{cluster:?}: expected the event keyed to the cluster, got \
                     {document:?}"
                );
            }
        }
    }

    /// Which values name a topic a publish can be addressed to: an SNS topic ARN
    /// whose Region field is a Region label, and nothing else.
    #[test]
    fn only_an_sns_topic_arn_names_somewhere_to_publish() {
        // (case, value, the Region it names)
        let rows: [(&str, &str, Option<&str>); 7] = [
            (
                "a topic in us-east-1",
                "arn:aws:sns:us-east-1:000000000000:cluster-events",
                Some("us-east-1"),
            ),
            (
                "a topic in another Region",
                "arn:aws:sns:eu-west-3:000000000000:cluster-events",
                Some("eu-west-3"),
            ),
            (
                "a topic in another partition",
                "arn:aws-cn:sns:cn-north-1:000000000000:cluster-events",
                Some("cn-north-1"),
            ),
            (
                "a queue, not a topic",
                "arn:aws:sqs:us-east-1:000000000000:cluster-events",
                None,
            ),
            (
                "a Region field that is not a label",
                "arn:aws:sns:us-east-1/../x:000000000000:cluster-events",
                None,
            ),
            ("no Region at all", "arn:aws:sns::000000000000:topic", None),
            ("not an ARN", "cluster-events", None),
        ];
        for (case, value, expected) in rows {
            assert_eq!(
                topic_region(value),
                expected,
                "case {case:?}: expected {expected:?} for {value:?}"
            );
        }
    }
}
