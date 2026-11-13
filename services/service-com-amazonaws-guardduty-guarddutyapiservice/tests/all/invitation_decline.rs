// SPDX-License-Identifier: CC0-1.0

//! `DeclineInvitations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. Declining ANSWERS the invitation each named account sent: an invited account "can
//!    either accept or decline the invitation"
//!    (`smithy_query(query="doc",shape="InviteMembers")`), so a declined invitation is
//!    no longer pending — it is gone from everything that reads the inbox
//!    (`ListInvitations`, `GetInvitationsCount`) — while the invitations the request
//!    did not name stay there unchanged.
//! 2. An account id the inbox holds no invitation from identifies nothing to decline
//!    and is REPORTED in `UnprocessedAccounts` — the account and a reason — rather than
//!    refusing the call (`smithy_query(operation="DeclineInvitations")` makes
//!    `UnprocessedAccounts` required and `len 0..50`), so a repeat of the same decline
//!    answers 200 with the account reported and the inbox unchanged. A repeated account
//!    id is one account, so it is declined once and reported at most once.
//! 3. The operation names no detector (`POST /invitation/decline`), so the inbox it
//!    answers from is the one the Region the request was signed for serves: another
//!    Region's inbox is untouched, and an endpoint holding no detector reports every
//!    named account rather than failing.
//! 4. Declining is the OTHER answer to an invitation, not acceptance: the declined
//!    invitation cannot be accepted afterwards, and it establishes no administrator
//!    relationship, so `GetAdministratorAccount` still reports none — while accepting
//!    an invitation the same call left pending does establish one.
//!
//! The account-id well-formedness refusal this operation shares with every other
//! operation that takes an `AccountIds` list is asserted in the
//! `account_id_well_formedness` suite, which drives this one too.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), so a row can hold an
//! invitation from any account id in the space rather than only from the one account
//! id this emulator presents, which is the account `InviteMembers` delivers an
//! invitation from; the delivery itself is the `invitation_delivery` suite's subject.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, error_shape, router_over, Router};
use crate::invitation_count::{seeds, Seed};
use crate::invitation_listing::{enabled_detector_in, page_invitations};
use crate::invitation_unprocessed_results::DECLINE_RESULT;
use crate::negative_class_rejections::seed_invitation;

/// The Region most rows use.
const REGION: &str = "us-east-1";

/// How many invitations one inbox can hold — the store's own ceiling, which is also
/// `AccountIds`' `len 1..50` (`smithy_query(operation="DeclineInvitations")`), so one
/// call can name every invitation a full inbox holds.
const MAX_INVITATIONS: usize =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::MAX_INVITATIONS_PER_DETECTOR;

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const BAD_REQUEST: &str = "BadRequestException";

/// Well-formed account ids no seeded inbox holds an invitation from, differing along
/// the digit space: the low and high ends of the 12-digit `AccountId` space
/// (`len 12..12` — `smithy_query(shape="AccountId")`) and a mid-range id.
const NEVER_INVITED: [&str; 3] = ["000000000000", "999999999999", "313131313131"];

// ---------------------------------------------------------------------------
// Rule 1: a decline ends exactly the invitations the request named.
// ---------------------------------------------------------------------------

/// Inboxes to decline from, each row a SET of invitations plus the ones the request
/// names, differing along what the rule generalises over: the COUNT the inbox holds,
/// WHICH of them the request names, the ORDER the request names them in, the length of
/// the request list, and whether it names one account twice.
fn decline_rows() -> Vec<(&'static str, usize, Vec<usize>)> {
    vec![
        ("one invitation, the only one declined", 1, vec![0]),
        ("two invitations, the first declined", 2, vec![0]),
        ("two invitations, the second declined", 2, vec![1]),
        (
            "six invitations, three declined in an order that is not the inbox's",
            6,
            vec![4, 0, 2],
        ),
        (
            "six invitations, every one declined in reverse order",
            6,
            (0..6).rev().collect(),
        ),
        ("six invitations, one named twice", 6, vec![3, 3]),
        (
            "the inbox at its ceiling, every one declined in one call",
            MAX_INVITATIONS,
            (0..MAX_INVITATIONS).collect(),
        ),
        (
            "the inbox at its ceiling, every other one declined",
            MAX_INVITATIONS,
            (0..MAX_INVITATIONS).filter(|index| index % 2 == 0).collect(),
        ),
    ]
}

