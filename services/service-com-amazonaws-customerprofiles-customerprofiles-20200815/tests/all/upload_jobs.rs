// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateUploadJob` and `GetUploadJob`.
//!
//! The rule under test: `CreateUploadJob` creates the metadata for an upload job in a
//! stored domain — "the metadata is created for the job with the provided field mapping
//! and unique key" — mints the job's `JobId`, and persists the job in the status a job
//! that has not started carries, so a following `GetUploadJob` reports exactly the
//! metadata that was stored; a reference to a domain or a job that is not stored is
//! `ResourceNotFoundException` (404) and input outside a modelled constraint is refused
//! with nothing persisted.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, upload_job_fields, Emulator};

/// `CreateUploadJobRequest$DataExpiry`: "If not provided, the system default of 2 weeks
/// is used."
const DEFAULT_DATA_EXPIRY_DAYS: i64 = 14;

/// The status `GetUploadJobResponse$Status` documents for a job that has not started:
/// "CREATED: The upload job has been created, but has not started processing yet."
const CREATED: &str = "CREATED";

/// Whether `job_id` is the 32 lowercase hex characters `uuid`'s modelled pattern
/// `^[a-f0-9]{32}$` requires.
fn is_modelled_job_id(job_id: &str) -> bool {
    job_id.len() == 32 && job_id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A create persists the metadata the request carried, and the read reports it — over a
/// table of requests differing in the size of the field mapping, the length of the unique
/// key, the display name, and whether `DataExpiry` is stated at all.
#[tokio::test]
async fn a_create_persists_the_job_and_the_read_reports_it() {
    let emulator = Emulator::new();
    emulator.create_domain("uploaddom", 30).await;

    let rows: Vec<(&str, Value, i64)> = vec![
        (
            "no DataExpiry, so the documented default applies",
            json!({
                "DisplayName": "contacts.csv",
                "Fields": upload_job_fields("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
                "UniqueKey": "email",
            }),
            DEFAULT_DATA_EXPIRY_DAYS,
        ),
        (
            "a DataExpiry at the modelled minimum",
            json!({
                "DisplayName": "one-day.csv",
                "Fields": upload_job_fields("phone", "PHONE_NUMBER", "_profile.PhoneNumber"),
                "UniqueKey": "phone",
                "DataExpiry": 1,
            }),
            1,
        ),
        (
            "a DataExpiry at the modelled maximum",
            json!({
                "DisplayName": "d",
                "Fields": upload_job_fields("account", "STRING", "_profile.AccountNumber"),
                "UniqueKey": "account",
                "DataExpiry": 1098,
            }),
            1098,
        ),
        (
            "a display name at the maximum length and a mapping of several columns",
            json!({
                "DisplayName": "n".repeat(255),
                "Fields": {
                    "first_name": { "ContentType": "NAME", "Source": "_source.first_name", "Target": "_profile.FirstName" },
                    "last.name": { "ContentType": "NAME", "Source": "_source.last.name", "Target": "_profile.LastName" },
                    "e-mail": { "ContentType": "EMAIL_ADDRESS", "Source": "_source.e-mail", "Target": "_profile.EmailAddress" },
                    "loyalty_id": { "ContentType": "NUMBER", "Source": "_source.loyalty_id", "Target": "_profile.Attributes.LoyaltyId" },
                },
                "UniqueKey": "e-mail",
                "DataExpiry": 90,
            }),
            90,
        ),
        (
            "a unique key at the maximum length and a field carrying only a target",
            json!({
                "DisplayName": "long-key.csv",
                "Fields": { "k": { "Target": "_profile.Attributes.K" } },
                "UniqueKey": "k".repeat(1000),
                "DataExpiry": 2,
            }),
            2,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body, expected_expiry) in rows {
        let (status, response) = emulator.create_upload_job("uploaddom", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {body:?}, got {status} {response:?}"
        );
        let job_id = response
            .get("JobId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_job_id(&job_id),
            "case {case:?}: expected a 32-character lowercase hex JobId, got {response:?}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every create must mint its own JobId, got {job_id:?} again \
             after {minted:?}"
        );
        minted.push(job_id.clone());

        let (status, read) = emulator.get_upload_job("uploaddom", &job_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetUploadJob 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("JobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: expected the read to report JobId {job_id:?}, got {read:?}"
        );
        for member in ["DisplayName", "Fields", "UniqueKey"] {
            assert_eq!(
                read.get(member),
                body.get(member),
                "case {case:?}: expected the read to report {member} as the create stored \
                 it ({:?}), got {:?}",
                body.get(member),
                read.get(member)
            );
        }
        assert_eq!(
            read.get("DataExpiry").and_then(Value::as_i64),
            Some(expected_expiry),
            "case {case:?}: expected DataExpiry {expected_expiry}, got {read:?}"
        );
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some(CREATED),
            "case {case:?}: a job nobody started must report {CREATED}, got {read:?}"
        );
        // Nothing has processed this job, so it reports no completion and no reason.
        for member in ["CompletedAt", "StatusReason"] {
            assert_eq!(
                read.get(member),
                None,
                "case {case:?}: a job nobody started must not report {member}, got {read:?}"
            );
        }
        // `ResultsSummary` is part of the stored job, so a read of it always reports the
        // structure; it is the counters inside that a job nobody started leaves unset.
        // Asserted in full by `upload_job_results_summary`.
        assert!(
            read.get("ResultsSummary").is_some_and(Value::is_object),
            "case {case:?}: expected a ResultsSummary object, got {read:?}"
        );
        assert!(
            read.get("CreatedAt").is_some(),
            "case {case:?}: expected CreatedAt, got {read:?}"
        );

        // The read changed nothing a later caller sees.
        let (status, again) = emulator.get_upload_job("uploaddom", &job_id).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: two reads of an unchanged job must be identical, got {read:?} \
             then {again:?}"
        );
    }
}

/// A job belongs to the domain it was created in: a read of the same id through another
/// domain does not find it, and each domain reports only its own jobs.
#[tokio::test]
async fn a_job_is_reachable_only_through_the_domain_that_holds_it() {
    let emulator = Emulator::new();
    emulator.create_domain("owningdom", 30).await;
    emulator.create_domain("otherdom", 30).await;

    let request = json!({
        "DisplayName": "scoped.csv",
        "Fields": upload_job_fields("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
        "UniqueKey": "email",
    });
    let (status, response) = emulator.create_upload_job("owningdom", request.clone()).await;
    assert_eq!(
        status, 200,
        "case \"the create\": expected 200, got {status} {response:?}"
    );
    let job_id = response
        .get("JobId")
        .and_then(Value::as_str)
        .expect("a minted JobId")
        .to_string();

    let (status, read) = emulator.get_upload_job("owningdom", &job_id).await;
    assert_eq!(
        status, 200,
        "case \"a read through the owning domain\": expected 200, got {status} {read:?}"
    );

    let (status, read) = emulator.get_upload_job("otherdom", &job_id).await;
    assert_eq!(
        (status, error_code(&read)),
        (404, "ResourceNotFoundException"),
        "case \"a read through another domain\": expected 404, got {status} {read:?}"
    );
}

/// The refusals: a domain that is not stored, a job id the domain does not hold, and
/// input outside a modelled constraint — each bound of `DataExpiry` and `DisplayName`,
/// one past each, and a missing required member. Every refused create persists nothing.
#[tokio::test]
async fn the_upload_job_operations_refuse_an_absent_resource_and_invalid_input() {
    let emulator = Emulator::new();
    emulator.create_domain("boundsjobdom", 30).await;

    let valid = json!({
        "DisplayName": "valid.csv",
        "Fields": upload_job_fields("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
        "UniqueKey": "email",
    });

    let (status, body) = emulator.create_upload_job("nosuchdomain", valid.clone()).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a create in a domain that is not stored\": expected 404, got {status} {body:?}"
    );

    // A well-formed id (32 hex characters) no create minted.
    let absent_id = "0".repeat(32);
    let (status, body) = emulator.get_upload_job("nosuchdomain", &absent_id).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a read in a domain that is not stored\": expected 404, got {status} {body:?}"
    );

    let (status, body) = emulator.get_upload_job("boundsjobdom", &absent_id).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a read of a job the domain does not hold\": expected 404, got {status} {body:?}"
    );

    // `DisplayName` is `string1To255`, len 1..255; `UniqueKey` is `text`, len 1..1000;
    // `DataExpiry` is `expirationDaysInteger`, range 1..1098; a `FieldMap` value is an
    // `ObjectTypeField` whose `ContentType` is the `FieldContentType` enum.
    let constraints: Vec<(&str, Value, u16)> = vec![
        (
            "a DisplayName at the minimum length",
            json!({ "DisplayName": "d", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email" }),
            200,
        ),
        (
            "a DisplayName one past the maximum length",
            json!({ "DisplayName": "n".repeat(256), "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email" }),
            400,
        ),
        (
            "a DisplayName below the minimum length",
            json!({ "DisplayName": "", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email" }),
            400,
        ),
        (
            "no DisplayName at all",
            json!({ "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email" }),
            400,
        ),
        (
            "no Fields at all",
            json!({ "DisplayName": "nofields.csv", "UniqueKey": "email" }),
            400,
        ),
        (
            "no UniqueKey at all",
            json!({ "DisplayName": "nokey.csv", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress") }),
            400,
        ),
        (
            "a UniqueKey below the minimum length",
            json!({ "DisplayName": "emptykey.csv", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "" }),
            400,
        ),
        (
            "a UniqueKey one past the maximum length",
            json!({ "DisplayName": "longkey.csv", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "k".repeat(1001) }),
            400,
        ),
        (
            "a DataExpiry below the modelled minimum",
            json!({ "DisplayName": "zeroexpiry.csv", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email", "DataExpiry": 0 }),
            400,
        ),
        (
            "a DataExpiry one past the modelled maximum",
            json!({ "DisplayName": "pastexpiry.csv", "Fields": upload_job_fields("email", "STRING", "_profile.EmailAddress"), "UniqueKey": "email", "DataExpiry": 1099 }),
            400,
        ),
        (
            "a ContentType outside the modelled enum",
            json!({ "DisplayName": "badtype.csv", "Fields": upload_job_fields("email", "TELEPHONE", "_profile.EmailAddress"), "UniqueKey": "email" }),
            400,
        ),
    ];

    for (case, body, expected_status) in constraints {
        let (status, response) = emulator.create_upload_job("boundsjobdom", body.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {body:?}, got {status} \
             {response:?}"
        );
        if expected_status == 200 {
            let job_id = response
                .get("JobId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                is_modelled_job_id(job_id),
                "case {case:?}: expected a 32-character lowercase hex JobId, got {response:?}"
            );
            continue;
        }
        assert_eq!(
            error_code(&response),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {response:?}"
        );
        assert_eq!(
            response.get("JobId"),
            None,
            "case {case:?}: a refused create must mint no JobId, got {response:?}"
        );
    }
}
