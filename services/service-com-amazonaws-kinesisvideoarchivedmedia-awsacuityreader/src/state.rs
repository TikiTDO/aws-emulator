// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader service.
//!
//! This service is a READ plane over a fragment archive: it has no create
//! operation of its own, so everything the handlers answer is computed from
//! state somebody else wrote.
//!
//! A stream itself belongs to the Kinesis Video Streams CONTROL plane
//! (`CreateStream` creates it, `UpdateDataRetention` changes its retention,
//! `DeleteStream` removes it), so a read resolves the stream it addresses in
//! that service's registry in the shared engine — not in a second copy of it
//! held here. The FRAGMENTS of a resolved stream are this service's own
//! archive, populated through
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::put_stream`] /
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::append_fragment`],
//! which an ingest path (or a seed/test harness) calls on the one shared engine
//! store; a registered stream nothing has been ingested into simply has an
//! empty archive.
//!
//! The query helpers (stream resolution, timestamp-range filtering, pagination
//! cursors) live here beside the data they read, so every operation resolves a
//! stream and filters fragments by exactly the same rule.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;

/// Milliseconds since the Unix epoch for `t`, negative for an instant before it.
///
/// Every timestamp comparison in this service happens in this integer space, so
/// a range bound and a fragment timestamp are compared exactly rather than
/// through a lossy float.
#[must_use]
pub fn epoch_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis())
            .map_or(i64::MIN, |millis| -millis),
    }
}

/// Milliseconds since the Unix epoch for a wire timestamp.
///
/// Exact: the `DateTime` seconds and sub-second nanos are combined as integers,
/// so a request's range bound lands in the same integer space the archive's own
/// timestamps are compared in.
#[must_use]
pub fn date_time_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// The instant `millis` milliseconds from the Unix epoch names, and the epoch
/// itself for an offset the platform's clock cannot represent.
///
/// TOTAL, through the checked arithmetic: `millis` spans the whole `i64` range
/// (`date_time_millis` saturates a wire timestamp into it rather than rejecting
/// one), and near either end the offset is some nine quadrillion seconds, which
/// the plain `Add`/`Sub` impls PANIC on wherever the clock cannot hold it. The
/// epoch is the answer for such a value because it names no instant a fragment
/// can be stamped at, and the only alternative is a panicking conversion of a
/// number a caller chose.
#[must_use]
pub fn from_epoch_millis(millis: i64) -> SystemTime {
    let offset = Duration::from_millis(millis.unsigned_abs());
    let shifted = if millis >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    };
    shifted.unwrap_or(UNIX_EPOCH)
}

/// `t` as the wire timestamp a response member carries, at the millisecond
/// precision the archive records.
#[must_use]
pub fn to_date_time(t: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis(t))
}

/// Which of a fragment's two timestamps a selector reads.
///
/// The wire values are the `FragmentSelectorType` / `ClipFragmentSelectorType` /
/// `HLSFragmentSelectorType` enum value set (`PRODUCER_TIMESTAMP`,
/// `SERVER_TIMESTAMP`) — one Rust type because all three enums carry the same
/// two values and mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampOrigin {
    /// The timestamp the producer stamped on the fragment.
    Producer,
    /// The timestamp the service stamped when it received the fragment.
    Server,
}

impl TimestampOrigin {
    /// Parse a wire enum value, or `None` when it is outside the value set.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "PRODUCER_TIMESTAMP" => Some(Self::Producer),
            "SERVER_TIMESTAMP" => Some(Self::Server),
            _ => None,
        }
    }

    /// The wire enum value for this origin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Producer => "PRODUCER_TIMESTAMP",
            Self::Server => "SERVER_TIMESTAMP",
        }
    }

    /// The timestamp of `fragment` this origin selects.
    #[must_use]
    pub fn of(self, fragment: &StoredFragment) -> SystemTime {
        match self {
            Self::Producer => fragment.producer_timestamp,
            Self::Server => fragment.server_timestamp,
        }
    }
}

/// One archived fragment: its identity, its two timestamps, its duration and
/// the media bytes the read operations concatenate.
#[derive(Debug, Clone)]
pub struct StoredFragment {
    /// The fragment number, a decimal string that increases with ingestion
    /// order (`^[0-9]+$`).
    pub fragment_number: String,
    /// The timestamp the producer stamped on the fragment.
    pub producer_timestamp: SystemTime,
    /// The timestamp the service stamped when it received the fragment.
    pub server_timestamp: SystemTime,
    /// The playback duration of the fragment.
    pub duration_millis: u64,
    /// The archived media bytes of this fragment.
    pub media: Vec<u8>,
}

impl StoredFragment {
    /// An archived fragment carrying `media`.
    #[must_use]
    pub fn new(
        fragment_number: impl Into<String>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
        duration_millis: u64,
        media: Vec<u8>,
    ) -> Self {
        Self {
            fragment_number: fragment_number.into(),
            producer_timestamp,
            server_timestamp,
            duration_millis,
            media,
        }
    }

