// SPDX-License-Identifier: CC0-1.0

//! The deletion time `ListApiKeys` reports for a key, over every SHAPE the listing can
//! be paged in, end to end over HTTP.
//!
//! The arithmetic itself — a `da2` key's `deletes` is its `expires` plus the service's
//! post-expiry retention, floored to the hour — and the fact that the expiration alone
//! determines it are asserted by [`crate::api_key_deletion_window`], whose constants,
//! helpers and rule assertion this suite reuses rather than restating. What this suite
//! adds is the dimension a client inventorying keys actually varies: the PAGINATION
//! SHAPE of the listing. `deletes` is a stored property of the key, written by the call
//! that set its expiration, so the page size a caller asks for, the page a key lands on
//! and the number of keys its API holds take no part in the value reported for it.
//!
//! The rows span:
//!
//! 1. the number of keys the API holds — one, two, several, exactly one modelled page,
//!    and more than one page — so no key count can be the one the projection is right
//!    for;
//! 2. the `maxResults` the caller names — unspecified, the modelled default sent
//!    explicitly, and values inside the modelled range up to its published maximum —
//!    plus, in the refusal test, one past each end of that range;
//! 3. the route each key reached its current expiration by, since one API's keys are
//!    re-walked at every page size after some of them are moved by `UpdateApiKey`;
//! 4. the position of each key's expiration inside its hour, so the flooring is
//!    exercised on every page rather than only on the first.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, expected_deletes, floor_to_hour, unix_now, update_key,
    BOUND_SLACK, DAY, HOUR, MIN_VALIDITY,
};
use crate::error_contract::{call, create_api, router, Answer};

/// The page a listing naming no `maxResults` serves: the whole modelled maximum. Source:
/// the `maxResults` member of `ListApiKeysRequest` (`@range` 0..25, default 0 — i.e.
/// unspecified), so the expectation is fixed by the MODEL rather than by whatever default
/// the implementation happens to carry.
const MODELLED_PAGE: usize = 25;

