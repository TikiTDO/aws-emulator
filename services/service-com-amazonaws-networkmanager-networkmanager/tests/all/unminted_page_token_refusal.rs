// SPDX-License-Identifier: CC0-1.0

//! Every paginated listing of this service refuses a `NextToken` it did not mint with ONE
//! refusal, driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`).
//!
//! The rule under test: a request whose single input member `NextToken` carries a value
//! this service never issued is itself a well-formed request, so it is refused as the
//! member failure it is — `ValidationException` (HTTP 400) with `Reason`
//! `FieldValidationFailed` and one `Fields` entry naming `NextToken` — and every listing
//! of the service answers the SAME body for it, whatever it lists. `CannotParse` is the
//! `Reason` this service keeps for a document it could not read as the format it is
//! written in (a resource policy that is no JSON), not for a member value it rejects.
//!
//! The tables below span that class: every listing the service pages with a token — all 24
//! of them, reached at the method and URI the model binds each to (`smithy_query
//! query="digest"`) — crossed with tokens differing in alphabet (not hex at all, lowercase
//! hex, uppercase hex), in length (odd-length hex, and the modelled 2048-character bound)
//! and in what makes them unusable (no hex, hex of text this service never prefixes its
//! tokens with). The counter-case is in the second test: a token one of those listings
//! really did mint is honoured by it, so the refusal above is about the token and not
//! about pagination.
//!
//! Nothing here reaches into `crate::state`: every resource is created and every token is
//! obtained through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::core_network_at;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The `Reason` a `ValidationException` carries when ONE input member is at fault, from
/// the modelled enumeration (`smithy_query shape=ValidationExceptionReason`:
/// `UnknownOperation`, `CannotParse`, `FieldValidationFailed`, `Other`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The input member every paginated listing carries its continuation token in
/// (`NextToken`, `@httpQuery("nextToken")` on each listing's request shape).
const FIELD_NEXT_TOKEN: &str = "NextToken";

/// The Region the core network below configures an edge in, and the segment its policy
/// declares — the two coordinates `ListCoreNetworkRoutingInformation` needs to address a
/// route table at all.
const EDGE_LOCATION: &str = "us-west-2";
/// See [`EDGE_LOCATION`]: the segment name `crate::attachment_lifecycle::policy_document`
/// declares.
const SEGMENT_NAME: &str = "shared";

/// One paginated listing under test: the case name and the request that reaches it.
pub(crate) struct Listing {
    pub(crate) case: &'static str,
    pub(crate) method: &'static str,
    pub(crate) uri: String,
    pub(crate) body: Option<Value>,
}

/// A listing reached with a GET and no request body.
fn listing(case: &'static str, uri: String) -> Listing {
    Listing {
        case,
        method: "GET",
        uri,
        body: None,
    }
}

/// Every operation of this service that pages with a `NextToken`, at the method and URI
/// the model binds it to, against the `global_network` and `core_network` the test set up.
pub(crate) fn listings(global_network: &str, core_network: &str, device: &str) -> Vec<Listing> {
    vec![
        listing(
            "ListOrganizationServiceAccessStatus",
            "/organizations/service-access".to_string(),
        ),
        listing("DescribeGlobalNetworks", "/global-networks".to_string()),
        listing(
            "GetSites",
            format!("/global-networks/{global_network}/sites"),
        ),
        listing(
            "GetDevices",
            format!("/global-networks/{global_network}/devices"),
        ),
        listing(
            "GetLinks",
            format!("/global-networks/{global_network}/links"),
        ),
        listing(
            "GetConnections",
            format!("/global-networks/{global_network}/connections"),
        ),
        // `GetLinkAssociations` selects on a device or a link — one of the two is required
        // — so this row names the device the fixture created.
        listing(
            "GetLinkAssociations",
            format!("/global-networks/{global_network}/link-associations?deviceId={device}"),
        ),
        listing(
            "GetConnectPeerAssociations",
            format!("/global-networks/{global_network}/connect-peer-associations"),
        ),
        listing(
            "GetCustomerGatewayAssociations",
            format!("/global-networks/{global_network}/customer-gateway-associations"),
        ),
        listing(
            "GetTransitGatewayRegistrations",
            format!("/global-networks/{global_network}/transit-gateway-registrations"),
        ),
        listing(
            "GetTransitGatewayConnectPeerAssociations",
            format!("/global-networks/{global_network}/transit-gateway-connect-peer-associations"),
        ),
        listing(
            "GetNetworkResources",
            format!("/global-networks/{global_network}/network-resources"),
        ),
        listing(
            "GetNetworkResourceRelationships",
            format!("/global-networks/{global_network}/network-resource-relationships"),
        ),
        listing(
            "GetNetworkResourceCounts",
            format!("/global-networks/{global_network}/network-resource-count"),
        ),
        listing(
            "GetNetworkTelemetry",
            format!("/global-networks/{global_network}/network-telemetry"),
        ),
        listing("ListAttachments", "/attachments".to_string()),
        listing("ListConnectPeers", "/connect-peers".to_string()),
        listing("ListPeerings", "/peerings".to_string()),
        listing(
            "ListCoreNetworkPolicyVersions",
            format!("/core-networks/{core_network}/core-network-policy-versions"),
        ),
        listing(
            "ListCoreNetworkPrefixListAssociations",
            format!("/prefix-list/core-network/{core_network}"),
        ),
        listing(
            "ListAttachmentRoutingPolicyAssociations",
            format!("/routing-policy-label/core-network/{core_network}"),
        ),
        listing(
            "GetCoreNetworkChangeSet",
            format!("/core-networks/{core_network}/core-network-change-sets/1"),
        ),
        listing(
            "GetCoreNetworkChangeEvents",
            format!("/core-networks/{core_network}/core-network-change-events/1"),
        ),
        Listing {
            case: "ListCoreNetworkRoutingInformation",
            method: "POST",
            uri: format!("/core-networks/{core_network}/core-network-routing-information"),
            body: Some(json!({
                "SegmentName": SEGMENT_NAME,
                "EdgeLocation": EDGE_LOCATION,
            })),
        },
    ]
}

