// SPDX-License-Identifier: CC0-1.0

//! How `GetTransitGatewayRegistrations` pages and selects the stored registrations of one
//! global network, driven over HTTP.
//!
//! The rule under test: a reporting listing of this service answers the stored records of
//! the resource its path names, in one stable order, as of the instant it reads them and
//! writing nothing; `MaxResults` fixes how many of them one page carries and a page carries
//! the continuation token only while records remain, so a client that follows the tokens
//! sees every record exactly once whatever page size it asks for inside the modelled range;
//! a size outside that range, and a token this service did not issue, are refused with the
//! modelled `ValidationException` (HTTP 400) and no page at all.
//!
//! The dimensions the table below spans are the ones that rule generalises over: the page
//! size (each published bound of `MaxResults` — `range 1..500`, `smithy_query
//! query="operation" operation="GetTransitGatewayRegistrations"` — the sizes below, at and
//! past the number of stored records, and no size at all), and, for the selector, how many
//! of the ARNs it names are stored (none, all, some, and a mixture of stored and unstored).
//!
//! `crate::site_ids_filter_maximum` pins the LENGTH bound of that selector and
//! `crate::transit_gateway_registration_lifecycle` the register/read/deregister round trip;
//! this suite pins the paging and the selection, which neither covers.
//!
//! Every call goes through the real `MultiServiceRouter`, and a registration is only ever
//! read back through an operation — nothing here reaches into `crate::state`.

use serde_json::json;

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::escape;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};
// The request helpers and the page reader of this slice, shared with the suite that tables
// its round trip, so the two cannot disagree about either.
use crate::transit_gateway_registration_lifecycle::{arns_of, get_registrations, register};

/// The output member the page is carried in.
const LIST: &str = "TransitGatewayRegistrations";

/// The published bounds of `MaxResults` (`range 1..500`, `smithy_query query="operation"
/// operation="GetTransitGatewayRegistrations"`).
const SMALLEST_PAGE: usize = 1;
const LARGEST_PAGE: usize = 500;

/// The ARNs the fixture registers, spanning the partition, Region and id width an EC2
/// transit gateway ARN can carry. Every one of them carries the account that owns the
/// global network, which is the only account a registration takes.
fn fixture_arns() -> Vec<String> {
    let account = own_account();
    let mut arns = vec![
        transit_gateway_arn("aws", "us-west-2", &account, 1, 8),
        transit_gateway_arn("aws", "eu-central-1", &account, 2, 17),
        transit_gateway_arn("aws-cn", "cn-north-1", &account, 3, 12),
        transit_gateway_arn("aws-us-gov", "us-gov-west-1", &account, 4, 16),
        transit_gateway_arn("aws", "ap-southeast-2", &account, 5, 10),
    ];
    arns.sort();
    arns
}

/// Well-formed transit gateway ARNs the suite registers with the OTHER global network, so
/// they are stored — just not under the one being read.
fn arns_registered_elsewhere() -> Vec<String> {
    vec![
        transit_gateway_arn("aws", "us-east-1", &own_account(), 0xf1, 9),
        transit_gateway_arn("aws", "sa-east-1", &own_account(), 0xf2, 17),
    ]
}

/// Register every ARN of `arns` with `network`.
async fn seed(emulator: &Emulator, network: &str, arns: &[String]) {
    for arn in arns {
        let (status, body) = register(emulator, network, arn).await;
        assert_eq!(
            status, 200,
            "seed {arn}: expected 200 from RegisterTransitGateway, got {status} with {body}"
        );
    }
}

/// Walk the whole listing of `network` with the page size `size` (`None` asks for no size
/// at all), following every token, and answer the ARNs reached in order.
///
/// Asserts on the way that no page is larger than the size asked for and that a token is
/// carried only while records remain. The walker is this operation's own — its sibling
/// `crate::get_customer_gateway_associations_paging::walk_pages` walks the OTHER paged
/// reader of this service, with that operation's own URI, selector and page member.
async fn walk(
    emulator: &Emulator,
    network: &str,
    case: &str,
    size: Option<usize>,
    total: usize,
) -> Vec<String> {
    let mut reached: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One request per registration is the most any size inside the range can need, plus the
    // one that reports the end; the bound turns a token that fails to advance into a
    // failure rather than a hang.
    for step in 0..=total {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(held) = &token {
            query.push(format!("nextToken={}", escape(held)));
        }
        let (status, body) = get_registrations(emulator, network, &query.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case} step {step}: expected 200, got {status} with {body}"
        );
        let page = arns_of(&body);
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
            "case {case} step {step}: a token must be carried only while registrations remain, got {body}"
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
    let network = emulator.create_global_network("registration paging").await;
    let expected = fixture_arns();
    seed(&emulator, &network, &expected).await;
    let total = expected.len();
    assert!(
        total >= 3,
        "the fixture must hold at least three registrations for the rows below, holds {total}"
    );

    // The rows: no page size at all, each published bound of the range, and sizes below, at
    // and past the number of stored registrations.
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
        let reached = walk(&emulator, &network, case, *size, total).await;
        assert_eq!(
            reached, expected,
            "case {case}: following every token must reach each registration exactly once, got {reached:?}"
        );
    }

    // Reading the listing — however it was paged — changed nothing a later caller sees.
    let (status, body) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the walks: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of(&body),
        expected,
        "case read after the walks: a read must not change what a later caller sees, got {body}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_lists_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("page size bounds").await;
    let expected = fixture_arns();
    seed(&emulator, &network, &expected).await;

    // The rows: one below the lower bound, one past the upper bound, far past it, and a
    // negative size — every integer the modelled `range 1..500` excludes.
    let refused = [
        ("one below the lower bound", "maxResults=0"),
        ("one past the upper bound", "maxResults=501"),
        ("far past the upper bound", "maxResults=100000"),
        ("a negative size", "maxResults=-1"),
    ];

    for (case, query) in &refused {
        let (status, body) = get_registrations(&emulator, &network, query).await;
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

    let (status, body) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {body}"
    );
    assert_eq!(
        arns_of(&body),
        expected,
        "case read after the refusals: a refused read must change nothing, got {body}"
    );
}

