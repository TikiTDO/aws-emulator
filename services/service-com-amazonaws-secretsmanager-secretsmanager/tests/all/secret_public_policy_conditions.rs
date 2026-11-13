// SPDX-License-Identifier: CC0-1.0

//! WHICH resource policy `BlockPublicPolicy` refuses, when its statement carries a
//! `Condition` — driven over the wire through `PutResourcePolicy` and the validating channel
//! that reports the same check.
//!
//! The rule under test: `BlockPublicPolicy` blocks a policy that allows "broad access to the
//! secret, for example those that use a wildcard for the principal"
//! (`PutResourcePolicyRequest$BlockPublicPolicy`, this service's Smithy model) — and a
//! wildcard-principal `Allow` whose `Condition` BINDS the request to a caller something names
//! reaches exactly those callers rather than everybody, so it is not broad access. A clause
//! binds when its key is one of IAM's principal- or source-scoping global condition keys, its
//! operator is a POSITIVE comparison, and every value it accepts names something concrete; a
//! negated comparison, an `…IfExists` spelling, a `Null` asking for the key to be ABSENT, a
//! value carrying an IAM wildcard and a key outside that family each leave the statement open
//! to a caller the account never named.
//!
//! The rows span what the rule generalises over rather than one recorded document: the
//! condition KEY (the principal family, the source family, the per-tag family, the network
//! path, the flag naming an AWS service, a key outside them all), the OPERATOR (equality, an
//! ARN comparison, a wildcard comparison, `Null`, each negated spelling, the `…IfExists`
//! suffix, and both set qualifiers over the same comparison), and the VALUES (one, several,
//! one carrying `*` or `?`, a CIDR that covers every address, each polarity of a flag, and
//! none at all).
//!
//! The same table is asserted on both channels, because the refusing one and the reporting
//! one answer the same question: `PutResourcePolicy(BlockPublicPolicy=true)` refuses exactly
//! the documents `ValidateResourcePolicy` reports the broad-access check against.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the credential
/// scope and this harness registers no authenticator.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service says about a policy granting public access to a secret,
/// written out rather than imported so the TEXT is pinned.
const PUBLIC_ACCESS_SENTENCE: &str =
    "You can not attach a resource policy that grants Public access to your secret.";

/// The name the broad-access check is reported under.
const PUBLIC_ACCESS_CHECK: &str = "BlockPublicPolicyCheck";

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

