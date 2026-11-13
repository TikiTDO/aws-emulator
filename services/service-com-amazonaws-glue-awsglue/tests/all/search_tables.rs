// SPDX-License-Identifier: CC0-1.0

//! `SearchTables`: the catalog-wide table search.
//!
//! The rule these suites assert: a search walks EVERY table of EVERY database in one
//! catalog and returns the stored tables whose own metadata, or whose parent database's
//! metadata, satisfies the request's free-text search and all of its property
//! predicates — so a name shared by tables in several databases is a hit in each —
//! ordered, paged and projected the way the single-table read projects a table.
//!
//! Every suite is a TABLE: its rows differ along what the rule generalises over (which
//! property a predicate names, the shape of the value matched, the comparator and the
//! form of a time value, the sort field and direction, the page size, the share type),
//! and the recorded "same name in every database" case is one row among them. Calls go
//! over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, ok};

/// Instants the fixture's `LastAccessTime` members carry, as epoch seconds — the scale
/// awsJson1_1 puts a modelled timestamp on the wire in.
const EARLY: i64 = 1_600_000_000;
/// The middle instant, whose RFC 3339 spelling is `2023-11-14T22:13:20Z`.
const MIDDLE: i64 = 1_700_000_000;
/// The latest instant of the three.
const LATE: i64 = 1_800_000_000;

/// One searched table, as a case compares them: the database that holds it and its name.
type Hit = (String, String);

/// `(database, table)` for every table of a `SearchTables` answer, in the order served.
fn hits(answer: &Value) -> Vec<Hit> {
    answer["TableList"]
        .as_array()
        .expect("TableList is a list")
        .iter()
        .map(|table| {
            (
                table["DatabaseName"]
                    .as_str()
                    .expect("Table.DatabaseName is a string")
                    .to_string(),
                table["Name"]
                    .as_str()
                    .expect("Table.Name is a string")
                    .to_string(),
            )
        })
        .collect()
}

/// The same, sorted — for a case that fixes WHICH tables are hits and not their order.
fn hit_set(answer: &Value) -> Vec<Hit> {
    let mut found = hits(answer);
    found.sort();
    found
}

/// `(database, table)` spelled the way a case writes an expectation.
fn hit(database: &str, table: &str) -> Hit {
    (database.to_string(), table.to_string())
}

/// A sorted expectation.
fn expected(pairs: &[(&str, &str)]) -> Vec<Hit> {
    let mut wanted: Vec<Hit> = pairs
        .iter()
        .map(|(database, table)| hit(database, table))
        .collect();
    wanted.sort();
    wanted
}

/// The catalog every suite here searches: three databases, nine tables, with the
/// metadata the rows select on. Three databases hold a table called `events`, which is
/// the recorded case; the `link` tables are the model's own fuzzy-match example
/// (`SearchTablesRequest$Filters`: `link` selects `customer-link` and `xx-link-yy` but
/// not `xxlinkyy`).
async fn seeded_catalog() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();

    for database in [
        json!({
            "Name": "sales",
            "Description": "the sales lake",
            "LocationUri": "s3://lake-sales/",
            "Parameters": { "team": "revenue" }
        }),
        json!({ "Name": "marketing" }),
        json!({ "Name": "archive", "Description": "cold storage" }),
    ] {
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": database }),
        )
        .await;
    }

    let tables: Vec<(&str, Value)> = vec![
        (
            "sales",
            json!({
                "Name": "events",
                "Owner": "analytics",
                "TableType": "EXTERNAL_TABLE",
                "Parameters": { "classification": "json" },
                "LastAccessTime": MIDDLE,
                "StorageDescriptor": {
                    "Location": "s3://lake-sales/events/",
                    "Columns": [
                        { "Name": "user_id", "Type": "string", "Comment": "the visitor" }
                    ]
                }
            }),
        ),
        (
            "sales",
            json!({
                "Name": "events_raw",
                "Owner": "analytics",
                "TableType": "EXTERNAL_TABLE",
                "LastAccessTime": LATE
            }),
        ),
        (
            "sales",
            json!({ "Name": "customer-link", "Owner": "crm", "TableType": "EXTERNAL_TABLE" }),
        ),
        (
            "sales",
            json!({ "Name": "xx-link-yy", "Owner": "crm", "TableType": "EXTERNAL_TABLE" }),
        ),
        (
            "sales",
            json!({ "Name": "xxlinkyy", "Owner": "crm", "TableType": "EXTERNAL_TABLE" }),
        ),
        (
            "marketing",
            json!({
                "Name": "events",
                "Owner": "growth",
                "TableType": "VIRTUAL_VIEW",
                "Parameters": { "classification": "csv" },
                "LastAccessTime": EARLY
            }),
        ),
        (
            "marketing",
            json!({
                "Name": "federated_orders",
                "Owner": "growth",
                "FederatedTable": {
                    "Identifier": "orders",
                    "DatabaseIdentifier": "marketing",
                    "ConnectionName": "warehouse-feed"
                }
            }),
        ),
        (
            "archive",
            json!({ "Name": "events", "Owner": "archivist", "Description": "cold events" }),
        ),
        (
            "archive",
            json!({
                "Name": "ledger",
                "Owner": "archivist",
                "Parameters": { "classification": "parquet" }
            }),
        ),
    ];
    for (database, table_input) in tables {
        ok(
            &router,
            "CreateTable",
            &json!({ "DatabaseName": database, "TableInput": table_input }),
        )
        .await;
    }

    router
}

