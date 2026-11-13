// SPDX-License-Identifier: CC0-1.0

//! `GetClientToken` over the wire: the client-credentials grant of a user pool.
//!
//! The rule under test. `GetClientToken` "issues an access token for machine-to-machine
//! (M2M) authorization. Your app client provides its client ID and secret, and receives
//! an access token that authorizes requests to your resource servers", and "to use this
//! operation, you must configure the app client with a client secret and enable the
//! ALLOW_CLIENT_TOKEN_AUTH authentication flow" (this operation in the service model).
//! So the grant is authorized by the STORED app client's secret and its stored
//! `ExplicitAuthFlows`, and the one token it mints is issued to that app client rather
//! than to any user: its subject is the client id, its lifetime is the client's own
//! configured access-token validity, and it is signed with the key the POOL publishes.
//!
//! "The custom scopes to authorize in the access token, in the format
//! resource-server-identifier/scope-name. Each scope must belong to a resource server in
//! your user pool. If you don't specify any scopes, Amazon Cognito authorizes the scopes
//! that are configured for the app client" (`Scopes`), so the rows below differ along
//! what that rule generalises over: how many scopes are asked for (none, one, several),
//! whether they are asked for at all or taken from the app client's configuration, which
//! of several resource servers defines them, a resource server identifier that itself
//! carries the `/` separator, and the access-token validity the lifetime is read off —
//! with a row on each bound of the window the model documents for an access token's
//! lifetime (five minutes to one day).
//!
//! A token issued to an app client is NOT the "access token that Amazon Cognito issued
//! to the currently signed-in user" the self-service operations ask for, and those
//! operations "must include the scope aws.cognito.signin.user.admin" (`GetUser` in this
//! service's model) — a scope a machine-to-machine token does not carry, since its scopes
//! authorize resource servers. So each row presents its token to `GetUser` and requires
//! the refusal a live token without the required scopes gets, which is a different answer
//! from the one a value resolving to no access token at all gets.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for, which is the region the pools they create
/// live in.
const DEFAULT_REGION: &str = "us-east-1";

/// The operation under test.
const GRANT: &str = "GetClientToken";

/// The `ExplicitAuthFlows` value that configures an app client for machine-to-machine
/// authorization, which this operation's own doc names.
const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) the suite's app
/// clients are created with, so a request can present the right one.
const CLIENT_SECRET: &str = "m2msecret00000000000000001";

/// A `ClientSecretType`-shaped secret no app client of the suite holds.
const WRONG_CLIENT_SECRET: &str = "wrongsecret0000000000000000";

/// A `ClientIdType`-shaped id (pattern `^[\w+]+$`, length 1..128) no create minted.
const UNKNOWN_CLIENT_ID: &str = "nosuchappclient0000";

/// One emulator with just this service mounted, beside the engine holding its state.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{DEFAULT_REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
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
    let (status, payload) = dispatch(router, operation, body).await;
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Call `operation` expecting a refusal, assert its status and wire error code, and hand
/// the body back so a caller can read the message too.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    payload
}

/// A base64url segment's bytes: the codec the wire serializer uses
/// (`aws_smithy_types::base64`), with the two alphabet substitutions and the padding
/// RFC 7515 §2 strips restored first.
fn decode_base64url(case: &str, segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("case {case:?}: {segment:?} must be base64url ({error})"))
}

/// The three segments of a compact JWS (RFC 7515 §3.1), each of which must be non-empty.
fn segments<'a>(case: &str, token: &'a str) -> Vec<&'a str> {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: a compact JWS carries three segments, got {token:?}"
    );
    assert!(
        segments.iter().all(|segment| !segment.is_empty()),
        "case {case:?}: every JWS segment is non-empty, got {token:?}"
    );
    segments
}

/// The decoded JOSE header of `token`.
fn jose_header(case: &str, token: &str) -> Value {
    let segment = segments(case, token)[0].to_string();
    serde_json::from_slice(&decode_base64url(case, &segment))
        .unwrap_or_else(|error| panic!("case {case:?}: the header must be JSON ({error})"))
}

