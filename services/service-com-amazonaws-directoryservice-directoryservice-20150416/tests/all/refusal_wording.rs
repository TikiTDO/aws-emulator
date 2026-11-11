// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two refusals a client reads a diagnostic out of, driven end
//! to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`).
//!
//! The rules under test, each asserted over a TABLE of operations rather than one
//! recorded call:
//!
//! * a call that needs a directory IN SERVICE and finds one that is not is refused with
//!   `DirectoryUnavailableException` whose diagnostic is the sentence Directory Service
//!   states that failure in — the operation it could not perform, the unquoted id the
//!   request asked for and the fact that the directory is unavailable, then the request-id
//!   suffix — which is a DIFFERENT sentence from the one the same id gets when it resolves
//!   to no directory at all, so a client can tell a directory that is not there from one
//!   that is not ready, while neither sentence discloses where in its build the directory
//!   is. Once the stage chain the create owed has run out, the same call is no longer
//!   refused that way, so the branch follows stored state.
//! * a value outside an enumerated type's value set is refused with the boundary's
//!   `ValidationException`, whose remediation hint lists that enum's admissible WIRE
//!   VALUES in the order this service lists them — descending — and not in the order the
//!   model declares them. Every other constraint class keeps the boundary's own wording.

use serde_json::{json, Value};

use crate::certificates::certificate_pem;
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, service, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// A `DirectoryId` that satisfies the modelled `^d-[0-9a-f]{10}$` and is created in no
/// test here, so the boundary admits it and a handler has to resolve it in the store.
const ABSENT_DIRECTORY_ID: &str = "d-0123456789";

/// The `Message` of a refusal body, whichever casing the envelope carries it in: a
/// modelled error of this service reports `Message`, the boundary's `ValidationException`
/// reports `message`.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// One row of the in-service table: an operation and an otherwise valid request body for
/// it, parameterised by the directory id it names.
struct UnavailableCase {
    case: &'static str,
    operation: &'static str,
    /// Whether the row needs a Managed Microsoft AD directory (`StartSchemaExtension`
    /// applies only to one), rather than the Simple AD every other row uses.
    microsoft_ad: bool,
    body: Box<dyn Fn(&str) -> Value>,
}

/// The operations that need a directory in service, each with a body that satisfies every
/// published constraint of its input so the request reaches that check. `certificate` is a
/// PEM-encoded X.509 certificate, which `RegisterCertificate` reads before it resolves the
/// directory.
///
/// The conditional-forwarder mutations are NOT here: Directory Service checks the
/// directory's stage as a PARAMETER of those calls and states the stages they need, which
/// `tests/all/refusal_texts.rs` is what asserts. `CreateComputer` is not here either: the
/// computer-account family states the directory-lookup sentence for this failure instead,
/// which `tests/all/computer_account_unavailable_wording.rs` is what asserts.
fn unavailable_rows(certificate: &str) -> Vec<UnavailableCase> {
    let certificate = certificate.to_string();
    vec![
        UnavailableCase {
            case: "AddIpRoutes",
            operation: "AddIpRoutes",
            microsoft_ad: false,
            body: Box::new(|id| {
                json!({
                    "DirectoryId": id,
                    "IpRoutes": [{ "CidrIp": "10.0.0.0/24", "Description": "office" }]
                })
            }),
        },
        UnavailableCase {
            case: "RemoveIpRoutes",
            operation: "RemoveIpRoutes",
            microsoft_ad: false,
            body: Box::new(|id| json!({ "DirectoryId": id, "CidrIps": ["10.0.0.0/24"] })),
        },
        UnavailableCase {
            case: "EnableDirectoryDataAccess",
            operation: "EnableDirectoryDataAccess",
            microsoft_ad: false,
            body: Box::new(|id| json!({ "DirectoryId": id })),
        },
        UnavailableCase {
            case: "RegisterCertificate",
            operation: "RegisterCertificate",
            microsoft_ad: false,
            body: Box::new(move |id| {
                json!({ "DirectoryId": id, "CertificateData": &certificate })
            }),
        },
        UnavailableCase {
            case: "StartSchemaExtension",
            operation: "StartSchemaExtension",
            microsoft_ad: true,
            body: Box::new(|id| {
                json!({
                    "DirectoryId": id,
                    "CreateSnapshotBeforeSchemaExtension": false,
                    "LdifContent": "dn: CN=Example,DC=corp,DC=example,DC=com\nchangetype: add\n",
                    "Description": "an extension"
                })
            }),
        },
    ]
}

