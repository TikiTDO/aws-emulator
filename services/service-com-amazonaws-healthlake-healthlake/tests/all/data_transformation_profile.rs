// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the data transformation profile slice, over the real
//! awsJson1_0 wire: `CreateDataTransformationProfile` and
//! `GetDataTransformationProfile`.
//!
//! The rule under test: a created profile is PERSISTED as its DRAFT (version 0)
//! carrying exactly the content its `Source` resolves to, and
//! `GetDataTransformationProfile` reads that same content and metadata back —
//! whatever the source format, however many mapping entries the content has and
//! whichever optional members the create carried. A version the profile does not
//! have is a 404, and content the modelled `ProfileMapping` could not carry is
//! refused rather than stored.
//!
//! Every call below goes through [`MultiServiceRouter::dispatch`] — the same entry
//! point `core/router` serves — so routing, deserialization, the modelled
//! constraints and the error envelope are all exercised, not bypassed.

use std::collections::BTreeMap;
use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_not_found_message, profile_version_not_found_message, ACTIVE_PROFILE_STATUS,
};

use crate::fhir_datastore_lifecycle::{error_code, string_at, HealthLake};

/// The success status this service answers `CreateDataTransformationProfile` with.
///
/// HealthLake speaks awsJson1_0, which binds nothing to the transport, so the
/// operation's `@http(code: 201)` is inert on the wire
/// (`smithy_query(query="operation",operation="CreateDataTransformationProfile")` ->
/// `httpBindingsInert`) and a success is the protocol's 200.
const CREATE_SUCCESS_STATUS: u16 = 200;

/// The shared router harness plus the case-labelled assertions this suite's tables
/// need: every row names itself in the failure message.
///
/// `pub(crate)` so the sibling profile suites in `tests/all/` drive the slice
/// through this one harness rather than each writing its own.
pub(crate) struct Profiles {
    healthlake: HealthLake,
}

impl Profiles {
    pub(crate) fn new() -> Self {
        Self {
            healthlake: HealthLake::new(),
        }
    }

    /// Dispatch one awsJson1_0 call, returning the HTTP status and the parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.healthlake.call(operation, body).await
    }

    /// Dispatch a call that must succeed, returning its body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: {operation} must succeed, got {status}: {payload}"
        );
        payload
    }

    /// Dispatch a call that must be refused, returning `(status, error code)`.
    pub(crate) async fn refused(&self, case: &str, operation: &str, body: &Value) -> (u16, String) {
        let (status, code, _) = self.refused_body(case, operation, body).await;
        (status, code)
    }

    /// Dispatch a call that must be refused, returning `(status, error code, body)`
    /// for a case that also asserts on the refusal's modelled `Message`.
    pub(crate) async fn refused_body(
        &self,
        case: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, String, Value) {
        let (status, payload) = self.call(operation, body).await;
        assert!(
            status >= 400,
            "case {case:?}: {operation} must be refused, got {status}: {payload}"
        );
        (status, error_code(&payload), payload)
    }
}

/// The modelled `Message` member of an error body, under that exact key — the key
/// this service's own error shapes declare, not the framework's lowercase one.
pub(crate) fn modelled_message(payload: &Value) -> Option<&str> {
    payload.get("Message").and_then(Value::as_str)
}

/// The sentence this service refuses `operation` with when the profile `request`
/// addresses is one no record holds.
///
/// The rule the profile surface's not-found texts follow: a call that resolves a
/// particular VERSION of the profile is refused for that version — a read that NAMED
/// one, an update (which edits the DRAFT, version 0) and a publish (which copies the
/// version it is told to, the DRAFT by default) — while a call that resolves the
/// PROFILE itself is refused for the profile. Either way the sentence names what the
/// call could not find, taken from the request.
///
/// `pub(crate)` so every profile suite asserts the one rule rather than restating it.
pub(crate) fn expected_profile_not_found(operation: &str, request: &Value) -> String {
    let profile_id = addressed_profile(request);
    match operation {
        "GetDataTransformationProfile" => {
            match request.get("ProfileVersion").and_then(Value::as_i64) {
                Some(version) => profile_version_not_found_message(&profile_id, version as i32),
                None => profile_not_found_message(&profile_id),
            }
        }
        "UpdateDataTransformationProfile" => profile_version_not_found_message(
            &profile_id,
            service_com_amazonaws_healthlake_healthlake::state::DRAFT_PROFILE_VERSION,
        ),
        "PublishDataTransformationProfile" => profile_version_not_found_message(
            &profile_id,
            request
                .get("FromExistingVersion")
                .and_then(Value::as_i64)
                .unwrap_or(i64::from(
                    service_com_amazonaws_healthlake_healthlake::state::DRAFT_PROFILE_VERSION,
                )) as i32,
        ),
        _ => profile_not_found_message(&profile_id),
    }
}

