// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack's Amazon Cognito user-pool resources are resources of Amazon
//! Cognito, under the identifiers the stack publishes for them.
//!
//! The rule under test: **a resource of a type this emulator provisions is CREATED in the
//! service that owns it, and the identifier the stack publishes for it is the one that service
//! knows the resource by — the id it MINTED where it mints one — so every call a client makes
//! with a `Ref` or an `Fn::GetAtt` of the stack's resource resolves there.** A user pool's
//! `UserPoolIdType` is `^[\w-]+_[0-9a-zA-Z]+$` and an app client's `ClientIdType` is
//! `^[\w+]+$`, neither of which a name CloudFormation computes satisfies, so the value a stack
//! publishes has to be the one the create answered with. Two crates have to agree for the rule
//! to hold (CloudFormation makes the call, Amazon Cognito serves it), so the suite lives here
//! rather than in either of them.
//!
//! The rows differ along what the rule generalises over: WHICH of the four provisioned types
//! the template declares (the pool alone, the pool and an app client, the pool and a federated
//! identity provider, the pool with a resource server whose custom scopes the client is
//! permitted), whether the template NAMES the pool and the client or leaves both names to
//! CloudFormation, whether the client asks for a generated secret, which template language the
//! body is written in (JSON and YAML), which intrinsic each output reads the resource with
//! (`Ref`, `Fn::GetAtt`, `Fn::Sub`) and the Region the stack is deployed in.
//!
//! Every expected value is computed here from what the row declares and from the identity the
//! SAME answer reports for the resource, never from a literal recorded elsewhere: what a row
//! asserts is that the published identifier is the handle the owning service resolves, and that
//! the resource it resolves to carries the configuration the template declared.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// How long one `DescribeStacks` poll waits before the next, and how many are made: a stack's
/// own status transition is owed on its clock, so the suite waits for the STATUS rather than
/// for a span, with a deadline generous enough for a loaded host.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 400;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::
            EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon Cognito user-pools operation over its awsJson1_1 wire, signed for `region`.