/// Every table the fixture holds.
fn every_table() -> Vec<Hit> {
    expected(&[
        ("archive", "events"),
        ("archive", "ledger"),
        ("marketing", "events"),
        ("marketing", "federated_orders"),
        ("sales", "customer-link"),
        ("sales", "events"),
        ("sales", "events_raw"),
        ("sales", "xx-link-yy"),
        ("sales", "xxlinkyy"),
    ])
}

// ---------------------------------------------------------------------------
// SearchText
// ---------------------------------------------------------------------------

/// `SearchText` matches the whole searchable metadata of a table and of its parent
/// database, by punctuation-split token or by the whole value, and quoted text matches
/// only a value equal to it.
///
/// The rows differ along the shape of the text: a bare word that several databases hold
/// a table for (the recorded case), the model's own fuzzy-match example and its
/// counter-example, a value carrying punctuation, a name joined by underscores (which are
/// word characters of a catalog name, not punctuation to split on), text in another case,
/// text that only the PARENT database carries, a column name, two terms that must both
/// match, and the quoted exact form.
#[tokio::test]
async fn a_search_text_selects_tables_by_their_metadata_in_every_database() {
    let router = seeded_catalog().await;

    let cases: Vec<(&str, Value, Vec<Hit>)> = vec![
        (
            "a bare word three databases hold a table for",
            json!({ "SearchText": "events" }),
            expected(&[
                ("archive", "events"),
                ("marketing", "events"),
                ("sales", "events"),
            ]),
        ),
        (
            "an underscored name is ONE token, so it is reached only in full",
            json!({ "SearchText": "events_raw" }),
            expected(&[("sales", "events_raw")]),
        ),
        (
            "the same text in another case matches nothing",
            json!({ "SearchText": "EVENTS" }),
            Vec::new(),
        ),
        (
            "the model's fuzzy-match example: link",
            json!({ "SearchText": "link" }),
            expected(&[("sales", "customer-link"), ("sales", "xx-link-yy")]),
        ),
        (
            "the counter-example: xxlinkyy is one token",
            json!({ "SearchText": "xxlinkyy" }),
            expected(&[("sales", "xxlinkyy")]),
        ),
        (
            "text carrying punctuation matches the whole value",
            json!({ "SearchText": "customer-link" }),
            expected(&[("sales", "customer-link")]),
        ),
        (
            "text only the parent database carries",
            json!({ "SearchText": "lake" }),
            expected(&[
                ("sales", "customer-link"),
                ("sales", "events"),
                ("sales", "events_raw"),
                ("sales", "xx-link-yy"),
                ("sales", "xxlinkyy"),
            ]),
        ),
        (
            "a parent database's description",
            json!({ "SearchText": "cold" }),
            expected(&[("archive", "events"), ("archive", "ledger")]),
        ),
        (
            "a column name",
            json!({ "SearchText": "user_id" }),
            expected(&[("sales", "events")]),
        ),
        (
            "two terms, both of which must match",
            json!({ "SearchText": "events json" }),
            expected(&[("sales", "events")]),
        ),
        (
            "two terms no one table satisfies together",
            json!({ "SearchText": "events parquet" }),
            Vec::new(),
        ),
        (
            "quoted text matches a value equal to it, in every database",
            json!({ "SearchText": "\"events\"" }),
            expected(&[
                ("archive", "events"),
                ("marketing", "events"),
                ("sales", "events"),
            ]),
        ),
        (
            "a search naming nothing searches the whole catalog",
            json!({}),
            every_table(),
        ),
        (
            "text no stored table carries",
            json!({ "SearchText": "nowhere" }),
            Vec::new(),
        ),
    ];

    for (case, request, wanted) in cases {
        let answer = ok(&router, "SearchTables", &request).await;
        let found = hit_set(&answer);
        assert_eq!(
            found, wanted,
            "case {case:?}: expected {wanted:?}, got {found:?}"
        );
        assert!(
            answer.get("NextToken").is_none() || answer["NextToken"].is_null(),
            "case {case:?}: a single page must carry no continuation token, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters: one named property each
// ---------------------------------------------------------------------------

/// A `PropertyPredicate` narrows by the property its `Key` names, under the same fuzzy
/// match applied to BOTH sides — the stored value and the wanted one are tokenized by the
/// same algorithm and the predicate holds when they share a token — and several predicates
/// must all hold.
///
/// The rows differ along the property named (the table's own name, its parent database's
/// name, its owner, its storage location), along the shape of the wanted value (a bare
/// word, one carrying punctuation, one joined by underscores, one in another case) and
/// along how the key is spelled, plus a predicate naming NO key and two predicates
/// together.
#[tokio::test]
async fn b_property_predicates_select_tables_by_one_named_property() {
    let router = seeded_catalog().await;

    let cases: Vec<(&str, Value, Vec<Hit>)> = vec![
        (
            "Name, fuzzy over the name's tokens",
            json!({ "Filters": [ { "Key": "Name", "Value": "events" } ] }),
            expected(&[
                ("archive", "events"),
                ("marketing", "events"),
                ("sales", "events"),
            ]),
        ),
        (
            "Name, a wanted value carrying punctuation is tokenized too",
            json!({ "Filters": [ { "Key": "Name", "Value": "customer-link" } ] }),
            expected(&[("sales", "customer-link"), ("sales", "xx-link-yy")]),
        ),
        (
            "Name, an underscored value is one token and selects only that name",
            json!({ "Filters": [ { "Key": "Name", "Value": "events_raw" } ] }),
            expected(&[("sales", "events_raw")]),
        ),
        (
            "Name, the same value in another case selects nothing",
            json!({ "Filters": [ { "Key": "Name", "Value": "EVENTS" } ] }),
            Vec::new(),
        ),
        (
            "DatabaseName selects one database's tables",
            json!({ "Filters": [ { "Key": "DatabaseName", "Value": "archive" } ] }),
            expected(&[("archive", "events"), ("archive", "ledger")]),
        ),
        (
            "the same key in lower camel case names the same property",
            json!({ "Filters": [ { "Key": "databaseName", "Value": "archive" } ] }),
            expected(&[("archive", "events"), ("archive", "ledger")]),
        ),
        (
            "Owner",
            json!({ "Filters": [ { "Key": "Owner", "Value": "crm" } ] }),
            expected(&[
                ("sales", "customer-link"),
                ("sales", "xx-link-yy"),
                ("sales", "xxlinkyy"),
            ]),
        ),
        (
            "Location, which only one table carries",
            json!({ "Filters": [ { "Key": "Location", "Value": "events" } ] }),
            expected(&[("sales", "events")]),
        ),
        (
            "a predicate naming no key matches any property",
            json!({ "Filters": [ { "Value": "crm" } ] }),
            expected(&[
                ("sales", "customer-link"),
                ("sales", "xx-link-yy"),
                ("sales", "xxlinkyy"),
            ]),
        ),
        (
            "two predicates, both of which must hold",
            json!({ "Filters": [
                { "Key": "Name", "Value": "events" },
                { "Key": "Owner", "Value": "growth" }
            ] }),
            expected(&[("marketing", "events")]),
        ),
        (
            "a predicate beside a search text",
            json!({
                "SearchText": "events",
                "Filters": [ { "Key": "DatabaseName", "Value": "sales" } ]
            }),
            expected(&[("sales", "events")]),
        ),
    ];

    for (case, request, wanted) in cases {
        let answer = ok(&router, "SearchTables", &request).await;
        let found = hit_set(&answer);
        assert_eq!(
            found, wanted,
            "case {case:?}: expected {wanted:?}, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters on a time field
// ---------------------------------------------------------------------------

/// On a time field the predicate's `Comparator` orders the comparison, and the instant
/// may be written as epoch seconds or as an RFC 3339 date-time.
///
/// The rows cover every value of the `Comparator` enum (`smithy_query shape=Comparator`:
/// `EQUALS | GREATER_THAN | LESS_THAN | GREATER_THAN_EQUALS | LESS_THAN_EQUALS`), both
/// spellings of the instant, each published bound of the stored range and one past it,
/// a comparator left out, and a server-set time field beside the client-set one.
#[tokio::test]
async fn c_a_time_predicate_compares_the_stored_instant_under_its_comparator() {
    let router = seeded_catalog().await;

    let at_middle = expected(&[("sales", "events")]);
    let cases: Vec<(&str, Value, Vec<Hit>)> = vec![
        (
            "EQUALS, epoch seconds",
            json!({ "Filters": [
                { "Key": "LastAccessTime", "Value": MIDDLE.to_string(), "Comparator": "EQUALS" }
            ] }),
            at_middle.clone(),
        ),
        (
            "EQUALS, the same instant as RFC 3339",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": "2023-11-14T22:13:20Z",
                    "Comparator": "EQUALS"
                }
            ] }),
            at_middle.clone(),
        ),
        (
            "no comparator on a time field is the instant itself",
            json!({ "Filters": [
                { "Key": "LastAccessTime", "Value": MIDDLE.to_string() }
            ] }),
            at_middle.clone(),
        ),
        (
            "GREATER_THAN the middle instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": MIDDLE.to_string(),
                    "Comparator": "GREATER_THAN"
                }
            ] }),
            expected(&[("sales", "events_raw")]),
        ),
        (
            "GREATER_THAN_EQUALS the middle instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": MIDDLE.to_string(),
                    "Comparator": "GREATER_THAN_EQUALS"
                }
            ] }),
            expected(&[("sales", "events"), ("sales", "events_raw")]),
        ),
        (
            "LESS_THAN the middle instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": MIDDLE.to_string(),
                    "Comparator": "LESS_THAN"
                }
            ] }),
            expected(&[("marketing", "events")]),
        ),
        (
            "LESS_THAN_EQUALS the middle instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": MIDDLE.to_string(),
                    "Comparator": "LESS_THAN_EQUALS"
                }
            ] }),
            expected(&[("marketing", "events"), ("sales", "events")]),
        ),
        (
            "LESS_THAN the earliest stored instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": EARLY.to_string(),
                    "Comparator": "LESS_THAN"
                }
            ] }),
            Vec::new(),
        ),
        (
            "GREATER_THAN the latest stored instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": LATE.to_string(),
                    "Comparator": "GREATER_THAN"
                }
            ] }),
            Vec::new(),
        ),
        (
            "LESS_THAN_EQUALS the latest stored instant",
            json!({ "Filters": [
                {
                    "Key": "LastAccessTime",
                    "Value": LATE.to_string(),
                    "Comparator": "LESS_THAN_EQUALS"
                }
            ] }),
            expected(&[
                ("marketing", "events"),
                ("sales", "events"),
                ("sales", "events_raw"),
            ]),
        ),
        (
            "a server-set time field: every table was created after the epoch",
            json!({ "Filters": [
                { "Key": "CreateTime", "Value": "0", "Comparator": "GREATER_THAN" }
            ] }),
            every_table(),
        ),
        (
            "a server-set time field, the other way round",
            json!({ "Filters": [
                { "Key": "UpdateTime", "Value": "0", "Comparator": "LESS_THAN" }
            ] }),
            Vec::new(),
        ),
    ];

    for (case, request, wanted) in cases {
        let answer = ok(&router, "SearchTables", &request).await;
        let found = hit_set(&answer);
        assert_eq!(
            found, wanted,
            "case {case:?}: expected {wanted:?}, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// SortCriteria
// ---------------------------------------------------------------------------

/// `SortCriteria` orders the result by one field, ascending by default and `DESC` as the
/// exact reverse; tables carrying no value for the field come last ascending, and the
/// order is broken, and defaulted, by database name then table name.
///
/// The rows differ along the field sorted on (none, the name, another text property, a
/// time field) and the direction.
#[tokio::test]
async fn d_sort_criteria_order_the_result_by_one_field() {
    let router = seeded_catalog().await;

    let cases: Vec<(&str, Value, Vec<Hit>)> = vec![
        (
            "no sort criteria: database then table",
            json!({ "SearchText": "events" }),
            vec![
                hit("archive", "events"),
                hit("marketing", "events"),
                hit("sales", "events"),
            ],
        ),
        (
            "Name ascending",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "Name", "Sort": "ASC" } ]
            }),
            vec![
                hit("archive", "events"),
                hit("marketing", "events"),
                hit("sales", "events"),
            ],
        ),
        (
            "Name descending is the exact reverse",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "Name", "Sort": "DESC" } ]
            }),
            vec![
                hit("sales", "events"),
                hit("marketing", "events"),
                hit("archive", "events"),
            ],
        ),
        (
            "Owner ascending, ties broken by database then table",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "Owner", "Sort": "ASC" } ]
            }),
            vec![
                hit("sales", "events"),
                hit("archive", "events"),
                hit("marketing", "events"),
            ],
        ),
        (
            "LastAccessTime ascending: the table carrying none comes last",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "LastAccessTime", "Sort": "ASC" } ]
            }),
            vec![
                hit("marketing", "events"),
                hit("sales", "events"),
                hit("archive", "events"),
            ],
        ),
        (
            "LastAccessTime descending",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "LastAccessTime", "Sort": "DESC" } ]
            }),
            vec![
                hit("archive", "events"),
                hit("sales", "events"),
                hit("marketing", "events"),
            ],
        ),
        (
            "a sort naming no direction is ascending",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "Name" } ]
            }),
            vec![
                hit("archive", "events"),
                hit("marketing", "events"),
                hit("sales", "events"),
            ],
        ),
        (
            "a sort field in lower camel case orders by the same property",
            json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "databaseName", "Sort": "DESC" } ]
            }),
            vec![
                hit("sales", "events"),
                hit("marketing", "events"),
                hit("archive", "events"),
            ],
        ),
    ];

    for (case, request, wanted) in cases {
        let answer = ok(&router, "SearchTables", &request).await;
        let found = hits(&answer);
        assert_eq!(
            found, wanted,
            "case {case:?}: expected {wanted:?}, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Walk every page of `request` and return the tables served, in order, with the number
/// of pages it took.
async fn walk(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
    page_size: i64,
) -> (Vec<Hit>, usize) {
    let mut collected: Vec<Hit> = Vec::new();
    let mut pages = 0;
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        let members = body.as_object_mut().expect("a request is an object");
        members.insert("MaxResults".to_string(), json!(page_size));
        if let Some(cursor) = token.as_ref() {
            members.insert("NextToken".to_string(), json!(cursor));
        }
        let answer = ok(router, "SearchTables", &body).await;
        let page = hits(&answer);
        assert!(
            i64::try_from(page.len()).expect("a page length fits in i64") <= page_size,
            "case {case:?}: a page must hold at most {page_size} tables, got {page:?}"
        );
        collected.extend(page);
        pages += 1;
        assert!(
            pages <= 32,
            "case {case:?}: paging must terminate, got {collected:?}"
        );
        token = answer["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return (collected, pages);
        }
    }
}

/// `MaxResults` truncates the page and the returned `NextToken` resumes exactly after
/// the last table served, so a walk over any page size serves every hit once, in the
/// one order the search defines.
///
/// The rows differ along the page size (every size from one to past the hit count) and
/// the order being paged (the default, and a descending sort).
#[tokio::test]
async fn e_a_walk_over_any_page_size_serves_every_hit_exactly_once() {
    let router = seeded_catalog().await;

    let ascending = vec![
        hit("archive", "events"),
        hit("marketing", "events"),
        hit("sales", "events"),
    ];
    let descending: Vec<Hit> = ascending.iter().rev().cloned().collect();

    for page_size in [1, 2, 3, 4, 10] {
        let case = format!("default order, MaxResults {page_size}");
        let (served, pages) = walk(&router, &case, &json!({ "SearchText": "events" }), page_size)
            .await;
        assert_eq!(
            served, ascending,
            "case {case:?}: expected {ascending:?}, got {served:?}"
        );
        let wanted_pages = usize::try_from(
            (i64::try_from(ascending.len()).expect("the hit count fits in i64") + page_size - 1)
                / page_size,
        )
        .expect("a page count fits in usize");
        assert_eq!(
            pages, wanted_pages,
            "case {case:?}: expected {wanted_pages} pages, got {pages}"
        );

        let case = format!("Name DESC, MaxResults {page_size}");
        let (served, _) = walk(
            &router,
            &case,
            &json!({
                "SearchText": "events",
                "SortCriteria": [ { "FieldName": "Name", "Sort": "DESC" } ]
            }),
            page_size,
        )
        .await;
        assert_eq!(
            served, descending,
            "case {case:?}: expected {descending:?}, got {served:?}"
        );
    }

    // The whole catalog, paged one table at a time: the walk covers every stored table.
    let (served, pages) = walk(&router, "the whole catalog, MaxResults 1", &json!({}), 1).await;
    let mut sorted = served.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        every_table(),
        "case \"the whole catalog, MaxResults 1\": expected {:?}, got {sorted:?}",
        every_table()
    );
    assert_eq!(
        pages,
        every_table().len(),
        "case \"the whole catalog, MaxResults 1\": expected one page per table, got {pages}"
    );
}

