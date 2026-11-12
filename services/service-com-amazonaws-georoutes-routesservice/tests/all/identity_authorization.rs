// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION of this service's operations.
//!
//! RULE: a routing call is made under the CALLER's own credentials, and AWS decides such a
//! call against that principal's identity-based IAM policies before the service reads the
//! request — the principal being the identity the access key id the request presents speaks
//! for, and for an `sts:AssumeRole` credential the session, whose permissions are the
//! role's. The decision is IAM's own: an explicit `Deny` about the request wins over every
//! `Allow`, an `Allow` with none permits, and a principal with neither falls to the default
//! deny. The action is `geo-routes:<OperationName>` — this service's own action namespace,
//! which is also its signing name — and the resource is the routing provider the action is
//! taken on, in the Region the call was signed for. A refusal is `AccessDeniedException`
//! with HTTP 403, carrying IAM's own sentence, and nothing is computed for it.
//!
//! The rows span what the rule generalises over: WHICH of the five operations is asked for,
//! and what the principal carries — no policy at all, a grant over this service's whole
//! namespace, a grant on the provider ARN the call addresses, a grant on another Region's
//! provider, a grant naming one operation only, a grant over another service, and a broad
//! grant with this service explicitly denied. Beside the table: rewriting the SAME inline
//! policy from `Allow` to `Deny` and back takes effect on the next call, and the account's
//! own published credential — which holds no IAM policies — keeps being served.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, so the
//! documents this service reads are the ones those services wrote.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the provider ARN a decision is
/// taken against therefore carries.
const REGION: &str = "us-east-1";

/// The namespace this service publishes its actions under, which is also its SigV4 signing
/// name (`smithy_query(query="service_info")`: `arnNamespace` and `sigv4Name` are both
/// `geo-routes`).
const ACTION_PREFIX: &str = "geo-routes";

