// SPDX-License-Identifier: CC0-1.0

//! WHO may ingest audit events into a CloudTrail Lake channel, driven end to end over the
//! real wire path (a restJson1 request through the multi-service router into the mounted
//! service), so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `PutAuditEvents` is authorized against the identity-based IAM
//! policies in force on the principal its credentials name, for the action
//! `cloudtrail-data:PutAuditEvents` — this service's own action namespace, which is
//! DISJOINT from the CloudTrail control plane's `cloudtrail:` — on the channel reference
//! the request carried, BEFORE the channel is resolved. A request no readable statement
//! allows (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` with IAM's own sentence, carries neither `successful` nor
//! `failed`, and stores nothing; a request a matching `Allow` covers reaches the service's
//! own verdict.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` / `iam:PutUserPolicy` store and the principal
//! a refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the document in force (none at all, the
//! whole CloudTrail control-plane namespace, this operation's own name under that wrong
//! namespace, another service, exactly this action, that action in another case, this whole
//! service, everything, an explicit deny beside an allow), the resource an allow is scoped
//! to (every resource, the channel the request names, another channel of the same account,
//! the same channel id in another Region), the kind of principal (an assumed-role session,
//! an IAM user's long-term key, the account's own published credential) and whether the
//! channel the request names is registered here at all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::channel_arn_refusals::message_of;
use crate::put_audit_events::{channel_arn, ev, Harness, Reply, DEFAULT_ACCESS_KEY_ID, REGION};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The action namespace this service's operations are published under: its `sigv4Name`
/// (`smithy_query(query="service_info")`), which is not the control plane's `cloudtrail`.
const ACTION_PREFIX: &str = "cloudtrail-data";

/// The action every row below is judged against.
const ACTION: &str = "cloudtrail-data:PutAuditEvents";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLECTDATASESS";

/// The access key id of the IAM user the long-term rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLECTDATAUSER";

/// The role the session is assumed from, the session's own name, the IAM user, and the name
/// of the inline policy each of them carries.
const ROLE: &str = "audit-event-publisher";
const SESSION: &str = "publish-session";
const USER: &str = "audit-event-writer";
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A JSON audit event that passes per-event validation, so nothing but authorization and
/// the channel reference decides these calls.
const GOOD_EVENT: &str = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

/// The error code and status the authorization layer answers an unauthorized call with.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", engine::emulator_account_id())
}

/// The ARN the account holds the IAM user under, which a refusal names as that caller.
fn user_arn() -> String {
    format!("arn:aws:iam::{}:user/{USER}", engine::emulator_account_id())
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLECTDATAPUBLISH".to_string(),
            arn: role_arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
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
                account: engine::emulator_account_id().to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it.
fn given_user(engine: &Arc<engine::InMemoryEngine>) {
    let minted = aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEXAMPLECTDATAWRITER1".to_string(),
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
            user_key: iam::name_key(USER),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Put `document` in force as the role's inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    put_inline_policy(engine, Target::Role, document);
}

/// The same for the IAM user, which is what `iam:PutUserPolicy` writes.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    put_inline_policy(engine, Target::User, document);
}

/// Which identity an inline policy is attached to.
#[derive(Debug, Clone, Copy)]
enum Target {
    Role,
    User,
}

