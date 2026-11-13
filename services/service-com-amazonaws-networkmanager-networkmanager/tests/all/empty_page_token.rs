// SPDX-License-Identifier: CC0-1.0

//! An EMPTY `NextToken` is no token at all, so every paginated listing of this service
//! answers it with the page it answers for the omitted member, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`, the listing table in
//! `crate::unminted_page_token_refusal`).
//!
//! The rule under test: `NextToken` is modelled `length 0..2048` (`smithy_query
//! query="operation" operation="GetSites"`, the same shape every listing of the service
//! binds) and travels in the query string, so `?nextToken=` reaches a listing as the empty
//! string — a value that satisfies the schema, names no position, and whose only page can be
//! the first one. That is the reading this service gives every optional string member whose
//! modelled length admits the empty string: empty is the ABSENCE of the value. So a listing
//! handed an empty token answers exactly what it answers for the omitted member, and only a
//! NON-empty token the service never issued is refused.
//!
//! The first table spans that class: every listing the service pages with a token — all 24
//! of them, reached at the method and URI the model binds each to (`smithy_query
//! query="digest"`) — crossed with the page size the request asks for (none, and one), each
//! driven twice: once with the member omitted and once with it present and empty. The
//! counter-cases are in the second test: on a listing that really pages, an empty token asks
//! for the FIRST page while the token that listing minted asks for the one after it, and a
//! token that merely looks blank (a single space, which is not the empty string) is refused
//! like any other token this service never issued.
//!
//! Nothing here reaches into `crate::state`: every resource is created and every token is
//! obtained through an operation.

use serde_json::Value;

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::unminted_page_token_refusal::{fixture, listings, Listing};

/// The `Reason` a `ValidationException` carries when ONE input member is at fault, from the
/// modelled enumeration (`smithy_query shape=ValidationExceptionReason`: `UnknownOperation`,
/// `CannotParse`, `FieldValidationFailed`, `Other`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The input member every paginated listing carries its continuation token in (`NextToken`,
/// `@httpQuery("nextToken")` on each listing's request shape).
const FIELD_NEXT_TOKEN: &str = "NextToken";

/// The member a listed network resource reports "the time that the resource definition was
/// retrieved" in (`smithy_query query="doc" shape="NetworkResource"
/// member="DefinitionTimestamp"`): it is the instant of the READ, so two reads of one page
/// differ in it whatever the token spelling, and [`without_read_instants`] drops it before
/// two pages are compared.
const READ_INSTANT_MEMBER: &str = "DefinitionTimestamp";

/// `body` with every [`READ_INSTANT_MEMBER`] dropped, at any depth, so what is compared is
/// the page itself: which items it carries, in which order, and the token that follows it.
fn without_read_instants(body: &Value) -> Value {
    match body {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(name, _)| name.as_str() != READ_INSTANT_MEMBER)
                .map(|(name, value)| (name.clone(), without_read_instants(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_read_instants).collect()),
        other => other.clone(),
    }
}

/// Drive `listing` with the page size and the `nextToken` spelling given: `None` omits the
/// member, `Some(value)` sends it with that value — `Some("")` being the `?nextToken=` a
/// client writes for a token it does not hold.
async fn page(
    emulator: &Emulator,
    listing: &Listing,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let mut uri = listing.uri.clone();
    if !query.is_empty() {
        uri.push(if uri.contains('?') { '&' } else { '?' });
        uri.push_str(&query.join("&"));
    }
    emulator
        .call(listing.method, &uri, listing.body.clone())
        .await
}

