// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE ACM refuses a cross-account resource identifier
//! with, across every ARN input member of its ACME surface.
//!
//! RULE: an ARN input member whose account segment is not the caller's is refused as
//! invalid input (`ValidationException`, HTTP 400) with ACM's identifier-validation
//! envelope — `Resource identifier for <resource type> has a validation error: ` followed
//! by the verdict on the ARN — where the one request-derived token is the resource TYPE
//! the offending ARN carries, and nothing else of the request (its account id, its ARN,
//! its domain name) appears in the message. That token is what tells a client WHICH
//! identifier member of a multi-ARN request was rejected, since every such refusal of
//! this service carries the one modelled error code.
//!
//! The rows differ along what the rule generalises over: the ARN member and so the
//! resource type it names (the three the ACME surface addresses — `acme-endpoint`,
//! `acme-domain-validation`, `acme-external-account-binding`), the operation reached
//! (reads, writes, deletes, creates and listings), and the shape of the foreign ARN
//! (partition, Region and account id).
//!
//! The contrast rows pin that the envelope is a verdict on the ARN rather than on the
//! store: an ARN of the caller's OWN account that no record answers to is the
//! missing-resource class, and the caller's own endpoint and validation are untouched by
//! the whole table, read back through a different operation than the one that wrote them.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on. The wire harness and the endpoint/validation
//! helpers are shared with [`crate::certificate_lifecycle`],
//! [`crate::acme_endpoint_lifecycle`] and [`crate::acme_domain_validation_lifecycle`].

use serde_json::{json, Value};

use crate::acme_authorization::cross_account_arn_message;
use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// The three resource types the ACME surface's ARN members address, as each member's own
/// modelled `@pattern` fixes them
/// (`smithy_query(query="operation", operation="DescribeAcmeEndpoint,DescribeAcmeDomainValidation,DescribeAcmeExternalAccountBinding")`):
/// `acme-endpoint/<id>`, `acme-endpoint/<id>/acme-domain-validation/<id>` and
/// `acme-endpoint/<id>/acme-external-account-binding/<id>`.
const ENDPOINT_TYPE: &str = "acme-endpoint";
const DOMAIN_VALIDATION_TYPE: &str = "acme-domain-validation";
const EXTERNAL_ACCOUNT_BINDING_TYPE: &str = "acme-external-account-binding";

/// One foreign-ARN shape: what it exercises, and the partition, Region and account id the
/// ARNs built for it carry. The account id is never this emulator's
/// ([`crate::certificate_lifecycle::ACCOUNT`]), which is what makes every ARN below a
/// cross-account one while still satisfying each member's modelled pattern.
type ForeignAccount<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The foreign accounts the table spans: the same partition and Region as the caller, a
/// neighbouring Region, another partition entirely, and an account id of all nines.
const FOREIGN_ACCOUNTS: &[ForeignAccount<'static>] = &[
    (
        "a neighbouring account in the caller's Region",
        "aws",
        REGION,
        "111122223333",
    ),
    (
        "another account in another Region",
        "aws",
        "eu-central-1",
        "210987654321",
    ),
    (
        "another account in another partition",
        "aws-us-gov",
        "us-gov-west-1",
        "444455556666",
    ),
    (
        "another account in the China partition",
        "aws-cn",
        "cn-north-1",
        "000000000001",
    ),
    ("an account of all nines", "aws", REGION, "999999999999"),
];

/// The ids the ARNs below carry, each satisfying its member's `[a-zA-Z0-9-]+` id charset.
const ENDPOINT_ID: &str = "22222222-2222-4222-8222-222222222222";
const VALIDATION_ID: &str = "33333333-3333-4333-8333-333333333333";
const BINDING_ID: &str = "44444444-4444-4444-8444-444444444444";

/// An ACME endpoint ARN in `account`, of the form `AcmeEndpointArn`'s pattern requires.
fn endpoint_arn_in(partition: &str, region: &str, account: &str) -> String {
    format!("arn:{partition}:acm:{region}:{account}:{ENDPOINT_TYPE}/{ENDPOINT_ID}")
}

