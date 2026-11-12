// SPDX-License-Identifier: CC0-1.0

//! An `AWS_IAM`-authorized REST method invoked with the temporary credentials an Amazon
//! Cognito identity pool VENDED, driven end to end through the WHOLE assembled router.
//!
//! Four services and one engine, so this is a router suite: `iam` holds the role and the
//! policies, `cognito-identity` vends a session of that role, `sts` is the registry the
//! session is recorded in, and the gate that reads it is API Gateway's own `execute-api` data
//! plane. This is the whole federation story for API Gateway — a browser or mobile client
//! exchanges a pool identity for credentials and signs the invoke with them.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A request to a deployed method whose `authorizationType` is `AWS_IAM` is authenticated as
//! > the principal its SigV4 credential was minted for, and then authorized:
//! > `execute-api:Invoke` on that method's own ARN must be allowed by the principal's
//! > identity-based policies or by the api's resource policy, with an explicit deny in either
//! > refusing whatever the other says. A principal neither grant allows is refused `403`
//! > carrying the sentence IAM words an authorization failure in — naming that principal, the
//! > `execute-api:Invoke` action and the method ARN — and the integration is not reached.
//!
//! The rows span what that rule generalises over: WHAT the vended session's role is granted
//! (every resource, this method's own ARN, another api's method, another `execute-api` action,
//! nothing at all, an explicit deny), and WHICH credential signs the request (a vended session,
//! the account's own published credential, none at all).
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of the api's own invoke host and of
/// the method ARN a policy names.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to, and the resource path of its one method.
const STAGE: &str = "prod";
const RESOURCE_PATH: &str = "orders";

/// The IAM action an invoke of a deployed method asks for.
const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The clause every IAM authorization refusal carries, which is what a 403 row asserts.
const NOT_AUTHORIZED_CLAUSE: &str = "is not authorized to perform: execute-api:Invoke";

/// The resource field of an STS assumed-role session ARN, which is how a row observes that the
/// refusal names the FEDERATED SESSION rather than the account.
const ASSUMED_ROLE: &str = "assumed-role/";

/// The sentence an `AWS_IAM` method reached with no credential at all is answered with.
const MISSING_TOKEN: &str = "Missing Authentication Token";

/// The body the `MOCK` integration behind the method answers an admitted request with.
const SERVED_BODY: &str = r#"{"served":"orders"}"#;

/// The API Gateway version every awsQuery call to IAM carries.
const IAM_VERSION: &str = "2010-05-08";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The `Message` member an API Gateway authorization refusal carries, or the `message`
    /// member every other gateway response carries, or the whole body.
    fn message(&self) -> String {
        let body = self.json();
        for member in ["Message", "message"] {
            if let Some(message) = body.get(member).and_then(Value::as_str) {
                return message.to_string();
            }
        }
        self.text.clone()
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
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    Answer { status, text }
}

/// The credential scope a signed SDK request carries, for `access_key_id`. The signature is
/// never checked — this router runs with authentication off — but the KEY ID is what the
/// invoke's authorization resolves its principal from.
fn credential_scope(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The access key id this emulator publishes, which resolves to no session and so speaks for
/// the account's own identity.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// One awsJson1_1 call to the Amazon Cognito Identity service, which must succeed.
async fn identity(
    router: &MultiServiceRouter,
    operation: &str,
    signed: bool,
    body: &Value,
) -> Value {
    let target = format!("AWSCognitoIdentityService.{operation}");
    let authorization = credential_scope(ACCOUNT_KEY_ID, "cognito-identity");
    let mut headers: Vec<(&str, &str)> = vec![
        ("content-type", "application/x-amz-json-1.1"),
        ("x-amz-target", &target),
    ];
    if signed {
        headers.push(("authorization", &authorization));
    }
    let answer = dispatch(
        router,
        &format!("cognito-identity.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &headers,
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: cognito-identity {operation} must answer 200, got {}",
        answer.summary()
    );
    answer.json()
}

/// One awsQuery call to IAM, which must succeed.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> String {
    let authorization = credential_scope(ACCOUNT_KEY_ID, "iam");
    let mut form = format!("Action={action}&Version={IAM_VERSION}");
    for (name, value) in parameters {
        form.push_str(&format!("&{name}={value}"));
    }
    let answer = dispatch(
        router,
        "iam.amazonaws.com",
        "POST",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization),
        ],
        form.into_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: iam {action} must answer 200, got {}",
        answer.summary()
    );
    answer.text
}

