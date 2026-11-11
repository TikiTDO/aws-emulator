// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideo-kinesisvideo-20170930 service.
//!
//! One store behind one lock, shaped per resource so the signaling-channel,
//! edge-configuration and tagging slices can be layered in beside the stream
//! index without a second copy of any record.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a stream's media APIs are addressed at, which is not this service's own
/// endpoint — see the module's own header.
pub mod data_endpoint;
/// What answers at an announced data endpoint, and over which transport.
pub mod data_plane_serving;

pub use data_endpoint::{
    data_plane_of, ensure_data_endpoint, published_plane, serve_on_data_plane, service_host,
    DataPlane, DataPlaneEndpoint, DATA_PLANE_SCHEME,
};

use crate::ops::types::{
    ChannelInfo, EdgeConfig, ImageGenerationConfiguration, MappedResourceConfigurationListItem,
    MediaStorageConfiguration, NotificationConfiguration, SingleMasterConfiguration, StreamInfo,
    StreamStorageConfiguration,
};

/// The region used when a request carries no SigV4 credential scope to read one
/// from (an anonymous or host-routed call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `Status` values, from `smithy_query(query="shape", shape="Status")` via
/// [`crate::ops::types::STATUS_VALUES`].
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";

/// `UpdateDataRetentionOperation` values, from
/// `smithy_query(query="shape", shape="UpdateDataRetentionOperation")` via
/// [`crate::ops::types::UPDATE_DATA_RETENTION_OPERATION_VALUES`]: which way
/// `UpdateDataRetention` moves a stream's retention.
pub const INCREASE_DATA_RETENTION: &str = "INCREASE_DATA_RETENTION";
/// See [`INCREASE_DATA_RETENTION`].
pub const DECREASE_DATA_RETENTION: &str = "DECREASE_DATA_RETENTION";

/// The most hours a stream may retain data for: "The minimum value for data retention
/// is 0 and the maximum value is 87600 (ten years)"
/// (`UpdateDataRetentionInput$DataRetentionChangeInHours` docs), so a published bound
/// rather than one chosen here.
pub const MAX_DATA_RETENTION_IN_HOURS: i64 = 87_600;

/// The `DefaultStorageTier` a stream carries when the create call named no
/// `StreamStorageConfiguration`: `HOT`, the tier `DefaultStorageTier`'s docs describe as
/// "Optimized for frequent access with the lowest latency and highest performance", from
/// [`crate::ops::types::DEFAULT_STORAGE_TIER_VALUES`].
pub const DEFAULT_STORAGE_TIER: &str = "HOT";

/// `SyncStatus` values, from `smithy_query(query="shape", shape="SyncStatus")` via
/// [`crate::ops::types::SYNC_STATUS_VALUES`]. `StartEdgeConfigurationUpdate` documents
/// the pair this emulator moves between: "a new edge configuration will be created for
/// the stream, and the sync status will be set to `SYNCING`. You will have to wait for
/// the sync status to reach a terminal state such as: `IN_SYNC`, or `SYNC_FAILED`".
pub const SYNC_STATUS_SYNCING: &str = "SYNCING";
/// See [`SYNC_STATUS_SYNCING`].
pub const SYNC_STATUS_IN_SYNC: &str = "IN_SYNC";

/// How long a just-submitted edge configuration reports `SYNCING` before the clock owes
/// it `IN_SYNC`.
///
/// `StartEdgeConfigurationUpdate` is "an asynchronous API" whose sync "time to sync can
/// vary and depends on the connectivity of the Hub Device", so the configuration is left
/// in a non-terminal sync status and the move to the terminal one is owed by the clock
/// rather than performed by whoever reads it next. AWS publishes no duration for it, so
/// this span is CHOSEN — deliberately non-zero, so a caller that describes the
/// configuration immediately still observes `SYNCING` and the refusal it carries.
pub const EDGE_SYNC_SPAN: Duration = Duration::from_millis(500);

/// The `ChannelType` a `CreateSignalingChannel` call that names none gets: "Currently,
/// `SINGLE_MASTER` is the only supported channel type" (`ChannelType`'s docs on
/// `CreateSignalingChannelInput`), from
/// [`crate::ops::types::CHANNEL_TYPE_VALUES`].
pub const CHANNEL_TYPE_SINGLE_MASTER: &str = "SINGLE_MASTER";

/// `MediaStorageConfigurationStatus` values, from
/// `smithy_query(query="shape", shape="MediaStorageConfigurationStatus")` via
/// [`crate::ops::types::MEDIA_STORAGE_CONFIGURATION_STATUS_VALUES`].
pub const MEDIA_STORAGE_DISABLED: &str = "DISABLED";
/// See [`MEDIA_STORAGE_DISABLED`].
pub const MEDIA_STORAGE_ENABLED: &str = "ENABLED";

