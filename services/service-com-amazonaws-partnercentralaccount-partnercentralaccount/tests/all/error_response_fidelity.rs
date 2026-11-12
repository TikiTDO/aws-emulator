// SPDX-License-Identifier: CC0-1.0

//! How this service REPORTS a client error, end to end over the wire: the status and
//! the message of a lookup that finds nothing, the `Message` member of a refusal on
//! an input constraint, and the receiver a connection invitation can be addressed to.
//!
//! Every call goes through the real [`MultiServiceRouter`] the production emulator
//! mounts, through the harness the lifecycle suite owns ([`Emulator`]), and only the
//! HTTP status and the JSON body are inspected.
//!
//! Each test is a TABLE: the rows differ along what its rule generalises over — the
//! operation and the resource for the not-found rule, the class of constraint for the
//! `Message` rule, and the `(ConnectionType, ReceiverIdentifier)` pair for the
//! addressability rule — and the case each divergence was recorded on is one row.

use serde_json::{json, Value};

use crate::connection_lifecycle::{
    assert_error, error_code, string, Emulator, RECEIVER_ACCOUNT, RECEIVER_PROFILE,
};

/// `Email`'s modelled `@pattern` with the anchors the model brackets it with removed,
/// which is how this service names a pattern requirement (source:
/// `smithy_query(query="shape",shape="SendEmailVerificationCodeRequest")`).
const EMAIL_PATTERN: &str =
    r#"[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*"#;

/// `ConnectionInvitationId`'s modelled `@pattern`, unanchored the same way (source:
/// `smithy_query(query="operation",operation="GetConnectionInvitation")`).
const INVITATION_ID_PATTERN: &str = "pacinv-[A-Za-z0-9]{13}";

/// Every value `PrimarySolutionType` admits (source:
/// `smithy_query(query="shape",shape="PrimarySolutionType")`). The service states the
/// admissible SET in the message; the order it enumerates a set in follows no
/// modelled rule, so each value is checked for membership rather than by position.
const PRIMARY_SOLUTION_TYPES: &[&str] = &[
    "SOFTWARE_PRODUCTS",
    "CONSULTING_SERVICES",
    "PROFESSIONAL_SERVICES",
    "MANAGED_SERVICES",
    "HARDWARE_PRODUCTS",
    "COMMUNICATION_SERVICES",
    "VALUE_ADDED_RESALE_AWS_SERVICES",
    "TRAINING_SERVICES",
];

/// The message a lookup that found nothing states, as this service builds it from the
/// request: the kind of resource, then the catalog and the identifier it was named by.
fn not_found_message(resource: &str, catalog: &str, identifier: &str) -> String {
    format!("{resource} not found with catalog: {catalog} and identifier: {identifier}.")
}

/// A `CreateConnectionInvitation` request for `connection_type` addressed to
/// `receiver`.
fn invitation(catalog: &str, token: &str, connection_type: &str, receiver: &str) -> Value {
    json!({
        "Catalog": catalog,
        "ClientToken": token,
        "ConnectionType": connection_type,
        "Email": "inviter@example.com",
        "Message": "please connect",
        "Name": "Inviter",
        "ReceiverIdentifier": receiver,
    })
}

