// SPDX-License-Identifier: CC0-1.0

//! Where an accepted span GOES once the account's trace segment destination is
//! CloudWatch Logs.
//!
//! The destination is not a reported value: it selects the store the spans this
//! service ingests are written to. Pointed at `XRay`, a span is readable only
//! through this service's own trace reads. Pointed at `CloudWatchLogs` — the
//! Transaction Search configuration — each ingested span is ALSO written into
//! the [`SPANS_LOG_GROUP`] log group of CloudWatch Logs, which is the documented
//! consumption path for Logs Insights queries, metric filters and subscription
//! filters over spans. So a client that switched the destination and then
//! queries that group finds the spans it submitted, rather than a log group that
//! does not exist.
//!
//! The event is written into the store CloudWatch Logs' OWN readers answer from,
//! through that service's own record types, so a delivered span and
//! `FilterLogEvents`, `GetLogEvents` or `DescribeLogStreams` cannot disagree. It
//! does not travel over `PutLogEvents`: the group is one the service itself
//! vends, and the write is made as X-Ray rather than as the caller.
//!
//! Nothing is synthesised. One event per accepted document, carrying that
//! document byte for byte as its message and the span's own `start_time` as its
//! instant, so an annotation value a client submitted is exactly what a
//! `filterPattern` over the group finds.
//!
//! LOCK ORDER: the caller drains what it has to deliver from THIS service's
//! store and releases its guard before calling [`deliver`], which then takes
//! CloudWatch Logs' — the two are never held at once, which is what keeps a
//! cycle between the two stores unreachable.

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

/// The CloudWatch Logs log group X-Ray delivers spans to while the account's
/// trace segment destination is `CloudWatchLogs`.
///
/// FIXED, and part of the feature rather than a choice: Transaction Search is
/// enabled by granting `xray.amazonaws.com` `logs:PutLogEvents` on the
/// `aws/spans` log group and switching the destination, and the queries,
/// metric filters and subscription filters that consume spans all name that
/// group. A client cannot ask for another one — no modelled member carries a
/// group name — so there is nothing here to take from a request.
pub const SPANS_LOG_GROUP: &str = "aws/spans";

/// The log stream of [`SPANS_LOG_GROUP`] the spans of an account are delivered
/// to.
///
/// Only the GROUP is documented, and no modelled member names a stream, so the
/// stream is the emulator's: one for the group, which is what the vended log
/// groups of other AWS services carry. It matters to no reader — a client
/// selects spans by their content, through a `filterPattern` over the group —
/// and keeping it to one stream means the streams of a group cannot grow with
/// the services that report spans.
pub const SPANS_LOG_STREAM: &str = "default";

/// How many milliseconds one second is, for the epoch-millisecond instant
/// `InputLogEvent$timestamp` carries, from the epoch SECONDS a span document
/// states its times in.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// One accepted span to deliver: what the caller ingested, with nothing added.
#[derive(Debug, Clone)]
pub struct Span {
    /// The submitted document, verbatim — the message of the delivered event.
    pub document: String,
    /// The span's `start_time`, in seconds of epoch time: when the work the
    /// span records began, which is the instant the event carries.
    pub start_time: f64,
}

impl Span {
    /// The instant the delivered event carries, in the epoch milliseconds
    /// `InputLogEvent$timestamp` is stated in.
    ///
    /// The span's own `start_time`, which ingest has already held to
    /// [`super::MAX_SEGMENT_EPOCH_SECONDS`], so the conversion is in range; the
    /// bounding here is the second belt, so no stored instant can make the
    /// delivery panic on a cast.
    fn timestamp_ms(&self) -> i64 {
        let seconds = if self.start_time.is_nan() {
            0.0
        } else {
            self.start_time
                .clamp(-super::MAX_SEGMENT_EPOCH_SECONDS, super::MAX_SEGMENT_EPOCH_SECONDS)
        };
        #[allow(clippy::cast_possible_truncation)]
        let millis = (seconds * MILLIS_PER_SECOND) as i64;
        millis
    }
}

