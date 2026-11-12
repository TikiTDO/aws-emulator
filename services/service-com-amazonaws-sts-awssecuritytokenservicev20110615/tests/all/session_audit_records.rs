// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this service: every call it serves is a management event of
//! `sts.amazonaws.com`, and a call made WITH a session it minted is recorded as that
//! session.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own name as `eventName`, this service's
//! `eventSource`, the members the request carried as `requestParameters`, the values a
//! MUTATING call answered as `responseElements` (`readOnly` is what tells a mint from a
//! report), and the FAULT SHAPE's name as `errorCode` for a call that failed.
//!
//! RULE, for the identity a record names: a call made with the TEMPORARY credentials of a
//! session carries that session's own identity — `userIdentity.type` from the kind of
//! session, its `principalId` and its ARN — and a `sessionContext` naming the entity that
//! ISSUED the session (`sessionIssuer.arn` is the ROLE's ARN for a role session) and the
//! session's source identity, which persists across chained role sessions and is absent
//! altogether from the record of a session assumed without one.
//!
//! RULE, for credential material: a member whose value these records withhold is left OUT
//! of the recorded document entirely — no key for it at all — so no secret this service
//! minted and no bearer token a caller presented appears anywhere in event history.
//!
//! The tables below span what those rules generalise over rather than the one call a
//! recording made: the three kinds of session this service mints, a session with a source
//! identity and the twin without one, a chained session that was given none of its own,
//! both outcomes (served and refused), and a reporting call beside a minting one.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator serves
//! with, as a real awsQuery form POST; the records are read back out of the SHARED log in
//! the engine the service was built from — the one store CloudTrail's `LookupEvents` reads.

use std::sync::Arc;

use serde_json::Value;
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::service::management_events::EVENT_SOURCE;
use smithy_server_runtime::MultiServiceRouter;

use super::sts_sessions::{call, element, emulator, member, seed_demo_role, LONG_TERM_KEY};

/// The Region every call of this suite is signed for, and so the Region its records are
/// filed in: the shared harness signs every call under this scope.
const REGION: &str = "us-east-1";

/// The literal CloudTrail stores in place of a REDACTED member's value. Nothing this
/// service records carries it: what it withholds, it withholds by omission, so the literal
/// is here to be asserted ABSENT.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// The source identity the first hop sets, inside `sourceIdentityType` (length 2..64,
/// pattern `^[\w+=,.@-]*$`).
const SOURCE_IDENTITY: &str = "alice@example.com";

/// Every record in the shared log, oldest first, as the CloudTrail event document a lookup
/// reports.
pub(crate) fn records(engine: &Arc<engine::InMemoryEngine>) -> Vec<Value> {
    let log = engine.state::<ManagementEventLog>();
    let held = log.lock();
    held.iter()
        .map(ManagementEvent::to_cloudtrail_event_json)
        .collect()
}

/// The records of the calls to `event_name`, oldest first.
pub(crate) fn records_of(engine: &Arc<engine::InMemoryEngine>, event_name: &str) -> Vec<Value> {
    records(engine)
        .into_iter()
        .filter(|event| event.get("eventName") == Some(&Value::String(event_name.to_string())))
        .collect()
}

/// The most recent record of a call to `event_name`.
fn record_of(case: &str, engine: &Arc<engine::InMemoryEngine>, event_name: &str) -> Value {
    records_of(engine, event_name)
        .pop()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {event_name} record, got none"))
}

/// A string member of a record, or `None` when the record does not carry it.
pub(crate) fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// `AssumeRole` of `role_arn`, answering the status, the access key id of the session it
/// minted and the body it answered with.
pub(crate) async fn assume(
    router: &MultiServiceRouter,
    caller_key: &str,
    role_arn: &str,
    session_name: &str,
    source_identity: Option<&str>,
) -> (u16, String, String) {
    let mut params = vec![
        ("RoleArn", role_arn.to_string()),
        ("RoleSessionName", session_name.to_string()),
    ];
    if let Some(source_identity) = source_identity {
        params.push(("SourceIdentity", source_identity.to_string()));
    }
    let (status, body) = call(router, caller_key, "AssumeRole", &params).await;
    let key_id = element(&body, "AccessKeyId").unwrap_or_default();
    (status, key_id, body)
}

