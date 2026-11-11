// SPDX-License-Identifier: CC0-1.0

//! The error channel a destination's `CloudWatchLoggingOptions` opens: what a delivery that
//! the destination did not take reports, and where.
//!
//! A delivery happens with no client call at all ([`super`]), so a producer learns nothing
//! about one that failed from the API it put its records through — "The Amazon CloudWatch
//! logging options for your Firehose stream" (`CloudWatchLoggingOptions`) is the one channel
//! the service gives an operator for it. A destination whose options are `Enabled` has each
//! failed delivery written as ONE log event into the `LogGroupName` / `LogStreamName` they
//! name, so a misconfigured stream and a healthy one can be told apart by reading that
//! stream; a destination that enables none, and a healthy delivery, write nothing at all.
//!
//! The event is the document Firehose publishes for a failed delivery: the stream it belongs
//! to, the destination it was going to, the stream's version, the condition's own
//! `errorCode` and the message that code states ([`DestinationError`]).
//!
//! The events go into the store CloudWatch Logs' own readers answer from — that service's
//! record types, through its own ceilings — so a delivery error and `GetLogEvents` /
//! `FilterLogEvents` cannot disagree. They do not travel over `PutLogEvents`: this is the
//! service publishing about its own work, not a client ingesting log events.
//!
//! ## Lock order
//!
//! Only the CloudWatch Logs store is locked here, and only after this service's own store has
//! been released by the delivery that calls it ([`super::deliver_due`]) — the same order the
//! object write takes, so no two stores are ever locked at once.

use std::time::SystemTime;

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use crate::ops::types::CloudWatchLoggingOptions;

/// The `errorCode` and message of ONE condition a delivery failed on.
///
/// Firehose publishes a fixed code per condition, with the message that code states; the
/// variable parts of an event are the stream, the destination and the version, which the
/// event document carries beside this ([`error_event`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationError {
    /// The `errorCode` the event names, in the `<destination service>.<condition>` form
    /// Firehose publishes these under.
    pub error_code: &'static str,
    /// What that code states, as the event's `message`.
    pub message: &'static str,
}

/// The destination's role is not allowed to write the object: Firehose delivers under the
/// role the destination's `RoleARN` names, so a role whose permissions do not allow the write
/// cannot make it.
pub const S3_ACCESS_DENIED: DestinationError = DestinationError {
    error_code: "S3.AccessDenied",
    message: "Access was denied. Ensure that the trust policy for the provided IAM role \
              allows Firehose to assume the role, and the access policy allows access to \
              the S3 bucket.",
};

/// The bucket the destination's `BucketARN` names does not exist, so there is nothing to
/// write the delivery into. The message is Amazon S3's own for that condition, which is what
/// the delivery was answered with.
pub const S3_NO_SUCH_BUCKET: DestinationError = DestinationError {
    error_code: "S3.NoSuchBucket",
    message: "The specified bucket does not exist",
};

/// Where a destination's failed deliveries are reported: the log group and log stream its
/// `CloudWatchLoggingOptions` name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DestinationLogging {
    /// `CloudWatchLoggingOptions$LogGroupName`, "the CloudWatch group name for logging".
    pub log_group: String,
    /// `CloudWatchLoggingOptions$LogStreamName`, "the CloudWatch log stream name for
    /// logging".
    pub log_stream: String,
}

impl DestinationLogging {
    /// Where `options` report this destination's failed deliveries, or `None` when they
    /// report none.
    ///
    /// Nothing is reported unless the options are switched on — "Enables or disables
    /// CloudWatch logging" (`CloudWatchLoggingOptions$Enabled`) — and unless they name BOTH
    /// the group and the stream the events go to, each of which "is required if CloudWatch
    /// logging is enabled" (`LogGroupName`, `LogStreamName`): an option set naming no
    /// destination for them names no place a reader could find them in.
    #[must_use]
    pub fn of(options: Option<&CloudWatchLoggingOptions>) -> Option<Self> {
        let options = options.filter(|options| options.enabled == Some(true))?;
        let log_group = options.log_group_name.clone().filter(|name| !name.is_empty())?;
        let log_stream = options
            .log_stream_name
            .clone()
            .filter(|name| !name.is_empty())?;
        Some(Self {
            log_group,
            log_stream,
        })
    }
}

