// SPDX-License-Identifier: CC0-1.0

use super::*;

/// A stored RDS event notification subscription: the record `DescribeEventSubscriptions`
/// reports.
///
/// Regional, like the DB instances and snapshots above: a subscription is created in one
/// region and `DescribeEventSubscriptions` lists that region's subscriptions.
#[derive(Debug, Clone)]
pub struct EventSubscriptionRecord {
    /// The caller's `SubscriptionName`, and the second half of this record's key.
    pub subscription_name: String,
    /// The region the subscription was created in; the scope of its ARN.
    pub region: String,
    /// `SnsTopicArn`: the topic RDS publishes the events to.
    pub sns_topic_arn: String,
    /// `Status`: "Can be one of the following: creating | modifying | deleting | active |
    /// no-permission | topic-not-exist" (`EventSubscription$Status`).
    pub status: String,
    /// `SubscriptionCreationTime`, which the model types as a STRING rather than a
    /// timestamp, so the record carries the instant already rendered — by the ONE renderer
    /// this service writes it with, [`event_subscription_creation_time`].
    pub subscription_creation_time: String,
    /// `SourceType`, absent when the subscription covers every source type.
    pub source_type: Option<String>,
    /// `SourceIdsList`.
    pub source_ids: Vec<String>,
    /// `EventCategoriesList`.
    pub event_categories: Vec<String>,
    /// `Enabled`.
    pub enabled: bool,
    /// The tags `CreateEventSubscription$Tags` assigned to the subscription. The modelled
    /// `EventSubscription` shape carries no tag member, so nothing reports them yet —
    /// `ListTagsForResource`, which does, is still scaffolded — but the caller's input is
    /// kept rather than dropped, so it is here when that operation lands.
    pub tags: Vec<Tag>,
}

impl EventSubscriptionRecord {
    /// A subscription as `CreateEventSubscription` mints it: RDS answers the create with
    /// the subscription already [`EVENT_SUBSCRIPTION_STATUS_ACTIVE`] — a subscription is a
    /// notification rule and provisions nothing — so it is in no transient status, and a
    /// later change to it is in force by the time that call answers too.
    #[allow(clippy::too_many_arguments)]
    pub fn active(
        subscription_name: String,
        region: String,
        sns_topic_arn: String,
        subscription_creation_time: String,
        source_type: Option<String>,
        source_ids: Vec<String>,
        event_categories: Vec<String>,
        enabled: bool,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            subscription_name,
            region,
            sns_topic_arn,
            status: EVENT_SUBSCRIPTION_STATUS_ACTIVE.to_string(),
            subscription_creation_time,
            source_type,
            source_ids,
            event_categories,
            enabled,
            tags,
        }
    }

    /// Accept a delete: the subscription's own answer reports the `deleting` status that
    /// says the delete was accepted. A notification rule has nothing provisioned to tear
    /// down, so the record leaves the store with that call rather than a span later — see
    /// `DeleteEventSubscription`.
    pub fn deleting(&self) -> Self {
        Self {
            status: EVENT_SUBSCRIPTION_STATUS_DELETING.to_string(),
            ..self.clone()
        }
    }

    /// `arn:aws:rds:<region>:<account>:es:<subscription name>` — the `es` resource type
    /// RDS gives an event subscription.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("es:{}", self.subscription_name),
        )
    }

    /// Project the stored record onto the modelled `EventSubscription` output shape.
    pub fn to_wire(&self) -> EventSubscription {
        EventSubscription {
            customer_aws_id: Some(emulator_server::emulator_account_id().to_string()),
            cust_subscription_id: Some(self.subscription_name.clone()),
            sns_topic_arn: Some(self.sns_topic_arn.clone()),
            status: Some(self.status.clone()),
            subscription_creation_time: Some(self.subscription_creation_time.clone()),
            source_type: self.source_type.clone(),
            source_ids_list: Some(self.source_ids.clone()),
            event_categories_list: Some(self.event_categories.clone()),
            enabled: Some(self.enabled),
            event_subscription_arn: Some(self.arn()),
        }
    }
}

/// The form an event subscription's `SubscriptionCreationTime` is rendered in: the
/// calendar date, then the time of day in 24-hour form to the MILLISECOND, separated by a
/// space and carrying no zone marker — `2026-10-29 21:33:43.917`, in UTC.
///
/// Read by `chrono`'s formatter, so the calendar, the zero padding and the fixed count of
/// fractional digits are library-rendered rather than written here.
const SUBSCRIPTION_CREATION_TIME_FORM: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// The instant `at` as an `EventSubscription$SubscriptionCreationTime` carries it.
///
/// The member is a STRING rather than a timestamp, so its form is the service's own rather
/// than one the model's `@timestampFormat` picks: the instant in UTC, to the millisecond,
/// in [`SUBSCRIPTION_CREATION_TIME_FORM`]. Every operation that answers with an
/// `EventSubscription` reports the stored string this produced, so the create, the
/// modify, the source-identifier changes, the delete and the describe cannot disagree
/// about one subscription's creation time.
pub fn event_subscription_creation_time(at: ::aws_smithy_types::DateTime) -> String {
    ::chrono::DateTime::from_timestamp(at.secs(), at.subsec_nanos())
        .map(|utc| utc.format(SUBSCRIPTION_CREATION_TIME_FORM).to_string())
        .unwrap_or_default()
}
