// SPDX-License-Identifier: CC0-1.0

//! `Scan` reports the items a table HOLDS — "accessing every item in a table" —
//! never a canned page.
//!
//! The rule these suites pin: a `Scan` of a table evaluates the items an earlier
//! request wrote, reports as many of them as the request's `Limit` and the service's
//! 1 MB dataset budget allow, reports `Count` equal to `ScannedCount` when no filter
//! is applied, and reports a `LastEvaluatedKey` exactly when items were left to
//! evaluate — so paging from that key, or scanning each `Segment` of a parallel scan,
//! covers every stored item exactly once.
//!
//! Each suite checks that over a SET of inputs rather than one recorded request:
//!
//! * `a_scan_reports_every_item_the_table_holds` — a table of key schemas (the
//!   partition-key types, with and without a sort key) and item counts (none, one,
//!   several), each scanned after its items were written and again after one was
//!   deleted.
//! * `limit_and_exclusive_start_key_page_the_table_exactly_once` — the paging
//!   invariant over every `Limit` from 1 to past the item count.
//! * `the_segments_of_a_parallel_scan_cover_the_table_exactly_once` — the same
//!   invariant over `TotalSegments` from 1 to past the item count, with and without
//!   a `Limit` paging each segment.
//! * `select_count_reports_the_count_without_the_items` — what each `Select` the
//!   table form accepts returns.
//! * `consumed_capacity_is_reported_only_when_asked_for` — the `ReturnConsumedCapacity`
//!   values and the read-consistency factor.
//! * `a_scan_request_the_service_refuses` — the refusals: the all-or-nothing
//!   `Segment`/`TotalSegments` pair, a key that is not this table's, a table that is
//!   not there, an index name it does not declare, and the member combinations that
//!   cannot hold together.

use serde_json::{json, Map, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

use smithy_server_runtime::MultiServiceRouter;

/// One table shape to scan: the case, its key schema as
/// `(attribute, key type, attribute type)` rows, and the items written into it.
struct TableCase {
    case: &'static str,
    name: String,
    keys: Vec<(&'static str, &'static str, &'static str)>,
    items: Vec<Value>,
}

/// The table shapes every set-level assertion below runs over. The rows differ along
/// what the rule generalises over: the type of the partition key (`S`, `N`, `B` — the
/// three types `KeySchemaElement` can name), whether the table declares a sort key,
/// and how many items the table holds (none, one, several).
///
/// `suite` names the test that seeds them, so two suites never address one table
/// name: a DynamoDB table name is unique within a region, and each test here builds
/// its own emulator.
fn table_cases(suite: &str) -> Vec<TableCase> {
    vec![
        TableCase {
            case: "a string partition key and no sort key",
            name: format!("{suite}Artists"),
            keys: vec![("Artist", "HASH", "S")],
            items: vec![
                json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "3"}}),
                json!({"Artist": {"S": "Beta Quartet"}, "Live": {"BOOL": true}}),
                json!({"Artist": {"S": "Ωmega"}, "Tags": {"SS": ["jazz", "live"]}}),
            ],
        },
        TableCase {
            case: "a string partition key and a number sort key",
            name: format!("{suite}Tracks"),
            keys: vec![("Artist", "HASH", "S"), ("Track", "RANGE", "N")],
            items: vec![
                json!({"Artist": {"S": "Acme Band"}, "Track": {"N": "1"}, "Title": {"S": "One"}}),
                json!({"Artist": {"S": "Acme Band"}, "Track": {"N": "2"}, "Title": {"S": "Two"}}),
                json!({"Artist": {"S": "Acme Band"}, "Track": {"N": "10"}, "Title": {"S": "Ten"}}),
                json!({"Artist": {"S": "Beta Quartet"}, "Track": {"N": "1"},
                       "Nested": {"M": {"Takes": {"L": [{"N": "1"}, {"N": "2"}]}}}}),
            ],
        },
        TableCase {
            case: "a number partition key and a string sort key",
            name: format!("{suite}Orders"),
            keys: vec![("Id", "HASH", "N"), ("Line", "RANGE", "S")],
            items: vec![
                json!({"Id": {"N": "1"}, "Line": {"S": "a"}, "Qty": {"N": "7"}}),
                json!({"Id": {"N": "2"}, "Line": {"S": "b"}, "Qty": {"N": "8"}}),
            ],
        },
        TableCase {
            case: "a binary partition key",
            name: format!("{suite}Blobs"),
            keys: vec![("Blob", "HASH", "B")],
            items: vec![
                json!({"Blob": {"B": "aGVsbG8="}, "Kind": {"S": "greeting"}}),
                json!({"Blob": {"B": "d29ybGQ="}, "Kind": {"S": "place"}}),
                json!({"Blob": {"B": "Yg=="}, "Kind": {"NULL": true}}),
            ],
        },
        TableCase {
            case: "a table that holds exactly one item",
            name: format!("{suite}Single"),
            keys: vec![("Artist", "HASH", "S")],
            items: vec![json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "1"}})],
        },
        TableCase {
            case: "a table that holds no items at all",
            name: format!("{suite}Empty"),
            keys: vec![("Artist", "HASH", "S")],
            items: vec![],
        },
    ]
}