/// The profile a request addresses: its `ProfileId`, or the id segment of the
/// `ResourceARN` a tagging call names it by.
fn addressed_profile(request: &Value) -> String {
    if let Some(profile_id) = request.get("ProfileId").and_then(Value::as_str) {
        return profile_id.to_string();
    }
    let arn = request
        .get("ResourceARN")
        .and_then(Value::as_str)
        .unwrap_or_default();
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A `Source` union naming raw profile content.
pub(crate) fn profile_mapping_source(mapping: &BTreeMap<String, String>) -> Value {
    let content: Map<String, Value> = mapping
        .iter()
        .map(|(path, content)| (path.clone(), Value::String(content.clone())))
        .collect();
    json!({ "ProfileMapping": { "ProfileMapping": Value::Object(content) } })
}

/// A mapping built from `(path, content)` pairs.
pub(crate) fn mapping(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
        .collect()
}

/// A mapping of `count` distinct entries, for the entry-count bounds.
pub(crate) fn mapping_of(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| {
            (
                format!("mappings/section-{index:04}.map"),
                format!("Observation.component[{index}] <- column_{index}"),
            )
        })
        .collect()
}

/// A mapping just past `bytes` bytes of keys and values together, built from entries
/// each INSIDE its own modelled bound (a 100 000-character value is under the
/// modelled 102 400 maximum), for the retained-byte ceiling.
pub(crate) fn mapping_of_bytes(bytes: usize) -> BTreeMap<String, String> {
    let content = "v".repeat(100_000);
    let entries = bytes / content.len() + 1;
    (0..entries)
        .map(|index| (format!("mappings/bulk-{index:04}.map"), content.clone()))
        .collect()
}

/// A mapping whose keys and values hold EXACTLY `bytes` bytes together — the
/// exact-size companion of [`mapping_of_bytes`], for a row that has to sit ON a
/// byte ceiling rather than past it.
///
/// Every entry stays inside its own modelled bound (`smithy_query(query="shape",
/// shape="ProfileMapping")`: 1..500 entries, keys `ProfileMappingKey` 1..500
/// characters, values `ProfileMappingValue` 0..102400), so this is content the
/// modelled shape carries.
pub(crate) fn mapping_of_exactly(bytes: usize) -> BTreeMap<String, String> {
    // Under the modelled 102 400-character maximum for one value.
    const VALUE_LEN: usize = 100_000;
    let mut content: BTreeMap<String, String> = BTreeMap::new();
    let mut remaining = bytes;
    let mut index = 0usize;
    while remaining > 0 {
        let key = format!("mappings/bulk-{index:04}.map");
        assert!(
            remaining >= key.len(),
            "a mapping of {bytes} bytes must divide into entries whose keys fit, {remaining} \
             bytes left over"
        );
        let value_len = (remaining - key.len()).min(VALUE_LEN);
        remaining -= key.len() + value_len;
        content.insert(key, "v".repeat(value_len));
        index += 1;
    }
    content
}

