// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAcmeDomainValidation` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: a delete of one ACME domain validation removes the stored record its ARN names
//! and nothing else — whatever status that record carries and however often the call is
//! repeated — so afterwards the ARN answers to no record at all (`DescribeAcmeDomainValidation`
//! and `ListTagsForResource` report it missing, `ListAcmeDomainValidations` no longer lists
//! it, and the endpoint may validate that domain again), while an ARN whose account segment
//! is another account's is refused as invalid input before any lookup and writes nothing.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT operation
//! than the write, so a response that is not computed from the persisted store cannot pass.
//! The wire harness, the endpoint and validation helpers and the clock helpers are shared
//! with [`crate::certificate_lifecycle`], [`crate::acme_endpoint_lifecycle`] and
//! [`crate::acme_domain_validation_lifecycle`].
//!
//! The table below spans what the rule generalises over: the status the validation stands
//! in when it is deleted (every value `AcmeDomainValidationStatus` declares), the domain
//! names and prevalidation configurations the shared case table covers, and tag sets of
//! several sizes — none of which the delete may consult.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
    settle_prevalidations_now, tag_case, validation_cases,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// Every `AcmeDomainValidationStatus` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidationStatus")`),
/// which is the set of states a validation can be standing in when a delete arrives.
const VALIDATION_STATUSES: &[&str] = &["VALIDATING", "VALID", "INVALID", "DELETING"];

/// `AcmeDomainValidationArn`'s modelled maximum length (`len 1..200`).
const MAX_VALIDATION_ARN_LENGTH: usize = 200;

