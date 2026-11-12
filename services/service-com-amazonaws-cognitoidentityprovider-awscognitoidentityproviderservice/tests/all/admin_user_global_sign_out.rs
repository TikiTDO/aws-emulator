// SPDX-License-Identifier: CC0-1.0

//! `AdminUserGlobalSignOut` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: an administrative global sign-out invalidates EVERY token
//! this emulator has issued to the stored user the request names — access, id and
//! refresh alike, from every outstanding sign-in — so none of them authorizes a
//! later request and none of them is exchanged for new tokens, while the stored
//! user profile itself is untouched and the user signs in again into fresh
//! tokens. The call lands on one user of one pool, the user resolving by its
//! username or by its `sub`, and a request naming a pool or a user that is not
//! stored is refused and revokes nothing.
//!
//! A refresh token the service DID issue for the app client and has since revoked
//! is refused as revoked, which is a different sentence from the one a refresh
//! token the service cannot resolve at all gets.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy, so a user can sign in
/// and the tokens the sign-in issues can be presented afterwards.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The sentence a refresh-token sign-in is refused with when the token it
/// presents is one this service issued for that app client and has since revoked.
const REVOKED_REFRESH_TOKEN: &str = "Refresh Token has been revoked";

/// The sentence a refresh-token sign-in is refused with when the token it
/// presents resolves to no refresh token of that app client at all. It ends in
/// no period, as the sentence Amazon Cognito denies a refresh token with does
/// not.
const INVALID_REFRESH_TOKEN: &str = "Invalid Refresh Token";

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
    let raw = payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.split('#').next_back().unwrap_or(raw)
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client of `user_pool_id` that allows the password sign-in and
/// the refresh-token flow, and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` in `user_pool_id` with a permanent password, so it is
/// `CONFIRMED` and can sign in, and return its generated `sub`.
async fn confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
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

/// The three tokens one sign-in issues.
#[derive(Debug, Clone)]
struct Tokens {
    access: String,
    id: String,
    refresh: String,
}

/// Sign `username` in on `client_id` and return the tokens the sign-in issues.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Tokens {
    let result = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let authentication = &result["AuthenticationResult"];
    let field = |name: &str| {
        authentication[name]
            .as_str()
            .unwrap_or_else(|| panic!("the sign-in must issue a {name}, got {result}"))
            .to_string()
    };
    Tokens {
        access: field("AccessToken"),
        id: field("IdToken"),
        refresh: field("RefreshToken"),
    }
}

/// Present `access_token` to an operation that identifies its caller by it, and
/// return the status and body.
async fn present_access_token(router: &MultiServiceRouter, access_token: &str) -> (u16, Value) {
    call(router, "GetUser", &json!({ "AccessToken": access_token })).await
}

/// Present `refresh_token` to the refresh-token sign-in of `client_id`, and
/// return the status and body.
async fn present_refresh_token(
    router: &MultiServiceRouter,
    client_id: &str,
    refresh_token: &str,
) -> (u16, Value) {
    call(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "REFRESH_TOKEN_AUTH",
            "AuthParameters": { "REFRESH_TOKEN": refresh_token },
        }),
    )
    .await
}

/// Sign `identifier` out of `user_pool_id`, and return the status and body.
async fn sign_out(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    identifier: &str,
) -> (u16, Value) {
    call(
        router,
        "AdminUserGlobalSignOut",
        &json!({ "UserPoolId": user_pool_id, "Username": identifier }),
    )
    .await
}

/// Assert that `tokens` no longer authorize anything: the access token names no
/// signed-in user, and the refresh token is refused AS REVOKED.
async fn assert_signed_out(
    router: &MultiServiceRouter,
    case: &str,
    client_id: &str,
    tokens: &Tokens,
) {
    for (what, token) in [("access token", &tokens.access), ("id token", &tokens.id)] {
        let (status, payload) = present_access_token(router, token).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: the {what} must not authorize a request after the sign-out, \
             got {status} with {payload}"
        );
    }
    let (status, payload) = present_refresh_token(router, client_id, &tokens.refresh).await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (400, "NotAuthorizedException", REVOKED_REFRESH_TOKEN),
        "case {case:?}: expected the refresh token to be refused as revoked, \
         got {status} with {payload}"
    );
}

