// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetPartitions`: the listing that serves "information about the
//! partitions in a table".
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a partition listing reports the partitions the addressed table
//! holds, in the ONE order the catalog keeps them in (lexicographic by value list), each
//! projected exactly as the single-partition read projects it; `Expression` selects by the
//! `WHERE`-clause filter it spells over the table's partition keys, compared in each key's
//! declared type; `Segment` narrows the scan to a non-overlapping region of that order;
//! `MaxResults` and `NextToken` page it; and `ExcludeColumnSchema` leaves out the column
//! schema and nothing else. A read changes nothing a later caller sees.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys a table defines and of which types, which operator and which key type
//! the filter uses, how large a page is asked for, how many segments a scan is split into,
//! and the ways a filter can fail to be one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{call, emulator, error_code, ok};

/// The largest `TotalSegments` a scan may be split into: `Segment$TotalSegments`, range
/// 1..10, from `smithy_query` on `GetPartitions`.
const MAX_TOTAL_SEGMENTS: i64 = 10;

/// Create `database` holding `table`, partitioned by `keys` — one `Column` per key, with
/// the type that key is declared with. A database a previous call of this helper already
/// created is used as it stands, so one suite can seed two tables of one database.
async fn seed_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    keys: &[(&str, &str)],
) {
    let (status, body) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&body).contains("AlreadyExistsException"),
        "seeding {database}: expected the database to exist, got {status} with {body}"
    );
    let partition_keys: Vec<Value> = keys
        .iter()
        .map(|(name, declared)| json!({ "Name": name, "Type": declared }))
        .collect();
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": table, "PartitionKeys": partition_keys }
        }),
    )
    .await;
}

/// Create one partition of `table` at `values`, with a storage descriptor naming a
/// location and one column, so a listing has something to project and something for
/// `ExcludeColumnSchema` to leave out.
async fn create_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[&str],
) {
    ok(
        router,
        "CreatePartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionInput": {
                "Values": values,
                "StorageDescriptor": {
                    "Location": format!("s3://lake/{table}/{}", values.join("/")),
                    "Columns": [{ "Name": "payload", "Type": "string" }]
                }
            }
        }),
    )
    .await;
}

/// One `GetPartitions` call that must answer 200, and its body.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "GetPartitions", request).await
}

/// The `Values` of each partition a listing reported, in the order it reported them.
fn listed_values(body: &Value) -> Vec<Vec<String>> {
    body["Partitions"]
        .as_array()
        .unwrap_or_else(|| panic!("GetPartitions must report a Partitions list, got {body}"))
        .iter()
        .map(|partition| {
            partition["Values"]
                .as_array()
                .unwrap_or_else(|| panic!("a reported partition carries Values, got {partition}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("a partition value is a string, got {value}"))
                        .to_string()
                })
                .collect()
        })
        .collect()
}

/// `values`, sorted the way the catalog orders a table's partitions: lexicographic by
/// value list.
fn in_catalog_order(values: &[Vec<&str>]) -> Vec<Vec<String>> {
    let mut ordered: Vec<Vec<String>> = values
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect();
    ordered.sort();
    ordered
}

/// A table of four partition keys, one of each type class a filter compares, and the
/// partitions it holds. Returned as the rows a filter suite selects from.
async fn seed_typed_table(router: &MultiServiceRouter) -> Vec<Vec<&'static str>> {
    seed_table(
        router,
        "filters",
        "events",
        &[
            ("region", "string"),
            ("year", "int"),
            ("dt", "date"),
            ("amount", "decimal(10,2)"),
        ],
    )
    .await;
    // Partition values spanning what the comparisons have to get right: a numeric key
    // whose text order disagrees with its numeric order (9 against 10 and 100), a date
    // shared by two partitions, and a decimal that is whole beside ones that are not.
    let rows = vec![
        vec!["us-east-1", "9", "2024-01-05", "1.5"],
        vec!["us-east-1", "10", "2024-02-10", "10.25"],
        vec!["eu-west-1", "2024", "2024-02-10", "2"],
        vec!["ap-south-1", "100", "2025-12-31", "0.5"],
    ];
    for row in &rows {
        create_partition(router, "filters", "events", row).await;
    }
    rows
}

