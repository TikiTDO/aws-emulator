// SPDX-License-Identifier: CC0-1.0

//! What an enum-constraint refusal states about the value it REFUSED: nothing.
//!
//! The rule under test: a clause refusing a value outside an enumerated type's value set
//! names the member and the set and states no value —
//! `Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value
//! set: [ … ]` — because the requirement is the set every value of that member has to come
//! from and not the one value that was not in it. So the refusal reads the same whatever a
//! caller sent, and it holds for the CLASS: every enum-constrained member of every
//! operation, whatever the type, at whatever depth. Every other constraint class keeps the
//! value half it is refused with, so the two cannot be confused.
//!
//! The rows differ along what the rule generalises over: which enumerated type carried the
//! refused value (six of this model's, of two, three, five and eight values), which
//! operation and member carried it (nine members over seven operations, one of them nested
//! inside a structure member), how the refused value was spelled (a name no value carries,
//! an admitted value miscased, padded, the empty string, a long value, a value carrying a
//! quote, an admitted value of another type), and whether the request breaks the enum alone
//! or an enum together with another class. The value SET's order is not pinned here — it is
//! `enum_value_set_order.rs`'s rule; this suite reads the set out of the message and holds
//! it to being the modelled set re-ordered.

use serde_json::{json, Value};

use service_com_amazonaws_partnercentralaccount_partnercentralaccount::ops::types::{
    ACCESS_TYPE_VALUES, CONNECTION_TYPE_VALUES, INVITATION_STATUS_VALUES, PARTICIPANT_TYPE_VALUES,
    PRIMARY_SOLUTION_TYPE_VALUES, PROFILE_VISIBILITY_VALUES,
};

use crate::connection_lifecycle::{error_code, string, Emulator, RECEIVER_PROFILE};
use crate::enum_value_set_order::create_partner;

/// `ClientToken`'s modelled maximum (`@length(1, 64)`, source
/// `smithy_query(query="operation",operation="CreatePartner")`).
const CLIENT_TOKEN_MAX: usize = 64;

/// Everything a clause states before the value set it lists: the envelope's count, the
/// member path, and the value half the enum class leaves out.
fn clause_opening(member_path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         Member must satisfy enum value set: ["
    )
}

/// A `StartProfileUpdateTask` request valid but for `solution_type`, which sits on the
/// nested `TaskDetails` member (source:
/// `smithy_query(query="shape",shape="TaskDetails")`).
fn start_profile_update_task(solution_type: &str) -> Value {
    json!({
        "Catalog": "AWS",
        "Identifier": "partner-ABCDEFGHIJKLM",
        "ClientToken": "tok-profile-task",
        "TaskDetails": {
            "DisplayName": "Example Partner",
            "Description": "An example partner profile.",
            "WebsiteUrl": "https://example.com",
            "LogoUrl": "https://example.com/logo.png",
            "PrimarySolutionType": solution_type,
            "IndustrySegments": ["RETAIL"],
            "TranslationSourceLocale": "en-US",
        },
    })
}

/// Every enum-constrained input member of this service, each refused a value outside its
/// type: one validation error, stating the member and the set it had to come from and no
/// value at all, and the set it lists is the modelled set re-ordered.
#[tokio::test]
async fn an_enum_refusal_states_no_submitted_value() {
    let emulator = Emulator::new();
    let outside = "NOT_A_MODELLED_VALUE";

    // (case, operation, request, member path, the type's modelled value set)
    let rows: Vec<(&str, &str, Value, &str, &[&str])> = vec![
        (
            "ListConnectionInvitations' status filter",
            "ListConnectionInvitations",
            json!({ "Catalog": "AWS", "Status": outside }),
            "status",
            INVITATION_STATUS_VALUES,
        ),
        (
            "ListConnectionInvitations' participant-type filter",
            "ListConnectionInvitations",
            json!({ "Catalog": "AWS", "ParticipantType": outside }),
            "participantType",
            PARTICIPANT_TYPE_VALUES,
        ),
        (
            "ListConnectionInvitations' connection-type filter",
            "ListConnectionInvitations",
            json!({ "Catalog": "AWS", "ConnectionType": outside }),
            "connectionType",
            CONNECTION_TYPE_VALUES,
        ),
        (
            "CreateConnectionInvitation's connection type",
            "CreateConnectionInvitation",
            json!({
                "Catalog": "AWS",
                "ClientToken": "tok-create-invitation",
                "ConnectionType": outside,
                "Email": "inviter@example.com",
                "Message": "please connect",
                "Name": "Inviter",
                "ReceiverIdentifier": RECEIVER_PROFILE,
            }),
            "connectionType",
            CONNECTION_TYPE_VALUES,
        ),
        (
            "CancelConnection's connection type",
            "CancelConnection",
            json!({
                "Catalog": "AWS",
                "Identifier": "pac-ABCDEFGHIJKLM",
                "ConnectionType": outside,
                "Reason": "no longer needed",
                "ClientToken": "tok-cancel",
            }),
            "connectionType",
            CONNECTION_TYPE_VALUES,
        ),
        (
            "PutProfileVisibility's visibility",
            "PutProfileVisibility",
            json!({
                "Catalog": "AWS",
                "Identifier": "partner-ABCDEFGHIJKLM",
                "Visibility": outside,
            }),
            "visibility",
            PROFILE_VISIBILITY_VALUES,
        ),
        (
            "UpdateConnectionPreferences' access type",
            "UpdateConnectionPreferences",
            json!({ "Catalog": "AWS", "Revision": 1, "AccessType": outside }),
            "accessType",
            ACCESS_TYPE_VALUES,
        ),
        (
            "CreatePartner's primary solution type",
            "CreatePartner",
            create_partner(outside, "tok-value-half"),
            "primarySolutionType",
            PRIMARY_SOLUTION_TYPE_VALUES,
        ),
        (
            "StartProfileUpdateTask's solution type, nested in TaskDetails",
            "StartProfileUpdateTask",
            start_profile_update_task(outside),
            "taskDetails.primarySolutionType",
            PRIMARY_SOLUTION_TYPE_VALUES,
        ),
    ];

    for (case, operation, request, member_path, modelled) in rows {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the modelled set is refused, got {status}: {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?}: {body}"
        );
        let message = string(&body, "Message", case);
        let listed = listed_set(&message, member_path, case);

        let mut sorted_listed = listed.clone();
        sorted_listed.sort_unstable();
        let mut sorted_modelled: Vec<String> =
            modelled.iter().map(|value| (*value).to_string()).collect();
        sorted_modelled.sort_unstable();
        assert_eq!(
            sorted_listed, sorted_modelled,
            "case {case:?}: the listed set is the modelled set re-ordered, got {listed:?}"
        );
    }
}

