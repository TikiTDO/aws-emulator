// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue Data Catalog's table-level column-statistics store:
//! `UpdateColumnStatisticsForTable`, `GetColumnStatisticsForTable` and
//! `DeleteColumnStatisticsForTable`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the statistics of a column are a record of the catalog, addressed
//! by catalog, database, table and column. A write persists each submitted
//! `ColumnStatistics` against that address, so a read naming that column answers with
//! exactly one entry carrying the stored `ColumnName`, `ColumnType`, `AnalyzedTime` and
//! `StatisticsData` — with the `ColumnStatisticsData` member its `Type` discriminator
//! selects — and does NOT name that column in `Errors`. The read is a partial-success
//! operation: it answers 200 whatever the catalog holds, and reports every requested column
//! the catalog holds no statistics for in `Errors`, as a `ColumnError` whose `ColumnName` is
//! the name the request asked for and whose `Error` is an `ErrorDetail` carrying the
//! service's `EntityNotFoundException` code and its own fixed wording. A delete returns a
//! column to exactly that state.
//!
//! The recorded case — one `int` column written with `LONG` figures and read back, then read
//! again once it has none — is ONE row of the tables below; the rows differ along the
//! declared column type and the `ColumnStatisticsType` its figures are reported under (every
//! value of the modelled enum), along which figures the payload carries, along the instant
//! the statistics were generated, along how many columns one call addresses, and along which
//! part of the address does not resolve.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    column_statistics_data_member_missing, column_statistics_limit_exceeded,
    COLUMN_STATISTICS_NOT_FOUND,
};
use service_com_amazonaws_glue_awsglue::state::MAX_COLUMN_STATISTICS_PER_TABLE;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, ERROR_DETAIL_TAG};

/// The `ErrorDetail$ErrorCode` the catalog reports a column it holds no statistics for
/// under. Source: the `EntityNotFoundException` this operation models ("a specified entity
/// does not exist"), `smithy_query operation=GetColumnStatisticsForTable`.
const NOT_FOUND_CODE: &str = "EntityNotFoundException";

/// The `ErrorDetail$ErrorCode` a submitted entry that states no usable figures is reported
/// under. Source: the `InvalidInputException` the write models ("the input provided was not
/// valid"), `smithy_query operation=UpdateColumnStatisticsForTable`.
const INVALID_INPUT_CODE: &str = "InvalidInputException";

/// The `ErrorDetail$ErrorCode` an entry the store has no room for is reported under: this
/// service's own name for a ceiling that was reached, the one its creates raise when a
/// container is full. Source: the `ResourceNumberLimitExceededException` shape of
/// `com.amazonaws.glue`, `smithy_query shape=ResourceNumberLimitExceededException`.
const LIMIT_EXCEEDED_CODE: &str = "ResourceNumberLimitExceededException";

/// One row of the round-trip table: a column, the type the table declares it under, and the
/// statistics a write stores for it.
struct StatisticsCase<'a> {
    case: &'a str,
    column: &'a str,
    /// `Column$Type`, the Hive type name the table declares.
    column_type: &'a str,
    /// `ColumnStatisticsData$Type`, the modelled `ColumnStatisticsType` the figures are
    /// reported under.
    statistics_type: &'a str,
    /// `ColumnStatistics$AnalyzedTime`, "the timestamp of when column statistics were
    /// generated", as epoch seconds on the wire.
    analyzed_time: f64,
    /// The `ColumnStatisticsData` member the row's `Type` selects, as the write sends it.
    figures: Value,
}

