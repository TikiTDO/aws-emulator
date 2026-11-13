// SPDX-License-Identifier: CC0-1.0

//! The member RELATIONSHIP each member operation acts on, over every relationship a
//! member account can be in.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over the CROSS PRODUCT of the relationships a
//! member can be in and the operations that take an `AccountIds` list, rather than
//! over one recorded call: holding a member record and being an ASSOCIATED member of
//! the administrator are different facts, and so is being MONITORED under it. An
//! account GuardDuty has only recorded (`Created`) or whose association a
//! disassociation ended (`Removed`) is not an associated member — "to only return
//! associated members or to return all members (including members who haven't been
//! invited yet or have been disassociated)"
//! (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`) —
//! and monitoring begins only when the member ACCOUNT answers the invitation. So each
//! operation processes only the accounts in the relationship it acts on, and reports
//! every other account the request names in `UnprocessedAccounts` with "a reason why
//! the account hasn't been processed" (`smithy_query(shape="UnprocessedAccount")`),
//! changing nothing for it.
//!
//! The reasons are GuardDuty's FIXED text: they name the administrator as "the
//! current account" and carry no account id at all, so a client matching the sentence
//! matches it whatever account the service answers for.
//!
//! The MONITORED relationship is driven through this service's state module rather
//! than over the wire, because GuardDuty reaches it when the member ACCOUNT answers
//! the administrator's invitation and that is not a call this administrator can make
//! for it; the operations under test are still driven over the wire against it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    service_state, RELATIONSHIP_ENABLED,
};

use crate::detector_lifecycle::{call, enabled_detector, new_router, router_over, Router};
use crate::member_association::{
    administrator, create_members, get_members, invite_members, member_of, unprocessed_ids,
    unprocessed_result, DEFAULT_REGION, MEMBERS, MEMBER_NOT_DELETED, NOT_AN_ASSOCIATED_MEMBER,
    NOT_ASSOCIATED,
};

/// An account id this suite records no member for, so the "no record at all" row is
/// not one of [`MEMBERS`].
const ABSENT_ACCOUNT_ID: &str = "606060606060";

/// The relationship a member account is in when an operation names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relationship {
    /// The administrator holds no member record for the account at all.
    Absent,
    /// Recorded by `CreateMembers` and not invited yet: `Created`.
    Recorded,
    /// Invited and not answered by the member account: `Invited`.
    Invited,
    /// An association a disassociation ended: `Removed`.
    Disassociated,
}

impl Relationship {
    /// The `relationshipStatus` a read reports for a member in this relationship, or
    /// `None` when there is no record to read.
    fn status(self) -> Option<&'static str> {
        match self {
            Self::Absent => None,
            Self::Recorded => Some("Created"),
            Self::Invited => Some("Invited"),
            Self::Disassociated => Some("Removed"),
        }
    }

    /// The member requests that drive a freshly recorded account into it, each a path
    /// segment under `/detector/{DetectorId}/member`
    /// (`smithy_query(operation="InviteMembers,DisassociateMembers")`).
    fn steps(self) -> &'static [&'static str] {
        match self {
            Self::Absent | Self::Recorded => &[],
            Self::Invited => &["invite"],
            Self::Disassociated => &["invite", "disassociate"],
        }
    }

    /// Is the account an ASSOCIATED member of the administrator here?
    fn associated(self) -> bool {
        self == Self::Invited
    }

    /// Is GuardDuty MONITORING the account under the administrator here? None of
    /// these relationships is: the monitoring begins with the member ACCOUNT's own
    /// answer to the invitation.
    fn monitored(self) -> bool {
        false
    }
}

/// Every relationship, with why it is a row. `Enabled` and `Disabled` are not among
/// them: GuardDuty reaches them only once the member ACCOUNT has accepted the
/// administrator's invitation, which is that account's own call to make and not one
/// this administrator can send.
const RELATIONSHIPS: [(Relationship, &str); 4] = [
    (Relationship::Absent, "an account with no member record"),
    (Relationship::Recorded, "recorded and not invited yet"),
    (Relationship::Invited, "invited and not answered"),
    (
        Relationship::Disassociated,
        "an association a disassociation ended",
    ),
];

/// What a member operation acts on, which is what decides the accounts it processes.
#[derive(Debug, Clone, Copy)]
enum ActsOn {
    /// The ASSOCIATION itself, which an invitation begins and a disassociation ends.
    Association,
    /// The MONITORING the member account's own acceptance starts — including the
    /// member account's DETECTOR, which GuardDuty maintains only while it monitors
    /// that account under the administrator.
    Monitoring,
    /// The member RECORD, which is what a delete drops.
    Record,
}