    /// The size the `Fragment$FragmentSizeInBytes` member reports.
    ///
    /// DERIVED from the stored media rather than held as a second field: a
    /// stored size could disagree with the bytes `GetMediaForFragmentList`
    /// returns for the same fragment, and then one of the two is a lie.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.media.len() as u64
    }
}

/// One stream's archive.
#[derive(Debug, Clone)]
pub struct ArchivedStream {
    /// The stream name (`StreamName`).
    pub name: String,
    /// The stream ARN (`StreamARN`).
    pub arn: String,
    /// How long the stream retains data. `0` means no retention is configured,
    /// which is what makes the archived-media operations answer
    /// `NoDataRetentionException`.
    pub data_retention_in_hours: u32,
    /// The stream's `MediaType` (e.g. `video/h264`).
    pub media_type: String,
    /// The content type of the archived container, reported by
    /// `GetMediaForFragmentList$ContentType`. Kinesis Video Streams archives
    /// fragments as Matroska (MKV), served as `video/webm`.
    pub archive_content_type: String,
    /// The stream's codec private data, when the producer supplied it.
    pub codec_private_data: Option<Vec<u8>>,
    /// The archive, kept in ingestion order (ascending fragment number).
    pub fragments: Vec<StoredFragment>,
}

/// The content type Kinesis Video Streams archives fragments as: the archive is
/// Matroska, served as `video/webm`.
pub const ARCHIVE_CONTENT_TYPE: &str = "video/webm";

/// The ceiling on the bytes ONE archived-media response may assemble.
///
/// The service publishes this ceiling for the clip it builds itself ("The
/// output will contain the first 100 MB or the first 200 fragments from the
/// specified start timestamp"), so the same number bounds every payload this
/// service assembles. It is a bound on the PRODUCT, not on the request: the
/// modelled list and page limits cap how many fragments a request may name,
/// but a caller may name the largest archived fragment repeatedly, so the
/// bytes must be bounded separately from the count. The limit is published as
/// a round "100 MB" with no unit base, so one is chosen here and named.
pub const MAX_PAYLOAD_BYTES: usize = 100 * 1024 * 1024;

/// The most fragments one stream's archive HOLDS.
///
/// A ceiling on the STORE, not an AWS limit: this emulator keeps archived media
/// in the process's memory, and an ingest plane may keep writing to a stream for
/// as long as the emulator runs, so an archive that only ever grew would consume
/// the host. [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::append_fragment`]
/// is the one site that grows it and the one that enforces this.
pub const MAX_ARCHIVED_FRAGMENTS_PER_STREAM: usize = 1024;

/// The most media bytes one stream's archive HOLDS, for the same reason.
///
/// It bounds the PRODUCT rather than the count, because a count ceiling alone
/// would admit that many of the largest fragment a producer can write. The
/// figure is one second of the media rate Kinesis Video Streams states for a
/// media session ("up to 25 megabytes per second", `GetMedia`), so a stream's
/// whole retained archive still fits inside one [`MAX_PAYLOAD_BYTES`] response.
pub const MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM: usize = 25_000_000;

/// The refusal every archived-media operation that models
/// `NoDataRetentionException` carries: the error is raised for "a stream that
/// does not retain data (that is, has a `DataRetentionInHours` of 0)", and the
/// condition is that retention value alone, so the message is entirely fixed
/// text naming no stream.
pub const NO_DATA_RETENTION_MESSAGE: &str = "The stream data retention is set to 0 hours.";

impl ArchivedStream {
    /// A stream with an empty archive, with the ARN shape the model's
    /// `ResourceARN` pattern requires
    /// (`arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<creation epoch seconds>`).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        region: &str,
        created_at: SystemTime,
        data_retention_in_hours: u32,
    ) -> Self {
        let name = name.into();
        let created_secs = epoch_millis(created_at).max(0) / 1000;
        let arn = ::engine::emulator_arn(
            "kinesisvideo",
            region,
            &format!("stream/{name}/{created_secs}"),
        );
        Self {
            name,
            arn,
            data_retention_in_hours,
            media_type: "video/h264".to_string(),
            archive_content_type: ARCHIVE_CONTENT_TYPE.to_string(),
            codec_private_data: None,
            fragments: Vec::new(),
        }
    }

    /// Whether this stream retains data at all. A stream with no retention
    /// holds no archive, which the archived-media operations report as
    /// `NoDataRetentionException`.
    #[must_use]
    pub fn retains_data(&self) -> bool {
        self.data_retention_in_hours > 0
    }

    /// The fragment with this fragment number, if the archive holds it.
    #[must_use]
    pub fn fragment(&self, fragment_number: &str) -> Option<&StoredFragment> {
        fragment_in(&self.fragments, fragment_number)
    }

    /// The archived fragments whose `origin` timestamp falls in the inclusive
    /// range `[start_millis, end_millis]`, in ingestion order.
    #[must_use]
    pub fn fragments_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> Vec<&StoredFragment> {
        fragments_in_range(&self.fragments, origin, start_millis, end_millis)
    }

    /// The total size of `fragments`, saturating rather than overflowing.
    #[must_use]
    pub fn total_bytes(fragments: &[&StoredFragment]) -> usize {
        fragments
            .iter()
            .fold(0usize, |sum, fragment| {
                sum.saturating_add(fragment.media.len())
            })
    }

    /// The media bytes of `fragments`, concatenated in the order given, or
    /// `Err(total)` when those bytes would exceed [`MAX_PAYLOAD_BYTES`].
    ///
    /// The ceiling is checked BEFORE anything is allocated, so no caller can
    /// materialise an unbounded response: a request names a bounded COUNT of
    /// fragments (the model's list and page limits), but a count says nothing
    /// about bytes, and a list may name one large fragment many times.
    pub fn concatenate(fragments: &[&StoredFragment]) -> Result<Vec<u8>, usize> {
        let total = Self::total_bytes(fragments);
        if total > MAX_PAYLOAD_BYTES {
            return Err(total);
        }
        let mut payload = Vec::with_capacity(total);
        for fragment in fragments {
            payload.extend_from_slice(&fragment.media);
        }
        Ok(payload)
    }
}