/// The `ProfileMapping` a response reports, as a comparable map.
pub(crate) fn reported_mapping(case: &str, body: &Value) -> BTreeMap<String, String> {
    body["ProfileMapping"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case:?}: response must carry a ProfileMapping object: {body}")
        })
        .iter()
        .map(|(path, content)| {
            (
                path.clone(),
                content
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every ProfileMapping value is a string: {body}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// `ProfileId` values satisfy the modelled `ProfileIdString` pattern
/// `^[a-f0-9]{32}$`.
pub(crate) fn assert_well_formed_profile_id(case: &str, id: &str) {
    assert_eq!(
        id.len(),
        32,
        "case {case:?}: ProfileId must be 32 characters, got {id:?}"
    );
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "case {case:?}: ProfileId must be lowercase hexadecimal, got {id:?}"
    );
}

/// One create-and-read-back row: the create request and the DRAFT content it must
/// leave behind.
struct RoundTrip {
    case: &'static str,
    request: Value,
    expected_mapping: BTreeMap<String, String>,
}

/// The round-trip table. The rule generalises over the source format, the number
/// and form of the mapping entries, and which optional members the create carries,
/// so the rows differ along exactly those: the modelled `ProfileMapping` entry
/// bounds (1..500 entries, keys 1..500 characters, values 0..102400) are each
/// taken at their published edge, and the optional members (`ProfileDescription`,
/// `KmsKeyId`, `Tags`) run from none present to all present.
fn round_trip_cases() -> Vec<RoundTrip> {
    let long_key = "p".repeat(500);
    vec![
        RoundTrip {
            case: "CSV, one mapping entry, no optional members",
            request: json!({
                "ProfileName": "csv-minimal",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping(&[(
                    "patients.csv.map",
                    "Patient.id <- patient_id",
                )])),
            }),
            expected_mapping: mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
        },
        RoundTrip {
            case: "CCDA, several entries, every optional member present",
            request: json!({
                "ProfileName": "ccda-full",
                "ProfileDescription": "C-CDA sections to FHIR R4 — ünïcode and punctuation: {}/[]",
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[
                    ("sections/problems.map", "Condition.code <- problem/value"),
                    ("sections/medications.map", "MedicationStatement <- substanceAdministration"),
                    ("sections/vitals.map", "Observation.value[x] <- vital/value"),
                ])),
                "Tags": { "team": "clinical-data", "env": "test" },
            }),
            expected_mapping: mapping(&[
                ("sections/problems.map", "Condition.code <- problem/value"),
                (
                    "sections/medications.map",
                    "MedicationStatement <- substanceAdministration",
                ),
                ("sections/vitals.map", "Observation.value[x] <- vital/value"),
            ]),
        },
        RoundTrip {
            case: "CSV, mapping at the modelled 500-entry maximum",
            request: json!({
                "ProfileName": "csv-at-entry-maximum",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping_of(500)),
            }),
            expected_mapping: mapping_of(500),
        },
        RoundTrip {
            case: "CCDA, key at the modelled 500-character maximum and an empty value",
            request: json!({
                "ProfileName": "ccda-at-key-maximum",
                "ProfileDescription": "",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[(long_key.as_str(), "")])),
            }),
            expected_mapping: mapping(&[(long_key.as_str(), "")]),
        },
    ]
}