/// The `SingleMasterConfiguration.MessageTtlSeconds` a channel created without one
/// carries: "The default configuration for the channel message's time to live is 60
/// seconds (1 minute)" (`CreateSignalingChannelInput$SingleMasterConfiguration`
/// docs). A DOCUMENTED duration, so the value is AWS's and not chosen here.
pub const DEFAULT_MESSAGE_TTL_SECONDS: i32 = 60;

/// How long a freshly created stream reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// The model documents `CreateStream` as "an asynchronous operation", so the
/// stream is returned in a non-terminal status and the move to `ACTIVE` is owed
/// by the clock rather than performed by whoever reads it next. AWS publishes no
/// duration for it, so this span is CHOSEN — deliberately non-zero, so a caller
/// that describes the stream immediately still observes `CREATING`.
pub const STREAM_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long a stream marked for deletion stays readable, reporting `DELETING`, before
/// the clock owes its removal from the store.
///
/// `DeleteStream` "marks the stream for deletion" rather than removing it, so the
/// record moves to the non-terminal [`STATUS_DELETING`] and its disappearance is owed
/// by the clock rather than performed by whoever reads it next. AWS publishes no
/// duration for the deletion itself, so this span is CHOSEN — deliberately non-zero,
/// so a caller that polls straight after the delete still observes `DELETING` and the
/// refusals that status carries.
pub const STREAM_DELETION_SPAN: Duration = Duration::from_secs(3);

/// The most streams this emulator holds at once, per account and region.
///
/// `streams` grows by one record per `CreateStream` request — each holding
/// caller-supplied strings and a tag map — and shrinks only on an explicit
/// `DeleteStream`, so without a ceiling its entry COUNT, and with it the
/// per-request work every stream operation does under the lock
/// ([`ComAmazonawsKinesisvideoKinesisvideo20170930State::settle_clock`] walks the
/// index), is set by how many requests one caller makes. AWS bounds the same thing
/// with a per-account stream quota and refuses a create past it; that quota's value
/// is adjustable and is not in the model, so this number is the EMULATOR's own bound
/// rather than a claim about AWS's, while the refusal it produces is the error the
/// model declares for exactly this case: `AccountStreamLimitExceededException`
/// (HTTP 400, "The number of streams created for the account is too high.").
pub const MAX_STREAMS: usize = 2_500;

/// How long a freshly created signaling channel reports `CREATING`, and a just-updated
/// one `UPDATING`, before the clock owes it `ACTIVE`.
///
/// The model documents `CreateSignalingChannel` as "an asynchronous operation" and
/// `UpdateSignalingChannel` as "an asynchronous operation [that] takes time to
/// complete", so the channel is left in a non-terminal status and the move to `ACTIVE`
/// is owed by the clock rather than performed by whoever reads it next. AWS publishes no
/// duration for either, so this span is CHOSEN — deliberately non-zero, so a caller that
/// describes the channel immediately still observes the transient status.
pub const CHANNEL_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The most signaling channels this emulator holds at once, per account and region.
///
/// The same bound, and for the same reason, as [`MAX_STREAMS`]: `signaling_channels`
/// grows by one record per `CreateSignalingChannel` and shrinks only on an explicit
/// `DeleteSignalingChannel`, so without a ceiling the per-request work every channel
/// operation does under the lock is set by how many requests one caller makes. AWS
/// bounds the same thing with an adjustable per-account channel quota that is not in
/// the model, so this number is the EMULATOR's own bound, while the refusal it
/// produces is the error the model declares for exactly this case:
/// `AccountChannelLimitExceededException` (HTTP 400).
pub const MAX_SIGNALING_CHANNELS: usize = 2_500;

/// How this service words a `ResourceNotFoundException` for a signaling channel.
///
/// Stated once so the four channel readers word one failure one way. It is the text
/// the real service uses, which the model does not pin: the REQUIREMENT this failure
/// carries is its code and its HTTP 404, and the message is an example.
pub const CHANNEL_NOT_FOUND_MESSAGE: &str = "The requested channel is not found or not active.";

/// How this service words a `ResourceNotFoundException` for a stream.
///
/// The stream-shaped companion of [`CHANNEL_NOT_FOUND_MESSAGE`], stated once so every
/// stream operation words one failure one way. It is the text the real service uses,
/// which the model does not pin: the REQUIREMENT this failure carries is its code and
/// its HTTP 404, and the message is an example.
pub const STREAM_NOT_FOUND_MESSAGE: &str = "The requested stream is not found or not active.";