/// The fragment of `fragments` carrying this fragment number, if there is one.
///
/// The one lookup rule every operation that addresses a fragment by number
/// uses, so [`ArchivedStream`] and [`ResolvedStream`] cannot drift apart.
#[must_use]
pub fn fragment_in<'a>(
    fragments: &'a [StoredFragment],
    fragment_number: &'_ str,
) -> Option<&'a StoredFragment> {
    fragments
        .iter()
        .find(|f| f.fragment_number == fragment_number)
}

/// The fragments of `fragments` whose `origin` timestamp falls in the inclusive
/// range `[start_millis, end_millis]`, in ingestion order.
///
/// The one range rule every selector-driven operation uses.
#[must_use]
pub fn fragments_in_range(
    fragments: &[StoredFragment],
    origin: TimestampOrigin,
    start_millis: i64,
    end_millis: i64,
) -> Vec<&StoredFragment> {
    fragments
        .iter()
        .filter(|f| {
            let at = epoch_millis(origin.of(f));
            at >= start_millis && at <= end_millis
        })
        .collect()
}

/// The one stream an archived-media read addresses.
///
/// Its IDENTITY and RETENTION are the Kinesis Video Streams control plane's,
/// read through [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::resolve_stream`]
/// on every call, so a retention change the control plane accepted is observed
/// by the very next read rather than by a copy taken earlier. Its FRAGMENTS are
/// this service's archive for that stream, which is empty for a stream nothing
/// has been ingested into.
#[derive(Debug)]
pub struct ResolvedStream<'a> {
    /// The stream's `StreamName`.
    pub name: String,
    /// The stream's `StreamARN`.
    pub arn: String,
    /// How long the stream retains data, in hours. `0` means no retention is
    /// configured, which is what makes the archived-media operations answer
    /// `NoDataRetentionException`.
    pub data_retention_in_hours: i64,
    /// The content type of the archived container, reported by
    /// `GetMediaForFragmentList$ContentType`.
    pub archive_content_type: String,
    /// This stream's archive, in ingestion order (ascending fragment number).
    pub fragments: &'a [StoredFragment],
}

impl ResolvedStream<'_> {
    /// Whether this stream retains data at all. A stream with no retention
    /// holds no archive, which the archived-media operations report as
    /// `NoDataRetentionException`.
    #[must_use]
    pub fn retains_data(&self) -> bool {
        self.data_retention_in_hours > 0
    }

    /// The fragment with this fragment number, if the archive holds it.
    #[must_use]
    pub fn fragment(&self, fragment_number: &str) -> Option<&StoredFragment> {
        fragment_in(self.fragments, fragment_number)
    }

    /// The archived fragments whose `origin` timestamp falls in the inclusive
    /// range `[start_millis, end_millis]`, in ingestion order.
    #[must_use]
    pub fn fragments_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> Vec<&StoredFragment> {
        fragments_in_range(self.fragments, origin, start_millis, end_millis)
    }

    /// The manifest/playlist snapshot a streaming session over this stream
    /// serves at `now`: the fragment numbers `window` selects, at most
    /// `max_fragments` of them.
    ///
    /// EMPTY when the session would serve nothing, which both streaming-session
    /// operations report as `ResourceNotFoundException` with
    /// [`NO_FRAGMENTS_MESSAGE`]. The one selection rule both of them use, so an
    /// HLS and an MPEG-DASH session over the same archive and the same
    /// parameters carry the same fragments.
    #[must_use]
    pub fn session_fragment_numbers(
        &self,
        window: SessionWindow,
        origin: TimestampOrigin,
        max_fragments: usize,
        now: SystemTime,
    ) -> Vec<String> {
        match window {
            SessionWindow::Live => {
                // LIVE serves the most recently INGESTED fragments and only
                // exists while the stream head is fresh. The ordering is by
                // server timestamp whatever `FragmentSelectorType` the request
                // names, because that is what the selector's own documentation
                // says of LIVE: "the most recently ingested fragments based on
                // server timestamps are included" — "this means that even if
                // fragments ingested in the past have producer timestamps with
                // values now, they are not included".
                let window_start = epoch_millis(now - LIVE_FRAGMENT_WINDOW);
                let fresh = self
                    .fragments
                    .iter()
                    .any(|fragment| epoch_millis(fragment.server_timestamp) >= window_start);
                if !fresh {
                    return Vec::new();
                }
                let mut newest: Vec<&StoredFragment> = self.fragments.iter().collect();
                newest.sort_by_key(|fragment| epoch_millis(fragment.server_timestamp));
                let skip = newest.len().saturating_sub(max_fragments);
                newest
                    .into_iter()
                    .skip(skip)
                    .map(|fragment| fragment.fragment_number.clone())
                    .collect()
            }
            // ON_DEMAND and LIVE_REPLAY serve the OLDEST fragments in the
            // requested range, up to the maximum.
            SessionWindow::Range {
                start_millis,
                end_millis,
            } => self
                .fragments_in_range(origin, start_millis, end_millis)
                .into_iter()
                .take(max_fragments)
                .map(|fragment| fragment.fragment_number.clone())
                .collect(),
        }
    }
}

