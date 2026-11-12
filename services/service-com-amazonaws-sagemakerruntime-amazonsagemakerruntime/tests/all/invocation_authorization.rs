// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION of this data plane's three invocation
//! routes.
//!
//! RULE: an endpoint is invoked under the CALLER's own credentials, and AWS decides such a
//! call against that principal's identity-based IAM policies at the front door — before the
//! endpoint the request addresses is resolved — under the action
//! `sagemaker:<OperationName>` on the ARN of the endpoint the request named. The decision is
//! IAM's own: an explicit `Deny` about the request wins over every `Allow`, an `Allow` with
//! no deny permits, and a principal with neither falls to the default deny. A refusal is
//! `AccessDeniedException` with HTTP 403, carrying IAM's own sentence, and it records
//! nothing: the invocation never reached the service.
//!
//! That status and code are what makes a MISSING GRANT distinguishable from an endpoint the
//! account does not host, which is this service's modelled `ValidationError` with HTTP 400 —
//! the two faults an operator has to tell apart, and they are ORDERED: authorization first,
//! endpoint existence second.
//!
//! The rows span what the rule generalises over: which of the three routes is called, the
//! shape of the grant in force (none at all, service-wide, scoped to this endpoint, scoped
//! to every endpoint of the account, scoped to ANOTHER endpoint, another service's actions,
//! a grant plus an explicit deny), whether the endpoint exists at all, and which Region the
//! call was signed for. Beside the tables: the account's own published credential, which
//! holds no IAM policies and is served.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, and the
//! endpoint with the SageMaker control plane's, so the documents this gate reads and the
//! endpoint it decides against are the ones those services wrote.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState as RuntimeState;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{async_inference_config, async_invocations, invocations, response_stream};

/// The Region every call below is signed for unless the case varies it, and so the Region
/// the endpoint ARN a decision is taken against carries.
const REGION: &str = "us-east-1";

/// A second Region, for the case that varies which Region the caller signed for.
const OTHER_REGION: &str = "eu-west-3";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies — the position AWS account root credentials are in. Every fixture is made
/// with it.
const ACCOUNT_KEY: &str = "AKIDEMULATOR000000000";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// The namespace SageMaker publishes this data plane's actions under, which is also this
/// service's SigV4 signing name and the namespace of its resource ARNs.
const ACTION_PREFIX: &str = "sagemaker";

/// The code and status an authorization refusal carries.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The code and status this service refuses an endpoint the account does not host with —
/// the modelled error, which an authorization failure must never be confused with.
const VALIDATION_ERROR: &str = "ValidationError";
const VALIDATION_ERROR_STATUS: u16 = 400;

/// Where the seeded deployment would write an asynchronous result, so the asynchronous
/// route gets past the endpoint's own configuration.
const OUTPUT_PATH: &str = "s3://aws-emulator-sagemaker/async/output";

/// The `InputLocation` the asynchronous route needs as its one input source.
const INPUT_LOCATION: (&str, &str) =
    ("x-amzn-sagemaker-inputlocation", "s3://bucket/in/req.json");

/// One invocation route of this data plane: the operation's own name (which is also the
/// last segment of the IAM action), the path an SDK addresses it at, the headers it needs
/// to reach the authorization step, the body it carries, and the status an AUTHORIZED call
/// to it is answered with — `ModelError` (424) for the two synchronous routes, since no
/// model container runs here, and the accepted-submission 202 for the asynchronous one.
struct Route {
    operation: &'static str,
    path: fn(&str) -> String,
    headers: &'static [(&'static str, &'static str)],
    body: &'static [u8],
    served_status: u16,
}

const ROUTES: &[Route] = &[
    Route {
        operation: "InvokeEndpoint",
        path: invocations,
        headers: &[],
        body: b"authorized-caller",
        served_status: 424,
    },
    Route {
        operation: "InvokeEndpointAsync",
        path: async_invocations,
        headers: &[INPUT_LOCATION],
        body: b"",
        served_status: 202,
    },
    Route {
        operation: "InvokeEndpointWithResponseStream",
        path: response_stream,
        headers: &[],
        body: b"authorized-caller",
        served_status: 424,
    },
];

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// One answer from the emulator.
struct Answer {
    status: u16,
    code: String,
    message: String,
    body: Value,
}

