// SPDX-License-Identifier: CC0-1.0

//! Where a notebook instance's lifecycle-configuration hook output GOES: the CloudWatch
//! Logs group and stream the platform publishes it under, and the publication itself.
//!
//! The hook shape's own doc names both: "View Amazon CloudWatch Logs for notebook instance
//! lifecycle configurations in log group `/aws/sagemaker/NotebookInstances` in log stream
//! `[notebook-instance-name]/[LifecycleConfigHook]`" — so a client reads what a hook wrote
//! through CloudWatch Logs' own API rather than off the notebook instance, and the group,
//! the stream and the events are that service's resources.
//!
//! What the hook wrote is computed by [`super::notebook_lifecycle_hook`]; this module only
//! delivers it. The delivery goes out over the in-process callback — the same seam this
//! service's other cross-service work uses (`super::s3_access::s3_client`) — and is BEST
//! EFFORT: an emulator with no CloudWatch Logs mounted beside SageMaker, or a write
//! CloudWatch Logs refuses, is reported in this service's own tracing rather than failing
//! the notebook-instance call, which asked for an instance and has it stored either way.
//! Nothing here deletes a group or a stream: the log records in it belong to the account.

use super::*;

/// Everything one hook publication needs, read while the store's lock is held so the
/// publication itself runs with none.
#[derive(Debug, Clone)]
pub struct HookLogPublication {
    /// The notebook instance the hook ran for, which names the log stream.
    pub notebook_instance_name: String,
    /// Which of the configuration's scripts ran, which names the rest of the stream.
    pub hook: LifecycleHook,
    /// The lines the script wrote, in the order it wrote them.
    pub lines: Vec<String>,
}

/// Publish what a hook wrote to the stream the hook shape documents for it: create the
/// notebook-instance log group if CloudWatch Logs does not hold it yet, open this hook's
/// stream, and deliver one log event per line the script wrote.
///
/// A hook that wrote nothing publishes nothing — there is no record to deliver, and a stream
/// opened for it would report output where there was none.
///
/// Best effort and never fatal — see this module's header. Call it with NO store guard
/// held: it awaits.
pub async fn publish_hook_output(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    publication: &HookLogPublication,
) {
    if publication.lines.is_empty() {
        return;
    }
    let logs = cloudwatch_logs_client(ctx, region);
    let log_stream = publication
        .hook
        .log_stream_name(&publication.notebook_instance_name);
    let created = logs
        .create_log_group()
        .log_group_name(NOTEBOOK_LOG_GROUP)
        .send()
        .await;
    match created {
        Ok(_) => {}
        // The group is already there — another notebook instance's hook has published
        // before, or the account created it itself — which is the state this step owes.
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                notebook_instance_name = %publication.notebook_instance_name,
                log_group = %NOTEBOOK_LOG_GROUP,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the notebook-instance log group could not be created, so this lifecycle \
                 hook's output is not published"
            );
            return;
        }
    }
    let opened = logs
        .create_log_stream()
        .log_group_name(NOTEBOOK_LOG_GROUP)
        .log_stream_name(&log_stream)
        .send()
        .await;
    match opened {
        Ok(_) => {}
        // The stream this hook already published under: a later run appends to it rather
        // than opening a second one.
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                notebook_instance_name = %publication.notebook_instance_name,
                log_group = %NOTEBOOK_LOG_GROUP,
                %log_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "this lifecycle hook's log stream could not be opened"
            );
            return;
        }
    }

    let wrote_at = epoch_millis(SystemTime::now());
    let events: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent> = publication
        .lines
        .iter()
        .filter_map(|line| {
            ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
                .timestamp(wrote_at)
                .message(line)
                .build()
                .ok()
        })
        .collect();
    if events.is_empty() {
        return;
    }
    let delivered = logs
        .put_log_events()
        .log_group_name(NOTEBOOK_LOG_GROUP)
        .log_stream_name(&log_stream)
        .set_log_events(Some(events))
        .send()
        .await;
    if let Err(error) = delivered {
        ::tracing::warn!(
            notebook_instance_name = %publication.notebook_instance_name,
            log_group = %NOTEBOOK_LOG_GROUP,
            %log_stream,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "this lifecycle hook's output could not be delivered"
        );
    }
}

/// `at` in milliseconds since the Unix epoch, which is the stamp a log event carries.
///
/// An instant before the epoch, which no log event can carry, clamps to the epoch rather
/// than wrapping.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The same seam, and the same reasoning, as this service's Amazon S3 client
/// ([`super::s3_access::s3_client`]): the callback routes in process, so the `endpoint_url`
/// is an ADDRESS the in-process router reads rather than a host any resolver looks up, no
/// socket is opened and this client reaches the emulator's own CloudWatch Logs and nothing
/// else. There is nothing to retry, time out or cache an identity for, each of which would
/// need a sleep implementation this client is not given.
fn cloudwatch_logs_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let region = ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION);
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}
