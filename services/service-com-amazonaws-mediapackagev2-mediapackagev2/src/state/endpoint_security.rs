// SPDX-License-Identifier: CC0-1.0

//! The rules an origin endpoint's CDN authorization, segment encryption and key-provider
//! URL are held to.
//!
//! One concern, split out of [`super`] so each part of this service's state module groups
//! the rules that belong together: every refusal here is about a block of an origin
//! endpoint's configuration that names something OUTSIDE the endpoint — the secrets and
//! role CDN authorization reads, the DRM systems and encryption contract a segment is
//! encrypted under, and the key-provider URL SPEKE is reached at. Each is worded by the
//! crate's one validation builder ([`super::validation_failed`]), so they read exactly
//! like every other `ValidationException` this service states.

use super::*;

/// Refuse a `CdnAuthConfiguration` whose ARNs are not ones this service could ever read.
///
/// CDN authorization makes MediaPackage fetch the named secrets through the named role, so
/// each entry of `CdnIdentifierSecretArns` has to be the ARN of a Secrets Manager secret in
/// the SAME account and Region as the endpoint the policy is attached to (the account every
/// resource here belongs to, [`::engine::emulator_account_id`], and `region`, the Region the
/// request is served in), the list must name each secret once, and `SecretsRoleArn` has to
/// be the ARN of an IAM role. A value outside that is an input failure whose
/// `ValidationExceptionType` names which of those it is, and `None` means the block is one
/// the service could act on.
///
/// How MANY secrets the list may carry is the model's own `@length 1..100` on
/// `CdnIdentifierSecretArns`, so the boundary validator refuses a list outside it before any
/// handler runs; this reads only what the model says nothing about.
pub fn cdn_auth_rejection(
    configuration: &CdnAuthConfiguration,
    region: &str,
) -> Option<ValidationException> {
    /// The wire path of the block's members, under the member the request carries it at.
    const SECRETS: &str = "cdnAuthConfiguration.cdnIdentifierSecretArns";
    const ROLE: &str = "cdnAuthConfiguration.secretsRoleArn";

    let account = ::engine::emulator_account_id();
    for (index, secret) in configuration.cdn_identifier_secret_arns.iter().enumerate() {
        let path = element_path(SECRETS, index);
        let Some(arn) = ::service_common::arn::parse(secret)
            .filter(|_| SECRETS_MANAGER_SECRET_ARN.admits(secret))
        else {
            return Some(validation_failed(
                &path,
                &arn_requirement_detail(SECRETS_MANAGER_SECRET_ARN),
                VALIDATION_MALFORMED_SECRET_ARN,
            ));
        };
        if arn.region != region {
            return Some(validation_failed(
                &path,
                &arn_scope_detail("the Region", region),
                VALIDATION_SECRET_FROM_DIFFERENT_REGION,
            ));
        }
        if arn.account != account {
            return Some(validation_failed(
                &path,
                &arn_scope_detail("the account", account),
                VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT,
            ));
        }
    }

    // Repetition is judged after the forms are, so a list of junk is told what an entry has
    // to BE before it is told it sent one twice. The model puts no `@uniqueItems` on the
    // list, so this is the handler's to read.
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for (index, secret) in configuration.cdn_identifier_secret_arns.iter().enumerate() {
        if !named.insert(secret.as_str()) {
            let requirement = format!(
                "Member must name a secret the list does not already name. {CHECK_THE_VALUE}"
            );
            return Some(validation_failed(
                &element_path(SECRETS, index),
                &requirement,
                VALIDATION_DUPLICATED_SECRET,
            ));
        }
    }

    (!IAM_ROLE_ARN.admits(&configuration.secrets_role_arn)).then(|| {
        validation_failed(
            ROLE,
            &arn_requirement_detail(IAM_ROLE_ARN),
            VALIDATION_ROLE_ARN_INVALID_FORMAT,
        )
    })
}

