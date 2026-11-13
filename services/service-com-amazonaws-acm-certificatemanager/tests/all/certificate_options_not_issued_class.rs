// SPDX-License-Identifier: CC0-1.0

//! Which MODELLED ERROR `UpdateCertificateOptions` refuses a certificate that is not
//! ISSUED with — end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: the options this operation changes are those of a certificate ACM
//! has ISSUED, and for a `CertificateArn` naming a record in any other
//! `CertificateStatus` the class of the refusal follows what the request asks for: a
//! member that directs issuance work ACM has yet to perform — the method it validates
//! the names by, the export it permits — is input this operation's constraints do not
//! admit, refused as a constraint failure of the REQUEST, `ValidationException` (HTTP
//! 400), while a request that asks only about the issued certificate's own record — the
//! deprecated transparency-logging preference — is admissible and is refused for the
//! certificate's processing state, `InvalidStateException` (HTTP 400). Each class has
//! one fixed sentence that names neither the certificate nor its status, and nothing is
//! written. The `Export` rule carries the request-constraint class under its own
//! sentence and is decided first, so the sentence is the only thing that tells those two
//! refusals apart — which is exactly what a client has to match on.
//!
//! The table spans what the rule generalises over: every non-ISSUED status a stored
//! certificate can be in, reached the way a request reaches it (a public request held
//! in its validation window, a name the public DNS delegates to nobody, a recorded
//! revocation), crossed with each `Options` member a request can name, several at once,
//! and an `Options` naming none. Every row reads the record back with
//! `DescribeCertificate` before and after the refusal, so nothing here can pass on a
//! response the store does not back.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router_with_engine,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The modelled error ACM refuses a request that fails a constraint of its own with
/// (`smithy_query(query="operation", operation="UpdateCertificateOptions")` lists it
/// among this operation's errors, at HTTP 400).
const REQUEST_CONSTRAINT_ERROR: &str = "ValidationException";

/// The modelled error ACM refuses an admissible request with when the certificate's
/// processing has reached a state the call cannot be served in
/// (`smithy_query(query="operation", operation="UpdateCertificateOptions")` lists it
/// among this operation's errors, at HTTP 400).
const PROCESSING_STATE_ERROR: &str = "InvalidStateException";

/// The one sentence `UpdateCertificateOptions` reports for a request that directs
/// issuance work for a certificate ACM has not issued: fixed text naming neither the
/// certificate nor its status, and terminated by no period.
const NOT_ISSUED: &str = "Certificate is not issued";

/// The one sentence `UpdateCertificateOptions` reports for a request that asks only
/// about the record of a certificate ACM has not issued: fixed text naming neither the
/// certificate nor its status, terminated by a period.
const NOT_ISSUED_OR_VALID: &str = "The selected certificate is not Issued and/or Valid.";

/// The one sentence `UpdateCertificateOptions` reports for a request that asks for an
/// `Export` value other than the one in force — "you cannot update the value of Export
/// after the the certificate is created" (`CertificateOptions$Export` doc).
const EXPORT_NOT_UPDATABLE: &str = "Export option for certificates cannot be updated";

/// Every `ValidationMethod` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`).
const VALIDATION_METHODS: &[&str] = &["DNS", "EMAIL", "HTTP"];

/// How a row's fixture reaches the status it asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixture {
    /// A public request held inside its validation window, so it is still pending.
    HeldPending,
    /// A request whose owed validation outcome is materialised, which the names it
    /// covers decide (a name the public DNS delegates to nobody fails, any other times
    /// out).
    Settled,
    /// An issued certificate carried all the way to a recorded revocation.
    Revoked,
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

