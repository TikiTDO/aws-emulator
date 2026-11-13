// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs side of an app monitor's `DataStorage.CwLog`: the permission a
//! caller needs to turn log delivery on, and the vended log group RUM provisions when it
//! is on.
//!
//! Two things happen when a `CreateAppMonitor` or `UpdateAppMonitor` request turns
//! `CwLogEnabled` on, and both are RUM's side of a MANAGED side effect — neither is the
//! client's call to make:
//!
//! 1. **the caller must hold `logs:PutResourcePolicy`.** RUM copies the monitor's
//!    telemetry into a log group in the caller's own account, which means installing a
//!    resource policy on that log group on the caller's behalf, so a caller that may not
//!    install one may not turn delivery on either. The check is an identity-based
//!    decision in the CloudWatch Logs namespace ([`authorization::LOGS_PUT_RESOURCE_POLICY`])
//!    against the log group's ARN, and it is made BEFORE anything is written: a refused
//!    request leaves `CwLogEnabled` exactly as it was. The refusal is RUM's own sentence
//!    ([`delivery_denied`]) from every handler that can turn delivery on, since the caller
//!    asked for an app-monitor write and never named the CloudWatch Logs action RUM would
//!    exercise for it.
//! 2. **the log group is created.** `GetAppMonitor` reports
//!    `DataStorage.CwLog.CwLogGroup` for a monitor with delivery on, and that name has to
//!    name a REAL CloudWatch Logs log group — a client that reads it and then tails it,
//!    sets retention on it or reads its policy must find it there. So RUM creates it, by
//!    calling the emulator's own CloudWatch Logs service over the in-process callback
//!    (`logs:CreateLogGroup`), exactly as the managed service does for a vended-logs
//!    delivery.
//!
//! Two consequences of provisioning that group, both deliberate and both the shape b2bi's
//! `CreateProfile` already has for its own log group:
//!
//! * a router that mounts RUM WITHOUT the CloudWatch Logs service cannot provision it, so
//!   a request that turns delivery on answers `InternalServerException` there rather than
//!   advertising a `CwLogGroup` that resolves nowhere. The assembled emulator is not such
//!   a router — `core/router`'s `build_router` registers every service crate in the
//!   workspace — and this crate's own tests mount the pair explicitly;
//! * the group is created before the record is written, so a call whose future is dropped
//!   at that `.await` can leave the group behind. It is created under the name RUM vended
//!   for that monitor ([`super::AppMonitorRecord::vended_log_group`]), so the next call
//!   that turns delivery on for the same monitor adopts that very group rather than
//!   colliding with it. A create this service REFUSES leaves none: every refusal a create
//!   reads off the store is decided, and the name and the monitor slot it grants are HELD
//!   ([`super::hold_app_monitor_name`]), before this step runs — so the group is
//!   provisioned only for a create nothing can still refuse.
//!
//! Turning delivery OFF needs neither: no policy is installed, and the group is left
//! alone, because the telemetry already in it belongs to the account (the same reason
//! b2bi's `DeleteProfile` leaves its group behind) and nothing documents RUM deleting a
//! customer's logs.
//!
//! # The copies themselves
//!
//! Provisioning the group is only half of the contract: `CwLogEnabled` is "whether RUM
//! sends a copy of this telemetry data to Amazon CloudWatch Logs in your account" and
//! `CwLog$CwLogGroup` is "the name of the log group where the copies are stored", so a
//! batch delivered by `PutRumEvents` to a monitor with delivery ON has to REACH that group
//! — a client that reads the advertised name and then tails it
//! (`logs:DescribeLogStreams`, `logs:GetLogEvents`, `logs:FilterLogEvents`) finds one log
//! event per delivered RUM event. Storing the batch in this service's own buffer and
//! advertising a group name is not the behaviour; the group has to receive something
//! ([`deliver_events`]).
//!
//! Three choices the copy's shape rests on, because the model describes the copy only as
//! "a copy of this telemetry data" and publishes no log schema for it:
//!
//! * the MESSAGE is one log event per delivered RUM event, carrying exactly the RUM event
//!   document the data plane enriched it into ([`super::rum_event::event_document`]) — the
//!   same bytes the monitor's buffer holds and `GetAppMonitorData` reports, since the copy
//!   is a copy and not a second rendering of the same telemetry;
//! * the STREAM is the user session the batch came from ([`delivery_stream_name`]), since
//!   "each PutRumEvents operation can send a batch of events from one user session", so one
//!   session's copies accumulate in one stream across the batches it sends;
//! * the log event's TIMESTAMP is the instant RUM delivered the copy, not the instant the
//!   client says the event occurred. `RumEvent$timestamp` is "the exact time that this
//!   event occurred" as measured by the sending application's own clock, which is neither
//!   this emulator's nor CloudWatch Logs': an event stamped with it would be dropped by
//!   CloudWatch Logs' ingestion window whenever the sender's clock is skewed or the batch
//!   is replayed, and the copy would silently not exist. The event's own instant is carried
//!   verbatim in the message, so a reader still sees when the event occurred.
//!
//! Delivery is BEST EFFORT, as it is for every vended-logs destination: a group the
//! account has since deleted, or a write CloudWatch Logs refuses, loses the copies and is
//! reported in this service's own tracing rather than failing the `PutRumEvents` call,
//! which asked for none of this and whose own buffer already holds the batch.

