// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's portal, portal-product and custom-domain-name
//! operations, over the restJson1 wire.
//!
//! The rule under test: every operation whose model declares `AccessDeniedException`
//! (HTTP 403) is served only to a caller whose identity-based IAM policies permit THAT
//! operation's own action (`apigateway:<OperationName>`) on the resource the request
//! addresses — the portal's, the portal product's, the page's or the custom domain name's
//! own ARN, and the collection's wildcard ARN for a create or a list, which address the
//! collection rather than one member of it. Every other caller is refused with
//! `AccessDeniedException` BEFORE the operation reads or writes anything, so a refused
//! caller learns nothing about what the account holds and a refused mutation changes
//! nothing.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateRole` + `PutRolePolicy` + `AssumeRole`,
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey`), so the documents this service reads
//! are the ones those services wrote.
//!
//! The tables span what the rule generalises over: the OPERATION asked for (a create, a
//! read, a list and a delete, over three different resource families, each with its own
//! action and its own resource), the kind of principal (the account's own published
//! credential, an assumed-role session, an IAM user) and what its policies say (nothing, a
//! wildcard allow, an allow for another action, an allow on another resource family, and a
//! wildcard allow beside an explicit deny). The recorded case that filed the issue — a
//! `CreatePortal` under an explicit deny on `apigateway:CreatePortal` — is one row of the
//! first table.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::service::authorization::ANY_MEMBER;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    domain_name_arn, portal_arn, portal_product_arn,
};

use crate::query_form::form_encode;

/// The region every call below is signed for, so the ARNs the decisions are made about are
/// the ones this region's resources carry.
pub(crate) const REGION: &str = "us-west-2";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY: &str = "AKIDTESTAPIGWV2ROOT";

/// The error code every refusal of this gate carries, from the model's own 403 error.
pub(crate) const ACCESS_DENIED: &str = "AccessDeniedException";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

impl Reply {
    /// Whether the gate let this call through — whatever the operation then answered.
    pub(crate) fn authorized(&self) -> bool {
        self.error_code != ACCESS_DENIED
    }

    /// The `message` of a refusal.
    pub(crate) fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
pub(crate) struct Principal {
    pub(crate) access_key_id: String,
    pub(crate) arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    pub(crate) async fn call(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("a request body serializes"))
            }
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://apigateway.{REGION}.amazonaws.com{path}"))
            .header("host", format!("apigateway.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20261021/{REGION}/\
                     apigateway/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
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
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20261021/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the session's access key id with the session ARN.
    pub(crate) async fn assumed_role(
        &self,
        role_name: &str,
        policy: Option<&str>,
    ) -> Principal {
        let account = ::engine::emulator_account_id();
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}"
        );
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
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
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
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{account}:role/{role_name}"),
                    ),
                    ("RoleSessionName", "probe".to_string()),
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

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint
    /// a long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A policy document allowing `action` on `resource`.
pub(crate) fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied`.
pub(crate) fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"apigateway:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The body `CreatePortal` needs: both of its required unions naming their one alternative,
/// and the portal's content — `PortalContent$DisplayName` and `PortalContent$Theme` are both
/// required, and the theme's `CustomColors` requires every one of its members
/// ([`crate::portal_lifecycle::portal_content`] builds the same value the portal suites use).
fn create_portal_body(display_name: &str) -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": crate::portal_lifecycle::portal_content(display_name)
    })
}

/// One operation this gate is asked about: how it is called, and the action and resource it
/// is judged under.
struct Operation {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    action: String,
    resource: String,
}

/// The operations the tables below span: a create and a list of the portal collection, a
/// read of one portal, a create and a list of the portal-product collection, and a create
/// of a custom domain name — three resource families, four kinds of call.
fn operations() -> Vec<Operation> {
    vec![
        Operation {
            case: "CreatePortal, which addresses the portal collection",
            method: "POST",
            path: "/v2/portals".to_string(),
            body: Some(create_portal_body("probe-portal")),
            action: "apigateway:CreatePortal".to_string(),
            resource: portal_arn(REGION, ANY_MEMBER),
        },
        Operation {
            case: "ListPortals, which addresses the portal collection",
            method: "GET",
            path: "/v2/portals".to_string(),
            body: None,
            action: "apigateway:ListPortals".to_string(),
            resource: portal_arn(REGION, ANY_MEMBER),
        },
        Operation {
            case: "GetPortal, which addresses one portal",
            method: "GET",
            path: "/v2/portals/nosuchportal".to_string(),
            body: None,
            action: "apigateway:GetPortal".to_string(),
            resource: portal_arn(REGION, "nosuchportal"),
        },
        Operation {
            case: "CreatePortalProduct, which addresses the portal-product collection",
            method: "POST",
            path: "/v2/portalproducts".to_string(),
            body: Some(json!({ "displayName": "probe-product" })),
            action: "apigateway:CreatePortalProduct".to_string(),
            resource: portal_product_arn(REGION, ANY_MEMBER),
        },
        Operation {
            case: "ListPortalProducts, which addresses the portal-product collection",
            method: "GET",
            path: "/v2/portalproducts".to_string(),
            body: None,
            action: "apigateway:ListPortalProducts".to_string(),
            resource: portal_product_arn(REGION, ANY_MEMBER),
        },
        Operation {
            case: "CreateDomainName, whose resource is the name the request carries",
            method: "POST",
            path: "/v2/domainnames".to_string(),
            body: Some(json!({
                "domainName": "probe.example.com",
                "domainNameConfigurations": [{ "endpointType": "REGIONAL" }]
            })),
            action: "apigateway:CreateDomainName".to_string(),
            resource: domain_name_arn("probe.example.com", REGION),
        },
    ]
}

