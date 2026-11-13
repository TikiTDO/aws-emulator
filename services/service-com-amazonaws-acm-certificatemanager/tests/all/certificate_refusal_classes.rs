// SPDX-License-Identifier: CC0-1.0

//! How ACM's certificate control plane CLASSIFIES and WORDS four of its refusals, end
//! to end over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`,
//! awsJson1_1).
//!
//! RULES under test, each asserted over a TABLE spanning the class it generalises over
//! rather than one request:
//!
//! 1. `ImportCertificate` refuses a defect in the MATERIAL a request carries — a body
//!    it cannot read as the PEM the member holds, or a certificate outside its own
//!    validity window — with `ValidationException` (HTTP 400) under the fixed sentence
//!    naming the defect and the member it was found in; the invalid-parameter class is
//!    for the request's own parameters, not for its material.
//! 2. `UpdateCertificateOptions` applies to a certificate ACM has ISSUED, and which
//!    class a record in any other status is refused with is decided per `Options`
//!    member: a member directing issuance work still to come draws
//!    `ValidationException` (HTTP 400) under one fixed sentence, a request asking only
//!    about the record draws `InvalidStateException` (HTTP 400) under its own, and a
//!    request that asks for an `Export` value other than the one in force is refused
//!    with `ValidationException` (HTTP 400) under a third, whatever status the
//!    certificate is in — and no refusal changes the record.
//! 3. `GetCertificate` on a certificate ACM holds no issued material for is refused
//!    with `RequestInProgressException` (HTTP 400) under a sentence naming the
//!    certificate the request asked for and the account that ARN belongs to.
//! 4. `ResendValidationEmail` called before ACM has dispatched a certificate's first
//!    validation mail is refused with `InvalidStateException` (HTTP 400) under one
//!    fixed sentence that names nothing of the request — the gate is short-lived, and
//!    that sentence is what tells it apart from the refusals of the same class that
//!    will never succeed.
//!
//! Every certificate the import rows feed the service is built by `rcgen` — something
//! other than the code under test — and every call goes through
//! `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`], so nothing here can pass on a response the store
//! does not back. A row that must observe the transient `PENDING_VALIDATION` state
//! moves the due instant the emulator itself stores instead of racing the wall clock.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{blob, generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, call_in_region, describe, dispatch_validation_mail_now, error_code, error_message,
    hold_pending_validations, list_all_arns, request_certificate, router, router_with_engine,
};
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The sentence ACM answers for a `Certificate` body it cannot read as one PEM-encoded
/// X.509 certificate, as recorded against the real service. Written out here rather
/// than imported from the service, so no assertion is made against the template it is
/// asserting.
const CERTIFICATE_NOT_PEM: &str = "The certificate is not PEM-encoded or is not valid.";

/// The sentence ACM answers for a certificate whose validity window has run out
/// ("the current time must be between the Not Before and Not After certificate fields",
/// `ImportCertificate` doc), as recorded against the real service.
const CERTIFICATE_EXPIRED: &str = "The certificate is expired. You can Import a certificate \
                                   only during its validity period.";

/// The same sentence for the other end of that window: a certificate whose Not Before
/// has not arrived yet.
const CERTIFICATE_NOT_YET_VALID: &str = "The certificate is not valid yet. You can Import a \
                                         certificate only during its validity period.";

/// The sentence ACM answers for a `PrivateKey` body it cannot read as an unencrypted
/// PEM private key — the same form as the one for the other two bodies an import
/// carries.
const PRIVATE_KEY_NOT_PEM: &str = "The private key is not PEM-encoded or is not valid.";

/// The one sentence ACM reports when `UpdateCertificateOptions` is asked to direct
/// issuance work — a validation method, an export opt-in — for a certificate that is
/// not issued: fixed text naming neither the certificate nor its status, and terminated
/// by no period. Two corpora record this refusal as `ValidationException` with this
/// sentence — the behavioral contract check for the operation and the recorded wire
/// traffic of the request-certificate lifecycle — which is what this suite holds the
/// operation to.
const NOT_ISSUED: &str = "Certificate is not issued";