fn put_inline_policy(
    engine: &Arc<engine::InMemoryEngine>,
    target: Target,
    document: Option<&Value>,
) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let (key, attached) = match target {
        Target::Role => (iam::name_key(ROLE), &mut store.role_inline_policies),
        Target::User => (iam::name_key(USER), &mut store.user_inline_policies),
    };
    match document {
        None => {
            attached.remove(&key);
        }
        Some(document) => {
            attached.insert(
                key,
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
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy allowing this operation's action on `resource` alone.
fn allow_on(resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": ACTION, "Resource": resource }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The default deny of a request no readable statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {ACTION} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// IAM's whole sentence for a refusal: who was refused, which action, on which resource,
/// and which of IAM's rules refused it. Spelled out here rather than built from the helpers
/// the implementation calls, so a change to either half of the wording fails this test.
fn denial(principal: &str, resource: &str, refusal: Refusal) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ACTION} on resource: {resource} {}",
        refusal.reason()
    )
}

/// Assert that `reply` is the authorization refusal naming `principal` and `resource`, and
/// that it reports nothing of the batch it refused.
fn assert_refused(case: &str, reply: &Reply, principal: &str, resource: &str, refusal: Refusal) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED:?}, got {} with body {}",
        reply.status,
        reply.body
    );
    let stated = message_of(reply);
    assert_eq!(
        stated,
        denial(principal, resource, refusal),
        "case {case:?}: expected IAM's wording, got {stated:?}"
    );
    assert!(
        reply.body.get("successful").is_none() && reply.body.get("failed").is_none(),
        "case {case:?}: a denied call reports no per-event outcome, got {}",
        reply.body
    );
}