async fn cognito(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", authorization("cognito-idp", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Wait for `stack` to report `wanted`, looking again until it does or the deadline passes.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    wanted: &str,
) -> String {
    let mut body = String::new();
    for _ in 0..POLLS {
        let (status, reported) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {reported}"
        );
        body = reported;
        if opt_element(&body, "StackStatus").as_deref() == Some(wanted) {
            return body;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("stack {stack:?} never reached {wanted:?}; last answer was {body}");
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The output of `outputs`, or a panic naming the case and what was there instead.
fn output(outputs: &BTreeMap<String, String>, key: &str, case: &str) -> String {
    outputs
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an output {key:?}, got {outputs:?}"))
}

/// Whether `id` is an identifier of the form Amazon Cognito mints for a user pool of `region`:
/// `UserPoolIdType`, `@pattern ^[\w-]+_[0-9a-zA-Z]+$`, whose first part is the pool's Region.
fn is_minted_pool_id(id: &str, region: &str) -> bool {
    id.strip_prefix(&format!("{region}_"))
        .is_some_and(|minted| !minted.is_empty() && minted.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether `id` is an identifier of the form Amazon Cognito mints for an app client:
/// `ClientIdType`, `@pattern ^[\w+]+$` with `@length(min: 1, max: 128)`.
fn is_minted_client_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
}

/// One row of the table: a template, and what the rule says the resources it declares must
/// then be in Amazon Cognito.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region its pool and client live in.
    region: &'static str,
    /// The stack name the row deploys under.
    stack: &'static str,
    /// The template body, in the language the row writes it in.
    template: &'static str,
    /// The `UserPoolName` the template declares, or `None` for a pool CloudFormation names.
    declared_pool_name: Option<&'static str>,
    /// The password length the template's `Policies.PasswordPolicy` declares, which
    /// `DescribeUserPool` must report back.
    minimum_password_length: i64,
    /// The `ClientName` the template declares for its app client, or `None` for a row that
    /// declares no client.
    declared_client_name: Option<&'static str>,
    /// Whether the client's declaration asks Amazon Cognito to generate a secret.
    generates_a_secret: bool,
    /// The `ExplicitAuthFlows` the client declares, which `DescribeUserPoolClient` must report.
    explicit_auth_flows: &'static [&'static str],
    /// The resource server the template declares, as `(identifier, scope name)`, and the scope
    /// the client is permitted from it (`<identifier>/<scope>`).
    resource_server: Option<(&'static str, &'static str)>,
    /// The identity provider the template declares, as `(provider name, idp identifier)`.
    identity_provider: Option<(&'static str, &'static str)>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named pool alone, in JSON, read with Ref, Fn::GetAtt and Fn::Sub",
        region: "us-east-1",
        stack: "cognito-pool-only",
        template: r#"{
            "Resources": {
                "Pool": {
                    "Type": "AWS::Cognito::UserPool",
                    "Properties": {
                        "UserPoolName": "shop-directory",
                        "Policies": {
                            "PasswordPolicy": {
                                "MinimumLength": 12,
                                "RequireNumbers": true
                            }
                        },
                        "AutoVerifiedAttributes": ["email"]
                    }
                }
            },
            "Outputs": {
                "PoolRef": { "Value": { "Ref": "Pool" } },
                "PoolProviderName": { "Value": { "Fn::GetAtt": ["Pool", "ProviderName"] } },
                "PoolArn": { "Value": { "Fn::Sub": "${Pool.Arn}" } }
            }
        }"#,
        declared_pool_name: Some("shop-directory"),
        minimum_password_length: 12,
        declared_client_name: None,
        generates_a_secret: false,
        explicit_auth_flows: &[],
        resource_server: None,
        identity_provider: None,
    },
    Row {
        case: "a pool and a named app client with a generated secret, in JSON",
        region: "us-west-2",
        stack: "cognito-pool-client",
        template: r#"{
            "Resources": {
                "Pool": {
                    "Type": "AWS::Cognito::UserPool",
                    "Properties": {
                        "UserPoolName": "authflows-pool",
                        "Policies": { "PasswordPolicy": { "MinimumLength": 8 } }
                    }
                },
                "Client": {
                    "Type": "AWS::Cognito::UserPoolClient",
                    "Properties": {
                        "UserPoolId": { "Ref": "Pool" },
                        "ClientName": "authflows-client",
                        "GenerateSecret": true,
                        "ExplicitAuthFlows": [
                            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                            "ALLOW_REFRESH_TOKEN_AUTH"
                        ]
                    }
                }
            },
            "Outputs": {
                "PoolRef": { "Value": { "Ref": "Pool" } },
                "PoolProviderName": { "Value": { "Fn::GetAtt": ["Pool", "ProviderName"] } },
                "PoolArn": { "Value": { "Fn::GetAtt": ["Pool", "Arn"] } },
                "ClientRef": { "Value": { "Ref": "Client" } },
                "ClientSecret": { "Value": { "Fn::GetAtt": ["Client", "ClientSecret"] } }
            }
        }"#,
        declared_pool_name: Some("authflows-pool"),
        minimum_password_length: 8,
        declared_client_name: Some("authflows-client"),
        generates_a_secret: true,
        explicit_auth_flows: &["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        resource_server: None,
        identity_provider: None,
    },
    Row {
        case: "a pool and a client neither of which the YAML template names",
        region: "eu-west-1",
        stack: "cognito-generated-names",
        template: "Resources:\n  \
                   Pool:\n    \
                   Type: AWS::Cognito::UserPool\n    \
                   Properties:\n      \
                   Policies:\n        \
                   PasswordPolicy:\n          \
                   MinimumLength: 16\n  \
                   Client:\n    \
                   Type: AWS::Cognito::UserPoolClient\n    \
                   Properties:\n      \
                   UserPoolId: !Ref Pool\n      \
                   ExplicitAuthFlows:\n        \
                   - ALLOW_USER_SRP_AUTH\n        \
                   - ALLOW_REFRESH_TOKEN_AUTH\n\
                   Outputs:\n  \
                   PoolRef:\n    \
                   Value: !Ref Pool\n  \
                   PoolProviderName:\n    \
                   Value: !GetAtt Pool.ProviderName\n  \
                   PoolArn:\n    \
                   Value: !GetAtt Pool.Arn\n  \
                   ClientRef:\n    \
                   Value: !Ref Client\n",
        declared_pool_name: None,
        minimum_password_length: 16,
        declared_client_name: None,
        generates_a_secret: false,
        explicit_auth_flows: &["ALLOW_USER_SRP_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        resource_server: None,
        identity_provider: None,
    },
    Row {
        case: "a pool, a resource server and a client permitted its custom scope",
        region: "ap-northeast-1",
        stack: "cognito-m2m",
        template: r#"{
            "Resources": {
                "Pool": {
                    "Type": "AWS::Cognito::UserPool",
                    "Properties": {
                        "UserPoolName": "m2m-pool",
                        "Policies": { "PasswordPolicy": { "MinimumLength": 10 } }
                    }
                },
                "Api": {
                    "Type": "AWS::Cognito::UserPoolResourceServer",
                    "Properties": {
                        "UserPoolId": { "Ref": "Pool" },
                        "Identifier": "orders-api",
                        "Name": "Orders API",
                        "Scopes": [
                            { "ScopeName": "read", "ScopeDescription": "read orders" }
                        ]
                    }
                },
                "Client": {
                    "Type": "AWS::Cognito::UserPoolClient",
                    "Properties": {
                        "UserPoolId": { "Ref": "Pool" },
                        "ClientName": "m2m-client",
                        "GenerateSecret": true,
                        "AllowedOAuthFlows": ["client_credentials"],
                        "AllowedOAuthFlowsUserPoolClient": true,
                        "AllowedOAuthScopes": [
                            { "Fn::Sub": "${Api}/read" }
                        ],
                        "ExplicitAuthFlows": ["ALLOW_REFRESH_TOKEN_AUTH"]
                    }
                }
            },
            "Outputs": {
                "PoolRef": { "Value": { "Ref": "Pool" } },
                "PoolProviderName": { "Value": { "Fn::GetAtt": ["Pool", "ProviderName"] } },
                "PoolArn": { "Value": { "Fn::GetAtt": ["Pool", "Arn"] } },
                "ClientRef": { "Value": { "Ref": "Client" } },
                "ClientSecret": { "Value": { "Fn::GetAtt": ["Client", "ClientSecret"] } },
                "ApiRef": { "Value": { "Ref": "Api" } }
            }
        }"#,
        declared_pool_name: Some("m2m-pool"),
        minimum_password_length: 10,
        declared_client_name: Some("m2m-client"),
        generates_a_secret: true,
        explicit_auth_flows: &["ALLOW_REFRESH_TOKEN_AUTH"],
        resource_server: Some(("orders-api", "read")),
        identity_provider: None,
    },
    Row {
        case: "a pool and the federated identity provider of it, in YAML",
        region: "us-east-2",
        stack: "cognito-federation",
        template: "Resources:\n  \
                   Pool:\n    \
                   Type: AWS::Cognito::UserPool\n    \
                   Properties:\n      \
                   UserPoolName: federation-pool\n      \
                   Policies:\n        \
                   PasswordPolicy:\n          \
                   MinimumLength: 6\n  \
                   Provider:\n    \
                   Type: AWS::Cognito::UserPoolIdentityProvider\n    \
                   Properties:\n      \
                   UserPoolId: !Ref Pool\n      \
                   ProviderName: PartnerOIDC\n      \
                   ProviderType: OIDC\n      \
                   IdpIdentifiers:\n        \
                   - partner.example\n      \
                   ProviderDetails:\n        \
                   client_id: partner-client\n        \
                   client_secret: partner-secret\n        \
                   attributes_request_method: GET\n        \
                   oidc_issuer: https://partner.example\n        \
                   authorize_scopes: openid email\n      \
                   AttributeMapping:\n        \
                   email: email\n\
                   Outputs:\n  \
                   PoolRef:\n    \
                   Value: !Ref Pool\n  \
                   PoolProviderName:\n    \
                   Value: !GetAtt Pool.ProviderName\n  \
                   PoolArn:\n    \
                   Value: !GetAtt Pool.Arn\n  \
                   ProviderRef:\n    \
                   Value: !Ref Provider\n",
        declared_pool_name: Some("federation-pool"),
        minimum_password_length: 6,
        declared_client_name: None,
        generates_a_secret: false,
        explicit_auth_flows: &[],
        resource_server: None,
        identity_provider: Some(("PartnerOIDC", "partner.example")),
    },
];

