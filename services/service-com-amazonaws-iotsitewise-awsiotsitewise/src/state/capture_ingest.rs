// SPDX-License-Identifier: CC0-1.0

//! Ingesting the video of a workspace's capture sources — the PRODUCER side of
//! [`CaptureSourceRecord`], which `GetCaptureData` reads back and the
//! workspace-scoped time-series operations report.
//!
//! A bulk import names "the files in the specified Amazon S3 bucket that contain
//! your data", and each file states the format it is in: `csv`, `parquet`, `mp4`
//! or `annotation` (`smithy_query(query="shape", shape="FileFormat")`). An `mp4`
//! file IS video, and the two members that place it say whose video it is and
//! when it starts — "the alias associated with the file's time series" and "the
//! nanosecond-precision start time for the file data" — so an accepted import of
//! such a file into a workspace is what puts video on that workspace's capture
//! source.
//!
//! The bytes come from the object the file names, read from the emulator's OWN
//! Amazon S3 through the one shared object read every service makes that hop with
//! ([`::service_common::s3_object_read::read_object`]), so a file no object store
//! serves brings in nothing and no media is invented. The span a stored segment
//! covers is the file's own: the instant the file states, up to that instant plus
//! the duration the MEDIA declares, read out of the container itself.

use super::*;

/// The `VideoDataType` of the media an `mp4` file carries. Source:
/// `smithy_query(query="shape", shape="VideoDataType")`, whose only value is this
/// one ([`VIDEO_DATA_TYPE_VALUES`]).
pub const VIDEO_DATA_TYPE_MP4: &str = "VIDEO-MP4";

/// How many video files ONE import reads.
///
/// A memory bound, not an AWS quota: every file read is held in this process as
/// the media bytes of a segment, so without a cap one request could grow the
/// emulator without limit. Together with [`MAX_CAPTURE_FILE_BYTES`] it bounds
/// what ONE import can bring in at all — the product of the two, and not either
/// factor alone, is what a request can make this process hold. `CreateBulkImportJob`
/// REFUSES an import naming more video files than this ([`is_video_file`] counts
/// them) rather than accepting one and leaving the rest unread, so the bound is
/// stated here and enforced before anything is written.
pub const MAX_IMPORTED_CAPTURE_FILES: usize = 8;

/// How many bytes of ONE video file an import reads. The read is RANGED to it, so
/// a larger object is reported as unreadable rather than buffered whole.
pub const MAX_CAPTURE_FILE_BYTES: usize = 4 * 1024 * 1024;

/// How many nanoseconds one second holds — what a duration stated in a media
/// timescale is converted against.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// One file's video, as an import read it: the time series the file's alias
/// names, and the segment that file's media and span make.
#[derive(Debug, Clone)]
pub struct IngestedCapture {
    /// "The alias associated with the file's time series" — which capture source
    /// of the workspace this video belongs to.
    pub alias: String,
    /// The media and the span it covers, ready for
    /// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`].
    pub segment: CaptureSegment,
}

/// Whether `file` is a video file: the format the FILE declares, or — for a file
/// declaring none — the format the job declares for every file it imports
/// ("contains the configuration information of a job, such as the file format
/// used to save data in Amazon S3").
pub fn is_video_file(file: &File, job_format: Option<&FileFormat>) -> bool {
    match &file.file_format {
        Some(format) => format.mp4.is_some(),
        None => job_format.is_some_and(|format| format.mp4.is_some()),
    }
}

/// The video the `mp4` files of `files` carry, in the order the request listed
/// them.
///
/// A file contributes nothing — and is passed over rather than refused, exactly
/// as a bulk import passes over a file naming a stream this store does not hold —
/// when it names no alias (nothing says whose video it is), states no start time
/// (nothing says when the video begins), the object store serves no such object
/// or serves one past [`MAX_CAPTURE_FILE_BYTES`], or the bytes are not a
/// container this reader can take a duration from. Nothing is stored for a file
/// that was not really read.
///
/// The `take` is the same bound `CreateBulkImportJob` refuses an import past, so
/// no file of an ACCEPTED import is passed over by it; it is restated here so the
/// read cannot grow this process further than the bound whoever calls it allows.
pub async fn read_capture_files(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    files: &[File],
    job_format: Option<&FileFormat>,
) -> Vec<IngestedCapture> {
    let mut ingested = Vec::new();
    for file in files
        .iter()
        .filter(|file| is_video_file(file, job_format))
        .take(MAX_IMPORTED_CAPTURE_FILES)
    {
        let (Some(alias), Some(start)) = (file.alias.as_deref(), file.start_time.as_ref()) else {
            continue;
        };
        // The ONE read of an object a service was pointed at: the AWS SDK over
        // this emulator's in-process transport, ranged to the ceiling above.
        // Every answer other than the bytes themselves — the store holds nothing
        // there, will not serve it, cannot serve it, or nothing answered at all —
        // establishes no video, so the file brings in none.
        let ::service_common::s3_object_read::ObjectRead::Served(data) =
            ::service_common::s3_object_read::read_object(
                ctx.callback(),
                region,
                &file.bucket,
                &file.key,
                file.version_id.as_deref(),
                MAX_CAPTURE_FILE_BYTES,
            )
            .await
        else {
            continue;
        };
        let Some(segment) = capture_segment(start, data) else {
            continue;
        };
        ingested.push(IngestedCapture {
            alias: alias.to_string(),
            segment,
        });
    }
    ingested
}

