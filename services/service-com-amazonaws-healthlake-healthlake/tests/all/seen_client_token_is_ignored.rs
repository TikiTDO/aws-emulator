// SPDX-License-Identifier: CC0-1.0

//! What a `ClientToken` the service has ALREADY seen does to the call carrying
//! it, over the real awsJson1_0 wire, for the two operations whose modelled
//! token says so: `StartDataTransformationJob` and
//! `CreateDataTransformationProfile`.
//!
//! The rule under test, as both operations' `ClientToken` member documents it
//! (`smithy_query(query="doc", shape="StartDataTransformationJobRequest",
//! member="ClientToken")` and the same for
//! `CreateDataTransformationProfileRequest`): "If this token matches a previous
//! request, the service ignores the request but does not return an error." So a
//! call under a token already seen
//!
//! 1. is served rather than refused, whatever the rest of its body says;
//! 2. answers out of the record that token minted;
//! 3. mints nothing — no second job, no second profile; and
//! 4. changes nothing on that record, which still reports what the FIRST call
//!    asked for.
//!
//! The tables span what the rule generalises over: HOW the second call differs
//! from the one that minted the record — a member added, a member dropped, a
//! member re-pointed, a boolean flipped, a union variant swapped, every member
//! at once — including bodies a FIRST call would be refused for, which the two
//! contrast assertions below pin as genuinely refused under a fresh token.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::ComAmazonawsHealthlakeHealthlakeState;

use crate::data_transformation_profile::{
    mapping, mapping_of, profile_mapping_source, reported_mapping,
};
use crate::fhir_datastore_lifecycle::{error_code, string_at, HealthLake};
use crate::start_data_transformation_job::profile;

/// The `DataAccessRoleArn` the starts below carry, satisfying the modelled
/// `DataTransformationIamRoleArn` pattern
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeTransform";

/// A well-formed `ProfileId` no profile is held for: 32 lowercase hexadecimal
/// characters, exactly what the modelled `ProfileIdString` pattern
/// (`^[a-f0-9]{32}$`) requires, so a FIRST call naming it is refused for the
/// profile not existing rather than for the identifier's shape.
const UNHELD_PROFILE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// The success status this service answers `CreateDataTransformationProfile` with:
/// awsJson1_0 binds nothing to the transport, so the operation's `@http(code: 201)`
/// is inert on the wire and a success is the protocol's 200.
const CREATE_SUCCESS_STATUS: u16 = 200;

/// One row: how the second call's body differs from the body that minted the
/// record, named by its case.
struct Variant {
    case: &'static str,
    /// Members written over the minting body (a JSON object).
    overrides: Value,
    /// Members dropped from the minting body.
    removed: &'static [&'static str],
}

impl Variant {
    fn new(case: &'static str, overrides: Value) -> Self {
        Self {
            case,
            overrides,
            removed: &[],
        }
    }

