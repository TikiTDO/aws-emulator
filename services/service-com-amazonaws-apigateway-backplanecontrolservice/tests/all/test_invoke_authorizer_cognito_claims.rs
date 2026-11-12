// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeAuthorizer` against a `COGNITO_USER_POOLS` authorizer: the `claims` member, and
//! the status a call that earned no decision reports.
//!
//! The rule the rows assert, one level above the recorded case:
//!
//! > `claims` is "the open identity claims, with any supported custom attributes, returned
//! > from the Cognito Your User Pool configured for the API"
//! > (`TestInvokeAuthorizerResponse$claims`), so a simulation serves that member exactly when
//! > a `COGNITO_USER_POOLS` authorizer ACCEPTED the identity token the simulated request
//! > presented — carrying that token's own claim set, each claim as the text a
//! > `$context.authorizer.claims.<name>` reference stands for — and omits it for every other
//! > answer, a token the authorizer did not accept included. Such an accepted token is an
//! > authorizer that succeeded, so `clientStatus` is 0; a token it did not accept earned no
//! > decision at all, so `clientStatus` is the status of the gateway response the client
//! > would have been answered through.
//!
//! The rows differ along what the rule generalises over: WHAT the simulated request presented
//! at the authorizer's identity source — the pool's own minted identity token, a token another
//! pool minted, that token with its signature segment rewritten, a value that is not a signed
//! token at all, and nothing.
//!
//! The Lambda-backed types of the same operation are `tests/all/test_invoke_authorizer.rs`,
//! whose every row asserts the member ABSENT: between the two suites, the member is served for
//! one authorizer type and for no other.
//!
//! Only a cross-service suite can pin this: the api and the authorizer record are API
//! Gateway's, while the pool that minted every token and publishes the key set it is verified
//! against is Amazon Cognito's — both on ONE engine, as `core/router/src/lib.rs` mounts them.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every call below is signed for, and so the Region of the pool's issuer.
const REGION: &str = "us-west-2";

/// The header the authorizer declares as its identity source.
const IDENTITY_HEADER: &str = "Authorization";

/// The permanent password the fixture user signs in with — written for this suite and used
/// nowhere else. It satisfies a pool's default password policy.
const PASSWORD: &str = "Str0ng-Passw0rd!";

/// The status a simulation reports for an authorizer that SUCCEEDED, whatever it decided:
/// "value is 0 if the authorizer succeeded" (`TestInvokeAuthorizerResponse$clientStatus`).
const SUCCEEDED: i64 = 0;

/// The status of the gateway response a request this authorizer does not admit would have met:
/// it authorized nobody, which is the `401` of the `UNAUTHORIZED` response type.
const UNAUTHORIZED: i64 = 401;

/// The status of the gateway response a token claiming a pool the authorizer TRUSTS that does
/// not verify against that pool's published key set would have met: the authorizer denying the
/// call, answered through the `ACCESS_DENIED` response type's `403`.
const DENIED: i64 = 403;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The two services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: a call carries a credential scope so the
    // router can name the target service, and the signature itself is never checked here.
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    signing_name: &str,
    content_type: &str,
    body: Bytes,
) -> Answered {
    let builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", content_type)
        .header("authorization", authorization(signing_name));
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsJson1_1 call to the user pools service, under the `X-Amz-Target` its protocol names
/// the operation by. It must succeed: every one of them is setup.
async fn cognito_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let host = format!("cognito-idp.{REGION}.amazonaws.com");
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", &target)
        .header("authorization", authorization("cognito-idp"));
    let response = router
        .dispatch(
            builder
                .body(Bytes::from(body.to_string()))
                .expect("a well-formed request"),
        )
        .await;
    let (parts, bytes) = response.into_parts();
    let answered = Answered {
        status: parts.status.as_u16(),
        body: bytes,
    };
    assert_eq!(
        answered.status,
        200,
        "setup: cognito-idp {operation} must answer HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One API Gateway control-plane call.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        "apigateway",
        "application/json",
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answered = apigw(router, method, path, body).await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what:?}: expected a non-empty {member:?}, got {body}"))
}

/// What one pool's sign-in produced: the pool's ARN, and the identity token of the sign-in.
struct Pool {
    arn: String,
    id_token: String,
}

/// Mint one pool, one app client and one confirmed user, and sign that user in.
async fn pool_with_signed_in_user(
    router: &MultiServiceRouter,
    pool_name: &str,
    username: &str,
) -> Pool {
    let created = cognito_call(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let id = as_str("CreateUserPool", &created["UserPool"], "Id");
    let arn = as_str("CreateUserPool", &created["UserPool"], "Arn");
    let client = cognito_call(
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
    cognito_call(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": id, "Username": username }),
    )
    .await;
    cognito_call(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": id,
            "Username": username,
            "Password": PASSWORD,
            "Permanent": true
        }),
    )
    .await;
    let signed_in = cognito_call(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": PASSWORD }
        }),
    )
    .await;
    Pool {
        arn,
        id_token: as_str(
            "AdminInitiateAuth",
            &signed_in["AuthenticationResult"],
            "IdToken",
        ),
    }
}

/// The claim set of a compact JWS: its second segment, decoded.
fn claims_of(token: &str) -> Value {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("a minted token is a compact JWS of three segments: {token:?}"));
    let bytes = ::base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .unwrap_or_else(|e| panic!("a minted token's claim segment must be base64url ({e})"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("a minted token's claim segment must be JSON ({e})"))
}

