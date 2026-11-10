// SPDX-License-Identifier: CC0-1.0

//! A `COGNITO_USER_POOLS` authorizer in front of a deployed REST API method, driven end to
//! end through the WHOLE assembled router.
//!
//! Two services and one engine, so this is a router suite: the tokens are minted by the
//! Amazon Cognito user pools service (signing name `cognito-idp`) and the gate that reads them
//! is API Gateway's own `execute-api` data plane (control plane signing name `apigateway`).
//! Nothing about a token is written here that a pool did not mint: the admitted rows present a
//! token `AdminInitiateAuth` answered with, and the refused rows present another pool's token,
//! or that same minted token re-encoded with one claim or its JOSE header changed.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A deployed method whose `authorizationType` is `COGNITO_USER_POOLS` admits a request only
//! > when the value at its `Authorizer$identitySource` is a signed token whose `iss` names a
//! > user pool that authorizer's `providerARNs` list, whose signature checks out against that
//! > pool's published key set, which is inside its own validity window, and — where the
//! > authorizer declares an `identityValidationExpression` — whose audience matches it.
//! > Anything else never reaches the integration, and is NEVER the `500` of an authorizer whose
//! > decision could not be obtained: a request this authorizer does not admit authorized
//! > nobody, which is `401` `{"message":"Unauthorized"}` — except for the token that claims a
//! > pool the authorizer TRUSTS and does not verify against that pool's key set, a value
//! > presenting itself as a credential that pool never signed, which is refused as the
//! > authorizer denying the call: `403` `{"Message":"User is not authorized to access this
//! > resource"}`. A method declaring `authorizationScopes` admits only a token claiming one of
//! > them. An admitted request reaches the integration with the token's claims available as
//! > `$context.authorizer.claims.<name>`.
//!
//! The rows span what that rule generalises over: WHICH pool minted the presented token (the
//! one the authorizer names, a second pool, none at all), WHICH kind of token it is (the
//! identity token and the access token of one sign-in), HOW it is presented (bare, with the
//! RFC 6750 bearer scheme, in a query parameter the authorizer's identity source names, absent,
//! empty, not a token at all), WHAT was changed in it (a claim, the JOSE `alg`), HOW MANY
//! pools the authorizer lists (one, two, one this emulator does not hold), WHETHER it pins
//! an audience (none, one the token matches, one it does not) and WHETHER the METHOD declares
//! `authorizationScopes` (none, one the access token claims, one no token claims).
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of each pool's issuer and of the
/// api's own invoke host.
const REGION: &str = "us-west-2";

/// The permanent password this suite's fixture users sign in with — a value written for this
/// test and used nowhere else. It satisfies a pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The header an authorizer's documented identity source names.
const TOKEN_HEADER: &str = "Authorization";

/// The query parameter the one query-sourced authorizer below extracts its token from.
const TOKEN_PARAMETER: &str = "access_token";

/// The status and sentence a request this authorizer does not admit earns, and that nothing the
/// method fronts ran.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status and sentence a token claiming a TRUSTED pool that does not verify against that
/// pool's key set earns: the authorizer denying the call, which the data plane answers through
/// the same `ACCESS_DENIED` gateway response a `CUSTOM` authorizer's refusal is answered
/// through — so the sentence travels in that response's IAM-shaped `Message` member.
const DENIED: u16 = 403;
const DENIED_MESSAGE: &str = "User is not authorized to access this resource";

/// The sentence the `API_CONFIGURATION_ERROR` / `AUTHORIZER_CONFIGURATION_ERROR` gateway
/// responses carry — the `500` that must never answer a token question.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The body the `MOCK` integration behind each gated resource answers: the claims of the token
/// the authorizer accepted, which is how a row observes that the gate let the request THROUGH
/// and that the integration could read the signed-in user.
const CLAIMS_TEMPLATE: &str =
    "{\"sub\":\"$context.authorizer.claims.sub\",\"use\":\"$context.authorizer.claims.token_use\"}";