/// Every `ColumnStatisticsType` a column's figures can be reported under, one row each.
///
/// Source for the member names and the figures each member carries:
/// `smithy_query shape=ColumnStatisticsData` and the data shapes it names. The declared
/// column types are the Hive type names a Data Catalog table states.
fn statistics_cases() -> Vec<StatisticsCase<'static>> {
    vec![
        StatisticsCase {
            case: "an integer column, reported under LONG",
            column: "id",
            column_type: "int",
            statistics_type: "LONG",
            analyzed_time: 1_700_000_000.0,
            figures: json!({
                "MinimumValue": 1,
                "MaximumValue": 1000,
                "NumberOfNulls": 0,
                "NumberOfDistinctValues": 100
            }),
        },
        StatisticsCase {
            case: "a floating-point column, reported under DOUBLE",
            column: "amount",
            column_type: "double",
            statistics_type: "DOUBLE",
            analyzed_time: 1_700_000_123.0,
            figures: json!({
                "MinimumValue": -17.5,
                "MaximumValue": 4096.25,
                "NumberOfNulls": 3,
                "NumberOfDistinctValues": 57
            }),
        },
        StatisticsCase {
            case: "a character-sequence column, reported under STRING",
            column: "label",
            column_type: "string",
            statistics_type: "STRING",
            analyzed_time: 1_650_000_000.0,
            figures: json!({
                "MaximumLength": 42,
                "AverageLength": 12.5,
                "NumberOfNulls": 7,
                "NumberOfDistinctValues": 19
            }),
        },
        StatisticsCase {
            case: "a Boolean column, reported under BOOLEAN with no distinct count",
            column: "active",
            column_type: "boolean",
            statistics_type: "BOOLEAN",
            analyzed_time: 1_600_000_500.0,
            figures: json!({
                "NumberOfTrues": 11,
                "NumberOfFalses": 9,
                "NumberOfNulls": 2
            }),
        },
        StatisticsCase {
            case: "a timestamp column, reported under DATE with timestamp extremes",
            column: "observed_at",
            column_type: "timestamp",
            statistics_type: "DATE",
            analyzed_time: 1_712_345_678.0,
            figures: json!({
                "MinimumValue": 1_600_000_000,
                "MaximumValue": 1_700_000_000,
                "NumberOfNulls": 0,
                "NumberOfDistinctValues": 365
            }),
        },
        StatisticsCase {
            case: "a fixed-point column, reported under DECIMAL with unscaled extremes",
            column: "price",
            column_type: "decimal(10,2)",
            statistics_type: "DECIMAL",
            analyzed_time: 1_666_666_666.0,
            figures: json!({
                // `DecimalNumber$UnscaledValue` is a blob: base64 on the wire.
                "MinimumValue": { "UnscaledValue": "AQ==", "Scale": 2 },
                "MaximumValue": { "UnscaledValue": "JxA=", "Scale": 2 },
                "NumberOfNulls": 1,
                "NumberOfDistinctValues": 8
            }),
        },
        StatisticsCase {
            case: "a bit-sequence column, reported under BINARY with no distinct count",
            column: "payload",
            column_type: "binary",
            statistics_type: "BINARY",
            analyzed_time: 1_633_000_000.0,
            figures: json!({
                "MaximumLength": 2048,
                "AverageLength": 512.5,
                "NumberOfNulls": 4
            }),
        },
    ]
}

/// The `ColumnStatisticsData` member that carries the figures of statistics reported under
/// `statistics_type`: "only one data object should be set and indicated by the `Type`
/// attribute". Source: `smithy_query shape=ColumnStatisticsData`.
fn data_member(statistics_type: &str) -> &'static str {
    match statistics_type {
        "BOOLEAN" => "BooleanColumnStatisticsData",
        "DATE" => "DateColumnStatisticsData",
        "DECIMAL" => "DecimalColumnStatisticsData",
        "DOUBLE" => "DoubleColumnStatisticsData",
        "LONG" => "LongColumnStatisticsData",
        "STRING" => "StringColumnStatisticsData",
        "BINARY" => "BinaryColumnStatisticsData",
        other => panic!("{other} is not a modelled ColumnStatisticsType"),
    }
}

/// The `ColumnStatistics` entry a row sends to the write.
fn entry_of(row: &StatisticsCase<'_>) -> Value {
    json!({
        "ColumnName": row.column,
        "ColumnType": row.column_type,
        "AnalyzedTime": row.analyzed_time,
        "StatisticsData": {
            "Type": row.statistics_type,
            data_member(row.statistics_type): row.figures.clone()
        }
    })
}

/// Create `database` holding `table`, declaring one column per row of the round-trip table:
/// statistics are addressed THROUGH a stored table, so the table has to exist first.
async fn seed(router: &MultiServiceRouter, database: &str, table: &str) {
    let columns: Vec<Value> = statistics_cases()
        .iter()
        .map(|row| json!({ "Name": row.column, "Type": row.column_type }))
        .collect();
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "StorageDescriptor": { "Location": "s3://lake/stats/", "Columns": columns }
            }
        }),
    )
    .await;
}