/// Deliver `spans` to [`SPANS_LOG_GROUP`] in `region`, answering how many events
/// were written.
///
/// The group is CREATED if the account does not hold it yet: X-Ray is what vends
/// it, and a group a reader cannot find is a destination nothing reaches. A
/// group the account ALREADY holds is left exactly as it is — its retention, its
/// tags and the events already in it belong to the account.
///
/// Every factor a caller can grow is admitted against the OWNING service's own
/// ceilings before anything is written: how many groups that store holds
/// ([`logs::MAX_LOG_GROUPS`]), how many streams one group holds
/// ([`logs::MAX_STREAMS_PER_GROUP`]), how large one event may be
/// ([`logs::MAX_LOG_EVENT_BYTES`]) and how much one stream retains (its own
/// `enforce_storage_ceiling`) — so the spans a client submits are bounded by
/// that store rather than by the requests made, and nothing is written that the
/// owning service's own `PutLogEvents` would have refused.
///
/// An engine carrying no CloudWatch Logs store has nowhere to deliver to, so
/// nothing is written: the spans stay readable through this service's own trace
/// reads, which is all such a deployment can serve.
pub fn deliver<Eng: Send + Sync + 'static>(engine: &Eng, region: &str, spans: &[Span]) -> usize {
    if spans.is_empty() {
        return 0;
    }
    let Some(state) = logs::service_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    // ADMISSION FIRST, before anything is written: a delivery the owning store
    // has no room for must leave that store exactly as it was, rather than
    // creating an empty group or stream a reader would then find and read as a
    // destination that received nothing.
    let held = store.log_groups.get(SPANS_LOG_GROUP);
    let no_room_for_the_group = held.is_none() && store.log_groups.len() >= logs::MAX_LOG_GROUPS;
    let no_room_for_the_stream = held.is_some_and(|group| {
        !group.streams.contains_key(SPANS_LOG_STREAM)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
    });
    if no_room_for_the_group || no_room_for_the_stream {
        ::tracing::warn!(
            log_group = %SPANS_LOG_GROUP,
            log_stream = %SPANS_LOG_STREAM,
            "the CloudWatch Logs store is at capacity; a span delivery was not made"
        );
        return 0;
    }
    let now_ms = logs::now_ms();
    // The data identifiers the data protection policies in force for the group
    // name: such a policy masks what is ingested into the group by whichever
    // path the events arrive ("Sensitive data is detected and masked when it is
    // ingested into the log group"). A group this delivery is about to create is
    // covered by no group-level policy.
    let data_identifiers = store
        .log_groups
        .get(SPANS_LOG_GROUP)
        .map(|group| store.data_protection_identifiers_for(group))
        .unwrap_or_default();
    let events: Vec<logs::StoredEvent> = spans
        .iter()
        // An event larger than the largest one CloudWatch Logs holds is not
        // delivered rather than stored at a size that service's own ingest
        // refuses. `PutTraceSegments` cannot report that as an unprocessed
        // segment — the segment itself was accepted — so it is reported where an
        // operator can see it.
        .filter(|span| {
            let bytes = i64::try_from(span.document.len()).unwrap_or(i64::MAX);
            if bytes > logs::MAX_LOG_EVENT_BYTES {
                ::tracing::warn!(
                    log_group = %SPANS_LOG_GROUP,
                    bytes,
                    limit = logs::MAX_LOG_EVENT_BYTES,
                    "the span document is larger than one CloudWatch Logs event may be; it was \
                     not delivered"
                );
                return false;
            }
            true
        })
        .map(|span| logs::StoredEvent {
            timestamp: span.timestamp_ms(),
            masked_message: logs::data_protection::masked(&span.document, &data_identifiers),
            message: span.document.clone(),
            ingestion_time: now_ms,
            ingestion_seq: store.take_ingestion_seq(),
        })
        .collect();
    if events.is_empty() {
        return 0;
    }
    let added: i64 = events
        .iter()
        .map(logs::StoredEvent::stored_bytes)
        .fold(0i64, i64::saturating_add);
    let delivered = events.len();
    let group = store
        .log_groups
        .entry(SPANS_LOG_GROUP.to_string())
        .or_insert_with(|| logs::LogGroupRecord::new(region, SPANS_LOG_GROUP, now_ms));
    let stream = group
        .streams
        .entry(SPANS_LOG_STREAM.to_string())
        .or_insert_with(|| {
            logs::LogStreamRecord::new(region, SPANS_LOG_GROUP, SPANS_LOG_STREAM, now_ms)
        });
    stream.events.extend(events);
    // The stream is kept in `(timestamp, ingestion_seq)` order, which is the
    // order its reader pages it in; a batch's spans may carry any instants, so
    // the sort is what keeps that order.
    stream.events.sort_by_key(logs::StoredEvent::key);
    let discarded = stream.enforce_storage_ceiling();
    stream.refresh_event_times();
    stream.last_ingestion_time = Some(now_ms);
    group.stored_bytes = group
        .stored_bytes
        .saturating_add(added)
        .saturating_sub(discarded)
        .max(0);
    delivered
}