/// The segment the video `data` makes when it starts at `start`, or `None` when
/// `data` is no readable container, declares no duration, or the span would run
/// past what the modelled `TimeInNanos` can express.
///
/// The media is stored EXACTLY as it arrived — nothing is decoded or re-encoded —
/// and the span is HALF-OPEN, as every span this store keeps is: it ends where
/// the media's own duration ends, so two files covering back-to-back spans leave
/// no instant covered twice.
pub fn capture_segment(start: &TimeInNanos, data: Vec<u8>) -> Option<CaptureSegment> {
    let duration = media_duration(&data)?;
    let end = instant_after(start, duration)?;
    Some(CaptureSegment {
        start_time: start.clone(),
        end_time: end,
        data,
    })
}

/// How long the media in `data` runs, read from the container's own movie header:
/// its duration in the timescale it states. `None` when `data` is not a container
/// this reader can read, states no timescale to read a duration against, or
/// declares a duration of no time at all.
///
/// The bytes are a CALLER's, so the read happens inside a panic barrier: a reader
/// that trips an assertion on a container nobody wrote by the book would
/// otherwise take the whole request with it, and a file no duration can be read
/// from is already an import that brings in nothing.
fn media_duration(data: &[u8]) -> Option<::std::time::Duration> {
    let (timescale, units) = ::std::panic::catch_unwind(|| {
        let size = u64::try_from(data.len()).ok()?;
        let media = ::mp4::Mp4Reader::read_header(::std::io::Cursor::new(data), size).ok()?;
        Some((media.moov.mvhd.timescale, media.moov.mvhd.duration))
    })
    .ok()
    .flatten()?;
    let timescale = u64::from(timescale);
    if timescale == 0 {
        return None;
    }
    let nanos = u128::from(units)
        .checked_mul(u128::from(NANOS_PER_SECOND))?
        .checked_div(u128::from(timescale))?;
    if nanos == 0 {
        return None;
    }
    Some(::std::time::Duration::from_nanos(u64::try_from(nanos).ok()?))
}

/// The instant `duration` after `at`, or `None` when that is past what the
/// modelled `TimeInNanos` can express ([`is_modelled_time`]).
fn instant_after(at: &TimeInNanos, duration: ::std::time::Duration) -> Option<TimeInNanos> {
    let offset = i64::from(at.offset_in_nanos.unwrap_or(0))
        .checked_add(i64::from(duration.subsec_nanos()))?;
    let seconds = at
        .time_in_seconds
        .checked_add(i64::try_from(duration.as_secs()).ok()?)?
        .checked_add(offset / i64::from(NANOS_PER_SECOND))?;
    let next = TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: i32::try_from(offset % i64::from(NANOS_PER_SECOND)).ok(),
    };
    is_modelled_time(&next).then_some(next)
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Store the video `ingested` carries on the capture sources of
    /// `workspace_name`, answering how many segments were stored.
    ///
    /// Each file's alias names ONE capture source of the workspace
    /// ([`Self::capture_time_series_id`]), and the write goes through the one
    /// producer entry point every stored segment is written by
    /// ([`Self::put_capture_segment`]), so what a read of that video answers
    /// from was written in exactly one place. A segment that entry point refuses
    /// stores nothing and is passed over: the rest of the import still lands.
    pub fn ingest_capture_segments(
        &mut self,
        workspace_name: &str,
        ingested: &[IngestedCapture],
        now: SystemTime,
    ) -> usize {
        let mut stored = 0usize;
        for capture in ingested {
            let time_series_id = self.capture_time_series_id(workspace_name, &capture.alias);
            let written = self.put_capture_segment(
                workspace_name,
                &time_series_id,
                Some(&capture.alias),
                VIDEO_DATA_TYPE_MP4,
                capture.segment.clone(),
                now,
            );
            if written.is_ok() {
                stored += 1;
            }
        }
        stored
    }

    /// The `TimeSeriesId` the capture source of `workspace_name` that `alias`
    /// names holds, minting one for a source that does not exist yet.
    ///
    /// An alias names ONE data stream, so the id is taken from the stream the
    /// alias already answers to rather than minted beside it: the capture source
    /// of that workspace when one holds the alias — which is what makes a second
    /// import of the same stream extend that source instead of creating a second
    /// one — then the account-wide stream the alias resolves to
    /// ([`Self::resolve_alias_series`]), so a workspace's video and the stream an
    /// asset property owns report one id. Only an alias no stream answers to at
    /// all gets a newly minted id, drawn until it is free in that workspace.
    pub fn capture_time_series_id(&self, workspace_name: &str, alias: &str) -> String {
        if let Some(source) = self.capture_source_by_alias(workspace_name, alias) {
            return source.time_series_id.clone();
        }
        if let Some(held) = self
            .resolve_alias_series(alias)
            .and_then(|key| self.series_time_series_id(&key))
        {
            return held;
        }
        loop {
            let candidate = new_id();
            if self
                .capture_source_by_id(workspace_name, &candidate)
                .is_none()
            {
                return candidate;
            }
        }
    }
}