/// The `AttributeValue` members that hold a SET, whose element order carries no
/// information: "DynamoDB does not preserve the order of set elements" (the
/// Developer Guide's "Data types" entry for sets), so two renderings of one set must
/// be equal whatever order its members arrive in.
const SET_MEMBERS: [&str; 3] = ["SS", "NS", "BS"];

/// One item rendered so that two renderings are equal exactly when the two items
/// carry the same attributes.
///
/// A JSON object's member ORDER is not data — a stored item is an attribute MAP, and
/// the order its members are serialised in is the store's business — so the rendering
/// sorts every object's members, and the members of a set attribute with them. A
/// list (`L`) keeps its order, because a list IS ordered.
pub(crate) fn canonical(value: &Value) -> String {
    match value {
        Value::Object(members) => {
            let mut rendered: Vec<String> = members
                .iter()
                .map(|(name, member)| {
                    let member = match member {
                        Value::Array(elements) if SET_MEMBERS.contains(&name.as_str()) => {
                            let mut rendered: Vec<String> =
                                elements.iter().map(canonical).collect();
                            rendered.sort();
                            format!("[{}]", rendered.join(","))
                        }
                        member => canonical(member),
                    };
                    format!("{name:?}:{member}")
                })
                .collect();
            rendered.sort();
            format!("{{{}}}", rendered.join(","))
        }
        Value::Array(elements) => {
            let rendered: Vec<String> = elements.iter().map(canonical).collect();
            format!("[{}]", rendered.join(","))
        }
        scalar => scalar.to_string(),
    }
}

/// The items a response reports, rendered canonically and sorted — a `Scan` returns a
/// SET of items, and DynamoDB promises no order over it, so every comparison here is
/// between sets.
pub(crate) fn reported_items(response: &Value) -> Vec<String> {
    let mut rendered: Vec<String> = response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(canonical)
        .collect();
    rendered.sort();
    rendered
}

/// The same rendering for the items a test wrote.
pub(crate) fn rendered(items: &[Value]) -> Vec<String> {
    let mut rendered: Vec<String> = items.iter().map(canonical).collect();
    rendered.sort();
    rendered
}

/// The primary key of one item, by the key attribute names of its table — the shape a
/// `LastEvaluatedKey` carries and an `ExclusiveStartKey` takes.
fn primary_key(item: &Value, key_names: &[&str]) -> Value {
    let mut key = Map::new();
    for name in key_names {
        key.insert((*name).to_string(), item[*name].clone());
    }
    Value::Object(key)
}

