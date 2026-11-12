// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS notification a job's STATE CHANGE is published as.
//!
//! A job's notification settings exist to be acted on: "When the notification settings are
//! defined during job creation, you can choose to notify based on a specific set of job
//! states using the `JobStatesToNotify` array of strings, or you can specify that you want
//! to have Amazon SNS notifications sent out for all job states with `NotifyAll` set to
//! true" (`Notification`), and `Notification$JobStatesToNotify` is "the list of job states
//! that will trigger a notification for this job". So a job that ENTERS a state its own
//! settings select is published onto the topic those settings name
//! (`Notification$SnsTopicARN`, the topic "you want to associate with this job", which "you
//! can subscribe email addresses to"); storing the settings and never contacting the topic
//! leaves every one of those sentences unimplemented.
//!
//! This module is that one rule, so every operation that moves a job — the creates that
//! start one in its first state, and each later transition this API makes — publishes
//! through the same reading of the same members instead of carrying its own.
//!
//! What a client can observe is one `sns:Publish` onto the topic, made the way every
//! emulated service calls another: over the IN-PROCESS callback into this same emulator
//! ([`smithy_server_runtime::OperationContext::callback`]), which is also how an order's
//! topic is resolved before the order is stored ([`super::named_resources`]). Nothing here
//! opens a socket or leaves the host — the ARN a caller supplied is not a URL this code
//! dials, it is a topic the request is routed to INSIDE this process, so the topic has to be
//! one this emulator itself holds or Amazon SNS answers that it does not exist. The only
//! thing read out of the caller's ARN is which Region of this emulator the publish is
//! addressed in, under the workspace's one Region-label grammar
//! (`service_common::arn::region_of`), so a value carrying anything else names no topic at
//! all and is not published to.
//!
//! The work splits in two so that no publish is made while the store is locked: [`owed`]
//! (and the two collectors beside it) decide what a record that has just moved owes, under
//! the lock, and [`publish`] performs the publishes once the guard is dropped. The number of
//! publishes one request can ask for is therefore the number of job records that request
//! moved — one for a plain job, one per record of an export order, one per node of a
//! cluster — each of which is bounded by this service's own order sizes, so no single
//! request can grow the work a state change does.
//!
//! A publish the topic's own service refuses — most often a topic that does not exist, or
//! one no Amazon SNS is mounted to hold — is not the Snowball operation's failure: the order
//! is placed, the job is cancelled, the shipment is reported, and a notification that could
//! not be delivered is logged while the rest are still published.

use std::time::SystemTime;

use crate::ops::types::Notification;

use super::{JobState, StoredJob};

/// The vendor field of an Amazon SNS ARN: the service namespace Amazon SNS publishes, which
/// is what says a `SnsTopicARN` names a topic at all.
const SNS_ARN_VENDOR: &str = "sns";

/// One notification a moved job owes: the topic it is published to and the document
/// published onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStateNotification {
    /// The topic the job's `Notification$SnsTopicARN` named.
    pub topic_arn: String,
    /// The notification, as [`document`] renders it.
    pub message: String,
}

/// Whether `notification` selects `state`, i.e. whether a job entering it is one this
/// configuration is told about.
///
/// The two members the model gives a caller to choose with, each read as its own
/// documentation defines it: `NotifyAll` — "any change in job state will trigger a
/// notification for this job" — takes every state, and otherwise the states taken are
/// exactly the ones `JobStatesToNotify` lists ("the list of job states that will trigger a
/// notification for this job"). A configuration that lists none and sets neither selects no
/// state, so it is told about nothing.
///
/// The list is matched against the wire spelling of the `JobState` enum, which is what a
/// caller sends and what `DescribeJob` reports, so a value outside that catalog names no
/// state and selects nothing.
pub fn selects(notification: &Notification, state: JobState) -> bool {
    notification.notify_all
        || notification
            .job_states_to_notify
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|selected| selected == state.as_wire())
}

