// SPDX-License-Identifier: CC0-1.0

//! The two `CreateDataTransformationProfile` source variants whose content lives
//! outside the request: `StarterProfile` and `SampleData`.
//!
//! The rule under test, over a SET of inputs rather than one case: EVERY variant of
//! the modelled `CreateDataTransformationProfileSource` union
//! (`smithy_query(query="shape",shape="CreateDataTransformationProfileSource")`)
//! resolves the new profile's initial DRAFT content from its own source and mints a
//! profile at version 0 that `GetDataTransformationProfile` reads that content
//! back from — while a source naming content nothing can be read from is refused
//! with a modelled error and mints nothing.
//!
//! For the two variants below that means:
//!
//! * a starter profile resolves to the content of the library entry it names
//!   ([`service_com_amazonaws_healthlake_healthlake::state::STARTER_PROFILES`]): one
//!   mapping file per C-CDA section that document type carries, naming the FHIR R4
//!   resource that section's entries convert to, plus the entry's manifest; and
//! * a sample data file resolves to a mapping INFERRED from the CSV the emulator's
//!   own Amazon S3 holds at that location: one mapping file per column of the header
//!   row, carrying the data type the sampled values establish and the FHIR R4
//!   element or `Observation.value[x]` choice that type maps onto.
//!
//! Amazon S3 is mounted beside this service on ONE engine, exactly as the assembled
//! router mounts them, and every sample file below is staged through that service's
//! OWN `PutObject` — so what a create reads is what Amazon S3 really holds.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the entry point
//! `core/router` serves, so routing, deserialization, the modelled constraints and
//! the error envelope are all exercised.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    sample_data_too_many_columns_message, sample_data_unreadable_message,
    starter_profile_not_found_message, StarterProfile, MAX_PROFILE_MAPPING_ENTRIES,
    SAMPLE_DATA_ABSENT, SAMPLE_DATA_NOT_A_FILE, SAMPLE_DATA_NOT_TEXT, SAMPLE_DATA_NO_HEADER,
    STARTER_PROFILES,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, string_at};
use crate::fhir_rest_and_validate_source::HealthLake;

/// The success status this service answers `CreateDataTransformationProfile` with:
/// awsJson1_0 binds nothing to the transport, so the operation's `@http(code: 201)`
/// is inert and a success is the protocol's 200.
const CREATE_SUCCESS_STATUS: u16 = 200;

/// The only value the modelled `TargetFormat` enum declares, which the operation
/// documents as "Always FHIR_R4".
const TARGET_FORMAT: &str = "FHIR_R4";

/// The bucket every sample data file below is staged in, and one no bucket is
/// created for.
const BUCKET: &str = "healthlake-sample-data";
const UNHELD_BUCKET: &str = "healthlake-bucket-that-was-never-created";

/// A `CreateDataTransformationProfile` body naming `source` at `source_format`.
fn create_request(name: &str, source_format: &str, source: Value) -> Value {
    json!({
        "ProfileName": name,
        "SourceFormat": source_format,
        "Source": source,
    })
}

/// Dispatch a create that must succeed, returning its body.
async fn created(service: &HealthLake, case: &str, request: &Value) -> Value {
    let (status, payload) = service
        .rpc("CreateDataTransformationProfile", request)
        .await;
    assert_eq!(
        status, CREATE_SUCCESS_STATUS,
        "case {case:?}: the create must answer {CREATE_SUCCESS_STATUS}, got {status}: {payload}"
    );
    payload
}

/// Dispatch a create that must be refused, returning its status, code and message.
async fn refused(service: &HealthLake, case: &str, request: &Value) -> (u16, String, String) {
    let (status, payload) = service
        .rpc("CreateDataTransformationProfile", request)
        .await;
    assert!(
        status >= 400,
        "case {case:?}: the create must be refused, got {status}: {payload}"
    );
    (status, error_code(&payload), error_message(&payload))
}

/// The `ProfileMapping` `GetDataTransformationProfile` reports for one version of a
/// profile, as a map of file name to the JSON that file holds.
async fn draft_mapping(service: &HealthLake, case: &str, profile_id: &str) -> BTreeMap<String, Value> {
    let (status, payload) = service
        .rpc(
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the DRAFT must be readable, got {status}: {payload}"
    );
    payload["ProfileMapping"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the DRAFT must carry a ProfileMapping: {payload}"))
        .iter()
        .map(|(file, content)| {
            let text = content.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the file {file:?} must hold a string, got {content}")
            });
            let parsed: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: the file {file:?} must hold JSON ({error}): {text}")
            });
            (file.clone(), parsed)
        })
        .collect()
}

