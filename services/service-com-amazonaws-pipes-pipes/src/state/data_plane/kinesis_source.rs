// SPDX-License-Identifier: CC0-1.0

//! How a pipe reads an Amazon KINESIS DATA STREAM source: which shards are read, where
//! each is read from, and what one record is handed to the pipe's stages as.
//!
//! Everything a pipe does with a batch once it HAS it — the filter, the enrichment, the
//! target and finishing the batch with its source — is the one rule for every source and
//! lives in [`super`]. This module is only the half that differs per source, the other
//! halves being the Amazon SQS receive and the DynamoDB-stream read there.
//!
//! The stream is read through the Kinesis surface of this same emulator (`ListShards`,
//! `GetShardIterator`, `GetRecords`) over the in-process callback, so a record is consumed
//! only through the data plane a client would consume it through, and every one of those
//! calls is made as the pipe's execution role ([`super::LiveRecord::refusal`]).
//!
//! Where a shard has been read up to is held as the POSITION itself ([`Cursor`]) rather
//! than as the iterator that reads it: "a shard iterator expires 5 minutes after it is
//! returned to the requester" (`GetShardIterator`), so a poller that kept one would lose
//! its place on an idle stream. Each read mints the iterator for the position the shard
//! stands at, and the position moves on only once the batch read from it has been
//! delivered — so a failed delivery reads the same records again, which is the
//! at-least-once a stream source promises.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Instant, SystemTime};

use ::serde_json::{json, Value};

use super::{Clients, LiveRecord, RunningPipe, MAX_SHARDS_PER_PASS, POLL_INTERVAL};

/// The IAM action each call this module makes is authorized under: the action name of the
/// Kinesis API call, which is what the policies in force on the pipe's execution role are
/// evaluated against.
const LIST_SHARDS_ACTION: &str = "kinesis:ListShards";
const GET_SHARD_ITERATOR_ACTION: &str = "kinesis:GetShardIterator";
pub(super) const GET_RECORDS_ACTION: &str = "kinesis:GetRecords";

/// The `eventSource` a Kinesis record envelope reports, and the `eventName` each record
/// carries.
pub(super) const KINESIS_EVENT_SOURCE: &str = "aws:kinesis";
/// See [`KINESIS_EVENT_SOURCE`].
const KINESIS_EVENT_NAME: &str = "aws:kinesis:record";
/// The schema version a Kinesis record envelope reports, and the version of the envelope
/// itself.
const KINESIS_SCHEMA_VERSION: &str = "1.0";
/// See [`KINESIS_SCHEMA_VERSION`].
const KINESIS_EVENT_VERSION: &str = "1.0";

/// The `StartingPosition` values a Kinesis stream source may name. Source:
/// `KinesisStreamStartPosition` in `smithy-model/com-amazonaws-pipes-pipes.json`
/// (`TRIM_HORIZON`, `LATEST`, `AT_TIMESTAMP`).
const TRIM_HORIZON_POSITION: &str = "TRIM_HORIZON";
const AT_TIMESTAMP_POSITION: &str = "AT_TIMESTAMP";

/// The most `ListShards` pages one pass walks. The EMULATOR's bound: a stream's shard
/// count is the client's, so a pass is bounded by work of its own.
const MAX_LIST_SHARDS_PAGES: usize = 10;

/// Where one shard of the source stream is read from next.
///
/// `StartingPosition` is "the position in a stream from which to start reading" — a
/// property of the PIPE — so a shard this pipe has delivered batches from is read on after
/// the newest record it delivered, and one it has delivered none from still stands at the
/// position the pipe's configuration fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cursor {
    /// From the oldest record the shard still retains, which is what `TRIM_HORIZON` names.
    TrimHorizon,
    /// From a stated instant: the `StartingPositionTimestamp` of an `AT_TIMESTAMP` pipe,
    /// and the instant the poller took its position up for a `LATEST` one — "start reading
    /// just after the most recent record in the shard"
    /// (`GetShardIterator$ShardIteratorType`) AT THAT MOMENT is the same set of records as
    /// everything that arrived from that instant on, and unlike a pinned iterator it does
    /// not expire while the stream is idle.
    AtTimestamp(SystemTime),
    /// Past the record this sequence number names: the newest record of the last batch
    /// this pipe finished from the shard.
    AfterSequenceNumber(String),
}