/// One row of the catalog-order suite: the case name, the partition keys the table is
/// created with (each a name and the type it is declared with), and the value lists to
/// create partitions at.
type ListingCase<'row> = (&'row str, Vec<(&'row str, &'row str)>, Vec<Vec<&'row str>>);

/// A listing reports every partition of the addressed table, in the catalog's order, each
/// projected exactly as the single-partition read projects it — and reading it twice
/// answers the same thing, because a read changes nothing.
#[tokio::test]
async fn a_listing_serves_every_partition_in_catalog_order() {
    // Rows differ along how many partition keys the table defines, how many partitions it
    // holds, and the form and casing of a partition value.
    let cases: Vec<ListingCase<'_>> = vec![
        (
            "one key, created out of order",
            vec![("dt", "string")],
            vec![vec!["2024-03-01"], vec!["2024-01-01"], vec!["2024-02-01"]],
        ),
        (
            "two keys",
            vec![("region", "string"), ("dt", "string")],
            vec![
                vec!["us-east-1", "2024-01-02"],
                vec!["eu-west-1", "2024-01-01"],
                vec!["us-east-1", "2024-01-01"],
            ],
        ),
        (
            "three keys of three types",
            vec![("region", "string"), ("year", "int"), ("dt", "date")],
            vec![
                vec!["us-east-1", "9", "2024-01-05"],
                vec!["us-east-1", "10", "2024-01-06"],
            ],
        ),
        (
            "values differing only in case",
            vec![("tag", "string")],
            vec![vec!["Alpha"], vec!["alpha"], vec!["ALPHA"]],
        ),
        ("a table with no partitions", vec![("dt", "string")], vec![]),
    ];

    for (case, keys, values) in cases {
        let router = emulator();
        seed_table(&router, "listing", "events", &keys).await;
        for row in &values {
            create_partition(&router, "listing", "events", row).await;
        }

        let request = json!({ "DatabaseName": "listing", "TableName": "events" });
        let body = list(&router, &request).await;
        let expected = in_catalog_order(&values);
        assert_eq!(
            listed_values(&body),
            expected,
            "case {case:?}: expected the partitions in catalog order {expected:?}, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: a listing that served every partition must carry no NextToken, \
             got {body}"
        );

        // Each element is the SAME projection the single-partition read serves.
        for (index, row) in expected.iter().enumerate() {
            let read = ok(
                &router,
                "GetPartition",
                &json!({
                    "DatabaseName": "listing",
                    "TableName": "events",
                    "PartitionValues": row
                }),
            )
            .await;
            assert_eq!(
                body["Partitions"][index], read["Partition"],
                "case {case:?}: partition {row:?} must be listed as it is read, got {body}"
            );
        }

        // Reporting only: the second listing answers exactly the first.
        let again = list(&router, &request).await;
        assert_eq!(
            again, body,
            "case {case:?}: listing a table twice must answer the same partitions, got {again}"
        );
    }
}

