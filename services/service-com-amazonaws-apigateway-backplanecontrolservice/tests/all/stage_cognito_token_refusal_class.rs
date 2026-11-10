// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a `COGNITO_USER_POOLS`-gated method answers a token it does not admit with.
//!
//! A deployed method whose `authorizationType` is `COGNITO_USER_POOLS` is gated by the
//! `Authorizer` it names, and that authorizer's verdict is a function of the presented token
//! and of the user pools its `providerARNs` list — every one of which this emulator holds in
//! this same process, publishing the key set a token is checked against. So this suite mounts
//! the user-pool service beside API Gateway on one engine: every token a row presents is one a
//! pool MINTED, or that same pool's own signing key applied to a claim set a row changed, and
//! nothing about a token is written here that no pool would sign.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A request a `COGNITO_USER_POOLS` authorizer does not admit is refused in the CLASS the
//! > failure belongs to, and never through the `500` of an authorizer whose decision could not
//! > be obtained. A request this authorizer does not admit authorized nobody, which is `401`
//! > `{"message":"Unauthorized"}`: nothing at the `Authorizer$identitySource`, a value that is
//! > not a signed token, a token naming no issuer, a token whose issuer no `providerARNs` entry
//! > names, one whose window cannot be read or has not begun, an audience its
//! > `identityValidationExpression` does not match, or none of the method's
//! > `authorizationScopes` claimed. Two classes this endpoint words differently. A token a
//! > trusted pool signed whose own `exp` has passed carries the sentence AWS words an expired
//! > token with at that same `401`, `{"message":"The incoming token has expired"}`, because the
//! > holder's remedy is another token. And a token claiming a pool the authorizer TRUSTS whose
//! > signature does not check out against that pool's published key set — a value presenting
//! > itself as a credential that pool never signed — is refused as the authorizer DENYING the
//! > call, answered through the same `ACCESS_DENIED` gateway response a `CUSTOM` authorizer's
//! > denial is: `403` `{"Message":"User is not authorized to access this resource"}`. A token
//! > that passes every one of those checks reaches the integration, with its claims readable as
//! > `$context.authorizer.claims.<name>`.
//!
//! The rows span what that rule generalises over: WHAT was presented (nothing, an empty header,
//! a value that is not a token, another pool's token, the pool's own token, that token
//! re-encoded with a claim changed, with its signature segment rewritten, with no issuer, with
//! no expiry, with a future `nbf`, with an elapsed `exp` — a second ago and an hour ago), HOW
//! it was presented (bare and under the RFC 6750 bearer scheme) and WHICH gate it met (an
//! authorizer over the pool, one pinning an audience, and a method declaring a scope).
//!
//! The two `403` rows are the class the recorded tampered-token leg of
//! `apigw-cognito-authorizer-invalid-token-negative` pins, and the untrusted-pool row the class
//! `apigw-cognito-authorizer-wrong-user-pool-negative` pins: both are a token the authorizer
//! does not accept, and the endpoint answers them differently, which is why the class rather
//! than the refusal is what this suite is about.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The region every call is signed for, and so the region of the pool's issuer and of the api's
/// own invoke host.
const REGION: &str = "us-west-2";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The permanent password this suite's fixture user signs in with — a value written for this
/// test and used nowhere else. It satisfies a pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The header the authorizers below name as their identity source.
const TOKEN_HEADER: &str = "Authorization";

/// The `401` a request this authorizer does not admit earns, and the sentence it carries.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The sentence a request whose token has expired earns, at that same `401`.
const EXPIRED_MESSAGE: &str = "The incoming token has expired";

/// The `403` a token claiming a TRUSTED pool that does not verify against that pool's key set
/// earns — the authorizer denying the call — and the sentence it carries, in the IAM-shaped
/// `Message` member an authorization refusal states its sentence under.
const DENIED: u16 = 403;
const DENIED_MESSAGE: &str = "User is not authorized to access this resource";

