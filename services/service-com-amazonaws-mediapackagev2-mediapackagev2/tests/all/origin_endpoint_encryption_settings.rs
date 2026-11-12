// SPDX-License-Identifier: CC0-1.0

//! The encryption settings an origin endpoint's `Segment` carries, driven end-to-end over
//! HTTP through the same router the emulator boots and reusing the restJson1 client in
//! `channel_lifecycle`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over one recorded
//! request: `Segment.Encryption` is a cross-member configuration, so the method, the
//! initialization vector, the DRM systems, key rotation, the encryption contract, the
//! content-key period timing and the key-server endpoint are read against each other AND
//! against the endpoint's `ContainerType` before anything is stored — a block the service
//! could not act on is a 400 `ValidationException` carrying the
//! `ValidationExceptionType` the model names for which of those it is, with no endpoint
//! created, and a block it can act on is stored with every submitted member unchanged —
//! beside the members the service supplies inside it, which
//! `segment_service_applied_blocks` pins — and reported by `GetOriginEndpoint`.
//!
//! Every discriminator below is a value of the model's own `ValidationExceptionType` enum,
//! and every enum value a row submits comes from the model's `ContainerType`,
//! `TsEncryptionMethod`, `CmafEncryptionMethod`, `IsmEncryptionMethod`, `DrmSystem`,
//! `PresetSpeke20Audio`, `PresetSpeke20Video`, `ContentKeyPeriodTiming` and `SpekeVersion`
//! shapes
//! (`smithy_query(query="shape", shape="ValidationExceptionType,ContainerType,EncryptionMethod,DrmSystem,PresetSpeke20Audio,PresetSpeke20Video,ContentKeyPeriodTiming,SpekeVersion")`).
//!
//! `SpekeKeyProvider$RoleArn` is the one member of the block covered elsewhere
//! (`caller_supplied_arns`), beside the other caller-supplied ARN of this model.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The parents every endpoint in this suite hangs under. A CMAF-input channel, because the
/// rows span every container type and an ISM container is the one an endpoint of a
/// multiview channel may not carry (`endpoint_settings_by_input_type` covers that).
const GROUP: &str = "enc-group";
const CHANNEL: &str = "enc-channel";

/// The role the key provider names: an IAM role ARN, so no row is refused for the ARN rule
/// `caller_supplied_arns` covers.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/SpekeAccess",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The key-server endpoint a row uses when it is not the thing under test: an `https` URL
/// at a resolvable host with no credentials and no port of its own.
const KEY_SERVER_URL: &str = "https://speke.example.com/copyProtection";

/// A 128-bit initialization vector as `Encryption$ConstantInitializationVector` takes it:
/// "A 128-bit, 16-byte hex value represented by a 32-character string" (`@length 32..32`,
/// `@pattern ^[0-9a-fA-F]+$`), so a shorter one would never reach a handler.
const CONSTANT_IV: &str = "A1B2C3D4E5F60718293A4B5C6D7E8F90";

/// The encryption block a row submits, built from the parts a row varies. Every required
/// member of `Encryption` and `SpekeKeyProvider` is always present, so what a row changes
/// is only the combination under test.
struct Block {
    /// `(member, value)` for the one `EncryptionMethod` member the row sets.
    method: (&'static str, &'static str),
    constant_initialization_vector: Option<&'static str>,
    key_rotation_interval_seconds: Option<i32>,
    cmaf_exclude_segment_drm_metadata: Option<bool>,
    drm_systems: &'static [&'static str],
    presets: (&'static str, &'static str),
    speke_version: Option<&'static str>,
    content_key_period_timing: Option<&'static str>,
    url: &'static str,
}

impl Default for Block {
    /// The block every row starts from: CMAF Common Encryption, one PlayReady content key
    /// per audio and video preset, at the key server above.
    fn default() -> Self {
        Self {
            method: ("CmafEncryptionMethod", "CENC"),
            constant_initialization_vector: None,
            key_rotation_interval_seconds: None,
            cmaf_exclude_segment_drm_metadata: None,
            drm_systems: &["PLAYREADY"],
            presets: ("PRESET_AUDIO_1", "PRESET_VIDEO_1"),
            speke_version: None,
            content_key_period_timing: None,
            url: KEY_SERVER_URL,
        }
    }
}