/// An `Expression` selects the partitions it names, comparing each partition key in the
/// type that key is declared with.
#[tokio::test]
async fn an_expression_selects_the_partitions_it_names() {
    let router = emulator();
    let rows = seed_typed_table(&router).await;
    let all: Vec<Vec<&str>> = rows.clone();

    // Rows differ along the operator, the key type it compares and how many partitions it
    // selects. The expected set is written out per row as the partition values it admits.
    let cases: Vec<(&str, &str, Vec<Vec<&str>>)> = vec![
        (
            "equality on a string key",
            "region = 'us-east-1'",
            vec![all[0].clone(), all[1].clone()],
        ),
        (
            "inequality on a string key",
            "region <> 'us-east-1'",
            vec![all[2].clone(), all[3].clone()],
        ),
        (
            "IN on a string key",
            "region IN ('eu-west-1', 'ap-south-1')",
            vec![all[2].clone(), all[3].clone()],
        ),
        (
            "NOT IN on a numeric key",
            "year NOT IN (9, 10)",
            vec![all[2].clone(), all[3].clone()],
        ),
        (
            "LIKE with a trailing run wildcard",
            "region LIKE 'us-%'",
            vec![all[0].clone(), all[1].clone()],
        ),
        (
            "LIKE with a single-character wildcard",
            "region LIKE 'us-east-_'",
            vec![all[0].clone(), all[1].clone()],
        ),
        (
            "a numeric key compares as a number, not as text",
            "year < 10",
            vec![all[0].clone()],
        ),
        (
            "a numeric range with AND",
            "year >= 10 AND year <= 100",
            vec![all[1].clone(), all[3].clone()],
        ),
        (
            "BETWEEN on a numeric key",
            "year BETWEEN 10 AND 2024",
            vec![all[1].clone(), all[2].clone(), all[3].clone()],
        ),
        (
            "NOT of an equality",
            "NOT (year = 9)",
            vec![all[1].clone(), all[2].clone(), all[3].clone()],
        ),
        (
            "a date key compares by calendar component",
            "dt > '2024-01-05'",
            vec![all[1].clone(), all[2].clone(), all[3].clone()],
        ),
        (
            "a date written without leading zeroes names the same day",
            "dt = '2024-2-10'",
            vec![all[1].clone(), all[2].clone()],
        ),
        (
            "a date key compares against a literal naming a time of day",
            "dt < '2024-02-10 00:00:01'",
            vec![all[0].clone(), all[1].clone(), all[2].clone()],
        ),
        (
            "NOT BETWEEN on a date key",
            "dt NOT BETWEEN '2024-01-01' AND '2024-12-31'",
            vec![all[3].clone()],
        ),
        (
            "a decimal key compares as a number",
            "amount > 1",
            vec![all[0].clone(), all[1].clone(), all[2].clone()],
        ),
        (
            "a partition key name is matched without regard to case",
            "YEAR = 9",
            vec![all[0].clone()],
        ),
        (
            "a qualified column name names the key",
            "events.year = 9",
            vec![all[0].clone()],
        ),
        (
            "OR of two keys",
            "region = 'us-east-1' OR year = 100",
            vec![all[0].clone(), all[1].clone(), all[3].clone()],
        ),
        ("IS NOT NULL selects every partition", "region IS NOT NULL", all.clone()),
        ("IS NULL selects none", "region IS NULL", vec![]),
        (
            "a quoted number filters a numeric key by its value",
            "year = '2024'",
            vec![all[2].clone()],
        ),
        ("an expression of no characters filters nothing", "", all.clone()),
        (
            "a filter nothing satisfies",
            "region = 'sa-east-1'",
            vec![],
        ),
    ];

    for (case, expression, expected) in cases {
        let body = list(
            &router,
            &json!({
                "DatabaseName": "filters",
                "TableName": "events",
                "Expression": expression
            }),
        )
        .await;
        let expected = in_catalog_order(&expected);
        assert_eq!(
            listed_values(&body),
            expected,
            "case {case:?}: expression {expression:?} must select {expected:?}, got {body}"
        );
    }
}