// ---------------------------------------------------------------------------
// ResourceShareType
// ---------------------------------------------------------------------------

/// `ResourceShareType` selects which tables are searched at all: `FEDERATED` only those
/// referencing an entity outside the catalog, `FOREIGN` only those shared with this
/// account from another (of which a single-account store holds none), `ALL` both those
/// and the local ones, and a request naming none searches this account's own tables.
///
/// The rows are every value of the enum (`smithy_query shape=ResourceShareType`:
/// `FOREIGN | ALL | FEDERATED`) plus the absent member.
#[tokio::test]
async fn f_resource_share_type_selects_which_tables_are_searched() {
    let router = seeded_catalog().await;

    let cases: Vec<(&str, Value, Vec<Hit>)> = vec![
        ("absent: the account's own tables", json!({}), every_table()),
        (
            "ALL",
            json!({ "ResourceShareType": "ALL" }),
            every_table(),
        ),
        (
            "FEDERATED: only tables referencing an outside entity",
            json!({ "ResourceShareType": "FEDERATED" }),
            expected(&[("marketing", "federated_orders")]),
        ),
        (
            "FOREIGN: nothing is shared into a single-account store",
            json!({ "ResourceShareType": "FOREIGN" }),
            Vec::new(),
        ),
        (
            "FEDERATED beside a search text that excludes the federated table",
            json!({ "ResourceShareType": "FEDERATED", "SearchText": "events" }),
            Vec::new(),
        ),
    ];

    for (case, request, wanted) in cases {
        let answer = ok(&router, "SearchTables", &request).await;
        let found = hit_set(&answer);
        assert_eq!(
            found, wanted,
            "case {case:?}: expected {wanted:?}, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The projection, and a catalog the store does not hold
// ---------------------------------------------------------------------------

/// Every searched table is served in the SAME projection the single-table read serves,
/// and a catalog this store does not hold holds no tables.
///
/// The rows are every table of the fixture — each read back through `GetTable` — plus
/// the two catalog ids: the one the account owns (named explicitly) and one it does not.
#[tokio::test]
async fn g_a_searched_table_is_the_table_the_single_read_serves() {
    let router = seeded_catalog().await;

    let answer = ok(&router, "SearchTables", &json!({})).await;
    let searched = answer["TableList"]
        .as_array()
        .expect("TableList is a list")
        .clone();
    assert_eq!(
        searched.len(),
        every_table().len(),
        "case \"the whole catalog\": expected {} tables, got {}",
        every_table().len(),
        searched.len()
    );
    for table in &searched {
        let database = table["DatabaseName"].as_str().expect("a database name");
        let name = table["Name"].as_str().expect("a table name");
        let case = format!("{database}.{name}");
        let read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": name }),
        )
        .await;
        assert_eq!(
            table, &read["Table"],
            "case {case:?}: expected the GetTable projection {}, got {table}",
            read["Table"]
        );
    }

    let own = ok(
        &router,
        "SearchTables",
        &json!({ "CatalogId": crate::wire::ACCOUNT_ID }),
    )
    .await;
    let found = hit_set(&own);
    assert_eq!(
        found,
        every_table(),
        "case \"the account's own catalog id\": expected {:?}, got {found:?}",
        every_table()
    );

    // A catalog of ANOTHER account is reachable only through that catalog's own resource
    // policy, so the search over one is refused rather than answered with an empty page.
    let (status, body) = call(
        &router,
        "SearchTables",
        &json!({ "CatalogId": "210987654321" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a catalog of another account\": expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("AccessDeniedException"),
        "case \"a catalog of another account\": expected AccessDeniedException, got {body}"
    );
}

/// A `Comparator` sent beside a predicate that names no time field is refused rather than
/// ignored: the model states it "is used only for time fields", so there is nothing for it
/// to order and a search that applied the predicate anyway would answer a filter the
/// caller did not ask for.
///
/// The rows differ along which non-time key the comparator was sent beside — the table's
/// own name, a property of its parent database, its storage location, and a predicate
/// naming no key at all — and every value of the `Comparator` enum is covered by
/// the time-field suite above, which keeps serving them.
#[tokio::test]
async fn h_a_comparator_beside_a_key_that_is_no_time_field_is_refused() {
    let router = seeded_catalog().await;

    // (case, the Filters the request carries).
    let refused: Vec<(&str, Value)> = vec![
        (
            "a comparator beside the table's own Name",
            json!({ "Filters": [ { "Key": "Name", "Value": "events", "Comparator": "EQUALS" } ] }),
        ),
        (
            "a comparator beside the parent database's name",
            json!({ "Filters": [
                { "Key": "DatabaseName", "Value": "sales", "Comparator": "GREATER_THAN" }
            ] }),
        ),
        (
            "a comparator beside the table's storage Location",
            json!({ "Filters": [
                { "Key": "Location", "Value": "events", "Comparator": "LESS_THAN" }
            ] }),
        ),
        (
            "a comparator beside a predicate naming no key",
            json!({ "Filters": [ { "Value": "events", "Comparator": "EQUALS" } ] }),
        ),
    ];

    for (case, request) in &refused {
        let (status, body) = call(&router, "SearchTables", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a comparator with no time field to order must be refused, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str).unwrap_or_default(),
            "Comparator can only apply to time fields",
            "case {case:?}: unexpected wording, got {body}"
        );
    }

    // The same predicates WITHOUT a comparator are served, so the refusals above are about
    // the comparator and not about the keys.
    for (case, request) in &refused {
        let mut without = request.clone();
        for predicate in without["Filters"]
            .as_array_mut()
            .expect("Filters is a list")
        {
            predicate
                .as_object_mut()
                .expect("a predicate is an object")
                .remove("Comparator");
        }
        let (status, body) = call(&router, "SearchTables", &without).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same predicate without a comparator is served, got {status} \
             with {body}"
        );
    }

    // A comparator beside a key that names no searchable property at all is the SAME
    // refusal: the comparator is judged before the key, so the caller is told about the
    // member that orders nothing rather than about the one that selects nothing.
    let (status, body) = call(
        &router,
        "SearchTables",
        &json!({ "Filters": [
            { "Key": "NoSuchProperty", "Value": "events", "Comparator": "EQUALS" }
        ] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a comparator beside an unsupported key\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        body.get("Message").and_then(Value::as_str).unwrap_or_default(),
        "Comparator can only apply to time fields",
        "case \"a comparator beside an unsupported key\": unexpected wording, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A request the search cannot evaluate is refused with the operation's own
/// `InvalidInputException`, and the one member this slice does not serve answers 501.
///
/// The rows differ along what cannot be evaluated: a predicate with no value to compare
/// against, a time field compared with text that is no instant (on each of the two time
/// fields), and the view-status member nothing keeps.
#[tokio::test]
async fn h_a_request_the_search_cannot_evaluate_is_refused() {
    let router = seeded_catalog().await;

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a predicate carrying no Value",
            json!({ "Filters": [ { "Key": "Name" } ] }),
            400,
            "InvalidInputException",
        ),
        (
            "a predicate carrying neither Key nor Value",
            json!({ "Filters": [ {} ] }),
            400,
            "InvalidInputException",
        ),
        (
            "text that is no instant on LastAccessTime",
            json!({ "Filters": [ { "Key": "LastAccessTime", "Value": "yesterday" } ] }),
            400,
            "InvalidInputException",
        ),
        (
            "text that is no instant on CreateTime",
            json!({ "Filters": [
                { "Key": "CreateTime", "Value": "soon", "Comparator": "LESS_THAN" }
            ] }),
            400,
            "InvalidInputException",
        ),
        (
            "the view-status member nothing keeps",
            json!({ "IncludeStatusDetails": true }),
            501,
            "NotImplemented",
        ),
    ];

    for (case, request, status, code) in cases {
        let (answered, body) = call(&router, "SearchTables", &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        assert!(
            error_code(&body).contains(code),
            "case {case:?}: expected {code}, got {}",
            error_code(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// The property namespace a Key and a FieldName name
// ---------------------------------------------------------------------------

/// A predicate's `Key` and a `SortCriterion`'s `FieldName` name a property out of ONE
/// namespace — the table's own name, its parent database's name, its description, its
/// owner, its storage location and the four time fields — without regard to case; a name
/// outside that namespace selects, or orders by, nothing and is refused.
///
/// The rows differ along the key named: each property of the namespace in turn (the source
/// of the list is the `Table` shape's own members, `Table$Name`, `Table$DatabaseName`,
/// `Table$Description`, `Table$Owner` and `StorageDescriptor$Location`, plus
/// `Table$CreateTime`, `Table$UpdateTime`, `Table$LastAccessTime` and
/// `Table$LastAnalyzedTime`), each in two spellings, then names outside it: a member of the
/// table the search holds no property for, a key of the table's `Parameters` map, and a
/// name no shape declares.
#[tokio::test]
async fn i_a_key_outside_the_searchable_property_namespace_is_refused() {
    let router = seeded_catalog().await;

    // Every property of the namespace, each spelled as the `Table` shape spells it and in
    // the lower camel case an SDK's field names use.
    let served: Vec<&str> = vec![
        "Name",
        "name",
        "DatabaseName",
        "databaseName",
        "Description",
        "description",
        "Owner",
        "owner",
        "Location",
        "location",
        "CreateTime",
        "createTime",
        "UpdateTime",
        "updateTime",
        "LastAccessTime",
        "lastAccessTime",
        "LastAnalyzedTime",
        "lastAnalyzedTime",
    ];
    for key in served {
        // A time field is compared against an instant, every other property against text,
        // so the value is written for the property the key names.
        let value = if key.to_ascii_lowercase().ends_with("time") {
            MIDDLE.to_string()
        } else {
            "events".to_string()
        };
        let (status, body) = call(
            &router,
            "SearchTables",
            &json!({ "Filters": [ { "Key": key, "Value": value } ] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {key:?}: a predicate on a searchable property is served, got {status} with \
             {body}"
        );
        let (status, body) = call(
            &router,
            "SearchTables",
            &json!({ "SortCriteria": [ { "FieldName": key } ] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {key:?}: a sort on a searchable property is served, got {status} with {body}"
        );
    }

    let outside: Vec<(&str, &str)> = vec![
        ("a member the search holds no property for", "TableType"),
        ("the same member in lower camel case", "tableType"),
        ("a key of the table's Parameters map", "classification"),
        ("a name no shape declares", "NoSuchProperty"),
    ];
    for (case, key) in outside {
        let (status, body) = call(
            &router,
            "SearchTables",
            &json!({ "Filters": [ { "Key": key, "Value": "events" } ] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a predicate key outside the namespace is refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str).unwrap_or_default(),
            "Predicate key not supported",
            "case {case:?}: unexpected wording, got {body}"
        );

        let (status, body) = call(
            &router,
            "SearchTables",
            &json!({ "SortCriteria": [ { "FieldName": key, "Sort": "ASC" } ] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a sort field outside the namespace is refused, got {status} with \
             {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str).unwrap_or_default(),
            "Invalid sort field name",
            "case {case:?}: unexpected wording, got {body}"
        );
    }

    // Wrong about both: the ORDERING is judged first, so that is what the caller is told.
    let (status, body) = call(
        &router,
        "SearchTables",
        &json!({
            "Filters": [ { "Key": "tableType", "Value": "EXTERNAL_TABLE" } ],
            "SortCriteria": [ { "FieldName": "NoSuchProperty", "Sort": "ASC" } ]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"both the sort field and the predicate key are outside the namespace\": \
         expected 400, got {status} with {body}"
    );
    assert_eq!(
        body.get("Message").and_then(Value::as_str).unwrap_or_default(),
        "Invalid sort field name",
        "case \"both the sort field and the predicate key are outside the namespace\": \
         unexpected wording, got {body}"
    );

    // An EMPTY field name never reaches the namespace check: `SortCriterion$FieldName`
    // carries the model's own minimum length, which the boundary walk enforces, so the
    // refusal is the constraint's rather than this search's.
    let (status, body) = call(
        &router,
        "SearchTables",
        &json!({ "SortCriteria": [ { "FieldName": "", "Sort": "ASC" } ] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an empty sort field name\": expected 400, got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// MaxResults
// ---------------------------------------------------------------------------

/// `MaxResults` truncates the page, and the search does not hold it to the range its
/// modelled type declares: a request that asks for NO results is SERVED, with the empty
/// `TableList` it asked for and no continuation token, a page larger than the maximum is
/// served as asked (the same leniency the catalog's other table listing has), and only a
/// NEGATIVE page size is refused with the operation's own `InvalidInputException`.
///
/// The rows are each published bound of the member's modelled range (`PageSize`, range
/// 1..1000) and one past it on each side, plus the zero the range excludes.
#[tokio::test]
async fn j_max_results_outside_its_modelled_range_is_served_or_refused_by_the_search() {
    let router = seeded_catalog().await;

    // (case, MaxResults, how many tables the page holds).
    let served: Vec<(&str, i64, usize)> = vec![
        ("the smallest page the range admits", 1, 1),
        ("the largest page the range admits", 1_000, every_table().len()),
        ("one past that maximum", 1_001, every_table().len()),
        ("no page at all", 0, 0),
    ];
    for (case, max_results, held) in served {
        let answer = ok(&router, "SearchTables", &json!({ "MaxResults": max_results })).await;
        let page = answer["TableList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: TableList must be present, got {answer}"));
        assert_eq!(
            page.len(),
            held,
            "case {case:?}: expected {held} tables, got {}",
            page.len()
        );
        if held == 0 {
            assert!(
                answer.get("NextToken").is_none() || answer["NextToken"].is_null(),
                "case {case:?}: a page of nothing carries no continuation token, got {answer}"
            );
        }
    }

    for (case, max_results) in [("one below zero", -1), ("far below zero", -1_000)] {
        let (status, body) = call(
            &router,
            "SearchTables",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a page smaller than none is refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
    }
}