/// The notification `job` owes for the state it is NOW in, or `None` when nothing is owed.
///
/// Nothing is owed when the job carries no notification settings, when those settings name
/// no topic (a `SnsTopicARN` supplied empty names none, as every other member of this
/// service read as a resource is), when they do not select the state the job is in
/// ([`selects`]), or when the value they name is not the ARN of an Amazon SNS topic in a
/// Region of this emulator.
///
/// The state published is read off the STORED record rather than passed in, so a
/// notification can only ever report the state a later `DescribeJob` reports.
///
/// `DevicePickupSnsTopicARN` is deliberately not published to here: it is "used to send SNS
/// notifications for the person picking up the device (identified during job creation)",
/// which is a different audience from the job's state changes, and nothing in this API
/// reports a device being picked up.
pub fn owed(job: &StoredJob, at: SystemTime) -> Option<JobStateNotification> {
    let notification = job.notification.as_ref()?;
    let topic_arn = notification
        .sns_topic_arn
        .as_deref()
        .filter(|topic_arn| !topic_arn.is_empty())?;
    if !selects(notification, job.job_state) {
        return None;
    }
    if ::service_common::arn::region_of(topic_arn, SNS_ARN_VENDOR).is_none() {
        ::tracing::debug!(
            job = %job.job_id,
            "this job's notification names no Amazon SNS topic in a Region of this emulator; \
             its state change is not published"
        );
        return None;
    }
    Some(JobStateNotification {
        topic_arn: topic_arn.to_string(),
        message: document(job, at)?,
    })
}

/// Every notification the records in `jobs` owe for the states they are now in.
///
/// For the operations that have the records in hand — a create, which holds the order it is
/// about to store.
pub fn owed_for_records<'job>(
    jobs: impl IntoIterator<Item = &'job StoredJob>,
    at: SystemTime,
) -> Vec<JobStateNotification> {
    jobs.into_iter().filter_map(|job| owed(job, at)).collect()
}

/// Every notification the jobs `job_ids` names owe for the states they are now in, read off
/// the records `store` holds.
///
/// For the operations that have moved records already IN the store — the cancels and the
/// shipment report. An id the store holds no job for owes nothing.
pub fn owed_for_jobs(
    store: &super::ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState,
    job_ids: &[String],
    at: SystemTime,
) -> Vec<JobStateNotification> {
    owed_for_records(job_ids.iter().filter_map(|job_id| store.jobs.get(job_id)), at)
}

/// The document a subscriber of the job's topic receives: the job the notification is about
/// and the state it has entered.
///
/// The model declares no shape for the published notification, so every member here is read
/// off the stored job record — the id the service minted for it, the state it is now in, the
/// kind of order it is and the cluster it is a node of — and the instant comes from the
/// clock. A member the record does not carry is left out rather than written empty: a plain
/// job is a node of no cluster.
///
/// `None` when the instant cannot be rendered as a timestamp, which is what a clock reading
/// outside the representable range would be; nothing is published for it rather than a
/// document with a member missing.
fn document(job: &StoredJob, at: SystemTime) -> Option<String> {
    let mut rendered = ::serde_json::Map::new();
    rendered.insert("JobId".to_string(), job.job_id.clone().into());
    rendered.insert(
        "JobState".to_string(),
        job.job_state.as_wire().to_string().into(),
    );
    if let Some(job_type) = job.job_type.clone() {
        rendered.insert("JobType".to_string(), job_type.into());
    }
    if let Some(cluster_id) = job.cluster_id.clone() {
        rendered.insert("ClusterId".to_string(), cluster_id.into());
    }
    rendered.insert("Time".to_string(), timestamp(at)?.into());
    Some(::serde_json::Value::Object(rendered).to_string())
}