/// A lookup of a connection or a connection invitation this service does not hold is
/// answered with the modelled `ResourceNotFoundException` AT HTTP 400 — the status
/// this service's frontend serializes that error at — and a message built from the
/// request: `<Resource> not found with catalog: <Catalog> and identifier:
/// <Identifier>.`
///
/// Rows: every operation that looks a resource up, over both resource kinds, several
/// catalogs, an id nobody minted and an id that exists in ANOTHER catalog (so the
/// message is shown to name the catalog the request asked about, not the one the
/// record is in).
#[tokio::test]
async fn a_lookup_that_finds_nothing_is_a_400_naming_the_catalog_and_identifier() {
    let emulator = Emulator::new();

    // A real invitation and a real connection in the AWS catalog, so the rows that
    // look them up in another catalog are looking up something that does exist.
    let created = emulator
        .ok(
            "CreateConnectionInvitation",
            &invitation(
                "AWS",
                "tok-known",
                "OPPORTUNITY_COLLABORATION",
                RECEIVER_PROFILE,
            ),
        )
        .await;
    let known_invitation = string(&created, "Id", "the stored invitation");
    let accepted = emulator
        .ok(
            "AcceptConnectionInvitation",
            &json!({
                "Catalog": "AWS",
                "Identifier": known_invitation,
                "ClientToken": "tok-accept",
            }),
        )
        .await;
    let known_connection = string(&accepted["Connection"], "Id", "the stored connection");

    // (case, operation, resource the message names, catalog, identifier)
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "an invitation id nobody minted",
            "GetConnectionInvitation",
            "Connection invitation",
            "Sandbox",
            "pacinv-0000000000000".to_string(),
        ),
        (
            "an invitation id nobody minted, another catalog",
            "GetConnectionInvitation",
            "Connection invitation",
            "aws",
            "pacinv-ZZZZZZZZZZZZZ".to_string(),
        ),
        (
            "an invitation stored in another catalog",
            "GetConnectionInvitation",
            "Connection invitation",
            "Sandbox",
            known_invitation.clone(),
        ),
        (
            "accepting an invitation nobody minted",
            "AcceptConnectionInvitation",
            "Connection invitation",
            "Sandbox",
            "pacinv-0000000000000".to_string(),
        ),
        (
            "accepting an invitation stored in another catalog",
            "AcceptConnectionInvitation",
            "Connection invitation",
            "Sandbox",
            known_invitation.clone(),
        ),
        (
            "a connection id nobody minted",
            "GetConnection",
            "Connection",
            "Sandbox",
            "pac-0000000000000".to_string(),
        ),
        (
            "a connection stored in another catalog",
            "GetConnection",
            "Connection",
            "Sandbox",
            known_connection.clone(),
        ),
        (
            "cancelling a connection nobody minted",
            "CancelConnection",
            "Connection",
            "AWS",
            "pac-0000000000000".to_string(),
        ),
        (
            "cancelling a connection stored in another catalog",
            "CancelConnection",
            "Connection",
            "Sandbox",
            known_connection.clone(),
        ),
    ];

    for (index, (case, operation, resource, catalog, identifier)) in rows.into_iter().enumerate() {
        let mut request = json!({ "Catalog": catalog, "Identifier": identifier });
        // The two mutating lookups carry more members; neither changes what the
        // lookup answers.
        if operation == "AcceptConnectionInvitation" {
            request["ClientToken"] = json!(format!("absent-{index}"));
        }
        if operation == "CancelConnection" {
            request["ClientToken"] = json!(format!("absent-{index}"));
            request["ConnectionType"] = json!("SUBSIDIARY");
            request["Reason"] = json!("no such connection");
        }

        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a lookup that finds nothing is HTTP 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code:?}: {body}"
        );
        let expected_reason = if resource == "Connection" {
            "CONNECTION_NOT_FOUND"
        } else {
            "CONNECTION_INVITATION_NOT_FOUND"
        };
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some(expected_reason),
            "case {case:?}: the refusal names which resource was not found: {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(not_found_message(resource, catalog, &identifier).as_str()),
            "case {case:?}: the message names the resource, the catalog and the identifier the request carried: {body}"
        );
    }
}

/// A member outside a modelled constraint is refused with the modelled
/// `ValidationException` — HTTP 400, and the required `Message` member carrying the
/// AWS validation envelope: `N validation error(s) detected: Value '<value>' at
/// '<member>' failed to satisfy constraint: <the constraint>`, with the member named
/// in lowerCamelCase and a `@pattern` quoted without its anchors.
///
/// Rows: one per class of constraint the model declares on this service's inputs —
/// `@pattern`, `@length` (both bounds), `@range` (both bounds), `@required` absence —
/// across five operations, including two whose handlers are still scaffolded (the
/// refusal is owed before any handler runs).
#[tokio::test]
async fn a_constraint_violation_is_refused_with_the_modelled_message_member() {
    let emulator = Emulator::new();
    let over_long_catalog = "c".repeat(65);
    let over_long_token = "t".repeat(65);

    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an email outside Email's pattern",
            "SendEmailVerificationCode",
            json!({ "Catalog": "Sandbox", "Email": "not-an-email-address" }),
            format!(
                "1 validation error detected: Value 'not-an-email-address' at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: {EMAIL_PATTERN}"
            ),
        ),
        (
            "an identifier outside ConnectionInvitationId's pattern",
            "GetConnectionInvitation",
            json!({ "Catalog": "AWS", "Identifier": "pacinv-short" }),
            format!(
                "1 validation error detected: Value 'pacinv-short' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: {INVITATION_ID_PATTERN}"
            ),
        ),
        (
            "a catalog longer than Catalog's maximum",
            "ListConnectionInvitations",
            json!({ "Catalog": over_long_catalog.clone() }),
            format!(
                "1 validation error detected: Value '{over_long_catalog}' at 'catalog' failed to satisfy constraint: Member must have length less than or equal to 64"
            ),
        ),
        (
            "MaxResults above its range",
            "ListConnectionInvitations",
            json!({ "Catalog": "AWS", "MaxResults": 101 }),
            "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "MaxResults below its range",
            "ListConnections",
            json!({ "Catalog": "AWS", "MaxResults": 0 }),
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "an absent required member",
            "GetConnection",
            json!({ "Catalog": "AWS" }),
            "1 validation error detected: Value null at 'identifier' failed to satisfy constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a client token longer than ClientToken's maximum",
            "CreatePartner",
            json!({
                "Catalog": "Sandbox",
                "ClientToken": over_long_token.clone(),
                "LegalName": "Example Partner Ltd",
                "PrimarySolutionType": "SOFTWARE_PRODUCTS",
                "AllianceLeadContact": {
                    "FirstName": "Alex",
                    "LastName": "Lead",
                    "Email": "alex.lead@example.com",
                    "BusinessTitle": "Alliance Lead",
                },
                "EmailVerificationCode": "123456",
            }),
            format!(
                "1 validation error detected: Value '{over_long_token}' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64"
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
            "case {case:?}: the refusal carries the modelled Message member: {body}"
        );
        assert!(
            body.get("message").is_none(),
            "case {case:?}: the reason is reported in the modelled Message member only: {body}"
        );
        assert!(
            body.get("Reason").is_none(),
            "case {case:?}: a member outside its declared constraint carries no ValidationExceptionReason: {body}"
        );
    }
}

