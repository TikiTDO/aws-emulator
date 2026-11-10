// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION every call of this service is subject to.
//!
//! RULE: a signed call is decided against the CALLING PRINCIPAL's identity-based IAM
//! policies BEFORE it is dispatched — whatever the operation's own modelled error set says,
//! because the refusal belongs to the authorization layer in front of the service rather
//! than to any one operation. The principal is the identity the access key id the request
//! presents speaks for, and for an `sts:AssumeRole` credential that is the session, whose
//! permissions are the role's. The decision is IAM's own: an explicit `Deny` about the
//! request wins over every `Allow`, an `Allow` with none permits, and a principal with
//! neither is refused. The action is `logs:<OperationName>` and the resource is what the
//! request addresses — the log group it names, else every resource of this service — and a
//! refusal is the AWS-common `AccessDeniedException` (HTTP 400) naming the principal, the
//! action, that resource and the rule that withheld the grant, with the handler never
//! reached and nothing of the account changed.
//!
//! Beside that rule, the second property of every response this service sends: it NAMES the
//! request it answers, in the header this `awsJson1_1` endpoint reports that id in, on a
//! success exactly as on a refusal.
//!
//! The rows span what the rule generalises over: which operation is called (a listing that
//! names no resource, a create, a write, a read and a tagging call — including operations
//! whose model declares no access error at all), whether the principal holds no policy / a
//! policy for another service's action / a grant for another action of this service / a
//! service-wide grant / a grant scoped to the resource addressed / a grant scoped to another
//! resource / a grant plus an explicit deny, and whether the credential is a role session or
//! the account's own.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, so the
//! documents this service reads are the ones those services wrote, and every case drives the
//! service the way a client does — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`.

use serde_json::{json, Value};

use crate::delivery_identity_authorization::{
    assumed_role, statement, Emulator, Principal, ACCOUNT_KEY,
};
use crate::harness::REGION;

/// The wire error code of an authorization refusal, as the AWS "Common Errors" table names
/// it. Stated here rather than read off the crate under test.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The header this protocol reports the id of the request a response answers in, matched
/// case-insensitively by `http::HeaderMap`.
const REQUEST_ID_HEADER: &str = "x-amzn-RequestId";

/// The log group every row below addresses.
const GROUP: &str = "/identity/authorized";

/// The wire error code an answer reports, or `None` for an answer that is no error body:
/// `__type` of this `awsJson1_1` envelope, with the namespace a shape id may carry dropped.
///
/// `crate::harness::error_code` requires the body to BE an error; a row here may be served or
/// refused, so the reading has to answer either way.
fn reported_code(body: &Value) -> Option<String> {
    let raw = body.get("__type").and_then(Value::as_str)?;
    Some(raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string())
}

/// The ARN CloudWatch Logs names a call on the log group `name` (and, where the request
/// names one, the log stream `stream`) by: its own resource ARN, whose group and stream
/// segments are empty where the request named none. Spelled here independently of the crate
/// under test.
fn logs_resource(name: &str, stream: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}:log-stream:{stream}",
        engine::emulator_account_id()
    )
}

/// The ARN a policy statement names the log group `name` of this account by: CloudWatch
/// Logs' log-group ARN, which ends with the `:*` that covers the streams under the group.
fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}:*",
        engine::emulator_account_id()
    )
}