/// Which identifier the sign-out names the user by.
#[derive(Debug, Clone, Copy)]
enum IdentifyBy {
    /// The stored username.
    Username,
    /// The user's `sub`, which `Username` also accepts.
    Sub,
}

/// One row: the username form, how many outstanding sign-ins the user holds when
/// it is signed out, and the identifier the sign-out names it by.
#[derive(Debug)]
struct Row {
    case: &'static str,
    username: String,
    sign_ins: usize,
    identify_by: IdentifyBy,
}

/// The rows differ along the FORM and LENGTH of the username (each value
/// satisfying `UsernameType`, length 1..128 and pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="operation",operation="AdminUserGlobalSignOut")`), the
/// COUNT of outstanding sign-ins the one call has to invalidate, and the
/// IDENTIFIER the call names the user by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one sign-in, signed out by username",
            username: "alice".to_string(),
            sign_ins: 1,
            identify_by: IdentifyBy::Username,
        },
        Row {
            case: "one sign-in, signed out by sub",
            username: "bob".to_string(),
            sign_ins: 1,
            identify_by: IdentifyBy::Sub,
        },
        Row {
            case: "two outstanding sign-ins, signed out by username",
            username: "a.b-c_d+e".to_string(),
            sign_ins: 2,
            identify_by: IdentifyBy::Username,
        },
        Row {
            case: "three outstanding sign-ins, signed out by sub",
            username: "ユーザー①".to_string(),
            sign_ins: 3,
            identify_by: IdentifyBy::Sub,
        },
        Row {
            case: "a username at the modelled length minimum 1",
            username: "u".to_string(),
            sign_ins: 2,
            identify_by: IdentifyBy::Username,
        },
        Row {
            case: "a username at the modelled length maximum 128",
            username: "u".repeat(128),
            sign_ins: 1,
            identify_by: IdentifyBy::Sub,
        },
    ]
}

#[tokio::test]
async fn one_call_invalidates_every_token_the_named_user_holds() {
    let router = router();
    let user_pool_id = pool(&router, "sign-out-pool").await;
    let client_id = client(&router, &user_pool_id, "sign-out-client").await;

    for row in rows() {
        let case = row.case;
        let sub = confirmed_user(&router, &user_pool_id, &row.username).await;

        // Every outstanding sign-in's tokens, each one authorizing before the
        // sign-out: presenting an access token is a READ, so the state the
        // sign-out then acts on is the one the sign-ins left.
        let mut held = Vec::new();
        for _ in 0..row.sign_ins {
            let tokens = sign_in(&router, &client_id, &row.username).await;
            let (status, payload) = present_access_token(&router, &tokens.access).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the held access token authorizes the request before the \
                 sign-out, got {status} with {payload}"
            );
            held.push(tokens);
        }

        let identifier = match row.identify_by {
            IdentifyBy::Username => row.username.clone(),
            IdentifyBy::Sub => sub.clone(),
        };
        let (status, payload) = sign_out(&router, &user_pool_id, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the sign-out must succeed, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            Value::Object(serde_json::Map::new()),
            "case {case:?}: the sign-out answers an empty body, got {payload}"
        );

        for (index, tokens) in held.iter().enumerate() {
            let labelled = format!("{case} (sign-in {index})");
            assert_signed_out(&router, &labelled, &client_id, tokens).await;
        }
    }
}