/// `Expires` when a streaming-session request names none: "The default is 300
/// (5 minutes)."
pub const DEFAULT_SESSION_EXPIRES_SECS: u32 = 300;

/// How many fragments a session's manifest or playlist carries when the request
/// names no maximum and the playback mode is LIVE or LIVE_REPLAY: "The default
/// is 5 fragments if PlaybackMode is LIVE or LIVE_REPLAY".
pub const DEFAULT_LIVE_SESSION_FRAGMENTS: usize = 5;

/// How many fragments a session's manifest or playlist carries when the request
/// names no maximum and the playback mode is ON_DEMAND: "and 1,000 if
/// PlaybackMode is ON_DEMAND".
pub const DEFAULT_ON_DEMAND_SESSION_FRAGMENTS: usize = 1000;

/// How recent a fragment must be for a LIVE session to exist: the
/// `ResourceNotFoundException` documentation states the error is thrown "if a
/// session with a PlaybackMode of LIVE is requested for a stream that has no
/// fragments within the last 30 seconds".
pub const LIVE_FRAGMENT_WINDOW: Duration = Duration::from_secs(30);

/// The refusal a streaming-session request carries when the stream holds no
/// fragment the session could serve — none in the requested range for
/// `ON_DEMAND`/`LIVE_REPLAY`, none in the last 30 seconds for `LIVE`, the two
/// cases `ResourceNotFoundException`'s own documentation gives these
/// operations. Entirely fixed text: it names neither the stream nor the window.
pub const NO_FRAGMENTS_MESSAGE: &str =
    "No fragments found in the stream for the streaming request.";

/// The host a session URL names when the request carries neither a `Host`
/// header nor an authority to read the emulator's own endpoint from. A
/// loopback literal, never a real AWS host: the session this URL names is
/// served by this emulator.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// The URL a minted streaming session is reported in: the manifest or playlist
/// resource of `protocol`, carrying `token`, on the endpoint the CALLER reached
/// this emulator on.
///
/// The host is read from the request itself — its `Host` header, else the
/// authority of the URI it addressed, else [`LOOPBACK_HOST`] — exactly as the
/// real service answers on its own data endpoint, and deliberately names no
/// real AWS host: this emulator serves the playback session, so a player or
/// test that follows the URL must come back here. The one rule both
/// streaming-session operations use, so they cannot drift on which endpoint a
/// session URL names.
#[must_use]
pub fn session_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    protocol: SessionProtocol,
    token: &str,
) -> String {
    let host = ctx
        .parts()
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| ctx.parts().uri.host())
        .unwrap_or(LOOPBACK_HOST);
    format!(
        "https://{host}/{}/v1/{}?SessionToken={token}",
        protocol.url_prefix(),
        protocol.manifest_resource()
    )
}

/// Which fragments of a stream a streaming session serves, as the request's
/// `PlaybackMode` and fragment selector describe them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionWindow {
    /// `LIVE`: the most recently INGESTED fragments, and only while the stream
    /// head is fresh — a LIVE session over a stream with no fragment in the
    /// last [`LIVE_FRAGMENT_WINDOW`] serves nothing.
    Live,
    /// `ON_DEMAND` / `LIVE_REPLAY`: the oldest fragments whose selected
    /// timestamp falls in the inclusive requested range. An unset bound leaves
    /// that end of the range open.
    Range {
        /// The inclusive start of the requested range, in epoch milliseconds.
        start_millis: i64,
        /// The inclusive end of the requested range, in epoch milliseconds.
        end_millis: i64,
    },
}

/// Which playback protocol a streaming session serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProtocol {
    /// HTTP Live Streaming, from `GetHLSStreamingSessionURL`.
    Hls,
    /// MPEG-DASH, from `GetDASHStreamingSessionURL`.
    Dash,
}