/// How this service words a `ResourceInUseException` for a stream that EXISTS but is
/// not in `ACTIVE` status — the case the shape is documented for ("if the provided
/// input StreamARN or ChannelARN is not in Active status … The DescribeStream or
/// DescribeSignalingChannel API to determine the status of the resource"), as against
/// the absent stream [`STREAM_NOT_FOUND_MESSAGE`] reports. Fixed text: the condition
/// is the stream's status, so no request value belongs in it.
pub const STREAM_NOT_ACTIVE_MESSAGE: &str = "The requested stream is not active.";

/// How this service words the `InvalidArgumentException` for a request that did not name
/// EXACTLY one identifier of the stream it addresses — it named neither, or it named both.
///
/// Every stream operation models the `StreamName`/`StreamARN` pair as two OPTIONAL members
/// and documents the choice between them as exclusive ("You must specify either the
/// `StreamName` or the `StreamARN`" — `smithy_query(query="doc",shape="DescribeStream")`),
/// so one refusal covers both ways of missing it and is stated once here. The sentence is
/// wholly fixed text: its only parts drawn from anywhere are the two member NAMES the
/// model declares, and nothing of the request's values belongs in it.
pub const STREAM_IDENTIFIER_MESSAGE: &str =
    "One of StreamName or StreamARN must be provided, but not both.";

/// How this service words the `VersionMismatchException` every stream mutation
/// guarded by `CurrentVersion` answers. The sentence the service sends names the two
/// versions it compared — the provided one and the existing one — without quoting
/// either, so the text is wholly fixed: no stream name, ARN or version string belongs
/// in it. It is NOT the error shape's documentation paragraph ("The stream version that
/// you specified is not the latest version. To get the latest version, use the
/// DescribeStream API."), which describes the condition rather than being the wire
/// message. Stated once so the operations that take a stream version word one failure
/// one way.
pub const STREAM_VERSION_MISMATCH_MESSAGE: &str =
    "The provided stream version does not match the existing stream version.";

/// [`STREAM_IDENTIFIER_MESSAGE`] for the signaling-channel pair, whose operations model and
/// document the same exclusive choice ("You must specify either the name or the Amazon
/// Resource Name (ARN) of the channel" —
/// `smithy_query(query="doc",shape="DescribeSignalingChannel")`): one sentence, with the
/// member names of the pair the operation takes.
pub const CHANNEL_IDENTIFIER_MESSAGE: &str =
    "One of ChannelName or ChannelARN must be provided, but not both.";

/// How this service words a `StreamEdgeConfigurationNotFoundException`: the stream
/// exists, but no edge configuration has ever been submitted for it, which is the
/// failure the shape is documented for ("The Exception rendered when the Amazon Kinesis
/// Video Stream can't find a stream's edge configuration that you specified"). The
/// requirement is the code and its HTTP 404; the message is an example.
pub const EDGE_CONFIGURATION_NOT_FOUND_MESSAGE: &str = "No edge configuration exist for the stream";

/// One Kinesis video stream, as this service persists it.
#[derive(Debug, Clone)]
pub struct Stream {
    pub stream_name: String,
    pub stream_arn: String,
    pub device_name: Option<String>,
    pub media_type: Option<String>,
    /// Never `None`: `CreateStream` defaults it to the Kinesis Video-managed key.
    pub kms_key_id: String,
    /// Opaque alphanumeric token, re-minted on every mutation of the record.
    pub version: String,
    pub status: String,
    /// `CreationTime`, as Unix epoch milliseconds.
    pub creation_time_millis: i64,
    pub data_retention_in_hours: i32,
    pub tags: BTreeMap<String, String>,
    /// The `DefaultStorageTier` the create call asked for, kept so the
    /// storage-configuration operations (their own gap issues) read the value the
    /// client sent rather than a fresh default.
    pub default_storage_tier: Option<String>,
    /// When the clock owes this stream the move out of `CREATING`; `None` once
    /// the status is terminal.
    pub activates_at: Option<Instant>,
    /// When the clock owes this stream its removal from the store, set by the delete
    /// that marked it for deletion; `None` while the stream is not being deleted.
    pub deletes_at: Option<Instant>,
    /// The stream's notification configuration, `None` while it has none —
    /// `UpdateNotificationConfiguration` sets it, and passing no structure "will be
    /// deleted from the stream", i.e. puts it back to `None`.
    pub notification_configuration: Option<NotificationConfiguration>,
    /// The stream's image-generation configuration, under the same rule as
    /// [`Self::notification_configuration`]: "If null, the configuration will be deleted
    /// from the stream" (`ImageGenerationConfiguration` docs).
    pub image_generation_configuration: Option<ImageGenerationConfiguration>,
    /// The stream's edge configuration, `None` until a `StartEdgeConfigurationUpdate`
    /// creates one — the state `DescribeEdgeConfiguration` reports, and whose absence is
    /// what makes that read a `StreamEdgeConfigurationNotFoundException`.
    pub edge: Option<EdgeConfiguration>,
}

