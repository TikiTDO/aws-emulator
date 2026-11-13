// SPDX-License-Identifier: CC0-1.0

//! How ACM CLASSIFIES a refusal, and what a `SearchCertificates` result reports —
//! end to end over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`,
//! awsJson1_1).
//!
//! Four rules under test, each asserted over a table that spans what it generalises
//! over rather than the one request that exposed it:
//!
//! 1. `ListCertificateDomainValidations` answers every defect of its own request
//!    PARAMETERS — a `CertificateArn` that names no ACM certificate, a `NextToken`
//!    this service did not issue — with one code, `ValidationException` (HTTP 400).
//!    A well-formed ARN no record answers to is a different class and keeps
//!    `ResourceNotFoundException`.
//! 2. `UpdateCertificateOptions` applies only to a certificate ACM has ISSUED, and
//!    which class a request naming one in any other status draws is decided per
//!    `Options` member: a member directing issuance work still to come fails a
//!    constraint of the REQUEST (`ValidationException`), while one that asks only about
//!    the issued certificate's own record draws the certificate's processing state
//!    (`InvalidStateException`) — both HTTP 400, with nothing written.
//! 3. A union of this service's input carries exactly one member, and a request that
//!    sets more than one is refused with `ValidationException` (HTTP 400) whose
//!    message is built FROM the request: it names the union, states the
//!    exactly-one-member constraint and lists the members the request set, in the
//!    order it set them.
//! 4. A `SearchCertificates` result reports each X.509 attribute as the value the
//!    FILTER on that attribute selects on, so a certificate a filter kept reports the
//!    value that kept it.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`], the path the emulator
//! serves real traffic on, and every certificate is created with `RequestCertificate`
//! and cross-read with `DescribeCertificate`, so nothing here can pass on a value
//! that was not computed from the persisted record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, call_raw, describe, error_code, error_message, hold_pending_validations,
    request_certificate, router, router_with_engine, ACCOUNT, KEY_ALGORITHMS, REGION,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The one sentence this service reports for a pagination token it did not issue.
const INVALID_NEXT_TOKEN: &str = "Invalid next token.";

/// The one sentence ACM reports when `UpdateCertificateOptions` is asked to direct
/// issuance work for a certificate it has not issued.
const NOT_ISSUED_MESSAGE: &str = "Certificate is not issued";

/// The one sentence ACM reports when `UpdateCertificateOptions` asks only about the
/// record of a certificate it has not issued.
const NOT_ISSUED_OR_VALID_MESSAGE: &str = "The selected certificate is not Issued and/or Valid.";

/// The byte `ListCertificateDomainValidations` joins a `NextToken`'s two keys with.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The `KeyAlgorithm` `RequestCertificate` falls back to when a request names none
/// ("RSA is the default key algorithm for ACM certificates",
/// `RequestCertificateRequest$KeyAlgorithm` doc).
const DEFAULT_KEY_ALGORITHM: &str = "RSA_2048";

/// The refusal ACM states for a union a request set more than one member of: the
/// union's name, the exactly-one-member constraint, and the members present in
/// request order.
///
/// Spelled out here rather than imported, so the test pins the sentence a client
/// reads instead of mirroring whatever the implementation builds.
fn exactly_one_member(union_name: &str, members: &[&str]) -> String {
    format!(
        "Each {union_name} must contain exactly one member. A {union_name} in your \
         request contains the members [{}].",
        members.join(", ")
    )
}

/// Call `ListCertificateDomainValidations` and return `(status, body)`.
async fn list_validations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "ListCertificateDomainValidations", body).await
}

/// Call `UpdateCertificateOptions` for one certificate and return `(status, body)`.
async fn update_options(
    router: &MultiServiceRouter,
    arn: &str,
    options: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateCertificateOptions",
        &json!({ "CertificateArn": arn, "Options": options }),
    )
    .await
}

/// Send `FilterStatement` to `SearchCertificates` as the EXACT bytes `statement`,
/// so the order the request sets a union's members in is the test's and not a map's.
async fn search_raw(router: &MultiServiceRouter, statement: &str) -> (u16, Value) {
    call_raw(
        router,
        REGION,
        "SearchCertificates",
        format!("{{\"FilterStatement\":{statement}}}").into_bytes(),
    )
    .await
}

/// Every result one `SearchCertificates` filter statement reports, failing the test on
/// any error.
async fn search_results(router: &MultiServiceRouter, statement: &Value) -> Vec<Value> {
    let (status, response) = call(
        router,
        "SearchCertificates",
        &json!({ "FilterStatement": statement }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {statement}: SearchCertificates must succeed, got {status} {response}"
    );
    response["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {statement}: the response carried no Results: {response}"))
        .clone()
}