/// `at` as the notification's own instant: the RFC 3339 instant in UTC, the one form this
/// emulator writes into a document it renders itself. The calendar conversion is the
/// library's.
fn timestamp(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// Publish every notification in `notifications` onto the topic it names.
///
/// Called by a handler AFTER its own write is complete and its store guard dropped, so no
/// lock is held across the in-process calls this makes. The client is built once per Region
/// and reused, so a set of notifications spanning one Region builds one client.
pub async fn publish(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    notifications: &[JobStateNotification],
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
            client = Some((region.to_string(), sns_client(ctx, region)));
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
                "a job-state notification could not be published to the job's topic"
            );
        }
    }
}

/// The Amazon SNS client a job-state notification is published through: the IN-PROCESS
/// callback into this same emulator.
///
/// Configured exactly as the clients an order's resources are resolved through
/// ([`super::named_resources`]) and for the same reasons: `ctx.callback()` is the
/// framework's seam for calling another emulated service, and the `http_client` it hands
/// over dispatches the request inside this process to whichever mounted service claims it.
/// The `endpoint_url` below is therefore an ADDRESS the in-process router reads, never a
/// host any resolver looks up, and `region` is the label read out of the topic's own ARN, so
/// a caller cannot steer that authority. There is nothing to retry, time out, cache an
/// identity for or watch for a stalled stream over that transport — each would need a sleep
/// implementation this client is not given — and the credentials the callback hands over are
/// the emulator's own, which only sign this operation's in-process calls and are never put
/// in a response or a log.
fn sns_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_ARN_VENDOR}.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{owed, selects};
    use crate::ops::types::Notification;
    use crate::state::{JobState, StoredJob};

    /// Every `JobState` this service models, in the wire spelling the enum publishes.
    const EVERY_JOB_STATE: &[JobState] = &[
        JobState::New,
        JobState::PreparingAppliance,
        JobState::PreparingShipment,
        JobState::InTransitToCustomer,
        JobState::WithCustomer,
        JobState::InTransitToAWS,
        JobState::WithAWSSortingFacility,
        JobState::WithAWS,
        JobState::InProgress,
        JobState::Complete,
        JobState::Cancelled,
        JobState::Listing,
        JobState::Pending,
    ];

    /// A job in `state` with `notification`, and nothing else that bears on what is owed.
    fn job(state: JobState, notification: Option<Notification>) -> StoredJob {
        let mut job = StoredJob {
            job_id: "JID00000000-0000-0000-0000-000000000000".to_string(),
            job_state: state,
            job_type: Some("IMPORT".to_string()),
            snowball_type: None,
            snowball_capacity_preference: None,
            shipping_option: None,
            creation_date: SystemTime::UNIX_EPOCH,
            description: None,
            address_id: None,
            forwarding_address_id: None,
            role_arn: None,
            kms_key_arn: None,
            resources: None,
            notification: None,
            on_device_service_configuration: None,
            cluster_id: None,
            long_term_pricing_id: None,
            tax_documents: None,
            device_configuration: None,
            remote_management: None,
            impact_level: None,
            pickup_details: None,
            job_part_ids: Vec::new(),
            unlock_code: String::new(),
            order_region: String::new(),
        };
        job.notification = notification;
        job
    }

    /// A configuration selects a state when `NotifyAll` is set, or when its
    /// `JobStatesToNotify` lists that state — over EVERY state the enum publishes, not only
    /// the ones a create and a cancel move through.
    #[test]
    fn a_configuration_selects_the_states_it_names_or_every_state() {
        let notify_all = Notification {
            notify_all: true,
            ..Notification::default()
        };
        let listed = Notification {
            job_states_to_notify: Some(vec![
                JobState::New.as_wire().to_string(),
                JobState::Cancelled.as_wire().to_string(),
            ]),
            ..Notification::default()
        };
        // A list carrying a value outside the enum catalog, and one carrying nothing.
        let outside = Notification {
            job_states_to_notify: Some(vec!["new".to_string(), "Canceled".to_string()]),
            ..Notification::default()
        };
        let empty = Notification::default();
        for state in EVERY_JOB_STATE {
            for (case, notification, expected) in [
                ("NotifyAll", &notify_all, true),
                (
                    "a list of two states",
                    &listed,
                    matches!(state, JobState::New | JobState::Cancelled),
                ),
                ("a list outside the catalog", &outside, false),
                ("no states and no NotifyAll", &empty, false),
            ] {
                let selected = selects(notification, *state);
                assert_eq!(
                    selected,
                    expected,
                    "case {case:?} with state {:?}: expected {expected}, got {selected}",
                    state.as_wire()
                );
            }
        }
    }

    /// What a moved job owes, over the ways a configuration can fail to owe anything: no
    /// configuration at all, a configuration naming no topic, one naming a value that is not
    /// an Amazon SNS topic ARN, and one whose selection does not cover the state.
    #[test]
    fn nothing_is_owed_without_a_selected_state_and_a_topic_in_a_region() {
        let topic = "arn:aws:sns:us-west-2:000000000000:snow-order".to_string();
        // (case, the configuration, whether a notification is owed in `New`)
        let rows: Vec<(&str, Option<Notification>, bool)> = vec![
            ("no notification at all", None, false),
            (
                "a topic and the state listed",
                Some(Notification {
                    sns_topic_arn: Some(topic.clone()),
                    job_states_to_notify: Some(vec![JobState::New.as_wire().to_string()]),
                    ..Notification::default()
                }),
                true,
            ),
            (
                "a topic and NotifyAll",
                Some(Notification {
                    sns_topic_arn: Some(topic.clone()),
                    notify_all: true,
                    ..Notification::default()
                }),
                true,
            ),
            (
                "a topic, selecting another state only",
                Some(Notification {
                    sns_topic_arn: Some(topic.clone()),
                    job_states_to_notify: Some(vec![JobState::Complete.as_wire().to_string()]),
                    ..Notification::default()
                }),
                false,
            ),
            (
                "NotifyAll with no topic",
                Some(Notification {
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
            (
                "NotifyAll with an empty topic",
                Some(Notification {
                    sns_topic_arn: Some(String::new()),
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
            (
                "NotifyAll with another vendor's ARN",
                Some(Notification {
                    sns_topic_arn: Some("arn:aws:sqs:us-west-2:000000000000:inbox".to_string()),
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
            (
                "NotifyAll with a topic ARN carrying no Region",
                Some(Notification {
                    sns_topic_arn: Some("arn:aws:sns::000000000000:snow-order".to_string()),
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
            (
                "NotifyAll with a Region field that is not a Region label",
                Some(Notification {
                    sns_topic_arn: Some(
                        "arn:aws:sns:us-west-2/../x:000000000000:snow-order".to_string(),
                    ),
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
            (
                "the pickup topic alone, which job states are not published to",
                Some(Notification {
                    device_pickup_sns_topic_arn: Some(topic.clone()),
                    notify_all: true,
                    ..Notification::default()
                }),
                false,
            ),
        ];
        for (case, notification, expected) in rows {
            let job = job(JobState::New, notification);
            let owed = owed(&job, SystemTime::UNIX_EPOCH);
            assert_eq!(
                owed.is_some(),
                expected,
                "case {case:?}: expected owed={expected}, got {owed:?}"
            );
            if let Some(owed) = owed {
                assert_eq!(
                    owed.topic_arn, topic,
                    "case {case:?}: expected the configured topic, got {:?}",
                    owed.topic_arn
                );
                let parsed: ::serde_json::Value = ::serde_json::from_str(&owed.message)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the document is not JSON ({error}): {owed:?}")
                    });
                assert_eq!(
                    parsed["JobId"].as_str(),
                    Some(job.job_id.as_str()),
                    "case {case:?}: expected the job's own id, got {parsed}"
                );
                assert_eq!(
                    parsed["JobState"].as_str(),
                    Some(JobState::New.as_wire()),
                    "case {case:?}: expected the state the job is in, got {parsed}"
                );
            }
        }
    }
}
