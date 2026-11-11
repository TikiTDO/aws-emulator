// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteMember`.
//!
//! The rule under test: removing a party from a collaboration takes that party's
//! authorization to the collaboration away without taking its grant out of the
//! collaboration — the member listing keeps reporting it, in `REMOVED` status, with its
//! own `updateTime` moved to the removal, and the membership it had joined with survives
//! in the `REMOVED` membership status. Only the collaboration's creator may remove a
//! party, the creator itself is the one party no removal can take out, and an account the
//! collaboration holds no grant for is not found; a repeat removal is the same success,
//! because the operation is `@idempotent`.
//!
//! The rows vary what the rule generalises over: how many parties the collaboration
//! declares, which of them is removed (first, middle, last, only), which abilities that
//! party holds (the whole `MemberAbility` catalog), whether it has joined with a
//! membership, how many times the removal is repeated, and — for the refusals — why the
//! caller is not the one who may remove (not the creator, no party at all, an id nothing
//! is stored under) and which account the request names. Every request goes over the wire
//! through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, minimal_collaboration,
    Emulator, ALL_LOG_STATUSES, ALL_MEMBER_ABILITIES, REGION,
};
use super::collaboration_change_requests::{declared_member, NEWCOMER_ACCOUNT, PARTNER_ACCOUNT};

/// A further 12-digit `AccountId` (model shape `AccountId`, len 12..12, pattern `^\d+$`),
/// so a collaboration in these tables can declare three parties beside its creator.
const THIRD_ACCOUNT: &str = "555566667777";

/// 12-digit `AccountId` values no collaboration in these tables declares.
const NEVER_A_PARTY: [&str; 2] = ["999988887777", "123456789012"];

/// A well-formed `CollaborationIdentifier` (the model's `UUID` form) that no create here
/// mints.
const NEVER_MINTED: &str = "0123456f-89ab-cdef-0123-456789abcdef";

/// The message the authorization-failure rule prescribes for a `DeleteMember` the caller
/// may not perform: the calling principal, this service's IAM action and the ARN of the
/// collaboration the decision was taken on.
fn denial(collaboration_id: &str) -> String {
    format!(
        "User: arn:aws:iam::{account}:user/emulator is not authorized to perform: \
         cleanrooms:DeleteMember on resource: \
         arn:aws:cleanrooms:{REGION}:{account}:collaboration/{collaboration_id}",
        account = account_id()
    )
}

/// `DeleteMember` over the wire.
async fn delete_member(emulator: &Emulator, collaboration_id: &str, account: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!("/collaborations/{collaboration_id}/member/{account}"),
            None,
        )
        .await
}

/// The `memberSummaries` a `ListMembers` call reports, which must succeed.
async fn roster(emulator: &Emulator, case: &str, collaboration_id: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("GET", &format!("/collaborations/{collaboration_id}/members"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListMembers 200, got {status} with {body}"
    );
    body["memberSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected memberSummaries, got {body}"))
        .clone()
}

/// One party's summary in a roster.
fn summary_of(case: &str, roster: &[Value], account: &str) -> Value {
    roster
        .iter()
        .find(|summary| summary["accountId"].as_str() == Some(account))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for {account}, got {roster:?}")
        })
        .clone()
}

/// One timestamp member of a summary, as the epoch seconds restJson1 serializes.
fn timestamp(case: &str, summary: &Value, member: &str) -> f64 {
    summary[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an epoch-seconds {member}, got {summary}")
    })
}

/// Create a collaboration whose creator is the caller, declaring `members`.
async fn collaboration_with(
    emulator: &Emulator,
    case: &str,
    creator_abilities: &[&str],
    members: Value,
) -> String {
    let mut body = minimal_collaboration(case);
    body["creatorMemberAbilities"] = json!(creator_abilities);
    body["members"] = members;
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created collaboration carries an id"))
        .to_string()
}

/// Give `account` a membership of `collaboration_id` and return its id.
///
/// Every emulated caller presents one account, so a membership created over the wire is
/// the CALLER's; the `(collaboration, account)` index entry — the one thing that says
/// whose membership a stored membership is — is then re-keyed onto `account`, the same
/// store surgery `collaboration_owned_elsewhere` does to give a collaboration another
/// creator. The handler under test reads that index, so the removal cascades exactly as
/// it would for a party that had joined itself.
async fn membership_for(emulator: &Emulator, case: &str, collaboration_id: &str, account: &str) -> String {
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created membership carries an id"))
        .to_string();
    {
        let store = emulator.store();
        let mut store = store.lock();
        store
            .membership_by_collaboration
            .remove(&(collaboration_id.to_string(), account_id().to_string()));
        store.membership_by_collaboration.insert(
            (collaboration_id.to_string(), account.to_string()),
            membership_id.clone(),
        );
    }
    membership_id
}

