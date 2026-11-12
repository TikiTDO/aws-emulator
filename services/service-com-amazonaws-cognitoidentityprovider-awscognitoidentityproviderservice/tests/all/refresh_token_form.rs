// SPDX-License-Identifier: CC0-1.0

//! The FORM the refresh token of a sign-in reaches the caller in, over the wire.
//!
//! The rule under test: every refresh token this service vends is a JOSE value in COMPACT
//! serialization — `.`-separated base64url segments whose first one decodes to a JOSE
//! header naming the algorithms the token is sealed under — and not an undelimited run of
//! characters. That is the form Amazon Cognito vends `AuthenticationResultType.RefreshToken`
//! in, and the `.` of the member's modelled `TokenModelType` pattern `^[A-Za-z0-9-_=.]+$`
//! (`smithy_query(query="shape",shape="TokenModelType,AuthenticationResultType")`) is what
//! admits it. A client, proxy or test that reads the token structurally — splitting it on
//! `.` and reading the header segment — sees a JOSE value, and the credential inside stays
//! sealed, so the token still states nothing its holder can read.
//!
//! The rows span what the rule generalises over: every operation of this service that vends
//! a refresh token (`InitiateAuth`, `AdminInitiateAuth`, `RespondToAuthChallenge`,
//! `AdminRespondToAuthChallenge`, `GetTokensFromRefreshToken`), each authentication flow
//! that completes a sign-in (the self-service password flow, the admin password flow, the
//! legacy admin flow, an answered `NEW_PASSWORD_REQUIRED` challenge) and the rotation of a
//! token already held. The suite then presents each token back, because a form that no
//! longer works as a credential would not be a refresh token at all.
//!
//! The wire harness is the one `crate::error_wire` owns, so this suite drives the same
//! endpoint as its siblings rather than a second copy of it.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

use crate::error_wire::{call, ok};

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The temporary password an administrator-created user signs in with once, which is what
/// leaves it owing a `NEW_PASSWORD_REQUIRED` challenge.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2026";

/// The challenge an administrator-created user has to answer.
const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

/// The `alg` of the JOSE header a refresh token of this service carries: RSAES-OAEP with
/// SHA-256 as the message digest and MGF1's hash, whose registered JOSE name this is
/// (RFC 7518 §4.3).
const EXPECTED_ALG: &str = "RSA-OAEP-256";

/// The `enc` of that same header: AES-256-GCM (RFC 7518 §5.3).
const EXPECTED_ENC: &str = "A256GCM";

/// How many segments a JWE in compact serialization carries (RFC 7516 §7.1).
const COMPACT_SEGMENTS: usize = 5;

/// The one-time-password challenge of the choice-based flow, which is how the
/// self-service `RespondToAuthChallenge` completes a sign-in here: its
/// `NEW_PASSWORD_REQUIRED` branch is still a scaffold stub (HTTP 501), and that stub is
/// its own issue.
const EMAIL_OTP: &str = "EMAIL_OTP";

/// One emulator with just this service mounted, beside the engine its state lives in: the
/// code a one-time-password challenge delivered is read out of that state, because no
/// response carries it.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// The one-time password a challenge session delivered, read out of the emulator's own
/// state because nothing in the response carries it.
fn session_code(engine: &Arc<engine::InMemoryEngine>, session: &str) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
        .expect("the challenge delivered a code")
}

/// A created pool with one app client that rotates no refresh token and one that does.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    rotating_client_id: String,
}

/// Create the pool and the two app clients every row below authenticates against.
///
/// The pool admits the one-time-password first factor beside the password, which is what
/// lets the choice-based flow raise the challenge the self-service
/// `RespondToAuthChallenge` row answers.
async fn fixture(router: &MultiServiceRouter, pool_name: &str) -> Fixture {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": pool_name,
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", EMAIL_OTP] }
            },
            "AutoVerifiedAttributes": ["email"],
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client_id = client(router, &user_pool_id, "plain", &json!({})).await;
    let rotating_client_id = client(
        router,
        &user_pool_id,
        "rotating",
        &json!({
            "RefreshTokenRotation": { "Feature": "ENABLED", "RetryGracePeriodSeconds": 30 },
        }),
    )
    .await;
    Fixture {
        user_pool_id,
        client_id,
        rotating_client_id,
    }
}

