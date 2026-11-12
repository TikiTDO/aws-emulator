// SPDX-License-Identifier: CC0-1.0

//! Which misses `DeleteIntegrationResourceProperty` answers AT THE BOUNDS of the one member
//! it takes: the suite is named for the misses it asserts, and the dimension its rows span is
//! the declared length of `ResourceArn`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a value INSIDE the member's declared length is a well-formed address
//! and reaches the record store, so one no record is keyed under is the delete's modelled
//! `EntityNotFoundException` carrying the record-miss sentence; a value past either bound is
//! a MALFORMED REQUEST, refused by the request boundary in AWS's constraint wording before
//! anything is looked up. Which values the lookup then misses, whatever resource kind they
//! name, is the table in [`crate::integration_resource_properties`], whose wording helper
//! this suite shares so the two cannot disagree about the sentence they pin.
//!
//! The rows differ along what the rule generalises over: each published bound of the member
//! and one value past it.

use serde_json::{json, Value};

use crate::integration_resource_properties::{connection_arn, record_miss_message};
use crate::wire::{call, emulator, error_code, error_message};

/// The bounds the member declares: "len 1..512" on `String512`
/// (`smithy_query operation=DeleteIntegrationResourceProperty`).
const MAX_RESOURCE_ARN_LENGTH: usize = 512;
const MIN_RESOURCE_ARN_LENGTH: usize = 1;

/// The member path a constraint clause names `ResourceArn` by.
const RESOURCE_ARN_MEMBER_PATH: &str = "resourceArn";

/// The clause a `ResourceArn` outside the member's declared length earns: the crate's one
/// constraint-refusal envelope ([`crate::refusal_classification::one_clause`]) over the value
/// as the walk quotes it — the row one byte past the bound is 513 bytes, so what the clause
/// carries for it is the elided form — the member path it was sent at and the bound it had to
/// satisfy.
fn expected_length_clause(value: &str, requirement: &str) -> String {
    crate::refusal_classification::one_clause(
        &::service_common::validation::quoted_echo_up_to(
            value,
            crate::refusal_classification::MAX_ECHOED_VALUE,
        ),
        RESOURCE_ARN_MEMBER_PATH,
        requirement,
    )
}

/// Delete one record and answer the HTTP status and body.
async fn delete_property(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteIntegrationResourceProperty",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await
}

/// The longest ARN the member admits reaches the lookup, so it is the delete's record miss,
/// while one byte past that bound and one byte short of the shortest admitted value are
/// refused by the request boundary instead — a different error class and a different
/// sentence, decided before any record is addressed.
#[tokio::test]
async fn the_resource_arn_length_bounds_decide_what_reaches_the_delete_lookup() {
    let router = emulator();

    let prefix = connection_arn("");
    let longest = format!(
        "{prefix}{}",
        "c".repeat(MAX_RESOURCE_ARN_LENGTH - prefix.len())
    );
    assert_eq!(
        longest.len(),
        MAX_RESOURCE_ARN_LENGTH,
        "case the longest admitted ARN: the row must sit on the bound, got {}",
        longest.len()
    );

    let (status, body) = delete_property(&router, &longest).await;
    assert_eq!(
        status, 400,
        "case the longest admitted ARN: expected the record miss, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case the longest admitted ARN: expected EntityNotFoundException, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        record_miss_message(&longest),
        "case the longest admitted ARN: expected {:?}, got {:?}",
        record_miss_message(&longest),
        error_message(&body)
    );

    // (case, the value, the bound it fails)
    let past_the_bounds: Vec<(&str, String, String)> = vec![
        (
            "one character past the longest admitted ARN",
            format!("{longest}c"),
            format!("Member must have length less than or equal to {MAX_RESOURCE_ARN_LENGTH}"),
        ),
        (
            "the empty value, one short of the shortest admitted",
            String::new(),
            format!("Member must have length greater than or equal to {MIN_RESOURCE_ARN_LENGTH}"),
        ),
    ];

    for (case, resource_arn, requirement) in &past_the_bounds {
        let (status, body) = delete_property(&router, resource_arn).await;
        assert_eq!(
            status, 400,
            "case {case}: a malformed value must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException, got {}",
            error_code(&body)
        );
        // A boundary refusal is carried in the error shape's lowercase `message` member,
        // not in the `Message` a modelled Glue error carries.
        let stated = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            stated,
            expected_length_clause(resource_arn, requirement),
            "case {case}: expected {:?}, got {stated:?}",
            expected_length_clause(resource_arn, requirement)
        );
    }
}
