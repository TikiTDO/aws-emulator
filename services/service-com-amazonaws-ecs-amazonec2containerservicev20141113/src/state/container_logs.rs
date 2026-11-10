// SPDX-License-Identifier: CC0-1.0

//! The `awslogs` log driver: where the STDOUT of a container that RUNS is delivered.
//!
//! # The rule this module implements
//!
//! A container definition's `logConfiguration` names "the log driver to use for the
//! container", and the `awslogs` driver "sends container logs to Amazon CloudWatch Logs":
//! each line the container writes becomes ONE log event in the log group the
//! `awslogs-group` option names, in a stream named from the `awslogs-stream-prefix`
//! option, the container's own name and the id of the task it belongs to —
//! `prefix/container-name/task-id`, the naming the driver's `awslogs-stream-prefix`
//! documentation states. A driver option naming no group delivers nothing, and a
//! configuration whose driver is any other one is not delivered here: this module is the
//! `awslogs` driver and nothing else.
//!
//! The events are written through CloudWatch Logs' OWN API over the emulator's in-process
//! callback, so what a client reads back with `GetLogEvents` is what that service stored:
//! the stream is created first (`awslogs-create-group` additionally creates the group, as
//! that option documents), and a group that does not exist and was not asked to be created
//! takes the delivery nowhere — exactly as the real driver fails to deliver when the group
//! is missing.
//!
//! # Bounds
//!
//! Delivery is bounded by this emulator, not by AWS: one container's run may deliver at
//! most [`MAX_CONTAINER_LOG_LINES`] lines totalling [`MAX_CONTAINER_LOG_BYTES`], after
//! which nothing more is delivered for that container, so a program that writes without
//! end cannot grow the log store without end. A line longer than the largest log event
//! CloudWatch Logs holds is split into events of at most that size
//! ([`MAX_LOG_EVENT_BYTES`]), which is what the driver does with a line past the ceiling.

use std::sync::Mutex;

use crate::ops::types::LogConfiguration;

use super::*;

/// The `logConfiguration` option naming the log group the driver writes to.
const OPTION_LOG_GROUP: &str = "awslogs-group";

/// The option naming the Region the group lives in; the launch's own Region when the
/// configuration names none.
const OPTION_LOG_REGION: &str = "awslogs-region";

/// The option naming the prefix of the stream each container's output goes to.
const OPTION_STREAM_PREFIX: &str = "awslogs-stream-prefix";

/// The option that has the driver create the log group as well as the stream.
const OPTION_CREATE_GROUP: &str = "awslogs-create-group";

/// The value an `awslogs` boolean option is TRUE at: the driver reads its options as the
/// strings a task definition carries, and `"true"` is the one that turns one on.
const OPTION_TRUE: &str = "true";

/// The longest Region name the `awslogs-region` option is read as naming one. An AWS Region
/// name is a short, lower-case, hyphenated code (`us-west-2`), and this bounds what a task
/// definition's option can put in front of the delivery: a value outside the shape leaves
/// the driver delivering to the Region the task was launched in instead of carrying the
/// option's text any further.
const MAX_REGION_NAME_CHARS: usize = 32;