/// One statistics write, answering the HTTP status and body.
async fn write_statistics(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    entries: &[Value],
) -> (u16, Value) {
    call(
        router,
        "UpdateColumnStatisticsForTable",
        &json!({
            "CatalogId": ACCOUNT_ID,
            "DatabaseName": database,
            "TableName": table,
            "ColumnStatisticsList": entries
        }),
    )
    .await
}

/// One statistics read, answering the HTTP status and body.
async fn read_statistics(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: &[&str],
) -> (u16, Value) {
    call(
        router,
        "GetColumnStatisticsForTable",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "ColumnNames": columns
        }),
    )
    .await
}

/// One statistics delete, answering the HTTP status and body.
async fn delete_statistics(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    column: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteColumnStatisticsForTable",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "ColumnName": column
        }),
    )
    .await
}

/// The `ColumnStatisticsList` of an answer, which is always present even when empty.
fn reported_statistics(body: &Value) -> &Vec<Value> {
    body.get("ColumnStatisticsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must carry a ColumnStatisticsList, got {body}"))
}

/// The `Errors` of an answer, which is always present even when empty.
fn reported_errors(body: &Value) -> &Vec<Value> {
    body.get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must carry an Errors list, got {body}"))
}

/// Assert that every member `sent` carries reappears on `got`, recursing into nested
/// structures.
///
/// Numbers are compared as numbers and an absent number counts as 0: awsJson omits a member
/// whose value equals its modelled default, so a figure sent as 0 comes back absent.
fn assert_figures_echoed(case: &str, member: &str, sent: &Value, got: Option<&Value>) {
    match sent {
        Value::Object(members) => {
            let got = got.unwrap_or_else(|| {
                panic!("case {case:?}: {member} must round-trip as a structure, got nothing")
            });
            for (name, value) in members {
                assert_figures_echoed(case, &format!("{member}.{name}"), value, got.get(name));
            }
        }
        Value::Number(_) => {
            let expected = sent.as_f64().unwrap_or_default();
            let actual = got.and_then(Value::as_f64).unwrap_or_default();
            assert!(
                (actual - expected).abs() < 1e-9,
                "case {case:?}: {member} must round-trip; expected {expected}, got {actual}"
            );
        }
        _ => assert_eq!(
            got,
            Some(sent),
            "case {case:?}: {member} must round-trip; expected {sent}, got {got:?}"
        ),
    }
}

/// Assert that `entry` is the statistics `row` wrote, member for member.
fn assert_entry_matches(case: &str, row: &StatisticsCase<'_>, entry: &Value) {
    assert_eq!(
        entry.get("ColumnName").and_then(Value::as_str),
        Some(row.column),
        "case {case:?}: the entry names the column its statistics belong to, got {entry}"
    );
    assert_eq!(
        entry.get("ColumnType").and_then(Value::as_str),
        Some(row.column_type),
        "case {case:?}: the entry reports the stored column type, got {entry}"
    );
    let analyzed = entry
        .get("AnalyzedTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        (analyzed - row.analyzed_time).abs() < 1e-6,
        "case {case:?}: expected AnalyzedTime {}, got {analyzed} in {entry}",
        row.analyzed_time
    );
    let data = entry
        .get("StatisticsData")
        .unwrap_or_else(|| panic!("case {case:?}: the entry must carry StatisticsData, got {entry}"));
    assert_eq!(
        data.get("Type").and_then(Value::as_str),
        Some(row.statistics_type),
        "case {case:?}: the data is tagged with the type it was stored under, got {data}"
    );
    let member = data_member(row.statistics_type);
    assert_figures_echoed(case, member, &row.figures, data.get(member));
    for other in [
        "BooleanColumnStatisticsData",
        "DateColumnStatisticsData",
        "DecimalColumnStatisticsData",
        "DoubleColumnStatisticsData",
        "LongColumnStatisticsData",
        "StringColumnStatisticsData",
        "BinaryColumnStatisticsData",
    ] {
        if other == member {
            continue;
        }
        assert!(
            data.get(other).is_none(),
            "case {case:?}: only the member the Type names carries figures, got {other} in {data}"
        );
    }
}

