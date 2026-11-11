// SPDX-License-Identifier: CC0-1.0

//! Which error class and which sentence each refusal on the authentication surface
//! answers with.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over the one request that showed it:
//!
//! * the client-credentials grant authenticates the pair "its client ID and secret"
//!   (`smithy_query(query="operation",operation="GetClientToken")`) or refuses it, and the
//!   refusal says which of the two was not accepted without saying WHICH of them was
//!   wrong, so no caller can read an app client's existence off it;
//! * that grant's `Scopes` "must belong to a resource server in your user pool"
//!   (`GetClientToken.Scopes`), and a scope the pool defines nowhere is a GRANT that is
//!   not authorized rather than a malformed request;
//! * an app client's `ExplicitAuthFlows` are "the authentication flows that you want your
//!   user pool client to support" (`CreateUserPoolClient.ExplicitAuthFlows`), and each API
//!   that gates on that list refuses an undeclared flow with the sentence its OWN API
//!   words that refusal with — the administrative sign-in's names no flow, the client-side
//!   sign-in's and the grant's name the flow asked for — and none of them names the app
//!   client or the pool;
//! * a sign-out "invalidates the identity, access, and refresh tokens that Amazon Cognito
//!   issued to a user" and afterwards "Amazon Cognito returns an `Access Token has been
//!   revoked` error" (`GlobalSignOut`), which is a different answer from the one a value
//!   that resolves to no access token at all gets;
//! * `GetTokensFromRefreshToken` exchanges "a valid refresh token that can authorize the
//!   request for new tokens" (`GetTokensFromRefreshToken.RefreshToken`), so a token this
//!   service issued for ANOTHER app client is told apart from a value it cannot resolve to
//!   a refresh token at all;
//! * a `Session` "maintains the state of authentication requests and challenge responses"
//!   (`RespondToAuthChallenge.Session`): answering a challenge spends its session, a
//!   replay of a spent one is refused as the replay it is, and a value that names no
//!   outstanding challenge is the `CodeMismatchException` both respond operations declare;
//! * a sign-in reads the stored user's own account status BEFORE the credential it
//!   presents, so the error class never says whether the password was right;
//! * `AdminLinkProviderForUser.SourceUser` is "an external IdP account ... This user must
//!   be a federated user (for example, a SAML or Facebook user), not another native user"
//!   (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`),
//!   so an identifier naming the provider the pool's own users sign in under links
//!   nothing.
//!
//! Each table row names the case it asserts and prints the value that was answered.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A credential scope naming this service's SigV4 signing name. The suite turns request
/// authentication off, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A password no user of the suite holds, satisfying the same policy so nothing refuses
/// it for its shape.
const WRONG_PASSWORD: &str = "NotTheP4ssword!";

/// The sentence the client-credentials grant answers a pair it does not authenticate
/// with.
const INVALID_CLIENT_OR_SECRET: &str = "Invalid client or secret";

/// The sentence that grant answers a scope no resource server of the pool defines with.
const INVALID_SCOPES_REQUESTED: &str = "Invalid scopes requested";

/// The one sentence the ADMINISTRATIVE sign-in owes for a flow the app client does not
/// declare, which names no flow at all.
const ADMIN_FLOW_NOT_ENABLED: &str = "Auth flow not enabled for this client";

/// The sentence the client-side sign-in and the machine-to-machine grant owe for
/// `auth_flow`, whose only variable part is the flow the request asked for.
fn client_flow_not_enabled(auth_flow: &str) -> String {
    format!("{auth_flow} flow not enabled for this client")
}

/// The sentence a self-service call made with a signed-out access token is refused with.
const ACCESS_TOKEN_REVOKED: &str = "Access Token has been revoked";

/// The sentence a self-service call made with a value that resolves to no access token at
/// all is refused with.
const INVALID_ACCESS_TOKEN: &str = "Invalid Access Token";

/// The sentence a refresh exchange answers a token this service issued for another app
/// client with.
const REFRESH_TOKEN_DIFFERENT_CLIENT: &str = "Refresh Token has different Client";

/// The sentence a refresh exchange answers a value it cannot resolve to a refresh token
/// with.
const INVALID_REFRESH_TOKEN: &str = "Invalid Refresh Token";

/// The sentence a refresh exchange answers a token it DID resolve for the app client the
/// request names and that still does not authorize the request with — here, one presented
/// without the confirmed device key a pool that remembers devices requires.
const UNAUTHORIZED_REFRESH_TOKEN: &str = "Invalid Refresh Token.";

/// The sentence a challenge response answers a session an earlier response already
/// answered with.
const SESSION_ALREADY_USED: &str = "Invalid session for the user, session can only be used once.";

/// The sentence a challenge response answers a stored session that does not belong to the
/// request it arrived on — one started for another pool or app client, or a value that
/// names no stored session at all — with.
const SESSION_NOT_FOR_THIS_REQUEST: &str = "Invalid session for the user.";