/// One row of the removal table.
struct RemovalCase {
    label: &'static str,
    /// The parties the collaboration declares beside its creator, in declaration order.
    members: Vec<&'static str>,
    /// The abilities every declared party is granted.
    abilities: Vec<&'static str>,
    /// The party the row removes.
    remove: &'static str,
    /// Whether that party has joined with a membership.
    joined: bool,
    /// How many times the row calls the removal.
    calls: usize,
}

fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            label: "the only declared party, not joined",
            members: vec![PARTNER_ACCOUNT],
            abilities: vec![ALL_MEMBER_ABILITIES[0]],
            remove: PARTNER_ACCOUNT,
            joined: false,
            calls: 1,
        },
        RemovalCase {
            label: "the first of three declared parties",
            members: vec![PARTNER_ACCOUNT, NEWCOMER_ACCOUNT, THIRD_ACCOUNT],
            abilities: ALL_MEMBER_ABILITIES.to_vec(),
            remove: PARTNER_ACCOUNT,
            joined: false,
            calls: 1,
        },
        RemovalCase {
            label: "the middle of three declared parties, joined",
            members: vec![PARTNER_ACCOUNT, NEWCOMER_ACCOUNT, THIRD_ACCOUNT],
            abilities: vec![ALL_MEMBER_ABILITIES[1], ALL_MEMBER_ABILITIES[3]],
            remove: NEWCOMER_ACCOUNT,
            joined: true,
            calls: 1,
        },
        RemovalCase {
            label: "the last of three declared parties",
            members: vec![PARTNER_ACCOUNT, NEWCOMER_ACCOUNT, THIRD_ACCOUNT],
            abilities: vec![ALL_MEMBER_ABILITIES[2]],
            remove: THIRD_ACCOUNT,
            joined: false,
            calls: 3,
        },
        RemovalCase {
            label: "the only declared party, joined, every ability, removed twice",
            members: vec![PARTNER_ACCOUNT],
            abilities: ALL_MEMBER_ABILITIES.to_vec(),
            remove: PARTNER_ACCOUNT,
            joined: true,
            calls: 2,
        },
    ]
}

#[tokio::test]
async fn delete_member_moves_the_party_to_removed_and_keeps_the_rest_of_the_roster() {
    for case in removal_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let members: Vec<Value> = case
            .members
            .iter()
            .map(|account| declared_member(account, &case.abilities))
            .collect();
        let collaboration_id =
            collaboration_with(&emulator, label, &case.abilities, json!(members)).await;
        let membership_id = if case.joined {
            Some(membership_for(&emulator, label, &collaboration_id, case.remove).await)
        } else {
            None
        };

        let before = roster(&emulator, label, &collaboration_id).await;
        let removed_before = summary_of(label, &before, case.remove);
        assert_eq!(
            removed_before["status"].as_str(),
            Some(if case.joined { "ACTIVE" } else { "INVITED" }),
            "case {label:?}: before the removal the party reports the status its membership \
             gives it, got {:?}",
            removed_before["status"]
        );

        // The FIRST call performs the removal and restamps the party's own `updateTime`;
        // every later call is the `@idempotent` repeat, which must change nothing at all.
        let mut first_after: Option<Vec<Value>> = None;
        for call in 1..=case.calls {
            let (status, body) = delete_member(&emulator, &collaboration_id, case.remove).await;
            assert_eq!(
                status, 204,
                "case {label:?} call {call}: expected the modelled 204, got {status} with {body}"
            );
            assert!(
                body.as_object().is_none_or(|members| members.is_empty()),
                "case {label:?} call {call}: the modelled output has no members, got {body}"
            );
            let settled = roster(&emulator, label, &collaboration_id).await;
            match &first_after {
                None => {
                    let removed_now = summary_of(label, &settled, case.remove);
                    assert!(
                        timestamp(label, &removed_now, "updateTime")
                            > timestamp(label, &removed_before, "updateTime"),
                        "case {label:?} call {call}: expected the removed party's updateTime to \
                         move to the removal, got {:?} from {:?}",
                        removed_now["updateTime"],
                        removed_before["updateTime"]
                    );
                    first_after = Some(settled);
                }
                Some(first) => assert_eq!(
                    &settled, first,
                    "case {label:?} call {call}: a repeat removal must change nothing, including \
                     the removed party's updateTime"
                ),
            }
        }

        let after = first_after.expect("every row calls the removal at least once");
        assert_eq!(
            after.len(),
            before.len(),
            "case {label:?}: a removal keeps the party in the roster, got {after:?}"
        );
        let accounts_before: Vec<Option<&str>> =
            before.iter().map(|s| s["accountId"].as_str()).collect();
        let accounts_after: Vec<Option<&str>> =
            after.iter().map(|s| s["accountId"].as_str()).collect();
        assert_eq!(
            accounts_after, accounts_before,
            "case {label:?}: a removal keeps declaration order, got {accounts_after:?}"
        );

        let removed = summary_of(label, &after, case.remove);
        assert_eq!(
            removed["status"].as_str(),
            Some("REMOVED"),
            "case {label:?}: expected the removed party in REMOVED status, got {removed}"
        );
        assert_eq!(
            removed["abilities"], removed_before["abilities"],
            "case {label:?}: a removal keeps the grant the party was declared with, got {:?}",
            removed["abilities"]
        );
        assert_eq!(
            removed["displayName"], removed_before["displayName"],
            "case {label:?}: a removal keeps the party's display name, got {:?}",
            removed["displayName"]
        );
        assert_eq!(
            timestamp(label, &removed, "createTime"),
            timestamp(label, &removed_before, "createTime"),
            "case {label:?}: a removal does not re-create the party's member record"
        );

        // Every party the removal did not name is untouched, creator included.
        for other in after.iter().filter(|summary| {
            summary["accountId"].as_str() != Some(case.remove)
        }) {
            let account = other["accountId"].as_str().unwrap_or_default();
            assert_eq!(
                other,
                &summary_of(label, &before, account),
                "case {label:?}: the removal must change nothing about {account}, got {other}"
            );
        }

        // The membership the removed party had joined with survives, in REMOVED status,
        // and the roster still reports it.
        if let Some(membership_id) = membership_id {
            assert_eq!(
                removed["membershipId"].as_str(),
                Some(membership_id.as_str()),
                "case {label:?}: the roster still reports the removed party's membership, \
                 got {removed}"
            );
            let (status, fetched) = emulator.get_membership(&membership_id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the membership survives the removal, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["membership"]["status"].as_str(),
                Some("REMOVED"),
                "case {label:?}: expected the membership in REMOVED status, got {:?}",
                fetched["membership"]
            );
        }
    }
}

