// SPDX-License-Identifier: CC0-1.0

//! The deletion time `ListApiKeys` projects for a key does not depend on how the caller
//! CUT THE PAGE it read the key on, end to end over HTTP.
//!
//! The rule the arithmetic obeys — a `da2` key's `deletes` is its `expires` plus the
//! fixed post-expiry retention the service keeps an expired key for, floored to the hour,
//! and every operation that reports the key reports that one value — is
//! [`crate::api_key_deletion_window`]'s, whose constants, helpers and rule assertion this
//! suite reuses rather than restating.
//!
//! What this suite adds is the dimension of the LISTING REQUEST: the `maxResults` the
//! caller names and the `nextToken` it resumes from. A projection derived from anything
//! but the key's own stored expiration — the position of the key inside its page, the
//! number of keys the page holds, the page the walk is on — would report one deletion
//! time when the inventory arrives in one page and another when it arrives one key at a
//! time. The inventory is deliberately larger than the largest `maxResults` the model
//! accepts, so a walk that names a bound is walked over more than one page.
//!
//! The rows differ along the two things the listing reads: the `maxResults`
//! (unspecified, the modelled default, both published bounds of its range and values
//! between them) and, per key, the expiration the derivation is a function of —
//! expirations spaced across the whole accepted validity window, each at a different
//! position inside its hour, plus a key that names no expiration at all and takes the
//! documented default.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, expected_deletes, floor_to_hour, unix_now, BOUND_SLACK,
    DEFAULT_VALIDITY, HOUR, MAX_VALIDITY, MIN_VALIDITY, RETENTION,
};
use crate::error_contract::{call, create_api, router};

/// The `maxResults` values one run walks the listing with: none at all, the modelled
/// default of `0` (i.e. "unspecified"), both published bounds of the range and values
/// between them.
///
/// Source: the `maxResults` member of `ListApiKeysRequest` — `@range` 0..25, `@default`
/// 0 (`smithy_query(query="operation", operation="ListApiKeys")`).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(24), Some(25)];

/// The modelled maximum page size, which the inventory below is deliberately larger
/// than, so even a walk at the largest `maxResults` spans several pages.
const MAX_PAGE_SIZE: usize = 25;

/// How many keys the inventory holds — three more than the modelled page maximum, so a
/// walk at the largest `maxResults` still has a short last page to report.
const KEYS: usize = MAX_PAGE_SIZE + 3;

/// The expiration key number `index` is created with, as a function of the clock the
/// create itself reads: the validities are spaced across the accepted window that
/// `ApiKeyValidityOutOfBoundsException` publishes, and each sits at a different position
/// inside its hour, so no two keys of the inventory share a derivation input.
fn expiration_of(index: usize, now: i64) -> i64 {
    // The flooring and the within-hour offset below each move the submitted instant by
    // up to an hour, so the drawn validity stays that much further inside each published
    // bound: a key is a case about the derivation, not about the validity window.
    let lowest = MIN_VALIDITY + BOUND_SLACK + HOUR;
    let highest = MAX_VALIDITY - BOUND_SLACK - HOUR;
    let step = (highest - lowest) / (KEYS as i64 - 1);
    let validity = lowest + step * index as i64;
    // A different second inside the hour per key, spread over the whole hour, so the
    // flooring is exercised at many offsets rather than at one.
    let within = (index as i64 * 131) % HOUR;
    floor_to_hour(now + validity) + within
}

/// One key of the inventory: the case that minted it and the projection its create
/// reported.
struct Minted {
    case: String,
    created: Value,
}