#[tokio::test]
async fn a_sign_out_touches_one_user_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;
    let first_client = client(&router, &first_pool, "first-client").await;
    let second_client = client(&router, &second_pool, "second-client").await;

    // The same usernames exist in both pools, each with a sign-in outstanding;
    // only the first pool's `alice` is signed out.
    let mut held = Vec::new();
    for (user_pool_id, client_id) in [
        (&first_pool, &first_client),
        (&second_pool, &second_client),
    ] {
        for username in ["alice", "bob"] {
            confirmed_user(&router, user_pool_id, username).await;
            let tokens = sign_in(&router, client_id, username).await;
            held.push((user_pool_id.clone(), client_id.clone(), username, tokens));
        }
    }

    let (status, payload) = sign_out(&router, &first_pool, "alice").await;
    assert_eq!(
        status, 200,
        "the sign-out must succeed, got {status} with {payload}"
    );

    for (user_pool_id, client_id, username, tokens) in &held {
        let signed_out = user_pool_id == &first_pool && *username == "alice";
        let case = format!("{username} of {user_pool_id}");
        if signed_out {
            assert_signed_out(&router, &case, client_id, tokens).await;
            continue;
        }
        let (status, payload) = present_access_token(&router, &tokens.access).await;
        assert_eq!(
            status, 200,
            "case {case:?}: another user's access token keeps authorizing, \
             got {status} with {payload}"
        );
        let (status, payload) = present_refresh_token(&router, client_id, &tokens.refresh).await;
        assert_eq!(
            status, 200,
            "case {case:?}: another user's refresh token keeps refreshing, \
             got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn the_profile_is_untouched_and_the_user_signs_in_again() {
    let router = router();
    let user_pool_id = pool(&router, "resign-in-pool").await;
    let client_id = client(&router, &user_pool_id, "resign-in-client").await;
    confirmed_user(&router, &user_pool_id, "carol").await;
    let first = sign_in(&router, &client_id, "carol").await;

    let read = json!({ "UserPoolId": user_pool_id, "Username": "carol" });
    let before = ok(&router, "AdminGetUser", &read).await;

    // Repeating the call is the same call: a user with nothing left to
    // invalidate is signed out again, and answers the same empty body.
    for attempt in 1..=3 {
        let (status, payload) = sign_out(&router, &user_pool_id, "carol").await;
        assert_eq!(
            (status, &payload),
            (200, &Value::Object(serde_json::Map::new())),
            "case \"sign-out attempt {attempt}\": expected an empty 200, \
             got {status} with {payload}"
        );
    }

    let after = ok(&router, "AdminGetUser", &read).await;
    assert_eq!(
        after, before,
        "the sign-out leaves the stored profile untouched, got {after} after {before}"
    );

    assert_signed_out(&router, "the tokens held at the sign-out", &client_id, &first).await;

    // Signing in again is a new sign-in: its tokens are fresh ones, and they
    // authorize, while the revoked ones stay revoked.
    let second = sign_in(&router, &client_id, "carol").await;
    assert_ne!(
        second.access, first.access,
        "the re-sign-in issues a freshly minted access token"
    );
    let (status, payload) = present_access_token(&router, &second.access).await;
    assert_eq!(
        status, 200,
        "case \"the re-issued access token\": expected a 200, got {status} with {payload}"
    );
    let (status, payload) = present_refresh_token(&router, &client_id, &second.refresh).await;
    assert_eq!(
        status, 200,
        "case \"the re-issued refresh token\": expected a 200, got {status} with {payload}"
    );
    let (status, payload) = present_refresh_token(&router, &client_id, &first.refresh).await;
    assert_eq!(
        (status, error_message(&payload)),
        (400, REVOKED_REFRESH_TOKEN),
        "case \"the revoked refresh token after a re-sign-in\": expected it to stay revoked, \
         got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_revoked_refresh_token_is_told_apart_from_an_unresolvable_one() {
    let router = router();
    let user_pool_id = pool(&router, "refresh-refusal-pool").await;
    let client_id = client(&router, &user_pool_id, "refresh-refusal-client").await;
    let other_client = client(&router, &user_pool_id, "other-refresh-client").await;
    confirmed_user(&router, &user_pool_id, "dave").await;
    confirmed_user(&router, &user_pool_id, "erin").await;

    let revoked = sign_in(&router, &client_id, "dave").await;
    let live = sign_in(&router, &client_id, "erin").await;
    let other_clients_token = sign_in(&router, &other_client, "erin").await;
    let (status, payload) = sign_out(&router, &user_pool_id, "dave").await;
    assert_eq!(
        status, 200,
        "the sign-out must succeed, got {status} with {payload}"
    );

    // The rows differ along WHY the presented value is or is not a live refresh
    // token of this app client: revoked by the sign-out, never issued, issued as
    // a token of another kind, issued for another app client, or live.
    for (case, token, expected) in [
        (
            "a refresh token the sign-out revoked",
            revoked.refresh.as_str(),
            Some(REVOKED_REFRESH_TOKEN),
        ),
        (
            "a value this emulator never issued",
            "notatokenthisemulatorissued",
            Some(INVALID_REFRESH_TOKEN),
        ),
        (
            "an access token presented as the refresh token",
            live.access.as_str(),
            Some(INVALID_REFRESH_TOKEN),
        ),
        (
            "an id token presented as the refresh token",
            live.id.as_str(),
            Some(INVALID_REFRESH_TOKEN),
        ),
        (
            "a live refresh token of another app client",
            other_clients_token.refresh.as_str(),
            Some(INVALID_REFRESH_TOKEN),
        ),
        (
            "a live refresh token of this app client",
            live.refresh.as_str(),
            None,
        ),
    ] {
        let (status, payload) = present_refresh_token(&router, &client_id, token).await;
        match expected {
            Some(message) => assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "NotAuthorizedException", message),
                "case {case:?}: expected a 400 NotAuthorizedException saying {message:?}, \
                 got {status} with {payload}"
            ),
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected a 200, got {status} with {payload}"
            ),
        }
    }
}

