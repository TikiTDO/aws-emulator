// SPDX-License-Identifier: CC0-1.0

//! WHICH modelled error `UpdateCertificateOptions` refuses a certificate that is not
//! ISSUED with, decided per `Options` member — end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: for a certificate ACM has not issued, the class of the refusal is
//! decided by what the request asks for and not by the status it is in. An `Options`
//! member that directs issuance work ACM has yet to perform — the method it validates
//! the names by (`CertificateOptions$ValidationMethod`: "the domain validation method
//! for the certificate"), the export it permits (`CertificateOptions$Export`) — cannot
//! be asked of a record ACM has not issued, so the REQUEST fails a constraint of this
//! operation and draws `ValidationException` (HTTP 400). A request that asks only about
//! the issued certificate's own record — the deprecated transparency-logging preference
//! — is admissible, so what rules it out is the certificate's processing state, and it
//! draws `InvalidStateException` (HTTP 400). Each class carries one fixed sentence, and
//! neither refusal writes anything.
//!
//! The table spans what the rule generalises over: every non-ISSUED status a stored
//! certificate can be in (a request held inside its validation window, a public request
//! whose validation timed out, a name the public DNS delegates to nobody, a recorded
//! revocation) crossed with every `Options` shape a request can send — each member on
//! its own over each value the model declares for it, pairs of members, and an `Options`
//! naming none.
//!
//! The legs of the same gate that must NOT move are asserted beside it: an imported
//! certificate is refused for being imported whatever the request names, an issued
//! certificate serves every one of those requests, a new public certificate defaults to
//! transparency logging ENABLED, and one requested with DISABLED keeps DISABLED. Every
//! row reads the record back with `DescribeCertificate` before and after, so nothing
//! here can pass on a response the store does not back.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router, router_with_engine,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The modelled error this operation answers when the REQUEST fails a constraint of its
/// own (`smithy_query(query="operation", operation="UpdateCertificateOptions")` lists it
/// among this operation's errors, at HTTP 400).
const REQUEST_CONSTRAINT_ERROR: &str = "ValidationException";

/// The modelled error this operation answers when an admissible request meets a
/// certificate whose processing cannot serve it
/// (`smithy_query(query="operation", operation="UpdateCertificateOptions")` lists it
/// among this operation's errors, at HTTP 400).
const PROCESSING_STATE_ERROR: &str = "InvalidStateException";

/// The one sentence the request-constraint refusal carries: fixed text naming neither
/// the certificate nor its status, and terminated by no period.
const NOT_ISSUED: &str = "Certificate is not issued";

/// The one sentence the processing-state refusal carries: fixed text naming neither the
/// certificate nor its status, terminated by a period.
const NOT_ISSUED_OR_VALID: &str = "The selected certificate is not Issued and/or Valid.";

/// The one sentence this operation answers for a certificate whose material came from
/// outside ACM, whatever the request names.
const IMPORTED_NOT_PERMITTED: &str =
    "The UpdateCertificateOptions command is not permitted for imported certificates.";

/// Every `ValidationMethod` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`).
const VALIDATION_METHODS: &[&str] = &["DNS", "EMAIL", "HTTP"];

/// Every `CertificateTransparencyLoggingPreference` the model declares
/// (`smithy_query(query="shape",
/// shape="com.amazonaws.acm#CertificateTransparencyLoggingPreference")`).
const LOGGING_PREFERENCES: &[&str] = &["ENABLED", "DISABLED"];

/// The `CertificateTransparencyLoggingPreference` a certificate that asked for none
/// carries: "all public certificates are recorded in a certificate transparency log"
/// (`CertificateOptions` doc).
const LOGGING_DEFAULT: &str = "ENABLED";

/// Call `UpdateCertificateOptions` for one certificate and return `(status, body)`.
async fn update_options(router: &MultiServiceRouter, arn: &str, options: &Value) -> (u16, Value) {
    call(
        router,
        "UpdateCertificateOptions",
        &json!({ "CertificateArn": arn, "Options": options }),
    )
    .await
}

/// How a fixture reaches the status it asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixture {
    /// A public request held inside its validation window, so it is still pending.
    HeldPending,
    /// A request whose owed validation outcome is materialised, which the names it
    /// covers decide.
    Settled,
    /// An issued certificate carried all the way to a recorded revocation.
    Revoked,
}

/// One certificate that is not issued: what it exercises, the `RequestCertificate` body,
/// how it reaches its status, the `CertificateStatus` it must then be in, and the
/// `Options.Export` value in force on it (the one value an `Export` row may name, since
/// `Export` can never be changed).
struct NotIssued<'a> {
    case: &'a str,
    body: Value,
    fixture: Fixture,
    status: &'a str,
    export_in_force: &'a str,
}