/// Every STS call this front door serves is recorded: the mint under its own name, with the
/// members the request carried and the session it answered with, and a reporting call beside
/// it — and no credential material in any record.
#[tokio::test]
async fn every_served_call_is_recorded_as_a_management_event() {
    let (router, engine) = emulator();
    let (role_arn, role_id) = seed_demo_role(&engine);

    let (status, session_key, body) = assume(
        &router,
        LONG_TERM_KEY,
        &role_arn,
        "audited",
        Some(SOURCE_IDENTITY),
    )
    .await;
    assert_eq!(status, 200, "setup: the assume must be served, got {body}");
    let secret = member("the assume", &body, "SecretAccessKey");
    let token = member("the assume", &body, "SessionToken");

    let record = record_of("the assume", &engine, "AssumeRole");
    // (the pointer into the event document, what the record must carry there)
    for (pointer, expected) in [
        ("/eventSource", EVENT_SOURCE.to_string()),
        ("/awsRegion", REGION.to_string()),
        ("/eventType", "AwsApiCall".to_string()),
        ("/requestParameters/roleArn", role_arn.clone()),
        ("/requestParameters/roleSessionName", "audited".to_string()),
        ("/requestParameters/sourceIdentity", SOURCE_IDENTITY.to_string()),
        (
            "/responseElements/assumedRoleUser/arn",
            format!(
                "arn:aws:sts::{}:assumed-role/demo/audited",
                engine::emulator_account_id()
            ),
        ),
        (
            "/responseElements/assumedRoleUser/assumedRoleId",
            format!("{role_id}:audited"),
        ),
        ("/responseElements/credentials/accessKeyId", session_key.clone()),
        // The caller of this assume holds a LONG-TERM key, so the record names the IAM user
        // it speaks for and no session at all.
        ("/userIdentity/type", "IAMUser".to_string()),
        ("/userIdentity/accessKeyId", LONG_TERM_KEY.to_string()),
    ] {
        assert_eq!(
            at(&record, pointer),
            Some(expected.clone()),
            "case {pointer:?}: expected {expected:?}, got {record}"
        );
    }
    assert_eq!(
        record.get("readOnly"),
        Some(&Value::Bool(false)),
        "case the assume: a mint is not a reporting call, got {record}"
    );
    assert_eq!(
        record.pointer("/userIdentity/sessionContext"),
        None,
        "case the assume: a long-term caller is in no session, got {record}"
    );
    assert_eq!(
        record.get("errorCode"),
        None,
        "case the assume: a served call carries no error code, got {record}"
    );
    assert_eq!(
        record.pointer("/resources"),
        None,
        "case the assume: the event document names no resources key, got {record}"
    );

    // A REPORTING call is recorded too, and with no `responseElements`.
    let (status, body) = call(&router, &session_key, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "setup: GetCallerIdentity under the session must be served, got {body}"
    );
    let reported = record_of("the read", &engine, "GetCallerIdentity");
    assert_eq!(
        reported.get("readOnly"),
        Some(&Value::Bool(true)),
        "case the read: a reporting call is read-only, got {reported}"
    );
    assert_eq!(
        reported.get("responseElements"),
        Some(&Value::Null),
        "case the read: a reporting call records no responseElements, got {reported}"
    );

    // No secret and no session token of the minted credential reaches any record.
    let whole_log = records(&engine)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join("\n");
    for (case, withheld) in [
        ("the secret access key", secret),
        ("the session token", token),
        ("the redaction placeholder", REDACTED.to_string()),
    ] {
        assert!(
            !whole_log.contains(&withheld),
            "case {case:?}: no record may carry it, got {whole_log}"
        );
    }
}

