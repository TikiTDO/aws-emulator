Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c8784923130dd4420e009c5117a8bbfb746443f0

# Materialise the documented service-applied values inside Segment.Encryption and Segment.Scte (ConstantInitializationVector above all)

_Filed: 2026-10-29T10:39:36Z_
_Operation: CreateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c8784923130dd4420e009c5117a8bbfb746443f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An origin endpoint's `Segment` is materialised by `crate::state::materialized_segment`, which applies the service's own value to every top-level member the caller left out (duration, segment name, the TS toggles, the output-timestamp mode) but hands the NESTED `Encryption` and `Scte` blocks through byte-for-byte. So the members the model documents the SERVICE as supplying inside those blocks are never applied:

1. `Encryption$ConstantInitializationVector`. The model states outright that MediaPackage CREATES this value when the request omits it. The handler stores the submitted `Encryption` unchanged, so `CreateOriginEndpoint`, every later `GetOriginEndpoint`/`UpdateOriginEndpoint` and the endpoint's whole lifetime report an `Encryption` block with no `ConstantInitializationVector` at all — a value the service is documented to have created is reported as absent, and nothing a caller does later can make the endpoint report one (the member is not even consulted anywhere in the crate).
2. `Encryption$CmafExcludeSegmentDrmMetadata`, whose doc ends "Default: false", stays absent for the same reason, where the sibling booleans of `Segment` (`TsUseAudioRenditionGroup`, `IncludeIframeOnlyStreams`, `TsIncludeDvbSubtitles`) are all materialised to a concrete value in the very same function.
3. The same hole in `Scte`: `ScteHls$ScteInManifests` ("If you don't specify a value, the default is ALL") and `Scte$ScteInSegments` ("None – SCTE-35 messages are not included in segments (default)") are documented service-applied values that are reported as absent.

This contradicts the crate's own stated rule for exactly this class of member (state.rs:26-32: such members "are materialised ONCE, at create time, from the submitted value or the service's default, and stored"). `UpdateOriginEndpoint` shares the same call and therefore the same gap; `CreateOriginEndpoint` is the first writer. This is NOT the open CreateOriginEndpoint `validation` issue (20261024T071622Z), which is about REFUSING bad encryption settings — here a well-formed, accepted request is stored and reported with a member the model says the service supplies.

## Evidence

Smithy (`smithy_query(query="doc", shape="Encryption", member="ConstantInitializationVector")`): "A 128-bit, 16-byte hex value represented by a 32-character string, used in conjunction with the key for encrypting content. If you don't specify a value, then MediaPackage creates the constant initialization vector (IV)."
Smithy (`Encryption$CmafExcludeSegmentDrmMetadata`): "... Default behavior is preserved when this setting is disabled ... Valid values: true | false ... Default: false".
Smithy (`ScteHls$ScteInManifests`): "Controls which SCTE-35 events appear in HLS manifests. ALL includes all non-implicit SCTE-35 events ... If you don't specify a value, the default is ALL."
Smithy (`Scte$ScteInSegments`): "None – SCTE-35 messages are not included in segments (default)".

Handler/state, services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1516-1517 (doc of `materialized_segment`): "`Scte` and `Encryption` are reported only when they were configured, so they pass through untouched." — and the body that follows materialises every sibling but nothing inside them, state.rs:1539-1563:
    segment.segment_duration_seconds = Some(segment.segment_duration_seconds.unwrap_or(DEFAULT_SEGMENT_DURATION_SECONDS));
    segment.segment_name = Some(segment.segment_name.unwrap_or_else(|| DEFAULT_SEGMENT_NAME.to_string()));
    segment.ts_use_audio_rendition_group = Some(segment.ts_use_audio_rendition_group.unwrap_or(DEFAULT_TS_USE_AUDIO_RENDITION_GROUP));
(no assignment to `segment.encryption` or `segment.scte` anywhere in the function, state.rs:1526-1564).

The crate's own rule it breaks, state.rs:26-32: "The same holds for the members the SERVICE applies rather than the caller: ... an origin endpoint's segment settings, startover window and URI/stream-name modes, and every manifest configuration's window, URI path type, child-manifest URL encoding ... are materialised ONCE, at create time, from the submitted value or the service's default, and stored — so the create response, a later read and the list projection all report the same concrete value."

Call sites: src/ops/create_origin_endpoint.rs:525-528 `segment: crate::state::materialized_segment(input.segment, channel_output_locking_mode.as_deref()),` and src/ops/update_origin_endpoint.rs:536-539 `let mut segment = crate::state::materialized_segment(input.segment, channel_output_locking_mode.as_deref());`.

No `grep` hit for `constant_initialization_vector` or `cmaf_exclude_segment_drm_metadata` outside the generated type declaration (src/ops/types/shared_types.rs:110 and :113), so no path applies either value.

Test coverage: tests/all/server_materialised_members.rs:3-12 enumerates the materialised members under test — "the channel's `InputSwitchConfiguration` / `OutputHeaderConfiguration`, the endpoint's `Segment`, `StartoverWindowSeconds`, `UriSeparator` and `StreamNameOutputMode`, and each manifest configuration's `ManifestWindowSeconds` / `UriPathType` / `UrlEncodeChildManifest`" — and names nothing inside `Encryption`/`Scte`. tests/all/caller_supplied_arns.rs:439-444 is the only test that reads a stored encryption block and it asserts one pointer only (`read["Segment"]["Encryption"]["SpekeKeyProvider"]["RoleArn"]`), so no test pins the member as absent.

## Steps to Reproduce

1. CreateChannelGroup `g`, then CreateChannel `g`/`c` (defaults, so InputType HLS).
2. CreateOriginEndpoint `g`/`c`/`e` with ContainerType `TS`, one HlsManifests entry, and Segment = {"Encryption": {"EncryptionMethod": {"TsEncryptionMethod": "AES_128"}, "SpekeKeyProvider": {"EncryptionContractConfiguration": {"PresetSpeke20Audio": "PRESET_AUDIO_1", "PresetSpeke20Video": "PRESET_VIDEO_1"}, "ResourceId": "MovieNight", "DrmSystems": ["WIDEVINE"], "RoleArn": "arn:aws:iam::123456789012:role/SpekeAccess", "Url": "https://speke.example.com/copyProtection"}}} — i.e. no ConstantInitializationVector.
3. Observe the 200 response: `Segment.Encryption` carries no `ConstantInitializationVector` and no `CmafExcludeSegmentDrmMetadata`, although the model says MediaPackage creates the IV when the request names none.
4. GetOriginEndpoint `g`/`c`/`e`: the same two members are still absent, so the value the service is documented to have created is never readable.
5. Repeat with Segment.Scte = {"ScteFilter": ["BREAK"]} and a manifest carrying `ScteHls` with no `ScteInManifests`: both `Scte.ScteInSegments` and `ScteHls.ScteInManifests` are reported absent instead of the documented `NONE`/`ALL`.
