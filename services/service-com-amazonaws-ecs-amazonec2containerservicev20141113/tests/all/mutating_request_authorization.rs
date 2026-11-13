// SPDX-License-Identifier: CC0-1.0

//! WHO may perform this service's MUTATING operations, driven end to end over the real wire
//! path (an `awsJson1_1` request through the multi-service router into the mounted service),
//! so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: before it resolves anything the request names, Amazon ECS decides
//! whether the identity-based IAM policies in force on the principal the request's credential
//! speaks for allow `ecs:<OperationName>` on the ARN of the resource the request addresses —
//! the cluster for a call addressed to a cluster, the service for one addressed to a service,
//! the task-definition family for a registration, the ARN the caller supplied for a tagging
//! call, and no resource at all for an action IAM publishes no resource type for. A request
//! an explicit `Deny` covers, or one no statement allows (IAM's default deny), is answered
//! HTTP 403 `AccessDeniedException` naming the principal, the action and that resource; it
//! writes nothing, EVEN WHEN what it names does not exist. A request a matching `Allow`
//! covers reaches the operation's own answer.
//!
//! The role, its inline policy and the session are made the way a client makes them —
//! `iam:CreateRole`, `iam:PutRolePolicy` and `sts:AssumeRole` over the wire, against the same
//! shared engine — so the documents under test are the ones IAM stores and the principal a
//! refusal blames is the one `sts:GetCallerIdentity` reports for the credential the call was
//! signed with.
//!
//! The tables span what the rule generalises over: the operation (a cluster create and
//! delete, a service update, a task-definition registration, a tagging write and an
//! account-setting write), the resource each is judged on (cluster, service, family, the ARN
//! the request carried, none), the document in force (an explicit deny beside an allow of
//! everything, a grant scoped to one resource, no document at all) and the principal (an
//! assumed-role session and the account's own published credential).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::ecs_wire::{iam, list, ok, ACCOUNT, SIGNED_REGION};

/// The error code an authorization refusal carries (`AccessDeniedException`, `@httpError
/// 403`).
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The status that error is answered with.
const FORBIDDEN: u16 = 403;

/// The access key id of the account's own published credential — the key every other suite
/// of this crate signs with, which neither STS nor IAM holds a record for: the position AWS
/// account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDECSTEST";

/// The STS API version an awsQuery body carries.
const STS_API_VERSION: &str = "2011-06-15";

/// A router carrying this service beside the two stores an authorization decision is read
/// from — IAM, which holds the policies in force on a principal, and STS, which holds the
/// session a credential speaks for — over ONE shared engine, mounted the way the production
/// router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call to this service signed with `access_key_id` in its SigV4 credential
/// scope — which is where this emulator reads which principal a request speaks for.
async fn ecs_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ecs.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{SIGNED_REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One awsQuery call to STS that must succeed, returning its XML body.
async fn sts(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
    case: &str,
) -> String {
    let mut body = format!("Action={action}&Version={STS_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(
            &percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
                .to_string(),
        );
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "sts.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{SIGNED_REGION}\
                 /sts/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected STS {action} to answer 200, got {status} with {text}"
    );
    text
}

/// The text of the first `<element>` of an awsQuery XML body.
fn xml_element(body: &str, element: &str) -> Option<String> {
    let open = format!("<{element}>");
    let close = format!("</{element}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One role session a row calls as: the access key id its requests are signed with, and the
/// session ARN a refusal names it by.
struct Session {
    access_key_id: String,
    session_arn: String,
}

/// Create the IAM role `role_name` carrying `document` as its inline policy (when one is
/// given), assume it, and answer the session the assume minted — each step through that
/// service's OWN API.
async fn assumed_role(
    router: &MultiServiceRouter,
    role_name: &str,
    document: Option<&str>,
    case: &str,
) -> Session {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
        case,
    )
    .await;
    if let Some(document) = document {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", document.to_string()),
            ],
            case,
        )
        .await;
    }
    let assumed = sts(
        router,
        "AssumeRole",
        &[
            (
                "RoleArn",
                format!("arn:aws:iam::{ACCOUNT}:role/{role_name}"),
            ),
            ("RoleSessionName", "ecs-session".to_string()),
        ],
        case,
    )
    .await;
    Session {
        access_key_id: xml_element(&assumed, "AccessKeyId").unwrap_or_else(|| {
            panic!("case {case:?}: AssumeRole must answer an AccessKeyId, got {assumed}")
        }),
        session_arn: xml_element(&assumed, "Arn").unwrap_or_else(|| {
            panic!("case {case:?}: AssumeRole must answer the session Arn, got {assumed}")
        }),
    }
}

