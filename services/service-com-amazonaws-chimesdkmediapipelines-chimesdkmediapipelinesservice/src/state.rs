// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice service.
//!
//! The media-pipeline resources this service mints live here, in ONE store behind
//! the engine's per-service lock. `MediaCapturePipeline`, `MediaInsightsPipeline`, the
//! media insights pipeline CONFIGURATIONS and the Kinesis Video Stream POOLS are the kinds
//! wired so far; the sibling kinds (concatenation, live-connector and stream pipelines)
//! get their own collections beside [`CapturePipeline`], [`InsightsPipeline`],
//! [`InsightsConfiguration`] and [`StreamPool`] as their operations are implemented, and
//! `GetMediaPipeline`/`ListMediaPipelines` read over all of them.
//!
//! The speaker search TASKS a media insights pipeline runs live here too, beside
//! [`SpeakerSearchTaskRecord`], together with the account's voice-analytics enrolment,
//! which every voice-analytics task operation is gated on.
//!
//! Besides the store this module holds the few derivations every pipeline handler
//! shares — the ARN shape, the signed region, the modelled error shapes and the
//! pagination cursor — so no two handlers spell one of them differently.
//!
//! One thing this service reads is NOT in the store below, because the other Chime
//! services read and write it too and the engine keys state by type: the family's
//! ARN-keyed TAG directory ([`chime_tags`]), which holds the tags of every pipeline and
//! configuration — including the ones a create was given.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BadRequestException, ChimeSdkMeetingConfiguration, ConflictException, ForbiddenException,
    KinesisVideoStreamConfiguration, KinesisVideoStreamPoolConfiguration,
    KinesisVideoStreamPoolSummary, KinesisVideoStreamRecordingSourceRuntimeConfiguration,
    KinesisVideoStreamSourceRuntimeConfiguration, KinesisVideoStreamSourceTaskConfiguration,
    MediaCapturePipeline, MediaCapturePipelineSummary, MediaInsightsPipeline,
    MediaInsightsPipelineConfiguration, MediaInsightsPipelineConfigurationElement,
    MediaInsightsPipelineConfigurationSummary, MediaPipelineSummary, NotFoundException,
    RealTimeAlertConfiguration,
    ResourceLimitExceededException, S3RecordingSinkRuntimeConfiguration, ServiceFailureException,
    SpeakerSearchTask, SseAwsKeyManagementParams,
};

/// The `MediaPipelineStatus` value a freshly created pipeline carries.
/// From `smithy_query(query="shape", shape="MediaPipelineStatus")`:
/// `Initializing | InProgress | Failed | Stopping | Stopped | Paused | NotStarted`.
pub const STATUS_INITIALIZING: &str = "Initializing";

/// The `MediaPipelineStatus` value a pipeline reaches once it is running.
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// The `MediaPipelineStatus` value a pipeline carries while a caller has paused it
/// (`UpdateMediaInsightsPipelineStatus` with `Pause`).
pub const STATUS_PAUSED: &str = "Paused";

/// The `KinesisVideoStreamPoolStatus` a freshly created stream pool carries.
/// From `smithy_query(query="shape", shape="KinesisVideoStreamPoolStatus")`:
/// `CREATING | ACTIVE | UPDATING | DELETING | FAILED`.
pub const POOL_STATUS_CREATING: &str = "CREATING";

/// The `KinesisVideoStreamPoolStatus` a pool reaches once it is usable.
pub const POOL_STATUS_ACTIVE: &str = "ACTIVE";

/// The `KinesisVideoStreamPoolStatus` a pool carries while an update is being applied.
pub const POOL_STATUS_UPDATING: &str = "UPDATING";

/// The `MediaPipelineTaskStatus` a freshly started voice-analytics task carries.
/// From `smithy_query(query="shape", shape="MediaPipelineTaskStatus")`:
/// `NotStarted | Initializing | InProgress | Failed | Stopping | Stopped`. That is a
/// DIFFERENT enum from `MediaPipelineStatus` above, so a task's statuses are named here
/// rather than borrowed from a pipeline's.
pub const TASK_STATUS_INITIALIZING: &str = "Initializing";

/// The `MediaPipelineTaskStatus` a task reaches once it is running.
pub const TASK_STATUS_IN_PROGRESS: &str = "InProgress";

/// The `ErrorCode` enum value a `BadRequestException` carries.
/// From `smithy_query(query="shape", shape="ErrorCode")`.
const ERROR_CODE_BAD_REQUEST: &str = "BadRequest";

/// The `ErrorCode` enum value a `ForbiddenException` carries.
const ERROR_CODE_FORBIDDEN: &str = "Forbidden";

/// The `ErrorCode` enum value a `NotFoundException` carries.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The `ErrorCode` enum value a `ServiceFailureException` carries.
const ERROR_CODE_SERVICE_FAILURE: &str = "ServiceFailure";

/// The `ErrorCode` enum value a `ResourceLimitExceededException` carries.
const ERROR_CODE_RESOURCE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// The most media capture pipelines this store holds at once.
///
/// This is the store's RESOURCE BOUND: the record count grows once per
/// `CreateMediaCapturePipeline`, so without a maximum a caller could grow resident
/// memory without limit (per-record size is already bounded by the model's
/// `@length` constraints) and make the per-read `settle_clock` sweep arbitrarily
/// long while holding the shared lock. It is also the more faithful answer: the
/// real service enforces a per-account quota and declares
/// `ResourceLimitExceededException` (HTTP 400) on this operation for it.
///
/// The number itself is CHOSEN, not quoted: AWS's quota for concurrent media
/// pipelines is not in the model and is adjustable per account, so this is the
/// emulator's own ceiling, set far above any realistic client workflow.
pub const MAX_CAPTURE_PIPELINES: usize = 1_000;

/// The most media insights pipeline configurations this store holds at once.
///
/// The same RESOURCE BOUND as [`MAX_CAPTURE_PIPELINES`], for the collection
/// `CreateMediaInsightsPipelineConfiguration` grows: the real service enforces a
/// per-account quota and declares `ResourceLimitExceededException` (HTTP 400) on
/// that operation for it. The number itself is CHOSEN, not quoted — AWS's quota is
/// not in the model and is adjustable per account.
pub const MAX_INSIGHTS_CONFIGURATIONS: usize = 1_000;

/// The most `Elements` one stored media insights pipeline configuration may carry.
///
/// This is the store's PER-RECORD bound, and it is what makes
/// [`MAX_INSIGHTS_CONFIGURATIONS`] a memory bound rather than only a record count: the
/// element list is the one caller-chosen size inside the record that the model leaves
/// open. `smithy_query(query="shape", shape="MediaInsightsPipelineConfigurationElements")`
/// reports a list with no `@length`, so no boundary validator can refuse an arbitrarily
/// long one, while every member INSIDE an element is `@length`-bounded by the model (the
/// ARNs at 1024, the vocabulary names at 200–3000, `PostCallAnalyticsSettings`' strings at
/// 4096, `CallAnalyticsStreamCategories` at 20 entries) — so capping the COUNT caps the
/// bytes one record can hold, and the two constants together cap what this store retains.
///
/// The number is CHOSEN, not quoted: AWS publishes no element quota in the model. It is
/// set far above any realistic configuration — the model declares nine element types
/// (`MediaInsightsPipelineConfigurationElementType`), and a real configuration pairs a few
/// processors with a few sinks — so no faithful client reaches it.
pub const MAX_INSIGHTS_CONFIGURATION_ELEMENTS: usize = 64;