/// The sentence `AdminLinkProviderForUser` answers a `SourceUser` naming the pool's own
/// native provider with.
const INVALID_SOURCE_USER: &str = "Invalid SourceUser: Cognito users with a username/password \
                                   may not be passed in as a SourceUser, only as a \
                                   DestinationUser";

/// The sentence a repeat of the link an identity already holds is refused with.
const ALREADY_LINKED: &str = "SourceUser is already linked to DestinationUser";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) the suite's app
/// clients are created with.
const CLIENT_SECRET: &str = "m2msecret00000000000000001";

/// The `ExplicitAuthFlows` value that configures an app client for machine-to-machine
/// authorization (`GetClientToken` in this service's model).
const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// A `SessionType`-shaped value (length 20..4096) this emulator never handed out.
const UNKNOWN_SESSION: &str = "session-nothing-ever-handed-out-0001";

/// The provider name a user pool's OWN users sign in under, which is not one of its
/// configured identity providers.
const NATIVE_PROVIDER: &str = "Cognito";

/// The `ProviderAttributeName` a social or SAML identity is named by
/// (`AdminLinkProviderForUser.SourceUser`).
const SUBJECT_ATTRIBUTE: &str = "Cognito_Subject";

/// One emulator with just this service mounted, beside the engine holding its state.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    router_and_engine().0
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The decoded claims of a compact JWS: the second segment is base64url (RFC 7515 §2,
/// whose padding is stripped), decoded with the same codec the wire serializer uses.
fn token_claims(case: &str, token: &str) -> Value {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("case {case:?}: a compact JWS carries three segments"));
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("case {case:?}: {segment:?} must be base64url ({error})"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the claims must be JSON ({error})"))
}

/// Create a pool and return its minted `UserPoolId`.
async fn create_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client of `user_pool_id` declaring `flows`, with the stored secret and
/// the `PreventUserExistenceErrors` / `AllowedOAuthScopes` settings each caller asks for,
/// and return its minted `ClientId`.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    flows: &[&str],
    secret: Option<&str>,
    scopes: Option<&[&str]>,
    prevent_user_existence_errors: Option<&str>,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    body.insert("ExplicitAuthFlows".to_string(), json!(flows));
    if let Some(secret) = secret {
        body.insert("ClientSecret".to_string(), json!(secret));
    }
    if let Some(scopes) = scopes {
        body.insert("AllowedOAuthScopes".to_string(), json!(scopes));
    }
    if let Some(setting) = prevent_user_existence_errors {
        body.insert("PreventUserExistenceErrors".to_string(), json!(setting));
    }
    ok(router, "CreateUserPoolClient", &Value::Object(body)).await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` in `user_pool_id` with `GOOD_PASSWORD` as its permanent password, so
/// it is a `CONFIRMED` user that can sign in.
async fn create_confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
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

/// Create `username` in `user_pool_id` with a VERIFIED email beside `GOOD_PASSWORD`, so
/// a reset message can be addressed to it.
async fn create_confirmed_user_with_email(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [
                { "Name": "email", "Value": format!("{username}@example.test") },
                { "Name": "email_verified", "Value": "true" },
            ],
        }),
    )
    .await;
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

/// The whole token triple a sign-in through the admin password flow answers with.
struct Tokens {
    access: String,
    id: String,
    refresh: String,
}

/// Sign `username` in through the admin password flow of `client_id` and return its
/// tokens.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Tokens {
    let signed_in = ok(
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
    let result = &signed_in["AuthenticationResult"];
    let token = |member: &str| {
        result[member]
            .as_str()
            .unwrap_or_else(|| panic!("a sign-in issues {member}, got {signed_in}"))
            .to_string()
    };
    Tokens {
        access: token("AccessToken"),
        id: token("IdToken"),
        refresh: token("RefreshToken"),
    }
}

/// RULE: the client-credentials grant either authenticates the client id and secret pair
/// it is given or refuses it with ONE sentence, which names neither value and does not say
/// which half of the pair was not accepted.
#[tokio::test]
async fn the_client_credentials_grant_names_neither_half_of_the_pair_it_refuses() {
    let router = router();
    let user_pool_id = create_pool(&router, "m2m-secret-pool").await;
    let served = create_client(
        &router,
        &user_pool_id,
        "m2m",
        &[CLIENT_TOKEN_AUTH_FLOW],
        Some(CLIENT_SECRET),
        None,
        None,
    )
    .await;
    // A second grant client of the same pool, whose secret is not this client's.
    let neighbours_secret = "neighboursecret0000000001";
    let neighbour = create_client(
        &router,
        &user_pool_id,
        "m2m-neighbour",
        &[CLIENT_TOKEN_AUTH_FLOW],
        Some(neighbours_secret),
        None,
        None,
    )
    .await;
    // A third, configured for the grant but holding NO secret, so there is nothing a
    // request could prove it knows.
    let secretless = create_client(
        &router,
        &user_pool_id,
        "m2m-secretless",
        &[CLIENT_TOKEN_AUTH_FLOW],
        None,
        None,
        None,
    )
    .await;

    // The rows differ along WHY the pair is not the stored one: a value of the right
    // shape that is nothing's secret, the secret of another app client of the pool, the
    // stored secret with a character added, the stored secret reversed, and a client that
    // stores no secret at all.
    let reversed: String = CLIENT_SECRET.chars().rev().collect();
    let lengthened = format!("{CLIENT_SECRET}+");
    let rows: [(&str, &str, &str); 5] = [
        (
            "a secret of the right shape that no app client stores",
            served.as_str(),
            "nothingssecret0000000001",
        ),
        (
            "the secret of another app client of the pool",
            served.as_str(),
            neighbours_secret,
        ),
        (
            "the stored secret with one more character",
            served.as_str(),
            lengthened.as_str(),
        ),
        (
            "the stored secret reversed",
            served.as_str(),
            reversed.as_str(),
        ),
        (
            "an app client that stores no secret at all",
            secretless.as_str(),
            CLIENT_SECRET,
        ),
    ];

    for (case, client_id, secret) in rows {
        let (status, payload) = call(
            &router,
            "GetClientToken",
            &json!({ "ClientId": client_id, "Secret": secret }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", INVALID_CLIENT_OR_SECRET),
            "case {case:?}: expected the one unauthenticated-pair sentence, got {status} with \
             {payload}"
        );
        let message = error_message(&payload);
        assert!(
            !message.contains(client_id) && !message.contains(secret),
            "case {case:?}: the refusal must name neither value, got {message:?}"
        );
    }

    // The stored pair of each app client that holds one IS served, so every refusal above
    // is the row's own and not a grant that cannot work at all.
    for (case, client_id, secret) in [
        ("the served app client", served.as_str(), CLIENT_SECRET),
        (
            "the neighbouring app client",
            neighbour.as_str(),
            neighbours_secret,
        ),
    ] {
        let granted = ok(
            &router,
            "GetClientToken",
            &json!({ "ClientId": client_id, "Secret": secret }),
        )
        .await;
        assert!(
            granted["ClientAuthenticationResult"]["AccessToken"]
                .as_str()
                .is_some_and(|token| !token.is_empty()),
            "case {case:?}: the stored secret must be served a token, got {granted}"
        );
    }
}

/// RULE: a scope the pool's resource servers do not define is a grant this app client is
/// not authorized for, so the client-credentials grant refuses it with
/// `NotAuthorizedException` rather than with the 400 class for a malformed request.
#[tokio::test]
async fn the_client_credentials_grant_refuses_an_undefined_scope_as_unauthorized() {
    let router = router();
    let user_pool_id = create_pool(&router, "m2m-scope-pool").await;
    for (identifier, name, scopes) in [
        ("https://api.example.test", "api", ["read", "write"]),
        ("urn:billing", "billing", ["invoice.read", "invoice.write"]),
    ] {
        ok(
            &router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                "Name": name,
                "Scopes": scopes
                    .iter()
                    .map(|scope| json!({
                        "ScopeName": scope,
                        "ScopeDescription": format!("the {scope} scope"),
                    }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
    }
    let client_id = create_client(
        &router,
        &user_pool_id,
        "m2m-scopes",
        &[CLIENT_TOKEN_AUTH_FLOW],
        Some(CLIENT_SECRET),
        Some(&["https://api.example.test/read"]),
        None,
    )
    .await;

    // The rows differ along the FORM of the scope the pool does not define: a resource
    // server nothing created, a scope name a created server does not define, a bare
    // identifier with no scope name, a bare scope name under no server, one undefined
    // scope beside a defined one, and a defined scope of one server written under
    // another.
    let rows: [(&str, Value); 6] = [
        (
            "a resource server the pool holds none of",
            json!(["https://other.example.test/read"]),
        ),
        (
            "a scope name the pool's resource server does not define",
            json!(["https://api.example.test/admin"]),
        ),
        (
            "a bare resource server identifier",
            json!(["https://api.example.test"]),
        ),
        ("a bare scope name", json!(["read"])),
        (
            "one undefined scope beside a defined one",
            json!(["https://api.example.test/read", "urn:billing/admin"]),
        ),
        (
            "a scope of one resource server written under another",
            json!(["https://api.example.test/invoice.read"]),
        ),
    ];

    for (case, scopes) in rows {
        let (status, payload) = call(
            &router,
            "GetClientToken",
            &json!({ "ClientId": client_id, "Secret": CLIENT_SECRET, "Scopes": scopes }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", INVALID_SCOPES_REQUESTED),
            "case {case:?}: an undefined scope must refuse the grant, got {status} with {payload}"
        );
    }

    // Every scope the POOL defines is granted, whether or not the app client is
    // configured with it, and a request naming none falls back to its configuration.
    for (case, scopes, expected) in [
        (
            "one scope of one resource server",
            Some(json!(["https://api.example.test/read"])),
            vec!["https://api.example.test/read"],
        ),
        (
            "scopes of two resource servers at once",
            Some(json!(["urn:billing/invoice.read", "https://api.example.test/write"])),
            vec!["urn:billing/invoice.read", "https://api.example.test/write"],
        ),
        (
            "no scopes at all, which takes the app client's own",
            None,
            vec!["https://api.example.test/read"],
        ),
    ] {
        let mut body = Map::new();
        body.insert("ClientId".to_string(), json!(client_id));
        body.insert("Secret".to_string(), json!(CLIENT_SECRET));
        if let Some(scopes) = scopes {
            body.insert("Scopes".to_string(), scopes);
        }
        let granted = ok(&router, "GetClientToken", &Value::Object(body)).await;
        let access_token = granted["ClientAuthenticationResult"]["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an issued access token, got {granted}"));
        // "The custom scopes to authorize in the access token" (`GetClientToken.Scopes`),
        // which the issued token carries as its `scope` claim.
        let claims = token_claims(case, access_token);
        assert_eq!(
            claims["scope"],
            json!(expected.join(" ")),
            "case {case:?}: the token must authorize the scopes asked for, got {claims}"
        );
    }
}

/// RULE: every API that gates on an app client's stored `ExplicitAuthFlows` refuses a
/// flow that list does not declare with `InvalidParameterException` carrying the sentence
/// its OWN API words that refusal with — the administrative sign-in's names no flow at
/// all, the client-side sign-in's and the grant's name the flow asked for and nothing
/// else — and none of them names the app client or the pool.
#[tokio::test]
async fn each_api_words_the_auth_flow_gate_as_its_own_api_does() {
    let router = router();
    let user_pool_id = create_pool(&router, "flow-gate-pool").await;
    create_confirmed_user(&router, &user_pool_id, "alice").await;

    // One app client per declared flow, so each row asks an app client for a flow it does
    // not declare while declaring another.
    let user_password = create_client(
        &router,
        &user_pool_id,
        "user-password",
        &["ALLOW_USER_PASSWORD_AUTH"],
        None,
        None,
        None,
    )
    .await;
    let refresh_only = create_client(
        &router,
        &user_pool_id,
        "refresh-only",
        &["ALLOW_REFRESH_TOKEN_AUTH"],
        None,
        None,
        None,
    )
    .await;
    let grant_client = create_client(
        &router,
        &user_pool_id,
        "grant-without-the-flow",
        &["ALLOW_USER_PASSWORD_AUTH"],
        Some(CLIENT_SECRET),
        None,
        None,
    )
    .await;

    // The rows differ along the API asked and the flow it was asked for: the
    // machine-to-machine grant, the client-side sign-in on three of its flows, and the
    // administrative sign-in.
    let rows: [(&str, &str, Value, String); 5] = [
        (
            "the machine-to-machine grant of a client configured for a user flow",
            "GetClientToken",
            json!({ "ClientId": grant_client, "Secret": CLIENT_SECRET }),
            client_flow_not_enabled(CLIENT_TOKEN_AUTH_FLOW),
        ),
        (
            "the client-side password sign-in of a refresh-only client",
            "InitiateAuth",
            json!({
                "ClientId": refresh_only,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
            client_flow_not_enabled("USER_PASSWORD_AUTH"),
        ),
        (
            "the choice-based sign-in of a password-only client",
            "InitiateAuth",
            json!({
                "ClientId": user_password,
                "AuthFlow": "USER_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PREFERRED_CHALLENGE": "EMAIL_OTP" },
            }),
            client_flow_not_enabled("USER_AUTH"),
        ),
        (
            "the refresh sign-in of a password-only client",
            "InitiateAuth",
            json!({
                "ClientId": user_password,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": "nothing-this-emulator-issued" },
            }),
            client_flow_not_enabled("REFRESH_TOKEN_AUTH"),
        ),
        (
            "the administrative sign-in of a password-only client",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": user_password,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
            ADMIN_FLOW_NOT_ENABLED.to_string(),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", expected.as_str()),
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            !message.contains(&user_pool_id) && !message.contains(&grant_client),
            "case {case:?}: the sentence must name neither the pool nor the app client, got \
             {message:?}"
        );
    }
}

/// RULE: a sign-out invalidates the tokens already issued to its user, and a later call
/// made with one of them reports that the token was REVOKED — which is a different answer
/// from the one a value that resolves to no access token at all gets.
#[tokio::test]
async fn a_signed_out_access_token_is_told_apart_from_one_nothing_issued() {
    let router = router();
    let user_pool_id = create_pool(&router, "sign-out-pool").await;
    let client_id = create_client(
        &router,
        &user_pool_id,
        "sign-out-client",
        &["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        None,
        None,
        None,
    )
    .await;

    // The self-service operations an access token identifies its caller to, so the answer
    // is the token check's and not one operation's own.
    let readers = ["GetUser", "GetUserAuthFactors", "ListDevices"];

    // The rows differ along WHICH sign-out invalidated the token: the self-service one,
    // which is authorized by the token itself, and the administrative one, which names the
    // user.
    for (case, username, sign_out) in [
        ("the self-service sign-out", "alice", "GlobalSignOut"),
        (
            "the administrative sign-out",
            "bob",
            "AdminUserGlobalSignOut",
        ),
    ] {
        create_confirmed_user(&router, &user_pool_id, username).await;
        let tokens = sign_in(&router, &user_pool_id, &client_id, username).await;

        // While the token is live every reader serves it, so each refusal below is the
        // sign-out's doing.
        for reader in readers {
            ok(&router, reader, &json!({ "AccessToken": tokens.access })).await;
        }

        let body = if sign_out == "GlobalSignOut" {
            json!({ "AccessToken": tokens.access })
        } else {
            json!({ "UserPoolId": user_pool_id, "Username": username })
        };
        ok(&router, sign_out, &body).await;

        for reader in readers {
            let (status, payload) =
                call(&router, reader, &json!({ "AccessToken": tokens.access })).await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "NotAuthorizedException", ACCESS_TOKEN_REVOKED),
                "case {case:?}: {reader} must report the token as revoked, got {status} with \
                 {payload}"
            );
        }

        // A value that resolves to no access token of this emulator is a different
        // refusal, so a revoked token is not reported as garbage.
        for (unresolvable, token) in [
            ("a value nothing issued", "not-a-token-this-emulator-issued"),
            ("the id token of the same sign-in", tokens.id.as_str()),
            (
                "the refresh token of the same sign-in",
                tokens.refresh.as_str(),
            ),
        ] {
            let (status, payload) =
                call(&router, "GetUser", &json!({ "AccessToken": token })).await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "NotAuthorizedException", INVALID_ACCESS_TOKEN),
                "case {case:?}: {unresolvable} must not be reported as revoked, got {status} \
                 with {payload}"
            );
        }
    }
}

/// RULE: the refresh exchange tells a refresh token this service issued for ANOTHER app
/// client apart from a value it cannot resolve to a refresh token at all, and words
/// neither refusal with a trailing period.
#[tokio::test]
async fn the_refresh_exchange_tells_another_clients_token_apart() {
    let router = router();
    let flows = ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"];
    let user_pool_id = create_pool(&router, "refresh-pool").await;
    let other_pool_id = create_pool(&router, "refresh-other-pool").await;
    let client_id = create_client(&router, &user_pool_id, "mine", &flows, None, None, None).await;
    let sibling = create_client(&router, &user_pool_id, "sibling", &flows, None, None, None).await;
    let abroad = create_client(
        &router,
        &other_pool_id,
        "abroad",
        &flows,
        None,
        None,
        None,
    )
    .await;
    create_confirmed_user(&router, &user_pool_id, "alice").await;
    create_confirmed_user(&router, &other_pool_id, "alice").await;

    let mine = sign_in(&router, &user_pool_id, &client_id, "alice").await;
    let siblings = sign_in(&router, &user_pool_id, &sibling, "alice").await;
    let abroads = sign_in(&router, &other_pool_id, &abroad, "alice").await;

    // The rows differ along WHAT the presented value is: a refresh token of another app
    // client of the same pool, one of an app client of another pool, a value nothing
    // issued, and the other two kinds of token of the caller's own sign-in.
    let rows: [(&str, &str, &str); 5] = [
        (
            "a refresh token of another app client of the pool",
            siblings.refresh.as_str(),
            REFRESH_TOKEN_DIFFERENT_CLIENT,
        ),
        (
            "a refresh token of an app client of another pool",
            abroads.refresh.as_str(),
            REFRESH_TOKEN_DIFFERENT_CLIENT,
        ),
        (
            "a value nothing issued",
            "not.a.token.this.emulator.issued",
            INVALID_REFRESH_TOKEN,
        ),
        (
            "the access token of the caller's own sign-in",
            mine.access.as_str(),
            INVALID_REFRESH_TOKEN,
        ),
        (
            "the id token of the caller's own sign-in",
            mine.id.as_str(),
            INVALID_REFRESH_TOKEN,
        ),
    ];

    for (case, refresh_token, expected) in rows {
        let (status, payload) = call(
            &router,
            "GetTokensFromRefreshToken",
            &json!({ "ClientId": client_id, "RefreshToken": refresh_token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", expected),
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The caller's own live refresh token is exchanged, so every refusal above is the
    // row's own.
    let refreshed = ok(
        &router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": client_id, "RefreshToken": mine.refresh }),
    )
    .await;
    assert!(
        refreshed["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "the caller's own refresh token must be exchanged, got {refreshed}"
    );

    // A token this service DID resolve for the app client named, which the pool's device
    // remembering does not authorize without "the confirmed device key in this request"
    // (`GetTokensFromRefreshToken.DeviceKey`), carries its own sentence: the value is not
    // one this service cannot resolve.
    let device_pool_id = create_pool(&router, "refresh-device-pool").await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": device_pool_id,
            "DeviceConfiguration": { "ChallengeRequiredOnNewDevice": true },
        }),
    )
    .await;
    let device_client =
        create_client(&router, &device_pool_id, "device", &flows, None, None, None).await;
    create_confirmed_user(&router, &device_pool_id, "alice").await;
    let on_a_device = sign_in(&router, &device_pool_id, &device_client, "alice").await;
    let (status, payload) = call(
        &router,
        "GetTokensFromRefreshToken",
        &json!({ "ClientId": device_client, "RefreshToken": on_a_device.refresh }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (
            400,
            "NotAuthorizedException",
            UNAUTHORIZED_REFRESH_TOKEN
        ),
        "case \"a resolved token with no device key on a pool that remembers devices\": \
         expected {UNAUTHORIZED_REFRESH_TOKEN:?}, got {status} with {payload}"
    );
}

/// Start the `NEW_PASSWORD_REQUIRED` challenge for a user `AdminCreateUser` just created
/// with a temporary password, and return the `Session` it hands out.
async fn challenge_session(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> String {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "TemporaryPassword": GOOD_PASSWORD,
        }),
    )
    .await;
    let challenged = ok(
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
    assert_eq!(
        challenged["ChallengeName"],
        json!("NEW_PASSWORD_REQUIRED"),
        "a user that owes a password change is answered with the challenge, got {challenged}"
    );
    challenged["Session"]
        .as_str()
        .expect("a challenge hands out a session")
        .to_string()
}

/// RULE: a challenge session answers its challenge ONCE — a replay of a spent session is
/// refused as the replay it is — while a `Session` that names no outstanding challenge at
/// all is the `CodeMismatchException` both respond operations declare.
#[tokio::test]
async fn a_challenge_session_answers_once_and_an_unknown_one_is_a_code_mismatch() {
    let router = router();
    let user_pool_id = create_pool(&router, "session-pool").await;
    let client_id = create_client(
        &router,
        &user_pool_id,
        "session-client",
        &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        None,
        None,
        None,
    )
    .await;

    /// The responses a `NEW_PASSWORD_REQUIRED` challenge is answered with.
    fn responses(username: &str) -> Value {
        json!({ "USERNAME": username, "NEW_PASSWORD": "Ch0senPassw0rd!" })
    }

    // A session this emulator never handed out, asked of BOTH respond operations: it
    // resolves no outstanding challenge, so it is no session of this request.
    for (case, operation, body) in [
        (
            "an unknown Session, administratively",
            "AdminRespondToAuthChallenge",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": UNKNOWN_SESSION,
                "ChallengeResponses": responses("alice"),
            }),
        ),
        (
            "an unknown Session, client-side",
            "RespondToAuthChallenge",
            json!({
                "ClientId": client_id,
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": UNKNOWN_SESSION,
                "ChallengeResponses": responses("alice"),
            }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", SESSION_NOT_FOR_THIS_REQUEST),
            "case {case:?}: expected the invalid-session class, got {status} with {payload}"
        );
    }

    // A session that DID answer its challenge is refused as a replay, whichever respond
    // operation the replay is sent to.
    for (case, username, replay_operation) in [
        (
            "a replay of a spent session, administratively",
            "replayadmin",
            "AdminRespondToAuthChallenge",
        ),
        (
            "a replay of a spent session, client-side",
            "replayclient",
            "RespondToAuthChallenge",
        ),
    ] {
        let session = challenge_session(&router, &user_pool_id, &client_id, username).await;
        let answered = ok(
            &router,
            "AdminRespondToAuthChallenge",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": session,
                "ChallengeResponses": responses(username),
            }),
        )
        .await;
        assert!(
            answered["AuthenticationResult"]["AccessToken"].is_string(),
            "case {case:?}: the first response must complete the sign-in, got {answered}"
        );

        let mut replay = Map::new();
        if replay_operation == "AdminRespondToAuthChallenge" {
            replay.insert("UserPoolId".to_string(), json!(user_pool_id));
        }
        replay.insert("ClientId".to_string(), json!(client_id));
        replay.insert(
            "ChallengeName".to_string(),
            json!("NEW_PASSWORD_REQUIRED"),
        );
        replay.insert("Session".to_string(), json!(session));
        replay.insert("ChallengeResponses".to_string(), responses(username));
        let (status, payload) = call(&router, replay_operation, &Value::Object(replay)).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "NotAuthorizedException", SESSION_ALREADY_USED),
            "case {case:?}: expected the spent-session sentence, got {status} with {payload}"
        );

        // A spent session is NOT this caller's to be told about where the request names
        // another app client of the pool, or another pool: there it resolves no challenge
        // the request could answer, which is the refusal a session started elsewhere
        // owes.
        let elsewhere = create_client(
            &router,
            &user_pool_id,
            &format!("{username}-neighbour"),
            &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
            None,
            None,
            None,
        )
        .await;
        let abroad_pool = create_pool(&router, &format!("{username}-other-pool")).await;
        let abroad = create_client(
            &router,
            &abroad_pool,
            "abroad",
            &["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
            None,
            None,
            None,
        )
        .await;
        for (elsewhere_case, pool, client) in [
            (
                "another app client of the pool",
                user_pool_id.as_str(),
                elsewhere.as_str(),
            ),
            ("an app client of another pool", abroad_pool.as_str(), abroad.as_str()),
        ] {
            let mut body = Map::new();
            if replay_operation == "AdminRespondToAuthChallenge" {
                body.insert("UserPoolId".to_string(), json!(pool));
            }
            body.insert("ClientId".to_string(), json!(client));
            body.insert("ChallengeName".to_string(), json!("NEW_PASSWORD_REQUIRED"));
            body.insert("Session".to_string(), json!(session));
            body.insert("ChallengeResponses".to_string(), responses(username));
            let (status, payload) = call(&router, replay_operation, &Value::Object(body)).await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (
                    400,
                    "NotAuthorizedException",
                    SESSION_NOT_FOR_THIS_REQUEST
                ),
                "case {case:?} replayed through {elsewhere_case:?}: the spent session must not \
                 be reported to it, got {status} with {payload}"
            );
        }
    }
}

/// What a sign-in row requires of the answer.
struct Answer {
    status: u16,
    code: &'static str,
    message: Option<&'static str>,
}

/// A served sign-in.
const SERVED: Answer = Answer {
    status: 200,
    code: "",
    message: None,
};

/// A refusal of `code` carrying `message`.
const fn refused(code: &'static str, message: &'static str) -> Answer {
    Answer {
        status: 400,
        code,
        message: Some(message),
    }
}

/// RULE: a password sign-in reads the stored user's own account status BEFORE the
/// credential the request presents, so a user that cannot sign in at all is reported for
/// what it is whether or not the password was right — while a user nobody registered stays
/// masked where the app client suppresses existence errors.
#[tokio::test]
async fn the_account_status_decides_the_refusal_whatever_the_credential() {
    let router = router();
    let flows = ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_ADMIN_USER_PASSWORD_AUTH"];
    let user_pool_id = create_pool(&router, "status-pool").await;
    let legacy = create_client(&router, &user_pool_id, "legacy", &flows, None, None, None).await;
    let hiding = create_client(
        &router,
        &user_pool_id,
        "hiding",
        &flows,
        None,
        None,
        Some("ENABLED"),
    )
    .await;

    // One user per account status the API can put a user in, each holding the same
    // password, so the ROW's password is what differs.
    create_confirmed_user(&router, &user_pool_id, "confirmed").await;
    ok(
        &router,
        "SignUp",
        &json!({ "ClientId": legacy, "Username": "unconfirmed", "Password": GOOD_PASSWORD }),
    )
    .await;
    // `AdminResetUserPassword` sends the user a message, so the user it is called for
    // needs a verified address.
    create_confirmed_user_with_email(&router, &user_pool_id, "resetrequired").await;
    ok(
        &router,
        "AdminResetUserPassword",
        &json!({ "UserPoolId": user_pool_id, "Username": "resetrequired" }),
    )
    .await;
    create_confirmed_user(&router, &user_pool_id, "disabled").await;
    ok(
        &router,
        "AdminDisableUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "disabled" }),
    )
    .await;

    // Each row: the user, the password presented, the answer the stored state owes — the
    // SAME answer for the right password and for the wrong one wherever the account
    // status decides it — and the answer an app client that suppresses existence errors
    // owes, where it is a different one.
    let rows: [(&str, &str, &str, Answer, Option<Answer>); 9] = [
        (
            "a confirmed user with its password",
            "confirmed",
            GOOD_PASSWORD,
            SERVED,
            None,
        ),
        (
            "a confirmed user with the wrong password",
            "confirmed",
            WRONG_PASSWORD,
            refused("NotAuthorizedException", "Incorrect username or password."),
            None,
        ),
        (
            "an unconfirmed user with its password",
            "unconfirmed",
            GOOD_PASSWORD,
            refused("UserNotConfirmedException", "User is not confirmed."),
            None,
        ),
        (
            "an unconfirmed user with the wrong password",
            "unconfirmed",
            WRONG_PASSWORD,
            refused("UserNotConfirmedException", "User is not confirmed."),
            None,
        ),
        (
            "a user in RESET_REQUIRED with its password",
            "resetrequired",
            GOOD_PASSWORD,
            refused(
                "PasswordResetRequiredException",
                "Password reset required for the user",
            ),
            None,
        ),
        (
            "a user in RESET_REQUIRED with the wrong password",
            "resetrequired",
            WRONG_PASSWORD,
            refused(
                "PasswordResetRequiredException",
                "Password reset required for the user",
            ),
            None,
        ),
        (
            "a disabled user with its password",
            "disabled",
            GOOD_PASSWORD,
            refused("NotAuthorizedException", "User is disabled."),
            None,
        ),
        (
            "a disabled user with the wrong password",
            "disabled",
            WRONG_PASSWORD,
            refused("NotAuthorizedException", "User is disabled."),
            None,
        ),
        (
            // `PreventUserExistenceErrors=ENABLED` "suppresses messages that might
            // indicate a valid user exists" (`CreateUserPoolClient` in this service's
            // model), which is about a user that does NOT exist: every row above names a
            // user that does, and gets its own answer on either app client.
            "a user nobody registered",
            "nosuchuser",
            GOOD_PASSWORD,
            refused("UserNotFoundException", "User does not exist."),
            Some(refused(
                "NotAuthorizedException",
                "Incorrect username or password.",
            )),
        ),
    ];

    for (case, username, password, expected, expected_when_hiding) in rows {
        // The same rule holds for the client-side sign-in and the administrative one, and
        // for an app client that suppresses existence errors as much as for one that does
        // not.
        for (client_label, client_id, expected) in [
            ("a legacy app client", legacy.as_str(), &expected),
            (
                "an app client that suppresses existence errors",
                hiding.as_str(),
                expected_when_hiding.as_ref().unwrap_or(&expected),
            ),
        ] {
            for (api, body) in [
                (
                    "InitiateAuth",
                    json!({
                        "ClientId": client_id,
                        "AuthFlow": "USER_PASSWORD_AUTH",
                        "AuthParameters": { "USERNAME": username, "PASSWORD": password },
                    }),
                ),
                (
                    "AdminInitiateAuth",
                    json!({
                        "UserPoolId": user_pool_id,
                        "ClientId": client_id,
                        "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                        "AuthParameters": { "USERNAME": username, "PASSWORD": password },
                    }),
                ),
            ] {
                let (status, payload) = call(&router, api, &body).await;
                assert_eq!(
                    status, expected.status,
                    "case {case:?} on {api} through {client_label}: expected HTTP {}, got \
                     {status} with {payload}",
                    expected.status
                );
                if let Some(message) = expected.message {
                    assert_eq!(
                        (error_code(&payload), error_message(&payload)),
                        (expected.code, message),
                        "case {case:?} on {api} through {client_label}: expected {:?} saying \
                         {message:?}, got {payload}",
                        expected.code
                    );
                }
            }
        }
    }
}

/// RULE: `AdminLinkProviderForUser` links an EXTERNAL identity, so a `SourceUser` that
/// resolves to one of the pool's own local profiles is refused as the native user it
/// names — the native provider outright, and a federated identity a live link has made a
/// local profile's — while a repeat of the link an identity already holds says so.
#[tokio::test]
async fn a_source_user_of_the_native_provider_is_not_an_external_identity() {
    let router = router();
    let user_pool_id = create_pool(&router, "provider-link-pool").await;
    for username in ["ada", "grace"] {
        create_confirmed_user(&router, &user_pool_id, username).await;
    }
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": "CorpSAML",
            "ProviderType": "SAML",
            "ProviderDetails": { "MetadataURL": "https://idp.example.test/metadata" },
        }),
    )
    .await;

    /// A `ProviderUserIdentifierType` naming every member.
    fn identity(provider_name: &str, attribute_name: &str, attribute_value: &str) -> Value {
        json!({
            "ProviderName": provider_name,
            "ProviderAttributeName": attribute_name,
            "ProviderAttributeValue": attribute_value,
        })
    }

    let link = |source: Value, destination: &str| {
        json!({
            "UserPoolId": user_pool_id,
            "SourceUser": source,
            "DestinationUser": {
                "ProviderName": NATIVE_PROVIDER,
                "ProviderAttributeValue": destination,
            },
        })
    };

    // The rows differ along the FORM of the native identifier: the documented subject
    // attribute, another attribute name, a value no user of the pool holds, and a value
    // that names a user by an attribute rather than by its username.
    let rows: [(&str, Value); 4] = [
        (
            "the native provider, by the documented subject attribute",
            identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, "ada"),
        ),
        (
            "the native provider, by another attribute name",
            identity(NATIVE_PROVIDER, "username", "ada"),
        ),
        (
            "the native provider, naming a user the pool does not hold",
            identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, "nosuchuser"),
        ),
        (
            "the native provider, by an attribute value",
            identity(NATIVE_PROVIDER, "email", "ada@example.test"),
        ),
    ];

    for (case, source) in rows {
        let (status, payload) =
            call(&router, "AdminLinkProviderForUser", &link(source, "grace")).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", INVALID_SOURCE_USER),
            "case {case:?}: a native SourceUser links nothing, got {status} with {payload}"
        );
    }

    // An EXTERNAL identity is what this member is for, and it links to one profile: a
    // call that points the identity at another profile submits an identity the live link
    // has made local, and a repeat of the link it already holds is told so.
    let source = identity("CorpSAML", "email", "ada@example.test");
    ok(
        &router,
        "AdminLinkProviderForUser",
        &link(source.clone(), "ada"),
    )
    .await;
    for (case, destination, expected) in [
        (
            "the linked identity re-pointed at another profile",
            "grace",
            INVALID_SOURCE_USER,
        ),
        (
            "the linked identity linked again to the profile it answers for",
            "ada",
            ALREADY_LINKED,
        ),
    ] {
        let (status, payload) = call(
            &router,
            "AdminLinkProviderForUser",
            &link(source.clone(), destination),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", expected),
            "case {case:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // ROUND TRIP: the one stored link is still the first one — every refusal above wrote
    // nothing — so deactivating it succeeds exactly once.
    ok(
        &router,
        "AdminDisableProviderForUser",
        &json!({ "UserPoolId": user_pool_id, "User": source }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "AdminDisableProviderForUser",
        &json!({ "UserPoolId": user_pool_id, "User": source }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "the link is removed once, got {status} with {payload}"
    );
}