/// The ARN of a cluster of this account in the signed Region, as this service mints one.
fn cluster_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}")
}

/// The ARN of a service of `cluster`, as this service mints one.
fn service_arn(cluster: &str, service: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{cluster}/{service}")
}

/// The ARN a registration of `family` is judged against: the family with every revision of
/// it, because the revision an accepted registration would mint does not exist yet.
fn family_arn(family: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:*")
}

/// The refusal IAM words for a principal an explicit `Deny` refuses `action` on `resource`
/// for. `resource` is `None` for an action IAM publishes no resource type for, which the
/// sentence names no resource clause for.
fn explicit_deny_message(principal: &str, action: &str, resource: Option<&str>) -> String {
    let clause = resource.map_or(String::new(), |resource| format!(" on resource: {resource}"));
    format!(
        "User: {principal} is not authorized to perform: ecs:{action}{clause} with an explicit \
         deny in an identity-based policy"
    )
}

/// The refusal IAM words for a principal nothing granted `action` to.
fn default_deny_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: ecs:{action} on resource: {resource} \
         because no identity-based policy allows the ecs:{action} action"
    )
}

/// A document allowing everything beside an explicit `Deny` of `denied`.
fn allow_all_but(denied: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "*", "Resource": "*" },
            { "Effect": "Deny", "Action": denied, "Resource": "*" },
        ],
    })
    .to_string()
}

/// The error code a refusal reports, as the short shape name its `__type` carries.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One row of the deny table: the call a denied session makes and the resource IAM is asked
/// about for it.
struct DeniedCall {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The resource the refusal names, or `None` for an action judged against no resource.
    resource: Option<String>,
}

#[tokio::test]
async fn an_explicit_deny_refuses_the_mutating_call_it_names_and_writes_nothing() {
    let router = emulator();
    let case = "an explicit deny beside an allow of everything";
    let denied_actions = [
        "ecs:CreateCluster",
        "ecs:DeleteCluster",
        "ecs:UpdateService",
        "ecs:RegisterTaskDefinition",
        "ecs:TagResource",
        "ecs:PutAccountSetting",
    ];
    let session = assumed_role(
        &router,
        "ecs-denied",
        Some(&allow_all_but(&denied_actions)),
        case,
    )
    .await;

    let tagged = cluster_arn("tagged-cluster");
    let rows = vec![
        DeniedCall {
            case: "CreateCluster, judged on the cluster it would create",
            operation: "CreateCluster",
            body: json!({ "clusterName": "denied-cluster" }),
            resource: Some(cluster_arn("denied-cluster")),
        },
        DeniedCall {
            case: "DeleteCluster, judged on the cluster it names",
            operation: "DeleteCluster",
            body: json!({ "cluster": "denied-cluster" }),
            resource: Some(cluster_arn("denied-cluster")),
        },
        DeniedCall {
            case: "UpdateService, judged on the service of the cluster it names",
            operation: "UpdateService",
            body: json!({ "cluster": "denied-cluster", "service": "denied-service" }),
            resource: Some(service_arn("denied-cluster", "denied-service")),
        },
        DeniedCall {
            case: "UpdateService addressed by the service's full ARN",
            operation: "UpdateService",
            body: json!({ "service": service_arn("arn-cluster", "arn-service") }),
            resource: Some(service_arn("arn-cluster", "arn-service")),
        },
        DeniedCall {
            case: "RegisterTaskDefinition, judged on the family it names",
            operation: "RegisterTaskDefinition",
            body: json!({
                "family": "denied-family",
                "containerDefinitions": [{
                    "name": "app",
                    "image": "public.ecr.aws/nginx/nginx:latest",
                    "memory": 128,
                }],
            }),
            resource: Some(family_arn("denied-family")),
        },
        DeniedCall {
            case: "TagResource, judged on the ARN the request carried",
            operation: "TagResource",
            body: json!({ "resourceArn": tagged, "tags": [{ "key": "team", "value": "a" }] }),
            resource: Some(tagged.clone()),
        },
        DeniedCall {
            case: "PutAccountSetting, an action judged against no resource of its own",
            operation: "PutAccountSetting",
            body: json!({ "name": "awsvpcTrunking", "value": "enabled" }),
            resource: None,
        },
    ];

    for row in &rows {
        let row_case = row.case;
        let (status, answered) = ecs_as(
            &router,
            &session.access_key_id,
            row.operation,
            row.body.clone(),
        )
        .await;
        assert_eq!(
            status, FORBIDDEN,
            "case {row_case:?}: expected {} to answer {FORBIDDEN}, got {status} with \
             {answered}",
            row.operation
        );
        assert_eq!(
            error_code(&answered),
            ACCESS_DENIED,
            "case {row_case:?}: expected an {ACCESS_DENIED}, got {answered}"
        );
        assert_eq!(
            answered["message"].as_str().unwrap_or_default(),
            explicit_deny_message(
                &session.session_arn,
                row.operation,
                row.resource.as_deref()
            ),
            "case {row_case:?}: expected IAM's wording for an explicit deny, got {answered}"
        );
    }

    // Nothing the denied calls asked for was written: the account holds no cluster, no
    // task-definition family and no account setting of its own.
    let clusters = ok(&router, "ListClusters", json!({}), case).await;
    assert!(
        list(&clusters, "clusterArns", case).is_empty(),
        "case {case:?}: a refused create must write nothing, got {clusters}"
    );
    let families = ok(&router, "ListTaskDefinitionFamilies", json!({}), case).await;
    assert!(
        list(&families, "families", case).is_empty(),
        "case {case:?}: a refused registration must write nothing, got {families}"
    );
    let settings = ok(
        &router,
        "ListAccountSettings",
        json!({ "effectiveSettings": false }),
        case,
    )
    .await;
    assert!(
        list(&settings, "settings", case).is_empty(),
        "case {case:?}: a refused account-setting write must write nothing, got {settings}"
    );
}

