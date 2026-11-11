// SPDX-License-Identifier: CC0-1.0

//! @op UpdateOriginEndpoint
//! IMPLEMENTED. Replaces the packaging configuration of an origin endpoint persisted in
//! [`crate::state`], leaving the endpoint's identity (its name, ARN and creation instant)
//! alone. A write that CHANGED the endpoint commits a new version of it, and its ETag is a
//! function of that version, so the tag rotates and the one the caller held stops
//! matching; a write that stored what was already there commits nothing and leaves both
//! the tag and `ModifiedAt` as they were.
//!
//! What it refuses, before anything is written — the rules about the request alone
//! first, then the precondition the moment the addressed endpoint is in hand, then the
//! rules that read the stored endpoint and its parents:
//!
//! * a manifest set the new `ContainerType` cannot serve, no manifest at all, or more
//!   manifests than [`crate::state::MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`] — the same rules
//!   `CreateOriginEndpoint` applies, because an update carries the whole manifest set;
//! * a manifest set that would publish two manifests at one egress URL
//!   (`MANIFEST_NAME_COLLISION`), and an encryption block whose method, initialization
//!   vector, DRM systems, key rotation, encryption contract, content-key period timing or
//!   key-server URL the service could not act on — both off the rules the create reads
//!   ([`crate::state::manifest_name_collision`],
//!   [`crate::state::segment_encryption_rejection`]);
//! * an `ETag` precondition (`x-amzn-update-if-match`) that is not the endpoint's current
//!   tag, which the model's doc says rejects the request — judged ahead of every rule
//!   below, exactly as this service's other conditional writers judge it, because a
//!   caller holding a tag that is no longer current lost a race over the very endpoint
//!   those rules compare against;
//! * a member the model documents as immutable after creation, when the request carries
//!   a different value — `ContainerType` (`CONTAINER_TYPE_IMMUTABLE`),
//!   `StreamNameOutputMode` (`STREAM_NAME_OUTPUT_MODE_IMMUTABLE`) and
//!   `Segment.OutputTimestampMode` (`OUTPUT_TIMESTAMP_MODE_IMMUTABLE`), each a 400
//!   `ValidationException` naming the member, with the stored endpoint untouched;
//! * a `Segment.OutputTimestampMode` on an endpoint whose channel's output locking mode
//!   may not carry the member at all — the same 400 `ValidationException` /
//!   `ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE` `CreateOriginEndpoint` answers,
//!   because the restriction is read in one place
//!   ([`crate::state::output_timestamp_mode_violation`]);
//! * a setting named for an endpoint whose parent channel's `InputType` may not carry it
//!   at all — the stream-name output mode
//!   (`ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE`), the failover settings
//!   (`ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION`) and the ISM
//!   container, low-latency HLS and MSS manifests and startover window a multiview
//!   channel's endpoint may not carry — off the same table the create reads
//!   ([`crate::state::origin_endpoint_input_type_violation`]);
//! * a manifest set that would stop publishing a manifest one of the endpoint's own
//!   harvest jobs is still harvesting — a 409 `ConflictException` / `RESOURCE_IN_USE`
//!   ([`crate::state::harvested_manifest_in_use`]), because the stored job's
//!   `HarvestedManifests` reference the endpoint's manifests by name and the write would
//!   leave that reference pointing at nothing; the reference is resolved through the same
//!   predicate `CreateHarvestJob` refuses to establish a dangling one with
//!   ([`crate::state::names_unpublished_manifest`]), and a job in a status it can no
//!   longer leave holds no such reference.
//!
//! A group, channel or endpoint that does not resolve is a `ResourceNotFoundException`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateOriginEndpoint::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateOriginEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")
//! ```
//!
//! Update the specified origin endpoint. Edit the packaging preferences on an endpoint to
//! optimize the viewing experience. You can't edit the name of the endpoint. Any edits you
//! make that impact the video output may not be reflected for a few minutes.
//!
//! ## Input
//!
//! * `ChannelGroupName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** —
//!   The name that describes the channel group. The name is the primary identifier for the
//!   channel group, and must be unique for your account in the AWS Region.
//! * `ChannelName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** — The
//!   name that describes the channel. The name is the primary identifier for the channel, and
//!   must be unique for your account in the AWS Region and channel group.
//! * `ContainerType` (ContainerType, one of: CMAF | ISM | TS) **required** — The type of
//!   container attached to this origin endpoint. A container type is a file format that
//!   encapsulates one or more media streams, such as audio and video, into a single file.
//! * `DashManifests` (CreateDashManifests) — A DASH manifest configuration.
//! * `Description` (ResourceDescription, len 0..1024) — Any descriptive information that you
//!   want to add to the origin endpoint for future identification purposes.
//! * `ETag` (EntityTag, len 1..256, pattern ^[\S]+$) — The expected current Entity Tag (ETag)
//!   for the resource. If the specified ETag does not match the resource's current entity
//!   tag, the update request will be rejected.
//! * `ForceEndpointErrorConfiguration` (ForceEndpointErrorConfiguration) — The failover
//!   settings for the endpoint.
//! * `HlsManifests` (CreateHlsManifests) — An HTTP live streaming (HLS) manifest
//!   configuration.
//! * `LowLatencyHlsManifests` (CreateLowLatencyHlsManifests) — A low-latency HLS manifest
//!   configuration.
//! * `MssManifests` (CreateMssManifests) — A list of Microsoft Smooth Streaming (MSS)
//!   manifest configurations to update for the origin endpoint. This replaces the existing
//!   MSS manifest configurations.
//! * `OriginEndpointName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** —
//!   The name that describes the origin endpoint. The name is the primary identifier for the
//!   origin endpoint, and and must be unique for your account in the AWS Region and channel.
//! * `Segment` (Segment) — The segment configuration, including the segment name, duration,
//!   and other configuration values.
//! * `StartoverWindowSeconds` (Integer) — The size of the window (in seconds) to create a
//!   window of the live stream that's available for on-demand viewing. Viewers can start-over
//!   or catch-up on content that falls within the window. The maximum startover window is
//!   1,209,600 seconds (14 days).
//! * `StreamNameOutputMode` (StreamNameOutputMode, one of: INDEX | PASSTHROUGH_NAME) — The
//!   output mode for stream names in egress manifests. If you provide a value, it must match
//!   the current value. You can't change the stream name output mode after you create the
//!   endpoint.
//! * `UriSeparator` (UriSeparator, one of: HYPHEN | UNDERSCORE) — The separator character to
//!   use in generated URIs for this origin endpoint. This setting applies to all manifest
//!   types on the endpoint. If you don't specify a value in the update request, the current
//!   value is preserved.
//!
//! ## Output
//!
//! * `Arn` (String) **required** — The ARN associated with the resource.
//! * `ChannelGroupName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** —
//!   The name that describes the channel group. The name is the primary identifier for the
//!   channel group, and must be unique for your account in the AWS Region.
//! * `ChannelName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** — The
//!   name that describes the channel. The name is the primary identifier for the channel, and
//!   must be unique for your account in the AWS Region and channel group.
//! * `ContainerType` (ContainerType, one of: CMAF | ISM | TS) **required** — The type of
//!   container attached to this origin endpoint.
//! * `CreatedAt` (Timestamp) **required** — The date and time the origin endpoint was
//!   created.
//! * `DashManifests` (GetDashManifests) — A DASH manifest configuration.
//! * `Description` (ResourceDescription, len 0..1024) — The description of the origin
//!   endpoint.
//! * `ETag` (EntityTag, len 1..256, pattern ^[\S]+$) — The current Entity Tag (ETag)
//!   associated with this resource. The entity tag can be used to safely make concurrent
//!   updates to the resource.
//! * `ForceEndpointErrorConfiguration` (ForceEndpointErrorConfiguration) — The failover
//!   settings for the endpoint.
//! * `HlsManifests` (GetHlsManifests) — An HTTP live streaming (HLS) manifest configuration.
//! * `LowLatencyHlsManifests` (GetLowLatencyHlsManifests) — A low-latency HLS manifest
//!   configuration.
//! * `ModifiedAt` (Timestamp) **required** — The date and time the origin endpoint was
//!   modified.
//! * `MssManifests` (GetMssManifests) — The updated Microsoft Smooth Streaming (MSS) manifest
//!   configurations for this origin endpoint.
//! * `OriginEndpointName` (ResourceName, len 1..256, pattern ^[a-zA-Z0-9_-]+$) **required** —
//!   The name that describes the origin endpoint. The name is the primary identifier for the
//!   origin endpoint, and and must be unique for your account in the AWS Region and channel.
//! * `Segment` (Segment) **required** — The segment configuration, including the segment
//!   name, duration, and other configuration values.
//! * `StartoverWindowSeconds` (Integer) — The size of the window (in seconds) to create a
//!   window of the live stream that's available for on-demand viewing. Viewers can start-over
//!   or catch-up on content that falls within the window.
//! * `StreamNameOutputMode` (StreamNameOutputMode, one of: INDEX | PASSTHROUGH_NAME) — The
//!   output mode for stream names in egress manifests for this origin endpoint.
//! * `Tags` (TagMap) — The comma-separated list of tag key:value pairs assigned to the origin
//!   endpoint.
//! * `UriSeparator` (UriSeparator, one of: HYPHEN | UNDERSCORE) — The separator character
//!   used in generated URIs for this origin endpoint.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied because either you don't have
//!   permissions to perform the requested operation or MediaPackage is getting throttling
//!   errors with CDN authorization. The user or role that is making the request must have at
//!   least one IAM permissions policy attached that grants the required permissions. For more
//!   information, see Access Management in the IAM User Guide. Or, if you're using CDN
//!   authorization, you will receive this exception if MediaPackage receives a throttling
//!   error from Secrets Manager.
//! * `ConflictException` (HTTP 409) — Updating or deleting this resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — Indicates that an error from the service occurred
//!   while trying to process a request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource doesn't exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would cause a service quota to
//!   be exceeded.
//! * `ThrottlingException` (HTTP 429) — The request throughput limit was exceeded.
//! * `ValidationException` (HTTP 400) — The input failed to meet the constraints specified by
//!   the AWS service.