/// The `HASH`/`RANGE` attribute names of a key schema, in schema order.
fn key_names(keys: &[(&'static str, &'static str, &'static str)]) -> Vec<&'static str> {
    keys.iter().map(|(attribute, _, _)| *attribute).collect()
}

/// Write every item of a case into its table, which is created first; the table's
/// own description, as `DescribeTable` reported it once the table went `ACTIVE`.
async fn seed(router: &MultiServiceRouter, table: &TableCase) -> Value {
    let described = create_active_table(router, &table.name, &table.keys).await;
    for item in &table.items {
        ok(
            router,
            "PutItem",
            json!({"TableName": &table.name, "Item": item}),
        )
        .await;
    }
    described
}

/// The `LastEvaluatedKey` a response carries, or `None` when it reports none — an
/// absent key is the last page.
pub(crate) fn last_evaluated_key(response: &Value) -> Option<Value> {
    response
        .get("LastEvaluatedKey")
        .filter(|key| !key.is_null())
        .cloned()
}

#[tokio::test]
async fn a_scan_reports_every_item_the_table_holds() {
    let router = emulator();
    for table in table_cases("ScanAll") {
        seed(&router, &table).await;
        let expected = rendered(&table.items);
        let count = json!(table.items.len());

        let scanned = ok(&router, "Scan", json!({"TableName": &table.name})).await;
        assert_eq!(
            reported_items(&scanned),
            expected,
            "case {:?}: a scan reports every item written, got {scanned}",
            table.case
        );
        assert_eq!(
            scanned["Count"], count,
            "case {:?}: Count is the number of items reported, got {scanned}",
            table.case
        );
        assert_eq!(
            scanned["ScannedCount"], scanned["Count"],
            "case {:?}: with no filter, ScannedCount equals Count, got {scanned}",
            table.case
        );
        assert_eq!(
            last_evaluated_key(&scanned),
            None,
            "case {:?}: a scan that reached the end of the table reports no \
             LastEvaluatedKey, got {scanned}",
            table.case
        );

        // The scan reads STATE: once an item is deleted, the next scan no longer
        // reports it, and the table's ARN addresses the same items as its name.
        let Some(removed) = table.items.first() else {
            continue;
        };
        let names = key_names(&table.keys);
        let key = primary_key(removed, &names);
        ok(
            &router,
            "DeleteItem",
            json!({"TableName": &table.name, "Key": key}),
        )
        .await;
        let after = ok(&router, "Scan", json!({"TableName": &table.name})).await;
        assert_eq!(
            reported_items(&after),
            rendered(&table.items[1..]),
            "case {:?}: a deleted item is gone from the next scan, got {after}",
            table.case
        );
        assert_eq!(
            after["Count"],
            json!(table.items.len() - 1),
            "case {:?}: Count follows the stored items, got {after}",
            table.case
        );
    }
}

#[tokio::test]
async fn limit_and_exclusive_start_key_page_the_table_exactly_once() {
    let router = emulator();
    for table in table_cases("ScanPaging") {
        seed(&router, &table).await;
        let names = key_names(&table.keys);
        let expected = rendered(&table.items);
        let total = table.items.len();

        // Every page size from one item to past the whole table: the pages must
        // partition the table, and their number is the number of pages that size
        // needs.
        for limit in [1usize, 2, 3, 4, total.max(1), total + 4] {
            let mut seen: Vec<String> = Vec::new();
            let mut start: Option<Value> = None;
            let mut pages = 0usize;
            loop {
                let mut request = json!({"TableName": &table.name, "Limit": limit});
                if let Some(key) = start.clone() {
                    request["ExclusiveStartKey"] = key;
                }
                let page = ok(&router, "Scan", request).await;
                pages += 1;
                assert!(
                    pages <= total + 1,
                    "case {:?} limit {limit}: paging must terminate, got page {pages} \
                     for {total} item(s): {page}",
                    table.case
                );
                let returned = page["Items"].as_array().cloned().unwrap_or_default();
                assert!(
                    returned.len() <= limit,
                    "case {:?} limit {limit}: a page evaluates at most Limit items, got {}",
                    table.case,
                    returned.len()
                );
                assert_eq!(
                    page["Count"],
                    json!(returned.len()),
                    "case {:?} limit {limit}: Count is this page's item count, got {page}",
                    table.case
                );
                assert_eq!(
                    page["ScannedCount"], page["Count"],
                    "case {:?} limit {limit}: with no filter ScannedCount equals Count, \
                     got {page}",
                    table.case
                );
                seen.extend(returned.iter().map(canonical));
                match last_evaluated_key(&page) {
                    Some(key) => {
                        assert!(
                            returned
                                .iter()
                                .any(|item| primary_key(item, &names) == key),
                            "case {:?} limit {limit}: LastEvaluatedKey is the key of the \
                             item this page stopped on, got {key} for {page}",
                            table.case
                        );
                        start = Some(key);
                    }
                    None => break,
                }
            }
            seen.sort();
            assert_eq!(
                seen, expected,
                "case {:?} limit {limit}: the pages together report every item exactly \
                 once, got {seen:?}",
                table.case
            );
            let expected_pages = match total {
                0 => 1,
                // The last page a walk takes is one whose own Limit was reached, so a
                // table whose items divide evenly into pages takes one page more.
                total if total % limit == 0 => total / limit + 1,
                total => total.div_ceil(limit),
            };
            // A page reports a `LastEvaluatedKey` when it stopped with items still to
            // evaluate AND when it stopped because it reached the caller's `Limit`, so
            // `n` items take `ceil(n / limit)` pages, plus one more when the limit was
            // reached by the last item — "If LastEvaluatedKey is not empty, it does not
            // necessarily mean that there is more data in the result set", and the
            // sibling `Query` reader makes the same choice.
            assert_eq!(
                pages, expected_pages,
                "case {:?} limit {limit}: {total} item(s) take {expected_pages} page(s), \
                 got {pages}",
                table.case
            );
        }
    }
}

#[tokio::test]
async fn the_segments_of_a_parallel_scan_cover_the_table_exactly_once() {
    let router = emulator();
    for table in table_cases("ScanSegments") {
        seed(&router, &table).await;
        let names = key_names(&table.keys);
        let expected = rendered(&table.items);
        let total_items = table.items.len();

        // One segment (the sequential scan), fewer segments than items, as many as
        // there are items, and more segments than items — the last of which leaves a
        // segment with nothing to report.
        for segments in [1usize, 2, 3, total_items.max(1), total_items + 2] {
            // Each segment read whole, and each segment paged one item at a time:
            // a segment's share of the table must not depend on how it is paged.
            for limit in [None, Some(1usize)] {
                let mut seen: Vec<String> = Vec::new();
                let mut scanned_total = 0usize;
                for segment in 0..segments {
                    let mut start: Option<Value> = None;
                    let mut pages = 0usize;
                    loop {
                        let mut request = json!({
                            "TableName": &table.name,
                            "Segment": segment,
                            "TotalSegments": segments,
                        });
                        if let Some(limit) = limit {
                            request["Limit"] = json!(limit);
                        }
                        if let Some(key) = start.clone() {
                            request["ExclusiveStartKey"] = key;
                        }
                        let page = ok(&router, "Scan", request).await;
                        pages += 1;
                        assert!(
                            pages <= total_items + 1,
                            "case {:?} segment {segment}/{segments} limit {limit:?}: paging \
                             must terminate, got page {pages}: {page}",
                            table.case
                        );
                        let returned = page["Items"].as_array().cloned().unwrap_or_default();
                        assert_eq!(
                            page["ScannedCount"],
                            json!(returned.len()),
                            "case {:?} segment {segment}/{segments} limit {limit:?}: \
                             ScannedCount is what this segment evaluated, got {page}",
                            table.case
                        );
                        scanned_total += returned.len();
                        seen.extend(returned.iter().map(canonical));
                        match last_evaluated_key(&page) {
                            Some(key) => {
                                assert!(
                                    returned
                                        .iter()
                                        .any(|item| primary_key(item, &names) == key),
                                    "case {:?} segment {segment}/{segments}: \
                                     LastEvaluatedKey is the key of an item this page \
                                     reported, got {key} for {page}",
                                    table.case
                                );
                                start = Some(key);
                            }
                            None => break,
                        }
                    }
                }
                seen.sort();
                assert_eq!(
                    seen, expected,
                    "case {:?} {segments} segment(s) limit {limit:?}: the segments together \
                     report every item exactly once, got {seen:?}",
                    table.case
                );
                assert_eq!(
                    scanned_total, total_items,
                    "case {:?} {segments} segment(s) limit {limit:?}: the segments together \
                     evaluate every item exactly once, got {scanned_total}",
                    table.case
                );
            }
        }
    }
}

#[tokio::test]
async fn select_count_reports_the_count_without_the_items() {
    let router = emulator();
    let table = TableCase {
        case: "Select over a table scan",
        name: "SelectScan".to_string(),
        keys: vec![("Artist", "HASH", "S")],
        items: vec![
            json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "3"}}),
            json!({"Artist": {"S": "Beta Quartet"}, "Albums": {"N": "4"}}),
        ],
    };
    seed(&router, &table).await;
    let expected = rendered(&table.items);

    // Each `Select` the table form of a scan accepts, and the request that names
    // none: "ALL_ATTRIBUTES - Returns all of the item attributes" (the default for a
    // table), "COUNT - Returns the number of matching items, rather than the matching
    // items themselves" (`Scan$Select`).
    let cases: [(&str, Option<&str>, bool); 3] = [
        ("no Select at all", None, true),
        ("ALL_ATTRIBUTES", Some("ALL_ATTRIBUTES"), true),
        ("COUNT", Some("COUNT"), false),
    ];
    for (case, select, carries_items) in cases {
        let mut request = json!({"TableName": &table.name});
        if let Some(select) = select {
            request["Select"] = json!(select);
        }
        let scanned = ok(&router, "Scan", request).await;
        assert_eq!(
            scanned["Count"],
            json!(table.items.len()),
            "case {case:?}: every Select reports the same Count, got {scanned}"
        );
        assert_eq!(
            scanned["ScannedCount"], scanned["Count"],
            "case {case:?}: with no filter ScannedCount equals Count, got {scanned}"
        );
        if carries_items {
            assert_eq!(
                reported_items(&scanned),
                expected,
                "case {case:?}: the items are reported, got {scanned}"
            );
        } else {
            assert!(
                scanned.get("Items").is_none_or(Value::is_null),
                "case {case:?}: COUNT returns the count rather than the items, got {scanned}"
            );
        }
    }
}