/// Where each shard of a Kinesis stream source has been read up to, for the length of one
/// poller.
///
/// Bounded by the stream's own shard count: a shard the stream no longer lists is
/// forgotten, and a closed shard that has been read to its end is remembered only by its
/// id.
#[derive(Debug, Default)]
pub(super) struct Cursors {
    /// The position each live shard is read from next, by shard id.
    shards: BTreeMap<String, Cursor>,
    /// The shards that are closed and read to their end, which are not read again.
    drained: BTreeSet<String>,
    /// The shard the last pass moved a batch from. The next pass lists the shards AFTER
    /// it and wraps round at the end of the stream, so each shard is served in turn rather
    /// than one busy shard starving the rest.
    last_served: Option<String>,
    /// The instant this poller took its position in the stream up, for a `LATEST` pipe.
    /// The configured position applies to the shards the stream had THEN; a shard the
    /// stream gains afterwards is read from its own beginning, so no record appended to it
    /// is skipped.
    started_at: Option<SystemTime>,
}

impl Cursors {
    /// Take up the position after the record `sequence_number` names: everything up to and
    /// including it has been finished with the source.
    fn advance(&mut self, shard: &str, sequence_number: &str) {
        self.shards.insert(
            shard.to_string(),
            Cursor::AfterSequenceNumber(sequence_number.to_string()),
        );
    }

    /// Forget every shard the stream no longer lists, so what this holds is bounded by the
    /// stream as it is now.
    fn retain_listed(&mut self, listed: &BTreeSet<String>) {
        self.shards.retain(|shard, _| listed.contains(shard));
        self.drained.retain(|shard| listed.contains(shard));
    }

    /// The position `shard` stands at, taking up the pipe's configured one the first time
    /// this poller sees the shard.
    fn position(&mut self, shard: &str, run: &RunningPipe, started_at: SystemTime) -> Cursor {
        if let Some(held) = self.shards.get(shard) {
            return held.clone();
        }
        let taken = initial_cursor(run, started_at);
        self.shards.insert(shard.to_string(), taken.clone());
        taken
    }
}

/// The position a shard this poller has not read yet is read from: the pipe's own
/// `StartingPosition`, with `LATEST` read as the instant the poller took its position up.
///
/// A shard the stream gains after that instant is read from `started_at` too rather than
/// from the tip it has when the read reaches it, which is what keeps a record appended to a
/// new shard from being skipped.
fn initial_cursor(run: &RunningPipe, started_at: SystemTime) -> Cursor {
    let super::Source::KinesisStream {
        starting_position,
        starting_position_timestamp,
    } = &run.source
    else {
        return Cursor::TrimHorizon;
    };
    match starting_position.as_str() {
        TRIM_HORIZON_POSITION => Cursor::TrimHorizon,
        // "With StartingPosition set to AT_TIMESTAMP, the time from which to start
        // reading" (`PipeSourceKinesisStreamParameters$StartingPositionTimestamp`). A
        // configuration naming the position and no instant names no time to read from, so
        // the oldest retained record is where it reads from.
        AT_TIMESTAMP_POSITION => starting_position_timestamp
            .as_ref()
            .map_or(Cursor::TrimHorizon, |at| Cursor::AtTimestamp(*at)),
        // `LATEST`, and — the boundary validator refuses a value outside the enum, so none
        // reaches here — anything else, read as the records that arrive from now on rather
        // than as a position that would silently skip them.
        _ => Cursor::AtTimestamp(started_at),
    }
}