impl SessionProtocol {
    /// The path segment the session's playlist URL uses.
    #[must_use]
    pub fn url_prefix(self) -> &'static str {
        match self {
            Self::Hls => "hls",
            Self::Dash => "dash",
        }
    }

    /// The resource a media player fetches first for this protocol: the HLS
    /// master playlist (`GetHLSMasterPlaylist`) or the MPEG-DASH manifest
    /// (`GetDASHManifest`), the "root resource needed for streaming" each
    /// operation's own documentation names.
    #[must_use]
    pub fn manifest_resource(self) -> &'static str {
        match self {
            Self::Hls => "getHLSMasterPlaylist.m3u8",
            Self::Dash => "getMPDManifest.mpd",
        }
    }
}

/// A recorded streaming session: what a session URL resolves back to.
#[derive(Debug, Clone)]
pub struct StreamingSession {
    /// The opaque session token carried in the session URL.
    pub token: String,
    /// The name of the stream the session plays.
    pub stream_name: String,
    /// The protocol the session serves.
    pub protocol: SessionProtocol,
    /// The session's `PlaybackMode`.
    pub playback_mode: String,
    /// The session's `ContainerFormat`. An MPEG-DASH session carries no such
    /// parameter: its manifest serves the MP4 initialization and media
    /// fragments `GetMP4InitFragment`/`GetMP4MediaFragment` answer, so it is
    /// recorded as the fragmented-MP4 container.
    pub container_format: String,
    /// The session's `DiscontinuityMode`, which only an HLS session carries:
    /// `None` for an MPEG-DASH session, whose input models no such parameter.
    pub discontinuity_mode: Option<String>,
    /// The session's `DisplayFragmentTimestamp`.
    pub display_fragment_timestamp: String,
    /// The session's `DisplayFragmentNumber`, which only an MPEG-DASH session
    /// carries: `None` for an HLS session, whose input models no such
    /// parameter.
    pub display_fragment_number: Option<String>,
    /// The playlist snapshot: the fragment numbers the session serves, capped
    /// by the request's max-results.
    pub fragment_numbers: Vec<String>,
    /// When the session was created.
    pub created_at: SystemTime,
    /// How long after `created_at` the session URL stays valid.
    pub expires_in_secs: u32,
}

impl StreamingSession {
    /// When this session's URL stops being valid.
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.created_at + Duration::from_secs(u64::from(self.expires_in_secs))
    }
}

/// The refusal an identifier no stream answers to carries, reported as
/// `ResourceNotFoundException` (404) — the error raised when Kinesis Video
/// Streams "can't find the stream that you specified". Entirely fixed text: it
/// names neither the stream nor which identifier form was used.
///
/// The control plane words its own 404 with the same sentence, and this copy is
/// DELIBERATE rather than an oversight: each service states the refusals it
/// answers with, and a reader of this crate must be able to see what it sends
/// without following a dependency.
pub const STREAM_NOT_FOUND_MESSAGE: &str = "The requested stream is not found or not active.";

/// The refusal a request that does not name exactly one stream carries.
///
/// `StreamName` and `StreamARN` are each documented as "You must specify either
/// the StreamName or the StreamARN", so one identifier is required and two are
/// a contradiction; the message states that whole rule, which is why both cases
/// carry it.
pub const ONE_STREAM_IDENTIFIER_MESSAGE: &str =
    "One of StreamName or StreamARN must be provided, but not both.";

/// The refusal a timestamp range whose end does not come after its start
/// carries: `EndTimestamp` "must be later than the StartTimestamp value".
pub const TIMESTAMP_RANGE_ORDER_MESSAGE: &str = "StartTimestamp must be before EndTimestamp.";

/// Why a `StreamName`/`StreamARN` pair did not name one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLookupError {
    /// The request did not name exactly one stream: a client error the
    /// archived-media operations report as `InvalidArgumentException` (400),
    /// stated with [`ONE_STREAM_IDENTIFIER_MESSAGE`].
    InvalidArgument(String),
    /// Exactly one of the two was given, and no stream answers to it: reported
    /// as `ResourceNotFoundException` (404), stated with
    /// [`STREAM_NOT_FOUND_MESSAGE`].
    NotFound(String),
}

/// A pagination cursor over one stream's archive, carried in `NextToken`.
///
/// Self-describing, so a replayed token continues where the previous page
/// stopped without the service holding per-token server state: it carries the
/// stream it was issued for, the selector the first page used (the model makes
/// `FragmentSelector` optional precisely because a continuation re-uses the
/// selector the token remembers) and the offset the next page starts at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentCursor {
    /// The name of the stream the token was issued for.
    pub stream_name: String,
    /// The timestamp field the first page selected on.
    pub origin: TimestampOrigin,
    /// The inclusive start of the selected range.
    pub start_millis: i64,
    /// The inclusive end of the selected range.
    pub end_millis: i64,
    /// How many results earlier pages already returned: matching fragments for a
    /// fragment listing, sampled instants for an image page.
    pub offset: usize,
}

