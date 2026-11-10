// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request it will not serve, over the whole class each
//! rule covers.
//!
//! Two rules are asserted here, each over a TABLE whose rows differ along what the
//! rule generalises over — which member is wrong, which value of the other member
//! made it wrong, which operation, and how deep in the request the member sits:
//!
//! 1. A cross-member requirement this service's model documents — a member required
//!    when another member holds a given value, or settable only when it does — is
//!    enforced BEFORE anything is persisted and refused with the modelled
//!    `InvalidRequestException` (HTTP 400).
//! 2. A value outside an enumerated member's modelled value set is refused with
//!    HTTP 400 `ValidationException` whose message names the member path and the
//!    value set, and never the rejected value.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator, ROLE};

/// A VPC id for the rows that host an endpoint in a VPC. `VpcId` carries no modelled
/// constraint; this is the shape EC2 hands out.
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// A subnet id for the same rows. `SubnetId` carries no modelled constraint either.
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// Satisfies `SecurityGroupId`'s modelled `len 11..20`, pattern `^sg-[0-9a-f]{8,17}$`.
const SECURITY_GROUP_ID: &str = "sg-0123456789abcdef0";

/// Satisfies `VpcEndpointId`'s modelled `len 22..22`, pattern `^vpce-[0-9a-f]{17}$`.
const VPC_ENDPOINT_ID: &str = "vpce-0123456789abcdef0";

/// An Elastic IP allocation id. `AddressAllocationId` carries no modelled constraint.
const ALLOCATION_ID: &str = "eipalloc-0123456789abcdef0";

/// An ACM certificate ARN for the FTPS rows; `Certificate` is `len 0..1600` with no
/// pattern.
const CERTIFICATE: &str = "arn:aws:acm:us-east-1:000000000000:certificate/abcd";

/// Satisfies `Function`'s modelled `len 1..170`, pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_IDP: &str = "arn:aws:lambda:us-east-1:000000000000:function:idp";

/// Satisfies `DirectoryId`'s modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`.
const DIRECTORY_ID: &str = "d-0123456789";

/// A `ServerId` that satisfies the modelled `^s-([0-9a-f]{17})$` without ever having
/// been minted. The rows that use it are refused before any lookup, so a well-formed
/// id is all they need.
const UNMINTED_SERVER_ID: &str = "s-0000000000000dead";

/// Assert that `answer` is the modelled cross-member refusal: HTTP 400
/// `InvalidRequestException` whose `Message` names each of `named` — the member that
/// was wrong, and the member whose value made it wrong — and that it answered no
/// identifier at all, so nothing was minted for it.
fn assert_cross_member_refusal(case: &str, answer: &Answer, named: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    for fragment in named {
        assert!(
            message.contains(fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
        );
    }
    assert!(
        answer.body.get("ServerId").is_none() && answer.body.get("ProfileId").is_none(),
        "case {case:?}: a refused create must answer no identifier, got {}",
        answer.body
    );
}

/// Create the server `request` asks for and read it back through `DescribeServer`,
/// asserting the stored configuration is the one submitted. Returns its `ServerId`.
async fn create_and_read_back(emulator: &Emulator, case: &str, request: &Value) -> String {
    let answer = emulator.call("CreateServer", request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    let server_id = answer
        .get("ServerId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ServerId, got {}", answer.body))
        .to_string();

    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected DescribeServer 200, got {} with body {}",
        described.status, described.body
    );
    let server = described.get("Server").clone();
    // `EndpointType` defaults to PUBLIC when the request names none.
    let want_endpoint_type = request
        .get("EndpointType")
        .cloned()
        .unwrap_or_else(|| json!("PUBLIC"));
    assert_eq!(
        member_of(&server, "EndpointType"),
        &want_endpoint_type,
        "case {case:?}: expected the stored EndpointType to be {want_endpoint_type}, got {}",
        member_of(&server, "EndpointType")
    );
    if let Some(details) = request.get("EndpointDetails") {
        assert_eq!(
            member_of(&server, "EndpointDetails"),
            details,
            "case {case:?}: expected the stored EndpointDetails to be {details}, got {}",
            member_of(&server, "EndpointDetails")
        );
    }
    server_id
}