/// Refuse a segment whose SPEKE key provider names something other than an IAM role.
///
/// `SpekeKeyProvider$RoleArn` is the role MediaPackage ASSUMES to reach the key provider, so
/// the same rule as the CDN authorization role applies to it: a value that is not the ARN of
/// an IAM role names no role to assume. Shared by the writers of an endpoint's segment
/// (`CreateOriginEndpoint` and `UpdateOriginEndpoint`) so both read it the same way.
/// `SpekeKeyProvider$CertificateArn` states its own `@pattern` in the model, so the boundary
/// validator reads that one and this does not.
pub fn speke_role_arn_rejection(segment: Option<&Segment>) -> Option<ValidationException> {
    let role_arn = &segment?.encryption.as_ref()?.speke_key_provider.role_arn;
    (!IAM_ROLE_ARN.admits(role_arn)).then(|| {
        validation_failed(
            "segment.encryption.spekeKeyProvider.roleArn",
            &arn_requirement_detail(IAM_ROLE_ARN),
            VALIDATION_ROLE_ARN_INVALID_FORMAT,
        )
    })
}

// =============================================================================
// Validation: the encryption settings an origin endpoint's segment carries
// =============================================================================

/// `ValidationExceptionType` values a refused `Segment.Encryption` carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way the block can be a configuration the service could not
/// act on: the encryption METHOD against the endpoint's container format
/// ([`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`]), the constant
/// initialization vector against that method ([`VALIDATION_CENC_IV_INCOMPATIBLE`]), the
/// DRM systems against it ([`VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE`]), key
/// rotation and the CMAF DRM-metadata toggle against the container
/// ([`VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION`],
/// [`VALIDATION_CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE`]), the
/// encryption contract's presets against each other and against the container
/// ([`VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED`],
/// [`VALIDATION_ENCRYPTION_CONTRACT_SHARED`],
/// [`VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE`]) and the content-key
/// period timing against the SPEKE version and key rotation it needs
/// ([`VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1`],
/// [`VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION`]).
pub const VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH: &str =
    "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CENC_IV_INCOMPATIBLE: &str = "CENC_IV_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE: &str =
    "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION: &str = "ISM_CONTAINER_WITH_KEY_ROTATION";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE: &str =
    "CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED: &str = "ENCRYPTION_CONTRACT_UNENCRYPTED";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_SHARED: &str = "ENCRYPTION_CONTRACT_SHARED";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE: &str =
    "ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1: &str =
    "CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1";
/// See [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`].
pub const VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION: &str =
    "CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION";

/// The wire path of the encryption block, under the member the request carries it at.
pub(super) const ENCRYPTION_MEMBER: &str = "segment.encryption";

/// The `EncryptionMethod` member an endpoint of this container type carries its method in,
/// and the container format that member names.
///
/// `EncryptionMethod` declares exactly one member per container format the service
/// packages — `TsEncryptionMethod`, `CmafEncryptionMethod`, `IsmEncryptionMethod`
/// (`smithy_query(query="shape",shape="EncryptionMethod")`) — so which of them an endpoint
/// may carry is decided by its `ContainerType` (TS | CMAF | ISM), and the model's
/// `ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH` is the refusal of any other. `None` for a
/// container value the model does not name; the boundary validator has already refused one
/// against the `ContainerType` enum.
pub(super) fn encryption_method_member(container: &str) -> Option<&'static str> {
    match container {
        "TS" => Some("tsEncryptionMethod"),
        CONTAINER_TYPE_CMAF => Some("cmafEncryptionMethod"),
        CONTAINER_TYPE_ISM => Some("ismEncryptionMethod"),
        _ => None,
    }
}

/// The `ContainerType` whose endpoints package CMAF segments, from the model's
/// `ContainerType` enum — the container the CMAF-only members of an encryption block
/// belong to (see [`container_carries_cmaf_exclude_segment_drm_metadata`]).
pub(super) const CONTAINER_TYPE_CMAF: &str = "CMAF";

