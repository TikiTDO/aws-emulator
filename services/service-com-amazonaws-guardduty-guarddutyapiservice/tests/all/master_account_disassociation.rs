// SPDX-License-Identifier: CC0-1.0

//! `DisassociateFromMasterAccount`: the DEPRECATED name of the member side of the
//! GuardDuty association, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test: the two names GuardDuty binds the member-side disassociation
//! under — `/master/disassociate` and `/administrator/disassociate`, "this operation is
//! deprecated, use DisassociateFromAdministratorAccount instead"
//! (`smithy_query(operation="DisassociateFromMasterAccount")`) — are ONE behaviour,
//! which drops the administrator relationship the account owning the named detector
//! accepted an invitation into and moves nothing else. Asserted as:
//!
//! 1. The deprecated call DROPS the relationship: HTTP 200 with an empty body, after
//!    which both administrator reads (`GetMasterAccount` and
//!    `GetAdministratorAccount`) report none — their members are all optional, so
//!    "none" is their ABSENCE — and the detector itself survives.
//! 2. EQUIVALENCE: for the same scenario the two paths answer alike and leave the
//!    store in the same observable condition, on the success path and on the refusals.
//! 3. An account no OTHER account administers administers itself, so the call asks it
//!    to leave a relationship whose two sides are the same account: it is refused with
//!    HTTP 400 under the `InvalidInputException` class, carrying GuardDuty's fixed
//!    sentence for that rule, and writes nothing — never associated, already
//!    disassociated, or holding an invitation that is only PENDING (which the refusal
//!    leaves pending).
//! 4. A `DetectorId` this account/Region does not hold is refused the same way — a
//!    malformed value as the parameter failure, a well-formed one as the ownership
//!    failure — and leaves an established relationship exactly where it was.
//! 5. Only that relationship moves: the member accounts the same detector administers
//!    are reported exactly as they were.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), so a row can hold an
//! invitation from any account id in the space rather than only from the one account
//! id this emulator presents, which is the account `InviteMembers` delivers an
//! invitation from; the delivery itself is the `invitation_delivery` suite's subject.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::{now_millis, DETECTOR_ID_MEMBER};

use crate::detector_lifecycle::{
    call_in, error_code, error_shape, router_over, Router, ABSENT_DETECTOR_ID,
};
use crate::member_association::{create_members, get_members};
use crate::negative_class_rejections::seed_invitation;

/// The error CLASS every GuardDuty client rejection carries in the body's `__type`
/// and in its own `type` member.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const BAD_REQUEST: &str = "BadRequestException";

/// The two names the administrator READ is bound under — "this operation is
/// deprecated, use GetAdministratorAccount instead"
/// (`smithy_query(operation="GetMasterAccount")`) — as
/// `(path segment, output member)`. Both report the one stored relationship, so every
/// rule below is checked through both.
const READS: [(&str, &str); 2] = [("master", "master"), ("administrator", "administrator")];

/// The two names the member-side DISASSOCIATION is bound under, as path segments:
/// `POST /detector/{DetectorId}/master/disassociate` and
/// `.../administrator/disassociate`
/// (`smithy_query(operation="DisassociateFromMasterAccount,DisassociateFromAdministratorAccount")`).
const DISASSOCIATIONS: [&str; 2] = ["master", "administrator"];

/// Established relationships to disassociate from, through the deprecated name.
///
/// The rows differ along the Region the detector lives in, along which of the two
/// acceptance names established the relationship (`AcceptInvitation` and
/// `AcceptAdministratorInvitation` are one behaviour under two names —
/// `smithy_query(operation="AcceptInvitation")`), along the administrator's account id
/// across the 12-digit account-id space, and along the invitation id's shape.
const ESTABLISHED: [(&str, &str, &str, &str); 4] = [
    (
        "us-east-1",
        "administrator",
        "111122223333",
        "d4b94fc952d6912b8f3060768220e2e7",
    ),
    (
        "eu-west-1",
        "master",
        "222233334444",
        "11111111-2222-3333-4444-555555555555",
    ),
    ("ap-southeast-2", "administrator", "999999999999", "i"),
    (
        "us-west-2",
        "master",
        "000000000000",
        "0123456789abcdef0123456789abcdef",
    ),
];