/// How many profiles this service holds at `source_format`, so a refused create can
/// be shown to have minted nothing. `SourceFormat` is a required member of
/// `ListDataTransformationProfiles`
/// (`smithy_query(query="operation",operation="ListDataTransformationProfiles")`),
/// and the listing reports its page under `Items`.
async fn held_profiles(service: &HealthLake, source_format: &str) -> usize {
    let (status, payload) = service
        .rpc(
            "ListDataTransformationProfiles",
            &json!({ "SourceFormat": source_format }),
        )
        .await;
    assert_eq!(status, 200, "the listing must answer 200, got {status}: {payload}");
    payload["Items"].as_array().map_or(0, Vec::len)
}

/// Assert the modelled create reply: version 0, the formats, the name, and a
/// `ProfileId` of the form the modelled `ProfileIdString` requires
/// (`^[a-f0-9]{32}$`).
fn assert_created_reply(case: &str, created: &Value, request: &Value) -> String {
    let profile_id = string_at(created, "ProfileId");
    assert_eq!(
        profile_id.len(),
        32,
        "case {case:?}: ProfileId must be 32 characters, got {profile_id:?}"
    );
    assert!(
        profile_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: ProfileId must be lowercase hexadecimal, got {profile_id:?}"
    );
    assert_eq!(
        created["Version"].as_i64(),
        Some(0),
        "case {case:?}: a created profile is the DRAFT, version 0, got {created}"
    );
    assert_eq!(
        created["SourceFormat"], request["SourceFormat"],
        "case {case:?}: the reply reports the requested SourceFormat, got {created}"
    );
    assert_eq!(
        created["TargetFormat"].as_str(),
        Some(TARGET_FORMAT),
        "case {case:?}: the only modelled TargetFormat is {TARGET_FORMAT}, got {created}"
    );
    assert_eq!(
        created["ProfileName"], request["ProfileName"],
        "case {case:?}: the reply reports the requested ProfileName, got {created}"
    );
    assert!(
        created["LastUpdatedAt"].is_number(),
        "case {case:?}: the reply must carry a LastUpdatedAt, got {created}"
    );
    profile_id
}

/// The library entry `name` addresses, for a row that names one.
fn library_entry(name: &str) -> &'static StarterProfile {
    STARTER_PROFILES
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("the library must publish {name:?}"))
}

/// A few of the C-CDA section to FHIR R4 resource pairs the starter profile library
/// maps, written here from HL7's C-CDA on FHIR mapping of the section templates onto
/// FHIR R4 resource types — so the content a create stores is asserted against the
/// standard rather than against whatever the library happens to hold.
const PUBLISHED_SECTION_TARGETS: &[(&str, &str)] = &[
    ("Allergies and Intolerances", "AllergyIntolerance"),
    ("Immunizations", "Immunization"),
    ("Medications", "MedicationStatement"),
    ("Payers", "Coverage"),
    ("Problems", "Condition"),
    ("Vital Signs", "Observation"),
];

/// The starter profile rows: entries across the library, addressed by the published
/// spelling of the name and by two other spellings of it, so what resolves an entry
/// is the name's letters and digits rather than its punctuation.
fn starter_profile_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "the published spelling of a document with many sections",
            "CONTINUITY_OF_CARE_DOCUMENT",
            "CONTINUITY_OF_CARE_DOCUMENT",
        ),
        (
            "a document with fewer sections",
            "PROGRESS_NOTE",
            "PROGRESS_NOTE",
        ),
        (
            "an entry the issue did not cite, lowercased",
            "transfer_summary",
            "TRANSFER_SUMMARY",
        ),
        ("a hyphenated spelling", "Care-Plan", "CARE_PLAN"),
        ("a spaced spelling", "operative note", "OPERATIVE_NOTE"),
    ]
}