/// Every row's create persists a DRAFT that `GetDataTransformationProfile` reports
/// back unchanged, and a profile with only a DRAFT has no published version.
#[tokio::test]
async fn create_persists_a_draft_every_get_reads_back() {
    let healthlake = Profiles::new();
    let mut minted: Vec<String> = Vec::new();

    for RoundTrip {
        case,
        request,
        expected_mapping,
    } in round_trip_cases()
    {
        let (status, created) = healthlake
            .call("CreateDataTransformationProfile", &request)
            .await;
        assert_eq!(
            status, CREATE_SUCCESS_STATUS,
            "case {case:?}: CreateDataTransformationProfile answers its modelled success status, got {created}"
        );

        let profile_id = string_at(&created, "ProfileId");
        assert_well_formed_profile_id(case, &profile_id);
        assert_eq!(
            created["Version"].as_i64(),
            Some(0),
            "case {case:?}: a created profile is the DRAFT, version 0, got {created}"
        );
        assert_eq!(
            created["SourceFormat"], request["SourceFormat"],
            "case {case:?}: the created profile reports the requested SourceFormat, got {created}"
        );
        assert_eq!(
            created["TargetFormat"].as_str(),
            Some("FHIR_R4"),
            "case {case:?}: the only modelled TargetFormat is FHIR_R4, got {created}"
        );
        assert_eq!(
            created["ProfileName"], request["ProfileName"],
            "case {case:?}: the created profile reports the requested ProfileName, got {created}"
        );
        assert!(
            created["LastUpdatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: LastUpdatedAt is the creation instant, got {created}"
        );
        assert_eq!(
            created["ProfileStatus"].as_str(),
            Some(ACTIVE_PROFILE_STATUS),
            "case {case:?}: the create reports the new profile's status, got {created}"
        );
        assert!(
            !minted.contains(&profile_id),
            "case {case:?}: every create mints a distinct ProfileId, got {profile_id:?} twice"
        );
        minted.push(profile_id.clone());

        // The DRAFT the create stored, read back through the modelled reader.
        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &draft),
            expected_mapping,
            "case {case:?}: the DRAFT carries exactly the content the Source resolved to"
        );
        assert_eq!(
            draft["Version"].as_i64(),
            Some(0),
            "case {case:?}: version 0 is the DRAFT, got {draft}"
        );
        assert_eq!(
            draft["ProfileId"].as_str(),
            Some(profile_id.as_str()),
            "case {case:?}: the reader reports the profile it was asked for, got {draft}"
        );
        assert_eq!(
            draft["ProfileName"], request["ProfileName"],
            "case {case:?}: the stored ProfileName is reported back, got {draft}"
        );
        assert_eq!(
            draft["ProfileDescription"],
            request
                .get("ProfileDescription")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the stored ProfileDescription is reported back, got {draft}"
        );
        assert_eq!(
            draft["SourceFormat"], request["SourceFormat"],
            "case {case:?}: the stored SourceFormat is reported back, got {draft}"
        );
        assert_eq!(
            draft["TargetFormat"].as_str(),
            Some("FHIR_R4"),
            "case {case:?}: the stored TargetFormat is reported back, got {draft}"
        );
        assert!(
            draft["ChangeDescription"].is_null(),
            "case {case:?}: a created DRAFT has no change to describe, got {draft}"
        );

        // A profile that has never been published is READ BACK at its DRAFT when the
        // request names no version: the DRAFT is what the create wrote, so the
        // authoring loop create -> get serves the content the caller just stored.
        let omitted = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        assert_eq!(
            omitted, draft,
            "case {case:?}: a read naming no version serves the DRAFT of an unpublished \
             profile, got {omitted}"
        );

        // A version the profile does not hold is refused by number, naming the
        // profile and that version. The status is the one this service puts a CLIENT
        // fault on the awsJson wire with, derived from the shape's modelled
        // `@httpError(404)` rather than hand-copied.
        let (status, code, payload) = healthlake
            .refused_body(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 1 }),
            )
            .await;
        assert_eq!(
            modelled_message(&payload),
            Some(profile_version_not_found_message(&profile_id, 1).as_str()),
            "case {case:?}: the refusal names the profile and the version, got {payload}"
        );
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(404), "ResourceNotFoundException"),
            "case {case:?}: an unpublished profile holds no version 1"
        );
    }
}

/// `ExistingVersionedProfileId` starts the new profile from the content of a
/// stored version — the same content, under a new identifier.
#[tokio::test]
async fn create_from_an_existing_version_clones_its_content() {
    let healthlake = Profiles::new();
    let case = "clone the DRAFT of a stored profile";
    let source_mapping = mapping(&[
        ("sections/allergies.map", "AllergyIntolerance <- allergy"),
        ("sections/results.map", "Observation <- result"),
    ]);

    let origin = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "clone-origin",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&source_mapping),
            }),
        )
        .await;
    let origin_id = string_at(&origin, "ProfileId");

    let clone = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "clone-target",
                "SourceFormat": "CCDA",
                "Source": { "ExistingVersionedProfileId": {
                    "ProfileId": origin_id,
                    "Version": 0,
                } },
            }),
        )
        .await;
    let clone_id = string_at(&clone, "ProfileId");
    assert_ne!(
        clone_id, origin_id,
        "case {case:?}: a clone is a new profile with its own ProfileId"
    );

    let cloned_draft = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": clone_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        reported_mapping(case, &cloned_draft),
        source_mapping,
        "case {case:?}: the clone's DRAFT carries the cloned version's content"
    );
    assert_eq!(
        cloned_draft["ProfileName"].as_str(),
        Some("clone-target"),
        "case {case:?}: the clone keeps its own name, got {cloned_draft}"
    );

    // The origin is untouched by the clone: its own content still reads back.
    let origin_draft = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": origin_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        reported_mapping(case, &origin_draft),
        source_mapping,
        "case {case:?}: cloning a version does not change the version it read"
    );
}