#[tokio::test]
async fn consumed_capacity_is_reported_only_when_asked_for() {
    let router = emulator();
    let table = TableCase {
        case: "capacity over a table scan",
        name: "CapacityScan".to_string(),
        keys: vec![("Artist", "HASH", "S")],
        items: vec![
            json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "3"}}),
            json!({"Artist": {"S": "Beta Quartet"}, "Albums": {"N": "4"}}),
        ],
    };
    seed(&router, &table).await;

    // The page is far inside one 4 KB read unit, which an eventually consistent read
    // takes half of: "one read capacity unit represents one strongly consistent read
    // per second ... for an item up to 4 KB" and "two eventually consistent reads per
    // second" (the Developer Guide's "Read/write capacity mode"). Each row: the
    // `ReturnConsumedCapacity`, the `ConsistentRead`, and the units the response must
    // report — `None` where it must report no `ConsumedCapacity` at all.
    let cases: [(&str, Option<&str>, bool, Option<f64>); 5] = [
        ("no ReturnConsumedCapacity", None, false, None),
        ("NONE", Some("NONE"), false, None),
        ("TOTAL, eventually consistent", Some("TOTAL"), false, Some(0.5)),
        ("TOTAL, strongly consistent", Some("TOTAL"), true, Some(1.0)),
        ("INDEXES, strongly consistent", Some("INDEXES"), true, Some(1.0)),
    ];
    for (case, requested, consistent, expected_units) in cases {
        let mut request = json!({"TableName": &table.name, "ConsistentRead": consistent});
        if let Some(requested) = requested {
            request["ReturnConsumedCapacity"] = json!(requested);
        }
        let scanned = ok(&router, "Scan", request).await;
        match expected_units {
            None => assert!(
                scanned.get("ConsumedCapacity").is_none_or(Value::is_null),
                "case {case:?}: no capacity is reported unless it was asked for, got {scanned}"
            ),
            Some(units) => {
                assert_eq!(
                    scanned["ConsumedCapacity"]["CapacityUnits"],
                    json!(units),
                    "case {case:?}: expected {units} capacity unit(s), got {scanned}"
                );
                assert_eq!(
                    scanned["ConsumedCapacity"]["TableName"],
                    json!(table.name),
                    "case {case:?}: the capacity names the table that served the scan, \
                     got {scanned}"
                );
            }
        }
        if requested == Some("INDEXES") {
            assert_eq!(
                scanned["ConsumedCapacity"]["Table"]["CapacityUnits"],
                json!(1.0),
                "case {case:?}: INDEXES breaks the table's own capacity out, got {scanned}"
            );
        }
    }
}