/// One operation of the class: its path segment under
/// `/detector/{DetectorId}/member`, the extra request members it carries, what it
/// acts on, and the reason it reports an account it did not process with.
struct MemberOperation {
    name: &'static str,
    path: &'static str,
    body: fn() -> Value,
    acts_on: ActsOn,
    reason: &'static str,
}

/// No request member beyond `AccountIds`.
fn no_extra_members() -> Value {
    Value::Null
}

/// The one `Features` switch `UpdateMemberDetectors` carries here; the configuration
/// itself is the subject of its own suite.
fn one_feature() -> Value {
    json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]})
}

/// Every operation that takes an `AccountIds` list and answers
/// `UnprocessedAccounts`, with the relationship it acts on.
const OPERATIONS: [MemberOperation; 5] = [
    MemberOperation {
        name: "DisassociateMembers",
        path: "disassociate",
        body: no_extra_members,
        acts_on: ActsOn::Association,
        reason: NOT_ASSOCIATED,
    },
    MemberOperation {
        name: "StartMonitoringMembers",
        path: "start",
        body: no_extra_members,
        acts_on: ActsOn::Monitoring,
        reason: NOT_AN_ASSOCIATED_MEMBER,
    },
    MemberOperation {
        name: "StopMonitoringMembers",
        path: "stop",
        body: no_extra_members,
        acts_on: ActsOn::Monitoring,
        reason: NOT_AN_ASSOCIATED_MEMBER,
    },
    MemberOperation {
        name: "GetMemberDetectors",
        path: "detector/get",
        body: no_extra_members,
        acts_on: ActsOn::Monitoring,
        reason: NOT_AN_ASSOCIATED_MEMBER,
    },
    MemberOperation {
        name: "UpdateMemberDetectors",
        path: "detector/update",
        body: one_feature,
        acts_on: ActsOn::Association,
        reason: NOT_AN_ASSOCIATED_MEMBER,
    },
];

/// `DeleteMembers`, which acts on the RECORD and is driven on its own because it
/// drops the record the other rows read back.
const DELETE: MemberOperation = MemberOperation {
    name: "DeleteMembers",
    path: "delete",
    body: no_extra_members,
    acts_on: ActsOn::Record,
    reason: MEMBER_NOT_DELETED,
};

/// Does `operation` process an account in `relationship`?
fn processes(operation: &MemberOperation, relationship: Relationship) -> bool {
    match operation.acts_on {
        ActsOn::Association => relationship.associated(),
        // The monitoring begins with the member account's own answer, so no
        // relationship a request to this administrator can reach is one it can act
        // on; [`a_monitored_member_is_the_one_the_monitoring_calls_move`] drives that
        // relationship through the state seam instead.
        ActsOn::Monitoring => false,
        // Deleting removes the details a disassociation retains, so it drops every
        // record except the one of a member GuardDuty monitors — and none of the
        // relationships above is a monitored one.
        ActsOn::Record => relationship.status().is_some() && !relationship.monitored(),
    }
}

/// Send one member operation naming `account_id` alone.
async fn send(
    router: &Router,
    operation: &MemberOperation,
    detector_id: &str,
    account_id: &str,
) -> (u16, Value) {
    let mut body = json!({"accountIds": [account_id]});
    for (key, value) in (operation.body)().as_object().into_iter().flatten() {
        body[key.as_str()] = value.clone();
    }
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/{}", operation.path),
        Some(body),
    )
    .await
}

