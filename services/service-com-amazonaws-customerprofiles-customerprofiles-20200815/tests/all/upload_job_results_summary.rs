// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two parts of the upload-job response envelope that are a
//! property of the RECORD rather than of one request.
//!
//! The rules under test:
//!
//! 1. A member whose target structure the service keeps for a resource is reported for
//!    every stored instance of it, even while every one of that structure's own optional
//!    members is unpopulated: the container's presence is the contract, its contents are
//!    state. `GetUploadJobResponse$ResultsSummary` (target structure `ResultsSummary`
//!    with `UpdatedRecords`, `CreatedRecords` and `FailedRecords`, each an `optionalLong`
//!    — "The summary of results for the upload job, including the number of updated,
//!    created, and failed records") is therefore an object on every read of a stored job,
//!    carrying no counter until the job has processed records.
//! 2. Each operation family reports an absent domain in the wording the service uses on
//!    that path; the upload-job operations word it "Domain with name <DomainName> could
//!    not be found", whose only variable part is the `DomainName` the request's URI label
//!    carried.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, error_message, upload_job_fields, Emulator};

/// The three `ResultsSummary` members, from `smithy_query(query="shape",
/// shape="ResultsSummary")`: `UpdatedRecords`, `CreatedRecords`, `FailedRecords`, each
/// target `optionalLong`.
const RESULTS_SUMMARY_COUNTERS: [&str; 3] = ["UpdatedRecords", "CreatedRecords", "FailedRecords"];

/// Every read of a stored job reports `ResultsSummary` as an object with none of its
/// three counters populated — over a table of creates differing in the size of the field
/// mapping, the unique key, the display name and whether `DataExpiry` is stated.
#[tokio::test]
async fn a_read_of_a_stored_job_always_reports_the_results_summary() {
    let emulator = Emulator::new();
    emulator.create_domain("summarydom", 30).await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "one mapped column and no DataExpiry",
            json!({
                "DisplayName": "contacts.csv",
                "Fields": upload_job_fields("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
                "UniqueKey": "email",
            }),
        ),
        (
            "a DataExpiry at the modelled minimum",
            json!({
                "DisplayName": "d",
                "Fields": upload_job_fields("phone", "PHONE_NUMBER", "_profile.PhoneNumber"),
                "UniqueKey": "phone",
                "DataExpiry": 1,
            }),
        ),
        (
            "a DataExpiry at the modelled maximum and a display name at its maximum length",
            json!({
                "DisplayName": "n".repeat(255),
                "Fields": upload_job_fields("account", "STRING", "_profile.AccountNumber"),
                "UniqueKey": "account",
                "DataExpiry": 1098,
            }),
        ),
        (
            "several mapped columns and a unique key at the maximum length",
            json!({
                "DisplayName": "wide.csv",
                "Fields": {
                    "first_name": { "ContentType": "NAME", "Source": "_source.first_name", "Target": "_profile.FirstName" },
                    "last_name": { "ContentType": "NAME", "Source": "_source.last_name", "Target": "_profile.LastName" },
                    "e-mail": { "ContentType": "EMAIL_ADDRESS", "Source": "_source.e-mail", "Target": "_profile.EmailAddress" },
                    "loyalty_id": { "ContentType": "NUMBER", "Source": "_source.loyalty_id", "Target": "_profile.Attributes.LoyaltyId" },
                },
                "UniqueKey": "k".repeat(1000),
                "DataExpiry": 90,
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, created) = emulator.create_upload_job("summarydom", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateUploadJob 200 for {body:?}, got {status} {created:?}"
        );
        let job_id = created
            .get("JobId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no JobId in {created:?}"))
            .to_string();

        let (status, read) = emulator.get_upload_job("summarydom", &job_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetUploadJob 200, got {status} {read:?}"
        );
        let summary = read.get("ResultsSummary");
        assert!(
            summary.is_some_and(Value::is_object),
            "case {case:?}: expected ResultsSummary to be reported as an object, got {:?} \
             in {read:?}",
            summary
        );
        let summary = summary.expect("the asserted ResultsSummary object");
        for counter in RESULTS_SUMMARY_COUNTERS {
            let value = summary.get(counter);
            assert!(
                value.is_none_or(Value::is_null),
                "case {case:?}: a job that has processed nothing must report {counter} \
                 unpopulated, got {value:?} in {summary:?}"
            );
        }

        // The read is a read: a second one reports the very same envelope.
        let (status, again) = emulator.get_upload_job("summarydom", &job_id).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: two reads of an unchanged job must be identical, got {read:?} \
             then {again:?}"
        );
    }
}

/// Both implemented upload-job operations report an absent domain with the wording the
/// service uses on that path, with the request's `DomainName` interpolated — over a table
/// of names spanning the modelled `name` shape (len 1..64, pattern ^[a-zA-Z0-9_-]+$).
#[tokio::test]
async fn the_upload_job_operations_word_an_absent_domain_as_aws_does() {
    let emulator = Emulator::new();
    // One stored domain, so the refusals below are about the name, not an empty store.
    emulator.create_domain("storeddom", 30).await;

    let valid = json!({
        "DisplayName": "absent-domain.csv",
        "Fields": upload_job_fields("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
        "UniqueKey": "email",
    });
    // A well-formed JobId (32 lowercase hex characters) no create minted.
    let absent_job_id = "0".repeat(32);

    let names: Vec<(&str, String)> = vec![
        ("a name at the modelled minimum length", "a".to_string()),
        ("a plain name", "nosuchdomain".to_string()),
        (
            "a name carrying the dash and underscore the pattern allows",
            "probe-cp_missing-01".to_string(),
        ),
        ("a name of digits only", "0123456789".to_string()),
        ("a name at the modelled maximum length", "d".repeat(64)),
        (
            "a name differing from a stored one only in case",
            "STOREDDOM".to_string(),
        ),
    ];

    for (case, name) in names {
        let expected = format!("Domain with name {name} could not be found");

        let (status, body) = emulator.create_upload_job(&name, valid.clone()).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (404, "ResourceNotFoundException", expected.as_str()),
            "case {case:?}: expected CreateUploadJob to refuse {name:?} with 404 \
             ResourceNotFoundException {expected:?}, got {status} {body:?}"
        );

        let (status, body) = emulator.get_upload_job(&name, &absent_job_id).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (404, "ResourceNotFoundException", expected.as_str()),
            "case {case:?}: expected GetUploadJob to refuse {name:?} with 404 \
             ResourceNotFoundException {expected:?}, got {status} {body:?}"
        );
    }

    // A stored domain that holds no such job is a different absence: the job is what is
    // missing, so the domain sentence must not be the answer.
    let (status, body) = emulator.get_upload_job("storeddom", &absent_job_id).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a job the stored domain does not hold\": expected 404, got {status} {body:?}"
    );
    assert!(
        !error_message(&body).starts_with("Domain with name "),
        "case \"a job the stored domain does not hold\": the absent resource is the job, \
         not the domain, got {body:?}"
    );
}
