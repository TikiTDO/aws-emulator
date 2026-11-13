// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven over the real wire path (an awsJson1_0
//! request through the multi-service router into the mounted service), so what is asserted
//! is what an SDK client sees.
//!
//! RULE: every operation whose model declares `AccessDeniedException` is served only to a
//! caller whose identity-based IAM policies allow that operation's own action
//! (`timestream-influxdb:<OperationName>`) on the resource the request addresses — the ARN
//! of the record whose id the request carries, and every resource of the addressed family
//! in the Region for a request that names no existing record (a create, whose id is minted
//! only after the decision, or a listing). Every other caller is refused with HTTP 403
//! `AccessDeniedException`, carrying IAM's own sentence about the principal the credential
//! speaks for, BEFORE the operation reads or writes anything — so a refused create
//! persists nothing and a refused read tells the caller nothing about what the account
//! holds, not even whether the operation is implemented.
//!
//! The decision is the real one: the principals below are made with IAM's OWN operations
//! over the same engine (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`), so the
//! documents this service reads are the ones IAM wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! The tables span what the rule generalises over: the OPERATION asked for (a create, a
//! read of one record, a listing, and one whose handler is still a scaffold — each with
//! its own action and its own kind of judged resource), what the caller's policies say
//! (nothing at all, an allow of exactly that action, an allow of the whole service, an
//! allow of another service, an allow scoped to another resource of the same family, and
//! an explicit deny beside an allow of everything), and the KIND of principal (an IAM user
//! and the account's own published credential).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{ACCOUNT, REGION};

/// The access key id standing for the account's own published credential: a key neither
/// STS nor IAM holds a record for, so it resolves to the account's ambient identity, which
/// holds no IAM policies and is allowed — the position AWS account root credentials are
/// in.
const ACCOUNT_KEY: &str = "AKIDEMULATORACCOUNT";

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "timestream-influxdb";

/// The wire name and status of the modelled refusal
/// (`smithy_query(query="shape", shape="AccessDeniedException")`: `@error("client")`,
/// `@httpError(403)`).
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The status a handler this emulator has not implemented answers a caller IAM permits
/// with — the scaffold channel's `NotImplemented`.
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The clauses an authorization refusal closes with, per IAM rule
/// (`service_common::iam_authorization`).
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The error code the awsJson envelope carries.
    fn error_code(&self) -> String {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `message` the error body carries.
    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id and Region
        // from.
        router.set_request_authentication(false);
        router.register_service(influxdb::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One `awsJson1_0` call to this service, with `access_key_id` in the SigV4 credential
    /// scope.
    async fn call_as(&self, access_key_id: &str, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("AmazonTimestreamInfluxDB.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}\
                     /{ACTION_PREFIX}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let body = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// One awsQuery call to IAM, signed with the account's own credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam\
                     /aws4_request, SignedHeaders=host, Signature=0"
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

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint
    /// a long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .iam("CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.iam(
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
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// The DB parameter groups this account holds, read as the account.
    async fn parameter_group_names(&self) -> Vec<String> {
        let reply = self
            .call_as(ACCOUNT_KEY, "ListDbParameterGroups", &json!({}))
            .await;
        assert_eq!(
            reply.status, 200,
            "the account's own read must be served, got {} with {}",
            reply.status, reply.body
        );
        reply.body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["name"].as_str().map(str::to_string))
            .collect()
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
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

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// A policy document allowing everything and denying `action` outright.
fn allow_everything_but(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "*", "Resource": "*"},
            {"Effect": "Deny", "Action": action, "Resource": "*"},
        ],
    })
    .to_string()
}

/// The ARN of one resource of `segment`'s family in [`REGION`], as this service mints it.
fn family_arn(segment: &str, id: &str) -> String {
    format!("arn:aws:{ACTION_PREFIX}:{REGION}:{ACCOUNT}:{segment}/{id}")
}