/// Build a detector whose one member account — [`MEMBERS`]`[0]`, or
/// [`ABSENT_ACCOUNT_ID`] when the row holds no record — is in `relationship`,
/// returning `(detector_id, account_id)`.
async fn detector_with(router: &Router, relationship: Relationship) -> (String, &'static str) {
    let detector_id = enabled_detector(router).await;
    if relationship == Relationship::Absent {
        return (detector_id, ABSENT_ACCOUNT_ID);
    }
    let (account_id, email) = MEMBERS[0];
    let (status, body) = create_members(router, &detector_id, &[(account_id, email)]).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    for step in relationship.steps() {
        let (status, body) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/member/{step}"),
            Some(json!({"accountIds": [account_id]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "member/{step}: expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "member/{step}: expected the member processed, got {body:?}"
        );
    }
    (detector_id, account_id)
}

/// The member record `GetMembers` reports for `account_id`, or `None`.
async fn stored(router: &Router, detector_id: &str, account_id: &str) -> Option<Value> {
    let (status, body) = get_members(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "get-members: expected 200, got {status} {body:?}"
    );
    member_of(&body, account_id)
}

/// Put `account_id`'s member record into the MONITORED relationship, through this
/// service's own state module — the store the handlers read.
///
/// The seam that relationship needs: GuardDuty reaches it when the member ACCOUNT
/// answers the administrator's invitation, which is that account's own call to make
/// and not one this emulator can be asked to make on its behalf — while every call
/// under test below is one the administrator itself sends. The same seam the
/// free-trial suite uses to age a trial window no test can wait out.
///
/// Shared with the monitoring suites, so the one relationship no administrator
/// request can reach is driven into the store from ONE place.
pub(crate) fn monitor_member(
    engine: &engine::InMemoryEngine,
    detector_id: &str,
    account_id: &str,
) {
    monitor_member_in(engine, DEFAULT_REGION, detector_id, account_id);
}

/// [`monitor_member`] for a detector another Region's endpoint serves, so a suite that
/// spans Regions drives the relationship through the same one place.
pub(crate) fn monitor_member_in(
    engine: &engine::InMemoryEngine,
    region: &str,
    detector_id: &str,
    account_id: &str,
) {
    let state = service_state(engine).expect("this service's state");
    let mut state = state.lock();
    let detector = state
        .detector_in_mut(region, detector_id)
        .expect("the detector the create minted");
    let member = detector
        .members
        .get_mut(account_id)
        .expect("the member the create recorded");
    member.relationship_status = RELATIONSHIP_ENABLED.to_string();
}

/// An enabled detector over an engine the test keeps a handle on, holding one member
/// account GuardDuty MONITORS under the administrator.
async fn detector_with_a_monitored_member() -> (Router, &'static str, String) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    let (account_id, email) = MEMBERS[0];
    let (status, body) = create_members(&router, &detector_id, &[(account_id, email)]).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    monitor_member(&engine, &detector_id, account_id);
    (router, account_id, detector_id)
}

/// The `relationshipStatus` a read reports for `account_id`.
async fn relationship_of(router: &Router, detector_id: &str, account_id: &str) -> String {
    let member = stored(router, detector_id, account_id)
        .await
        .unwrap_or_else(|| panic!("case {account_id}: expected a member record"));
    member["relationshipStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {account_id}: expected a relationshipStatus, got {member:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The matrix: every operation against every relationship.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_operation_processes_only_the_relationship_it_acts_on() {
    for (relationship, why) in RELATIONSHIPS {
        for operation in &OPERATIONS {
            let case = format!("{} on {why}", operation.name);
            // One store per cell: each cell drives one account into one relationship
            // and must not see what another cell did.
            let router = new_router();
            let (detector_id, account_id) = detector_with(&router, relationship).await;
            let before = stored(&router, &detector_id, account_id).await;
            assert_eq!(
                before
                    .as_ref()
                    .and_then(|member| member["relationshipStatus"].as_str()),
                relationship.status(),
                "case {case}: expected the row's relationship, got {before:?}"
            );

            let (status, body) = send(&router, operation, &detector_id, account_id).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {body:?}"
            );

            if processes(operation, relationship) {
                assert!(
                    unprocessed_ids(&body).is_empty(),
                    "case {case}: expected the account processed, got {body:?}"
                );
                continue;
            }

            assert_eq!(
                unprocessed_ids(&body),
                vec![account_id.to_string()],
                "case {case}: expected exactly that account reported, got {body:?}"
            );
            assert_eq!(
                unprocessed_result(&body, account_id).as_deref(),
                Some(operation.reason),
                "case {case}: expected the reason it was not processed, got {body:?}"
            );
            // A reported account is left exactly as it was: no relationship is
            // written for an account the call did not process.
            let after = stored(&router, &detector_id, account_id).await;
            assert_eq!(
                after, before,
                "case {case}: expected nothing to change, got {after:?} after {before:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_drops_every_record_the_administrator_must_not_keep() {
    for (relationship, why) in RELATIONSHIPS {
        let case = format!("{} on {why}", DELETE.name);
        let router = new_router();
        let (detector_id, account_id) = detector_with(&router, relationship).await;

        let (status, body) = send(&router, &DELETE, &detector_id, account_id).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body:?}"
        );

        if processes(&DELETE, relationship) {
            assert!(
                unprocessed_ids(&body).is_empty(),
                "case {case}: expected the record dropped, got {body:?}"
            );
            // The details are GONE: the read no longer reports the account at all.
            let after = stored(&router, &detector_id, account_id).await;
            assert!(
                after.is_none(),
                "case {case}: expected no member record left, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            unprocessed_ids(&body),
            vec![account_id.to_string()],
            "case {case}: expected exactly that account reported, got {body:?}"
        );
        assert_eq!(
            unprocessed_result(&body, account_id).as_deref(),
            Some(DELETE.reason),
            "case {case}: expected the reason it was not processed, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The reasons are fixed text, not the administrator's account number.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reported_account_carries_the_fixed_reason_and_no_account_id() {
    let administrator = administrator();
    let mut reported = 0_usize;

    for (relationship, why) in RELATIONSHIPS {
        for operation in OPERATIONS.iter().chain(std::iter::once(&DELETE)) {
            if processes(operation, relationship) {
                continue;
            }
            let case = format!("{} on {why}", operation.name);
            let router = new_router();
            let (detector_id, account_id) = detector_with(&router, relationship).await;
            let (status, body) = send(&router, operation, &detector_id, account_id).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {body:?}"
            );
            let result = unprocessed_result(&body, account_id)
                .unwrap_or_else(|| panic!("case {case}: expected a reported account, got {body:?}"));
            assert_eq!(
                result, operation.reason,
                "case {case}: expected GuardDuty's fixed reason, got {result:?}"
            );
            // The administrator is "the current account", never its account number:
            // the sentence is the same whatever account the service answers for.
            assert!(
                !result.contains(&administrator),
                "case {case}: expected no account id in the reason, got {result:?}"
            );
            assert!(
                result.contains("the current account"),
                "case {case}: expected the administrator named as the current account, got {result:?}"
            );
            reported += 1;
        }
    }

    // The matrix really did report something: every row above has at least one
    // operation that could not process it.
    assert!(
        reported >= RELATIONSHIPS.len(),
        "expected every relationship to be reported by at least one operation, got {reported} reports"
    );
}

// ---------------------------------------------------------------------------
// The record a reporting operation reads is a different fact from the
// association.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_members_reports_the_record_whatever_relationship_it_carries() {
    for (relationship, why) in RELATIONSHIPS {
        let case = format!("GetMembers on {why}");
        let router = new_router();
        let (detector_id, account_id) = detector_with(&router, relationship).await;

        let (status, body) = get_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body:?}"
        );
        match relationship.status() {
            // The RECORD is what this read answers, so a member that is not an
            // associated member of the administrator is still reported, with the
            // relationship the store holds for it.
            Some(expected) => {
                let member = member_of(&body, account_id).unwrap_or_else(|| {
                    panic!("case {case}: expected the record reported, got {body:?}")
                });
                assert_eq!(
                    member["relationshipStatus"].as_str(),
                    Some(expected),
                    "case {case}: expected relationshipStatus {expected}, got {member:?}"
                );
                assert!(
                    unprocessed_ids(&body).is_empty(),
                    "case {case}: expected nothing reported unprocessed, got {body:?}"
                );
            }
            // Only an account it holds no record for is reported here.
            None => {
                assert!(
                    member_of(&body, account_id).is_none(),
                    "case {case}: expected no member reported, got {body:?}"
                );
                assert_eq!(
                    unprocessed_result(&body, account_id).as_deref(),
                    Some(NOT_AN_ASSOCIATED_MEMBER),
                    "case {case}: expected the not-an-associated-member reason, got {body:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A batch answers per account, not per call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_processes_and_reports_the_accounts_of_a_mixed_batch() {
    // Three accounts in three relationships, named in one call: the invited one is
    // the only association `DisassociateMembers` can end, so the other two come back
    // reported while it is processed.
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (invited, invited_email) = MEMBERS[0];
    let (recorded, recorded_email) = MEMBERS[1];
    let (status, body) = create_members(
        &router,
        &detector_id,
        &[(invited, invited_email), (recorded, recorded_email)],
    )
    .await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    let (status, body) = invite_members(&router, &detector_id, &[invited]).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );

    let requested = [recorded, invited, ABSENT_ACCOUNT_ID, recorded];
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/member/disassociate"),
        Some(json!({"accountIds": requested})),
    )
    .await;
    assert_eq!(
        status, 200,
        "mixed disassociate: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![recorded.to_string(), ABSENT_ACCOUNT_ID.to_string()],
        "mixed disassociate: expected each unprocessed account once, in request order, got {body:?}"
    );
    for account_id in [recorded, ABSENT_ACCOUNT_ID] {
        assert_eq!(
            unprocessed_result(&body, account_id).as_deref(),
            Some(NOT_ASSOCIATED),
            "case {account_id}: expected the not-associated reason, got {body:?}"
        );
    }

    let association = stored(&router, &detector_id, invited)
        .await
        .unwrap_or_else(|| panic!("case {invited}: expected a retained record"));
    assert_eq!(
        association["relationshipStatus"].as_str(),
        Some("Removed"),
        "case {invited}: expected the association ended, got {association:?}"
    );
    let untouched = stored(&router, &detector_id, recorded)
        .await
        .unwrap_or_else(|| panic!("case {recorded}: expected a retained record"));
    assert_eq!(
        untouched["relationshipStatus"].as_str(),
        Some("Created"),
        "case {recorded}: expected the reported account unchanged, got {untouched:?}"
    );
}

// ---------------------------------------------------------------------------
// The other side of the rule: the relationship the monitoring calls DO act on.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_monitoring_calls_move_a_member_guardduty_monitors() {
    let (router, account_id, detector_id) = detector_with_a_monitored_member().await;
    assert_eq!(
        relationship_of(&router, &detector_id, account_id).await,
        "Enabled",
        "case {account_id}: expected a monitored member to start monitored"
    );

    // Stopping and restarting are inverses over the one stored relationship, and
    // each is a DESIRED STATE: the second call of a pair processes the member and
    // changes nothing at all.
    for (path, moved_to) in [("stop", "Disabled"), ("start", "Enabled")] {
        let before = stored(&router, &detector_id, account_id).await;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/member/{path}"),
            Some(json!({"accountIds": [account_id]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {path}: expected a monitored member processed, got {body:?}"
        );
        let after = stored(&router, &detector_id, account_id).await;
        assert_eq!(
            after
                .as_ref()
                .and_then(|member| member["relationshipStatus"].as_str()),
            Some(moved_to),
            "case {path}: expected the relationship to move to {moved_to}, got {after:?}"
        );
        assert_ne!(
            after, before,
            "case {path}: expected the member to move, got {after:?}"
        );

        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/member/{path}"),
            Some(json!({"accountIds": [account_id]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {path} repeated: expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {path} repeated: expected the member still processed, got {body:?}"
        );
        let again = stored(&router, &detector_id, account_id).await;
        assert_eq!(
            again, after,
            "case {path} repeated: expected nothing to change, got {again:?} after {after:?}"
        );
    }

    // A member GuardDuty monitors is the one whose detector the administrator
    // maintains, so the member-detector READ reports its configuration.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/member/detector/get"),
        Some(json!({"accountIds": [account_id]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "get-member-detectors: expected 200, got {status} {body:?}"
    );
    assert!(
        member_of(&body, account_id).is_some(),
        "case {account_id}: expected a monitored member's configuration, got {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {account_id}: expected nothing reported for a monitored member, got {body:?}"
    );
}

#[tokio::test]
async fn deleting_reports_a_member_guardduty_monitors_until_it_is_disassociated() {
    let (router, account_id, detector_id) = detector_with_a_monitored_member().await;
    let administrator = administrator();
    let before = stored(&router, &detector_id, account_id).await;

    // The record a delete does NOT drop: the member has to be disassociated first,
    // which is what the reason states.
    let (status, body) = send(&router, &DELETE, &detector_id, account_id).await;
    assert_eq!(
        status, 200,
        "delete a monitored member: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![account_id.to_string()],
        "delete a monitored member: expected the account reported, got {body:?}"
    );
    let result = unprocessed_result(&body, account_id)
        .unwrap_or_else(|| panic!("delete a monitored member: expected a reason, got {body:?}"));
    assert_eq!(
        result, MEMBER_NOT_DELETED,
        "delete a monitored member: expected the reason the record was not dropped, got {result:?}"
    );
    assert!(
        !result.contains(&administrator),
        "delete a monitored member: expected no account id in the reason, got {result:?}"
    );
    let after = stored(&router, &detector_id, account_id).await;
    assert_eq!(
        after, before,
        "delete a monitored member: expected the record kept unchanged, got {after:?} after {before:?}"
    );

    // Ending the association is what makes the record droppable.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/member/disassociate"),
        Some(json!({"accountIds": [account_id]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "disassociate: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "disassociate: expected a monitored member processed, got {body:?}"
    );
    assert_eq!(
        relationship_of(&router, &detector_id, account_id).await,
        "Removed",
        "case {account_id}: expected the association ended"
    );

    let (status, body) = send(&router, &DELETE, &detector_id, account_id).await;
    assert_eq!(
        status, 200,
        "delete a disassociated member: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "delete a disassociated member: expected the record dropped, got {body:?}"
    );
    assert!(
        stored(&router, &detector_id, account_id).await.is_none(),
        "case {account_id}: expected no member record left"
    );
}