/// Every `EncryptionMethod` member the request set, as `(member, value)` pairs in the
/// order the model declares them.
pub(super) fn submitted_encryption_methods(method: &EncryptionMethod) -> Vec<(&'static str, &str)> {
    [
        ("tsEncryptionMethod", method.ts_encryption_method.as_deref()),
        (
            "cmafEncryptionMethod",
            method.cmaf_encryption_method.as_deref(),
        ),
        (
            "ismEncryptionMethod",
            method.ism_encryption_method.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(member, value)| value.map(|value| (member, value)))
    .collect()
}

/// The `DrmSystem` values an encryption method can protect content with, or an empty slice
/// for a method value the model does not name.
///
/// Each encryption method is a packaging of ONE content-protection scheme, and each DRM
/// system supports the schemes its vendor implements: `AES_128` is HLS AES-128 with a key
/// the player fetches in the clear, which is what `CLEAR_KEY_AES_128` names; `SAMPLE_AES`
/// is Apple's sample encryption, used by `FAIRPLAY`; `CENC` and `CBCS` are the two Common
/// Encryption schemes, and of them FairPlay signals only `cbcs`, while the Common
/// Encryption systems (`PLAYREADY`, `WIDEVINE`, `IRDETO`) carry both. The model's
/// `DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE` is the refusal of a pair outside that, and
/// the values are the model's own (`smithy_query(query="shape",shape="DrmSystem,
/// CmafEncryptionMethod,TsEncryptionMethod,IsmEncryptionMethod")`).
pub(super) fn drm_systems_for_encryption_method(method: &str) -> &'static [&'static str] {
    match method {
        "AES_128" => &["CLEAR_KEY_AES_128"],
        "SAMPLE_AES" => &["FAIRPLAY"],
        ENCRYPTION_METHOD_CENC => &["PLAYREADY", "WIDEVINE", "IRDETO"],
        "CBCS" => &["FAIRPLAY", "PLAYREADY", "WIDEVINE", "IRDETO"],
        _ => &[],
    }
}

/// The Common Encryption method value both the CMAF and the ISM container name, from the
/// model's `CmafEncryptionMethod` and `IsmEncryptionMethod` enums.
pub(super) const ENCRYPTION_METHOD_CENC: &str = "CENC";

/// The `PresetSpeke20Audio` / `PresetSpeke20Video` value that asks for ONE content key
/// across every audio and video track ("SHARED - Use the same content key for all of the
/// audio and video tracks in your stream").
pub(super) const PRESET_SHARED: &str = "SHARED";

/// The preset value that asks for no encryption of its track group ("UNENCRYPTED - Don't
/// encrypt any of the audio tracks in your stream").
pub(super) const PRESET_UNENCRYPTED: &str = "UNENCRYPTED";

/// The `ContentKeyPeriodTiming` values whose doc states that they signal the start and end
/// times a content key is used for, and therefore "Requires `SpekeVersion` `V2_1`" and key
/// rotation ("Signaling start and end times … also requires key rotation to be enabled",
/// `smithy_query(query="doc",shape="ContentKeyPeriodConfiguration",member="ContentKeyPeriodTiming")`).
pub(super) const CONTENT_KEY_PERIOD_TIMINGS_WITH_START_END: &[&str] =
    &["START_END_ONLY", "INDEX_WITH_START_END"];

/// The `SpekeVersion` those timings require, from the model's `SpekeVersion` enum.
pub(super) const SPEKE_VERSION_WITH_KEY_PERIOD_TIMING: &str = "V2_1";

/// The `SpekeVersion` an endpoint's key provider is read as when the request names none
/// ("If you don't specify a value, the default is `V2_0`").
pub(super) const DEFAULT_SPEKE_VERSION: &str = "V2_0";

/// The content-key period timing MediaPackage signals when a CONFIGURED
/// `ContentKeyPeriodConfiguration` names none ("If you don't specify a value, the default
/// is `INDEX_ONLY`"), from the model's `ContentKeyPeriodTiming` enum.
pub(super) const DEFAULT_CONTENT_KEY_PERIOD_TIMING: &str = "INDEX_ONLY";

/// The encryption method the segments of an endpoint of this container type are encrypted
/// with, or `None` when the block names none in the member the container configures its
/// method through ([`encryption_method_member`]).
///
/// The ONE reading of which method an endpoint encrypts with: both the refusals that are
/// about the method ([`segment_encryption_rejection`]) and the materialisation of the
/// member the method decides the presence of
/// ([`encryption_carries_constant_initialization_vector`]) resolve through it, so a stored
/// block and a refused one can never disagree about what the endpoint encrypts with.
pub(super) fn encryption_method_of<'a>(
    encryption: &'a Encryption,
    container: &str,
) -> Option<&'a str> {
    let expected_member = encryption_method_member(container)?;
    submitted_encryption_methods(&encryption.encryption_method)
        .into_iter()
        .find_map(|(member, value)| (member == expected_member).then_some(value))
}

