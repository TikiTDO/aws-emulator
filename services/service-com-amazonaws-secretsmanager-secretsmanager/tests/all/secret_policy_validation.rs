// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ValidateResourcePolicy`, driven over the wire through
//! the multi-service router and read back through `GetResourcePolicy` and
//! `DescribeSecret`.
//!
//! The rules under test:
//!
//! * a document that is not a policy document fails the SYNTAX check and is
//!   REFUSED with `MalformedPolicyDocumentException`, never reported as a
//!   validation error;
//! * a statement names the identities it applies to, and each has to be one that
//!   exists: a statement naming no principal, or one no identity of the account
//!   this emulator serves could be, is refused the same way;
//! * every other check is REPORTED, not refused: a document that fails one comes
//!   back with `PolicyValidationPassed` false and one entry per failed check —
//!   the broad-access check for an ALLOW of every principal, the lock-out check
//!   for an unconditional DENY of every principal — and a document that fails
//!   none passes with no entries;
//! * the `SecretId` is optional, resolves by name, full ARN or partial ARN, is
//!   `ResourceNotFoundException` when it names nothing and
//!   `InvalidRequestException` when the secret it names is scheduled for
//!   deletion;
//! * validating is reporting: it attaches nothing, replaces nothing and removes
//!   nothing, so the document `GetResourcePolicy` reports is unchanged by any
//!   number of validations.
//!
//! Every test is a TABLE over the class its rule generalises: the form of the
//! principal (bare string, principal type, list of identifiers), the account it
//! belongs to, the statement's effect, the number of statements and of failed
//! checks, the shape of a document that is not a policy document, and the form
//! of the `SecretId`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    BROAD_ACCESS_CHECK_MESSAGE, BROAD_ACCESS_CHECK_NAME, LOCKS_OUT_CALLER_CHECK_MESSAGE,
    LOCKS_OUT_CALLER_CHECK_NAME, MARKED_FOR_DELETION_MESSAGE, SECRET_NOT_FOUND_MESSAGE,
    VALIDATE_SYNTAX_ERROR_MESSAGE, VALIDATE_UNSUPPORTED_PRINCIPAL_MESSAGE,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// An account that is not the one this emulator serves, so no identity it names
/// is one this service can resolve.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// The trailing hyphen and six random characters Secrets Manager appends to a
/// secret's name in its ARN, which a partial ARN leaves off
/// (`CreateSecretRequest$Name`, this service's Smithy model).
const ARN_SUFFIX_LEN: usize = 7;

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail, and return its wire error code and message.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// Create a secret and return its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
    created
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {created}"))
        .to_string()
}

/// The account every identity this emulator can resolve belongs to.
fn this_account() -> &'static str {
    engine::emulator_account_id()
}