/// The certificates the table runs over: one per non-ISSUED status, reached the way a
/// request reaches it.
fn not_issued_fixtures() -> Vec<NotIssued<'static>> {
    vec![
        NotIssued {
            case: "a public request held inside its validation window",
            body: json!({
                "DomainName": "member-pending.probe-domain.net",
                "ValidationMethod": "EMAIL",
            }),
            fixture: Fixture::HeldPending,
            status: "PENDING_VALIDATION",
            export_in_force: "DISABLED",
        },
        NotIssued {
            case: "a wildcard public request whose validation timed out",
            body: json!({
                "DomainName": "*.member-timedout.probe-domain.io",
                "SubjectAlternativeNames": ["member-timedout.probe-domain.io"],
                "ValidationMethod": "DNS",
            }),
            fixture: Fixture::Settled,
            status: "VALIDATION_TIMED_OUT",
            export_in_force: "DISABLED",
        },
        NotIssued {
            // A name whose top-level label the public DNS delegates to nobody
            // (RFC 6761), so the request fails up front rather than waiting.
            case: "a request for a name that can never be proven",
            body: json!({ "DomainName": "member-failed.invalid", "ValidationMethod": "HTTP" }),
            fixture: Fixture::Settled,
            status: "FAILED",
            export_in_force: "DISABLED",
        },
        NotIssued {
            case: "a certificate whose revocation is recorded",
            body: json!({
                "DomainName": "member-revoked.probe-domain.net",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            fixture: Fixture::Revoked,
            status: "REVOKED",
            export_in_force: "ENABLED",
        },
    ]
}

/// Every `Options` shape a request can send, each with the class and sentence the rule
/// says it draws from a certificate that is not issued. `export_in_force` is the value
/// an `Export` row names, so no row collides with the separate rule that `Export` can
/// never be changed.
fn option_shapes(export_in_force: &str) -> Vec<(String, Value, &'static str, &'static str)> {
    let mut shapes: Vec<(String, Value, &'static str, &'static str)> = Vec::new();

    for method in VALIDATION_METHODS {
        shapes.push((
            format!("ValidationMethod {method} alone"),
            json!({ "ValidationMethod": method }),
            REQUEST_CONSTRAINT_ERROR,
            NOT_ISSUED,
        ));
    }
    for preference in LOGGING_PREFERENCES {
        shapes.push((
            format!("CertificateTransparencyLoggingPreference {preference} alone"),
            json!({ "CertificateTransparencyLoggingPreference": preference }),
            PROCESSING_STATE_ERROR,
            NOT_ISSUED_OR_VALID,
        ));
        shapes.push((
            format!("CertificateTransparencyLoggingPreference {preference} beside a method"),
            json!({
                "CertificateTransparencyLoggingPreference": preference,
                "ValidationMethod": "DNS",
            }),
            REQUEST_CONSTRAINT_ERROR,
            NOT_ISSUED,
        ));
        shapes.push((
            format!("CertificateTransparencyLoggingPreference {preference} beside the export in force"),
            json!({
                "CertificateTransparencyLoggingPreference": preference,
                "Export": export_in_force,
            }),
            REQUEST_CONSTRAINT_ERROR,
            NOT_ISSUED,
        ));
    }
    shapes.push((
        format!("the Export value {export_in_force} already in force, alone"),
        json!({ "Export": export_in_force }),
        REQUEST_CONSTRAINT_ERROR,
        NOT_ISSUED,
    ));
    shapes.push((
        "an Options naming no member at all".to_string(),
        json!({}),
        PROCESSING_STATE_ERROR,
        NOT_ISSUED_OR_VALID,
    ));

    shapes
}

/// Stand up a store holding one certificate in the status `fixture` reaches, and return
/// the router beside its ARN.
async fn certificate_in_status(fixture: &NotIssued<'_>) -> (MultiServiceRouter, String) {
    let case = fixture.case;
    let (router, engine) = router_with_engine();
    let arn = request_certificate(&router, &fixture.body).await;
    match fixture.fixture {
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

/// Request the certificate `body` asks for and carry it to `ISSUED`, returning its ARN.
async fn issued_certificate(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    body: &Value,
) -> String {
    let arn = request_certificate(router, body).await;
    make_validation_due_now(engine);
    let detail = describe(router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case}: the certificate must be issued first, got {detail}"
    );
    arn
}

/// RULE: for a certificate that is not issued the class of the refusal is decided by the
/// `Options` the request names — the request-constraint class for a member that directs
/// issuance work still to come, the processing-state class for a request that asks only
/// about the record — over every non-ISSUED status, and the refusal writes nothing.
#[tokio::test]
async fn the_refusal_class_of_a_certificate_that_is_not_issued_follows_the_member_asked_for() {
    for fixture in not_issued_fixtures() {
        let (router, arn) = certificate_in_status(&fixture).await;
        let before = describe(&router, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(fixture.status),
            "case {}: the fixture must be {}, got {before}",
            fixture.case,
            fixture.status
        );

        for (shape, options, expected_code, expected_message) in
            option_shapes(fixture.export_in_force)
        {
            let case = format!("{} asked for {shape}", fixture.case);
            let (status, response) = update_options(&router, &arn, &options).await;
            let code = error_code(&response);
            let message = error_message(&response);
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, expected_code, expected_message),
                "case {case}: a {} certificate must be refused 400 {expected_code} \
                 {expected_message:?}, got {status} {response}",
                fixture.status
            );
            // The sentence is fixed text: nothing of the request travels in it, so a
            // client can match it.
            assert!(
                !message.contains(&arn),
                "case {case}: the sentence names nothing of the request, got {message:?}"
            );

            // The refusal wrote nothing: the status, the stored options and each
            // domain's validation record read as they did before the call.
            let after = describe(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the record untouched, got {after}"
            );
        }
    }
}