/// Whether `value` is spelled as an AWS Region name: between one and
/// [`MAX_REGION_NAME_CHARS`] characters, each of them a lower-case letter, a digit or a
/// hyphen.
fn is_region_name(value: &str) -> bool {
    let length = value.chars().count();
    (1..=MAX_REGION_NAME_CHARS).contains(&length)
        && value
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

/// The largest single log event CloudWatch Logs holds ("Each log event can be no larger
/// than 1 MB", `PutLogEvents`), as the ceiling a longer line is split at.
const MAX_LOG_EVENT_BYTES: usize = 1_048_576;

/// What `PutLogEvents` counts for each event BESIDE its message: a batch's size "is
/// calculated as the sum of all event messages in UTF-8, plus 26 bytes for each log event",
/// so the longest message one event may carry is [`MAX_LOG_EVENT_BYTES`] less this.
const LOG_EVENT_OVERHEAD_BYTES: usize = 26;

/// The longest MESSAGE one delivered event carries: see [`LOG_EVENT_OVERHEAD_BYTES`].
const MAX_LOG_EVENT_MESSAGE_BYTES: usize = MAX_LOG_EVENT_BYTES - LOG_EVENT_OVERHEAD_BYTES;

/// How many lines ONE container's run may deliver. THIS NUMBER IS THE EMULATOR'S, not an
/// AWS quota: the events are held in this process by the emulated log service, and a
/// container program that writes without end would otherwise grow that store without end.
/// It is far above what a real container writes in a test run.
pub const MAX_CONTAINER_LOG_LINES: usize = 10_000;

/// How many bytes of output ONE container's run may deliver, under
/// [`MAX_CONTAINER_LOG_LINES`]' rule applied to the SIZE rather than the count, so neither
/// very many short lines nor a few enormous ones can grow the log store past a stated
/// ceiling.
pub const MAX_CONTAINER_LOG_BYTES: usize = 4 * 1024 * 1024;

/// Where one container's output is delivered: the group, the Region and the stream the
/// driver's options and the task's own identifiers come to.
#[derive(Debug, Clone)]
pub struct AwsLogsTarget {
    /// The log group the `awslogs-group` option names.
    pub log_group: String,
    /// The Region the group lives in: the `awslogs-region` option, or the Region the task
    /// was launched in when the configuration names none.
    pub region: String,
    /// The stream this container's output goes to.
    pub log_stream: String,
    /// Whether the driver creates the group as well as the stream
    /// (`awslogs-create-group`).
    pub create_group: bool,
}

impl AwsLogsTarget {
    /// The delivery `configuration` describes for the container named `container_name` of
    /// the task `task_id`, or `None` when it describes none: a configuration whose driver
    /// is not `awslogs`, one that names no log group, or one whose `awslogs-region` names
    /// no Region at all, delivers nothing.
    ///
    /// The stream name follows the driver's own documentation for
    /// `awslogs-stream-prefix`: the prefix, the container's name and the task's id. A
    /// configuration that names no prefix leaves the driver naming the stream after the
    /// container it is delivering for, which here is that container's own id — the last
    /// segment of its ARN, which is what identifies one container of one task.
    pub fn resolve(
        configuration: Option<&LogConfiguration>,
        container_name: &str,
        task_id: &str,
        container_id: &str,
        launch_region: &str,
    ) -> Option<Self> {
        let configuration = configuration?;
        if configuration.log_driver != LOG_DRIVER_AWSLOGS {
            return None;
        }
        let options = configuration.options.as_ref()?;
        let log_group = options.get(OPTION_LOG_GROUP)?.clone();
        let region = match options.get(OPTION_LOG_REGION) {
            // A Region the option spells is where the group is, so it is carried through.
            Some(region) if is_region_name(region) => region.clone(),
            // A value that names no Region names no log store: the driver has nowhere to
            // deliver, and a DIFFERENT Region's store is not quietly put in its place.
            Some(_) => return None,
            None => launch_region.to_string(),
        };
        let log_stream = match options
            .get(OPTION_STREAM_PREFIX)
            .map(String::as_str)
            .filter(|prefix| !prefix.is_empty())
        {
            Some(prefix) => format!("{prefix}/{container_name}/{task_id}"),
            None => container_id.to_string(),
        };
        Some(Self {
            log_group,
            region,
            log_stream,
            create_group: options
                .get(OPTION_CREATE_GROUP)
                .is_some_and(|value| value == OPTION_TRUE),
        })
    }
}

/// How much output one container's run may still deliver.
#[derive(Debug)]
struct OutputBudget {
    /// How many more lines may be delivered ([`MAX_CONTAINER_LOG_LINES`]).
    lines_left: usize,
    /// How many more bytes may be delivered ([`MAX_CONTAINER_LOG_BYTES`]).
    bytes_left: usize,
}

/// One container's STDOUT, as the thing its program writes lines to.
///
/// Held behind an `Arc` by the session that runs the container's program, so every part of
/// that program writes to the one output with the one budget. A container whose
/// configuration describes no delivery still has an output — its program runs and answers a
/// status — and the lines it writes are counted against the same budget and go nowhere
/// else: nothing is kept in this store, because the log group is where container output is
/// read from.
#[derive(Debug)]
pub struct ContainerOutput {
    /// The CloudWatch Logs client the lines are delivered through, and `None` for a
    /// container whose configuration describes no `awslogs` delivery.
    client: Option<::aws_sdk_cloudwatchlogs::Client>,
    /// Where they are delivered.
    target: Option<AwsLogsTarget>,
    /// What is left of this run's output budget.
    budget: Mutex<OutputBudget>,
}

impl ContainerOutput {
    /// The output of a container delivering to `target`, over the in-process callback.
    ///
    /// The client is built even for a target this emulator holds no log group for: the
    /// write is what discovers that, exactly as the driver's own write does.
    pub fn new(
        callback: &::smithy_server_runtime::EmbeddedEmulator,
        target: Option<AwsLogsTarget>,
    ) -> Self {
        let client = target
            .as_ref()
            .map(|target| logs_client(callback, &target.region));
        Self {
            client,
            target,
            budget: Mutex::new(OutputBudget {
                lines_left: MAX_CONTAINER_LOG_LINES,
                bytes_left: MAX_CONTAINER_LOG_BYTES,
            }),
        }
    }

    /// Create the stream this container's output goes to — and the group as well when the
    /// driver was configured to ([`AwsLogsTarget::create_group`]) — so the first line has
    /// somewhere to land.
    ///
    /// Best effort: a group this emulator does not hold, or a stream the log service
    /// refuses, is traced and the container's program still runs. That is the real
    /// driver's shape too: the container runs and its output is not delivered.
    pub async fn open(&self) {
        let (Some(client), Some(target)) = (self.client.as_ref(), self.target.as_ref()) else {
            return;
        };
        if target.create_group {
            match client
                .create_log_group()
                .log_group_name(&target.log_group)
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
                        log_group = %target.log_group,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "a container's awslogs log group could not be created"
                    );
                }
            }
        }
        match client
            .create_log_stream()
            .log_group_name(&target.log_group)
            .log_stream_name(&target.log_stream)
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
                    log_group = %target.log_group,
                    log_stream = %target.log_stream,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a container's awslogs log stream could not be created, so its output \
                     is not delivered"
                );
            }
        }
    }

    /// Deliver ONE line the container wrote, as the log events it comes to.
    ///
    /// One line is one event, verbatim — the driver "sends container logs to Amazon
    /// CloudWatch Logs" as the container wrote them — split into events of at most
    /// [`MAX_LOG_EVENT_MESSAGE_BYTES`] when the line is longer than one event may be. A
    /// line past this run's own budget is not delivered at all, and nothing later is
    /// either.
    pub async fn write_line(&self, line: &str) {
        if !self.claim(line.len()) {
            return;
        }
        let (Some(client), Some(target)) = (self.client.as_ref(), self.target.as_ref()) else {
            return;
        };
        let at = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let timestamp = i64::try_from(at).unwrap_or(i64::MAX);
        for chunk in split_event_messages(line) {
            let Ok(event) = ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
                .timestamp(timestamp)
                .message(chunk)
                .build()
            else {
                continue;
            };
            if let Err(error) = client
                .put_log_events()
                .log_group_name(&target.log_group)
                .log_stream_name(&target.log_stream)
                .log_events(event)
                .send()
                .await
            {
                ::tracing::warn!(
                    log_group = %target.log_group,
                    log_stream = %target.log_stream,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a container's output could not be delivered to CloudWatch Logs"
                );
                return;
            }
        }
    }

    /// Take one line of `bytes` out of this run's output budget, answering whether there
    /// was room for it. Once there is not, nothing more is delivered for this container.
    fn claim(&self, bytes: usize) -> bool {
        let Ok(mut budget) = self.budget.lock() else {
            return false;
        };
        if budget.lines_left == 0 || budget.bytes_left < bytes {
            budget.lines_left = 0;
            budget.bytes_left = 0;
            return false;
        }
        budget.lines_left -= 1;
        budget.bytes_left -= bytes;
        true
    }
}