#[allow(unused_imports)]
use super::types::*;
// The write-only-commits-a-change rule lives on the state module's `VersionedRecord`,
// which every `Update*` handler in this crate decides its commit through.
use crate::state::VersionedRecord;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateOriginEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateOriginEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mediapackagev2#UpdateOriginEndpoint")]
#[derive(Debug)]
pub enum UpdateOriginEndpointErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_origin_endpoint::UpdateOriginEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateOriginEndpointError = crate::scaffolded::Scaffolded<UpdateOriginEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateOriginEndpointErrorModeled> for UpdateOriginEndpointError {
    fn from(modeled: UpdateOriginEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// A member of an origin endpoint the model documents as fixed once the endpoint exists:
/// the member path a message names, the `ValidationExceptionType` that names its
/// refusal, and the sentence the refusal states.
struct ImmutableMember {
    member: &'static str,
    exception_type: &'static str,
    detail: &'static str,
}

/// `ContainerType`: "You can't change the container type after you create an origin
/// endpoint" — the rule the model's `CONTAINER_TYPE_IMMUTABLE` names.
const CONTAINER_TYPE: ImmutableMember = ImmutableMember {
    member: "containerType",
    exception_type: "CONTAINER_TYPE_IMMUTABLE",
    detail: "You can't change the container type after you create an origin endpoint. \
             To use a different container type, you'll need to create a new origin endpoint",
};

/// `StreamNameOutputMode`: the model's member doc says "If you provide a value, it must
/// match the current value. You can't change the stream name output mode after you create
/// the endpoint", and `ValidationExceptionType` names
/// `STREAM_NAME_OUTPUT_MODE_IMMUTABLE` for it.
const STREAM_NAME_OUTPUT_MODE: ImmutableMember = ImmutableMember {
    member: "streamNameOutputMode",
    exception_type: "STREAM_NAME_OUTPUT_MODE_IMMUTABLE",
    detail: "You can't change the stream name output mode after you create an origin \
             endpoint. To use a different stream name output mode, you'll need to create \
             a new origin endpoint",
};

/// The segment output-timestamp mode of an endpoint whose channel carries one is fixed
/// from the create onwards — the service materialises the member's documented default,
/// so there is no endpoint without a mode for an update to set:
/// `Segment$OutputTimestampMode` is documented "immutable after endpoint creation", and
/// `ValidationExceptionType` names `OUTPUT_TIMESTAMP_MODE_IMMUTABLE` for it.
const OUTPUT_TIMESTAMP_MODE: ImmutableMember = ImmutableMember {
    member: "segment.outputTimestampMode",
    exception_type: "OUTPUT_TIMESTAMP_MODE_IMMUTABLE",
    detail: "You can't change the output timestamp mode after you create an origin \
             endpoint. To use a different output timestamp mode, you'll need to create a \
             new origin endpoint",
};

/// The `ValidationException` for an attempt to change an immutable member: the service's
/// own message form, naming the member and restating the rule, plus the discriminator.
fn immutable_violation(member: &ImmutableMember) -> ValidationException {
    ValidationException {
        message: Some(crate::state::validation_message(
            Some(member.member),
            member.detail,
        )),
        validation_exception_type: Some(member.exception_type.to_string()),
    }
}

/// The response for the stored endpoint, answered from the record the update left behind
/// so nothing is reported that a later `GetOriginEndpoint` will not read.
fn endpoint_output(
    endpoint: &crate::state::OriginEndpoint,
) -> UpdateOriginEndpointOutput {
    UpdateOriginEndpointOutput {
        arn: endpoint.arn.clone(),
        channel_group_name: endpoint.group_name.clone(),
        channel_name: endpoint.channel_name.clone(),
        origin_endpoint_name: endpoint.name.clone(),
        container_type: endpoint.container_type.clone(),
        segment: endpoint.segment.clone(),
        created_at: endpoint.created_at,
        modified_at: endpoint.modified_at,
        description: endpoint.description.clone(),
        startover_window_seconds: endpoint.startover_window_seconds,
        hls_manifests: Some(endpoint.hls_manifests.clone()),
        low_latency_hls_manifests: Some(endpoint.low_latency_hls_manifests.clone()),
        dash_manifests: Some(endpoint.dash_manifests.clone()),
        mss_manifests: Some(endpoint.mss_manifests.clone()),
        force_endpoint_error_configuration: endpoint
            .force_endpoint_error_configuration
            .clone(),
        uri_separator: Some(endpoint.uri_separator.clone()),
        stream_name_output_mode: endpoint.stream_name_output_mode.clone(),
        e_tag: Some(endpoint.etag.clone()),
        tags: Some(endpoint.tags.clone()),
    }
}

/// The `UpdateOriginEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateOriginEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateOriginEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateOriginEndpoint<Eng>
{
    type Input = UpdateOriginEndpointInput;
    type Output = UpdateOriginEndpointOutput;
    type Error = UpdateOriginEndpointError;

    const NAME: &'static str = "UpdateOriginEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ORIGIN_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ORIGIN_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every derived value this update reports (ARN, manifest URLs) is a function of
        // the stored resource; what the request's own metadata decides is WHICH resource
        // that is — the names it carries resolve in the Region it was signed for.
        let region = crate::state::request_region(ctx.parts());
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdateOriginEndpointErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the MediaPackageV2 store is unavailable".to_string()),
                },
            )
            .into());
        };
        let mut store = state.lock();

        // The manifest-set rules are about the request alone, so they are decided first
        // and before anything is written. An update REPLACES the whole manifest set, each
        // manifest persisted with its computed URL, so the same count range bounds it:
        // without that an endpoint's stored record size would be caller-chosen again.
        if let Some(violation) = crate::state::manifest_set_violation(
            &input.container_type,
            // An update carries the WHOLE manifest set ("This replaces the existing MSS
            // manifest configurations"), so the counts the request carries are the counts
            // the endpoint will have — built by the same builder the create counts
            // through, which owns the array's family order.
            crate::state::manifest_counts(
                input.hls_manifests.as_deref(),
                input.low_latency_hls_manifests.as_deref(),
                input.dash_manifests.as_deref(),
                input.mss_manifests.as_deref(),
            ),
            crate::state::ManifestSetOn::Update,
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                ValidationException {
                    message: Some(violation.message),
                    validation_exception_type: Some(
                        violation.validation_exception_type.to_string(),
                    ),
                },
            )
            .into());
        }

        // The same rule the create reads: a manifest name is what makes a manifest's
        // egress URL identify THAT manifest, so a replacement set that uses one name twice
        // is refused before anything is written.
        if let Some(rejection) = crate::state::manifest_name_collision(
            input.hls_manifests.as_deref(),
            input.low_latency_hls_manifests.as_deref(),
            input.dash_manifests.as_deref(),
            input.mss_manifests.as_deref(),
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(rejection).into());
        }

        // The same rule the create reads: a segment's key provider role has to be the ARN
        // of an IAM role, decided before a single field is written.
        if let Some(rejection) = crate::state::speke_role_arn_rejection(input.segment.as_ref()) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(rejection).into());
        }

        // And the rest of the encryption block, through the same reading of it the create
        // uses: the replacement segment is judged against the container type the endpoint
        // packages with before it replaces the stored one.
        if let Some(rejection) = crate::state::segment_encryption_rejection(
            input.segment.as_ref(),
            &input.container_type,
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(rejection).into());
        }

        let Some(group) = store.channel_group_mut(&region, &input.channel_group_name) else {
            return Err(UpdateOriginEndpointErrorModeled::ResourceNotFoundException(
                crate::state::ResourceKind::ChannelGroup.not_found(),
            )
            .into());
        };
        let egress_domain = group.egress_domain.clone();
        let group_name = group.name.clone();
        let Some(channel) = group.channels.get_mut(&input.channel_name) else {
            return Err(UpdateOriginEndpointErrorModeled::ResourceNotFoundException(
                crate::state::ResourceKind::Channel.not_found(),
            )
            .into());
        };
        let channel_name = channel.name.clone();
        // The parent channel's input type decides which of the endpoint's members may be
        // carried at all, and it is immutable, so it is the same one the create read.
        let input_type = channel.input_type.clone();
        let channel_output_locking_mode = channel.output_locking_mode.clone();
        let Some(endpoint) = channel
            .origin_endpoints
            .get_mut(&input.origin_endpoint_name)
        else {
            return Err(UpdateOriginEndpointErrorModeled::ResourceNotFoundException(
                crate::state::ResourceKind::OriginEndpoint.not_found(),
            )
            .into());
        };

        // `ETag` is the precondition the caller read from an earlier response: "If the
        // specified ETag does not match the resource's current entity tag, the update
        // request will be rejected". The model declares no precondition error for this
        // operation, and the resource having moved on under the caller is what
        // `ConflictException` / `CONFLICTING_OPERATION` describes — which is the ONE rule
        // every conditional writer of this service resolves through
        // (`crate::state::stale_entity_tag_rejection`), so this operation judges the
        // precondition and words the refusal exactly as they do: it is the FIRST thing
        // decided once the addressed resource is in hand, before any rule of this
        // operation's own that reads the stored resource or its parents. A caller whose
        // tag is no longer current is holding a stale picture of the very resource those
        // rules compare against, so the lost race is what it has to be told about; the
        // request-only rules above are decided without the store at all and so cannot
        // come after it.
        if let Some(conflict) =
            crate::state::stale_entity_tag_rejection(input.e_tag.as_deref(), &endpoint.etag)
        {
            return Err(UpdateOriginEndpointErrorModeled::ConflictException(conflict).into());
        }

        // The immutable members: `ContainerType` is required on every update, so it is
        // compared always; `StreamNameOutputMode` and `Segment.OutputTimestampMode` only
        // when the request provides one, because an absent request value means "keep what
        // is there". An endpoint under a channel whose input type may carry a stream-name
        // output mode HAS one, and every endpoint whose channel carries an output-timestamp
        // mode has one of those too — the service applies both at create whether or not the
        // request named them — so a request naming a different value is asking to change an
        // immutable member, while a mode named where the channel carries none at all is
        // refused as not configurable rather than as a change, through the same rules the
        // create reads. All are refused BEFORE a single field is written, so a rejected
        // update leaves the endpoint — and its ETag — exactly as it was.
        if input.container_type != endpoint.container_type {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                immutable_violation(&CONTAINER_TYPE),
            )
            .into());
        }
        if let Some(rejection) = crate::state::origin_endpoint_input_type_violation(
            crate::state::SubmittedOriginEndpointMembers {
                stream_name_output_mode: input.stream_name_output_mode.is_some(),
                force_endpoint_error_configuration: input
                    .force_endpoint_error_configuration
                    .is_some(),
                ism_container_type: crate::state::is_ism_container(&input.container_type),
                low_latency_hls_manifests: input
                    .low_latency_hls_manifests
                    .as_ref()
                    .is_some_and(|manifests| !manifests.is_empty()),
                mss_manifests: input
                    .mss_manifests
                    .as_ref()
                    .is_some_and(|manifests| !manifests.is_empty()),
                startover_window_seconds: input.startover_window_seconds.is_some(),
            },
            &input_type,
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(rejection).into());
        }
        if input
            .stream_name_output_mode
            .as_deref()
            .is_some_and(|requested| Some(requested) != endpoint.stream_name_output_mode.as_deref())
        {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                immutable_violation(&STREAM_NAME_OUTPUT_MODE),
            )
            .into());
        }
        if let Some(rejection) = crate::state::output_timestamp_mode_violation(
            input.segment.as_ref(),
            channel_output_locking_mode.as_deref(),
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(rejection).into());
        }
        if let (Some(requested), Some(stored)) = (
            input
                .segment
                .as_ref()
                .and_then(|segment| segment.output_timestamp_mode.as_ref()),
            endpoint.segment.output_timestamp_mode.as_ref(),
        ) {
            if requested != stored {
                return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                    immutable_violation(&OUTPUT_TIMESTAMP_MODE),
                )
                .into());
            }
        }
        // The URLs of the replacement manifests are rooted exactly where the endpoint's
        // own are: the parent group's egress domain and this resource path.
        let at = crate::state::ManifestLocation {
            egress_domain: &egress_domain,
            region: &region,
            group: &group_name,
            channel: &channel_name,
            endpoint: &input.origin_endpoint_name,
        };
        // The manifest set this update would leave the endpoint publishing, materialised
        // before anything is written so the rule below can read it: an update carries the
        // WHOLE set, so a family the request omits ends up empty.
        let hls_manifests: Vec<_> = input
            .hls_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_hls_manifest(manifest, at))
            .collect();
        let low_latency_hls_manifests: Vec<_> = input
            .low_latency_hls_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_low_latency_hls_manifest(manifest, at))
            .collect();
        let dash_manifests: Vec<_> = input
            .dash_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_dash_manifest(manifest, at))
            .collect();
        let mss_manifests: Vec<_> = input
            .mss_manifests
            .unwrap_or_default()
            .into_iter()
            .map(|manifest| crate::state::stored_mss_manifest(manifest, at))
            .collect();

        let now = crate::state::now();
        // The status each of the endpoint's harvest jobs HAS at this instant is what the
        // rule below is about, so the transitions the clock already owed them are
        // materialised first — each was due at the same instant whether or not this
        // update arrived, exactly as the reporting operations materialise them.
        for job in endpoint.harvest_jobs.values_mut() {
            job.materialise_due_transitions(now);
        }
        // A harvest job of this endpoint that has not finished harvests the manifests it
        // named, and those names resolve against the manifests the endpoint publishes —
        // the reference `CreateHarvestJob` refuses to establish against a manifest that is
        // not published. An update that stopped publishing one of them would leave that
        // reference pointing at nothing, which is the inconsistent state the model's
        // `ConflictException` describes, so the replacement set is judged against the
        // stored jobs through the same predicate the create reads and the endpoint is left
        // exactly as it was.
        if crate::state::harvest_job_harvests_unpublished_manifest(
            endpoint,
            crate::state::PublishedManifests {
                hls: &hls_manifests,
                low_latency_hls: &low_latency_hls_manifests,
                dash: &dash_manifests,
            },
        ) {
            return Err(UpdateOriginEndpointErrorModeled::ConflictException(
                crate::state::harvested_manifest_in_use(),
            )
            .into());
        }
        // What the endpoint was before this request wrote anything, so the commit below
        // can tell a real change from an update that stores what was already there.
        let before = endpoint.configuration_fingerprint();
        endpoint.hls_manifests = hls_manifests;
        endpoint.low_latency_hls_manifests = low_latency_hls_manifests;
        endpoint.dash_manifests = dash_manifests;
        endpoint.mss_manifests = mss_manifests;
        // An update carries the whole packaging configuration, so a member the request
        // omits is cleared rather than kept — except the ones the model says are preserved
        // when absent (`UriSeparator`: "If you don't specify a value in the update
        // request, the current value is preserved") or cannot be changed at all. The
        // members the SERVICE applies are re-materialised here exactly as the create
        // materialises them, so an update that names none of them leaves the endpoint
        // reporting the service's own settings rather than nothing.
        let mut segment = crate::state::materialized_segment(
            input.segment,
            channel_output_locking_mode.as_deref(),
        );
        // Except the output-timestamp mode: that member is immutable after creation, so
        // the mode the create materialised stands whatever the replacement segment
        // carries — a request naming a different one, or naming one at all under a
        // channel that carries none, was refused above, and a channel cannot start or
        // stop carrying the member later (`UpdateChannel` takes no `OutputLockingMode`).
        segment.output_timestamp_mode = endpoint.segment.output_timestamp_mode.clone();
        endpoint.segment = segment;
        endpoint.description = input.description;
        endpoint.startover_window_seconds = crate::state::materialized_startover_window_seconds(
            input.startover_window_seconds,
            &input_type,
        );
        endpoint.force_endpoint_error_configuration = input.force_endpoint_error_configuration;
        if let Some(separator) = input.uri_separator {
            endpoint.uri_separator = separator;
        }
        // The immutability check above has already established that a provided mode equals
        // the stored one, so this assignment keeps the endpoint's mode as it was.
        if let Some(mode) = input.stream_name_output_mode {
            endpoint.stream_name_output_mode = Some(mode);
        }
        // A write that CHANGED the endpoint commits a new version of it, and the ETag is
        // a function of that version as well as of the endpoint's content: the tag this
        // update answers with then differs from the one the caller held — which is what
        // makes `x-amzn-update-if-match` detect a lost update — and a following
        // `GetOriginEndpoint` reports exactly this tag. An update that stored what was
        // already there changed nothing, so it commits nothing: neither the tag nor
        // `ModifiedAt` moves, and the tag the caller holds stays usable as the
        // precondition of the update it is entitled to make.
        endpoint.commit_if_changed(&before, now);

        Ok(endpoint_output(endpoint))
    }
}