impl FragmentCursor {
    /// Encode this cursor as a `NextToken`.
    ///
    /// Base64 of the cursor's JSON, which satisfies the model's
    /// `^[a-zA-Z0-9+/]+={0,2}$` token pattern (the standard base64 alphabet
    /// plus at most two `=` of padding).
    #[must_use]
    pub fn encode(&self) -> String {
        let json = ::serde_json::json!({
            "s": self.stream_name,
            "o": self.origin.as_str(),
            "f": self.start_millis,
            "t": self.end_millis,
            "i": self.offset,
        });
        ::aws_smithy_types::base64::encode(json.to_string())
    }

    /// Decode a `NextToken` produced by [`Self::encode`], or `None` when it is
    /// not one.
    #[must_use]
    pub fn decode(token: &str) -> Option<Self> {
        let raw = ::aws_smithy_types::base64::decode(token).ok()?;
        let json: ::serde_json::Value = ::serde_json::from_slice(&raw).ok()?;
        Some(Self {
            stream_name: json.get("s")?.as_str()?.to_string(),
            origin: TimestampOrigin::parse(json.get("o")?.as_str()?)?,
            start_millis: json.get("f")?.as_i64()?,
            end_millis: json.get("t")?.as_i64()?,
            offset: usize::try_from(json.get("i")?.as_u64()?).ok()?,
        })
    }
}

/// The most streaming sessions this service keeps at once.
///
/// A session is minted per request, so the store that holds them needs a
/// ceiling of its own: expiry alone bounds nothing inside one `Expires` window
/// (up to 12 hours). The number is this emulator's own choice — AWS publishes
/// no per-account session cap — and it is well above any playback a caller can
/// have in flight, so reaching it means sessions are being created and
/// abandoned, which is exactly when the oldest is the right one to drop.
pub const MAX_STREAMING_SESSIONS: usize = 1024;

/// Per-service state for com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Every stream's archive, keyed by stream name.
    pub streams: HashMap<String, ArchivedStream>,
    /// `StreamARN` -> stream name, so an ARN and a name resolve to the same
    /// [`ArchivedStream`] rather than to two copies of one stream.
    pub arn_index: HashMap<String, String>,
    /// Every minted streaming session, keyed by its session token.
    pub sessions: HashMap<String, StreamingSession>,
    /// Monotonic counter behind [`Self::mint_session_token`], so two sessions
    /// minted in the same instant still get distinct tokens.
    session_ordinal: u64,
}

