// SPDX-License-Identifier: CC0-1.0

//! Resolving the log destination a `DestinationConfiguration` names, against the service
//! that OWNS it.
//!
//! Every member of the modelled union names a resource of ANOTHER service — "Name of the
//! Amazon S3 bucket where chat activity will be logged" (`S3DestinationConfiguration`),
//! "Name of the Amazon Cloudwatch Logs destination where chat activity will be logged"
//! (`CloudWatchLogsDestinationConfiguration`), "Name of the Amazon Kinesis Firehose
//! delivery stream where chat activity will be logged"
//! (`FirehoseDestinationConfiguration`) — and chat content cannot be logged to a resource
//! that is not there. So a write that NAMES one resolves it where the owning service holds
//! it, BEFORE anything of this service's own is minted or stored, and a request naming a
//! destination that service reports absent is refused with the modelled
//! `ResourceNotFoundException` (HTTP 404) instead of leaving behind a configuration that
//! can never deliver.
//!
//! Each owning service is reached the way the framework documents a handler reaching
//! another emulated service ([`smithy_server_runtime::OperationContext::callback`]): an AWS
//! SDK client over the in-process callback's HTTP client and credentials, so nothing here
//! opens a socket, leaves the process, or names another service crate. The question is put
//! with that service's own read, and only that service's ANSWER refuses anything: a read
//! that reports no such resource is what refuses the request, while every other outcome —
//! an emulator assembled without that service to ask, or any other failure to ask it —
//! establishes nothing about the resource and so refuses nothing. This gate keeps nothing
//! of the destination but the name the request named, so a refusal on silence would be
//! assumed rather than grounded (`services/common/src/network.rs` states which resolution
//! is which).
//!
//! The one caller-supplied value that reaches a client's authority is the Region, which
//! [`super::request_region`] has already narrowed to the Region grammar the modelled ARN
//! patterns admit, so no request can re-point one of these clients at a host of its own
//! choosing.

use super::{resource_not_found, DestinationConfiguration, ResourceNotFoundException};

/// The endpoint prefix of each service that owns a log destination, used to build the
/// in-process client's endpoint.
const S3_ENDPOINT_PREFIX: &str = "s3";
const LOGS_ENDPOINT_PREFIX: &str = "logs";
const FIREHOSE_ENDPOINT_PREFIX: &str = "firehose";

/// Refuse a destination the service that owns it reports absent — and only then.
///
/// Call it with NO guard of this service's store held: it awaits.
///
/// `None` means the create or update may proceed. The refusal carries the message rule
/// every not-found refusal of this service follows ([`resource_not_found`]), naming the
/// destination resource the request named, which is the value the caller has to fix.
pub async fn unresolvable_destination(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination: &DestinationConfiguration,
) -> Option<ResourceNotFoundException> {
    let absent = match destination {
        DestinationConfiguration::S3(s3) => {
            bucket_is_absent(ctx, region, &s3.bucket_name).await
        }
        DestinationConfiguration::cloudWatchLogs(logs) => {
            log_group_is_absent(ctx, region, &logs.log_group_name).await
        }
        DestinationConfiguration::Firehose(firehose) => {
            delivery_stream_is_absent(ctx, region, &firehose.delivery_stream_name).await
        }
    };
    absent.then(|| resource_not_found(destination_name(destination)))
}

/// The name a destination gives the resource it logs to: whichever member of the union the
/// request carries. One reading, so the refusal below and any diagnostic name the same
/// thing.
pub fn destination_name(destination: &DestinationConfiguration) -> &str {
    match destination {
        DestinationConfiguration::S3(s3) => &s3.bucket_name,
        DestinationConfiguration::cloudWatchLogs(logs) => &logs.log_group_name,
        DestinationConfiguration::Firehose(firehose) => &firehose.delivery_stream_name,
    }
}

/// Whether Amazon S3 reports holding no bucket under `name`.
///
/// Asked with `HeadBucket`, the operation S3 documents for exactly this ("You can use this
/// operation to determine if a bucket exists"): its `NotFound` answer is S3 saying the
/// bucket is not there.
async fn bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> bool {
    let answer = s3_client(ctx, region).head_bucket().bucket(name).send().await;
    match answer {
        Ok(_) => false,
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found,
            ) =>
        {
            true
        }
        Err(error) => {
            unresolved(name, "Amazon S3 bucket", &error);
            false
        }
    }
}