#[tokio::test]
async fn an_absent_pool_or_user_is_refused_and_revokes_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    let other_pool = pool(&router, "other-refusal-pool").await;
    let client_id = client(&router, &user_pool_id, "refusal-client").await;
    confirmed_user(&router, &user_pool_id, "frank").await;
    confirmed_user(&router, &other_pool, "grace").await;
    let held = sign_in(&router, &client_id, "frank").await;

    for (case, identifier, pool_named, expected) in [
        (
            "an absent user pool",
            "frank".to_string(),
            "us-east-1_absent".to_string(),
            (
                400,
                "ResourceNotFoundException",
                "User pool us-east-1_absent does not exist.".to_string(),
            ),
        ),
        (
            "a user the pool does not hold",
            "absent-user".to_string(),
            user_pool_id.clone(),
            (400, "UserNotFoundException", "User does not exist.".to_string()),
        ),
        (
            "a user of another pool",
            "grace".to_string(),
            user_pool_id.clone(),
            (400, "UserNotFoundException", "User does not exist.".to_string()),
        ),
        (
            "a sub that names no user of the pool",
            "11111111-2222-3333-4444-555555555555".to_string(),
            user_pool_id.clone(),
            (400, "UserNotFoundException", "User does not exist.".to_string()),
        ),
    ] {
        let (status, payload) = sign_out(&router, &pool_named, &identifier).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload).to_string()),
            expected,
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Nothing above revoked anything: the token held before them still works.
    let (status, payload) = present_access_token(&router, &held.access).await;
    assert_eq!(
        status, 200,
        "a refused sign-out revokes nothing, so the held access token still \
         authorizes, got {status} with {payload}"
    );
    let (status, payload) = present_refresh_token(&router, &client_id, &held.refresh).await;
    assert_eq!(
        status, 200,
        "a refused sign-out revokes nothing, so the held refresh token still \
         refreshes, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    confirmed_user(&router, &user_pool_id, "heidi").await;

    // One value past each published bound of the input
    // (`smithy_query(query="operation",operation="AdminUserGlobalSignOut")`):
    // `Username` length 1..128 and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, and
    // `UserPoolId` length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for (case, body) in [
        (
            "an empty username, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Username": "" }),
        ),
        (
            "a username of 129 characters, past the length maximum 128",
            json!({ "UserPoolId": user_pool_id, "Username": "u".repeat(129) }),
        ),
        (
            "a username holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Username": "two words" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Username": "heidi" }),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            json!({
                "UserPoolId": format!("us-east-1_{}", "a".repeat(46)),
                "Username": "heidi",
            }),
        ),
        (
            "a request missing the required Username",
            json!({ "UserPoolId": user_pool_id }),
        ),
        (
            "a request missing the required UserPoolId",
            json!({ "Username": "heidi" }),
        ),
    ] {
        let (status, payload) = call(&router, "AdminUserGlobalSignOut", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modelled constraint is a 400, \
             got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {case:?}: a bad request is never answered with the scaffold's 501, \
             got {payload}"
        );
    }
}