/// The decoded claims of `token`.
fn claims(case: &str, token: &str) -> Value {
    let segment = segments(case, token)[1].to_string();
    serde_json::from_slice(&decode_base64url(case, &segment))
        .unwrap_or_else(|error| panic!("case {case:?}: the claims must be JSON ({error})"))
}

/// The key set the stored pool publishes, read from this service's state the way the
/// endpoint that serves it does.
fn pool_jwks(engine: &engine::InMemoryEngine, user_pool_id: &str) -> Value {
    let service_state = state::service_state(engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .unwrap_or_else(|| panic!("the stored pool {user_pool_id}"))
        .jwks()
}

/// The stored record of `token`, as this service wrote it when it issued the token.
fn token_record(engine: &engine::InMemoryEngine, token: &str) -> Option<state::TokenRecord> {
    let service_state = state::service_state(engine).expect("this service's state");
    let guard = service_state.lock();
    guard.token(token).cloned()
}

/// One row of the grant table: the pool and app client it is served by, what the request
/// asks for, and what the issued token must carry.
struct GrantCase {
    case: &'static str,
    pool_name: &'static str,
    /// The resource servers created in the pool, each as its identifier and its scope
    /// names.
    resource_servers: &'static [(&'static str, &'static [&'static str])],
    /// The app client's `AllowedOAuthScopes`.
    configured_scopes: &'static [&'static str],
    /// The app client's `AccessTokenValidity` and the `TokenValidityUnits.AccessToken`
    /// unit it is counted in, when the client configures one.
    access_token_validity: Option<(i64, &'static str)>,
    /// The `Scopes` the request asks for, and `None` for a request that sends no `Scopes`
    /// member at all.
    requested_scopes: Option<&'static [&'static str]>,
    /// The scopes the issued token must authorize, in order.
    expected_scopes: &'static [&'static str],
    /// The `ExpiresIn` the result must carry, in seconds.
    expected_expires_in: i64,
}

/// A created pool and the app client of it the grant is served for.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create the pool, resource servers and app client `case` describes.
async fn fixture(router: &MultiServiceRouter, case: &GrantCase) -> Fixture {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": case.pool_name }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    for (at, (identifier, scope_names)) in case.resource_servers.iter().enumerate() {
        let scopes: Vec<Value> = scope_names
            .iter()
            .map(|name| json!({ "ScopeName": name, "ScopeDescription": format!("the {name} scope") }))
            .collect();
        ok(
            router,
            "CreateResourceServer",
            &json!({
                "UserPoolId": user_pool_id,
                "Identifier": identifier,
                // `ResourceServerNameType`'s pattern is `^[\w\s+=,.@-]+$`, which an
                // identifier's own characters need not satisfy.
                "Name": format!("resource server {}", at + 1),
                "Scopes": scopes,
            }),
        )
        .await;
    }

    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("m2m-client"));
    body.insert("ClientSecret".to_string(), json!(CLIENT_SECRET));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([CLIENT_TOKEN_AUTH_FLOW]),
    );
    body.insert("AllowedOAuthFlows".to_string(), json!(["client_credentials"]));
    body.insert(
        "AllowedOAuthFlowsUserPoolClient".to_string(),
        json!(true),
    );
    body.insert(
        "AllowedOAuthScopes".to_string(),
        json!(case.configured_scopes),
    );
    if let Some((validity, unit)) = case.access_token_validity {
        body.insert("AccessTokenValidity".to_string(), json!(validity));
        body.insert(
            "TokenValidityUnits".to_string(),
            json!({ "AccessToken": unit }),
        );
    }
    let client = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    Fixture {
        user_pool_id,
        client_id: client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
    }
}

/// The request body `case` sends for `client_id`.
fn grant_body(case: &GrantCase, client_id: &str) -> Value {
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(client_id));
    body.insert("Secret".to_string(), json!(CLIENT_SECRET));
    if let Some(scopes) = case.requested_scopes {
        body.insert("Scopes".to_string(), json!(scopes));
    }
    Value::Object(body)
}