/// Assert that `entry` is the per-column failure the catalog reports for `column`.
fn assert_not_found_error(case: &str, column: &str, entry: &Value) {
    assert_eq!(
        entry.get("ColumnName").and_then(Value::as_str),
        Some(column),
        "case {case:?}: the failure names the column that failed, got {entry}"
    );
    let detail = entry
        .get("Error")
        .unwrap_or_else(|| panic!("case {case:?}: the failure must carry an Error, got {entry}"));
    assert_eq!(
        detail.get("__type").and_then(Value::as_str),
        Some(ERROR_DETAIL_TAG),
        "case {case:?}: the nested detail carries its own type tag, got {detail}"
    );
    assert_eq!(
        detail.get("ErrorCode").and_then(Value::as_str),
        Some(NOT_FOUND_CODE),
        "case {case:?}: a column with no statistics is a miss, got {detail}"
    );
    assert_eq!(
        detail.get("ErrorMessage").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_NOT_FOUND),
        "case {case:?}: the detail says why the column was not satisfied, got {detail}"
    );
}

// ---------------------------------------------------------------------------
// write -> read: what was stored is what comes back
// ---------------------------------------------------------------------------

/// Statistics a write stored for a column are read back entry for entry, and that column is
/// NOT named in `Errors`.
///
/// One row per `ColumnStatisticsType`, each with its own declared column type, its own
/// instant and its own figures.
#[tokio::test]
async fn statistics_written_for_a_column_are_read_back() {
    let router = emulator();
    seed(&router, "stats_db", "events").await;

    for row in statistics_cases() {
        let case = row.case;
        let (status, body) =
            write_statistics(&router, "stats_db", "events", &[entry_of(&row)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the write must answer 200, got {status} with {body}"
        );
        assert!(
            reported_errors(&body).is_empty(),
            "case {case:?}: a well-formed entry is stored, so nothing failed, got {body}"
        );

        let (status, answer) = read_statistics(&router, "stats_db", "events", &[row.column]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must answer 200, got {status} with {answer}"
        );
        assert!(
            reported_errors(&answer).is_empty(),
            "case {case:?}: the column has statistics, so it is not reported as a failure, got \
             {answer}"
        );
        let entries = reported_statistics(&answer);
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: exactly one entry is reported for one requested column, got {answer}"
        );
        assert_entry_matches(case, &row, &entries[0]);
    }
}

/// One call writes up to the whole submitted list, and one read reports the columns it asked
/// for in the order it asked for them — a column asked for twice is answered twice.
#[tokio::test]
async fn a_read_answers_every_requested_column_in_request_order() {
    let router = emulator();
    seed(&router, "order_db", "events").await;
    let rows = statistics_cases();
    let entries: Vec<Value> = rows.iter().map(entry_of).collect();

    let (status, body) = write_statistics(&router, "order_db", "events", &entries).await;
    assert_eq!(
        status, 200,
        "the write of the whole list must answer 200, got {status} with {body}"
    );
    assert!(
        reported_errors(&body).is_empty(),
        "every entry of the list is well formed, so nothing failed, got {body}"
    );

    // (case, the columns the read asks for)
    let requests: Vec<(&str, Vec<&str>)> = vec![
        ("one column", vec![rows[0].column]),
        (
            "three columns, in an order of the caller's own",
            vec![rows[2].column, rows[0].column, rows[4].column],
        ),
        (
            "the same column twice",
            vec![rows[1].column, rows[1].column],
        ),
        (
            "every column the table declares",
            rows.iter().map(|row| row.column).collect(),
        ),
        ("no column at all", Vec::new()),
    ];

    for (case, columns) in requests {
        let (status, answer) = read_statistics(&router, "order_db", "events", &columns).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must answer 200, got {status} with {answer}"
        );
        assert!(
            reported_errors(&answer).is_empty(),
            "case {case:?}: every requested column has statistics, got {answer}"
        );
        let reported: Vec<&str> = reported_statistics(&answer)
            .iter()
            .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(
            reported, columns,
            "case {case:?}: the answer follows the request's own list, got {answer}"
        );
    }
}

