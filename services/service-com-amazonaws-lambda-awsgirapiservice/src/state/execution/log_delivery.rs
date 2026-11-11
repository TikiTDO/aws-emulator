// SPDX-License-Identifier: CC0-1.0

//! Delivering one invocation's LOG to CloudWatch Logs, which is where a client reads it.
//!
//! A function's output is not part of its response: "Lambda automatically monitors Lambda
//! functions on your behalf and sends function metrics to Amazon CloudWatch", and every
//! invocation's `START`/`END`/`REPORT` framing together with whatever the handler wrote is
//! delivered to a log stream of the function's log group — "By default, Lambda functions
//! send logs to a default log group named `/aws/lambda/<function name>`"
//! (`LoggingConfig$LogGroup`). So the run's own log ([`super::run_log`]) is PUT to that
//! group here, over CloudWatch Logs' own API, through the same in-process seam a handler's
//! SDK call travels ([`super::aws_api`]): the group and the stream are created and the
//! events are put, so `DescribeLogGroups`, `DescribeLogStreams`, `GetLogEvents` and
//! `FilterLogEvents` report exactly what the run produced.
//!
//! Lambda makes those calls ACTING AS the function's execution role, not as the caller, so
//! each is made only while the policies in force on that role allow it
//! ([`super::super::role_actions`], over the one reading of those documents this crate
//! states). A role short of `logs:CreateLogGroup` creates no group,
//! and one short of `logs:PutLogEvents` writes no events — which is why a function whose
//! role carries no logs grant leaves nothing behind for a client to read. A withheld call is
//! not an invocation failure: the answer to `Invoke` is the handler's, whatever became of
//! the log.
//!
//! Nothing here is synthesised. Every event's message is a line the run wrote, and its
//! instant is the run's own: the opening frame carries the instant the run started, and the
//! closing frames the instant it finished.

use std::time::{Duration, SystemTime};

// How an instant of a run is stated in milliseconds since the epoch, which is what
// `InputLogEvent$timestamp` carries: the one reading this module tree shares.
use super::epoch_millis;

use super::aws_api::AwsAccess;

/// The IAM action namespace CloudWatch Logs' own calls are named under, which is also the
/// vendor field of its ARNs (`smithy_query(query="service_info")`: `arnNamespace` `logs`).
const LOGS_NAMESPACE: &str = "logs";

/// The call that creates the function's log group.
const CREATE_LOG_GROUP: &str = "CreateLogGroup";

/// The call that opens the stream one invocation writes to.
const CREATE_LOG_STREAM: &str = "CreateLogStream";

/// The call that writes the invocation's own lines.
const PUT_LOG_EVENTS: &str = "PutLogEvents";

/// How long ONE of those calls may take. The delivery is in-process, so this is a ceiling on
/// a stall rather than a budget: an invocation's answer is never held up for longer than this
/// per call by the writing of its log.
const CALL_CEILING: Duration = Duration::from_secs(5);

/// The error shape CloudWatch Logs refuses a create of something already there with, which
/// for a group or a stream the runtime re-opens every invocation is the ordinary answer
/// rather than a failure.
const ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";

/// ONE line of a run's log, as the event CloudWatch Logs holds it as.
#[derive(Debug, Clone)]
pub struct LogEvent {
    /// When the line was written, in the epoch milliseconds `InputLogEvent$timestamp`
    /// carries.
    pub timestamp: i64,
    /// The line itself, as `InputLogEvent$message`.
    pub message: String,
}

/// Which of the three calls the function's execution role may make.
///
/// `true` is "nothing shows the role may not" — an engine with no IAM store, a role the
/// store holds no record for and a role carrying an attachment whose document this account
/// does not hold each show nothing, so each leaves the call to be made
/// ([`super::super::role_actions`]).
#[derive(Debug, Clone, Copy)]
pub struct LogGrants {
    /// `logs:CreateLogGroup` on the function's log group.
    pub create_log_group: bool,
    /// `logs:CreateLogStream` on it.
    pub create_log_stream: bool,
    /// `logs:PutLogEvents` on its streams.
    pub put_log_events: bool,
}

/// Where one function's invocations are logged, and what its execution role may do there.
#[derive(Debug, Clone)]
pub struct LogTarget {
    /// The log group the function writes to: the one its `LoggingConfig` names, else the
    /// default one for its name.
    pub log_group: String,
    /// What the function's execution role may do to that group.
    pub grants: LogGrants,
}