/// The documents a row puts in force, and which of IAM's refusals the request then meets —
/// `None` for a request a matching `Allow` permits.
fn documents_in_force() -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            // The recorded arm: the CloudTrail control plane's whole namespace, which
            // publishes no action of this service.
            "every action of the CloudTrail control plane",
            Some(allow(json!("cloudtrail:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            // The right action name under the wrong service prefix.
            "this operation's name under the control plane's namespace",
            Some(allow(json!("cloudtrail:PutAuditEvents"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "every action of another service entirely",
            Some(allow(json!("s3:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of a prefix that only starts like this one",
            Some(allow(json!("cloudtrail-datax:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": ACTION, "Resource": "*" },
                    { "Effect": "Deny", "Action": ACTION, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of the action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": ACTION, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        ("an allow of exactly this action", Some(allow(json!(ACTION))), None),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(ACTION.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside the control plane's",
            Some(allow(json!(["cloudtrail:LookupEvents", ACTION]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// THE RULE: the verdict depends on the caller's identity-based policies, judged against
/// `cloudtrail-data:PutAuditEvents` — a grant of the CloudTrail control plane's actions
/// covers none of it — and a refusal is IAM's 403 naming the principal, the action and the
/// channel the request carried. An allowed call falls through to the service's own verdict,
/// which for a registered channel is the ingest and for an unregistered one
/// `ChannelNotFound`.
#[tokio::test]
async fn put_audit_events_is_authorized_against_the_cloudtrail_data_action() {
    // (case suffix, is the channel registered here?, the status an allowed call gets)
    let targets: [(&str, bool, u16); 2] = [
        ("a registered channel", true, 200),
        ("a channel nothing registered", false, 400),
    ];

    for (target, registered, allowed_status) in targets {
        for (row, document, refused) in documents_in_force() {
            let case = format!("{row} / {target}");
            // A fresh engine per row: no row can pass on another's write.
            let harness = Harness::new();
            given_session(harness.engine());
            given_role_policy(harness.engine(), document.as_ref());
            let arn = channel_arn("3f2e1d0c-9b8a-4756-8463-52413a2b1c0d");
            if registered {
                harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));
            }

            let reply = harness
                .put_as(SESSION_KEY_ID, &arn, None, &[ev("evt-1", GOOD_EVENT)])
                .await;
            match refused {
                Some(refusal) => {
                    assert_refused(&case, &reply, &session_arn(), &arn, refusal);
                    assert!(
                        harness.ingest_log(&arn).is_empty(),
                        "case {case:?}: a denied call must persist nothing, got {:?}",
                        harness.ingest_log(&arn)
                    );
                }
                None => {
                    assert_eq!(
                        reply.status, allowed_status,
                        "case {case:?}: an allowed call reaches the service's own verdict, got \
                         {} with body {}",
                        reply.status, reply.body
                    );
                    assert_ne!(
                        reply.error_code(),
                        ACCESS_DENIED.1,
                        "case {case:?}: an allowed call is not an authorization refusal, got \
                         body {}",
                        reply.body
                    );
                }
            }
        }
    }
}

/// RULE: the refusal is the SAME whether or not the channel exists here, so it cannot be
/// read as an answer about what the emulator holds.
#[tokio::test]
async fn a_denial_says_nothing_about_whether_the_channel_exists() {
    let registered = channel_arn("1c2d3e4f-5061-4273-8495-a6b7c8d9e0f1");
    let absent = channel_arn("2d3e4f50-6172-4384-95a6-b7c8d9e0f1a2");

    let mut bodies: Vec<(&str, String)> = Vec::new();
    for (case, arn, register) in [
        ("a channel registered here", &registered, true),
        ("a channel nothing registered", &absent, false),
    ] {
        let harness = Harness::new();
        given_session(harness.engine());
        // The wrong namespace, which is the policy the recorded client held.
        given_role_policy(harness.engine(), Some(&allow(json!("cloudtrail:*"))));
        if register {
            harness.register(Channel::from_arn(arn, "Custom").expect("a channel ARN"));
        }
        let reply = harness
            .put_as(SESSION_KEY_ID, arn, None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        assert_refused(case, &reply, &session_arn(), arn, Refusal::DefaultDeny);
        bodies.push((case, reply.body.to_string().replace(arn.as_str(), "<resource>")));
    }
    let (first_case, first) = &bodies[0];
    for (case, body) in &bodies[1..] {
        assert_eq!(
            body, first,
            "case {case:?}: the denial must not differ from case {first_case:?} by anything \
             but the resource it names, got {body}"
        );
    }
}

/// RULE: the decision is taken against the CHANNEL the request named, so an allow scoped to
/// one channel ARN covers that channel and no other.
#[tokio::test]
async fn an_allow_is_scoped_to_the_channel_it_names() {
    let addressed = channel_arn("4f506172-8394-45a6-b7c8-d9e0f1a2b3c4");
    let other = channel_arn("50617283-94a5-46b7-88d9-e0f1a2b3c4d5");
    let other_region = engine::emulator_arn(
        "cloudtrail",
        "eu-west-3",
        "channel/4f506172-8394-45a6-b7c8-d9e0f1a2b3c4",
    );

    // (case, the Resource the allow is scoped to, whether it covers the addressed channel)
    let rows: Vec<(&str, String, bool)> = vec![
        ("every resource", "*".to_string(), true),
        ("the channel the request names", addressed.clone(), true),
        (
            "every channel of this account and Region",
            engine::emulator_arn("cloudtrail", REGION, "channel/*"),
            true,
        ),
        ("another channel of the same account", other, false),
        ("the same channel id in another Region", other_region, false),
    ];

    for (case, resource, covers) in rows {
        let harness = Harness::new();
        given_session(harness.engine());
        given_role_policy(harness.engine(), Some(&allow_on(&resource)));
        harness.register(Channel::from_arn(&addressed, "Custom").expect("a channel ARN"));

        let reply = harness
            .put_as(SESSION_KEY_ID, &addressed, None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        if covers {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: an allow of that resource must cover this call, got {} with \
                 body {}",
                reply.status, reply.body
            );
        } else {
            assert_refused(
                case,
                &reply,
                &session_arn(),
                &addressed,
                Refusal::DefaultDeny,
            );
        }
    }
}

/// RULE: the gate is about the caller's policies and nothing else — the identical call,
/// once the action is granted, is ingested and shows up in the channel's log; and the
/// decision is re-read per request, so revoking the grant refuses the very call it served.
#[tokio::test]
async fn granting_the_action_serves_the_very_call_that_was_refused() {
    let harness = Harness::new();
    given_session(harness.engine());
    let arn = channel_arn("61728394-a5b6-47c8-99e0-f1a2b3c4d5e6");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));
    let batch = [ev("evt-1", GOOD_EVENT)];

    // Everything of the CloudTrail control plane, which grants no action of this service.
    given_role_policy(harness.engine(), Some(&allow(json!("cloudtrail:*"))));
    let refused = harness.put_as(SESSION_KEY_ID, &arn, None, &batch).await;
    assert_refused(
        "the control plane's namespace",
        &refused,
        &session_arn(),
        &arn,
        Refusal::DefaultDeny,
    );
    assert!(
        harness.ingest_log(&arn).is_empty(),
        "case \"the control plane's namespace\": the refused call must store nothing, got {:?}",
        harness.ingest_log(&arn)
    );

    given_role_policy(harness.engine(), Some(&allow(json!(ACTION))));
    let served = harness.put_as(SESSION_KEY_ID, &arn, None, &batch).await;
    assert_eq!(
        served.status, 200,
        "case \"the action granted\": the identical call must be served, got {} with body {}",
        served.status, served.body
    );
    let log = harness.ingest_log(&arn);
    assert_eq!(
        log.len(),
        1,
        "case \"the action granted\": the served call must be stored once, got {log:?}"
    );

    // Revoked again: the decision is read per request, not cached from the served one.
    given_role_policy(harness.engine(), None);
    let refused_again = harness.put_as(SESSION_KEY_ID, &arn, None, &batch).await;
    assert_refused(
        "the grant revoked",
        &refused_again,
        &session_arn(),
        &arn,
        Refusal::DefaultDeny,
    );
    assert_eq!(
        harness.ingest_log(&arn).len(),
        1,
        "case \"the grant revoked\": the refused call must store nothing more, got {:?}",
        harness.ingest_log(&arn)
    );
}

/// RULE: the rule holds for every kind of principal a request can present — an assumed-role
/// session is judged on the ROLE's policies, an IAM user's long-term key on the USER's, and
/// the account's own published credential (a key neither store holds a record for) is not a
/// principal the IAM store holds policies for and is served.
#[tokio::test]
async fn each_kind_of_principal_is_judged_on_its_own_policies() {
    let arn = channel_arn("728394a5-b6c7-48d9-a0f1-a2b3c4d5e6f7");

    // (case, the key id the call signs with, the principal a refusal names, whether a
    // policy on this principal decides the call at all)
    let principals: [(&str, &str, Option<String>); 3] = [
        (
            "an assumed-role session",
            SESSION_KEY_ID,
            Some(session_arn()),
        ),
        ("an IAM user's long-term key", USER_KEY_ID, Some(user_arn())),
        (
            "the account's own published credential",
            DEFAULT_ACCESS_KEY_ID,
            None,
        ),
    ];

    for (case, key_id, principal) in principals {
        // Nothing is granted to either identity, so only the kind of principal differs.
        let harness = Harness::new();
        given_session(harness.engine());
        given_user(harness.engine());
        given_role_policy(harness.engine(), None);
        given_user_policy(harness.engine(), None);
        harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

        let reply = harness
            .put_as(key_id, &arn, None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        match &principal {
            Some(principal) => {
                assert_refused(case, &reply, principal, &arn, Refusal::DefaultDeny);
            }
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: the account's own credential holds no IAM policy and must be \
                 served, got {} with body {}",
                reply.status, reply.body
            ),
        }
    }

    // The same two identities, each granted the action on its own: the grant in force is
    // the one attached to the principal that signed, not to the other.
    for (case, key_id, grant_role) in [
        ("the role granted, signed by the session", SESSION_KEY_ID, true),
        ("the role granted, signed by the user", USER_KEY_ID, false),
    ] {
        let harness = Harness::new();
        given_session(harness.engine());
        given_user(harness.engine());
        given_role_policy(harness.engine(), Some(&allow(json!(ACTION))));
        given_user_policy(harness.engine(), None);
        harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

        let reply = harness
            .put_as(key_id, &arn, None, &[ev("evt-1", GOOD_EVENT)])
            .await;
        if grant_role {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the signing principal holds the grant and must be served, got \
                 {} with body {}",
                reply.status, reply.body
            );
        } else {
            assert_refused(case, &reply, &user_arn(), &arn, Refusal::DefaultDeny);
        }
    }
}