/// Whether MediaPackage supplies a constant initialization vector to an endpoint of this
/// container type whose encryption settings name none.
///
/// `Encryption$ConstantInitializationVector` is documented as a value the service creates
/// for a request that omits it ("If you don't specify a value, then MediaPackage creates
/// the constant initialization vector (IV)"), and Common Encryption derives each sample's
/// vector from the content key instead, so an endpoint encrypting with that method has no
/// constant one at all — which is the combination a SUBMITTED vector is refused in
/// ([`VALIDATION_CENC_IV_INCOMPATIBLE`]). Both read the method off
/// [`encryption_method_of`], so the service can never create the value its own validation
/// refuses.
pub fn encryption_carries_constant_initialization_vector(
    encryption: &Encryption,
    container: &str,
) -> bool {
    encryption_method_of(encryption, container) != Some(ENCRYPTION_METHOD_CENC)
}

/// Whether segment metadata of a CMAF container keeps its DRM metadata boxes when the
/// encryption settings do not ask for their removal — the model states the member's
/// default outright ("Valid values: `true` | `false`", "Default: `false`").
pub const DEFAULT_CMAF_EXCLUDE_SEGMENT_DRM_METADATA: bool = false;

/// Whether an endpoint of this container type carries the CMAF DRM-metadata toggle at all.
///
/// `Encryption$CmafExcludeSegmentDrmMetadata` excludes boxes from "segment metadata in CMAF
/// containers" and "only affects CMAF container formats", and the model declares
/// `CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE` for the member on any
/// other container — so, like every other member with a documented default that only some
/// parents carry, its default is applied exactly where the endpoint can carry one and the
/// member stays absent everywhere else.
pub fn container_carries_cmaf_exclude_segment_drm_metadata(container: &str) -> bool {
    container == CONTAINER_TYPE_CMAF
}

/// What the digest of a created initialization vector is of, so the value can collide with
/// no other digest this module derives.
const INITIALIZATION_VECTOR_DOMAIN: &str = "mediapackagev2:segment-encryption-iv";

/// How many bytes of that digest the vector carries: `ConstantInitializationVector` is "a
/// 128-bit, 16-byte hex value represented by a 32-character string" (`@length 32..32`,
/// `@pattern ^[0-9a-fA-F]+$`).
const INITIALIZATION_VECTOR_BYTES: usize = 16;

/// The constant initialization vector MediaPackage creates for an endpoint whose
/// encryption settings name none.
///
/// Derived from the endpoint's ARN, which is the identity of the endpoint across its whole
/// lifetime: the value is therefore unique to the endpoint, the same on the create
/// response and on every later read of it, and the same one an update that names no vector
/// re-materialises — so an update that re-submits what was already stored still changes
/// nothing. Rendered as the hex of the first [`INITIALIZATION_VECTOR_BYTES`] digest bytes,
/// in the upper case AWS's own examples of the member use (the modelled pattern admits
/// either case).
///
/// It is a reported configuration value and nothing else: this service packages no
/// content and encrypts nothing, so the value is never used to encrypt or decrypt
/// anything.
pub fn created_constant_initialization_vector(endpoint_arn: &str) -> String {
    let digest = field_digest(&[
        INITIALIZATION_VECTOR_DOMAIN.to_string(),
        endpoint_arn.to_string(),
    ]);
    hex::encode_upper(&digest[..INITIALIZATION_VECTOR_BYTES])
}

