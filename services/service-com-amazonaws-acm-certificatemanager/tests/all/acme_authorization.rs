// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO may reach this service's operations and WHICH account's ARNs
//! they may name, driven over the real wire path (an awsJson1_1 request through the
//! multi-service router into the mounted service), so what is asserted is what a client
//! sees.
//!
//! RULE A: every operation whose model declares `AccessDeniedException` evaluates the
//! identity-based IAM policies in force on the principal the request's credentials name
//! against `acm:<OperationName>` BEFORE it reads or writes anything, and a request no
//! statement allows — or one an explicit `Deny` covers — is refused with HTTP 403 and
//! performs nothing, while the account's own credential and a session a matching `Allow`
//! covers are served.
//!
//! RULE B: an ARN input member naming the resource a request acts on is held to the
//! caller's own account: an ARN whose account segment is another account's is refused as
//! invalid input (`ValidationException`, HTTP 400) before any store lookup — so the answer
//! cannot depend on whether a record with that id exists, and the refusal never echoes the
//! ARN — while an ARN of the caller's own account that no record answers to stays the
//! missing-resource verdict.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rules generalise over: for RULE A the operation (a
//! credential-bearing read, a read of a resource, a list, a create, a mutation and an
//! account-level read) and the document in force (none at all, every other action of this
//! service, an explicit deny beside an allow of everything, an allow of exactly the action,
//! an allow of the whole service); for RULE B the operation an ARN reaches and the shape of
//! the foreign ARN (another account in this Region, another account in another Region,
//! another account in another partition, an account of all nines).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_endpoint_lifecycle::{AUTHORIZATION_BEHAVIORS, CONTACTS};
use crate::certificate_lifecycle::{error_code, error_message, ACCOUNT, REGION};

/// The sentence ACM refuses a cross-account ARN with: the identifier-validation
/// envelope, scoped to the resource TYPE the offending ARN carries, and carrying nothing
/// else from the request or from the store. Stated here once for every suite of this
/// crate that pins it.
pub(crate) fn cross_account_arn_message(resource_type: &str) -> String {
    format!(
        "Resource identifier for {resource_type} has a validation error: Same account ARN \
         expected but got cross-account ARN"
    )
}

