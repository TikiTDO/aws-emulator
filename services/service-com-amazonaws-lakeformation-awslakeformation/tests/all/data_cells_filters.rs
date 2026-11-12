// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the data cells filter lifecycle: `CreateDataCellsFilter`,
//! `UpdateDataCellsFilter` and the `GetDataCellsFilter` reader that proves what each
//! persisted.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The tables below span the classes the rules generalise over: how a filter names its
//! columns (an explicit list, or a wildcard with and without exclusions), how it names
//! its rows (a PartiQL expression, the all-rows wildcard, or no predicate at all), how
//! many columns it names, how deeply a nested column attribute nests, which part of
//! the (catalog, database, table, name) identity differs, and what an update changes.
//!
//! Two things the service adds to the filter a request names: the VERSION it assigned (a
//! create's is the first, and each accepted update advances it), and both forms of the
//! all-rows predicate for a filter that selects every row.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lakeformation_awslakeformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator over one shared engine, reached exactly as the production router reaches
/// it: `MultiServiceRouter::dispatch` of a buffered HTTP request.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, answering `(status, body)`.
///
/// The `Authorization` credential scope names `lakeformation`, which is the routing
/// signal the router reads first; no authenticator is registered, so the signature itself
/// is not checked.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.us-east-1.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lakeformation/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The wire error code (`__type`) of an error envelope, or `""` for a success body.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `GetDataCellsFilter` request that addresses the filter `table_data` describes.
fn locator(table_data: &Value) -> Value {
    json!({
        "TableCatalogId": table_data["TableCatalogId"],
        "DatabaseName": table_data["DatabaseName"],
        "TableName": table_data["TableName"],
        "Name": table_data["Name"],
    })
}

/// The PartiQL predicate that selects every row, which is what the all-rows wildcard says.
const ALL_ROWS: &str = "TRUE";

/// The filter a read reports for a `TableData` stored at `version`.
///
/// Two things the service adds to what the request named: the `VersionId` it assigned (the
/// version is the service's counter, not a value the request supplies), and — for a filter
/// that selects every row — both forms of that predicate, the all-rows wildcard and the
/// `TRUE` expression that says the same thing.
fn reported(table_data: &Value, version: &str) -> Value {
    let mut expected = table_data.clone();
    expected["VersionId"] = json!(version);
    let all_rows = expected.get("RowFilter").is_some_and(|row_filter| {
        row_filter.get("AllRowsWildcard").is_some()
            || row_filter
                .get("FilterExpression")
                .and_then(Value::as_str)
                .is_some_and(|expression| expression.eq_ignore_ascii_case(ALL_ROWS))
    });
    if all_rows {
        expected["RowFilter"] = json!({ "FilterExpression": ALL_ROWS, "AllRowsWildcard": {} });
    }
    expected
}