/// The region an unsigned request is attributed to; the emulator's default.
const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateMediaCapturePipeline` the `Initializing -> InProgress`
/// transition falls due.
///
/// `Initializing` is a NON-TERMINAL `MediaPipelineStatus`, so the real service
/// leaves that transition to its own clock rather than to a caller's call. AWS
/// publishes no duration for it, so this span is CHOSEN, and kept inside the
/// 250–750 ms window this project fixes for a chosen transition: the span is never
/// zero, so a caller reading straight back still observes `Initializing`.
pub const INITIALIZING_TO_IN_PROGRESS: Duration = Duration::from_millis(500);

/// A `MediaPipelineId`: a lowercase v4 GUID, matching the model's `GuidString`
/// pattern `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`.
///
/// Minted from a counter the STORE advances (see
/// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::mint_media_pipeline_id`]),
/// never from the request, so two creates of the same pipeline yield two ids. The
/// RFC 4122 version and variant bits are set, so the value is a well-formed v4
/// UUID and not merely 32 hex digits.
fn format_guid(high: u64, low: u64) -> String {
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (high >> 32) as u32,
        ((high >> 16) & 0xFFFF) as u16,
        (high & 0xFFFF) as u16,
        ((low >> 48) & 0xFFFF) as u16,
        low & 0x0000_FFFF_FFFF_FFFF,
    )
}

/// Wall-clock nanoseconds since the epoch, forced odd, used only to seed the id
/// counter on first use so two emulator runs do not mint the same first id.
fn seed_from_clock() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A media pipeline's ARN, in this service's ARN namespace (`chime`, per the
/// model's `aws.api#service`): `arn:aws:chime:<region>:<account>:media-pipeline/<id>`.
pub fn media_pipeline_arn(region: &str, media_pipeline_id: &str) -> String {
    ::engine::emulator_arn(
        "chime",
        region,
        &format!("{MEDIA_PIPELINE_RESOURCE_TYPE}/{media_pipeline_id}"),
    )
}

/// The resource TYPE a media pipeline's ARN carries, as [`media_pipeline_arn`] derives it.
pub const MEDIA_PIPELINE_RESOURCE_TYPE: &str = "media-pipeline";

/// The resource TYPE a media insights pipeline configuration's ARN carries, as
/// [`media_insights_pipeline_configuration_arn`] derives it.
pub const MEDIA_INSIGHTS_CONFIGURATION_RESOURCE_TYPE: &str =
    "media-insights-pipeline-configuration";

/// The resource TYPE a Kinesis Video Stream pool's ARN carries, as
/// [`kinesis_video_stream_pool_arn`] derives it.
pub const KINESIS_VIDEO_STREAM_POOL_RESOURCE_TYPE: &str =
    "media-pipeline-kinesis-video-stream-pool";

/// A Kinesis Video Stream pool's ARN, in this service's ARN namespace (`chime`):
/// `arn:aws:chime:<region>:<account>:media-pipeline-kinesis-video-stream-pool/<pool id>`.
///
/// The resource part carries the minted `PoolId` rather than the caller's `PoolName`, so
/// renaming nothing and reusing a freed name cannot make one ARN name two pools; the name
/// is reported beside it as its own member and resolves the pool too
/// ([`StreamPool::is_named_by`]).
pub fn kinesis_video_stream_pool_arn(region: &str, pool_id: &str) -> String {
    ::engine::emulator_arn(
        "chime",
        region,
        &format!("{KINESIS_VIDEO_STREAM_POOL_RESOURCE_TYPE}/{pool_id}"),
    )
}

/// The most Kinesis Video Stream pools this store holds at once.
///
/// The same RESOURCE BOUND as [`MAX_CAPTURE_PIPELINES`], for the collection
/// `CreateMediaPipelineKinesisVideoStreamPool` grows: the real service enforces a
/// per-account quota and declares `ResourceLimitExceededException` (HTTP 400) on that
/// operation for it. The number itself is CHOSEN — AWS's quota is not in the model and is
/// adjustable per account.
pub const MAX_STREAM_POOLS: usize = 1_000;

/// The most media insights pipelines this store holds at once: the bound for the
/// collection `CreateMediaInsightsPipeline` grows, for the reason [`MAX_STREAM_POOLS`]
/// gives, and the modelled `ResourceLimitExceededException` (HTTP 400) when it is reached.
pub const MAX_INSIGHTS_PIPELINES: usize = 1_000;

/// How long after `CreateMediaPipelineKinesisVideoStreamPool` — and after an update that
/// changes one — a pool's `CREATING`/`UPDATING -> ACTIVE` transition falls due.
///
/// `CREATING` and `UPDATING` are NON-TERMINAL `KinesisVideoStreamPoolStatus` values, so the
/// real service leaves that transition to its own clock rather than to a caller's call. AWS
/// publishes no duration for it, so this span is CHOSEN: long enough that a caller reading
/// straight back still observes the transient status (it is never zero), short enough not
/// to stall a client workflow.
pub const POOL_TRANSIENT_TO_ACTIVE: Duration = Duration::from_secs(3);

/// How long after a voice-analytics task is started its `Initializing -> InProgress`
/// transition falls due.
///
/// `Initializing` is a NON-TERMINAL `MediaPipelineTaskStatus`, so the real service leaves
/// that transition to its own clock rather than to a caller's call. AWS publishes no
/// duration for it, so this span is CHOSEN: never zero, so a caller reading straight back
/// still observes the transient status, and short enough not to stall a client workflow.
pub const TASK_INITIALIZING_TO_IN_PROGRESS: Duration = Duration::from_secs(3);

/// The most voice-analytics tasks this store holds at once.
///
/// The RESOURCE BOUND for the collection `StartSpeakerSearchTask` grows: the record count
/// rises once per start, so without a maximum a caller could grow resident memory without
/// limit and make the per-read `settle_clock` sweep arbitrarily long while holding the
/// shared lock. The number itself is CHOSEN, not quoted — AWS's concurrent-task quota is
/// not in the model and is adjustable per account — and set far above any realistic client
/// workflow.
pub const MAX_SPEAKER_SEARCH_TASKS: usize = 1_000;

/// How many tags one resource of THIS service carries at once.
///
/// Its own model bounds `TagList` at 200 members, not the 50 the other Chime models
/// declare (`smithy_query(query="operation",operation="TagResource")` on this model:
/// `Tags` -> `length {min: 1, max: 200}`), so this is the count the shared tag directory
/// holds a resource of this service to.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A media insights pipeline configuration's ARN, in the same (`chime`) namespace:
/// `arn:aws:chime:<region>:<account>:media-insights-pipeline-configuration/<name>`.
///
/// The resource part carries the configuration NAME, which is the key that
/// identifies the configuration in an account: the model documents
/// `Identifier` — the `@httpLabel` of the get, update and delete — as "the name and
/// ARN of the media insights pipeline configuration"
/// (`smithy_query(query="doc",shape="DeleteMediaInsightsPipelineConfigurationRequest",
/// member="Identifier")`), so the two forms name one resource. The minted
/// `MediaInsightsPipelineConfigurationId` is reported beside it as its own member.
pub fn media_insights_pipeline_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "chime",
        region,
        &format!("{MEDIA_INSIGHTS_CONFIGURATION_RESOURCE_TYPE}/{name}"),
    )
}

/// The region the caller signed this request for, from the SigV4 credential
/// scope; [`DEFAULT_REGION`] for a request carrying no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A wall-clock instant as the modelled `Iso8601Timestamp`, truncated to the
/// millisecond the service's `date-time` rendering carries.
pub fn timestamp(at: SystemTime) -> DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_and_nanos(
        since_epoch.as_secs() as i64,
        since_epoch.subsec_millis() * 1_000_000,
    )
}

