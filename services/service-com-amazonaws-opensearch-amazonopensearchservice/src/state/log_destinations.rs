// SPDX-License-Identifier: CC0-1.0

//! Whether the destination an enabled log-publishing option names is one the service can
//! really publish to — the ONE statement of that rule, so every call that takes a
//! `LogPublishingOptions` map judges it alike.
//!
//! A `LogPublishingOption` is "whether the log should be published" plus "the ARN of the
//! CloudWatch Logs group to publish logs to"
//! (`smithy_query(query="shape",shape="LogPublishingOption")`). Whether that group EXISTS
//! is not a constraint the model can carry — the member is a string — so it is checked
//! here: an option that is enabled against a log group nobody created describes a
//! delivery that can never happen, so the request is refused with the operation's
//! `ValidationException` and nothing is stored. [`super::log_publishing_refusal`] is the
//! step before it: an option that names no log group at all.
//!
//! The group is resolved in the CloudWatch Logs service's own store, reached through the
//! SAME shared engine this service's handlers reach their own store through — the store
//! `logs:CreateLogGroup` writes and `logs:DescribeLogGroups` reports — the way
//! [`super::cognito_authentication`] resolves a user pool. That store belongs to the
//! ENGINE rather than to a mounted service, so which services the emulator happens to
//! mount does not change what this check resolves.
//!
//! What a caller SEES for such a destination is the service's own sentence for a log group
//! it cannot resolve: the group is not there ([`nonexistent_destination`]). That sentence
//! carries no part of the request, because WHICH group is missing is not what it states —
//! it states that the destination the request named does not exist, which is the one thing
//! a caller has to act on.

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as cloudwatch_logs;

use crate::ops::types::{LogPublishingOption, ValidationException};

/// The suffix the WILDCARD form of a log group's ARN carries (`LogGroup$arn`, which
/// names the group's log streams); the plain form (`LogGroup$logGroupArn`) carries none.
/// A caller may hand over either, so the suffix is dropped before the ARN is resolved.
const LOG_GROUP_STREAMS_SUFFIX: &str = ":*";

/// The log group an option publishes to, when its `CloudWatchLogsLogGroupArn` names one:
/// the ARN without the wildcard suffix either form may carry, and the group's own name.
///
/// WHICH values name a log group, and WHICH part of one is the group's name, are the
/// CloudWatch Logs service's own readings of its own ARN — so they are read with its own
/// helpers rather than parsed a second time here, and the two crates cannot drift on what
/// counts as a log group's ARN.
///
/// `None` for a value that is not the ARN of a CloudWatch Logs log group — an option
/// carrying one of those names no destination at all, which is
/// [`super::log_publishing_refusal`]'s refusal rather than this module's.
///
/// `pub(crate)` because the DELIVERY into that group reads the destination the same way
/// ([`super::log_publishing`]): one reading of what an option's ARN names, so what the
/// create resolved is what a record is published to.
pub(crate) fn named_log_group(arn: &str) -> Option<(&str, &str)> {
    if !cloudwatch_logs::is_log_group_arn(arn) {
        return None;
    }
    let name = cloudwatch_logs::log_group_name_from_identifier(arn);
    let arn = arn.strip_suffix(LOG_GROUP_STREAMS_SUFFIX).unwrap_or(arn);
    // A group's name is what identifies it, so an ARN carrying none names no group.
    (!name.is_empty()).then_some((arn, name))
}

/// Whether `arn` names a CloudWatch Logs log group AT ALL — the shape half of the rule,
/// so the one place that reads a destination ARN is this module.
///
/// A value that is not a log group's ARN names no destination, which is the refusal
/// [`super::log_publishing_refusal`] answers an option publishing to nowhere with.
pub fn names_a_log_group(arn: &str) -> bool {
    named_log_group(arn).is_some()
}

/// What the service answers an enabled option whose destination does not exist.
///
/// A log group nobody created is no destination at all, and the service says so in a fixed
/// sentence: the refusal states the CONDITION that held and carries no value of the request
/// — not the group's name, not the log type — so it reads the same for every destination
/// that resolves to nothing. It is the refusal for a group this account and Region does not
/// hold, whichever field of the ARN it is that misses them.
pub fn nonexistent_destination() -> ValidationException {
    ValidationException {
        message: Some("The specified CloudWatch Logs log group doesn't exist.".to_string()),
    }
}

/// Whether every enabled option in `options` publishes to a log group that EXISTS, and
/// the refusal for the first destination that does not otherwise
/// ([`nonexistent_destination`]).
///
/// The log types are visited in order, so one request with two unresolvable destinations
/// is refused with the same sentence however the map was iterated. A DISABLED option
/// publishes nothing, so the destination it carries is not resolved — nothing would be
/// delivered to it.
///
/// `None` when the engine hands out no CloudWatch Logs store: a caller that cannot reach
/// the store cannot tell a missing group from a missing store, and refusing a
/// configuration on the strength of a store that was never readable would reject a
/// destination that does exist.
pub fn log_publishing_destination_refusal<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    options: impl IntoIterator<Item = (&'a String, &'a LogPublishingOption)>,
) -> Option<ValidationException> {
    let mut enabled: Vec<(&str, &str)> = options
        .into_iter()
        .filter(|(_, option)| option.enabled.unwrap_or(false))
        .filter_map(|(log_type, option)| {
            let arn = option.cloud_watch_logs_log_group_arn.as_deref()?;
            Some((log_type.as_str(), arn))
        })
        .collect();
    if enabled.is_empty() {
        return None;
    }
    enabled.sort_unstable();
    let store = cloudwatch_logs::service_state(engine)?;
    let logs = store.lock();
    for (_, arn) in enabled {
        // A destination that is not a log group's ARN is the other refusal's
        // ([`names_a_log_group`]), so it is passed over here rather than reported as a
        // group that does not exist.
        let Some((arn, name)) = named_log_group(arn) else {
            continue;
        };
        // The group is resolved by the name it is stored under, and the record's own ARN
        // is what says WHICH group that is: a destination in another account, another
        // partition or another Region names a group this account does not hold here,
        // whatever a group of that name is called locally.
        if logs
            .log_groups
            .get(name)
            .is_none_or(|group| group.log_group_arn != arn)
        {
            return Some(nonexistent_destination());
        }
    }
    None
}