impl ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Add (or replace) a stream's archive, keeping [`Self::arn_index`] in step.
    ///
    /// Replacing a stream drops the ARN the previous entry was indexed under:
    /// a re-created stream has a new ARN (its creation instant is part of it),
    /// and a stale ARN that still resolved to the live stream would answer a
    /// request that named a stream which no longer exists.
    pub fn put_stream(&mut self, stream: ArchivedStream) {
        if let Some(replaced) = self.streams.get(&stream.name) {
            if replaced.arn != stream.arn {
                self.arn_index.remove(&replaced.arn);
            }
        }
        self.arn_index
            .insert(stream.arn.clone(), stream.name.clone());
        self.streams.insert(stream.name.clone(), stream);
    }

    /// Append one ingested fragment to a stream's archive, keeping it in
    /// ascending fragment-number order. Answers `false` when no such stream
    /// exists.
    ///
    /// The archive is already ordered, so the insertion point is found by
    /// binary search rather than by re-sorting the whole archive per ingest.
    ///
    /// What one stream's archive HOLDS is bounded here, at the one site that
    /// grows it: this emulator keeps archived media in the process's memory and
    /// an ingest plane may keep writing for as long as the emulator runs, so
    /// once an archive is at [`MAX_ARCHIVED_FRAGMENTS_PER_STREAM`] or
    /// [`MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM`] its OLDEST fragments are dropped
    /// to make room for the newest. The reads then report the fragments the
    /// stream still holds, which is what they already do for a window that holds
    /// none. A `NextToken` minted before a drop counts from the start of the
    /// result list, so a page taken across one may skip a fragment — the same
    /// artifact a real archive's retention produces mid-pagination.
    pub fn append_fragment(&mut self, stream_name: &str, fragment: StoredFragment) -> bool {
        let Some(stream) = self.streams.get_mut(stream_name) else {
            return false;
        };
        let at = stream
            .fragments
            .binary_search_by(|held| {
                numeric_order(&held.fragment_number, &fragment.fragment_number)
            })
            .unwrap_or_else(|insertion_point| insertion_point);
        stream.fragments.insert(at, fragment);
        // The newest fragment is never dropped, however large it is: an archive
        // that held nothing a producer just wrote would report media it never
        // took.
        let mut bytes: usize = stream
            .fragments
            .iter()
            .map(|held| held.media.len())
            .sum();
        while stream.fragments.len() > MAX_ARCHIVED_FRAGMENTS_PER_STREAM
            || (bytes > MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM && stream.fragments.len() > 1)
        {
            let dropped = stream.fragments.remove(0);
            bytes = bytes.saturating_sub(dropped.media.len());
        }
        true
    }

    /// Resolve the one stream a request names.
    ///
    /// The model documents `StreamName` and `StreamARN` as "Specify either this
    /// parameter or the StreamName parameter", so exactly one must be present:
    /// neither and both are client errors.
    ///
    /// A stream EXISTS because the Kinesis Video Streams control plane created
    /// it, so the identifier is resolved in that service's registry in the
    /// shared engine first, and its record is what the stream's ARN and current
    /// retention are read from — re-read here on every call, so a retention
    /// change is visible to the next read. A registered stream is READ only
    /// while its record is in the one status that serves reads, `ACTIVE`: a
    /// record the registry still holds in a non-terminal status — created and
    /// not yet active, or marked for deletion — is as unreadable as one the
    /// registry does not hold at all, and is reported with the same
    /// [`STREAM_NOT_FOUND_MESSAGE`] ("not found or not active"). An identifier
    /// neither that registry nor this service's own archive answers to is a
    /// missing resource too.
    ///
    /// The control-plane store is locked while this service's own store is
    /// already locked (the caller holds that guard); the control plane never
    /// reads this service's state, so there is no cycle between the two.
    pub fn resolve_stream<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<ResolvedStream<'_>, StreamLookupError> {
        match (stream_name, stream_arn) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(StreamLookupError::InvalidArgument(
                    ONE_STREAM_IDENTIFIER_MESSAGE.to_string(),
                ))
            }
            _ => {}
        }

        // The stream as the control plane holds it right now, with this
        // service's archive for it — which is empty for a registered stream
        // nothing has been ingested into, and that emptiness is what the
        // per-operation no-fragments behaviour is computed from.
        if let Some(registered) = registered_stream(engine, stream_name, stream_arn) {
            // A record the registry holds in a status that does not serve reads
            // is not a readable stream: the archived-media plane refuses it as
            // the missing resource it is, rather than reporting the archive of
            // a stream that is not there yet or is on its way out.
            if !registered.readable() {
                return Err(StreamLookupError::NotFound(
                    STREAM_NOT_FOUND_MESSAGE.to_string(),
                ));
            }
            let archive = self.streams.get(&registered.name);
            return Ok(ResolvedStream {
                name: registered.name,
                arn: registered.arn,
                data_retention_in_hours: registered.data_retention_in_hours,
                archive_content_type: archive.map_or_else(
                    || ARCHIVE_CONTENT_TYPE.to_string(),
                    |held| held.archive_content_type.clone(),
                ),
                fragments: archive.map_or(&[][..], |held| held.fragments.as_slice()),
            });
        }

        // No control-plane record: the archive's own entry is the registration
        // of a stream whose fragments were ingested into this store directly,
        // so it answers for itself rather than being reported absent.
        let archive = match (stream_name, stream_arn) {
            (Some(name), _) => self.streams.get(name),
            (_, Some(arn)) => self.arn_index.get(arn).and_then(|name| self.streams.get(name)),
            (None, None) => None,
        };
        match archive {
            Some(held) => Ok(ResolvedStream {
                name: held.name.clone(),
                arn: held.arn.clone(),
                data_retention_in_hours: i64::from(held.data_retention_in_hours),
                archive_content_type: held.archive_content_type.clone(),
                fragments: held.fragments.as_slice(),
            }),
            None => Err(StreamLookupError::NotFound(
                STREAM_NOT_FOUND_MESSAGE.to_string(),
            )),
        }
    }

    /// Mint a fresh, unique streaming-session token.
    ///
    /// Opaque and URL-safe: a monotonic ordinal and the creation instant, so
    /// two sessions for the same stream and the same parameters never share a
    /// token.
    pub fn mint_session_token(&mut self, created_at: SystemTime) -> String {
        self.session_ordinal = self.session_ordinal.wrapping_add(1);
        let nanos = created_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{:016x}{:032x}", self.session_ordinal, nanos)
    }

    /// Record a minted session, so the URL it is reported in resolves back to
    /// the session's own parameters and playlist snapshot.
    ///
    /// The store is BOUNDED here, at the one site that grows it, because a
    /// session is minted per request:
    ///
    /// * every session whose `Expires` window the clock has already closed is
    ///   dropped — the session record carries its own TTL
    ///   ([`StreamingSession::expires_at`]), and an expired playback session
    ///   can no longer be used ("When a session expires, no new calls to
    ///   GetHLSMasterPlaylist ... can be made for that session"), so removing
    ///   it changes nothing a caller could still observe;
    /// * and if live sessions alone reach [`MAX_STREAMING_SESSIONS`], the
    ///   oldest is evicted to make room, so the map can never hold more than
    ///   that many entries however many requests arrive.
    ///
    /// Every operation that mints a session — HLS and MPEG-DASH alike —
    /// records it through this one method, so the bound covers them all.
    pub fn record_session(&mut self, session: StreamingSession) {
        let now = session.created_at;
        self.sessions.retain(|_, held| held.expires_at() > now);
        while self.sessions.len() >= MAX_STREAMING_SESSIONS {
            let oldest = self
                .sessions
                .values()
                .min_by_key(|held| held.created_at)
                .map(|held| held.token.clone());
            match oldest {
                Some(token) => {
                    self.sessions.remove(&token);
                }
                None => break,
            }
        }
        self.sessions.insert(session.token.clone(), session);
    }
}

