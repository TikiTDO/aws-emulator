// SPDX-License-Identifier: CC0-1.0

//! `AdminAddUserToGroup` over the wire: group membership is PERSISTED on the
//! stored user and shows up in the tokens a later sign-in issues.
//!
//! The rule under test: `AdminAddUserToGroup` records, in the stored user named
//! by `Username` in the pool named by `UserPoolId`, membership of the stored
//! group named by `GroupName`; membership is a SET, so a repeated add is a
//! no-op that still succeeds; and every group the stored user belongs to is
//! carried by the access and identity tokens of a later sign-in as their
//! `cognito:groups` claim (`AdminAddUserToGroup`'s own model documentation),
//! while the refresh token carries none. An absent pool or group is a 400
//! `ResourceNotFoundException` and an absent user a 400 `UserNotFoundException`,
//! and a refused request persists nothing.
//!
//! The stored membership is read back here through the issued tokens'
//! `cognito:groups` claim; the membership listings have suites of their own.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// A base64url JWS segment's bytes (RFC 7515 §2): the alphabet substitutions
/// and the stripped padding are restored before decoding.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// The claim set of a compact-serialized JWS.
fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// The `cognito:groups` claim of `token`, or `None` when it carries none.
fn groups_claim(token: &str) -> Option<Vec<String>> {
    let claims = claims_of(token);
    let claim = claims.get("cognito:groups")?.clone();
    let names = claim
        .as_array()
        .unwrap_or_else(|| panic!("cognito:groups must be a list, got {claim}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("a group name must be a string, got {name}"))
                .to_string()
        })
        .collect();
    Some(names)
}

/// A pool with one app client that admits `ADMIN_USER_PASSWORD_AUTH`, returned
/// as (`UserPoolId`, `ClientId`).
async fn pool_with_client(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let pool = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-client"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a permanent password so it can sign in, and return
/// its generated `sub`.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    sub
}

/// Sign `username` in and return the `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    let result = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    result["AuthenticationResult"].clone()
}

/// One row of the membership table: the user's name, the groups it is added to
/// (in request order, with repeats where the row exercises idempotence) and the
/// distinct group names the stored membership must then hold.
struct MembershipCase {
    case: &'static str,
    username: &'static str,
    adds: Vec<String>,
}

impl MembershipCase {
    /// The distinct names the stored membership must hold after this row's adds.
    fn expected(&self) -> BTreeSet<String> {
        self.adds.iter().cloned().collect()
    }
}

/// Every group name here satisfies `GroupNameType` (length 1..128, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="AdminAddUserToGroup")`. The rows
/// differ along the COUNT of memberships (none, one, several), the FORM of the
/// names (ASCII, punctuation, non-ASCII letters, the length maximum) and
/// whether the same add is REPEATED.
fn membership_cases() -> Vec<MembershipCase> {
    vec![
        MembershipCase {
            case: "a user added to no group",
            username: "nobody",
            adds: vec![],
        },
        MembershipCase {
            case: "a user added to one group",
            username: "single",
            adds: vec!["admins".to_string()],
        },
        MembershipCase {
            case: "a user added to three groups, one name non-ASCII",
            username: "several",
            adds: vec![
                "admins".to_string(),
                "ops.team-1_a".to_string(),
                "グループ①".to_string(),
            ],
        },
        MembershipCase {
            case: "the same group added three times: membership is a set",
            username: "repeated",
            adds: vec![
                "readers".to_string(),
                "readers".to_string(),
                "readers".to_string(),
            ],
        },
        MembershipCase {
            case: "a group name at the modelled length maximum 128",
            username: "longname",
            adds: vec!["g".repeat(128)],
        },
    ]
}

/// Every group name any row of the table names, so the pool holds them all.
fn all_group_names() -> BTreeSet<String> {
    membership_cases()
        .iter()
        .flat_map(|case| case.adds.clone())
        .collect()
}

#[tokio::test]
async fn an_added_membership_is_persisted_and_shows_up_in_the_issued_tokens() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "membership-pool").await;
    for group_name in all_group_names() {
        ok(
            &router,
            "CreateGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;
    }

    for case in membership_cases() {
        let label = case.case;
        user(&router, &user_pool_id, case.username).await;
        for group_name in &case.adds {
            let (status, payload) = call(
                &router,
                "AdminAddUserToGroup",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": case.username,
                    "GroupName": group_name,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: adding {group_name:?} must succeed, got {status} with {payload}"
            );
        }

        let expected = case.expected();
        let result = sign_in(&router, &user_pool_id, &client_id, case.username).await;

        // The claim is carried by the access and the identity token, and by
        // neither the refresh token.
        for member in ["AccessToken", "IdToken"] {
            let token = result[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: {member} must be issued, got {result}"));
            let actual = groups_claim(token);
            if expected.is_empty() {
                assert!(
                    actual.is_none(),
                    "case {label:?}: {member} of a user in no group carries no cognito:groups claim, got {actual:?}"
                );
            } else {
                let names: BTreeSet<String> = actual
                    .clone()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: {member} must carry cognito:groups, got {}",
                            claims_of(token)
                        )
                    })
                    .into_iter()
                    .collect();
                assert_eq!(
                    names, expected,
                    "case {label:?}: {member} must list every stored membership once, got {actual:?}"
                );
                assert_eq!(
                    actual.as_ref().map(Vec::len),
                    Some(expected.len()),
                    "case {label:?}: {member} lists each group exactly once, got {actual:?}"
                );
            }
        }
        let refresh = result["RefreshToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: RefreshToken must be issued"));
        // The refresh token is SEALED: a JWE in compact serialization (RFC 7516 §7.1,
        // five segments) rather than the three of a JWS, so it states no claim.
        assert_eq!(
            refresh.split('.').count(),
            5,
            "case {label:?}: the refresh token is sealed in JWE compact serialization, \
             got {refresh}"
        );
        assert!(
            !refresh.contains(case.username),
            "case {label:?}: the refresh token is an opaque credential that states no \
             claim, found the user's own name in {refresh}"
        );

        // The membership is STORED, not an artefact of the first sign-in: a
        // second sign-in renders the same claim.
        let again = sign_in(&router, &user_pool_id, &client_id, case.username).await;
        let first = result["IdToken"].as_str().expect("an id token");
        let second = again["IdToken"].as_str().expect("an id token");
        assert_eq!(
            groups_claim(second),
            groups_claim(first),
            "case {label:?}: a later sign-in renders the same stored membership"
        );
    }
}