/// Whether CloudWatch Logs reports holding no log group under `name`.
///
/// Asked with `DescribeLogGroups`, whose `logGroupIdentifiers` names groups "by name or
/// ARN": an answer carrying no group of exactly that name is CloudWatch Logs saying the
/// group is not there.
async fn log_group_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> bool {
    let answer = logs_client(ctx, region)
        .describe_log_groups()
        .log_group_identifiers(name)
        .send()
        .await;
    match answer {
        Ok(answer) => !answer
            .log_groups()
            .iter()
            .any(|group| group.log_group_name() == Some(name)),
        Err(error) => {
            unresolved(name, "Amazon CloudWatch Logs log group", &error);
            false
        }
    }
}

/// Whether Amazon Kinesis Data Firehose reports holding no delivery stream under `name`.
///
/// Asked with `DescribeDeliveryStream`, whose own `ResourceNotFoundException` is Firehose
/// saying the stream is not there.
async fn delivery_stream_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> bool {
    let answer = firehose_client(ctx, region)
        .describe_delivery_stream()
        .delivery_stream_name(name)
        .send()
        .await;
    match answer {
        Ok(_) => false,
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_firehose::operation::describe_delivery_stream::DescribeDeliveryStreamError
                    ::is_resource_not_found_exception,
            ) =>
        {
            true
        }
        Err(error) => {
            unresolved(name, "Amazon Kinesis Data Firehose delivery stream", &error);
            false
        }
    }
}

/// Trace a destination this emulator could not ask the owning service about, which is a
/// deployment with no such service mounted rather than a destination reported missing.
///
/// The caller's own value is rendered in its escaped `Debug` form, so a name carrying a
/// newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(name: &str, destination: &str, error: &E) {
    ::tracing::warn!(
        destination_name = ?name,
        destination = %destination,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a logging configuration's destination could not be resolved against the service \
         that holds it"
    );
}

/// Amazon S3's client for `region`, over the in-process callback into this same emulator.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each
/// need a sleep implementation the callback transport is not given, and there is nothing to
/// retry behind it anyway.
///
/// Path style, because the emulator serves one endpoint for every service: the bucket
/// travels in the path rather than in a per-bucket host the router could not resolve.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(endpoint(S3_ENDPOINT_PREFIX, region))
        .force_path_style(true)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// CloudWatch Logs' client for `region`, over the same in-process callback and under the
/// same client policy as [`s3_client`].
fn logs_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(endpoint(LOGS_ENDPOINT_PREFIX, region))
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Amazon Kinesis Data Firehose's client for `region`, over the same in-process callback
/// and under the same client policy as [`s3_client`].
fn firehose_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(endpoint(FIREHOSE_ENDPOINT_PREFIX, region))
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}

/// The endpoint a service's client is pointed at. The Region is already narrowed to the
/// Region grammar by [`super::request_region`], so nothing of the request's own text
/// reaches the authority.
fn endpoint(prefix: &str, region: &str) -> String {
    format!("https://{prefix}.{region}.amazonaws.com")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{
        CloudWatchLogsDestinationConfiguration, FirehoseDestinationConfiguration,
        S3DestinationConfiguration,
    };

    /// Which name a refusal reports, and the `resourceId` it echoes it under, for each
    /// member of the modelled union.
    #[test]
    fn a_refusal_names_the_destination_the_request_named() {
        // (case, the destination, the name the refusal reports)
        let cases: Vec<(&str, DestinationConfiguration, &str)> = vec![
            (
                "an s3 bucket whose name carries dashes",
                DestinationConfiguration::S3(S3DestinationConfiguration {
                    bucket_name: "chat-logs-bucket".to_string(),
                }),
                "chat-logs-bucket",
            ),
            (
                "an s3 bucket whose name is alphanumeric throughout",
                DestinationConfiguration::S3(S3DestinationConfiguration {
                    bucket_name: "chatlogs2026".to_string(),
                }),
                "chatlogs2026",
            ),
            (
                "a cloudWatchLogs log group",
                DestinationConfiguration::cloudWatchLogs(
                    CloudWatchLogsDestinationConfiguration {
                        log_group_name: "/aws/ivschat/room-logs".to_string(),
                    },
                ),
                "/aws/ivschat/room-logs",
            ),
            (
                "a firehose delivery stream",
                DestinationConfiguration::Firehose(FirehoseDestinationConfiguration {
                    delivery_stream_name: "chat.logs-stream".to_string(),
                }),
                "chat.logs-stream",
            ),
        ];
        for (case, destination, name) in cases {
            let actual = destination_name(&destination);
            assert_eq!(
                actual, name,
                "case {case:?}: expected the destination name {name:?}, got {actual:?}"
            );
            let refusal = resource_not_found(actual);
            let expected_message = format!("Resource: {name} not found");
            assert_eq!(
                refusal.message, expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {:?}",
                refusal.message
            );
            assert_eq!(
                refusal.resource_id, name,
                "case {case:?}: expected the identifier echoed as resourceId, got {:?}",
                refusal.resource_id
            );
        }
    }
}
