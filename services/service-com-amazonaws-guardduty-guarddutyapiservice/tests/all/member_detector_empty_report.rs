// SPDX-License-Identifier: CC0-1.0

//! What `GetMemberDetectors` answers for a request whose accounts it cannot report a
//! configuration for.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler together.
//!
//! The rule under test, asserted over a TABLE of requests rather than one call:
//!
//! `GetMemberDetectors` reports "which data sources are enabled for the member
//! account's detector" (`smithy_query(query="doc",shape="GetMemberDetectors")`), and
//! GuardDuty maintains a detector for a member account only once that account has
//! ANSWERED the administrator's invitation and is monitored under it. So the operation
//! PARTITIONS the distinct account ids a request names: an account whose detector the
//! administrator monitors gets a `members` configuration, and every other account —
//! one it holds no member record for, its own, one recorded and never invited, one
//! whose invitation is still unanswered, and one a disassociation moved — gets an
//! `unprocessedAccounts` row stating the reason, with no `members` entry at all. The
//! call answers HTTP 200 either way, including when the partition puts NO account on
//! the `members` side, where the list is present and empty. The partition is total and
//! exclusive (each distinct id appears on exactly one of the two lists, once, in
//! request order), which is what makes the empty `members` list the answer rather than
//! a missing one: an account whose detector this administrator does not maintain has
//! no configuration to report, and inventing one would report data sources and
//! features for an account that runs no detector under it.
//!
//! `GetMemberDetectorsResponse.MemberDataSourceConfigurations` is modelled
//! `len 1..50` (`smithy_query(shape="MemberDataSourceConfigurations")`) while its
//! `GetMembers` counterpart `Members` is `len 0..50`, but that bound is a declared
//! constraint on the shape's values, not a floor the service refuses a request to stay
//! above: GuardDuty answers this request 200 with no configuration and one unprocessed
//! row per account, so these tests pin the 200-with-empty-`members` answer as the
//! deliberate one.
//!
//! A read is also a read: repeating the call answers the same body, and an
//! unreportable account changes nothing a later caller sees.

use std::sync::Arc;

use serde_json::Value;

use crate::detector_lifecycle::{enabled_detector, router_over, Router};
// The member-suite table, helpers and projections live in one place, so the member
// suites cannot drift on what a non-member account id is.
use crate::member_association::{
    administrator, create_members, disassociate_members, invite_members, unprocessed_ids,
    unprocessed_result, MEMBERS, NON_MEMBER_ACCOUNT_IDS, NOT_AN_ASSOCIATED_MEMBER,
};
use crate::member_detector_configuration::get_member_detectors;
use crate::member_relationship_scope::monitor_member;

/// A well-formed account id the fixture records as a member and never invites, so it
/// has a member record and no association.
const RECORDED_ONLY: &str = "707070707070";

/// The account ids `members` reports a configuration for, in the order the response
/// lists them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Is `members` present as a LIST at all? The member is modelled `required`, so the
/// answer carries the list even when the partition puts nothing on its side.
fn members_present(body: &Value) -> bool {
    body.get("members").is_some_and(Value::is_array)
}

/// An enabled detector whose members span every class the partition distinguishes:
/// [`MEMBERS`]`[0]` has ANSWERED its invitation, so the administrator monitors its
/// detector; `[1]` is invited and has not answered; `[2]` is invited and then
/// disassociated; [`RECORDED_ONLY`] is recorded and never invited.
///
/// Answering an invitation is the member ACCOUNT's own call to make, so the monitored
/// relationship is driven through the one state seam the
/// `member_relationship_scope` suite owns — which is why this fixture keeps the engine
/// handle and hands back the router with the detector.
async fn detector_with_every_member_class() -> (Router, String) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;

    let (status, body) = create_members(&router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "fixture create-members: expected 200, got {status} {body:?}"
    );
    let ids: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();
    let (status, body) = invite_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "fixture invite-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "fixture invite-members: expected every account processed, got {body:?}"
    );

    monitor_member(&engine, &detector_id, MEMBERS[0].0);

    let (disassociated, _) = MEMBERS[2];
    let (status, body) = disassociate_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(
        status, 200,
        "fixture disassociate {disassociated}: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "fixture disassociate {disassociated}: expected an invited member processed, got {body:?}"
    );

    let (status, body) =
        create_members(&router, &detector_id, &[(RECORDED_ONLY, "recorded@example.com")]).await;
    assert_eq!(
        status, 200,
        "fixture create {RECORDED_ONLY}: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "fixture create {RECORDED_ONLY}: expected the account recorded, got {body:?}"
    );

    (router, detector_id)
}