#[tokio::test]
async fn a_membership_belongs_to_one_user_of_one_pool() {
    let router = router();
    let (first_pool, first_client) = pool_with_client(&router, "first-pool").await;
    let (second_pool, second_client) = pool_with_client(&router, "second-pool").await;

    // The same group name and the same username exist in both pools; only the
    // first pool's user is added to the group.
    for user_pool_id in [&first_pool, &second_pool] {
        ok(
            &router,
            "CreateGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
        )
        .await;
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
        }
    }
    ok(
        &router,
        "AdminAddUserToGroup",
        &json!({ "UserPoolId": first_pool, "Username": "alice", "GroupName": "admins" }),
    )
    .await;

    for (label, user_pool_id, client_id, username, expected) in [
        (
            "the added user of the pool it was added in",
            &first_pool,
            &first_client,
            "alice",
            Some(vec!["admins".to_string()]),
        ),
        (
            "another user of the same pool",
            &first_pool,
            &first_client,
            "bob",
            None,
        ),
        (
            "the same username in another pool",
            &second_pool,
            &second_client,
            "alice",
            None,
        ),
    ] {
        let result = sign_in(&router, user_pool_id, client_id, username).await;
        let token = result["IdToken"].as_str().expect("an id token");
        let actual = groups_claim(token);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_user_is_resolved_by_username_or_by_sub() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "resolve-pool").await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "by-sub" }),
    )
    .await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "by-username" }),
    )
    .await;
    let sub = user(&router, &user_pool_id, "carol").await;

    // `Username` "must be the sub of a local user or the username" — both
    // resolve to the same stored user, so both memberships land on it.
    for (label, identifier, group_name) in [
        ("by username", "carol".to_string(), "by-username"),
        ("by sub", sub, "by-sub"),
    ] {
        let (status, payload) = call(
            &router,
            "AdminAddUserToGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": identifier,
                "GroupName": group_name,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the user must resolve, got {status} with {payload}"
        );
    }

    let result = sign_in(&router, &user_pool_id, &client_id, "carol").await;
    let token = result["IdToken"].as_str().expect("an id token");
    let actual = groups_claim(token);
    let expected: BTreeSet<String> = ["by-sub".to_string(), "by-username".to_string()].into();
    assert_eq!(
        actual.clone().map(BTreeSet::from_iter),
        Some(expected.clone()),
        "both identifiers name one stored user: expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn an_absent_pool_group_or_user_is_refused_and_persists_nothing() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "refusal-pool").await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    user(&router, &user_pool_id, "dave").await;

    for (label, body, expected) in [
        (
            "an absent user pool",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Username": "dave",
                "GroupName": "admins",
            }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a group the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "absent-group",
            }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a group name differing only in case, which is a different group",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "Admins",
            }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a user the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "absent-user",
                "GroupName": "admins",
            }),
            (400, "UserNotFoundException"),
        ),
    ] {
        let (status, payload) = call(&router, "AdminAddUserToGroup", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Nothing above was persisted: the stored user still belongs to no group.
    let result = sign_in(&router, &user_pool_id, &client_id, "dave").await;
    let token = result["IdToken"].as_str().expect("an id token");
    let actual = groups_claim(token);
    assert!(
        actual.is_none(),
        "a refused add persists nothing, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let (user_pool_id, _client_id) = pool_with_client(&router, "constraint-pool").await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;

    // One value past each published bound of `AdminAddUserToGroup`'s input
    // (`smithy_query(query="operation",operation="AdminAddUserToGroup")`):
    // `GroupName` and `Username` length 1..128 and pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, and `UserPoolId`'s pattern.
    for (label, body) in [
        (
            "an empty group name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Username": "dave", "GroupName": "" }),
        ),
        (
            "a group name of 129 characters, past the length maximum 128",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "g".repeat(129),
            }),
        ),
        (
            "a group name holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Username": "dave", "GroupName": "two words" }),
        ),
        (
            "an empty username, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Username": "", "GroupName": "admins" }),
        ),
        (
            "a username of 129 characters, past the length maximum 128",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "u".repeat(129),
                "GroupName": "admins",
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Username": "dave", "GroupName": "admins" }),
        ),
        (
            "a request missing the required GroupName",
            json!({ "UserPoolId": user_pool_id, "Username": "dave" }),
        ),
    ] {
        let (status, payload) = call(&router, "AdminAddUserToGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }
}