/// One stream's edge configuration, as this service persists it.
#[derive(Debug, Clone)]
pub struct EdgeConfiguration {
    /// The configuration the caller submitted, reported back unchanged.
    pub config: EdgeConfig,
    /// When the FIRST `StartEdgeConfigurationUpdate` created it, as Unix epoch
    /// milliseconds; stable across later updates of the same configuration.
    pub creation_time_millis: i64,
    /// When the latest `StartEdgeConfigurationUpdate` rewrote it, as Unix epoch
    /// milliseconds.
    pub last_updated_time_millis: i64,
    /// A `SyncStatus` value — [`SYNC_STATUS_SYNCING`] until the clock owes
    /// [`SYNC_STATUS_IN_SYNC`].
    pub sync_status: String,
    /// When the clock owes this configuration the move out of `SYNCING`; `None` once the
    /// sync status is terminal.
    pub synced_at: Option<Instant>,
}

impl Stream {
    /// This record as the modeled `StreamInfo` every reader returns.
    pub fn to_stream_info(&self) -> StreamInfo {
        StreamInfo {
            device_name: self.device_name.clone(),
            stream_name: Some(self.stream_name.clone()),
            stream_arn: Some(self.stream_arn.clone()),
            media_type: self.media_type.clone(),
            kms_key_id: Some(self.kms_key_id.clone()),
            version: Some(self.version.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.creation_time_millis,
            )),
            data_retention_in_hours: Some(self.data_retention_in_hours),
        }
    }

    /// This record's storage half, as `DescribeStreamStorageConfiguration` reports it.
    ///
    /// `StreamStorageConfiguration.DefaultStorageTier` is modeled `@required`, so a
    /// stream created without one still reports a tier: the default,
    /// [`DEFAULT_STORAGE_TIER`].
    pub fn to_stream_storage_configuration(&self) -> StreamStorageConfiguration {
        StreamStorageConfiguration {
            default_storage_tier: self
                .default_storage_tier
                .clone()
                .unwrap_or_else(|| DEFAULT_STORAGE_TIER.to_string()),
        }
    }
}

/// The most tags one stream carries at once.
///
/// `ResourceTags`, the map every tagging operation reads and writes, is modeled
/// `@length(min: 1, max: 50)` (`smithy_query(query="shape", shape="ResourceTags")`),
/// and `TagsPerResourceExceededLimitException` states the same bound in prose: "You
/// have exceeded the limit of tags that you can associate with the resource. A
/// Kinesis video stream can support up to 50 tags." One request cannot carry more
/// than this (the boundary validator refuses it), so the ceiling bites when a
/// request's tags are MERGED into the ones a stream already holds.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s modeled minimum length, from
/// `smithy_query(query="shape", shape="TagKey")` (`@length(min: 1, max: 128)`).
pub const MIN_TAG_KEY_LENGTH: usize = 1;

/// `TagKey`'s modeled maximum length. See [`MIN_TAG_KEY_LENGTH`].
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// `TagValue`'s modeled maximum length, from
/// `smithy_query(query="shape", shape="TagValue")` (`@length(min: 0, max: 256)`);
/// its minimum is 0, so an empty value is valid.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `key` is not a `TagKey`, or `None` when it is one.
///
/// A MAP KEY's own `@length` is not among the constraints the request walk's validator
/// checks — an empty key reaches the handler — so the operations that accept tag keys
/// enforce it themselves. The message is built from the request, naming the member and
/// the constraint it failed, in the form the framework's own validation envelope uses.
///
/// `TagKey`'s `@pattern` (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, a regular expression over
/// Unicode general categories) is NOT evaluated here: `@pattern` checking belongs to
/// the framework's validating walk, which owns it for every member it does reach, and
/// a key the pattern rejects is therefore stored rather than refused. Closing that gap
/// belongs in that walk rather than in one service's handlers.
pub fn tag_key_violation(member: &str, key: &str) -> Option<String> {
    let length = key.chars().count();
    if !(MIN_TAG_KEY_LENGTH..=MAX_TAG_KEY_LENGTH).contains(&length) {
        return Some(format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must have length greater than or equal to {MIN_TAG_KEY_LENGTH} and less \
             than or equal to {MAX_TAG_KEY_LENGTH}"
        ));
    }
    None
}