/// Whether a policy row leaves one operation authorized, judged from the operation's own
/// action and resource.
type Authorizes = Box<dyn Fn(&Operation) -> bool>;

/// One policy row: the case, the role's inline policy, and which operations it leaves
/// authorized.
type PolicyRow = (&'static str, Option<String>, Authorizes);

/// RULE: an operation is served only to a principal whose identity-based policies permit
/// that operation's own action on the resource the request addresses.
#[tokio::test]
async fn each_operation_is_judged_on_its_own_action_and_its_own_resource() {
    // The rows span what the rule turns on: no document at all, a wildcard allow, an allow
    // for one action, an allow confined to one resource family, and a wildcard allow beside
    // an explicit deny.
    let portals_only = allow("apigateway:*", &portal_arn(REGION, ANY_MEMBER));
    let rows: Vec<PolicyRow> = vec![
        (
            "no identity-based policy at all",
            None,
            Box::new(|_: &Operation| false),
        ),
        (
            "a wildcard allow of every action of this service",
            Some(allow("apigateway:*", "*")),
            Box::new(|_: &Operation| true),
        ),
        (
            "an allow of one action only",
            Some(allow("apigateway:GetPortal", "*")),
            Box::new(|operation: &Operation| operation.action == "apigateway:GetPortal"),
        ),
        (
            "an allow confined to the portal collection",
            Some(portals_only),
            Box::new(|operation: &Operation| {
                // IAM's `*` is an any-run wildcard, so a document written about the
                // collection covers every portal under it and nothing of another family.
                let under_portals = portal_arn(REGION, ANY_MEMBER);
                operation
                    .resource
                    .starts_with(under_portals.trim_end_matches(ANY_MEMBER))
            }),
        ),
        (
            "a wildcard allow beside an explicit deny of CreatePortal",
            Some(allow_all_but("apigateway:CreatePortal")),
            Box::new(|operation: &Operation| operation.action != "apigateway:CreatePortal"),
        ),
    ];

    for (index, (case, policy, expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let principal = emulator
            .assumed_role(&format!("portal-author-{index}"), policy.as_deref())
            .await;
        for operation in operations() {
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    operation.method,
                    &operation.path,
                    operation.body.clone(),
                )
                .await;
            let authorized = expected(&operation);
            assert_eq!(
                reply.authorized(),
                authorized,
                "case ({case:?}, {:?}): expected authorized={authorized}, got {} {:?} {}",
                operation.case,
                reply.status,
                reply.error_code,
                reply.body
            );
            if !authorized {
                assert_eq!(
                    reply.status, 403,
                    "case ({case:?}, {:?}): expected HTTP 403, got {} {}",
                    operation.case, reply.status, reply.body
                );
            }
        }
    }
}

/// RULE: the refusal states IAM's own sentence — the principal the credential speaks for,
/// the action the operation names, and the ARN the request addressed.
#[tokio::test]
async fn the_refusal_names_the_principal_the_action_and_the_resource() {
    let emulator = Emulator::new();
    let denied = emulator
        .assumed_role(
            "portal-denied",
            Some(&allow_all_but("apigateway:CreatePortal")),
        )
        .await;
    let reply = emulator
        .call(
            &denied.access_key_id,
            "POST",
            "/v2/portals",
            Some(create_portal_body("denied-portal")),
        )
        .await;
    assert_eq!(
        reply.error_code, ACCESS_DENIED,
        "expected {ACCESS_DENIED}, got {} {}",
        reply.status, reply.body
    );
    let message = reply.message();
    assert_eq!(
        message,
        format!(
            "User: {} is not authorized to perform: apigateway:CreatePortal on resource: {} \
             with an explicit deny in an identity-based policy",
            denied.arn,
            portal_arn(REGION, ANY_MEMBER)
        ),
        "case \"an explicit deny\": got {message:?}"
    );

    // A caller holding no grant at all is refused by IAM's DEFAULT deny instead, which the
    // sentence says rather than blaming a statement nobody wrote.
    let ungranted = emulator.assumed_role("portal-ungranted", None).await;
    let reply = emulator
        .call(
            &ungranted.access_key_id,
            "POST",
            "/v2/portals",
            Some(create_portal_body("ungranted-portal")),
        )
        .await;
    let message = reply.message();
    assert_eq!(
        message,
        format!(
            "User: {} is not authorized to perform: apigateway:CreatePortal on resource: {} \
             because no identity-based policy allows the apigateway:CreatePortal action",
            ungranted.arn,
            portal_arn(REGION, ANY_MEMBER)
        ),
        "case \"a default deny\": got {message:?}"
    );
}

/// RULE: a refused mutation changes nothing — the refusal happens before the store is
/// reached — while the same action succeeds for a principal the account does permit and the
/// resource it created is readable.
#[tokio::test]
async fn a_refused_create_writes_nothing_and_a_permitted_one_round_trips() {
    let emulator = Emulator::new();
    let denied = emulator
        .assumed_role(
            "portal-writer-denied",
            Some(&allow_all_but("apigateway:CreatePortal")),
        )
        .await;
    let refused = emulator
        .call(
            &denied.access_key_id,
            "POST",
            "/v2/portals",
            Some(create_portal_body("never-created")),
        )
        .await;
    assert_eq!(
        refused.error_code, ACCESS_DENIED,
        "expected the create to be refused, got {} {}",
        refused.status, refused.body
    );

    // The action the deny does not name still succeeds on the SAME session, which is what
    // makes the refusal above about that action rather than about the session.
    let listed = emulator
        .call(&denied.access_key_id, "GET", "/v2/portals", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected ListPortals to be served to the same session, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body.get("items"),
        Some(&json!([])),
        "expected the refused create to have stored no portal, got {}",
        listed.body
    );

    // An unrestricted principal creates the portal and reads it back by the id it minted.
    let created = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v2/portals",
            Some(create_portal_body("permitted-portal")),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 for the account's own credential, got {} {}",
        created.status, created.body
    );
    let portal_id = created
        .body
        .get("portalId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !portal_id.is_empty(),
        "expected CreatePortal to answer a PortalId, got {}",
        created.body
    );
    let fetched = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
    assert_eq!(
        fetched.status, 200,
        "expected GetPortal to report the created portal, got {} {}",
        fetched.status, fetched.body
    );
    assert_eq!(
        fetched.body.get("portalContent"),
        created.body.get("portalContent"),
        "expected GetPortal to report what CreatePortal stored, got {}",
        fetched.body
    );

    // And a session the account DOES permit creates one too, so the gate admits more than
    // the account's own credential.
    let permitted = emulator
        .assumed_role("portal-writer", Some(&allow("apigateway:*", "*")))
        .await;
    let created = emulator
        .call(
            &permitted.access_key_id,
            "POST",
            "/v2/portals",
            Some(create_portal_body("session-portal")),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 for a session the account permits, got {} {}",
        created.status, created.body
    );
}