/// ONE failed delivery, as the destination's logging reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryErrorReport {
    /// Where it is reported.
    pub logging: DestinationLogging,
    /// The destination it was going to, by the ARN the configuration named it with — the
    /// event's `destination`.
    pub destination: String,
    /// The condition it failed on.
    pub error: DestinationError,
}

/// The event document one failed delivery is written as.
///
/// The members Firehose publishes for a delivery error: the stream the records were put on,
/// the destination they were going to, the stream's `VersionId` at the time of the delivery,
/// the condition's message and its `errorCode`. Every variable part is read from the stream
/// and from the destination configuration in force — none of it is fixed text.
#[must_use]
fn error_event(stream_arn: &str, version_id: u64, report: &DeliveryErrorReport) -> String {
    ::serde_json::json!({
        "deliveryStreamARN": stream_arn,
        "destination": report.destination,
        "deliveryStreamVersionId": version_id,
        "message": report.error.message,
        "errorCode": report.error.error_code,
    })
    .to_string()
}

/// Write one log event per report of `reports` into the group and stream each names.
///
/// `region` is the Region the reporting stream lives in, so a group or stream this has to
/// open carries the ARN its own reader reports for it. A destination's configured group or
/// stream that the account does not hold yet is OPENED here: the configuration says where the
/// stream's delivery errors go, and a destination a reader cannot find is a channel nothing
/// reaches — exactly what enabling these options is for.
///
/// Every factor a caller can grow is admitted against the OWNING service's own ceiling before
/// anything is written: how many groups that store holds ([`logs::MAX_LOG_GROUPS`]), how many
/// streams one group holds ([`logs::MAX_STREAMS_PER_GROUP`], the ceiling its own
/// `CreateLogStream` admits one against) and how much one stream retains (its own
/// `enforce_storage_ceiling`), so the events a retried delivery writes are bounded by that
/// store rather than by the attempts made.
pub fn report_delivery_errors<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    stream_arn: &str,
    version_id: u64,
    reports: &[DeliveryErrorReport],
    now: SystemTime,
) {
    if reports.is_empty() {
        return;
    }
    let Some(state) = logs::service_state(engine) else {
        ::tracing::warn!(
            "the log store is not reachable from this engine, so a delivery error was not \
             reported"
        );
        return;
    };
    let timestamp = epoch_millis(now);
    let mut store = state.lock();
    for report in reports {
        let now_ms = logs::now_ms();
        if !store.log_groups.contains_key(&report.logging.log_group)
            && store.log_groups.len() >= logs::MAX_LOG_GROUPS
        {
            ::tracing::warn!(
                log_group = %report.logging.log_group,
                "the log store is at capacity, so a delivery error was not reported"
            );
            continue;
        }
        let event = logs::StoredEvent {
            timestamp,
            message: error_event(stream_arn, version_id, report),
            ingestion_time: now_ms,
            ingestion_seq: store.take_ingestion_seq(),
        };
        let added = event.stored_bytes();
        let group = store
            .log_groups
            .entry(report.logging.log_group.clone())
            .or_insert_with(|| logs::LogGroupRecord::new(region, &report.logging.log_group, now_ms));
        if !group.streams.contains_key(&report.logging.log_stream)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
        {
            ::tracing::warn!(
                log_group = %report.logging.log_group,
                log_stream = %report.logging.log_stream,
                "the log group already holds the maximum number of streams, so a delivery \
                 error was not reported"
            );
            continue;
        }
        let stream = group
            .streams
            .entry(report.logging.log_stream.clone())
            .or_insert_with(|| {
                logs::LogStreamRecord::new(
                    region,
                    &report.logging.log_group,
                    &report.logging.log_stream,
                    now_ms,
                )
            });
        stream.events.push(event);
        // The stream is kept in `(timestamp, ingestion_seq)` order, which is the order its
        // reader pages it in; only an event that predates what is already there needs the
        // sort, which this leaves to the owning service's own comparison.
        stream.events.sort_by_key(logs::StoredEvent::key);
        let discarded = stream.enforce_storage_ceiling();
        stream.refresh_event_times();
        stream.last_ingestion_time = Some(now_ms);
        group.stored_bytes = group
            .stored_bytes
            .saturating_add(added)
            .saturating_sub(discarded)
            .max(0);
    }
}

