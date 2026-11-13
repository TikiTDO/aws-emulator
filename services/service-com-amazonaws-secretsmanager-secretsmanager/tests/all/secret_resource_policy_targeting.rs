// SPDX-License-Identifier: CC0-1.0

//! Which secret a `PutResourcePolicy` writes, and where the `ARN` and `Name` it
//! reports come from.
//!
//! The rules under test, over the multi-service router:
//!
//! * a `SecretId` names ONE secret — "The ARN or name of the secret to attach
//!   the resource-based policy. For an ARN, we recommend that you specify a
//!   complete ARN rather than a partial ARN"
//!   (`PutResourcePolicyRequest$SecretId`, this service's Smithy model) — so the
//!   document is attached to the secret that identifier resolves to and to no
//!   other, in whichever of its forms the caller spelled it, and a second attach
//!   replaces only that secret's document;
//! * the `ARN` (`SecretARNType`) and `Name` (`NameType`) the call reports are
//!   the resolved secret's own, read back from the stored record rather than
//!   echoed from the request — so addressing a secret by its ARN still reports
//!   its name, and addressing it by a partial ARN still reports the complete
//!   one;
//! * the attached document belongs to the secret, not to one of its versions or
//!   to its tags, so a mutation of the secret's other state leaves it as it was.
//!
//! `tests/all/secret_resource_policy.rs` covers the document's own class (its
//! shape, size, charset and the `BlockPublicPolicy` decision); these tables span
//! the identifier form, the secret the write lands on, and the neighbouring
//! mutation.

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

/// A policy document as this service REPORTS it: an attached policy is rendered into the
/// canonical layout every emulated service that attaches an IAM policy document reports,
/// rather than echoed back as the caller's bytes (the layout itself is pinned in
/// `tests/all/secret_resource_policy_reported_form.rs`).
fn reported_policy(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}

/// A policy document whose `Sid` records which write produced it, so a document
/// that landed on the wrong secret is visible in what is found there.
fn policy_tagged(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// The partial form of a secret ARN: the complete one without the hyphen and six
/// random characters Secrets Manager appends to the name
/// (`CreateSecretRequest$Name`, this service's Smithy model).
fn partial_arn(arn: &str) -> String {
    arn.rsplit_once('-')
        .map(|(head, _)| head.to_string())
        .expect("a full secret ARN ends in a hyphen and six characters")
}

/// A `PutResourcePolicy` attaches its document to the one secret its `SecretId`
/// resolves to, whichever form that identifier takes, and reports that secret's
/// stored `ARN` and `Name`.
#[tokio::test]
async fn a_put_writes_only_the_secret_its_secret_id_resolves() {
    let router = emulator();
    let names = ["targeting-alpha", "targeting-beta", "targeting-gamma"];
    let mut arns: BTreeMap<&str, String> = BTreeMap::new();
    for name in names {
        arns.insert(name, create(&router, name, "setup").await);
    }

    // What each secret must carry at this point in the sequence; every secret
    // starts out carrying no document at all.
    let mut expected: BTreeMap<&str, Option<String>> =
        names.iter().map(|name| (*name, None)).collect();

    // The rows differ along the FORM of the `SecretId` (name, complete ARN,
    // partial ARN) and along which of the three secrets it names; the last two
    // re-address a secret that already carries a document, so a replacement is
    // spanned as well as a first attach.
    let rows: Vec<(&str, &str, String)> = vec![
        ("by-name", names[0], names[0].to_string()),
        ("by-full-arn", names[1], arns[names[1]].clone()),
        ("by-partial-arn", names[2], partial_arn(&arns[names[2]])),
        ("replace-by-full-arn", names[0], arns[names[0]].clone()),
        ("replace-by-name", names[2], names[2].to_string()),
    ];

    for (case, target, secret_id) in rows {
        let document = policy_tagged(&format!("{case}-on-{target}"));
        let attached = ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &secret_id, "ResourcePolicy": &document}),
            case,
        )
        .await;

        // The identifiers answered are the resolved secret's, not the request's:
        // a call addressed by ARN still reports the short name, and one
        // addressed by a partial ARN still reports the complete ARN.
        assert_eq!(
            attached.get("Name").and_then(Value::as_str),
            Some(target),
            "case {case:?}: expected Name {target:?} for SecretId {secret_id:?}, got {attached}"
        );
        assert_eq!(
            attached.get("ARN").and_then(Value::as_str),
            Some(arns[target].as_str()),
            "case {case:?}: expected ARN {:?} for SecretId {secret_id:?}, got {attached}",
            arns[target]
        );

        expected.insert(target, Some(document));

        // Each secret carries what was last written to IT: the row's target has
        // the new document, and the secrets it did not name are untouched.
        for name in names {
            let read = ok(
                &router,
                "GetResourcePolicy",
                json!({"SecretId": name}),
                case,
            )
            .await;
            let expected = expected[name].as_deref().map(reported_policy);
            assert_eq!(
                policy_of(&read),
                expected.as_deref(),
                "case {case:?}: expected secret {name:?} to carry {expected:?}, got {:?}",
                policy_of(&read)
            );
        }
    }
}

/// The attached document belongs to the secret itself, so a mutation of the
/// secret's other state leaves it as it was — and the policy is
/// still reported after the secret's current version has been replaced.
#[tokio::test]
async fn a_policy_survives_mutations_of_the_secret_s_other_state() {
    // The rows span the neighbouring mutation: a new secret value (a new
    // version, so a new `AWSCURRENT`), a tag write and a tag removal.
    let mutations: Vec<(&str, &str, Value)> = vec![
        (
            "put-secret-value",
            "PutSecretValue",
            json!({"SecretString": "rotated"}),
        ),
        (
            "tag-resource",
            "TagResource",
            json!({"Tags": [{"Key": "owner", "Value": "platform"}]}),
        ),
        (
            "untag-resource",
            "UntagResource",
            json!({"TagKeys": ["owner"]}),
        ),
    ];

    for (case, operation, extra) in mutations {
        let router = emulator();
        let name = format!("policy-survives-{case}");
        let arn = create(&router, &name, case).await;
        let document = policy_tagged(&format!("kept-across-{case}"));
        ok(
            &router,
            "PutResourcePolicy",
            json!({"SecretId": &name, "ResourcePolicy": &document}),
            case,
        )
        .await;

        let mut body = extra.clone();
        body["SecretId"] = json!(&name);
        ok(&router, operation, body, case).await;

        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"SecretId": &arn}),
            case,
        )
        .await;
        assert_eq!(
            policy_of(&read),
            Some(reported_policy(&document).as_str()),
            "case {case:?}: {operation} must leave the attached document unchanged, got {:?}",
            policy_of(&read)
        );
        assert_eq!(
            read.get("ARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected ARN {arn:?}, got {read}"
        );
    }
}
