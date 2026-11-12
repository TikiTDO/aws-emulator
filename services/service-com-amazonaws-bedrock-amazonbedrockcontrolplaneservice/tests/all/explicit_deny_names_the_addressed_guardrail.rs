// SPDX-License-Identifier: CC0-1.0

//! WHICH resource an authorization refusal names when a guardrail read is covered by an
//! explicit `Deny`, driven over the real wire path (a restJson1 request through the
//! multi-service router into this service mounted beside IAM and STS on one engine), so
//! what is asserted is what a client sees.
//!
//! THE RULE UNDER TEST: IAM's refusal names the resource the REQUEST addressed — for a read
//! whose identifier label admits a bare resource id beside the whole ARN, the ARN that label
//! resolves to (`GuardrailIdentifier` is
//! `^(([a-z0-9]+)|(arn:…:guardrail/[a-z0-9]+))$` and `GuardrailArn`
//! `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$`, which admits
//! no wildcard:
//! `smithy_query(query="operation",operation="GetGuardrail")`) — and NEVER the `Resource`
//! element of the statement that refused it, whatever that element was scoped to. A client
//! parses that segment to learn which resource was refused, so the two halves are asserted
//! together: the sentence is compared whole, and the family's wildcard ARN must appear
//! nowhere in it.
//!
//! The rows span what the rule generalises over:
//!
//! * how the request NAMES the guardrail — by its bare `guardrailId`, by the whole
//!   `guardrailArn` percent-encoded into the same label, and either of those with the
//!   `guardrailVersion` query the operation also binds (the `DRAFT` and a published version);
//! * what the refusing statement's `Resource` element is scoped to — IAM's own wildcard, the
//!   guardrail's own ARN, the family's collection, and a list naming the collection beside
//!   another family's — none of which may reach the sentence.
//!
//! The guardrail under test is a real one: it is created, versioned, and carries a Bedrock
//! RESOURCE policy of the documented guardrail shape, which is the setting the identity-based
//! decision is taken in — a resource policy granting the account the action does not lift an
//! explicit identity-based `Deny`, and the refusal still names the guardrail rather than
//! anything either policy mentions.
//!
//! The emulator, the signed call, the seeded session and the policy documents are the harness
//! `crate::custom_model_authorization` owns, reused rather than copied: one rule, one dispatch
//! path.

use serde_json::{json, Value};

use crate::custom_model_authorization::{
    emulator, given_role_policy, given_session, session_arn, Verdict, ACCOUNT_KEY_ID,
    SESSION_KEY_ID,
};
use crate::support::{label, Reply, ACCOUNT, REGION};

