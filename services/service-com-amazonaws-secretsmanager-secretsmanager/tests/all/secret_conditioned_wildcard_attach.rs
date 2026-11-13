// SPDX-License-Identifier: CC0-1.0

//! WHICH wildcard-principal documents `PutResourcePolicy` attaches when the request asks to
//! block public policies — driven over the wire through the multi-service router.
//!
//! The rule under test: `BlockPublicPolicy` blocks a policy that allows "broad access to the
//! secret, for example those that use a wildcard for the principal"
//! (`PutResourcePolicyRequest$BlockPublicPolicy`, this service's Smithy model), and a
//! statement whose `Condition` confines the request to a caller the account names reaches
//! only those callers, so it is not broad access. Such a document is ATTACHED: HTTP 200
//! carrying the secret's own ARN and name (`PutResourcePolicyResponse` = `ARN` + `Name`), and
//! `GetResourcePolicy` reports it with its `Condition` intact. Only an `Allow`
//! whose wildcard nothing confines is refused, with `PublicPolicyException` on HTTP 400 and
//! the one sentence this service says about a public policy — and the very same document
//! attaches when the request does not ask for the check.
//!
//! The rows span what the rule generalises over rather than one recorded document: the FORM
//! the `Principal` element takes (the bare wildcard, the wildcard under the `AWS` type, the
//! wildcard alone in a list of identifiers, the wildcard beside a concrete one, a concrete
//! account principal, a service principal — the forms of the IAM `Principal` policy element),
//! the condition KEY that confines it (the principal's account, its organization, its ARN),
//! the number of STATEMENTS a document carries, and the statement's EFFECT.
//!
//! The condition-key family itself — which operators and values bind a caller — is the
//! neighbouring suite's table (`secret_public_policy_conditions`); this one holds the key
//! fixed and varies the document around it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the credential
/// scope and this harness registers no authenticator.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service says about a resource policy granting public access, written
/// out rather than imported so the TEXT is pinned.
const PUBLIC_ACCESS_SENTENCE: &str =
    "You can not attach a resource policy that grants Public access to your secret.";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and answer its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to name; see
            // `secret_client_request_token::with_staked_version`.
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

/// The account every identity this emulator can resolve belongs to.
fn this_account() -> &'static str {
    engine::emulator_account_id()
}