/// One shard's worth of Kinesis records, with the shard they came from.
#[derive(Debug)]
pub(super) struct Batch {
    /// The shard they came from: one partition key's records are all in one shard, which
    /// is why reading a shard in order preserves the order they were put in.
    pub(super) shard: String,
    pub(super) records: Vec<::aws_sdk_kinesis::types::Record>,
}

impl Batch {
    /// A poll that found no record on any shard.
    fn nothing() -> Self {
        Self {
            shard: String::new(),
            records: Vec::new(),
        }
    }
}

/// The identifier its source names each of `records` by: the record's own sequence number,
/// "the unique identifier of the record within its shard" (`Record$SequenceNumber`), which
/// is both what names its position in the shard and what a target's report of the records
/// it did not process names it by.
pub(super) fn identifiers(records: &[::aws_sdk_kinesis::types::Record]) -> Vec<String> {
    records
        .iter()
        .map(|record| record.sequence_number().to_string())
        .collect()
}

/// Take up the position every shard the stream lists NOW is read from, without reading
/// anything.
///
/// This is what a pipe that is still reaching RUNNING does: the position a `LATEST` pipe
/// reads from is the instant it was taken up, so a record put between the call that started
/// the pipe and the moment it runs is inside the span the pipe then reads.
pub(super) async fn establish(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    cursors: &mut Cursors,
) -> Result<(), String> {
    let started_at = *cursors.started_at.get_or_insert_with(SystemTime::now);
    for shard in list_shards(callback, clients, record, run, cursors).await? {
        cursors.position(&shard, run, started_at);
    }
    Ok(())
}

/// Read up to `BatchSize` records from ONE shard of the stream, collecting for up to the
/// batching window once the first record has been read.
///
/// The shards are walked in the order the pass listed them — which starts after the shard
/// the previous pass served — and the FIRST one holding records is the batch: a pipe moves
/// one batch at a time, so the records of one shard, and therefore the records of one
/// partition key, which all land in one shard, reach the target in the order they were put.
///
/// Each read asks only for the records the batch still has room for, so the batch never
/// holds more than `BatchSize` and no record this pass read is left out of it.
pub(super) async fn collect(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    cursors: &mut Cursors,
) -> Result<Batch, String> {
    let started_at = *cursors.started_at.get_or_insert_with(SystemTime::now);
    let shards = list_shards(callback, clients, record, run, cursors).await?;
    for shard in shards {
        if cursors.drained.contains(&shard) {
            continue;
        }
        let position = cursors.position(&shard, run, started_at);
        let Some(iterator) = shard_iterator(callback, clients, record, run, &shard, &position).await?
        else {
            cursors.drained.insert(shard.clone());
            continue;
        };
        let (mut records, mut next) =
            get_records(callback, clients, record, run, &iterator, run.batch_size).await?;
        if records.is_empty() {
            // A closed shard that answered nothing more is drained: it is retired here
            // rather than read again, because its position cannot move any further.
            if next.is_none() {
                cursors.drained.insert(shard.clone());
            }
            continue;
        }
        let deadline = Instant::now() + run.batching_window;
        // The same run gate every other source's collection is held to: a stream that is
        // still being read when the pipe leaves RUNNING is read no further, so the records
        // put after that move are left for a later poll.
        while records.len() < run.batch_size && Instant::now() < deadline && record.runs_now() {
            let Some(iterator) = next.clone() else {
                break;
            };
            let wanted = run.batch_size - records.len();
            let (more, after) =
                get_records(callback, clients, record, run, &iterator, wanted).await?;
            if more.is_empty() {
                ::tokio::time::sleep(POLL_INTERVAL).await;
            } else {
                records.extend(more);
                next = after;
            }
        }
        cursors.last_served = Some(shard.clone());
        return Ok(Batch { shard, records });
    }
    Ok(Batch::nothing())
}