/// A granted token is one signed access token of the app client that asked for it,
/// carrying the scopes the grant authorized and the lifetime the stored app client
/// configures — and it is not a user's access token.
#[tokio::test]
async fn a_client_credentials_grant_issues_one_scoped_access_token_of_the_app_client() {
    // The rows span what the rule generalises over: whether scopes are requested or
    // taken from the app client's configuration, how many there are, which resource
    // server defines them, an identifier carrying the `/` separator itself, and the
    // access-token lifetime, including both published bounds of `AccessTokenValidity`.
    let cases = [
        GrantCase {
            case: "no Scopes member falls back to the app client's configured scopes",
            pool_name: "configured-scopes-pool",
            resource_servers: &[("https://api.example.test", &["read", "write"])],
            configured_scopes: &["https://api.example.test/read"],
            access_token_validity: None,
            requested_scopes: None,
            expected_scopes: &["https://api.example.test/read"],
            // "If you don't specify otherwise in the configuration of your app client,
            // your access tokens are valid for one hour" (`AccessTokenValidity`).
            expected_expires_in: 3_600,
        },
        GrantCase {
            case: "one requested scope of the pool's only resource server",
            pool_name: "one-scope-pool",
            resource_servers: &[("https://inventory.example.test", &["items.read"])],
            configured_scopes: &[],
            access_token_validity: Some((15, "minutes")),
            requested_scopes: Some(&["https://inventory.example.test/items.read"]),
            expected_scopes: &["https://inventory.example.test/items.read"],
            expected_expires_in: 900,
        },
        GrantCase {
            case: "several requested scopes across two resource servers, in the order asked for",
            pool_name: "many-scopes-pool",
            resource_servers: &[
                ("https://orders.example.test", &["place", "cancel"]),
                ("urn:billing", &["invoice.read"]),
            ],
            configured_scopes: &["https://orders.example.test/place"],
            access_token_validity: Some((2, "hours")),
            requested_scopes: Some(&[
                "urn:billing/invoice.read",
                "https://orders.example.test/cancel",
                "https://orders.example.test/place",
            ]),
            expected_scopes: &[
                "urn:billing/invoice.read",
                "https://orders.example.test/cancel",
                "https://orders.example.test/place",
            ],
            expected_expires_in: 7_200,
        },
        GrantCase {
            case: "an app client configured with no scopes, asked for none",
            pool_name: "no-scopes-pool",
            resource_servers: &[],
            configured_scopes: &[],
            // The lower bound of the window the model documents for an access
            // token's lifetime: five minutes, named in seconds.
            access_token_validity: Some((300, "seconds")),
            requested_scopes: None,
            expected_scopes: &[],
            expected_expires_in: 300,
        },
        GrantCase {
            case: "a resource server identifier that carries the scope separator itself",
            pool_name: "separator-pool",
            resource_servers: &[("https://api.example.test/v2", &["read"])],
            configured_scopes: &[],
            // The upper bound of that window: one day, named in seconds, which
            // is also the member's modelled maximum.
            access_token_validity: Some((86_400, "seconds")),
            requested_scopes: Some(&["https://api.example.test/v2/read"]),
            expected_scopes: &["https://api.example.test/v2/read"],
            expected_expires_in: 86_400,
        },
    ];

    let (router, engine) = router_and_engine();
    for grant in &cases {
        let case = grant.case;
        let fixture = fixture(&router, grant).await;
        let answered = ok(&router, GRANT, &grant_body(grant, &fixture.client_id)).await;
        let result = &answered["ClientAuthenticationResult"];

        assert_eq!(
            result["TokenType"].as_str(),
            Some("Bearer"),
            "case {case:?}: the token is presented as a bearer token, got {result}"
        );
        assert_eq!(
            result["ExpiresIn"].as_i64(),
            Some(grant.expected_expires_in),
            "case {case:?}: ExpiresIn is the configured access-token lifetime in \
             seconds, expected {}, got {result}",
            grant.expected_expires_in
        );

        let access_token = result["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an issued access token, got {result}"))
            .to_string();

        // The token names the key the ISSUING POOL publishes, so a resource server can
        // verify it against that pool's key set.
        let header = jose_header(case, &access_token);
        let jwks = pool_jwks(&engine, &fixture.user_pool_id);
        let published = &jwks["keys"][0];
        assert_eq!(
            (header["kid"].as_str(), header["alg"].as_str()),
            (published["kid"].as_str(), published["alg"].as_str()),
            "case {case:?}: the header names the pool's published key and algorithm, \
             got {header} against {jwks}"
        );

        let claims = claims(case, &access_token);
        let expected_scope = grant.expected_scopes.join(" ");
        let expected_claims = json!({
            "sub": fixture.client_id,
            "client_id": fixture.client_id,
            "token_use": "access",
            "scope": expected_scope,
            "iss": format!("https://{ENDPOINT_HOST}/{}", fixture.user_pool_id),
        });
        for (name, expected) in expected_claims
            .as_object()
            .expect("the expected claims")
            .iter()
        {
            assert_eq!(
                claims.get(name),
                Some(expected),
                "case {case:?}: claim {name:?} must be {expected}, got {claims}"
            );
        }
        assert!(
            claims.get("username").is_none() && claims.get("cognito:username").is_none(),
            "case {case:?}: a token issued to an app client names no user, got {claims}"
        );
        assert!(
            claims["jti"].as_str().is_some_and(|jti| !jti.is_empty()),
            "case {case:?}: an issued token carries its own identifier, got {claims}"
        );
        let (issued_at, expires) = (claims["iat"].as_i64(), claims["exp"].as_i64());
        assert_eq!(
            expires.zip(issued_at).map(|(exp, iat)| exp - iat),
            Some(grant.expected_expires_in),
            "case {case:?}: the token expires {} seconds after it was issued, got {claims}",
            grant.expected_expires_in
        );

        // The grant WROTE the token it handed out: a later caller sees the record this
        // service stored, with the app client as its subject and no user at all.
        let record = token_record(&engine, &access_token).unwrap_or_else(|| {
            panic!("case {case:?}: the issued token must be recorded in this service's state")
        });
        assert_eq!(
            (
                record.kind,
                record.client_id.as_str(),
                record.user_pool_id.as_str(),
                record.username.as_str(),
                record.sub.as_str(),
                record.revoked,
            ),
            (
                state::TokenKind::ClientCredentials,
                fixture.client_id.as_str(),
                fixture.user_pool_id.as_str(),
                "",
                fixture.client_id.as_str(),
                false,
            ),
            "case {case:?}: the stored record is a client-credentials token of this app \
             client, got {record:?}"
        );

        // "A valid access token that Amazon Cognito issued to the currently signed-in
        // user" (`GetUser.AccessToken`), which "must include a scope claim for
        // aws.cognito.signin.user.admin", is not what this grant issued: its scopes
        // authorize resource servers, so the self-service surface refuses it as a token
        // without the scopes it requires — not as a token nothing issued.
        let refusal = refused(
            &router,
            case,
            "GetUser",
            &json!({ "AccessToken": access_token }),
            400,
            "NotAuthorizedException",
        )
        .await;
        assert_eq!(
            error_message(&refusal),
            "Access Token does not have required scopes",
            "case {case:?}: a machine-to-machine token does not authorize a user \
             self-service operation, got {refusal}"
        );
    }
}