/// The sentence the `API_CONFIGURATION_ERROR` / `AUTHORIZER_CONFIGURATION_ERROR` gateway
/// responses carry: the `500` that must never answer a question about a token.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// A scope no Amazon Cognito token claims, used as the method scope a token cannot meet. The
/// form is a resource server's `{identifier}/{scope}`, as `Method$authorizationScopes`
/// documents the member ("the scopes are used with a COGNITO_USER_POOLS authorizer to authorize
/// the method invocation").
const UNCLAIMED_SCOPE: &str = "things/read";

/// The body the `MOCK` integration behind each gated resource answers: the claims of the token
/// the authorizer accepted, which is how a row observes that the gate let the request THROUGH.
const CLAIMS_TEMPLATE: &str =
    "{\"sub\":\"$context.authorizer.claims.sub\",\"use\":\"$context.authorizer.claims.token_use\"}";

/// One dispatched request, and what came back.
struct Answer {
    status: u16,
    body: Value,
    text: String,
}

impl Answer {
    /// One string member of a data-plane body, or `"<no message>"`.
    fn member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }

    fn summary(&self) -> String {
        format!("{} {}", self.status, self.text)
    }
}

/// API Gateway and the Amazon Cognito user pools service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the pool a token comes from is the pool this api's
/// authorizer reads its key set out of. The engine is answered too, because the pool's own
/// signing key is what the rows that need a token NO sign-in produces are signed with.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer { status, body, text }
}

/// One awsJson1_1 call to the user pools service, which must succeed.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let answer = dispatch(
        router,
        "POST",
        &format!("cognito-idp.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
            ("authorization", &authorization("cognito-idp")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: cognito-idp {operation} must answer 200, got {}",
        answer.summary()
    );
    answer.body
}

/// One API Gateway control-plane call that has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {}",
        answer.summary()
    );
    answer.body
}

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// `bytes` as one base64url segment of a compact JWS: "base64url without padding"
/// (RFC 7515 §2).
fn segment(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// One base64url segment of a compact JWS, decoded.
fn decoded(part: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .unwrap_or_else(|e| panic!("a minted token's segment must be base64url ({e})"))
}

/// The JOSE header and the claim set of a compact JWS: its first two segments.
fn parts_of(token: &str) -> (Value, Value) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {}",
        segments.len()
    );
    let read = |part: &str| -> Value {
        serde_json::from_slice(&decoded(part))
            .unwrap_or_else(|e| panic!("a minted token's segment must be JSON ({e})"))
    };
    (read(segments[0]), read(segments[1]))
}

/// The token the pool minted, re-encoded with the claim set given and its own signature segment
/// kept — so the only difference from the admitted token is the claim the row changed, and the
/// signature no longer checks out over it.
fn rebuilt(token: &str, claims: &Value) -> String {
    let mut segments = token.split('.');
    let header = segments.next().expect("a compact JWS has a header segment");
    let signature = token
        .split('.')
        .next_back()
        .expect("a compact JWS has a signature segment");
    format!(
        "{header}.{}.{signature}",
        segment(claims.to_string().as_bytes())
    )
}

/// `claims` as a token the POOL ITSELF signs: the key `pool_id` names in the user-pool service's
/// own store, under the `alg` and `kid` of the JWK that pool publishes.
///
/// This is how a row presents a token no sign-in produces — one whose `exp` has passed, whose
/// `nbf` has not arrived, or that names no issuer — without writing a signature by hand: the
/// bytes are signed by the same key `AdminInitiateAuth` signs with, so the only thing under test
/// is the claim the row changed.
fn signed_by_pool(engine: &Arc<engine::InMemoryEngine>, pool_id: &str, claims: &Value) -> String {
    let pools = cognito_idp::state::service_state(&**engine)
        .expect("the user pool service's state lives in the shared engine");
    let held = pools.lock();
    let pool = held
        .pool(pool_id)
        .unwrap_or_else(|| panic!("the pool {pool_id:?} created above must be held"));
    let key = pool.signing_key();
    let algorithm = key
        .to_jwk()
        .get("alg")
        .and_then(Value::as_str)
        .expect("the published JWK names the algorithm it verifies under")
        .to_string();
    let header = json!({ "alg": algorithm, "kid": key.kid(), "typ": "JWT" });
    let signing_input = format!(
        "{}.{}",
        segment(header.to_string().as_bytes()),
        segment(claims.to_string().as_bytes())
    );
    let signature = key
        .sign(signing_input.as_bytes())
        .expect("the pool's own key signs the token bytes");
    format!("{signing_input}.{}", segment(&signature))
}

