// SPDX-License-Identifier: CC0-1.0

//! Which `principal` a cross-account grant on a signing profile may name, end to
//! end over HTTP.
//!
//! The rule under test: `AddProfilePermission$principal` carries an AWS
//! PRINCIPAL — "The AWS principal receiving cross-account permissions. This may
//! be an IAM role or another AWS account ID"
//! (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
//! member="principal")`) — so a value in one of the forms AWS's policy grammar
//! publishes for the `AWS` key of a statement's `Principal` (an account id, an
//! account root ARN, an IAM user or role ARN, a role-session ARN) is written and
//! reported back verbatim, while a value in none of them is refused 400
//! `ValidationException` and writes nothing: no statement, no new revision, no
//! larger `policySizeBytes`.
//!
//! The rows span what the rule generalises over: the form of the principal
//! (bare account id, which identity service the ARN names, which resource type,
//! whether it carries an IAM path or a session name, which partition), the way a
//! value falls outside the forms (not an ARN at all, the wrong digit count, a
//! resource type that is no identity, an identity ARN carrying a region, an
//! empty name, a service principal, an ARN of another service) and its size —
//! including a value past the ceiling a refusal quotes a request value through.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpLabel` binding, deserialization, the handler, the persisted policy and
//! serialization — the path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `code` a refusal on the request's own values carries. Spelled out here
/// rather than imported so a wrong code in the handler fails this suite.
const ERROR_CODE_VALIDATION: &str = "ValidationException";

/// The `code` a read of a profile that holds no permission policy carries.
const ERROR_CODE_POLICY_NOT_FOUND: &str = "PolicyNotFound";

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is
/// one `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// An action a cross-account grant may carry on a profile of [`PLATFORM_ID`].
/// Source: `AddProfilePermissionRequest$action` (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="action")`).
const GRANTABLE_ACTION: &str = "signer:GetSigningProfile";