    fn dropping(case: &'static str, removed: &'static [&'static str]) -> Self {
        Self {
            case,
            overrides: json!({}),
            removed,
        }
    }

    /// The minting body with this row's difference applied.
    fn applied(&self, minting: &Value) -> Value {
        let mut body = minting.clone();
        let object = body.as_object_mut().expect("the request is an object");
        for (member, value) in self
            .overrides
            .as_object()
            .expect("the overrides are an object")
        {
            object.insert(member.clone(), value.clone());
        }
        for member in self.removed {
            object.remove(*member);
        }
        body
    }
}

/// One row's body, under a `ClientToken` the service has NOT seen — the
/// contrast that shows what the same body is answered with on its own merits.
fn under_a_fresh_token(
    variants: &[Variant],
    case: &str,
    minting: &Value,
    token: &str,
) -> Value {
    let variant = variants
        .iter()
        .find(|variant| variant.case == case)
        .unwrap_or_else(|| panic!("the table must hold the case {case:?}"));
    let mut body = variant.applied(minting);
    body.as_object_mut()
        .expect("the request is an object")
        .insert("ClientToken".to_string(), json!(token));
    body
}

/// How many data transformation jobs, and how many profiles, the store holds.
fn held(service: &HealthLake) -> (usize, usize) {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let store = store.lock();
    (store.transformation_jobs.len(), store.profiles.len())
}

/// The `TransformationJobProperties` `DescribeDataTransformationJob` reports.
async fn describe_job(service: &HealthLake, job_id: &str) -> Value {
    service
        .ok("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
        .await["TransformationJobProperties"]
        .clone()
}

/// The `StartDataTransformationJob` body the job under test is started from:
/// every optional member present, so a row can drop one as well as change one.
fn minting_start(profile_id: &str) -> Value {
    json!({
        "ClientToken": "token-seen-start",
        "ProfileId": profile_id,
        "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
        "JobName": "hourly",
        "InputDataConfig": { "S3Uri": "s3://transform-in-bucket/hourly", "SourceFormat": "CSV" },
        "OutputDataConfig": { "S3Configuration": {
            "S3Uri": "s3://transform-out-bucket/r4",
            "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
        } },
        "DriftDetectionEnabled": true,
        "ProvenanceEnabled": false,
    })
}

/// The rows for the start: one per kind of difference the modelled input admits,
/// `other_profile` being a second profile the service really holds.
fn start_variants(other_profile: &str) -> Vec<Variant> {
    vec![
        Variant::new("the identical request", json!({})),
        Variant::new("a different JobName", json!({ "JobName": "nightly" })),
        Variant::dropping("no JobName at all", &["JobName"]),
        Variant::new(
            "a re-pointed InputDataConfig",
            json!({ "InputDataConfig": { "S3Uri": "s3://transform-in-bucket/elsewhere" } }),
        ),
        Variant::new(
            "the other modelled InputDataConfig SourceFormat",
            json!({ "InputDataConfig": {
                "S3Uri": "s3://transform-in-bucket/hourly",
                "SourceFormat": "CCDA",
            } }),
        ),
        Variant::new(
            "a re-pointed OutputDataConfig under a key alias",
            json!({ "OutputDataConfig": { "S3Configuration": {
                "S3Uri": "s3://transform-out-bucket/elsewhere",
                "KmsKeyId": "alias/healthlake/other",
            } } }),
        ),
        Variant::new(
            "a different DataAccessRoleArn",
            json!({ "DataAccessRoleArn": "arn:aws:iam::111122223333:role/OtherTransform" }),
        ),
        Variant::new(
            "ProvenanceEnabled flipped",
            json!({ "ProvenanceEnabled": true }),
        ),
        Variant::dropping(
            "DriftDetectionEnabled dropped",
            &["DriftDetectionEnabled"],
        ),
        Variant::new(
            "another profile this service holds",
            json!({ "ProfileId": other_profile }),
        ),
        Variant::new(
            "a ProfileId no profile is held for",
            json!({ "ProfileId": UNHELD_PROFILE_ID }),
        ),
        Variant::new(
            "every member at once",
            json!({
                "ProfileId": UNHELD_PROFILE_ID,
                "DataAccessRoleArn": "arn:aws:iam::444455556666:role/Third",
                "JobName": "rebuilt",
                "InputDataConfig": { "S3Uri": "s3://third-in-bucket/all", "SourceFormat": "CCDA" },
                "OutputDataConfig": { "S3Configuration": {
                    "S3Uri": "s3://third-out-bucket/all",
                    "KmsKeyId": "arn:aws:kms:us-east-1:444455556666:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                } },
                "DriftDetectionEnabled": false,
                "ProvenanceEnabled": true,
            }),
        ),
    ]
}

/// The members `DescribeDataTransformationJob` reports for the job the minting
/// start created, so an ignored call can be shown to have changed none of them.
fn minted_job_members(profile_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("JobName", json!("hourly")),
        ("ProfileId", json!(profile_id)),
        ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
        (
            "InputDataConfig",
            json!({ "S3Uri": "s3://transform-in-bucket/hourly", "SourceFormat": "CSV" }),
        ),
        (
            "OutputDataConfig",
            json!({ "S3Configuration": {
                "S3Uri": "s3://transform-out-bucket/r4",
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            } }),
        ),
        ("DriftDetectionEnabled", json!(true)),
        ("ProvenanceEnabled", json!(false)),
    ]
}

#[tokio::test]
async fn a_start_under_a_seen_token_is_ignored_and_answered_with_its_job() {
    let service = HealthLake::new();
    let (profile_id, _name) = profile(&service, "seen-token-start").await;
    let (other_profile, _other_name) = profile(&service, "seen-token-start-other").await;
    let minting = minting_start(&profile_id);

    let first = service.ok("StartDataTransformationJob", &minting).await;
    let job_id = string_at(&first, "JobId");

    for variant in start_variants(&other_profile) {
        let case = variant.case;
        let body = variant.applied(&minting);
        // The status the stored job holds on either side of the call, so the
        // assertion below holds whether or not the clock settled the job in
        // between (this job's one owed transition is SUBMITTED -> FAILED).
        let before = describe_job(&service, &job_id).await;
        let (status, payload) = service.call("StartDataTransformationJob", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call under a token already seen must not be refused, \
             got {status}: {payload}"
        );
        assert_eq!(
            payload["JobId"],
            json!(job_id),
            "case {case:?}: the call must answer the job the token started ({job_id:?}), \
             got {payload}"
        );

        // Answered out of the stored record: the status it reports is one the
        // reporting operation reads off that same job, not one of its own.
        let described = describe_job(&service, &job_id).await;
        assert!(
            payload["JobStatus"] == before["JobStatus"]
                || payload["JobStatus"] == described["JobStatus"],
            "case {case:?}: the call must report a status the stored job held, got {payload} \
             between {before} and {described}"
        );

        // Nothing the ignored call carried reached the store.
        for (member, expected) in minted_job_members(&profile_id) {
            assert_eq!(
                described[member], expected,
                "case {case:?}: an ignored call must leave {member} as the first call set \
                 it ({expected}), got {described}"
            );
        }
        let (jobs, _profiles) = held(&service);
        assert_eq!(
            jobs, 1,
            "case {case:?}: an ignored call must start no further job, got {jobs} held"
        );
    }

    // The ignoring is the TOKEN's doing: the very bodies the rows above carried
    // are served or refused on their own merits under a token not yet seen — a
    // held profile starts a second job, an unheld one is the modelled 404.
    let variants = start_variants(&other_profile);
    let fresh = under_a_fresh_token(
        &variants,
        "another profile this service holds",
        &minting,
        "token-fresh-held",
    );
    let served = service.ok("StartDataTransformationJob", &fresh).await;
    assert_ne!(
        served["JobId"],
        json!(job_id),
        "an unseen token must start its own job, got {served}"
    );

    let unheld = under_a_fresh_token(
        &variants,
        "a ProfileId no profile is held for",
        &minting,
        "token-fresh-unheld",
    );
    let (status, payload) = service.call("StartDataTransformationJob", &unheld).await;
    assert_eq!(
        status, 400,
        "a FIRST start naming a profile no record holds must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "a FIRST start naming a profile no record holds must be ResourceNotFoundException, \
         got {payload}"
    );
    let (jobs, _profiles) = held(&service);
    assert_eq!(
        jobs, 2,
        "only the served first start above may have added a job, got {jobs} held"
    );
}

/// The `CreateDataTransformationProfile` body the profile under test is minted
/// from, carrying raw profile content.
fn minting_create() -> Value {
    json!({
        "ClientToken": "token-seen-create",
        "ProfileName": "seen-token-profile",
        "SourceFormat": "CSV",
        "ProfileDescription": "the content the first call carried",
        "Source": profile_mapping_source(&mapping(&[(
            "patients.csv.map",
            "Patient.id <- patient_id",
        )])),
    })
}

/// The rows for the create. The `Source` variants come from the modelled
/// `CreateDataTransformationProfileSource` union
/// (`smithy_query(query="shape", shape="CreateDataTransformationProfileSource")`),
/// including the two whose content lives outside the request — and which a FIRST
/// call therefore refuses here, because this harness mounts no Amazon S3 to read a
/// sample file from and the library holds no entry under the name below.
fn create_variants() -> Vec<Variant> {
    vec![
        Variant::new("the identical request", json!({})),
        Variant::new(
            "a different ProfileName",
            json!({ "ProfileName": "renamed" }),
        ),
        Variant::new(
            "the other modelled SourceFormat",
            json!({ "SourceFormat": "CCDA" }),
        ),
        Variant::dropping("no ProfileDescription at all", &["ProfileDescription"]),
        Variant::new(
            "every other optional member added",
            json!({
                "KmsKeyId": "alias/healthlake-profiles",
                "Tags": { "team": "clinical-data", "env": "test" },
            }),
        ),
        Variant::new(
            "many more mapping entries",
            json!({ "Source": profile_mapping_source(&mapping_of(24)) }),
        ),
        Variant::new(
            "a clone of a version no profile holds",
            json!({ "Source": { "ExistingVersionedProfileId": {
                "ProfileId": UNHELD_PROFILE_ID,
                "Version": 0,
            } } }),
        ),
        Variant::new(
            "a starter profile the built-in library does not hold",
            json!({
                "SourceFormat": "CCDA",
                "Source": { "StarterProfile": { "StarterProfileName": "CCDA_TO_FHIR_R4" } },
            }),
        ),
        Variant::new(
            "a sample data file this harness has no Amazon S3 to read",
            json!({ "Source": { "SampleData": { "S3Uri": "s3://sample-bucket/patients.csv" } } }),
        ),
    ]
}

#[tokio::test]
async fn a_create_under_a_seen_token_is_ignored_and_answered_with_its_profile() {
    let service = HealthLake::new();
    let minting = minting_create();

    let (status, first) = service
        .call("CreateDataTransformationProfile", &minting)
        .await;
    assert_eq!(
        status, CREATE_SUCCESS_STATUS,
        "the minting create must answer its modelled {CREATE_SUCCESS_STATUS}, got {status}: {first}"
    );
    let profile_id = string_at(&first, "ProfileId");

    for variant in create_variants() {
        let case = variant.case;
        let body = variant.applied(&minting);
        let (status, payload) = service
            .call("CreateDataTransformationProfile", &body)
            .await;
        assert_eq!(
            status, CREATE_SUCCESS_STATUS,
            "case {case:?}: a call under a token already seen must not be refused, \
             got {status}: {payload}"
        );
        assert_eq!(
            payload, first,
            "case {case:?}: the call must answer exactly what the create that minted the \
             profile answered"
        );

        // Nothing the ignored call carried reached the stored profile: its DRAFT
        // still holds the content and the metadata the first call set.
        let draft = service
            .ok(
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &draft),
            mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
            "case {case:?}: an ignored call must leave the DRAFT content the first call \
             carried, got {draft}"
        );
        for (member, expected) in [
            ("ProfileName", json!("seen-token-profile")),
            ("SourceFormat", json!("CSV")),
            ("ProfileDescription", json!("the content the first call carried")),
        ] {
            assert_eq!(
                draft[member], expected,
                "case {case:?}: an ignored call must leave {member} as the first call set \
                 it ({expected}), got {draft}"
            );
        }
        let (_jobs, profiles) = held(&service);
        assert_eq!(
            profiles, 1,
            "case {case:?}: an ignored call must mint no further profile, got {profiles} held"
        );
    }

    // The ignoring is the TOKEN's doing: under a token not yet seen, the bodies
    // above are answered on their own merits — a clone of a version nothing
    // holds is the modelled 404, and a source whose content nothing could be read
    // from is the modelled 400.
    for (case, token, expected_code) in [
        (
            "a clone of a version no profile holds",
            "token-fresh-clone",
            "ResourceNotFoundException",
        ),
        (
            "a starter profile the built-in library does not hold",
            "token-fresh-starter",
            "ValidationException",
        ),
        (
            "a sample data file this harness has no Amazon S3 to read",
            "token-fresh-sample",
            "ValidationException",
        ),
    ] {
        let fresh = under_a_fresh_token(&create_variants(), case, &minting, token);
        let (status, payload) = service
            .call("CreateDataTransformationProfile", &fresh)
            .await;
        assert!(
            status >= 400,
            "case {case:?}: a FIRST create carrying this source must be refused, \
             got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_code,
            "case {case:?}: a FIRST create carrying this source must answer \
             {expected_code}, got {payload}"
        );
        let (_jobs, profiles) = held(&service);
        assert_eq!(
            profiles, 1,
            "case {case:?}: a refused create must mint no profile, got {profiles} held"
        );
    }

    // A fresh token with the minting body mints a SECOND profile, so the rows
    // above were ignored rather than collapsed by anything else.
    let fresh = under_a_fresh_token(
        &create_variants(),
        "the identical request",
        &minting,
        "token-fresh-create",
    );
    let (status, payload) = service
        .call("CreateDataTransformationProfile", &fresh)
        .await;
    assert_eq!(
        status, CREATE_SUCCESS_STATUS,
        "a create under an unseen token must be served, got {status}: {payload}"
    );
    assert_ne!(
        payload["ProfileId"],
        json!(profile_id),
        "an unseen token must mint its own profile, got {payload}"
    );
}