/// The one sentence ACM reports when `UpdateCertificateOptions` asks only about the
/// record of a certificate that is not issued — the deprecated transparency-logging
/// preference — as recorded against the real service: fixed text naming neither the
/// certificate nor its status, terminated by a period.
const NOT_ISSUED_OR_VALID: &str = "The selected certificate is not Issued and/or Valid.";

/// The sentence `UpdateCertificateOptions` answers for a request that asks to change
/// `Export`, which "cannot [be updated] after the the certificate is created"
/// (`CertificateOptions$Export` doc), as recorded against the real service.
const EXPORT_NOT_UPDATABLE: &str = "Export option for certificates cannot be updated";

/// The sentence `ResendValidationEmail` answers while ACM still owes the certificate
/// its first validation mail, as recorded against the real service: fixed text naming
/// nothing of the request, and terminated by no period.
const RESEND_REQUEST_IN_PROGRESS: &str = "The certificate request is still in progress. Try \
                                          again later";

/// The sentence `ResendValidationEmail` answers for a domain this certificate does not
/// prove control of by email, as recorded against the real service: the certificate the
/// request named, the validation method a resend is for, and the domain asked about.
fn not_using_email_validation_message(arn: &str, domain: &str) -> String {
    format!("Certificate {arn} is not using EMAIL validation for domain {domain}")
}

/// The `RequestInProgressException` sentence for a certificate ACM has not issued: the
/// ARN the request named, and the account that ARN names, inside fixed text.
fn not_yet_issued_message(arn: &str) -> String {
    format!("Certificate {arn} in account {} not yet issued", arn_account(arn))
}

/// The account an ACM ARN names — its fifth colon-separated field
/// (`arn:<partition>:acm:<region>:<account>:<resource>`).
fn arn_account(arn: &str) -> &str {
    arn.split(':').nth(4).unwrap_or_default()
}

/// Every certificate ARN the store holds under the key algorithms these rows create
/// certificates with. The default listing reports only RSA key types, so the EC ones
/// are named explicitly.
async fn stored_arns(router: &MultiServiceRouter) -> Vec<String> {
    list_all_arns(
        router,
        &json!({
            "Includes": {
                "keyTypes": ["EC_prime256v1", "EC_secp384r1", "RSA_1024", "RSA_2048"]
            }
        }),
    )
    .await
}

/// Require that `operation` refuses `body` with HTTP 400, `code` and exactly `message`,
/// and hand back the message the service served.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
    message: &str,
) -> String {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to be refused with 400, got {status} {response}"
    );
    let actual_code = error_code(&response);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code}, got {actual_code:?} in {response}"
    );
    let actual = error_message(&response);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
    actual
}