/// The ceiling this service states for a request value a refusal quotes, written
/// out rather than imported: the bytes a client reads are pinned here
/// independently of the service's own constant.
const ECHO_BOUND: usize = 16 * 1024;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> &str {
        self.body[member].as_str().unwrap_or_default()
    }

    /// The principals the reported statements name, keyed by `statementId`.
    fn principals(&self) -> Vec<(String, String)> {
        self.body["permissions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "ListProfilePermissions must report a permissions list: {:?}",
                    self.body
                )
            })
            .iter()
            .map(|permission| {
                (
                    permission["statementId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    permission["principal"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    }

    /// `policySizeBytes`, which `ListProfilePermissions` declares required.
    fn policy_size_bytes(&self) -> i64 {
        self.body["policySizeBytes"].as_i64().unwrap_or_else(|| {
            panic!(
                "ListProfilePermissions must report policySizeBytes: {:?}",
                self.body
            )
        })
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A profile of `name`, and the version `PutSigningProfile` minted for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("profileVersion").to_string()
}

async fn add_permission(
    router: &MultiServiceRouter,
    name: &str,
    statement_id: &str,
    principal: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/signing-profiles/{name}/permissions"),
        Some(json!({
            "action": GRANTABLE_ACTION,
            "principal": principal,
            "statementId": statement_id,
        })),
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-profiles/{name}/permissions"),
        None,
    )
    .await
}

/// The principal forms a grant may name, a row per class the rule generalises
/// over.
///
/// Source for the ARN rows: the `AWS` key of a policy statement's `Principal`,
/// which AWS's policy grammar publishes as an account id, an account root ARN,
/// an IAM user or role ARN (either of which may carry a path) and a role-session
/// ARN (an assumed role, a federated user). Only the account-id and the plain
/// role-ARN rows are forms the issue cited.
fn accepted_principals() -> Vec<(&'static str, String)> {
    vec![
        ("a bare account id", "123456789012".to_string()),
        (
            "an account id whose digits are all zero",
            "000000000000".to_string(),
        ),
        (
            "an account root ARN",
            "arn:aws:iam::210987654321:root".to_string(),
        ),
        (
            "an IAM role ARN",
            "arn:aws:iam::123456789012:role/CodeSigner".to_string(),
        ),
        (
            "an IAM role ARN carrying a path",
            "arn:aws:iam::123456789012:role/service-role/Signer_Role".to_string(),
        ),
        (
            "an IAM user ARN",
            "arn:aws:iam::210987654321:user/Bob".to_string(),
        ),
        (
            "an IAM user ARN carrying a path",
            "arn:aws:iam::210987654321:user/division_abc/subdivision_xyz/Bob".to_string(),
        ),
        (
            "a role-session ARN",
            "arn:aws:sts::123456789012:assumed-role/CodeSigner/release-build".to_string(),
        ),
        (
            "a federated-user ARN",
            "arn:aws:sts::123456789012:federated-user/Carol".to_string(),
        ),
        (
            "a role ARN of another partition",
            "arn:aws-cn:iam::123456789012:role/CodeSigner".to_string(),
        ),
        (
            "a role ARN whose name is as long as a policy admits",
            format!("arn:aws:iam::123456789012:role/{}", "r".repeat(900)),
        ),
    ]
}

/// The values that are no AWS principal, a row per way of falling outside the
/// forms above.
fn refused_principals() -> Vec<(&'static str, String)> {
    vec![
        (
            "text that is no identifier at all",
            "not-a-principal".to_string(),
        ),
        ("text carrying a space", "hello world".to_string()),
        ("too few digits for an account id", "12345".to_string()),
        (
            "one digit too many for an account id",
            "1234567890123".to_string(),
        ),
        (
            "twelve characters, one of them not a digit",
            "12345678901a".to_string(),
        ),
        (
            "twelve digits with surrounding space",
            " 123456789012 ".to_string(),
        ),
        ("no principal at all", String::new()),
        ("whitespace only", "   ".to_string()),
        ("a wildcard naming no account", "*".to_string()),
        (
            "an ARN missing its resource",
            "arn:aws:iam::bogus".to_string(),
        ),
        (
            "an identity ARN whose account is not an account id",
            "arn:aws:iam::bogus:role/CodeSigner".to_string(),
        ),
        (
            "an identity ARN carrying a region",
            "arn:aws:iam:us-west-2:123456789012:role/CodeSigner".to_string(),
        ),
        (
            "an identity ARN with no partition",
            "arn::iam::123456789012:role/CodeSigner".to_string(),
        ),
        (
            "an IAM resource type that is no principal",
            "arn:aws:iam::123456789012:group/Admins".to_string(),
        ),
        (
            "a root ARN with something after it",
            "arn:aws:iam::123456789012:root/extra".to_string(),
        ),
        (
            "a role ARN carrying no name",
            "arn:aws:iam::123456789012:role/".to_string(),
        ),
        (
            "a role session carrying no session name",
            "arn:aws:sts::123456789012:assumed-role/CodeSigner".to_string(),
        ),
        (
            "an ARN of a service that holds no identities",
            "arn:aws:s3:::my-bucket".to_string(),
        ),
        (
            "a service principal rather than an AWS principal",
            "lambda.amazonaws.com".to_string(),
        ),
        (
            "an account id with a leading plus",
            "+23456789012".to_string(),
        ),
    ]
}

/// Every form AWS publishes for an `AWS` principal is written and read back
/// exactly as it arrived.
#[tokio::test]
async fn a_grant_naming_an_aws_principal_is_written_and_read_back() {
    let router = emulator();
    seed_profile(&router, "Principal_Profile").await;

    let mut expected: Vec<(String, String)> = Vec::new();
    for (index, (case, principal)) in accepted_principals().into_iter().enumerate() {
        let statement_id = format!("grant-{index:02}");
        let answer =
            add_permission(&router, "Principal_Profile", &statement_id, &principal).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.str_member("revisionId").is_empty(),
            "case {case:?}: expected the write to report the revision the policy moved to, \
             got {:?}",
            answer.body
        );
        expected.push((statement_id, principal));
        expected.sort();

        // Read back through the implemented reader: the stored statement names
        // the principal the grant carried, byte for byte.
        let listed = list_permissions(&router, "Principal_Profile").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: list expected 200, got {} {:?}",
            listed.status, listed.body
        );
        let mut reported = listed.principals();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the policy to hold {expected:?}, got {reported:?}"
        );
    }
}

