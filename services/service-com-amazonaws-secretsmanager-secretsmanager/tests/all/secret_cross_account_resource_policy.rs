// SPDX-License-Identifier: CC0-1.0

//! Sharing a secret with another account: the document `PutResourcePolicy`
//! attaches, and the secret record that document lives on.
//!
//! The rules under test, driven end to end over the multi-service router:
//!
//! * `PutResourcePolicy` "Attaches a resource-based permission policy to a
//!   secret" (this service's Smithy model), and the `ResourcePolicy`
//!   (`NonEmptyResourcePolicyType`) it attaches is the CALLER's document: the
//!   identities its statements name are not this service's to resolve, so a
//!   statement naming a principal in another account is attached like any other
//!   one, `GetResourcePolicy` reports it, and the `ARN`
//!   (`SecretARNType`) and `Name` (`NameType`) both calls report are the
//!   resolved secret's own rather than anything read out of the document;
//! * the document belongs to the one secret its `SecretId` resolved to, so a
//!   `DeleteResourcePolicy` detaches that secret's document and leaves every
//!   other secret's exactly where it was;
//! * the document is part of the secret RESOURCE record and lasts as long as
//!   that record does: while a `DeleteSecret` has the secret scheduled for
//!   deletion `GetResourcePolicy` is refused with `InvalidRequestException` —
//!   "A parameter value is not valid for the current state of the resource.
//!   Possible causes: The secret is scheduled for deletion." — and a
//!   `RestoreSecret` that cancels that deletion reports the very document that
//!   was attached, whichever recovery window scheduled it.
//!
//! `tests/all/secret_resource_policy.rs` spans the document's own class (its
//! shape, size and charset, and the `BlockPublicPolicy` decision) and
//! `tests/all/secret_resource_policy_targeting.rs` spans which secret a write
//! lands on; these tables span the ACCOUNT the statements' principals belong to
//! and the secret's deletion-and-restore lifecycle.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The published bounds of `RecoveryWindowInDays`: "The number of days from 7 to
/// 30 that Secrets Manager waits before permanently deleting the secret"
/// (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy model).
const MIN_RECOVERY_WINDOW_DAYS: u64 = 7;
const MAX_RECOVERY_WINDOW_DAYS: u64 = 30;

/// Two account ids that are NOT the one account this emulator serves, so a
/// statement naming a principal under them names an identity this emulator holds
/// nothing about. Checked against [`this_account`] by every case that uses them.
const OTHER_ACCOUNT: &str = "210987654321";
const THIRD_ACCOUNT: &str = "345678901234";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// The one account this emulator serves — the account every secret it holds
/// belongs to.
fn this_account() -> &'static str {
    engine::emulator_account_id()
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