/// An `Expression` that is no filter on the addressed table's partitions is refused with
/// the listing's own input error, and nothing is served.
#[tokio::test]
async fn an_expression_outside_the_filter_language_is_refused() {
    let router = emulator();
    seed_typed_table(&router).await;
    seed_table(&router, "filters", "flagged", &[("flag", "boolean")]).await;
    create_partition(&router, "filters", "flagged", &["true"]).await;

    // Rows differ along the way an expression can fail to be a filter: a column the table
    // does not partition by, a partition key of a type an expression cannot compare, a
    // literal that is no value of the key's type, and text that is no expression at all.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a column that is no partition key",
            "events",
            "country = 'us'",
            "Expression references country, which is not a partition key of the table",
        ),
        (
            "a partition key of a type an expression cannot compare",
            "flagged",
            "flag = 'true'",
            "Expression filters partition key flag of type boolean; the supported \
             partition key types are string, date, timestamp, int, bigint, long, \
             tinyint, smallint, decimal",
        ),
        (
            "a word compared against a numeric key",
            "events",
            "year = 'abc'",
            "Expression compares partition key year against a value that is not of its \
             type int",
        ),
        (
            "NULL compared against a key",
            "events",
            "year = NULL",
            "Expression compares partition key year against a value that is not of its \
             type int",
        ),
        (
            "a value that is no instant against a date key",
            "events",
            "dt >= 'yesterday'",
            "Expression compares partition key dt against a value that is not of its \
             type date",
        ),
        (
            "components that name no day against a date key",
            "events",
            "dt = '2024-13-45'",
            "Expression compares partition key dt against a value that is not of its \
             type date",
        ),
        (
            "a time of day no clock shows against a date key",
            "events",
            "dt > '2024-01-05 25:00:00'",
            "Expression compares partition key dt against a value that is not of its \
             type date",
        ),
        (
            "text that is no expression",
            "events",
            "this is not a filter(",
            "Expression this is not a filter( is not a filter on the table's partitions; \
             an expression uses SQL syntax similar to the SQL WHERE filter clause, built \
             from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and \
             IS NULL",
        ),
        (
            "a column name on its own",
            "events",
            "region",
            "Expression region is not a filter on the table's partitions; an expression \
             uses SQL syntax similar to the SQL WHERE filter clause, built from the \
             operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and IS NULL",
        ),
        (
            "text past the expression",
            "events",
            "region = 'us-east-1' leftover",
            "Expression region = 'us-east-1' leftover is not a filter on the table's \
             partitions; an expression uses SQL syntax similar to the SQL WHERE filter \
             clause, built from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, \
             LIKE, NOT and IS NULL",
        ),
        (
            "an operator the member does not document",
            "events",
            "year + 1 = 10",
            "Expression year + 1 = 10 is not a filter on the table's partitions; an \
             expression uses SQL syntax similar to the SQL WHERE filter clause, built \
             from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and \
             IS NULL",
        ),
    ];

    for (case, table, expression, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "GetPartitions",
            &json!({
                "DatabaseName": "filters",
                "TableName": table,
                "Expression": expression
            }),
            "InvalidInputException",
            expected_message,
        )
        .await;
    }
}

/// `MaxResults` and `NextToken` page the selection: every page is at most the size asked
/// for, the pages follow the catalog's order with nothing repeated and nothing dropped,
/// and the last page carries no token.
#[tokio::test]
async fn pages_walk_the_whole_selection_once() {
    let router = emulator();
    seed_table(&router, "paging", "events", &[("dt", "string")]).await;
    let values: Vec<Vec<&str>> = vec![
        vec!["2024-01-01"],
        vec!["2024-01-02"],
        vec!["2024-01-03"],
        vec!["2024-01-04"],
        vec!["2024-01-05"],
    ];
    for row in &values {
        create_partition(&router, "paging", "events", row).await;
    }
    let whole = in_catalog_order(&values);

    // Rows span the page sizes a request can ask for: the smallest the model admits, ones
    // that divide the selection and ones that do not, the count itself, more than the
    // count, and the largest page the model describes.
    for page_size in [1_i64, 2, 3, 5, 7, 1_000] {
        let mut seen: Vec<Vec<String>> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({
                "DatabaseName": "paging",
                "TableName": "events",
                "MaxResults": page_size
            });
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let body = list(&router, &request).await;
            let page = listed_values(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a page size fits a usize"),
                "case page size {page_size}: a page must hold at most what was asked for, \
                 got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case page size {page_size}: paging must end, got {pages} pages"
            );
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, whole,
            "case page size {page_size}: the pages together must be the whole selection \
             in catalog order"
        );
    }

    // A token this listing never wrote names no place in the listing.
    assert_refusal(
        &router,
        "a continuation token the listing did not write",
        "GetPartitions",
        &json!({
            "DatabaseName": "paging",
            "TableName": "events",
            "NextToken": "not-a-token"
        }),
        "InvalidInputException",
        "The parameter value contains one or more characters that are not valid. \
         Parameter Name: NextToken",
    )
    .await;
}

