// SPDX-License-Identifier: CC0-1.0

//! The AUTHORIZATION a request that hands this service an execution ROLE is subject to,
//! driven over each service's own wire protocol through the real `MultiServiceRouter`
//! with EventBridge Scheduler, IAM and the token service mounted on ONE engine — the
//! shape `core/router/src/lib.rs` mounts them in — so the policies a decision reads are
//! the documents IAM's own handlers wrote and the caller is the principal the token
//! service minted.
//!
//! The RULE asserted here: naming a `Target.RoleArn` HANDS this service a role it will
//! act as, so the request is served only where the identity-based policies in force on
//! the calling principal allow it `iam:PassRole` for THAT role; otherwise it is refused
//! with HTTP 403 `AccessDeniedException`, worded as the principal, the action, the role
//! the request named and which of IAM's rules withheld the grant, and nothing is created
//! or changed.
//!
//! The rows span the class rather than the one recorded case. They differ along:
//!
//! * the OPERATION that hands the role over — every operation of this service that
//!   carries a `Target.RoleArn` (`CreateSchedule`, `UpdateSchedule`);
//! * what the caller's documents say about the pass — an allow scoped to a path of roles,
//!   an allow of every role, an allow scoped by `iam:PassedToService` to this service and
//!   one scoped to another service, documents that say nothing about `iam:PassRole` at
//!   all, and an allow followed by an explicit deny;
//! * which role the request names — one the grant covers and one it does not;
//! * the KIND of caller — an IAM user holding a long-term access key, an assumed-role
//!   session, and the account's own published credential, which no identity policy
//!   governs.
//!
//! [`the_pass_is_charged_before_the_role_is_resolved`] adds the ordering half: the pass is
//! the CALLER's permission, decided from the request alone, so it is charged before the
//! role is resolved against IAM — a caller that may not pass a role learns nothing about
//! whether that role exists.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::{
    pass_role::PASS_ROLE_ACTION, EXECUTION_ROLE_REFUSAL, SCHEDULER_SERVICE_PRINCIPAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{account_id, encode_label, error_code, error_message};

/// The Region every call below is signed for.
const REGION: &str = "eu-west-2";

/// The access key id the account's own administrative calls present: the account holds no
/// record of it, so it resolves to the account's own published credential.
const ACCOUNT_KEY: &str = "AKIDTESTTESTTEST";

/// The code and the status AWS refuses an authorization failure with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The reason clause of a refusal no readable document allowed, and of one an explicit
/// `Deny` refused — the two IAM states a row of this table lands in.
fn nothing_allows_it() -> String {
    format!("because no identity-based policy allows the {PASS_ROLE_ACTION} action")
}
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The path the scoped grant below is written over, and the two roles a request names.
const SCOPED_PATH: &str = "/scheduler/";
const SCOPED_ROLE: &str = "Runner";
const OTHER_ROLE: &str = "Elsewhere";

/// The role an assumed-role caller is a session of, and the session name it presents.
const CALLER_ROLE: &str = "schedule-author";
const SESSION_NAME: &str = "probe";

/// The API versions the two awsQuery services below are called at.
const IAM_VERSION: &str = "2010-05-08";
const STS_VERSION: &str = "2011-06-15";

/// EventBridge Scheduler, IAM and the token service over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential
    // scope, which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
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