/// The `ServerId`s `ListServers` currently holds, sorted.
async fn listed_server_ids(emulator: &Emulator) -> Vec<String> {
    let listed = emulator.call("ListServers", json!({})).await;
    let mut ids: Vec<String> = listed
        .get("Servers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|server| server.get("ServerId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// Cross-member requirements the model documents
// ---------------------------------------------------------------------------

/// RULE: a server's VPC endpoint settings are required exactly where `EndpointType`
/// hosts the endpoint in a VPC, and each `EndpointDetails` member is settable only
/// under the `EndpointType` (and `IpAddressType`) its own documentation names;
/// anything else is refused with `InvalidRequestException` (HTTP 400) and persists
/// nothing.
///
/// The rows span the `EndpointType` value set (`[PUBLIC, VPC, VPC_ENDPOINT]`, plus
/// the absent member that resolves to `PUBLIC`) crossed with which `EndpointDetails`
/// member was submitted — including the one the issue never cited, `VpcEndpointId`
/// under `VPC`, and the `DUALSTACK`/`AddressAllocationIds` pair.
#[tokio::test]
async fn endpoint_details_are_required_exactly_where_the_endpoint_type_hosts_in_a_vpc() {
    let emulator = Emulator::new();

    // `None` is an accepted configuration; `Some(fragments)` is a refusal whose
    // message must name each fragment.
    let cases: Vec<(&str, Value, Option<Vec<&str>>)> = vec![
        ("no EndpointType at all", json!({}), None),
        ("PUBLIC with no details", json!({"EndpointType": "PUBLIC"}), None),
        (
            "VPC with no details",
            json!({"EndpointType": "VPC"}),
            // The service's fixed text for a VPC-hosted endpoint with no settings; it
            // names neither the member nor the endpoint type, so both rows that host in
            // a VPC are refused with the same sentence.
            Some(vec!["EndpointDetails required for VPC"]),
        ),
        (
            "VPC_ENDPOINT with no details",
            json!({"EndpointType": "VPC_ENDPOINT"}),
            Some(vec!["EndpointDetails required for VPC"]),
        ),
        (
            "VPC with a VpcId",
            json!({"EndpointType": "VPC", "EndpointDetails": {"VpcId": VPC_ID}}),
            None,
        ),
        (
            "VPC with subnets and security groups",
            json!({
                "EndpointType": "VPC",
                "EndpointDetails": {
                    "SubnetIds": [SUBNET_ID],
                    "SecurityGroupIds": [SECURITY_GROUP_ID],
                    "VpcId": VPC_ID,
                },
            }),
            None,
        ),
        (
            "VPC_ENDPOINT with its VpcEndpointId",
            json!({
                "EndpointType": "VPC_ENDPOINT",
                "EndpointDetails": {"VpcEndpointId": VPC_ENDPOINT_ID},
            }),
            None,
        ),
        (
            "VPC carrying a VpcEndpointId",
            json!({
                "EndpointType": "VPC",
                "EndpointDetails": {"VpcId": VPC_ID, "VpcEndpointId": VPC_ENDPOINT_ID},
            }),
            Some(vec![
                "EndpointDetails.VpcEndpointId",
                "EndpointType is VPC_ENDPOINT",
            ]),
        ),
        (
            "VPC_ENDPOINT carrying a VpcId",
            json!({
                "EndpointType": "VPC_ENDPOINT",
                "EndpointDetails": {"VpcEndpointId": VPC_ENDPOINT_ID, "VpcId": VPC_ID},
            }),
            Some(vec!["EndpointDetails.VpcId", "EndpointType is VPC"]),
        ),
        (
            "PUBLIC carrying subnets",
            json!({
                "EndpointType": "PUBLIC",
                "EndpointDetails": {"SubnetIds": [SUBNET_ID]},
            }),
            Some(vec!["EndpointDetails.SubnetIds", "EndpointType is VPC"]),
        ),
        (
            "VPC with address allocations on the default IPV4",
            json!({
                "EndpointType": "VPC",
                "EndpointDetails": {
                    "VpcId": VPC_ID,
                    "SubnetIds": [SUBNET_ID],
                    "AddressAllocationIds": [ALLOCATION_ID],
                },
            }),
            None,
        ),
        (
            "VPC with address allocations on DUALSTACK",
            json!({
                "EndpointType": "VPC",
                "IpAddressType": "DUALSTACK",
                "EndpointDetails": {
                    "VpcId": VPC_ID,
                    "AddressAllocationIds": [ALLOCATION_ID],
                },
            }),
            Some(vec![
                "EndpointDetails.AddressAllocationIds",
                "IpAddressType is DUALSTACK",
            ]),
        ),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, request, refusal) in cases {
        match refusal {
            Some(named) => {
                let answer = emulator.call("CreateServer", request).await;
                assert_cross_member_refusal(case, &answer, &named);
            }
            None => accepted.push(create_and_read_back(&emulator, case, &request).await),
        }
    }

    // A refused create wrote nothing: the store holds exactly the accepted servers.
    accepted.sort();
    let listed = listed_server_ids(&emulator).await;
    assert_eq!(
        listed, accepted,
        "case \"the store after the whole table\": expected only the accepted servers \
         {accepted:?}, got {listed:?}"
    );
}

/// RULE (same rule, another documented pair): `IdentityProviderDetails` is required
/// for exactly the three `IdentityProviderType` values the model documents it as
/// required for, and cannot be specified for `SERVICE_MANAGED`.
///
/// The rows span the whole `IdentityProviderType` value set
/// (`[SERVICE_MANAGED, API_GATEWAY, AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`) plus the
/// absent member that resolves to `SERVICE_MANAGED`, crossed with whether the details
/// were sent.
#[tokio::test]
async fn identity_provider_details_are_required_exactly_for_the_types_that_use_them() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value, Option<Vec<&str>>)> = vec![
        ("no IdentityProviderType at all", json!({}), None),
        (
            "SERVICE_MANAGED without details",
            json!({"IdentityProviderType": "SERVICE_MANAGED"}),
            None,
        ),
        (
            "SERVICE_MANAGED with details",
            json!({
                "IdentityProviderType": "SERVICE_MANAGED",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
            Some(vec![
                "IdentityProviderDetails",
                "IdentityProviderType is SERVICE_MANAGED",
            ]),
        ),
        (
            "details with no IdentityProviderType, which resolves to SERVICE_MANAGED",
            json!({"IdentityProviderDetails": {"Url": "https://example.invalid/auth"}}),
            Some(vec![
                "IdentityProviderDetails",
                "IdentityProviderType is SERVICE_MANAGED",
            ]),
        ),
        (
            "API_GATEWAY without details",
            json!({"IdentityProviderType": "API_GATEWAY"}),
            Some(vec![
                "IdentityProviderDetails",
                "IdentityProviderType is API_GATEWAY",
            ]),
        ),
        (
            "AWS_DIRECTORY_SERVICE without details",
            json!({"IdentityProviderType": "AWS_DIRECTORY_SERVICE"}),
            Some(vec![
                "IdentityProviderDetails",
                "IdentityProviderType is AWS_DIRECTORY_SERVICE",
            ]),
        ),
        (
            "AWS_LAMBDA without details",
            json!({"IdentityProviderType": "AWS_LAMBDA"}),
            Some(vec![
                "IdentityProviderDetails",
                "IdentityProviderType is AWS_LAMBDA",
            ]),
        ),
        (
            "API_GATEWAY with its URL",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
            None,
        ),
        (
            "AWS_DIRECTORY_SERVICE with its directory id",
            json!({
                "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
            }),
            None,
        ),
        (
            "AWS_LAMBDA with its function",
            json!({
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
            }),
            None,
        ),
    ];

    for (case, request, refusal) in cases {
        match refusal {
            Some(named) => {
                let answer = emulator.call("CreateServer", request).await;
                assert_cross_member_refusal(case, &answer, &named);
            }
            None => {
                create_and_read_back(&emulator, case, &request).await;
            }
        }
    }
}

/// RULE (same rule, the requirements `Protocols` states): a protocol the model says
/// needs a certificate, a VPC-hosted endpoint or a custom identity provider is
/// refused unless the request carries them.
///
/// The rows differ along which protocol is named, how many protocols the list holds,
/// and which of the three requirements the request leaves unsatisfied — including an
/// empty `Certificate`, which the member's `len 0..1600` admits but which names no
/// certificate.
#[tokio::test]
async fn protocol_requirements_on_other_members_are_enforced() {
    let emulator = Emulator::new();

    let vpc = json!({"VpcId": VPC_ID, "SubnetIds": [SUBNET_ID]});
    let lambda_idp = json!({"Function": LAMBDA_IDP});

    let cases: Vec<(&str, Value, Option<Vec<&str>>)> = vec![
        (
            "SFTP alone on a public, service-managed server",
            json!({"Protocols": ["SFTP"]}),
            None,
        ),
        (
            "FTPS with no certificate",
            json!({
                "Protocols": ["SFTP", "FTPS"],
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": lambda_idp,
            }),
            Some(vec!["Certificate", "Protocols includes FTPS"]),
        ),
        (
            "FTPS with an empty certificate",
            json!({
                "Protocols": ["FTPS"],
                "Certificate": "",
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": lambda_idp,
            }),
            Some(vec!["Certificate", "Protocols includes FTPS"]),
        ),
        (
            "FTPS on a public endpoint",
            json!({
                "Protocols": ["FTPS"],
                "Certificate": CERTIFICATE,
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": lambda_idp,
            }),
            Some(vec!["EndpointType", "Protocols includes FTPS"]),
        ),
        (
            "FTP on a VPC endpoint with the service-managed provider",
            json!({
                "Protocols": ["FTP"],
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            // The identity-provider half of the requirement, which is evaluated
            // before the endpoint-type half: this provider cannot serve FTP at all,
            // whatever the endpoint type (here already the required `VPC`).
            Some(vec![
                "FTP and FTPS protocols unsupported",
                "IdentityProviderType SERVICE_MANAGED",
            ]),
        ),
        (
            "AS2 on a public endpoint",
            json!({"Protocols": ["AS2"]}),
            Some(vec!["EndpointType", "Protocols includes AS2"]),
        ),
        (
            "AS2 over the EFS domain",
            json!({
                "Protocols": ["AS2"],
                "Domain": "EFS",
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            Some(vec!["Domain", "Protocols includes AS2"]),
        ),
        (
            "AS2 on a VPC endpoint over S3",
            json!({
                "Protocols": ["AS2"],
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            None,
        ),
        (
            "all four protocols with every requirement satisfied",
            json!({
                "Protocols": ["SFTP", "FTP", "FTPS", "AS2"],
                "Certificate": CERTIFICATE,
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": lambda_idp,
            }),
            None,
        ),
    ];

    for (case, request, refusal) in cases {
        match refusal {
            Some(named) => {
                let answer = emulator.call("CreateServer", request).await;
                assert_cross_member_refusal(case, &answer, &named);
            }
            None => {
                create_and_read_back(&emulator, case, &request).await;
            }
        }
    }
}

/// RULE (same rule, another operation): a user's `HomeDirectoryMappings` are required
/// when `HomeDirectoryType` is `LOGICAL` and settable only then, so each direction of
/// that documented pair is refused with `InvalidRequestException` (HTTP 400) and
/// persists no user.
///
/// The rows span the `HomeDirectoryType` value set (`[PATH, LOGICAL]`, plus the absent
/// member) crossed with how many mappings were sent.
#[tokio::test]
async fn home_directory_mappings_are_required_exactly_for_a_logical_home_directory() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let one_mapping = json!([{"Entry": "/", "Target": "/bucket/home"}]);
    let two_mappings = json!([
        {"Entry": "/", "Target": "/bucket/home"},
        {"Entry": "/shared", "Target": "/bucket/shared", "Type": "DIRECTORY"},
    ]);

    // (case, user name, the members under test, the refusal or `None` to accept)
    let cases: Vec<(&str, &str, Value, Option<Vec<&str>>)> = vec![
        ("no home directory at all", "u-none", json!({}), None),
        (
            "PATH with an absolute home directory",
            "u-path",
            json!({"HomeDirectoryType": "PATH", "HomeDirectory": "/bucket/u-path"}),
            None,
        ),
        (
            "LOGICAL with one mapping",
            "u-logical-1",
            json!({"HomeDirectoryType": "LOGICAL", "HomeDirectoryMappings": one_mapping}),
            None,
        ),
        (
            "LOGICAL with two mappings",
            "u-logical-2",
            json!({"HomeDirectoryType": "LOGICAL", "HomeDirectoryMappings": two_mappings}),
            None,
        ),
        (
            "LOGICAL with no mappings",
            "u-logical-0",
            json!({"HomeDirectoryType": "LOGICAL"}),
            // The violated requirement is the LIST's own modelled minimum, so that is
            // what the refusal states. See `error_message_rules.rs` for the whole text.
            Some(vec!["HomeDirectoryMappings", "must not be empty"]),
        ),
        (
            "PATH with mappings",
            "u-path-mapped",
            json!({"HomeDirectoryType": "PATH", "HomeDirectoryMappings": one_mapping}),
            Some(vec!["HomeDirectoryMappings", "HomeDirectoryType is LOGICAL"]),
        ),
        (
            "mappings with no HomeDirectoryType",
            "u-untyped-mapped",
            json!({"HomeDirectoryMappings": two_mappings}),
            Some(vec!["HomeDirectoryMappings", "HomeDirectoryType is LOGICAL"]),
        ),
    ];

    for (case, user_name, members, refusal) in cases {
        let mut request = json!({"ServerId": &server_id, "UserName": user_name, "Role": ROLE});
        for (member, value) in members.as_object().expect("the members are an object") {
            request[member] = value.clone();
        }
        let answer = emulator.call("CreateUser", request).await;

        match refusal {
            Some(named) => {
                assert_cross_member_refusal(case, &answer, &named);
                assert!(
                    answer.body.get("UserName").is_none(),
                    "case {case:?}: a refused create must answer no UserName, got {}",
                    answer.body
                );
                let described = emulator
                    .call(
                        "DescribeUser",
                        json!({"ServerId": &server_id, "UserName": user_name}),
                    )
                    .await;
                // The user is absent, whatever status this service words a
                // not-found refusal with (that choice is another issue's).
                assert_eq!(
                    described.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: a refused create must persist no user, got {} with body {}",
                    described.status,
                    described.body
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with body {}",
                    answer.status, answer.body
                );
                let described = emulator
                    .call(
                        "DescribeUser",
                        json!({"ServerId": &server_id, "UserName": user_name}),
                    )
                    .await;
                let user = described.get("User").clone();
                for (member, want) in members.as_object().expect("an object") {
                    assert_eq!(
                        member_of(&user, member),
                        want,
                        "case {case:?}: expected the stored {member} to be {want}, got {}",
                        member_of(&user, member)
                    );
                }
            }
        }
    }
}

/// A refusal for a `ServerId` that was never minted is still the cross-member refusal:
/// the request is wrong whatever the store holds, so it is refused before any lookup.
#[tokio::test]
async fn a_cross_member_refusal_precedes_the_resource_lookup() {
    let emulator = Emulator::new();

    let answer = emulator
        .call(
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-logical",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
            }),
        )
        .await;
    assert_cross_member_refusal(
        "LOGICAL with no mappings on an unminted server",
        &answer,
        &["HomeDirectoryMappings", "must not be empty"],
    );
}

// ---------------------------------------------------------------------------
// Enum-constraint refusals
// ---------------------------------------------------------------------------

/// The message for ONE enum-constraint violation, written here from the rule rather
/// than from the implementation: the AWS envelope over a clause that names the member
/// PATH and the value set the member must satisfy — and nothing of the value that was
/// rejected.
fn enum_refusal_message(path: &str, allowed: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    )
}

/// One row of the enum-refusal table: the case name, the operation, the request, the
/// member path the message must name, the value set as this service LISTS it, and the
/// value the row sends for that member.
type EnumCase<'a> = (&'a str, &'a str, Value, &'a str, Vec<&'a str>, &'a str);

/// RULE: a value outside an enumerated member's modelled value set is refused with
/// HTTP 400 `ValidationException` whose message names the member path and the value
/// set, and never quotes the rejected value back to the caller.
///
/// The rows differ along the operation, the enum, the number of values in the set, and
/// how deep the member sits in the request: a top-level member, a member of a nested
/// structure, and a member of a structure inside a list. The value sets are the ones
/// this service's model declares (`smithy_query(query="shape", shape="…#Domain,…")`).
#[tokio::test]
async fn an_enum_refusal_names_the_member_and_the_value_set_but_not_the_value() {
    let emulator = Emulator::new();

    // (case, operation, request, member path, the value set as listed, rejected value)
    let cases: Vec<EnumCase<'_>> = vec![
        (
            "CreateProfile ProfileType",
            "CreateProfile",
            json!({"As2Id": "partner-as2-id", "ProfileType": "BOGUS"}),
            "profileType",
            // This service publishes its OWN listing of this enum's values
            // (`crate::enum_value_set_listing` is that rule's suite); the model declares
            // the same membership in the other order.
            vec!["PARTNER", "LOCAL"],
            "BOGUS",
        ),
        (
            "CreateServer Domain",
            "CreateServer",
            json!({"Domain": "GLACIER"}),
            "domain",
            vec!["S3", "EFS"],
            "GLACIER",
        ),
        (
            "CreateServer EndpointType",
            "CreateServer",
            json!({"EndpointType": "PRIVATE"}),
            "endpointType",
            vec!["PUBLIC", "VPC", "VPC_ENDPOINT"],
            "PRIVATE",
        ),
        (
            "CreateServer IdentityProviderType",
            "CreateServer",
            json!({"IdentityProviderType": "COGNITO"}),
            "identityProviderType",
            vec![
                "SERVICE_MANAGED",
                "API_GATEWAY",
                "AWS_DIRECTORY_SERVICE",
                "AWS_LAMBDA",
            ],
            "COGNITO",
        ),
        (
            "CreateServer IpAddressType",
            "CreateServer",
            json!({"IpAddressType": "IPV6"}),
            "ipAddressType",
            vec!["IPV4", "DUALSTACK"],
            "IPV6",
        ),
        (
            "CreateServer S3StorageOptions.DirectoryListingOptimization (nested)",
            "CreateServer",
            json!({"S3StorageOptions": {"DirectoryListingOptimization": "MAYBE"}}),
            "s3StorageOptions.directoryListingOptimization",
            vec!["ENABLED", "DISABLED"],
            "MAYBE",
        ),
        (
            "CreateUser HomeDirectoryType",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-enum",
                "Role": ROLE,
                "HomeDirectoryType": "ABSOLUTE",
            }),
            "homeDirectoryType",
            vec!["PATH", "LOGICAL"],
            "ABSOLUTE",
        ),
        (
            "CreateUser HomeDirectoryMappings[2].Type (inside a list)",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-enum",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [
                    {"Entry": "/", "Target": "/bucket/home"},
                    {"Entry": "/shared", "Target": "/bucket/shared", "Type": "SYMLINK"},
                ],
            }),
            "homeDirectoryMappings.2.member.type",
            vec!["FILE", "DIRECTORY"],
            "SYMLINK",
        ),
    ];

    for (case, op, request, path, allowed, rejected) in cases {
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message,
            enum_refusal_message(path, &allowed),
            "case {case:?}: expected the member and its value set, got {message:?}"
        );
        assert!(
            !message.contains(rejected),
            "case {case:?}: expected the rejected value {rejected:?} to be absent from the \
             message, got {message:?}"
        );
    }
}