/// Where the function `config` logs, and what its execution role may do there — read off the
/// SAME shared engine the function's own store lives in, so the decision is made against the
/// records IAM's handlers wrote.
#[must_use]
pub fn log_target<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    config: &crate::state::FunctionConfig,
) -> LogTarget {
    // The ONE reading of where this function's invocations are logged, shared with what a
    // READ of the function reports for its `LoggingConfig`.
    let log_group =
        crate::state::configured_log_group(config.logging_config.as_ref(), &config.name);
    LogTarget {
        grants: grants(engine, region, &config.role, &log_group),
        log_group,
    }
}

/// What the role `role_arn` may do to the log group `log_group`.
fn grants<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    role_arn: &str,
    log_group: &str,
) -> LogGrants {
    let Some(iam) = crate::state::event_source_authorization::iam_state(engine) else {
        // No IAM store to ask is evidence of nothing about the role.
        return LogGrants {
            create_log_group: true,
            create_log_stream: true,
            put_log_events: true,
        };
    };
    let iam = iam.lock();
    // The resource each call is made on, as CloudWatch Logs' own IAM reference names it: a
    // group-level call on the group, and a write on the group's streams. The grant AWS's own
    // `AWSLambdaBasicExecutionRole` carries — the group's ARN with its streams under it —
    // covers both.
    let group = ::engine::emulator_arn(LOGS_NAMESPACE, region, &format!("log-group:{log_group}:*"));
    let streams = ::engine::emulator_arn(
        LOGS_NAMESPACE,
        region,
        &format!("log-group:{log_group}:log-stream:*"),
    );
    // The decision over them is the one reading of the documents in force on a role this
    // crate states (`crate::state::role_actions` over
    // `crate::state::event_source_authorization`), so this gate and the ones on an event
    // source, a VPC attachment and an invocation record's destination answer alike. A role
    // nothing is known about withholds nothing, which leaves the call to be made.
    let may = |action: &'static str, resource: &str| {
        let asked = [(format!("{LOGS_NAMESPACE}:{action}"), action)];
        crate::state::role_actions::withheld_action(&iam, role_arn, resource, &asked).is_none()
    };
    LogGrants {
        create_log_group: may(CREATE_LOG_GROUP, &group),
        create_log_stream: may(CREATE_LOG_STREAM, &group),
        put_log_events: may(PUT_LOG_EVENTS, &streams),
    }
}

/// The events of one run's log: the opening frame at the instant the run started, the
/// closing frames at the instant it finished, and every line the handler wrote in between.
///
/// `opened` is how many of `lines` belong to the opening frame, and `closed` how many to the
/// closing one; the rest are the handler's own output, which was written while the run was
/// going and so carries the instant the run started.
#[must_use]
pub(super) fn events(
    lines: &[String],
    opened: usize,
    started: SystemTime,
    elapsed: Duration,
) -> Vec<LogEvent> {
    let start = epoch_millis(started);
    let end = epoch_millis(started.checked_add(elapsed).unwrap_or(started)).max(start);
    let closes_at = lines.len().saturating_sub(CLOSING_FRAME_LINES);
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| LogEvent {
            timestamp: if index >= opened && index >= closes_at {
                end
            } else {
                start
            },
            message: line.clone(),
        })
        .collect()
}

/// How many lines close a run's log: the `END` line and the `REPORT` line
/// ([`super::run_log`]).
const CLOSING_FRAME_LINES: usize = 2;

/// How many events one `PutLogEvents` call carries at most, which is what AWS documents for
/// one batch ("The maximum number of log events in a batch is 10,000") and what the model's
/// own `logEvents` member allows (`@length(min: 1, max: 10000)`).
const MAX_EVENTS_PER_BATCH: usize = 10_000;

/// How many bytes of messages one call carries at most. AWS's own batch ceiling is 1 MB
/// including a per-event overhead; this is well inside it and inside what ONE call of the SDK
/// seam may carry ([`super::aws_api::MAX_REQUEST_BYTES`]), so a run that wrote the whole of
/// its log budget is delivered in several calls rather than refused as one oversized request.
const MAX_MESSAGE_BYTES_PER_BATCH: usize = 512 * 1024;