/// `at` as the epoch milliseconds a log event's `timestamp` is counted in.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a destination reports its failed deliveries only where its
    /// `CloudWatchLoggingOptions` are switched on AND name both the group and the stream the
    /// events go to.
    #[test]
    fn the_options_decide_whether_a_failure_is_reported_at_all() {
        let options = |enabled: Option<bool>, group: Option<&str>, stream: Option<&str>| {
            Some(CloudWatchLoggingOptions {
                enabled,
                log_group_name: group.map(str::to_string),
                log_stream_name: stream.map(str::to_string),
            })
        };
        let reported = Some(DestinationLogging {
            log_group: "/aws/kinesisfirehose/stream".to_string(),
            log_stream: "DestinationDelivery".to_string(),
        });
        // (case, the configured options, where failures are reported)
        let rows = [
            ("no options at all", None, None),
            (
                "options that are switched off",
                options(
                    Some(false),
                    Some("/aws/kinesisfirehose/stream"),
                    Some("DestinationDelivery"),
                ),
                None,
            ),
            (
                "options whose Enabled is absent",
                options(
                    None,
                    Some("/aws/kinesisfirehose/stream"),
                    Some("DestinationDelivery"),
                ),
                None,
            ),
            (
                "options naming no log group",
                options(Some(true), None, Some("DestinationDelivery")),
                None,
            ),
            (
                "options naming no log stream",
                options(Some(true), Some("/aws/kinesisfirehose/stream"), None),
                None,
            ),
            (
                "options naming an empty log stream",
                options(Some(true), Some("/aws/kinesisfirehose/stream"), Some("")),
                None,
            ),
            (
                "options naming both",
                options(
                    Some(true),
                    Some("/aws/kinesisfirehose/stream"),
                    Some("DestinationDelivery"),
                ),
                reported.clone(),
            ),
        ];
        for (case, configured, expected) in rows {
            let read = DestinationLogging::of(configured.as_ref());
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the event document names the stream, the destination and the stream's version
    /// the delivery was attempted under, beside the condition's own code and message.
    #[test]
    fn the_event_document_names_the_stream_the_destination_and_the_condition() {
        let report = DeliveryErrorReport {
            logging: DestinationLogging {
                log_group: "/aws/kinesisfirehose/stream".to_string(),
                log_stream: "DestinationDelivery".to_string(),
            },
            destination: "arn:aws:s3:::archive".to_string(),
            error: S3_ACCESS_DENIED,
        };
        let stream_arn = format!(
            "arn:aws:firehose:us-east-1:{}:deliverystream/archiver",
            ::engine::emulator_account_id()
        );
        for (case, version_id) in [("the stream's first version", 1u64), ("a later one", 7)] {
            let document: ::serde_json::Value =
                ::serde_json::from_str(&error_event(&stream_arn, version_id, &report))
                    .expect("the event document is JSON");
            let expected = ::serde_json::json!({
                "deliveryStreamARN": stream_arn,
                "destination": "arn:aws:s3:::archive",
                "deliveryStreamVersionId": version_id,
                "message": S3_ACCESS_DENIED.message,
                "errorCode": "S3.AccessDenied",
            });
            assert_eq!(
                document, expected,
                "case {case:?}: expected {expected}, got {document}"
            );
        }
    }
}