/// Why `value` is not a `TagValue`, or `None` when it is one. The companion of
/// [`tag_key_violation`]: a map VALUE's `@length` is checked by the request walk for a
/// list or structure member, but not for the value side of a map, so the tagging
/// operations check it here.
pub fn tag_value_violation(member: &str, value: &str) -> Option<String> {
    if value.chars().count() > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must have length less than or equal to {MAX_TAG_VALUE_LENGTH}"
        ));
    }
    None
}

/// Why a `StreamName`/`StreamARN` or `ChannelName`/`ChannelARN` pair did not name a
/// stored resource. One enum for both resources: the two ways such a pair fails to
/// resolve do not depend on which resource it addresses, so the readers of either one
/// state the same two failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupError {
    /// The pair did not name EXACTLY one identifier: neither was supplied, or both were.
    /// The choice the model documents is exclusive, so both ways of missing it are the
    /// one refusal ([`STREAM_IDENTIFIER_MESSAGE`], [`CHANNEL_IDENTIFIER_MESSAGE`]) — a
    /// request that names both resolves to no ONE resource even when the two agree, since
    /// which of them the service would have read is the caller's to state.
    NotExactlyOneIdentifier,
    /// No resource of that name or ARN exists in this account and region.
    NotFound,
}

/// One Kinesis Video WebRTC signaling channel, as this service persists it.
#[derive(Debug, Clone)]
pub struct SignalingChannel {
    pub channel_name: String,
    pub channel_arn: String,
    /// A `ChannelType` value; never `None`, since a create that names none gets
    /// [`CHANNEL_TYPE_SINGLE_MASTER`].
    pub channel_type: String,
    /// A `Status` value — [`STATUS_CREATING`] until the clock owes [`STATUS_ACTIVE`].
    pub status: String,
    /// `CreationTime`, as Unix epoch milliseconds.
    pub creation_time_millis: i64,
    /// `SingleMasterConfiguration.MessageTtlSeconds`; never `None`, since a create
    /// that names none gets [`DEFAULT_MESSAGE_TTL_SECONDS`].
    pub message_ttl_seconds: i32,
    /// Opaque alphanumeric token, re-minted on every mutation of the record.
    pub version: String,
    pub tags: BTreeMap<String, String>,
    /// The channel's media-storage configuration: a
    /// `MediaStorageConfigurationStatus` value, [`MEDIA_STORAGE_DISABLED`] on a
    /// freshly created channel because a create associates no stream with it — "If
    /// `StorageStatus` is disabled, no data will be stored, and the `StreamARN`
    /// parameter will not be needed" (`MediaStorageConfiguration` docs).
    /// `UpdateMediaStorageConfiguration` is what moves it, and reads report it.
    pub media_storage_status: String,
    /// The stream the media-storage configuration points at, `None` while it is
    /// disabled.
    pub media_storage_stream_arn: Option<String>,
    /// When the clock owes this channel the move out of `CREATING`; `None` once the
    /// status is terminal.
    pub activates_at: Option<Instant>,
}

impl SignalingChannel {
    /// This record as the modeled `ChannelInfo` every reader returns.
    pub fn to_channel_info(&self) -> ChannelInfo {
        ChannelInfo {
            channel_name: Some(self.channel_name.clone()),
            channel_arn: Some(self.channel_arn.clone()),
            channel_type: Some(self.channel_type.clone()),
            channel_status: Some(self.status.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from_millis(
                self.creation_time_millis,
            )),
            single_master_configuration: Some(SingleMasterConfiguration {
                message_ttl_seconds: Some(self.message_ttl_seconds),
            }),
            version: Some(self.version.clone()),
        }
    }

