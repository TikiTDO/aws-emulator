// SPDX-License-Identifier: CC0-1.0

//! Whether the resource an event destination publishes to EXISTS, asked of the service that
//! OWNS it — the pre-flight every operation that STORES an event destination runs before it
//! persists anything.
//!
//! An event destination names "the Amazon Web Services service to which Amazon SES publishes
//! the email sending events associated with a configuration set"
//! (`CreateConfigurationSetEventDestination`), so the destination's target is another
//! service's resource and that service is the ONE answer to "is it there". A destination
//! whose target does not exist could never carry one event, so it is refused when it is
//! written rather than being accepted and then publishing every matching event into nothing —
//! and the refusal is the one the addressed member carries: `InvalidSNSDestination` for the
//! topic an `SNSDestination` names.
//!
//! The question is put the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]), exactly as the receipt
//! rule's destination pre-flight beside this one does ([`super::receipt_destinations`]): the
//! AWS SDK over the emulator's own in-process transport, so nothing here opens a socket or
//! leaves the host. The call made is a READ, so a refused write leaves the target exactly as
//! the caller left it.
//!
//! A refusal is GROUNDED in what the owning service stated: only that service's own "no such
//! resource" is evidence the target is missing. An answer that is not that service stating
//! the target's facts — an emulator with no Amazon SNS mounted beside this one, a transport
//! that carried nothing, any failure to ask — establishes nothing, so it refuses nothing.
//!
//! Nothing is cached: a topic created a moment ago resolves at once, and one that was deleted
//! stops resolving at once.
//!
//! This reads only what [`super::check_event_destination`] has already held to the published
//! form of the member that names it, so a value reaching here is an ARN of this account, this
//! partition and the Region the request is signed for.

/// How Amazon SES words the refusal of an `SNSDestination` whose topic Amazon SNS holds
/// nothing under: the topic is named, in angle brackets, as not found.
///
/// Everything but the ARN is FIXED text, and the one variable part is the
/// `SNSDestination.TopicARN` the request carried. The caller answers it with the modelled
/// `InvalidSNSDestinationException`, whose Query API code is `InvalidSNSDestination` and
/// whose status is 400.
#[must_use]
pub fn sns_topic_not_found_message(topic_arn: &str) -> String {
    format!("SNS topic <{topic_arn}> not found.")
}

/// Why the event destination `destination` names a target Amazon SES could not publish to,
/// and `None` when every target it names is there — or when nothing establishes otherwise.
///
/// `region` is the Region the REQUEST is signed for, which is the Region the destination and
/// its target belong to: no text of a request chooses which endpoint is asked.
///
/// Only the `SNSDestination` is resolved here, because Amazon SNS is the one owner of a
/// target this emulator can ask: the member is the ARN of a topic, and Amazon SNS states
/// whether it holds one ([`sns_topic_is_absent`]).
pub async fn unreachable_event_destination(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination: &crate::ops::types::EventDestination,
) -> Option<super::EventDestinationFault> {
    let topic_arn = destination.sns_destination.as_ref()?.topic_arn.trim();
    sns_topic_is_absent(ctx, region, topic_arn)
        .await
        .then(|| super::EventDestinationFault::Sns(sns_topic_not_found_message(topic_arn)))
}

/// Whether Amazon SNS ANSWERED that it holds no topic under the ARN `topic_arn`.
///
/// `false` for a topic it holds AND for a question it could not be asked, which is what makes
/// a refusal grounded in this answer one the owner of topics really gave. The answer is read
/// through the ONE reading of it this service makes
/// ([`super::receipt_destinations::topic_answer`]), so the two pre-flights cannot drift apart
/// on which Amazon SNS answers mean "no such topic".
async fn sns_topic_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
) -> bool {
    matches!(
        super::receipt_destinations::topic_answer(
            ctx,
            region,
            topic_arn,
            "the event destination's Amazon SNS topic",
        )
        .await,
        super::receipt_destinations::TopicAnswer::NoSuchTopic
    )
}