/// A `Segment` names a non-overlapping region of the table's partitions: the segments of
/// one scan are disjoint and together they are the whole selection, whatever the scan is
/// split into.
#[tokio::test]
async fn the_segments_of_a_scan_cover_the_table_exactly_once() {
    let router = emulator();
    seed_table(&router, "scanning", "events", &[("dt", "string")]).await;
    let values: Vec<Vec<&str>> = (1..=7)
        .map(|day| match day {
            1 => vec!["2024-01-01"],
            2 => vec!["2024-01-02"],
            3 => vec!["2024-01-03"],
            4 => vec!["2024-01-04"],
            5 => vec!["2024-01-05"],
            6 => vec!["2024-01-06"],
            _ => vec!["2024-01-07"],
        })
        .collect();
    for row in &values {
        create_partition(&router, "scanning", "events", row).await;
    }
    let whole = in_catalog_order(&values);

    // Rows span the segment counts the member admits: one segment, counts below and above
    // the partition count, and the published maximum.
    for total_segments in [1_i64, 2, 3, MAX_TOTAL_SEGMENTS] {
        let mut scanned: Vec<Vec<String>> = Vec::new();
        let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
        for segment_number in 0..total_segments {
            let body = list(
                &router,
                &json!({
                    "DatabaseName": "scanning",
                    "TableName": "events",
                    "Segment": {
                        "SegmentNumber": segment_number,
                        "TotalSegments": total_segments
                    }
                }),
            )
            .await;
            for row in listed_values(&body) {
                assert!(
                    seen.insert(row.clone()),
                    "case {total_segments} segments: partition {row:?} was served by two \
                     segments"
                );
                scanned.push(row);
            }
        }
        assert_eq!(
            scanned, whole,
            "case {total_segments} segments: the segments together must be the whole \
             selection in catalog order"
        );
    }

    // A segment outside the scan it names: the zero-based index has to be below the count,
    // so the count itself and anything above it name no region.
    for segment_number in [MAX_TOTAL_SEGMENTS, MAX_TOTAL_SEGMENTS + 5] {
        assert_refusal(
            &router,
            "a segment number at or past the segment count",
            "GetPartitions",
            &json!({
                "DatabaseName": "scanning",
                "TableName": "events",
                "Segment": {
                    "SegmentNumber": segment_number,
                    "TotalSegments": MAX_TOTAL_SEGMENTS
                }
            }),
            "InvalidInputException",
            &format!(
                "SegmentNumber {segment_number} is not a segment of a scan of 10 \
                 segments; SegmentNumber is zero-based and ranges from 0 through 9"
            ),
        )
        .await;
    }

    // One past the member's published maximum is refused as the constraint violation it is,
    // before any partition is selected.
    let (status, body) = call(
        &router,
        "GetPartitions",
        &json!({
            "DatabaseName": "scanning",
            "TableName": "events",
            "Segment": { "SegmentNumber": 0, "TotalSegments": MAX_TOTAL_SEGMENTS + 1 }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case TotalSegments past its maximum: expected 400, got {status} with {body}"
    );
    assert!(
        body.get("Partitions").is_none(),
        "case TotalSegments past its maximum: nothing may be served, got {body}"
    );
}

/// `ExcludeColumnSchema` leaves out the column schema of each reported partition and
/// nothing else.
#[tokio::test]
async fn excluding_the_column_schema_drops_only_the_column_list() {
    let router = emulator();
    seed_table(&router, "projection", "events", &[("dt", "string")]).await;
    create_partition(&router, "projection", "events", &["2024-01-01"]).await;

    // Rows: the member absent, false and true — the two values it admits plus its absence.
    let cases: Vec<(&str, Option<bool>, bool)> = vec![
        ("absent", None, true),
        ("false", Some(false), true),
        ("true", Some(true), false),
    ];
    for (case, exclude, columns_expected) in cases {
        let mut request = json!({ "DatabaseName": "projection", "TableName": "events" });
        if let Some(exclude) = exclude {
            request["ExcludeColumnSchema"] = json!(exclude);
        }
        let body = list(&router, &request).await;
        let partition = &body["Partitions"][0];
        assert_eq!(
            partition["StorageDescriptor"]["Columns"].is_array(),
            columns_expected,
            "case ExcludeColumnSchema {case:?}: expected the column schema \
             {}, got {body}",
            if columns_expected { "reported" } else { "left out" }
        );
        assert_eq!(
            partition["StorageDescriptor"]["Location"].as_str(),
            Some("s3://lake/events/2024-01-01"),
            "case ExcludeColumnSchema {case:?}: every other member stays, got {body}"
        );
    }
}

/// A listing of a database or a table the catalog does not hold is refused as the missing
/// entity it is, with the wording that miss answers everywhere.
#[tokio::test]
async fn listing_what_the_catalog_does_not_hold_is_refused() {
    let router = emulator();
    seed_table(&router, "present", "events", &[("dt", "string")]).await;

    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "a database the catalog does not hold",
            "absent",
            "events",
            "Database absent not found.".to_string(),
        ),
        (
            "a table the database does not hold",
            "present",
            "missing",
            "Table missing not found.".to_string(),
        ),
    ];
    for (case, database, table, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "GetPartitions",
            &json!({ "DatabaseName": database, "TableName": table }),
            "EntityNotFoundException",
            &expected_message,
        )
        .await;
    }
}