/// The code and status an authorization refusal carries, from the model's own
/// `AccessDeniedException` (`errorKind` client, `httpError` 403).
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The status a served call answers with (`CalculateRoutes` and its four siblings all
/// declare HTTP 200).
const SERVED_STATUS: u16 = 200;

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies — the position AWS account root credentials are in. Every fixture is made
/// with it.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// Positions in mapped territory, where this service's road network has coverage, so a
/// permitted call computes a real answer rather than reporting no route.
const START: [f64; 2] = [-123.1, 49.28];
const FINISH: [f64; 2] = [-123.12, 49.26];
const NEARBY: [f64; 2] = [-123.11, 49.27];

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// This service beside the IAM and STS services its callers are made with, all on ONE
/// shared engine — what `core/router/src/lib.rs` assembles for the whole emulator, narrowed
/// to the three services this suite needs.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id and Region from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_georoutes_routesservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
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
        Self { router }
    }

    /// One `restJson1` call to this service, signed for [`REGION`] with `access_key_id` in
    /// the credential scope: its status and its body parsed as JSON.
    async fn call_as(&self, access_key_id: &str, path: &str, body: &Value) -> (u16, Value) {
        let host = format!("{ACTION_PREFIX}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {ACTION_PREFIX}/aws4_request, SignedHeaders=host, \
                     Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
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
        let host = format!("{signing_name}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
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
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
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

/// Create an IAM role carrying `policies` as its inline policies, assume it through STS, and
/// answer the session's access key id with the session ARN.
async fn assumed_role(aws: &Emulator, role_name: &str, policies: &[String]) -> Principal {
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
    aws.query(
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
        aws.query(
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
    let assumed = aws
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

/// Create an IAM user with a long-term access key, and answer that key id with the user's
/// own ARN — the principal a call signed with it speaks for.
async fn iam_user(aws: &Emulator, user_name: &str) -> Principal {
    let created = aws
        .query(
            "iam",
            "2010-05-08",
            "CreateUser",
            &[("UserName", user_name.to_string())],
        )
        .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
    let key = aws
        .query(
            "iam",
            "2010-05-08",
            "CreateAccessKey",
            &[("UserName", user_name.to_string())],
        )
        .await;
    Principal {
        access_key_id: xml_element(&key, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {key}")),
        arn,
    }
}

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The provider ARN a call signed for `region` is decided against, spelled here
/// independently of the crate under test — as the Service Authorization Reference spells
/// this service's one resource type: `provider/<name>` under the `geo-routes` namespace,
/// with no account id, because the provider is the service's own rather than the account's.
fn provider_arn(region: &str) -> String {
    format!("arn:aws:{ACTION_PREFIX}:{region}::provider/default")
}

/// What the gate must answer for one (principal, operation) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The call is authorized, so the operation computes its answer.
    Served,
    /// No readable statement allows the action: IAM's default deny.
    RefusedImplicitly,
    /// A statement about the request states `Deny`.
    RefusedExplicitly,
}

/// One operation of this service: the name its model gives it (which is the name the action
/// this gate asks about is built from), the path a client posts to, and a body that satisfies
/// every modelled constraint — so the authorization decision is the only thing that can
/// refuse the call.
struct Operation {
    name: &'static str,
    path: &'static str,
    body: fn() -> Value,
}

/// Every operation this service serves (`smithy_query(query="digest")`).
const OPERATIONS: &[Operation] = &[
    Operation {
        name: "CalculateRoutes",
        path: "/v2/routes",
        body: || json!({ "Origin": START, "Destination": FINISH }),
    },
    Operation {
        name: "CalculateRouteMatrix",
        path: "/v2/route-matrix",
        body: || {
            json!({
                "Origins": [{ "Position": START }],
                "Destinations": [{ "Position": FINISH }],
                "RoutingBoundary": { "Unbounded": true },
            })
        },
    },
    Operation {
        name: "CalculateIsolines",
        path: "/v2/isolines",
        body: || json!({ "Origin": START, "Thresholds": { "Time": [600] } }),
    },
    Operation {
        name: "OptimizeWaypoints",
        path: "/v2/optimize-waypoints",
        body: || json!({ "Origin": START, "Waypoints": [{ "Position": NEARBY }] }),
    },
    Operation {
        name: "SnapToRoads",
        path: "/v2/snap-to-roads",
        body: || json!({ "TracePoints": [{ "Position": START }, { "Position": NEARBY }] }),
    },
];

/// One row of the grant table: the documents in force on the caller, and what each operation
/// must answer under them.
struct Row {
    case: &'static str,
    /// The documents the principal carries, built from the provider ARN the call addresses.
    policies: fn(&str) -> Vec<String>,
    /// What the gate must answer for the operation named.
    expect: fn(&str) -> Expect,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role carrying no policy at all",
        policies: |_provider| Vec::new(),
        expect: |_operation| Expect::RefusedImplicitly,
    },
    Row {
        case: "a role granted every action of this service",
        policies: |_provider| vec![statement("Allow", &["geo-routes:*"], "*")],
        expect: |_operation| Expect::Served,
    },
    Row {
        case: "a role granted every action on the provider the call addresses",
        policies: |provider| vec![statement("Allow", &["geo-routes:*"], provider)],
        expect: |_operation| Expect::Served,
    },
    Row {
        case: "a role granted every action on another Region's provider",
        policies: |_provider| {
            vec![statement(
                "Allow",
                &["geo-routes:*"],
                &provider_arn("eu-central-1"),
            )]
        },
        expect: |_operation| Expect::RefusedImplicitly,
    },
    Row {
        case: "a role granted one operation only",
        policies: |_provider| vec![statement("Allow", &["geo-routes:CalculateRoutes"], "*")],
        expect: |operation| {
            if operation == "CalculateRoutes" {
                Expect::Served
            } else {
                Expect::RefusedImplicitly
            }
        },
    },
    Row {
        case: "a role granted another service's actions",
        policies: |_provider| vec![statement("Allow", &["geo-places:*"], "*")],
        expect: |_operation| Expect::RefusedImplicitly,
    },
    Row {
        case: "a role granted everything with this service explicitly denied",
        policies: |_provider| {
            vec![
                statement("Allow", &["*"], "*"),
                statement("Deny", &["geo-routes:*"], "*"),
            ]
        },
        expect: |_operation| Expect::RefusedExplicitly,
    },
    Row {
        case: "a role granted every action with one operation explicitly denied",
        policies: |_provider| {
            vec![
                statement("Allow", &["geo-routes:*"], "*"),
                statement("Deny", &["geo-routes:SnapToRoads"], "*"),
            ]
        },
        expect: |operation| {
            if operation == "SnapToRoads" {
                Expect::RefusedExplicitly
            } else {
                Expect::Served
            }
        },
    },
];

/// The sentence IAM refuses a request with, stated here rather than read off the crate under
/// test: who was refused, which action, on which resource, and which of IAM's rules refused.
fn denial(principal: &str, operation: &str, resource: &str, explicit_deny: bool) -> String {
    let reason = if explicit_deny {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {ACTION_PREFIX}:{operation} action")
    };
    format!(
        "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{operation} on \
         resource: {resource} {reason}"
    )
}

/// Assert one call's answer against what the row expects of it.
async fn expect_answer(
    aws: &Emulator,
    case: &str,
    caller: &Principal,
    operation: &Operation,
    expected: Expect,
) {
    let case = format!("{case}, {}", operation.name);
    let (status, parsed) = aws
        .call_as(&caller.access_key_id, operation.path, &(operation.body)())
        .await;
    let explicit_deny = match expected {
        Expect::Served => {
            assert_eq!(
                status, SERVED_STATUS,
                "case {case:?}: an authorized call must be served, got {status} with {parsed}"
            );
            return;
        }
        Expect::RefusedImplicitly => false,
        Expect::RefusedExplicitly => true,
    };
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} with {parsed}"
    );
    let code = parsed["__type"]
        .as_str()
        .map(|named| named.split('#').next_back().unwrap_or(named).to_string());
    assert_eq!(
        code,
        Some(ACCESS_DENIED.to_string()),
        "case {case:?}: expected the {ACCESS_DENIED} code, got {parsed}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some(
            denial(
                &caller.arn,
                operation.name,
                &provider_arn(REGION),
                explicit_deny,
            )
            .as_str()
        ),
        "case {case:?}: expected IAM's own sentence, got {parsed}"
    );
}

/// RULE: every operation of this service is decided against the calling principal's own
/// identity-based policies before anything is computed, under the action
/// `geo-routes:<OperationName>` and the provider the call addresses — so a caller no readable
/// statement grants, or one an explicit deny refuses, is answered `AccessDeniedException`
/// with HTTP 403 and IAM's own sentence, while the same call is served once the grant is in
/// force.
#[tokio::test]
async fn every_operation_is_decided_against_the_callers_identity_policies() {
    let aws = Emulator::new();
    let provider = provider_arn(REGION);

    for (index, row) in ROWS.iter().enumerate() {
        let caller = assumed_role(
            &aws,
            &format!("geo-routes-row-{index}"),
            &(row.policies)(&provider),
        )
        .await;
        for operation in OPERATIONS {
            expect_answer(
                &aws,
                row.case,
                &caller,
                operation,
                (row.expect)(operation.name),
            )
            .await;
        }
    }
}

/// RULE: the documents the decision reads are the ones the IAM store holds AT THE INSTANT OF
/// THE CALL, so rewriting one inline policy from `Allow` to `Deny` under the same name
/// refuses the next identical call — with the explicit-deny clause — and rewriting it back
/// serves it again. The principal here is an IAM USER signing with its own long-term access
/// key, so the refusal blames the user's own ARN.
#[tokio::test]
async fn rewriting_an_inline_policy_decides_the_next_call() {
    let aws = Emulator::new();
    let caller = iam_user(&aws, "geo-routes-operator").await;
    let operation = &OPERATIONS[0];
    let policy_name = "route-access";

    // (case, the effect the single statement states, what the next call must answer)
    let steps: &[(&str, &str, Expect)] = &[
        ("the grant in force", "Allow", Expect::Served),
        (
            "the same policy rewritten to Deny",
            "Deny",
            Expect::RefusedExplicitly,
        ),
        ("the grant restored", "Allow", Expect::Served),
    ];
    for (case, effect, expected) in steps {
        aws.query(
            "iam",
            "2010-05-08",
            "PutUserPolicy",
            &[
                ("UserName", "geo-routes-operator".to_string()),
                ("PolicyName", policy_name.to_string()),
                (
                    "PolicyDocument",
                    statement(effect, &["geo-routes:CalculateRoutes"], "*"),
                ),
            ],
        )
        .await;
        expect_answer(&aws, case, &caller, operation, *expected).await;
    }
}

/// RULE: a request whose access key id neither STS nor IAM holds a record for speaks for the
/// account's own published credential, which is not a principal the IAM store holds policies
/// for — the position AWS account root credentials are in — so every operation keeps being
/// served for it.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let aws = Emulator::new();
    let caller = Principal {
        access_key_id: ACCOUNT_KEY.to_string(),
        arn: String::new(),
    };
    for operation in OPERATIONS {
        expect_answer(
            &aws,
            "the account's own published credential",
            &caller,
            operation,
            Expect::Served,
        )
        .await;
    }
}