/// One row of the authorization table.
struct Row {
    case: &'static str,
    /// The operation the principal calls.
    operation: &'static str,
    /// Its request body.
    body: fn() -> Value,
    /// The ARN the action is judged against, which the refusal names.
    resource: fn() -> String,
    /// The inline policy in force on the caller, or `None` for a user with no policy at
    /// all.
    document: Option<fn() -> String>,
    /// `None` where the request reaches the handler; otherwise the clause the refusal
    /// closes with, or an empty string for IAM's default deny (whose clause names the
    /// action).
    refused_because: Option<&'static str>,
    /// The status the request earns when it is NOT refused: 200 for an implemented
    /// operation, 501 for one still scaffolded.
    served_status: u16,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create with no policy at all",
            operation: "CreateDbParameterGroup",
            body: || json!({ "name": "denied-group" }),
            resource: || family_arn("db-parameter-group", "*"),
            document: None,
            refused_because: Some(""),
            served_status: 200,
        },
        Row {
            case: "a create allowed by a grant of exactly that action",
            operation: "CreateDbParameterGroup",
            body: || json!({ "name": "allowed-group" }),
            resource: || family_arn("db-parameter-group", "*"),
            document: Some(|| {
                allow(
                    &format!("{ACTION_PREFIX}:CreateDbParameterGroup"),
                    &family_arn("db-parameter-group", "*"),
                )
            }),
            refused_because: None,
            served_status: 200,
        },
        Row {
            case: "a create allowed by a grant of the whole service",
            operation: "CreateDbParameterGroup",
            body: || json!({ "name": "wildcard-group" }),
            resource: || family_arn("db-parameter-group", "*"),
            document: Some(|| allow(&format!("{ACTION_PREFIX}:*"), "*")),
            refused_because: None,
            served_status: 200,
        },
        Row {
            case: "a create whose only grant is of another service",
            operation: "CreateDbParameterGroup",
            body: || json!({ "name": "other-service" }),
            resource: || family_arn("db-parameter-group", "*"),
            document: Some(|| allow("s3:*", "*")),
            refused_because: Some(""),
            served_status: 200,
        },
        Row {
            case: "a create an explicit deny covers beside an allow of everything",
            operation: "CreateDbParameterGroup",
            body: || json!({ "name": "denied-outright" }),
            resource: || family_arn("db-parameter-group", "*"),
            document: Some(|| {
                allow_everything_but(&format!("{ACTION_PREFIX}:CreateDbParameterGroup"))
            }),
            refused_because: Some(EXPLICIT_DENY),
            served_status: 200,
        },
        Row {
            case: "a listing with no policy at all",
            operation: "ListDbParameterGroups",
            body: || json!({}),
            resource: || family_arn("db-parameter-group", "*"),
            document: None,
            refused_because: Some(""),
            served_status: 200,
        },
        Row {
            case: "a listing allowed by a grant of the whole service",
            operation: "ListDbParameterGroups",
            body: || json!({}),
            resource: || family_arn("db-parameter-group", "*"),
            document: Some(|| allow(&format!("{ACTION_PREFIX}:*"), "*")),
            refused_because: None,
            served_status: 200,
        },
        Row {
            case: "a read of one record with no policy at all",
            operation: "GetDbParameterGroup",
            body: || json!({ "identifier": "abc123" }),
            resource: || family_arn("db-parameter-group", "abc123"),
            document: None,
            refused_because: Some(""),
            served_status: 404,
        },
        Row {
            case: "a read of one record, granted on ANOTHER record of the same family",
            operation: "GetDbParameterGroup",
            body: || json!({ "identifier": "abc123" }),
            resource: || family_arn("db-parameter-group", "abc123"),
            document: Some(|| {
                allow(
                    &format!("{ACTION_PREFIX}:GetDbParameterGroup"),
                    &family_arn("db-parameter-group", "zzz999"),
                )
            }),
            refused_because: Some(""),
            served_status: 404,
        },
        Row {
            case: "a read of one record, granted on that record",
            operation: "GetDbParameterGroup",
            body: || json!({ "identifier": "abc123" }),
            resource: || family_arn("db-parameter-group", "abc123"),
            document: Some(|| {
                allow(
                    &format!("{ACTION_PREFIX}:GetDbParameterGroup"),
                    &family_arn("db-parameter-group", "abc123"),
                )
            }),
            // Nothing created that id, so a permitted read reaches the handler and is
            // answered by the handler's own not-found rather than by the gate.
            refused_because: None,
            served_status: 404,
        },
        Row {
            case: "an operation whose handler is still a scaffold, with no policy at all",
            operation: "GetDbBackup",
            body: || json!({ "identifier": "bkp123" }),
            resource: || family_arn("db-backup", "bkp123"),
            document: None,
            refused_because: Some(""),
            served_status: NOT_IMPLEMENTED_STATUS,
        },
        Row {
            case: "an operation whose handler is still a scaffold, allowed",
            operation: "GetDbBackup",
            body: || json!({ "identifier": "bkp123" }),
            resource: || family_arn("db-backup", "bkp123"),
            document: Some(|| allow(&format!("{ACTION_PREFIX}:*"), "*")),
            // The authorized caller reaches the scaffold, which answers its own 501: the
            // gate decides access, not whether the operation exists.
            refused_because: None,
            served_status: NOT_IMPLEMENTED_STATUS,
        },
    ]
}