/// A later write of the same column supersedes the earlier one: there is at most one entry
/// per column, so the read reports the figures the LAST write stored.
#[tokio::test]
async fn a_second_write_of_a_column_supersedes_the_first() {
    let router = emulator();
    seed(&router, "replace_db", "events").await;
    let first = StatisticsCase {
        case: "the first write",
        column: "id",
        column_type: "int",
        statistics_type: "LONG",
        analyzed_time: 1_690_000_000.0,
        figures: json!({
            "MinimumValue": 1,
            "MaximumValue": 10,
            "NumberOfNulls": 0,
            "NumberOfDistinctValues": 10
        }),
    };
    let second = StatisticsCase {
        case: "the second write",
        column: "id",
        column_type: "bigint",
        statistics_type: "LONG",
        analyzed_time: 1_695_000_000.0,
        figures: json!({
            "MinimumValue": -4,
            "MaximumValue": 9000,
            "NumberOfNulls": 12,
            "NumberOfDistinctValues": 77
        }),
    };

    for row in [&first, &second] {
        let (status, body) = write_statistics(&router, "replace_db", "events", &[entry_of(row)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the write must answer 200, got {status} with {body}",
            row.case
        );
    }

    let (_, answer) = read_statistics(&router, "replace_db", "events", &["id"]).await;
    let entries = reported_statistics(&answer);
    assert_eq!(
        entries.len(),
        1,
        "case {:?}: a column carries at most one entry, got {answer}",
        second.case
    );
    assert_entry_matches(second.case, &second, &entries[0]);
}

/// The statistics of a column belong to ONE table: the same column name on another table
/// carries the statistics that table's own write stored, and a table nothing wrote for
/// carries none.
#[tokio::test]
async fn statistics_belong_to_the_table_they_were_written_for() {
    let router = emulator();
    seed(&router, "one_db", "events").await;
    seed(&router, "other_db", "events").await;

    let mine = StatisticsCase {
        case: "the table that was written for",
        column: "id",
        column_type: "int",
        statistics_type: "LONG",
        analyzed_time: 1_681_000_000.0,
        figures: json!({
            "MinimumValue": 5,
            "MaximumValue": 50,
            "NumberOfNulls": 1,
            "NumberOfDistinctValues": 9
        }),
    };
    let (status, body) = write_statistics(&router, "one_db", "events", &[entry_of(&mine)]).await;
    assert_eq!(
        status, 200,
        "the write must answer 200, got {status} with {body}"
    );

    let (_, answer) = read_statistics(&router, "one_db", "events", &["id"]).await;
    assert_entry_matches(mine.case, &mine, &reported_statistics(&answer)[0]);

    let (status, answer) = read_statistics(&router, "other_db", "events", &["id"]).await;
    assert_eq!(
        status, 200,
        "case {:?}: the read must answer 200, got {status} with {answer}",
        "a table nothing wrote for"
    );
    assert!(
        reported_statistics(&answer).is_empty(),
        "case {:?}: another table's write stored nothing here, got {answer}",
        "a table nothing wrote for"
    );
    assert_not_found_error(
        "a table nothing wrote for",
        "id",
        &reported_errors(&answer)[0],
    );
}

// ---------------------------------------------------------------------------
// a column with no statistics is reported in Errors
// ---------------------------------------------------------------------------

/// Every requested column the catalog holds no statistics for is reported in `Errors`, with
/// the status staying 200 — whether the column was never written, was written and then
/// deleted, or is a name the table declares no column under.
///
/// The rows differ along WHY the column has no statistics and along how many of the
/// requested columns are satisfied.
#[tokio::test]
async fn a_requested_column_with_no_statistics_is_reported_in_errors() {
    let router = emulator();
    seed(&router, "missing_db", "events").await;
    let rows = statistics_cases();

    // Only the first row's column is written, and the next one is written and deleted
    // again, so the table holds statistics for exactly one of its declared columns.
    let (_, body) = write_statistics(
        &router,
        "missing_db",
        "events",
        &[entry_of(&rows[0]), entry_of(&rows[1])],
    )
    .await;
    assert!(
        reported_errors(&body).is_empty(),
        "both entries are well formed, so nothing failed, got {body}"
    );
    let (status, body) = delete_statistics(&router, "missing_db", "events", rows[1].column).await;
    assert_eq!(
        status, 200,
        "the delete of stored statistics must answer 200, got {status} with {body}"
    );

    // (case, the columns the read asks for, the ones it cannot satisfy)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "a declared column nothing ever wrote statistics for",
            vec![rows[2].column],
            vec![rows[2].column],
        ),
        (
            "a column whose statistics were deleted again",
            vec![rows[1].column],
            vec![rows[1].column],
        ),
        (
            "a name the table declares no column under",
            vec!["never_declared"],
            vec!["never_declared"],
        ),
        (
            "a satisfied column beside three unsatisfied ones",
            vec![
                rows[0].column,
                rows[1].column,
                rows[2].column,
                "never_declared",
            ],
            vec![rows[1].column, rows[2].column, "never_declared"],
        ),
    ];

    for (case, columns, unsatisfied) in cases {
        let (status, answer) = read_statistics(&router, "missing_db", "events", &columns).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unsatisfied column does not fail the call, got {status} with \
             {answer}"
        );
        let failed: Vec<&str> = reported_errors(&answer)
            .iter()
            .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(
            failed, unsatisfied,
            "case {case:?}: every column with no statistics is reported, got {answer}"
        );
        for entry in reported_errors(&answer) {
            let column = entry.get("ColumnName").and_then(Value::as_str).unwrap_or("");
            assert_not_found_error(case, column, entry);
        }
        let reported: Vec<&str> = reported_statistics(&answer)
            .iter()
            .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
            .collect();
        for column in &unsatisfied {
            assert!(
                !reported.contains(column),
                "case {case:?}: an unsatisfied column carries no statistics entry, got {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// the delete
// ---------------------------------------------------------------------------

/// A delete drops the statistics of the column it names and leaves every other column
/// alone, and a second delete of the same column is the catalog's own miss.
#[tokio::test]
async fn a_delete_drops_one_columns_statistics() {
    let router = emulator();
    seed(&router, "delete_db", "events").await;
    let rows = statistics_cases();
    let entries: Vec<Value> = rows.iter().map(entry_of).collect();
    write_statistics(&router, "delete_db", "events", &entries).await;

    for row in &rows {
        let (status, body) = delete_statistics(&router, "delete_db", "events", row.column).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete of stored statistics must answer 200, got {status} with \
             {body}",
            row.case
        );

        let (_, answer) = read_statistics(&router, "delete_db", "events", &[row.column]).await;
        assert!(
            reported_statistics(&answer).is_empty(),
            "case {:?}: the deleted column has no statistics left, got {answer}",
            row.case
        );
        assert_not_found_error(row.case, row.column, &reported_errors(&answer)[0]);

        let (status, body) = delete_statistics(&router, "delete_db", "events", row.column).await;
        assert_eq!(
            status, 400,
            "case {:?}: a second delete finds nothing to delete, got {status} with {body}",
            row.case
        );
        assert!(
            error_code(&body).ends_with(NOT_FOUND_CODE),
            "case {:?}: expected {NOT_FOUND_CODE}, got {} in {body}",
            row.case,
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            COLUMN_STATISTICS_NOT_FOUND,
            "case {:?}: the miss carries the catalog's own wording, got {body}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// an entry that states no figures
// ---------------------------------------------------------------------------

/// A submitted entry whose `StatisticsData` does not carry the member its own `Type` names
/// states no statistics: it is reported in the write's `Errors`, with the entry as sent, and
/// nothing is stored for its column — while the well-formed entries of the same call are.
///
/// The rows differ along which `ColumnStatisticsType` was stated and which member was sent
/// instead of the one it selects.
#[tokio::test]
async fn an_entry_that_states_no_figures_is_reported_and_not_stored() {
    let router = emulator();
    seed(&router, "refused_db", "events").await;
    let good = statistics_cases().remove(0);

    // (case, the stated Type, the data members the entry carries)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("LONG with no member at all", "LONG", json!({})),
        (
            "DOUBLE carrying the LONG member instead",
            "DOUBLE",
            json!({
                "LongColumnStatisticsData": {
                    "MinimumValue": 1,
                    "MaximumValue": 2,
                    "NumberOfNulls": 0,
                    "NumberOfDistinctValues": 2
                }
            }),
        ),
        (
            "STRING carrying the BINARY member instead",
            "STRING",
            json!({
                "BinaryColumnStatisticsData": {
                    "MaximumLength": 8,
                    "AverageLength": 4.0,
                    "NumberOfNulls": 0
                }
            }),
        ),
        ("BOOLEAN with no member at all", "BOOLEAN", json!({})),
        ("DATE with no member at all", "DATE", json!({})),
        ("DECIMAL with no member at all", "DECIMAL", json!({})),
        ("BINARY with no member at all", "BINARY", json!({})),
    ];

    for (case, statistics_type, members) in cases {
        let mut data = json!({ "Type": statistics_type });
        for (member, value) in members.as_object().expect("the members are an object") {
            data[member] = value.clone();
        }
        let refused = json!({
            "ColumnName": "label",
            "ColumnType": "string",
            "AnalyzedTime": 1_672_000_000.0,
            "StatisticsData": data
        });

        let (status, body) = write_statistics(
            &router,
            "refused_db",
            "events",
            &[refused.clone(), entry_of(&good)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused entry does not fail the call, got {status} with {body}"
        );
        let errors = reported_errors(&body);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: the only failure is the entry that states no figures, got {body}"
        );
        assert_figures_echoed(
            case,
            "ColumnStatistics",
            &refused,
            errors[0].get("ColumnStatistics"),
        );
        let detail = &errors[0]["Error"];
        assert_eq!(
            detail.get("__type").and_then(Value::as_str),
            Some(ERROR_DETAIL_TAG),
            "case {case:?}: the nested detail carries its own type tag, got {detail}"
        );
        assert_eq!(
            detail.get("ErrorCode").and_then(Value::as_str),
            Some(INVALID_INPUT_CODE),
            "case {case:?}: an entry the service cannot store is invalid input, got {detail}"
        );
        assert_eq!(
            detail.get("ErrorMessage").and_then(Value::as_str),
            Some(
                column_statistics_data_member_missing(
                    statistics_type,
                    data_member(statistics_type)
                )
                .as_str()
            ),
            "case {case:?}: the detail names the member the stated type requires, got {detail}"
        );

        // The refused entry stored nothing, and the well-formed one beside it did.
        let (_, answer) = read_statistics(&router, "refused_db", "events", &["label", "id"]).await;
        let failed: Vec<&str> = reported_errors(&answer)
            .iter()
            .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(
            failed,
            vec!["label"],
            "case {case:?}: the refused entry stored nothing for its column, got {answer}"
        );
        assert_entry_matches(case, &good, &reported_statistics(&answer)[0]);
    }
}

// ---------------------------------------------------------------------------
// the store's capacity
// ---------------------------------------------------------------------------

/// The columns ONE table keeps statistics for are bounded: once the table holds the
/// ceiling, an entry for a FURTHER column is reported in `Errors` and stored nowhere, while
/// an entry for a column the table already holds statistics for is still stored.
#[tokio::test]
async fn the_columns_one_table_keeps_statistics_for_are_bounded() {
    let router = emulator();
    seed(&router, "capacity_db", "events").await;
    let row = statistics_cases().remove(0);

    // Fill the table to the ceiling, 25 entries per call (the modelled maximum of
    // `UpdateColumnStatisticsList`).
    let mut stored = 0usize;
    while stored < MAX_COLUMN_STATISTICS_PER_TABLE {
        let batch: Vec<Value> = (stored..(stored + 25).min(MAX_COLUMN_STATISTICS_PER_TABLE))
            .map(|index| {
                let mut entry = entry_of(&row);
                entry["ColumnName"] = json!(format!("column_{index}"));
                entry
            })
            .collect();
        stored += batch.len();
        let (status, body) = write_statistics(&router, "capacity_db", "events", &batch).await;
        assert_eq!(
            status, 200,
            "case {:?}: a write up to the ceiling must answer 200, got {status} with {body}",
            "filling the table"
        );
        assert!(
            reported_errors(&body).is_empty(),
            "case {:?}: an entry inside the ceiling is stored, got {body}",
            "filling the table"
        );
    }

    let mut beyond = entry_of(&row);
    beyond["ColumnName"] = json!("one_column_too_many");
    let (status, body) = write_statistics(&router, "capacity_db", "events", &[beyond]).await;
    assert_eq!(
        status, 200,
        "case {:?}: a refused entry does not fail the call, got {status} with {body}",
        "one column past the ceiling"
    );
    let errors = reported_errors(&body);
    assert_eq!(
        errors.len(),
        1,
        "case {:?}: the entry the store has no room for is reported, got {body}",
        "one column past the ceiling"
    );
    assert_eq!(
        errors[0]["Error"].get("ErrorCode").and_then(Value::as_str),
        Some(LIMIT_EXCEEDED_CODE),
        "case {:?}: a ceiling that was reached is a limit failure, got {body}",
        "one column past the ceiling"
    );
    assert_eq!(
        errors[0]["Error"].get("ErrorMessage").and_then(Value::as_str),
        Some(column_statistics_limit_exceeded(MAX_COLUMN_STATISTICS_PER_TABLE).as_str()),
        "case {:?}: the detail states the ceiling that was reached, got {body}",
        "one column past the ceiling"
    );

    let (_, answer) = read_statistics(
        &router,
        "capacity_db",
        "events",
        &["one_column_too_many", "column_0"],
    )
    .await;
    let failed: Vec<&str> = reported_errors(&answer)
        .iter()
        .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
        .collect();
    assert_eq!(
        failed,
        vec!["one_column_too_many"],
        "case {:?}: the refused entry stored nothing, got {answer}",
        "one column past the ceiling"
    );

    // A column the table already holds statistics for is replaced whatever the count.
    let mut refreshed = entry_of(&row);
    refreshed["ColumnName"] = json!("column_0");
    refreshed["AnalyzedTime"] = json!(1_720_000_000.0);
    let (status, body) = write_statistics(&router, "capacity_db", "events", &[refreshed]).await;
    assert_eq!(
        status, 200,
        "case {:?}: refreshing a stored column must answer 200, got {status} with {body}",
        "a refresh at the ceiling"
    );
    assert!(
        reported_errors(&body).is_empty(),
        "case {:?}: a column the table already holds is replaced, not refused, got {body}",
        "a refresh at the ceiling"
    );
    let (_, answer) = read_statistics(&router, "capacity_db", "events", &["column_0"]).await;
    let analyzed = reported_statistics(&answer)[0]
        .get("AnalyzedTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        (analyzed - 1_720_000_000.0).abs() < 1e-6,
        "case {:?}: expected the refreshed AnalyzedTime 1720000000, got {analyzed} in {answer}",
        "a refresh at the ceiling"
    );
}

// ---------------------------------------------------------------------------
// the address has to resolve
// ---------------------------------------------------------------------------

/// Statistics are addressed THROUGH a stored table, so an address that resolves to no table
/// refuses the write, the read and the delete alike with `EntityNotFoundException` — and
/// persists nothing.
///
/// The rows differ along which part of the address does not resolve: the database, the
/// table, and the catalog (an id no catalog of this account answers to).
#[tokio::test]
async fn an_address_that_resolves_to_no_table_is_not_found() {
    let router = emulator();
    seed(&router, "address_db", "events").await;
    let row = statistics_cases().remove(0);

    // (case, the catalog id, the database, the table)
    let cases: [(&str, &str, &str, &str); 3] = [
        (
            "a database the catalog does not hold",
            ACCOUNT_ID,
            "no_such_db",
            "events",
        ),
        (
            "a table the database does not hold",
            ACCOUNT_ID,
            "address_db",
            "no_such_table",
        ),
        (
            "a catalog this account does not hold",
            "210987654321",
            "address_db",
            "events",
        ),
    ];

    for (case, catalog_id, database, table) in cases {
        let (status, body) = call(
            &router,
            "UpdateColumnStatisticsForTable",
            &json!({
                "CatalogId": catalog_id,
                "DatabaseName": database,
                "TableName": table,
                "ColumnStatisticsList": [entry_of(&row)]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a write needs a stored table, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE} from the write, got {} in {body}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "GetColumnStatisticsForTable",
            &json!({
                "CatalogId": catalog_id,
                "DatabaseName": database,
                "TableName": table,
                "ColumnNames": [row.column]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a read needs a stored table, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE} from the read, got {} in {body}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "DeleteColumnStatisticsForTable",
            &json!({
                "CatalogId": catalog_id,
                "DatabaseName": database,
                "TableName": table,
                "ColumnName": row.column
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a delete needs a stored table, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE} from the delete, got {} in {body}",
            error_code(&body)
        );
    }

    // The refused writes stored nothing on the table that DOES exist.
    let (_, answer) = read_statistics(&router, "address_db", "events", &[row.column]).await;
    assert!(
        reported_statistics(&answer).is_empty(),
        "a refused write stores nothing, got {answer}"
    );
}