/// Seconds since the epoch, which is what a token's `exp`/`nbf`/`iat` are
/// (RFC 7519 §2 NumericDate).
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch second fits an i64")
}

/// `claims` with `members` applied — the one claim a row changes, and the claims it removes.
fn with_claims(claims: &Value, members: &[(&str, Option<Value>)]) -> Value {
    let mut changed = claims.clone();
    let object = changed
        .as_object_mut()
        .expect("a token's claim set is a JSON object");
    for (name, value) in members {
        match value {
            Some(value) => {
                object.insert((*name).to_string(), value.clone());
            }
            None => {
                object.remove(*name);
            }
        }
    }
    changed
}

/// What one pool's sign-in produced: the pool's id and ARN, its app client id and the identity
/// token of the sign-in.
struct Pool {
    id: String,
    arn: String,
    client_id: String,
    id_token: String,
}

/// Mint one pool, one app client and one confirmed user, and sign that user in.
async fn pool_with_signed_in_user(
    router: &MultiServiceRouter,
    pool_name: &str,
    username: &str,
) -> Pool {
    let created = cognito(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let id = as_str("CreateUserPool", &created["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &created["UserPool"], "Arn");
    let client = cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": id,
            "ClientName": format!("{pool_name}-client"),
            // The flow the sign-in below uses, plus the refresh flow AWS requires beside any
            // other `ALLOW_` flow.
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]
        }),
    )
    .await;
    let client_id = as_str("CreateUserPoolClient", &client["UserPoolClient"], "ClientId");
    cognito(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true
        }),
    )
    .await;
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD }
        }),
    )
    .await;
    Pool {
        id,
        arn,
        client_id,
        id_token: as_str(
            "AdminInitiateAuth",
            &signed_in["AuthenticationResult"],
            "IdToken",
        ),
    }
}