use super::{authorization, StoredRumEvent};
use crate::ops::types::AccessDeniedException;

/// The most bytes ONE `PutLogEvents` batch may carry, from that operation's documented
/// quota: "The maximum batch size is 1,048,576 bytes."
const MAX_LOG_EVENT_BATCH_BYTES: usize = 1_048_576;

/// What that quota charges per event beside its message: the batch size "is calculated as
/// the sum of all event messages in UTF-8, plus 26 bytes for each log event".
const LOG_EVENT_OVERHEAD_BYTES: usize = 26;

/// The most events ONE `PutLogEvents` batch may carry, from the same quota: "The maximum
/// number of log events in a batch is 10,000."
const MAX_LOG_EVENTS_PER_BATCH: usize = 10_000;

/// The most bytes ONE log event's message may carry, from the same quota: "Each log event
/// can be no larger than 1 MB." A copy past it is one CloudWatch Logs refuses, so it is
/// left out rather than taking the whole batch down with it.
const MAX_LOG_EVENT_BYTES: usize = 1_048_576;

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. `region` is the
/// request's own signing region, so the group lands where the monitor's ARN says it does.
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
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Whether the caller may have RUM turn log delivery on: does it hold
/// `logs:PutResourcePolicy` on the log group RUM would install the delivery policy on?
///
/// Identity-based only: the resource lives in CloudWatch Logs, so there is no RUM resource
/// policy to consult for it.
/// `log_group` is the name RUM vended for the monitor the call names
/// ([`super::AppMonitorRecord::vended_log_group`]) — the group the policy would be
/// installed on, and so the resource the decision is made about.
pub fn caller_may_enable_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
) -> Result<(), authorization::Denied> {
    authorization::authorize_identity(
        engine,
        ctx,
        authorization::LOGS_PUT_RESOURCE_POLICY,
        &super::cw_log_group_arn(region, log_group),
    )
}

/// What the refused call was doing to the app monitor, for [`delivery_denied`]: one
/// variant per handler that can turn `CwLogEnabled` on, so the vocabulary the refusal is
/// worded in is closed rather than a string a later handler could spell any way it liked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// `CreateAppMonitor` was asked to create the monitor with delivery already on. The
    /// gerund is the create side of the sentence the sibling's refusal is pinned to: the
    /// crate has the update wording on record and words this one the same way, since one
    /// check cannot be reported in two vocabularies.
    Creating,
    /// `UpdateAppMonitor` was asked to turn delivery on for an existing monitor.
    Updating,
}

impl Doing {
    /// The gerund RUM's sentence carries for this call.
    fn gerund(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Updating => "updating",
        }
    }
}

/// RUM's own wording for refusing a call because the caller may not have RUM install the
/// vended log group's delivery policy ([`caller_may_enable_delivery`]).
///
/// The IAM-style projection (`crate::state::access_denied`) reports a refusal of the
/// action the CALLER itself asked for, naming the principal, that action and its resource.
/// This precondition is different in kind: the caller asked for an app-monitor write and
/// never named the CloudWatch Logs action RUM would exercise on its behalf, so RUM reports
/// the operation it could not carry out, naming the app monitor the request named. One
/// condition, decided by one check, is therefore reported in ONE vocabulary by every
/// handler that can turn delivery on.
pub fn delivery_denied(doing: Doing, app_monitor_name: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "Error {} the application {app_monitor_name}",
            doing.gerund()
        ),
    }
}