/// `DeleteAcmeDomainValidation`, asserted to succeed with an empty body.
async fn delete_validation(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, answer) = call(
        router,
        "DeleteAcmeDomainValidation",
        &json!({ "AcmeDomainValidationArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAcmeDomainValidation({arn}) must succeed, got {status} {answer}"
    );
    assert!(
        answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: DeleteAcmeDomainValidation answers an empty body, got {answer}"
    );
}

/// Every validation ARN `acme_endpoint_arn` still holds, read back through
/// `ListAcmeDomainValidations` (one page: the endpoints here hold a handful).
async fn listed_validation_arns(
    router: &MultiServiceRouter,
    case: &str,
    acme_endpoint_arn: &str,
) -> Vec<String> {
    let (status, listed) = call(
        router,
        "ListAcmeDomainValidations",
        &json!({ "AcmeEndpointArn": acme_endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeDomainValidations must succeed, got {status} {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "case {case:?}: these endpoints hold one page of validations, got {listed}"
    );
    listed["AcmeDomainValidations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no AcmeDomainValidations in {listed}"))
        .iter()
        .filter_map(|summary| summary["AcmeDomainValidationArn"].as_str())
        .map(str::to_string)
        .collect()
}

/// Stand the stored validation `arn` in `status`, as the service's own clock would leave
/// it: nothing is left pending, so the status a reader sees is this one.
///
/// Used only for the statuses no call in this emulator reaches yet — nothing here can fail
/// a prevalidation or tear a validation down, so `INVALID` and `DELETING` have no wire path
/// (see `state::ACME_DOMAIN_VALIDATION_STATUS_INVALID`). The delete under test still goes
/// over the wire and still reads the status from the store.
fn stand_in_status(engine: &Arc<engine::InMemoryEngine>, arn: &str, status: &str) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let validation = store
        .acme_domain_validations
        .get_mut(arn)
        .unwrap_or_else(|| panic!("no stored ACME domain validation at {arn}"));
    validation.pending_status = None;
    validation.status = status.to_string();
}

/// Leave the stored validation `arn` standing in `status` before the delete under test:
/// through the clock where a wire path reaches that status, and by writing the record where
/// none does.
fn reach_status(engine: &Arc<engine::InMemoryEngine>, arn: &str, status: &str) {
    if status == state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING {
        // The transient state the create leaves, pushed out of the test's way so the wall
        // clock cannot settle it mid-row.
        hold_prevalidations(engine);
    } else if status == state::ACME_DOMAIN_VALIDATION_STATUS_VALID {
        // The transition the create already owes, dated into the past so the next request
        // materialises it.
        settle_prevalidations_now(engine);
    } else {
        stand_in_status(engine, arn, status);
    }
}

/// RULE: the delete removes the record its ARN names whatever status that record stands in,
/// leaves the endpoint's other validations alone, is a success when repeated, and frees the
/// domain for a fresh validation.
#[tokio::test]
async fn deleting_a_validation_removes_it_whatever_status_it_carries() {
    let (router, engine) = router_with_engine();

    for (index, (row, domain, options)) in validation_cases().into_iter().enumerate() {
        let status = VALIDATION_STATUSES[index % VALIDATION_STATUSES.len()];
        let case = format!("{row}, deleted while {status}");

        // One endpoint per row, holding a sibling validation the row never deletes: the
        // call must act on the one ARN it names.
        let endpoint_arn = create_endpoint(&router, &case, &endpoint_body()).await;
        let sibling = create_validation(
            &router,
            &case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": "sibling.example.com",
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        )
        .await;

        let mut body = json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": domain,
            "PrevalidationOptions": options,
        });
        if let Some(tags) = tag_case(index) {
            body["Tags"] = tags;
        }
        let arn = create_validation(&router, &case, &body).await;

        reach_status(&engine, &arn, status);
        let before = describe_validation(&router, &case, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(status),
            "case {case:?}: expected the validation to stand in {status} before the delete, got {before}"
        );

        delete_validation(&router, &case, &arn).await;

        // The ARN answers to no record: the read goes through a different operation than
        // the delete, so a response that is not computed from the store cannot pass.
        let (status_code, refusal) = call(
            &router,
            "DescribeAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": arn }),
        )
        .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: a deleted validation must be reported missing, got {status_code} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException after the delete, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            state::acme_domain_validation_not_found_message(&arn),
            "case {case:?}: expected the service's not-found sentence, got {refusal}"
        );

        // The tags the validation carried went with the record.
        let (tag_status, tag_answer) =
            call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            tag_status, 400,
            "case {case:?}: a deleted validation holds no tags to list, got {tag_status} {tag_answer}"
        );
        assert_eq!(
            error_code(&tag_answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for the deleted ARN's tags, got {tag_answer}"
        );

        // The endpoint no longer lists it, and still lists the sibling.
        let remaining = listed_validation_arns(&router, &case, &endpoint_arn).await;
        assert_eq!(
            remaining,
            vec![sibling.clone()],
            "case {case:?}: expected only {sibling} to remain, got {remaining:?}"
        );

        // The operation is `@idempotent`: a repeated delete of a validation that is already
        // gone is the state it asks for, so it succeeds and still writes nothing.
        delete_validation(&router, &format!("{case} repeated"), &arn).await;
        let repeated = listed_validation_arns(&router, &case, &endpoint_arn).await;
        assert_eq!(
            repeated,
            vec![sibling.clone()],
            "case {case:?}: a repeated delete must write nothing, got {repeated:?}"
        );

        // The domain is free again: a create the endpoint would have refused as a conflict
        // while the validation stood now succeeds, with an ARN of its own.
        let again = create_validation(&router, &case, &body).await;
        assert_ne!(
            again, arn,
            "case {case:?}: the re-created validation must carry its own ARN, got {again}"
        );
        let mut expected = vec![again.clone(), sibling.clone()];
        expected.sort();
        let mut now_listed = listed_validation_arns(&router, &case, &endpoint_arn).await;
        now_listed.sort();
        assert_eq!(
            now_listed, expected,
            "case {case:?}: expected the endpoint to hold the sibling and the re-created \
             validation, got {now_listed:?}"
        );
    }
}