/// The filter `GetDataCellsFilter` answers for the filter `table_data` addresses, with the
/// status the read answered.
async fn get_filter(router: &MultiServiceRouter, table_data: &Value) -> (u16, Value) {
    let (status, payload) = call(router, "GetDataCellsFilter", &locator(table_data)).await;
    (
        status,
        payload
            .get("DataCellsFilter")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn create_then_get_reads_the_stored_filter_back() {
    // Rows differ along what the rule generalises over: the column-naming form (explicit
    // list, bare wildcard, wildcard with exclusions), the number of columns, the
    // row-predicate form (expression, all-rows wildcard, no predicate), whether the
    // caller supplied a `VersionId`, and the modelled nesting depth bound of 5
    // ("Nested attributes within this list may not exceed a depth of 5." —
    // `smithy_query(query="doc",shape="DataCellsFilter",member="ColumnNames")`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "expression over named columns",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "us-only",
                "RowFilter": { "FilterExpression": "region = 'us-east-1'" },
                "ColumnNames": ["order_id", "region"],
            }),
        ),
        (
            "all rows over one column",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "all-rows",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnNames": ["order_id"],
            }),
        ),
        (
            "bare column wildcard",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "every-column",
                "RowFilter": { "FilterExpression": "amount > 10" },
                "ColumnWildcard": {},
            }),
        ),
        (
            "column wildcard with exclusions",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "no-pii",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnWildcard": { "ExcludedColumnNames": ["ssn", "email"] },
            }),
        ),
        (
            "no row predicate at all",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "columns-only",
                "ColumnNames": ["order_id", "amount", "region"],
            }),
        ),
        (
            "caller-supplied version id",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "versioned",
                "RowFilter": { "FilterExpression": "amount > 0" },
                "ColumnNames": ["amount"],
                "VersionId": "7",
            }),
        ),
        (
            "nested attribute at the documented depth bound",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "deep-nesting",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnNames": ["\"a\".\"b\".\"c\".\"d\".\"e\"", "\"address\".\"zip\""],
            }),
        ),
        (
            // A dot INSIDE a quoted segment is part of that segment's name, not a level
            // separator, so this path nests two attributes and not three.
            "quoted dot inside one segment",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "quoted-dot",
                "ColumnNames": ["\"address.line\".\"zip\""],
            }),
        ),
        (
            "a different catalog, database and table",
            json!({
                "TableCatalogId": "444455556666",
                "DatabaseName": "marketing",
                "TableName": "leads",
                "Name": "us-only",
                "RowFilter": { "FilterExpression": "country = 'US'" },
                "ColumnWildcard": { "ExcludedColumnNames": ["phone"] },
            }),
        ),
    ];

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataCellsFilter 200, got {status} {payload:?}"
        );
    }

    // Every filter reads back as what its request named, plus the version the service
    // assigned it: the create invents no other member and drops none.
    for (case, table_data) in &cases {
        let (status, actual) = get_filter(&router, table_data).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataCellsFilter 200, got {status} {actual:?}"
        );
        let expected = reported(table_data, "1");
        assert_eq!(
            &actual, &expected,
            "case {case:?}: expected the stored filter {expected:?}, got {actual:?}"
        );
    }

    // A read changes nothing a later caller sees: the second read answers the first.
    for (case, table_data) in &cases {
        let (status, actual) = get_filter(&router, table_data).await;
        let expected = reported(table_data, "1");
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeat read to answer 200, got {status} {actual:?}"
        );
        assert_eq!(
            &actual, &expected,
            "case {case:?}: expected a repeat read to answer {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_is_identified_by_catalog_database_table_and_name() {
    // One filter NAME under four different identities is four different filters; the rows
    // differ in exactly one component of the identity each.
    let base = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "shared-name",
        "ColumnNames": ["order_id"],
    });
    let mut cases: Vec<(&str, Value)> = vec![("the base identity", base.clone())];
    for (case, member, value) in [
        ("a different catalog", "TableCatalogId", "444455556666"),
        ("a different database", "DatabaseName", "marketing"),
        ("a different table", "TableName", "returns"),
        ("a different filter name", "Name", "other-name"),
    ] {
        let mut variant = base.clone();
        variant[member] = json!(value);
        // Each variant gets its own column list so a lookup that confused two identities
        // would answer the wrong columns rather than coincidentally matching.
        variant["ColumnNames"] = json!([format!("col_{value}")]);
        cases.push((case, variant));
    }

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for a distinct identity, got {status} {payload:?}"
        );
    }
    for (case, table_data) in &cases {
        let (status, actual) = get_filter(&router, table_data).await;
        let expected = reported(table_data, "1");
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {actual:?}"
        );
        assert_eq!(
            &actual, &expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn creating_an_existing_filter_is_refused_and_keeps_the_stored_one() {
    let router = emulator();
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "RowFilter": { "FilterExpression": "region = 'us-east-1'" },
        "ColumnNames": ["order_id", "region"],
    });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(
        status, 200,
        "case \"first create\": expected 200, got {status} {payload:?}"
    );

    // A second create of the same identity, with a different body, is refused — and the
    // refusal leaves the first filter in place rather than replacing it.
    let mut replacement = stored.clone();
    replacement["ColumnNames"] = json!(["order_id"]);
    replacement["RowFilter"] = json!({ "AllRowsWildcard": {} });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": replacement })).await;
    assert_eq!(
        status, 400,
        "case \"duplicate create\": expected 400, got {status} {payload:?}"
    );
    let code = error_code(&payload);
    assert_eq!(
        code, "AlreadyExistsException",
        "case \"duplicate create\": expected AlreadyExistsException, got {code:?} {payload:?}"
    );

    let (status, actual) = get_filter(&router, &stored).await;
    let expected = reported(&stored, "1");
    assert_eq!(
        status, 200,
        "case \"after duplicate create\": expected 200, got {status} {actual:?}"
    );
    assert_eq!(
        &actual, &expected,
        "case \"after duplicate create\": expected the first filter {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn update_replaces_the_stored_filter_and_advances_its_version() {
    // RULE: an update carries the WHOLE filter, so what the request names is what the
    // filter holds afterwards — a member it leaves out is gone — and the service's version
    // counter advances by one for each accepted update.
    //
    // Rows differ along what the update changes: which cells are named (a narrower column
    // list, a wider one, a switch between an explicit list and each form of the column
    // wildcard) and which rows (a different predicate, a switch to the all-rows wildcard
    // and back, and dropping the predicate altogether).
    let identity = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
    });
    let filter = |extra: Value| {
        let mut body = identity.clone();
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key] = value;
            }
        }
        body
    };

    let created = filter(json!({
        "ColumnNames": ["order_id", "region", "amount"],
        "RowFilter": { "FilterExpression": "region = 'us-east-1'" },
    }));
    let updates: Vec<(&str, Value)> = vec![
        (
            "a narrower column list and the all-rows wildcard",
            filter(json!({
                "ColumnNames": ["order_id"],
                "RowFilter": { "AllRowsWildcard": {} },
            })),
        ),
        (
            "a wider column list and a predicate again",
            filter(json!({
                "ColumnNames": ["order_id", "amount"],
                "RowFilter": { "FilterExpression": "amount > 10" },
            })),
        ),
        (
            "the column wildcard with exclusions instead of a list",
            filter(json!({
                "ColumnWildcard": { "ExcludedColumnNames": ["ssn"] },
                "RowFilter": { "FilterExpression": "amount > 10" },
            })),
        ),
        (
            "the bare column wildcard and no predicate at all",
            filter(json!({ "ColumnWildcard": {} })),
        ),
        (
            "an explicit list again, with a nested attribute",
            filter(json!({
                "ColumnNames": ["order_id", "\"address\".\"zip\""],
                "RowFilter": { "AllRowsWildcard": {} },
            })),
        ),
        (
            // The same body twice is still an update: the version advances with the call.
            "the same filter again",
            filter(json!({
                "ColumnNames": ["order_id", "\"address\".\"zip\""],
                "RowFilter": { "AllRowsWildcard": {} },
            })),
        ),
    ];

    let router = emulator();
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": created })).await;
    assert_eq!(
        status, 200,
        "case \"create\": expected 200, got {status} {payload:?}"
    );
    let (status, actual) = get_filter(&router, &created).await;
    assert_eq!(
        (status, &actual),
        (200, &reported(&created, "1")),
        "case \"create\": a created filter is at the first version, got {status} {actual:?}"
    );

    for (index, (case, table_data)) in updates.iter().enumerate() {
        let (status, payload) =
            call(&router, "UpdateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateDataCellsFilter 200, got {status} {payload:?}"
        );

        // The version after n updates is n + 1: the create assigned the first.
        let version = (index + 2).to_string();
        let expected = reported(table_data, &version);
        let (status, actual) = get_filter(&router, table_data).await;
        assert_eq!(
            (status, &actual),
            (200, &expected),
            "case {case:?}: expected the updated filter {expected:?}, got {status} {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_update_is_refused_for_a_filter_the_store_does_not_hold() {
    // An update addresses a filter by its (catalog, database, table, name) identity, so a
    // tuple nothing was created for is `EntityNotFoundException` and writes nothing. Rows
    // change exactly one component of a stored filter's identity each.
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "ColumnNames": ["order_id"],
        "RowFilter": { "FilterExpression": "amount > 0" },
    });

    let router = emulator();
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(
        status, 200,
        "case \"seed\": expected 200, got {status} {payload:?}"
    );

    let cases: [(&str, &str, &str); 4] = [
        ("unknown catalog", "TableCatalogId", "444455556666"),
        ("unknown database", "DatabaseName", "marketing"),
        ("unknown table", "TableName", "returns"),
        ("unknown filter name", "Name", "eu-only"),
    ];
    for (case, member, value) in cases {
        let mut table_data = stored.clone();
        table_data[member] = json!(value);
        table_data["ColumnNames"] = json!(["amount"]);
        let (status, payload) =
            call(&router, "UpdateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException, got {code:?} {payload:?}"
        );

        // Nothing was created by the refused update, and the filter it did not address is
        // still at the version the create assigned.
        let (status, actual) = get_filter(&router, &table_data).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused update must persist nothing, got {status} {actual:?}"
        );
        let (status, actual) = get_filter(&router, &stored).await;
        assert_eq!(
            (status, &actual),
            (200, &reported(&stored, "1")),
            "case {case:?}: the stored filter must be untouched, got {status} {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_update_the_service_cannot_store_is_refused_and_changes_nothing() {
    // The rules a create is held to hold of an update too, because `TableData` is the
    // whole filter: exactly one way of naming columns, the documented nesting depth, and
    // exactly one row predicate — except that the all-rows predicate beside the wildcard
    // says what the wildcard says and is accepted.
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "ColumnNames": ["order_id"],
        "RowFilter": { "FilterExpression": "amount > 0" },
    });
    let change = |extra: Value| {
        let mut body = stored.clone();
        body.as_object_mut().expect("an object").remove("ColumnNames");
        body.as_object_mut().expect("an object").remove("RowFilter");
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key] = value;
            }
        }
        body
    };

    let refused: Vec<(&str, Value)> = vec![
        (
            "neither ColumnNames nor ColumnWildcard",
            change(json!({ "RowFilter": { "AllRowsWildcard": {} } })),
        ),
        (
            "both ColumnNames and ColumnWildcard",
            change(json!({ "ColumnNames": ["order_id"], "ColumnWildcard": {} })),
        ),
        (
            "an empty ColumnNames list",
            change(json!({ "ColumnNames": [] })),
        ),
        (
            "a ColumnNames path one level past the depth bound",
            change(json!({ "ColumnNames": ["\"a\".\"b\".\"c\".\"d\".\"e\".\"f\""] })),
        ),
        (
            // Both alternatives at once, even where the expression is the all-rows
            // predicate the wildcard stands for: that pairing is what the service REPORTS
            // for a filter selecting every row, not something a request may name.
            "a RowFilter naming two different predicates",
            change(json!({
                "ColumnNames": ["order_id"],
                "RowFilter": { "FilterExpression": "amount > 0", "AllRowsWildcard": {} },
            })),
        ),
        (
            "the all-rows predicate beside the wildcard",
            change(json!({
                "ColumnNames": ["order_id"],
                "RowFilter": { "FilterExpression": "TRUE", "AllRowsWildcard": {} },
            })),
        ),
        (
            "a RowFilter naming neither alternative",
            change(json!({ "ColumnNames": ["order_id"], "RowFilter": {} })),
        ),
    ];

    let router = emulator();
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(
        status, 200,
        "case \"seed\": expected 200, got {status} {payload:?}"
    );

    for (case, table_data) in &refused {
        let (status, payload) =
            call(&router, "UpdateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {code:?} {payload:?}"
        );

        // A refused update changes neither the filter nor its version.
        let (status, actual) = get_filter(&router, &stored).await;
        assert_eq!(
            (status, &actual),
            (200, &reported(&stored, "1")),
            "case {case:?}: a refused update must change nothing, got {status} {actual:?}"
        );
    }

    // The wildcard ALONE selects every row, and that is the request form the service
    // takes: the update is served and the filter is reported carrying both forms of the
    // predicate, at the next version.
    let all_rows = change(json!({
        "ColumnNames": ["order_id"],
        "RowFilter": { "AllRowsWildcard": {} },
    }));
    let (status, payload) =
        call(&router, "UpdateDataCellsFilter", &json!({ "TableData": all_rows })).await;
    assert_eq!(
        status, 200,
        "case \"the all-rows wildcard alone\": expected 200, got {status} {payload:?}"
    );
    let (status, actual) = get_filter(&router, &all_rows).await;
    assert_eq!(
        (status, &actual),
        (200, &reported(&all_rows, "2")),
        "case \"the all-rows wildcard alone\": expected both forms at version 2, got \
         {status} {actual:?}"
    );
}