/// The SageMaker control plane that owns the endpoint, this data plane that invokes it, and
/// the IAM and STS services the callers are made with — all on ONE shared engine.
///
/// `crate::endpoint_invocation::Emulator` mounts the two SageMaker services alone and signs
/// every call with one key, which is right for a suite about what this service serves; a
/// suite about WHO is calling needs the services that hold principals and their policies,
/// and a credential scope naming a different key per call, so it assembles the router here.
struct Aws {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id and Region from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// How many invocations this data plane has logged, and how many asynchronous
    /// submissions it has queued — read to prove a refused call served nothing.
    fn recorded(&self) -> (usize, usize) {
        let state = self.engine.state::<RuntimeState>();
        let store = state.lock();
        (store.invocations().count(), store.async_invocations().count())
    }

    /// One data-plane invocation, signed for `region` with `access_key_id` in the credential
    /// scope.
    async fn invoke(
        &self,
        access_key_id: &str,
        region: &str,
        route: &Route,
        endpoint: &str,
    ) -> Answer {
        let path = (route.path)(endpoint);
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://runtime.sagemaker.{region}.amazonaws.com{path}"))
            .header("host", format!("runtime.sagemaker.{region}.amazonaws.com"))
            .header("authorization", authorization(access_key_id, region, ACTION_PREFIX));
        for (name, value) in route.headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::copy_from_slice(route.body))
                    .expect("a well-formed test request"),
            )
            .await;
        let (parts, body) = response.into_parts();
        let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        // restJson1 names the error shape in `x-amzn-errortype`, which this service
        // qualifies with its own locator after a `:`; a client resolves the shape from the
        // text before that separator.
        let named = match parts.headers.get("x-amzn-errortype") {
            Some(value) => value.to_str().unwrap_or_default().to_string(),
            None => parsed
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        Answer {
            status: parts.status.as_u16(),
            code: named
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .split(':')
                .next()
                .unwrap_or_default()
                .to_string(),
            message: parsed
                .get("Message")
                .or_else(|| parsed.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            body: parsed,
        }
    }

    /// One SageMaker control-plane call (`awsJson1_1`), made with the account's own
    /// credential, that must be served.
    async fn control(&self, target: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://api.sagemaker.us-east-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                authorization(ACCOUNT_KEY, REGION, ACTION_PREFIX),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "setup {target}: expected HTTP 200 from the control plane, got {status} with {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS, made with the account's own credential, that must be
    /// served.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                authorization(ACCOUNT_KEY, REGION, signing_name),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "setup {action}: the call must succeed, got {status} with {text}"
        );
        text
    }

    /// Deploy `endpoint` (model -> configuration -> endpoint, with the asynchronous output
    /// path the asynchronous route needs) and wait until the clock owes it `InService`.
    async fn deploy(&self, endpoint: &str) {
        let model = format!("{endpoint}-model");
        let config = format!("{endpoint}-config");
        self.control(
            "CreateModel",
            &json!({
                "ModelName": model,
                "PrimaryContainer": { "Image": "123456789012.dkr.ecr.us-east-1.amazonaws.com/demo:1" },
            }),
        )
        .await;
        let mut config_body = json!({
            "EndpointConfigName": config,
            "ProductionVariants": [{
                "VariantName": "AllTraffic",
                "ModelName": model,
                "InitialInstanceCount": 1,
                "InstanceType": "ml.m5.large",
            }],
        });
        for (member, value) in async_inference_config(OUTPUT_PATH, None)
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            config_body[member] = value;
        }
        self.control("CreateEndpointConfig", &config_body).await;
        self.control(
            "CreateEndpoint",
            &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
        )
        .await;
        // Long enough to pass the control plane's own `Creating -> InService` span.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let described = self
            .control("DescribeEndpoint", &json!({ "EndpointName": endpoint }))
            .await;
        assert_eq!(
            described["EndpointStatus"].as_str(),
            Some("InService"),
            "setup {endpoint}: the endpoint must be InService before it is invoked, got \
             {described}"
        );
    }

    /// Create an IAM role carrying `policies` as its inline policies, assume it through STS,
    /// and answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policies: &[String]) -> Principal {
        let account = engine::emulator_account_id();
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        for (index, policy) in policies.iter().enumerate() {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", format!("inline-{index}")),
                    ("PolicyDocument", policy.clone()),
                ],
            )
            .await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                    ("RoleSessionName", SESSION.to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `access_key_id`, `region`
/// and `signing_name`. Request authentication is off in these tests, so the signature is
/// not checked; the scope is what the caller's identity and the ARN Region are read from.
fn authorization(access_key_id: &str, region: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the
/// round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The ARN of an endpoint, spelled here independently of the crate under test — as
/// SageMaker's Service Authorization Reference spells the `endpoint` resource type.
fn endpoint_arn(region: &str, endpoint: &str) -> String {
    format!(
        "arn:aws:{ACTION_PREFIX}:{region}:{}:endpoint/{endpoint}",
        engine::emulator_account_id()
    )
}

/// The sentence IAM refuses a request with, stated here rather than read off the crate under
/// test: who was refused, which action, on which resource, and which of IAM's rules refused.
fn denial(principal: &str, operation: &str, resource: &str, explicit_deny: bool) -> String {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let reason = if explicit_deny {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// One row of the grant table: the documents the caller's role carries — built from the
/// action the route asks for and the ARN of the endpoint it addresses — and whether IAM
/// allows the call under them.
struct Grant {
    case: &'static str,
    policies: fn(&str, &str) -> Vec<String>,
    allowed: bool,
    explicit_deny: bool,
}

const GRANTS: &[Grant] = &[
    Grant {
        case: "a role carrying no policy at all",
        policies: |_action, _arn| Vec::new(),
        allowed: false,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted every SageMaker action",
        policies: |_action, _arn| vec![statement("Allow", &["sagemaker:*"], "*")],
        allowed: true,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted this route's action on this endpoint",
        policies: |action, arn| vec![statement("Allow", &[action], arn)],
        allowed: true,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted this route's action on every endpoint of the account",
        policies: |action, _arn| {
            vec![statement(
                "Allow",
                &[action],
                &endpoint_arn(REGION, "*"),
            )]
        },
        allowed: true,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted every action on another endpoint",
        policies: |_action, _arn| {
            vec![statement(
                "Allow",
                &["sagemaker:*"],
                &endpoint_arn(REGION, "another-endpoint"),
            )]
        },
        allowed: false,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted another service's actions",
        policies: |_action, _arn| vec![statement("Allow", &["s3:*"], "*")],
        allowed: false,
        explicit_deny: false,
    },
    Grant {
        case: "a role granted every action with this route's action denied",
        policies: |action, _arn| {
            vec![
                statement("Allow", &["sagemaker:*"], "*"),
                statement("Deny", &[action], "*"),
            ]
        },
        allowed: false,
        explicit_deny: true,
    },
];

/// Assert that `answer` is the authorization refusal `principal` is owed for `operation` on
/// `resource`.
fn assert_denied(
    answer: &Answer,
    case: &str,
    principal: &str,
    operation: &str,
    resource: &str,
    explicit_deny: bool,
) {
    assert_eq!(
        answer.status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code, ACCESS_DENIED,
        "case {case:?}: expected the {ACCESS_DENIED} code, got {:?} with {}",
        answer.code, answer.body
    );
    assert_eq!(
        answer.message,
        denial(principal, operation, resource, explicit_deny),
        "case {case:?}: expected IAM's own sentence, got {:?}",
        answer.message
    );
}

/// RULE: every route of this data plane is decided against the calling principal's own
/// identity-based policies, under `sagemaker:<OperationName>` on the ARN of the endpoint the
/// request addressed — so a caller no readable statement grants, or one an explicit deny
/// refuses, is answered `AccessDeniedException` with HTTP 403 and IAM's own sentence and
/// serves nothing, while the same call is served once the grant is in force.
#[tokio::test]
async fn every_route_is_decided_against_the_callers_identity_policies() {
    let aws = Aws::new();
    let endpoint = "authz-ep";
    aws.deploy(endpoint).await;
    let arn = endpoint_arn(REGION, endpoint);

    for (route_index, route) in ROUTES.iter().enumerate() {
        let action = format!("{ACTION_PREFIX}:{}", route.operation);
        for (grant_index, grant) in GRANTS.iter().enumerate() {
            let case = format!("{} / {}", route.operation, grant.case);
            let caller = aws
                .assumed_role(
                    &format!("smrt-authz-{route_index}-{grant_index}"),
                    &(grant.policies)(&action, &arn),
                )
                .await;
            let before = aws.recorded();
            let answer = aws
                .invoke(&caller.access_key_id, REGION, route, endpoint)
                .await;
            if grant.allowed {
                assert_eq!(
                    answer.status, route.served_status,
                    "case {case:?}: an authorized call must be served ({}), got {} with {}",
                    route.served_status, answer.status, answer.body
                );
                continue;
            }
            assert_denied(
                &answer,
                &case,
                &caller.arn,
                route.operation,
                &arn,
                grant.explicit_deny,
            );
            assert_eq!(
                aws.recorded(),
                before,
                "case {case:?}: a refused call must log no invocation and queue no submission"
            );
        }
    }
}

/// RULE: authorization is decided BEFORE the endpoint is resolved, so a caller holding no
/// grant is refused `AccessDeniedException` (403) whether or not the endpoint exists, while a
/// permitted caller naming an endpoint the account does not host gets this service's modelled
/// `ValidationError` (400) — two distinct, ordered outcomes. The account's own published
/// credential holds no IAM policies, so it is the permitted caller.
#[tokio::test]
async fn a_missing_grant_and_a_missing_endpoint_are_different_ordered_refusals() {
    let aws = Aws::new();
    let deployed = "ordered-ep";
    aws.deploy(deployed).await;
    let denied = aws.assumed_role("smrt-ordered-denied", &[]).await;

    for route in ROUTES {
        // (case, the endpoint the call addresses, the status and code a PERMITTED caller
        // gets for it)
        let rows: &[(&str, &str, u16, &str)] = &[
            (
                "an endpoint the account hosts",
                deployed,
                route.served_status,
                "",
            ),
            (
                "an endpoint that was never created",
                "never-created-endpoint",
                VALIDATION_ERROR_STATUS,
                VALIDATION_ERROR,
            ),
        ];
        for (row, endpoint, permitted_status, permitted_code) in rows {
            let case = format!("{} / {row}", route.operation);
            // The account's own credential: no IAM policies, so the endpoint decides.
            let served = aws.invoke(ACCOUNT_KEY, REGION, route, endpoint).await;
            assert_eq!(
                served.status, *permitted_status,
                "case {case:?}: a permitted caller must reach the endpoint verdict \
                 (HTTP {permitted_status}), got {} with {}",
                served.status, served.body
            );
            if !permitted_code.is_empty() {
                assert_eq!(
                    served.code, *permitted_code,
                    "case {case:?}: a permitted caller must get the {permitted_code} code, got \
                     {:?} with {}",
                    served.code, served.body
                );
            }

            // The same call for a caller holding no grant: refused on authorization, and the
            // refusal says nothing about the endpoint.
            let refused = aws.invoke(&denied.access_key_id, REGION, route, endpoint).await;
            assert_denied(
                &refused,
                &case,
                &denied.arn,
                route.operation,
                &endpoint_arn(REGION, endpoint),
                false,
            );
            assert!(
                refused.body.get("ErrorCode").is_none(),
                "case {case:?}: an authorization refusal must carry no endpoint-lookup \
                 discriminator, got {}",
                refused.body
            );
        }
    }
}

/// RULE: the resource a call is decided against is the endpoint ARN in the Region the caller
/// SIGNED for — so a grant scoped to one Region's endpoint ARN allows the call signed for
/// that Region and nothing signed for another, and the refusal names the ARN the caller
/// addressed.
#[tokio::test]
async fn the_resource_carries_the_region_the_call_was_signed_for() {
    let aws = Aws::new();
    let endpoint = "region-ep";
    aws.deploy(endpoint).await;

    for (index, route) in ROUTES.iter().enumerate() {
        let action = format!("{ACTION_PREFIX}:{}", route.operation);
        let caller = aws
            .assumed_role(
                &format!("smrt-region-{index}"),
                &[statement(
                    "Allow",
                    &[action.as_str()],
                    &endpoint_arn(REGION, endpoint),
                )],
            )
            .await;

        let served = aws.invoke(&caller.access_key_id, REGION, route, endpoint).await;
        let case = format!("{} / the Region the grant names", route.operation);
        assert_eq!(
            served.status, route.served_status,
            "case {case:?}: the grant must allow the call signed for that Region \
             (HTTP {}), got {} with {}",
            route.served_status, served.status, served.body
        );

        let refused = aws
            .invoke(&caller.access_key_id, OTHER_REGION, route, endpoint)
            .await;
        assert_denied(
            &refused,
            &format!("{} / another Region", route.operation),
            &caller.arn,
            route.operation,
            &endpoint_arn(OTHER_REGION, endpoint),
            false,
        );
    }
}