impl Block {
    /// The `Segment` a create or update carries this block in.
    fn segment(&self) -> Value {
        let (method_member, method_value) = self.method;
        let (audio, video) = self.presets;
        let mut provider = json!({
            "EncryptionContractConfiguration": {
                "PresetSpeke20Audio": audio,
                "PresetSpeke20Video": video,
            },
            "ResourceId": "MovieNight20171126093045",
            "DrmSystems": self.drm_systems,
            "RoleArn": role_arn(),
            "Url": self.url,
        });
        if let Some(version) = self.speke_version {
            provider["SpekeVersion"] = json!(version);
        }
        if let Some(timing) = self.content_key_period_timing {
            provider["ContentKeyPeriodConfiguration"] = json!({"ContentKeyPeriodTiming": timing});
        }
        let mut encryption = json!({
            "EncryptionMethod": {method_member: method_value},
            "SpekeKeyProvider": provider,
        });
        if let Some(vector) = self.constant_initialization_vector {
            encryption["ConstantInitializationVector"] = json!(vector);
        }
        if let Some(interval) = self.key_rotation_interval_seconds {
            encryption["KeyRotationIntervalSeconds"] = json!(interval);
        }
        if let Some(exclude) = self.cmaf_exclude_segment_drm_metadata {
            encryption["CmafExcludeSegmentDrmMetadata"] = json!(exclude);
        }
        json!({"Encryption": encryption})
    }
}

/// What a create carrying the row's block must answer.
enum Expected {
    /// 200, with the block stored as submitted.
    Stored,
    /// 400 `ValidationException` with this `ValidationExceptionType`, naming this member
    /// path, and with no endpoint created.
    Refused(&'static str, &'static str),
}

/// One row: the endpoint's container type, the encryption block, and the answer.
struct Case {
    label: &'static str,
    container: &'static str,
    block: Block,
    expected: Expected,
}