/// The whole sentence IAM refuses a call with, stated here independently of the crate under
/// test: who was refused, which action, on which resource, and which of IAM's rules withheld
/// the grant.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The reason clause of a call no readable document allows — IAM's default deny.
fn nothing_allows(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause of a call an explicit `Deny` refuses.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The IAM action one operation of this service is performed under.
fn action_of(operation: &str) -> String {
    format!("logs:{operation}")
}

/// One operation under test: its name, the body a client calls it with, and the resource the
/// request addresses — `*` for a call that names no log group of its own.
struct Called {
    case: &'static str,
    operation: &'static str,
    body: Value,
    resource: String,
}

/// The operations the table below is run over: a listing that names no resource, a create, a
/// retention write, a stream read and a tagging call. Only one of them declares an access
/// error in its own model, which is the point: the decision does not come from the
/// operation's error set.
fn called() -> Vec<Called> {
    vec![
        Called {
            case: "a listing that names no log group",
            operation: "DescribeLogGroups",
            body: json!({}),
            resource: logs_resource("", ""),
        },
        Called {
            case: "a create, named by the group it would mint",
            operation: "CreateLogGroup",
            body: json!({"logGroupName": GROUP}),
            resource: logs_resource(GROUP, ""),
        },
        Called {
            case: "a retention write on the group",
            operation: "PutRetentionPolicy",
            body: json!({"logGroupName": GROUP, "retentionInDays": 7}),
            resource: logs_resource(GROUP, ""),
        },
        Called {
            case: "a stream read addressed by log group identifier",
            operation: "DescribeLogStreams",
            body: json!({"logGroupIdentifier": GROUP}),
            resource: logs_resource(GROUP, ""),
        },
        Called {
            case: "a write addressed by group and stream",
            operation: "PutLogEvents",
            body: json!({
                "logGroupName": GROUP,
                "logStreamName": "stream-1",
                "logEvents": [{"timestamp": 1_767_225_600_000i64, "message": "one"}],
            }),
            resource: logs_resource(GROUP, "stream-1"),
        },
        Called {
            case: "a tagging call addressed by this service's own resource ARN",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": group_arn(GROUP)}),
            resource: group_arn(GROUP),
        },
    ]
}

/// Assert `operation` called as `principal` was refused as unauthorized, with the whole
/// sentence IAM states for `reason`.
async fn refused(
    aws: &Emulator,
    principal: &Principal,
    call: &Called,
    case: &str,
    reason: &str,
) {
    let (status, parsed) = aws
        .call_as(&principal.access_key_id, call.operation, &call.body)
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {} to be refused with HTTP 400, got {status} {parsed}",
        call.operation
    );
    assert_eq!(
        reported_code(&parsed).as_deref(),
        Some(ACCESS_DENIED),
        "case {case:?}: expected the error code {ACCESS_DENIED}, got {parsed}"
    );
    let action = action_of(call.operation);
    let expected = not_authorized(&principal.arn, &action, &call.resource, reason);
    let stated = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {parsed}"));
    assert_eq!(
        stated, expected,
        "case {case:?}: expected IAM's wording {expected:?}, got {stated:?}"
    );
}

/// Assert `operation` called as `principal` was NOT refused as unauthorized: the answer is
/// the operation's own, whatever it is — a call the policies admit reaches the handler, and
/// what the handler then answers (a success, or a refusal of its own about the resource) is
/// not this rule's business.
async fn served(aws: &Emulator, principal: &Principal, call: &Called, case: &str) {
    let (status, parsed) = aws
        .call_as(&principal.access_key_id, call.operation, &call.body)
        .await;
    assert_ne!(
        reported_code(&parsed).as_deref(),
        Some(ACCESS_DENIED),
        "case {case:?}: expected {} to reach its handler, got {status} {parsed}",
        call.operation
    );
}

/// RULE: every operation of this service is decided against the caller's identity-based
/// policies, so a principal whose documents allow nothing of this service is refused
/// whichever operation it calls — with the action and the resource that call addresses.
#[tokio::test]
async fn a_call_no_policy_allows_is_refused_whichever_operation_it_is() {
    let aws = Emulator::new();
    // The two principals the recorded least-privilege case is about: one carrying no policy
    // at all, and one carrying a single inline policy for ANOTHER service's action.
    let empty = assumed_role(&aws, "logs-no-policy", &[]).await;
    let elsewhere = assumed_role(
        &aws,
        "logs-other-service",
        &[statement("Allow", "sts:GetCallerIdentity", "*")],
    )
    .await;
    for call in called() {
        for (who, principal) in [("no policy at all", &empty), ("one unrelated grant", &elsewhere)]
        {
            let case = format!("{} under a principal with {who}", call.case);
            let reason = nothing_allows(&action_of(call.operation));
            refused(&aws, principal, &call, &case, &reason).await;
        }
    }
}

/// RULE: the decision is IAM's own evaluation of the documents in force, so WHICH statements
/// the principal carries decides the call: a grant about the action and the resource permits
/// it, one about either alone does not, and an explicit deny wins over every grant.
#[tokio::test]
async fn the_documents_in_force_decide_the_call() {
    let aws = Emulator::new();
    let write = Called {
        case: "a retention write on the group",
        operation: "PutRetentionPolicy",
        body: json!({"logGroupName": GROUP, "retentionInDays": 7}),
        resource: logs_resource(GROUP, ""),
    };
    let action = action_of(write.operation);
    // (case, the documents the role carries, the expected outcome)
    let rows: Vec<(&str, Vec<String>, Option<String>)> = vec![
        (
            "a service-wide grant",
            vec![statement("Allow", "logs:*", "*")],
            None,
        ),
        (
            "a grant scoped to the group the request addresses",
            vec![statement("Allow", &action, &group_arn(GROUP))],
            None,
        ),
        (
            "a grant scoped to another group",
            vec![statement("Allow", &action, &group_arn("/identity/other"))],
            Some(nothing_allows(&action)),
        ),
        (
            "a grant for another action of this service on the same group",
            vec![statement(
                "Allow",
                &action_of("DeleteRetentionPolicy"),
                &group_arn(GROUP),
            )],
            Some(nothing_allows(&action)),
        ),
        (
            "a service-wide grant and an explicit deny of this action",
            vec![
                statement("Allow", "logs:*", "*"),
                statement("Deny", &action, "*"),
            ],
            Some(EXPLICIT_DENY.to_string()),
        ),
        (
            "two documents, one of which grants the call",
            vec![
                statement("Allow", "sts:GetCallerIdentity", "*"),
                statement("Allow", &action, &group_arn(GROUP)),
            ],
            None,
        ),
    ];
    // The group exists, so a call the documents admit answers the operation's own success
    // rather than a refusal about the resource.
    aws.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    for (index, (case, documents, refusal)) in rows.into_iter().enumerate() {
        let principal = assumed_role(&aws, &format!("logs-documents-{index}"), &documents).await;
        match refusal {
            Some(reason) => refused(&aws, &principal, &write, case, &reason).await,
            None => served(&aws, &principal, &write, case).await,
        }
    }
}