// ---------------------------------------------------------------------------
// Rule 1: the deprecated name drops the relationship, and both reads say so.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_from_the_master_account_clears_the_relationship_both_reads_report() {
    for (region, accept_segment, administrator_id, invitation_id) in ESTABLISHED {
        let case = format!("{region}/{accept_segment}: {administrator_id} via {invitation_id:?}");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, region).await;
        seed_invitation(
            &engine,
            region,
            &detector_id,
            administrator_id,
            invitation_id,
            now_millis(),
        );
        accept(
            &router,
            region,
            accept_segment,
            &detector_id,
            administrator_id,
            invitation_id,
        )
        .await;

        // The relationship is established first, through BOTH reads: otherwise a
        // disassociation that did nothing would be indistinguishable from one that
        // worked.
        for (read, member) in READS {
            let reported = administrator_account(&router, region, read, member, &detector_id).await;
            assert_eq!(
                reported["accountId"],
                json!(administrator_id),
                "case {case}: expected the accepted administrator before the disassociation, got {reported:?}"
            );
        }

        let (status, body) = disassociate(&router, region, "master", &detector_id).await;
        assert_eq!(
            status, 200,
            "case {case}: disassociating expected 200, got {status} {body:?}"
        );
        assert!(
            is_empty_output(&body),
            "case {case}: the operation models no output members, expected an empty body, got {body:?}"
        );

        // And the relationship is gone, through both reads: every member of the
        // reported shape is optional, so "no administrator" is their absence rather
        // than invented values.
        for (read, member) in READS {
            let reported = administrator_account(&router, region, read, member, &detector_id).await;
            for absent in ["accountId", "invitationId", "relationshipStatus", "invitedAt"] {
                assert_eq!(
                    reported[absent],
                    Value::Null,
                    "case {case}: expected {absent} absent after the disassociation, got {reported:?}"
                );
            }
        }

        // The detector is a resource of its own and survives: leaving an
        // administrator is not deleting the detector.
        let (status, read) = call_in(
            &router,
            region,
            "GET",
            &format!("/detector/{detector_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the detector must survive the disassociation, got {status} {read:?}"
        );
        assert_eq!(
            read["status"],
            json!("ENABLED"),
            "case {case}: expected the detector still enabled, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the deprecated name and the modern one are one behaviour.
// ---------------------------------------------------------------------------

/// What a scenario looks like from outside: the disassociation's own answer, plus what
/// both administrator reads report afterwards. Two paths are the same behaviour exactly
/// when this is equal for both.
type Observed = (u16, Value, Vec<Value>);

#[tokio::test]
async fn the_deprecated_and_current_disassociation_names_are_one_behaviour() {
    // The scenarios span the success path and each refusal class, so equivalence is
    // asserted where the two names could disagree, not only where both succeed.
    let scenarios: [(&str, Scenario); 4] = [
        ("an established relationship", Scenario::Established),
        ("no relationship ever accepted", Scenario::NeverAssociated),
        ("an invitation still pending", Scenario::InvitationPending),
        ("a detector this Region does not hold", Scenario::Absent),
    ];

    for (what, scenario) in scenarios {
        let mut observed: Vec<(&str, Observed)> = Vec::new();
        for segment in DISASSOCIATIONS {
            // A fresh engine per path: the two runs must be compared on the same
            // starting state, and a disassociation is not idempotent.
            observed.push((segment, run_scenario(scenario, segment).await));
        }
        let (first_segment, first) = &observed[0];
        for (segment, other) in &observed[1..] {
            assert_eq!(
                other, first,
                "case {what}: /{segment}/disassociate and /{first_segment}/disassociate must answer alike and leave the same state, got {other:?} vs {first:?}"
            );
        }
    }
}

/// The starting states rule 2 compares the two names over.
#[derive(Clone, Copy, Debug)]
enum Scenario {
    /// An invitation accepted, so there is a relationship to leave.
    Established,
    /// A detector that has accepted nothing.
    NeverAssociated,
    /// A detector holding an invitation that is only pending.
    InvitationPending,
    /// A well-formed detector id this Region does not hold.
    Absent,
}

/// Build `scenario`, call the disassociation bound under `segment`, and report what an
/// outside caller can see afterwards.
async fn run_scenario(scenario: Scenario, segment: &str) -> Observed {
    let region = "us-east-1";
    let administrator_id = "111122223333";
    let invitation_id = "d4b94fc952d6912b8f3060768220e2e7";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, region).await;

    match scenario {
        Scenario::NeverAssociated | Scenario::Absent => {}
        Scenario::InvitationPending | Scenario::Established => seed_invitation(
            &engine,
            region,
            &detector_id,
            administrator_id,
            invitation_id,
            // A fixed instant, so the two runs' reads are comparable value for value.
            1_700_000_000_000,
        ),
    }
    if matches!(scenario, Scenario::Established) {
        accept(
            &router,
            region,
            "administrator",
            &detector_id,
            administrator_id,
            invitation_id,
        )
        .await;
    }

    let target = if matches!(scenario, Scenario::Absent) {
        ABSENT_DETECTOR_ID
    } else {
        detector_id.as_str()
    };
    let (status, body) = disassociate(&router, region, segment, target).await;
    let mut reads = Vec::new();
    for (read, member) in READS {
        reads.push(administrator_account(&router, region, read, member, &detector_id).await);
    }
    (status, body, reads)
}

// ---------------------------------------------------------------------------
// Rule 3: no relationship to leave is the refusal, and it writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_without_a_master_account_is_refused_and_writes_nothing() {
    let region = "eu-west-1";
    let administrator_id = "777788889999";
    let invitation_id = "11111111-2222-3333-4444-555555555555";

    // Row 1: a detector that has accepted no invitation at all, called twice — the
    // refusal is a lookup, so repeating it changes nothing.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, region).await;
    for attempt in 1..=2 {
        let case = format!("never associated, attempt {attempt}");
        let (status, body) = disassociate(&router, region, "master", &detector_id).await;
        assert_self_disassociation(&case, status, &body, &detector_id);
        assert_no_relationship(&case, &router, region, &detector_id).await;
    }

    // Row 2: a detector holding an invitation that is only PENDING. An invitation is
    // not a relationship, and the refusal must not consume it: accepting it
    // afterwards still succeeds.
    seed_invitation(
        &engine,
        region,
        &detector_id,
        administrator_id,
        invitation_id,
        now_millis(),
    );
    let case = "invitation pending, not accepted";
    let (status, body) = disassociate(&router, region, "master", &detector_id).await;
    assert_self_disassociation(case, status, &body, &detector_id);
    assert_no_relationship(case, &router, region, &detector_id).await;
    accept(
        &router,
        region,
        "master",
        &detector_id,
        administrator_id,
        invitation_id,
    )
    .await;
    let reported = administrator_account(&router, region, "master", "master", &detector_id).await;
    assert_eq!(
        reported["accountId"],
        json!(administrator_id),
        "case {case}: the refusal must leave the invitation pending, got {reported:?}"
    );

    // Row 3: a detector whose relationship a disassociation already removed.
    let (status, body) = disassociate(&router, region, "master", &detector_id).await;
    assert_eq!(
        status, 200,
        "the established relationship expected 200, got {status} {body:?}"
    );
    for attempt in 1..=2 {
        let case = format!("already disassociated, attempt {attempt}");
        let (status, body) = disassociate(&router, region, "master", &detector_id).await;
        assert_self_disassociation(&case, status, &body, &detector_id);
        assert_no_relationship(&case, &router, region, &detector_id).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a DetectorId this account/Region does not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_the_master_of_a_detector_this_account_does_not_hold_is_refused() {
    let region = "us-west-2";
    let administrator_id = "333344445555";
    let invitation_id = "0123456789abcdef0123456789abcdef";

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, region).await;
    seed_invitation(
        &engine,
        region,
        &detector_id,
        administrator_id,
        invitation_id,
        now_millis(),
    );
    accept(
        &router,
        region,
        "master",
        &detector_id,
        administrator_id,
        invitation_id,
    )
    .await;

    let upper_cased = detector_id.to_uppercase();
    // The rows span the ways a `DetectorId` can fail to resolve: well formed but
    // never minted, well formed but not of this account (a held id upper-cased), and
    // values that cannot be a detector id at all. `len 1..300` is all the model
    // constrains (`smithy_query(operation="DisassociateFromMasterAccount")`), so
    // every row below reaches the handler.
    let rows: [(&str, &str, bool); 5] = [
        (
            ABSENT_DETECTOR_ID,
            "32 hex characters nobody minted",
            true,
        ),
        (
            upper_cased.as_str(),
            "a held id upper-cased, which is still well formed",
            true,
        ),
        ("not-a-detector-id", "not hexadecimal at all", false),
        ("abc123", "too short to be a detector id", false),
        (
            "0123456789abcdef0123456789abcdef0",
            "33 characters: one past the id length",
            false,
        ),
    ];

    // Every row is refused with the classified 400 naming `detectorId`, and the two
    // CLASSES are told apart from EACH OTHER rather than against the handler's own
    // wording: a value that cannot be a detector id at all is the parameter failure,
    // a well-formed one this account/Region does not hold the ownership failure, so
    // the two messages must differ while the rows within one class agree.
    let mut ownership: Option<String> = None;
    let mut parameter: Option<String> = None;
    for (candidate, why, well_formed) in rows {
        let case = format!("{candidate} ({why})");
        let (status, body) = disassociate(&router, region, "master", candidate).await;
        let message = assert_rejected(&case, status, &body, DETECTOR_ID_MEMBER, candidate);
        let seen = if well_formed {
            &mut ownership
        } else {
            &mut parameter
        };
        match seen {
            Some(first) => assert_eq!(
                &message, first,
                "case {case}: every row of one class must be refused alike, got {message:?}"
            ),
            None => *seen = Some(message),
        }

        // And the relationship the real detector holds is untouched.
        let reported =
            administrator_account(&router, region, "master", "master", &detector_id).await;
        assert_eq!(
            reported["accountId"],
            json!(administrator_id),
            "case {case}: a refused disassociation must leave the relationship where it was, got {reported:?}"
        );
    }
    let ownership = ownership.expect("a well-formed row");
    let parameter = parameter.expect("a malformed row");
    assert_ne!(
        ownership, parameter,
        "a value that cannot be a detector id is a different failure from a well-formed one this account does not hold"
    );

    // The same id through ANOTHER Region's endpoint: a detector is a regional
    // resource, so that endpoint holds no such detector — the OWNERSHIP failure the
    // rows above pinned, and the relationship stays.
    let case = "the held id through another Region's endpoint";
    let (status, body) = disassociate(&router, "eu-central-1", "master", &detector_id).await;
    let message = assert_rejected(case, status, &body, DETECTOR_ID_MEMBER, &detector_id);
    assert_eq!(
        message, ownership,
        "case {case}: expected the ownership failure, got {message:?}"
    );
    let reported = administrator_account(&router, region, "master", "master", &detector_id).await;
    assert_eq!(
        reported["accountId"],
        json!(administrator_id),
        "case {case}: expected the relationship untouched, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: only the administrator relationship moves.
// ---------------------------------------------------------------------------

/// Member accounts the detector under test administers: 12 decimal digits each, which
/// is the form `AccountId` takes (`len 12..12` — `smithy_query(shape="AccountId")`).
/// They are the OTHER side of the association, so nothing about them may move.
const ADMINISTERED: [(&str, &str); 3] = [
    ("444455556666", "member-one@example.com"),
    ("555566667777", "first.last@sub.example-corp.com"),
    ("666677778888", "m+tag@example.io"),
];

#[tokio::test]
async fn disassociating_the_master_leaves_the_administered_members_untouched() {
    let region = "us-east-1";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector_in(&router, region).await;

    let (status, body) = create_members(&router, &detector_id, &ADMINISTERED).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    let administered: Vec<&str> = ADMINISTERED.iter().map(|(id, _)| *id).collect();
    let (status, before) = get_members(&router, &detector_id, &administered).await;
    assert_eq!(
        status, 200,
        "get-members before: expected 200, got {status} {before:?}"
    );

    seed_invitation(
        &engine,
        region,
        &detector_id,
        "111122223333",
        "d4b94fc952d6912b8f3060768220e2e7",
        now_millis(),
    );
    accept(
        &router,
        region,
        "master",
        &detector_id,
        "111122223333",
        "d4b94fc952d6912b8f3060768220e2e7",
    )
    .await;

    let (status, body) = disassociate(&router, region, "master", &detector_id).await;
    assert_eq!(
        status, 200,
        "disassociating expected 200, got {status} {body:?}"
    );

    let (status, after) = get_members(&router, &detector_id, &administered).await;
    assert_eq!(
        status, 200,
        "get-members after: expected 200, got {status} {after:?}"
    );
    assert_eq!(
        after, before,
        "the members this detector administers must be reported exactly as before, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The calls and the shared assertions.
// ---------------------------------------------------------------------------

/// `POST /detector/{DetectorId}/{segment}/disassociate`, where `segment` picks one of
/// the two names the member-side disassociation is bound under.
async fn disassociate(
    router: &Router,
    region: &str,
    segment: &str,
    detector_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/{segment}/disassociate"),
        None,
    )
    .await
}

/// Accept the pending invitation through one of the two acceptance names, failing the
/// test unless it succeeded.
async fn accept(
    router: &Router,
    region: &str,
    segment: &str,
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) {
    let member = if segment == "master" {
        "masterId"
    } else {
        "administratorId"
    };
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/{segment}"),
        Some(json!({member: administrator_id, "invitationId": invitation_id})),
    )
    .await;
    assert_eq!(
        status, 200,
        "accepting through /{segment} in {region}: expected 200, got {status} {body:?}"
    );
}

/// The administrator relationship one of the two reads reports —
/// `GET /detector/{DetectorId}/{segment}`.
async fn administrator_account(
    router: &Router,
    region: &str,
    segment: &str,
    output_member: &str,
    detector_id: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/{segment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading /{segment} in {region}: expected 200, got {status} {body:?}"
    );
    body[output_member].clone()
}

/// Create an enabled detector through `region`'s endpoint and return its id.
async fn enabled_detector_in(router: &Router, region: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in {region}: expected 200, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector in {region}: got {body:?}"))
        .to_string()
}

/// Does `body` carry no output members? The operation's output shape is empty
/// (`smithy_query(operation="DisassociateFromMasterAccount")`), so the body is either
/// absent or an empty object.
fn is_empty_output(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// Assert a response is this service's 400 client rejection: the status, the class in
/// all three places it appears, a message that names `names`, and no echo of the
/// caller's `value`. Returns the message, so a caller can compare the CLASSES of two
/// rejections against each other.
///
/// The message is checked against the rule rather than pinned to the handler's own
/// constant: an HTTP status and an error class are requirements, while this service's
/// refusal WORDING is not published, so a literal comparison would only assert the
/// implementation against itself.
fn assert_rejected(case: &str, status: u16, body: &Value, names: &str, value: &str) -> String {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        BAD_REQUEST,
        "case {case}: expected the modelled envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case}: expected the invalid-input class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(INVALID_INPUT),
        "case {case}: expected the invalid-input class in the type member, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(names),
        "case {case}: expected a message naming {names:?}, got {message:?}"
    );
    assert!(
        !message.contains(value),
        "case {case}: a rejection must not echo the caller's value {value:?}, got {message:?}"
    );
    message
}

/// GuardDuty's FIXED refusal for a member-side disassociation sent by an account that
/// no other account administers: such an account administers itself, so the call asks
/// it to leave a relationship whose two sides are the same account. One sentence for
/// both names the disassociation is bound under.
const SELF_DISASSOCIATION: &str =
    "The request failed because admin account cannot disassociate from itself.";

/// Assert a response is the self-disassociation refusal: the classified 400 carrying
/// GuardDuty's fixed sentence for that rule, with nothing of the caller's echoed.
fn assert_self_disassociation(case: &str, status: u16, body: &Value, detector_id: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        BAD_REQUEST,
        "case {case}: expected the modelled envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case}: expected the invalid-input class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(INVALID_INPUT),
        "case {case}: expected the invalid-input class in the type member, got {body:?}"
    );
    assert_eq!(
        body["message"],
        json!(SELF_DISASSOCIATION),
        "case {case}: expected the self-disassociation sentence, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains(detector_id),
        "case {case}: a rejection must not echo the caller's value {detector_id:?}, got {message:?}"
    );
}

/// Assert both reads report no administrator relationship at all.
async fn assert_no_relationship(case: &str, router: &Router, region: &str, detector_id: &str) {
    for (read, member) in READS {
        let reported = administrator_account(router, region, read, member, detector_id).await;
        assert_eq!(
            reported["accountId"],
            Value::Null,
            "case {case}: expected no administrator through /{read}, got {reported:?}"
        );
    }
}