#[tokio::test]
async fn delete_member_refuses_to_remove_the_collaboration_creator() {
    // Rows differ in what the creator holds: the abilities it declared for itself, how
    // many other parties the collaboration has, and whether the creator has joined with
    // a membership — the rule is about which party the request names, not about any of
    // those.
    for (row, (abilities, members, joined)) in [
        (vec![ALL_MEMBER_ABILITIES[0]], vec![], false),
        (ALL_MEMBER_ABILITIES.to_vec(), vec![PARTNER_ACCOUNT], true),
        (
            vec![ALL_MEMBER_ABILITIES[1]],
            vec![PARTNER_ACCOUNT, NEWCOMER_ACCOUNT],
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("creator holding {abilities:?} with {} parties", members.len());
        let case = format!("creator-removal-{row}");
        let emulator = Emulator::new();
        let declared: Vec<Value> = members
            .iter()
            .map(|account| declared_member(account, &abilities))
            .collect();
        let collaboration_id =
            collaboration_with(&emulator, &case, &abilities, json!(declared)).await;
        if joined {
            let (status, body) = emulator
                .create_membership(json!({
                    "collaborationIdentifier": collaboration_id,
                    "queryLogStatus": ALL_LOG_STATUSES[0],
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: the creator's join must succeed, got {status} with {body}"
            );
        }

        let before = roster(&emulator, &label, &collaboration_id).await;
        let (status, refused) = delete_member(&emulator, &collaboration_id, account_id()).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected the modelled 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            "Cannot delete member for the creator of the collaboration.",
            "case {label:?}: the refusal states the rule in fixed text, interpolating no \
             account or collaboration id, got {:?}",
            error_message(&refused)
        );
        assert_eq!(
            refused["reason"].as_str(),
            Some("INVALID_STATE"),
            "case {label:?}: expected the modelled reason, got {:?}",
            refused["reason"]
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(collaboration_id.as_str()),
            "case {label:?}: the refusal names the collaboration it is about, got {:?}",
            refused["resourceId"]
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("COLLABORATION"),
            "case {label:?}: expected the collaboration's resource type, got {:?}",
            refused["resourceType"]
        );

        // The refusal changed nothing.
        assert_eq!(
            roster(&emulator, &label, &collaboration_id).await,
            before,
            "case {label:?}: a refused removal must change no party"
        );
    }
}

#[tokio::test]
async fn delete_member_reports_an_account_the_collaboration_holds_no_grant_for_as_not_found() {
    let emulator = Emulator::new();
    let collaboration_id = collaboration_with(
        &emulator,
        "member-not-found",
        &ALL_MEMBER_ABILITIES,
        json!([declared_member(PARTNER_ACCOUNT, &[ALL_MEMBER_ABILITIES[0]])]),
    )
    .await;
    let before = roster(&emulator, "member-not-found", &collaboration_id).await;

    for account in NEVER_A_PARTY {
        let label = format!("account {account} was never declared");
        let (status, refused) = delete_member(&emulator, &collaboration_id, account).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the modelled 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("No such member: {account} in collaboration {collaboration_id}"),
            "case {label:?}: the message names the account and the collaboration whose \
             roster was searched, got {:?}",
            error_message(&refused)
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(account),
            "case {label:?}: the refusal names the account it did not find, got {:?}",
            refused["resourceId"]
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("MEMBER"),
            "case {label:?}: expected the member resource type, got {:?}",
            refused["resourceType"]
        );
        assert_eq!(
            roster(&emulator, &label, &collaboration_id).await,
            before,
            "case {label:?}: a refused removal must change no party"
        );
    }
}

#[tokio::test]
async fn only_the_creator_of_a_collaboration_may_remove_a_party_from_it() {
    let emulator = Emulator::new();
    // Rows: a collaboration another account created that this one PARTICIPATES in, one it
    // is no party to at all, and an id nothing is stored under. The caller is not the
    // creator in any of them, so each is the same authorization failure, which is also
    // what keeps the refusal from saying whether anything is stored under the id.
    let participant =
        collaboration_owned_elsewhere(&emulator, "removal-participant", Some(&ALL_MEMBER_ABILITIES))
            .await;
    let foreign = collaboration_owned_elsewhere(&emulator, "removal-foreign", None).await;

    for (why, collaboration_id, account) in [
        (
            "a collaboration the caller only participates in",
            participant.clone(),
            account_id().to_string(),
        ),
        (
            "a collaboration the caller is no party to",
            foreign.clone(),
            PARTNER_ACCOUNT.to_string(),
        ),
        (
            "an id no create minted",
            NEVER_MINTED.to_string(),
            PARTNER_ACCOUNT.to_string(),
        ),
    ] {
        let (status, refused) = delete_member(&emulator, &collaboration_id, &account).await;
        assert_eq!(
            status, 403,
            "case {why:?}: expected the modelled 403, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {why:?}: expected AccessDeniedException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            denial(&collaboration_id),
            "case {why:?}: expected the authorization-failure wording, got {:?}",
            error_message(&refused)
        );
        assert_eq!(
            refused["reason"].as_str(),
            Some("INSUFFICIENT_PERMISSIONS"),
            "case {why:?}: expected the modelled reason, got {:?}",
            refused["reason"]
        );
    }

    // The refusals removed nobody: the collaboration another account owns still holds the
    // grant it was declared with.
    {
        let store = emulator.store();
        let store = store.lock();
        let record = store
            .collaborations
            .get(&participant)
            .expect("the collaboration the caller participates in");
        assert!(
            !record.is_removed(account_id()),
            "a refused removal must record no removal, got {:?}",
            record.removed_members
        );
    }
}

#[tokio::test]
async fn a_removed_party_can_no_longer_reach_the_collaboration() {
    // The emulator serves ONE caller account, so the only way to call AS the removed
    // party is to apply the removal the handler applies — through the same state mutator
    // — to a collaboration another account created, and then read it over the wire.
    let emulator = Emulator::new();
    let collaboration_id = collaboration_owned_elsewhere(
        &emulator,
        "removed-participant",
        Some(&ALL_MEMBER_ABILITIES),
    )
    .await;
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup: the participant's join must succeed, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .expect("the created membership carries an id")
        .to_string();

    {
        let store = emulator.store();
        let mut store = store.lock();
        assert!(
            store.remove_member(&collaboration_id, account_id(), cleanrooms_state::now()),
            "setup: the participant is a party, so the removal must apply"
        );
    }

    for (label, method, path) in [
        (
            "GetCollaboration",
            "GET",
            format!("/collaborations/{collaboration_id}"),
        ),
        (
            "ListMembers",
            "GET",
            format!("/collaborations/{collaboration_id}/members"),
        ),
    ] {
        let (status, refused) = emulator.call(method, &path, None).await;
        assert_eq!(
            status, 403,
            "case {label:?}: a removed party cannot interact with the collaboration, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {label:?}: expected AccessDeniedException, got {refused}"
        );
    }

    // Its membership survives the removal, in REMOVED status.
    let (status, fetched) = emulator.get_membership(&membership_id).await;
    assert_eq!(
        status, 200,
        "case membership of a removed party: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["membership"]["status"].as_str(),
        Some("REMOVED"),
        "case membership of a removed party: expected REMOVED, got {:?}",
        fetched["membership"]
    );
}