/// The same, for the claim whose NAME is no bare identifier: a Cognito authorizer publishes
/// namespaced claims, and the bracketed member access is how a template reads one.
const NAMESPACED_CLAIM_TEMPLATE: &str =
    "{\"user\":\"$context.authorizer.claims['cognito:username']\"}";

/// A scope no Amazon Cognito token claims, used as the method scope a token cannot meet. The
/// form is a resource server's `{identifier}/{scope}`.
const UNCLAIMED_SCOPE: &str = "things/read";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    body: Value,
    /// The body as it arrived, for a response that is not JSON at all.
    text: String,
}

impl Answer {
    /// The `message` member of a data-plane body, or `"<no message>"`.
    fn message(&self) -> String {
        self.member("message")
    }

    /// The `Message` member an AUTHORIZATION refusal's body carries its sentence under, or
    /// `"<no message>"`.
    fn authorization_message(&self) -> String {
        self.member("Message")
    }

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

/// Dispatch one request with the host, path, headers and body given.
async fn dispatch(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer { status, body, text }
}

/// The credential scope a signed SDK request carries. The signature is never checked — this
/// router runs with authentication off.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsJson1_1 call to the Amazon Cognito user pools service, which must succeed.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let authorization = credential_scope("cognito-idp");
    let answer = dispatch(
        router,
        &format!("cognito-idp.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
            ("authorization", &authorization),
        ],
        body.to_string().into_bytes(),
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

/// One API Gateway REST control-plane call, which must answer `expected_status`.
async fn control_plane(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_status: u16,
) -> Value {
    let authorization = credential_scope("apigateway");
    let answer = dispatch(
        router,
        &format!("apigateway.{REGION}.amazonaws.com"),
        method,
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization),
        ],
        body.map(|body| body.to_string().into_bytes())
            .unwrap_or_default(),
    )
    .await;
    assert_eq!(
        answer.status,
        expected_status,
        "{what} must answer HTTP {expected_status}, got {}",
        answer.summary()
    );
    answer.body
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// One base64url segment, decoded: "base64url without padding" (RFC 7515 §2), which is
/// RFC 4648 §5 — the two alphabet substitutions and the restored padding turn it back into the
/// standard alphabet this workspace already decodes.
fn decode_segment(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|e| panic!("a minted token's segment must be base64url ({e})"))
}

/// `value` as one base64url segment of a compact JWS.
fn encode_segment(value: &Value) -> String {
    aws_smithy_types::base64::encode(value.to_string().as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// The JOSE header and the claim set of a compact JWS: its first two segments.
fn parts_of(token: &str) -> (Value, Value) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {} segment(s)",
        segments.len()
    );
    let read = |segment: &str| -> Value {
        serde_json::from_slice(&decode_segment(segment))
            .unwrap_or_else(|e| panic!("a minted token's segment must be JSON ({e})"))
    };
    (read(segments[0]), read(segments[1]))
}

/// The token the pool minted, re-encoded with the JOSE header and claim set given and its
/// signature segment kept — so the only difference from the admitted token is what the row
/// changed.
fn rebuilt(token: &str, header: &Value, claims: &Value) -> String {
    let signature = token
        .split('.')
        .next_back()
        .expect("a compact JWS has a signature segment");
    format!(
        "{}.{}.{signature}",
        encode_segment(header),
        encode_segment(claims)
    )
}

/// One claim of a minted token, as a string.
fn claim(token: &str, name: &str) -> String {
    let (_header, claims) = parts_of(token);
    claims
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a minted token must carry a string {name:?} claim, got {claims}"))
        .to_string()
}

/// What one pool's sign-in produced: the pool's id and ARN, its app client id, and the two
/// tokens of the sign-in.
struct Pool {
    arn: String,
    client_id: String,
    id_token: String,
    access_token: String,
}