/// Finish `batch` with its shard: the shard is read on from after the records the target
/// processed.
///
/// `through` is the newest record of the run at the HEAD of the batch the target processed,
/// or `None` when it processed none of them — a shard has ONE position, so the records it
/// can be read past are that run and no more, and a batch whose own first record was not
/// processed moves the position not at all.
pub(super) fn finish(cursors: &mut Cursors, batch: &Batch, through: Option<&str>) -> bool {
    match through {
        Some(sequence_number) => {
            cursors.advance(&batch.shard, sequence_number);
            true
        }
        None => false,
    }
}

/// The shards of the source stream this pass may read, in the order `ListShards` lists
/// them.
///
/// The listing WINDOW starts after the shard the previous pass served
/// ([`Cursors::last_served`]) and wraps round when it runs past the last shard, so each
/// shard of the stream is served in turn — an earlier shard that is written continuously
/// cannot starve the ones behind it, and a stream with more shards than one pass lists is
/// covered pass by pass.
///
/// What this poller remembers about OTHER shards is pruned only when the window covered the
/// whole stream, so a shard outside this window keeps the position it was read up to.
async fn list_shards(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    cursors: &mut Cursors,
) -> Result<Vec<String>, String> {
    if let Some(detail) = record.refusal(LIST_SHARDS_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let from = cursors.last_served.clone();
    let mut listed = listed_shards(callback, clients, run, from.clone()).await?;
    if listed.shards.is_empty() && from.is_some() {
        // The window ran past the stream's last shard: the next one is its beginning.
        cursors.last_served = None;
        listed = listed_shards(callback, clients, run, None).await?;
    }
    if listed.complete {
        cursors.retain_listed(&listed.shards.iter().cloned().collect());
    }
    Ok(listed.shards)
}

/// The shards one run of `ListShards` pages lists, starting after `after`.
struct ListedShards {
    shards: Vec<String>,
    /// Whether the run covered the WHOLE stream: it started at the beginning and the
    /// service reported no shard beyond the ones it listed. Only then is what this poller
    /// remembers about shards outside the run safe to prune.
    complete: bool,
}

/// One run of `ListShards` pages, from the shard after `after` (or from the stream's first
/// shard when there is none).
async fn listed_shards(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &RunningPipe,
    after: Option<String>,
) -> Result<ListedShards, String> {
    let from_the_beginning = after.is_none();
    let mut shards: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut whole_stream = false;
    for page in 0..MAX_LIST_SHARDS_PAGES {
        let mut request = clients.kinesis(callback, &run.source_region).list_shards();
        // A continuation carries the stream, the window and the filter the first page
        // named, so `NextToken` is the only member a later page may carry.
        request = match next_token.take() {
            Some(token) => request.next_token(token),
            None => {
                let mut first = request.stream_arn(&run.source_arn);
                if let (0, Some(after)) = (page, after.as_deref()) {
                    first = first.exclusive_start_shard_id(after);
                }
                first
            }
        };
        let answer = request.send().await.map_err(|error| {
            format!(
                "the shards of the source stream {} could not be listed ({})",
                run.source_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        shards.extend(
            answer
                .shards()
                .iter()
                .map(|shard| shard.shard_id().to_string()),
        );
        match answer.next_token() {
            Some(token) => next_token = Some(token.to_string()),
            None => {
                whole_stream = true;
                break;
            }
        }
        if shards.len() >= MAX_SHARDS_PER_PASS {
            break;
        }
    }
    let within_the_bound = shards.len() <= MAX_SHARDS_PER_PASS;
    shards.truncate(MAX_SHARDS_PER_PASS);
    Ok(ListedShards {
        shards,
        complete: from_the_beginning && whole_stream && within_the_bound,
    })
}

/// The iterator that reads `shard` from the position it stands at, or `None` for a shard
/// the stream answers no iterator for — one that is closed and already past its end.
async fn shard_iterator(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    shard: &str,
    position: &Cursor,
) -> Result<Option<String>, String> {
    use ::aws_sdk_kinesis::types::ShardIteratorType;

    if let Some(detail) = record.refusal(GET_SHARD_ITERATOR_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let request = clients
        .kinesis(callback, &run.source_region)
        .get_shard_iterator()
        .stream_arn(&run.source_arn)
        .shard_id(shard);
    let request = match position {
        Cursor::TrimHorizon => request.shard_iterator_type(ShardIteratorType::TrimHorizon),
        Cursor::AtTimestamp(instant) => request
            .shard_iterator_type(ShardIteratorType::AtTimestamp)
            .timestamp(::aws_sdk_kinesis::primitives::DateTime::from(*instant)),
        Cursor::AfterSequenceNumber(sequence_number) => request
            .shard_iterator_type(ShardIteratorType::AfterSequenceNumber)
            .starting_sequence_number(sequence_number),
    };
    let answer = request.send().await.map_err(|error| {
        format!(
            "the shard {shard} of the source stream {} could not be opened ({})",
            run.source_arn,
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )
    })?;
    Ok(answer.shard_iterator().map(str::to_string))
}

/// One `GetRecords` from `iterator` for at most `wanted` records: the records it answered
/// and the iterator the shard is read on with.
async fn get_records(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    iterator: &str,
    wanted: usize,
) -> Result<(Vec<::aws_sdk_kinesis::types::Record>, Option<String>), String> {
    if let Some(detail) = record.refusal(GET_RECORDS_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let limit = i32::try_from(wanted).unwrap_or(i32::MAX).max(1);
    let answer = clients
        .kinesis(callback, &run.source_region)
        .get_records()
        .shard_iterator(iterator)
        .limit(limit)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the source stream {} could not be read ({})",
                run.source_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    let next = answer.next_shard_iterator().map(str::to_string);
    Ok((answer.records, next))
}

/// One polled Kinesis record as the record envelope a pipe hands to its stages: the
/// record's own data, partition key and sequence number, the shard it came from and the
/// stream it was read off.
///
/// The members are the ones EventBridge Pipes documents for a Kinesis source, at the one
/// level the record carries them: `data` is the record's bytes base64-encoded, as the
/// envelope travels them, and `approximateArrivalTimestamp` is the instant the stream
/// reports for the record, in epoch seconds.
pub(super) fn record_envelope(
    record: &::aws_sdk_kinesis::types::Record,
    shard: &str,
    run: &RunningPipe,
) -> Value {
    let sequence_number = record.sequence_number();
    let mut envelope = json!({
        "kinesisSchemaVersion": KINESIS_SCHEMA_VERSION,
        "partitionKey": record.partition_key(),
        "sequenceNumber": sequence_number,
        "data": ::aws_smithy_types::base64::encode(record.data().as_ref()),
        "eventSource": KINESIS_EVENT_SOURCE,
        "eventVersion": KINESIS_EVENT_VERSION,
        // The record is identified by the shard it came from and its own sequence number
        // within that shard.
        "eventID": format!("{shard}:{sequence_number}"),
        "eventName": KINESIS_EVENT_NAME,
        "awsRegion": run.source_region,
        "eventSourceARN": run.source_arn,
    });
    if let Some(arrived_at) = record.approximate_arrival_timestamp() {
        envelope["approximateArrivalTimestamp"] = json!(arrived_at.as_secs_f64());
    }
    envelope
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::super::{test_support::kinesis_run, Source};
    use super::{identifiers, initial_cursor, record_envelope, Cursor};

    /// One record as the Kinesis data plane answers it.
    fn record(sequence_number: &str, data: &[u8]) -> ::aws_sdk_kinesis::types::Record {
        ::aws_sdk_kinesis::types::Record::builder()
            .sequence_number(sequence_number)
            .partition_key("orders")
            .data(::aws_sdk_kinesis::primitives::Blob::new(data.to_vec()))
            .approximate_arrival_timestamp(::aws_sdk_kinesis::primitives::DateTime::from(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            ))
            .build()
            .expect("the record is well formed")
    }

    /// RULE: a polled record is handed to the pipe's stages carrying ITS OWN data,
    /// base64-encoded as the envelope travels it, beside the partition key and sequence
    /// number the stream named it by and the shard it was read from.
    ///
    /// The rows differ along what the rule generalises over: what the record's data is —
    /// a JSON document, plain text, nothing at all, and bytes that are no text.
    #[test]
    fn a_record_is_handed_over_carrying_its_own_data_base64_encoded() {
        let run = kinesis_run("LATEST");
        // (case, the record's data)
        let rows: Vec<(&str, &[u8])> = vec![
            ("a-json-document", br#"{"city":"Seattle"}"#),
            ("plain-text", b"hello"),
            ("no-data-at-all", b""),
            ("bytes-that-are-no-text", &[0x00, 0xff, 0x80]),
        ];
        for (case, data) in rows {
            let sequence_number = "49590338271490256608559692538361571095921575989136588898";
            let envelope = record_envelope(
                &record(sequence_number, data),
                "shardId-000000000000",
                &run,
            );
            assert_eq!(
                envelope["data"],
                ::serde_json::json!(::aws_smithy_types::base64::encode(data)),
                "case {case:?}: expected the record's own data base64-encoded, got {envelope:?}"
            );
            assert_eq!(
                envelope["partitionKey"],
                ::serde_json::json!("orders"),
                "case {case:?}: expected the record's partition key, got {envelope:?}"
            );
            assert_eq!(
                envelope["eventID"],
                ::serde_json::json!(format!("shardId-000000000000:{sequence_number}")),
                "case {case:?}: expected the shard and sequence number as the event id, got \
                 {envelope:?}"
            );
            assert_eq!(
                envelope["eventSource"],
                ::serde_json::json!("aws:kinesis"),
                "case {case:?}: expected the Kinesis event source, got {envelope:?}"
            );
            assert_eq!(
                envelope["eventSourceARN"],
                ::serde_json::json!(run.source_arn),
                "case {case:?}: expected the stream it was read off, got {envelope:?}"
            );
        }
        let batch = [record("s-1", b"one"), record("s-2", b"two")];
        assert_eq!(
            identifiers(&batch),
            vec!["s-1".to_string(), "s-2".to_string()],
            "a batch is named by its records' own sequence numbers"
        );
    }

    /// RULE: the position a shard this poller has not read yet is read from is the pipe's
    /// own `StartingPosition` — the oldest retained record for `TRIM_HORIZON`, the
    /// configured instant for `AT_TIMESTAMP`, and the instant the position was taken up for
    /// `LATEST`, because the records that arrive from then on are what reading just after
    /// the tip at that moment comes to.
    ///
    /// The rows differ along the configured starting position, which is what the rule
    /// generalises over, and along whether an `AT_TIMESTAMP` configuration names the instant
    /// its own member carries.
    #[test]
    fn a_shard_is_read_from_the_position_the_pipe_configured() {
        let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        // (case, the configured position, the instant it carries, the position taken up)
        let rows: Vec<(&str, &str, Option<SystemTime>, Cursor)> = vec![
            (
                "trim-horizon",
                "TRIM_HORIZON",
                None,
                Cursor::TrimHorizon,
            ),
            ("latest", "LATEST", None, Cursor::AtTimestamp(started_at)),
            (
                "at-timestamp",
                "AT_TIMESTAMP",
                Some(at),
                Cursor::AtTimestamp(at),
            ),
            (
                "at-timestamp-with-no-instant",
                "AT_TIMESTAMP",
                None,
                Cursor::TrimHorizon,
            ),
        ];
        for (case, position, instant, expected) in rows {
            let mut run = kinesis_run(position);
            if let Source::KinesisStream {
                starting_position_timestamp,
                ..
            } = &mut run.source
            {
                *starting_position_timestamp = instant;
            }
            let taken = initial_cursor(&run, started_at);
            assert_eq!(
                taken, expected,
                "case {case:?}: expected the shard to be read from {expected:?}, got {taken:?}"
            );
        }
    }
}