/// A one-statement policy document with this `Effect` and `Principal`, rendered
/// through `serde_json` so it is a document the service has to parse rather than
/// one it can match on.
fn policy(effect: &str, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// A policy document carrying exactly these statements.
fn statements(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One statement, as a row writes it.
fn statement(effect: &str, principal: Value) -> Value {
    json!({
        "Effect": effect,
        "Principal": principal,
        "Action": "secretsmanager:GetSecretValue",
        "Resource": "*"
    })
}

/// One reported check, as the response spells it: its name and its message.
type ReportedCheck = (String, String);

/// The `(CheckName, ErrorMessage)` pairs a `ValidateResourcePolicy` response
/// reports, in the order it reports them.
fn reported_checks(response: &Value, case: &str) -> Vec<ReportedCheck> {
    let errors = response
        .get("ValidationErrors")
        .unwrap_or_else(|| panic!("case {case:?}: no ValidationErrors in {response}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ValidationErrors is not a list in {response}"))
        .clone();
    errors
        .iter()
        .map(|entry| {
            let text = |member: &str| {
                entry
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no {member} in entry {entry}"))
                    .to_string()
            };
            (text("CheckName"), text("ErrorMessage"))
        })
        .collect()
}

/// Validate `document` against `secret_id` and return the checks the response
/// reports, asserting `PolicyValidationPassed` agrees with them.
async fn validated_checks(
    router: &MultiServiceRouter,
    secret_id: Option<&str>,
    document: &str,
    case: &str,
) -> Vec<ReportedCheck> {
    let mut body = json!({"ResourcePolicy": document});
    if let Some(secret_id) = secret_id {
        body["SecretId"] = json!(secret_id);
    }
    let response = ok(router, "ValidateResourcePolicy", body, case).await;
    let reported = reported_checks(&response, case);
    let passed = response
        .get("PolicyValidationPassed")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyValidationPassed in {response}"));
    assert_eq!(
        passed,
        reported.is_empty(),
        "case {case:?}: PolicyValidationPassed was {passed} while the response reported \
         {reported:?}"
    );
    reported
}

/// Every check a document fails is reported, and a document that fails none
/// passes with no entries.
///
/// The rows span the statement's effect, the form of the principal (bare string,
/// a principal type, a list of identifiers, a service principal), the number of
/// statements, whether `Statement` is one statement or a list, and the number of
/// checks one document fails.
#[tokio::test]
async fn every_failed_check_is_reported_and_a_clean_document_passes() {
    let account = this_account();
    let root = format!("arn:aws:iam::{account}:root");
    let reader = format!("arn:aws:iam::{account}:role/reader");
    let broad = (
        BROAD_ACCESS_CHECK_NAME.to_string(),
        BROAD_ACCESS_CHECK_MESSAGE.to_string(),
    );
    let locked_out = (
        LOCKS_OUT_CALLER_CHECK_NAME.to_string(),
        LOCKS_OUT_CALLER_CHECK_MESSAGE.to_string(),
    );

    let rows: Vec<(&str, String, Vec<ReportedCheck>)> = vec![
        (
            "allow-the-account-root",
            policy("Allow", json!({"AWS": &root})),
            vec![],
        ),
        (
            "allow-a-bare-account-id",
            policy("Allow", json!({"AWS": account})),
            vec![],
        ),
        (
            "allow-a-role-in-the-account",
            policy("Allow", json!({"AWS": &reader})),
            vec![],
        ),
        (
            "allow-two-concrete-principals",
            policy("Allow", json!({"AWS": [&root, &reader]})),
            vec![],
        ),
        (
            "allow-a-service-principal",
            policy("Allow", json!({"Service": "lambda.amazonaws.com"})),
            vec![],
        ),
        (
            "deny-a-concrete-principal",
            policy("Deny", json!({"AWS": &reader})),
            vec![],
        ),
        (
            "statement-object-rather-than-list",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": &root},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }
            })
            .to_string(),
            vec![],
        ),
        (
            "deny-every-principal-under-a-condition",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*",
                    "Condition": {"StringNotEquals": {"aws:PrincipalArn": &reader}}
                }]
            })
            .to_string(),
            vec![],
        ),
        (
            "allow-the-bare-wildcard",
            policy("Allow", json!("*")),
            vec![broad.clone()],
        ),
        (
            "allow-the-wildcard-under-a-principal-type",
            policy("Allow", json!({"AWS": "*"})),
            vec![broad.clone()],
        ),
        (
            "allow-the-wildcard-among-concrete-identifiers",
            policy("Allow", json!({"AWS": [&root, "*"]})),
            vec![broad.clone()],
        ),
        (
            "one-wildcard-allow-among-three-statements",
            statements(json!([
                statement("Deny", json!({"AWS": &reader})),
                statement("Allow", json!({"AWS": &root})),
                statement("Allow", json!("*")),
            ])),
            vec![broad.clone()],
        ),
        (
            "deny-the-bare-wildcard",
            policy("Deny", json!("*")),
            vec![locked_out.clone()],
        ),
        (
            "deny-the-wildcard-under-a-principal-type",
            policy("Deny", json!({"AWS": "*"})),
            vec![locked_out.clone()],
        ),
        (
            "allow-and-deny-every-principal",
            statements(json!([
                statement("Allow", json!("*")),
                statement("Deny", json!({"AWS": "*"})),
            ])),
            vec![broad.clone(), locked_out.clone()],
        ),
    ];

    let router = emulator();
    let name = "validated-secret";
    create(&router, name, "setup").await;

    for (case, document, expected) in &rows {
        for secret_id in [Some(name), None] {
            let row = format!("{case}/SecretId={secret_id:?}");
            let reported = validated_checks(&router, secret_id, document, &row).await;
            assert_eq!(
                &reported, expected,
                "case {row:?}: expected the checks {expected:?}, got {reported:?}"
            );
        }
    }
}

/// A document that is not a policy document fails the syntax check and is
/// refused, rather than reported on.
///
/// The rows span the ways a document can fail the policy-document grammar: not
/// JSON at all, JSON that is not an object, an object with no `Statement`, a
/// `Statement` that is neither a statement nor a list of them, an empty list,
/// and a statement with no usable `Effect`.
#[tokio::test]
async fn a_document_that_is_not_a_policy_document_is_refused() {
    let malformed: Vec<(&str, String)> = vec![
        ("not-json", "this is not a policy".to_string()),
        ("truncated-json", "{\"Statement\": [".to_string()),
        ("json-array", "[]".to_string()),
        ("json-string", "\"Statement\"".to_string()),
        ("json-number", "17".to_string()),
        (
            "object-without-statement",
            json!({"Version": "2012-10-17"}).to_string(),
        ),
        (
            "statement-is-a-string",
            json!({"Statement": "Allow everything"}).to_string(),
        ),
        ("statement-list-empty", json!({"Statement": []}).to_string()),
        (
            "statement-list-of-strings",
            json!({"Statement": ["Allow"]}).to_string(),
        ),
        (
            "statement-without-effect",
            json!({"Statement": [{"Principal": "*", "Action": "*", "Resource": "*"}]}).to_string(),
        ),
        (
            "effect-is-neither-allow-nor-deny",
            json!({"Statement": [{"Effect": "Maybe", "Principal": "*"}]}).to_string(),
        ),
    ];

    let router = emulator();
    let name = "malformed-validation-secret";
    create(&router, name, "setup").await;

    for (case, document) in &malformed {
        let (code, message) = err(
            &router,
            "ValidateResourcePolicy",
            json!({"SecretId": name, "ResourcePolicy": document}),
            case,
        )
        .await;
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code} with \
             {message:?}"
        );
        assert_eq!(
            message, VALIDATE_SYNTAX_ERROR_MESSAGE,
            "case {case:?}: expected the syntax-check message, got {message:?}"
        );
    }
}