/// The create requests this service refuses, and what each is refused with.
fn refusal_cases() -> Vec<(&'static str, Value, u16, &'static str)> {
    let past_entry_maximum = mapping_of(501);
    vec![
        (
            "empty mapping, below the modelled 1-entry minimum",
            json!({
                "ProfileName": "below-entry-minimum",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&BTreeMap::new()),
            }),
            400,
            "ValidationException",
        ),
        (
            "mapping one entry past the modelled 500-entry maximum",
            json!({
                "ProfileName": "past-entry-maximum",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&past_entry_maximum),
            }),
            400,
            "ValidationException",
        ),
        (
            "mapping key one character past the modelled 500-character maximum",
            json!({
                "ProfileName": "past-key-maximum",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("k".repeat(501).as_str(), "x")])),
            }),
            400,
            "ValidationException",
        ),
        (
            "mapping value one character past the modelled 102400-character maximum",
            json!({
                "ProfileName": "past-value-maximum",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[(
                    "sections/huge.map",
                    "v".repeat(102_401).as_str(),
                )])),
            }),
            400,
            "ValidationException",
        ),
        (
            // Every entry below is inside its own modelled bound; together they are
            // past the retained-byte ceiling this emulator holds for one mapping
            // (`state::MAX_PROFILE_MAPPING_BYTES`, 1 MiB).
            "mapping whose entries together pass the retained-byte ceiling",
            json!({
                "ProfileName": "past-mapping-bytes",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping_of_bytes(1 << 20)),
            }),
            400,
            "ValidationException",
        ),
        (
            "StarterProfile with a source format other than its documented CCDA",
            json!({
                "ProfileName": "starter-wrong-format",
                "SourceFormat": "CSV",
                "Source": { "StarterProfile": { "StarterProfileName": "ccda-ccd" } },
            }),
            400,
            "ValidationException",
        ),
        (
            "SampleData with a source format other than its documented CSV",
            json!({
                "ProfileName": "sample-wrong-format",
                "SourceFormat": "CCDA",
                "Source": { "SampleData": { "S3Uri": "s3://sample-bucket/patients.csv" } },
            }),
            400,
            "ValidationException",
        ),
        (
            "ExistingVersionedProfileId whose ProfileId is not 32 hexadecimal characters",
            json!({
                "ProfileName": "clone-malformed-id",
                "SourceFormat": "CSV",
                "Source": { "ExistingVersionedProfileId": {
                    "ProfileId": "not-a-profile-id",
                    "Version": 0,
                } },
            }),
            400,
            "ValidationException",
        ),
        (
            "ExistingVersionedProfileId naming a well-formed id no profile has",
            json!({
                "ProfileName": "clone-unknown-id",
                "SourceFormat": "CSV",
                "Source": { "ExistingVersionedProfileId": {
                    "ProfileId": "0123456789abcdef0123456789abcdef",
                    "Version": 0,
                } },
            }),
            wire_http_status(404),
            "ResourceNotFoundException",
        ),
        (
            // The starter profile library this service holds
            // (`state::STARTER_PROFILES`) publishes no entry under this name, and a
            // name it does not hold resolves to no content — see
            // `tests/all/profile_source_variants.rs` for the names it does hold and
            // the content each resolves to.
            "StarterProfile naming an entry the built-in library does not hold",
            json!({
                "ProfileName": "starter-ccda",
                "SourceFormat": "CCDA",
                "Source": { "StarterProfile": { "StarterProfileName": "ccda-ccd" } },
            }),
            400,
            "ValidationException",
        ),
        (
            // No Amazon S3 is mounted beside this service in THIS suite's harness,
            // so nothing answers the read of the sample data file and no mapping can
            // be inferred from it. The suite that stages a file through Amazon S3's
            // own `PutObject` and reads the inferred DRAFT back is
            // `tests/all/profile_source_variants.rs`.
            "SampleData naming a location no mapping can be read from",
            json!({
                "ProfileName": "sample-csv",
                "SourceFormat": "CSV",
                "Source": { "SampleData": { "S3Uri": "s3://sample-bucket/patients.csv" } },
            }),
            400,
            "ValidationException",
        ),
    ]
}