#[tokio::test]
async fn a_scan_request_the_service_refuses() {
    let router = emulator();
    let table = TableCase {
        case: "refusals over a table scan",
        name: "RefusedScan".to_string(),
        keys: vec![("Artist", "HASH", "S")],
        items: vec![json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "3"}})],
    };
    seed(&router, &table).await;

    // Each row: the case, the request, the HTTP status and wire error code it must be
    // answered with, and a phrase the message must carry (empty where the wording is
    // not this test's business). The rows span the refusal classes: the
    // all-or-nothing parallel-scan pair and its range, a key that is not this
    // table's, a table that is not there, an index name the table does not declare,
    // and each pair of members that cannot hold together.
    let cases: Vec<(&str, Value, u16, &str, &str)> = vec![
        (
            "Segment without TotalSegments",
            json!({"TableName": &table.name, "Segment": 0}),
            400,
            "ValidationException",
            "TotalSegments",
        ),
        (
            "TotalSegments without Segment",
            json!({"TableName": &table.name, "TotalSegments": 4}),
            400,
            "ValidationException",
            "Segment",
        ),
        (
            "Segment equal to TotalSegments",
            json!({"TableName": &table.name, "Segment": 4, "TotalSegments": 4}),
            400,
            "ValidationException",
            "zero-based",
        ),
        (
            "Segment past TotalSegments",
            json!({"TableName": &table.name, "Segment": 9, "TotalSegments": 4}),
            400,
            "ValidationException",
            "zero-based",
        ),
        (
            "an ExclusiveStartKey that is not this table's key",
            json!({"TableName": &table.name, "ExclusiveStartKey": {"Nope": {"S": "x"}}}),
            400,
            "ValidationException",
            "The provided starting key is invalid",
        ),
        (
            "an ExclusiveStartKey whose key attribute has the wrong type",
            json!({"TableName": &table.name, "ExclusiveStartKey": {"Artist": {"N": "1"}}}),
            400,
            "ValidationException",
            "The provided starting key is invalid",
        ),
        (
            "a table that was never created",
            json!({"TableName": "NoSuchScanTable"}),
            400,
            "ResourceNotFoundException",
            "",
        ),
        (
            "ExpressionAttributeValues with no expression to substitute into",
            json!({"TableName": &table.name,
                   "ExpressionAttributeValues": {":a": {"S": "Acme Band"}}}),
            400,
            "ValidationException",
            "ExpressionAttributeValues",
        ),
        (
            "ExpressionAttributeNames with no expression to substitute into",
            json!({"TableName": &table.name, "ExpressionAttributeNames": {"#a": "Artist"}}),
            400,
            "ValidationException",
            "ExpressionAttributeNames",
        ),
        (
            "an IndexName this table declares no index for",
            json!({"TableName": &table.name, "IndexName": "ByAlbums"}),
            400,
            "ValidationException",
            "The table does not have the specified index: ByAlbums",
        ),
        (
            "a FilterExpression beside the legacy ScanFilter it replaces",
            json!({"TableName": &table.name, "FilterExpression": "Albums > :a",
                   "ExpressionAttributeValues": {":a": {"N": "1"}},
                   "ScanFilter": {"Albums": {"ComparisonOperator": "EQ",
                                             "AttributeValueList": [{"N": "3"}]}}}),
            400,
            "ValidationException",
            "Can not use both expression and non-expression parameters",
        ),
        (
            "a ScanFilter entry naming an operator outside the modeled set",
            json!({"TableName": &table.name,
                   "ScanFilter": {"Albums": {"ComparisonOperator": "SOUNDS_LIKE",
                                             "AttributeValueList": [{"N": "3"}]}}}),
            400,
            "ValidationException",
            "Member must satisfy enum value set",
        ),
        (
            "a ScanFilter entry given the wrong number of operands",
            json!({"TableName": &table.name,
                   "ScanFilter": {"Albums": {"ComparisonOperator": "BETWEEN",
                                             "AttributeValueList": [{"N": "3"}]}}}),
            400,
            "ValidationException",
            "Invalid number of argument(s) for the BETWEEN ComparisonOperator",
        ),
        (
            "the legacy ConditionalOperator with no ScanFilter to combine",
            json!({"TableName": &table.name, "ConditionalOperator": "AND"}),
            400,
            "ValidationException",
            "ConditionalOperator cannot be used without Filter or Expected",
        ),
        (
            "a ProjectionExpression beside the legacy AttributesToGet it replaces",
            json!({"TableName": &table.name, "ProjectionExpression": "Albums",
                   "AttributesToGet": ["Albums"]}),
            400,
            "ValidationException",
            "Can not use both expression and non-expression parameters",
        ),
        (
            "a ProjectionExpression beside a Select other than SPECIFIC_ATTRIBUTES",
            json!({"TableName": &table.name, "ProjectionExpression": "Albums",
                   "Select": "ALL_ATTRIBUTES"}),
            400,
            "ValidationException",
            "Cannot specify the ProjectionExpression when choosing to get ALL_ATTRIBUTES",
        ),
        (
            "Select SPECIFIC_ATTRIBUTES with nothing listed to return",
            json!({"TableName": &table.name, "Select": "SPECIFIC_ATTRIBUTES"}),
            400,
            "ValidationException",
            "SPECIFIC_ATTRIBUTES",
        ),
        (
            "a Select only an index scan accepts",
            json!({"TableName": &table.name, "Select": "ALL_PROJECTED_ATTRIBUTES"}),
            400,
            "ValidationException",
            "can be used only when Querying using an IndexName",
        ),
    ];

    for (case, request, status, code, phrase) in cases {
        let (actual, body) = call(&router, "Scan", request.clone()).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code}, got {body}"
        );
        let stated = message(&body);
        assert!(
            stated.contains(phrase),
            "case {case:?}: the message must state {phrase:?}, got {stated:?}"
        );
    }

    // Nothing above wrote to the table: a refused scan leaves the stored items
    // exactly as they were.
    let scanned = ok(&router, "Scan", json!({"TableName": &table.name})).await;
    assert_eq!(
        reported_items(&scanned),
        rendered(&table.items),
        "case refusals: a refused scan changes nothing a later scan reports, got {scanned}"
    );
}