/// Tokens no listing of this service can have minted. The rows differ in alphabet, in
/// length — including the modelled `NextToken` bound of 2048 characters — and in what
/// makes them unusable: text that is no hex at all, hex of an odd length, and hex of text
/// this service never prefixes a token with.
fn unminted_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("not hex at all", "zzz".to_string()),
        ("a word", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        // "not-ours" and "NOT-OURS", in hex: readable text carrying no prefix of this
        // service, in each hex alphabet.
        ("lowercase hex of foreign text", "6e6f742d6f75727300".to_string()),
        ("uppercase hex of foreign text", "4E4F542D4F555253".to_string()),
        ("at the modelled length bound", "a".repeat(2048)),
    ]
}

/// Drive `listing` with `token` in its `nextToken` query member.
async fn refuse(emulator: &Emulator, listing: &Listing, token: &str) -> (u16, Value) {
    let separator = if listing.uri.contains('?') { '&' } else { '?' };
    let uri = format!("{}{separator}nextToken={token}", listing.uri);
    emulator
        .call(listing.method, &uri, listing.body.clone())
        .await
}

/// A global network, an AVAILABLE core network inside it and a device of it, so every
/// listing in the table addresses resources that exist and reaches its own token check.
pub(crate) async fn fixture(emulator: &Emulator) -> (String, String, String) {
    let (global_network, core_network) =
        core_network_at(emulator, "unminted page token", &[EDGE_LOCATION]).await;
    let device = create_device_ok(
        emulator,
        &global_network,
        json!({ "Description": "unminted page token" }),
    )
    .await;
    // The policy is in force once the CREATING -> AVAILABLE transition the clock owes has
    // fallen due, which is when the routing listing can address a route table.
    tokio::time::sleep(PAST_SETTLE).await;
    (global_network, core_network, device)
}

#[tokio::test]
async fn every_listing_refuses_an_unminted_next_token_the_same_way() {
    let emulator = Emulator::new();
    let (global_network, core_network, device) = fixture(&emulator).await;

    // The refusal every row must answer: whatever listing, whatever token, the body is one
    // body. The first (listing, token) pair fixes it and every later pair is compared
    // against it.
    let mut expected: Option<(String, Value)> = None;

    for listing in listings(&global_network, &core_network, &device) {
        for (token_case, token) in unminted_tokens() {
            let case = format!("{}/{token_case}", listing.case);
            let (status, body) = refuse(&emulator, &listing, &token).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400 for a token this service did not mint, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Reason"],
                json!(REASON_FIELD_VALIDATION_FAILED),
                "case {case}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"],
                json!(FIELD_NEXT_TOKEN),
                "case {case}: the refusal must name the {FIELD_NEXT_TOKEN} member, got {body}"
            );
            assert!(
                body["Fields"][1].is_null(),
                "case {case}: one member is at fault, so one Fields entry is due, got {body}"
            );
            match &expected {
                Some((first, first_body)) => assert_eq!(
                    &body, first_body,
                    "case {case}: every listing must answer the refusal {first} answers, got {body}"
                ),
                None => expected = Some((case.clone(), body.clone())),
            }
        }
    }

    let (first, _) = expected.expect("the listing table is not empty");
    assert!(
        !first.is_empty(),
        "the table must have driven at least one listing, got {first:?}"
    );
}

#[tokio::test]
async fn a_token_a_listing_minted_is_honoured_by_it() {
    let emulator = Emulator::new();
    let global_network = emulator.create_global_network("minted page token").await;
    let first = emulator.create_site(&global_network, "site-a").await;
    let second = emulator.create_site(&global_network, "site-b").await;

    // One site per page, so the first page is followed by a token this service minted.
    let sites_uri = format!("/global-networks/{global_network}/sites");
    let (status, page) = emulator.get(&format!("{sites_uri}?maxResults=1")).await;
    assert_eq!(
        status, 200,
        "case first page: expected 200, got {status} with {page}"
    );
    let token = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: two sites need a NextToken, got {page}"))
        .to_string();

    // The minted token resumes the listing instead of being refused, so the refusal above
    // is about tokens this service never issued.
    let (status, resumed) = emulator
        .get(&format!("{sites_uri}?maxResults=1&nextToken={token}"))
        .await;
    assert_eq!(
        status, 200,
        "case resumed page: a token this service minted must be honoured, got {status} with {resumed}"
    );
    let reported: Vec<String> = [&page, &resumed]
        .into_iter()
        .map(|body| {
            body["Sites"][0]["SiteId"]
                .as_str()
                .unwrap_or_else(|| panic!("case resumed page: expected one site in {body}"))
                .to_string()
        })
        .collect();
    let mut expected = [first, second];
    expected.sort();
    assert_eq!(
        reported,
        expected.to_vec(),
        "case resumed page: the two pages must carry the two sites in key order, got {reported:?}"
    );
}