/// Create an app client of `user_pool_id` carrying `members`, allowing every flow this
/// suite drives, and return its minted id.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    members: &Value,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([
            "ALLOW_USER_PASSWORD_AUTH",
            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            "ALLOW_USER_AUTH",
            "ALLOW_REFRESH_TOKEN_AUTH",
        ]),
    );
    for (member, value) in members.as_object().expect("an object of members") {
        body.insert(member.clone(), value.clone());
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` holding `temporary_password` and `attributes`, which leaves it owing
/// a password challenge when it first signs in.
async fn created_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    temporary_password: &str,
    attributes: &[(&str, &str)],
) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    body.insert(
        "TemporaryPassword".to_string(),
        json!(temporary_password),
    );
    if !attributes.is_empty() {
        body.insert(
            "UserAttributes".to_string(),
            Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        );
    }
    ok(router, "AdminCreateUser", &Value::Object(body)).await;
}

/// Create `username` with a PERMANENT password and `attributes`, so it is confirmed and
/// signs straight in.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    created_user(router, user_pool_id, username, TEMPORARY_PASSWORD, attributes).await;
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
}

/// The `AuthenticationResult.RefreshToken` of a successful authentication response.
fn refresh_token(case: &str, payload: &Value) -> String {
    payload["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: this answer must vend a refresh token, got {payload}")
        })
        .to_string()
}

/// Whether `value` satisfies the modelled `TokenModelType` pattern `^[A-Za-z0-9-_=.]+$`,
/// the target of every token member of `AuthenticationResultType`.
fn is_modelled_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | '.'))
}

/// Assert that `token` is a JWE in compact serialization: [`COMPACT_SEGMENTS`] non-empty
/// base64url segments whose first decodes to the JOSE header naming the two algorithms.
fn assert_compact_jose_form(case: &str, token: &str) {
    assert!(
        is_modelled_token(token),
        "case {case:?}: a refresh token satisfies TokenModelType, got {token}"
    );
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        COMPACT_SEGMENTS,
        "case {case:?}: expected {COMPACT_SEGMENTS} compact segments, got {} in {token}",
        segments.len()
    );
    for (at, segment) in segments.iter().enumerate() {
        assert!(
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
            "case {case:?}: segment {at} is non-empty base64url, got {segment:?}"
        );
    }
    // The leading segment is a JOSE header, read with this crate's own base64url codec —
    // the one it encodes a JOSE segment with.
    let header: Value = state::base64_url_decode(segments[0])
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the leading segment decodes to a JOSE header, got {token}")
        });
    assert_eq!(
        header["alg"], EXPECTED_ALG,
        "case {case:?}: expected the header's alg to be {EXPECTED_ALG}, got {header}"
    );
    assert_eq!(
        header["enc"], EXPECTED_ENC,
        "case {case:?}: expected the header's enc to be {EXPECTED_ENC}, got {header}"
    );
}

/// RULE: every operation of this service that vends a refresh token vends it in JOSE
/// compact serialization, in every flow that completes or refreshes a sign-in; the
/// credential inside states nothing its holder can read, and still refreshes its sign-in.
#[tokio::test]
async fn every_vended_refresh_token_is_a_compact_jose_value() {
    let (router, engine) = router_and_engine();
    let fixture = fixture(&router, "refresh-token-form").await;
    for username in ["alice", "bob"] {
        confirmed_user(&router, &fixture.user_pool_id, username, &[]).await;
    }
    // The user the choice-based flow challenges for a one-time password needs a verified
    // address to deliver it to.
    confirmed_user(
        &router,
        &fixture.user_pool_id,
        "otp",
        &[("email", "otp@example.com"), ("email_verified", "true")],
    )
    .await;
    // A user still owing the password challenge the admin half answers.
    created_user(
        &router,
        &fixture.user_pool_id,
        "newcomer",
        TEMPORARY_PASSWORD,
        &[],
    )
    .await;

    let mut vended: Vec<(String, String)> = Vec::new();

    // The flows that complete a sign-in in one call, over both the self-service and the
    // admin halves of the API, including the legacy admin flow.
    for (case, operation, body) in [
        (
            "InitiateAuth with the self-service password flow",
            "InitiateAuth",
            json!({
                "ClientId": fixture.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "AdminInitiateAuth with the admin password flow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "AdminInitiateAuth with the legacy admin flow",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "AuthFlow": "ADMIN_NO_SRP_AUTH",
                "AuthParameters": { "USERNAME": "bob", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "InitiateAuth on the app client that rotates its refresh tokens",
            "InitiateAuth",
            json!({
                "ClientId": fixture.rotating_client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "bob", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
    ] {
        let payload = ok(&router, operation, &body).await;
        vended.push((case.to_string(), refresh_token(case, &payload)));
    }

    // An answered challenge, on both halves of the API: the sign-in completes in the
    // RespondToAuthChallenge call, which is its own response serializer. The admin half
    // answers the password challenge of an administrator-created user; the self-service
    // half answers the one-time password of the choice-based flow.
    let challenged = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "newcomer", "PASSWORD": TEMPORARY_PASSWORD },
        }),
    )
    .await;
    let password_session = challenged["Session"]
        .as_str()
        .expect("the password challenge carries a session")
        .to_string();
    let answered = ok(
        &router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": password_session,
            "ChallengeResponses": { "USERNAME": "newcomer", "NEW_PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let case = "AdminRespondToAuthChallenge answering the password challenge";
    vended.push((case.to_string(), refresh_token(case, &answered)));

    let started = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": "otp", "PREFERRED_CHALLENGE": EMAIL_OTP },
        }),
    )
    .await;
    let otp_session = started["Session"]
        .as_str()
        .expect("the one-time-password challenge carries a session")
        .to_string();
    let code = session_code(&engine, &otp_session);
    let answered = ok(
        &router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": fixture.client_id,
            "ChallengeName": EMAIL_OTP,
            "Session": otp_session,
            "ChallengeResponses": { "USERNAME": "otp", "EMAIL_OTP_CODE": code },
        }),
    )
    .await;
    let case = "RespondToAuthChallenge answering the one-time password";
    vended.push((case.to_string(), refresh_token(case, &answered)));

    // A rotation: the app client that rotates its refresh tokens vends a fresh one for the
    // token presented back.
    let rotating = vended
        .iter()
        .find(|(case, _)| case.contains("rotates"))
        .map(|(_, token)| token.clone())
        .expect("the rotating client's sign-in is among the rows");
    let rotated = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": fixture.rotating_client_id, "RefreshToken": rotating }),
    )
    .await;
    vended.push((
        "GetTokensFromRefreshToken rotating the token presented back".to_string(),
        refresh_token("GetTokensFromRefreshToken rotating the token presented back", &rotated),
    ));

    for (case, token) in &vended {
        assert_compact_jose_form(case, token);
        // The credential inside is sealed: nothing of the sign-in it belongs to is
        // readable in it, and no segment decodes to a document its holder can read.
        //
        // An identifier long enough that a chance match is impossible is looked for in the
        // WHOLE token; the usernames are looked for in what a holder can actually READ (the
        // segments that decode below), because a three-character name occurring somewhere
        // inside a thousand base64url characters of ciphertext is a coincidence rather than
        // a disclosure — asserting its absence from the ciphertext fails at random.
        for identifier in [fixture.user_pool_id.as_str(), fixture.client_id.as_str()] {
            assert!(
                !token.contains(identifier),
                "case {case:?}: a refresh token states nothing of its sign-in, \
                 found {identifier:?} in {token}"
            );
        }
        for (at, segment) in token.split('.').enumerate() {
            let readable: Option<Value> = state::base64_url_decode(segment)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            assert!(
                at == 0 || readable.is_none(),
                "case {case:?}: only the header segment is readable, segment {at} decoded \
                 to {readable:?}"
            );
            // The one segment a holder can read states the sealing itself and nothing of
            // the sign-in: no user of this pool is named in it.
            if let Some(document) = readable {
                let stated = document.to_string();
                for named in ["alice", "bob", "otp", "newcomer"] {
                    assert!(
                        !stated.contains(named),
                        "case {case:?}: a readable segment states nothing of its sign-in, \
                         found {named:?} in {stated}"
                    );
                }
            }
        }
    }

    // Every token is its own credential, and no two sign-ins vend the same one.
    for (at, (case, token)) in vended.iter().enumerate() {
        assert!(
            !vended
                .iter()
                .enumerate()
                .any(|(other, (_, seen))| other != at && seen == token),
            "case {case:?}: every sign-in vends its own refresh credential, {token} repeated"
        );
    }

    // A form that no longer works as a credential would not be a refresh token: the first
    // sign-in's token refreshes its sign-in through both the flow and the exchange.
    let (case, token) = &vended[0];
    let refreshed = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "REFRESH_TOKEN_AUTH",
            "AuthParameters": { "REFRESH_TOKEN": token },
        }),
    )
    .await;
    assert!(
        refreshed["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|access| !access.is_empty()),
        "case {case:?}: a sealed refresh token still refreshes its sign-in, got {refreshed}"
    );
    let exchanged = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": fixture.client_id, "RefreshToken": token }),
    )
    .await;
    assert!(
        exchanged["AuthenticationResult"]["IdToken"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "case {case:?}: a sealed refresh token still exchanges for fresh tokens, \
         got {exchanged}"
    );
}

/// RULE: a value that has the form but was not vended by this service is no credential —
/// the sealing is what the exchange resolves, so a hand-built JOSE value of the same shape
/// authorizes nothing.
#[tokio::test]
async fn a_jose_shaped_value_this_service_did_not_vend_is_refused() {
    let (router, _engine) = router_and_engine();
    let fixture = fixture(&router, "refresh-token-form-forged").await;
    confirmed_user(&router, &fixture.user_pool_id, "alice", &[]).await;

    let signed_in = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let vended = refresh_token("the sign-in that is forged from", &signed_in);
    let segments: Vec<&str> = vended.split('.').collect();

    // (case, the value presented as a refresh token)
    let header = segments[0].to_string();
    let forged: [(&str, String); 3] = [
        (
            "a JOSE header with five empty segments after it",
            format!("{header}.{}", ["AA"; 4].join(".")),
        ),
        (
            "the vended token with its ciphertext segment replaced",
            format!(
                "{}.{}.{}.{}.{}",
                segments[0], segments[1], segments[2], "Zm9yZ2Vk", segments[4]
            ),
        ),
        (
            "a value that is not a JOSE token at all",
            "not-a-refresh-token".to_string(),
        ),
    ];

    for (case, presented) in forged {
        let answered = call(
            &router,
            "GetTokensFromRefreshToken",
            &json!({ "ClientId": fixture.client_id, "RefreshToken": presented }),
        )
        .await;
        assert_eq!(
            answered.status,
            crate::error_wire::REFUSED,
            "case {case:?}: expected the exchange to be refused, got {} with {}",
            answered.status,
            answered.payload
        );
    }
}