/// One statement of a resource policy: its effect, its principal, and the `Condition` it
/// carries (none when `condition` is `None`).
fn statement(effect: &str, principal: Value, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": principal,
        "Action": "secretsmanager:GetSecretValue",
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A policy document carrying `statements`, rendered through `serde_json` so the service has
/// to parse it.
fn document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The `Condition` clauses that confine a wildcard principal to a caller the account names.
/// Each key is one of IAM's global condition context keys for the principal making the
/// request (IAM User Guide, "AWS global condition context keys").
fn account_scope() -> Value {
    json!({"StringEquals": {"aws:PrincipalAccount": this_account()}})
}

fn organization_scope() -> Value {
    json!({"StringEquals": {"aws:PrincipalOrgID": "o-exampleorgid"}})
}

fn principal_arn_scope() -> Value {
    json!({"ArnEquals": {"aws:PrincipalArn": format!("arn:aws:iam::{}:role/reader", this_account())}})
}

/// A concrete `AWS` principal of this account, which names one identity rather than every
/// one.
fn account_root() -> Value {
    json!({"AWS": format!("arn:aws:iam::{}:root", this_account())})
}

/// One row: the case, the statements the document carries, and whether the request that asks
/// to block public policies attaches it.
struct Row {
    case: String,
    statements: Vec<Value>,
    attaches: bool,
}

/// The `Principal` element forms the rule is asserted over, each with whether it names the
/// wildcard. The forms are IAM's for the `Principal` policy element: the bare string, and a
/// map from principal type to one identifier or a list of them.
fn principal_forms() -> Vec<(&'static str, Value, bool)> {
    let role = format!("arn:aws:iam::{}:role/reader", this_account());
    vec![
        ("the bare wildcard", json!("*"), true),
        ("the wildcard under the AWS type", json!({"AWS": "*"}), true),
        ("the wildcard alone in a list", json!({"AWS": ["*"]}), true),
        (
            "the wildcard beside a concrete role ARN",
            json!({"AWS": ["*", role]}),
            true,
        ),
        ("a concrete account root principal", account_root(), false),
        (
            "a service principal",
            json!({"Service": "secretsmanager.amazonaws.com"}),
            false,
        ),
    ]
}

/// The documents the rule is asserted over: every principal form both unconditioned and
/// confined to this account, each confining key over the bare wildcard, and documents of
/// more than one statement.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (form, principal, wildcard) in principal_forms() {
        rows.push(Row {
            case: format!("{form}, with no Condition"),
            statements: vec![statement("Allow", principal.clone(), None)],
            // A wildcard nothing confines is broad access; a concrete principal never is.
            attaches: !wildcard,
        });
        rows.push(Row {
            case: format!("{form}, confined to this account"),
            statements: vec![statement("Allow", principal, Some(account_scope()))],
            attaches: true,
        });
    }
    for (key, condition) in [
        ("the principal's organization", organization_scope()),
        ("the principal's ARN", principal_arn_scope()),
    ] {
        rows.push(Row {
            case: format!("the bare wildcard, confined by {key}"),
            statements: vec![statement("Allow", json!("*"), Some(condition))],
            attaches: true,
        });
    }
    rows.push(Row {
        case: "a confined wildcard allow beside a concrete allow".to_string(),
        statements: vec![
            statement("Allow", json!("*"), Some(account_scope())),
            statement("Allow", account_root(), None),
        ],
        attaches: true,
    });
    rows.push(Row {
        case: "a confined wildcard allow beside an unconditioned one".to_string(),
        statements: vec![
            statement("Allow", json!("*"), Some(account_scope())),
            statement("Allow", json!("*"), None),
        ],
        attaches: false,
    });
    rows.push(Row {
        case: "an unconditioned wildcard DENY beside a confined allow".to_string(),
        statements: vec![
            statement("Deny", json!("*"), None),
            statement("Allow", json!("*"), Some(account_scope())),
        ],
        attaches: true,
    });
    rows
}

/// Create a secret and answer the ARN it reports, so the attach's output can be compared
/// against the secret's own identifiers.
async fn create_secret(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let (status, parsed) = call(
        router,
        "CreateSecret",
        &json!({"Name": name, "SecretString": "account-scoped-value"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to succeed, got {status} with {parsed}"
    );
    parsed["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no ARN, got {parsed}"))
        .to_string()
}

/// Attach `policy` to `name`, blocking public policies when `block` is set.
async fn attach(
    router: &MultiServiceRouter,
    name: &str,
    policy: &str,
    block: bool,
) -> (u16, Value) {
    let mut request = json!({"SecretId": name, "ResourcePolicy": policy});
    if block {
        request["BlockPublicPolicy"] = json!(true);
    }
    call(router, "PutResourcePolicy", &request).await
}

/// RULE: a wildcard-principal `Allow` a `Condition` confines to a caller the account names is
/// not broad access, so a request that blocks public policies attaches it and reports the
/// secret's ARN and name; only an unconfined wildcard `Allow` is refused.
#[tokio::test]
async fn blocking_public_policies_attaches_a_confined_wildcard() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case.as_str();
        let name = format!("confined-wildcard-{index}");
        let arn = create_secret(&router, &name, case).await;
        let policy = document(row.statements.clone());
        let (status, parsed) = attach(&router, &name, &policy, true).await;
        if row.attaches {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the document to attach, got {status} with {parsed} for \
                 {policy}"
            );
            assert_eq!(
                parsed["ARN"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the secret's ARN back, got {parsed}"
            );
            assert_eq!(
                parsed["Name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: expected the secret's name back, got {parsed}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the unconfined wildcard to be refused, got {status} \
                 with {parsed} for {policy}"
            );
            assert_eq!(
                parsed["__type"]
                    .as_str()
                    .and_then(|raw| raw.split('#').next_back()),
                Some("PublicPolicyException"),
                "case {case:?}: expected PublicPolicyException, got {parsed}"
            );
            assert_eq!(
                parsed["Message"].as_str().or(parsed["message"].as_str()),
                Some(PUBLIC_ACCESS_SENTENCE),
                "case {case:?}: expected the public-access sentence, got {parsed}"
            );
        }
    }
}