/// A PEM body with a character outside the base64 alphabet injected into its content,
/// leaving the BEGIN/END framing intact — a body that still frames a PEM message of the
/// right type but can no longer be decoded.
fn garbled_pem(pem: &str) -> String {
    let mut lines: Vec<String> = pem.lines().map(str::to_string).collect();
    let body = lines
        .iter_mut()
        .find(|line| !line.starts_with("-----"))
        .expect("the PEM carries a content line");
    body.replace_range(0..1, "!");
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The `ImportCertificate` body for one certificate and one key, each as a PEM text.
fn material(certificate: &str, private_key: &str) -> Value {
    json!({ "Certificate": blob(certificate), "PrivateKey": blob(private_key) })
}

/// RULE 1: every defect of the material an import carries is the modelled constraint
/// failure for supplied input, under the fixed sentence naming the defect — whichever
/// body it was found in, whichever form it takes, and whether the request is a fresh
/// import or a reimport onto an existing ARN. Nothing is stored for any of them.
#[tokio::test]
async fn import_material_defects_are_one_class_under_their_own_sentence() {
    let router = router();

    // The positive control: a valid import is served, and the ARN it was stored under
    // is what the reimport rows below address.
    let valid = generate(&Blueprint {
        common_name: Some("control.probe-domain.net"),
        ..Blueprint::default()
    });
    let stored = import(&router, "a valid import", &import_body(&valid)).await;

    // Certificates whose validity windows are closed and not yet open: two closed
    // windows, decades apart and on different curves, and one that opens in the far
    // future.
    let long_expired = generate(&Blueprint {
        common_name: Some("long-expired.probe-domain.net"),
        validity: (2000, 2001),
        ..Blueprint::default()
    });
    let recently_expired = generate(&Blueprint {
        common_name: Some("recently-expired.probe-domain.net"),
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        validity: (2015, 2016),
        ..Blueprint::default()
    });
    let not_yet_valid = generate(&Blueprint {
        common_name: Some("not-yet-valid.probe-domain.net"),
        validity: (2990, 2999),
        ..Blueprint::default()
    });

    // The one key form an import may not carry: the same key framed as an encrypted
    // private key (RFC 7468 section 11).
    let encrypted = format!(
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\n{}\n-----END ENCRYPTED PRIVATE KEY-----\n",
        aws_smithy_types::base64::encode(valid.private_key.as_bytes())
    );
    // A blob that is not text at all, so it cannot be a PEM encoding of anything.
    let not_text: Value = json!(aws_smithy_types::base64::encode([0xff_u8, 0xfe, 0x00, 0x80]));

    // Rows differ along the member the defect sits in, the FORM the defect takes, the
    // curve of the certificate and the end of the validity window that excludes it.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a Certificate framing no PEM message at all",
            material("this is not a certificate", &valid.private_key),
            CERTIFICATE_NOT_PEM,
        ),
        (
            "a CERTIFICATE block with a non-base64 character in its body",
            material(&garbled_pem(&valid.certificate), &valid.private_key),
            CERTIFICATE_NOT_PEM,
        ),
        (
            "a CERTIFICATE block whose content is not a certificate",
            material(
                "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n",
                &valid.private_key,
            ),
            CERTIFICATE_NOT_PEM,
        ),
        (
            "a well-formed PEM of another type as the Certificate",
            material(&valid.private_key, &valid.private_key),
            CERTIFICATE_NOT_PEM,
        ),
        (
            "a Certificate body that is not text",
            json!({ "Certificate": not_text, "PrivateKey": blob(&valid.private_key) }),
            CERTIFICATE_NOT_PEM,
        ),
        (
            "a certificate whose validity window closed decades ago",
            import_body(&long_expired),
            CERTIFICATE_EXPIRED,
        ),
        (
            "a P-384 certificate whose validity window closed later",
            import_body(&recently_expired),
            CERTIFICATE_EXPIRED,
        ),
        (
            "a certificate whose validity window has not opened",
            import_body(&not_yet_valid),
            CERTIFICATE_NOT_YET_VALID,
        ),
        (
            "a PrivateKey framing no PEM message at all",
            material(&valid.certificate, "this is not a key"),
            PRIVATE_KEY_NOT_PEM,
        ),
        (
            "a PRIVATE KEY block whose content is not a key",
            material(
                &valid.certificate,
                "-----BEGIN PRIVATE KEY-----\nZm9v\n-----END PRIVATE KEY-----\n",
            ),
            PRIVATE_KEY_NOT_PEM,
        ),
        (
            "an encrypted PrivateKey",
            material(&valid.certificate, &encrypted),
            PRIVATE_KEY_NOT_PEM,
        ),
    ];

    for (case, body, message) in &cases {
        refused(
            &router,
            case,
            "ImportCertificate",
            body,
            "ValidationException",
            message,
        )
        .await;

        // The same material, as a REIMPORT of the stored certificate: the refusal
        // belongs to the material, not to which of the two shapes the request has.
        let mut reimport = body.clone();
        reimport["CertificateArn"] = json!(stored);
        let reimport_case = format!("{case}, onto an existing ARN");
        refused(
            &router,
            &reimport_case,
            "ImportCertificate",
            &reimport,
            "ValidationException",
            message,
        )
        .await;
    }

    // Every refusal above stored nothing, and left the control's material as it was.
    let listed = stored_arns(&router).await;
    assert_eq!(
        listed,
        vec![stored.clone()],
        "case {:?}: only the control import may be stored, got {listed:?}",
        "every material refusal above"
    );
    let (status, served) = call(
        &router,
        "GetCertificate",
        &json!({ "CertificateArn": &stored }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: GetCertificate must still serve the control, got {status} {served}",
        "every material refusal above"
    );
    assert_eq!(
        served["Certificate"].as_str(),
        Some(valid.certificate.as_str()),
        "case {:?}: expected the control's material back unchanged, got {served}",
        "every material refusal above"
    );
}