/// A call made with a session's credentials is recorded as that session: its own identity,
/// the role that issued it, and the source identity it carries — which a chained session
/// carries forward and a session assumed without one does not have at all.
#[tokio::test]
async fn a_session_call_records_the_session_it_was_made_by() {
    let (router, engine) = emulator();
    let (role_arn, role_id) = seed_demo_role(&engine);
    let account = engine::emulator_account_id().to_string();

    // The three sessions the rows read: one assumed WITH a source identity, one assumed
    // without, and one CHAINED from the first supplying none of its own — where the rule
    // says the value persists.
    let (status, identified_key, body) = assume(
        &router,
        LONG_TERM_KEY,
        &role_arn,
        "identified",
        Some(SOURCE_IDENTITY),
    )
    .await;
    assert_eq!(status, 200, "setup: the identified assume must be served, got {body}");
    let (status, anonymous_key, body) =
        assume(&router, LONG_TERM_KEY, &role_arn, "anonymous", None).await;
    assert_eq!(status, 200, "setup: the anonymous assume must be served, got {body}");
    let (status, chained_key, body) =
        assume(&router, &identified_key, &role_arn, "chained", None).await;
    assert_eq!(status, 200, "setup: the chained assume must be served, got {body}");

    // (case, the credential the call is made with, the session name it was minted under,
    // the source identity the record must report — `None` for a session carrying none)
    let rows = [
        (
            "a session assumed with a source identity",
            identified_key.as_str(),
            "identified",
            Some(SOURCE_IDENTITY),
        ),
        (
            "a session assumed without one",
            anonymous_key.as_str(),
            "anonymous",
            None,
        ),
        (
            "a session chained from one that carries a source identity",
            chained_key.as_str(),
            "chained",
            Some(SOURCE_IDENTITY),
        ),
    ];
    for (case, credential, session_name, source_identity) in rows {
        let (status, body) = call(&router, credential, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the session call must be served, got {body}"
        );
        let record = record_of(case, &engine, "GetCallerIdentity");
        for (pointer, expected) in [
            ("/userIdentity/type", "AssumedRole".to_string()),
            (
                "/userIdentity/principalId",
                format!("{role_id}:{session_name}"),
            ),
            (
                "/userIdentity/arn",
                format!("arn:aws:sts::{account}:assumed-role/demo/{session_name}"),
            ),
            ("/userIdentity/accessKeyId", credential.to_string()),
            // The issuer is the ROLE, never the session: that is what ties an action taken
            // through a shared role back to the role itself.
            ("/userIdentity/sessionContext/sessionIssuer/arn", role_arn.clone()),
            (
                "/userIdentity/sessionContext/sessionIssuer/type",
                "Role".to_string(),
            ),
            (
                "/userIdentity/sessionContext/sessionIssuer/principalId",
                role_id.clone(),
            ),
            (
                "/userIdentity/sessionContext/sessionIssuer/userName",
                "demo".to_string(),
            ),
        ] {
            assert_eq!(
                at(&record, pointer),
                Some(expected.clone()),
                "case {case:?}: expected {pointer} = {expected:?}, got {record}"
            );
        }
        assert_eq!(
            at(&record, "/userIdentity/sessionContext/sourceIdentity").as_deref(),
            source_identity,
            "case {case:?}: expected sourceIdentity {source_identity:?}, got {record}"
        );
        assert!(
            record
                .pointer("/userIdentity/sessionContext/attributes/creationDate")
                .is_some(),
            "case {case:?}: the session's creation instant is recorded, got {record}"
        );
    }

    // The CHAINED assume is itself a call made BY the first session, so its own record
    // carries that session's context — which is what makes the chain auditable hop by hop.
    let chained = records_of(&engine, "AssumeRole")
        .into_iter()
        .find(|event| at(event, "/requestParameters/roleSessionName").as_deref() == Some("chained"))
        .expect("the chained assume is recorded");
    assert_eq!(
        at(&chained, "/userIdentity/sessionContext/sourceIdentity").as_deref(),
        Some(SOURCE_IDENTITY),
        "case the chained assume: the calling session's source identity is recorded, got \
         {chained}"
    );
    assert_eq!(
        at(&chained, "/requestParameters/sourceIdentity"),
        None,
        "case the chained assume: the request named none, got {chained}"
    );
}