/// The claim set as the caller reads it back: one entry per claim, a string claim as its own
/// text and every other value as the compact JSON it is — which is the text a
/// `$context.authorizer.claims.<name>` reference stands for.
fn as_published(claims: &Value) -> BTreeMap<String, String> {
    claims
        .as_object()
        .unwrap_or_else(|| panic!("a token's claim set is a JSON object, got {claims}"))
        .iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (name.clone(), text)
        })
        .collect()
}

/// The `claims` member of a report, as a map; `None` where the report carries no such member.
fn reported_claims(reported: &Value) -> Option<BTreeMap<String, String>> {
    let member = reported.get("claims")?;
    Some(
        member
            .as_object()
            .unwrap_or_else(|| panic!("the claims member is a map, got {member}"))
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
    )
}

/// Simulate the authorizer with the value `presented` at its identity source, or with no
/// header at all.
async fn simulate(
    router: &MultiServiceRouter,
    api: &str,
    authorizer: &str,
    presented: Option<&str>,
) -> Answered {
    let mut body = serde_json::Map::new();
    body.insert("pathWithQueryString".to_string(), json!("/orders"));
    if let Some(value) = presented {
        body.insert(
            "headers".to_string(),
            json!({ IDENTITY_HEADER: value }),
        );
    }
    apigw(
        router,
        "POST",
        &format!("/restapis/{api}/authorizers/{authorizer}"),
        Some(Value::Object(body)),
    )
    .await
}

/// What a simulation owes one row: the status it reports, and whether the token's claim set is
/// what the `claims` member carries.
struct Row {
    case: &'static str,
    /// The value the identity header carries, or `None` for a request carrying no header.
    presented: Option<String>,
    client_status: i64,
    /// `true` where the report carries the presented token's claim set, `false` where it
    /// carries no `claims` member at all.
    claims_served: bool,
}

#[tokio::test]
async fn a_simulated_user_pool_authorizer_serves_the_claims_of_a_token_it_accepted() {
    let router = emulator();

    // The pool whose tokens the authorizer accepts, and a second pool whose tokens come from
    // an issuer it names none of.
    let home = pool_with_signed_in_user(&router, "simulated-claims-pool", "claims-user").await;
    let foreign =
        pool_with_signed_in_user(&router, "simulated-claims-other-pool", "other-user").await;
    let minted = claims_of(&home.id_token);
    let published = as_published(&minted);

    let api = accepted(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "simulated-cognito-authorizer-api" })),
        201,
    )
    .await;
    let api = as_str("CreateRestApi", &api, "id");
    let created = accepted(
        &router,
        "CreateAuthorizer",
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(json!({
            "name": "one-pool",
            "type": "COGNITO_USER_POOLS",
            "providerARNs": [home.arn],
            "identitySource": format!("method.request.header.{IDENTITY_HEADER}")
        })),
        201,
    )
    .await;
    let authorizer = as_str("CreateAuthorizer", &created, "id");
    // The record the simulation executes is read back through the control plane's own reader,
    // so it is the one a client can see.
    let read = accepted(
        &router,
        "GetAuthorizer",
        "GET",
        &format!("/restapis/{api}/authorizers/{authorizer}"),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str("GetAuthorizer", &read, "type"),
        "COGNITO_USER_POOLS",
        "setup: expected the stored authorizer to carry its own type, got {read}"
    );

    // The minted token with its SIGNATURE segment rewritten and everything else left as the
    // pool signed it: a value claiming a pool the authorizer trusts that pool never signed.
    let tampered = {
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

    let rows = vec![
        Row {
            case: "the pool's own identity token",
            presented: Some(home.id_token.clone()),
            client_status: SUCCEEDED,
            claims_served: true,
        },
        Row {
            case: "a token another pool minted",
            presented: Some(foreign.id_token.clone()),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
        Row {
            case: "a value that is not a signed token",
            presented: Some("not-a-token".to_string()),
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
        Row {
            case: "the pool's token with its signature rewritten",
            presented: Some(tampered),
            client_status: DENIED,
            claims_served: false,
        },
        Row {
            case: "nothing at the identity source",
            presented: None,
            client_status: UNAUTHORIZED,
            claims_served: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let answered = simulate(&router, &api, &authorizer, row.presented.as_deref()).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected the simulation to be answered HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered.json();
        assert_eq!(
            reported.get("clientStatus").and_then(Value::as_i64),
            Some(row.client_status),
            "case {case:?}: expected clientStatus {}, got {reported}",
            row.client_status
        );
        let claims = reported_claims(&reported);
        if row.claims_served {
            assert_eq!(
                claims.as_ref(),
                Some(&published),
                "case {case:?}: expected the accepted token's claim set, got {reported}"
            );
        } else {
            assert!(
                claims.is_none(),
                "case {case:?}: expected no claims member at all, got {reported}"
            );
        }
        let log = reported
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            log.contains("Execution log for request"),
            "case {case:?}: expected an execution log, got {log:?}"
        );
        // A caller's credential never travels into a log: the header NAMES are reported and
        // the value the row presented is not.
        if let Some(value) = row.presented.as_deref() {
            assert!(
                !log.contains(value),
                "case {case:?}: expected the presented value to stay out of the execution log, \
                 got {log:?}"
            );
        }
        assert!(
            reported.get("latency").and_then(Value::as_i64).is_some(),
            "case {case:?}: expected a measured latency, got {reported}"
        );
    }
}