/// RULE: a create whose `Source` names a starter profile mints a DRAFT at version 0
/// whose content is the library entry's own — one file per C-CDA section that
/// document carries, naming the FHIR R4 resource it converts to, plus the entry's
/// manifest — and `GetDataTransformationProfile` reads that content back.
#[tokio::test]
async fn a_starter_profile_source_creates_a_draft_from_the_library() {
    let service = HealthLake::new();
    let mut minted: Vec<String> = Vec::new();

    for (case, addressed, resolved) in starter_profile_rows() {
        let entry = library_entry(resolved);
        let request = create_request(
            &format!("starter-{}", entry.name.to_ascii_lowercase()),
            "CCDA",
            json!({ "StarterProfile": { "StarterProfileName": addressed } }),
        );
        let created = created(&service, case, &request).await;
        let profile_id = assert_created_reply(case, &created, &request);
        assert!(
            !minted.contains(&profile_id),
            "case {case:?}: every create mints its own profile, got {profile_id:?} twice"
        );
        minted.push(profile_id.clone());

        let mapping = draft_mapping(&service, case, &profile_id).await;
        assert_eq!(
            mapping.len(),
            entry.sections.len() + 1,
            "case {case:?}: the DRAFT holds one file per section of {:?} plus its manifest, \
             got {:?}",
            entry.document_type,
            mapping.keys().collect::<Vec<_>>()
        );

        // The manifest states which library entry the content came from and what it
        // converts between.
        let manifest = mapping
            .get("profile.json")
            .unwrap_or_else(|| panic!("case {case:?}: the DRAFT must carry profile.json"));
        assert_eq!(
            manifest["starterProfileName"].as_str(),
            Some(entry.name),
            "case {case:?}: the manifest names the library entry, got {manifest}"
        );
        assert_eq!(
            manifest["sourceDocumentType"].as_str(),
            Some(entry.document_type),
            "case {case:?}: the manifest names the C-CDA document type, got {manifest}"
        );
        assert_eq!(
            manifest["sourceFormat"].as_str(),
            Some("CCDA"),
            "case {case:?}: the manifest names the source format, got {manifest}"
        );
        assert_eq!(
            manifest["targetFormat"].as_str(),
            Some(TARGET_FORMAT),
            "case {case:?}: the manifest names the target format, got {manifest}"
        );

        // One file per section, and each file names the section and a FHIR R4
        // resource type.
        for section in entry.sections {
            let file = format!(
                "sections/{}.json",
                section
                    .to_ascii_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-")
            );
            let rule = mapping
                .get(&file)
                .unwrap_or_else(|| panic!("case {case:?}: the DRAFT must carry {file:?}"));
            assert_eq!(
                rule["sourceSection"].as_str(),
                Some(*section),
                "case {case:?}: {file:?} names its C-CDA section, got {rule}"
            );
            assert_eq!(
                rule["targetFormat"].as_str(),
                Some(TARGET_FORMAT),
                "case {case:?}: {file:?} converts to {TARGET_FORMAT}, got {rule}"
            );
            let target = rule["targetResourceType"].as_str().unwrap_or_default();
            assert!(
                target
                    .chars()
                    .all(char::is_alphanumeric)
                    && target.starts_with(char::is_uppercase),
                "case {case:?}: {file:?} must name a FHIR R4 resource type, got {rule}"
            );
            // Where HL7 publishes the pair, it is the pair the content carries.
            if let Some((_, published)) = PUBLISHED_SECTION_TARGETS
                .iter()
                .find(|(published_section, _)| published_section == section)
            {
                assert_eq!(
                    target, *published,
                    "case {case:?}: {file:?} must convert to {published}, got {rule}"
                );
            }
        }
    }

    // The library is not consumed by being read: the same entry creates a second,
    // distinct profile carrying the same content.
    let case = "the same starter profile twice";
    let request = create_request(
        "starter-twice",
        "CCDA",
        json!({ "StarterProfile": { "StarterProfileName": "PROGRESS_NOTE" } }),
    );
    let again = created(&service, case, &request).await;
    let second_id = assert_created_reply(case, &again, &request);
    let first_id = minted
        .get(1)
        .cloned()
        .expect("the PROGRESS_NOTE row minted a profile");
    assert_ne!(
        second_id, first_id,
        "case {case:?}: a second create mints its own profile"
    );
    assert_eq!(
        draft_mapping(&service, case, &second_id).await,
        draft_mapping(&service, case, &first_id).await,
        "case {case:?}: the library entry resolves to the same content every time"
    );
}