/// Create the vended log group `log_group`, so the `CwLogGroup` RUM reports names a group
/// `logs:DescribeLogGroups` finds.
///
/// `log_group` is the name RUM vended for this monitor and holds on its record
/// ([`super::AppMonitorRecord::vended_log_group`]), never a name rebuilt from the request:
/// the group that exists is the group the read advertises.
///
/// `Ok(())` once the group exists — whether this call created it or it was already there,
/// since a group under that name is exactly what this step owes the caller. `Err` carries
/// what CloudWatch Logs said, for the modelled server error the operation answers with.
pub async fn provision_log_group(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
) -> Result<(), String> {
    match logs_client(ctx, region)
        .create_log_group()
        .log_group_name(log_group)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) =>
        {
            Ok(())
        }
        Err(error) => Err(
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string(),
        ),
    }
}

/// The log stream one batch's copies are written to: the user session the batch was
/// collected from, and the app monitor's own id for a batch that named no session.
///
/// `UserDetails$sessionId` is "the session ID that the performance events are from" and
/// one `PutRumEvents` call "can send a batch of events from one user session", so the
/// session is what groups copies into a stream: the batches of one session accumulate in
/// one stream, and two sessions never interleave in one. Both values are 36-character
/// identifiers in the model, so neither carries a character a CloudWatch Logs stream name
/// may not (`:` or `*`).
pub fn delivery_stream_name(session_id: Option<&str>, app_monitor_id: &str) -> String {
    session_id
        .filter(|session| !session.is_empty())
        .unwrap_or(app_monitor_id)
        .to_string()
}

/// Write a copy of every event of one delivered batch into `log_group`, in the stream
/// `log_stream` ([`delivery_stream_name`]).
///
/// The stream is created first (a stream that is already there is the one this session has
/// been writing to), and the copies go out in batches within `PutLogEvents`' documented
/// quotas, so a large batch delivers all of its copies instead of having one oversized
/// request refused. Every copy carries the ONE delivery instant, which is both why no
/// reordering is needed — a batch must be in chronological order — and what keeps the copy
/// inside CloudWatch Logs' ingestion window.
///
/// Best effort and never fatal: see this module's header. Call it with NO state guard
/// held — it awaits.
pub async fn deliver_events(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
    log_stream: &str,
    events: &[StoredRumEvent],
) {
    if events.is_empty() {
        return;
    }
    let delivered_at = epoch_millis(::std::time::SystemTime::now());
    let client = logs_client(ctx, region);
    match client
        .create_log_stream()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
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
                log_group = %log_group,
                log_stream = %log_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the app monitor's log stream could not be created, so this batch is not \
                 copied to CloudWatch Logs"
            );
            return;
        }
    }

    let mut batch: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent> = Vec::new();
    let mut batch_bytes = 0usize;
    for event in events {
        let message = event.event_json.clone();
        if message.len() > MAX_LOG_EVENT_BYTES {
            ::tracing::warn!(
                log_group = %log_group,
                log_stream = %log_stream,
                bytes = message.len(),
                limit = MAX_LOG_EVENT_BYTES,
                "a delivered RUM event is larger than one log event may be, so it is not \
                 copied to CloudWatch Logs"
            );
            continue;
        }
        let cost = message.len() + LOG_EVENT_OVERHEAD_BYTES;
        if !batch.is_empty()
            && (batch.len() >= MAX_LOG_EVENTS_PER_BATCH
                || batch_bytes + cost > MAX_LOG_EVENT_BATCH_BYTES)
        {
            put_log_events(&client, log_group, log_stream, batch).await;
            batch = Vec::new();
            batch_bytes = 0;
        }
        let Ok(input_event) = ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
            .timestamp(delivered_at)
            .message(message)
            .build()
        else {
            continue;
        };
        batch_bytes += cost;
        batch.push(input_event);
    }
    if !batch.is_empty() {
        put_log_events(&client, log_group, log_stream, batch).await;
    }
}

/// Send ONE batch of copies to `log_group`/`log_stream`, tracing a refusal rather than
/// failing the `PutRumEvents` call that produced them.
async fn put_log_events(
    client: &::aws_sdk_cloudwatchlogs::Client,
    log_group: &str,
    log_stream: &str,
    events: Vec<::aws_sdk_cloudwatchlogs::types::InputLogEvent>,
) {
    let count = events.len();
    if let Err(error) = client
        .put_log_events()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .set_log_events(Some(events))
        .send()
        .await
    {
        ::tracing::warn!(
            log_group = %log_group,
            log_stream = %log_stream,
            events = count,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the app monitor's telemetry could not be copied to CloudWatch Logs"
        );
    }
}

/// `at` in milliseconds since the Unix epoch, which is the stamp a log event carries.
///
/// An instant before the epoch, which no log event can carry, clamps to the epoch rather
/// than wrapping.
fn epoch_millis(at: ::std::time::SystemTime) -> i64 {
    at.duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
