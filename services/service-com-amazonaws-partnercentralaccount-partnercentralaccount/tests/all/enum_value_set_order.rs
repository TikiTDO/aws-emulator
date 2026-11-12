// SPDX-License-Identifier: CC0-1.0

//! The ORDER a constraint refusal lists an enumerated type's admissible values in,
//! end to end over the wire.
//!
//! The rule under test: `Member must satisfy enum value set: [ … ]` states the values
//! the type admits in the order this service enumerates a set in — a property of the
//! VALUES themselves (it is neither the order the model declares the type in nor the
//! sorted one), so one enumerated type is listed the same way whichever member,
//! operation or request reached it, and the listed set is always the modelled set
//! re-ordered: nothing added, nothing dropped, nothing repeated. The clause states NO
//! submitted value: the requirement is the set a value has to come from, not the one
//! value that was not in it, so the refusal reads the same whatever a caller sent.
//!
//! The rows differ along what the rule generalises over: which enumerated type the
//! refused value belongs to (five of this model's, of two, five and eight values),
//! which operation and member carried it, how the refused value was spelled (a name
//! no value carries, an admitted value miscased, the empty string, a value carrying
//! the set's own text), and whether the request breaks one constraint or two. The
//! orders the two sets this service's refusals are known to state are held to are
//! written out; every other set is held to the invariants, which is what makes this a
//! test of the rule rather than of whatever the formatter happened to emit.

use serde_json::{json, Value};

use service_com_amazonaws_partnercentralaccount_partnercentralaccount::ops::types::{
    CONNECTION_TYPE_VALUES, INVITATION_STATUS_VALUES, PARTICIPANT_TYPE_VALUES,
    PRIMARY_SOLUTION_TYPE_VALUES, PROFILE_VISIBILITY_VALUES,
};

use crate::connection_lifecycle::{error_code, string, Emulator, RECEIVER_PROFILE};

/// `PrimarySolutionType`'s admissible set as this service's refusals state it
/// (declared `SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES,
/// MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES,
/// VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES`, source
/// `smithy_query(query="shape",shape="PrimarySolutionType")`).
const PRIMARY_SOLUTION_TYPE_SET: &str = "TRAINING_SERVICES, COMMUNICATION_SERVICES, \
     CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, \
     HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES";

/// `InvitationStatus`'s admissible set as this service's refusals state it (declared
/// `PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED`, source
/// `smithy_query(query="shape",shape="InvitationStatus")`).
const INVITATION_STATUS_SET: &str = "EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED";

/// `ClientToken`'s modelled maximum (`@length(1, 64)`, source
/// `smithy_query(query="operation",operation="CreatePartner")`).
const CLIENT_TOKEN_MAX: usize = 64;

/// The requirement clause an enum-constraint violation states for `member_path`, with
/// the value half the class leaves out.
fn enum_clause(member_path: &str, value_set: &str) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{value_set}]"
    )
}

/// A `CreatePartner` request that is valid but for `PrimarySolutionType`, and — when
/// `client_token` is given — for that token.
pub(crate) fn create_partner(solution_type: &str, client_token: &str) -> Value {
    json!({
        "Catalog": "Sandbox",
        "ClientToken": client_token,
        "LegalName": "Example Partner Ltd",
        "PrimarySolutionType": solution_type,
        "AllianceLeadContact": {
            "FirstName": "Alex",
            "LastName": "Lead",
            "Email": "alex.lead@example.com",
            "BusinessTitle": "Alliance Lead",
        },
        "EmailVerificationCode": "123456",
    })
}

/// The bracketed set a refusal states for `member_path`, read off the message.
fn stated_set(message: &str, member_path: &str) -> Option<Vec<String>> {
    let marker = format!("at '{member_path}' failed to satisfy constraint: ");
    let (_, after) = message.split_once(marker.as_str())?;
    let (_, listed) = after.split_once("Member must satisfy enum value set: [")?;
    let (listed, _) = listed.split_once(']')?;
    Some(listed.split(", ").map(str::to_string).collect())
}

