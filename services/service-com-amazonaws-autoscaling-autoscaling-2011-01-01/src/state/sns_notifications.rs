// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS notifications this service publishes to the topics a group's
//! notification configurations name.
//!
//! # The rule this module implements
//!
//! A notification configuration exists to be acted on: `PutNotificationConfiguration`
//! "configures an Auto Scaling group to send notifications when specified events take
//! place. Subscribers to the specified topic can have messages delivered to an endpoint
//! such as a web server or an email address"
//! (`smithy_query(query="operation",operation="PutNotificationConfiguration")`). So a group
//! configured for an event type PUBLISHES that event onto every topic configured for it,
//! and onto no other topic — storing the configuration and never contacting the topic
//! leaves that sentence unimplemented, and a subscriber of the topic receives nothing at
//! all.
//!
//! Three things follow from it, each implemented here:
//!
//! * a configuration that selects `autoscaling:TEST_NOTIFICATION` — the event of this
//!   service's own catalog ([`super::AUTO_SCALING_NOTIFICATION_TYPES`]) that reports nothing
//!   but the configuration itself — publishes it to the topic the moment that configuration
//!   is stored, which is the only instant there is anything to tell that topic. The model
//!   documents the same confirmation for the other place a caller names a notification
//!   target, a lifecycle hook's: "When you specify a notification target, Amazon EC2 Auto
//!   Scaling sends it a test message. Test messages contain the following additional
//!   key-value pair: "Event": "autoscaling:TEST_NOTIFICATION""
//!   (`PutLifecycleHookType$NotificationTargetARN`);
//! * every instance the group LAUNCHES is published as `autoscaling:EC2_INSTANCE_LAUNCH`
//!   and every instance it TERMINATES as `autoscaling:EC2_INSTANCE_TERMINATE`, naming the
//!   instance that moved and the scaling activity that moved it — the same activity
//!   `DescribeScalingActivities` reports, so a notification and the activity log are one
//!   fact;
//! * a topic nobody configured for an event receives nothing, which is only distinguishable
//!   from "nothing was ever published" because the publication happens.
//!
//! # Where a notification comes from
//!
//! Nothing here decides WHEN an instance moves, and nothing here reads the store: a
//! lifecycle step's notifications are selected from the group's own configurations at the
//! instant the step is recorded ([`super::StoredGroup::notification_topics`], carried on
//! [`PendingLifecycleEvent::notification_topics`]) and published by the same drain that puts
//! that step on the account's event bus ([`super::lifecycle_events`]), so one step produces
//! one notification per configured topic however it was asked for and none can be published
//! twice.
//!
//! What a client can observe is one `sns:Publish` onto the topic, made the way every
//! emulated service calls another: over the IN-PROCESS callback into this same emulator
//! ([`smithy_server_runtime::EmbeddedEmulator`]). Nothing here opens a socket or leaves the
//! host — the ARN a caller supplied is not a URL this code dials, it is a topic the request
//! is routed to INSIDE this process, so the topic has to be one this emulator itself holds
//! or Amazon SNS answers that it does not exist. The only thing read out of the caller's
//! ARN is which Region of this emulator the publish is addressed in, under the workspace's
//! one Region-label grammar (`service_common::arn::region_of`), so a value carrying
//! anything else names no topic at all and is not published to.
//!
//! How many publishes one step can ask for is bounded by the group's own configuration
//! ceiling ([`super::MAX_NOTIFICATION_TOPICS_PER_GROUP`] topics per group, so at most that
//! many per instance moved), on top of the bounds the queue it is drained from already
//! carries ([`super::lifecycle_events::MAX_PENDING_EVENTS`] queued, and one drain's own
//! cap), so no single request can grow the work a capacity change does.
//!
//! A publish the topic's own service refuses — most often a topic that does not exist, or
//! one no Amazon SNS is mounted to hold — is not the Auto Scaling operation's failure: the
//! configuration is stored and the capacity change stands, and a notification that could
//! not be delivered is logged while the rest are still published.

use std::time::SystemTime;

use super::{PendingLifecycleEvent, StoredGroup, ACTIVITY_SUCCESSFUL};

/// The vendor field of an Amazon SNS ARN: the service namespace Amazon SNS publishes,
/// which is what says a `TopicARN` names a topic at all.
const SNS_ARN_VENDOR: &str = "sns";