/// RULE: the principal a request speaks for is read from the credential it presented — an
/// assumed-role session is judged on its ROLE's policies, an IAM user on its own, and the
/// account's own published credential holds no IAM policies and is not bounded.
#[tokio::test]
async fn each_kind_of_principal_is_judged_on_the_policies_in_force_on_it() {
    let emulator = Emulator::new();
    let user = emulator
        .user(
            "portal-user",
            Some(&allow("apigateway:ListPortals", "*")),
        )
        .await;
    let session = emulator
        .assumed_role("portal-session", Some(&allow("apigateway:ListPortals", "*")))
        .await;

    // (case, the key the call presents, whether CreatePortal is authorized for it)
    let rows: &[(&str, &str, bool)] = &[
        ("the account's own published credential", ACCOUNT_KEY, true),
        (
            "an IAM user allowed only to list",
            &user.access_key_id,
            false,
        ),
        (
            "an assumed-role session allowed only to list",
            &session.access_key_id,
            false,
        ),
    ];
    for (case, key, authorized) in rows {
        let reply = emulator
            .call(
                key,
                "POST",
                "/v2/portals",
                Some(create_portal_body(&format!("portal-for-{}", key.len()))),
            )
            .await;
        assert_eq!(
            reply.authorized(),
            *authorized,
            "case {case:?}: expected authorized={authorized}, got {} {:?} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        // The grant each principal DOES hold is served, so the refusal above is about the
        // action and not about the credential.
        let listed = emulator.call(key, "GET", "/v2/portals", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListPortals to be served, got {} {}",
            listed.status, listed.body
        );
    }
    // The refusals blame the principal each credential speaks for, which is what
    // `sts:GetCallerIdentity` reports for it.
    for (case, key, arn) in [
        ("an IAM user", &user.access_key_id, &user.arn),
        ("an assumed-role session", &session.access_key_id, &session.arn),
    ] {
        let reply = emulator
            .call(
                key,
                "POST",
                "/v2/portals",
                Some(create_portal_body("blamed")),
            )
            .await;
        let message = reply.message();
        assert!(
            message.starts_with(&format!("User: {arn} is not authorized to perform:")),
            "case {case:?}: expected the refusal to blame {arn:?}, got {message:?}"
        );
    }
}