/// Order two fragment numbers the way ingestion order does: numerically when
/// both are decimal (the model's `^[0-9]+$`), lexicographically otherwise, so a
/// non-decimal number still sorts deterministically instead of panicking.
fn numeric_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u128>(), b.parse::<u128>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// What the Kinesis Video Streams control plane holds for one stream: the half
/// of a [`ResolvedStream`] this service does not own.
#[derive(Debug, Clone)]
struct RegisteredStream {
    /// The registered `StreamName`.
    name: String,
    /// The registered `StreamARN`.
    arn: String,
    /// The stream's retention AS OF THIS CALL.
    data_retention_in_hours: i64,
    /// The stream's `Status` AS OF THIS CALL, which decides whether the record
    /// is one an archived-media read may serve.
    status: String,
}

impl RegisteredStream {
    /// Whether this record is one the archived-media plane reads.
    ///
    /// `ACTIVE` is the only stream status that serves reads: the other values
    /// `StreamInfo$Status` takes are the non-terminal ones a stream passes
    /// through on its way in or out, and the archived-media plane words its
    /// refusal for both halves at once — the stream is "not found or not
    /// active".
    fn readable(&self) -> bool {
        self.status == control_plane::STATUS_ACTIVE
    }
}

/// The stream the Kinesis Video Streams control plane registry holds under this
/// `StreamName`/`StreamARN`, or `None` when it holds none.
///
/// The registry is that service's state in the SHARED engine, so a stream a
/// client created through `CreateStream` is the very record read here — no copy
/// of it is kept on this side, and nothing here writes to it.
///
/// The registry's own clock is settled first, which is not a mutation this read
/// invents: the move out of `CREATING` and the removal of a stream marked for
/// deletion were each owed at a fixed instant whether or not anybody called, so
/// a read that did not settle them would report a stream in a state the clock
/// had already left — a deleted stream, in particular, would stay readable for
/// as long as nothing else touched the control plane.
///
/// The record's `Status` travels with it, because a stream is readable here only
/// while the control plane holds it `ACTIVE`.
fn registered_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<RegisteredStream> {
    let registry = control_plane::service_state(engine)?;
    let mut registry = registry.lock();
    registry.settle_clock();
    let stream = registry.resolve(stream_name, stream_arn).ok()?;
    Some(RegisteredStream {
        name: stream.stream_name.clone(),
        arn: stream.stream_arn.clone(),
        data_retention_in_hours: i64::from(stream.data_retention_in_hours),
        status: stream.status.clone(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>)
}

#[cfg(test)]
mod tests {
    use super::{epoch_millis, from_epoch_millis};
    use std::time::UNIX_EPOCH;

    /// RULE: the epoch-millisecond conversion is TOTAL over the whole `i64`
    /// range — every offset the platform's clock can represent round-trips, and
    /// one it cannot answers the epoch instead of panicking.
    ///
    /// The rows span what the conversion generalises over: the sign and the
    /// magnitude (zero, a millisecond, a realistic archive instant, a thousand
    /// years).
    #[test]
    fn the_epoch_conversion_round_trips_every_representable_offset() {
        // (case, the millisecond offset)
        let rows: [(&str, i64); 6] = [
            ("the epoch itself", 0),
            ("one millisecond after", 1),
            ("one millisecond before", -1),
            ("an archived fragment's instant", 1_761_000_000_000),
            ("an instant before the epoch", -1_761_000_000_000),
            ("a thousand years on", 31_556_952_000_000),
        ];
        for (case, millis) in rows {
            let actual = epoch_millis(from_epoch_millis(millis));
            assert_eq!(
                actual, millis,
                "case {case:?}: expected {millis} ms from the epoch, got {actual}"
            );
        }
    }

    /// RULE (the other half): at either end of the `i64` range the conversion
    /// ANSWERS — the offset there is some nine quadrillion seconds, which the
    /// plain `Add`/`Sub` impls panic on. A platform whose clock represents it
    /// reads the same number back; one whose clock does not answers the epoch.
    #[test]
    fn the_epoch_conversion_answers_at_both_ends_of_the_range() {
        for (case, millis) in [
            ("the largest i64", i64::MAX),
            ("one short of the largest", i64::MAX - 1),
            ("the smallest i64", i64::MIN),
            ("one past the smallest", i64::MIN + 1),
        ] {
            let instant = from_epoch_millis(millis);
            let actual = epoch_millis(instant);
            assert!(
                actual == millis || instant == UNIX_EPOCH,
                "case {case:?}: expected {millis} ms from the epoch or the epoch itself, got \
                 {actual}"
            );
        }
    }
}