/// Stand up a store holding one certificate in the status `fixture` reaches from
/// `body`, and return the router beside its ARN.
async fn certificate_in_status(
    case: &str,
    body: &Value,
    fixture: Fixture,
) -> (MultiServiceRouter, String) {
    let (router, engine) = router_with_engine();
    let arn = request_certificate(&router, body).await;
    match fixture {
        Fixture::HeldPending => hold_pending_validations(&engine),
        Fixture::Settled => make_validation_due_now(&engine),
        Fixture::Revoked => {
            make_validation_due_now(&engine);
            mark_exported(&engine, &arn);
            let (status, response) = call(
                &router,
                "RevokeCertificate",
                &json!({ "CertificateArn": &arn, "RevocationReason": "KEY_COMPROMISE" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the fixture's revocation must succeed, got {status} {response}"
            );
        }
    }
    (router, arn)
}

/// One row: what it exercises, the `RequestCertificate` body, how the fixture reaches
/// its status, the `CertificateStatus` it must then be in, the `Options` the refused
/// update asks for, and the modelled error and sentence that draws.
type NotIssuedCase<'a> = (&'a str, Value, Fixture, &'a str, Value, &'a str, &'a str);

/// The rows the class table runs over.
fn not_issued_cases() -> Vec<NotIssuedCase<'static>> {
    let mut cases: Vec<NotIssuedCase<'static>> = Vec::new();

    // Every target `ValidationMethod`, over a public request still inside its
    // validation window — the migration this operation documents, asked for too early.
    for method in VALIDATION_METHODS {
        cases.push((
            match *method {
                "DNS" => "a pending EMAIL request asked to migrate to DNS",
                "EMAIL" => "a pending EMAIL request asked for EMAIL again",
                _ => "a pending EMAIL request asked to migrate to HTTP",
            },
            json!({
                "DomainName": format!("pending-{}.probe-domain.net", method.to_lowercase()),
                "ValidationMethod": "EMAIL",
            }),
            Fixture::HeldPending,
            "PENDING_VALIDATION",
            json!({ "ValidationMethod": method }),
            REQUEST_CONSTRAINT_ERROR,
            NOT_ISSUED,
        ));
    }

    cases.push((
        "a pending private request asked for transparency logging",
        json!({
            "DomainName": "pending-logging.probe-domain.net",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
        Fixture::HeldPending,
        "PENDING_VALIDATION",
        json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
        PROCESSING_STATE_ERROR,
        NOT_ISSUED_OR_VALID,
    ));
    cases.push((
        "a pending request asked for the export value already in force",
        json!({
            "DomainName": "pending-export.probe-domain.net",
            "ValidationMethod": "HTTP",
        }),
        Fixture::HeldPending,
        "PENDING_VALIDATION",
        json!({ "Export": "DISABLED" }),
        REQUEST_CONSTRAINT_ERROR,
        NOT_ISSUED,
    ));
    cases.push((
        "a pending request whose Options names no member at all",
        json!({
            "DomainName": "pending-empty.probe-domain.net",
            "ValidationMethod": "DNS",
        }),
        Fixture::HeldPending,
        "PENDING_VALIDATION",
        json!({}),
        PROCESSING_STATE_ERROR,
        NOT_ISSUED_OR_VALID,
    ));
    cases.push((
        "a wildcard request that timed out, asked for two members at once",
        json!({
            "DomainName": "*.timedout.probe-domain.io",
            "SubjectAlternativeNames": ["timedout.probe-domain.io"],
            "ValidationMethod": "DNS",
        }),
        Fixture::Settled,
        "VALIDATION_TIMED_OUT",
        json!({
            "ValidationMethod": "EMAIL",
            "CertificateTransparencyLoggingPreference": "ENABLED",
        }),
        REQUEST_CONSTRAINT_ERROR,
        NOT_ISSUED,
    ));
    cases.push((
        // A name whose top-level label the public DNS delegates to nobody (RFC 6761),
        // so the request fails up front rather than waiting.
        "a request that failed, asked to migrate validation",
        json!({ "DomainName": "failed.invalid", "ValidationMethod": "EMAIL" }),
        Fixture::Settled,
        "FAILED",
        json!({ "ValidationMethod": "HTTP" }),
        REQUEST_CONSTRAINT_ERROR,
        NOT_ISSUED,
    ));
    cases.push((
        "a request that failed, asked only about transparency logging",
        json!({ "DomainName": "failed-logging.invalid", "ValidationMethod": "DNS" }),
        Fixture::Settled,
        "FAILED",
        json!({ "CertificateTransparencyLoggingPreference": "ENABLED" }),
        PROCESSING_STATE_ERROR,
        NOT_ISSUED_OR_VALID,
    ));
    cases.push((
        "a certificate whose revocation is recorded",
        json!({
            "DomainName": "revoked.probe-domain.net",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
        Fixture::Revoked,
        "REVOKED",
        json!({ "ValidationMethod": "EMAIL" }),
        REQUEST_CONSTRAINT_ERROR,
        NOT_ISSUED,
    ));
    cases.push((
        "a revoked certificate asked only about transparency logging",
        json!({
            "DomainName": "revoked-logging.probe-domain.net",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
        Fixture::Revoked,
        "REVOKED",
        json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
        PROCESSING_STATE_ERROR,
        NOT_ISSUED_OR_VALID,
    ));

    cases
}

/// RULE: every non-ISSUED status draws the SAME pair of refusals, decided by the
/// `Options` the request names and not by the status: the request-constraint class under
/// the not-issued sentence for a member that directs issuance work still to come, the
/// processing-state class under its own sentence for a request that asks only about the
/// certificate's record — and the refused call writes nothing.
#[tokio::test]
async fn a_certificate_that_is_not_issued_is_refused_in_the_class_its_options_decide() {
    for (case, body, fixture, expected_status, options, expected_code, expected_message) in
        not_issued_cases()
    {
        let (router, arn) = certificate_in_status(case, &body, fixture).await;

        let before = describe(&router, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(expected_status),
            "case {case}: the fixture must be {expected_status}, got {before}"
        );

        let (status, response) = update_options(&router, &arn, &options).await;
        let code = error_code(&response);
        let message = error_message(&response);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, expected_code, expected_message),
            "case {case}: a {expected_status} certificate must be refused \
             400 {expected_code} {expected_message:?}, got {status} {response}"
        );
        // The sentence is fixed text: nothing of the request travels in it, so a client
        // can match it.
        assert!(
            !message.contains(&arn),
            "case {case}: the sentence names nothing of the request, got {message:?}"
        );

        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the record untouched, got {after}"
        );
    }
}

/// RULE: the `Export` rule and the not-issued-for-issuance-work rule carry the same
/// modelled code, and `Export` is decided first — so a certificate that is not issued
/// AND asks for a different `Export` is told about `Export`, and those two refusals are
/// distinguishable only by their sentence; the third leg of the same gate, a request
/// asking only about the certificate's record, carries the processing-state code
/// instead. The same update is served once the certificate is issued, so the refusals
/// are about the status and not about the payload.
#[tokio::test]
async fn the_refusals_of_this_operation_differ_in_their_class_or_their_sentence() {
    for method in VALIDATION_METHODS {
        let case = format!("a pending EMAIL request migrating to {method}");
        let body = json!({
            "DomainName": format!("legs-{}.probe-domain.net", method.to_lowercase()),
            "ValidationMethod": "EMAIL",
            "Options": { "CertificateTransparencyLoggingPreference": "ENABLED" },
        });
        let (router, arn) = certificate_in_status(&case, &body, Fixture::HeldPending).await;
        let before = describe(&router, &arn).await;

        // Leg one: the validation-method migration, refused for the status.
        let (status, response) = update_options(&router, &arn, &json!({ "ValidationMethod": method }))
            .await;
        let status_leg = error_message(&response);
        assert_eq!(
            (status, error_code(&response).as_str(), status_leg.as_str()),
            (400, REQUEST_CONSTRAINT_ERROR, NOT_ISSUED),
            "case {case}: the migration is refused for the status, got {status} {response}"
        );

        // Leg two: the `Export` change, refused for the member — the same code, its own
        // sentence, decided before the status.
        let (status, response) =
            update_options(&router, &arn, &json!({ "Export": "ENABLED" })).await;
        let export_leg = error_message(&response);
        assert_eq!(
            (status, error_code(&response).as_str(), export_leg.as_str()),
            (400, REQUEST_CONSTRAINT_ERROR, EXPORT_NOT_UPDATABLE),
            "case {case}: the export change is refused for the member, got {status} {response}"
        );
        assert_ne!(
            status_leg, export_leg,
            "case {case}: the sentence is the only discriminator between the two \
             refusals, got {status_leg:?} twice"
        );

        // Leg three: the transparency-logging preference, which asks about the record
        // rather than about work still to come — the processing-state class, told apart
        // from the other two by its code.
        let (status, response) = update_options(
            &router,
            &arn,
            &json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str(), error_message(&response).as_str()),
            (400, PROCESSING_STATE_ERROR, NOT_ISSUED_OR_VALID),
            "case {case}: the logging preference is refused for the certificate's \
             processing state, got {status} {response}"
        );

        // Neither refusal wrote anything: the status, the per-domain validation method
        // and the stored options all read as they did before.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: three refusals must leave the record untouched, got {after}"
        );
    }

    // The positive control: the same migration is served for a certificate ACM issued,
    // and the new method is readable back.
    let (router, engine) = router_with_engine();
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "served.probe-domain.net",
            "ValidationMethod": "EMAIL",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    make_validation_due_now(&engine);
    let (status, response) = update_options(&router, &arn, &json!({ "ValidationMethod": "DNS" })).await;
    assert_eq!(
        status, 200,
        "case once issued: the same migration must be served, got {status} {response}"
    );
    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Options"]["ValidationMethod"].as_str(),
        Some("DNS"),
        "case once issued: the new method must be readable back, got {detail}"
    );
    assert_eq!(
        detail["DomainValidationOptions"][0]["ValidationMethod"].as_str(),
        Some("DNS"),
        "case once issued: each domain's evidence must be re-minted for the new \
         method, got {detail}"
    );
}
