// SPDX-License-Identifier: CC0-1.0

//! The Amazon Kinesis Data Firehose destination of an event destination: the role this
//! service sends a set's events to the stream UNDER, and the record each event is put on
//! that stream as.
//!
//! `KinesisFirehoseDestination$IamRoleArn` is "the ARN of the IAM role that the Amazon SES
//! API v2 uses to send email events to the Amazon Kinesis Data Firehose stream" — this
//! service does not reach the stream as its owner, it ACTS AS that role. A role is one
//! this service can act as only where the role's own trust policy admits the assume for
//! this service's principal ([`super::SES_SERVICE_PRINCIPAL`]), so a destination naming a
//! role that withholds it is a destination that could never publish anything, and the
//! request that would establish it is refused with the operation's modelled input error
//! instead.
//!
//! The question is put to the service that HOLDS the role records, over the in-process
//! callback, and the trust document it answers with is read by the tree's one reading of
//! an IAM trust policy ([`::service_common::iam_role::held_role_is_unassumable`]), so this
//! crate neither keeps IAM records of its own nor re-derives what a trust policy says. A
//! role this emulator's IAM holds nothing under refuses nothing: an event-publishing role
//! is routinely provisioned out of band, so its absence is evidence about this store and
//! not about the role the caller named.
//!
//! What is PUT on the stream, once a destination is established, is the same notification
//! document every other destination kind of that destination receives
//! ([`super::notification`]), as one record per event: Firehose "buffers records before
//! delivering them to the destination" (`PutRecord`), and it is the stream's own
//! destination configuration — not this service — that decides where those records then
//! land. The record carries the document and a newline, so a consumer reading a delivered
//! buffer reads one JSON document per event however many records that buffer was flushed
//! with.

/// The ARN namespace of Amazon Kinesis Data Firehose, which the Region of the stream a
/// destination names is read out of.
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";

/// The resource type a Firehose stream ARN names the stream under
/// (`arn:<partition>:firehose:<region>:<account>:deliverystream/<name>`), which is what
/// the stream's own name — the name its ingestion API addresses it by — is read out of.
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The authorization failure the Firehose destination `definition` carries, or `None` when
/// it carries none — including a definition naming no Firehose destination at all.
///
/// Asked by every operation that establishes an event destination, through
/// [`super::destination_refusal`], so a create of a destination and an update of the same
/// destination cannot accept different roles.
pub async fn role_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &crate::ops::types::EventDestinationDefinition,
) -> Option<String> {
    let role_arn = definition
        .kinesis_firehose_destination
        .as_ref()?
        .iam_role_arn
        .as_str();
    let region = super::super::CallerScope::of(ctx.parts()).region;
    let withheld = ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        &region,
        role_arn,
        super::SES_SERVICE_PRINCIPAL,
    )
    .await;
    withheld.then(|| unassumable_role(role_arn))
}

/// How this service words the refusal of a destination whose role it cannot act as: the
/// fixed sentence SES answers for a role it could not assume, carrying the role the
/// destination named — the only variable part, taken from the request — in the angle
/// brackets this service names a resource inside ([`super::publish_denied`]).
pub fn unassumable_role(role_arn: &str) -> String {
    format!("Could not assume IAM role <{role_arn}>.")
}

/// Put `document` on the Firehose stream `destination` names, as one record.
///
/// A stream this call cannot reach — one of another account, one nothing created, or one
/// whose ARN names no Region or no stream — takes nothing, which is logged and changes
/// nothing about the send: the message was accepted before any event of it existed.
pub async fn put_event(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    destination: &crate::ops::types::KinesisFirehoseDestination,
    document: &::serde_json::Value,
) {
    let stream_arn = destination.delivery_stream_arn.as_str();
    let Some(region) = ::service_common::arn::region_of(stream_arn, FIREHOSE_ARN_NAMESPACE) else {
        ::tracing::debug!(
            stream_arn,
            "an event destination's delivery stream ARN names no Region, so no event is put on it"
        );
        return;
    };
    let Some(stream_name) =
        ::service_common::arn::resource_id_of_type(stream_arn, DELIVERY_STREAM_RESOURCE_TYPE)
            .filter(|name| !name.is_empty())
    else {
        ::tracing::debug!(
            stream_arn,
            "an event destination's delivery stream ARN names no stream, so no event is put on it"
        );
        return;
    };
    // One record per event, each a JSON document on its own line.
    let record = ::aws_sdk_firehose::types::Record::builder()
        .data(::aws_sdk_firehose::primitives::Blob::new(
            format!("{document}\n").into_bytes(),
        ))
        .build();
    let put = match record {
        Ok(record) => firehose_client(callback, region)
            .put_record()
            .delivery_stream_name(stream_name)
            .record(record)
            .send()
            .await
            .err()
            .map(|error| {
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
            }),
        Err(error) => Some(
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string(),
        ),
    };
    if let Some(failure) = put {
        ::tracing::debug!(
            stream_arn,
            error = %failure,
            "an event destination's delivery stream did not take a published email event"
        );
    }
}

/// A Firehose client over the IN-PROCESS callback into this same emulator, built the way
/// this module's sibling builds its own ([`super::sns_client`]): the `endpoint_url` is an
/// address the in-process router reads rather than a host any resolver looks up, and the
/// credentials are the emulator's own root credentials, which only sign this operation's
/// in-process calls and are never put in a response or a log.
fn firehose_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{FIREHOSE_ARN_NAMESPACE}.{region}.amazonaws.com"
        ))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given.
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}