/// RULE (the same refusal, every other constraint class): a violation of a
/// single-member constraint that is not an enum value set is answered as HTTP 400
/// `ValidationException` naming the member and the constraint it failed, and still
/// never the submitted value.
///
/// The rows differ along the constraint class — an absent required member, a pattern,
/// a length and a range — so the enum rewording above cannot silently swallow them.
/// `error_message_rules.rs` pins each class's whole message; this keeps the
/// member-naming assertion beside the enum rows it belongs with.
#[tokio::test]
async fn the_other_constraint_classes_are_still_refused_naming_member_and_constraint() {
    let emulator = Emulator::new();

    // (case, operation, request, the member path the message must name)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "absent required member",
            "CreateProfile",
            json!({"As2Id": "partner-as2-id"}),
            "profileType",
        ),
        (
            "pattern",
            "CreateUser",
            json!({"ServerId": UNMINTED_SERVER_ID, "UserName": "a b", "Role": ROLE}),
            "userName",
        ),
        (
            "length",
            "CreateUser",
            json!({"ServerId": UNMINTED_SERVER_ID, "UserName": "ab", "Role": ROLE}),
            "userName",
        ),
        (
            "range",
            "ListServers",
            json!({"MaxResults": 0}),
            "maxResults",
        ),
    ];

    for (case, op, request, named) in cases {
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
    }
}