/// The counts of keys an API is listed at: none, one, two, several, exactly one modelled
/// page, and more than one page. All are well inside the service quota for keys on one
/// API, so every row's seeding succeeds.
const COUNTS: &[usize] = &[0, 1, 2, 7, MODELLED_PAGE, MODELLED_PAGE + 3];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListApiKeysRequest` (`@range` 0..25, default 0 —
/// i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(3), Some(7), Some(25)];

/// How far into its hour the expiration of the key at each position sits, rotated over
/// the positions the flooring has to treat alike: the boundary itself, the second after
/// it, the second before the next one, and one mid-hour.
const WITHIN_HOUR: &[i64] = &[0, 1, HOUR - 1, 1_234];

/// How far apart two seeded keys' expirations sit. Chosen so the largest row's keys all
/// stay inside the published validity window: the last of `MODELLED_PAGE + 3` keys sits
/// about 325 days out, short of the one-year maximum.
const EXPIRATION_STEP: i64 = 12 * DAY;

/// The validity, in seconds past the clock the seeding read, of the key at `index` — a
/// distinct expiration per key, each at a different position in its hour, so a listing
/// reporting a neighbour's derivation for a key could not pass.
fn seeded_validity(index: usize) -> i64 {
    MIN_VALIDITY
        + BOUND_SLACK
        + (index as i64) * EXPIRATION_STEP
        + WITHIN_HOUR[index % WITHIN_HOUR.len()]
}

/// Create `count` API keys under `api_id`, asserting each create's own projection, and
/// return the keys as the creates reported them, in creation order.
async fn seed(router: &MultiServiceRouter, api_id: &str, count: usize) -> Vec<Value> {
    let mut created = Vec::with_capacity(count);
    for index in 0..count {
        let case = format!("seed key {index} of {count}");
        let submitted = unix_now() + seeded_validity(index);
        let answer = create_key(
            router,
            api_id,
            json!({ "description": case, "expires": submitted }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected CreateApiKey 200, got {answer:?}"
        );
        let key = answer.body["apiKey"].clone();
        let expires = assert_retention_span(&case, "CreateApiKey", &key);
        assert_eq!(
            expires,
            floor_to_hour(submitted),
            "case {case}: expected the submitted expiration {submitted} floored to the \
             hour, got {key:?}"
        );
        created.push(key);
    }
    created
}

/// Send one `ListApiKeys`, with the `maxResults` and `nextToken` the caller names.
async fn list_page(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let path = if query.is_empty() {
        format!("/v1/apis/{api_id}/apikeys")
    } else {
        format!("/v1/apis/{api_id}/apikeys?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// Page through the whole API at `max_results` and return every key the listing
/// reported, in the order it reported them, asserting on each page that it holds at
/// most the page size, that it is as full as the remainder allows, that the cursor is
/// reported exactly while keys remain, and that every key on it carries the deletion
/// time its own expiration derives.
async fn walk(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    // An unspecified `maxResults` — absent, or the modelled default of `0` — selects
    // the whole modelled page.
    let bound = match max_results {
        Some(size) if size > 0 => size as usize,
        _ => MODELLED_PAGE,
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one key while keys remain, so the walk cannot take more
    // pages than there are keys, plus the single page of an empty listing.
    for page_index in 0..expected_total + 1 {
        let case = format!("walk at maxResults {max_results:?}, page {page_index} from {token:?}");
        let answer = list_page(router, api_id, max_results, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected ListApiKeys 200, got {answer:?}"
        );
        let keys = answer.body["apiKeys"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an apiKeys list, got {answer:?}"))
            .clone();
        assert!(
            keys.len() <= bound,
            "case {case}: expected at most {bound} keys on a page, got {} ({keys:?})",
            keys.len()
        );
        assert!(
            collected.len() + keys.len() <= expected_total,
            "case {case}: expected at most the {expected_total} key(s) the API holds, got \
             {} already and {} more ({keys:?})",
            collected.len(),
            keys.len()
        );
        let remaining = expected_total.saturating_sub(collected.len());
        assert_eq!(
            keys.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows \
             ({}), got {keys:?}",
            remaining.min(bound)
        );
        for (offset, key) in keys.iter().enumerate() {
            assert_retention_span(&format!("{case}, entry {offset}"), "ListApiKeys", key);
        }
        collected.extend(keys);
        let more = collected.len() < expected_total;
        let next_token = answer.body["nextToken"].as_str().map(str::to_string);
        assert_eq!(
            next_token.is_some(),
            more,
            "case {case}: expected a nextToken {}, got {next_token:?}",
            if more {
                "while keys remain"
            } else {
                "on the last page"
            }
        );
        match next_token {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!("case walk at maxResults {max_results:?}: the walk never reached a last page");
}

/// However the caller pages, and however many keys the API holds, every key is reported
/// exactly once, in creation order, with the value its own write computed — so the page
/// size and the page a key lands on take no part in the derivation.
#[tokio::test]
async fn every_page_size_reports_each_key_with_the_deletion_time_it_derives() {
    let router = router();

    for count in COUNTS.iter().copied() {
        let api_id = create_api(&router, &format!("Api_key_deletes_pages_{count}")).await;
        let created = seed(&router, &api_id, count).await;

        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, &api_id, max_results, count).await;
            assert_eq!(
                listed, created,
                "case {count} keys at maxResults {max_results:?}: expected the keys the \
                 creates stored, in creation order, got {listed:?}"
            );
            for (index, key) in listed.iter().enumerate() {
                let expires = key["expires"].as_i64().unwrap_or_else(|| {
                    panic!(
                        "case {count} keys at maxResults {max_results:?}, key {index}: \
                         expected an expires, got {key:?}"
                    )
                });
                assert_eq!(
                    key["deletes"].as_i64(),
                    Some(expected_deletes(expires)),
                    "case {count} keys at maxResults {max_results:?}, key {index}: \
                     expected deletes {} for the expiration {expires}, got {key:?}",
                    expected_deletes(expires)
                );
            }
        }
    }
}

/// A key moved onto a new expiration is reported with the deletion time that NEW
/// expiration derives, at every page size — the listing projects the stored key rather
/// than anything the page it is cut on could supply.
#[tokio::test]
async fn an_updated_key_keeps_one_deletion_time_at_every_page_size() {
    let router = router();
    let count = MODELLED_PAGE + 3;
    let api_id = create_api(&router, "Api_key_deletes_pages_updated").await;
    let mut expected = seed(&router, &api_id, count).await;

    // Move the first key, one in the middle of the second page, and the last one, so the
    // walk crosses a page boundary between two keys whose expirations were set by
    // different calls. The new expirations run the other way round the window than the
    // seeded ones, so a listing that reported a neighbour's value could not pass.
    for position in [0, count / 2, count - 1] {
        let case = format!("move key {position} of {count}");
        let submitted = unix_now() + seeded_validity(count - position) + 7 * HOUR;
        let key_id = expected[position]["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the stored key to carry an id, got {:?}",
                    expected[position]
                )
            })
            .to_string();
        let answer = update_key(&router, &api_id, &key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        let expires = assert_retention_span(&case, "UpdateApiKey", &updated);
        assert_eq!(
            expires,
            floor_to_hour(submitted),
            "case {case}: expected the submitted expiration {submitted} floored to the \
             hour, got {updated:?}"
        );
        expected[position] = updated;
    }

    for max_results in PAGE_SIZES.iter().copied() {
        let listed = walk(&router, &api_id, max_results, count).await;
        assert_eq!(
            listed, expected,
            "case the moved keys at maxResults {max_results:?}: expected every key as its \
             own last write reported it, got {listed:?}"
        );
    }
}

/// A `maxResults` outside the modelled range is refused, and a refused listing changes
/// no key: the next listing reports the same keys with the same deletion times.
///
/// The rows sit one past each published end of the range (`@range` 0..25 on
/// `ListApiKeysRequest$maxResults`), so the refusal belongs to the bound rather than to
/// one chosen value.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_moves_no_deletion_time() {
    let router = router();
    let count = 4;
    let api_id = create_api(&router, "Api_key_deletes_pages_refused").await;
    let created = seed(&router, &api_id, count).await;

    for max_results in [-1, 26] {
        let case = format!("maxResults {max_results}");
        let answer = list_page(&router, &api_id, Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected ListApiKeys 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {case}: expected error type BadRequestException, got {answer:?}"
        );

        // The refusal left the inventory alone, and the stored derivation is still what a
        // served listing reports — read back both with no page size at all and one key per
        // page, so the check spans a single-page and a multi-page walk.
        for accepted in [None, Some(1)] {
            let listed = walk(&router, &api_id, accepted, count).await;
            assert_eq!(
                listed, created,
                "case {case}, then a listing at maxResults {accepted:?}: expected the \
                 stored keys unchanged, got {listed:?}"
            );
        }
    }
}
