// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS notification configurations of an Auto Scaling group: which topic the
//! group notifies, and about which events.
//!
//! A notification configuration is a CHILD of one Auto Scaling group keyed by the topic it
//! names, so it is stored on [`StoredGroup`] itself
//! ([`StoredGroup::notification_configurations`]): one collection that
//! `PutNotificationConfiguration` upserts into, `DescribeNotificationConfigurations`
//! projects from, and the group's own delete takes with the group. Nothing in this crate
//! holds a second copy.
//!
//! # The upsert
//!
//! `PutNotificationConfiguration` "configures an Auto Scaling group to send notifications
//! when specified events take place […] This configuration overwrites any existing
//! configuration"
//! (`smithy_query(query="operation",operation="PutNotificationConfiguration")`), and the
//! configuration a call carries is one TOPIC and the event types it is to be told about
//! (`TopicARN`, `NotificationTypes`). So the collection is keyed by topic and a second call
//! naming the same topic REPLACES that topic's event types rather than adding to them,
//! while a call naming another topic leaves the first one alone — which is also what makes
//! the documented ceiling ("your maximum limit of SNS topics, which is 10 per Auto Scaling
//! group") a count of topics.
//!
//! # What the group is told about
//!
//! [`StoredGroup::notification_topics`] is the one reading of "which topics does this group
//! publish THIS event to", so the publisher of an instance launch and the publisher of an
//! instance termination select their topics by the same rule
//! ([`super::sns_notifications`]).

use std::collections::{BTreeMap, BTreeSet};

use super::StoredGroup;

/// The most Amazon SNS topics one Auto Scaling group may be configured to notify.
///
/// From the model's own documentation for `PutNotificationConfiguration`: "If you exceed
/// your maximum limit of SNS topics, which is 10 per Auto Scaling group, the call fails."
/// The member carries no modelled bound, so the handler checks it and answers the
/// operation's modelled `LimitExceededFault` past it.
pub const MAX_NOTIFICATION_TOPICS_PER_GROUP: usize = 10;

/// The notification configurations of ONE group: the event types it notifies about, keyed
/// by the `TopicARN` they are notified to.
///
/// A set rather than a list, because `DescribeNotificationConfigurations` reports one
/// configuration per (group, topic, event type) and a request that names the same type
/// twice configures one.
pub type NotificationConfigurations = BTreeMap<String, BTreeSet<String>>;

/// Why a notification configuration could not be stored, for the operation to answer the
/// error class each situation belongs to.
#[derive(Debug, PartialEq, Eq)]
pub enum NotificationConfigurationRefusal {
    /// The request names an event type this service does not notify about; carries what it
    /// named.
    UnknownNotificationType(String),
    /// The group is already configured with [`MAX_NOTIFICATION_TOPICS_PER_GROUP`] topics
    /// and this request names one more.
    LimitExceeded,
}

impl NotificationConfigurationRefusal {
    /// What the caller is told. ONE spelling per situation, so no two writers of a
    /// configuration refuse the same request differently.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownNotificationType(notification_type) => {
                unknown_notification_type_message(notification_type)
            }
            Self::LimitExceeded => too_many_topics_message(),
        }
    }
}

impl StoredGroup {
    /// Create or replace the configuration naming `topic_arn`, per the upsert rule in this
    /// module's own documentation.
    ///
    /// Every rule is checked BEFORE the first write, so a refused request leaves the
    /// group's configurations exactly as they were.
    pub fn put_notification_configuration(
        &mut self,
        topic_arn: &str,
        notification_types: &[String],
    ) -> Result<(), NotificationConfigurationRefusal> {
        if let Some(unknown) = notification_types
            .iter()
            .find(|candidate| !super::AUTO_SCALING_NOTIFICATION_TYPES.contains(&candidate.as_str()))
        {
            return Err(NotificationConfigurationRefusal::UnknownNotificationType(
                unknown.clone(),
            ));
        }
        let configures_a_new_topic = !self.notification_configurations.contains_key(topic_arn);
        if configures_a_new_topic
            && self.notification_configurations.len() >= MAX_NOTIFICATION_TOPICS_PER_GROUP
        {
            return Err(NotificationConfigurationRefusal::LimitExceeded);
        }
        self.notification_configurations.insert(
            topic_arn.to_string(),
            notification_types.iter().cloned().collect(),
        );
        Ok(())
    }

    /// The topics this group notifies about `notification_type`, in the order the
    /// collection holds them.
    ///
    /// A topic configured for other event types is not told about this one, which is what
    /// makes a configuration a selection rather than a subscription to everything.
    #[must_use]
    pub fn notification_topics(&self, notification_type: &str) -> Vec<String> {
        self.notification_configurations
            .iter()
            .filter(|(_, types)| types.contains(notification_type))
            .map(|(topic_arn, _)| topic_arn.clone())
            .collect()
    }
}

/// The message a request naming an event type outside this service's own catalog is
/// refused with, in the shape this service refuses every value outside one of its
/// catalogs: the member as the model names it, the value from the request, and the catalog
/// the caller has to choose from.
pub fn unknown_notification_type_message(notification_type: &str) -> String {
    format!(
        "The NotificationType value {notification_type} is not valid: it must be one of {}",
        super::stated_value_set(super::AUTO_SCALING_NOTIFICATION_TYPES.iter().copied())
    )
}

/// The message a request that would configure a topic past
/// [`MAX_NOTIFICATION_TOPICS_PER_GROUP`] is refused with, which quotes the limit the
/// caller has to come back under — the shape this service reports every per-group ceiling
/// in.
pub fn too_many_topics_message() -> String {
    format!(
        "The notification configuration exceeds the limit of \
         {MAX_NOTIFICATION_TOPICS_PER_GROUP} SNS topics per Auto Scaling group"
    )
}