/// Every row: the identifiers the stack publishes for its Amazon Cognito resources are the
/// ones that service knows them by, and the resources they resolve to carry the configuration
/// the template declared.
#[tokio::test]
async fn a_stack_publishes_the_identifiers_amazon_cognito_minted() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let region = row.region;
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", row.template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        let described = wait_for_stack_status(&router, region, row.stack, "CREATE_COMPLETE").await;
        let outputs = outputs_of(&described);

        // The POOL: `Ref` is the id Amazon Cognito minted, and that id resolves there to the
        // pool the template declared.
        let pool = output(&outputs, "PoolRef", case);
        assert!(
            is_minted_pool_id(&pool, region),
            "case {case:?}: expected Ref of the pool to be the id Amazon Cognito minted, got \
             {pool:?}"
        );
        let (status, read) = cognito(
            &router,
            region,
            "DescribeUserPool",
            &json!({ "UserPoolId": &pool }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeUserPool to resolve the published id {pool:?}, got \
             {status} with body {read}"
        );
        let described_pool = read.get("UserPool").cloned().unwrap_or(Value::Null);
        assert_eq!(
            described_pool.get("Id").and_then(Value::as_str),
            Some(pool.as_str()),
            "case {case:?}: expected the pool to report the published id, got {described_pool}"
        );
        match row.declared_pool_name {
            Some(declared) => assert_eq!(
                described_pool.get("Name").and_then(Value::as_str),
                Some(declared),
                "case {case:?}: expected the pool to be created under the declared name \
                 {declared:?}, got {described_pool}"
            ),
            None => {
                let name = described_pool
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    name.starts_with(row.stack) && name.contains("Pool"),
                    "case {case:?}: expected the pool to be created under the name \
                     CloudFormation generated, got {name:?}"
                );
            }
        }
        // The configuration the declaration asked for really reached the pool.
        assert_eq!(
            described_pool
                .get("Policies")
                .and_then(|policies| policies.get("PasswordPolicy"))
                .and_then(|policy| policy.get("MinimumLength"))
                .and_then(Value::as_i64),
            Some(row.minimum_password_length),
            "case {case:?}: expected the declared minimum password length {}, got \
             {described_pool}",
            row.minimum_password_length
        );

        // The pool's published attributes name the pool by the id in force, not by its name.
        let provider_name = output(&outputs, "PoolProviderName", case);
        assert_eq!(
            provider_name,
            format!("cognito-idp.{region}.amazonaws.com/{pool}"),
            "case {case:?}: expected ProviderName to name the pool by its minted id, got \
             {provider_name:?}"
        );
        let arn = output(&outputs, "PoolArn", case);
        assert_eq!(
            arn,
            described_pool
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "case {case:?}: expected the published Arn to be the one Amazon Cognito holds the \
             pool under, got {arn:?}"
        );
        assert!(
            arn.ends_with(&format!(":userpool/{pool}")),
            "case {case:?}: expected the pool's ARN to carry its minted id, got {arn:?}"
        );

        // The APP CLIENT, where the row declares one.
        if row.explicit_auth_flows.is_empty() {
            assert!(
                !outputs.contains_key("ClientRef"),
                "case {case:?}: a row declaring no client publishes none, got {outputs:?}"
            );
        } else {
            let client = output(&outputs, "ClientRef", case);
            assert!(
                is_minted_client_id(&client),
                "case {case:?}: expected Ref of the client to be the id Amazon Cognito minted, \
                 got {client:?}"
            );
            let (status, read) = cognito(
                &router,
                region,
                "DescribeUserPoolClient",
                &json!({ "UserPoolId": &pool, "ClientId": &client }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeUserPoolClient to resolve the published pair \
                 ({pool:?}, {client:?}), got {status} with body {read}"
            );
            let described_client = read.get("UserPoolClient").cloned().unwrap_or(Value::Null);
            assert_eq!(
                described_client.get("ClientId").and_then(Value::as_str),
                Some(client.as_str()),
                "case {case:?}: expected the client to report the published id, got \
                 {described_client}"
            );
            assert_eq!(
                described_client.get("UserPoolId").and_then(Value::as_str),
                Some(pool.as_str()),
                "case {case:?}: expected the client to be in the stack's own pool, got \
                 {described_client}"
            );
            let flows: Vec<String> = described_client
                .get("ExplicitAuthFlows")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|flow| flow.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                flows, row.explicit_auth_flows,
                "case {case:?}: expected the declared auth flows {:?}, got {flows:?}",
                row.explicit_auth_flows
            );
            match row.declared_client_name {
                Some(declared) => assert_eq!(
                    described_client.get("ClientName").and_then(Value::as_str),
                    Some(declared),
                    "case {case:?}: expected the client to be created under the declared name \
                     {declared:?}, got {described_client}"
                ),
                None => {
                    let name = described_client
                        .get("ClientName")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        name.starts_with(row.stack) && name.contains("Client"),
                        "case {case:?}: expected the client to be created under the name \
                         CloudFormation generated, got {name:?}"
                    );
                }
            }
            // The SECRET a declaration asks for is the one that service generated, published
            // through the attribute the type publishes it as.
            let secret = outputs.get("ClientSecret").cloned();
            let held = described_client
                .get("ClientSecret")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if row.generates_a_secret {
                assert!(
                    held.is_some() && secret == held,
                    "case {case:?}: expected the published ClientSecret to be the one Amazon \
                     Cognito generated ({held:?}), got {secret:?}"
                );
            } else {
                assert!(
                    held.is_none(),
                    "case {case:?}: a client that asked for no secret holds none, got {held:?}"
                );
            }
        }

        // The RESOURCE SERVER, where the row declares one: its custom scope is what made the
        // client's own declaration admissible, so it has to exist in the owning service.
        if let Some((identifier, scope)) = row.resource_server {
            let published = output(&outputs, "ApiRef", case);
            assert_eq!(
                published, identifier,
                "case {case:?}: expected Ref of the resource server to be its declared \
                 identifier, got {published:?}"
            );
            let (status, read) = cognito(
                &router,
                region,
                "DescribeResourceServer",
                &json!({ "UserPoolId": &pool, "Identifier": identifier }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeResourceServer to resolve the published \
                 identifier, got {status} with body {read}"
            );
            let server = read.get("ResourceServer").cloned().unwrap_or(Value::Null);
            assert_eq!(
                server
                    .get("Scopes")
                    .and_then(Value::as_array)
                    .and_then(|scopes| scopes.first())
                    .and_then(|first| first.get("ScopeName"))
                    .and_then(Value::as_str),
                Some(scope),
                "case {case:?}: expected the declared scope {scope:?}, got {server}"
            );
            // And the client is permitted that scope in its `<identifier>/<scope>` form.
            let client = output(&outputs, "ClientRef", case);
            let (_, read) = cognito(
                &router,
                region,
                "DescribeUserPoolClient",
                &json!({ "UserPoolId": &pool, "ClientId": &client }),
            )
            .await;
            let scopes: Vec<String> = read
                .get("UserPoolClient")
                .and_then(|client| client.get("AllowedOAuthScopes"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                scopes,
                vec![format!("{identifier}/{scope}")],
                "case {case:?}: expected the client to carry the resource server's own scope, \
                 got {scopes:?}"
            );
        }

        // The IDENTITY PROVIDER, where the row declares one: a stack's provider is resolvable
        // both by the name it publishes and by the identifier its declaration gave it.
        if let Some((provider, idp_identifier)) = row.identity_provider {
            let published = output(&outputs, "ProviderRef", case);
            assert_eq!(
                published, provider,
                "case {case:?}: expected Ref of the provider to be its declared name, got \
                 {published:?}"
            );
            let (status, read) = cognito(
                &router,
                region,
                "GetIdentityProviderByIdentifier",
                &json!({ "UserPoolId": &pool, "IdpIdentifier": idp_identifier }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetIdentityProviderByIdentifier to resolve \
                 {idp_identifier:?} in the stack's pool, got {status} with body {read}"
            );
            let described_provider = read.get("IdentityProvider").cloned().unwrap_or(Value::Null);
            assert_eq!(
                described_provider
                    .get("ProviderName")
                    .and_then(Value::as_str),
                Some(provider),
                "case {case:?}: expected the declared provider name, got {described_provider}"
            );
            assert_eq!(
                described_provider
                    .get("ProviderType")
                    .and_then(Value::as_str),
                Some("OIDC"),
                "case {case:?}: expected the declared provider type, got {described_provider}"
            );
        }

        // Deleting the stack deletes the pool the stack created, so the id it published stops
        // resolving there.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = cognito(
            &router,
            region,
            "DescribeUserPool",
            &json!({ "UserPoolId": &pool }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted pool to stop resolving, got {status} with body \
             {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException for the deleted pool, got {gone}"
        );
    }
}

/// A CHANGED declaration reaches the resources the stack already has through the owning
/// service's own update calls: the pool and its client keep the identifiers that service
/// minted, the pool reports the new password policy, and the custom attribute the new
/// declaration adds to its schema is there.
#[tokio::test]
async fn a_changed_declaration_keeps_the_identifiers_and_applies_the_change() {
    let router = build_router();
    let case = "an update that changes the password policy and adds a custom attribute";
    let region = "us-west-1";
    let stack = "cognito-pool-update";

    let template = |minimum: i64, extra: &str| {
        format!(
            r#"{{
                "Resources": {{
                    "Pool": {{
                        "Type": "AWS::Cognito::UserPool",
                        "Properties": {{
                            "UserPoolName": "update-pool",
                            "Policies": {{ "PasswordPolicy": {{ "MinimumLength": {minimum} }} }},
                            "Schema": [
                                {{ "Name": "tenant", "AttributeDataType": "String" }}{extra}
                            ]
                        }}
                    }},
                    "Client": {{
                        "Type": "AWS::Cognito::UserPoolClient",
                        "Properties": {{
                            "UserPoolId": {{ "Ref": "Pool" }},
                            "ClientName": "update-client",
                            "ExplicitAuthFlows": ["ALLOW_REFRESH_TOKEN_AUTH"]
                        }}
                    }}
                }},
                "Outputs": {{
                    "PoolRef": {{ "Value": {{ "Ref": "Pool" }} }},
                    "ClientRef": {{ "Value": {{ "Ref": "Client" }} }}
                }}
            }}"#
        )
    };

    let created = template(8, "");
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &created)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let described = wait_for_stack_status(&router, region, stack, "CREATE_COMPLETE").await;
    let outputs = outputs_of(&described);
    let pool = output(&outputs, "PoolRef", case);
    let client = output(&outputs, "ClientRef", case);

    let updated = template(
        14,
        ",\n                                { \"Name\": \"tier\", \"AttributeDataType\": \"Number\" }",
    );
    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack), ("TemplateBody", &updated)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    let described = wait_for_stack_status(&router, region, stack, "UPDATE_COMPLETE").await;
    let outputs = outputs_of(&described);

    // Neither identifier moved: the update reached the resources that exist.
    assert_eq!(
        output(&outputs, "PoolRef", case),
        pool,
        "case {case:?}: expected the pool to keep the id Amazon Cognito minted"
    );
    assert_eq!(
        output(&outputs, "ClientRef", case),
        client,
        "case {case:?}: expected the app client to keep the id Amazon Cognito minted"
    );

    let (status, read) = cognito(
        &router,
        region,
        "DescribeUserPool",
        &json!({ "UserPoolId": &pool }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the pool to still resolve, got {status} with body {read}"
    );
    let described_pool = read.get("UserPool").cloned().unwrap_or(Value::Null);
    assert_eq!(
        described_pool
            .get("Policies")
            .and_then(|policies| policies.get("PasswordPolicy"))
            .and_then(|policy| policy.get("MinimumLength"))
            .and_then(Value::as_i64),
        Some(14),
        "case {case:?}: expected the new minimum password length to be in force, got \
         {described_pool}"
    );
    // The attribute the new declaration added is in the pool's schema, under the `custom:`
    // prefix Amazon Cognito stores a custom attribute's name with.
    let attributes: Vec<String> = described_pool
        .get("SchemaAttributes")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    for wanted in ["custom:tenant", "custom:tier"] {
        assert!(
            attributes.iter().any(|held| held == wanted),
            "case {case:?}: expected the schema to carry {wanted:?}, got {attributes:?}"
        );
    }
}

/// An app client whose declaration names no pool is refused by the provisioner itself, naming
/// the property the type requires, rather than sent as a request Amazon Cognito would reject:
/// `UserPoolId` is the other half of an app client's identity.
#[tokio::test]
async fn a_client_declaring_no_user_pool_fails_the_stack() {
    let router = build_router();
    let case = "an app client declaration with no UserPoolId";
    let region = "eu-central-1";
    let stack = "cognito-client-no-pool";
    let template = r#"{
        "Resources": {
            "Client": {
                "Type": "AWS::Cognito::UserPoolClient",
                "Properties": { "ClientName": "poolless" }
            }
        }
    }"#;

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    wait_for_stack_status(&router, region, stack, "ROLLBACK_COMPLETE").await;

    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", stack)],
    )
    .await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("UserPoolId")),
        "case {case:?}: expected the failing resource's event to name the missing property, got \
         {events}"
    );
}