/// Send one request that must fail with a 400, and return its wire error code.
async fn err_code(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> String {
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
    raw.split('#').next_back().unwrap_or(raw).to_string()
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

/// The `ResourcePolicy` member of a `GetResourcePolicy` response, absent when
/// the secret carries no document.
fn policy_of(response: &Value) -> Option<&str> {
    response.get("ResourcePolicy").and_then(Value::as_str)
}

/// The text `GetResourcePolicy` reports for the document `submitted`: this service
/// renders an attached policy into the canonical layout every emulated service that
/// attaches an IAM policy document reports, rather than echoing the caller's bytes, so a
/// suite that attaches a document and reads it back compares against this. WHAT that
/// layout is — the two-space indentation, the ` : ` separator, the inline lists — is pinned
/// against text written out by hand in
/// `tests/all/secret_resource_policy_reported_form.rs`; here it is only the reference
/// point a round trip is measured from.
fn reported_policy(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}

/// A one-statement policy document with this `Effect`, `Principal` and
/// `Action`, rendered through `serde_json` so it is a document the service has
/// to parse rather than one it can match on.
fn policy(effect: &str, principal: Value, action: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "SharedWithAnotherAccount",
            "Effect": effect,
            "Principal": principal,
            "Action": action,
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Read a secret's attached document back, asserting the `ARN` and `Name` the
/// read reports are the secret's own.
async fn read_policy(
    router: &MultiServiceRouter,
    secret_id: &str,
    expected_arn: &str,
    expected_name: &str,
    case: &str,
) -> Option<String> {
    let read = ok(
        router,
        "GetResourcePolicy",
        json!({"SecretId": secret_id}),
        case,
    )
    .await;
    assert_eq!(
        read.get("ARN").and_then(Value::as_str),
        Some(expected_arn),
        "case {case:?}: expected GetResourcePolicy to report ARN {expected_arn:?}, got {read}"
    );
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some(expected_name),
        "case {case:?}: expected GetResourcePolicy to report Name {expected_name:?}, got {read}"
    );
    policy_of(&read).map(str::to_string)
}

/// Every principal form a cross-account share is written in: the row's name, and
/// the document it attaches. The forms are the ones an IAM `Principal` element
/// takes — a bare account id, an account root ARN, a role ARN, an assumed-role
/// session ARN, a list of identifiers, a service principal — crossed with the
/// effect and with a condition-scoped statement.
fn cross_account_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "role-in-another-account",
            policy(
                "Allow",
                json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/app-reader")}),
                json!("secretsmanager:GetSecretValue"),
            ),
        ),
        (
            "another-account-root",
            policy(
                "Allow",
                json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
                json!("secretsmanager:GetSecretValue"),
            ),
        ),
        (
            "bare-account-id",
            policy(
                "Allow",
                json!({"AWS": OTHER_ACCOUNT}),
                json!(["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]),
            ),
        ),
        (
            "assumed-role-session",
            policy(
                "Allow",
                json!({"AWS": format!("arn:aws:sts::{OTHER_ACCOUNT}:assumed-role/app-reader/session")}),
                json!("secretsmanager:GetSecretValue"),
            ),
        ),
        (
            "two-accounts-in-one-list",
            policy(
                "Allow",
                json!({"AWS": [
                    format!("arn:aws:iam::{OTHER_ACCOUNT}:role/app-reader"),
                    format!("arn:aws:iam::{THIRD_ACCOUNT}:role/auditor"),
                ]}),
                json!("secretsmanager:GetSecretValue"),
            ),
        ),
        (
            "service-principal-in-another-account",
            policy(
                "Allow",
                json!({"Service": "lambda.amazonaws.com"}),
                json!("secretsmanager:GetSecretValue"),
            ),
        ),
        (
            "deny-every-principal-but-one-role",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "OnlyOneRole",
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*",
                    "Condition": {"StringNotEquals": {
                        "aws:PrincipalArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/app-reader")
                    }}
                }]
            })
            .to_string(),
        ),
    ]
}