/// A statement's principals have to be identities that exist: one that names
/// none, or names an identity no account this emulator serves could hold, is
/// refused; every principal that does name one is validated.
///
/// The rows span the form of the principal (bare string, a principal type, a
/// list), the account it belongs to, the principal type it sits under, and the
/// element naming it (`Principal` or `NotPrincipal`).
#[tokio::test]
async fn a_principal_that_names_no_existing_identity_is_refused() {
    let account = this_account();
    let root = format!("arn:aws:iam::{account}:root");
    let other_root = format!("arn:aws:iam::{ANOTHER_ACCOUNT}:root");
    let other_role = format!("arn:aws:iam::{ANOTHER_ACCOUNT}:role/reader");

    // `supported` is whether the document names only identities this service can
    // resolve, so the call is answered rather than refused.
    let rows: Vec<(&str, String, bool)> = vec![
        ("bare-wildcard", policy("Allow", json!("*")), true),
        (
            "this-account-root",
            policy("Allow", json!({"AWS": &root})),
            true,
        ),
        (
            "this-account-bare-id",
            policy("Allow", json!({"AWS": account})),
            true,
        ),
        (
            "this-account-role-arn",
            policy("Allow", json!({"AWS": format!("arn:aws:iam::{account}:role/r")})),
            true,
        ),
        (
            "this-account-arn-as-a-bare-principal",
            policy("Allow", json!(&root)),
            true,
        ),
        (
            "a-service-principal",
            policy("Allow", json!({"Service": "lambda.amazonaws.com"})),
            true,
        ),
        (
            "a-canonical-user",
            policy("Allow", json!({"CanonicalUser": "79a59df900b949e55d96a1e698fbace"})),
            true,
        ),
        (
            "another-account-root",
            policy("Allow", json!({"AWS": &other_root})),
            false,
        ),
        (
            "another-account-bare-id",
            policy("Allow", json!({"AWS": ANOTHER_ACCOUNT})),
            false,
        ),
        (
            "another-account-role-arn",
            policy("Deny", json!({"AWS": &other_role})),
            false,
        ),
        (
            "another-account-among-identifiers-of-this-one",
            policy("Allow", json!({"AWS": [&root, &other_root]})),
            false,
        ),
        (
            "an-identifier-that-is-no-account-and-no-arn",
            policy("Allow", json!({"AWS": "reader"})),
            false,
        ),
        (
            "an-account-id-that-is-too-short",
            policy("Allow", json!({"AWS": "12345"})),
            false,
        ),
        (
            "a-statement-naming-no-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            false,
        ),
        (
            "another-account-under-not-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": &other_root},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*"
                }]
            })
            .to_string(),
            false,
        ),
        (
            "another-account-in-the-second-of-two-statements",
            statements(json!([
                statement("Allow", json!({"AWS": &root})),
                statement("Allow", json!({"AWS": &other_role})),
            ])),
            false,
        ),
    ];

    let router = emulator();
    let name = "principal-validation-secret";
    create(&router, name, "setup").await;

    for (case, document, supported) in &rows {
        let body = json!({"SecretId": name, "ResourcePolicy": document});
        if *supported {
            let response = ok(&router, "ValidateResourcePolicy", body, case).await;
            assert!(
                response.get("PolicyValidationPassed").is_some(),
                "case {case:?}: expected a validated document, got {response}"
            );
        } else {
            let (code, message) = err(&router, "ValidateResourcePolicy", body, case).await;
            assert_eq!(
                code, "MalformedPolicyDocumentException",
                "case {case:?}: expected MalformedPolicyDocumentException, got {code} with \
                 {message:?}"
            );
            assert_eq!(
                message, VALIDATE_UNSUPPORTED_PRINCIPAL_MESSAGE,
                "case {case:?}: expected the unsupported-principal message, got {message:?}"
            );
        }
    }
}