/// RULE, from the other side: the refusals above are about the certificate's state, so
/// an ISSUED certificate ACM minted serves every one of those requests and the change is
/// readable back; while a certificate whose material came from outside ACM is refused for
/// being imported, under its own sentence, whatever the request names.
#[tokio::test]
async fn the_same_requests_are_served_once_issued_and_refused_for_an_imported_certificate() {
    // Served: each member on its own, over an issued private certificate, read back
    // through `DescribeCertificate`.
    for preference in LOGGING_PREFERENCES {
        for method in VALIDATION_METHODS {
            let case = format!("logging {preference} then method {method}");
            let (router, engine) = router_with_engine();
            let arn = issued_certificate(
                &router,
                &engine,
                &case,
                &json!({
                    "DomainName": format!(
                        "served-{}-{}.probe-domain.net",
                        preference.to_lowercase(),
                        method.to_lowercase(),
                    ),
                    "ValidationMethod": "EMAIL",
                    "CertificateAuthorityArn": PRIVATE_CA_ARN,
                }),
            )
            .await;

            let (status, response) = update_options(
                &router,
                &arn,
                &json!({ "CertificateTransparencyLoggingPreference": preference }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the logging preference must be served once issued, \
                 got {status} {response}"
            );
            let (status, response) =
                update_options(&router, &arn, &json!({ "ValidationMethod": method })).await;
            assert_eq!(
                status, 200,
                "case {case}: the method must be served once issued, got {status} {response}"
            );

            let detail = describe(&router, &arn).await;
            assert_eq!(
                (
                    detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
                    detail["Options"]["ValidationMethod"].as_str(),
                ),
                (Some(*preference), Some(*method)),
                "case {case}: both updates must be readable back, got {detail}"
            );
        }
    }

    // Refused for being imported: the same `Options` shapes, over a certificate whose
    // material came from outside ACM.
    let router = router();
    let generated = generate(&Blueprint {
        common_name: Some("member-imported.probe-domain.com"),
        ..Blueprint::default()
    });
    let arn = import(&router, "an imported certificate", &import_body(&generated)).await;
    let before = describe(&router, &arn).await;
    for (shape, options, _, _) in option_shapes("DISABLED") {
        let case = format!("an imported certificate asked for {shape}");
        let (status, response) = update_options(&router, &arn, &options).await;
        assert_eq!(
            (status, error_code(&response).as_str(), error_message(&response).as_str()),
            (400, PROCESSING_STATE_ERROR, IMPORTED_NOT_PERMITTED),
            "case {case}: an imported certificate must keep its own refusal, \
             got {status} {response}"
        );
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the record untouched, got {after}"
        );
    }
}

/// RULE: the transparency-logging preference a certificate carries is the one its
/// request named, and ENABLED when it named none — the neighbouring reads of the same
/// gate, which the per-member refusal must not disturb.
#[tokio::test]
async fn a_requested_certificate_reports_the_logging_preference_its_request_named() {
    // Rows: no `Options` at all, and each preference the model declares.
    let mut rows: Vec<(String, Value, &str)> = vec![(
        "no Options at all".to_string(),
        json!({ "DomainName": "default-logging.probe-domain.net", "ValidationMethod": "DNS" }),
        LOGGING_DEFAULT,
    )];
    for preference in LOGGING_PREFERENCES {
        rows.push((
            format!("Options asking for {preference}"),
            json!({
                "DomainName": format!("asked-{}.probe-domain.net", preference.to_lowercase()),
                "ValidationMethod": "DNS",
                "Options": { "CertificateTransparencyLoggingPreference": preference },
            }),
            preference,
        ));
    }

    for (case, body, expected) in rows {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;
        // Hold the owed validation outcome, so the row reads a certificate that is still
        // pending without racing the settle span.
        hold_pending_validations(&engine);
        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
            Some(expected),
            "case {case}: the certificate must report {expected}, got {detail}"
        );
        // The refusal of a change to that preference does not alter it.
        let (status, response) = update_options(
            &router,
            &arn,
            &json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str(), error_message(&response).as_str()),
            (400, PROCESSING_STATE_ERROR, NOT_ISSUED_OR_VALID),
            "case {case}: a pending certificate's logging preference is refused for its \
             processing state, got {status} {response}"
        );
        let after = describe(&router, &arn).await;
        assert_eq!(
            after["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
            Some(expected),
            "case {case}: the refused change must leave {expected} in place, got {after}"
        );
    }
}