/// The two sets this service's refusals are known to state, each stated in that one
/// order — through the operation that carries the member, and whatever else the
/// request gets wrong.
#[tokio::test]
async fn a_known_enum_set_is_stated_in_the_order_the_refusals_state_it() {
    let emulator = Emulator::new();
    let over_long_token = "t".repeat(CLIENT_TOKEN_MAX + 1);

    // (case, operation, request, the whole Message the refusal must carry)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a solution type no value carries",
            "CreatePartner",
            create_partner("NOT_A_SOLUTION_TYPE", "tok-enum"),
            format!(
                "1 validation error detected: {}",
                enum_clause("primarySolutionType", PRIMARY_SOLUTION_TYPE_SET)
            ),
        ),
        (
            "a solution type outside the set together with an over-long client token",
            "CreatePartner",
            create_partner("NOT_A_SOLUTION_TYPE", &over_long_token),
            format!(
                "2 validation errors detected: Value '{over_long_token}' at 'clientToken' failed \
                 to satisfy constraint: Member must have length less than or equal to \
                 {CLIENT_TOKEN_MAX}; {}",
                enum_clause("primarySolutionType", PRIMARY_SOLUTION_TYPE_SET)
            ),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a constraint violation is HTTP 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?}: {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The same rule through the other member it is known for, whose clause is read
    // out of the message so the rest of its wording is not pinned here.
    let case = "an invitation status no value carries";
    let (status, body) = emulator
        .call(
            "ListConnectionInvitations",
            &json!({ "Catalog": "AWS", "Status": "NOT_A_REAL_STATUS" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a constraint violation is HTTP 400, got {status}: {body}"
    );
    let message = string(&body, "Message", case);
    let expected = enum_clause("status", INVITATION_STATUS_SET);
    assert!(
        message.contains(&expected),
        "case {case:?}: expected the clause {expected:?}, got {message:?}"
    );
}

/// The stated order belongs to the TYPE, not to the request: one enumerated type
/// refused through every spelling of an inadmissible value a caller can send states
/// one set, in one order.
#[tokio::test]
async fn the_stated_order_does_not_follow_the_refused_value() {
    let emulator = Emulator::new();

    let spellings = [
        ("a name no value carries", "NOT_A_SOLUTION_TYPE"),
        ("an admitted value miscased", "software_products"),
        ("an admitted value padded", " SOFTWARE_PRODUCTS"),
        ("the empty string", ""),
        ("a value carrying the set's own text", PRIMARY_SOLUTION_TYPE_SET),
        ("an admitted value of ANOTHER type", "PENDING"),
    ];

    for (case, refused) in spellings {
        let (status, body) = emulator
            .call("CreatePartner", &create_partner(refused, "tok-spelling"))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the set is refused, got {status}: {body}"
        );
        let message = string(&body, "Message", case);
        let stated = stated_set(&message, "primarySolutionType").unwrap_or_else(|| {
            panic!("case {case:?}: the message states no enum value set: {message:?}")
        });
        assert_eq!(
            stated.join(", "),
            PRIMARY_SOLUTION_TYPE_SET,
            "case {case:?}: the set is stated in one order whatever the value, got {message:?}"
        );
    }
}

/// Every enum-constrained input member of this service: the set it states is the
/// MODELLED set re-ordered — every admissible value stated exactly once — and one
/// type is stated in the same order through every member that reaches it.
///
/// Rows: five enumerated types over seven members of five operations, including the
/// three members that reach `ConnectionType` (so the "same type, same order" half is
/// exercised), two-value types, a five-value one and an eight-value one.
#[tokio::test]
async fn the_stated_set_is_the_modelled_set_re_ordered() {
    let emulator = Emulator::new();
    let outside = "NOT_A_MODELLED_VALUE";

    // (case, operation, request, member path, the type's modelled value set)
    let rows: Vec<(&str, &str, Value, &str, &[&str])> = vec![
        (
            "CreatePartner's primary solution type",
            "CreatePartner",
            create_partner(outside, "tok-modelled"),
            "primarySolutionType",
            PRIMARY_SOLUTION_TYPE_VALUES,
        ),
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
    ];

    // What each member path stated, so the two that reach one type can be compared.
    let mut stated_for_connection_type: Vec<(&str, Vec<String>)> = Vec::new();

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
        let stated = stated_set(&message, member_path).unwrap_or_else(|| {
            panic!("case {case:?}: the message states no enum value set: {message:?}")
        });

        let mut sorted_stated = stated.clone();
        sorted_stated.sort_unstable();
        let mut sorted_modelled: Vec<String> =
            modelled.iter().map(|value| (*value).to_string()).collect();
        sorted_modelled.sort_unstable();
        assert_eq!(
            sorted_stated, sorted_modelled,
            "case {case:?}: the stated set must be the modelled set re-ordered, got {stated:?}"
        );

        if modelled == CONNECTION_TYPE_VALUES {
            stated_for_connection_type.push((case, stated));
        }
    }

    let (first_case, first) = stated_for_connection_type
        .first()
        .expect("three rows reach ConnectionType");
    for (case, stated) in &stated_for_connection_type {
        assert_eq!(
            stated, first,
            "case {case:?}: one type is stated in the order it owns, which {first_case:?} states \
             as {first:?}, got {stated:?}"
        );
    }
}