/// The three kinds of session this service mints are each recorded as what they speak for,
/// and only a ROLE session names an issuer — the twin that separates a session's own
/// identity from the entity that issued it.
#[tokio::test]
async fn each_kind_of_session_is_recorded_as_what_it_speaks_for() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let account = engine::emulator_account_id().to_string();

    let (status, role_key, body) =
        assume(&router, LONG_TERM_KEY, &role_arn, "byrole", None).await;
    assert_eq!(status, 200, "setup: the assume must be served, got {body}");
    let (status, body) = call(&router, LONG_TERM_KEY, "GetSessionToken", &[]).await;
    assert_eq!(status, 200, "setup: GetSessionToken must be served, got {body}");
    let session_token_key = member("GetSessionToken", &body, "AccessKeyId");
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "GetFederationToken",
        &[("Name", "Bob".to_string())],
    )
    .await;
    assert_eq!(status, 200, "setup: GetFederationToken must be served, got {body}");
    let federated_key = member("GetFederationToken", &body, "AccessKeyId");

    // (case, the credential, `userIdentity.type`, the session's own ARN, the issuer the
    // record names — `None` for the one kind whose issuer the session record does not hold
    // — and the `userIdentity.userName` the type implies)
    let rows = [
        (
            "an assumed-role session",
            role_key.as_str(),
            "AssumedRole",
            format!("arn:aws:sts::{account}:assumed-role/demo/byrole"),
            Some(("Role", role_arn.clone(), "demo".to_string())),
            None,
        ),
        (
            "a session token",
            session_token_key.as_str(),
            "IAMUser",
            format!("arn:aws:iam::{account}:user/emulator"),
            Some((
                "IAMUser",
                format!("arn:aws:iam::{account}:user/emulator"),
                "emulator".to_string(),
            )),
            Some("emulator"),
        ),
        (
            "a federation token",
            federated_key.as_str(),
            "FederatedUser",
            format!("arn:aws:sts::{account}:federated-user/Bob"),
            None,
            None,
        ),
    ];
    for (case, credential, principal_type, arn, issuer, user_name) in rows {
        // `GetCallerIdentity` is the one operation every kind of temporary credential may
        // call ("cannot call any STS operations except GetCallerIdentity",
        // `GetFederationToken`), so one call serves every row.
        let (status, body) = call(&router, credential, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the session call must be served, got {body}"
        );
        let record = record_of(case, &engine, "GetCallerIdentity");
        assert_eq!(
            at(&record, "/userIdentity/type").as_deref(),
            Some(principal_type),
            "case {case:?}: expected userIdentity.type {principal_type:?}, got {record}"
        );
        assert_eq!(
            at(&record, "/userIdentity/arn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the session's own ARN {arn:?}, got {record}"
        );
        let recorded_issuer = issuer.map(|(issuer_type, issuer_arn, issuer_name)| {
            (
                Some(issuer_type.to_string()),
                Some(issuer_arn),
                Some(issuer_name),
            )
        });
        assert_eq!(
            record
                .pointer("/userIdentity/sessionContext/sessionIssuer")
                .map(|held| {
                    (
                        at(held, "/type"),
                        at(held, "/arn"),
                        at(held, "/userName"),
                    )
                }),
            recorded_issuer,
            "case {case:?}: expected the issuing entity {recorded_issuer:?}, got {record}"
        );
        // The name of an IAM USER is reported on the identity; the name behind a role
        // session or a federated user is the entity its ARN names.
        assert_eq!(
            at(&record, "/userIdentity/userName").as_deref(),
            user_name,
            "case {case:?}: expected userIdentity.userName {user_name:?}, got {record}"
        );
    }
}

/// A call this service REFUSED is recorded with the fault shape's own name and the message
/// its caller was answered with: event history is the record of the calls that were served,
/// whatever they answered.
#[tokio::test]
async fn a_refused_call_is_recorded_with_the_code_its_caller_saw() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);

    // (case, the role the assume names, the status, the code the record carries)
    let rows = [
        (
            "a role no trust policy of this account admits",
            format!("arn:aws:iam::{}:role/unknown", engine::emulator_account_id()),
            403,
            "AccessDenied",
        ),
        (
            "a session policy that is not a policy document",
            role_arn.clone(),
            400,
            "MalformedPolicyDocumentException",
        ),
    ];
    for (case, role, status, code) in rows {
        let mut params = vec![
            ("RoleArn", role.clone()),
            ("RoleSessionName", "refused".to_string()),
        ];
        if code == "MalformedPolicyDocumentException" {
            params.push(("Policy", "not-a-policy".to_string()));
        }
        let (answered, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        let record = record_of(case, &engine, "AssumeRole");
        assert_eq!(
            at(&record, "/errorCode").as_deref(),
            Some(code),
            "case {case:?}: expected errorCode {code:?}, got {record}"
        );
        assert!(
            at(&record, "/errorMessage").is_some_and(|message| !message.is_empty()),
            "case {case:?}: the message the caller saw is recorded, got {record}"
        );
        assert_eq!(
            record.get("responseElements"),
            Some(&Value::Null),
            "case {case:?}: a refused call answered nothing, got {record}"
        );
        assert_eq!(
            at(&record, "/requestParameters/roleArn").as_deref(),
            Some(role.as_str()),
            "case {case:?}: the request it refused is recorded, got {record}"
        );
    }
}
