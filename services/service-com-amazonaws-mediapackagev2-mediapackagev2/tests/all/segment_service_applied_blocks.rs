// SPDX-License-Identifier: CC0-1.0

//! The members MediaPackage supplies INSIDE a configured block of an origin endpoint,
//! driven end-to-end over HTTP through the same router the emulator boots and reusing the
//! restJson1 client in `channel_lifecycle`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over one recorded
//! request: a member the model documents the SERVICE as supplying inside a block of an
//! endpoint's `Segment` or of one of its manifest configurations is materialised with the
//! rest of the resource by the call that WRITES it — the submitted value when the request
//! names one, the service's own when it does not, and nothing at all where the model says
//! the endpoint does not carry the member — so the create response, every later read and
//! an update that names none of them report the same concrete value.
//!
//! The members and their documented values come from the model: `Encryption`'s
//! `ConstantInitializationVector` ("If you don't specify a value, then MediaPackage
//! creates the constant initialization vector (IV)", `@length 32..32`, `@pattern
//! ^[0-9a-fA-F]+$`) and `CmafExcludeSegmentDrmMetadata` ("Default: `false`", a member only
//! CMAF containers carry — `CMAF_EXCLUDE_SEGMENT_DRM_METADATA_INCOMPATIBLE_CONTAINER_TYPE`),
//! `SpekeKeyProvider$SpekeVersion` ("If you don't specify a value, the default is `V2_0`"),
//! `ContentKeyPeriodConfiguration$ContentKeyPeriodTiming` ("If you don't specify a value,
//! the default is `INDEX_ONLY`"), `Scte$ScteInSegments` ("None – SCTE-35 messages are not
//! included in segments (default)") and `ScteHls$ScteInManifests` / `ScteDash$ScteInManifests`
//! ("If you don't specify a value, the default is `ALL`")
//! (`smithy_query(query="shape", shape="Encryption,Scte,ScteHls,ScteDash,SpekeKeyProvider,ContentKeyPeriodConfiguration,ScteInSegments,ScteInManifests,ContentKeyPeriodTiming,SpekeVersion")`).
//!
//! Every row differs from the others along what the rule generalises over: which block
//! carries the member (segment encryption, segment SCTE, the HLS / low-latency HLS / DASH
//! manifest SCTE blocks), whether the request names the member or leaves it out, which
//! container type and encryption method the endpoint packages with (which is what decides
//! whether a constant initialization vector exists at all), and which manifest family —
//! and how many manifests — the configuration sits in.

use serde_json::{json, Value};

use crate::channel_lifecycle::{at, Emulator};

/// The parents every endpoint in this suite hangs under. A CMAF-input channel, so the rows
/// can span the container types.
const GROUP: &str = "blocks-group";
const CHANNEL: &str = "blocks-channel";

/// The key-server endpoint every row's SPEKE provider names: an `https` URL at a
/// resolvable host, so no row is refused for the URL rule.
const KEY_SERVER_URL: &str = "https://speke.example.com/copyProtection";

/// A 128-bit initialization vector as `Encryption$ConstantInitializationVector` takes it —
/// 32 hex characters — for the rows that submit one of their own.
const SUBMITTED_IV: &str = "0123456789ABCDEFFEDCBA9876543210";

/// The role the key provider names: an IAM role ARN in this account, so no row is refused
/// for the ARN rule.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/SpekeAccess",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// An encryption block for `container`, with `method` as the one `EncryptionMethod` member
/// the container configures, and `extra` merged in — the members a row names itself.
///
/// The presets and DRM systems are the ones the method and the container admit: an ISM
/// container needs the `SHARED` preset for both track groups, and each method protects
/// only the DRM systems its scheme signals.
fn encryption(container: &str, method: &str, drm_systems: Value, extra: Value) -> Value {
    let (audio, video) = if container == "ISM" {
        ("SHARED", "SHARED")
    } else {
        ("PRESET_AUDIO_1", "PRESET_VIDEO_1")
    };
    let member = match container {
        "TS" => "TsEncryptionMethod",
        "ISM" => "IsmEncryptionMethod",
        _ => "CmafEncryptionMethod",
    };
    let mut block = json!({
        "EncryptionMethod": {member: method},
        "SpekeKeyProvider": {
            "EncryptionContractConfiguration": {
                "PresetSpeke20Audio": audio,
                "PresetSpeke20Video": video,
            },
            "ResourceId": "MovieNight20171126093045",
            "DrmSystems": drm_systems,
            "RoleArn": role_arn(),
            "Url": KEY_SERVER_URL,
        },
    });
    for (member, value) in extra.as_object().expect("an object of extra members") {
        block[member] = value.clone();
    }
    block
}