/// A document whose statements name principals in another account attaches like
/// any other, is reported back, and is reported alongside the
/// resolved secret's own `ARN` and `Name`.
#[tokio::test]
async fn a_policy_naming_a_principal_in_another_account_attaches_and_round_trips() {
    // The emulator serves exactly one account, so these ids name identities it
    // holds nothing about — which is the point of the class under test.
    for account in [OTHER_ACCOUNT, THIRD_ACCOUNT] {
        assert_ne!(
            account,
            this_account(),
            "case {account:?}: the cross-account fixture must not name the account this \
             emulator serves ({})",
            this_account()
        );
    }

    let router = emulator();
    let rows = cross_account_rows();

    // Each row lands on its own secret, so the documents can all be read back
    // afterwards and a write that leaked onto a neighbour is visible.
    let mut attached: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for (case, document) in &rows {
        let name = format!("x-account-{case}");
        let arn = create(&router, &name, case).await;
        let reply = ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": document}),
            case,
        )
        .await;
        assert_eq!(
            reply.get("ARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected PutResourcePolicy to report ARN {arn:?}, got {reply}"
        );
        assert_eq!(
            reply.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected PutResourcePolicy to report Name {name:?}, got {reply}"
        );
        attached.insert(case, (name, arn));
    }

    for (case, document) in &rows {
        let (name, arn) = &attached[case];
        let read = read_policy(&router, arn, arn, name, case).await;
        assert_eq!(
            read.as_deref(),
            Some(reported_policy(document).as_str()),
            "case {case:?}: expected the attached document back, got {read:?}"
        );
    }

    // Detaching one secret's document is a write on THAT secret: the row it
    // names reports no document afterwards, and every other row still reports
    // the one it attached.
    let (detached_case, _) = &rows[0];
    let (detached_name, detached_arn) = &attached[detached_case];
    let detached = ok(
        &router,
        "DeleteResourcePolicy",
        json!({"SecretId": detached_name}),
        detached_case,
    )
    .await;
    assert_eq!(
        detached.get("ARN").and_then(Value::as_str),
        Some(detached_arn.as_str()),
        "case {detached_case:?}: expected DeleteResourcePolicy to report ARN {detached_arn:?}, \
         got {detached}"
    );
    for (case, document) in &rows {
        let (name, arn) = &attached[case];
        let expected = if case == detached_case {
            None
        } else {
            Some(reported_policy(document))
        };
        let read = read_policy(&router, name, arn, name, case).await;
        assert_eq!(
            read.as_deref(),
            expected.as_deref(),
            "case {case:?}: after detaching {detached_case:?} expected {expected:?}, got {read:?}"
        );
    }
}

/// The document is part of the secret's own record: a scheduled deletion refuses
/// the read while it stands, and a restore brings back the document that was
/// attached — whichever recovery window scheduled the deletion.
#[tokio::test]
async fn an_attached_policy_survives_a_scheduled_deletion_and_its_restore() {
    // The rows span the recovery window the deletion is scheduled with: each
    // published bound, one inside the range and the window the service picks
    // when the request names none.
    let windows: Vec<(&str, Option<u64>)> = vec![
        ("minimum-window", Some(MIN_RECOVERY_WINDOW_DAYS)),
        ("maximum-window", Some(MAX_RECOVERY_WINDOW_DAYS)),
        ("default-window", None),
        ("mid-window", Some((MIN_RECOVERY_WINDOW_DAYS + MAX_RECOVERY_WINDOW_DAYS) / 2)),
    ];

    for (case, window) in windows {
        let router = emulator();
        let name = format!("x-account-lifecycle-{case}");
        let arn = create(&router, &name, case).await;
        let document = policy(
            "Allow",
            json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/{case}")}),
            json!("secretsmanager:GetSecretValue"),
        );
        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": &document}),
            case,
        )
        .await;

        let mut delete = json!({"SecretId": &name});
        if let Some(days) = window {
            delete["RecoveryWindowInDays"] = json!(days);
        }
        ok(&router, "DeleteSecret", delete, case).await;

        // The secret still exists inside its recovery window, and the state it
        // is in is what refuses the read.
        let code = err_code(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": &name}),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected a scheduled deletion to refuse the read with \
             InvalidRequestException, got {code}"
        );

        ok(&router, "RestoreSecret", json!({"SecretId": &name}), case).await;
        let read = read_policy(&router, &name, &arn, &name, case).await;
        assert_eq!(
            read.as_deref(),
            Some(reported_policy(&document).as_str()),
            "case {case:?}: expected the restored secret to carry the document it was attached, \
             got {read:?}"
        );

        // And the restored secret is writable again: the next attach replaces
        // the document the restore brought back.
        let replacement = policy(
            "Allow",
            json!({"AWS": format!("arn:aws:iam::{THIRD_ACCOUNT}:role/{case}")}),
            json!("secretsmanager:GetSecretValue"),
        );
        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &arn, "ResourcePolicy": &replacement}),
            case,
        )
        .await;
        let read = read_policy(&router, &name, &arn, &name, case).await;
        assert_eq!(
            read.as_deref(),
            Some(reported_policy(&replacement).as_str()),
            "case {case:?}: expected the replacement document after the restore, got {read:?}"
        );
    }
}
