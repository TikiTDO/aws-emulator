// SPDX-License-Identifier: CC0-1.0

//! STACK EVENT NOTIFICATIONS: the Amazon SNS message CloudFormation publishes for every
//! `StackEvent` of a stack that carries `NotificationARNs`.
//!
//! ## The rule this module implements
//!
//! "The Amazon SNS topic ARNs to publish stack related events" (`CreateStackInput`,
//! `UpdateStackInput` and `CreateChangeSetInput` all carry the member): a stack whose
//! `NotificationARNs` name topics gets ONE notification per stack event published to EACH of
//! them — the stack's own status changes and every per-resource transition alike — whichever
//! operation caused the event, the execution of a change set included. A stack that carries
//! no topics publishes nothing, so clearing the list stops the stream and naming a topic
//! again resumes it.
//!
//! The message body is the event as `Key='value'` lines, one per line, in the order
//! [`NOTIFICATION_KEYS`] fixes. What a member the event has no value for renders as is stated
//! per member, once: the two members a notification states only where the event HAS them
//! ([`OMITTED_WHEN_ABSENT`]) carry no line at all without one, and every other member states
//! its own absent form ([`absent_value`]) rather than being left out — so a subscriber parses
//! the same line, at the same position, for it on every event.
//!
//! ## When a notification is owed, and to whom
//!
//! The topics an event is published to are the ones in force WHEN THE EVENT WAS RECORDED: an
//! operation that clears the stack's topics does not retroactively silence the events
//! recorded before it, and one that names a topic again does not deliver the backlog of the
//! window in which the stack had none.
//!
//! What travels with the event is not that list but a DIGEST of it
//! ([`crate::state::StoredEvent::owed_notification`], eight bytes, [`topics_digest`]): the
//! stack already holds the one copy of its own `NotificationARNs`, so an event owing a
//! notification is one whose digest still matches that list, and an event recorded under a
//! configuration the stack has since left owes nothing. That is what keeps a caller-sized
//! list — an ARN is caller text, and the model bounds the list's LENGTH at five but not the
//! length of an entry — out of the per-stack event log, which retains up to
//! [`crate::state::MAX_EVENTS_PER_STACK`] events.
//!
//! Publishing is a call to another service, so it cannot be made where the event is recorded
//! (that is under this store's lock, and a stack's events are recorded as the clock reaches
//! them). [`deliver`] is what makes it: it CLAIMS every notification the stacks of a Region
//! owe under one lock, releases it, and publishes each claimed message — the same shape
//! [`crate::state::provisioning::advance_stabilisations`] uses for every other call a stack's
//! own progress owes, and it is driven from there.
//!
//! ## Bounds
//!
//! One claim per recorded event per stack, and an event is claimed exactly once: the digest
//! comes off the event as it is claimed, so nothing is published twice. A claim carries ONE
//! copy of a stack's topics for every message of that stack, so the work and the memory of a
//! claim are the events claimed (at most [`crate::state::MAX_EVENTS_PER_STACK`]) plus the
//! topics the stack holds (at most five, and already held), never their product.
//!
//! ## Where the notification goes
//!
//! Nowhere outside this process. The `TopicArn` is caller-supplied text, and it is never
//! resolved, connected to or used to address anything: what it names is a topic OF THIS
//! EMULATOR, reached through the same signed, in-process callback every other call this crate
//! makes rides ([`crate::state::provisioning::call_query`]), whose endpoint is fixed here from
//! Amazon SNS's own signing name. A topic this emulator does not hold is a refusal from its
//! own SNS, not a dial.

use super::provisioning::call_query;

/// Amazon SNS's signing name and awsQuery API version, from its model (`sigv4Name: sns`,
/// `version: 2010-03-31`) — the call one notification is published with.
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// The `Subject` every one of these notifications carries.
const SUBJECT: &str = "AWS CloudFormation Notification";

/// The DEFAULT form a member the event carries no value for is rendered as; the two lists
/// below name the members whose absent form differs.
pub const ABSENT_VALUE: &str = "null";

/// The members whose absent form is the EMPTY STRING rather than [`ABSENT_VALUE`]: the
/// success/failure message of the transition and its detailed status are STATUS TEXT, which
/// an event that reports none states as empty rather than as a value.
const EMPTY_WHEN_ABSENT: [&str; 2] = ["ResourceStatusReason", "DetailedStatus"];