/// RULE: a `StarterProfile` naming content the library does not hold resolves to
/// nothing, so the create is refused with the modelled `ValidationException` naming
/// the member and the names the library does hold — and mints no profile.
#[tokio::test]
async fn a_starter_profile_the_library_does_not_hold_is_refused() {
    let service = HealthLake::new();
    let rows: &[(&str, &str)] = &[
        ("a name nothing publishes", "CCDA_TO_FHIR_R4"),
        ("a C-CDA document type the library ships no entry for", "UNSTRUCTURED_DOCUMENT"),
        ("another one", "DIAGNOSTIC_IMAGING_REPORT"),
        ("a prefix of a published name", "PROGRESS"),
        ("a single character", "x"),
    ];
    for (case, addressed) in rows {
        let request = create_request(
            "starter-unknown",
            "CCDA",
            json!({ "StarterProfile": { "StarterProfileName": addressed } }),
        );
        let (status, code, message) = refused(&service, case, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        assert_eq!(
            message,
            starter_profile_not_found_message(),
            "case {case:?}: the refusal names the member and the library's names, got {message:?}"
        );
    }
    assert_eq!(
        held_profiles(&service, "CCDA").await,
        0,
        "a refused create must mint no profile"
    );
}

/// One sample data row: the CSV staged in Amazon S3 and what the inference must
/// read out of it.
struct SampleRow {
    case: &'static str,
    /// The object key the file is staged under.
    key: &'static str,
    /// The file's bytes, exactly as a caller staged them.
    csv: String,
    /// What each column must become: the mapping file, the inferred data type, the
    /// FHIR R4 resource type and the element the column maps onto.
    columns: Vec<(String, &'static str, &'static str, String)>,
    /// How many data rows the inference reports having sampled.
    rows_sampled: usize,
}

/// The sample data rows: the column classes the inference spans — a column named for
/// a FHIR R4 `Patient` element and one that is not, every data type a column's values
/// establish, a quoted field carrying the delimiter, a header row with no data under
/// it, and a file of many columns.
fn sample_rows() -> Vec<SampleRow> {
    let many: Vec<String> = (1..=60).map(|index| format!("measure_{index:02}")).collect();
    vec![
        SampleRow {
            case: "patient elements beside numeric observations",
            key: "patients.csv",
            csv: "patient_id,birth_date,gender,systolic_bp,weight_kg\n\
                  p-1,1980-02-29,female,120,61.5\n\
                  p-2,1975-11-03,male,131,88\n"
                .to_string(),
            columns: vec![
                (
                    "columns/001-patient-id.json".to_string(),
                    "string",
                    "Patient",
                    "Patient.id".to_string(),
                ),
                (
                    "columns/002-birth-date.json".to_string(),
                    "date",
                    "Patient",
                    "Patient.birthDate".to_string(),
                ),
                (
                    "columns/003-gender.json".to_string(),
                    "string",
                    "Patient",
                    "Patient.gender".to_string(),
                ),
                (
                    "columns/004-systolic-bp.json".to_string(),
                    "integer",
                    "Observation",
                    "Observation.valueQuantity".to_string(),
                ),
                (
                    "columns/005-weight-kg.json".to_string(),
                    "decimal",
                    "Observation",
                    "Observation.valueQuantity".to_string(),
                ),
            ],
            rows_sampled: 2,
        },
        SampleRow {
            case: "a quoted field carrying the delimiter, a boolean and a date-time",
            key: "nested/encounters.csv",
            csv: "name,deceased,recorded_at,note\n\
                  \"Doe, Jane\",false,2026-01-02T03:04:05Z,\"two, words\"\n\
                  \"Roe, John\",TRUE,2026-01-03,ok\n"
                .to_string(),
            columns: vec![
                (
                    "columns/001-name.json".to_string(),
                    "string",
                    "Patient",
                    "Patient.name".to_string(),
                ),
                (
                    "columns/002-deceased.json".to_string(),
                    "boolean",
                    "Observation",
                    "Observation.valueBoolean".to_string(),
                ),
                (
                    "columns/003-recorded-at.json".to_string(),
                    "dateTime",
                    "Observation",
                    "Observation.valueDateTime".to_string(),
                ),
                (
                    "columns/004-note.json".to_string(),
                    "string",
                    "Observation",
                    "Observation.valueString".to_string(),
                ),
            ],
            rows_sampled: 2,
        },
        SampleRow {
            case: "a header row with no data under it",
            key: "header-only.csv",
            csv: "marital_status\n".to_string(),
            columns: vec![(
                "columns/001-marital-status.json".to_string(),
                "string",
                "Patient",
                "Patient.maritalStatus".to_string(),
            )],
            rows_sampled: 0,
        },
        SampleRow {
            case: "a file of many columns",
            key: "wide.csv",
            csv: format!("{}\n", many.join(",")),
            columns: many
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    (
                        format!(
                            "columns/{:03}-{}.json",
                            index + 1,
                            column.replace('_', "-")
                        ),
                        "string",
                        "Observation",
                        "Observation.valueString".to_string(),
                    )
                })
                .collect(),
            rows_sampled: 0,
        },
    ]
}