/// One restJson1 call against EventBridge Scheduler presenting `access_key_id`, which is
/// what selects the principal the request speaks for.
async fn scheduler_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |json| {
        bytes::Bytes::from(json.to_string())
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("scheduler.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/scheduler/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(payload)
        .expect("a well-formed restJson1 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One awsQuery call against `signing_name`, answered as the XML text it is.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&encode_label(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// The text of the first `<tag>` element of an awsQuery answer.
fn xml_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An identity-based document holding `statements`.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// Every action of this service, which each caller below holds: the row is about the
/// PASS, so nothing of it turns on the caller's own scheduler permissions.
fn every_scheduler_action() -> Value {
    json!({ "Effect": "Allow", "Action": "scheduler:*", "Resource": "*" })
}

/// A document allowing the pass of exactly the roles `roles` names.
fn passing_only(roles: Vec<String>) -> Value {
    document(vec![
        every_scheduler_action(),
        json!({ "Effect": "Allow", "Action": PASS_ROLE_ACTION, "Resource": roles }),
    ])
}

/// A document saying nothing at all about passing a role.
fn passing_nothing() -> Value {
    document(vec![every_scheduler_action()])
}

/// A document that allows passing every role and then DENIES it explicitly, which is what
/// an explicit deny winning over every allow looks like.
fn passing_denied() -> Value {
    document(vec![
        every_scheduler_action(),
        json!({ "Effect": "Allow", "Action": PASS_ROLE_ACTION, "Resource": "*" }),
        json!({ "Effect": "Deny", "Action": PASS_ROLE_ACTION, "Resource": "*" }),
    ])
}

/// A document allowing the pass of every role, but only to the service `service` —
/// the `iam:PassedToService` scoping AWS guidance tells a caller to write.
fn passing_to_service(service: &str) -> Value {
    document(vec![
        every_scheduler_action(),
        json!({
            "Effect": "Allow",
            "Action": PASS_ROLE_ACTION,
            "Resource": "*",
            "Condition": { "StringEquals": { "iam:PassedToService": service } },
        }),
    ])
}

/// Which kind of credential a row's caller presents.
enum CallerKind {
    /// An IAM user holding a long-term access key, carrying `policy` inline.
    User(Value),
    /// An assumed-role session of a role carrying `policy` inline.
    RoleSession(Value),
    /// The account's own published credential, which holds no identity policy at all.
    AccountCredential,
}

/// A caller, and the roles it may hand this service under its documents.
struct CallerCase {
    case: &'static str,
    kind: CallerKind,
    /// Whether a pass of the role under [`SCOPED_PATH`] is allowed.
    passes_the_scoped_role: bool,
    /// Whether a pass of the role outside it is allowed.
    passes_another_role: bool,
    /// The reason a refusal of this caller states, when a row of it is refused.
    denied_reason: String,
}

/// Every caller the table below is taken as.
fn callers() -> Vec<CallerCase> {
    vec![
        CallerCase {
            case: "a user whose grant is scoped to one path of roles",
            kind: CallerKind::User(passing_only(vec![format!(
                "arn:aws:iam::{}:role{SCOPED_PATH}*",
                account_id()
            )])),
            passes_the_scoped_role: true,
            passes_another_role: false,
            denied_reason: nothing_allows_it(),
        },
        CallerCase {
            case: "a user whose grant names every role",
            kind: CallerKind::User(passing_only(vec!["*".to_string()])),
            passes_the_scoped_role: true,
            passes_another_role: true,
            denied_reason: nothing_allows_it(),
        },
        CallerCase {
            case: "a user whose documents say nothing about passing a role",
            kind: CallerKind::User(passing_nothing()),
            passes_the_scoped_role: false,
            passes_another_role: false,
            denied_reason: nothing_allows_it(),
        },
        CallerCase {
            case: "a user whose documents deny the pass explicitly",
            kind: CallerKind::User(passing_denied()),
            passes_the_scoped_role: false,
            passes_another_role: false,
            denied_reason: EXPLICIT_DENY.to_string(),
        },
        CallerCase {
            case: "a role session whose grant is scoped to this service",
            kind: CallerKind::RoleSession(passing_to_service(SCHEDULER_SERVICE_PRINCIPAL)),
            passes_the_scoped_role: true,
            passes_another_role: true,
            denied_reason: nothing_allows_it(),
        },
        CallerCase {
            case: "a role session whose grant is scoped to another service",
            kind: CallerKind::RoleSession(passing_to_service("ec2.amazonaws.com")),
            passes_the_scoped_role: false,
            passes_another_role: false,
            denied_reason: nothing_allows_it(),
        },
        CallerCase {
            case: "the account's own published credential",
            kind: CallerKind::AccountCredential,
            passes_the_scoped_role: true,
            passes_another_role: true,
            denied_reason: nothing_allows_it(),
        },
    ]
}

/// A role this service may assume, created under `path` and trusting the scheduler
/// service principal, by the ARN IAM holds it under.
async fn given_passable_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    path: &str,
) -> String {
    let trust = document(vec![json!({
        "Effect": "Allow",
        "Principal": { "Service": SCHEDULER_SERVICE_PRINCIPAL },
        "Action": "sts:AssumeRole",
    })]);
    let (status, body) = query(
        router,
        ACCOUNT_KEY,
        "iam",
        IAM_VERSION,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("Path", path.to_string()),
            ("AssumeRolePolicyDocument", trust.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole {name} answered {status} {body}"
    );
    xml_text(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole {name} answered no Arn: {body}"))
}

/// The credential a row's caller presents, and the principal ARN a refusal of it names.
async fn given_caller(
    router: &MultiServiceRouter,
    case: &str,
    kind: &CallerKind,
) -> (String, String) {
    match kind {
        CallerKind::AccountCredential => (
            ACCOUNT_KEY.to_string(),
            format!("arn:aws:iam::{}:user/emulator", account_id()),
        ),
        CallerKind::User(policy) => {
            let name = "schedule-author";
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "iam",
                IAM_VERSION,
                "CreateUser",
                &[("UserName", name.to_string())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateUser answered {status} {body}"
            );
            let principal = xml_text(&body, "Arn")
                .unwrap_or_else(|| panic!("case {case:?}: CreateUser answered no Arn: {body}"));
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "iam",
                IAM_VERSION,
                "PutUserPolicy",
                &[
                    ("UserName", name.to_string()),
                    ("PolicyName", "pass-roles".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutUserPolicy answered {status} {body}"
            );
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "iam",
                IAM_VERSION,
                "CreateAccessKey",
                &[("UserName", name.to_string())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateAccessKey answered {status} {body}"
            );
            let key = xml_text(&body, "AccessKeyId").unwrap_or_else(|| {
                panic!("case {case:?}: CreateAccessKey answered no AccessKeyId: {body}")
            });
            (key, principal)
        }
        CallerKind::RoleSession(policy) => {
            let trust = document(vec![json!({
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{}:root", account_id()) },
                "Action": "sts:AssumeRole",
            })]);
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "iam",
                IAM_VERSION,
                "CreateRole",
                &[
                    ("RoleName", CALLER_ROLE.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateRole {CALLER_ROLE} answered {status} {body}"
            );
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "iam",
                IAM_VERSION,
                "PutRolePolicy",
                &[
                    ("RoleName", CALLER_ROLE.to_string()),
                    ("PolicyName", "pass-roles".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutRolePolicy answered {status} {body}"
            );
            let (status, body) = query(
                router,
                ACCOUNT_KEY,
                "sts",
                STS_VERSION,
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{}:role/{CALLER_ROLE}", account_id()),
                    ),
                    ("RoleSessionName", SESSION_NAME.to_string()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AssumeRole answered {status} {body}"
            );
            let key = xml_text(&body, "AccessKeyId").unwrap_or_else(|| {
                panic!("case {case:?}: AssumeRole answered no AccessKeyId: {body}")
            });
            // The principal the session speaks for, read from the answer rather than
            // spelled here: it is what `sts:GetCallerIdentity` reports and what a refusal
            // blames.
            let principal = xml_text(&body, "Arn")
                .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no Arn: {body}"));
            (key, principal)
        }
    }
}

/// A whole `CreateSchedule`/`UpdateSchedule` body naming `role` as the target's execution
/// role.
///
/// The expression is a one-time instant far ahead, so no occurrence of a schedule this
/// suite writes is ever due and the row asserts the control plane alone.
fn schedule_body(role: &str) -> Value {
    json!({
        "ScheduleExpression": "at(2099-12-31T23:59:59)",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": {
            "Arn": format!("arn:aws:sqs:{REGION}:{}:target", account_id()),
            "RoleArn": role,
        },
    })
}

/// The `Target.RoleArn` the schedule `name` carries now, read back through `GetSchedule`.
async fn stored_role(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = scheduler_as(
        router,
        ACCOUNT_KEY,
        "GET",
        &format!("/schedules/{name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSchedule {name} answered {status} {body}"
    );
    body["Target"]["RoleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Target.RoleArn in {body}"))
        .to_string()
}

/// A request that hands this service an execution role is served only where the caller's
/// identity-based policies allow it `iam:PassRole` for that role, and a refused one
/// changes nothing.
#[tokio::test]
async fn a_request_handing_over_a_role_needs_the_callers_pass_role() {
    for (index, caller) in callers().into_iter().enumerate() {
        // One emulator per caller: the roles and the identity are built fresh, so no row
        // depends on what another row left behind.
        let router = emulator();
        let scoped = given_passable_role(&router, caller.case, SCOPED_ROLE, SCOPED_PATH).await;
        let other = given_passable_role(&router, caller.case, OTHER_ROLE, "/").await;
        let (key, principal) = given_caller(&router, caller.case, &caller.kind).await;

        for (position, (role, allowed)) in [
            (&scoped, caller.passes_the_scoped_role),
            (&other, caller.passes_another_role),
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{} / {role}", caller.case);
            let refusal = format!(
                "User: {principal} is not authorized to perform: {PASS_ROLE_ACTION} on \
                 resource: {role} {}",
                caller.denied_reason
            );

            // CreateSchedule hands the role over on the way in.
            let created = format!("created-{index}-{position}");
            let (status, answer) = scheduler_as(
                &router,
                &key,
                "POST",
                &format!("/schedules/{created}"),
                Some(schedule_body(role)),
            )
            .await;
            if allowed {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a pass the caller's policies allow is served, got \
                     {status} with {answer}"
                );
                assert_eq!(
                    stored_role(&router, &case, &created).await,
                    role.as_str(),
                    "case {case:?}: the created schedule runs the role it was handed"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case:?}: a pass the caller's policies do not allow is refused, \
                     got {status} with {answer}"
                );
                assert_eq!(
                    error_message(&answer),
                    refusal,
                    "case {case:?}: expected IAM's wording, got {answer}"
                );
                // Nothing was created: the schedule a refused create named is not there.
                let (status, body) = scheduler_as(
                    &router,
                    ACCOUNT_KEY,
                    "GET",
                    &format!("/schedules/{created}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: a refused create stored nothing, got {status} with {body}"
                );
            }

            // UpdateSchedule hands a role over for a schedule that already exists, which
            // the account's own credential created with the scoped role.
            let updated = format!("updated-{index}-{position}");
            let (status, answer) = scheduler_as(
                &router,
                ACCOUNT_KEY,
                "POST",
                &format!("/schedules/{updated}"),
                Some(schedule_body(&scoped)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the set-up create answered {status} with {answer}"
            );
            let (status, answer) = scheduler_as(
                &router,
                &key,
                "PUT",
                &format!("/schedules/{updated}"),
                Some(schedule_body(role)),
            )
            .await;
            if allowed {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an update whose pass is allowed is served, got {status} \
                     with {answer}"
                );
                assert_eq!(
                    stored_role(&router, &case, &updated).await,
                    role.as_str(),
                    "case {case:?}: the update applied the role it handed over"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case:?}: an update whose pass is refused answers the refusal, \
                     got {status} with {answer}"
                );
                assert_eq!(
                    error_message(&answer),
                    refusal,
                    "case {case:?}: expected IAM's wording, got {answer}"
                );
                assert_eq!(
                    stored_role(&router, &case, &updated).await,
                    scoped.as_str(),
                    "case {case:?}: a refused update leaves the role it could not pass \
                     unapplied"
                );
            }
        }
    }
}

/// The pass is the CALLER's permission, decided from the request alone: it is charged
/// BEFORE the role is resolved against IAM, so a caller that may not pass a role is
/// refused the same way whether or not the account holds that role — while an authorized
/// caller naming a role the account does not hold still earns the execution-role refusal.
#[tokio::test]
async fn the_pass_is_charged_before_the_role_is_resolved() {
    let router = emulator();
    let case = "a role the account does not hold";
    let (key, principal) = given_caller(&router, case, &CallerKind::User(passing_nothing())).await;
    let absent = format!("arn:aws:iam::{}:role/not-held", account_id());

    let (status, answer) = scheduler_as(
        &router,
        &key,
        "POST",
        "/schedules/unauthorized",
        Some(schedule_body(&absent)),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?}: an unauthorized caller is refused the pass, not told about the \
         role, got {status} with {answer}"
    );
    assert_eq!(
        error_message(&answer),
        format!(
            "User: {principal} is not authorized to perform: {PASS_ROLE_ACTION} on resource: \
             {absent} {}",
            nothing_allows_it()
        ),
        "case {case:?}: expected IAM's wording, got {answer}"
    );

    // The account's own credential may pass any role, so the SAME request reaches the
    // role resolution and is refused by it instead — which is what shows the two gates
    // are both in force and in this order.
    let (status, answer) = scheduler_as(
        &router,
        ACCOUNT_KEY,
        "POST",
        "/schedules/authorized",
        Some(schedule_body(&absent)),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ValidationException"),
        "case {case:?}: an authorized caller reaches the role resolution, got {status} with \
         {answer}"
    );
    assert_eq!(
        error_message(&answer),
        EXECUTION_ROLE_REFUSAL,
        "case {case:?}: expected the execution-role refusal, got {answer}"
    );
}