/// The members a notification states only for an event that HAS one, and leaves out
/// altogether otherwise: the physical resource an event about a resource nothing has created
/// yet does not name, and the operating principal, which is stated on an event about the
/// STACK itself (`PrincipalId` is an attribute of the stack operation, not of a resource's
/// transition).
const OMITTED_WHEN_ABSENT: [&str; 2] = ["PhysicalResourceId", "PrincipalId"];

/// How one key's value is rendered when the event carries none.
fn absent_value(key: &str) -> &'static str {
    if EMPTY_WHEN_ABSENT.contains(&key) {
        ""
    } else {
        ABSENT_VALUE
    }
}

/// The keys one notification body carries, in the order it carries them.
pub const NOTIFICATION_KEYS: [&str; 14] = [
    "StackId",
    "Timestamp",
    "EventId",
    "LogicalResourceId",
    "Namespace",
    "PhysicalResourceId",
    "PrincipalId",
    "ResourceProperties",
    "ResourceStatus",
    "ResourceStatusReason",
    "DetailedStatus",
    "ResourceType",
    "StackName",
    "ClientRequestToken",
];

/// Every notification ONE stack owes: the topics they go to, held once, and one message per
/// event that owes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedNotifications {
    /// The Amazon SNS topic ARNs every message below goes to — the stack's own list, cloned
    /// once per claim rather than once per message.
    pub topics: Vec<String>,
    /// The message bodies ([`body_of`]), in the order the events were recorded.
    pub messages: Vec<String>,
}