#[tokio::test]
async fn a_page_token_this_service_did_not_issue_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("page tokens").await;
    let expected = fixture_arns();
    seed(&emulator, &network, &expected).await;

    // The token this service DOES issue, which is what makes the rows below a refusal of
    // the token rather than of the request.
    let (status, first) = get_registrations(&emulator, &network, "maxResults=1").await;
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
    // and one that decodes to a token another service would have minted. Each is NON-empty:
    // nothing at all in the slot is no token rather than an unminted one — it satisfies the
    // modelled `length 0..2048` and names no position, so it asks for the first page
    // (`crate::empty_page_token`).
    let rows: Vec<(&str, String)> = vec![
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

    for (case, token) in &rows {
        let query = format!("maxResults=1&nextToken={}", escape(token));
        let (status, body) = get_registrations(&emulator, &network, &query).await;
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
    let (status, resumed) = get_registrations(
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
        arns_of(&resumed),
        expected[1..2].to_vec(),
        "case the issued token: it must resume after the first registration, got {resumed}"
    );
}

#[tokio::test]
async fn the_selector_narrows_the_listing_to_the_stored_registrations_it_names() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("selected registrations").await;
    let elsewhere = emulator.create_global_network("another network").await;
    let stored = fixture_arns();
    seed(&emulator, &network, &stored).await;

    // A gateway is registered with at most one global network, so the ARNs the OTHER
    // network holds are stored — just not here, which is one of the rows below.
    let other_arns: Vec<String> = arns_registered_elsewhere();
    seed(&emulator, &elsewhere, &other_arns).await;
    let absent = transit_gateway_arn("aws", "me-south-1", "123456789012", 0xfe, 11);

    // The rows differ along how many of the ARNs the selector names are registrations of
    // THIS global network: none of them, one, several, every one, a mixture of stored and
    // unstored, an ARN registered with another global network, and one registered nowhere.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("no selector", Vec::new(), stored.clone()),
        (
            "one stored ARN",
            vec![stored[2].clone()],
            vec![stored[2].clone()],
        ),
        (
            "several stored ARNs",
            vec![stored[0].clone(), stored[3].clone()],
            vec![stored[0].clone(), stored[3].clone()],
        ),
        ("every stored ARN", stored.clone(), stored.clone()),
        (
            "a mixture of stored and unstored ARNs",
            vec![stored[1].clone(), absent.clone(), stored[4].clone()],
            vec![stored[1].clone(), stored[4].clone()],
        ),
        (
            "an ARN registered with another global network",
            other_arns.clone(),
            Vec::new(),
        ),
        ("an ARN registered nowhere", vec![absent.clone()], Vec::new()),
    ];

    for (case, selector, expected) in &rows {
        let query = selector
            .iter()
            .map(|arn| format!("transitGatewayArns={}", escape(arn)))
            .collect::<Vec<String>>()
            .join("&");
        let (status, body) = get_registrations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            arns_of(&body),
            *expected,
            "case {case}: expected exactly {expected:?}, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case}: a page holding the whole selection carries no token, got {body}"
        );
    }

    // The other global network answers its OWN registrations and none of these, whatever
    // this one's selector named.
    let (status, body) = get_registrations(&emulator, &elsewhere, "").await;
    assert_eq!(
        status, 200,
        "case the other global network: expected 200, got {status} with {body}"
    );
    let mut expected_elsewhere = other_arns.clone();
    expected_elsewhere.sort();
    assert_eq!(
        arns_of(&body),
        expected_elsewhere,
        "case the other global network: it must answer only its own registrations, got {body}"
    );
}