#[tokio::test]
async fn a_grant_decides_the_resource_the_request_addresses_and_nothing_else() {
    let router = emulator();
    let granted = "granted-cluster";
    let case = "a grant scoped to one cluster";
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "ecs:CreateCluster",
            "Resource": cluster_arn(granted),
        }],
    })
    .to_string();
    let session = assumed_role(&router, "ecs-scoped", Some(&document), case).await;

    // The cluster the grant names is created, so the decision is made about the resource the
    // REQUEST addresses rather than about the operation alone.
    let (status, created) = ecs_as(
        &router,
        &session.access_key_id,
        "CreateCluster",
        json!({ "clusterName": granted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the granted cluster to be created, got {status} with \
         {created}"
    );
    assert_eq!(
        created["cluster"]["clusterArn"],
        json!(cluster_arn(granted)),
        "case {case:?}: the created cluster is the one the grant names, got {created}"
    );

    // Every other row the grant does not name falls to IAM's default deny: another cluster
    // name, and another action on the cluster it does name.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "another cluster name",
            "CreateCluster",
            json!({ "clusterName": "other-cluster" }),
            cluster_arn("other-cluster"),
        ),
        (
            "another action on the granted cluster",
            "DeleteCluster",
            json!({ "cluster": granted }),
            cluster_arn(granted),
        ),
    ];
    for (row_case, operation, body, resource) in rows {
        let (status, answered) = ecs_as(&router, &session.access_key_id, operation, body).await;
        assert_eq!(
            status, FORBIDDEN,
            "case {row_case:?}: expected {operation} to answer {FORBIDDEN}, got {status} \
             with {answered}"
        );
        assert_eq!(
            answered["message"].as_str().unwrap_or_default(),
            default_deny_message(&session.session_arn, operation, &resource),
            "case {row_case:?}: expected IAM's default-deny wording, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_session_with_no_policy_is_refused_and_the_accounts_own_credential_is_served() {
    let router = emulator();

    let case = "a session whose role carries no policy";
    let session = assumed_role(&router, "ecs-unprivileged", None, case).await;
    let (status, answered) = ecs_as(
        &router,
        &session.access_key_id,
        "CreateCluster",
        json!({ "clusterName": "unprivileged-cluster" }),
    )
    .await;
    assert_eq!(
        status, FORBIDDEN,
        "case {case:?}: expected {FORBIDDEN}, got {status} with {answered}"
    );
    assert_eq!(
        answered["message"].as_str().unwrap_or_default(),
        default_deny_message(
            &session.session_arn,
            "CreateCluster",
            &cluster_arn("unprivileged-cluster")
        ),
        "case {case:?}: expected IAM's default-deny wording, got {answered}"
    );

    // The account's own published credential holds no IAM policies — the position AWS
    // account root credentials are in — so its call is served.
    let case = "the account's own published credential";
    let (status, created) = ecs_as(
        &router,
        ACCOUNT_KEY,
        "CreateCluster",
        json!({ "clusterName": "account-cluster" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the account's own credential to be served, got {status} \
         with {created}"
    );
    assert_eq!(
        created["cluster"]["clusterArn"],
        json!(cluster_arn("account-cluster")),
        "case {case:?}: the created cluster is the one the request named, got {created}"
    );
}