/// RULE 1: every defect of `ListCertificateDomainValidations`' own request parameters
/// is one refusal class with one code — `ValidationException` (HTTP 400) — whether the
/// defect is in the `CertificateArn` or in the `NextToken`.
#[tokio::test]
async fn bad_parameters_of_a_domain_validation_listing_are_one_validation_exception() {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "classify.example.com",
            "SubjectAlternativeNames": ["alt.classify.example.com"],
            "ValidationMethod": "DNS",
        }),
    )
    .await;
    let other = request_certificate(
        &router,
        &json!({ "DomainName": "other.classify.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    hold_pending_validations(&engine);

    // Each row satisfies the member's modelled length (20..2048) and `@pattern`
    // (`^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:certificate/[\w+=,.@-]+`, anchored at
    // the start only), so every one reaches the handler. The rows differ along the ways
    // what follows a certificate ARN's prefix can stop it naming a certificate: a
    // nested resource, a deeper one, a trailing separator, and a seventh ARN field.
    let malformed: &[(&str, String)] = &[
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra"),
        ),
        (
            "a resource nested two levels below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra/deeper"),
        ),
        (
            "a trailing resource separator",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/"),
        ),
        (
            "a seventh ARN field",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef:extra"),
        ),
    ];
    for (case, bad_arn) in malformed {
        let (status, response) =
            list_validations(&router, &json!({ "CertificateArn": bad_arn })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for {bad_arn}, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            format!("{bad_arn} is not a valid ARN"),
            "case {case}: expected the invalid-ARN sentence, got {message:?}"
        );
    }

    // The token the service itself issued for the first page, which the rows below
    // mutate. Every row satisfies `NextToken`'s modelled length and `@pattern`, so each
    // reaches the handler's decode.
    let (status, response) =
        list_validations(&router, &json!({ "CertificateArn": &arn, "MaxItems": 1 })).await;
    assert_eq!(
        status, 200,
        "a MaxItems=1 page must succeed, got {status} {response}"
    );
    let issued = response["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that stopped short must issue a NextToken: {response}"))
        .to_string();

    // The rows differ along the ways a token can fail to be one this service issued for
    // this certificate: not hex, hex of a value with no position in it, a token issued
    // for another certificate, a name this certificate does not cover, an empty name,
    // and the issued token truncated.
    let forged: &[(&str, String)] = &[
        ("not hex at all", "not-a-token".to_string()),
        ("hex with no separator", hex::encode("classify.example.com")),
        (
            "a token issued for another certificate",
            hex::encode(format!("{other}{CURSOR_SEPARATOR}other.classify.example.com")),
        ),
        (
            "a name this certificate does not cover",
            hex::encode(format!("{arn}{CURSOR_SEPARATOR}absent.example.com")),
        ),
        ("an empty name", hex::encode(format!("{arn}{CURSOR_SEPARATOR}"))),
        (
            "the issued token truncated",
            issued[..issued.len() - 2].to_string(),
        ),
    ];
    for (case, token) in forged {
        let (status, response) =
            list_validations(&router, &json!({ "CertificateArn": &arn, "NextToken": token })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for {token:?}, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, INVALID_NEXT_TOKEN,
            "case {case}: expected {INVALID_NEXT_TOKEN:?}, got {message:?}"
        );
    }

    // A well-formed certificate ARN with no record behind it is a DIFFERENT class and
    // keeps its own code, so the rule above did not swallow the lookup failure.
    let absent = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555");
    let (status, response) = list_validations(&router, &json!({ "CertificateArn": &absent })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case absent certificate: expected 400 ResourceNotFoundException, got {status} {response}"
    );

    // The refusals left the certificate alone: the issued token still resumes.
    let (status, response) =
        list_validations(&router, &json!({ "CertificateArn": &arn, "NextToken": issued })).await;
    assert_eq!(
        status, 200,
        "the issued token must still resume the walk, got {status} {response}"
    );
    let names: Vec<&str> = response["DomainValidationSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("the resumed page carried no list: {response}"))
        .iter()
        .map(|summary| summary["DomainName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["alt.classify.example.com"],
        "the resumed page carries the name after the one the token named, got {names:?}"
    );
}

/// RULE 2: `UpdateCertificateOptions` applies only to a certificate ACM has ISSUED, and
/// which class a request naming one in any other status draws is decided per `Options`
/// member — a member that directs issuance work ACM has yet to perform fails a
/// constraint of the REQUEST (`ValidationException`), one that asks only about the
/// issued certificate's own record reports the certificate's processing state
/// (`InvalidStateException`) — both HTTP 400, with nothing written.
#[tokio::test]
async fn updating_the_options_of_a_certificate_that_is_not_issued_is_refused_per_member() {
    // The rows differ along the statuses a stored certificate can be in other than
    // ISSUED, along the `Options` member the request asks for, and so along the class
    // the refusal carries.
    let cases: &[(&str, Value, bool, &str, Value, &str, &str)] = &[
        (
            "a public certificate still being validated",
            json!({ "DomainName": "pending.classify.example.com", "ValidationMethod": "EMAIL" }),
            true,
            "PENDING_VALIDATION",
            json!({ "ValidationMethod": "DNS" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a private certificate still being validated",
            json!({
                "DomainName": "pending.classify.example.org",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            true,
            "PENDING_VALIDATION",
            json!({ "CertificateTransparencyLoggingPreference": "ENABLED" }),
            "InvalidStateException",
            NOT_ISSUED_OR_VALID_MESSAGE,
        ),
        (
            "a public certificate whose validation timed out",
            json!({ "DomainName": "timedout.classify.probe-domain.net", "ValidationMethod": "DNS" }),
            false,
            "VALIDATION_TIMED_OUT",
            json!({ "ValidationMethod": "HTTP" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a failed certificate asked for the export option it already has",
            json!({ "DomainName": "failed.classify.invalid", "ValidationMethod": "EMAIL" }),
            false,
            "FAILED",
            json!({ "Export": "DISABLED" }),
            "ValidationException",
            NOT_ISSUED_MESSAGE,
        ),
        (
            "a failed certificate asked only about its transparency logging",
            json!({ "DomainName": "failed.classify.localhost", "ValidationMethod": "HTTP" }),
            false,
            "FAILED",
            json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
            "InvalidStateException",
            NOT_ISSUED_OR_VALID_MESSAGE,
        ),
    ];

    for (case, body, hold, expected_status, options, expected_code, expected_message) in cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        if *hold {
            hold_pending_validations(&engine);
        } else {
            make_validation_due_now(&engine);
        }

        let before = describe(&router, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(*expected_status),
            "case {case}: the fixture must be {expected_status}, got {before}"
        );

        let (status, response) = update_options(&router, &arn, options).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case}: expected 400 {expected_code} for a {expected_status} \
             certificate, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, *expected_message,
            "case {case}: expected {expected_message:?}, got {message:?}"
        );
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the record untouched, got {after}"
        );
    }

    // The same rule once a certificate has LEFT the issued status: the update that
    // succeeded while it was issued is refused with the same code after a revocation.
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "revoked.classify.example.com",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    make_validation_due_now(&engine);
    let (status, response) =
        update_options(&router, &arn, &json!({ "ValidationMethod": "EMAIL" })).await;
    assert_eq!(
        status, 200,
        "case while issued: the update must succeed, got {status} {response}"
    );
    mark_exported(&engine, &arn);
    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": &arn, "RevocationReason": "KEY_COMPROMISE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case revoke: RevokeCertificate must succeed, got {status} {response}"
    );
    let (status, response) =
        update_options(&router, &arn, &json!({ "ValidationMethod": "HTTP" })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case once revoked: expected 400 ValidationException, got {status} {response}"
    );
}

/// RULE 3: a union of this service's input carries exactly one member; a request that
/// sets more than one is refused with a message built from the request — the union's
/// name, the constraint, and the members present in the order the request set them.
#[tokio::test]
async fn a_union_with_more_than_one_member_names_the_members_the_request_set() {
    let router = router();
    let arn = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555");
    let leaf = format!("{{\"Filter\":{{\"CertificateArn\":\"{arn}\"}}}}");

    // The rows span every union of this service's input the model gives more than one
    // member (so every union a request CAN set two members of), two member orders of
    // one pair, a collision of three members, and a collision nested inside an `And`
    // list rather than at the top of the statement.
    let cases: &[(&str, String, &str, &[&str])] = &[
        (
            "And beside Or",
            format!("{{\"And\":[{leaf}],\"Or\":[{leaf}]}}"),
            "CertificateFilterStatement",
            &["And", "Or"],
        ),
        (
            "Or beside And, the other request order",
            format!("{{\"Or\":[{leaf}],\"And\":[{leaf}]}}"),
            "CertificateFilterStatement",
            &["Or", "And"],
        ),
        (
            "Not beside Filter",
            format!("{{\"Not\":{leaf},\"Filter\":{{\"CertificateArn\":\"{arn}\"}}}}"),
            "CertificateFilterStatement",
            &["Not", "Filter"],
        ),
        (
            "three members at once",
            format!("{{\"And\":[{leaf}],\"Not\":{leaf},\"Or\":[{leaf}]}}"),
            "CertificateFilterStatement",
            &["And", "Not", "Or"],
        ),
        (
            "a collision nested inside an And list",
            format!("{{\"And\":[{{\"Not\":{leaf},\"And\":[{leaf}]}}]}}"),
            "CertificateFilterStatement",
            &["Not", "And"],
        ),
        (
            "a CertificateFilter with an ARN and an attribute filter",
            format!(
                "{{\"Filter\":{{\"CertificateArn\":\"{arn}\",\
                 \"X509AttributeFilter\":{{\"KeyAlgorithm\":\"RSA_2048\"}}}}}}"
            ),
            "CertificateFilter",
            &["CertificateArn", "X509AttributeFilter"],
        ),
        (
            "an X509AttributeFilter with two attributes",
            "{\"Filter\":{\"X509AttributeFilter\":{\"KeyUsage\":\"DIGITAL_SIGNATURE\",\
             \"KeyAlgorithm\":\"RSA_2048\"}}}"
                .to_string(),
            "X509AttributeFilter",
            &["KeyUsage", "KeyAlgorithm"],
        ),
        (
            "an AcmCertificateMetadataFilter with two members",
            "{\"Filter\":{\"AcmCertificateMetadataFilter\":{\"Status\":\"ISSUED\",\
             \"InUse\":true}}}"
                .to_string(),
            "AcmCertificateMetadataFilter",
            &["Status", "InUse"],
        ),
    ];

    for (case, statement, union_name, members) in cases {
        let (status, response) = search_raw(&router, statement).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {response}"
        );
        let message = error_message(&response);
        let expected = exactly_one_member(union_name, members);
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }

    // A statement that sets exactly one member of every union it carries is served, so
    // the refusal above is about the arity and not about the shape of the request.
    let (status, response) = search_raw(&router, &leaf).await;
    assert_eq!(
        status, 200,
        "case one member per union: the search must succeed, got {status} {response}"
    );
}

/// RULE 4: a `SearchCertificates` result reports each X.509 attribute as the value the
/// FILTER on that attribute selects on, so a certificate a filter kept reports the
/// value that kept it.
#[tokio::test]
async fn a_search_result_reports_the_key_algorithm_its_filter_selects_on() {
    let (router, engine) = router_with_engine();

    // One certificate per `KeyAlgorithm` the model declares, plus one that names none
    // at all and so carries ACM's default — the rows span the whole value set rather
    // than the two the recorded case used.
    let mut stored: Vec<(String, String)> = Vec::new();
    for algorithm in KEY_ALGORITHMS {
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": format!("{}.algorithms.example.com", algorithm.to_lowercase()),
                "ValidationMethod": "DNS",
                "KeyAlgorithm": algorithm,
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
        )
        .await;
        stored.push(((*algorithm).to_string(), arn));
    }
    let defaulted = request_certificate(
        &router,
        &json!({
            "DomainName": "defaulted.algorithms.example.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    stored.push((DEFAULT_KEY_ALGORITHM.to_string(), defaulted));
    make_validation_due_now(&engine);

    for (algorithm, arn) in &stored {
        // The filter is the one the recorded case used: an ARN scope ANDed with the
        // key-algorithm attribute, so the result set is exactly this certificate.
        let statement = json!({
            "And": [
                { "Filter": { "CertificateArn": arn } },
                { "Filter": { "X509AttributeFilter": { "KeyAlgorithm": algorithm } } },
            ]
        });
        let results = search_results(&router, &statement).await;
        assert_eq!(
            results.len(),
            1,
            "case {algorithm}: the filter must keep exactly the one certificate, got {results:?}"
        );
        let result = &results[0];
        assert_eq!(
            result["CertificateArn"].as_str(),
            Some(arn.as_str()),
            "case {algorithm}: expected {arn}, got {result}"
        );
        // The attribute the filter selected on reports the value that selected it.
        assert_eq!(
            result["X509Attributes"]["KeyAlgorithm"].as_str(),
            Some(algorithm.as_str()),
            "case {algorithm}: expected the filtered algorithm, got {result}"
        );
        // And it is the SAME algorithm the certificate's own detail reports, which
        // states it as the X.509 descriptor (`RSA-2048`) rather than as the enum value.
        let detail = describe(&router, arn).await;
        let described = detail["KeyAlgorithm"]
            .as_str()
            .unwrap_or_else(|| panic!("case {algorithm}: no KeyAlgorithm in {detail}"));
        assert_eq!(
            described,
            algorithm.replacen('_', "-", 1),
            "case {algorithm}: expected the described algorithm to be the same one, \
             got {described:?}"
        );

        // A filter naming a DIFFERENT algorithm keeps this certificate out, so the
        // reported value is the record's and not an echo of the filter.
        let other_algorithm = KEY_ALGORITHMS
            .iter()
            .find(|candidate| *candidate != algorithm)
            .expect("the model declares more than one key algorithm");
        let mismatched = json!({
            "And": [
                { "Filter": { "CertificateArn": arn } },
                { "Filter": { "X509AttributeFilter": { "KeyAlgorithm": other_algorithm } } },
            ]
        });
        let results = search_results(&router, &mismatched).await;
        assert!(
            results.is_empty(),
            "case {algorithm}: a filter for {other_algorithm} must keep nothing, got {results:?}"
        );
    }
}