/// The access key id of the account's own published credential, as every other suite in
/// this crate signs with it: a key neither STS nor IAM holds a record for, which is the
/// position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session every row below signs with.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "acm-acme-reader";
const SESSION: &str = "acme-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving this service beside the IAM and STS services, over ONE shared engine,
/// so the policies and the session the cases seed are the ones those services hold.
pub(crate) fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Call one ACM operation signed with `access_key_id`, and return `(status, body)`.
///
/// The same wire shape every other suite of this crate uses, with the access key id as a
/// parameter: it is what names the principal the request speaks for.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
pub(crate) fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them: the identity is the `assumed-role`
/// ARN `sts:GetCallerIdentity` reports, and the role is the IAM entity whose policies are
/// in force on it.
pub(crate) fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEACMEROLE01".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Attach `arn` to the role as its PERMISSIONS BOUNDARY, the ceiling an identity-based
/// grant cannot exceed — what `iam:PutRolePermissionsBoundary` does. `None` removes it.
fn given_role_boundary(engine: &Arc<engine::InMemoryEngine>, arn: Option<&str>) {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::AttachedPermissionsBoundary;
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let role = store
        .roles
        .get_mut(&iam::name_key(ROLE))
        .unwrap_or_else(|| panic!("the IAM store holds no role {ROLE}"));
    role.permissions_boundary = arn.map(|arn| AttachedPermissionsBoundary {
        permissions_boundary_type: Some("Policy".to_string()),
        permissions_boundary_arn: Some(arn.to_string()),
    });
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
pub(crate) fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on every resource.
pub(crate) fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// The IAM user whose LONG-TERM access key one case signs with, that key's id, and two
/// managed policies the IAM emulator's own catalog publishes a document for (so attaching
/// either is a grant this emulator CAN read): one that allows every action, and one that
/// allows another service's actions only.
const USER_NAME: &str = "acm-acme-operator";
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";
const MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AdministratorAccess";
const OTHER_SERVICE_MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// That user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        ::engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it (the map IAM's own `GetUser` resolves a caller through).
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLEACMEUSER01".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Put `inline` as that user's inline policy and attach `managed` to it, replacing
/// whatever it carried — what `iam:PutUserPolicy` and `iam:AttachUserPolicy` do, and what
/// their delete counterparts do for `None`.
fn given_user_policies(
    engine: &Arc<engine::InMemoryEngine>,
    inline: Option<&Value>,
    managed: Option<&str>,
) {
    let user_key = iam::name_key(USER_NAME);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&user_key);
    if let Some(document) = inline {
        store.user_inline_policies.insert(
            user_key.clone(),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document: document.to_string(),
                },
            )]),
        );
    }
    store.user_attached_policies.remove(&user_key);
    if let Some(arn) = managed {
        store
            .user_attached_policies
            .entry(user_key)
            .or_default()
            .insert(arn.to_string());
    }
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    pub(crate) fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The policy documents every suite of this crate holds an authorized operation to, and
/// which of IAM's refusals the request then meets — `None` for a request a matching `Allow`
/// permits. Stated here ONCE so two suites cannot drift apart on the rule they pin: no
/// policy at all, an allow of the other actions of this service, an explicit deny of the
/// action beside an allow of the whole service, an allow of exactly the action, and an allow
/// of every action of this service.
///
/// `action` is the `acm:<OperationName>` under test; `others` the actions a row grants
/// instead of it. A suite with more of the class to span appends its own rows.
pub(crate) fn identity_documents(
    action: &str,
    others: &[String],
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of the other actions of this service, but not this one",
            Some(allow(json!(others))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this action beside an allow of everything",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "acm:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ]
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("acm:*"))),
            None,
        ),
    ]
}

/// The operations of this service a request can reach with a credential, as a table of
/// `(operation, body)` — one per class the authorization rule generalises over: the
/// credential-bearing read this suite was filed for, a read of a resource, a list under a
/// resource, a create, a mutation of a resource, and an account-level read.
fn authorized_operations(endpoint_arn: &str, binding_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "GetAcmeExternalAccountBindingCredentials",
            json!({ "AcmeExternalAccountBindingArn": binding_arn }),
        ),
        (
            "DescribeAcmeExternalAccountBinding",
            json!({ "AcmeExternalAccountBindingArn": binding_arn }),
        ),
        (
            "ListAcmeExternalAccountBindings",
            json!({ "AcmeEndpointArn": endpoint_arn }),
        ),
        (
            "CreateAcmeEndpoint",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": CONTACTS[0],
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
            }),
        ),
        (
            "UpdateAcmeEndpoint",
            json!({ "AcmeEndpointArn": endpoint_arn, "Contact": CONTACTS[1] }),
        ),
        ("GetAccountConfiguration", json!({})),
    ]
}

/// The resource clause the refusal of `operation` carries: ` on resource: <the ARN the
/// request ADDRESSED>` — the resource member it carries, or, for an operation that names no
/// single resource, the collection it reaches (every ACME endpoint of this account in the
/// Region) — and nothing at all for an action that addresses no resource.
fn resource_clause(operation: &str, endpoint_arn: &str, binding_arn: &str) -> String {
    match operation {
        "GetAcmeExternalAccountBindingCredentials" | "DescribeAcmeExternalAccountBinding" => {
            format!(" on resource: {binding_arn}")
        }
        "ListAcmeExternalAccountBindings" | "UpdateAcmeEndpoint" => {
            format!(" on resource: {endpoint_arn}")
        }
        "CreateAcmeEndpoint" => {
            format!(" on resource: arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/*")
        }
        "GetAccountConfiguration" => String::new(),
        other => panic!("case {other:?}: this suite states no resource for that operation"),
    }
}