/// One options row: what it exercises, the `RequestCertificate` body, whether the
/// validation outcome the clock owes has to be materialised (otherwise it is held out),
/// the `CertificateStatus` the fixture must then be in, the `Options` the update asks
/// for, and the modelled error and sentence the refusal must carry.
type OptionsCase<'a> = (&'a str, Value, bool, &'a str, Value, &'a str, &'a str);

/// RULE 2: `UpdateCertificateOptions` is served only for a certificate ACM has ISSUED,
/// and `Export` can never be changed — each refused under its OWN fixed sentence, with
/// the record left exactly as it was, and the class decided by what the request asks
/// for: a constraint failure of the request (`ValidationException`) for a member
/// directing issuance work still to come or for an `Export` change, the certificate's
/// processing state (`InvalidStateException`) for a request that asks only about its
/// record. The `Export` sentence is decided first, so a certificate that is not issued
/// AND asks for a different `Export` is told about `Export`.
#[tokio::test]
async fn options_update_refusals_carry_the_sentence_of_the_rule_they_break() {
    // Rows differ along the status the certificate is in, the `Options` member the
    // request names (and how many at once), and whether the value asked for is the one
    // in force.
    let cases: Vec<OptionsCase<'_>> = vec![
        (
            "a pending EMAIL request asked to migrate to DNS validation",
            json!({ "DomainName": "pending.probe-domain.net", "ValidationMethod": "EMAIL" }),
            false,
            "PENDING_VALIDATION",
            json!({ "ValidationMethod": "DNS" }),
            "ValidationException",
            NOT_ISSUED,
        ),
        (
            "a pending request asked for transparency logging",
            json!({ "DomainName": "logging.probe-domain.net", "ValidationMethod": "DNS" }),
            false,
            "PENDING_VALIDATION",
            json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
            "InvalidStateException",
            NOT_ISSUED_OR_VALID,
        ),
        (
            // The export value in force asks for no change, so it passes the Export
            // rule and the status is what refuses the call.
            "a pending request asked for the export value it already has",
            json!({ "DomainName": "noop.probe-domain.net", "ValidationMethod": "HTTP" }),
            false,
            "PENDING_VALIDATION",
            json!({ "Export": "DISABLED" }),
            "ValidationException",
            NOT_ISSUED,
        ),
        (
            "a pending request asked to enable export",
            json!({ "DomainName": "export.probe-domain.net", "ValidationMethod": "EMAIL" }),
            false,
            "PENDING_VALIDATION",
            json!({ "Export": "ENABLED" }),
            "ValidationException",
            EXPORT_NOT_UPDATABLE,
        ),
        (
            "a request that failed, asked to migrate validation",
            json!({ "DomainName": "failed.invalid", "ValidationMethod": "EMAIL" }),
            true,
            "FAILED",
            json!({ "ValidationMethod": "HTTP" }),
            "ValidationException",
            NOT_ISSUED,
        ),
        (
            "a request that timed out, asked for two options at once",
            json!({ "DomainName": "timedout.probe-domain.net", "ValidationMethod": "DNS" }),
            true,
            "VALIDATION_TIMED_OUT",
            json!({
                "ValidationMethod": "EMAIL",
                "CertificateTransparencyLoggingPreference": "ENABLED",
            }),
            "ValidationException",
            NOT_ISSUED,
        ),
        (
            // An ISSUED certificate: the status rule is satisfied, and the Export rule
            // alone refuses the call.
            "an issued certificate asked to drop the export it was created with",
            json!({
                "DomainName": "issued.probe-domain.net",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            true,
            "ISSUED",
            json!({ "Export": "DISABLED" }),
            "ValidationException",
            EXPORT_NOT_UPDATABLE,
        ),
    ];

    for (case, body, settle, expected_status, options, code, message) in &cases {
        // A store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        if *settle {
            make_validation_due_now(&engine);
        } else {
            hold_pending_validations(&engine);
        }

        let before = describe(&router, &arn).await;
        let status = before["Status"].as_str();
        assert_eq!(
            status,
            Some(*expected_status),
            "case {case:?}: the fixture must be {expected_status}, got {status:?} in {before}"
        );

        refused(
            &router,
            case,
            "UpdateCertificateOptions",
            &json!({ "CertificateArn": &arn, "Options": options }),
            code,
            message,
        )
        .await;

        // The refusal wrote nothing: the status, the options and the validation method
        // the record reports are the ones it reported before the call.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the record untouched, got {after}"
        );
    }
}