/// The text of one XML element of an awsQuery answer.
fn xml_text(body: &str, element: &str) -> String {
    let open = format!("<{element}>");
    let close = format!("</{element}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("setup: no {open} in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("setup: no {close} in {body}"))
        + start;
    body[start..end].to_string()
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
    let authorization = credential_scope(ACCOUNT_KEY_ID, "apigateway");
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
    answer.json()
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// The trust policy of the role an identity pool vends sessions of, written the way the Amazon
/// Cognito Developer Guide writes one: the `Federated` Cognito Identity principal, the
/// web-identity assume, and the `:aud` condition restricting the assume to that one pool.
///
/// Compact and carrying none of the characters significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn pool_trust_policy(pool_id: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{pool_id}\"}}}}}}]}}"
    )
}

/// An identity-based policy document stating `effect` about `action` on `resource`.
///
/// Compact and free of the characters an urlencoded body treats specially, for the reason
/// [`pool_trust_policy`] gives.
fn policy_document(effect: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// Vend a session of a fresh role of a fresh identity pool, the way a client reaches it:
/// `iam:CreateRole`, `cognito-identity:SetIdentityPoolRoles`, then the unsigned `GetId` and
/// `GetCredentialsForIdentity` a browser client sends. Answers the session's access key id.
///
/// `policy` is the inline identity-based document attached to that role, or `None` for a role
/// carrying none.
async fn vended_session(
    router: &MultiServiceRouter,
    name: &str,
    policy: Option<String>,
) -> String {
    let pool = identity(
        router,
        "CreateIdentityPool",
        true,
        &json!({ "IdentityPoolName": name, "AllowUnauthenticatedIdentities": true }),
    )
    .await;
    let pool_id = as_str("CreateIdentityPool", &pool, "IdentityPoolId");

    let role_name = format!("{name}-role");
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", &role_name),
            ("AssumeRolePolicyDocument", &pool_trust_policy(&pool_id)),
        ],
    )
    .await;
    let role_arn = xml_text(&created, "Arn");
    if let Some(document) = policy {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", &role_name),
                ("PolicyName", "invoke"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    }
    identity(
        router,
        "SetIdentityPoolRoles",
        true,
        &json!({
            "IdentityPoolId": pool_id,
            "Roles": { "unauthenticated": role_arn }
        }),
    )
    .await;

    let got = identity(router, "GetId", false, &json!({ "IdentityPoolId": pool_id })).await;
    let identity_id = as_str("GetId", &got, "IdentityId");
    let vended = identity(
        router,
        "GetCredentialsForIdentity",
        false,
        &json!({ "IdentityId": identity_id }),
    )
    .await;
    as_str(
        "GetCredentialsForIdentity",
        &vended["Credentials"],
        "AccessKeyId",
    )
}

/// What the gate owes one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The invoke was authorized and the `MOCK` integration answered it.
    Served,
    /// Neither grant allows the invoke: `403`, with the IAM authorization sentence naming the
    /// federated session.
    NotAuthorized,
    /// The method is `AWS_IAM` and the request carried no credential at all.
    MissingToken,
}

/// One invoke of the deployed method: the credential it signs with, and what the gate owes it.
struct Row {
    case: &'static str,
    /// The access key id in the request's credential scope, or `None` to carry no credential.
    access_key_id: Option<String>,
    expected: Expected,
}