/// One ACME endpoint and one external account binding under it, created with the account's
/// own credential — the precondition every case starts from.
async fn fixture(router: &MultiServiceRouter) -> (String, String) {
    let (status, created) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "CreateAcmeEndpoint",
        &json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": CONTACTS[0],
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture endpoint must be created, got {status} {created}"
    );
    let endpoint_arn = created["AcmeEndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reported no AcmeEndpointArn in {created}"))
        .to_string();

    let (status, created) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "CreateAcmeExternalAccountBinding",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture binding must be created, got {status} {created}"
    );
    let binding_arn = created["ExternalAccountBinding"]["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reported no binding ARN in {created}"))
        .to_string();
    (endpoint_arn, binding_arn)
}

/// RULE A: each operation is judged against its OWN action, so a principal granted every
/// other action of this service is refused this one with 403 and performs nothing, an
/// explicit deny refuses it whatever else the role carries, and a grant of exactly that
/// action — or of the whole service — lets the request through to its own answer. The
/// account's own credential is served throughout.
#[tokio::test]
async fn every_operation_that_models_the_refusal_is_authorized_against_its_own_action() {
    let (router, engine) = router_with_identity();
    let (endpoint_arn, binding_arn) = fixture(&router).await;
    given_session(&engine);

    let table = authorized_operations(&endpoint_arn, &binding_arn);
    let every_operation: Vec<&'static str> = table.iter().map(|(name, _)| *name).collect();

    for (operation, body) in &table {
        let action = format!("acm:{operation}");
        let others: Vec<String> = every_operation
            .iter()
            .filter(|other| *other != operation)
            .map(|other| format!("acm:{other}"))
            .collect();

        // Each row: the case, the document in force on the role, and which refusal IAM
        // reaches — or `None` for a request a matching `Allow` permits.
        let rows = identity_documents(&action, &others);

        for (row, document, refused) in rows {
            let case = format!("{operation} with {row}");
            given_role_policy(&engine, document.as_ref());
            let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, body).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        status, 403,
                        "case {case:?}: expected HTTP 403, got {status} {answer}"
                    );
                    assert_eq!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: expected an AccessDeniedException, got {answer}"
                    );
                    assert_eq!(
                        error_message(&answer),
                        format!(
                            "User: {} is not authorized to perform: {action}{} {}",
                            session_arn(),
                            resource_clause(operation, &endpoint_arn, &binding_arn),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's refusal wording, got {answer}"
                    );
                    // A refused call returns no body of the operation's own: nothing of
                    // the binding's credentials leaks through the refusal.
                    assert!(
                        answer.get("KeyId").is_none() && answer.get("MacKey").is_none(),
                        "case {case:?}: a refusal must carry no part of the answer, got {answer}"
                    );
                }
                None => {
                    assert_ne!(
                        status, 403,
                        "case {case:?}: a granted action must not be refused, got {answer}"
                    );
                    assert_ne!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: a granted action must not be refused, got {answer}"
                    );
                }
            }

            // The account's own credential is never bounded by a role's policies: the
            // same call it makes is served whatever the role carries.
            let (status, answer) = call_as(&router, ACCOUNT_KEY_ID, operation, body).await;
            assert_ne!(
                status, 403,
                "case {case:?}: the account's own credential must be served, got {answer}"
            );
        }
    }

    // And the refusals wrote nothing: the owner still reads the binding's credentials, and
    // they are the pair the create minted.
    given_role_policy(&engine, None);
    let (status, answer) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owner must still read the binding's credentials, got {status} {answer}"
    );
    let stored = {
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let store = store.lock();
        let record = store
            .acme_external_account_bindings
            .get(&binding_arn)
            .unwrap_or_else(|| panic!("the store holds no binding for {binding_arn}"));
        (record.key_id.clone(), record.mac_key.clone())
    };
    assert_eq!(
        (
            answer["KeyId"].as_str().unwrap_or_default().to_string(),
            answer["MacKey"].as_str().unwrap_or_default().to_string()
        ),
        stored,
        "expected the credentials the create persisted {stored:?}, got {answer}"
    );
}