/// RULE: a create whose `Source` names a sample data file mints a DRAFT at version 0
/// whose content is INFERRED from that file — one mapping file per column of its
/// header row, carrying the data type the sampled values establish and the FHIR R4
/// target that column maps onto — and `GetDataTransformationProfile` reads it back.
#[tokio::test]
async fn a_sample_data_source_infers_a_draft_from_the_file() {
    let service = HealthLake::with_object_store();
    let created_bucket = service.s3().create_bucket().bucket(BUCKET).send().await;
    assert!(
        created_bucket.is_ok(),
        "Amazon S3 must create the sample bucket, got {created_bucket:?}"
    );

    for row in sample_rows() {
        let staged = service
            .s3()
            .put_object()
            .bucket(BUCKET)
            .key(row.key)
            .body(aws_sdk_s3::primitives::ByteStream::from(
                row.csv.clone().into_bytes(),
            ))
            .send()
            .await;
        assert!(
            staged.is_ok(),
            "case {:?}: Amazon S3 must hold the sample file, got {staged:?}",
            row.case
        );

        let request = create_request(
            &format!("sample-{}", row.key.replace(['/', '.'], "-")),
            "CSV",
            json!({ "SampleData": { "S3Uri": format!("s3://{BUCKET}/{}", row.key) } }),
        );
        let created = created(&service, row.case, &request).await;
        let profile_id = assert_created_reply(row.case, &created, &request);

        let mapping = draft_mapping(&service, row.case, &profile_id).await;
        assert_eq!(
            mapping.len(),
            row.columns.len() + 1,
            "case {:?}: the DRAFT holds one file per column plus its manifest, got {:?}",
            row.case,
            mapping.keys().collect::<Vec<_>>()
        );
        let manifest = mapping
            .get("profile.json")
            .unwrap_or_else(|| panic!("case {:?}: the DRAFT must carry profile.json", row.case));
        assert_eq!(
            manifest["columnCount"].as_u64(),
            Some(row.columns.len() as u64),
            "case {:?}: the manifest counts the columns read, got {manifest}",
            row.case
        );
        assert_eq!(
            manifest["rowsSampled"].as_u64(),
            Some(row.rows_sampled as u64),
            "case {:?}: the manifest counts the data rows sampled, got {manifest}",
            row.case
        );
        assert_eq!(
            manifest["sourceFormat"].as_str(),
            Some("CSV"),
            "case {:?}: the manifest names the source format, got {manifest}",
            row.case
        );

        for (number, (file, data_type, resource, element)) in row.columns.iter().enumerate() {
            let rule = mapping.get(file).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the DRAFT must carry {file:?}, got {:?}",
                    row.case,
                    mapping.keys().collect::<Vec<_>>()
                )
            });
            assert_eq!(
                rule["sourceColumnNumber"].as_u64(),
                Some(number as u64 + 1),
                "case {:?}: {file:?} carries the column's position, got {rule}",
                row.case
            );
            assert_eq!(
                rule["sourceDataType"].as_str(),
                Some(*data_type),
                "case {:?}: {file:?} carries the data type its values establish, got {rule}",
                row.case
            );
            assert_eq!(
                rule["targetResourceType"].as_str(),
                Some(*resource),
                "case {:?}: {file:?} maps onto {resource}, got {rule}",
                row.case
            );
            assert_eq!(
                rule["targetElement"].as_str(),
                Some(element.as_str()),
                "case {:?}: {file:?} maps onto {element}, got {rule}",
                row.case
            );
            assert_eq!(
                rule["targetFormat"].as_str(),
                Some(TARGET_FORMAT),
                "case {:?}: {file:?} converts to {TARGET_FORMAT}, got {rule}",
                row.case
            );
        }
    }
}