/// The encryption block an origin endpoint's segment is stored with: the submitted one,
/// plus the members the model documents the SERVICE as supplying inside it.
///
/// Those are the constant initialization vector, created for an endpoint whose request
/// named none and whose encryption method takes one at all
/// ([`created_constant_initialization_vector`],
/// [`encryption_carries_constant_initialization_vector`]); the CMAF DRM-metadata
/// toggle, whose documented default ([`DEFAULT_CMAF_EXCLUDE_SEGMENT_DRM_METADATA`]) is
/// applied on an endpoint whose container carries the member
/// ([`container_carries_cmaf_exclude_segment_drm_metadata`], which is also what a SUBMITTED
/// toggle is refused by in [`segment_encryption_rejection`], so no caller's value is
/// dropped here instead of refused); and, one level down, the key
/// provider's SPEKE version ([`DEFAULT_SPEKE_VERSION`]) and the content-key period timing
/// of a configured period configuration ([`DEFAULT_CONTENT_KEY_PERIOD_TIMING`]) — the same
/// members the refusals above read a submitted block through, so a stored block reports the
/// version and the timing those rules act on. The key-rotation interval is NOT one of them:
/// its doc says an omitted interval leaves content keys unrotated, so there is no value to
/// report. Reached through [`super::materialized_segment`], so the create and the update of
/// an endpoint store one block.
pub fn materialized_encryption(submitted: Encryption, at: SegmentContext<'_>) -> Encryption {
    let mut encryption = submitted;
    if encryption_carries_constant_initialization_vector(&encryption, at.container_type) {
        encryption.constant_initialization_vector = Some(
            encryption
                .constant_initialization_vector
                .unwrap_or_else(|| created_constant_initialization_vector(at.endpoint_arn)),
        );
    }
    encryption.cmaf_exclude_segment_drm_metadata =
        container_carries_cmaf_exclude_segment_drm_metadata(at.container_type).then(|| {
            encryption
                .cmaf_exclude_segment_drm_metadata
                .unwrap_or(DEFAULT_CMAF_EXCLUDE_SEGMENT_DRM_METADATA)
        });
    let provider = &mut encryption.speke_key_provider;
    provider.speke_version = Some(
        provider
            .speke_version
            .take()
            .unwrap_or_else(|| DEFAULT_SPEKE_VERSION.to_string()),
    );
    if let Some(period) = provider.content_key_period_configuration.as_mut() {
        period.content_key_period_timing = Some(
            period
                .content_key_period_timing
                .take()
                .unwrap_or_else(|| DEFAULT_CONTENT_KEY_PERIOD_TIMING.to_string()),
        );
    }
    encryption
}

