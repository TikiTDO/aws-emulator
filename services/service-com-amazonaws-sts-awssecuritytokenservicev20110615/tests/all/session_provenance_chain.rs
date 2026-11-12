// SPDX-License-Identifier: CC0-1.0

//! A role session's PROVENANCE, hop by hop: who called an assume, which role issued the
//! session it minted, and the source identity the chain carries.
//!
//! RULE: a management-event record names the session a call was made WITH, not the resource
//! the call acted on — `userIdentity.type` `AssumedRole`, `userIdentity.arn` the CALLING
//! session's own ARN and `sessionContext.sessionIssuer.arn` the ARN of the role THAT session
//! was minted of — while the role an assume names and the session it answered are the call's
//! `requestParameters.roleArn` / `responseElements.assumedRoleUser.arn`. The source identity
//! a mint sets is recorded as `requestParameters.sourceIdentity` on the assume and as
//! `sessionContext.sourceIdentity` on every later call of that session; it persists into a
//! chained session that supplies none of its own ("The source identity value persists across
//! chained role sessions", `AssumeRoleRequest$SourceIdentity`), and a session assumed without
//! one carries no `sourceIdentity` key at all while still naming its own issuer.
//!
//! What the rows below span, beyond the suite in `session_audit_records.rs` (one role, one
//! source-identity value):
//!
//! * a chain of THREE DISTINCT roles — launcher, entry, chained — so each record's
//!   `sessionIssuer` must be the role of the session that made THAT call rather than a single
//!   role every row shares, and the assume's `userIdentity` must be the calling session and
//!   not the role being assumed; and
//! * the source identity itself at both published bounds of `sourceIdentityType`
//!   (`smithy_query(query="shape", shape="sourceIdentityType")` -> length 2..64, pattern
//!   `^[\w+=,.@-]*$`), one value using every character class that pattern admits, and the
//!   twin control that sets none.
//!
//! Every call goes over the wire as an awsQuery form POST through the same
//! `MultiServiceRouter` the emulator serves with, and the records are read back out of the
//! SHARED log in that engine — the one store CloudTrail's `LookupEvents` reports from.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::service::management_events::EVENT_SOURCE;
use smithy_server_runtime::MultiServiceRouter;

use super::session_audit_records::{assume, at, records_of};
use super::sts_sessions::{call, emulator, emulator_account, member, seed_role, trust_policy, LONG_TERM_KEY};

/// The grants every role of this suite publishes: assuming it, and setting a source
/// identity on the session — the second is needed by the mint that CARRIES a value forward
/// just as by the one that sets it ("an administrator must grant you the permissions
/// necessary", `AssumeRole`).
const TRUSTED_ACTIONS: &[&str] = &["sts:AssumeRole", "sts:SetSourceIdentity"];

/// One role of the chain, as a row reads it.
struct Role {
    /// The role's name, which is the `sessionIssuer.userName` of its sessions and the
    /// `assumed-role/<name>/<session>` segment of their ARNs.
    name: &'static str,
    /// The id IAM issued for it — the `sessionIssuer.principalId` of its sessions, and the
    /// first half of their `userIdentity.principalId`.
    id: &'static str,
    /// Its ARN, as IAM stored it.
    arn: String,
}

/// The ARN of the session `session_name` of `role`, as STS mints one.
fn session_arn(role: &Role, session_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{}/{session_name}",
        emulator_account(),
        role.name
    )
}

/// The record of the call `credential` made to `event_name`, found by the access key id the
/// request was signed with rather than by position, so a row reads its OWN call.
fn record_by_credential(
    case: &str,
    engine: &Arc<engine::InMemoryEngine>,
    event_name: &str,
    credential: &str,
) -> Value {
    let records = records_of(engine, event_name);
    records
        .iter()
        .find(|event| at(event, "/userIdentity/accessKeyId").as_deref() == Some(credential))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: no {event_name} record made with {credential:?}, got {records:?}")
        })
}

/// The record of the assume that named `session_name`.
fn assume_record(case: &str, engine: &Arc<engine::InMemoryEngine>, session_name: &str) -> Value {
    let records = records_of(engine, "AssumeRole");
    records
        .iter()
        .find(|event| {
            at(event, "/requestParameters/roleSessionName").as_deref() == Some(session_name)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: no AssumeRole record for {session_name:?}, got {records:?}")
        })
}

/// A session of `role` assumed with `caller_key`, answering the access key id it was minted
/// under and the ARN the assume reported for it.
async fn assume_session(
    case: &str,
    router: &MultiServiceRouter,
    caller_key: &str,
    role: &Role,
    session_name: &str,
    source_identity: Option<&str>,
) -> (String, String) {
    let (status, key_id, body) =
        assume(router, caller_key, &role.arn, session_name, source_identity).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole({session_name:?}) must be served, got {status} with {body}"
    );
    let arn = member(case, &body, "Arn");
    (key_id, arn)
}

