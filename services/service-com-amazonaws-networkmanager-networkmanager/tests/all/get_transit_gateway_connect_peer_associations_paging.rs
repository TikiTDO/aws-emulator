// SPDX-License-Identifier: CC0-1.0

//! How `GetTransitGatewayConnectPeerAssociations` pages, selects and scopes the stored
//! transit gateway Connect peer associations of one global network, driven over HTTP.
//!
//! The rule under test: a reporting listing of this service answers the stored records of
//! the resource its path names, in one stable order, as of the instant it reads them and
//! writing nothing; `MaxResults` fixes how many of them one page carries and a page carries
//! the continuation token only while records remain, so a client that follows the tokens
//! sees every record exactly once whatever page size it asks for inside the modelled range,
//! and whatever selector narrows the listing; a size outside that range, and a token this
//! service did not issue, are refused with the modelled `ValidationException` (HTTP 400) and
//! no page at all.
//!
//! The dimensions the table below spans are the ones that rule generalises over: the page
//! size (each published bound of `MaxResults` — `range 1..500`, `smithy_query
//! query="operation" operation="GetTransitGatewayConnectPeerAssociations"` — sizes below, at
//! and past the number of stored associations, and no size at all), whether a selector
//! narrows the listing while the tokens are followed, and which global network the path
//! names.
//!
//! `crate::transit_gateway_connect_peer_association_lifecycle` pins the
//! associate/read/disassociate round trip and what one ARN selects; this suite pins the
//! paging, the page-size and page-token refusals and the per-global-network scoping, which
//! that suite does not cover.
//!
//! Every call goes through the real `MultiServiceRouter`, and an association is only ever
//! read back through an operation — nothing here reaches into `crate::state`.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::escape;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};
// The request helpers and the page reader of this slice, shared with the suite that tables
// its round trip, so the two cannot disagree about either.
use crate::transit_gateway_connect_peer_association_lifecycle::{
    arns_of, associate, connect_peer_arn, get_associations, register_gateway,
};

/// The output member the page is carried in.
const LIST: &str = "TransitGatewayConnectPeerAssociations";

/// The published bounds of `MaxResults` (`range 1..500`, `smithy_query query="operation"
/// operation="GetTransitGatewayConnectPeerAssociations"`).
const SMALLEST_PAGE: usize = 1;
const LARGEST_PAGE: usize = 500;

/// How many hex digits the transit gateway ids the fixture registers carry; the width of the
/// PEER id is what each row below varies.
const GATEWAY_DIGITS: usize = 17;

/// One Connect peer the fixture ties to a device of its global network.
struct Peer {
    /// Names the peer in a seeding assertion.
    name: &'static str,
    /// The ARN partition the peer's ARN — and the gateway it was created on — carries.
    partition: &'static str,
    /// The Region the peer is in, which is the Region of the gateway carrying it.
    region: &'static str,
    /// The account that owns the peer, which owns that gateway too.
    account: String,
    /// How many hex digits the peer id carries.
    digits: usize,
}

/// The peers one global network holds, spanning the ARN partition, Region and id width a
/// well-formed Connect peer ARN can carry. The account is the one that owns the global
/// network on every row: a peer sits on a transit gateway registered with it, and a gateway
/// is registered only from that account.
fn peers() -> Vec<Peer> {
    vec![
        Peer {
            name: "the shortest published id",
            partition: "aws",
            region: "us-west-2",
            account: own_account(),
            digits: 8,
        },
        Peer {
            name: "the longest published id, in another Region",
            partition: "aws",
            region: "eu-central-1",
            account: own_account(),
            digits: 17,
        },
        Peer {
            name: "another partition and Region",
            partition: "aws-cn",
            region: "cn-north-1",
            account: own_account(),
            digits: 12,
        },
        Peer {
            name: "the GovCloud partition",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            account: own_account(),
            digits: 16,
        },
        Peer {
            name: "a third Region",
            partition: "aws",
            region: "ap-southeast-2",
            account: own_account(),
            digits: 10,
        },
    ]
}