/// Refuse a segment whose encryption settings are a configuration the service could not
/// act on, or `None` when the block is one it can.
///
/// `Segment.Encryption` is a cross-member configuration: the method, the initialization
/// vector, the DRM systems, the encryption contract and the key-provider endpoint only make
/// sense together and against the endpoint's `ContainerType`, and the model states each way
/// they can disagree as its own `ValidationExceptionType` (see
/// [`VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH`]) — none of which the boundary
/// validator can decide, since each is about two members at once. Shared by the writers of
/// an endpoint's segment (`CreateOriginEndpoint` and `UpdateOriginEndpoint`) so both read
/// the block the same way, and decided before anything is stored: an endpoint persisted
/// with settings the key provider could never serve publishes manifests no player can
/// decrypt.
///
/// `SpekeKeyProvider$RoleArn` is read by [`speke_role_arn_rejection`], beside the other
/// caller-supplied ARN of this model.
pub fn segment_encryption_rejection(
    segment: Option<&Segment>,
    container: &str,
) -> Option<ValidationException> {
    let encryption = segment?.encryption.as_ref()?;
    let provider = &encryption.speke_key_provider;
    // The member this endpoint's container format configures its method through. `None`
    // for a container value the model does not name — the method rules below then decide
    // nothing, while the rules that are about the block alone still do.
    let expected_member = encryption_method_member(container);
    let submitted = submitted_encryption_methods(&encryption.encryption_method);

    // The method first: every later rule is about the method the endpoint encrypts with.
    if let Some(expected_member) = expected_member {
        for (member, _) in &submitted {
            if *member != expected_member {
                return Some(validation_failed(
                    &format!("{ENCRYPTION_MEMBER}.encryptionMethod.{member}"),
                    &format!(
                        "An encryption method of a {container} container endpoint is \
                         configured through {expected_member}. {CHECK_THE_VALUE}"
                    ),
                    VALIDATION_ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH,
                ));
            }
        }
    }
    let method = encryption_method_of(encryption, container);

    if let Some(method) = method {
        // Common Encryption derives each sample's initialization vector from the content
        // key, so a CENC endpoint has no constant one to be given.
        if encryption.constant_initialization_vector.is_some()
            && method == ENCRYPTION_METHOD_CENC
        {
            return Some(validation_failed(
                &format!("{ENCRYPTION_MEMBER}.constantInitializationVector"),
                &format!(
                    "A constant initialization vector can't be combined with the \
                     {method} encryption method. Remove the vector, or encrypt with a \
                     method that takes one"
                ),
                VALIDATION_CENC_IV_INCOMPATIBLE,
            ));
        }
        let carried = drm_systems_for_encryption_method(method);
        let incompatible = provider
            .drm_systems
            .iter()
            .find(|system| !carried.contains(&system.as_str()));
        // A method value the model does not name constrains nothing here; the boundary
        // validator has already refused one against the method's own enum.
        if let Some(system) = incompatible.filter(|_| !carried.is_empty()) {
            return Some(validation_failed(
                &format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.drmSystems"),
                &format!(
                    "The {system} DRM system can't be combined with the {method} \
                     encryption method. The DRM systems that method supports are {}",
                    carried.join(", ")
                ),
                VALIDATION_DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE,
            ));
        }
    }

    // The CMAF-only member of the block, against the container: the toggle excludes boxes
    // from "segment metadata in CMAF containers", so an endpoint that packages no CMAF
    // segments has none to exclude them from — the member the materialisation applies the
    // documented default to on exactly the containers that carry it, read off the same
    // predicate ([`container_carries_cmaf_exclude_segment_drm_metadata`]), so a submitted
    // value can never be dropped instead of refused.
    if encryption.cmaf_exclude_segment_drm_metadata.is_some()
        && !container_carries_cmaf_exclude_segment_drm_metadata(container)
    {
        return Some(validation_failed(
            &format!("{ENCRYPTION_MEMBER}.cmafExcludeSegmentDrmMetadata"),
            &format!(
                "A {container} container endpoint packages no CMAF segments to exclude DRM \
                 metadata boxes from. Remove the setting, or create an origin endpoint \
                 whose container type is {CONTAINER_TYPE_CMAF}"
            ),
            VALIDATION_CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE,
        ));
    }

    // Key rotation, and then the encryption contract, against the container.
    if is_ism_container(container) && encryption.key_rotation_interval_seconds.is_some() {
        return Some(validation_failed(
            &format!("{ENCRYPTION_MEMBER}.keyRotationIntervalSeconds"),
            &format!(
                "An {container} container endpoint can't rotate content keys. Remove the \
                 rotation interval, or create an origin endpoint whose container type \
                 rotates keys"
            ),
            VALIDATION_ISM_CONTAINER_WITH_KEY_ROTATION,
        ));
    }
    let contract = &provider.encryption_contract_configuration;
    let audio = contract.preset_speke20_audio.as_str();
    let video = contract.preset_speke20_video.as_str();
    let contract_member = format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.encryptionContractConfiguration");
    if audio == PRESET_UNENCRYPTED && video == PRESET_UNENCRYPTED {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "An encryption contract that leaves both the audio and the video tracks \
                 {PRESET_UNENCRYPTED} encrypts nothing. Choose a preset for the audio or \
                 the video tracks, or remove the encryption settings"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_UNENCRYPTED,
        ));
    }
    if (audio == PRESET_SHARED) != (video == PRESET_SHARED) {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "The {PRESET_SHARED} preset uses one content key for all of the audio and \
                 video tracks, so it has to be the preset of both. {CHECK_THE_VALUE}"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_SHARED,
        ));
    }
    if is_ism_container(container) && !(audio == PRESET_SHARED && video == PRESET_SHARED) {
        return Some(validation_failed(
            &contract_member,
            &format!(
                "An {container} container endpoint encrypts every track with one content \
                 key, so its encryption contract must use the {PRESET_SHARED} preset for \
                 the audio and the video tracks. {CHECK_THE_VALUE}"
            ),
            VALIDATION_ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE,
        ));
    }

    // The content-key period timing, against the two things its doc says it needs.
    let timing = provider
        .content_key_period_configuration
        .as_ref()
        .and_then(|configuration| configuration.content_key_period_timing.as_deref());
    if let Some(timing) = timing.filter(|timing| {
        CONTENT_KEY_PERIOD_TIMINGS_WITH_START_END.contains(timing)
    }) {
        let timing_member =
            format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.contentKeyPeriodConfiguration.contentKeyPeriodTiming");
        let speke_version = provider
            .speke_version
            .as_deref()
            .unwrap_or(DEFAULT_SPEKE_VERSION);
        if speke_version != SPEKE_VERSION_WITH_KEY_PERIOD_TIMING {
            return Some(validation_failed(
                &timing_member,
                &format!(
                    "Signaling the times a content key is used for requires SpekeVersion \
                     {SPEKE_VERSION_WITH_KEY_PERIOD_TIMING}. Use SpekeVersion \
                     {SPEKE_VERSION_WITH_KEY_PERIOD_TIMING}, or signal {timing} only as \
                     the content key index"
                ),
                VALIDATION_CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1,
            ));
        }
        if encryption.key_rotation_interval_seconds.is_none() {
            return Some(validation_failed(
                &timing_member,
                &format!(
                    "Signaling the times a content key is used for requires key rotation \
                     to be enabled. Specify KeyRotationIntervalSeconds, or signal \
                     {timing} only as the content key index"
                ),
                VALIDATION_CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION,
            ));
        }
    }

    key_provider_url_rejection(&provider.url)
}

