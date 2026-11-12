// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListCidrLocations`, the metadata-only read over a CIDR
//! collection: "a paginated list of CIDR locations for the given collection (metadata
//! only, does not include CIDR blocks)".
//!
//! The rules under test:
//!
//! 1. The listing reports exactly the locations the collection associates at least one
//!    block with — one row per location however many blocks it holds, and no row for a
//!    location every block has been taken out of — in one stable total order (ascending
//!    by name), whatever order the changes named them in.
//! 2. `MaxResults` ("the maximum number of CIDR collection locations to return in the
//!    response") bounds the page, and `NextToken` ("an opaque pagination token to
//!    indicate where the service is to begin enumerating results") resumes it: the pages
//!    partition the whole listing in that same order, and a token is answered exactly
//!    when locations remain.
//! 3. A collection id the account holds no collection for is the modelled
//!    `NoSuchCidrCollectionException` (HTTP 404), and an id outside the modelled UUID
//!    pattern or a `NextToken` this listing never minted is `InvalidInput` (HTTP 400) —
//!    none of them an empty listing.
//! 4. It is a read: repeating it answers the same locations and moves nothing the
//!    collection's own listing reports.
//!
//! Every test drives the service through the real router, using the SAME harness the
//! hosted-zone and CIDR-block suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees. The tables differ along what the rules generalise
//! over — the number of locations, the order they were named in, the characters in their
//! names, the number of blocks behind each one, and the page size — rather than merely
//! renaming a collection.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::DEFAULT_PAGE_SIZE;

use crate::cidr_collection_blocks::{
    change, change_collection, create_collection, DELETE_IF_EXISTS, PUT,
};
use crate::hosted_zone_records::{call, router, Reply};

/// The wire error codes this suite's refusals carry, each the shape name the model
/// declares for that error.
const NO_SUCH_COLLECTION: &str = "NoSuchCidrCollectionException";
const INVALID_INPUT: &str = "InvalidInput";