/// Deliver one run's log: open the group and the stream, and put the run's events.
///
/// Every call is made only while the role may make it, and a call CloudWatch Logs refuses is
/// not an invocation failure — there is simply less of the log to read, which is exactly what
/// a client sees of a function whose role is short of a logs grant.
pub(super) async fn deliver(
    aws: &AwsAccess,
    target: &LogTarget,
    region: &str,
    log_stream: &str,
    events: &[LogEvent],
) {
    if events.is_empty() {
        return;
    }
    if target.grants.create_log_group {
        let body = ::serde_json::json!({ "logGroupName": target.log_group }).to_string();
        made(aws, region, CREATE_LOG_GROUP, &body).await;
    }
    if target.grants.create_log_stream {
        let body = ::serde_json::json!({
            "logGroupName": target.log_group,
            "logStreamName": log_stream,
        })
        .to_string();
        made(aws, region, CREATE_LOG_STREAM, &body).await;
    }
    if !target.grants.put_log_events {
        return;
    }
    // One call per batch of the bounds above, in order, so the whole log reaches the stream
    // however many lines the run wrote — and no one call is larger than a batch may be.
    for batch in batched(events) {
        let carried: Vec<::serde_json::Value> = batch
            .iter()
            .map(|event| {
                ::serde_json::json!({ "timestamp": event.timestamp, "message": event.message })
            })
            .collect();
        let body = ::serde_json::json!({
            "logGroupName": target.log_group,
            "logStreamName": log_stream,
            "logEvents": carried,
        })
        .to_string();
        made(aws, region, PUT_LOG_EVENTS, &body).await;
    }
}

/// `events` split into the batches one call may carry: at most
/// [`MAX_EVENTS_PER_BATCH`] events and at most [`MAX_MESSAGE_BYTES_PER_BATCH`] bytes of
/// messages each.
///
/// A single event larger than that budget is a batch of its own: the run's log is capped
/// before it reaches here, so the last word on how much is delivered is the run's own log
/// budget rather than this split.
#[must_use]
pub(super) fn batched(events: &[LogEvent]) -> Vec<&[LogEvent]> {
    let mut batches = Vec::new();
    let mut from = 0;
    while from < events.len() {
        let mut bytes = 0;
        let mut to = from;
        while to < events.len() && to - from < MAX_EVENTS_PER_BATCH {
            let size = events[to].message.len();
            if to > from && bytes + size > MAX_MESSAGE_BYTES_PER_BATCH {
                break;
            }
            bytes += size;
            to += 1;
        }
        batches.push(&events[from..to]);
        from = to;
    }
    batches
}

/// Make ONE of those calls, and report what it came to where an operator can see it.
///
/// A group or a stream that is already there is the ordinary answer for a runtime that
/// re-opens both every invocation, so it is not reported as a problem.
async fn made(aws: &AwsAccess, region: &str, operation: &str, body: &str) {
    match aws
        .call_within(LOGS_NAMESPACE, operation, region, body, CALL_CEILING)
        .await
    {
        Ok(answer) if answer.status < 300 => {}
        Ok(answer) if answer.error_type.as_deref().is_some_and(already_exists) => {}
        Ok(answer) => ::tracing::debug!(
            operation,
            status = answer.status,
            error_type = answer.error_type.as_deref().unwrap_or_default(),
            "an invocation's log was not delivered"
        ),
        Err(refusal) => ::tracing::debug!(
            operation,
            refusal = ?refusal,
            "an invocation's log was not delivered"
        ),
    }
}