/// A one-statement `Allow` for the WILDCARD principal, carrying `condition` (none when it is
/// [`Value::Null`]), rendered through `serde_json` so the service has to parse it.
fn wildcard_policy(condition: Value) -> String {
    let mut statement = json!({
        "Sid": "ConditionedWildcard",
        "Effect": "Allow",
        "Principal": "*",
        "Action": "secretsmanager:GetSecretValue",
        "Resource": "*",
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// One row: the case, the `Condition` the wildcard statement carries, and whether that
/// condition binds the caller — a bound statement is NOT broad access, an unbound one is.
struct Row {
    case: &'static str,
    condition: Value,
    binds: bool,
}

/// The condition shapes the rule is asserted over.
fn rows() -> Vec<Row> {
    let account = this_account();
    let role = format!("arn:aws:iam::{account}:role/reader");
    vec![
        Row {
            case: "no Condition at all",
            condition: Value::Null,
            binds: false,
        },
        Row {
            case: "StringEquals on the principal's account",
            condition: json!({"StringEquals": {"aws:PrincipalAccount": account}}),
            binds: true,
        },
        Row {
            case: "StringEquals on the principal's organization",
            condition: json!({"StringEquals": {"aws:PrincipalOrgID": "o-exampleorgid"}}),
            binds: true,
        },
        Row {
            case: "ArnEquals on the principal's ARN",
            condition: json!({"ArnEquals": {"aws:PrincipalArn": role}}),
            binds: true,
        },
        Row {
            case: "StringEquals on the calling service's source ARN",
            condition: json!({"StringEquals": {
                "aws:SourceArn": format!("arn:aws:sns:us-west-2:{account}:rotations")
            }}),
            binds: true,
        },
        Row {
            case: "StringEquals on a principal tag",
            condition: json!({"StringEquals": {"aws:PrincipalTag/team": "payments"}}),
            binds: true,
        },
        Row {
            case: "StringEquals naming several accounts",
            condition: json!({"StringEquals": {
                "aws:PrincipalAccount": [account, "111122223333"]
            }}),
            binds: true,
        },
        Row {
            case: "StringEquals naming an account and the wildcard",
            condition: json!({"StringEquals": {"aws:PrincipalAccount": [account, "*"]}}),
            binds: false,
        },
        Row {
            case: "StringLike with a wildcard principal ARN",
            condition: json!({"StringLike": {"aws:PrincipalArn": "arn:aws:iam::*:role/*"}}),
            binds: false,
        },
        Row {
            case: "StringNotEquals on the principal's account",
            condition: json!({"StringNotEquals": {"aws:PrincipalAccount": "111122223333"}}),
            binds: false,
        },
        Row {
            case: "ArnNotLike on the principal's ARN",
            condition: json!({"ArnNotLike": {"aws:PrincipalArn": role}}),
            binds: false,
        },
        Row {
            case: "a set qualifier over a positive comparison",
            condition: json!({"ForAnyValue:StringEquals": {"aws:PrincipalAccount": account}}),
            binds: true,
        },
        Row {
            case: "a set qualifier over a NEGATED comparison",
            condition: json!({
                "ForAnyValue:StringNotEquals": {"aws:PrincipalAccount": "111122223333"}
            }),
            binds: false,
        },
        Row {
            case: "a set qualifier over an IfExists comparison",
            condition: json!({
                "ForAllValues:StringEqualsIfExists": {"aws:PrincipalAccount": account}
            }),
            binds: false,
        },
        Row {
            case: "the IfExists spelling, which a caller carrying no value satisfies",
            condition: json!({"StringEqualsIfExists": {"aws:PrincipalAccount": account}}),
            binds: false,
        },
        Row {
            case: "Null demanding the principal's account be PRESENT",
            condition: json!({"Null": {"aws:PrincipalAccount": "false"}}),
            binds: true,
        },
        Row {
            case: "Null demanding the principal's account be ABSENT",
            condition: json!({"Null": {"aws:PrincipalAccount": "true"}}),
            binds: false,
        },
        Row {
            case: "a key outside the caller-binding family",
            condition: json!({"StringEquals": {"secretsmanager:VersionStage": "AWSCURRENT"}}),
            binds: false,
        },
        Row {
            case: "an address range the caller must arrive on",
            condition: json!({"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}}),
            binds: true,
        },
        Row {
            case: "one address the caller must arrive on",
            condition: json!({"IpAddress": {"aws:SourceIp": "203.0.113.7"}}),
            binds: true,
        },
        Row {
            case: "a CIDR covering every address, which binds nobody",
            condition: json!({"IpAddress": {"aws:SourceIp": "0.0.0.0/0"}}),
            binds: false,
        },
        Row {
            case: "a CIDR covering every address of the other family",
            condition: json!({"IpAddress": {"aws:SourceIp": "::/0"}}),
            binds: false,
        },
        Row {
            case: "the flag demanding the caller BE an AWS service",
            condition: json!({"Bool": {"aws:PrincipalIsAWSService": "true"}}),
            binds: true,
        },
        Row {
            case: "the flag demanding the caller NOT be an AWS service, which binds nobody",
            condition: json!({"Bool": {"aws:PrincipalIsAWSService": "false"}}),
            binds: false,
        },
        Row {
            case: "a clause naming no value at all",
            condition: json!({"StringEquals": {"aws:PrincipalAccount": []}}),
            binds: false,
        },
        Row {
            case: "a bound clause beside an unbound one, which both have to satisfy",
            condition: json!({
                "StringEquals": {"aws:PrincipalAccount": account},
                "StringLike": {"secretsmanager:SecretId": "*"},
            }),
            binds: true,
        },
    ]
}

/// Create the secret `name` so there is a resource to attach a policy to.
async fn create_secret(router: &MultiServiceRouter, name: &str, case: &str) {
    let (status, parsed) = call(
        router,
        "CreateSecret",
        &json!({"Name": name, "SecretString": "the-current-value"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to succeed, got {status} with {parsed}"
    );
}

/// RULE: `BlockPublicPolicy` refuses exactly the wildcard-principal documents whose
/// `Condition` does NOT bind the request to a caller something names.
#[tokio::test]
async fn block_public_policy_admits_a_wildcard_a_condition_binds() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let name = format!("conditioned-wildcard-{index}");
        create_secret(&router, &name, case).await;
        let document = wildcard_policy(row.condition.clone());
        let (status, parsed) = call(
            &router,
            "PutResourcePolicy",
            &json!({
                "SecretId": name,
                "ResourcePolicy": document,
                "BlockPublicPolicy": true,
            }),
        )
        .await;
        if row.binds {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the bound wildcard to be attached, got {status} with \
                 {parsed} for {document}"
            );
            // Attached means attached: the document reads back in the layout this
            // service reports a policy in — the canonical rendering of the document
            // it parsed rather than the caller's bytes
            // (`tests/all/secret_resource_policy_reported_form.rs` pins that layout).
            let (read_status, read) = call(
                &router,
                "GetResourcePolicy",
                &json!({"SecretId": name}),
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected the policy to read back, got {read_status} with {read}"
            );
            let expected = ::service_common::iam_policy::canonical_document(&document);
            assert_eq!(
                read["ResourcePolicy"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the attached document back, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the unbound wildcard to be refused, got {status} with \
                 {parsed} for {document}"
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
            // And nothing was attached.
            let (read_status, read) = call(
                &router,
                "GetResourcePolicy",
                &json!({"SecretId": name}),
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected a readable secret, got {read_status} with {read}"
            );
            assert!(
                read["ResourcePolicy"].is_null(),
                "case {case:?}: a refused attach stored a policy, got {read}"
            );
        }
    }
}

/// RULE: the VALIDATING channel reports the same check against the same documents — it
/// reports the broad-access entry for exactly the documents the refusing channel blocks.
#[tokio::test]
async fn the_validating_channel_reports_the_same_check() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let document = wildcard_policy(row.condition.clone());
        let (status, parsed) = call(
            &router,
            "ValidateResourcePolicy",
            &json!({"ResourcePolicy": document}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the validation to answer 200, got {status} with {parsed}"
        );
        let reported: Vec<&str> = parsed["ValidationErrors"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["CheckName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported.contains(&PUBLIC_ACCESS_CHECK),
            !row.binds,
            "case {case:?}: expected the broad-access check reported={}, got {parsed} for \
             {document}",
            !row.binds
        );
        assert_eq!(
            parsed["PolicyValidationPassed"].as_bool(),
            Some(row.binds),
            "case {case:?}: expected PolicyValidationPassed={}, got {parsed}",
            row.binds
        );
    }
}