/// One `COGNITO_USER_POOLS`-gated `MOCK` method under the api's root, answering the claims the
/// authorizer published. `scopes` is the method's own `authorizationScopes`, empty for a method
/// declaring none.
async fn gated_resource(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
    authorizer: Value,
    scopes: &[&str],
) {
    let created = apigw(
        router,
        &format!("CreateAuthorizer({path_part})"),
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(authorizer),
        201,
    )
    .await;
    let authorizer_id = as_str("CreateAuthorizer", &created, "id");
    let resource = apigw(
        router,
        &format!("CreateResource({path_part})"),
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": path_part })),
        201,
    )
    .await;
    let resource_id = as_str("CreateResource", &resource, "id");
    let mut method = json!({
        "authorizationType": "COGNITO_USER_POOLS",
        "authorizerId": authorizer_id
    });
    if !scopes.is_empty() {
        if let Some(members) = method.as_object_mut() {
            members.insert("authorizationScopes".to_string(), json!(scopes));
        }
    }
    apigw(
        router,
        &format!("PutMethod({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
        Some(method),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutIntegration({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration"),
        Some(json!({ "type": "MOCK" })),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutMethodResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutIntegrationResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration/responses/200"),
        Some(json!({ "responseTemplates": { "application/json": CLAIMS_TEMPLATE } })),
        201,
    )
    .await;
}

/// What the gate owes one row.
#[derive(Debug, Clone)]
enum Expected {
    /// The gate admitted the request and the `MOCK` integration answered it with the claims of
    /// the token presented: this `sub`.
    Served(String),
    /// The authorizer does not admit what the request presented, so it authorized nobody:
    /// `401`, carrying `sentence` under the `message` member.
    Unauthorized(&'static str),
    /// The token claims a pool the authorizer trusts and does not verify against that pool's
    /// key set, so the authorizer denies the call: `403`, carrying its sentence under the
    /// IAM-shaped `Message` member.
    Denied,
}

/// One request to the deployed stage: the gated resource it addresses, the value presented at
/// that resource's authorizer identity source, and what the gate owes it.
struct Row {
    case: String,
    path_part: &'static str,
    /// The value the `Authorization` header carries, or `None` to present nothing at all.
    presented: Option<String>,
    expected: Expected,
}

#[tokio::test]
async fn a_cognito_authorizer_refuses_a_token_in_the_class_of_its_failure() {
    let (router, engine) = emulator();

    // 1. The pool whose tokens the api accepts, and a second pool whose tokens come from a
    //    FOREIGN issuer.
    let home = pool_with_signed_in_user(&router, "refusal-class-pool", "gate-user").await;
    let foreign = pool_with_signed_in_user(&router, "refusal-class-other-pool", "other-user").await;
    let (_header, claims) = parts_of(&home.id_token);
    let sub = as_str("the pool's identity token", &claims, "sub");
    assert_eq!(
        claims.get("aud").and_then(Value::as_str),
        Some(home.client_id.as_str()),
        "the pool's identity token must name the client it was minted for, got {claims}"
    );

    // 2. The api: one gate per authorizer shape, and the stage they are deployed to.
    let api = apigw(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "cognito-refusal-class-api" })),
        201,
    )
    .await;
    let api_id = as_str("CreateRestApi", &api, "id");
    let root = as_str("CreateRestApi", &api, "rootResourceId");
    let header_source = format!("method.request.header.{TOKEN_HEADER}");
    gated_resource(
        &router,
        &api_id,
        &root,
        "by-one-pool",
        json!({
            "name": "one-pool",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": [home.arn],
            "identitySource": header_source
        }),
        &[],
    )
    .await;
    gated_resource(
        &router,
        &api_id,
        &root,
        "by-other-audience",
        json!({
            "name": "other-audience",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": [home.arn],
            "identitySource": header_source,
            // The audience this gate pins is the OTHER pool's app client, which the home pool's
            // token does not name.
            "identityValidationExpression": format!("^{}$", foreign.client_id)
        }),
        &[],
    )
    .await;
    gated_resource(
        &router,
        &api_id,
        &root,
        "by-unclaimed-scope",
        json!({
            "name": "unclaimed-scope",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": [home.arn],
            "identitySource": header_source
        }),
        &[UNCLAIMED_SCOPE],
    )
    .await;
    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api_id}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    // 3. The tokens no sign-in produces, each signed by the home pool's OWN key over a claim set
    //    that differs from the minted one in exactly what the row is about.
    let now = now_seconds();
    let expired_an_hour_ago = signed_by_pool(
        &engine,
        &home.id,
        &with_claims(&claims, &[("exp", Some(json!(now - 3600)))]),
    );
    let expired_a_second_ago = signed_by_pool(
        &engine,
        &home.id,
        &with_claims(&claims, &[("exp", Some(json!(now - 1)))]),
    );
    let not_yet_valid = signed_by_pool(
        &engine,
        &home.id,
        &with_claims(
            &claims,
            &[
                ("exp", Some(json!(now + 3600))),
                ("nbf", Some(json!(now + 600))),
            ],
        ),
    );
    let no_expiry = signed_by_pool(&engine, &home.id, &with_claims(&claims, &[("exp", None)]));
    let no_issuer = signed_by_pool(&engine, &home.id, &with_claims(&claims, &[("iss", None)]));
    // The recorded case: the minted token with its SIGNATURE segment rewritten, everything else
    // left exactly as the pool signed it.
    let tampered_signature = {
        let (head, signature) = home
            .id_token
            .rsplit_once('.')
            .expect("a compact JWS has a signature segment");
        let flipped: String = signature
            .chars()
            .map(|c| if c == 'A' { 'B' } else { 'A' })
            .collect();
        format!("{head}.{flipped}")
    };
    // The same token with ONE claim rewritten after signing, which the pool's signature does
    // not cover.
    let rewritten_subject = rebuilt(
        &home.id_token,
        &with_claims(
            &claims,
            &[(
                "sub",
                Some(json!("00000000-0000-0000-0000-000000000000")),
            )],
        ),
    );

    let rows: Vec<Row> = vec![
        Row {
            case: "the pool's identity token, bare".to_string(),
            path_part: "by-one-pool",
            presented: Some(home.id_token.clone()),
            expected: Expected::Served(sub.clone()),
        },
        Row {
            case: "the same token under the RFC 6750 bearer scheme".to_string(),
            path_part: "by-one-pool",
            presented: Some(format!("Bearer {}", home.id_token)),
            expected: Expected::Served(sub.clone()),
        },
        Row {
            case: "no token at all".to_string(),
            path_part: "by-one-pool",
            presented: None,
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "an empty token header".to_string(),
            path_part: "by-one-pool",
            presented: Some(String::new()),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a value that is not a token at all".to_string(),
            path_part: "by-one-pool",
            presented: Some("not-a-token".to_string()),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "another pool's token, which this authorizer does not list".to_string(),
            path_part: "by-one-pool",
            presented: Some(foreign.id_token.clone()),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed that names no issuer".to_string(),
            path_part: "by-one-pool",
            presented: Some(no_issuer),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed that names no expiry".to_string(),
            path_part: "by-one-pool",
            presented: Some(no_expiry),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose window has not begun".to_string(),
            path_part: "by-one-pool",
            presented: Some(not_yet_valid),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose exp passed an hour ago".to_string(),
            path_part: "by-one-pool",
            presented: Some(expired_an_hour_ago),
            expected: Expected::Unauthorized(EXPIRED_MESSAGE),
        },
        Row {
            case: "a token the pool signed whose exp passed a second ago".to_string(),
            path_part: "by-one-pool",
            presented: Some(expired_a_second_ago),
            expected: Expected::Unauthorized(EXPIRED_MESSAGE),
        },
        Row {
            case: "the pool's token with its signature segment rewritten".to_string(),
            path_part: "by-one-pool",
            presented: Some(tampered_signature),
            expected: Expected::Denied,
        },
        Row {
            case: "the pool's token with its subject rewritten after signing".to_string(),
            path_part: "by-one-pool",
            presented: Some(rewritten_subject),
            expected: Expected::Denied,
        },
        Row {
            case: "the pool's token at a gate pinning another audience".to_string(),
            path_part: "by-other-audience",
            presented: Some(home.id_token.clone()),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
        Row {
            case: "the pool's token at a method declaring a scope it claims none of".to_string(),
            path_part: "by-unclaimed-scope",
            presented: Some(home.id_token.clone()),
            expected: Expected::Unauthorized(UNAUTHORIZED_MESSAGE),
        },
    ];

    let invoke_host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
    for row in &rows {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(presented) = &row.presented {
            headers.push((TOKEN_HEADER, presented.as_str()));
        }
        let answer = dispatch(
            &router,
            "GET",
            &invoke_host,
            &format!("/{STAGE}/{}", row.path_part),
            &headers,
            Bytes::new(),
        )
        .await;
        // A question about a token is never answered by the 500 of an authorizer whose decision
        // could not be obtained.
        assert_ne!(
            answer.member("message"),
            CONFIGURATION_ERROR,
            "case {:?}: the authorizer must decide the call, not report a configuration error; \
             got {}",
            row.case,
            answer.summary()
        );
        match &row.expected {
            Expected::Unauthorized(sentence) => {
                assert_eq!(
                    (answer.status, answer.member("message")),
                    (UNAUTHORIZED, (*sentence).to_string()),
                    "case {:?}: expected {UNAUTHORIZED} {sentence:?}, got {}",
                    row.case,
                    answer.summary()
                );
            }
            Expected::Denied => {
                assert_eq!(
                    (answer.status, answer.member("Message")),
                    (DENIED, DENIED_MESSAGE.to_string()),
                    "case {:?}: expected {DENIED} {DENIED_MESSAGE:?}, got {}",
                    row.case,
                    answer.summary()
                );
            }
            Expected::Served(expected_sub) => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {:?}: expected the integration's 200, got {}",
                    row.case,
                    answer.summary()
                );
                assert_eq!(
                    answer.member("sub"),
                    *expected_sub,
                    "case {:?}: expected the token's sub {expected_sub:?} to reach the \
                     integration, got {}",
                    row.case,
                    answer.summary()
                );
                assert_eq!(
                    answer.member("use"),
                    "id",
                    "case {:?}: expected the token's token_use to reach the integration, got {}",
                    row.case,
                    answer.summary()
                );
            }
        }
    }
}