/// Every refused create is refused with the modelled status and code — and
/// persists nothing, so no profile is left behind for a reader to find.
#[tokio::test]
async fn refused_creates_persist_nothing() {
    let healthlake = Profiles::new();
    for (case, request, expected_status, expected_code) in refusal_cases() {
        let (status, code, payload) = healthlake
            .refused_body(case, "CreateDataTransformationProfile", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {payload}"
        );
        // A clone that names no stored profile is told the PROFILE is missing, not
        // its version: the two lookups answer with their own sentence, each naming
        // what it could not find.
        if expected_code == "ResourceNotFoundException" {
            let named = string_at(&request["Source"]["ExistingVersionedProfileId"], "ProfileId");
            assert_eq!(
                modelled_message(&payload),
                Some(profile_not_found_message(&named).as_str()),
                "case {case:?}: a clone of no stored profile names the profile it could not \
                 find, got {payload}"
            );
        }
    }

    // A clone of a profile that EXISTS, at a version it does not have, is the other
    // half of that distinction: the version's own sentence, not the profile's.
    let case = "a clone of a stored profile at a version it does not have";
    let origin = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "clone-origin-draft-only",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let origin_id = string_at(&origin, "ProfileId");
    let (status, code, payload) = healthlake
        .refused_body(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "clone-unpublished-version",
                "SourceFormat": "CSV",
                "Source": { "ExistingVersionedProfileId": {
                    "ProfileId": origin_id,
                    "Version": 7,
                } },
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str(), modelled_message(&payload)),
        (
            wire_http_status(404),
            "ResourceNotFoundException",
            Some(profile_version_not_found_message(&origin_id, 7).as_str())
        ),
        "case {case:?}: the version, not the profile, is what is missing"
    );

    // A create that mints a profile still works after every refusal above, and the
    // refusals left nothing behind: the one profile that was created is the only
    // one a reader can find at the id it was given.
    let case = "a create after the refusals";
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "after-refusals",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");
    let draft = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        reported_mapping(case, &draft),
        mapping(&[("a.map", "Patient.id <- id")]),
        "case {case:?}: the surviving profile is the one that was created"
    );
}

/// A version a profile does not have is a 404, at every version number the model
/// admits; one past the modelled range is refused at the boundary.
#[tokio::test]
async fn reading_a_version_a_profile_does_not_have_is_not_found() {
    let healthlake = Profiles::new();
    let case = "versions of a profile that has only its DRAFT";
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "draft-only",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    // The modelled `ProfileVersion` range is 0..99: 0 is the DRAFT this profile
    // has, every published version number above it is one it does not.
    for version in [1, 2, 50, 99] {
        let (status, code, payload) = healthlake
            .refused_body(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
            )
            .await;
        assert_eq!(
            modelled_message(&payload),
            Some(profile_version_not_found_message(&profile_id, version).as_str()),
            "case {case:?}: version {version} is refused naming the profile and that version"
        );
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(404), "ResourceNotFoundException"),
            "case {case:?}: version {version} was never published"
        );
    }

    // One past the published range: refused before the handler sees it.
    let (status, _) = healthlake
        .refused(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 100 }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: ProfileVersion 100 is outside the modelled 0..99 range"
    );

    // A well-formed id no profile has: a read that NAMED a version is refused for
    // that version of it, and one that named none for the profile itself.
    for request in [
        json!({ "ProfileId": "ffffffffffffffffffffffffffffffff", "ProfileVersion": 0 }),
        json!({ "ProfileId": "ffffffffffffffffffffffffffffffff" }),
    ] {
        let (status, code, payload) = healthlake
            .refused_body(
                "a well-formed ProfileId no profile has",
                "GetDataTransformationProfile",
                &request,
            )
            .await;
        let expected = expected_profile_not_found("GetDataTransformationProfile", &request);
        assert_eq!(
            modelled_message(&payload),
            Some(expected.as_str()),
            "an unknown ProfileId is refused with a sentence naming it, got {payload}"
        );
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(404), "ResourceNotFoundException"),
            "an unknown ProfileId is not found"
        );
    }
}