/// The values a refusal lists for `member_path`, with the whole clause around them held to
/// the wording the rule states — so a value half anywhere in the message fails here.
fn listed_set(message: &str, member_path: &str, case: &str) -> Vec<String> {
    let opening = clause_opening(member_path);
    let listed = message
        .strip_prefix(opening.as_str())
        .and_then(|tail| tail.strip_suffix(']'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {opening:?} then the set, got {message:?}")
        });
    assert!(
        !message.contains("Value '"),
        "case {case:?}: an enum clause quotes no value, got {message:?}"
    );
    listed.split(", ").map(str::to_string).collect()
}

/// The clause belongs to the member and its type, not to the request: one member refused
/// every spelling of an inadmissible value a caller can send answers one message, byte for
/// byte.
#[tokio::test]
async fn the_clause_does_not_follow_the_refused_value() {
    let emulator = Emulator::new();

    let spellings = [
        ("a name no value carries", "NOT_A_MODELLED_VALUE"),
        ("an admitted value miscased", "pending"),
        ("an admitted value padded", " PENDING"),
        ("the empty string", ""),
        ("a value carrying a quote", "O'Brien"),
        ("a value carrying the set's own text", "EXPIRED, ACCEPTED"),
        ("an admitted value of ANOTHER type", "SUBSIDIARY"),
        ("a long value", &"V".repeat(512)),
    ];

    let mut stated: Option<(&str, String)> = None;
    for (case, refused) in spellings {
        let (status, body) = emulator
            .call(
                "ListConnectionInvitations",
                &json!({ "Catalog": "AWS", "Status": refused }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the set is refused, got {status}: {body}"
        );
        let message = string(&body, "Message", case);
        listed_set(&message, "status", case);
        match &stated {
            None => stated = Some((case, message)),
            Some((first_case, first)) => assert_eq!(
                &message, first,
                "case {case:?}: expected the message {first_case:?} states, {first:?}, got \
                 {message:?}"
            ),
        }
    }
}

/// The value half is dropped for the enum CLASS and not for the request: a request that
/// breaks an enum and a `@length` maximum together states the over-long value it sent and
/// not the inadmissible one.
#[tokio::test]
async fn another_class_in_the_same_request_still_states_its_value() {
    let emulator = Emulator::new();
    let over_long_token = "t".repeat(CLIENT_TOKEN_MAX + 1);
    let case = "an over-long client token and a solution type outside its set";

    let (status, body) = emulator
        .call(
            "CreatePartner",
            &create_partner("NOT_A_MODELLED_VALUE", &over_long_token),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: two constraint violations are HTTP 400, got {status}: {body}"
    );
    let message = string(&body, "Message", case);
    let token_clause = format!(
        "Value '{over_long_token}' at 'clientToken' failed to satisfy constraint: Member must \
         have length less than or equal to {CLIENT_TOKEN_MAX}"
    );
    assert!(
        message.contains(&token_clause),
        "case {case:?}: expected the length clause {token_clause:?}, got {message:?}"
    );
    let enum_clause = "Value at 'primarySolutionType' failed to satisfy constraint: Member must \
                       satisfy enum value set: [";
    assert!(
        message.contains(enum_clause),
        "case {case:?}: expected the enum clause {enum_clause:?}, got {message:?}"
    );
    assert!(
        !message.contains("Value 'NOT_A_MODELLED_VALUE'"),
        "case {case:?}: the enum clause quotes no value, got {message:?}"
    );
}

/// The refusal is owed to a value OUTSIDE the set and to nothing else: every value the
/// type admits is served.
#[tokio::test]
async fn every_admitted_value_is_served() {
    let emulator = Emulator::new();

    for status_filter in INVITATION_STATUS_VALUES {
        let answered = emulator
            .ok(
                "ListConnectionInvitations",
                &json!({ "Catalog": "AWS", "Status": status_filter }),
            )
            .await;
        assert!(
            answered
                .get("ConnectionInvitationSummaries")
                .and_then(Value::as_array)
                .is_some(),
            "case {status_filter:?}: an admitted filter value is served a list, got {answered}"
        );
    }
}