/// `ValidationExceptionType` values a refused `SpekeKeyProvider$Url` carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way the value can fail to be an endpoint the service could
/// fetch content keys from: what it IS ([`VALIDATION_URL_INVALID`]), the scheme
/// ([`VALIDATION_URL_SCHEME`]), the credentials ([`VALIDATION_URL_USER_INFO`]) and port
/// ([`VALIDATION_URL_PORT`]) it carries, and the host it names
/// ([`VALIDATION_URL_UNKNOWN_HOST`], [`VALIDATION_URL_LOOPBACK_ADDRESS`],
/// [`VALIDATION_URL_LOCAL_ADDRESS`], [`VALIDATION_URL_LINK_LOCAL_ADDRESS`],
/// [`VALIDATION_URL_MULTICAST_ADDRESS`]).
pub const VALIDATION_URL_INVALID: &str = "URL_INVALID";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_SCHEME: &str = "URL_SCHEME";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_USER_INFO: &str = "URL_USER_INFO";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_PORT: &str = "URL_PORT";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_UNKNOWN_HOST: &str = "URL_UNKNOWN_HOST";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LOOPBACK_ADDRESS: &str = "URL_LOOPBACK_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LOCAL_ADDRESS: &str = "URL_LOCAL_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_LINK_LOCAL_ADDRESS: &str = "URL_LINK_LOCAL_ADDRESS";
/// See [`VALIDATION_URL_INVALID`].
pub const VALIDATION_URL_MULTICAST_ADDRESS: &str = "URL_MULTICAST_ADDRESS";

/// The scheme a key-provider URL has to use: `SpekeKeyProvider$Url` is "The URL of the API
/// Gateway proxy that you set up to talk to your key server", which "must start with
/// https://".
pub(super) const KEY_PROVIDER_URL_SCHEME: &str = "https";

/// The `ValidationExceptionType` for an IP address a service endpoint may not be at, or
/// `None` for an address outside those classes.
///
/// The model names one value per class of address that is not a reachable, public endpoint
/// of a key server: the loopback of whoever resolves it, an address private to a network,
/// a link-local address and a multicast group. Both IP versions, since a URL may carry
/// either.
pub(super) fn local_address_class(address: ::std::net::IpAddr) -> Option<&'static str> {
    match address {
        ::std::net::IpAddr::V4(address) => {
            if address.is_loopback() {
                Some(VALIDATION_URL_LOOPBACK_ADDRESS)
            } else if address.is_link_local() {
                Some(VALIDATION_URL_LINK_LOCAL_ADDRESS)
            } else if address.is_multicast() {
                Some(VALIDATION_URL_MULTICAST_ADDRESS)
            } else if address.is_private() || address.is_unspecified() {
                Some(VALIDATION_URL_LOCAL_ADDRESS)
            } else {
                None
            }
        }
        ::std::net::IpAddr::V6(address) => {
            if address.is_loopback() {
                Some(VALIDATION_URL_LOOPBACK_ADDRESS)
            } else if address.is_multicast() {
                Some(VALIDATION_URL_MULTICAST_ADDRESS)
            } else if address.segments()[0] & 0xffc0 == 0xfe80 {
                Some(VALIDATION_URL_LINK_LOCAL_ADDRESS)
            } else if address.segments()[0] & 0xfe00 == 0xfc00 || address.is_unspecified() {
                Some(VALIDATION_URL_LOCAL_ADDRESS)
            } else {
                None
            }
        }
    }
}