/// A listing that asks for a Lake Formation audit record, or for a transactional or
/// point-in-time read, is answered "not written yet" rather than served from the latest
/// state — and the pair the model forbids is a client error.
#[tokio::test]
async fn a_listing_asking_for_what_is_not_kept_is_not_served_from_the_latest_state() {
    let router = emulator();
    seed_table(&router, "unkept", "events", &[("dt", "string")]).await;
    create_partition(&router, "unkept", "events", &["2024-01-01"]).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "an audit context",
            json!({
                "DatabaseName": "unkept",
                "TableName": "events",
                "AuditContext": { "AdditionalAuditContext": "query-1" }
            }),
        ),
        (
            "a transaction id",
            json!({
                "DatabaseName": "unkept",
                "TableName": "events",
                "TransactionId": "txn-1"
            }),
        ),
        (
            "a point in time",
            json!({
                "DatabaseName": "unkept",
                "TableName": "events",
                "QueryAsOfTime": 1_700_000_000
            }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = call(&router, "GetPartitions", &request).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501 rather than a page of the latest state, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("NotImplemented"),
            "case {case:?}: expected NotImplemented, got {body}"
        );
    }

    assert_refusal(
        &router,
        "a point in time together with a transaction",
        "GetPartitions",
        &json!({
            "DatabaseName": "unkept",
            "TableName": "events",
            "TransactionId": "txn-1",
            "QueryAsOfTime": 1_700_000_000
        }),
        "InvalidInputException",
        "QueryAsOfTime cannot be specified along with TransactionId",
    )
    .await;
}