#[tokio::test]
async fn a_request_is_served_only_where_the_callers_policies_allow_the_action() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // One emulator per row, so what a served request changed cannot reach the next.
        let emulator = Emulator::new();
        let document = row.document.map(|build| build());
        let caller = emulator
            .user(&format!("caller-{index}"), document.as_deref())
            .await;

        let reply = emulator
            .call_as(&caller.access_key_id, row.operation, &(row.body)())
            .await;

        let Some(reason) = row.refused_because else {
            assert_eq!(
                reply.status, row.served_status,
                "case {case:?}: expected the request to reach the handler ({}), got {} with {}",
                row.served_status, reply.status, reply.body
            );
            continue;
        };

        assert_eq!(
            reply.status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected {ACCESS_DENIED}, got {}",
            reply.body
        );
        let action = format!("{ACTION_PREFIX}:{}", row.operation);
        let clause = if reason.is_empty() {
            format!("because no identity-based policy allows the {action} action")
        } else {
            reason.to_string()
        };
        let expected = format!(
            "User: {} is not authorized to perform: {action} on resource: {} {clause}",
            caller.arn,
            (row.resource)(),
        );
        let message = reply.message();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The refused call performed NOTHING: no parameter group the row asked for exists,
        // whatever the operation was.
        let held = emulator.parameter_group_names().await;
        let asked_for = (row.body)()["name"].as_str().map(str::to_string);
        if let Some(name) = asked_for {
            assert!(
                !held.contains(&name),
                "case {case:?}: the refused create must persist nothing, but the account \
                 holds {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_accounts_own_credential_is_served_and_an_unprivileged_user_is_not() {
    // The two principals side by side on ONE emulator: the account's own published
    // credential, which holds no IAM policies, and a freshly created IAM user with no
    // policy attached. Same operation, same request — only the credential differs.
    let emulator = Emulator::new();
    let unprivileged = emulator.user("no-policies", None).await;

    let served = emulator
        .call_as(
            ACCOUNT_KEY,
            "CreateDbParameterGroup",
            &json!({ "name": "by-the-account" }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case \"the account's own credential\": expected the create to be served, got {} \
         with {}",
        served.status, served.body
    );

    let refused = emulator
        .call_as(
            &unprivileged.access_key_id,
            "CreateDbParameterGroup",
            &json!({ "name": "by-the-user" }),
        )
        .await;
    assert_eq!(
        refused.status, ACCESS_DENIED_STATUS,
        "case \"an unprivileged user\": expected HTTP {ACCESS_DENIED_STATUS}, got {} with {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        ACCESS_DENIED,
        "case \"an unprivileged user\": expected {ACCESS_DENIED}, got {}",
        refused.body
    );
    let action = format!("{ACTION_PREFIX}:CreateDbParameterGroup");
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {} because no \
         identity-based policy allows the {action} action",
        unprivileged.arn,
        family_arn("db-parameter-group", "*"),
    );
    assert_eq!(
        refused.message(),
        expected,
        "case \"an unprivileged user\": expected {expected:?}, got {:?}",
        refused.message()
    );

    // Only the account's own create took effect.
    let held = emulator.parameter_group_names().await;
    assert!(
        held.contains(&"by-the-account".to_string()),
        "case \"the account's own credential\": expected its group to be held, got {held:?}"
    );
    assert!(
        !held.contains(&"by-the-user".to_string()),
        "case \"an unprivileged user\": the refused create must persist nothing, got {held:?}"
    );
}

#[tokio::test]
async fn a_grant_withdrawn_stops_being_a_grant() {
    // The documents are read per request, so deleting the policy that allowed the action
    // refuses the very next call — and re-attaching it serves again.
    let emulator = Emulator::new();
    let caller = emulator
        .user(
            "grant-cycle",
            Some(&allow(&format!("{ACTION_PREFIX}:*"), "*")),
        )
        .await;

    for (case, name) in [("granted", "cycle-one"), ("granted again", "cycle-three")] {
        let reply = emulator
            .call_as(
                &caller.access_key_id,
                "CreateDbParameterGroup",
                &json!({ "name": name }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the create to be served, got {} with {}",
            reply.status, reply.body
        );

        if case == "granted" {
            emulator
                .iam(
                    "DeleteUserPolicy",
                    &[
                        ("UserName", "grant-cycle".to_string()),
                        ("PolicyName", "inline".to_string()),
                    ],
                )
                .await;
            let refused = emulator
                .call_as(
                    &caller.access_key_id,
                    "CreateDbParameterGroup",
                    &json!({ "name": "cycle-two" }),
                )
                .await;
            assert_eq!(
                refused.status, ACCESS_DENIED_STATUS,
                "case \"the grant withdrawn\": expected HTTP {ACCESS_DENIED_STATUS}, got {} \
                 with {}",
                refused.status, refused.body
            );
            emulator
                .iam(
                    "PutUserPolicy",
                    &[
                        ("UserName", "grant-cycle".to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", allow(&format!("{ACTION_PREFIX}:*"), "*")),
                    ],
                )
                .await;
        }
    }

    let held = emulator.parameter_group_names().await;
    for name in ["cycle-one", "cycle-three"] {
        assert!(
            held.contains(&name.to_string()),
            "case \"the served creates\": expected {name:?} to be held, got {held:?}"
        );
    }
    assert!(
        !held.contains(&"cycle-two".to_string()),
        "case \"the grant withdrawn\": the refused create must persist nothing, got {held:?}"
    );
}