/// Walk the whole listing at `max_results`, following `nextToken` to the last page, and
/// return the keys in the order the pages reported them.
async fn walk(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
) -> Vec<Value> {
    // The page bound the REQUEST names: a positive `maxResults` only. What an unspecified
    // `maxResults` (or the modelled default of `0`) cuts is the service's own choice, which
    // the model does not state, so a walk that names none holds its own first page to be the
    // bound rather than asserting a number.
    let requested_limit = max_results.filter(|size| *size > 0).map(|size| size as usize);
    let mut first_page: Option<usize> = None;
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut page_index = 0usize;
    loop {
        let case = format!("maxResults {max_results:?}, page {page_index}");
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &next_token {
            // The cursor is sent as written, so the walk resumes from the token the service
            // minted only while that token needs no percent-encoding.
            assert!(
                token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')),
                "case {case:?}: expected a nextToken of unreserved URI characters, got {token:?}"
            );
            query.push(format!("nextToken={token}"));
        }
        let path = if query.is_empty() {
            format!("/v1/apis/{api_id}/apikeys")
        } else {
            format!("/v1/apis/{api_id}/apikeys?{}", query.join("&"))
        };
        let answer = call(router, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListApiKeys 200, got {answer:?}"
        );
        let keys = answer.body["apiKeys"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let bound = requested_limit.or(first_page).unwrap_or(KEYS);
        assert!(
            !keys.is_empty() && keys.len() <= bound,
            "case {case:?}: expected between 1 and {bound} keys on a page, got {} ({keys:?})",
            keys.len()
        );
        first_page = first_page.or(Some(keys.len()));
        collected.extend(keys);
        next_token = answer.body["nextToken"].as_str().map(str::to_string);
        page_index += 1;
        if next_token.is_none() {
            break;
        }
        assert!(
            page_index <= KEYS,
            "case {case:?}: expected the walk to reach a last page within {KEYS} pages, \
             got {collected:?}"
        );
    }
    // A walk that NAMED a bound smaller than the inventory must have taken several pages to
    // report it, so the projection is asserted on more than a first page.
    if requested_limit.is_some_and(|limit| limit < KEYS) {
        assert!(
            page_index > 1,
            "case maxResults {max_results:?}: expected an inventory of {KEYS} keys to span \
             several pages, got {page_index} page(s)"
        );
    }
    collected
}

/// Every key is listed with the deletion time its own expiration derives — the one its
/// create reported — whichever `maxResults` the walk cut its pages at.
#[tokio::test]
async fn the_listed_deletion_time_is_the_same_however_the_page_is_cut() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletes_page_cut").await;

    let mut minted: Vec<Minted> = Vec::new();
    for index in 0..KEYS {
        // The first key names no expiration at all and takes the documented default; the
        // rest name an expiration spaced across the accepted window.
        let case = if index == 0 {
            "a key naming no expiration at all".to_string()
        } else {
            format!("key {index} of the inventory")
        };
        let before = unix_now();
        let mut body = json!({ "description": case });
        let submitted = (index > 0).then(|| expiration_of(index, before));
        if let Some(submitted) = submitted {
            body["expires"] = json!(submitted);
        }
        let answer = create_key(&router, &api_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateApiKey 200, got {answer:?}"
        );
        let created = answer.body["apiKey"].clone();
        let expires = assert_retention_span(&case, "CreateApiKey", &created);
        match submitted {
            Some(submitted) => assert_eq!(
                expires,
                floor_to_hour(submitted),
                "case {case:?}: expected the submitted expiration {submitted} floored to \
                 the hour, got {created:?}"
            ),
            // The handler reads its own clock, which sits at or after this test's.
            None => assert!(
                (floor_to_hour(before + DEFAULT_VALIDITY)
                    ..=floor_to_hour(unix_now() + DEFAULT_VALIDITY))
                    .contains(&expires),
                "case {case:?}: expected the documented default expiration, got {created:?}"
            ),
        }
        minted.push(Minted { case, created });
    }

    let expected: Vec<Value> = minted.iter().map(|key| key.created.clone()).collect();
    for max_results in PAGE_SIZES {
        let listed = walk(&router, &api_id, *max_results).await;
        assert_eq!(
            listed.len(),
            minted.len(),
            "case maxResults {max_results:?}: expected the {} keys minted under this API, \
             got {listed:?}",
            minted.len()
        );
        for (key, found) in minted.iter().zip(&listed) {
            let case = format!("{} at maxResults {max_results:?}", key.case);
            let expires = assert_retention_span(&case, "ListApiKeys", found);
            assert_eq!(
                found["deletes"].as_i64(),
                Some(expected_deletes(expires)),
                "case {case:?}: expected deletes {} (the expiration {expires} plus the \
                 {RETENTION}-second retention, floored to the hour), got {found:?}",
                expected_deletes(expires)
            );
            assert_eq!(
                found, &key.created,
                "case {case:?}: expected the listed key to carry the projection its create \
                 reported ({:?}), got {found:?}",
                key.created
            );
        }
        assert_eq!(
            listed, expected,
            "case maxResults {max_results:?}: expected the whole inventory in creation \
             order, got {listed:?}"
        );
    }

    // The retention span itself is FIXED: it does not scale with the validity the key was
    // given, so every key of an inventory spanning the whole window is the same distance
    // from its own expiration.
    let spans: BTreeSet<i64> = expected
        .iter()
        .map(|key| {
            key["deletes"].as_i64().unwrap_or_default() - key["expires"].as_i64().unwrap_or_default()
        })
        .collect();
    assert_eq!(
        spans,
        BTreeSet::from([RETENTION]),
        "case the whole inventory: expected one fixed retention span of {RETENTION} \
         seconds across every validity, got {spans:?}"
    );
}