/// The `Service` every notification this service publishes names itself by, as Amazon EC2
/// Auto Scaling words it in the message a subscriber receives.
const NOTIFICATION_SERVICE: &str = "AWS Auto Scaling";

/// The event a notification configuration is CONFIRMED on its topic by, from this
/// service's own catalog ([`super::AUTO_SCALING_NOTIFICATION_TYPES`]).
pub const TEST_NOTIFICATION: &str = "autoscaling:TEST_NOTIFICATION";

/// The event an instance the group LAUNCHED is published under (same catalog).
pub const EC2_INSTANCE_LAUNCH: &str = "autoscaling:EC2_INSTANCE_LAUNCH";

/// The event an instance the group TERMINATED is published under (same catalog).
pub const EC2_INSTANCE_TERMINATE: &str = "autoscaling:EC2_INSTANCE_TERMINATE";

/// One notification waiting to be published: the topic it goes to and the document a
/// subscriber of that topic receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNotification {
    /// The topic one of the group's notification configurations named.
    pub topic_arn: String,
    /// The notification, as this module renders it.
    pub message: String,
}

/// The notification that confirms a stored configuration on the topic it names, or `None`
/// when that topic is owed none.
///
/// Owed when the group's STORED configuration for this topic selects
/// [`TEST_NOTIFICATION`] — read off the group rather than off the request, so the
/// selection rule is the one every event of this service is published under
/// ([`super::StoredGroup::notification_topics`]) — and the value names an Amazon SNS topic
/// in a Region of this emulator.
///
/// `request_id` is the id of the call that stored the configuration: the notification
/// reports which request produced it, exactly as the notification for a lifecycle step
/// reports the activity that produced that one.
#[must_use]
pub fn test_notification(
    group: &StoredGroup,
    topic_arn: &str,
    request_id: &str,
    at: SystemTime,
) -> Option<PendingNotification> {
    if !group
        .notification_topics(TEST_NOTIFICATION)
        .iter()
        .any(|configured| configured == topic_arn)
    {
        return None;
    }
    if ::service_common::arn::region_of(topic_arn, SNS_ARN_VENDOR).is_none() {
        ::tracing::debug!(
            group = %group.name,
            "this notification configuration names no Amazon SNS topic in a Region of this \
             emulator; nothing is published to it"
        );
        return None;
    }
    let mut document = ::serde_json::Map::new();
    insert_text(&mut document, "AccountId", ::engine::emulator_account_id());
    insert_text(&mut document, "RequestId", request_id);
    insert_text(&mut document, "AutoScalingGroupARN", &group.arn);
    insert_text(&mut document, "AutoScalingGroupName", &group.name);
    insert_text(&mut document, "Service", NOTIFICATION_SERVICE);
    insert_text(&mut document, "Event", TEST_NOTIFICATION);
    insert_text(&mut document, "Time", &date_time_text(at));
    Some(PendingNotification {
        topic_arn: topic_arn.to_string(),
        message: ::serde_json::Value::Object(document).to_string(),
    })
}

/// Every notification one instance lifecycle step owes: one per topic the group's
/// configurations selected for that step when it was recorded.
///
/// The document is rendered against the step's own activity — the group it moved an
/// instance of, the instance, the activity's id, description, cause and span, and the
/// placement the account's Amazon EC2 store gave the instance — so a notification reports
/// the activity a `DescribeScalingActivities` of the same instant reports and never a
/// second, invented copy of it. The activity has completed by the instant the step is
/// published (that completion is what the step reports), so it carries the successful
/// status code, the full progress and no status message.
#[must_use]
pub fn owed_for_lifecycle_event(event: &PendingLifecycleEvent) -> Vec<PendingNotification> {
    event
        .notification_topics
        .iter()
        .filter(|topic_arn| {
            let addressable =
                ::service_common::arn::region_of(topic_arn, SNS_ARN_VENDOR).is_some();
            if !addressable {
                ::tracing::debug!(
                    group = %event.group_name,
                    "this notification configuration names no Amazon SNS topic in a Region of \
                     this emulator; the group's instance lifecycle event is not published to it"
                );
            }
            addressable
        })
        .map(|topic_arn| PendingNotification {
            topic_arn: topic_arn.clone(),
            message: lifecycle_document(event),
        })
        .collect()
}