/// The sentence this service states for a directory that is stored but not in service,
/// with the request id the answer itself reported appended — the shape every refusal of
/// this service's unavailable family carries.
fn unavailable_directory(directory_id: &str, request_id: &str) -> String {
    format!(
        "Operation could not be performed because directory {directory_id} is unavailable. : \
         RequestId: {request_id}"
    )
}

/// The sentence this service states for an id that resolves to NO directory, with the same
/// suffix — the lookup family's wording, which the unavailable one above is not.
fn unreachable_directory(directory_id: &str, request_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist. : RequestId: {request_id}")
}

/// The modelled `RequestId` the refusal reports.
fn request_id(parsed: &Value, case: &str) -> String {
    parsed["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RequestId, got {parsed}"))
        .to_string()
}

#[tokio::test]
async fn a_directory_that_is_not_in_service_is_refused_with_the_unavailable_sentence() {
    // `RegisterCertificate` reads its `CertificateData` before it resolves the directory,
    // so that row carries a real certificate (the sibling suite's builder).
    let (certificate, _) = certificate_pem(Some("unavailable.example.com"), (2032, 7, 7));
    let rows = unavailable_rows(&certificate);

    for row in &rows {
        // A fresh router per row: the window before the transition falls due is what this
        // row exercises, so each row starts its own directory's clock.
        let router = service();
        let directory_id = if row.microsoft_ad {
            create_microsoft_ad(
                &router,
                minimal_microsoft_ad("pending.corp.example.com"),
                row.case,
            )
            .await
        } else {
            create_directory(&router, minimal_create("pending.example.com"), row.case).await
        };

        let case = format!("{} on a directory still being built", row.case);
        let (status, parsed) = call(&router, row.operation, (row.body)(&directory_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("DirectoryUnavailableException"),
            "case {case:?}: expected the modelled unavailable refusal, got {parsed}"
        );
        let reported = request_id(&parsed, &case);
        assert_eq!(
            message(&parsed, &case),
            unavailable_directory(&directory_id, &reported),
            "case {case:?}: expected the unavailable-directory sentence, got {parsed}"
        );

        // The same call against an id that resolves to NO directory is refused in the
        // LOOKUP family's sentence instead: the two failures are worded differently, so a
        // client can tell them apart — and neither says where in its build a directory is.
        let absent_case = format!("{} on an id naming no directory", row.case);
        let (absent_status, absent) =
            call(&router, row.operation, (row.body)(ABSENT_DIRECTORY_ID)).await;
        assert_eq!(
            absent_status, 400,
            "case {absent_case:?}: expected 400, got {absent_status} with {absent}"
        );
        let absent_reported = request_id(&absent, &absent_case);
        assert_eq!(
            message(&absent, &absent_case),
            unreachable_directory(ABSENT_DIRECTORY_ID, &absent_reported),
            "case {absent_case:?}: expected the lookup sentence for an absent id, got {absent}"
        );

        // The refusal was about the directory being out of service, so once the
        // transition its create owed has fallen due the same call is no longer refused
        // that way.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let in_service = format!("{} once the directory is in service", row.case);
        let (_, answered) = call(&router, row.operation, (row.body)(&directory_id)).await;
        assert_ne!(
            answered["__type"],
            json!("DirectoryUnavailableException"),
            "case {in_service:?}: expected the unavailable refusal to be gone, got {answered}"
        );
    }
}

/// One row of the enum table: a request carrying one value outside an enumerated member's
/// value set, and the list this service states as that member's admissible values.
struct EnumCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member path a clause names, as the boundary spells it (lowerCamel, dotted
    /// through a nested structure).
    member: &'static str,
    /// The value the request sent, echoed back by the clause.
    submitted: &'static str,
    /// The admissible values as this service LISTS them.
    stated: &'static str,
}

/// The whole message a request breaking ONE member constraint is refused with: the
/// boundary's envelope and clause, with the enum's value list in this service's order.
fn one_enum_error(submitted: &str, member: &str, stated: &str) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{stated}]"
    )
}

