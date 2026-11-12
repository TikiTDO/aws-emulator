// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs group a server's logging configuration publishes to, and how a
//! server write provisions it.
//!
//! A server configured with a `LoggingRole` and no structured destination publishes its
//! activity to the SERVICE-managed log group, whose name is the service's own logging
//! prefix followed by the server's identifier ([`vended_log_group_name`]) — the
//! predictable place a client reads a server's logs, and the destination the metric
//! filters and alarms built on those logs name. So that name has to name a log group
//! that EXISTS: a client which reads a server's logs (`logs:DescribeLogGroups`,
//! `logs:FilterLogEvents`), sets retention on the group or puts a metric filter on it
//! must find it there. The group is therefore created by the write that configures the
//! role, by calling the emulator's own CloudWatch Logs service over the in-process
//! callback (`logs:CreateLogGroup`), exactly as `service-com-amazonaws-rum-rum` does for
//! its own vended group.
//!
//! Three consequences, each deliberate:
//!
//! * the group belongs to the ACCOUNT, so it is created once and left alone afterwards:
//!   provisioning the same name again is a success that changes nothing, and clearing
//!   the role or deleting the server does not delete the logs already in it;
//! * a router that mounts this service WITHOUT CloudWatch Logs cannot provision it. The
//!   server itself is unaffected — nothing this service reports about a server names the
//!   group — so the server write still succeeds and the failure is reported in this
//!   service's own tracing, rather than refusing a `CreateServer` the caller asked for
//!   over a managed side effect of its logging configuration;
//! * it runs AFTER the server is stored, since the group's name is the identifier the
//!   store minted, and a refused write therefore provisions nothing.

/// The prefix the service-managed log group of a server carries: the group a server
/// configured with a `LoggingRole` alone publishes to is this prefix followed by the
/// server's own `ServerId`.
pub const VENDED_LOG_GROUP_PREFIX: &str = "/aws/transfer/";

/// The name of the service-managed log group `server_id`'s activity is published to.
///
/// `ServerId` is `len 19..19`, pattern `^s-([0-9a-f]{17})$`, so the name is within
/// CloudWatch Logs' own `LogGroupName` (1..512 characters matching
/// `^[\.\-_/#A-Za-z0-9]+$`) for every server identifier this service mints.
pub fn vended_log_group_name(server_id: &str) -> String {
    format!("{VENDED_LOG_GROUP_PREFIX}{server_id}")
}

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and nothing leaves the
/// process. `region` is the request's own signing region, so the group lands in the
/// Region the server itself is in.
fn logs_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Create the service-managed log group of the server `server_id` in `region`, so the
/// name a client builds from the server's identifier names a group CloudWatch Logs
/// finds.
///
/// Called by the server write that leaves the server with a `LoggingRole` configured.
/// The group already existing is the same outcome as this call creating it — a group
/// under that name is exactly what the step owes — so both are `Ok`. Everything else is
/// TRACED and the server write goes on: the group is a managed side effect of the
/// server's logging configuration, not part of the answer the caller asked for.
pub async fn provision(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    region: &str,
    server_id: &str,
) {
    let log_group = vended_log_group_name(server_id);
    match logs_client(ctx, region)
        .create_log_group()
        .log_group_name(&log_group)
        .send()
        .await
    {
        Ok(_) => {}
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                operation = operation,
                server_id = server_id,
                log_group = %log_group,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the server's service-managed log group could not be provisioned"
            );
        }
    }
}