/// Whether the error shape an answer named is the already-exists refusal, however the header
/// qualified it (an `x-amzn-errortype` may carry a shape id prefix and a URL suffix).
fn already_exists(error_type: &str) -> bool {
    error_type
        .rsplit('#')
        .next()
        .is_some_and(|shape| shape.split(':').next() == Some(ALREADY_EXISTS))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the framing table: the lines of a run's log, how many of them open it, and
    /// the instant each event must carry.
    struct Framed {
        case: &'static str,
        /// The lines of the log, in order.
        lines: &'static [&'static str],
        /// How many of them open it.
        opened: usize,
        /// The instant each resulting event must carry.
        expected: &'static [i64],
    }

    /// Every line of a run's log becomes one event, the opening frame carrying the instant
    /// the run started and the closing frames the instant it finished — one row per shape of
    /// log a run can produce.
    #[test]
    fn a_runs_log_becomes_one_event_per_line_framed_by_the_runs_own_instants() {
        let started = SystemTime::UNIX_EPOCH + Duration::from_millis(1_793_000_000_000);
        let elapsed = Duration::from_millis(250);
        let cases: &[Framed] = &[
            Framed {
                case: "a run that wrote nothing of its own",
                lines: &["START", "END", "REPORT"],
                opened: 1,
                expected: &[1_793_000_000_000, 1_793_000_000_250, 1_793_000_000_250],
            },
            Framed {
                case: "a run that wrote one line",
                lines: &["START", "hello", "END", "REPORT"],
                opened: 1,
                expected: &[
                    1_793_000_000_000,
                    1_793_000_000_000,
                    1_793_000_000_250,
                    1_793_000_000_250,
                ],
            },
            Framed {
                case: "a run that wrote several",
                lines: &["START", "one", "two", "three", "END", "REPORT"],
                opened: 1,
                expected: &[
                    1_793_000_000_000,
                    1_793_000_000_000,
                    1_793_000_000_000,
                    1_793_000_000_000,
                    1_793_000_000_250,
                    1_793_000_000_250,
                ],
            },
        ];
        for row in cases {
            let (case, opened, expected) = (row.case, row.opened, row.expected);
            let lines: Vec<String> = row.lines.iter().map(|line| (*line).to_string()).collect();
            let built = events(&lines, opened, started, elapsed);
            let stamps: Vec<i64> = built.iter().map(|event| event.timestamp).collect();
            assert_eq!(
                stamps,
                expected.to_vec(),
                "case {case:?}: expected {expected:?}, got {stamps:?}"
            );
            let messages: Vec<&str> = built.iter().map(|event| event.message.as_str()).collect();
            assert_eq!(
                messages,
                lines.iter().map(String::as_str).collect::<Vec<&str>>(),
                "case {case:?}: every event's message is a line the run wrote"
            );
            assert!(
                stamps.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case:?}: a batch's events are ordered by instant, got {stamps:?}"
            );
        }
    }

    /// A log is delivered in batches of what one call may carry — one row per size class
    /// around each bound — and every event is in exactly one batch.
    #[test]
    fn the_log_is_delivered_in_batches_of_what_one_call_may_carry() {
        // (case, how many events, how long each message is, how many batches)
        let cases: &[(&str, usize, usize, usize)] = &[
            ("one line", 1, 16, 1),
            ("a batch's worth of lines", MAX_EVENTS_PER_BATCH, 1, 1),
            ("one line past it", MAX_EVENTS_PER_BATCH + 1, 1, 2),
            ("two batches' worth", MAX_EVENTS_PER_BATCH * 2, 1, 2),
            (
                "lines whose bytes fill a batch",
                4,
                MAX_MESSAGE_BYTES_PER_BATCH / 2,
                2,
            ),
            (
                "one line larger than a batch's bytes",
                2,
                MAX_MESSAGE_BYTES_PER_BATCH + 1,
                2,
            ),
        ];
        for (case, count, length, expected) in cases {
            let events: Vec<LogEvent> = (0..*count)
                .map(|index| LogEvent {
                    timestamp: i64::try_from(index).unwrap_or_default(),
                    message: "L".repeat(*length),
                })
                .collect();
            let batches = batched(&events);
            assert_eq!(
                batches.len(),
                *expected,
                "case {case:?}: expected {expected} batch(es), got {}",
                batches.len()
            );
            let delivered: usize = batches.iter().map(|batch| batch.len()).sum();
            assert_eq!(
                delivered, *count,
                "case {case:?}: expected every event in exactly one batch, got {delivered} of \
                 {count}"
            );
            for batch in &batches {
                assert!(
                    !batch.is_empty() && batch.len() <= MAX_EVENTS_PER_BATCH,
                    "case {case:?}: expected a batch of at most {MAX_EVENTS_PER_BATCH} events, \
                     got {}",
                    batch.len()
                );
            }
        }
    }

    /// The already-exists refusal is recognised however the answer's header qualified the
    /// shape it named, and no other refusal is taken for it.
    #[test]
    fn the_already_exists_refusal_is_read_through_the_headers_qualifiers() {
        // (case, the header value, whether it names the already-exists refusal)
        let cases: &[(&str, &str, bool)] = &[
            ("the bare shape name", ALREADY_EXISTS, true),
            (
                "one carrying a shape id prefix",
                "com.amazonaws.cloudwatchlogs#ResourceAlreadyExistsException",
                true,
            ),
            (
                "one carrying a URL suffix",
                "ResourceAlreadyExistsException:http://internal.amazon.com/coral/",
                true,
            ),
            ("another refusal", "ResourceNotFoundException", false),
            ("one naming nothing", "", false),
        ];
        for (case, header, expected) in cases {
            let actual = already_exists(header);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected already_exists({header:?}) to be {expected}, got \
                 {actual}"
            );
        }
    }
}