/// What the grant refuses, and why: the app client has to exist, prove the secret it
/// stores, be configured for this flow, and ask only for scopes its pool defines.
#[tokio::test]
async fn a_client_credentials_grant_is_refused_against_stored_configuration() {
    let (router, _engine) = router_and_engine();

    // The app client the happy path of this suite is served by: a secret, the
    // machine-to-machine flow, and one resource server with one scope.
    let served = fixture(
        &router,
        &GrantCase {
            case: "the served app client",
            pool_name: "refusal-pool",
            resource_servers: &[("https://api.example.test", &["read"])],
            configured_scopes: &["https://api.example.test/read"],
            access_token_validity: None,
            requested_scopes: None,
            expected_scopes: &[],
            expected_expires_in: 0,
        },
    )
    .await;

    // A second app client of the SAME pool that holds a secret but is configured for a
    // user authentication flow instead — "the ALLOW_CLIENT_TOKEN_AUTH flow is mutually
    // exclusive with user authentication flows" (this operation's own doc).
    let user_flow_client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": served.user_pool_id,
            "ClientName": "user-flow-client",
            "ClientSecret": CLIENT_SECRET,
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    // A third app client configured for this flow but holding NO secret, so there is
    // nothing a request could prove it knows.
    let secretless_client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": served.user_pool_id,
            "ClientName": "secretless-client",
            "ExplicitAuthFlows": [CLIENT_TOKEN_AUTH_FLOW],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    // The message each refusal owes: the app client the request names for an id no pool
    // holds, the one sentence a pair this grant does not authenticate gets, the flow the
    // app client is not configured for, and the refusal of a scope the pool defines
    // nowhere.
    let bad_secret = "Invalid client or secret";
    let flow_not_enabled = format!("{CLIENT_TOKEN_AUTH_FLOW} flow not enabled for this client");
    let bad_scope = "Invalid scopes requested";

    let rows: [(&str, Value, u16, &str, String); 9] = [
        (
            "a ClientId nothing created",
            json!({ "ClientId": UNKNOWN_CLIENT_ID, "Secret": CLIENT_SECRET }),
            400,
            "ResourceNotFoundException",
            format!("User pool client {UNKNOWN_CLIENT_ID} does not exist."),
        ),
        (
            "a Secret that is not the one the app client stores",
            json!({ "ClientId": served.client_id, "Secret": WRONG_CLIENT_SECRET }),
            400,
            "NotAuthorizedException",
            bad_secret.to_string(),
        ),
        (
            "an app client that holds no secret at all",
            json!({ "ClientId": secretless_client, "Secret": CLIENT_SECRET }),
            400,
            "NotAuthorizedException",
            bad_secret.to_string(),
        ),
        (
            "an app client configured for a user authentication flow",
            json!({ "ClientId": user_flow_client, "Secret": CLIENT_SECRET }),
            400,
            "InvalidParameterException",
            flow_not_enabled.clone(),
        ),
        (
            "a scope no resource server of the pool defines",
            json!({
                "ClientId": served.client_id,
                "Secret": CLIENT_SECRET,
                "Scopes": ["https://other.example.test/read"],
            }),
            400,
            "NotAuthorizedException",
            bad_scope.to_string(),
        ),
        (
            "a scope name the pool's resource server does not define",
            json!({
                "ClientId": served.client_id,
                "Secret": CLIENT_SECRET,
                "Scopes": ["https://api.example.test/write"],
            }),
            400,
            "NotAuthorizedException",
            bad_scope.to_string(),
        ),
        (
            "a bare resource server identifier, with no scope name",
            json!({
                "ClientId": served.client_id,
                "Secret": CLIENT_SECRET,
                "Scopes": ["https://api.example.test"],
            }),
            400,
            "NotAuthorizedException",
            bad_scope.to_string(),
        ),
        (
            "a defined scope name under no resource server",
            json!({
                "ClientId": served.client_id,
                "Secret": CLIENT_SECRET,
                "Scopes": ["read"],
            }),
            400,
            "NotAuthorizedException",
            bad_scope.to_string(),
        ),
        (
            "one defined scope beside one the pool does not define",
            json!({
                "ClientId": served.client_id,
                "Secret": CLIENT_SECRET,
                "Scopes": ["https://api.example.test/read", "https://api.example.test/admin"],
            }),
            400,
            "NotAuthorizedException",
            bad_scope.to_string(),
        ),
    ];

    for (case, body, status, code, expected_message) in rows {
        let refusal = refused(&router, case, GRANT, &body, status, code).await;
        assert_eq!(
            error_message(&refusal),
            expected_message,
            "case {case:?}: expected message {expected_message:?}, got {refusal}"
        );
    }
}