/// Build a global network holding one association per row of `rows`, and answer it with the
/// peer ARNs those associations carry in the order the store keys them (the key is the
/// global network plus the ARN, so that order is the ARNs sorted).
///
/// `base` offsets the transit gateway ids the fixture registers, so two fixtures of one
/// emulator register different gateways — a gateway is registered with at most one global
/// network.
async fn seed(emulator: &Emulator, name: &str, base: usize, rows: &[Peer]) -> (String, Vec<String>) {
    let network = emulator.create_global_network(name).await;
    let device = create_device_ok(emulator, &network, json!({ "Description": name })).await;

    let mut arns: Vec<String> = Vec::new();
    for (index, peer) in rows.iter().enumerate() {
        // A Connect peer can only be associated when a transit gateway of its Region and
        // account is registered with the global network, so one gateway is registered per
        // peer.
        register_gateway(
            emulator,
            &network,
            &transit_gateway_arn(
                peer.partition,
                peer.region,
                &peer.account,
                base + index,
                GATEWAY_DIGITS,
            ),
        )
        .await;

        let arn = connect_peer_arn(
            peer.partition,
            peer.region,
            &peer.account,
            base + index,
            peer.digits,
        );
        let (status, body) = associate(
            emulator,
            &network,
            json!({ "TransitGatewayConnectPeerArn": arn, "DeviceId": device }),
        )
        .await;
        assert_eq!(
            status, 200,
            "seed {:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} with {body}",
            peer.name
        );
        arns.push(arn);
    }

    arns.sort();
    (network, arns)
}

/// Walk the whole listing of `network` — narrowed by `selector` when it names any ARN — with
/// the page size `size` (`None` asks for no size at all), following every token, and answer
/// the peer ARNs reached in order.
///
/// Asserts on the way that no page is larger than the size asked for and that a token is
/// carried only while records remain. The walker is this operation's own — its siblings
/// `crate::get_transit_gateway_registrations_paging::walk` and
/// `crate::get_customer_gateway_associations_paging::walk_pages` walk the other paged
/// readers of this service, with their own URIs, selectors and page members.
async fn walk(
    emulator: &Emulator,
    network: &str,
    case: &str,
    selector: &[String],
    size: Option<usize>,
    total: usize,
) -> Vec<String> {
    let mut reached: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One request per association is the most any size inside the range can need, plus the
    // one that reports the end; the bound turns a token that fails to advance into a failure
    // rather than a hang.
    for step in 0..=total {
        let mut query: Vec<String> = selector
            .iter()
            .map(|arn| format!("transitGatewayConnectPeerArns={}", escape(arn)))
            .collect();
        if let Some(size) = size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(held) = &token {
            query.push(format!("nextToken={}", escape(held)));
        }
        let (status, body) = get_associations(emulator, network, &query.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case} step {step}: expected 200, got {status} with {body}"
        );
        let page = arns_of(case, &body);
        let taken = page.len();
        if let Some(size) = size {
            assert!(
                taken <= size,
                "case {case} step {step}: a page of {size} must hold at most that many, got {body}"
            );
        }
        reached.extend(page);
        token = body["NextToken"].as_str().map(str::to_string);
        assert_eq!(
            token.is_some(),
            reached.len() < total,
            "case {case} step {step}: a token must be carried only while associations remain, got {body}"
        );
        assert!(
            token.is_none() || taken > 0,
            "case {case} step {step}: a page that resumes must not be empty, got {body}"
        );
        if token.is_none() {
            return reached;
        }
    }
    panic!("case {case}: the tokens never ran out after {total} pages, having reached {reached:?}")
}