/// Each hop of a role chain is audited as the session that MADE it, with the issuing role of
/// that session, and the source identity set at the first hop reaches every later record of
/// the chain — or no record at all when none was set.
#[tokio::test]
async fn each_hop_of_a_role_chain_is_audited_with_its_own_session_and_issuer() {
    let (router, engine) = emulator();
    let trust = trust_policy(TRUSTED_ACTIONS);

    // Three DISTINCT roles, each with its own id, so a record naming the wrong one is
    // visible: the role whose session LAUNCHES the chain, the role the chain enters, and the
    // role it chains on into.
    let roles: Vec<Role> = [
        ("provenance-launch", "AROASEEDEDLAUNCHROLE1"),
        ("provenance-entry", "AROASEEDEDENTRYROLE01"),
        ("provenance-chain", "AROASEEDEDCHAINROLE01"),
    ]
    .into_iter()
    .map(|(name, id)| {
        let (arn, _) = seed_role(&engine, id, name, "/", Some(&trust), Some(43_200));
        Role { name, id, arn }
    })
    .collect();
    let (launch, entry, chain) = (&roles[0], &roles[1], &roles[2]);

    // The value `sourceIdentityType` admits, spanned at both published bounds and over the
    // character classes its pattern allows, beside the twin control that sets none.
    let longest = format!("{}@example.com", "a".repeat(52));
    assert_eq!(
        longest.chars().count(),
        64,
        "the longest row must sit on the published maximum, got {longest:?}"
    );
    // (case, the source identity the entry hop sets)
    let rows: [(&str, Option<&str>); 4] = [
        ("the shortest value the model admits", Some("ab")),
        ("the longest value the model admits", Some(longest.as_str())),
        (
            "every character class the pattern admits",
            Some("dev+ops=lead,role.one@example.com-1"),
        ),
        ("no source identity at all", None),
    ];

    for (row, (case, source_identity)) in rows.into_iter().enumerate() {
        let launch_session = format!("launch{row}");
        let entry_session = format!("entry{row}");
        let chain_session = format!("chain{row}");

        // The chain: a launcher session, the entry hop that sets (or does not set) the source
        // identity, and a chained hop into a THIRD role that sets none of its own.
        let (launch_key, launch_arn) =
            assume_session(case, &router, LONG_TERM_KEY, launch, &launch_session, None).await;
        let (entry_key, entry_arn) = assume_session(
            case,
            &router,
            &launch_key,
            entry,
            &entry_session,
            source_identity,
        )
        .await;
        let (chain_key, chain_arn) =
            assume_session(case, &router, &entry_key, chain, &chain_session, None).await;
        assert_eq!(
            (launch_arn.as_str(), entry_arn.as_str(), chain_arn.as_str()),
            (
                session_arn(launch, &launch_session).as_str(),
                session_arn(entry, &entry_session).as_str(),
                session_arn(chain, &chain_session).as_str()
            ),
            "case {case:?}: each assume answers the session of the role it named"
        );

        // The ENTRY assume's own record: the call was made BY the launcher session, so its
        // `userIdentity` names that session and the role that issued IT, while the role it
        // asked for and the session it answered are the call's parameters and response.
        let record = assume_record(case, &engine, &entry_session);
        // (what the record must carry, the pointer it carries it at)
        for (pointer, expected) in [
            ("/eventSource", EVENT_SOURCE.to_string()),
            ("/userIdentity/type", "AssumedRole".to_string()),
            ("/userIdentity/arn", launch_arn.clone()),
            ("/userIdentity/principalId", format!("{}:{launch_session}", launch.id)),
            ("/userIdentity/accessKeyId", launch_key.clone()),
            (
                "/userIdentity/sessionContext/sessionIssuer/arn",
                launch.arn.clone(),
            ),
            (
                "/userIdentity/sessionContext/sessionIssuer/userName",
                launch.name.to_string(),
            ),
            ("/requestParameters/roleArn", entry.arn.clone()),
            ("/requestParameters/roleSessionName", entry_session.clone()),
            ("/responseElements/assumedRoleUser/arn", entry_arn.clone()),
            (
                "/responseElements/assumedRoleUser/assumedRoleId",
                format!("{}:{entry_session}", entry.id),
            ),
        ] {
            assert_eq!(
                at(&record, pointer),
                Some(expected.clone()),
                "case {case:?}: expected {pointer} = {expected:?}, got {record}"
            );
        }
        assert_eq!(
            at(&record, "/requestParameters/sourceIdentity").as_deref(),
            source_identity,
            "case {case:?}: expected requestParameters.sourceIdentity {source_identity:?}, got \
             {record}"
        );
        assert_eq!(
            record.get("errorCode"),
            None,
            "case {case:?}: a served assume carries no error code, got {record}"
        );

        // And the calls the two identified sessions make: each is recorded as ITS OWN session
        // and ITS OWN issuing role, and both carry the source identity the chain started with
        // — the chained hop supplied none, so what it reports is what it inherited.
        for (hop, credential, role, session_name) in [
            ("the entry session", entry_key.as_str(), entry, &entry_session),
            (
                "the chained session, which set no source identity of its own",
                chain_key.as_str(),
                chain,
                &chain_session,
            ),
        ] {
            let (status, body) = call(&router, credential, "GetCallerIdentity", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {hop}: the session's call must be served, got {status} with {body}"
            );
            let made = record_by_credential(case, &engine, "GetCallerIdentity", credential);
            for (pointer, expected) in [
                ("/userIdentity/type", "AssumedRole".to_string()),
                ("/userIdentity/arn", session_arn(role, session_name)),
                (
                    "/userIdentity/principalId",
                    format!("{}:{session_name}", role.id),
                ),
                (
                    "/userIdentity/sessionContext/sessionIssuer/arn",
                    role.arn.clone(),
                ),
                (
                    "/userIdentity/sessionContext/sessionIssuer/principalId",
                    role.id.to_string(),
                ),
                (
                    "/userIdentity/sessionContext/sessionIssuer/userName",
                    role.name.to_string(),
                ),
            ] {
                assert_eq!(
                    at(&made, pointer),
                    Some(expected.clone()),
                    "case {case:?} / {hop}: expected {pointer} = {expected:?}, got {made}"
                );
            }
            assert_eq!(
                at(&made, "/userIdentity/sessionContext/sourceIdentity").as_deref(),
                source_identity,
                "case {case:?} / {hop}: expected sessionContext.sourceIdentity \
                 {source_identity:?}, got {made}"
            );
        }
    }
}