/// The manifest list a container serves, under the member it is submitted at: a TS
/// container serves the HLS families, CMAF adds DASH and ISM serves MSS alone.
fn manifests(member: &str, manifests: Value) -> Value {
    let mut body = json!({});
    body[member] = manifests;
    body
}

/// One row: a create body, and the `(pointer, value)` pairs every projection of the
/// created endpoint must report — `Value::Null` for a member the endpoint must NOT carry.
struct Case {
    label: &'static str,
    body: fn() -> Value,
    expected: Vec<(&'static str, Value)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "TS AES-128 encryption with no vector, no toggle and no SPEKE version",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("TS");
                body["Segment"] = json!({
                    "Encryption": encryption("TS", "AES_128", json!(["CLEAR_KEY_AES_128"]), json!({})),
                });
                body
            },
            expected: vec![
                // The vector is created, so only its form is fixed here; the value is
                // pinned by `a_created_vector_has_the_modelled_form_and_belongs_to_one_endpoint`.
                ("/Segment/Encryption/SpekeKeyProvider/SpekeVersion", json!("V2_0")),
                // Not a CMAF container, so the CMAF-only toggle is carried by nothing.
                ("/Segment/Encryption/CmafExcludeSegmentDrmMetadata", Value::Null),
                // No SCTE block was configured, so none is invented.
                ("/Segment/Scte", Value::Null),
            ],
        },
        Case {
            label: "TS sample-AES encryption naming its own vector",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("TS");
                body["Segment"] = json!({
                    "Encryption": encryption(
                        "TS",
                        "SAMPLE_AES",
                        json!(["FAIRPLAY"]),
                        json!({"ConstantInitializationVector": SUBMITTED_IV}),
                    ),
                });
                body
            },
            expected: vec![
                (
                    "/Segment/Encryption/ConstantInitializationVector",
                    json!(SUBMITTED_IV),
                ),
                ("/Segment/Encryption/SpekeKeyProvider/SpekeVersion", json!("V2_0")),
            ],
        },
        Case {
            label: "CMAF CBCS encryption with the DRM-metadata toggle left out",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("CMAF");
                body["Segment"] = json!({
                    "Encryption": encryption("CMAF", "CBCS", json!(["FAIRPLAY", "WIDEVINE"]), json!({})),
                });
                body
            },
            expected: vec![
                (
                    "/Segment/Encryption/CmafExcludeSegmentDrmMetadata",
                    json!(false),
                ),
                ("/Segment/Encryption/SpekeKeyProvider/SpekeVersion", json!("V2_0")),
            ],
        },
        Case {
            label: "CMAF CBCS encryption asking for the toggle and naming SPEKE 2.1",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("CMAF");
                let mut block =
                    encryption("CMAF", "CBCS", json!(["WIDEVINE"]), json!({"CmafExcludeSegmentDrmMetadata": true}));
                block["SpekeKeyProvider"]["SpekeVersion"] = json!("V2_1");
                body["Segment"] = json!({"Encryption": block});
                body
            },
            expected: vec![
                (
                    "/Segment/Encryption/CmafExcludeSegmentDrmMetadata",
                    json!(true),
                ),
                ("/Segment/Encryption/SpekeKeyProvider/SpekeVersion", json!("V2_1")),
            ],
        },
        Case {
            label: "CMAF Common Encryption, which derives its own per-sample vectors",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("CMAF");
                body["Segment"] = json!({
                    "Encryption": encryption("CMAF", "CENC", json!(["PLAYREADY"]), json!({})),
                });
                body
            },
            expected: vec![
                // The method takes no constant vector at all, so the service creates none.
                (
                    "/Segment/Encryption/ConstantInitializationVector",
                    Value::Null,
                ),
                (
                    "/Segment/Encryption/CmafExcludeSegmentDrmMetadata",
                    json!(false),
                ),
            ],
        },
        Case {
            label: "ISM Common Encryption on an MSS manifest, one shared content key",
            body: || {
                let mut body = manifests("MssManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("ISM");
                body["Segment"] = json!({
                    "Encryption": encryption("ISM", "CENC", json!(["PLAYREADY"]), json!({})),
                });
                body
            },
            expected: vec![
                (
                    "/Segment/Encryption/ConstantInitializationVector",
                    Value::Null,
                ),
                (
                    "/Segment/Encryption/CmafExcludeSegmentDrmMetadata",
                    Value::Null,
                ),
                ("/Segment/Encryption/SpekeKeyProvider/SpekeVersion", json!("V2_0")),
            ],
        },
        Case {
            label: "a configured content-key period configuration naming no timing",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("CMAF");
                let mut block = encryption("CMAF", "CBCS", json!(["WIDEVINE"]), json!({}));
                block["SpekeKeyProvider"]["ContentKeyPeriodConfiguration"] = json!({});
                body["Segment"] = json!({"Encryption": block});
                body
            },
            expected: vec![(
                "/Segment/Encryption/SpekeKeyProvider/ContentKeyPeriodConfiguration/ContentKeyPeriodTiming",
                json!("INDEX_ONLY"),
            )],
        },
        Case {
            label: "a segment SCTE block with a filter but no segment choice",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("TS");
                body["Segment"] = json!({"Scte": {"ScteFilter": ["BREAK"]}});
                body
            },
            expected: vec![
                ("/Segment/Scte/ScteInSegments", json!("NONE")),
                ("/Segment/Scte/ScteFilter/0", json!("BREAK")),
                // No value is documented for the custom ad types, so none is invented.
                ("/Segment/Scte/CustomAdTypes", Value::Null),
                // And no encryption block was configured.
                ("/Segment/Encryption", Value::Null),
            ],
        },
        Case {
            label: "a segment SCTE block naming the segment choice itself",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("TS");
                body["Segment"] = json!({
                    "Scte": {
                        "ScteFilter": ["SPLICE_INSERT", "PROVIDER_ADVERTISEMENT"],
                        "ScteInSegments": "MATCHES_FILTER",
                    },
                });
                body
            },
            expected: vec![("/Segment/Scte/ScteInSegments", json!("MATCHES_FILTER"))],
        },
        Case {
            label: "an empty segment SCTE block, present with no member set",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["ContainerType"] = json!("TS");
                body["Segment"] = json!({"Scte": {}});
                body
            },
            expected: vec![("/Segment/Scte/ScteInSegments", json!("NONE"))],
        },
        Case {
            label: "two HLS manifests, one SCTE block naming no manifest choice",
            body: || {
                let mut body = manifests(
                    "HlsManifests",
                    json!([
                        {"ManifestName": "index", "ScteHls": {"AdMarkerHls": "DATERANGE"}},
                        {"ManifestName": "second"},
                    ]),
                );
                body["ContainerType"] = json!("TS");
                body
            },
            expected: vec![
                ("/HlsManifests/0/ScteHls/ScteInManifests", json!("ALL")),
                ("/HlsManifests/0/ScteHls/AdMarkerHls", json!("DATERANGE")),
                // The second manifest configured no block, so it carries none.
                ("/HlsManifests/1/ScteHls", Value::Null),
            ],
        },
        Case {
            label: "a low-latency HLS manifest naming the manifest choice itself",
            body: || {
                let mut body = manifests(
                    "LowLatencyHlsManifests",
                    json!([{
                        "ManifestName": "ll",
                        "ScteHls": {"ScteInManifests": "MATCHES_FILTER"},
                    }]),
                );
                body["ContainerType"] = json!("TS");
                body
            },
            expected: vec![(
                "/LowLatencyHlsManifests/0/ScteHls/ScteInManifests",
                json!("MATCHES_FILTER"),
            )],
        },
        Case {
            label: "a DASH manifest with an empty SCTE block beside an HLS manifest",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["DashManifests"] = json!([{"ManifestName": "dash", "ScteDash": {}}]);
                body["ContainerType"] = json!("CMAF");
                body
            },
            expected: vec![
                ("/DashManifests/0/ScteDash/ScteInManifests", json!("ALL")),
                ("/HlsManifests/0/ScteHls", Value::Null),
            ],
        },
        Case {
            label: "a DASH manifest naming the ad marker and the manifest choice",
            body: || {
                let mut body = manifests("HlsManifests", json!([{"ManifestName": "index"}]));
                body["DashManifests"] = json!([{
                    "ManifestName": "dash",
                    "ScteDash": {"AdMarkerDash": "XML", "ScteInManifests": "MATCHES_FILTER"},
                }]);
                body["ContainerType"] = json!("CMAF");
                body
            },
            expected: vec![
                (
                    "/DashManifests/0/ScteDash/ScteInManifests",
                    json!("MATCHES_FILTER"),
                ),
                ("/DashManifests/0/ScteDash/AdMarkerDash", json!("XML")),
            ],
        },
    ]
}