/// The eight-byte digest of one topic LIST, which is what an event carries instead of the
/// list itself.
///
/// Each entry is length-prefixed, so two different lists cannot digest alike by running
/// their entries together. Taken from the same SHA-256 this crate digests every identifier
/// with.
pub fn topics_digest(topics: &[String]) -> u64 {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for topic in topics {
        hasher.update((topic.len() as u64).to_be_bytes());
        hasher.update(topic.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// The digest an event records as the notification it OWES, or `None` for a stack carrying no
/// topics — which owes nothing.
pub fn owed_by(topics: &[String]) -> Option<u64> {
    (!topics.is_empty()).then(|| topics_digest(topics))
}

/// The body one event is published as: `Key='value'` lines in [`NOTIFICATION_KEYS`] order.
pub fn body_of(event: &super::StoredEvent) -> String {
    let timestamp = notification_timestamp(event.timestamp);
    let mut body = String::new();
    for key in NOTIFICATION_KEYS {
        let value = match key {
            "StackId" => Some(event.stack_id.as_str()),
            "Timestamp" => Some(timestamp.as_str()),
            "EventId" => Some(event.event_id.as_str()),
            "LogicalResourceId" => Some(event.logical_resource_id.as_str()),
            // "Namespace" is the account the stack belongs to.
            "Namespace" => Some(ACCOUNT_NAMESPACE.get_or_init(account_namespace).as_str()),
            "PhysicalResourceId" => event.physical_resource_id.as_deref(),
            // The IAM principal the stack operation was made by ([`caller_principal_id`]),
            // which the notification of an event about the STACK itself carries.
            "PrincipalId" => (event.resource_type == super::STACK_RESOURCE_TYPE)
                .then(|| CALLER_PRINCIPAL_ID.get_or_init(caller_principal_id).as_str()),
            "ResourceProperties" => event.resource_properties.as_deref(),
            "ResourceStatus" => Some(event.resource_status.as_str()),
            "ResourceStatusReason" => event.resource_status_reason.as_deref(),
            // `StackEvent$DetailedStatus`, "an optional field that contains information
            // about the detailed status of the stack event": the event's own value, stated
            // once for every reader of an event ([`super::detailed_status_of`]), with the
            // member's empty absent form for an event that reports none.
            "DetailedStatus" => super::detailed_status_of(event),
            "ResourceType" => Some(event.resource_type.as_str()),
            "StackName" => Some(event.stack_name.as_str()),
            "ClientRequestToken" => event.client_request_token.as_deref(),
            // Every key of the list above is answered; `NOTIFICATION_KEYS` is this
            // module's own constant, so there is no other key to answer.
            _ => None,
        };
        let rendered = match value.filter(|value| !value.is_empty()) {
            Some(value) => value,
            // A member the body states only where the event HAS it carries no line at all
            // without one ([`OMITTED_WHEN_ABSENT`]); every other member states its own
            // absent form, so a subscriber parses the same line for it on every event.
            None if OMITTED_WHEN_ABSENT.contains(&key) => continue,
            None => absent_value(key),
        };
        body.push_str(key);
        body.push_str("='");
        body.push_str(rendered);
        body.push_str("'\n");
    }
    body
}

/// The account every stack of this emulator belongs to, computed once: the same value for
/// every event, so it is not rebuilt per line.
static ACCOUNT_NAMESPACE: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();

/// The account id that namespace is.
fn account_namespace() -> String {
    ::emulator_server::emulator_account_id().to_string()
}

/// The IAM principal a notification states as the one that operated the stack, computed once
/// for the same reason the namespace above is.
static CALLER_PRINCIPAL_ID: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();

/// The unique id of the IAM entity whose credential operates this account's stacks.
///
/// The emulator publishes ONE long-term credential, and the principal it speaks for — the
/// account's single IAM user — is stated once for the whole tree
/// (`service_common::caller_principal`): the `UserId` STS reports for a caller presenting
/// that credential, and the id IAM holds for that user. So the principal a notification
/// names is the one a subscriber that asks for its own identity reads back, rather than an
/// id nothing resolves.
fn caller_principal_id() -> String {
    ::service_common::caller_principal::ambient_user_id()
}

/// One event's `Timestamp`, as a notification states it: the instant in the
/// millisecond-precision `date-time` form, which is the precision a stack event's
/// notification carries.
fn notification_timestamp(at: ::std::time::SystemTime) -> String {
    let rendered = super::timestamp(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let Some(instant) = rendered.strip_suffix('Z') else {
        return rendered;
    };
    let (seconds, fraction) = match instant.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (instant, ""),
    };
    let mut millis: String = fraction
        .chars()
        .filter(|digit| digit.is_ascii_digit())
        .take(MILLISECOND_DIGITS)
        .collect();
    while millis.len() < MILLISECOND_DIGITS {
        millis.push('0');
    }
    format!("{seconds}.{millis}Z")
}

/// How many fractional-second digits that form carries.
const MILLISECOND_DIGITS: usize = 3;

/// Claim every notification the stacks of `region` owe, and publish each one.
///
/// Claimed under one lock and published with it released, exactly as every other call a
/// stack's own progress owes is made ([`super::provisioning::advance_stabilisations`], which
/// drives this). A publish the topic's own service refuses is not retried: the notification
/// was owed when the event was recorded, and a topic that is gone is not a reason to keep the
/// message — the refusal is logged instead of being dropped silently.
pub async fn deliver(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    let owed: Vec<OwedNotifications> = {
        let mut store = state.lock();
        // Every event the clock has made due is recorded first, so the notifications this
        // call publishes are every one owed AS OF NOW rather than as of the last caller.
        store.settle(::std::time::SystemTime::now());
        store.claim_owed_notifications(region)
    };
    for notification in owed {
        for topic_arn in &notification.topics {
            for message in &notification.messages {
                let parameters = vec![
                    ("TopicArn".to_string(), topic_arn.clone()),
                    ("Subject".to_string(), SUBJECT.to_string()),
                    ("Message".to_string(), message.clone()),
                ];
                if let Err(refusal) = call_query(
                    ctx,
                    SNS_SIGNING_NAME,
                    region,
                    SNS_API_VERSION,
                    "Publish",
                    &parameters,
                )
                .await
                {
                    ::tracing::warn!(
                        code = refusal.code,
                        "a stack event notification was not published to its topic"
                    );
                }
            }
        }
    }
}

impl super::ComAmazonawsCloudformationCloudformationState {
    /// Take every notification the stacks of `region` owe, one entry per stack that owes any,
    /// with its messages in the order the events were recorded.
    ///
    /// Claiming is TAKING: the digest comes off each event, so each message is built once and
    /// published once however many callers drive [`deliver`]. An event that owes nothing — the
    /// stack carried no topics when it was recorded — and one recorded under a topic list the
    /// stack has since LEFT are both passed over: the window they belong to is over, and its
    /// backlog is not delivered to whatever the stack names now.
    pub fn claim_owed_notifications(&mut self, region: &str) -> Vec<OwedNotifications> {
        let mut owed = Vec::new();
        for stack in self.stacks.iter_mut().filter(|stack| stack.region == region) {
            // The stack's own list, read once: the digest every event is matched against, and
            // the topics the messages below go to.
            let topics = stack.notification_arns.clone();
            let in_force = owed_by(&topics);
            let mut messages = Vec::new();
            for event in &mut stack.events {
                let Some(digest) = event.owed_notification.take() else {
                    continue;
                };
                if Some(digest) == in_force {
                    messages.push(body_of(event));
                }
            }
            if !messages.is_empty() {
                owed.push(OwedNotifications { topics, messages });
            }
        }
        owed
    }
}