/// The ARN of every resource of `resource_type` in this account and Region — the collection
/// ARN a request addressing no single member of the family is judged against, and the value
/// a refusal of a request that DOES name one must never carry.
fn collection_of(resource_type: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/*")
}

/// A policy allowing everything and denying `action` on `resource` — the shape a
/// least-privilege guardrail takes, and the one the refusal must attribute to an explicit
/// deny.
fn allow_all_but_on(action: &str, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "*", "Resource": "*" },
            { "Effect": "Deny", "Action": action, "Resource": resource },
        ]
    })
}

/// A policy allowing `action` on `resource` ALONE.
fn allow_on(action: &str, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }]
    })
}

/// The `message` a reply carries, or the empty string when it carries none.
fn message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The refusal one row expects: the modelled 403, the modelled error code, and IAM's own
/// sentence with every part — principal, action, resource, reason — taken from the request
/// and the stored policy, the resource being the ARN of the guardrail the request addressed.
fn assert_denied_about(case: &str, reply: &Reply, action: &str, resource: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected the modelled 403, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("AccessDeniedException"),
        "case {case:?}: expected AccessDeniedException, got {:?}",
        reply.error
    );
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {}",
        session_arn(),
        Verdict::ExplicitDeny.reason(action)
    );
    let stated = message(reply);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected IAM's wording {expected:?}, got {stated:?}"
    );
    // The defect this suite pins: the resource segment is the guardrail the request named,
    // so the family's wildcard ARN appears nowhere in the sentence — a caller refused one
    // guardrail out of many must be able to read which.
    assert!(
        !stated.contains(&collection_of("guardrail")),
        "case {case:?}: the sentence must name the guardrail the request addressed rather \
         than the family's wildcard, got {stated:?}"
    );
}

/// The guardrail every row addresses: created, versioned, and governed by a resource policy
/// of the documented guardrail shape. Hands back its id, its ARN and the version published
/// on it.
async fn given_guardrail(
    emu: &crate::custom_model_authorization::Emulator,
) -> (String, String, String) {
    let created = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            "/guardrails",
            Some(json!({
                "name": "denial-names-the-guardrail",
                "blockedInputMessaging": "Input blocked by the denial suite.",
                "blockedOutputsMessaging": "Output blocked by the denial suite.",
                "wordPolicyConfig": { "wordsConfig": [{ "text": "forbidden" }] },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "setup: CreateGuardrail must answer 202, got {} {:?}",
        created.status, created.body
    );
    let id = created.text("guardrailId");
    let arn = created.text("guardrailArn");

    let versioned = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            &format!("/guardrails/{}", label(&id)),
            Some(json!({ "description": "published by the denial suite" })),
        )
        .await;
    assert_eq!(
        versioned.status, 202,
        "setup: CreateGuardrailVersion must answer 202, got {} {:?}",
        versioned.status, versioned.body
    );
    let version = versioned.text("version");

    // The documented guardrail resource policy: one `Allow` for the account, naming the
    // guardrail's own ARN. It grants the action the identity-based `Deny` below withholds,
    // which is what makes the rows state WHICH decision answers.
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": ["bedrock:ApplyGuardrail", "bedrock:GetGuardrail"],
            "Resource": arn,
        }],
    })
    .to_string();
    let put = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            "/resource-policy",
            Some(json!({ "resourceArn": arn, "resourcePolicy": document })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "setup: PutResourcePolicy must answer 201, got {} {:?}",
        put.status, put.body
    );
    let read_back = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "GET",
            &format!("/resource-policy/{}", label(&arn)),
            None,
        )
        .await;
    assert_eq!(
        read_back.text("resourcePolicy"),
        document,
        "setup: GetResourcePolicy must round-trip the document, got {:?}",
        read_back.body
    );

    (id, arn, version)
}

/// Every way a request names that one guardrail: the label's two spellings, each on its own
/// and with the `guardrailVersion` query the operation also binds.
fn addressings(id: &str, arn: &str, version: &str) -> Vec<(String, String)> {
    vec![
        (
            "by its bare guardrailId".to_string(),
            format!("/guardrails/{}", label(id)),
        ),
        (
            "by its bare guardrailId, asking for the DRAFT".to_string(),
            format!("/guardrails/{}?guardrailVersion=DRAFT", label(id)),
        ),
        (
            format!("by its bare guardrailId, asking for version {version}"),
            format!("/guardrails/{}?guardrailVersion={version}", label(id)),
        ),
        (
            "by the whole guardrail ARN".to_string(),
            format!("/guardrails/{}", label(arn)),
        ),
        (
            format!("by the whole guardrail ARN, asking for version {version}"),
            format!("/guardrails/{}?guardrailVersion={version}", label(arn)),
        ),
    ]
}

/// Every `Resource` element a statement denying the read can be scoped to that still covers
/// this guardrail — none of which may reach the refusal's sentence.
fn deny_scopes(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("a Deny scoped to IAM's own wildcard", json!("*")),
        (
            "a Deny scoped to the guardrail's own ARN",
            json!(arn.to_string()),
        ),
        (
            "a Deny scoped to the family's collection",
            json!(collection_of("guardrail")),
        ),
        (
            "a Deny listing the family's collection beside another family's",
            json!([collection_of("guardrail"), collection_of("imported-model")]),
        ),
    ]
}

/// RULE: every way of naming one guardrail, under every scoping of the statement that denies
/// the read, is refused with IAM's sentence about THAT guardrail's ARN — and a policy
/// allowing the action on that same ARN serves the identical calls, which is what shows the
/// resource the decision was taken about is the one the sentence named.
#[tokio::test]
async fn an_explicit_deny_of_a_guardrail_read_names_the_guardrail_the_request_addressed() {
    let emu = emulator();
    given_session(&emu.engine);
    let (id, arn, version) = given_guardrail(&emu).await;
    let action = "bedrock:GetGuardrail";

    for (addressing, target) in addressings(&id, &arn, &version) {
        for (deny_case, scope) in deny_scopes(&arn) {
            given_role_policy(
                &emu.engine,
                Some(&allow_all_but_on(action, scope.clone())),
            );
            let refused = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
            let case = format!("a guardrail read {addressing} under {deny_case}");
            assert_denied_about(&case, &refused, action, &arn);
            // The decision stands in front of the handler, so nothing of the guardrail is
            // disclosed with the refusal.
            for withheld in ["guardrailId", "guardrailArn", "name", "status"] {
                assert_eq!(
                    refused.get(withheld),
                    Value::Null,
                    "case {case:?}: the refusal must disclose no {withheld}, got {:?}",
                    refused.body
                );
            }
        }
    }

    // The positive control: the resource the refusals named is the resource the decision is
    // taken about, so a policy allowing the action on exactly that ARN serves every one of
    // the same calls.
    given_role_policy(&emu.engine, Some(&allow_on(action, json!(arn.clone()))));
    for (addressing, target) in addressings(&id, &arn, &version) {
        let served = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
        assert_eq!(
            served.status, 200,
            "case {addressing:?}: a policy scoped to {arn} must serve the read, got {} {:?}",
            served.status, served.body
        );
        assert_eq!(
            served.text("guardrailArn"),
            arn,
            "case {addressing:?}: the read must report the guardrail the request addressed, \
             got {:?}",
            served.body
        );
    }
}

/// RULE, the contrast: the statement is matched against the ARN the request addressed, so a
/// `Deny` scoped to a resource that is NOT this guardrail — another member of its family, or
/// another family's collection — leaves the read served.
#[tokio::test]
async fn a_deny_scoped_elsewhere_leaves_the_guardrail_read_served() {
    let emu = emulator();
    given_session(&emu.engine);
    let (id, arn, version) = given_guardrail(&emu).await;
    let action = "bedrock:GetGuardrail";

    // Another member of the same family (the same id form, a different id), and another
    // family altogether.
    let elsewhere: &[(&str, String)] = &[
        (
            "another member of the same family",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/zzzzzzzzzzzz"),
        ),
        (
            "another family's collection",
            collection_of("imported-model"),
        ),
    ];

    for (scope_case, scope) in elsewhere {
        given_role_policy(
            &emu.engine,
            Some(&allow_all_but_on(action, json!(scope.clone()))),
        );
        for (addressing, target) in addressings(&id, &arn, &version) {
            let reply = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
            let case = format!("a guardrail read {addressing} under a Deny scoped to {scope_case}");
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a Deny naming {scope} does not cover {arn}, so the read must \
                 be served, got {} {:?}",
                reply.status, reply.body
            );
        }
    }
}