/// A channel group and a CMAF-input channel for the endpoints below.
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

/// RULE — every projection of an endpoint reports the members the service supplies inside
/// a configured block: the create response, a later read, and an update that re-submits
/// the same configuration while naming none of those members.
#[tokio::test]
async fn an_endpoint_reports_the_members_the_service_supplies_inside_its_blocks() {
    let emulator = emulator_with_parents().await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let endpoint = format!("blocks-ep-{index}");
        let body = (case.body)();

        let (status, created) = emulator
            .create_endpoint(GROUP, CHANNEL, &endpoint, body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );
        // The same body again, which names none of the service-applied members it did not
        // name the first time: the update writes the whole configuration, so it has to
        // materialise them again rather than report nothing.
        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, &endpoint, body, &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );

        for (pointer, expected) in &case.expected {
            for (projection, answer) in [
                ("CreateOriginEndpoint", &created),
                ("GetOriginEndpoint", &read),
                ("UpdateOriginEndpoint", &updated),
            ] {
                let actual = at(answer, pointer);
                assert_eq!(
                    &actual, expected,
                    "case {label:?}: {projection} {pointer} expected {expected}, got \
                     {actual}"
                );
            }
        }
    }
}

/// RULE, the created value's own form — the vector the service creates for an endpoint
/// whose request named none is a 32-character hex string (`ConstantInitializationVector`
/// is "a 128-bit, 16-byte hex value represented by a 32-character string", `@length
/// 32..32`, `@pattern ^[0-9a-fA-F]+$`), it belongs to the endpoint it was created for —
/// two endpoints under one channel never share one — and it does not move: a read and an
/// update that names no vector report the one the create did.
#[tokio::test]
async fn a_created_vector_has_the_modelled_form_and_belongs_to_one_endpoint() {
    let emulator = emulator_with_parents().await;

    let mut vectors: Vec<(String, String)> = Vec::new();
    // Two endpoints, each with a method that TAKES a constant vector, so each is handed
    // one of its own.
    for (endpoint, method, drm) in [
        ("iv-ts-ep", "AES_128", json!(["CLEAR_KEY_AES_128"])),
        ("iv-ts-other-ep", "SAMPLE_AES", json!(["FAIRPLAY"])),
    ] {
        let body = json!({
            "ContainerType": "TS",
            "HlsManifests": [{"ManifestName": "index"}],
            "Segment": {"Encryption": encryption("TS", method, drm, json!({}))},
        });
        let (status, created) = emulator
            .create_endpoint(GROUP, CHANNEL, endpoint, body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {endpoint:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let vector = at(&created, "/Segment/Encryption/ConstantInitializationVector");
        let vector = vector
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {endpoint:?}: the service must create a vector, got {created}")
            })
            .to_string();
        assert_eq!(
            vector.len(),
            32,
            "case {endpoint:?}: a vector is a 32-character string, got {vector:?}"
        );
        assert!(
            vector.chars().all(|character| character.is_ascii_hexdigit()),
            "case {endpoint:?}: a vector is hex, got {vector:?}"
        );

        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, endpoint).await;
        assert_eq!(
            status, 200,
            "case {endpoint:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );
        let on_read = at(&read, "/Segment/Encryption/ConstantInitializationVector");
        assert_eq!(
            on_read,
            json!(vector),
            "case {endpoint:?}: a read must report the created vector, got {on_read}"
        );
        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, endpoint, body, &[])
            .await;
        assert_eq!(
            status, 200,
            "case {endpoint:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        let on_update = at(&updated, "/Segment/Encryption/ConstantInitializationVector");
        assert_eq!(
            on_update,
            json!(vector),
            "case {endpoint:?}: an update naming no vector must report the created one, \
             got {on_update}"
        );
        vectors.push((endpoint.to_string(), vector));
    }

    let (first, second) = (&vectors[0], &vectors[1]);
    assert_ne!(
        first.1, second.1,
        "case \"two endpoints\": {} and {} must not share a vector, both got {:?}",
        first.0, second.0, first.1
    );
}