    /// This record's media-storage half, as `DescribeMediaStorageConfiguration`
    /// reports it.
    pub fn to_media_storage_configuration(&self) -> MediaStorageConfiguration {
        MediaStorageConfiguration {
            stream_arn: self.media_storage_stream_arn.clone(),
            status: self.media_storage_status.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-kinesisvideo-kinesisvideo-20170930. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideoKinesisvideo20170930State {
    /// The primary stream index. `BTreeMap`, not `HashMap`: `ListStreams`
    /// paginates over the sorted key set, and a token resolves against that set
    /// rather than against a snapshot copy.
    pub streams: BTreeMap<String, Stream>,
    /// `StreamARN` -> `StreamName`, so every operation that accepts an ARN
    /// resolves to the same record the name form does.
    pub stream_arn_index: BTreeMap<String, String>,
    /// The primary signaling-channel index, keyed by `ChannelName` — the
    /// per-account, per-region identifier a channel is unique under.
    /// `BTreeMap`, not `HashMap`: `ListSignalingChannels` paginates over the sorted
    /// key set, and a token resolves against that set rather than against a snapshot
    /// copy.
    pub signaling_channels: BTreeMap<String, SignalingChannel>,
    /// `ChannelARN` -> `ChannelName`, so every operation that accepts a channel ARN
    /// resolves to the same record the name form does.
    pub channel_arn_index: BTreeMap<String, String>,
    /// Bumped on every mutation, and mixed into the draw [`mint_version_token`] makes,
    /// so two mints never ask the same question of the entropy source. It is NOT part
    /// of the token it helps mint.
    pub version_seq: u64,
    /// The address each data plane of this store is published at, with the listener
    /// that answers there — opened on the first `GetDataEndpoint` for that plane (see
    /// [`ensure_data_endpoint`]) and held here for the life of the store, so a plane's
    /// announced endpoint never moves under a client.
    pub data_endpoints: BTreeMap<DataPlane, DataPlaneEndpoint>,
}

impl ComAmazonawsKinesisvideoKinesisvideo20170930State {
    /// Materialise every status transition, and every removal, the clock already owes.
    ///
    /// Called at the top of each stream and signaling-channel operation, including
    /// the reporting ones: the move out of `CREATING`, and the removal of a stream
    /// marked for deletion, were each due at a fixed instant whether or not anybody
    /// called, so applying them here changes nothing a later caller would not have
    /// seen anyway.
    pub fn settle_clock(&mut self) {
        let now = Instant::now();

        // A stream whose deletion has fallen due is gone: the delete that marked it
        // `DELETING` fixed the instant, so removing it here is what a caller reading at
        // that instant would have seen anyway. One walk, and an empty `Vec` allocates
        // nothing, so the common path — no removal due — costs no allocation.
        let mut due: Vec<(String, String)> = Vec::new();
        for stream in self.streams.values() {
            if stream.deletes_at.is_some_and(|instant| now >= instant) {
                due.push((stream.stream_name.clone(), stream.stream_arn.clone()));
            }
        }
        for (stream_name, stream_arn) in due {
            self.streams.remove(&stream_name);
            self.stream_arn_index.remove(&stream_arn);
        }

        for stream in self.streams.values_mut() {
            if stream.activates_at.is_some_and(|due| now >= due) {
                stream.activates_at = None;
                stream.status = STATUS_ACTIVE.to_string();
            }
            if let Some(edge) = stream.edge.as_mut() {
                if edge.synced_at.is_some_and(|due| now >= due) {
                    edge.synced_at = None;
                    edge.sync_status = SYNC_STATUS_IN_SYNC.to_string();
                }
            }
        }
        for channel in self.signaling_channels.values_mut() {
            if channel.activates_at.is_some_and(|due| now >= due) {
                channel.activates_at = None;
                channel.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// The resources mapped to the stream `stream_arn`, as
    /// `DescribeMappedResourceConfiguration` reports them, in sorted ARN order so a
    /// page token resolves against a stable sequence.
    ///
    /// A mapping exists in exactly one direction and is held once: a signaling channel's
    /// media-storage configuration names the stream its data is stored in ("If
    /// `StorageStatus` is enabled, the data will be stored in the `StreamARN`
    /// provided"), so the resources mapped TO a stream are the channels whose enabled
    /// configuration points at it — the same records
    /// `DescribeMediaStorageConfiguration` reports from the channel's side.
    pub fn mapped_resources(&self, stream_arn: &str) -> Vec<MappedResourceConfigurationListItem> {
        let mut items: Vec<MappedResourceConfigurationListItem> = self
            .signaling_channels
            .values()
            .filter(|channel| channel.media_storage_status == MEDIA_STORAGE_ENABLED)
            .filter(|channel| {
                channel
                    .media_storage_stream_arn
                    .as_deref()
                    .is_some_and(|mapped| mapped == stream_arn)
            })
            .map(|channel| MappedResourceConfigurationListItem {
                r#type: arn_resource_type(&channel.channel_arn),
                arn: Some(channel.channel_arn.clone()),
            })
            .collect();
        items.sort_by(|left, right| left.arn.cmp(&right.arn));
        items
    }

    /// A fresh opaque `Version` token for ONE record: alphanumeric, 1..64 characters,
    /// per `smithy_query(query="shape", shape="Version")`.
    ///
    /// The token is what a caller hands straight back as `CurrentVersion`, so the only
    /// thing it has to support is equality with the stored one. It therefore carries
    /// nothing a caller can read: [`mint_version_token`] draws it, and the store's
    /// mint count only varies that draw rather than appearing in the token, so one
    /// record's version reveals neither how many mutations the store has seen nor what
    /// the next token will be.
    pub fn mint_version(&mut self) -> String {
        self.version_seq = self.version_seq.wrapping_add(1);
        mint_version_token(self.version_seq)
    }

    /// The stored stream a `StreamName`/`StreamARN` pair names.
    ///
    /// The pair is an EXCLUSIVE choice ([`LookupError::NotExactlyOneIdentifier`]): exactly
    /// one of the two identifies the stream, and a request naming neither or both never
    /// reaches the store at all.
    pub fn resolve(
        &self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<&Stream, LookupError> {
        let name = match (stream_name, stream_arn) {
            (None, None) | (Some(_), Some(_)) => {
                return Err(LookupError::NotExactlyOneIdentifier)
            }
            (Some(name), None) => name.to_string(),
            (None, Some(arn)) => self
                .stream_arn_index
                .get(arn)
                .cloned()
                .ok_or(LookupError::NotFound)?,
        };
        self.streams.get(&name).ok_or(LookupError::NotFound)
    }

    /// The stored signaling channel a `ChannelName`/`ChannelARN` pair names.
    ///
    /// The same rule [`Self::resolve`] applies to a stream's identifier pair: the choice
    /// between the two identifiers is exclusive, so exactly one of them names the record
    /// and a pair that names neither or both is refused before the store is read.
    pub fn resolve_channel(
        &self,
        channel_name: Option<&str>,
        channel_arn: Option<&str>,
    ) -> Result<&SignalingChannel, LookupError> {
        let name = match (channel_name, channel_arn) {
            (None, None) | (Some(_), Some(_)) => {
                return Err(LookupError::NotExactlyOneIdentifier)
            }
            (Some(name), None) => name.to_string(),
            (None, Some(arn)) => self
                .channel_arn_index
                .get(arn)
                .cloned()
                .ok_or(LookupError::NotFound)?,
        };
        self.signaling_channels
            .get(&name)
            .ok_or(LookupError::NotFound)
    }

    /// The tag map of the stored resource `resource_arn` names, for READING.
    ///
    /// The tagging operations keyed on a bare `ResourceARN` (`ListTagsForResource`,
    /// `TagResource`, `UntagResource`) address whichever resource the ARN names: the
    /// resource TYPE in the ARN is what the modeled `ResourceARN` pattern leaves open
    /// (`…:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+`), and this service mints exactly two of them,
    /// `channel` and `stream`. `None` when the ARN names neither.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(name) = self.channel_arn_index.get(resource_arn) {
            return self
                .signaling_channels
                .get(name)
                .map(|channel| &channel.tags);
        }
        let name = self.stream_arn_index.get(resource_arn)?;
        self.streams.get(name).map(|stream| &stream.tags)
    }

    /// [`Self::resource_tags`] for WRITING, so a tagging mutation reaches the ONE stored
    /// record its reader reports rather than a second copy of it.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        if let Some(name) = self.channel_arn_index.get(resource_arn).cloned() {
            return self
                .signaling_channels
                .get_mut(&name)
                .map(|channel| &mut channel.tags);
        }
        let name = self.stream_arn_index.get(resource_arn).cloned()?;
        self.streams.get_mut(&name).map(|stream| &mut stream.tags)
    }

    /// [`Self::resolve`] for an operation that MUTATES the stream it names, so the
    /// two forms of identifier reach the one stored record through the same rule.
    pub fn resolve_mut(
        &mut self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<&mut Stream, LookupError> {
        // The immutable resolution decides WHICH record this is; the name it yields
        // is then re-borrowed mutably, so the lookup rule lives in one place.
        let name = self.resolve(stream_name, stream_arn)?.stream_name.clone();
        self.streams
            .get_mut(&name)
            .ok_or(LookupError::NotFound)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideoKinesisvideo20170930State>> {
    shared_engine(engine)
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideoKinesisvideo20170930State>)
}

/// The one shared engine behind a handler's `Eng`, which is also what vends the
/// listener a stream's data plane is published on ([`ensure_data_endpoint`]).
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}

/// Unix epoch milliseconds now.
pub fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or(i64::MAX)
}

/// The symbols a `Version` token is written in: the whole alphanumeric set the modeled
/// `@pattern` `^[a-zA-Z0-9]+$` admits, both cases included.
const VERSION_TOKEN_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// How many of [`VERSION_TOKEN_ALPHABET`]'s symbols a minted `Version` carries.
///
/// The model bounds `Version` only by its `@length` 1..64 and that `@pattern`, and AWS
/// publishes no width, so this one is CHOSEN: fixed, so every record's token is the
/// same width whatever the store has done, and wide enough (20 symbols out of 62,
/// about 119 bits) that two draws never collide.
const VERSION_TOKEN_LENGTH: usize = 20;

/// [`VERSION_TOKEN_ALPHABET`]'s symbol count, as the radix a drawn token is written in.
const VERSION_TOKEN_RADIX: u128 = VERSION_TOKEN_ALPHABET.len() as u128;

/// One opaque `Version` token.
///
/// The bits come from the standard library's [`std::collections::hash_map::RandomState`],
/// whose SipHash keys the host seeds, and which draws its own keys per instance — so two
/// draws of the same `sequence` yield independent tokens. Neither the sequence nor the
/// clock is rendered into the result: the token is a drawn identifier, so it orders
/// nothing, reveals no store-wide count, and shares no run-local prefix or suffix with
/// any other record's token.
fn mint_version_token(sequence: u64) -> String {
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    // Two independently keyed draws, so the rendered width is backed by more bits than
    // it spends: one `u64` would run out after 10 of the 20 symbols.
    let high = RandomState::new().hash_one(sequence);
    let low = RandomState::new().hash_one(!sequence);
    let mut drawn = (u128::from(high) << 64) | u128::from(low);

    let mut token = Vec::with_capacity(VERSION_TOKEN_LENGTH);
    for _ in 0..VERSION_TOKEN_LENGTH {
        let symbol = usize::try_from(drawn % VERSION_TOKEN_RADIX).unwrap_or(0);
        token.push(VERSION_TOKEN_ALPHABET[symbol]);
        drawn /= VERSION_TOKEN_RADIX;
    }
    // The alphabet is ASCII, so the bytes are valid UTF-8 by construction.
    String::from_utf8(token).unwrap_or_else(|_| "0".repeat(VERSION_TOKEN_LENGTH))
}

/// The `StreamARN` of a stream, in the one form the modeled `@pattern`
/// `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`
/// accepts. The trailing numeric segment is the record's creation instant, so the
/// ARN is stable for the record's whole lifetime.
pub fn stream_arn(region: &str, stream_name: &str, creation_time_millis: i64) -> String {
    ::engine::emulator_arn(
        "kinesisvideo",
        region,
        &format!("stream/{stream_name}/{creation_time_millis}"),
    )
}

/// The resource TYPE an ARN names: the first segment of its resource field, upper-cased
/// — `channel/<name>/<millis>` is a `CHANNEL`, `stream/<name>/<millis>` a `STREAM`.
///
/// `MappedResourceConfigurationListItem.Type` is modeled as a plain string ("The type of
/// the associated resource for the kinesis video stream") with no enumerated values, so
/// the type reported for a mapped resource is READ OFF that resource's own ARN rather
/// than chosen per operation. `None` for a string that carries no resource field.
fn arn_resource_type(arn: &str) -> Option<String> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let kind = resource.split('/').next()?;
    (!kind.is_empty()).then(|| kind.to_ascii_uppercase())
}

/// The `ChannelARN` of a signaling channel, in the one form the modeled `@pattern`
/// `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`
/// accepts. Built exactly as [`stream_arn`] is, with the resource type the signaling
/// channel's own: the trailing numeric segment is the record's creation instant, so
/// the ARN is stable for the record's whole lifetime.
pub fn channel_arn(region: &str, channel_name: &str, creation_time_millis: i64) -> String {
    ::engine::emulator_arn(
        "kinesisvideo",
        region,
        &format!("channel/{channel_name}/{creation_time_millis}"),
    )
}

/// The Kinesis Video-managed key a stream is encrypted with when the create call
/// names none: "If no key ID is specified, the default, Kinesis Video-managed key
/// (aws/kinesisvideo) is used" (`KmsKeyId` docs), reported as that key's ARN.
pub fn default_kms_key_id(region: &str) -> String {
    ::engine::emulator_arn("kms", region, "alias/aws/kinesisvideo")
}

/// The region this request is scoped to, from its SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The authority this request arrived on, falling back to the service's own regional
/// host form when the request carries none.
///
/// It is NOT where a stream's media planes live: those are resolved per access mode by
/// [`ensure_data_endpoint`], which is the whole point of `GetDataEndpoint`. The
/// signaling-channel endpoints report this authority under the scheme each
/// `ChannelProtocol` names.
pub fn request_authority(parts: &::http::request::Parts, region: &str) -> String {
    let authority = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(ToString::to_string));
    match authority {
        Some(authority) => authority,
        None => service_host(region),
    }
}