/// RULE: the decision is taken before the operation reads or writes anything, so a refused
/// call leaves nothing behind — and it is taken before the resource is resolved, so a caller
/// holding no grant is refused for a group the account does not even hold.
#[tokio::test]
async fn a_refused_call_changes_nothing_and_resolves_nothing() {
    let aws = Emulator::new();
    let principal = assumed_role(
        &aws,
        "logs-refused-write",
        &[statement("Allow", "sts:GetCallerIdentity", "*")],
    )
    .await;
    let absent = "/identity/never-created";
    for (case, operation, body, resource) in [
        (
            "a create the policies do not allow",
            "CreateLogGroup",
            json!({"logGroupName": absent}),
            logs_resource(absent, ""),
        ),
        (
            "a delete of a group the account does not hold",
            "DeleteLogGroup",
            json!({"logGroupName": absent}),
            logs_resource(absent, ""),
        ),
    ] {
        let call = Called {
            case,
            operation,
            body,
            resource,
        };
        let reason = nothing_allows(&action_of(operation));
        refused(&aws, &principal, &call, case, &reason).await;
    }
    // Read back with the account's own credential, which holds no IAM policies and is served:
    // the refused create stored nothing.
    let page = aws
        .ok("DescribeLogGroups", &json!({"logGroupNamePrefix": absent}))
        .await;
    let groups = page["logGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeLogGroups reports a logGroups list, got {page}"))
        .clone();
    assert!(
        groups.is_empty(),
        "case \"a refused create\": expected the account to hold no log group, got {groups:?}"
    );
}

/// RULE: the account's own published credential holds no IAM policies — the position AWS
/// account root credentials are in — so it is served, which is also what keeps an emulated
/// service's in-process call to this one from being refused for want of a grant.
#[tokio::test]
async fn the_accounts_own_credential_keeps_being_served() {
    let aws = Emulator::new();
    for call in called() {
        let (status, parsed) = aws
            .call_as(ACCOUNT_KEY, call.operation, &call.body)
            .await;
        assert_ne!(
            reported_code(&parsed).as_deref(),
            Some(ACCESS_DENIED),
            "case {:?}: expected the account's own credential to be served, got {status} {parsed}",
            call.case
        );
    }
}

/// RULE: every response of this service names the request it answers, in the header this
/// protocol reports that id in — a success, a modelled refusal and the authorization refusal
/// alike — and each call is named by an id of its own.
#[tokio::test]
async fn every_response_names_the_request_it_answers() {
    let aws = Emulator::new();
    let denied = assumed_role(
        &aws,
        "logs-request-id",
        &[statement("Allow", "sts:GetCallerIdentity", "*")],
    )
    .await;
    // (case, the key the call is signed with, the operation, the body, the expected status)
    let rows: Vec<(&str, &str, &str, Value, u16)> = vec![
        (
            "a success",
            ACCOUNT_KEY,
            "DescribeLogGroups",
            json!({}),
            200,
        ),
        (
            "a modelled refusal",
            ACCOUNT_KEY,
            "DeleteLogGroup",
            json!({"logGroupName": "/identity/absent"}),
            400,
        ),
        (
            "a constraint refusal raised before the handler",
            ACCOUNT_KEY,
            "PutRetentionPolicy",
            json!({"logGroupName": GROUP, "retentionInDays": 0}),
            400,
        ),
        (
            "an authorization refusal",
            &denied.access_key_id,
            "DescribeLogGroups",
            json!({}),
            400,
        ),
    ];
    let mut named: Vec<String> = Vec::new();
    for (case, key, operation, body, expected_status) in rows {
        let (status, headers, parsed) = aws.reported_as(key, operation, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {parsed}"
        );
        let reported = headers
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a {REQUEST_ID_HEADER} header, got {headers:?}")
            })
            .to_string();
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected the response to name the request it answers, got {reported:?}"
        );
        assert!(
            !named.contains(&reported),
            "case {case:?}: expected each response to name its own request, got {reported} twice"
        );
        named.push(reported);
    }
}