/// Every request this suite sends with a cross-account ARN: the operation, the body
/// carrying the ARN, the resource type that ARN names, and the ARN itself.
fn requests_for(foreign: &ForeignAccount<'_>) -> Vec<(&'static str, Value, &'static str, String)> {
    let (_case, partition, region, account) = *foreign;
    let endpoint = endpoint_arn_in(partition, region, account);
    let validation = format!("{endpoint}/{DOMAIN_VALIDATION_TYPE}/{VALIDATION_ID}");
    let binding = format!("{endpoint}/{EXTERNAL_ACCOUNT_BINDING_TYPE}/{BINDING_ID}");
    // An `AccountUrl` and a `RoleArn` the two-member operations need; neither is the
    // member under test.
    let account_url = format!("https://acme.example.com/{ENDPOINT_ID}/account/{BINDING_ID}");
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/acme-binding");

    vec![
        (
            "DescribeAcmeEndpoint",
            json!({ "AcmeEndpointArn": endpoint }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "UpdateAcmeEndpoint",
            json!({ "AcmeEndpointArn": endpoint, "Contact": "NOT_REQUIRED" }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "DeleteAcmeEndpoint",
            json!({ "AcmeEndpointArn": endpoint }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "CreateAcmeDomainValidation",
            json!({
                "AcmeEndpointArn": endpoint,
                "DomainName": "refused.example.com",
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "CreateAcmeExternalAccountBinding",
            json!({ "AcmeEndpointArn": endpoint, "RoleArn": role_arn }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "ListAcmeDomainValidations",
            json!({ "AcmeEndpointArn": endpoint }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "ListAcmeAccounts",
            json!({ "AcmeEndpointArn": endpoint }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "ListAcmeExternalAccountBindings",
            json!({ "AcmeEndpointArn": endpoint }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "DescribeAcmeAccount",
            json!({ "AcmeEndpointArn": endpoint, "AccountUrl": account_url }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "RevokeAcmeAccount",
            json!({ "AcmeEndpointArn": endpoint, "AccountUrl": account_url }),
            ENDPOINT_TYPE,
            endpoint.clone(),
        ),
        (
            "DescribeAcmeDomainValidation",
            json!({ "AcmeDomainValidationArn": validation }),
            DOMAIN_VALIDATION_TYPE,
            validation.clone(),
        ),
        (
            "UpdateAcmeDomainValidation",
            json!({
                "AcmeDomainValidationArn": validation,
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
            DOMAIN_VALIDATION_TYPE,
            validation.clone(),
        ),
        (
            "DeleteAcmeDomainValidation",
            json!({ "AcmeDomainValidationArn": validation }),
            DOMAIN_VALIDATION_TYPE,
            validation.clone(),
        ),
        (
            "DescribeAcmeExternalAccountBinding",
            json!({ "AcmeExternalAccountBindingArn": binding }),
            EXTERNAL_ACCOUNT_BINDING_TYPE,
            binding.clone(),
        ),
        (
            "GetAcmeExternalAccountBindingCredentials",
            json!({ "AcmeExternalAccountBindingArn": binding }),
            EXTERNAL_ACCOUNT_BINDING_TYPE,
            binding.clone(),
        ),
        (
            "RevokeAcmeExternalAccountBinding",
            json!({ "AcmeExternalAccountBindingArn": binding }),
            EXTERNAL_ACCOUNT_BINDING_TYPE,
            binding.clone(),
        ),
        (
            "DeleteAcmeExternalAccountBinding",
            json!({ "AcmeExternalAccountBindingArn": binding }),
            EXTERNAL_ACCOUNT_BINDING_TYPE,
            binding,
        ),
    ]
}

/// RULE: every operation that holds an ARN member to the caller's account refuses a
/// cross-account ARN with the identifier-validation envelope scoped to the resource type
/// that ARN carries, and carries nothing else of the request.
#[tokio::test]
async fn a_cross_account_arn_is_refused_in_the_member_scoped_envelope() {
    let (router, _engine) = router_with_engine();

    for foreign in FOREIGN_ACCOUNTS {
        let (account_case, _partition, _region, account) = *foreign;
        for (operation, body, resource_type, arn) in requests_for(foreign) {
            let case = format!("{operation} with {account_case}");
            let (status, refusal) = call(&router, operation, &body).await;
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
            assert_eq!(
                message,
                cross_account_arn_message(resource_type),
                "case {case:?}: expected the envelope scoped to {resource_type}, got {message:?}"
            );
            // The resource type is the ONLY thing the sentence takes from the request.
            assert!(
                !message.contains(arn.as_str()) && !message.contains(account),
                "case {case:?}: the refusal must withhold the submitted ARN and account, got \
                 {message:?}"
            );
        }
    }
}

/// RULE, the contrast: the envelope is a verdict on the ARN the request carried, not a
/// report about the store — an ARN of the caller's OWN account that no record answers to
/// is the missing-resource class instead, and none of the cross-account refusals touches
/// what the caller does hold.
#[tokio::test]
async fn an_own_account_arn_is_not_answered_with_the_cross_account_envelope() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "owned endpoint", &endpoint_body()).await;
    let validation_arn = create_validation(
        &router,
        "owned validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "owned.example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;

    // Every cross-account request of the table, against a caller that holds resources of
    // its own: not one of them may be served, and not one may write.
    for foreign in FOREIGN_ACCOUNTS {
        for (operation, body, resource_type, _arn) in requests_for(foreign) {
            let case = format!("{operation} beside an owned endpoint");
            let (_status, refusal) = call(&router, operation, &body).await;
            assert_eq!(
                error_message(&refusal),
                cross_account_arn_message(resource_type),
                "case {case:?}: expected the envelope scoped to {resource_type}, got {refusal}"
            );
        }
    }

    // Read back through a DIFFERENT operation than the one that wrote: the caller's own
    // endpoint still answers, and still holds exactly the one validation it was given.
    let described = describe_validation(&router, "owned validation", &validation_arn).await;
    assert_eq!(
        described["DomainName"].as_str().unwrap_or_default(),
        "owned.example.com",
        "the owned validation must survive the refusals, got {described}"
    );
    let (status, listed) = call(
        &router,
        "ListAcmeDomainValidations",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owned endpoint must still list its validations, got {status} {listed}"
    );
    let arns: Vec<&str> = listed["AcmeDomainValidations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["AcmeDomainValidationArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![validation_arn.as_str()],
        "expected exactly the caller's own validation, got {listed}"
    );

    // An ARN of this account that names no record is the missing-resource verdict, never
    // the cross-account envelope.
    let unknown = format!("arn:aws:acm:{REGION}:{ACCOUNT}:{ENDPOINT_TYPE}/{ENDPOINT_ID}");
    let (status, refusal) = call(
        &router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": unknown }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown endpoint of this account must be refused, got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ResourceNotFoundException",
        "expected the missing-resource class for {unknown}, got {refusal}"
    );
    assert_ne!(
        error_message(&refusal),
        cross_account_arn_message(ENDPOINT_TYPE),
        "an ARN of this account must not be answered with the cross-account envelope, got \
         {refusal}"
    );
}