#[tokio::test]
async fn a_decline_ends_exactly_the_invitations_it_names() {
    for (case, size, named) in decline_rows() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;

        let inbox = seeds(size);
        for (account_id, invitation_id, millis) in &inbox {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }

        let account_ids: Vec<String> = named.iter().map(|index| inbox[*index].0.clone()).collect();
        let unprocessed = decline_ok(&router, REGION, &account_ids, case).await;
        assert!(
            unprocessed.is_empty(),
            "case {case:?}: every named account had an invitation to decline, expected no unprocessed account, got {unprocessed:?}"
        );

        // Exactly the invitations the request did NOT name are still pending, each
        // with every member it was seeded with: a decline that touched one of them, or
        // dropped a member, would show here.
        let kept: Vec<&Seed> = inbox
            .iter()
            .enumerate()
            .filter(|(index, _)| !named.contains(index))
            .map(|(_, seed)| seed)
            .collect();
        let listed = page_invitations(&router, REGION, "?maxResults=50", case).await;
        assert_eq!(
            listed.len(),
            kept.len(),
            "case {case:?}: expected {} invitation(s) left pending, got {listed:?}",
            kept.len()
        );
        for (position, (account_id, invitation_id, millis)) in kept.iter().enumerate() {
            let reported = &listed[position];
            assert_eq!(
                reported["accountId"],
                json!(account_id),
                "case {case:?}: expected the kept invitation from {account_id}, got {reported:?}"
            );
            assert_eq!(
                reported["invitationId"],
                json!(invitation_id),
                "case {case:?}: expected the kept invitation id {invitation_id:?}, got {reported:?}"
            );
            assert!(
                reported["invitedAt"]
                    .as_str()
                    .is_some_and(|at| !at.is_empty()),
                "case {case:?}: expected the kept invitation's invitedAt ({millis}), got {reported:?}"
            );
        }

        // And the count agrees with the listing, because both read the inbox the
        // decline wrote.
        let counted = invitations_count(&router, REGION, case).await;
        assert_eq!(
            counted,
            kept.len() as i64,
            "case {case:?}: expected the count to be the {} invitation(s) left, got {counted}",
            kept.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an account with no invitation is reported, not refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_the_inbox_holds_no_invitation_from_is_reported_as_unprocessed() {
    // The inbox every row below is seeded with. Each row gets an inbox of its own, so
    // one row's declines cannot explain another's report.
    let inbox = seeds(3);

    // The rows differ along how many never-invited accounts the request names, where
    // they sit in the list, and whether a named account is repeated. The expectation
    // is the never-invited accounts, each reported once, in the order the request
    // named them.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one never-invited account alone",
            vec![NEVER_INVITED[0].to_string()],
            vec![NEVER_INVITED[0].to_string()],
        ),
        (
            "a never-invited account before a seeded one",
            vec![NEVER_INVITED[1].to_string(), inbox[0].0.clone()],
            vec![NEVER_INVITED[1].to_string()],
        ),
        (
            "a never-invited account between two seeded ones",
            vec![
                inbox[1].0.clone(),
                NEVER_INVITED[2].to_string(),
                inbox[2].0.clone(),
            ],
            vec![NEVER_INVITED[2].to_string()],
        ),
        (
            "every never-invited account, one of them named twice",
            vec![
                NEVER_INVITED[2].to_string(),
                NEVER_INVITED[0].to_string(),
                NEVER_INVITED[2].to_string(),
                NEVER_INVITED[1].to_string(),
            ],
            vec![
                NEVER_INVITED[2].to_string(),
                NEVER_INVITED[0].to_string(),
                NEVER_INVITED[1].to_string(),
            ],
        ),
        (
            "the current account's own id, which cannot have invited itself",
            vec![current_account()],
            vec![current_account()],
        ),
    ];

    for (case, account_ids, expected) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;
        for (account_id, invitation_id, millis) in &inbox {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }

        let unprocessed = decline_ok(&router, REGION, &account_ids, case).await;
        let reported: Vec<String> = unprocessed
            .iter()
            .map(|entry| {
                entry["accountId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected an accountId, got {entry:?}")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the accounts with no invitation reported once each, got {unprocessed:?}"
        );
        for entry in &unprocessed {
            assert_eq!(
                entry["result"],
                json!(DECLINE_RESULT),
                "case {case:?}: expected the reason the account was not processed, got {entry:?}"
            );
        }
    }

    // Repeating a decline that DID end an invitation reports the account the second
    // time round and leaves the inbox where the first call left it: the operation is
    // not a failure on an invitation that is already answered.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, REGION).await;
    for (account_id, invitation_id, millis) in &inbox {
        seed_invitation(
            &engine,
            REGION,
            &detector_id,
            account_id,
            invitation_id,
            *millis,
        );
    }
    let named = vec![inbox[0].0.clone()];
    let case = "the first decline of a pending invitation";
    assert!(
        decline_ok(&router, REGION, &named, case).await.is_empty(),
        "case {case:?}: expected the pending invitation to be declined"
    );
    let after_first = page_invitations(&router, REGION, "?maxResults=50", case).await;
    for round in 0..3 {
        let case = format!("decline round {} of the same account", round + 2);
        let unprocessed = decline_ok(&router, REGION, &named, &case).await;
        assert_eq!(
            unprocessed.len(),
            1,
            "case {case:?}: expected the already-declined account reported, got {unprocessed:?}"
        );
        assert_eq!(
            page_invitations(&router, REGION, "?maxResults=50", &case).await,
            after_first,
            "case {case:?}: a repeated decline must leave the inbox where the first one left it"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the inbox answered is the one the request's Region serves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_decline_addresses_the_inbox_the_region_the_request_was_signed_for_serves() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));

    // Every Region's inbox holds invitations from the SAME inviting accounts, so a
    // decline that reached across Regions could not hide behind different ids.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let inbox = seeds(3);
    for region in regions {
        let detector_id = enabled_detector_in(&router, region).await;
        for (account_id, invitation_id, millis) in &inbox {
            seed_invitation(
                &engine,
                region,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }
    }

    let named = vec![inbox[1].0.clone()];
    let case = "declining through the us-east-1 endpoint";
    assert!(
        decline_ok(&router, regions[0], &named, case).await.is_empty(),
        "case {case:?}: expected the invitation this Region holds to be declined"
    );

    for region in regions {
        let expected = if region == regions[0] { 2 } else { 3 };
        let case = format!("the {region} inbox after the us-east-1 decline");
        let counted = invitations_count(&router, region, &case).await;
        assert_eq!(
            counted, expected,
            "case {case:?}: expected {expected} invitation(s), got {counted}"
        );
    }

    // Each other Region can still decline the same invitation from its OWN inbox, so
    // the first call answered one record and not the account everywhere.
    for region in &regions[1..] {
        let case = format!("declining through the {region} endpoint");
        assert!(
            decline_ok(&router, region, &named, &case).await.is_empty(),
            "case {case:?}: expected this Region's own invitation to be declined"
        );
        let counted = invitations_count(&router, region, &case).await;
        assert_eq!(
            counted, 2,
            "case {case:?}: expected 2 invitation(s) left, got {counted}"
        );
    }

    // An endpoint holding no detector at all has an empty inbox, so every named
    // account is reported — not a failure, and not a hollow success.
    let case = "an endpoint holding no detector";
    let account_ids: Vec<String> = inbox.iter().map(|(id, _, _)| id.clone()).collect();
    let unprocessed = decline_ok(&router, "us-west-2", &account_ids, case).await;
    assert_eq!(
        unprocessed.len(),
        account_ids.len(),
        "case {case:?}: expected every named account reported, got {unprocessed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: declining is the other answer — never an acceptance.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_declined_invitation_cannot_be_accepted_and_establishes_no_administrator() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, REGION).await;

    let inbox = seeds(3);
    for (account_id, invitation_id, millis) in &inbox {
        seed_invitation(
            &engine,
            REGION,
            &detector_id,
            account_id,
            invitation_id,
            *millis,
        );
    }

    // Decline the first invitation, leave the other two pending.
    let case = "declining the first invitation";
    assert!(
        decline_ok(&router, REGION, &[inbox[0].0.clone()], case)
            .await
            .is_empty(),
        "case {case:?}: expected the invitation to be declined"
    );

    // Declining establishes NO relationship: that is what accepting is for, so the
    // read still reports no administrator.
    let administrator = administrator_account(&router, &detector_id, case).await;
    assert_eq!(
        administrator["accountId"],
        Value::Null,
        "case {case:?}: declining must establish no administrator, got {administrator:?}"
    );

    // The declined invitation is no longer pending, so accepting it is refused under
    // the service's client-rejection class — never a 200 that establishes a
    // relationship from a declined invitation.
    let (status, body) = accept(&router, &detector_id, &inbox[0].0, &inbox[0].1).await;
    assert_eq!(
        status, 400,
        "case {case:?}: accepting a declined invitation expected 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_INPUT,
        "case {case:?}: expected {INVALID_INPUT} on the wire, got {body:?}"
    );
    assert_eq!(
        error_shape(&body),
        BAD_REQUEST,
        "case {case:?}: expected {BAD_REQUEST} in x-amzn-errortype, got {body:?}"
    );

    // An invitation the same call left alone is still pending, so it still accepts —
    // and that acceptance IS what establishes the relationship.
    let case = "accepting an invitation the decline left pending";
    let (status, body) = accept(&router, &detector_id, &inbox[1].0, &inbox[1].1).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let administrator = administrator_account(&router, &detector_id, case).await;
    assert_eq!(
        administrator["accountId"],
        json!(inbox[1].0),
        "case {case:?}: expected the accepted invitation's account as administrator, got {administrator:?}"
    );

    // Accepting took that invitation out of the inbox too, so declining it now
    // reports it rather than answering anything.
    let case = "declining an accepted invitation";
    let unprocessed = decline_ok(&router, REGION, &[inbox[1].0.clone()], case).await;
    assert_eq!(
        unprocessed.len(),
        1,
        "case {case:?}: expected the accepted invitation's account reported, got {unprocessed:?}"
    );

    // One invitation was never named by any call: it is still pending.
    let listed = page_invitations(&router, REGION, "?maxResults=50", case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected the untouched invitation to remain, got {listed:?}"
    );
    assert_eq!(
        listed[0]["accountId"],
        json!(inbox[2].0),
        "case {case:?}: expected the untouched invitation from {}, got {listed:?}",
        inbox[2].0
    );
}

// ---------------------------------------------------------------------------
// The calls and the helpers.
// ---------------------------------------------------------------------------

/// One `DeclineInvitations` call — `POST /invitation/decline`
/// (`smithy_query(operation="DeclineInvitations")`).
async fn decline_invitations(
    router: &Router,
    region: &str,
    account_ids: &[String],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/invitation/decline",
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// [`decline_invitations`], returning the `unprocessedAccounts` it reported and failing
/// the test if the call did not succeed or reported no list at all — the member is
/// required (`smithy_query(operation="DeclineInvitations")`), so an absent list is a
/// defect whatever it declined.
async fn decline_ok(
    router: &Router,
    region: &str,
    account_ids: &[String],
    case: &str,
) -> Vec<Value> {
    let (status, body) = decline_invitations(router, region, account_ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: decline-invitations expected 200, got {status} {body:?}"
    );
    body.get("unprocessedAccounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an unprocessedAccounts array, got {body:?}")
        })
}

/// One `AcceptAdministratorInvitation` call — the operation that answers an invitation
/// the inbox holds by ACCEPTING it
/// (`smithy_query(operation="AcceptAdministratorInvitation")`).
async fn accept(
    router: &Router,
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "POST",
        &format!("/detector/{detector_id}/administrator"),
        Some(json!({"administratorId": administrator_id, "invitationId": invitation_id})),
    )
    .await
}

/// The administrator relationship `GetAdministratorAccount` reports —
/// `GET /detector/{DetectorId}/administrator`.
async fn administrator_account(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call_in(
        router,
        REGION,
        "GET",
        &format!("/detector/{detector_id}/administrator"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-administrator-account expected 200, got {status} {body:?}"
    );
    body["administrator"].clone()
}

/// The size of the inbox `GetInvitationsCount` reports — the second reader of the
/// records a decline writes.
async fn invitations_count(router: &Router, region: &str, case: &str) -> i64 {
    let (status, body) = call_in(router, region, "GET", "/invitation/count", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-invitations-count expected 200, got {status} {body:?}"
    );
    body.get("invitationsCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an invitationsCount number, got {body:?}")
        })
}

/// The single account this emulator serves — the one every request it answers is
/// signed for, and therefore an account that cannot have invited itself.
fn current_account() -> String {
    ::engine::emulator_account_id().to_string()
}
