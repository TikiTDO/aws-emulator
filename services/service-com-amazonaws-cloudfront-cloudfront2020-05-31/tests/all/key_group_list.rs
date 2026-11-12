// SPDX-License-Identifier: CC0-1.0

//! `ListKeyGroups`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's listing surface states:
//!
//! * the operation "gets a list of key groups": one summary per STORED key group, every
//!   member read from the record the last mutation wrote — the minted id, the instant it was
//!   written and the configuration as submitted — rather than from the request being served,
//!   so an account using none of the resource lists nothing;
//! * `MaxItems` cuts the page ("the maximum number of key groups that you want in the
//!   response"), `NextMarker` is present exactly when groups remain and names the id the page
//!   ended at, and "the response includes key groups in the list that occur after the
//!   marker" — so walking the pages at ANY admissible page size enumerates every group
//!   exactly once, in one order;
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the error the operation models (`InvalidArgument`, 400) and
//!   change nothing;
//! * a listing is a READ: the same request answers the same records again, and it follows
//!   every later mutation of the collection (an update is reported, a deleted group is not).
//!
//! The rows are the key group suite's table, which spans what a summary generalises over:
//! how many public key ids a group names (one, two, and the published quota), whether those
//! ids were minted by `CreatePublicKey` or are opaque strings a client submitted, and the
//! comment forms (absent, empty, plain ASCII, XML metacharacters and exactly the published
//! bound). The page sizes span sizes below, at and past the collection size, plus one past
//! this emulator's own ceiling.
//!
//! The harness (the router, the request helpers, the element scanner) and the key group
//! writer and readers are the sibling suites', reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_KEY_GROUP_MARKER_MESSAGE, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, Node, API_VERSION};
use crate::key_group_lifecycle::{create_group, get_group, table, Group};
use crate::public_key_lifecycle::is_wire_timestamp;

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/key-group`, with `query` appended verbatim (`?MaxItems=2&Marker=…`),
/// from the operation's `http` binding.
async fn list_groups(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-group{query}"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/key-group/{Id}` with the `KeyGroupConfig` document `body` under the
/// version token `if_match`.
async fn update_group(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-group/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// `DELETE /2020-05-31/key-group/{Id}` under the version token `if_match`.
async fn delete_group(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/key-group/{id}"),
        None,
        if_match,
    )
    .await
}

// ---------------------------------------------------------------------------
// Reading a page
// ---------------------------------------------------------------------------

/// The `KeyGroup` elements of one page, in the order they are serialised — each wrapped in
/// the `KeyGroupSummary` the model puts around it.
fn page_groups(page: &Answer) -> Vec<&Node> {
    page.root()
        .at("Items")
        .map(|items| {
            items
                .children_named("KeyGroupSummary")
                .into_iter()
                .filter_map(|summary| summary.at("KeyGroup"))
                .collect()
        })
        .unwrap_or_default()
}