/// The source of every `RequestId` this service reports.
///
/// Process-wide rather than a field of the state struct on purpose: a reporting
/// operation puts a `RequestId` on the error it answers with, and a read must not
/// write to the store it reports on.
static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// The `RequestId` to report on one response: a well-formed RFC 4122 v4 UUID string,
/// which is the shape an AWS request id has.
///
/// Every error shape of this service carries a `RequestId` member
/// (`smithy_query(query="shape",shape="NotFoundException")`), and AWS fills it on
/// every response: it is what a caller quotes when reporting a failed call. The value
/// is the monotonic counter rather than randomness, so it is unique per response
/// without being derived from the request.
pub fn next_request_id() -> String {
    let n = REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// The modelled 400 `BadRequestException`, carrying `message`.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: Some(ERROR_CODE_BAD_REQUEST.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 404 `NotFoundException`.
///
/// Takes NO message on purpose: this service words every absent-resource refusal with
/// the one [`NOT_FOUND_MESSAGE`], so no handler can compose a per-resource sentence or
/// copy an identifier out of the request into the error body.
pub fn not_found() -> NotFoundException {
    NotFoundException {
        code: Some(ERROR_CODE_NOT_FOUND.to_string()),
        message: Some(NOT_FOUND_MESSAGE.to_string()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 500 `ServiceFailureException`, carrying `message`. Returned when
/// the engine hands back no store, which is the one condition a handler cannot
/// serve through.
pub fn service_failure(message: impl Into<String>) -> ServiceFailureException {
    ServiceFailureException {
        code: Some(ERROR_CODE_SERVICE_FAILURE.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 403 `ForbiddenException`, carrying `message`.
pub fn forbidden(message: impl Into<String>) -> ForbiddenException {
    ForbiddenException {
        code: Some(ERROR_CODE_FORBIDDEN.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The `message` every voice-analytics task operation refuses an account that is not
/// enrolled for the feature with.
///
/// Biometric speaker identification is enabled per account out of band, and the model
/// states no text for the refusal, so this is RECORDED behaviour: a fixed string with no
/// variable parts — it names neither the account, the pipeline nor the task.
pub const VOICE_ANALYTICS_NOT_ENABLED_MESSAGE: &str = "AWS account is not enabled";

/// The `message` this service's `NotFoundException` carries, on every operation and for
/// every resource kind it can report absent.
///
/// The model states no text for it, so this is RECORDED behaviour: one fixed string for
/// every operation and every resource kind, with no variable parts — no resource-kind
/// noun, no name, no id, no ARN. In particular it echoes NO identifier from the request,
/// so a caller's identifier never appears in a 404 body.
pub const NOT_FOUND_MESSAGE: &str = "Unable to find the requested identifier";

/// The modelled 400 `ResourceLimitExceededException`, carrying `message`.
pub fn resource_limit_exceeded(message: impl Into<String>) -> ResourceLimitExceededException {
    ResourceLimitExceededException {
        code: Some(ERROR_CODE_RESOURCE_LIMIT_EXCEEDED.to_string()),
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The modelled 409 `ConflictException`, carrying `message`.
///
/// Its `Code` member targets the model's `ErrorCode` enum
/// (`smithy_query(query="shape", shape="ErrorCode")`), which declares no value for a
/// conflict — `BadRequest | Forbidden | NotFound | ResourceLimitExceeded | ServiceFailure
/// | ServiceUnavailable | Throttling` — so it is left ABSENT rather than filled with a
/// value the enum does not carry. The error code a client branches on is the shape name
/// (`ConflictException`), which the wire envelope carries either way.
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: None,
        message: Some(message.into()),
        request_id: Some(next_request_id()),
    }
}

/// The `message` a refused pagination token is reported with.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The next-token value is not a valid pagination token.";

/// The message a tagging call that would take a resource past
/// [`MAX_TAGS_PER_RESOURCE`] tags is refused with; `requested` is how many tags the
/// resource would have ended up with. This service's tagging operations model no
/// limit error, so the refusal is the modelled `BadRequestException` carrying this
/// sentence.
pub fn too_many_tags_message(resource_arn: &str, requested: usize) -> String {
    format!(
        "{requested} tags were requested for {resource_arn}, which is more than the maximum \
         of {MAX_TAGS_PER_RESOURCE} tags per resource"
    )
}

/// A pagination token this service did not issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// The store already holds [`MAX_CAPTURE_PIPELINES`] pipelines, so it refused to
/// grow. A create reports it as the modelled `ResourceLimitExceededException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePipelineLimitReached;

/// Why the store refused to persist a media insights pipeline configuration. A
/// create reports each as the modelled error named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightsConfigurationRefused {
    /// The store already holds [`MAX_INSIGHTS_CONFIGURATIONS`] configurations — the
    /// modelled `ResourceLimitExceededException` (HTTP 400).
    LimitReached,
    /// A configuration with that name already exists. The name is what identifies a
    /// configuration in an account (it is one of the two forms of `Identifier`), so
    /// a second one under the same name is refused — and since the model declares no
    /// `ConflictException` for the create, it is the modelled
    /// `BadRequestException` (HTTP 400).
    NameInUse,
    /// The configuration carries more than [`MAX_INSIGHTS_CONFIGURATION_ELEMENTS`]
    /// elements; the payload is how many it carried. The model puts no `@length` on the
    /// element list, so this is the one caller-chosen size in the record the boundary
    /// cannot refuse, and the store is where it is admitted or not — reported as the
    /// modelled `BadRequestException` (HTTP 400), the input-restriction failure it is.
    TooManyElements(usize),
}

/// Why the store refused to apply an UPDATE to a media insights pipeline
/// configuration. An update reports each as the modelled error named here.
///
/// A separate enum from [`InsightsConfigurationRefused`] because the two refusals
/// differ: an update never grows the collection and never changes a name, so neither
/// the record quota nor a name clash can refuse it, while "nothing is stored under
/// that identifier" can — and cannot refuse a create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightsConfigurationUpdateRefused {
    /// No configuration this store holds is named by the identifier — the modelled
    /// `NotFoundException` (HTTP 404).
    NoSuchConfiguration,
    /// The settings carry more than [`MAX_INSIGHTS_CONFIGURATION_ELEMENTS`] elements;
    /// the payload is how many they carried. The update writes the caller's element
    /// list into the record the store keeps, so the same per-record bound a create is
    /// admitted under applies here — the modelled `BadRequestException` (HTTP 400).
    TooManyElements(usize),
}

/// Why the store refused to persist a Kinesis Video Stream pool. A create reports each as
/// the modelled error named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPoolRefused {
    /// The store already holds [`MAX_STREAM_POOLS`] pools — the modelled
    /// `ResourceLimitExceededException` (HTTP 400).
    LimitReached,
    /// A pool with that name already exists. The name is one of the two forms of the
    /// `Identifier` every later read, update and delete resolves, so a second pool under
    /// one name would leave that lookup ambiguous — and the create models
    /// `ConflictException` (HTTP 409) for exactly a clash with the current state of the
    /// resource.
    NameInUse,
}

/// Why the store refused to persist a media insights pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightsPipelineRefused {
    /// The store already holds [`MAX_INSIGHTS_PIPELINES`] pipelines — the modelled
    /// `ResourceLimitExceededException` (HTTP 400).
    LimitReached,
}

/// Why the store refused to persist a voice-analytics task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerSearchTaskRefused {
    /// The store already holds [`MAX_SPEAKER_SEARCH_TASKS`] tasks. `StartSpeakerSearchTask`
    /// models no resource-limit error, so a start refused for the store's bound is reported
    /// as the modelled `ThrottledClientException` (HTTP 429) — the allowance refusal the
    /// operation does model.
    LimitReached,
}

/// Why a requested media insights pipeline status change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdateRefused {
    /// No pipeline this store holds is named by the identifier — the modelled
    /// `NotFoundException` (HTTP 404).
    NoSuchPipeline,
    /// The pipeline is not in a status the requested change can be made from; the payload
    /// is the status it is actually in, which is what the refusal reports. The modelled
    /// `ConflictException` (HTTP 409).
    WrongStatus(String),
}

/// How a refusal of too many elements is worded: built from the request (the count it
/// sent and the maximum), stated ONCE here so the admission check and the handler's
/// pre-lock check cannot word the same refusal differently.
pub fn too_many_elements_message(count: usize) -> String {
    format!(
        "Elements carries {count} elements, more than the {MAX_INSIGHTS_CONFIGURATION_ELEMENTS} \
         a media insights pipeline configuration accepts."
    )
}

/// Encode the page cursor — the creation sequence of the last item returned — as
/// the opaque token a caller echoes back in `next-token`.
///
/// The cursor is the creation SEQUENCE and not the last item's id, so deleting the
/// item a token names does not strand the pages after it. Rendered as fixed-width
/// hex so the token is a flat, protocol-safe string with no internal structure a
/// caller could rely on.
fn encode_page_token(after_seq: u64) -> String {
    format!("{after_seq:016x}")
}

/// Recover the cursor from a token [`encode_page_token`] produced; `Err` for
/// anything else, which the caller reports as a `BadRequestException`.
fn decode_page_token(token: &str) -> Result<u64, InvalidPageToken> {
    if token.len() != 16 {
        return Err(InvalidPageToken);
    }
    u64::from_str_radix(token, 16).map_err(|_| InvalidPageToken)
}

/// One persisted media capture pipeline: everything `CreateMediaCapturePipeline`
/// minted or stored, which is what every later read renders from.
#[derive(Debug, Clone)]
pub struct CapturePipeline {
    /// Creation sequence, assigned by [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_capture_pipeline`].
    /// Monotonic and never reused, so it is a stable pagination cursor.
    pub seq: u64,
    pub media_pipeline_id: String,
    pub media_pipeline_arn: String,
    pub source_type: String,
    pub source_arn: String,
    pub sink_type: String,
    pub sink_arn: String,
    pub sink_iam_role_arn: Option<String>,
    pub status: String,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
    pub chime_sdk_meeting_configuration: Option<ChimeSdkMeetingConfiguration>,
    pub sse_aws_key_management_params: Option<SseAwsKeyManagementParams>,
    /// When the `Initializing -> InProgress` transition the clock owes falls due,
    /// or `None` once it has been materialised. Stored as an instant rather than
    /// slept for, so the transition is owed whether or not anybody calls.
    pub in_progress_due_at: Option<SystemTime>,
}

impl CapturePipeline {
    /// This pipeline as the modelled `MediaCapturePipeline` every read returns.
    pub fn as_modeled(&self) -> MediaCapturePipeline {
        MediaCapturePipeline {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
            source_type: Some(self.source_type.clone()),
            source_arn: Some(self.source_arn.clone()),
            status: Some(self.status.clone()),
            sink_type: Some(self.sink_type.clone()),
            sink_arn: Some(self.sink_arn.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
            chime_sdk_meeting_configuration: self.chime_sdk_meeting_configuration.clone(),
            sse_aws_key_management_params: self.sse_aws_key_management_params.clone(),
            sink_iam_role_arn: self.sink_iam_role_arn.clone(),
        }
    }

    /// This pipeline as `ListMediaCapturePipelines` summarises it.
    pub fn as_capture_summary(&self) -> MediaCapturePipelineSummary {
        MediaCapturePipelineSummary {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
        }
    }

    /// This pipeline as `ListMediaPipelines` summarises it.
    pub fn as_pipeline_summary(&self) -> MediaPipelineSummary {
        MediaPipelineSummary {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
        }
    }
}

/// One persisted media insights pipeline configuration: everything
/// `CreateMediaInsightsPipelineConfiguration` minted or stored, which is what every
/// later read renders from.
///
/// The processors and sinks (`elements`) and the alert rules are kept exactly as the
/// caller submitted them: they are the caller's configuration, which the service
/// stores and reports back, not something it derives.
#[derive(Debug, Clone)]
pub struct InsightsConfiguration {
    /// Creation sequence, assigned by
    /// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_insights_configuration`].
    /// Monotonic and never reused, so it is a stable listing order.
    pub seq: u64,
    pub name: String,
    pub arn: String,
    pub id: String,
    pub resource_access_role_arn: String,
    pub real_time_alert_configuration: Option<RealTimeAlertConfiguration>,
    pub elements: Vec<MediaInsightsPipelineConfigurationElement>,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
}

impl InsightsConfiguration {
    /// Whether `identifier` names this configuration. The model documents the
    /// `Identifier` label as the configuration's NAME or its ARN, so both forms
    /// resolve here and nothing else does.
    pub fn is_named_by(&self, identifier: &str) -> bool {
        self.name == identifier || self.arn == identifier
    }

    /// This configuration as the modelled `MediaInsightsPipelineConfiguration`
    /// every read returns.
    pub fn as_modeled(&self) -> MediaInsightsPipelineConfiguration {
        MediaInsightsPipelineConfiguration {
            media_insights_pipeline_configuration_name: Some(self.name.clone()),
            media_insights_pipeline_configuration_arn: Some(self.arn.clone()),
            resource_access_role_arn: Some(self.resource_access_role_arn.clone()),
            real_time_alert_configuration: self.real_time_alert_configuration.clone(),
            elements: Some(self.elements.clone()),
            media_insights_pipeline_configuration_id: Some(self.id.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
        }
    }

    /// This configuration as `ListMediaInsightsPipelineConfigurations` summarises it:
    /// the three members that identify it, rendered from the stored record.
    pub fn as_summary(&self) -> MediaInsightsPipelineConfigurationSummary {
        MediaInsightsPipelineConfigurationSummary {
            media_insights_pipeline_configuration_name: Some(self.name.clone()),
            media_insights_pipeline_configuration_id: Some(self.id.clone()),
            media_insights_pipeline_configuration_arn: Some(self.arn.clone()),
        }
    }
}

/// One persisted Kinesis Video Stream pool: everything
/// `CreateMediaPipelineKinesisVideoStreamPool` minted or stored, which is what every later
/// read renders from.
#[derive(Debug, Clone)]
pub struct StreamPool {
    /// Creation sequence, assigned by
    /// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_stream_pool`].
    /// Monotonic and never reused, so it is a stable pagination cursor.
    pub seq: u64,
    pub pool_id: String,
    pub pool_name: String,
    pub pool_arn: String,
    pub pool_status: String,
    /// The caller's stream configuration, stored as submitted and reported back: it is the
    /// caller's configuration, not something the service derives.
    pub stream_configuration: KinesisVideoStreamConfiguration,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
    /// When the `CREATING`/`UPDATING -> ACTIVE` transition the clock owes falls due, or
    /// `None` once it has been materialised. Stored as an instant rather than slept for,
    /// so the transition is owed whether or not anybody calls.
    pub active_due_at: Option<SystemTime>,
}

impl StreamPool {
    /// Whether `identifier` names this pool. The model documents the `Identifier` label as
    /// the pool's NAME or its ARN, so both forms resolve here and nothing else does.
    pub fn is_named_by(&self, identifier: &str) -> bool {
        self.pool_name == identifier || self.pool_arn == identifier
    }

    /// This pool as the modelled `KinesisVideoStreamPoolConfiguration` every read returns.
    ///
    /// `PoolSize` is left ABSENT: the model documents it as "the size of the video stream
    /// pool in the configuration" — how many Kinesis Video Streams the pool holds — and
    /// this emulator provisions no Kinesis Video Stream, so there is no count to report
    /// and none is invented.
    pub fn as_modeled(&self) -> KinesisVideoStreamPoolConfiguration {
        KinesisVideoStreamPoolConfiguration {
            pool_arn: Some(self.pool_arn.clone()),
            pool_name: Some(self.pool_name.clone()),
            pool_id: Some(self.pool_id.clone()),
            pool_status: Some(self.pool_status.clone()),
            pool_size: None,
            stream_configuration: Some(self.stream_configuration.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
        }
    }

    /// This pool as `ListMediaPipelineKinesisVideoStreamPools` summarises it.
    pub fn as_summary(&self) -> KinesisVideoStreamPoolSummary {
        KinesisVideoStreamPoolSummary {
            pool_name: Some(self.pool_name.clone()),
            pool_id: Some(self.pool_id.clone()),
            pool_arn: Some(self.pool_arn.clone()),
        }
    }
}

/// One persisted media insights pipeline: everything `CreateMediaInsightsPipeline` minted
/// or stored, which is what every later read renders from.
///
/// The runtime configurations are kept exactly as the caller submitted them. The modelled
/// `MediaInsightsPipeline` carries no `UpdatedTimestamp`, so a status change moves only the
/// status.
#[derive(Debug, Clone)]
pub struct InsightsPipeline {
    /// Creation sequence, assigned by
    /// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState::insert_insights_pipeline`].
    pub seq: u64,
    pub media_pipeline_id: String,
    pub media_pipeline_arn: String,
    /// The ARN of the configuration the create resolved, as the stored configuration
    /// carries it.
    pub configuration_arn: String,
    pub status: String,
    pub kinesis_video_stream_source_runtime_configuration:
        Option<KinesisVideoStreamSourceRuntimeConfiguration>,
    pub media_insights_runtime_metadata: Option<HashMap<String, String>>,
    pub kinesis_video_stream_recording_source_runtime_configuration:
        Option<KinesisVideoStreamRecordingSourceRuntimeConfiguration>,
    pub s3_recording_sink_runtime_configuration: Option<S3RecordingSinkRuntimeConfiguration>,
    pub created_timestamp: DateTime,
    /// When the `Initializing -> InProgress` transition the clock owes falls due, or `None`
    /// once it has been materialised or the pipeline has been paused.
    pub in_progress_due_at: Option<SystemTime>,
}

impl InsightsPipeline {
    /// Whether `identifier` names this pipeline. The model documents the `Identifier`
    /// label as the pipeline's ID or its ARN, so both forms resolve here and nothing else
    /// does.
    pub fn is_named_by(&self, identifier: &str) -> bool {
        self.media_pipeline_id == identifier || self.media_pipeline_arn == identifier
    }

    /// This pipeline as the modelled `MediaInsightsPipeline` every read returns.
    ///
    /// `ElementStatuses` is left ABSENT: the model documents it as "the statuses that the
    /// elements in a media insights pipeline can have during data processing", and this
    /// emulator runs no processor or sink, so there is no per-element progress to report.
    pub fn as_modeled(&self) -> MediaInsightsPipeline {
        MediaInsightsPipeline {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
            media_insights_pipeline_configuration_arn: Some(self.configuration_arn.clone()),
            status: Some(self.status.clone()),
            kinesis_video_stream_source_runtime_configuration: self
                .kinesis_video_stream_source_runtime_configuration
                .clone(),
            media_insights_runtime_metadata: self.media_insights_runtime_metadata.clone(),
            kinesis_video_stream_recording_source_runtime_configuration: self
                .kinesis_video_stream_recording_source_runtime_configuration
                .clone(),
            s3_recording_sink_runtime_configuration: self
                .s3_recording_sink_runtime_configuration
                .clone(),
            created_timestamp: Some(self.created_timestamp),
            element_statuses: None,
        }
    }

    /// This pipeline as `ListMediaPipelines` summarises it.
    pub fn as_pipeline_summary(&self) -> MediaPipelineSummary {
        MediaPipelineSummary {
            media_pipeline_id: Some(self.media_pipeline_id.clone()),
            media_pipeline_arn: Some(self.media_pipeline_arn.clone()),
        }
    }
}

/// One persisted speaker search task: what `StartSpeakerSearchTask` minted and stored,
/// which is what every later read of the task renders from.
///
/// The task's own resources — the voice profile domain its matches are drawn from and the
/// Kinesis video stream source it reads — are kept exactly as the caller submitted them, so
/// the record says what the task was started on and no read has to recover it from a
/// request.
#[derive(Debug, Clone)]
pub struct SpeakerSearchTaskRecord {
    pub speaker_search_task_id: String,
    /// The `MediaPipelineId` of the media insights pipeline the task runs on: a task is
    /// addressed under its pipeline, so the pair is what identifies it.
    pub media_pipeline_id: String,
    pub voice_profile_domain_arn: String,
    pub kinesis_video_stream_source_task_configuration:
        Option<KinesisVideoStreamSourceTaskConfiguration>,
    pub status: String,
    pub created_timestamp: DateTime,
    pub updated_timestamp: DateTime,
    /// When the `Initializing -> InProgress` transition the clock owes falls due, or `None`
    /// once it has been materialised.
    pub in_progress_due_at: Option<SystemTime>,
}

impl SpeakerSearchTaskRecord {
    /// This task as the modelled `SpeakerSearchTask` the start and every later read return.
    pub fn as_modeled(&self) -> SpeakerSearchTask {
        SpeakerSearchTask {
            speaker_search_task_id: Some(self.speaker_search_task_id.clone()),
            speaker_search_task_status: Some(self.status.clone()),
            created_timestamp: Some(self.created_timestamp),
            updated_timestamp: Some(self.updated_timestamp),
        }
    }
}

/// Per-service state for com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState {
    /// Every created-and-not-deleted capture pipeline, in creation order, so
    /// paging over it is stable.
    capture_pipelines: Vec<CapturePipeline>,
    /// `ClientRequestToken -> MediaPipelineId`, so a replayed create answers with
    /// the pipeline the first one minted instead of minting a second.
    ///
    /// Bounded by `capture_pipelines`, and so by [`MAX_CAPTURE_PIPELINES`]: an
    /// entry is written ONLY by [`Self::insert_capture_pipeline`], beside the
    /// pipeline it minted, and dropped by [`Self::remove_capture_pipeline`], so
    /// every entry names a live pipeline and ids are unique — the map can never
    /// hold more entries than there are records.
    request_tokens: HashMap<String, String>,
    /// Every created-and-not-deleted media insights pipeline configuration, in
    /// creation order.
    insights_configurations: Vec<InsightsConfiguration>,
    /// `ClientRequestToken -> the configuration NAME`, so a replayed create answers
    /// with the configuration the first one minted instead of refusing the name.
    ///
    /// Bounded exactly as [`Self::request_tokens`] is: an entry is written ONLY by
    /// [`Self::insert_insights_configuration`], beside the configuration it minted,
    /// and dropped by [`Self::remove_insights_configuration`].
    insights_configuration_tokens: HashMap<String, String>,
    /// Every created-and-not-deleted Kinesis Video Stream pool, in creation order, so
    /// paging over it is stable.
    stream_pools: Vec<StreamPool>,
    /// `ClientRequestToken -> the pool NAME`, so a replayed create answers with the pool
    /// the first one minted instead of refusing its name. Bounded exactly as
    /// [`Self::request_tokens`] is.
    stream_pool_tokens: HashMap<String, String>,
    /// Every created-and-not-deleted media insights pipeline, in creation order.
    insights_pipelines: Vec<InsightsPipeline>,
    /// `ClientRequestToken -> the pipeline ID`, so a replayed create answers with the
    /// pipeline the first one minted instead of minting a second.
    insights_pipeline_tokens: HashMap<String, String>,
    /// The next creation sequence to hand out.
    next_seq: u64,
    /// Every started speaker search task, in start order.
    speaker_search_tasks: Vec<SpeakerSearchTaskRecord>,
    /// `ClientRequestToken -> the task ID`, so a replayed start answers with the task the
    /// first one minted instead of starting a second. Bounded exactly as
    /// [`Self::request_tokens`] is.
    speaker_search_task_tokens: HashMap<String, String>,
    /// Whether this account is enrolled for the Chime SDK's voice-analytics features.
    ///
    /// Biometric speaker identification is enabled per account out of band — no modelled
    /// operation of this service turns it on — so the stored default is NOT enrolled, which
    /// is what every voice-analytics task operation refuses against.
    voice_analytics_enabled: bool,
    /// The id counter, seeded from the clock on first use and advanced through
    /// `splitmix64` once per minted id, so no id is ever a function of a request.
    id_seed: u64,
}

impl ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState {
    /// Mint a fresh `MediaPipelineId`: a lowercase v4 GUID matching the model's
    /// `GuidString` pattern, drawn from this store's own counter.
    pub fn mint_media_pipeline_id(&mut self) -> String {
        self.mint_guid()
    }

    /// Mint a fresh `KinesisVideoStreamPoolId`, drawn from the same counter: the model
    /// bounds that id at 1..256 characters matching `^[0-9a-zA-Z._-]+$`, which a v4 GUID
    /// satisfies, so the two ids are minted by ONE derivation rather than two.
    pub fn mint_stream_pool_id(&mut self) -> String {
        self.mint_guid()
    }

    /// Mint a fresh `SpeakerSearchTaskId`, drawn from the same counter: the model bounds
    /// that id by the same 36-character `GuidString` pattern a `MediaPipelineId` carries.
    pub fn mint_speaker_search_task_id(&mut self) -> String {
        self.mint_guid()
    }

    /// The next id from this store's counter, as a well-formed lowercase v4 GUID.
    fn mint_guid(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = seed_from_clock();
        }
        self.id_seed = splitmix64(self.id_seed);
        let high = self.id_seed;
        let low = splitmix64(high ^ 0xA076_1D64_78BD_642F);
        format_guid(high, low)
    }

    /// Materialise every transition the clock already owes as of `now`.
    ///
    /// A reporting operation calls this before it answers: the change was due at
    /// `due` whether or not anybody called, so the timestamp recorded is `due` and
    /// not the instant of the call that happened to observe it.
    pub fn settle_clock(&mut self, now: SystemTime) {
        for pipeline in &mut self.capture_pipelines {
            let Some(due) = pipeline.in_progress_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            pipeline.status = STATUS_IN_PROGRESS.to_string();
            pipeline.updated_timestamp = timestamp(due);
            pipeline.in_progress_due_at = None;
        }
        for pipeline in &mut self.insights_pipelines {
            let Some(due) = pipeline.in_progress_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            pipeline.status = STATUS_IN_PROGRESS.to_string();
            pipeline.in_progress_due_at = None;
        }
        for task in &mut self.speaker_search_tasks {
            let Some(due) = task.in_progress_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            task.status = TASK_STATUS_IN_PROGRESS.to_string();
            task.updated_timestamp = timestamp(due);
            task.in_progress_due_at = None;
        }
        for pool in &mut self.stream_pools {
            let Some(due) = pool.active_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            pool.pool_status = POOL_STATUS_ACTIVE.to_string();
            pool.updated_timestamp = timestamp(due);
            pool.active_due_at = None;
        }
    }

    /// Persist `pipeline` (and, when the create carried one, the
    /// `ClientRequestToken` that minted it), assign its creation sequence, and hand
    /// back the STORED record so a create renders its response from the store.
    ///
    /// THE STORE'S BOUND: refuses before growing EITHER collection once the store
    /// already holds [`MAX_CAPTURE_PIPELINES`] records. Both growth sites are here,
    /// behind this one check, so the cap dominates them.
    pub fn insert_capture_pipeline(
        &mut self,
        mut pipeline: CapturePipeline,
        request_token: Option<String>,
    ) -> Result<&CapturePipeline, CapturePipelineLimitReached> {
        if self.capture_pipelines.len() >= MAX_CAPTURE_PIPELINES {
            return Err(CapturePipelineLimitReached);
        }
        pipeline.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.request_tokens
                .insert(token, pipeline.media_pipeline_id.clone());
        }
        self.capture_pipelines.push(pipeline);
        Ok(self
            .capture_pipelines
            .last()
            .expect("a pipeline was just pushed"))
    }

    /// The capture pipeline with this id, or `None` when none was created under it
    /// or it has been deleted.
    pub fn capture_pipeline(&self, media_pipeline_id: &str) -> Option<&CapturePipeline> {
        self.capture_pipelines
            .iter()
            .find(|pipeline| pipeline.media_pipeline_id == media_pipeline_id)
    }

    /// Remove the capture pipeline with this id, with its idempotency record.
    /// `false` when there was nothing to remove.
    pub fn remove_capture_pipeline(&mut self, media_pipeline_id: &str) -> bool {
        let before = self.capture_pipelines.len();
        self.capture_pipelines
            .retain(|pipeline| pipeline.media_pipeline_id != media_pipeline_id);
        if self.capture_pipelines.len() == before {
            return false;
        }
        self.request_tokens.retain(|_, id| id != media_pipeline_id);
        true
    }

    /// The pipeline a previous create minted under `token`, for the idempotent
    /// replay. `None` when the token is new, or named a pipeline since deleted.
    pub fn capture_pipeline_for_request_token(&self, token: &str) -> Option<&CapturePipeline> {
        let id = self.request_tokens.get(token)?;
        self.capture_pipeline(id)
    }

    /// One page of the capture pipelines, in creation order, as
    /// [`page_by_sequence`] pages every listing of this service.
    pub fn page_capture_pipelines(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<(Vec<&CapturePipeline>, Option<String>), InvalidPageToken> {
        page_by_sequence(
            self.capture_pipelines
                .iter()
                .map(|pipeline| (pipeline.seq, pipeline)),
            next_token,
            max_results,
        )
    }

    /// One page of EVERY media pipeline this store holds, whatever its kind, summarised as
    /// `ListMediaPipelines` reports them.
    ///
    /// The creation sequence is handed out by one counter across all the kinds, so sorting
    /// the kinds together by it is the order they were created in — and a page token keeps
    /// its meaning when a later create adds a pipeline of another kind.
    pub fn page_media_pipelines(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<(Vec<MediaPipelineSummary>, Option<String>), InvalidPageToken> {
        let mut every_kind: Vec<(u64, MediaPipelineSummary)> = self
            .capture_pipelines
            .iter()
            .map(|pipeline| (pipeline.seq, pipeline.as_pipeline_summary()))
            .chain(
                self.insights_pipelines
                    .iter()
                    .map(|pipeline| (pipeline.seq, pipeline.as_pipeline_summary())),
            )
            .collect();
        every_kind.sort_by_key(|(seq, _)| *seq);
        page_by_sequence(every_kind.into_iter(), next_token, max_results)
    }

    /// One page of the Kinesis Video Stream pools, in creation order.
    pub fn page_stream_pools(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<(Vec<&StreamPool>, Option<String>), InvalidPageToken> {
        page_by_sequence(
            self.stream_pools.iter().map(|pool| (pool.seq, pool)),
            next_token,
            max_results,
        )
    }

    /// Persist `pool` (and, when the create carried one, the `ClientRequestToken` that
    /// minted it), assign its creation sequence, and hand back the STORED record so a
    /// create renders its response from the store.
    ///
    /// THE STORE'S BOUND: refuses before growing either collection once the store already
    /// holds [`MAX_STREAM_POOLS`] pools, and refuses a name another live pool already
    /// carries — the name is one of the two forms of the `Identifier` every later read,
    /// update and delete resolves. Both checks are HERE, at the one growth site, so two
    /// concurrent creates of one name cannot both be admitted: the store lock serialises
    /// them and the second sees the first's record.
    pub fn insert_stream_pool(
        &mut self,
        mut pool: StreamPool,
        request_token: Option<String>,
    ) -> Result<&StreamPool, StreamPoolRefused> {
        if self.stream_pools.len() >= MAX_STREAM_POOLS {
            return Err(StreamPoolRefused::LimitReached);
        }
        if self
            .stream_pools
            .iter()
            .any(|stored| stored.pool_name == pool.pool_name)
        {
            return Err(StreamPoolRefused::NameInUse);
        }
        pool.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.stream_pool_tokens.insert(token, pool.pool_name.clone());
        }
        self.stream_pools.push(pool);
        Ok(self.stream_pools.last().expect("a pool was just pushed"))
    }

    /// The pool `identifier` names — its own name or its ARN — or `None` when none was
    /// created under it or it has been deleted.
    pub fn stream_pool(&self, identifier: &str) -> Option<&StreamPool> {
        self.stream_pools
            .iter()
            .find(|pool| pool.is_named_by(identifier))
    }

    /// The pool a previous create minted under `token`, for the idempotent replay.
    pub fn stream_pool_for_request_token(&self, token: &str) -> Option<&StreamPool> {
        let name = self.stream_pool_tokens.get(token)?;
        self.stream_pool(name)
    }

    /// Apply `data_retention_in_hours` to the pool `identifier` names and start the
    /// `UPDATING -> ACTIVE` transition the change owes, answering the STORED record.
    ///
    /// An update that names no retention change leaves the stored configuration alone and
    /// still reports the pool, which is what an update carrying no change is.
    pub fn update_stream_pool(
        &mut self,
        identifier: &str,
        data_retention_in_hours: Option<i32>,
        now: SystemTime,
    ) -> Option<&StreamPool> {
        let pool = self
            .stream_pools
            .iter_mut()
            .find(|pool| pool.is_named_by(identifier))?;
        if let Some(hours) = data_retention_in_hours {
            pool.stream_configuration.data_retention_in_hours = Some(hours);
            pool.pool_status = POOL_STATUS_UPDATING.to_string();
            pool.updated_timestamp = timestamp(now);
            pool.active_due_at = Some(now + POOL_TRANSIENT_TO_ACTIVE);
        }
        Some(pool)
    }

    /// Remove the pool `identifier` names, with its idempotency records, and answer its
    /// ARN so the caller can drop its tags. `None` when there was nothing to remove.
    pub fn remove_stream_pool(&mut self, identifier: &str) -> Option<String> {
        let (name, arn) = self
            .stream_pool(identifier)
            .map(|pool| (pool.pool_name.clone(), pool.pool_arn.clone()))?;
        self.stream_pools.retain(|pool| pool.pool_name != name);
        self.stream_pool_tokens.retain(|_, minted| *minted != name);
        Some(arn)
    }

    /// Persist `pipeline` (and, when the create carried one, the `ClientRequestToken` that
    /// minted it), assign its creation sequence, and hand back the STORED record.
    ///
    /// THE STORE'S BOUND: refuses before growing either collection once the store already
    /// holds [`MAX_INSIGHTS_PIPELINES`] pipelines.
    pub fn insert_insights_pipeline(
        &mut self,
        mut pipeline: InsightsPipeline,
        request_token: Option<String>,
    ) -> Result<&InsightsPipeline, InsightsPipelineRefused> {
        if self.insights_pipelines.len() >= MAX_INSIGHTS_PIPELINES {
            return Err(InsightsPipelineRefused::LimitReached);
        }
        pipeline.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.insights_pipeline_tokens
                .insert(token, pipeline.media_pipeline_id.clone());
        }
        self.insights_pipelines.push(pipeline);
        Ok(self
            .insights_pipelines
            .last()
            .expect("a pipeline was just pushed"))
    }

    /// The media insights pipeline `identifier` names — its own id or its ARN — or `None`
    /// when none was created under it.
    pub fn insights_pipeline(&self, identifier: &str) -> Option<&InsightsPipeline> {
        self.insights_pipelines
            .iter()
            .find(|pipeline| pipeline.is_named_by(identifier))
    }

    /// The pipeline a previous create minted under `token`, for the idempotent replay.
    pub fn insights_pipeline_for_request_token(&self, token: &str) -> Option<&InsightsPipeline> {
        let id = self.insights_pipeline_tokens.get(token)?;
        self.insights_pipeline(id)
    }

    /// Pause or resume the media insights pipeline `identifier` names.
    ///
    /// `pause` is the requested change (`UpdateStatus` `Pause` / `Resume`). A pause is
    /// made only from the RUNNING status and a resume only from the paused one: every
    /// other current status is a clash with the state of the resource, which the operation
    /// models `ConflictException` for, and the status it clashed with is carried back so
    /// the refusal can name it. A pause also drops the transition the clock owed, since a
    /// paused pipeline is not on its way to running.
    pub fn set_insights_pipeline_paused(
        &mut self,
        identifier: &str,
        pause: bool,
        now: SystemTime,
    ) -> Result<&InsightsPipeline, StatusUpdateRefused> {
        // The status a pause or resume may be made from is decided against the status the
        // clock already owes, not the one the record happens to still carry.
        self.settle_clock(now);
        let pipeline = self
            .insights_pipelines
            .iter_mut()
            .find(|pipeline| pipeline.is_named_by(identifier))
            .ok_or(StatusUpdateRefused::NoSuchPipeline)?;
        let from = if pause {
            STATUS_IN_PROGRESS
        } else {
            STATUS_PAUSED
        };
        if pipeline.status != from {
            return Err(StatusUpdateRefused::WrongStatus(pipeline.status.clone()));
        }
        pipeline.status = if pause {
            STATUS_PAUSED.to_string()
        } else {
            STATUS_IN_PROGRESS.to_string()
        };
        pipeline.in_progress_due_at = None;
        Ok(pipeline)
    }

    /// Whether this account is enrolled for the voice-analytics features, which is what
    /// every voice-analytics task operation is gated on.
    pub fn voice_analytics_enabled(&self) -> bool {
        self.voice_analytics_enabled
    }

    /// Record whether this account is enrolled for the voice-analytics features. The
    /// enrolment is made out of band — no modelled operation of this service carries it —
    /// so this is the one way the store's flag moves.
    pub fn set_voice_analytics_enabled(&mut self, enabled: bool) {
        self.voice_analytics_enabled = enabled;
    }

    /// Persist `task` (and, when the start carried one, the `ClientRequestToken` that minted
    /// it) and hand back the STORED record, so a start renders its response from the store.
    ///
    /// THE STORE'S BOUND: refuses before growing either collection once the store already
    /// holds [`MAX_SPEAKER_SEARCH_TASKS`] tasks.
    pub fn insert_speaker_search_task(
        &mut self,
        task: SpeakerSearchTaskRecord,
        request_token: Option<String>,
    ) -> Result<&SpeakerSearchTaskRecord, SpeakerSearchTaskRefused> {
        if self.speaker_search_tasks.len() >= MAX_SPEAKER_SEARCH_TASKS {
            return Err(SpeakerSearchTaskRefused::LimitReached);
        }
        if let Some(token) = request_token {
            self.speaker_search_task_tokens
                .insert(token, task.speaker_search_task_id.clone());
        }
        self.speaker_search_tasks.push(task);
        Ok(self
            .speaker_search_tasks
            .last()
            .expect("a task was just pushed"))
    }

    /// The speaker search task `task_id` names ON the media insights pipeline
    /// `media_pipeline_id`, or `None` when that pipeline has no such task: a task is
    /// addressed under the pipeline it was started on, so the same id under another pipeline
    /// resolves to nothing.
    pub fn speaker_search_task(
        &self,
        media_pipeline_id: &str,
        task_id: &str,
    ) -> Option<&SpeakerSearchTaskRecord> {
        self.speaker_search_tasks.iter().find(|task| {
            task.media_pipeline_id == media_pipeline_id && task.speaker_search_task_id == task_id
        })
    }

    /// The task a previous start minted under `token`, for the idempotent replay.
    pub fn speaker_search_task_for_request_token(
        &self,
        token: &str,
    ) -> Option<&SpeakerSearchTaskRecord> {
        let task_id = self.speaker_search_task_tokens.get(token)?;
        self.speaker_search_tasks
            .iter()
            .find(|task| task.speaker_search_task_id == *task_id)
    }

    /// Persist `configuration` (and, when the create carried one, the
    /// `ClientRequestToken` that minted it), assign its creation sequence, and hand
    /// back the STORED record so a create renders its response from the store.
    ///
    /// Refuses before growing either collection once the store already holds
    /// [`MAX_INSIGHTS_CONFIGURATIONS`] records, and refuses a name another live
    /// configuration already carries: the name is one of the two forms of the
    /// `Identifier` every later read, update and delete resolves, so two
    /// configurations under one name would leave that lookup ambiguous.
    pub fn insert_insights_configuration(
        &mut self,
        mut configuration: InsightsConfiguration,
        request_token: Option<String>,
    ) -> Result<&InsightsConfiguration, InsightsConfigurationRefused> {
        // The record's own caller-chosen size, checked BEFORE any mutation: the element
        // list is the one part of the record the model leaves unbounded, and this is the
        // store's growth site, so admission is decided here as well as at the handler's
        // pre-lock check — the constant cannot fall out of use while this is the gate.
        if configuration.elements.len() > MAX_INSIGHTS_CONFIGURATION_ELEMENTS {
            return Err(InsightsConfigurationRefused::TooManyElements(
                configuration.elements.len(),
            ));
        }
        if self.insights_configurations.len() >= MAX_INSIGHTS_CONFIGURATIONS {
            return Err(InsightsConfigurationRefused::LimitReached);
        }
        if self
            .insights_configurations
            .iter()
            .any(|stored| stored.name == configuration.name)
        {
            return Err(InsightsConfigurationRefused::NameInUse);
        }
        configuration.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(token) = request_token {
            self.insights_configuration_tokens
                .insert(token, configuration.name.clone());
        }
        self.insights_configurations.push(configuration);
        Ok(self
            .insights_configurations
            .last()
            .expect("a configuration was just pushed"))
    }

    /// The configuration `identifier` names — its own name or its ARN — or `None`
    /// when none was created under it or it has been deleted.
    pub fn insights_configuration(&self, identifier: &str) -> Option<&InsightsConfiguration> {
        self.insights_configurations
            .iter()
            .find(|configuration| configuration.is_named_by(identifier))
    }

    /// One page of the media insights pipeline configurations, in creation order, as
    /// [`page_by_sequence`] pages every listing of this service.
    pub fn page_insights_configurations(
        &self,
        next_token: Option<&str>,
        max_results: Option<i32>,
    ) -> Result<(Vec<&InsightsConfiguration>, Option<String>), InvalidPageToken> {
        page_by_sequence(
            self.insights_configurations
                .iter()
                .map(|configuration| (configuration.seq, configuration)),
            next_token,
            max_results,
        )
    }

    /// The configuration a previous create minted under `token`, for the idempotent
    /// replay. `None` when the token is new, or named a configuration since deleted.
    pub fn insights_configuration_for_request_token(
        &self,
        token: &str,
    ) -> Option<&InsightsConfiguration> {
        let name = self.insights_configuration_tokens.get(token)?;
        self.insights_configuration(name)
    }

    /// Replace the SETTINGS of the configuration `identifier` names — the access role,
    /// the processors and sinks and the alert rules — and hand back the STORED record so
    /// an update renders its response from the store.
    ///
    /// The settings are everything a caller may change; the configuration's IDENTITY —
    /// its name, ARN, id and creation timestamp — is minted once by the create and is
    /// left alone here, so every identifier that resolved the record before still
    /// resolves it. `updated_timestamp` moves to `now` because the record really did
    /// change.
    ///
    /// `real_time_alert_configuration` is applied as given, absence included: the
    /// settings are replaced wholesale, so an update that names no alert configuration
    /// leaves the configuration with none.
    pub fn update_insights_configuration(
        &mut self,
        identifier: &str,
        resource_access_role_arn: String,
        real_time_alert_configuration: Option<RealTimeAlertConfiguration>,
        elements: Vec<MediaInsightsPipelineConfigurationElement>,
        now: SystemTime,
    ) -> Result<&InsightsConfiguration, InsightsConfigurationUpdateRefused> {
        // The identifier is resolved FIRST: "nothing is stored under that identifier" is
        // the modelled refusal, so it is authoritative over this store's own per-record
        // admission bound below.
        if !self
            .insights_configurations
            .iter()
            .any(|configuration| configuration.is_named_by(identifier))
        {
            return Err(InsightsConfigurationUpdateRefused::NoSuchConfiguration);
        }
        // The record's own caller-chosen size, checked BEFORE any mutation and against
        // the same bound a create is admitted under: an update replaces the element list,
        // so it is the other way a record grows.
        if elements.len() > MAX_INSIGHTS_CONFIGURATION_ELEMENTS {
            return Err(InsightsConfigurationUpdateRefused::TooManyElements(
                elements.len(),
            ));
        }
        let configuration = self
            .insights_configurations
            .iter_mut()
            .find(|configuration| configuration.is_named_by(identifier))
            .expect("the identifier resolved a configuration just above");
        configuration.resource_access_role_arn = resource_access_role_arn;
        configuration.real_time_alert_configuration = real_time_alert_configuration;
        configuration.elements = elements;
        configuration.updated_timestamp = timestamp(now);
        Ok(configuration)
    }

    /// Remove the configuration `identifier` names, with its idempotency record.
    /// `false` when there was nothing to remove.
    pub fn remove_insights_configuration(&mut self, identifier: &str) -> bool {
        let Some(name) = self
            .insights_configuration(identifier)
            .map(|configuration| configuration.name.clone())
        else {
            return false;
        };
        self.insights_configurations
            .retain(|configuration| configuration.name != name);
        self.insights_configuration_tokens
            .retain(|_, minted| *minted != name);
        true
    }

    /// Whether a tagging call may address `arn`.
    ///
    /// The family's one rule for that question lives in
    /// [`::service_common::chime_tags::addresses_taggable_resource`]; this store answers
    /// the half of it only this service can — whether an ARN that names a resource THIS
    /// service mints (a [`MEDIA_PIPELINE_RESOURCE_TYPE`] or a
    /// [`MEDIA_INSIGHTS_CONFIGURATION_RESOURCE_TYPE`]) is one it still holds. An ARN of
    /// any other Chime resource type is another family member's, which this store cannot
    /// check.
    pub fn taggable(&self, arn: &str) -> bool {
        let own_resource_exists = match ::service_common::arn::resource_type(arn) {
            // Every kind of media pipeline carries this resource type, so the id is
            // looked for among all the kinds the store holds.
            Some(MEDIA_PIPELINE_RESOURCE_TYPE) => Some(
                ::service_common::arn::resource_id_of_type(arn, MEDIA_PIPELINE_RESOURCE_TYPE)
                    .is_some_and(|id| {
                        self.capture_pipeline(id).is_some() || self.insights_pipeline(id).is_some()
                    }),
            ),
            Some(KINESIS_VIDEO_STREAM_POOL_RESOURCE_TYPE) => {
                // A pool is named by its ARN as well as by its name, which is what
                // `StreamPool::is_named_by` reads.
                Some(self.stream_pool(arn).is_some())
            }
            // A configuration is named by its ARN as well as by its name, which is what
            // `InsightsConfiguration::is_named_by` reads.
            Some(MEDIA_INSIGHTS_CONFIGURATION_RESOURCE_TYPE) => {
                Some(self.insights_configuration(arn).is_some())
            }
            _ => None,
        };
        ::service_common::chime_tags::addresses_taggable_resource(arn, own_resource_exists)
    }
}

/// One page of records ordered by their creation SEQUENCE: the items, and the token to
/// resume after them — present ONLY when at least one record remains beyond the page.
///
/// `next_token` resumes after the cursor it carries; `max_results` truncates. An absent
/// `max_results` returns everything remaining — as does a non-positive one, which the
/// modelled 1..100 range already refuses at the boundary; treating it as "no limit" rather
/// than as a page size of zero keeps the function total, so no caller can ever be handed an
/// empty page with no resume token while records remain.
///
/// Every listing this service serves pages through here, so the cursor rule is stated once
/// and the collections cannot drift apart on it. `items` must be in ascending sequence
/// order, which is the order each collection is held in.
fn page_by_sequence<T, I>(
    items: I,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), InvalidPageToken>
where
    I: Iterator<Item = (u64, T)>,
{
    let after = match next_token.filter(|token| !token.is_empty()) {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };
    let remaining: Vec<(u64, T)> = items
        .filter(|(seq, _)| after.is_none_or(|cursor| *seq > cursor))
        .collect();
    let limit = max_results
        .filter(|max| *max > 0)
        .map_or(remaining.len(), |max| max as usize);
    let more = remaining.len() > limit;
    let page: Vec<(u64, T)> = remaining.into_iter().take(limit).collect();
    let token = if more {
        page.last().map(|(seq, _)| encode_page_token(*seq))
    } else {
        None
    };
    Ok((page.into_iter().map(|(_, item)| item).collect(), token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState>)
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the records of
/// this store, because the family's generic tagging routes are served by one member for
/// every member's resource: a copy inside
/// [`ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState`] would be
/// invisible to the sibling serving a tagging call, which would then report "no tags" for
/// a pipeline that has them.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}