/// The enum table. Every row names a different enum of this service's model, and the
/// `stated` list is that enum's wire values as a refusal lists them; the DECLARED order
/// in each comment is the model's own
/// (`smithy_query(query="shape", shape="<Enum>")`), which is what the list must NOT
/// follow.
fn enum_rows() -> Vec<EnumCase> {
    vec![
        // ClientAuthenticationType, declared [SmartCard, SmartCardOrPassword].
        EnumCase {
            case: "ClientAuthenticationType on DescribeClientAuthenticationSettings",
            operation: "DescribeClientAuthenticationSettings",
            body: json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "Type": "SMART_CARD" }),
            member: "type",
            submitted: "SMART_CARD",
            stated: "SmartCardOrPassword, SmartCard",
        },
        // The same enum on another operation: the list is the enum's, not the
        // operation's.
        EnumCase {
            case: "ClientAuthenticationType on EnableClientAuthentication",
            operation: "EnableClientAuthentication",
            body: json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "Type": "smartcard" }),
            member: "type",
            submitted: "smartcard",
            stated: "SmartCardOrPassword, SmartCard",
        },
        // UpdateType, declared [OS, NETWORK, SIZE].
        EnumCase {
            case: "UpdateType on DescribeUpdateDirectory",
            operation: "DescribeUpdateDirectory",
            body: json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "UpdateType": "Os" }),
            member: "updateType",
            submitted: "Os",
            stated: "SIZE, OS, NETWORK",
        },
        // LDAPSType, declared [Client]: one value, so only the frame can differ.
        EnumCase {
            case: "LDAPSType on DescribeLDAPSSettings",
            operation: "DescribeLDAPSSettings",
            body: json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "Type": "CLIENT" }),
            member: "type",
            submitted: "CLIENT",
            stated: "Client",
        },
        // DirectorySize, declared [Small, Large].
        EnumCase {
            case: "DirectorySize on CreateDirectory",
            operation: "CreateDirectory",
            body: json!({
                "Name": "sized.example.com",
                "Password": PASSWORD,
                "Size": "Medium"
            }),
            member: "size",
            submitted: "Medium",
            stated: "Small, Large",
        },
        // TrustDirection, declared [One-Way: Outgoing, One-Way: Incoming, Two-Way].
        EnumCase {
            case: "TrustDirection on CreateTrust",
            operation: "CreateTrust",
            body: json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "RemoteDomainName": "remote.example.com",
                "TrustPassword": "Trust-Pw-1",
                "TrustDirection": "TwoWay"
            }),
            member: "trustDirection",
            submitted: "TwoWay",
            stated: "Two-Way, One-Way: Outgoing, One-Way: Incoming",
        },
        // ShareMethod, declared [ORGANIZATIONS, HANDSHAKE].
        EnumCase {
            case: "ShareMethod on ShareDirectory",
            operation: "ShareDirectory",
            body: json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "ShareTarget": { "Id": "111122223333", "Type": "ACCOUNT" },
                "ShareMethod": "organizations"
            }),
            member: "shareMethod",
            submitted: "organizations",
            stated: "ORGANIZATIONS, HANDSHAKE",
        },
        // RadiusAuthenticationProtocol, declared [PAP, CHAP, MS-CHAPv1, MS-CHAPv2], on a
        // member nested inside a structure, so the clause names the dotted path.
        EnumCase {
            case: "RadiusAuthenticationProtocol on EnableRadius",
            operation: "EnableRadius",
            body: json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "RadiusSettings": { "AuthenticationProtocol": "MSCHAPV2" }
            }),
            member: "radiusSettings.authenticationProtocol",
            submitted: "MSCHAPV2",
            stated: "PAP, MS-CHAPv2, MS-CHAPv1, CHAP",
        },
        // CertificateType, declared [ClientCertAuth, ClientLDAPS].
        EnumCase {
            case: "CertificateType on RegisterCertificate",
            operation: "RegisterCertificate",
            body: json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "CertificateData": "-----BEGIN CERTIFICATE-----",
                "Type": "CLIENT_LDAPS"
            }),
            member: "type",
            submitted: "CLIENT_LDAPS",
            stated: "ClientLDAPS, ClientCertAuth",
        },
    ]
}