/// The self-service surface tells a LIVE access token that does not authorize it apart
/// from a value that resolves to no access token at all: each of those operations "must
/// include the scope `aws.cognito.signin.user.admin`" (this service's model), which the
/// access token of a sign-in carries and a machine-to-machine token does not, while an
/// id or refresh token is not an access token in the first place.
#[tokio::test]
async fn a_token_without_the_self_service_scope_is_told_apart_from_no_access_token() {
    let (router, _engine) = router_and_engine();
    let m2m = GrantCase {
        case: "the machine-to-machine app client",
        pool_name: "scope-check-pool",
        resource_servers: &[("https://api.example.test", &["read"])],
        configured_scopes: &["https://api.example.test/read"],
        access_token_validity: None,
        requested_scopes: None,
        expected_scopes: &["https://api.example.test/read"],
        expected_expires_in: 3_600,
    };
    let fixture = fixture(&router, &m2m).await;
    let client_token = ok(&router, GRANT, &grant_body(&m2m, &fixture.client_id)).await
        ["ClientAuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("an issued access token")
        .to_string();

    // A user of the SAME pool, signed in through an app client of its own: the one
    // whose access token authorizes the self-service operations.
    let user_client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientName": "user-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "ada" }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "ada",
            "Password": "Passw0rd!2024",
            "Permanent": true,
        }),
    )
    .await;
    let signed_in = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": user_client,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "ada", "PASSWORD": "Passw0rd!2024" },
        }),
    )
    .await["AuthenticationResult"]
        .clone();
    let token = |member: &str| {
        signed_in[member]
            .as_str()
            .unwrap_or_else(|| panic!("the sign-in's {member}"))
            .to_string()
    };

    // The access token of the sign-in is the one `GetUser` asks for.
    let answered = ok(&router, "GetUser", &json!({ "AccessToken": token("AccessToken") })).await;
    assert_eq!(
        answered["Username"].as_str(),
        Some("ada"),
        "case {:?}: the signed-in user's own access token is served, got {answered}",
        "the access token of a signed-in user"
    );

    let rows: [(&str, String, &str); 4] = [
        (
            "the id token of the same sign-in",
            token("IdToken"),
            "Invalid Access Token",
        ),
        (
            "the refresh token of the same sign-in",
            token("RefreshToken"),
            "Invalid Access Token",
        ),
        (
            "a value this emulator never issued",
            "not.a.token".to_string(),
            "Invalid Access Token",
        ),
        (
            "the machine-to-machine token of an app client of the same pool",
            client_token,
            "Access Token does not have required scopes",
        ),
    ];

    for (case, presented, expected_message) in rows {
        let refusal = refused(
            &router,
            case,
            "GetUser",
            &json!({ "AccessToken": presented }),
            400,
            "NotAuthorizedException",
        )
        .await;
        assert_eq!(
            error_message(&refusal),
            expected_message,
            "case {case:?}: expected message {expected_message:?}, got {refusal}"
        );
    }
}