/// Several violations in one request are reported together: the count, then one clause
/// per violation joined with `; `, in the order the members are walked in.
#[tokio::test]
async fn several_violations_are_reported_in_one_message() {
    let emulator = Emulator::new();
    let over_long_token = "t".repeat(65);

    let (status, body) = emulator
        .call(
            "CreatePartner",
            &json!({
                "Catalog": "Sandbox",
                "ClientToken": over_long_token.clone(),
                "LegalName": "Example Partner Ltd",
                "PrimarySolutionType": "NOT_A_SOLUTION_TYPE",
                "AllianceLeadContact": {
                    "FirstName": "Alex",
                    "LastName": "Lead",
                    "Email": "alex.lead@example.com",
                    "BusinessTitle": "Alliance Lead",
                },
                "EmailVerificationCode": "123456",
            }),
        )
        .await;
    let case = "an over-long client token and an unmodelled solution type";
    assert_eq!(
        status, 400,
        "case {case:?}: two constraint violations are HTTP 400, got {status}: {body}"
    );
    let message = string(&body, "Message", case);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: the envelope counts the violations, got {message:?}"
    );
    let token_clause = format!(
        "Value '{over_long_token}' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64"
    );
    // An enum clause states no submitted value (see `tests/all/enum_value_set_order.rs`),
    // where the length clause above states the token it was sent.
    let enum_clause = "Value at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [";
    assert!(
        message.contains(&token_clause),
        "case {case:?}: the message states the length violation, got {message:?}"
    );
    assert!(
        message.contains(enum_clause),
        "case {case:?}: the message states the enum violation, got {message:?}"
    );
    for admissible in PRIMARY_SOLUTION_TYPES {
        assert!(
            message.contains(admissible),
            "case {case:?}: the enum clause names the admissible value {admissible}, got {message:?}"
        );
    }
    let (token_at, enum_at) = (
        message.find(&token_clause).unwrap_or_default(),
        message.find(enum_clause).unwrap_or_default(),
    );
    assert!(
        token_at < enum_at,
        "case {case:?}: the clauses follow the order the members are walked in, got {message:?}"
    );
}