/// RULE A, continued: a session whose role carries a grant this emulator cannot READ — an
/// AWS-managed policy, whose document AWS owns — is served rather than refused, because
/// refusing on evidence the store does not hold would deny access AWS permits; but an
/// explicit `Deny` in a document the store CAN read still refuses, whatever unreadable
/// grant sits beside it.
#[tokio::test]
async fn a_grant_this_emulator_cannot_read_is_not_a_refusal() {
    let (router, engine) = router_with_identity();
    let (_endpoint_arn, binding_arn) = fixture(&router).await;
    given_session(&engine);
    given_role_policy(&engine, None);

    // The row the store CAN read: no policy, so IAM's default deny refuses.
    let (status, answer) = call_as(
        &router,
        SESSION_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 403,
        "case \"no policy\": expected HTTP 403, got {status} {answer}"
    );

    // An AWS-managed attachment, whose document the account does not hold.
    {
        let store = iam::service_state(&*engine).expect("the IAM store");
        store
            .lock()
            .role_attached_policies
            .entry(iam::name_key(ROLE))
            .or_default()
            .insert("arn:aws:iam::aws:policy/AWSCertificateManagerFullAccess".to_string());
    }
    let (status, answer) = call_as(
        &router,
        SESSION_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_ne!(
        status, 403,
        "case \"an AWS-managed attachment\": a grant this emulator cannot read must not be \
         refused, got {answer}"
    );

    // And the limit of that allowance: an explicit `Deny` in a document the store CAN read
    // refuses the request even while the unreadable attachment stays on the role.
    let action = "acm:GetAcmeExternalAccountBindingCredentials";
    given_role_policy(
        &engine,
        Some(&json!({
            "Statement": [{ "Effect": "Deny", "Action": action, "Resource": "*" }]
        })),
    );
    let (status, answer) = call_as(
        &router,
        SESSION_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 403,
        "case \"an explicit deny beside an AWS-managed attachment\": expected HTTP 403, got \
         {status} {answer}"
    );
    assert_eq!(
        error_message(&answer),
        format!(
            "User: {} is not authorized to perform: {action} on resource: {binding_arn} with an \
             explicit deny in an identity-based policy",
            session_arn()
        ),
        "case \"an explicit deny beside an AWS-managed attachment\": expected the explicit-deny \
         wording, got {answer}"
    );
}

/// RULE A, continued: a request signed with the LONG-TERM access key of an IAM user is
/// judged on that user's own identity policies — inline and attached alike — so a user with
/// none is refused and one whose attached managed policy allows the action is served. The
/// account's own published credential, which the IAM store holds no key record for, stays
/// unbounded.
///
/// The two attached rows are what discriminate the attached-document path from the
/// allowance for a grant this emulator cannot read: both policies are ones the catalog
/// publishes a document for, and the request is refused or served according to what that
/// document says, not according to the attachment existing.
#[tokio::test]
async fn a_long_term_user_key_is_judged_on_the_users_own_policies() {
    let (router, engine) = router_with_identity();
    let (_endpoint_arn, binding_arn) = fixture(&router).await;
    given_user_key(&engine);
    let read = json!({ "AcmeExternalAccountBindingArn": binding_arn });
    let action = "acm:GetAcmeExternalAccountBindingCredentials";

    // (case, the user's inline policy, the managed policy attached to it, refused?)
    let rows: Vec<(&str, Option<Value>, Option<&str>, bool)> = vec![
        ("no policy at all", None, None, true),
        (
            "an inline allow of another action",
            Some(allow(json!("acm:ListCertificates"))),
            None,
            true,
        ),
        (
            "an inline allow of this action",
            Some(allow(json!(action))),
            None,
            false,
        ),
        (
            "an attached managed policy whose document allows another service only",
            None,
            Some(OTHER_SERVICE_MANAGED_POLICY_ARN),
            true,
        ),
        (
            "an attached managed policy whose document allows every action",
            None,
            Some(MANAGED_POLICY_ARN),
            false,
        ),
    ];

    for (case, inline, attached, refused) in rows {
        given_user_policies(&engine, inline.as_ref(), attached);
        let (status, answer) = call_as(
            &router,
            USER_KEY_ID,
            "GetAcmeExternalAccountBindingCredentials",
            &read,
        )
        .await;
        if refused {
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status} {answer}"
            );
            assert_eq!(
                error_message(&answer),
                format!(
                    "User: {} is not authorized to perform: {action} on resource: {binding_arn} \
                     because no identity-based policy allows the {action} action",
                    user_arn()
                ),
                "case {case:?}: the refusal must name the USER the key belongs to, got {answer}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a granted action must be served, got {status} {answer}"
            );
        }

        // The account's own credential is not the user's: it stays served whatever the
        // user carries.
        let (status, answer) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "GetAcmeExternalAccountBindingCredentials",
            &read,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own credential must be served, got {status} {answer}"
        );
    }
}