/// The `SecretId` is optional, resolves the same way every operation of this
/// service resolves one, and refuses a secret it cannot use.
///
/// The rows span the form of the `SecretId` — absent, a name, a full ARN, a
/// partial ARN — and, for the refusals, a `SecretId` that names nothing and one
/// whose secret is scheduled for deletion.
#[tokio::test]
async fn the_secret_id_is_optional_and_resolved_like_every_other() {
    let router = emulator();
    let name = "resolved-validation-secret";
    let arn = create(&router, name, "setup").await;
    let partial = arn[..arn.len() - ARN_SUFFIX_LEN].to_string();
    let document = policy(
        "Allow",
        json!({"AWS": format!("arn:aws:iam::{}:root", this_account())}),
    );

    for (case, secret_id) in [
        ("absent", None),
        ("name", Some(name.to_string())),
        ("full-arn", Some(arn.clone())),
        ("partial-arn", Some(partial)),
    ] {
        let reported = validated_checks(&router, secret_id.as_deref(), &document, case).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a clean document must report no checks, got {reported:?}"
        );
    }

    for (case, secret_id) in [
        ("missing-name", "no-such-secret".to_string()),
        (
            "missing-full-arn",
            format!(
                "arn:aws:secretsmanager:us-west-2:{}:secret:no-such-secret-aBcDeF",
                this_account()
            ),
        ),
    ] {
        let (code, message) = err(
            &router,
            "ValidateResourcePolicy",
            json!({"SecretId": secret_id, "ResourcePolicy": &document}),
            case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code} with {message:?}"
        );
        assert_eq!(
            message, SECRET_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the not-found message, got {message:?}"
        );
    }

    // A secret inside its recovery window still exists, so it is refused by its
    // state rather than reported as missing.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": name, "RecoveryWindowInDays": 7}),
        "deleted",
    )
    .await;
    let (code, message) = err(
        &router,
        "ValidateResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &document}),
        "scheduled-for-deletion",
    )
    .await;
    assert_eq!(
        code, "InvalidRequestException",
        "a secret scheduled for deletion: expected InvalidRequestException, got {code} with \
         {message:?}"
    );
    assert_eq!(
        message, MARKED_FOR_DELETION_MESSAGE,
        "a secret scheduled for deletion: expected the deletion message, got {message:?}"
    );
}

/// Validating reports on a document and changes nothing: whatever is validated,
/// the policy the secret carries is the one that was attached, and a secret
/// carrying none still carries none.
///
/// The rows span the kind of document validated: one that passes, one that fails
/// each reported check, and one that is refused outright.
#[tokio::test]
async fn validating_a_document_attaches_nothing() {
    let router = emulator();
    let name = "unchanged-validation-secret";
    create(&router, name, "setup").await;
    let account = this_account();
    let attached = policy("Allow", json!({"AWS": format!("arn:aws:iam::{account}:root")}));

    // Nothing is attached yet, so every validation leaves the secret without a
    // document.
    let documents: Vec<(&str, String)> = vec![
        (
            "passes",
            policy("Allow", json!({"AWS": format!("arn:aws:iam::{account}:role/reader")})),
        ),
        ("fails-the-broad-access-check", policy("Allow", json!("*"))),
        ("fails-the-lock-out-check", policy("Deny", json!("*"))),
        ("is-refused-as-malformed", "not a policy".to_string()),
        (
            "is-refused-for-its-principal",
            policy("Allow", json!({"AWS": ANOTHER_ACCOUNT})),
        ),
    ];

    for (case, document) in &documents {
        let body = json!({"SecretId": name, "ResourcePolicy": document});
        let _ = call(&router, "ValidateResourcePolicy", &body).await;
        let read = ok(&router, "GetResourcePolicy", json!({"SecretId": name}), case).await;
        assert!(
            read.get("ResourcePolicy").is_none(),
            "case {case:?}: validating must attach nothing, got {read}"
        );
    }

    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": name, "ResourcePolicy": &attached}),
        "attach",
    )
    .await;

    for (case, document) in &documents {
        let body = json!({"SecretId": name, "ResourcePolicy": document});
        let _ = call(&router, "ValidateResourcePolicy", &body).await;
        let read = ok(&router, "GetResourcePolicy", json!({"SecretId": name}), case).await;
        let reported = read
            .get("ResourcePolicy")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicy in {read}"));
        // Compared against the layout this service reports a policy in: an attached
        // document is rendered into the canonical layout every emulated service that
        // attaches an IAM policy document reports, rather than echoed back as the
        // caller's bytes (`tests/all/secret_resource_policy_reported_form.rs` pins it).
        let expected = ::service_common::iam_policy::canonical_document(&attached);
        assert_eq!(
            reported, expected,
            "case {case:?}: the attached document must survive validating, got {reported:?}"
        );
    }
}