/// The ids one page reports, in order.
fn page_ids(page: &Answer) -> Vec<String> {
    page_groups(page)
        .into_iter()
        .map(|group| group.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Assert that `group` — a `KeyGroup` element of a listed summary — reports exactly what the
/// record created from `row` holds: the minted `id`, the stored `last_modified` instant and
/// the configuration as submitted.
fn assert_listed(row: &Group, group: &Node, id: &str, last_modified: &str, raw: &str) {
    for (member, expected) in [
        ("Id", id.to_string()),
        ("LastModifiedTime", last_modified.to_string()),
    ] {
        let actual = group.text_at(member).map(str::to_string);
        assert_eq!(
            actual.as_deref(),
            Some(expected.as_str()),
            "case {}: expected the listed group's {member} to be {expected:?}, got {actual:?} \
             in {raw}",
            row.case
        );
    }
    let config = group
        .at("KeyGroupConfig")
        .unwrap_or_else(|| panic!("case {}: no configuration in {raw}", row.case));
    row.assert_matches(config, "ListKeyGroups", raw);
}

/// Create every row of `rows` through the API and report `(row, id, etag, last_modified)` in
/// creation order.
async fn create_all(
    router: &MultiServiceRouter,
    rows: &[Group],
) -> Vec<(Group, String, String, String)> {
    let mut created: Vec<(Group, String, String, String)> = Vec::new();
    for row in rows {
        let answer = create_group(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_minted_uuid(&id),
            "case {}: expected a minted key group id, got {id:?}",
            row.case
        );
        let last_modified = answer
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {}: expected a wire timestamp for LastModifiedTime, got {last_modified:?}",
            row.case
        );
        created.push((
            row.clone(),
            id,
            answer.header("etag").to_string(),
            last_modified,
        ));
    }
    created
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// An account that holds no key group lists nothing: the wrapper states a length of zero at
/// the default page size, carries no `Items` element at all and names no next page.
#[tokio::test]
async fn an_account_holding_no_key_groups_lists_no_items() {
    let router = emulator();

    let listed = list_groups(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case empty account: expected HTTP 200 from ListKeyGroups, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case empty account: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case empty account: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case empty account: an account holding no key group carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case empty account: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports one summary per stored key group, read from the record the create
/// stored, and walking the pages at any admissible page size enumerates every group exactly
/// once in one order.
#[tokio::test]
async fn a_listing_reports_every_stored_key_group_and_pages_through_them() {
    let router = emulator();
    let rows = table(&router).await;
    let created = create_all(&router, &rows).await;

    // One stable total order over the stored groups, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection, each
    // summary reporting what the stored record holds.
    let listed = list_groups(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_ids(&listed),
        order,
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(order.len().to_string().as_str()),
        "case whole collection: expected Quantity {}, got {}",
        order.len(),
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case whole collection: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
    for ((row, id, _, last_modified), group) in expected.iter().zip(page_groups(&listed)) {
        assert_listed(row, group, id, last_modified, &listed.raw);
    }

    // Walking the pages at ANY admissible page size enumerates every group exactly once, in
    // that same order: sizes below, at and past the collection size, and one past the
    // server's own ceiling.
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_groups(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {size}: expected HTTP 200 for {query}, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size}: expected the requested MaxItems echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            assert_eq!(
                page.root().text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size}: expected Quantity to state the page's length, got {}",
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let next = page.root().text_at("NextMarker").map(str::to_string);
            // `KeyGroupList` models no `IsTruncated`: the presence of `NextMarker` is the
            // whole of what says more groups remain.
            assert_eq!(
                next.is_some(),
                walked.len() < order.len(),
                "case page size {size}: expected NextMarker present {} after {} of {} items, \
                 got {}",
                walked.len() < order.len(),
                walked.len(),
                order.len(),
                page.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size}: expected NextMarker to name the id the page \
                         ended at, got {}",
                        page.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every group exactly \
             once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again, and the
    // addressed reader of each group still answers too.
    let again = list_groups(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
    for (row, id, _, _) in &created {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected the group to stay readable after the listings, got {} with {}",
            row.case, read.status, read.raw
        );
    }
}

/// `MaxItems` BOUNDS the page rather than demanding one, so a request that asks for none of
/// the items is served the empty page — the page size this service's one clamp
/// (`state::page_size`) gives a `MaxItems` of zero — rather than refused, with the size it
/// asked for echoed back and nothing from the collection consumed.
///
/// `KeyGroupList` models no `IsTruncated` and its `NextMarker` is "the value that you should
/// use in the Marker field of a subsequent request", which is the id a page ENDED at: a page
/// that ended at no group has no such value to report, so none is carried and the stored
/// groups are all still there for the next request to walk.
#[tokio::test]
async fn a_page_size_of_zero_serves_an_empty_page_and_consumes_nothing() {
    let router = emulator();
    let rows = table(&router).await;
    let created = create_all(&router, &rows).await;

    let page = list_groups(&router, "?MaxItems=0").await;
    assert_eq!(
        page.status, 200,
        "case MaxItems=0: expected HTTP 200, got {} with {}",
        page.status, page.raw
    );
    assert_eq!(
        page.root().text_at("MaxItems"),
        Some("0"),
        "case MaxItems=0: expected the requested MaxItems echoed, got {}",
        page.raw
    );
    assert_eq!(
        page.root().text_at("Quantity"),
        Some("0"),
        "case MaxItems=0: expected Quantity 0, got {}",
        page.raw
    );
    assert!(
        page.root().at("Items").is_none(),
        "case MaxItems=0: a page that lists nothing carries no Items element, got {}",
        page.raw
    );

    // Nothing was consumed: the very next request at a usable size still enumerates every
    // stored group.
    let whole = list_groups(&router, "").await;
    assert_eq!(
        page_ids(&whole).len(),
        created.len(),
        "case MaxItems=0: expected the {} stored groups to remain listable, got {}",
        created.len(),
        whole.raw
    );
}

/// A page size that names no page and a `Marker` that is no resume point this service handed
/// out are refused with the error the operation models, and change nothing. A well-shaped
/// marker naming no stored group is a resume POSITION rather than a refusal.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let rows = table(&router).await;
    let created = create_all(&router, &rows).await;
    let before: Vec<String> = page_ids(&list_groups(&router, "").await);

    // (case, query, whether the refusal is about the marker)
    let refusals = [
        ("a negative page size", "?MaxItems=-1", false),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number",
            false,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        (
            "a marker with too few groups for a key group id",
            "?Marker=0123456789ab",
            true,
        ),
        (
            "a marker whose groups are the wrong width",
            "?Marker=0123456-78ab-cdef-0123-456789abcdef",
            true,
        ),
        (
            "a marker in upper-case hex",
            "?Marker=0123456789AB-CDEF-0123-4567-89ABCDEF0123",
            true,
        ),
        (
            "a marker carrying characters outside hex",
            "?Marker=g123456789ab-cdef-0123-4567-89abcdef0123",
            true,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=K1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters an id cannot hold",
            "?Marker=0123456789ab%20cdef",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=01234567-89ab-cdef-0123-456789abcdef",
            false,
        ),
    ];
    for (case, query, over_marker) in refusals {
        let answer = list_groups(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        if over_marker {
            // What the refusal states is the CONSTRAINT on the parameter — a marker is a key
            // group id — so every unusable marker is refused with that one sentence rather
            // than with text quoted out of the request.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_KEY_GROUP_MARKER_MESSAGE,
                "case {case}: expected the marker constraint stated for {query}, got {}",
                answer.raw
            );
        }
    }

    // A well-shaped marker naming no stored group is a resume POSITION, not a refusal: the
    // page it names holds the groups ordered after it. The two bounds of the id space show
    // both ends of that order.
    for (case, marker, expected) in [
        (
            "the lowest id there is",
            "00000000-0000-0000-0000-000000000000",
            before.clone(),
        ),
        (
            "the highest id there is",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            Vec::new(),
        ),
    ] {
        let page = list_groups(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            page.status, 200,
            "case {case}: expected HTTP 200 resuming at {marker}, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page_ids(&page),
            expected,
            "case {case}: expected the groups ordered after {marker}, got {}",
            page.raw
        );
    }

    // Every refusal above left the store exactly as it was.
    let listed = list_groups(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created groups to be the listed ones"
    );
}

/// The listing answers from the STORE, so it follows every later mutation of the collection:
/// a group an update rewrote is listed as the update left it, and a group a delete removed is
/// listed no more.
#[tokio::test]
async fn a_listing_follows_the_mutations_of_the_collection() {
    let router = emulator();
    let rows = table(&router).await;
    let created = create_all(&router, &rows).await;

    // Rewrite the FIRST group created: a different name, a different comment and a shorter
    // key list, so every member of the summary has to come from the new record.
    let (row, id, etag, _) = created.first().cloned().expect("the table created groups");
    let rewritten = Group {
        case: format!("{} rewritten", row.case),
        name: format!("{}-rewritten", row.name),
        items: row.items[..1].to_vec(),
        comment: Some("rewritten by the listing suite".to_string()),
    };
    let updated = update_group(&router, &id, &rewritten.xml(), Some(&etag)).await;
    assert_eq!(
        updated.status, 200,
        "case {}: expected HTTP 200 from UpdateKeyGroup, got {} with {}",
        rewritten.case, updated.status, updated.raw
    );
    let updated_last_modified = updated
        .root()
        .text_at("LastModifiedTime")
        .unwrap_or_default()
        .to_string();

    // Delete the LAST group created, under the version token its create handed back.
    let (gone_row, gone_id, gone_etag, _) =
        created.last().cloned().expect("the table created groups");
    let deleted = delete_group(&router, &gone_id, Some(&gone_etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {}: expected HTTP 204 from DeleteKeyGroup, got {} with {}",
        gone_row.case, deleted.status, deleted.raw
    );

    let listed = list_groups(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case after mutations: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let ids = page_ids(&listed);
    assert_eq!(
        ids.len(),
        created.len() - 1,
        "case after mutations: expected {} groups listed, got {} with {}",
        created.len() - 1,
        ids.len(),
        listed.raw
    );
    assert!(
        !ids.contains(&gone_id),
        "case {}: a deleted group must not be listed, got {}",
        gone_row.case,
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(ids.len().to_string().as_str()),
        "case after mutations: expected Quantity to state the page's length, got {}",
        listed.raw
    );
    let listed_group = page_groups(&listed)
        .into_iter()
        .find(|group| group.text_at("Id") == Some(id.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "case {}: the updated group was not listed in {}",
                rewritten.case, listed.raw
            )
        });
    assert_listed(
        &rewritten,
        listed_group,
        &id,
        &updated_last_modified,
        &listed.raw,
    );
}