/// A connection invitation is addressed to a participant of the kind its
/// `ConnectionType` connects: an `OPPORTUNITY_COLLABORATION` to a partner profile
/// (`PartnerProfileId`, `^pprofile-[A-Za-z0-9]{13}$`) and a `SUBSIDIARY` to an AWS
/// account (`AwsAccountId`, `^[0-9]{12}$`). Any other receiver form is refused with a
/// field-level `ValidationException` on `ReceiverIdentifier`, and nothing is stored.
///
/// Rows: both connection types crossed with receiver forms on either side of each
/// published form — the profile prefix with a body of the wrong length, an account id
/// one digit short and one digit long, a non-digit in an account id, the identifier
/// form of the OTHER connection type, and a bare identifier
/// `ParticipantIdentifier`'s own pattern admits.
#[tokio::test]
async fn a_receiver_must_be_addressable_under_the_requested_connection_type() {
    let emulator = Emulator::new();

    // (case, connection type, receiver, whether the create is accepted)
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "a partner profile id for a collaboration",
            "OPPORTUNITY_COLLABORATION",
            RECEIVER_PROFILE,
            true,
        ),
        (
            "another partner profile id for a collaboration",
            "OPPORTUNITY_COLLABORATION",
            "pprofile-0000000000001",
            true,
        ),
        (
            "an account id for a subsidiary",
            "SUBSIDIARY",
            RECEIVER_ACCOUNT,
            true,
        ),
        (
            "the highest account id for a subsidiary",
            "SUBSIDIARY",
            "999999999999",
            true,
        ),
        (
            "the recorded case: a bare identifier for a collaboration",
            "OPPORTUNITY_COLLABORATION",
            "partner-0000000000001",
            false,
        ),
        (
            "a profile prefix with too short a body",
            "OPPORTUNITY_COLLABORATION",
            "pprofile-ABCDEFGHIJKL",
            false,
        ),
        (
            "a profile prefix with too long a body",
            "OPPORTUNITY_COLLABORATION",
            "pprofile-ABCDEFGHIJKLMN",
            false,
        ),
        (
            "an account id for a collaboration",
            "OPPORTUNITY_COLLABORATION",
            RECEIVER_ACCOUNT,
            false,
        ),
        (
            "a profile id for a subsidiary",
            "SUBSIDIARY",
            RECEIVER_PROFILE,
            false,
        ),
        (
            "an account id one digit short",
            "SUBSIDIARY",
            "00000000001",
            false,
        ),
        (
            "an account id one digit long",
            "SUBSIDIARY",
            "0000000000012",
            false,
        ),
        (
            "an account id carrying a letter",
            "SUBSIDIARY",
            "00000000000a",
            false,
        ),
        (
            "a bare identifier for a subsidiary",
            "SUBSIDIARY",
            "subsidiary-1",
            false,
        ),
    ];

    // What a refused receiver's `ErrorDetails` must carry: one `FieldValidationError`
    // naming the member and the code `FieldValidationCode` declares for a value the
    // service rejects.
    let expected_details = json!([{
        "FieldValidationError": {
            "Name": "ReceiverIdentifier",
            "Code": "INVALID_VALUE",
            "Message": "Invalid receiver identifier for this connection type.",
        }
    }]);

    let mut accepted_ids: Vec<String> = Vec::new();
    for (index, (case, connection_type, receiver, accepted)) in rows.into_iter().enumerate() {
        let request = invitation(
            "Sandbox",
            &format!("tok-{index}"),
            connection_type,
            receiver,
        );
        let (status, body) = emulator.call("CreateConnectionInvitation", &request).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: an addressable receiver is accepted, got {status}: {body}"
            );
            assert_eq!(
                body.get("OtherParticipantIdentifier").and_then(Value::as_str),
                Some(receiver),
                "case {case:?}: the stored invitation names the receiver it was addressed to: {body}"
            );
            accepted_ids.push(string(&body, "Id", case));
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: a receiver that is not addressable is refused, got {status}: {body}"
        );
        assert_error(case, (status, body.clone()), 400, "ValidationException");
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some("1 validation error detected."),
            "case {case:?}: the refusal counts the field it rejected: {body}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("REQUEST_VALIDATION_FAILED"),
            "case {case:?}: a refusal on the request's own members names its reason: {body}"
        );
        assert_eq!(
            body.get("ErrorDetails"),
            Some(&expected_details),
            "case {case:?}: the refusal names the member it rejected: {body}"
        );
    }

    // A refused create stored nothing: the catalog holds exactly the invitations the
    // addressable rows created, and no phantom invitation from a refused one.
    let listed = emulator
        .ok(
            "ListConnectionInvitations",
            &json!({ "Catalog": "Sandbox", "MaxResults": 100 }),
        )
        .await;
    let summaries = listed["ConnectionInvitationSummaries"]
        .as_array()
        .expect("ConnectionInvitationSummaries is a list");
    let listed_ids: Vec<String> = summaries
        .iter()
        .map(|summary| string(summary, "Id", "a listed invitation"))
        .collect();
    assert_eq!(
        listed_ids, accepted_ids,
        "a refused create must leave nothing to list: {listed}"
    );
}

/// The same rule on an empty catalog: when every create is refused, the list of the
/// catalog's invitations is empty — the state-visible half of the refusal.
#[tokio::test]
async fn a_refused_create_leaves_the_invitation_list_empty() {
    let emulator = Emulator::new();

    for (index, (connection_type, receiver)) in [
        ("OPPORTUNITY_COLLABORATION", "partner-0000000000001"),
        ("SUBSIDIARY", "partner-0000000000001"),
        ("OPPORTUNITY_COLLABORATION", "0000000000"),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{connection_type} to {receiver}");
        let answer = emulator
            .call(
                "CreateConnectionInvitation",
                &invitation(
                    "Sandbox",
                    &format!("tok-refused-{index}"),
                    connection_type,
                    receiver,
                ),
            )
            .await;
        assert_error(&case, answer, 400, "ValidationException");
    }

    let listed = emulator
        .ok(
            "ListConnectionInvitations",
            &json!({ "Catalog": "Sandbox" }),
        )
        .await;
    assert_eq!(
        listed["ConnectionInvitationSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "no invitation was created, so there is nothing to list: {listed}"
    );
    assert!(
        listed.get("NextToken").is_none_or(Value::is_null),
        "an empty page carries no NextToken: {listed}"
    );
}