/// Mint one pool, one app client and one confirmed user, and sign that user in.
async fn pool_with_signed_in_user(
    router: &MultiServiceRouter,
    pool_name: &str,
    username: &str,
) -> Pool {
    let pool = cognito(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let pool_id = as_str("CreateUserPool", &pool["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &pool["UserPool"], "Arn");

    let client = cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": pool_id,
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
        &json!({ "UserPoolId": pool_id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": pool_id,
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
            "UserPoolId": pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD }
        }),
    )
    .await;
    let result = &signed_in["AuthenticationResult"];
    Pool {
        arn,
        client_id,
        id_token: as_str("AdminInitiateAuth", result, "IdToken"),
        access_token: as_str("AdminInitiateAuth", result, "AccessToken"),
    }
}

/// One `COGNITO_USER_POOLS`-gated `MOCK` method under the api's root, answering the claims the
/// authorizer published. `scopes` is the method's own `authorizationScopes`, empty for a method
/// declaring none. Answers the authorizer id so the row can read it back.
async fn gated_resource(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
    authorizer: Value,
    scopes: &[String],
    template: &str,
) -> String {
    let created = control_plane(
        router,
        &format!("CreateAuthorizer({path_part})"),
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(authorizer),
        201,
    )
    .await;
    let authorizer_id = as_str("CreateAuthorizer", &created, "id");

    let resource = control_plane(
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
    control_plane(
        router,
        &format!("PutMethod({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
        Some(method),
        201,
    )
    .await;
    control_plane(
        router,
        &format!("PutIntegration({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration"),
        Some(json!({ "type": "MOCK" })),
        201,
    )
    .await;
    control_plane(
        router,
        &format!("PutMethodResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    // What an ADMITTED request is answered with: the claims of the token the authorizer
    // accepted, which is how these rows observe that the gate let the request THROUGH and that
    // the integration could read the signed-in user.
    control_plane(
        router,
        &format!("PutIntegrationResponse({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/GET/integration/responses/200"),
        Some(json!({ "responseTemplates": { "application/json": template } })),
        201,
    )
    .await;
    authorizer_id
}

/// What the gate owes one row.
#[derive(Debug, Clone)]
enum Expected {
    /// The gate admitted the request and the `MOCK` integration answered it with the claims of
    /// the token presented: this `sub` and this `token_use`.
    Served { sub: String, token_use: String },
    /// The authorizer does not admit what the request presented, so it authorized nobody.
    Unauthorized,
    /// The token claims a pool the authorizer trusts and does not verify against that pool's
    /// published key set, which is that authorizer denying the call.
    Denied,
}

/// One request to the deployed stage: the gated resource it addresses, the value presented at
/// that resource's authorizer identity source, and what the gate owes it.
struct Row {
    case: String,
    path_part: &'static str,
    /// The value the `Authorization` header carries, or `None` to present nothing at all.
    header: Option<String>,
    /// The value the [`TOKEN_PARAMETER`] query parameter carries.
    query: Option<String>,
    expected: Expected,
}

#[test]
fn a_cognito_user_pools_authorizer_gates_a_deployed_rest_method() {
    std::thread::Builder::new()
        .name("apigateway-cognito-user-pools".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                // 1. The pool whose tokens the api accepts, and a second pool whose tokens are
                //    from a FOREIGN issuer.
                let home = pool_with_signed_in_user(&router, "rest-gate-pool", "gate-user").await;
                let foreign =
                    pool_with_signed_in_user(&router, "rest-gate-other-pool", "other-user").await;

                let sub = claim(&home.id_token, "sub");
                let (header, claims) = parts_of(&home.id_token);
                assert_eq!(
                    claims.get("aud").and_then(Value::as_str),
                    Some(home.client_id.as_str()),
                    "the pool's identity token must name the client it was minted for, got \
                     {claims}"
                );

                // 2. The api, one gated method per authorizer shape, and the stage.
                let api = control_plane(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "cognito-gated-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");

                let header_source = format!("method.request.header.{TOKEN_HEADER}");
                // A user-pool ARN of the shape `Authorizer$providerARNs` lists, naming a pool
                // nothing here holds: the account and region are the home pool's, only the id
                // is one no pool carries.
                let unheld_pool_arn = format!(
                    "{}/{REGION}_000000000",
                    home.arn
                        .rsplit_once('/')
                        .map_or(home.arn.as_str(), |(prefix, _id)| prefix)
                );

                let one_pool = gated_resource(
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
                    CLAIMS_TEMPLATE,
                )
                .await;
                gated_resource(
                    &router,
                    &api_id,
                    &root,
                    "by-both-pools",
                    json!({
                        "name": "both-pools",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [home.arn, foreign.arn],
                        "identitySource": header_source
                    }),
                    &[],
                    CLAIMS_TEMPLATE,
                )
                .await;
                gated_resource(
                    &router,
                    &api_id,
                    &root,
                    "by-unheld-pool",
                    json!({
                        "name": "unheld-pool",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [unheld_pool_arn],
                        "identitySource": header_source
                    }),
                    &[],
                    CLAIMS_TEMPLATE,
                )
                .await;
                gated_resource(
                    &router,
                    &api_id,
                    &root,
                    "by-matching-audience",
                    json!({
                        "name": "matching-audience",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [home.arn],
                        "identitySource": header_source,
                        "identityValidationExpression": format!("^{}$", home.client_id)
                    }),
                    &[],
                    CLAIMS_TEMPLATE,
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
                        "identityValidationExpression": format!("^{}$", foreign.client_id)
                    }),
                    &[],
                    CLAIMS_TEMPLATE,
                )
                .await;
                gated_resource(
                    &router,
                    &api_id,
                    &root,
                    "by-query-source",
                    json!({
                        "name": "query-source",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [home.arn],
                        "identitySource": format!("method.request.querystring.{TOKEN_PARAMETER}")
                    }),
                    &[],
                    CLAIMS_TEMPLATE,
                )
                .await;
                // The claim whose name is no bare identifier, which a Cognito authorizer
                // publishes and only the bracketed member access can read.
                gated_resource(
                    &router,
                    &api_id,
                    &root,
                    "by-namespaced-claim",
                    json!({
                        "name": "namespaced-claim",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [home.arn],
                        "identitySource": header_source
                    }),
                    &[],
                    NAMESPACED_CLAIM_TEMPLATE,
                )
                .await;
                // The METHOD's own `authorizationScopes`: "the method invocation is authorized
                // if any method scopes matches a claimed scope in the access token"
                // (`Method$authorizationScopes`). The scope one row requires is the ACCESS
                // token's own — read off the minted token rather than written here — and the
                // other requires one no Amazon Cognito token claims.
                let claimed_scope = claim(&home.access_token, "scope")
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !claimed_scope.is_empty(),
                    "the pool's access token must carry a scope claim, got {:?}",
                    parts_of(&home.access_token).1
                );
                assert_ne!(
                    claimed_scope, UNCLAIMED_SCOPE,
                    "the unmatched method scope must be one the token does not claim"
                );
                for (path_part, name, scopes) in [
                    ("by-claimed-scope", "claimed-scope", vec![claimed_scope.clone()]),
                    (
                        "by-unclaimed-scope",
                        "unclaimed-scope",
                        vec![UNCLAIMED_SCOPE.to_string()],
                    ),
                ] {
                    gated_resource(
                        &router,
                        &api_id,
                        &root,
                        path_part,
                        json!({
                            "name": name,
                            "type": "COGNITO_USER_POOLS",
                            "providerARNs": [home.arn],
                            "identitySource": header_source
                        }),
                        &scopes,
                        CLAIMS_TEMPLATE,
                    )
                    .await;
                }

                // The authorizer the api stored is what a later read reports, which is what
                // makes the gate's decision one about persisted records.
                let read = control_plane(
                    &router,
                    "GetAuthorizer(one-pool)",
                    "GET",
                    &format!("/restapis/{api_id}/authorizers/{one_pool}"),
                    None,
                    200,
                )
                .await;
                assert_eq!(
                    read.get("type").and_then(Value::as_str),
                    Some("COGNITO_USER_POOLS"),
                    "GetAuthorizer must report the stored type, got {read}"
                );
                assert_eq!(
                    read.get("providerARNs"),
                    Some(&json!([home.arn])),
                    "GetAuthorizer must report the stored providerARNs, got {read}"
                );

                control_plane(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // 3. The rows.
                let served = |token_use: &str| Expected::Served {
                    sub: sub.clone(),
                    token_use: token_use.to_string(),
                };
                let mut unsecured_header = header.clone();
                if let Some(members) = unsecured_header.as_object_mut() {
                    members.insert("alg".to_string(), json!("none"));
                }
                let mut other_sub = claims.clone();
                if let Some(members) = other_sub.as_object_mut() {
                    members.insert("sub".to_string(), json!("00000000-0000-0000-0000-000000000000"));
                }
                let rows: Vec<Row> = vec![
                    Row {
                        case: "the pool's identity token, bare".to_string(),
                        path_part: "by-one-pool",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: served("id"),
                    },
                    Row {
                        case: "the same token with the RFC 6750 bearer scheme".to_string(),
                        path_part: "by-one-pool",
                        header: Some(format!("Bearer {}", home.id_token)),
                        query: None,
                        expected: served("id"),
                    },
                    Row {
                        case: "the pool's ACCESS token of the same sign-in".to_string(),
                        path_part: "by-one-pool",
                        header: Some(home.access_token.clone()),
                        query: None,
                        expected: served("access"),
                    },
                    Row {
                        case: "a second pool's token at an authorizer naming one pool"
                            .to_string(),
                        path_part: "by-one-pool",
                        header: Some(foreign.id_token.clone()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "no token at all".to_string(),
                        path_part: "by-one-pool",
                        header: None,
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "an empty token header".to_string(),
                        path_part: "by-one-pool",
                        header: Some(String::new()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "a value that is not a token at all".to_string(),
                        path_part: "by-one-pool",
                        header: Some("not-a-token".to_string()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pool's token re-encoded with the unsecured alg".to_string(),
                        path_part: "by-one-pool",
                        header: Some(rebuilt(&home.id_token, &unsecured_header, &claims)),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pool's token with its subject rewritten".to_string(),
                        path_part: "by-one-pool",
                        header: Some(rebuilt(&home.id_token, &header, &other_sub)),
                        query: None,
                        // The issuer is the trusted pool, so this value presents itself as a
                        // credential that pool signed: a claim set rewritten after signing does
                        // not verify against the pool's key set, which the recorded
                        // tampered-token leg of
                        // `apigw-cognito-authorizer-invalid-token-negative` answers `403`.
                        expected: Expected::Denied,
                    },
                    Row {
                        case: "the home pool's token at an authorizer naming both pools"
                            .to_string(),
                        path_part: "by-both-pools",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: served("id"),
                    },
                    Row {
                        case: "the second pool's token at an authorizer naming both pools"
                            .to_string(),
                        path_part: "by-both-pools",
                        header: Some(foreign.id_token.clone()),
                        query: None,
                        expected: Expected::Served {
                            sub: claim(&foreign.id_token, "sub"),
                            token_use: "id".to_string(),
                        },
                    },
                    Row {
                        case: "a valid token at an authorizer naming a pool nothing holds"
                            .to_string(),
                        path_part: "by-unheld-pool",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "an audience the validation expression matches".to_string(),
                        path_part: "by-matching-audience",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: served("id"),
                    },
                    Row {
                        case: "an audience the validation expression does not match".to_string(),
                        path_part: "by-other-audience",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the token in the query parameter the identity source names"
                            .to_string(),
                        path_part: "by-query-source",
                        header: None,
                        query: Some(home.id_token.clone()),
                        expected: served("id"),
                    },
                    Row {
                        case: "nothing in the query parameter the identity source names"
                            .to_string(),
                        path_part: "by-query-source",
                        header: None,
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "an access token claiming the method's own scope".to_string(),
                        path_part: "by-claimed-scope",
                        header: Some(home.access_token.clone()),
                        query: None,
                        expected: served("access"),
                    },
                    Row {
                        case: "an identity token at a method declaring a scope, which it \
                               claims none of"
                            .to_string(),
                        path_part: "by-claimed-scope",
                        header: Some(home.id_token.clone()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "an access token at a method requiring a scope it does not claim"
                            .to_string(),
                        path_part: "by-unclaimed-scope",
                        header: Some(home.access_token.clone()),
                        query: None,
                        expected: Expected::Unauthorized,
                    },
                ];

                let invoke_host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                for row in &rows {
                    let path = match &row.query {
                        Some(token) => format!(
                            "/{STAGE}/{}?{TOKEN_PARAMETER}={token}",
                            row.path_part
                        ),
                        None => format!("/{STAGE}/{}", row.path_part),
                    };
                    let mut headers: Vec<(&str, &str)> = Vec::new();
                    if let Some(presented) = &row.header {
                        headers.push((TOKEN_HEADER, presented.as_str()));
                    }
                    let answer =
                        dispatch(&router, &invoke_host, "GET", &path, &headers, Vec::new()).await;
                    // A token question is never answered by the 500 of an authorizer whose
                    // decision could not be obtained: that inversion is what this issue
                    // reported.
                    assert_ne!(
                        answer.message(),
                        CONFIGURATION_ERROR,
                        "case {:?}: the authorizer must decide the call, not report a \
                         configuration error; got {}",
                        row.case,
                        answer.summary()
                    );
                    match &row.expected {
                        Expected::Unauthorized => {
                            assert_eq!(
                                (answer.status, answer.message()),
                                (UNAUTHORIZED, UNAUTHORIZED_MESSAGE.to_string()),
                                "case {:?}: expected {UNAUTHORIZED} {UNAUTHORIZED_MESSAGE:?}, \
                                 got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Denied => {
                            assert_eq!(
                                (answer.status, answer.authorization_message()),
                                (DENIED, DENIED_MESSAGE.to_string()),
                                "case {:?}: expected {DENIED} {DENIED_MESSAGE:?}, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Served { sub, token_use } => {
                            assert_eq!(
                                answer.status,
                                200,
                                "case {:?}: expected the integration's 200, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.body.get("sub").and_then(Value::as_str),
                                Some(sub.as_str()),
                                "case {:?}: expected the token's sub {sub:?} to reach the \
                                 integration, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.body.get("use").and_then(Value::as_str),
                                Some(token_use.as_str()),
                                "case {:?}: expected the token's token_use {token_use:?} to \
                                 reach the integration, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                    }
                }

                // The namespaced claim, read through the bracketed member access: a Cognito
                // authorizer publishes `cognito:username`, whose `:` no bare identifier
                // admits, so this is the only spelling a template can read it by.
                let namespaced = claim(&home.id_token, "cognito:username");
                let answer = dispatch(
                    &router,
                    &invoke_host,
                    "GET",
                    &format!("/{STAGE}/by-namespaced-claim"),
                    &[(TOKEN_HEADER, home.id_token.as_str())],
                    Vec::new(),
                )
                .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case \"a namespaced claim\": expected the integration's 200, got {}",
                    answer.summary()
                );
                assert_eq!(
                    answer.body.get("user").and_then(Value::as_str),
                    Some(namespaced.as_str()),
                    "case \"a namespaced claim\": expected the token's cognito:username \
                     {namespaced:?} to reach the integration, got {}",
                    answer.summary()
                );
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