#[tokio::test]
async fn a_filter_the_store_does_not_hold_is_not_found() {
    let router = emulator();
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "ColumnNames": ["order_id"],
    });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(status, 200, "case \"seed\": expected 200, got {status} {payload:?}");

    // Each row changes exactly one component of a stored filter's identity, so the lookup
    // misses for a different reason each time.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown catalog", "TableCatalogId", "444455556666"),
        ("unknown database", "DatabaseName", "marketing"),
        ("unknown table", "TableName", "returns"),
        ("unknown filter name", "Name", "eu-only"),
    ];
    for (case, member, value) in cases {
        let mut request = locator(&stored);
        request[member] = json!(value);
        let (status, payload) = call(&router, "GetDataCellsFilter", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException, got {code:?} {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_that_does_not_name_its_cells_is_refused() {
    // The model states both rules in prose: "You must specify either a `ColumnNames` list
    // or the `ColumnWildCard`" (`DataCellsFilter$ColumnWildcard`), and "Nested attributes
    // within this list may not exceed a depth of 5."
    // (`smithy_query(query="doc",shape="DataCellsFilter",member="ColumnNames")`). Rows
    // span both, on each side of the bound and on each of the two column lists.
    let identity = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
    });
    let case_body = |name: &str, extra: Value| {
        let mut body = identity.clone();
        body["Name"] = json!(name);
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key] = value;
            }
        }
        body
    };

    let cases: Vec<(&str, Value)> = vec![
        (
            "neither ColumnNames nor ColumnWildcard",
            case_body("no-columns", json!({ "RowFilter": { "AllRowsWildcard": {} } })),
        ),
        (
            "both ColumnNames and ColumnWildcard",
            case_body(
                "both-columns",
                json!({ "ColumnNames": ["order_id"], "ColumnWildcard": {} }),
            ),
        ),
        (
            "an empty ColumnNames list",
            case_body("empty-columns", json!({ "ColumnNames": [] })),
        ),
        (
            "a ColumnNames path one level past the depth bound",
            case_body(
                "too-deep",
                json!({ "ColumnNames": ["\"a\".\"b\".\"c\".\"d\".\"e\".\"f\""] }),
            ),
        ),
        (
            "an ExcludedColumnNames path one level past the depth bound",
            case_body(
                "too-deep-exclusion",
                json!({
                    "ColumnWildcard": {
                        "ExcludedColumnNames": ["ssn", "a.b.c.d.e.f"]
                    }
                }),
            ),
        ),
        (
            // An unterminated quote would otherwise hide every dot after it, scoring a
            // path shallower than the levels it actually names.
            "an unterminated quoted segment hiding six levels",
            case_body(
                "unbalanced-quote",
                json!({ "ColumnNames": ["\"a\".b.c.d.e.f.g"] }),
            ),
        ),
        (
            "a RowFilter naming both alternatives",
            case_body(
                "both-rows",
                json!({
                    "ColumnNames": ["order_id"],
                    "RowFilter": { "FilterExpression": "amount > 0", "AllRowsWildcard": {} },
                }),
            ),
        ),
        (
            "a RowFilter naming neither alternative",
            case_body(
                "no-rows",
                json!({ "ColumnNames": ["order_id"], "RowFilter": {} }),
            ),
        ),
    ];

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {code:?} {payload:?}"
        );
        // A refused create persists nothing: the filter it named is not in the store.
        let (status, payload) = call(&router, "GetDataCellsFilter", &locator(table_data)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refused create to persist nothing, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException after a refusal, got {code:?} {payload:?}"
        );
    }
}