/// RULE 3: the refusal for a certificate ACM holds no issued material for names that
/// certificate and the account whose ARN was addressed — over every validation method,
/// over a wildcard request, and in a Region other than the one the suite usually signs
/// for, where the ARN the sentence carries is that Region's.
#[tokio::test]
async fn the_in_progress_refusal_names_the_certificate_and_its_account() {
    // Rows differ along the validation method the request asks for, the shape and
    // number of the names it covers, and the Region the certificate is created in.
    let cases: &[(&str, &str, Value)] = &[
        (
            "a request validated by DNS",
            "us-west-2",
            json!({ "DomainName": "dns.probe-domain.net", "ValidationMethod": "DNS" }),
        ),
        (
            "a request validated by email",
            "us-west-2",
            json!({ "DomainName": "email.probe-domain.net", "ValidationMethod": "EMAIL" }),
        ),
        (
            "a request validated by HTTP",
            "us-west-2",
            json!({ "DomainName": "http.probe-domain.net", "ValidationMethod": "HTTP" }),
        ),
        (
            "a wildcard request covering several names",
            "us-west-2",
            json!({
                "DomainName": "*.wild.probe-domain.net",
                "SubjectAlternativeNames": ["alt.wild.probe-domain.net"],
                "ValidationMethod": "DNS",
            }),
        ),
        (
            "a request created in another Region",
            "eu-west-1",
            json!({ "DomainName": "regional.probe-domain.net", "ValidationMethod": "DNS" }),
        ),
    ];

    for (case, region, body) in cases {
        let (router, engine) = router_with_engine();
        let (status, response) = call_in_region(&router, region, "RequestCertificate", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RequestCertificate must succeed in {region}, got {status} {response}"
        );
        let arn = response["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {response}"))
            .to_string();
        hold_pending_validations(&engine);

        let (status, response) = call_in_region(
            &router,
            region,
            "GetCertificate",
            &json!({ "CertificateArn": &arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a certificate with no material is refused, got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "RequestInProgressException",
            "case {case:?}: expected RequestInProgressException, got {code:?} in {response}"
        );
        let message = error_message(&response);
        let expected = not_yet_issued_message(&arn);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // The two variable parts really are the request's own: the ARN it named, and
        // the account that ARN belongs to.
        assert!(
            message.contains(&arn) && message.contains(arn_account(&arn)),
            "case {case:?}: the sentence must name the certificate and its account, got \
             {message:?}"
        );

        // The refused read changed nothing: the certificate is still awaiting its
        // validation, so the identical call is refused the identical way.
        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: a refused read leaves the request pending, got {detail}"
        );
    }
}