/// RULE, the write side — a materialised member is the SUBMITTED value whenever a write
/// names one, so an update that names the members the create left to the service reports
/// the new values; and because the service's own values are a function of the endpoint
/// rather than of the instant the write happened, an update that re-submits exactly what
/// was stored changes nothing, so the entity tag the caller holds still stands.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_re_submitting_commits_nothing() {
    let emulator = emulator_with_parents().await;
    let endpoint = "rewritten-ep";
    let submitted = json!({
        "ContainerType": "TS",
        "HlsManifests": [{"ManifestName": "index", "ScteHls": {}}],
        "Segment": {
            "Scte": {},
            "Encryption": encryption("TS", "AES_128", json!(["CLEAR_KEY_AES_128"]), json!({})),
        },
    });

    let (status, created) = emulator
        .create_endpoint(GROUP, CHANNEL, endpoint, submitted.clone())
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {created}"
    );

    // The same configuration again: every service-applied member materialises to what is
    // already stored, so nothing was committed and the tag did not rotate.
    let (status, resubmitted) = emulator
        .update_endpoint(GROUP, CHANNEL, endpoint, submitted, &[])
        .await;
    assert_eq!(
        status, 200,
        "UpdateOriginEndpoint expected 200, got {status} {resubmitted}"
    );
    assert_eq!(
        resubmitted["ETag"], created["ETag"],
        "case \"re-submitting what was stored\": the entity tag must not rotate, got {} \
         after {}",
        resubmitted["ETag"], created["ETag"]
    );

    // Now a write that NAMES each of those members itself.
    let named = json!({
        "ContainerType": "TS",
        "HlsManifests": [
            {"ManifestName": "index", "ScteHls": {"ScteInManifests": "MATCHES_FILTER"}},
        ],
        "Segment": {
            "Scte": {"ScteInSegments": "ALL"},
            "Encryption": encryption(
                "TS",
                "AES_128",
                json!(["CLEAR_KEY_AES_128"]),
                json!({"ConstantInitializationVector": SUBMITTED_IV}),
            ),
        },
    });
    let (status, updated) = emulator
        .update_endpoint(GROUP, CHANNEL, endpoint, named, &[])
        .await;
    assert_eq!(
        status, 200,
        "UpdateOriginEndpoint expected 200, got {status} {updated}"
    );
    let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, endpoint).await;
    assert_eq!(
        status, 200,
        "GetOriginEndpoint expected 200, got {status} {read}"
    );
    for (pointer, expected) in [
        ("/Segment/Scte/ScteInSegments", json!("ALL")),
        (
            "/Segment/Encryption/ConstantInitializationVector",
            json!(SUBMITTED_IV),
        ),
        (
            "/HlsManifests/0/ScteHls/ScteInManifests",
            json!("MATCHES_FILTER"),
        ),
    ] {
        for (projection, answer) in [
            ("UpdateOriginEndpoint", &updated),
            ("GetOriginEndpoint", &read),
        ] {
            let actual = at(answer, pointer);
            assert_eq!(
                actual, expected,
                "case \"an update naming the members\": {projection} {pointer} expected \
                 {expected}, got {actual}"
            );
        }
    }
    assert_ne!(
        read["ETag"], created["ETag"],
        "case \"an update naming the members\": a write that changed the endpoint must \
         rotate the entity tag, got {}",
        read["ETag"]
    );
}