/// RULE A, continued: a permissions boundary CAPS what the identity-based policies grant —
/// "the maximum permissions an identity-based policy can grant" — so a session whose
/// policies allow the action is still refused while the boundary in force does not allow
/// it, and is served once the boundary does.
#[tokio::test]
async fn a_permissions_boundary_caps_what_the_policies_grant() {
    let (router, engine) = router_with_identity();
    let (_endpoint_arn, binding_arn) = fixture(&router).await;
    given_session(&engine);
    let action = "acm:GetAcmeExternalAccountBindingCredentials";
    // The policies grant the action throughout; only the ceiling differs.
    given_role_policy(&engine, Some(&allow(json!(action))));
    let read = json!({ "AcmeExternalAccountBindingArn": binding_arn });

    // (case, the boundary in force, the clause the refusal carries — `None` to be served)
    let rows: Vec<(&str, Option<&str>, Option<String>)> = vec![
        ("no boundary at all", None, None),
        (
            "a boundary allowing every action",
            Some(MANAGED_POLICY_ARN),
            None,
        ),
        (
            "a boundary allowing another service only",
            Some(OTHER_SERVICE_MANAGED_POLICY_ARN),
            Some(format!(
                "because no permissions boundary allows the {action} action"
            )),
        ),
    ];

    for (case, boundary, refused) in rows {
        given_role_boundary(&engine, boundary);
        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "GetAcmeExternalAccountBindingCredentials",
            &read,
        )
        .await;
        match refused {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a grant inside the boundary must be served, got {status} {answer}"
            ),
            Some(reason) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: a grant outside the boundary must be refused, got {status} \
                     {answer}"
                );
                assert_eq!(
                    error_message(&answer),
                    format!(
                        "User: {} is not authorized to perform: {action} on resource: \
                         {binding_arn} {reason}",
                        session_arn()
                    ),
                    "case {case:?}: expected the boundary's own refusal clause, got {answer}"
                );
            }
        }
    }
}