#[test]
fn an_iam_authorized_method_authorizes_the_vended_sessions_execute_api_invoke() {
    std::thread::Builder::new()
        .name("apigateway-iam-authorized-invoke".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs; see the
                // sibling `apigateway_private_vpce_invoke` suite.
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                // 1. The api: one `AWS_IAM` method behind a `MOCK` integration, deployed.
                let api = control_plane(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "iam-authorized-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");

                let resource = control_plane(
                    &router,
                    "CreateResource",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{root}"),
                    Some(json!({ "pathPart": RESOURCE_PATH })),
                    201,
                )
                .await;
                let resource_id = as_str("CreateResource", &resource, "id");
                let method = format!("/restapis/{api_id}/resources/{resource_id}/methods/GET");
                control_plane(
                    &router,
                    "PutMethod",
                    "PUT",
                    &method,
                    Some(json!({ "authorizationType": "AWS_IAM" })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutIntegration",
                    "PUT",
                    &format!("{method}/integration"),
                    Some(json!({ "type": "MOCK" })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutMethodResponse",
                    "PUT",
                    &format!("{method}/responses/200"),
                    Some(json!({})),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutIntegrationResponse",
                    "PUT",
                    &format!("{method}/integration/responses/200"),
                    Some(json!({
                        "responseTemplates": { "application/json": SERVED_BODY }
                    })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // The method's own ARN, as a policy statement names it. The ACCOUNT field is
                // written as the wildcard IAM admits, so the document names this method
                // without this suite restating the account contract.
                let this_method = format!(
                    "arn:aws:execute-api:{REGION}:*:{api_id}/{STAGE}/GET/{RESOURCE_PATH}"
                );
                let another_method =
                    format!("arn:aws:execute-api:{REGION}:*:otherapi00/{STAGE}/GET/{RESOURCE_PATH}");

                // 2. One vended session per grant the rule generalises over.
                let every_resource = vended_session(
                    &router,
                    "invoke-every-resource",
                    Some(policy_document("Allow", INVOKE_ACTION, "*")),
                )
                .await;
                let this_method_only = vended_session(
                    &router,
                    "invoke-this-method",
                    Some(policy_document("Allow", INVOKE_ACTION, &this_method)),
                )
                .await;
                let another_method_only = vended_session(
                    &router,
                    "invoke-another-method",
                    Some(policy_document("Allow", INVOKE_ACTION, &another_method)),
                )
                .await;
                let another_action = vended_session(
                    &router,
                    "invalidate-only",
                    Some(policy_document(
                        "Allow",
                        "execute-api:InvalidateCache",
                        "*",
                    )),
                )
                .await;
                let ungranted = vended_session(&router, "no-policy", None).await;
                let denied = vended_session(
                    &router,
                    "denied-invoke",
                    Some(policy_document("Deny", INVOKE_ACTION, "*")),
                )
                .await;

                let rows: Vec<Row> = vec![
                    Row {
                        case: "a session granted execute-api:Invoke on every resource",
                        access_key_id: Some(every_resource),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "a session granted this method's own ARN",
                        access_key_id: Some(this_method_only),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "a session granted another api's method",
                        access_key_id: Some(another_method_only),
                        expected: Expected::NotAuthorized,
                    },
                    Row {
                        case: "a session granted another execute-api action",
                        access_key_id: Some(another_action),
                        expected: Expected::NotAuthorized,
                    },
                    Row {
                        case: "a session whose role holds no policy at all",
                        access_key_id: Some(ungranted),
                        expected: Expected::NotAuthorized,
                    },
                    Row {
                        case: "a session whose role explicitly denies the invoke",
                        access_key_id: Some(denied),
                        expected: Expected::NotAuthorized,
                    },
                    Row {
                        case: "the account's own published credential",
                        access_key_id: Some(ACCOUNT_KEY_ID.to_string()),
                        expected: Expected::Served,
                    },
                    Row {
                        case: "no credential at all",
                        access_key_id: None,
                        expected: Expected::MissingToken,
                    },
                ];

                let invoke_host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                for row in &rows {
                    let authorization = row
                        .access_key_id
                        .as_deref()
                        .map(|key_id| credential_scope(key_id, "execute-api"));
                    let headers: Vec<(&str, &str)> = authorization
                        .as_deref()
                        .map(|value| vec![("authorization", value)])
                        .unwrap_or_default();
                    let answer = dispatch(
                        &router,
                        &invoke_host,
                        "GET",
                        &format!("/{STAGE}/{RESOURCE_PATH}"),
                        &headers,
                        Vec::new(),
                    )
                    .await;
                    match row.expected {
                        Expected::Served => {
                            assert_eq!(
                                answer.status,
                                200,
                                "case {:?}: expected the integration's 200, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.json(),
                                serde_json::from_str::<Value>(SERVED_BODY).unwrap_or(Value::Null),
                                "case {:?}: expected the integration's body, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::NotAuthorized => {
                            assert_eq!(
                                answer.status,
                                403,
                                "case {:?}: expected 403, got {}",
                                row.case,
                                answer.summary()
                            );
                            let message = answer.message();
                            assert!(
                                message.contains(NOT_AUTHORIZED_CLAUSE),
                                "case {:?}: expected the refusal to name the {INVOKE_ACTION} \
                                 action, got {message:?}",
                                row.case
                            );
                            assert!(
                                message.contains(ASSUMED_ROLE),
                                "case {:?}: expected the refusal to name the federated session, \
                                 got {message:?}",
                                row.case
                            );
                            assert!(
                                message.contains(&api_id),
                                "case {:?}: expected the refusal to name the method ARN of api \
                                 {api_id:?}, got {message:?}",
                                row.case
                            );
                        }
                        Expected::MissingToken => {
                            assert_eq!(
                                (answer.status, answer.message()),
                                (403, MISSING_TOKEN.to_string()),
                                "case {:?}: expected 403 {MISSING_TOKEN:?}, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