/// The rows. They differ along everything the rule generalises over: the container type
/// (TS | CMAF | ISM), which `EncryptionMethod` member the request sets and which value it
/// names, whether a constant initialization vector, key rotation and the CMAF
/// DRM-metadata toggle are configured, which
/// DRM systems the key provider lists, which presets the encryption contract names, which
/// content-key period timing is signalled under which SPEKE version, and the form of the
/// key-server URL.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "CMAF with Common Encryption and a PlayReady key per track group",
            container: "CMAF",
            block: Block::default(),
            expected: Expected::Stored,
        },
        Case {
            label: "CMAF with CBCS and FairPlay, which signals that scheme",
            container: "CMAF",
            block: Block {
                method: ("CmafEncryptionMethod", "CBCS"),
                drm_systems: &["FAIRPLAY", "WIDEVINE"],
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        Case {
            label: "TS with AES-128, a clear key and a constant initialization vector",
            container: "TS",
            block: Block {
                method: ("TsEncryptionMethod", "AES_128"),
                drm_systems: &["CLEAR_KEY_AES_128"],
                constant_initialization_vector: Some(CONSTANT_IV),
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        Case {
            label: "TS with sample AES and FairPlay",
            container: "TS",
            block: Block {
                method: ("TsEncryptionMethod", "SAMPLE_AES"),
                drm_systems: &["FAIRPLAY"],
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        Case {
            label: "ISM with Common Encryption and one key for every track",
            container: "ISM",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                presets: ("SHARED", "SHARED"),
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        // The encryption method against the container format.
        Case {
            label: "a TS method on a CMAF endpoint",
            container: "CMAF",
            block: Block {
                method: ("TsEncryptionMethod", "AES_128"),
                drm_systems: &["CLEAR_KEY_AES_128"],
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH",
                "segment.encryption.encryptionMethod.tsEncryptionMethod",
            ),
        },
        Case {
            label: "a CMAF method on a TS endpoint",
            container: "TS",
            block: Block::default(),
            expected: Expected::Refused(
                "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH",
                "segment.encryption.encryptionMethod.cmafEncryptionMethod",
            ),
        },
        Case {
            label: "an ISM method on a CMAF endpoint",
            container: "CMAF",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH",
                "segment.encryption.encryptionMethod.ismEncryptionMethod",
            ),
        },
        Case {
            label: "a CMAF method on an ISM endpoint",
            container: "ISM",
            block: Block {
                presets: ("SHARED", "SHARED"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_METHOD_CONTAINER_TYPE_MISMATCH",
                "segment.encryption.encryptionMethod.cmafEncryptionMethod",
            ),
        },
        // The initialization vector against the method.
        Case {
            label: "a constant initialization vector with Common Encryption",
            container: "CMAF",
            block: Block {
                constant_initialization_vector: Some(CONSTANT_IV),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CENC_IV_INCOMPATIBLE",
                "segment.encryption.constantInitializationVector",
            ),
        },
        Case {
            label: "a constant initialization vector on an ISM endpoint, which is CENC too",
            container: "ISM",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                presets: ("SHARED", "SHARED"),
                constant_initialization_vector: Some(CONSTANT_IV),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CENC_IV_INCOMPATIBLE",
                "segment.encryption.constantInitializationVector",
            ),
        },
        // The DRM systems against the method.
        Case {
            label: "FairPlay with Common Encryption, which it does not signal",
            container: "CMAF",
            block: Block {
                drm_systems: &["FAIRPLAY"],
                ..Block::default()
            },
            expected: Expected::Refused(
                "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE",
                "segment.encryption.spekeKeyProvider.drmSystems",
            ),
        },
        Case {
            label: "a clear AES-128 key with CBCS",
            container: "CMAF",
            block: Block {
                method: ("CmafEncryptionMethod", "CBCS"),
                drm_systems: &["CLEAR_KEY_AES_128"],
                ..Block::default()
            },
            expected: Expected::Refused(
                "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE",
                "segment.encryption.spekeKeyProvider.drmSystems",
            ),
        },
        Case {
            label: "Widevine with TS AES-128, which carries a clear key only",
            container: "TS",
            block: Block {
                method: ("TsEncryptionMethod", "AES_128"),
                drm_systems: &["WIDEVINE"],
                ..Block::default()
            },
            expected: Expected::Refused(
                "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE",
                "segment.encryption.spekeKeyProvider.drmSystems",
            ),
        },
        Case {
            label: "a second DRM system the method cannot carry",
            container: "TS",
            block: Block {
                method: ("TsEncryptionMethod", "SAMPLE_AES"),
                drm_systems: &["FAIRPLAY", "PLAYREADY"],
                ..Block::default()
            },
            expected: Expected::Refused(
                "DRM_SYSTEMS_ENCRYPTION_METHOD_INCOMPATIBLE",
                "segment.encryption.spekeKeyProvider.drmSystems",
            ),
        },
        // Key rotation and the encryption contract against the container.
        Case {
            label: "the CMAF DRM-metadata toggle on a TS endpoint, which packages no CMAF",
            container: "TS",
            block: Block {
                method: ("TsEncryptionMethod", "AES_128"),
                drm_systems: &["CLEAR_KEY_AES_128"],
                cmaf_exclude_segment_drm_metadata: Some(true),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE",
                "segment.encryption.cmafExcludeSegmentDrmMetadata",
            ),
        },
        Case {
            label: "the CMAF DRM-metadata toggle turned OFF on an ISM endpoint",
            container: "ISM",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                presets: ("SHARED", "SHARED"),
                cmaf_exclude_segment_drm_metadata: Some(false),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE",
                "segment.encryption.cmafExcludeSegmentDrmMetadata",
            ),
        },
        Case {
            label: "key rotation on an ISM endpoint",
            container: "ISM",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                presets: ("SHARED", "SHARED"),
                key_rotation_interval_seconds: Some(300),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ISM_CONTAINER_WITH_KEY_ROTATION",
                "segment.encryption.keyRotationIntervalSeconds",
            ),
        },
        Case {
            label: "a per-track encryption contract on an ISM endpoint",
            container: "ISM",
            block: Block {
                method: ("IsmEncryptionMethod", "CENC"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE",
                "segment.encryption.spekeKeyProvider.encryptionContractConfiguration",
            ),
        },
        Case {
            label: "an encryption contract that encrypts neither audio nor video",
            container: "CMAF",
            block: Block {
                presets: ("UNENCRYPTED", "UNENCRYPTED"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_CONTRACT_UNENCRYPTED",
                "segment.encryption.spekeKeyProvider.encryptionContractConfiguration",
            ),
        },
        Case {
            label: "one shared key for the audio tracks but a preset for the video ones",
            container: "CMAF",
            block: Block {
                presets: ("SHARED", "PRESET_VIDEO_1"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_CONTRACT_SHARED",
                "segment.encryption.spekeKeyProvider.encryptionContractConfiguration",
            ),
        },
        Case {
            label: "one shared key for the video tracks but a preset for the audio ones",
            container: "CMAF",
            block: Block {
                presets: ("PRESET_AUDIO_2", "SHARED"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "ENCRYPTION_CONTRACT_SHARED",
                "segment.encryption.spekeKeyProvider.encryptionContractConfiguration",
            ),
        },
        Case {
            label: "unencrypted audio beside an encrypted video preset is a contract",
            container: "CMAF",
            block: Block {
                presets: ("UNENCRYPTED", "PRESET_VIDEO_8"),
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        // The content-key period timing against the SPEKE version and key rotation.
        Case {
            label: "the index-only timing, which needs neither",
            container: "CMAF",
            block: Block {
                content_key_period_timing: Some("INDEX_ONLY"),
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        Case {
            label: "start and end times signalled under the default SPEKE version",
            container: "CMAF",
            block: Block {
                content_key_period_timing: Some("START_END_ONLY"),
                key_rotation_interval_seconds: Some(600),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1",
                "segment.encryption.spekeKeyProvider.contentKeyPeriodConfiguration.contentKeyPeriodTiming",
            ),
        },
        Case {
            label: "the index with start and end times under SPEKE 2.0",
            container: "CMAF",
            block: Block {
                content_key_period_timing: Some("INDEX_WITH_START_END"),
                speke_version: Some("V2_0"),
                key_rotation_interval_seconds: Some(600),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CONTENT_KEY_PERIOD_TIMING_REQUIRES_SPEKE_V2_1",
                "segment.encryption.spekeKeyProvider.contentKeyPeriodConfiguration.contentKeyPeriodTiming",
            ),
        },
        Case {
            label: "start and end times under SPEKE 2.1 but without key rotation",
            container: "CMAF",
            block: Block {
                content_key_period_timing: Some("START_END_ONLY"),
                speke_version: Some("V2_1"),
                ..Block::default()
            },
            expected: Expected::Refused(
                "CONTENT_KEY_PERIOD_TIMING_WITHOUT_KEY_ROTATION",
                "segment.encryption.spekeKeyProvider.contentKeyPeriodConfiguration.contentKeyPeriodTiming",
            ),
        },
        Case {
            label: "start and end times under SPEKE 2.1 with key rotation",
            container: "CMAF",
            block: Block {
                content_key_period_timing: Some("INDEX_WITH_START_END"),
                speke_version: Some("V2_1"),
                key_rotation_interval_seconds: Some(31_536_000),
                ..Block::default()
            },
            expected: Expected::Stored,
        },
        // The key-server endpoint.
        Case {
            label: "a key-server URL that is not a URL at all",
            container: "CMAF",
            block: Block {
                url: "speke.example.com/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused("URL_INVALID", "segment.encryption.spekeKeyProvider.url"),
        },
        Case {
            label: "a key server addressed over another scheme",
            container: "CMAF",
            block: Block {
                url: "ftp://speke.example.com/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused("URL_SCHEME", "segment.encryption.spekeKeyProvider.url"),
        },
        Case {
            label: "a key-server URL carrying credentials",
            container: "CMAF",
            block: Block {
                url: "https://operator:secret@speke.example.com/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused("URL_USER_INFO", "segment.encryption.spekeKeyProvider.url"),
        },
        Case {
            label: "a key-server URL naming a port of its own",
            container: "CMAF",
            block: Block {
                url: "https://speke.example.com:8443/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused("URL_PORT", "segment.encryption.spekeKeyProvider.url"),
        },
        Case {
            label: "a key server at the loopback name",
            container: "CMAF",
            block: Block {
                url: "https://localhost/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_LOOPBACK_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server at the loopback address",
            container: "CMAF",
            block: Block {
                url: "https://127.0.0.1/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_LOOPBACK_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server at the IPv6 loopback address",
            container: "CMAF",
            block: Block {
                url: "https://[::1]/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_LOOPBACK_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server on a private network",
            container: "CMAF",
            block: Block {
                url: "https://10.0.0.5/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_LOCAL_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server at a link-local address",
            container: "CMAF",
            block: Block {
                url: "https://169.254.169.254/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_LINK_LOCAL_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server at a multicast group",
            container: "CMAF",
            block: Block {
                url: "https://239.1.2.3/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_MULTICAST_ADDRESS",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
        Case {
            label: "a key server at a single-label host, which resolves nowhere",
            container: "CMAF",
            block: Block {
                url: "https://keyserver/copyProtection",
                ..Block::default()
            },
            expected: Expected::Refused(
                "URL_UNKNOWN_HOST",
                "segment.encryption.spekeKeyProvider.url",
            ),
        },
    ]
}

/// The manifest a container type's endpoint publishes: ISM serves MSS, the others the HLS
/// families ([`crate::channel_lifecycle::minimal_endpoint`] makes the same choice).
fn endpoint_body(container: &str, segment: Value) -> Value {
    let manifest = json!([{"ManifestName": "index"}]);
    let mut body = match container {
        "ISM" => json!({"ContainerType": "ISM", "MssManifests": manifest}),
        _ => json!({"ContainerType": container, "HlsManifests": manifest}),
    };
    body["Segment"] = segment;
    body
}

/// Assert that `reported` carries every member `submitted` named, with the value it was
/// given — the stored block may carry MORE (the members the service supplies inside it,
/// pinned by `segment_service_applied_blocks`), but never a different value for one the
/// request named. Recurses into nested objects, so a value the service supplies beside a
/// submitted sub-block does not read as a changed sub-block.
fn assert_submitted_members_survive(case: &str, path: &str, submitted: &Value, reported: &Value) {
    let Some(members) = submitted.as_object() else {
        assert_eq!(
            reported, submitted,
            "case {case:?}: {path} expected {submitted}, got {reported}"
        );
        return;
    };
    for (member, expected) in members {
        assert_submitted_members_survive(
            case,
            &format!("{path}.{member}"),
            expected,
            &reported[member],
        );
    }
}

/// Assert that `body` is this service's `ValidationException` carrying `expected_type` and
/// naming `member`.
fn assert_refused(case: &str, status: u16, body: &Value, expected_type: &str, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body.get("ValidationExceptionType")
            .and_then(Value::as_str)
            .unwrap_or(""),
        expected_type,
        "case {case:?}: expected ValidationExceptionType {expected_type}, got {body}"
    );
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with(&format!("Validation error encountered at request.{member}:")),
        "case {case:?}: the refusal must name {member} in this service's message form, \
         got {body}"
    );
}

/// An emulator with the group and channel this suite's endpoints live in.
async fn emulator_with_parents() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "CreateChannelGroup expected 200, got {body}");
    let (status, body) = emulator
        .create_channel(GROUP, CHANNEL, json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(status, 200, "CreateChannel expected 200, got {body}");
    emulator
}

/// RULE, on the create — the whole encryption block is read before anything is stored: a
/// combination the service could not act on is refused with the model's own discriminator
/// and mints nothing, and one it can act on is stored and reported back with every
/// submitted member unchanged.
#[tokio::test]
async fn an_endpoints_encryption_settings_are_read_against_each_other_and_the_container() {
    let emulator = emulator_with_parents().await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let endpoint = format!("enc-ep-{index}");
        let segment = case.block.segment();
        let (status, body) = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                &endpoint,
                endpoint_body(case.container, segment.clone()),
            )
            .await;

        match case.expected {
            Expected::Refused(expected_type, member) => {
                assert_refused(label, status, &body, expected_type, member);
                let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must mint nothing, got {status} {read}"
                );
            }
            Expected::Stored => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateOriginEndpoint expected 200, got {status} {body}"
                );
                let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
                );
                assert_submitted_members_survive(
                    label,
                    "segment.encryption",
                    &segment["Encryption"],
                    &read["Segment"]["Encryption"],
                );
                assert_eq!(
                    read["Segment"]["Encryption"], body["Segment"]["Encryption"],
                    "case {label:?}: the create and the read must report one block, got \
                     {read} after {body}"
                );
            }
        }
    }
}

/// RULE, through the update — `UpdateOriginEndpoint` writes the same member, so it reads
/// the same rules: a refused replacement leaves the stored block, and the endpoint's ETag,
/// exactly as the create left them.
#[tokio::test]
async fn an_update_reads_the_encryption_settings_before_replacing_them() {
    let emulator = emulator_with_parents().await;
    let endpoint = "enc-update-ep";
    let (status, created) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            endpoint,
            endpoint_body("CMAF", Block::default().segment()),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {created}"
    );

    for case in cases() {
        let Expected::Refused(expected_type, member) = case.expected else {
            continue;
        };
        // Only the rows whose container type is the stored endpoint's can be submitted as
        // an update: `ContainerType` is immutable after creation, and that refusal is
        // `update_roundtrip`'s.
        if case.container != "CMAF" {
            continue;
        }
        let label = case.label;
        let (status, body) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                endpoint,
                endpoint_body("CMAF", case.block.segment()),
                &[],
            )
            .await;
        assert_refused(label, status, &body, expected_type, member);

        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the endpoint must still be readable, got {status} {read}"
        );
        assert_eq!(
            read["Segment"]["Encryption"], created["Segment"]["Encryption"],
            "case {label:?}: a refused update must leave the stored block as it was, got \
             {read}"
        );
        assert_eq!(
            read["ETag"], created["ETag"],
            "case {label:?}: a refused update must not rotate the entity tag, got {read}"
        );
    }
}