#[tokio::test]
async fn a_resource_policy_that_denies_the_scan_answers_the_denial() {
    let router = emulator();
    let table = TableCase {
        case: "a policy over a table scan",
        name: "PolicyScan".to_string(),
        keys: vec![("Artist", "HASH", "S")],
        items: vec![json!({"Artist": {"S": "Acme Band"}, "Albums": {"N": "3"}})],
    };
    let described = seed(&router, &table).await;
    let arn = described["Table"]["TableArn"]
        .as_str()
        .expect("the created table reports its ARN")
        .to_string();

    // Each row: the action one `Deny` statement names, and whether a scan of the
    // table is then refused. A policy denies the action the operation PERFORMS, so
    // only the scan's own action takes the items away.
    for (case, action, denied) in [
        ("the scan's own action", "dynamodb:Scan", true),
        ("a neighbouring read action", "dynamodb:Query", false),
        ("a write action", "dynamodb:PutItem", false),
    ] {
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": action,
                "Resource": &arn,
            }],
        })
        .to_string();
        ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": &arn, "Policy": policy}),
        )
        .await;
        // "PutResourcePolicy is an asynchronous operation" — the policy applies to
        // the data plane one transition span after the call.
        tokio::time::sleep(STATUS_TRANSITION + std::time::Duration::from_millis(200)).await;

        let (status, body) = call(&router, "Scan", json!({"TableName": &table.name})).await;
        if denied {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {case:?}: the scan is refused rather than served, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, reported_items(&body)),
                (200, rendered(&table.items)),
                "case {case:?}: a policy that denies another action still serves the \
                 scan, got {status} {body}"
            );
        }
    }
}