#[tokio::test]
async fn every_page_size_in_the_modelled_range_walks_the_whole_listing_once() {
    let emulator = Emulator::new();
    let rows = peers();
    let (network, expected) = seed(&emulator, "association paging", 0, &rows).await;
    let total = expected.len();
    assert!(
        total >= 3,
        "the fixture must hold at least three associations for the rows below, holds {total}"
    );

    // The rows: no page size at all, each published bound of the range, and sizes below, at
    // and past the number of stored associations.
    let sizes: Vec<(String, Option<usize>)> = vec![
        ("no page size".to_string(), None),
        (
            format!("the lower bound of the range ({SMALLEST_PAGE})"),
            Some(SMALLEST_PAGE),
        ),
        (
            "a size below the count".to_string(),
            Some(total.saturating_sub(2)),
        ),
        (
            "a size one below the count".to_string(),
            Some(total.saturating_sub(1)),
        ),
        ("a size exactly the count".to_string(), Some(total)),
        ("a size one past the count".to_string(), Some(total + 1)),
        (
            format!("the upper bound of the range ({LARGEST_PAGE})"),
            Some(LARGEST_PAGE),
        ),
    ];

    for (case, size) in &sizes {
        let reached = walk(&emulator, &network, case, &[], *size, total).await;
        assert_eq!(
            reached, expected,
            "case {case}: following every token must reach each association exactly once, got {reached:?}"
        );
    }

    // Reading the listing — however it was paged — changed nothing a later caller sees.
    let (status, body) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the walks: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of("read after the walks", &body),
        expected,
        "case read after the walks: a read must not change what a later caller sees, got {body}"
    );
}

#[tokio::test]
async fn the_tokens_walk_a_selected_subset_once_at_every_page_size() {
    let emulator = Emulator::new();
    let rows = peers();
    let (network, stored) = seed(&emulator, "selected paging", 0, &rows).await;

    // An ARN of the same well-formed shape that names no stored association, so a row can
    // mix the selected with the unselectable.
    let absent = connect_peer_arn("aws", "us-west-2", "123456789012", 0xdead, 17);

    // The rows differ along how much of the listing the selector leaves and whether it names
    // anything unstored; each is then walked at every page size below.
    let selections: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one stored ARN", vec![stored[2].clone()], vec![stored[2].clone()]),
        (
            "three stored ARNs",
            vec![stored[0].clone(), stored[2].clone(), stored[4].clone()],
            vec![stored[0].clone(), stored[2].clone(), stored[4].clone()],
        ),
        (
            "stored ARNs and one stored nowhere",
            vec![stored[1].clone(), absent.clone(), stored[3].clone()],
            vec![stored[1].clone(), stored[3].clone()],
        ),
        ("every stored ARN", stored.clone(), stored.clone()),
    ];

    for (selection, selector, expected) in &selections {
        for size in [None, Some(SMALLEST_PAGE), Some(2), Some(LARGEST_PAGE)] {
            let case = format!("{selection} at page size {size:?}");
            let reached = walk(&emulator, &network, &case, selector, size, expected.len()).await;
            assert_eq!(
                reached, *expected,
                "case {case}: following every token must reach exactly {expected:?}, got {reached:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_lists_nothing() {
    let emulator = Emulator::new();
    let rows = peers();
    let (network, expected) = seed(&emulator, "page size bounds", 0, &rows).await;

    // The rows: one below the lower bound, one past the upper bound, far past it, and a
    // negative size — every integer the modelled `range 1..500` excludes.
    let refused = [
        ("one below the lower bound", "maxResults=0"),
        ("one past the upper bound", "maxResults=501"),
        ("far past the upper bound", "maxResults=100000"),
        ("a negative size", "maxResults=-1"),
    ];

    for (case, query) in &refused {
        let (status, body) = get_associations(&emulator, &network, query).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {query}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert!(
            body[LIST].is_null(),
            "case {case}: a refused request must carry no page, got {body}"
        );
    }

    let (status, body) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of("read after the refusals", &body),
        expected,
        "case read after the refusals: a refused read must change nothing, got {body}"
    );
}

#[tokio::test]
async fn a_page_token_this_service_did_not_issue_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();
    let rows = peers();
    let (network, expected) = seed(&emulator, "page tokens", 0, &rows).await;

    // The token this service DOES issue, which is what makes the rows below a refusal of the
    // token rather than of the request.
    let (status, first) = get_associations(&emulator, &network, "maxResults=1").await;
    assert_eq!(
        status, 200,
        "case the first page: expected 200, got {status} with {first}"
    );
    let issued = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first page: expected a NextToken in {first}"))
        .to_string();

    // The rows: a string that is no token of any kind, one that cannot be decoded at all,
    // one that decodes to a plain word, one that decodes to bytes that are no text at all,
    // and one that decodes to a token another service would have minted.
    let tokens: Vec<(&str, String)> = vec![
        // Every row is a NON-empty value. The empty string — which the modelled `len
        // 0..2048, pattern ^[\s\S]*$` admits as a VALUE — names no position at all, so it
        // is no token rather than a foreign one and asks for the first page. That is how
        // every paged reader of this service reads it (`crate::empty_page_token` pins it
        // over all of them), so the two cannot disagree.
        ("a string that is no token", "not-a-token".to_string()),
        (
            "a half-decodable token",
            hex::encode("a token")[1..].to_string(),
        ),
        ("a token that decodes to a word", hex::encode("abcde")),
        (
            "a token that decodes to no text",
            hex::encode([0xff_u8, 0xfe, 0x00]),
        ),
        (
            "a token minted for another service",
            hex::encode("otherservice:global-network-00000000000000001/entry"),
        ),
    ];

    for (case, token) in &tokens {
        let query = format!("maxResults=1&nextToken={}", escape(token));
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {token:?}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!("NextToken"),
            "case {case}: the refusal must name the member at fault, got {body}"
        );
        assert!(
            body[LIST].is_null(),
            "case {case}: a refused request must carry no page, got {body}"
        );
    }

    // The issued token still resumes the listing after every refusal above.
    let (status, resumed) = get_associations(
        &emulator,
        &network,
        &format!("maxResults=1&nextToken={}", escape(&issued)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the issued token: expected 200, got {status} with {resumed}"
    );
    assert_eq!(
        arns_of("the issued token", &resumed),
        expected[1..2].to_vec(),
        "case the issued token: it must resume after the first association, got {resumed}"
    );
}