/// Repeating the grant mints a NEW token each time and leaves every token it issued
/// before standing: nothing about the app client or the earlier token is consumed.
#[tokio::test]
async fn repeating_the_grant_mints_a_new_token_and_keeps_the_earlier_one() {
    let (router, engine) = router_and_engine();
    let case = GrantCase {
        case: "repeated grants",
        pool_name: "repeat-pool",
        resource_servers: &[("https://api.example.test", &["read"])],
        configured_scopes: &["https://api.example.test/read"],
        access_token_validity: None,
        requested_scopes: None,
        expected_scopes: &["https://api.example.test/read"],
        expected_expires_in: 3_600,
    };
    let fixture = fixture(&router, &case).await;

    let mut issued: Vec<String> = Vec::new();
    for _ in 0..3 {
        let answered = ok(&router, GRANT, &grant_body(&case, &fixture.client_id)).await;
        issued.push(
            answered["ClientAuthenticationResult"]["AccessToken"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: an issued access token", case.case))
                .to_string(),
        );
    }

    for (at, token) in issued.iter().enumerate() {
        assert!(
            token_record(&engine, token).is_some(),
            "case {:?}: the token from grant {at} is still recorded, got none",
            case.case
        );
        assert_eq!(
            issued.iter().filter(|other| *other == token).count(),
            1,
            "case {:?}: every grant mints its own token, grant {at} repeated one of {issued:?}",
            case.case
        );
    }
}