/// RULE 4: the gate before ACM's own first validation mail is one fixed sentence that
/// names nothing of the request — over every covered name and validation domain a
/// resend can ask for — it is short-lived, so the identical call succeeds once the
/// dispatch the clock owed has happened, and the other refusals of the same modelled
/// class carry different sentences so a client can tell them apart.
#[tokio::test]
async fn a_resend_before_the_first_mail_carries_the_fixed_in_progress_sentence() {
    // Rows differ along which covered name the resend names (the requested name, a
    // subject alternative name, a wildcard) and which suffix it asks for (the name
    // itself, a superdomain).
    let cases: &[(&str, Value, &str, &str)] = &[
        (
            "the requested name, through itself",
            json!({ "DomainName": "gate.probe-domain.net", "ValidationMethod": "EMAIL" }),
            "gate.probe-domain.net",
            "gate.probe-domain.net",
        ),
        (
            "a subject alternative name, through its superdomain",
            json!({
                "DomainName": "gate.deep.probe-domain.net",
                "SubjectAlternativeNames": ["san.deep.probe-domain.net"],
                "ValidationMethod": "EMAIL",
            }),
            "san.deep.probe-domain.net",
            "probe-domain.net",
        ),
        (
            "a wildcard name, through the domain it covers",
            json!({
                "DomainName": "gate.probe-domain.org",
                "SubjectAlternativeNames": ["*.wild.probe-domain.org"],
                "ValidationMethod": "EMAIL",
            }),
            "*.wild.probe-domain.org",
            "wild.probe-domain.org",
        ),
        (
            // No method named: ACM validates by email, so the same gate applies.
            "a request that named no validation method",
            json!({ "DomainName": "gate.probe-domain.io" }),
            "gate.probe-domain.io",
            "probe-domain.io",
        ),
    ];

    for (case, body, domain, validation_domain) in cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        // Both transitions the clock owes are held out, so the row observes the state
        // before the first mail went out whatever the wall clock does.
        hold_pending_validations(&engine);

        let request = json!({
            "CertificateArn": &arn,
            "Domain": domain,
            "ValidationDomain": validation_domain,
        });
        let served = refused(
            &router,
            case,
            "ResendValidationEmail",
            &request,
            "InvalidStateException",
            RESEND_REQUEST_IN_PROGRESS,
        )
        .await;
        // Nothing of the request appears in what was SERVED: a caller cannot be told
        // which certificate or domain it was, because ACM's sentence names neither.
        assert!(
            !served.contains(&arn) && !served.contains(*domain),
            "case {case:?}: the gate's sentence is fixed text, got {served:?}"
        );

        // The gate is short-lived: once the dispatch the clock owed has happened, the
        // identical call is served.
        dispatch_validation_mail_now(&engine);
        let (status, response) = call(&router, "ResendValidationEmail", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same resend must succeed once the first mail has gone \
             out, got {status} {response}"
        );
    }

    // The other refusals of the same modelled class carry their own sentences, so the
    // short-lived gate is distinguishable from the ones that will never succeed: a
    // domain this certificate validates by DNS, and a certificate past validation
    // altogether.
    let (router, engine) = router_with_engine();
    let dns = request_certificate(
        &router,
        &json!({ "DomainName": "dns-gate.probe-domain.net", "ValidationMethod": "DNS" }),
    )
    .await;
    hold_pending_validations(&engine);
    let (status, response) = call(
        &router,
        "ResendValidationEmail",
        &json!({
            "CertificateArn": &dns,
            "Domain": "dns-gate.probe-domain.net",
            "ValidationDomain": "dns-gate.probe-domain.net",
        }),
    )
    .await;
    let code = error_code(&response);
    let message = error_message(&response);
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidStateException"),
        "case {:?}: expected 400 InvalidStateException, got {status} {response}",
        "a domain validated by DNS"
    );
    let expected = not_using_email_validation_message(&dns, "dns-gate.probe-domain.net");
    assert_eq!(
        message, expected,
        "case {:?}: expected the message {expected:?}, got {message:?}",
        "a domain validated by DNS"
    );
    assert_ne!(
        message, RESEND_REQUEST_IN_PROGRESS,
        "case {:?}: a refusal that will never succeed must not read as the short-lived \
         gate, got {message:?}",
        "a domain validated by DNS"
    );
}