#[tokio::test]
async fn an_enum_value_set_is_listed_in_the_order_this_service_lists_it() {
    let router = service();

    for row in enum_rows() {
        let case = row.case;
        let (status, parsed) = call(&router, row.operation, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the boundary validation envelope, got {parsed}"
        );
        assert_eq!(
            message(&parsed, case),
            one_enum_error(row.submitted, row.member, row.stated),
            "case {case:?}: expected the value set in this service's order, got {parsed}"
        );
        // The boundary refused the request before anything was resolved, so the envelope
        // carries none of a modelled error's members.
        assert!(
            parsed["Message"].is_null() && parsed["RequestId"].is_null(),
            "case {case:?}: expected no modelled error members on the boundary envelope, \
             got {parsed}"
        );
    }
}

#[tokio::test]
async fn the_refusal_is_the_enum_one_whatever_the_directory_id_resolves_to() {
    let router = service();

    // A stored directory, so a refusal cannot be read as "there was nothing at all", and
    // one id that resolves to nothing: the enum is checked at the boundary, before either
    // is looked up, so both answer the same clause.
    let stored = create_directory(
        &router,
        minimal_create("resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (subject, directory_id) in [
        ("a stored directory", stored.as_str()),
        ("an id naming no directory", ABSENT_DIRECTORY_ID),
    ] {
        let case = format!("DescribeClientAuthenticationSettings against {subject}");
        let (status, parsed) = call(
            &router,
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": directory_id, "Type": "SMART_CARD" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            message(&parsed, &case),
            one_enum_error("SMART_CARD", "type", "SmartCardOrPassword, SmartCard"),
            "case {case:?}: expected the enum clause rather than a resolution error, got \
             {parsed}"
        );
    }

    // A value INSIDE the set is not refused that way: the same operation answers it.
    let answer = ok(
        &router,
        "DescribeClientAuthenticationSettings",
        json!({ "DirectoryId": &stored, "Type": "SmartCard" }),
        "a value inside the set",
    )
    .await;
    assert!(
        answer["ClientAuthenticationSettingsInfo"].is_array(),
        "case \"a value inside the set\": expected the operation to answer, got {answer}"
    );
}

#[tokio::test]
async fn every_other_constraint_class_keeps_the_boundary_wording() {
    let router = service();

    // One row per constraint class this service's inputs declare besides an enum value
    // set: a numeric `@range` bound, a `@length` bound and a `@pattern`. None of them
    // names a value set, so none of them has its clause restated — the `@pattern` row's
    // EXPRESSION is quoted as the model declares it, anchors and all
    // (`tests/all/validation_pattern_wording.rs` pins that rendering over this service's
    // patterns), and nothing else about the clause changes.
    let rows = [
        (
            "a @range minimum",
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "Type": "SmartCard", "Limit": 0 }),
            "Member must have value greater than or equal to 1",
        ),
        (
            "a @range maximum",
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "Type": "SmartCard", "Limit": 51 }),
            "Member must have value less than or equal to 50",
        ),
        (
            "a @length minimum",
            "DescribeUpdateDirectory",
            json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "UpdateType": "OS",
                "RegionName": "short"
            }),
            "Member must have length greater than or equal to 8",
        ),
        (
            "a @pattern",
            "DescribeUpdateDirectory",
            json!({ "DirectoryId": "not-a-directory-id", "UpdateType": "OS" }),
            "Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$",
        ),
    ];

    for (case, operation, body, requirement) in rows {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        let stated = message(&parsed, case);
        assert!(
            stated.ends_with(requirement),
            "case {case:?}: expected the clause to end in {requirement:?}, got {stated:?}"
        );
        assert!(
            !stated.contains("enum value set"),
            "case {case:?}: expected no value set in this clause, got {stated:?}"
        );
    }
}