/// The idempotency table: the rule must hold whatever the request carries, so the
/// rows differ along the number of mapping entries, along the tags and along which
/// optional members are present.
fn idempotency_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one mapping entry, no optional members",
            json!({
                "ProfileName": "idempotent-one-entry",
                "SourceFormat": "CSV",
                "ClientToken": "token-0001",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        ),
        (
            "three mapping entries",
            json!({
                "ProfileName": "idempotent-three-entries",
                "SourceFormat": "CCDA",
                "ClientToken": "token-0002",
                "Source": profile_mapping_source(&mapping(&[
                    ("sections/problems.map", "Condition.code <- problem/value"),
                    ("sections/vitals.map", "Observation.value[x] <- vital/value"),
                    ("sections/results.map", "Observation <- result"),
                ])),
            }),
        ),
        (
            "many mapping entries, several tags and every optional member",
            json!({
                "ProfileName": "idempotent-many-entries",
                "ProfileDescription": "many entries, many tags",
                "KmsKeyId": "alias/healthlake-profiles",
                "SourceFormat": "CSV",
                "ClientToken": "token-0003",
                "Source": profile_mapping_source(&mapping_of(24)),
                "Tags": { "team": "clinical-data", "env": "test", "owner": "emulator" },
            }),
        ),
        (
            "a clone of a stored version, carrying a token",
            json!({
                "ProfileName": "idempotent-clone",
                "SourceFormat": "CCDA",
                "ClientToken": "token-0004",
                "Source": { "ExistingVersionedProfileId": {
                    "ProfileId": CLONE_ORIGIN_PLACEHOLDER,
                    "Version": 0,
                } },
            }),
        ),
    ]
}

/// Stands in for the origin profile's id in the clone row of
/// [`idempotency_cases`], which only exists once the test has created it.
const CLONE_ORIGIN_PLACEHOLDER: &str = "<origin>";

/// `ClientToken` is this operation's `@idempotencyToken`: a call carrying a token
/// the service has already seen is ignored and answered with the profile the first
/// call minted, mints no second profile and returns no error.
#[tokio::test]
async fn client_token_makes_the_create_idempotent() {
    let healthlake = Profiles::new();

    // The origin the clone row names, created once so that row has a stored
    // version to clone.
    let origin = healthlake
        .ok(
            "clone origin",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "idempotent-clone-origin",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[
                    ("sections/problems.map", "Condition.code <- problem/value"),
                    ("sections/vitals.map", "Observation.value[x] <- vital/value"),
                ])),
            }),
        )
        .await;
    let origin_id = string_at(&origin, "ProfileId");

    for (case, mut request) in idempotency_cases() {
        if request["Source"]["ExistingVersionedProfileId"]["ProfileId"]
            == json!(CLONE_ORIGIN_PLACEHOLDER)
        {
            request["Source"]["ExistingVersionedProfileId"]["ProfileId"] = json!(origin_id);
        }

        let first = healthlake
            .ok(case, "CreateDataTransformationProfile", &request)
            .await;
        let second = healthlake
            .ok(case, "CreateDataTransformationProfile", &request)
            .await;
        assert_eq!(
            first, second,
            "case {case:?}: the repeat answers exactly what the first call answered"
        );

        // Two creates WITHOUT a token are two profiles, so the sameness above is
        // the token's doing and not a collapsed create.
        let mut untokenized = request.clone();
        untokenized
            .as_object_mut()
            .expect("the request is an object")
            .remove("ClientToken");
        let third = healthlake
            .ok(case, "CreateDataTransformationProfile", &untokenized)
            .await;
        assert_ne!(
            third["ProfileId"], first["ProfileId"],
            "case {case:?}: a create without a token mints its own profile"
        );

        // The same token with a DIFFERENT request is IGNORED, not refused:
        // `CreateDataTransformationProfileRequest$ClientToken` documents that
        // "the service ignores the request but does not return an error".
        let mut altered = request.clone();
        altered["ProfileName"] = json!(format!(
            "{}-renamed",
            altered["ProfileName"].as_str().unwrap_or_default()
        ));
        let ignored = healthlake
            .ok(case, "CreateDataTransformationProfile", &altered)
            .await;
        assert_eq!(
            ignored, first,
            "case {case:?}: a call under a token already seen answers what the first call answered"
        );

        // The profile the token minted is still readable, and is the one the
        // repeat answered with.
        let profile_id = string_at(&first, "ProfileId");
        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            draft["ProfileId"].as_str(),
            Some(profile_id.as_str()),
            "case {case:?}: the token's profile is readable, got {draft}"
        );
    }
}