/// RULE: a `SampleData` location nothing can be read a mapping from is refused with
/// the modelled `ValidationException` saying which, and mints no profile — never a
/// profile whose content nothing read.
#[tokio::test]
async fn a_sample_data_file_nothing_can_be_read_from_is_refused() {
    let service = HealthLake::with_object_store();
    let created_bucket = service.s3().create_bucket().bucket(BUCKET).send().await;
    assert!(
        created_bucket.is_ok(),
        "Amazon S3 must create the sample bucket, got {created_bucket:?}"
    );
    // The files that exist but hold nothing a mapping can be inferred from.
    let staged: &[(&str, Vec<u8>)] = &[
        // Blank lines carry no CSV record at all, so this file has no header row.
        ("blank.csv", b"\n\n".to_vec()),
        ("not-text.csv", vec![0xff, 0xfe, 0x00, 0x01]),
        (
            "too-wide.csv",
            format!(
                "{}\n",
                (1..=MAX_PROFILE_MAPPING_ENTRIES)
                    .map(|index| format!("c{index}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .into_bytes(),
        ),
    ];
    for (key, bytes) in staged {
        let put = service
            .s3()
            .put_object()
            .bucket(BUCKET)
            .key(*key)
            .body(aws_sdk_s3::primitives::ByteStream::from(bytes.clone()))
            .send()
            .await;
        assert!(
            put.is_ok(),
            "Amazon S3 must hold the staged file {key:?}, got {put:?}"
        );
    }

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a bucket Amazon S3 holds nothing under",
            format!("s3://{UNHELD_BUCKET}/patients.csv"),
            SAMPLE_DATA_ABSENT.to_string(),
        ),
        (
            "a key the bucket holds no object under",
            format!("s3://{BUCKET}/never-staged.csv"),
            SAMPLE_DATA_ABSENT.to_string(),
        ),
        (
            "a URI naming a bucket and no file",
            format!("s3://{BUCKET}"),
            SAMPLE_DATA_NOT_A_FILE.to_string(),
        ),
        (
            "a URI naming a bucket and an empty key",
            format!("s3://{BUCKET}/"),
            SAMPLE_DATA_NOT_A_FILE.to_string(),
        ),
        (
            "a file holding no header row",
            format!("s3://{BUCKET}/blank.csv"),
            SAMPLE_DATA_NO_HEADER.to_string(),
        ),
        (
            "a file that is not UTF-8 text",
            format!("s3://{BUCKET}/not-text.csv"),
            SAMPLE_DATA_NOT_TEXT.to_string(),
        ),
        (
            "a header row of more columns than one mapping holds entries",
            format!("s3://{BUCKET}/too-wide.csv"),
            sample_data_too_many_columns_message(),
        ),
    ];
    for (case, s3_uri, expected) in rows {
        let request = create_request("sample-refused", "CSV", json!({ "SampleData": { "S3Uri": s3_uri } }));
        let (status, code, message) = refused(&service, case, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A zero-byte object has no first byte to serve, which is what the shared object
    // read reports as unreadable — still a refusal in the same words as any other
    // file nothing could be read from, and still no profile.
    let empty = service
        .s3()
        .put_object()
        .bucket(BUCKET)
        .key("empty.csv")
        .body(aws_sdk_s3::primitives::ByteStream::from(Vec::new()))
        .send()
        .await;
    assert!(
        empty.is_ok(),
        "Amazon S3 must hold the empty file, got {empty:?}"
    );
    let case = "a zero-byte file";
    let request = create_request(
        "sample-empty",
        "CSV",
        json!({ "SampleData": { "S3Uri": format!("s3://{BUCKET}/empty.csv") } }),
    );
    let (status, code, message) = refused(&service, case, &request).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
    );
    assert_eq!(
        message,
        sample_data_unreadable_message(),
        "case {case:?}: the refusal must say nothing could be read, got {message:?}"
    );

    assert_eq!(
        held_profiles(&service, "CSV").await,
        0,
        "a refused create must mint no profile"
    );
}