#[tokio::test]
async fn every_listing_reads_an_empty_next_token_as_no_token() {
    let emulator = Emulator::new();
    let (global_network, core_network, device) = fixture(&emulator).await;

    for listing in listings(&global_network, &core_network, &device) {
        // The page size the request asks for is independent of how "no token" is spelled,
        // so both spellings are driven at each: no `MaxResults` at all, and the smallest
        // page the model admits (`range 1..500`).
        for max_results in [None, Some(1)] {
            let size = match max_results {
                Some(size) => size.to_string(),
                None => "no maxResults".to_string(),
            };
            let case = format!("{}/{size}", listing.case);

            let (omitted_status, omitted) = page(&emulator, &listing, max_results, None).await;
            assert_eq!(
                omitted_status, 200,
                "case {case}/omitted token: expected 200, got {omitted_status} with {omitted}"
            );

            let (empty_status, empty) = page(&emulator, &listing, max_results, Some("")).await;
            assert_eq!(
                empty_status, 200,
                "case {case}/empty token: an empty NextToken satisfies the modelled length 0..2048 and names no position, so it asks for the first page, got {empty_status} with {empty}"
            );
            let (empty_page, omitted_page) =
                (without_read_instants(&empty), without_read_instants(&omitted));
            assert_eq!(
                empty_page, omitted_page,
                "case {case}/empty token: expected the page the omitted member gets, {omitted_page}, got {empty_page}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_token_asks_for_the_first_page_and_a_blank_one_is_refused() {
    let emulator = Emulator::new();
    let global_network = emulator.create_global_network("empty page token").await;
    let first = emulator.create_site(&global_network, "site-a").await;
    let second = emulator.create_site(&global_network, "site-b").await;
    let mut in_key_order = [first, second];
    in_key_order.sort();

    let sites = format!("/global-networks/{global_network}/sites");
    let site_on_page = |body: &Value, case: &str| -> String {
        body["Sites"][0]["SiteId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected one site in {body}"))
            .to_string()
    };

    // One site per page, so the first page is followed by a token this listing minted.
    let (status, opening) = emulator.get(&format!("{sites}?maxResults=1")).await;
    assert_eq!(
        status, 200,
        "case first page: expected 200, got {status} with {opening}"
    );
    assert_eq!(
        site_on_page(&opening, "first page"),
        in_key_order[0],
        "case first page: expected the first site in key order, got {opening}"
    );
    let minted = opening["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: two sites need a NextToken, got {opening}"))
        .to_string();

    // An EMPTY token asks for that same first page: it names no position, so it cannot
    // advance the listing.
    let (status, empty) = emulator.get(&format!("{sites}?maxResults=1&nextToken=")).await;
    assert_eq!(
        status, 200,
        "case empty token: expected 200, got {status} with {empty}"
    );
    assert_eq!(
        empty, opening,
        "case empty token: expected the first page {opening}, got {empty}"
    );

    // The token the listing minted advances it, so reading the empty token as "no token"
    // has not made this listing ignore its tokens.
    let (status, resumed) = emulator
        .get(&format!("{sites}?maxResults=1&nextToken={minted}"))
        .await;
    assert_eq!(
        status, 200,
        "case minted token: expected 200, got {status} with {resumed}"
    );
    assert_eq!(
        site_on_page(&resumed, "minted token"),
        in_key_order[1],
        "case minted token: expected the site after the first one, got {resumed}"
    );

    // A token that merely LOOKS blank is not the empty string, so it is one more token this
    // service never issued and gets the refusal every such token gets. The rows differ in
    // what the whitespace is and how much of it there is.
    for (case, token) in [
        ("one space", "%20"),
        ("two spaces", "%20%20"),
        ("a tab", "%09"),
        ("a space around a word", "%20not-a-token%20"),
    ] {
        let (status, refused) = emulator
            .get(&format!("{sites}?maxResults=1&nextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a non-empty token this service did not issue, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Reason"],
            Value::String(REASON_FIELD_VALIDATION_FAILED.to_string()),
            "case {case}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            Value::String(FIELD_NEXT_TOKEN.to_string()),
            "case {case}: the refusal must name the {FIELD_NEXT_TOKEN} member, got {refused}"
        );
    }
}