/// RULE B: every operation that takes the ARN of the resource it acts on refuses an ARN of
/// another account as invalid input, before any lookup and without echoing the ARN — and
/// the same operation still reports an ARN of the caller's own account that no record
/// answers to as missing, so the two verdicts stay distinguishable.
#[tokio::test]
async fn an_arn_of_another_account_is_refused_as_invalid_input() {
    let (router, _engine) = router_with_identity();
    let (endpoint_arn, binding_arn) = fixture(&router).await;
    let endpoint_id = endpoint_arn
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("{endpoint_arn} is not an ACME endpoint ARN"))
        .1
        .to_string();
    let binding_id = binding_arn
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("{binding_arn} is not an external account binding ARN"))
        .1
        .to_string();

    // The rows differ in the account segment, the Region and the partition — everything
    // the account half of an ARN generalises over.
    let foreign_endpoints: &[(&str, String)] = &[
        (
            "another account in this Region",
            format!("arn:aws:acm:{REGION}:111122223333:acme-endpoint/{endpoint_id}"),
        ),
        (
            "another account in another Region",
            format!("arn:aws:acm:eu-central-1:210987654321:acme-endpoint/{endpoint_id}"),
        ),
        (
            "another account in another partition",
            format!("arn:aws-us-gov:acm:us-gov-west-1:444455556666:acme-endpoint/{endpoint_id}"),
        ),
        (
            "an account of all nines",
            format!("arn:aws:acm:{REGION}:999999999999:acme-endpoint/{endpoint_id}"),
        ),
    ];

    for (case, foreign_endpoint) in foreign_endpoints {
        let foreign_binding = state::acme_external_account_binding_arn(foreign_endpoint, &binding_id);
        let foreign_validation = state::acme_domain_validation_arn(foreign_endpoint, "a-validation");

        // Each row: the operation and the body carrying the foreign ARN — one per ARN
        // member of this service's ACME surface that names the resource a request acts on.
        let requests: Vec<(&str, Value, &str)> = vec![
            (
                "GetAcmeExternalAccountBindingCredentials",
                json!({ "AcmeExternalAccountBindingArn": foreign_binding }),
                "acme-external-account-binding",
            ),
            (
                "DescribeAcmeExternalAccountBinding",
                json!({ "AcmeExternalAccountBindingArn": foreign_binding }),
                "acme-external-account-binding",
            ),
            (
                "RevokeAcmeExternalAccountBinding",
                json!({ "AcmeExternalAccountBindingArn": foreign_binding }),
                "acme-external-account-binding",
            ),
            (
                "DescribeAcmeEndpoint",
                json!({ "AcmeEndpointArn": foreign_endpoint }),
                "acme-endpoint",
            ),
            (
                "UpdateAcmeEndpoint",
                json!({ "AcmeEndpointArn": foreign_endpoint, "Contact": CONTACTS[1] }),
                "acme-endpoint",
            ),
            (
                "DescribeAcmeDomainValidation",
                json!({ "AcmeDomainValidationArn": foreign_validation }),
                "acme-domain-validation",
            ),
            (
                "UpdateAcmeDomainValidation",
                json!({
                    "AcmeDomainValidationArn": foreign_validation,
                    "PrevalidationOptions": { "DnsPrevalidation": {} },
                }),
                "acme-domain-validation",
            ),
        ];

        for (operation, body, resource_type) in requests {
            let case = format!("{operation} on {case}");
            let (status, refusal) = call_as(&router, ACCOUNT_KEY_ID, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a cross-account ARN must be refused, got {status} {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {refusal}"
            );
            let message = error_message(&refusal);
            assert!(
                !message.contains(foreign_endpoint.as_str()),
                "case {case:?}: a cross-account refusal must withhold the submitted ARN, got \
                 {refusal}"
            );
            assert_eq!(
                message,
                cross_account_arn_message(resource_type),
                "case {case:?}: expected the cross-account verdict scoped to {resource_type}, got \
                 {refusal}"
            );
        }
    }

    // The contrast, on the operation this suite was filed for: an ARN of the CALLER's own
    // account that no record answers to is a missing resource, not a bad request — so the
    // cross-account verdict above cannot be coming from the lookup.
    let absent = state::acme_external_account_binding_arn(
        &endpoint_arn,
        "00000000-0000-4000-8000-000000000000",
    );
    let (status, refusal) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": absent }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an absent binding of this account\": expected a refusal, got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ResourceNotFoundException",
        "case \"an absent binding of this account\": expected ResourceNotFoundException, got \
         {refusal}"
    );

    // And the owner's own binding still answers, so neither refusal touched the store.
    let (status, answer) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owner's own binding must still answer, got {status} {answer}"
    );
}