/// One line as the log event messages it is delivered as: the line itself when it fits in
/// one event, and otherwise its pieces, each at most [`MAX_LOG_EVENT_MESSAGE_BYTES`] bytes
/// — the most a message may carry once the per-event overhead `PutLogEvents` counts is
/// taken off the event ceiling.
///
/// Split on a CHARACTER boundary, so a line carrying multi-byte text is delivered as text
/// rather than as broken bytes.
fn split_event_messages(line: &str) -> Vec<String> {
    if line.len() <= MAX_LOG_EVENT_MESSAGE_BYTES {
        return vec![line.to_string()];
    }
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for character in line.chars() {
        if piece.len() + character.len_utf8() > MAX_LOG_EVENT_MESSAGE_BYTES {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.push(character);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no socket is opened and no name is resolved, and this crate names
/// no other service's crate. The configuration is the one every callback client in this
/// workspace uses (`service_common::eventbridge_client` for the bus this service publishes
/// on): nothing to retry, time out or watch for a stalled stream, because the transport is
/// in process, and the credentials the callback hands over are the emulator's own.
///
/// `region` is ADDRESSING for that transport rather than a host anything connects to, and it
/// is a Region name ([`is_region_name`]) rather than a destination: the driver's option names
/// which Region's log store the group is in, and the only thing it can reach is this process.
///
/// The callback is taken as the `EmbeddedEmulator` rather than as an `OperationContext`,
/// because a container's run outlives the request that launched it.
fn logs_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}