/// The document a subscriber of a configured topic receives for one lifecycle step.
fn lifecycle_document(event: &PendingLifecycleEvent) -> String {
    let mut placement = ::serde_json::Map::new();
    if let Some(zone) = event.availability_zone.as_ref() {
        insert_text(&mut placement, "Availability Zone", zone);
    }
    if let Some(subnet_id) = event.subnet_id.as_ref() {
        insert_text(&mut placement, "Subnet ID", subnet_id);
    }
    let mut document = ::serde_json::Map::new();
    document.insert(
        "Progress".to_string(),
        ::serde_json::Value::from(COMPLETED_PROGRESS),
    );
    insert_text(&mut document, "AccountId", ::engine::emulator_account_id());
    insert_text(&mut document, "Description", &event.description);
    // The request that carried the activity out is the activity itself, which is how this
    // service reports it everywhere: one id for the activity and for the request behind it.
    insert_text(&mut document, "RequestId", &event.activity_id);
    insert_text(&mut document, "EndTime", &date_time_text(event.due));
    insert_text(&mut document, "AutoScalingGroupARN", &event.group_arn);
    insert_text(&mut document, "ActivityId", &event.activity_id);
    insert_text(&mut document, "StartTime", &date_time_text(event.start_time));
    insert_text(&mut document, "Service", NOTIFICATION_SERVICE);
    insert_text(&mut document, "Time", &date_time_text(event.due));
    insert_text(&mut document, "EC2InstanceId", &event.instance_id);
    insert_text(&mut document, "StatusCode", ACTIVITY_SUCCESSFUL);
    insert_text(&mut document, "StatusMessage", "");
    document.insert(
        "Details".to_string(),
        ::serde_json::Value::Object(placement),
    );
    insert_text(&mut document, "AutoScalingGroupName", &event.group_name);
    insert_text(&mut document, "Cause", &event.cause);
    insert_text(&mut document, "Event", event.kind.notification_event());
    ::serde_json::Value::Object(document).to_string()
}

/// `Progress` of the activity a published step reports: it has completed by the instant
/// the step is a fact, which is the instant it is published at.
const COMPLETED_PROGRESS: i64 = 100;

/// Write one text member into a rendered document.
fn insert_text(document: &mut ::serde_json::Map<String, ::serde_json::Value>, key: &str, value: &str) {
    document.insert(
        key.to_string(),
        ::serde_json::Value::String(value.to_string()),
    );
}

/// The date-time text a notification carries an instant in: RFC 3339 in UTC, the one form
/// this emulator writes into a document it renders itself.
fn date_time_text(at: SystemTime) -> String {
    super::to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Publish every notification in `notifications` onto the topic it names.
///
/// Called with NO store lock held, so nothing awaits while this service's state is locked.
/// The client is built once per Region and reused, so a set of notifications spanning one
/// Region builds one client.
pub async fn publish(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    notifications: &[PendingNotification],
) {
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for notification in notifications {
        // Re-read from the ARN rather than carried alongside it, so the Region a publish is
        // addressed in is always one that came out of the ARN under the Region grammar.
        let Some(region) = ::service_common::arn::region_of(&notification.topic_arn, SNS_ARN_VENDOR)
        else {
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
        if let Err(error) = published {
            ::tracing::warn!(
                topic = %notification.topic_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Auto Scaling notification could not be published to the configured topic"
            );
        }
    }
}

/// The Amazon SNS client a notification is published through: the IN-PROCESS callback into
/// this same emulator.
///
/// Configured exactly as the EventBridge client the same steps are published onto the
/// account's event bus through (`service_common::eventbridge_client`) and for the same
/// reasons: the callback is the framework's seam for calling another emulated service, and
/// the `http_client` it hands over dispatches the request inside this process to whichever
/// mounted service claims it. The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up, and `region` is the label
/// read out of the topic's own ARN, so a caller cannot steer that authority. There is
/// nothing to retry, time out, cache an identity for or watch for a stalled stream over
/// that transport — each would need a sleep implementation this client is not given — and
/// the credentials the callback hands over are the emulator's own, which only sign this
/// operation's in-process calls and are never put in a response or a log.
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
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