/// RULE: an attached document reads back whole, so every `Condition` the accepted
/// statements carried survives the attach; a refused one leaves the secret carrying none.
#[tokio::test]
async fn an_attached_document_reads_back_with_its_conditions() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case.as_str();
        let name = format!("confined-wildcard-readback-{index}");
        create_secret(&router, &name, case).await;
        let policy = document(row.statements.clone());
        let (attached, answer) = attach(&router, &name, &policy, true).await;
        let expected = if row.attaches { 200 } else { 400 };
        assert_eq!(
            attached, expected,
            "case {case:?}: expected the attach to answer {expected}, got {attached} with \
             {answer} for {policy}"
        );
        let (status, read) = call(&router, "GetResourcePolicy", &json!({"SecretId": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the policy read to answer 200, got {status} with {read}"
        );
        if !row.attaches {
            assert!(
                read["ResourcePolicy"].is_null(),
                "case {case:?}: a refused attach stored a policy, got {read}"
            );
            continue;
        }
        assert_eq!(
            read["ResourcePolicy"].as_str(),
            Some(reported_policy(&policy).as_str()),
            "case {case:?}: expected the attached document back, got {read}"
        );
        let stored: Value = serde_json::from_str(
            read["ResourcePolicy"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no document to read, got {read}")),
        )
        .unwrap_or_else(|_| panic!("case {case:?}: the stored document is not JSON, got {read}"));
        let sent: Vec<Option<&Value>> = row
            .statements
            .iter()
            .map(|statement| statement.get("Condition"))
            .collect();
        let stored_conditions: Vec<Option<&Value>> = stored["Statement"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the stored Statement is not a list"))
            .iter()
            .map(|statement| statement.get("Condition"))
            .collect();
        assert_eq!(
            stored_conditions, sent,
            "case {case:?}: expected every Condition intact, got {read}"
        );
    }
}

/// RULE: the refusal belongs to the request's own `BlockPublicPolicy` flag — "By default,
/// public policies aren't blocked" (`PutResourcePolicyRequest$BlockPublicPolicy`) — so every
/// document this suite's check refuses attaches when the flag is absent, and reads back.
#[tokio::test]
async fn without_the_flag_every_document_attaches() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case.as_str();
        let name = format!("unguarded-wildcard-{index}");
        let arn = create_secret(&router, &name, case).await;
        let policy = document(row.statements.clone());
        let (status, parsed) = attach(&router, &name, &policy, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an unguarded attach to succeed, got {status} with {parsed} \
             for {policy}"
        );
        assert_eq!(
            parsed["ARN"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the secret's ARN back, got {parsed}"
        );
        let (read_status, read) =
            call(&router, "GetResourcePolicy", &json!({"SecretId": name})).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected the policy read to answer 200, got {read_status} with {read}"
        );
        assert_eq!(
            read["ResourcePolicy"].as_str(),
            Some(reported_policy(&policy).as_str()),
            "case {case:?}: expected the attached document back, got {read}"
        );
    }
}

/// The text `GetResourcePolicy` reports for the document `submitted`: this service
/// renders an attached policy into the canonical layout every emulated service that
/// attaches an IAM policy document reports, rather than echoing the caller's bytes, so a
/// suite that attaches a document and reads it back compares against this. WHAT that
/// layout is is pinned against text written out by hand in
/// `tests/all/secret_resource_policy_reported_form.rs`.
fn reported_policy(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}
