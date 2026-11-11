// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS topic an event subscription DELIVERS to, resolved against the service that
//! OWNS topics.
//!
//! A topic is not a resource of this service: it is Amazon SNS's. "This operation requires a
//! topic Amazon Resource Name (ARN) created by either the RDS console, the SNS console, or
//! the SNS API" (`CreateEventSubscriptionMessage$SnsTopicArn`), so Amazon SNS is the ONE
//! answer to "does this topic exist", and the ARN a request names is resolved there and
//! nowhere else — reached the way this service reaches any other, through the in-process
//! callback ([`smithy_server_runtime::OperationContext::callback`]), so nothing leaves the
//! process.
//!
//! An ARN Amazon SNS ANSWERED it holds no topic for names no topic of the account, and a
//! subscription against it is refused: the topic is the subscription's delivery target, so a
//! subscription on an ARN nobody minted could never deliver the event it was created for.
//!
//! A hop that could not be made at all (no Amazon SNS in this process, an ARN that names no
//! Region to address, an answer that could not be read) is evidence of nothing: the topic
//! then stays UNKNOWN and nothing is refused on silence, exactly as the subnets a DB subnet
//! group names are treated when the service that owns them cannot be reached
//! ([`super::ec2_subnets`]).
//!
//! Nothing is cached: a topic created a moment ago resolves at once, and one that was deleted
//! stops resolving at once.

/// Whether Amazon SNS ANSWERED that it holds no topic under the ARN `topic_arn`.
///
/// `false` for a topic it holds AND for a question it could not be asked, which is what makes
/// a refusal grounded in this answer one the owner of topics really gave.
///
/// The question is put in `region`, the Region the request was signed for: an event
/// subscription and the topic it delivers to belong to the same Region, so that is the
/// endpoint that serves the topic. A topic ARN naming ANOTHER Region is not asked about at
/// all — this endpoint serves no topic of another Region, so its answer would say nothing
/// about whether that topic exists.
pub async fn sns_topic_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
) -> bool {
    if topic_region(topic_arn) != Some(region) {
        return false;
    }
    let Some(client) = sns(ctx, region) else {
        return false;
    };
    let answered = client
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await;
    match answered {
        Ok(_) => false,
        Err(error) => {
            // Only the owner's own "no such topic" is evidence the topic is missing. Any
            // other failure — Amazon SNS not mounted in this process, a response that could
            // not be read — says nothing about the topic, so nothing is refused for it.
            let absent = error.as_service_error().is_some_and(
                ::aws_sdk_sns::operation::get_topic_attributes::GetTopicAttributesError::is_not_found_exception,
            );
            if !absent {
                // At `debug`: in an emulator carrying no Amazon SNS this is the normal
                // answer, and it changes nothing a caller sees.
                ::tracing::debug!(
                    topic = %topic_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the topic a subscription names could not be resolved against Amazon SNS"
                );
            }
            absent
        }
    }
}

/// How Amazon RDS refuses an event subscription whose delivery topic does not exist: it says
/// the subscription could not be created because the topic was not found, and names the topic
/// by the ARN the request carried.
///
/// Everything but that ARN is fixed text. The caller answers it with the modelled
/// `SNSTopicArnNotFoundFault`, whose Query API code is `SNSTopicArnNotFound` and whose status
/// is 404.
pub fn sns_topic_not_found_message(topic_arn: &str) -> String {
    format!("Failed to create Subscription because of Topic {topic_arn} Not Found.")
}

/// The Region the topic ARN `topic_arn` names, as a Region LABEL.
///
/// Read so a cross-Region topic can be told apart from one of this Region: the ARN is
/// caller-supplied text, held to the published Region-id grammar
/// (`service_common::arn::region_label`, the one place this tree states it), and a value
/// outside that grammar names no Region at all.
fn topic_region(topic_arn: &str) -> Option<&str> {
    let region = topic_arn.split(':').nth(3)?;
    ::service_common::arn::region_label(region)
}

/// An AWS SDK client for Amazon SNS over the in-process callback into this same emulator,
/// addressed in `region`.
///
/// The endpoint URL is only the address the callback's transport routes on IN PROCESS: no
/// socket is opened, no name is resolved and nothing leaves this process. The credentials are
/// the callback's, which sign this in-process hop and never appear in a response. Everything
/// else mirrors the EC2 client beside it ([`super::ec2_subnets`]): no retries, no timeouts and
/// no identity cache, none of which an in-process hop has anything to do.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar) instead
/// of being interpolated as given: `None` for a value that would name an authority the caller
/// chose rather than the peer service this emulator asks.
fn sns(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_sns::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_sns::Client::from_conf(config))
}