/// A value in none of those forms is refused 400 `ValidationException`, and the
/// refusal names the member and quotes the value.
#[tokio::test]
async fn a_grant_naming_no_aws_principal_is_refused() {
    let router = emulator();
    seed_profile(&router, "Refused_Profile").await;

    for (case, principal) in refused_principals() {
        let answer = add_permission(&router, "Refused_Profile", "refused", &principal).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for principal {principal:?}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_VALIDATION),
            "case {case:?}: expected code {ERROR_CODE_VALIDATION}, got {:?}",
            answer.body
        );
        assert!(
            answer.message().contains("principal"),
            "case {case:?}: expected the refusal to name the member, got {:?}",
            answer.message()
        );
        if !principal.trim().is_empty() {
            assert!(
                answer.message().contains(principal.trim()),
                "case {case:?}: expected the refusal to quote the value {principal:?}, got {:?}",
                answer.message()
            );
        }
    }

    // Not one of them wrote a statement, so the profile holds no policy at all.
    let listed = list_permissions(&router, "Refused_Profile").await;
    assert_eq!(
        listed.status, 404,
        "expected the refused grants to write nothing, so no policy to read, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.code(),
        Some(ERROR_CODE_POLICY_NOT_FOUND),
        "expected the refused grants to leave no policy, got {:?}",
        listed.body
    );
}

/// A refused grant leaves a policy the profile ALREADY holds exactly as it was:
/// the same statements, the same revision, the same size.
#[tokio::test]
async fn a_refused_grant_leaves_the_policy_and_its_revision_alone() {
    let router = emulator();
    seed_profile(&router, "Untouched_Profile").await;
    let seeded = add_permission(&router, "Untouched_Profile", "held", "123456789012").await;
    assert_eq!(
        seeded.status, 200,
        "seeding a statement: expected 200, got {} {:?}",
        seeded.status, seeded.body
    );
    let before = list_permissions(&router, "Untouched_Profile").await;

    for (case, principal) in refused_principals() {
        let refused = add_permission(
            &router,
            "Untouched_Profile",
            &format!("refused-{case}"),
            &principal,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );

        let after = list_permissions(&router, "Untouched_Profile").await;
        assert_eq!(
            after.principals(),
            before.principals(),
            "case {case:?}: expected the policy to hold what it held, got {:?}",
            after.principals()
        );
        assert_eq!(
            after.str_member("revisionId"),
            before.str_member("revisionId"),
            "case {case:?}: expected the policy's revision to be unchanged, got {:?}",
            after.str_member("revisionId")
        );
        assert_eq!(
            after.policy_size_bytes(),
            before.policy_size_bytes(),
            "case {case:?}: expected policySizeBytes to be unchanged, got {}",
            after.policy_size_bytes()
        );
    }
}

/// The refusal quotes the value through the one ceiling this service applies to
/// a clause that states a request value whose member carries no modelled bound:
/// an oversized principal cannot make the error body as large as itself.
#[tokio::test]
async fn an_oversized_principal_is_refused_with_a_bounded_echo() {
    let router = emulator();
    seed_profile(&router, "Oversized_Profile").await;

    // Rows differ in size around the ceiling, the last one in character width,
    // so the cut has to walk back off a multi-byte character.
    for (case, principal) in [
        ("well inside the ceiling", "n".repeat(1_000)),
        ("exactly on the ceiling", "n".repeat(ECHO_BOUND)),
        ("one byte past the ceiling", "n".repeat(ECHO_BOUND + 1)),
        ("far past the ceiling", "n".repeat(4 * ECHO_BOUND)),
        ("multi-byte characters past the ceiling", "€".repeat(6_000)),
    ] {
        let answer = add_permission(&router, "Oversized_Profile", "oversized", &principal).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains(&quoted(&principal)),
            "case {case:?}: expected the refusal to quote the value as {:?}, got {:?}",
            short(&quoted(&principal)),
            short(answer.message())
        );
        assert!(
            answer.message().len() < principal.len() + ECHO_BOUND,
            "case {case:?}: expected the refusal to be bounded whatever the value's size, \
             got {} bytes for a {}-byte value",
            answer.message().len(),
            principal.len()
        );
    }
}

/// `value` as a clause of a refusal quotes it: itself up to the ceiling, past it
/// the prefix that fits — cut on a character boundary — followed by the whole
/// value's byte count. Written out here rather than taken from the service, so
/// the two cannot agree by construction.
fn quoted(value: &str) -> String {
    if value.len() <= ECHO_BOUND {
        return value.to_string();
    }
    let mut end = ECHO_BOUND;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// `value`'s first characters, for an assertion message that must not carry
/// kilobytes of a test value.
fn short(value: &str) -> String {
    value.chars().take(40).collect()
}