#[tokio::test]
async fn get_member_detectors_partitions_the_requested_accounts_and_answers_200_with_an_empty_members_list() {
    let (router, detector_id) = detector_with_every_member_class().await;
    let administrator = administrator();
    let administrator = administrator.as_str();
    let monitored = MEMBERS[0].0;
    let invited = MEMBERS[1].0;
    let disassociated = MEMBERS[2].0;

    // Rows differing along the COUNT of accounts one call names, along the CLASS of
    // account that cannot be reported (an id nothing recorded, at either end of the
    // account-id space and beside a recorded member; the administrator's own account;
    // a record never invited; a record whose invitation is unanswered; a record a
    // disassociation moved), along the ORDER the request names them in, and along
    // whether an id is repeated.
    //
    // `NON_MEMBER_ACCOUNT_IDS[0]` — the low end of the account-id space — IS the
    // account this emulator serves, so it is the administrator's own account id and no
    // row names both: that would name one id twice rather than two classes.
    #[allow(clippy::type_complexity)]
    let rows: [(&str, Vec<&str>, Vec<&str>, Vec<&str>); 12] = [
        (
            "one id nothing recorded",
            vec![NON_MEMBER_ACCOUNT_IDS[1].0],
            vec![],
            vec![NON_MEMBER_ACCOUNT_IDS[1].0],
        ),
        (
            "the administrator's own account",
            vec![administrator],
            vec![],
            vec![administrator],
        ),
        (
            "a member recorded and not invited yet",
            vec![RECORDED_ONLY],
            vec![],
            vec![RECORDED_ONLY],
        ),
        (
            "a member whose invitation is still unanswered",
            vec![invited],
            vec![],
            vec![invited],
        ),
        (
            "a member a disassociation already moved",
            vec![disassociated],
            vec![],
            vec![disassociated],
        ),
        (
            "every id nothing recorded, together",
            NON_MEMBER_ACCOUNT_IDS.iter().map(|(id, _)| *id).collect(),
            vec![],
            NON_MEMBER_ACCOUNT_IDS.iter().map(|(id, _)| *id).collect(),
        ),
        (
            "one of every unreportable class, together",
            vec![
                NON_MEMBER_ACCOUNT_IDS[1].0,
                administrator,
                RECORDED_ONLY,
                invited,
                disassociated,
            ],
            vec![],
            vec![
                NON_MEMBER_ACCOUNT_IDS[1].0,
                administrator,
                RECORDED_ONLY,
                invited,
                disassociated,
            ],
        ),
        (
            "the same unreportable id twice",
            vec![NON_MEMBER_ACCOUNT_IDS[2].0, NON_MEMBER_ACCOUNT_IDS[2].0],
            vec![],
            vec![NON_MEMBER_ACCOUNT_IDS[2].0],
        ),
        (
            "one monitored member alone",
            vec![monitored],
            vec![monitored],
            vec![],
        ),
        (
            "an unreportable account before a monitored member",
            vec![NON_MEMBER_ACCOUNT_IDS[3].0, monitored],
            vec![monitored],
            vec![NON_MEMBER_ACCOUNT_IDS[3].0],
        ),
        (
            "a monitored member interleaved with unreportable accounts and repeats",
            vec![
                invited,
                NON_MEMBER_ACCOUNT_IDS[1].0,
                monitored,
                invited,
                administrator,
            ],
            vec![monitored],
            vec![invited, NON_MEMBER_ACCOUNT_IDS[1].0, administrator],
        ),
        (
            "the monitored member twice",
            vec![monitored, monitored],
            vec![monitored],
            vec![],
        ),
    ];

    for (case, requested, expected_members, expected_unprocessed) in rows {
        let (status, body) = get_member_detectors(&router, &detector_id, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?} {requested:?}: expected 200, got {status} {body:?}"
        );
        assert!(
            members_present(&body),
            "case {case:?} {requested:?}: expected the modelled members list present, got {body:?}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_members
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<String>>(),
            "case {case:?} {requested:?}: expected exactly the monitored members reported, \
             once each in request order, got {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            expected_unprocessed
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<String>>(),
            "case {case:?} {requested:?}: expected exactly the accounts with no configuration \
             reported unprocessed, once each in request order, got {body:?}"
        );
        for account_id in &expected_unprocessed {
            assert_eq!(
                unprocessed_result(&body, account_id).as_deref(),
                Some(NOT_AN_ASSOCIATED_MEMBER),
                "case {case:?} {account_id}: expected the not-an-associated-member reason, \
                 got {body:?}"
            );
        }
        // The partition is total: every DISTINCT id the request named is on one of the
        // two lists, so an empty `members` list is accounted for by the unprocessed
        // rows rather than by a dropped account.
        let mut distinct = requested.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            reported_ids(&body).len() + unprocessed_ids(&body).len(),
            distinct.len(),
            "case {case:?} {requested:?}: expected every distinct account on exactly one list, \
             got {body:?}"
        );
        // A reported configuration is the stored one, not a placeholder: both views of
        // the member's switches are there.
        for entry in body["members"].as_array().into_iter().flatten() {
            assert!(
                entry["dataSources"].is_object() && entry["features"].is_array(),
                "case {case:?}: expected a reported member to carry both configuration views, \
                 got {entry:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_member_report_changes_nothing_a_later_caller_sees() {
    let (router, detector_id) = detector_with_every_member_class().await;
    let monitored = MEMBERS[0].0;

    let (status, before) = get_member_detectors(&router, &detector_id, &[monitored]).await;
    assert_eq!(
        status, 200,
        "baseline read: expected 200, got {status} {before:?}"
    );

    // Every class of unreportable account, read twice: the answer is stable, which is
    // what a report has to be. `NON_MEMBER_ACCOUNT_IDS[0]` is the account this emulator
    // serves, so the administrator's own account id is one of these rows.
    let unreportable: Vec<&str> = NON_MEMBER_ACCOUNT_IDS
        .iter()
        .map(|(id, _)| *id)
        .chain([RECORDED_ONLY, MEMBERS[1].0, MEMBERS[2].0])
        .collect();
    for account_id in unreportable {
        let (status, first) = get_member_detectors(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id}: expected 200, got {status} {first:?}"
        );
        let (status, second) = get_member_detectors(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id}: expected 200 on the repeat, got {status} {second:?}"
        );
        assert_eq!(
            second, first,
            "case {account_id}: expected the repeated report to answer the same body, \
             got {second:?} after {first:?}"
        );
        assert!(
            reported_ids(&first).is_empty(),
            "case {account_id}: expected no configuration reported, got {first:?}"
        );
    }

    let (status, after) = get_member_detectors(&router, &detector_id, &[monitored]).await;
    assert_eq!(
        status, 200,
        "read after the empty reports: expected 200, got {status} {after:?}"
    );
    assert_eq!(
        after, before,
        "expected the monitored member's configuration unchanged by the empty reports, \
         got {after:?} after {before:?}"
    );
}