/// RULE: an ARN whose account segment is another account's names nothing the caller could
/// delete, so the call is refused as invalid input (`ValidationException`, HTTP 400) before
/// any lookup, states the verdict for the resource type the member's own pattern fixes,
/// withholds the submitted ARN, and writes nothing.
///
/// The rows differ along what the rule generalises over: the Region, the partition and the
/// account id of the foreign ARN.
#[tokio::test]
async fn deleting_a_validation_of_another_account_is_refused_and_writes_nothing() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "owned host", &endpoint_body()).await;
    let owned = create_validation(
        &router,
        "owned validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "owned.example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    hold_prevalidations(&engine);

    let foreign: &[(&str, String)] = &[
        (
            "another account in this Region",
            format!(
                "arn:aws:acm:{REGION}:111122223333:acme-endpoint/\
                 2b260b3a-a1db-47b2-98dd-b80925898547/acme-domain-validation/\
                 7c4e4a9c-7f1e-4b1a-9c3a-6d2f0b1a5e11"
            ),
        ),
        (
            "another account in another Region",
            "arn:aws:acm:eu-central-1:210987654321:acme-endpoint/\
             33333333-3333-4333-8333-333333333333/acme-domain-validation/\
             44444444-4444-4444-8444-444444444444"
                .to_string(),
        ),
        (
            "another account in another partition",
            "arn:aws-us-gov:acm:us-gov-west-1:444455556666:acme-endpoint/55555555/\
             acme-domain-validation/66666666"
                .to_string(),
        ),
        (
            "an account of all nines",
            format!("arn:aws:acm:{REGION}:999999999999:acme-endpoint/a/acme-domain-validation/b"),
        ),
    ];

    for (case, arn) in foreign {
        let (status, refusal) = call(
            &router,
            "DeleteAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {arn} must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: expected ValidationException for {arn}, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            !message.contains(arn.as_str()),
            "case {case:?}: a cross-account refusal must withhold the submitted ARN, got {refusal}"
        );
        assert_eq!(
            message,
            crate::acme_authorization::cross_account_arn_message("acme-domain-validation"),
            "case {case:?}: expected the cross-account verdict for the validation's own \
             resource type, got {refusal}"
        );
    }

    // None of those refusals wrote anything: the caller's own validation still stands, in
    // the status it was created in.
    let record = describe_validation(&router, "owned validation", &owned).await;
    assert_eq!(
        record["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
        "expected the owned validation to be untouched, got {record}"
    );
    let remaining = listed_validation_arns(&router, "owned endpoint", &endpoint_arn).await;
    assert_eq!(
        remaining,
        vec![owned.clone()],
        "expected the refusals to have deleted nothing, got {remaining:?}"
    );
}

/// RULE: an ARN of the caller's own account that no record answers to is already in the
/// state the `@idempotent` delete asks for, so the call succeeds and writes nothing; an ARN
/// past the member's modelled length is invalid input, not a missing resource.
///
/// The rows differ along what the rule generalises over: whether the endpoint the ARN hangs
/// under exists, the Region it names, and the ARN's length — at the modelled bound and one
/// byte past it.
#[tokio::test]
async fn deleting_a_validation_that_no_record_answers_to_succeeds() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "idempotent host", &endpoint_body()).await;
    let owned = create_validation(
        &router,
        "owned validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "kept.example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    hold_prevalidations(&engine);

    // An ARN of exactly `AcmeDomainValidationArn`'s maximum modelled length, built by
    // padding the validation id of a well-formed ARN.
    let at_bound = {
        let prefix =
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/e/acme-domain-validation/");
        format!(
            "{prefix}{}",
            "a".repeat(MAX_VALIDATION_ARN_LENGTH - prefix.len())
        )
    };
    assert_eq!(
        at_bound.len(),
        MAX_VALIDATION_ARN_LENGTH,
        "the bound row must sit exactly on the modelled maximum, got {at_bound}"
    );

    let gone: &[(&str, String)] = &[
        (
            "an id under an endpoint this account holds",
            format!("{endpoint_arn}/acme-domain-validation/00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an endpoint this account never created",
            format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/\
                 11111111-1111-4111-8111-111111111111/acme-domain-validation/22222222"
            ),
        ),
        (
            "another Region of this account",
            format!("arn:aws:acm:eu-west-1:{ACCOUNT}:acme-endpoint/a/acme-domain-validation/b"),
        ),
        ("an ARN at the member's maximum length", at_bound.clone()),
    ];

    for (case, arn) in gone {
        delete_validation(&router, case, arn).await;
    }

    // One byte past the bound is a constraint violation, not a resource that is already
    // gone — and never the scaffold's refusal.
    let past_bound = format!("{at_bound}a");
    let (status, refusal) = call(
        &router,
        "DeleteAcmeDomainValidation",
        &json!({ "AcmeDomainValidationArn": past_bound }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {past_bound:?}: an ARN past the modelled length must be refused, got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ValidationException",
        "case {past_bound:?}: expected ValidationException past the modelled length, got {refusal}"
    );

    // Nothing the caller holds was touched by any of it.
    let remaining = listed_validation_arns(&router, "idempotent host", &endpoint_arn).await;
    assert_eq!(
        remaining,
        vec![owned.clone()],
        "expected the deletes of absent ARNs to leave the stored validation alone, got {remaining:?}"
    );
    let record = describe_validation(&router, "owned validation", &owned).await;
    assert_eq!(
        record["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
        "expected the stored validation to be untouched, got {record}"
    );
}