#[tokio::test]
async fn each_global_network_pages_only_the_associations_stored_under_it() {
    let emulator = Emulator::new();
    let rows = peers();
    // Two global networks of one emulator, each holding its own associations: the second
    // registers gateways from another id offset, since a gateway is registered with at most
    // one global network.
    let (first, first_arns) = seed(&emulator, "the first network", 0, &rows).await;
    let (second, second_arns) = seed(&emulator, "the second network", 100, &rows[..2]).await;

    // Whatever page size the walk asks for, each path answers its own associations and none
    // of the other's.
    for size in [None, Some(SMALLEST_PAGE), Some(2), Some(LARGEST_PAGE)] {
        for (case, network, expected) in [
            ("the first network", &first, &first_arns),
            ("the second network", &second, &second_arns),
        ] {
            let case = format!("{case} at page size {size:?}");
            let reached = walk(&emulator, network, &case, &[], size, expected.len()).await;
            assert_eq!(
                reached, *expected,
                "case {case}: a global network must page only its own associations, got {reached:?}"
            );
        }
    }

    // A selector naming the OTHER network's associations selects none of them here.
    for (case, network, foreign) in [
        ("the first network", &first, &second_arns),
        ("the second network", &second, &first_arns),
    ] {
        let query = foreign
            .iter()
            .map(|arn| format!("transitGatewayConnectPeerArns={}", escape(arn)))
            .collect::<Vec<String>>()
            .join("&");
        let (status, body) = get_associations(&emulator, network, &query).await;
        assert_eq!(
            status, 200,
            "case {case} with the other network's ARNs: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body[LIST],
            json!([]),
            "case {case} with the other network's ARNs: expected an empty page, got {body}"
        );
    }
}