/// A well-formed collection id (`UUID`, pattern
/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`) that no create in this suite mints,
/// so it names no collection of the account.
const UNHELD_COLLECTION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// `ListCidrLocations` for the collection `id` with the query string `query` (`""` for
/// none): `GET /2013-04-01/cidrcollection/{CollectionId}`, as the model binds it.
async fn list_locations(router: &MultiServiceRouter, id: &str, query: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/cidrcollection/{id}{query}"),
        "",
    )
    .await
}

/// The location names a reply reports, in document order.
fn reported(reply: &Reply) -> Vec<String> {
    reply
        .all("LocationName")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Apply `changes` to the collection `id`, held to no version, and assert it was served.
async fn apply(router: &MultiServiceRouter, id: &str, case: &str, changes: &str) {
    let applied = change_collection(router, id, None, changes).await;
    assert_eq!(
        applied.status, 200,
        "case {case:?}: expected 200 from ChangeCidrCollection, got {} with body {}",
        applied.status, applied.body
    );
}

// ---------------------------------------------------------------------------
// What the listing reports
// ---------------------------------------------------------------------------

/// One row of the listing table: the changes that seed a collection, and the locations
/// the listing must then report, in the order it must report them.
struct ListingRow {
    case: &'static str,
    /// The `(location, blocks)` pairs one `PUT` change request associates, in the order
    /// the request names them.
    put: &'static [(&'static str, &'static [&'static str])],
    /// The `(location, blocks)` pairs a later `DELETE_IF_EXISTS` request takes back out.
    deleted: &'static [(&'static str, &'static [&'static str])],
    expected: &'static [&'static str],
}

/// The rows, differing along the count, the naming order, the characters in a name
/// (`CidrLocationNameDefaultNotAllowed`: 1..16 of `[0-9A-Za-z_\-]`) and how many blocks
/// stand behind each location.
const LISTING_ROWS: &[ListingRow] = &[
    ListingRow {
        case: "one location",
        put: &[("alpha", &["10.0.0.0/24"])],
        deleted: &[],
        expected: &["alpha"],
    },
    ListingRow {
        case: "locations named out of order",
        put: &[
            ("zulu", &["10.1.0.0/24"]),
            ("alpha", &["10.2.0.0/24"]),
            ("mike", &["10.3.0.0/24"]),
        ],
        deleted: &[],
        expected: &["alpha", "mike", "zulu"],
    },
    ListingRow {
        // The name pattern admits upper case, lower case, digits, `_` and `-`, so the
        // order is over the whole admitted alphabet, not just lower-case letters.
        case: "names differing in case and punctuation",
        put: &[
            ("us-east-1", &["10.4.0.0/24"]),
            ("A_1", &["10.5.0.0/24"]),
            ("a1", &["10.6.0.0/24"]),
            ("US-East_1", &["10.7.0.0/24"]),
        ],
        deleted: &[],
        expected: &["A_1", "US-East_1", "a1", "us-east-1"],
    },
    ListingRow {
        case: "a location holding several blocks",
        put: &[
            (
                "east",
                &["198.51.100.0/25", "198.51.100.128/25", "203.0.113.0/24"],
            ),
            ("west", &["192.0.2.0/24"]),
        ],
        deleted: &[],
        expected: &["east", "west"],
    },
    ListingRow {
        case: "a location every block was taken out of",
        put: &[
            ("east", &["198.51.100.0/24"]),
            ("west", &["203.0.113.0/24"]),
        ],
        deleted: &[("west", &["203.0.113.0/24"])],
        expected: &["east"],
    },
    ListingRow {
        case: "a location only some blocks were taken out of",
        put: &[("east", &["198.51.100.0/25", "198.51.100.128/25"])],
        deleted: &[("east", &["198.51.100.0/25"])],
        expected: &["east"],
    },
    ListingRow {
        case: "a collection no change has put a block in",
        put: &[],
        deleted: &[],
        expected: &[],
    },
];

/// Build one change request body out of a table row's `(location, blocks)` pairs.
fn changes_of(pairs: &[(&str, &[&str])], action: &str) -> String {
    pairs
        .iter()
        .map(|(location, blocks)| change(location, action, blocks))
        .collect()
}

#[tokio::test]
async fn the_listing_reports_every_location_the_collection_holds_a_block_in() {
    for row in LISTING_ROWS {
        let router = router();
        let id = create_collection(&router, "office-ranges").await;
        if !row.put.is_empty() {
            apply(&router, &id, row.case, &changes_of(row.put, PUT)).await;
        }
        if !row.deleted.is_empty() {
            apply(
                &router,
                &id,
                row.case,
                &changes_of(row.deleted, DELETE_IF_EXISTS),
            )
            .await;
        }

        let listed = list_locations(&router, &id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListCidrLocations, got {} with body {}",
            row.case, listed.status, listed.body
        );
        assert_eq!(
            reported(&listed),
            row.expected,
            "case {:?}: expected the locations the collection associates blocks with, got {}",
            row.case,
            listed.body
        );
        // Every row is far inside the page a request naming no `MaxResults` is served,
        // so the whole listing is one page and there is nothing left to resume from.
        assert!(
            (row.expected.len() as i32) < DEFAULT_PAGE_SIZE,
            "case {:?}: this table row must fit one default page",
            row.case
        );
        assert_eq!(
            listed.first("NextToken"),
            None,
            "case {:?}: a listing that reported everything must answer no token, got {:?}",
            row.case,
            listed.first("NextToken")
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The locations the paging table seeds, in the order the seeding change names them —
/// deliberately not sorted, so a page that merely echoes the request's order fails.
const PAGED_LOCATIONS: &[&str] = &[
    "loc-04", "loc-00", "loc-06", "loc-02", "loc-05", "loc-01", "loc-03",
];

/// The page sizes under test: one that divides the listing exactly, ones that leave a
/// remainder, one that is the whole listing, one past it, and the size a request naming
/// none is served.
const PAGE_SIZES: &[i32] = &[1, 2, 3, 7, 8, DEFAULT_PAGE_SIZE];

#[tokio::test]
async fn the_pages_partition_the_listing_in_its_own_order() {
    let router = router();
    let id = create_collection(&router, "paged-ranges").await;
    let seeding: String = PAGED_LOCATIONS
        .iter()
        .enumerate()
        .map(|(index, location)| change(location, PUT, &[&format!("10.{index}.0.0/24")]))
        .collect();
    apply(&router, &id, "the seeding change", &seeding).await;

    let mut whole: Vec<String> = PAGED_LOCATIONS.iter().map(|name| name.to_string()).collect();
    whole.sort();

    // The unpaged listing is the order the pages must partition.
    let listed = list_locations(&router, &id, "").await;
    assert_eq!(
        reported(&listed),
        whole,
        "case \"the whole listing\": expected every seeded location in name order, got {}",
        listed.body
    );

    for &page_size in PAGE_SIZES {
        let case = format!("maxresults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // Every page carries at least one location while any remain, so the walk cannot
        // take more pages than there are locations; one more iteration than that is a
        // paging defect rather than a slow test.
        for _ in 0..=PAGED_LOCATIONS.len() {
            let query = match &token {
                Some(token) => format!("?maxresults={page_size}&nexttoken={token}"),
                None => format!("?maxresults={page_size}"),
            };
            let page = list_locations(&router, &id, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from ListCidrLocations, got {} with body {}",
                page.status, page.body
            );
            let names = reported(&page);
            let expected_len = (whole.len() - walked.len()).min(page_size as usize);
            assert_eq!(
                names.len(),
                expected_len,
                "case {case:?}: expected a page of {expected_len} after {} walked, got \
                 {names:?}; body {}",
                walked.len(),
                page.body
            );
            walked.extend(names);
            token = page.first("NextToken").map(str::to_string);
            assert_eq!(
                token.is_some(),
                walked.len() < whole.len(),
                "case {case:?}: a token is answered exactly when locations remain, got \
                 {token:?} after {} of {}",
                walked.len(),
                whole.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the pages to partition the listing {whole:?}, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table: how the request addresses the collection, and the
/// refusal it must answer.
struct RefusalRow {
    case: &'static str,
    /// The collection id the request addresses, or `None` for the one this test holds.
    id: Option<&'static str>,
    query: &'static str,
    status: u16,
    code: &'static str,
}

const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "a collection id the account holds nothing for",
        id: Some(UNHELD_COLLECTION_ID),
        query: "",
        status: 404,
        code: NO_SUCH_COLLECTION,
    },
    RefusalRow {
        case: "a collection id outside the modelled pattern",
        id: Some("not-a-collection-id"),
        query: "",
        status: 400,
        code: INVALID_INPUT,
    },
    RefusalRow {
        case: "a next token this listing never minted",
        id: None,
        query: "?nexttoken=not.a.token",
        status: 400,
        code: INVALID_INPUT,
    },
    RefusalRow {
        case: "a next token this listing never minted, on a bounded page",
        id: None,
        query: "?maxresults=1&nexttoken=%2Fnot-a-token",
        status: 400,
        code: INVALID_INPUT,
    },
];

#[tokio::test]
async fn a_request_naming_no_page_of_this_listing_is_refused() {
    let router = router();
    let held = create_collection(&router, "held-ranges").await;
    apply(
        &router,
        &held,
        "the seeding change",
        &change("east", PUT, &["198.51.100.0/24"]),
    )
    .await;

    for row in REFUSAL_ROWS {
        let refused = list_locations(&router, row.id.unwrap_or(&held), row.query).await;
        assert_eq!(
            refused.status, row.status,
            "case {:?}: expected {}, got {} with body {}",
            row.case, row.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            row.code,
            "case {:?}: expected {}, got {:?}; body {}",
            row.case,
            row.code,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.first("LocationName").is_none(),
            "case {:?}: a refused listing must report no location; body {}",
            row.case,
            refused.body
        );
    }

    // The refusals changed nothing: the collection still reports the location it holds.
    let listed = list_locations(&router, &held, "").await;
    assert_eq!(
        reported(&listed),
        vec!["east".to_string()],
        "case \"after the refusals\": expected the held location, got {}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// The listing is a read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_the_locations_changes_nothing_a_later_caller_sees() {
    let router = router();
    let id = create_collection(&router, "read-ranges").await;
    apply(
        &router,
        &id,
        "the seeding change",
        &format!(
            "{}{}",
            change("east", PUT, &["198.51.100.0/24"]),
            change("west", PUT, &["203.0.113.0/24"])
        ),
    )
    .await;

    /// The `Version` `ListCidrCollections` reports for the collection `id`: "a
    /// sequential counter … incremented by 1 each time you update the collection", so a
    /// read that moved it would show up here.
    async fn version_of(router: &MultiServiceRouter, id: &str, case: &str) -> String {
        let listed = call(router, "GET", "/2013-04-01/cidrcollection", "").await;
        let ids = listed.all("Id");
        let versions = listed.all("Version");
        let at = ids
            .iter()
            .position(|held| *held == id)
            .unwrap_or_else(|| panic!("case {case:?}: the collection must be listed; body {}", listed.body));
        versions
            .get(at)
            .unwrap_or_else(|| panic!("case {case:?}: the collection must carry a version; body {}", listed.body))
            .to_string()
    }

    let before = version_of(&router, &id, "before the reads").await;
    let first = reported(&list_locations(&router, &id, "").await);
    // Read again, including one walked page by page: paging must not consume the
    // listing either.
    let page = list_locations(&router, &id, "?maxresults=1").await;
    let token = page
        .first("NextToken")
        .unwrap_or_else(|| panic!("the bounded page must answer a token; body {}", page.body))
        .to_string();
    let mut paged = reported(&page);
    paged.extend(reported(
        &list_locations(&router, &id, &format!("?maxresults=1&nexttoken={token}")).await,
    ));
    let again = reported(&list_locations(&router, &id, "").await);

    assert_eq!(
        again, first,
        "case \"repeated reads\": expected the same locations {first:?}, got {again:?}"
    );
    assert_eq!(
        paged, first,
        "case \"a paged read\": expected the same locations {first:?}, got {paged:?}"
    );
    let after = version_of(&router, &id, "after the reads").await;
    assert_eq!(
        after, before,
        "case \"the collection's version\": expected the reads to leave it at {before:?}, got \
         {after:?}"
    );
}