/// The host NAMES that resolve to the loopback of whoever resolves them, so a URL naming
/// one addresses the resolver itself rather than a key server: `localhost` and anything
/// under it (RFC 6761 reserves the whole `.localhost` tree for exactly this).
///
/// Names only — an address literal is read as an address
/// ([`local_address_class`]) — and a reading of a MODELLED member's value, which is why it
/// is not the framework's inbound-listener guard: that one decides which `Host` header a
/// listener answers, over a whole authority including literals and zone ids.
pub(super) fn is_loopback_domain_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// The port a key server is reached at: the default port of [`KEY_PROVIDER_URL_SCHEME`],
/// which is the only one a URL that names the scheme's own endpoint can carry.
pub(super) const KEY_PROVIDER_URL_PORT: u16 = 443;

/// Refuse a SPEKE key-provider URL that is not an endpoint this service could fetch
/// content keys from, or `None` when it is one.
///
/// The service RESOLVES this URL and calls it for every content key, so what it names has
/// to be a reachable key server: the model documents the value as "The URL of the API
/// Gateway proxy that you set up to talk to your key server", which "must reside in the
/// same AWS Region as MediaPackage and must start with https://", and its
/// `ValidationExceptionType` names one value per way the value fails that (see
/// [`VALIDATION_URL_INVALID`]). A URL carrying credentials or a port of its own does not
/// name that endpoint, and one naming a host that is not a public name — the resolver's own
/// loopback, a private, link-local or multicast address, or a name that is not a
/// fully-qualified one — names no key server reachable from the service at all.
///
/// Every class is decided from the VALUE: the emulator serves inbound requests and
/// resolves nothing, so what the host classes cover is what the text itself settles — the
/// address literals, and a single-label name, which is not a name the public DNS this
/// endpoint has to be reachable through can answer. The URI grammar itself is read by the
/// `http` crate's parser, the one every request's own URI is read with, rather than by a
/// hand-rolled split.
pub fn key_provider_url_rejection(url: &str) -> Option<ValidationException> {
    let member = format!("{ENCRYPTION_MEMBER}.spekeKeyProvider.url");
    let refused = |requirement: String, exception_type: &str| {
        Some(validation_failed(&member, &requirement, exception_type))
    };
    let parsed = url.parse::<::http::Uri>().ok().and_then(|parsed| {
        let scheme = parsed.scheme_str()?.to_string();
        let authority = parsed.authority()?.clone();
        Some((scheme, authority))
    });
    // A value the grammar does not admit, or one naming no scheme or no authority, is not
    // an endpoint at all rather than an endpoint of the wrong kind.
    let Some((scheme, authority)) = parsed else {
        return refused(
            format!("Member must be a URL naming a scheme and a host. {CHECK_THE_VALUE}"),
            VALIDATION_URL_INVALID,
        );
    };
    if scheme != KEY_PROVIDER_URL_SCHEME {
        return refused(
            format!(
                "Member must be a {KEY_PROVIDER_URL_SCHEME} URL. {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_SCHEME,
        );
    }
    // The authority carries any userinfo ahead of the host, which the host itself does not.
    if authority.as_str().contains('@') {
        return refused(
            format!("Member must not carry user information. {CHECK_THE_VALUE}"),
            VALIDATION_URL_USER_INFO,
        );
    }
    if authority
        .port_u16()
        .is_some_and(|port| port != KEY_PROVIDER_URL_PORT)
    {
        return refused(
            format!(
                "Member must not name a port other than {KEY_PROVIDER_URL_PORT}. \
                 {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_PORT,
        );
    }
    let host = authority.host();
    if is_loopback_domain_name(host) {
        return refused(
            format!(
                "Member must name a host that is not the loopback of the service \
                 resolving it. {CHECK_THE_VALUE}"
            ),
            VALIDATION_URL_LOOPBACK_ADDRESS,
        );
    }
    // An IPv6 literal is bracketed in an authority, which is where its own colons live.
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    match literal.parse::<::std::net::IpAddr>() {
        Ok(address) => local_address_class(address).and_then(|exception_type| {
            refused(
                format!(
                    "Member must name a host that is reachable from the service. \
                     {CHECK_THE_VALUE}"
                ),
                exception_type,
            )
        }),
        // A single-label name is not a name the service can resolve to a key server.
        Err(_) => (!host.trim_end_matches('.').contains('.')).then(|| {
            validation_failed(
                &member,
                &format!("Member must name a host that resolves. {CHECK_THE_VALUE}"),
                VALIDATION_URL_UNKNOWN_HOST,
            )
        }),
    }
}
