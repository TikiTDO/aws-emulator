// SPDX-License-Identifier: CC0-1.0

//! Athena's STATEMENTS over the AWS Glue Data Catalog: the DDL that registers a table or
//! a view, the statements that report what the catalog holds, and the reads that resolve a
//! relation through it and scan the objects at its location.
//!
//! The rule under test, from `StartQueryExecution`'s own documentation ("runs the SQL
//! query statements contained in the Query") and from what Athena is — a query service
//! over a data catalog whose `AwsDataCatalog` IS the account's Glue Data Catalog:
//!
//! * a statement that DEFINES a catalog object is performed against that catalog, so Hive
//!   `CREATE DATABASE` (and its `CREATE SCHEMA` spelling), Hive `CREATE EXTERNAL TABLE`
//!   and Presto `CREATE [OR REPLACE] VIEW` reach State SUCCEEDED with StatementType DDL
//!   and leave an entry Glue's own reads report — a database carrying the catalog's
//!   identity, assigned id, create instant and seeded Lake Formation grant, an
//!   `EXTERNAL_TABLE` owned by the DDL engine, carrying `EXTERNAL=TRUE`, the declared
//!   columns and the location with its trailing separator dropped, or a `VIRTUAL_VIEW`
//!   carrying the view's query and an empty location;
//! * a statement that DROPS one removes that same record, so a dropped table and a dropped
//!   database are gone from Glue's reads and from its own deletes — a `CASCADE` takes the
//!   tables of a database with it, and a drop naming a database the catalog holds no record
//!   of is the engine's own to perform;
//! * a statement that addresses an object the catalog ALREADY holds, or does NOT hold,
//!   fails with the category and the error type Athena publishes for that class, and
//!   leaves the catalog exactly as it was;
//! * a statement that READS a relation resolves it through the catalog and computes its
//!   answer from the objects at that relation's location, decoded by the SerDe the catalog
//!   recorded — so a table with no objects under it answers no rows, and a relation the
//!   catalog does not hold is Athena's own not-found refusal.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how a
//! name is qualified and quoted, which grammar the statement is written in, what the
//! declared columns and delimiters are, how many objects the location holds and how they
//! are laid out under it. The recorded cases — a `CREATE EXTERNAL TABLE` whose location
//! drops its trailing slash, a `CREATE VIEW` over an existing table, and a `SELECT` over a
//! table whose fields are separated by the default Hive delimiter — are rows among them.
//!
//! Every call goes over the wire the production front door uses: Athena and Glue through
//! `POST /` with `X-Amz-Target`, and Amazon S3 through its own REST data plane, all on one
//! `MultiServiceRouter` over ONE shared engine, so what is asserted is HTTP behaviour and
//! the three services see each other's state exactly as they do in the emulator.
//!
//! `tests/all/database_statements.rs` holds the EXECUTION axis of the database statements —
//! which clause forms reach SUCCEEDED, and the classification of each metastore refusal —
//! and owns those refusal classes; the axis here is what the CATALOG then holds for them:
//! the record Glue's own reads report for a database the DDL defined, the listing of the
//! tables it holds, and the catalog's own delete of it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::catalog_sql::ddl;
use service_com_amazonaws_athena_amazonathena::state::{
    query_engine, QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_code;

/// The name Athena reaches the account's AWS Glue Data Catalog by.
const GLUE_CATALOG: &str = "AwsDataCatalog";

/// The bucket every table in these suites keeps its data in.
const BUCKET: &str = "athena-catalog-data";

/// Where a query's results are written, which every submission has to name.
const OUTPUT_LOCATION: &str = "s3://athena-catalog-data/results/";

/// How long past [`QUERY_EXECUTION_COMPLETION`] a test waits for a started query to be
/// readable in its terminal state.
const SETTLE_GRACE: Duration = Duration::from_millis(400);

/// The character Hive separates fields with when a table's SerDe names none.
const UNIT_SEPARATOR: char = '\u{1}';

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena, Glue and Amazon S3 mounted on ONE shared engine, the way the router mounts
/// every service: the catalog Glue holds is the catalog Athena resolves against, and the
/// objects S3 holds are the data Athena reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_1 call to `service`, and its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.us-east-1.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header("authorization", authorization(service))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Athena call, as a client makes it.
async fn athena(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "athena", "AmazonAthena", operation, body).await
}

/// One Glue call, as a client makes it.
async fn glue(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "glue", "AWSGlue", operation, body).await
}

/// One Glue call that must succeed.
async fn glue_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = glue(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {answer:?}"
    );
    answer
}

/// Create the bucket every table's data lives in.
async fn create_bucket(router: &MultiServiceRouter) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{BUCKET}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "setup: creating s3://{BUCKET} must succeed, got {} with {}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Write one object, which is what a table's location then holds.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{BUCKET}/{key}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "case {case:?}: writing s3://{BUCKET}/{key} must succeed, got {} with {}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create one Glue database, as a client creates one.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    glue_ok(
        router,
        name,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// Start one statement in `database` and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, database: &str, sql: &str) -> String {
    let (status, answer) = athena(
        router,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": database, "Catalog": GLUE_CATALOG },
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartQueryExecution({sql}) must be accepted, got {status} with \
         {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {answer:?}"))
        .to_string()
}

/// Wait out the span a completion is owed after, once for a whole suite.
async fn settle() {
    ::tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = athena(
        router,
        "GetQueryExecution",
        &json!({ "QueryExecutionId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryExecution must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// Assert that the execution `id` SUCCEEDED, and answer its stored record.
async fn succeeded(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let found = execution(router, case, id).await;
    assert_eq!(
        found["Status"]["State"].as_str(),
        Some(QUERY_STATE_SUCCEEDED),
        "case {case:?}: expected {QUERY_STATE_SUCCEEDED}, got {found:?}"
    );
    found
}

/// Assert that the execution `id` FAILED with the category and error type Athena
/// publishes for that failure class, and answer the message it reported.
async fn failed(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    category: i64,
    error_type: Option<i64>,
) -> String {
    let found = execution(router, case, id).await;
    assert_eq!(
        found["Status"]["State"].as_str(),
        Some(QUERY_STATE_FAILED),
        "case {case:?}: expected {QUERY_STATE_FAILED}, got {found:?}"
    );
    assert_eq!(
        found["Status"]["AthenaError"]["ErrorCategory"].as_i64(),
        Some(category),
        "case {case:?}: expected error category {category}, got {found:?}"
    );
    assert_eq!(
        found["Status"]["AthenaError"]["ErrorType"].as_i64(),
        error_type,
        "case {case:?}: expected error type {error_type:?}, got {found:?}"
    );
    let message = found["Status"]["AthenaError"]["ErrorMessage"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // The reason a failed query reports is what PERFORMED the statement wrote about the
    // failure, and the error message is the failure itself: for a statement the Hive DDL
    // engine performs the reason is that engine's own task or compilation line with the
    // metastore's message inside it, and for every other class the two are one sentence.
    // Either way the reason states at least what the error message states.
    let reason = found["Status"]["StateChangeReason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains(&message),
        "case {case:?}: the reason must state what the error message states, got \
         {found:?}"
    );
    message
}

/// The DATA rows a succeeded query's result set carries, without the row of column
/// labels Athena puts in front of a data-manipulating statement's rows.
async fn rows_of(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<Vec<Option<String>>> {
    let labelled = execution(router, case, id).await["StatementType"].as_str()
        == Some(query_engine::STATEMENT_TYPE_DML);
    let (status, body) = athena(
        router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": id, "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryResults must answer 200, got {status} with {body:?}"
    );
    body["ResultSet"]["Rows"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .skip(usize::from(labelled))
        .map(|row| {
            row["Data"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|datum| datum["VarCharValue"].as_str().map(ToString::to_string))
                .collect()
        })
        .collect()
}

/// The single value one row of one column carries.
fn only_value(case: &str, rows: &[Vec<Option<String>>]) -> String {
    match rows {
        [row] => row
            .first()
            .cloned()
            .flatten()
            .unwrap_or_else(|| panic!("case {case:?}: expected one value, got {rows:?}")),
        _ => panic!("case {case:?}: expected exactly one row, got {rows:?}"),
    }
}

/// The stored Glue table `database`.`table`, or `None` when the catalog holds none.
async fn glue_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
) -> Option<Value> {
    let (status, answer) = glue(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": table }),
    )
    .await;
    (status == 200).then(|| answer["Table"].clone())
}

/// One row of the DDL table: a `CREATE EXTERNAL TABLE` statement and the catalog entry it
/// must leave.
struct DdlCase {
    /// What this row is about.
    case: &'static str,
    /// The database the statement runs in.
    database: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// The table the catalog must then hold, as `(database, name)`.
    table: (&'static str, &'static str),
    /// The `StorageDescriptor.Location` it must carry.
    location: &'static str,
    /// Its columns, as `(name, type)`.
    columns: Vec<(&'static str, &'static str)>,
    /// Its partition keys, as `(name, type)`.
    partition_keys: Vec<(&'static str, &'static str)>,
    /// The table properties it must carry beyond `EXTERNAL`.
    properties: Vec<(&'static str, &'static str)>,
    /// The SerDe parameters it must carry.
    serde_parameters: Vec<(&'static str, &'static str)>,
    /// The input format, output format and serialization library it must carry.
    storage: (&'static str, &'static str, &'static str),
}

/// Hive DDL submitted to Athena REGISTERS the table it declares in the Data Catalog: the
/// execution succeeds as a DDL statement and Glue's own read reports an external table
/// owned by the DDL engine, with the declared columns, the declared properties and the
/// location the statement named without the separator that closed it.
#[tokio::test]
async fn hive_ddl_registers_the_table_it_declares_in_the_data_catalog() {
    let router = emulator();
    create_bucket(&router).await;
    for database in ["shop", "warehouse"] {
        create_database(&router, database).await;
    }

    // The rows differ along how the name is qualified and quoted, which columns and types
    // are declared, whether a row format, a storage format or table properties are
    // declared, whether the table is partitioned, and whether the location closes with a
    // separator.
    let cases: Vec<DdlCase> = vec![
        DdlCase {
            case: "a back-quoted qualified name and a location that closes with a separator",
            database: "warehouse",
            sql: "CREATE EXTERNAL TABLE `shop`.`ddl_tbl` (id bigint) \
                  LOCATION 's3://athena-catalog-data/ddl/'",
            table: ("shop", "ddl_tbl"),
            location: "s3://athena-catalog-data/ddl",
            columns: vec![("id", "bigint")],
            partition_keys: vec![],
            properties: vec![],
            serde_parameters: vec![],
            storage: (
                ddl::TEXT_INPUT_FORMAT,
                ddl::TEXT_OUTPUT_FORMAT,
                ddl::LAZY_SIMPLE_SERDE,
            ),
        },
        DdlCase {
            case: "an unqualified name, resolved against the context database",
            database: "shop",
            sql: "CREATE EXTERNAL TABLE events (id int, name string, amount double, \
                  paid boolean) LOCATION 's3://athena-catalog-data/events'",
            table: ("shop", "events"),
            location: "s3://athena-catalog-data/events",
            columns: vec![
                ("id", "int"),
                ("name", "string"),
                ("amount", "double"),
                ("paid", "boolean"),
            ],
            partition_keys: vec![],
            properties: vec![],
            serde_parameters: vec![],
            storage: (
                ddl::TEXT_INPUT_FORMAT,
                ddl::TEXT_OUTPUT_FORMAT,
                ddl::LAZY_SIMPLE_SERDE,
            ),
        },
        DdlCase {
            case: "a declared row format, storage format and table properties",
            database: "shop",
            sql: "CREATE EXTERNAL TABLE IF NOT EXISTS shop.logs (line string) \
                  ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' STORED AS TEXTFILE \
                  LOCATION 's3://athena-catalog-data/logs/' \
                  TBLPROPERTIES ('skip.header.line.count' = '1', 'classification' = 'csv')",
            table: ("shop", "logs"),
            location: "s3://athena-catalog-data/logs",
            columns: vec![("line", "string")],
            partition_keys: vec![],
            properties: vec![
                ("skip.header.line.count", "1"),
                ("classification", "csv"),
            ],
            // A declared field separator is recorded in BOTH parameters Hive reads one
            // back from.
            serde_parameters: vec![("field.delim", ","), ("serialization.format", ",")],
            storage: (
                ddl::TEXT_INPUT_FORMAT,
                ddl::TEXT_OUTPUT_FORMAT,
                ddl::LAZY_SIMPLE_SERDE,
            ),
        },
        DdlCase {
            case: "a partitioned table in a double-quoted qualified name",
            database: "shop",
            sql: "CREATE EXTERNAL TABLE \"warehouse\".\"daily\" (total double) \
                  PARTITIONED BY (day string) \
                  LOCATION 's3://athena-catalog-data/daily/'",
            table: ("warehouse", "daily"),
            location: "s3://athena-catalog-data/daily",
            columns: vec![("total", "double")],
            partition_keys: vec![("day", "string")],
            properties: vec![],
            serde_parameters: vec![],
            storage: (
                ddl::TEXT_INPUT_FORMAT,
                ddl::TEXT_OUTPUT_FORMAT,
                ddl::LAZY_SIMPLE_SERDE,
            ),
        },
        DdlCase {
            case: "a table stored in a columnar format",
            database: "shop",
            sql: "CREATE EXTERNAL TABLE shop.columnar (id bigint) STORED AS PARQUET \
                  LOCATION 's3://athena-catalog-data/columnar/'",
            table: ("shop", "columnar"),
            location: "s3://athena-catalog-data/columnar",
            columns: vec![("id", "bigint")],
            partition_keys: vec![],
            properties: vec![],
            serde_parameters: vec![],
            // The declared storage format is what the catalog records — never the format
            // of a statement that declared another one.
            storage: (
                ddl::PARQUET_INPUT_FORMAT,
                ddl::PARQUET_OUTPUT_FORMAT,
                ddl::PARQUET_SERDE,
            ),
        },
        DdlCase {
            case: "a table stored in the optimised row-columnar format",
            database: "shop",
            sql: "CREATE EXTERNAL TABLE shop.rows_columns (id bigint) STORED AS ORC \
                  LOCATION 's3://athena-catalog-data/orc/'",
            table: ("shop", "rows_columns"),
            location: "s3://athena-catalog-data/orc",
            columns: vec![("id", "bigint")],
            partition_keys: vec![],
            properties: vec![],
            serde_parameters: vec![],
            storage: (ddl::ORC_INPUT_FORMAT, ddl::ORC_OUTPUT_FORMAT, ddl::ORC_SERDE),
        },
    ];

    let mut started: Vec<(&DdlCase, String)> = Vec::new();
    for case in &cases {
        let id = start(&router, case.case, case.database, case.sql).await;
        started.push((case, id));
    }
    settle().await;

    for (case, id) in started {
        let found = succeeded(&router, case.case, &id).await;
        assert_eq!(
            found["StatementType"].as_str(),
            Some(query_engine::STATEMENT_TYPE_DDL),
            "case {:?}: a statement that defines an object is a DDL statement, got {found:?}",
            case.case
        );
        let (database, name) = case.table;
        let table = glue_table(&router, database, name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the Data Catalog must hold {database}.{name} after the DDL",
                    case.case
                )
            });
        assert_eq!(
            table["TableType"].as_str(),
            Some(ddl::TABLE_TYPE_EXTERNAL),
            "case {:?}: unexpected table type in {table:?}",
            case.case
        );
        assert_eq!(
            table["Owner"].as_str(),
            Some(ddl::DDL_TABLE_OWNER),
            "case {:?}: unexpected owner in {table:?}",
            case.case
        );
        assert_eq!(
            table["Parameters"][ddl::EXTERNAL_PROPERTY].as_str(),
            Some(ddl::EXTERNAL_PROPERTY_VALUE),
            "case {:?}: an external table is marked as one, got {table:?}",
            case.case
        );
        for (property, value) in &case.properties {
            assert_eq!(
                table["Parameters"][*property].as_str(),
                Some(*value),
                "case {:?}: the declared property {property} must be stored, got {table:?}",
                case.case
            );
        }
        assert_eq!(
            table["StorageDescriptor"]["Location"].as_str(),
            Some(case.location),
            "case {:?}: unexpected location in {table:?}",
            case.case
        );
        assert_eq!(
            columns_of(&table["StorageDescriptor"]["Columns"]),
            case.columns
                .iter()
                .map(|(name, data_type)| ((*name).to_string(), (*data_type).to_string()))
                .collect::<Vec<_>>(),
            "case {:?}: unexpected columns in {table:?}",
            case.case
        );
        assert_eq!(
            columns_of(&table["PartitionKeys"]),
            case.partition_keys
                .iter()
                .map(|(name, data_type)| ((*name).to_string(), (*data_type).to_string()))
                .collect::<Vec<_>>(),
            "case {:?}: unexpected partition keys in {table:?}",
            case.case
        );
        assert_eq!(
            table["StorageDescriptor"]["SerdeInfo"]["SerializationLibrary"].as_str(),
            Some(case.storage.2),
            "case {:?}: the serialization library is the declared storage format's, got \
             {table:?}",
            case.case
        );
        assert_eq!(
            (
                table["StorageDescriptor"]["InputFormat"].as_str(),
                table["StorageDescriptor"]["OutputFormat"].as_str(),
            ),
            (Some(case.storage.0), Some(case.storage.1)),
            "case {:?}: the formats are the declared storage format's, got {table:?}",
            case.case
        );
        for (parameter, value) in &case.serde_parameters {
            assert_eq!(
                table["StorageDescriptor"]["SerdeInfo"]["Parameters"][*parameter].as_str(),
                Some(*value),
                "case {:?}: the declared serde parameter {parameter} must be stored, got \
                 {table:?}",
                case.case
            );
        }
        assert_eq!(
            table["StorageDescriptor"]["NumberOfBuckets"].as_i64(),
            Some(-1),
            "case {:?}: a table the DDL registers declares no bucketing, got {table:?}",
            case.case
        );

        // The entry is readable through Athena's own metadata surface too: one catalog,
        // so what the DDL wrote is what Athena reports.
        let (status, metadata) = athena(
            &router,
            "GetTableMetadata",
            &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": database, "TableName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetTableMetadata must answer 200, got {status} with {metadata:?}",
            case.case
        );
        assert_eq!(
            metadata["TableMetadata"]["TableType"].as_str(),
            Some(ddl::TABLE_TYPE_EXTERNAL),
            "case {:?}: unexpected metadata in {metadata:?}",
            case.case
        );
    }
}

/// The stored Glue database `name`, or `None` when the catalog holds none.
async fn glue_database(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, answer) = glue(router, "GetDatabase", &json!({ "Name": name })).await;
    (status == 200).then(|| answer["Database"].clone())
}

/// One row of the database-DDL table: a statement that DEFINES a database, and the
/// catalog record it must leave.
struct DatabaseCase {
    /// What this row is about.
    case: &'static str,
    /// The database the statement runs in, which an unqualified name resolves against.
    context: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// The folded name the catalog must then hold the database under.
    name: &'static str,
    /// The `LocationUri` it must carry.
    location: Option<&'static str>,
}

/// A statement that DEFINES a database writes the Data Catalog's own database record, so
/// every catalog read of it answers: `GetDatabase` reports the record with the catalog's
/// identity, its assigned id, its create instant and the Lake Formation grant the catalog
/// seeds, and `GetTables` lists the (so far empty) set of tables it holds rather than
/// refusing a database it does not know.
#[tokio::test]
async fn database_ddl_writes_the_catalog_database_glue_reads_report() {
    let router = emulator();
    let account = ::engine::emulator_account_id();

    // The rows differ along the grammar word the statement uses for the object, whether it
    // tolerates an existing database, how the name is cased and quoted, whether the name
    // is qualified by its catalog, and whether a location is declared.
    let cases: Vec<DatabaseCase> = vec![
        DatabaseCase {
            case: "a bare CREATE DATABASE",
            context: "ddl_shop",
            sql: "CREATE DATABASE ddl_shop",
            name: "ddl_shop",
            location: None,
        },
        DatabaseCase {
            case: "a SCHEMA, which names the same catalog object",
            context: "ddl_shop",
            sql: "CREATE SCHEMA ddl_depot",
            name: "ddl_depot",
            location: None,
        },
        DatabaseCase {
            case: "a mixed-case name, folded the way the catalog folds one",
            context: "ddl_shop",
            sql: "CREATE DATABASE DDL_Mixed",
            name: "ddl_mixed",
            location: None,
        },
        DatabaseCase {
            case: "a back-quoted name under IF NOT EXISTS",
            context: "ddl_shop",
            sql: "CREATE DATABASE IF NOT EXISTS `ddl_quoted`",
            name: "ddl_quoted",
            location: None,
        },
        DatabaseCase {
            case: "a name whose hyphens oblige the statement to quote it",
            context: "ddl_shop",
            sql: "CREATE DATABASE `ddl-hyphen-name`",
            name: "ddl-hyphen-name",
            location: None,
        },
        DatabaseCase {
            case: "a database whose location the statement declares",
            context: "ddl_shop",
            sql: "CREATE DATABASE ddl_located LOCATION 's3://athena-catalog-data/located/'",
            name: "ddl_located",
            // A location names a PREFIX, which the catalog records without the separator
            // that closed it.
            location: Some("s3://athena-catalog-data/located"),
        },
    ];

    let mut started: Vec<(&DatabaseCase, String)> = Vec::new();
    for case in &cases {
        let id = start(&router, case.case, case.context, case.sql).await;
        started.push((case, id));
    }
    settle().await;

    let mut seen_ids: BTreeMap<String, &str> = BTreeMap::new();
    for (case, id) in started {
        let found = succeeded(&router, case.case, &id).await;
        assert_eq!(
            found["StatementType"].as_str(),
            Some(query_engine::STATEMENT_TYPE_DDL),
            "case {:?}: a statement that defines an object is a DDL statement, got {found:?}",
            case.case
        );
        let database = glue_database(&router, case.name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the Data Catalog must hold {} after the DDL",
                    case.case, case.name
                )
            });
        assert_eq!(
            database["Name"].as_str(),
            Some(case.name),
            "case {:?}: unexpected name in {database:?}",
            case.case
        );
        assert_eq!(
            (
                database["CatalogId"].as_str(),
                database["CatalogIdentifier"].as_str(),
            ),
            (Some(account), Some(account)),
            "case {:?}: the record names the catalog that owns it, got {database:?}",
            case.case
        );
        assert!(
            database["CreateTime"].as_f64().is_some(),
            "case {:?}: a stored database carries its create instant, got {database:?}",
            case.case
        );
        assert_eq!(
            database["CreateTableDefaultPermissions"],
            json!([ {
                "__type": "PrincipalPermissions",
                "Permissions": [ "ALL" ],
                "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            } ]),
            "case {:?}: the catalog seeds the Lake Formation default grant, got {database:?}",
            case.case
        );
        assert_eq!(
            database["LocationUri"].as_str(),
            case.location,
            "case {:?}: unexpected location in {database:?}",
            case.case
        );
        let assigned = database["DatabaseId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a stored database carries the id the catalog assigned it, \
                     got {database:?}",
                    case.case
                )
            })
            .to_string();
        if let Some(owner) = seen_ids.insert(assigned.clone(), case.case) {
            let case = case.case;
            panic!(
                "case {case:?}: the catalog assigns each database its own id, but \
                 {assigned:?} is already {owner:?}'s"
            );
        }

        // A database the DDL created holds no tables yet, which a listing REPORTS as an
        // empty list — the answer that tells "no tables" from "no database".
        let listed = glue_ok(
            &router,
            case.case,
            "GetTables",
            &json!({ "DatabaseName": case.name }),
        )
        .await;
        assert_eq!(
            listed["TableList"],
            json!([]),
            "case {:?}: a database with no tables lists an empty TableList, got {listed:?}",
            case.case
        );
    }

    // The control: a database no statement defined is still absent, so the reads above
    // report what the DDL wrote and not every name asked for.
    assert!(
        glue_database(&router, "ddl_never_created").await.is_none(),
        "a database no statement created must not be in the catalog"
    );
}

/// A statement that defines a database the catalog ALREADY holds is answered by the
/// statement's own words: `IF NOT EXISTS` succeeds and changes nothing, and without it the
/// statement fails with the already-exists class Athena publishes — the same refusal the
/// catalog API answers for that name, because both doors write one record.
#[tokio::test]
async fn a_database_statement_over_one_the_catalog_holds_is_kept_or_refused() {
    let router = emulator();

    let created = start(
        &router,
        "the first create",
        "ddl_twice",
        "CREATE DATABASE ddl_twice",
    )
    .await;
    let before = glue_database(&router, "ddl_twice")
        .await
        .expect("the first create must store the database");

    // (case, the statement, whether it must succeed)
    let cases: Vec<(&str, &str, bool)> = vec![
        ("a plain create of a database that exists", "CREATE DATABASE ddl_twice", false),
        (
            "a create that tolerates an existing database",
            "CREATE DATABASE IF NOT EXISTS ddl_twice",
            true,
        ),
        (
            "a SCHEMA naming the database a CREATE DATABASE defined",
            "CREATE SCHEMA ddl_twice",
            false,
        ),
    ];

    let mut started: Vec<(&str, bool, String)> = Vec::new();
    for (case, sql, succeeds) in &cases {
        started.push((case, *succeeds, start(&router, case, "ddl_twice", sql).await));
    }
    settle().await;

    succeeded(&router, "the first create", &created).await;
    for (case, succeeds, id) in started {
        if succeeds {
            succeeded(&router, case, &id).await;
            continue;
        }
        let message = failed(&router, case, &id, 2, Some(1006)).await;
        assert!(
            message.contains("Database ddl_twice already exists"),
            "case {case:?}: expected the metastore's already-exists sentence, got {message:?}"
        );
    }

    // Neither answer rewrote the record: the id the catalog assigned it is the one it
    // still reports.
    let after = glue_database(&router, "ddl_twice")
        .await
        .expect("the database is still stored");
    assert_eq!(
        after["DatabaseId"], before["DatabaseId"],
        "a refused or tolerated create leaves the stored database as it was, got {after:?} \
         against {before:?}"
    );

    // The catalog API refuses that name for the same reason: one record, one name.
    let (status, refused) = glue(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "ddl_twice" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a catalog create of a database the DDL defined is refused, got {status} with \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "AlreadyExistsException",
        "expected the catalog's already-exists exception, got {refused:?}"
    );
}

/// One row of the drop table: a statement that DROPS a catalog object, and what it must
/// answer.
struct DropCase {
    /// What this row is about.
    case: &'static str,
    /// The database the statement runs in, which an unqualified name resolves against.
    context: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// `None` when the statement must succeed, else the fragment its refusal must carry
    /// and the error type Athena publishes for that refusal's class.
    refusal: Option<(&'static str, Option<i64>)>,
}

/// A statement that DROPS a catalog object removes it from the Data Catalog, so no reader
/// is left holding what the other dropped: a dropped table is gone from `GetTable` and from
/// the partition-index records kept beside it, a dropped database is gone from
/// `GetDatabase` — and from `DeleteDatabase`, which then refuses it as an absent entity — and
/// a `CASCADE` takes the tables of a database with it. A `DROP TABLE` naming a database the
/// catalog holds no record of addresses a table of the engine's own and is performed where
/// that table's create was, and one naming a VIEW addresses no table at all.
#[tokio::test]
async fn dropping_a_catalog_object_removes_it_from_the_catalog() {
    let router = emulator();
    create_bucket(&router).await;

    // Everything these drops address is created through the DDL, so what is dropped is
    // what that DDL wrote.
    for (case, sql) in [
        ("the empty database", "CREATE DATABASE ddl_empty"),
        ("the database whose table is dropped", "CREATE DATABASE ddl_full"),
        ("the database dropped with its tables", "CREATE DATABASE ddl_cascade"),
        ("the database the catalog API deletes", "CREATE DATABASE ddl_glue"),
        ("the database holding a view", "CREATE DATABASE ddl_views"),
    ] {
        start(&router, case, "ddl_empty", sql).await;
    }
    for (case, database) in [
        ("the table that is dropped", "ddl_full"),
        ("the table dropped with its database", "ddl_cascade"),
    ] {
        start(
            &router,
            case,
            database,
            &format!(
                "CREATE EXTERNAL TABLE {database}.items (id bigint) \
                 LOCATION 's3://{BUCKET}/items/'"
            ),
        )
        .await;
    }

    // A VIEW, which a `DROP TABLE` must leave alone: the two statements address different
    // kinds of catalog entry.
    start(
        &router,
        "the view a DROP TABLE must not remove",
        "ddl_views",
        "CREATE VIEW ddl_views.v_items AS SELECT 1 AS id",
    )
    .await;

    // Both tables are in the catalog before anything is dropped, so the assertions below
    // are about the drops and not about a table that was never registered.
    for database in ["ddl_full", "ddl_cascade"] {
        assert!(
            glue_table(&router, database, "items").await.is_some(),
            "setup: the Data Catalog must hold {database}.items after the DDL"
        );
    }

    // A table of the ENGINE's own, in a database the catalog holds no record of: a
    // `CREATE TABLE` that declares no location is not a catalog statement, so the drop of
    // it below is answered where its create was.
    let engine_table = start(
        &router,
        "the table the engine itself defines",
        "engine_only",
        "CREATE TABLE engine_tbl (id integer)",
    )
    .await;

    // The rows differ along which kind of object the statement drops, whether it tolerates
    // its absence, whether it states CASCADE, and whether the name it addresses is one the
    // catalog holds at all.
    let cases: Vec<DropCase> = vec![
        DropCase {
            case: "a table the catalog holds",
            context: "ddl_full",
            sql: "DROP TABLE ddl_full.items",
            refusal: None,
        },
        DropCase {
            case: "an empty database",
            context: "ddl_full",
            sql: "DROP DATABASE ddl_empty",
            refusal: None,
        },
        DropCase {
            case: "a database dropped with the tables it holds",
            context: "ddl_full",
            sql: "DROP DATABASE ddl_cascade CASCADE",
            refusal: None,
        },
        DropCase {
            case: "a SCHEMA naming a database the catalog does not hold",
            context: "ddl_full",
            sql: "DROP SCHEMA ddl_absent",
            refusal: Some(("Database does not exist: ddl_absent", Some(1301))),
        },
        DropCase {
            case: "an absent database the statement tolerates",
            context: "ddl_full",
            sql: "DROP DATABASE IF EXISTS ddl_absent",
            refusal: None,
        },
        DropCase {
            case: "a view, which a DROP TABLE addresses no table of",
            context: "ddl_views",
            sql: "DROP TABLE ddl_views.v_items",
            refusal: Some((
                "Table 'awsdatacatalog.ddl_views.v_items' does not exist",
                Some(1301),
            )),
        },
        DropCase {
            case: "an absent table the statement tolerates",
            context: "ddl_full",
            sql: "DROP TABLE IF EXISTS ddl_full.items",
            refusal: None,
        },
        DropCase {
            case: "an absent table in a database the catalog holds",
            context: "ddl_full",
            sql: "DROP TABLE ddl_full.never_registered",
            refusal: Some((
                "Table 'awsdatacatalog.ddl_full.never_registered' does not exist",
                Some(1301),
            )),
        },
        DropCase {
            case: "a table of the engine's own, in no catalog database",
            context: "engine_only",
            sql: "DROP TABLE engine_tbl",
            refusal: None,
        },
    ];

    let mut started: Vec<(&DropCase, String)> = Vec::new();
    for case in &cases {
        let id = start(&router, case.case, case.context, case.sql).await;
        started.push((case, id));
    }
    settle().await;

    // The engine's own create and drop both succeed, and leave the catalog with no record
    // of either the database or the table.
    succeeded(&router, "the table the engine itself defines", &engine_table).await;
    assert!(
        glue_database(&router, "engine_only").await.is_none(),
        "a table the engine defines registers no catalog database"
    );

    for (case, id) in started {
        let Some((expected, error_type)) = case.refusal else {
            succeeded(&router, case.case, &id).await;
            continue;
        };
        let message = failed(&router, case.case, &id, 2, error_type).await;
        assert!(
            message.contains(expected),
            "case {:?}: expected a refusal carrying {expected:?}, got {message:?}",
            case.case
        );
    }

    assert!(
        glue_table(&router, "ddl_full", "items").await.is_none(),
        "a dropped table is gone from the catalog"
    );
    // The database that held it is untouched, and still lists its tables — now none.
    let listed = glue_ok(
        &router,
        "the database whose table was dropped",
        "GetTables",
        &json!({ "DatabaseName": "ddl_full" }),
    )
    .await;
    assert_eq!(
        listed["TableList"],
        json!([]),
        "the database of a dropped table lists no tables, got {listed:?}"
    );
    for database in ["ddl_empty", "ddl_cascade"] {
        assert!(
            glue_database(&router, database).await.is_none(),
            "the dropped database {database} is gone from the catalog"
        );
    }
    assert!(
        glue_table(&router, "ddl_cascade", "items").await.is_none(),
        "the tables of a database dropped with CASCADE go with it"
    );
    // The VIEW the refused DROP TABLE named is still there: that statement addresses
    // tables, and `DROP VIEW` is what addresses a view.
    assert!(
        glue_table(&router, "ddl_views", "v_items").await.is_some(),
        "a DROP TABLE naming a view leaves the view in the catalog"
    );

    // A delete of a database the catalog no longer holds is refused as an absent entity,
    // so a client can tell a real delete from a no-op.
    let (status, refused) = glue(&router, "DeleteDatabase", &json!({ "Name": "ddl_empty" })).await;
    assert_eq!(
        status, 400,
        "deleting a database the DDL dropped is refused, got {status} with {refused:?}"
    );
    assert_eq!(
        (
            error_code(&refused),
            refused["Message"].as_str().unwrap_or_default(),
        ),
        (
            "EntityNotFoundException".to_string(),
            "Database ddl_empty not found.",
        ),
        "expected the catalog's absent-entity refusal, got {refused:?}"
    );

    // And the catalog API deletes a database the DDL created, exactly as it deletes one
    // of its own: the first delete succeeds, the second finds nothing.
    glue_ok(
        &router,
        "the database the catalog API deletes",
        "DeleteDatabase",
        &json!({ "Name": "ddl_glue" }),
    )
    .await;
    assert!(
        glue_database(&router, "ddl_glue").await.is_none(),
        "a database the catalog API deleted is gone"
    );
    let (status, again) = glue(&router, "DeleteDatabase", &json!({ "Name": "ddl_glue" })).await;
    assert_eq!(
        status, 400,
        "a second delete of the same database is refused, got {status} with {again:?}"
    );
}

/// `SHOW DATABASES` and `SHOW SCHEMAS` report the databases the Data Catalog holds, in the
/// catalog's own name order, whichever door created each one — and a statement that states
/// a pattern lists only the names it matches as a whole.
#[tokio::test]
async fn show_databases_lists_the_catalog_databases() {
    let router = emulator();
    create_database(&router, "show_api_db").await;
    for sql in [
        "CREATE DATABASE show_ddl_one",
        "CREATE SCHEMA show_ddl_two",
    ] {
        start(&router, "setup", "show_ddl_one", sql).await;
    }

    // (case, the statement, the names it must list)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "every database the catalog holds",
            "SHOW DATABASES",
            vec!["show_api_db", "show_ddl_one", "show_ddl_two"],
        ),
        (
            "the same listing under the SCHEMAS spelling",
            "SHOW SCHEMAS",
            vec!["show_api_db", "show_ddl_one", "show_ddl_two"],
        ),
        (
            "a listing narrowed by a pattern",
            "SHOW DATABASES LIKE 'show_ddl%'",
            vec!["show_ddl_one", "show_ddl_two"],
        ),
        (
            "a pattern no database matches",
            "SHOW DATABASES LIKE 'nothing%'",
            vec![],
        ),
    ];

    let mut started: Vec<(&str, &Vec<&str>, String)> = Vec::new();
    for (case, sql, expected) in &cases {
        started.push((case, expected, start(&router, case, "show_ddl_one", sql).await));
    }
    settle().await;

    for (case, expected, id) in started {
        succeeded(&router, case, &id).await;
        let listed = names_in(&rows_of(&router, case, &id).await);
        assert_eq!(
            listed,
            expected.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the databases {expected:?}, got {listed:?}"
        );
    }
}

/// The `(name, type)` pairs a column list carries.
fn columns_of(columns: &Value) -> Vec<(String, String)> {
    columns
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|column| {
            (
                column["Name"].as_str().unwrap_or_default().to_string(),
                column["Type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A `CREATE EXTERNAL TABLE` over a table the catalog ALREADY holds is answered by the
/// statement's own words: `IF NOT EXISTS` succeeds and changes nothing, and without it the
/// statement fails with the already-exists class Athena publishes and leaves the stored
/// entry exactly as it was.
#[tokio::test]
async fn ddl_over_a_table_the_catalog_holds_is_kept_or_refused() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;

    // The table is created through GLUE's own API first, with a definition this DDL does
    // not declare, so what "changes nothing" means is checkable.
    glue_ok(
        &router,
        "setup",
        "CreateTable",
        &json!({
            "DatabaseName": "shop",
            "TableInput": {
                "Name": "api_tbl",
                "TableType": "EXTERNAL_TABLE",
                "Owner": "analytics",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "bigint" } ],
                    "Location": "s3://athena-catalog-data/api",
                },
            },
        }),
    )
    .await;
    let before = glue_table(&router, "shop", "api_tbl")
        .await
        .expect("setup: the Glue table must be there");

    let tolerant = start(
        &router,
        "if not exists over an existing table",
        "shop",
        "CREATE EXTERNAL TABLE IF NOT EXISTS shop.api_tbl (other string) \
         LOCATION 's3://athena-catalog-data/ddl/'",
    )
    .await;
    let plain = start(
        &router,
        "a plain create over an existing table",
        "shop",
        "CREATE EXTERNAL TABLE shop.api_tbl (other string) \
         LOCATION 's3://athena-catalog-data/ddl/'",
    )
    .await;
    let absent_database = start(
        &router,
        "a create in a database the catalog does not hold",
        "shop",
        "CREATE EXTERNAL TABLE nowhere.api_tbl (id bigint) \
         LOCATION 's3://athena-catalog-data/ddl/'",
    )
    .await;
    settle().await;

    succeeded(&router, "if not exists over an existing table", &tolerant).await;
    failed(
        &router,
        "a plain create over an existing table",
        &plain,
        2,
        Some(1006),
    )
    .await;
    let message = failed(
        &router,
        "a create in a database the catalog does not hold",
        &absent_database,
        2,
        Some(1301),
    )
    .await;
    assert!(
        message.contains("nowhere"),
        "case {:?}: the refusal must name the database it could not resolve, got \
         {message:?}",
        "a create in a database the catalog does not hold"
    );

    let after = glue_table(&router, "shop", "api_tbl")
        .await
        .expect("the stored table must still be there");
    assert_eq!(
        after["Owner"], before["Owner"],
        "neither statement may touch the stored entry, got {after:?}"
    );
    assert_eq!(
        after["StorageDescriptor"], before["StorageDescriptor"],
        "neither statement may touch the stored definition, got {after:?}"
    );
}

/// Presto view DDL writes a VIEW entry in the Data Catalog, the statements that report the
/// catalog classify it as one, and every statement that addresses a view fails with the
/// published view class when what it names is not the view it needs.
#[tokio::test]
async fn view_ddl_writes_a_view_entry_and_the_catalog_classifies_it() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;
    glue_ok(
        &router,
        "setup",
        "CreateTable",
        &json!({
            "DatabaseName": "shop",
            "TableInput": {
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "bigint" },
                        { "Name": "name", "Type": "string" },
                        { "Name": "created", "Type": "timestamp" },
                    ],
                    "Location": "s3://athena-catalog-data/events",
                },
            },
        }),
    )
    .await;
    put_object(
        &router,
        "setup",
        "events/rows.txt",
        &format!(
            "1{UNIT_SEPARATOR}desk{UNIT_SEPARATOR}2024-01-02 03:04:05\n\
             2{UNIT_SEPARATOR}lamp{UNIT_SEPARATOR}2024-02-03 04:05:06\n\
             3{UNIT_SEPARATOR}rug{UNIT_SEPARATOR}2024-03-04 05:06:07\n"
        ),
    )
    .await;

    // Started in order, so each statement sees what the one before it left: the view is
    // defined, then addressed by every statement that reports or drops one.
    let created = start(
        &router,
        "a view over an existing table",
        "shop",
        "CREATE VIEW \"shop\".\"v_recent\" AS SELECT id, name FROM \"shop\".\"events\" \
         WHERE id > 1",
    )
    .await;
    let again = start(
        &router,
        "a plain create over an existing view",
        "shop",
        "CREATE VIEW \"shop\".\"v_recent\" AS SELECT id FROM \"shop\".\"events\"",
    )
    .await;
    let replaced = start(
        &router,
        "a replace over an existing view",
        "shop",
        "CREATE OR REPLACE VIEW \"shop\".\"v_recent\" AS SELECT id, name FROM \
         \"shop\".\"events\" WHERE id > 1",
    )
    .await;
    let over_absent = start(
        &router,
        "a view over a table the catalog does not hold",
        "shop",
        "CREATE VIEW shop.v_absent AS SELECT id FROM shop.not_a_table",
    )
    .await;
    let dropping_a_table = start(
        &router,
        "dropping a view that is a table",
        "shop",
        "DROP VIEW shop.events",
    )
    .await;
    let showing_a_table = start(
        &router,
        "the definition of a view that is a table",
        "shop",
        "SHOW CREATE VIEW shop.events",
    )
    .await;
    let showing_the_view = start(
        &router,
        "the definition of the view",
        "shop",
        "SHOW CREATE VIEW shop.v_recent",
    )
    .await;
    let show_tables = start(&router, "the tables of the database", "shop", "SHOW TABLES")
        .await;
    let show_views = start(&router, "the views of the database", "shop", "SHOW VIEWS").await;
    let narrowed_like = start(
        &router,
        "the relations a SQL pattern narrows to",
        "shop",
        "SHOW TABLES LIKE 'v%'",
    )
    .await;
    let narrowed_pattern = start(
        &router,
        "the relations the catalog's own pattern narrows to",
        "shop",
        "SHOW TABLES IN shop 'ev*'",
    )
    .await;
    let classified = start(
        &router,
        "the catalog's own classification",
        "shop",
        "SELECT table_name, table_type FROM information_schema.tables \
         WHERE table_schema = 'shop' ORDER BY table_name",
    )
    .await;
    let read_through_the_view = start(
        &router,
        "the rows the view projects",
        "shop",
        "SELECT count(*) FROM shop.v_recent",
    )
    .await;
    settle().await;

    let found = succeeded(&router, "a view over an existing table", &created).await;
    assert_eq!(
        found["StatementType"].as_str(),
        Some(query_engine::STATEMENT_TYPE_DDL),
        "case {:?}: a view definition is a DDL statement, got {found:?}",
        "a view over an existing table"
    );
    assert_eq!(
        found["SubstatementType"].as_str(),
        Some("CREATE_VIEW"),
        "case {:?}: unexpected substatement in {found:?}",
        "a view over an existing table"
    );

    let view = glue_table(&router, "shop", "v_recent")
        .await
        .expect("the Data Catalog must hold the view the DDL defined");
    assert_eq!(
        view["TableType"].as_str(),
        Some(ddl::TABLE_TYPE_VIEW),
        "a view entry carries the view table type, got {view:?}"
    );
    assert_eq!(
        view["ViewExpandedText"].as_str(),
        Some(ddl::PRESTO_VIEW_EXPANDED_TEXT),
        "unexpected expanded text in {view:?}"
    );
    assert_eq!(
        view["Parameters"][ddl::PRESTO_VIEW_PROPERTY].as_str(),
        Some(ddl::PRESTO_VIEW_PROPERTY_VALUE),
        "a view entry says which engine defined it, got {view:?}"
    );
    assert_eq!(
        view["StorageDescriptor"]["Location"].as_str(),
        Some(""),
        "a view holds no data, so its location is empty, got {view:?}"
    );
    assert!(
        view["ViewOriginalText"]
            .as_str()
            .is_some_and(|text| ddl::view_definition_sql(text).contains("WHERE (id > 1)")),
        "a view entry carries the query it was defined by, as the engine renders it, got \
         {view:?}"
    );
    assert_eq!(
        columns_of(&view["StorageDescriptor"]["Columns"]),
        vec![
            ("id".to_string(), "bigint".to_string()),
            ("name".to_string(), "string".to_string()),
        ],
        "a view's columns are the ones its query PROJECTS, with the types the source \
         columns carry, got {view:?}"
    );

    failed(
        &router,
        "a plain create over an existing view",
        &again,
        1,
        Some(1502),
    )
    .await;
    succeeded(&router, "a replace over an existing view", &replaced).await;
    failed(
        &router,
        "a view over a table the catalog does not hold",
        &over_absent,
        2,
        Some(1301),
    )
    .await;
    assert!(
        glue_table(&router, "shop", "v_absent").await.is_none(),
        "a view the statement could not define must not be stored"
    );
    failed(
        &router,
        "dropping a view that is a table",
        &dropping_a_table,
        1,
        Some(1502),
    )
    .await;
    assert!(
        glue_table(&router, "shop", "events").await.is_some(),
        "a refused drop must leave the table it named"
    );
    failed(
        &router,
        "the definition of a view that is a table",
        &showing_a_table,
        1,
        Some(1502),
    )
    .await;

    succeeded(&router, "the definition of the view", &showing_the_view).await;
    let definition = only_value(
        "the definition of the view",
        &rows_of(&router, "the definition of the view", &showing_the_view).await,
    );
    assert!(
        definition.contains("CREATE VIEW") && definition.contains("events"),
        "case {:?}: expected the statement the view was defined by, got {definition:?}",
        "the definition of the view"
    );
    assert!(
        definition.contains("WHERE (id > 1)"),
        "case {:?}: the definition is rendered from the engine's own parse, where a \
         predicate is an expression, got {definition:?}",
        "the definition of the view"
    );

    succeeded(&router, "the tables of the database", &show_tables).await;
    let listed = names_in(&rows_of(&router, "the tables of the database", &show_tables).await);
    assert_eq!(
        listed,
        vec!["events".to_string(), "v_recent".to_string()],
        "every relation of the database is listed, got {listed:?}"
    );
    succeeded(&router, "the views of the database", &show_views).await;
    let views = names_in(&rows_of(&router, "the views of the database", &show_views).await);
    assert_eq!(
        views,
        vec!["v_recent".to_string()],
        "only the views are listed, got {views:?}"
    );

    // A statement that gives a PATTERN narrows its answer to the names that match it.
    succeeded(
        &router,
        "the relations a SQL pattern narrows to",
        &narrowed_like,
    )
    .await;
    let narrowed = names_in(
        &rows_of(
            &router,
            "the relations a SQL pattern narrows to",
            &narrowed_like,
        )
        .await,
    );
    assert_eq!(
        narrowed,
        vec!["v_recent".to_string()],
        "a filtered listing reports only the names the pattern matches, got {narrowed:?}"
    );
    succeeded(
        &router,
        "the relations the catalog's own pattern narrows to",
        &narrowed_pattern,
    )
    .await;
    let narrowed = names_in(
        &rows_of(
            &router,
            "the relations the catalog's own pattern narrows to",
            &narrowed_pattern,
        )
        .await,
    );
    assert_eq!(
        narrowed,
        vec!["events".to_string()],
        "a filtered listing reports only the names the pattern matches, got {narrowed:?}"
    );

    succeeded(&router, "the catalog's own classification", &classified).await;
    let rows = rows_of(&router, "the catalog's own classification", &classified).await;
    let classification: BTreeMap<String, String> = rows
        .iter()
        .filter_map(|row| match row.as_slice() {
            [Some(name), Some(kind)] => Some((name.clone(), kind.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        classification,
        BTreeMap::from([
            ("events".to_string(), "BASE TABLE".to_string()),
            ("v_recent".to_string(), "VIEW".to_string()),
        ]),
        "the catalog classifies a table and a view, got {classification:?}"
    );

    succeeded(&router, "the rows the view projects", &read_through_the_view).await;
    let counted = only_value(
        "the rows the view projects",
        &rows_of(&router, "the rows the view projects", &read_through_the_view).await,
    );
    assert_eq!(
        counted, "2",
        "the view's rows are the ones its query selects, got {counted:?}"
    );

    // Dropping the view itself takes it out of the catalog for every later reader.
    let dropped = start(
        &router,
        "dropping the view",
        "shop",
        "DROP VIEW shop.v_recent",
    )
    .await;
    settle().await;
    succeeded(&router, "dropping the view", &dropped).await;
    assert!(
        glue_table(&router, "shop", "v_recent").await.is_none(),
        "a dropped view is gone from the catalog"
    );
}

/// The names a single-column result set carries.
fn names_in(rows: &[Vec<Option<String>>]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| row.first().cloned().flatten())
        .collect()
}

/// One row of the data-reading table: a table definition, the objects under its location
/// and what a read of it must answer.
struct ReadCase {
    /// What this row is about.
    case: &'static str,
    /// The table's name.
    table: &'static str,
    /// The SerDe parameters the catalog holds for it.
    serde_parameters: Value,
    /// The table properties the catalog holds for it.
    parameters: Value,
    /// The objects under its location, as `(key suffix, body)`.
    objects: Vec<(&'static str, String)>,
    /// The rows a `SELECT id, name` over it must report, newest-first by `id`.
    expected: Vec<(&'static str, &'static str)>,
}

/// A read resolves its relation through the Data Catalog and computes its answer from the
/// OBJECTS at that relation's location, decoded by the SerDe the catalog recorded.
#[tokio::test]
async fn a_read_scans_the_objects_at_the_relations_location() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;

    // The rows differ along the separator the SerDe names, how many objects the location
    // holds and how they are laid out under it, whether a header line is declared, and
    // whether the location holds anything at all.
    let cases: Vec<ReadCase> = vec![
        ReadCase {
            case: "the default Hive field separator",
            table: "ctrl_a",
            serde_parameters: json!({}),
            parameters: json!({}),
            objects: vec![(
                "rows.txt",
                format!(
                    "1{UNIT_SEPARATOR}desk\n2{UNIT_SEPARATOR}lamp\n3{UNIT_SEPARATOR}rug\n"
                ),
            )],
            expected: vec![("1", "desk"), ("2", "lamp"), ("3", "rug")],
        },
        ReadCase {
            case: "a declared comma separator over two objects",
            table: "commas",
            serde_parameters: json!({ "field.delim": "," }),
            parameters: json!({}),
            objects: vec![
                ("part-0.csv", "1,desk\n2,lamp\n".to_string()),
                ("nested/part-1.csv", "3,rug\n".to_string()),
            ],
            expected: vec![("1", "desk"), ("2", "lamp"), ("3", "rug")],
        },
        ReadCase {
            case: "a declared tab separator and a header line",
            table: "tabs",
            serde_parameters: json!({ "field.delim": "\t" }),
            parameters: json!({ "skip.header.line.count": "1" }),
            objects: vec![(
                "rows.tsv",
                "id\tname\n1\tdesk\n2\tlamp\n".to_string(),
            )],
            expected: vec![("1", "desk"), ("2", "lamp")],
        },
        ReadCase {
            case: "a location holding only the files a read skips",
            table: "hidden",
            serde_parameters: json!({}),
            parameters: json!({}),
            objects: vec![("_SUCCESS", "1\u{1}desk\n".to_string())],
            expected: vec![],
        },
        ReadCase {
            case: "a registered table with nothing under its location",
            table: "empty",
            serde_parameters: json!({}),
            parameters: json!({}),
            objects: vec![],
            expected: vec![],
        },
    ];

    for case in &cases {
        glue_ok(
            &router,
            case.case,
            "CreateTable",
            &json!({
                "DatabaseName": "shop",
                "TableInput": {
                    "Name": case.table,
                    "TableType": "EXTERNAL_TABLE",
                    "StorageDescriptor": {
                        "Columns": [
                            { "Name": "id", "Type": "bigint" },
                            { "Name": "name", "Type": "string" },
                        ],
                        "Location": format!("s3://{BUCKET}/{}/", case.table),
                        "SerdeInfo": {
                            "SerializationLibrary": ddl::LAZY_SIMPLE_SERDE,
                            "Parameters": case.serde_parameters,
                        },
                    },
                    "Parameters": case.parameters,
                },
            }),
        )
        .await;
        for (key, body) in &case.objects {
            put_object(
                &router,
                case.case,
                &format!("{}/{key}", case.table),
                body,
            )
            .await;
        }
    }

    let mut started: Vec<(&ReadCase, String, String)> = Vec::new();
    for case in &cases {
        let projected = start(
            &router,
            case.case,
            "shop",
            &format!(
                "SELECT id, name FROM \"shop\".\"{}\" ORDER BY id",
                case.table
            ),
        )
        .await;
        let counted = start(
            &router,
            case.case,
            "shop",
            &format!("SELECT count(*) AS rows_held FROM shop.{}", case.table),
        )
        .await;
        started.push((case, projected, counted));
    }
    settle().await;

    for (case, projected, counted) in started {
        succeeded(&router, case.case, &projected).await;
        let rows = rows_of(&router, case.case, &projected).await;
        let actual: Vec<(String, String)> = rows
            .iter()
            .filter_map(|row| match row.as_slice() {
                [Some(id), Some(name)] => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            actual,
            case.expected
                .iter()
                .map(|(id, name)| ((*id).to_string(), (*name).to_string()))
                .collect::<Vec<_>>(),
            "case {:?}: expected the rows the objects hold, got {rows:?}",
            case.case
        );

        succeeded(&router, case.case, &counted).await;
        let aggregate = only_value(case.case, &rows_of(&router, case.case, &counted).await);
        assert_eq!(
            aggregate,
            case.expected.len().to_string(),
            "case {:?}: the aggregate is computed over the rows the location holds, got \
             {aggregate:?}",
            case.case
        );
    }
}

/// A typed read: the column types the catalog records are the types the engine computes
/// with and the result set reports, and a field that is no value of its column's type is
/// no value at all.
#[tokio::test]
async fn a_read_types_its_columns_the_way_the_catalog_declares_them() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;
    glue_ok(
        &router,
        "setup",
        "CreateTable",
        &json!({
            "DatabaseName": "shop",
            "TableInput": {
                "Name": "typed",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "int" },
                        { "Name": "big", "Type": "bigint" },
                        { "Name": "amount", "Type": "double" },
                        { "Name": "paid", "Type": "boolean" },
                        { "Name": "name", "Type": "string" },
                        { "Name": "created", "Type": "timestamp" },
                    ],
                    "Location": format!("s3://{BUCKET}/typed/"),
                    "SerdeInfo": {
                        "SerializationLibrary": ddl::LAZY_SIMPLE_SERDE,
                        "Parameters": { "field.delim": "," },
                    },
                },
            },
        }),
    )
    .await;
    // The second row's numeric fields carry text that is no number, which Hive reads as a
    // missing value rather than failing the read.
    put_object(
        &router,
        "setup",
        "typed/rows.csv",
        "1,4294967296,249.5,true,desk,2024-01-02 03:04:05\n\
         2,-,-,maybe,lamp,2024-02-03 04:05:06.12\n",
    )
    .await;

    let projected = start(
        &router,
        "typed columns",
        "shop",
        "SELECT id, big, amount, paid, name, created FROM shop.typed ORDER BY id",
    )
    .await;
    let aggregated = start(
        &router,
        "an aggregate over a typed column",
        "shop",
        "SELECT sum(amount) AS takings FROM shop.typed",
    )
    .await;
    settle().await;

    succeeded(&router, "typed columns", &projected).await;
    let (status, results) = athena(
        &router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": projected, "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: GetQueryResults must answer 200, got {status} with {results:?}",
        "typed columns"
    );
    let types: Vec<(String, String)> = results["ResultSet"]["ResultSetMetadata"]["ColumnInfo"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|column| {
            (
                column["Name"].as_str().unwrap_or_default().to_string(),
                column["Type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        types,
        vec![
            ("id".to_string(), "integer".to_string()),
            ("big".to_string(), "bigint".to_string()),
            ("amount".to_string(), "double".to_string()),
            ("paid".to_string(), "boolean".to_string()),
            ("name".to_string(), "varchar".to_string()),
            ("created".to_string(), "varchar".to_string()),
        ],
        "case {:?}: the reported types are the catalog's own, got {types:?}",
        "typed columns"
    );
    let rows = rows_of(&router, "typed columns", &projected).await;
    assert_eq!(
        rows,
        vec![
            vec![
                Some("1".to_string()),
                Some("4294967296".to_string()),
                Some("249.5".to_string()),
                Some("true".to_string()),
                Some("desk".to_string()),
                // A timestamp is reported to the precision that type carries, whatever
                // the stored text writes.
                Some("2024-01-02 03:04:05.000".to_string()),
            ],
            vec![
                Some("2".to_string()),
                None,
                None,
                None,
                Some("lamp".to_string()),
                Some("2024-02-03 04:05:06.120".to_string()),
            ],
        ],
        "case {:?}: unexpected rows",
        "typed columns"
    );

    succeeded(&router, "an aggregate over a typed column", &aggregated).await;
    let takings = only_value(
        "an aggregate over a typed column",
        &rows_of(&router, "an aggregate over a typed column", &aggregated).await,
    );
    assert_eq!(
        takings, "249.5",
        "case {:?}: the aggregate is computed over the values read, got {takings:?}",
        "an aggregate over a typed column"
    );
}

/// A table whose data is stored in a format this emulator's engine cannot decode is
/// REPORTED, not decoded as something else: the entry the catalog holds says how the data
/// is stored, so a read of it either reads that format or says it cannot.
#[tokio::test]
async fn a_table_stored_in_a_format_the_engine_cannot_read_is_reported() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;

    // (case, the table it registers, the DDL that registers it, the object key under its
    // location)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a columnar format",
            "columnar",
            "CREATE EXTERNAL TABLE shop.columnar (id bigint, name string) \
             STORED AS PARQUET LOCATION 's3://athena-catalog-data/columnar/'",
            "columnar/part-0.parquet",
        ),
        (
            "the optimised row-columnar format",
            "rows_columns",
            "CREATE EXTERNAL TABLE shop.rows_columns (id bigint, name string) \
             STORED AS ORC LOCATION 's3://athena-catalog-data/orc/'",
            "orc/part-0.orc",
        ),
        (
            "Avro containers",
            "containers",
            "CREATE EXTERNAL TABLE shop.containers (id bigint, name string) \
             STORED AS AVRO LOCATION 's3://athena-catalog-data/avro/'",
            "avro/part-0.avro",
        ),
    ];

    let mut started: Vec<(&str, String, String)> = Vec::new();
    for (case, table, ddl_sql, key) in &cases {
        let registered = start(&router, case, "shop", ddl_sql).await;
        // The object carries bytes that are not delimited text at all, which is exactly
        // what must NOT be read as rows.
        put_object(&router, case, key, "\u{0}\u{1}not delimited text\u{0}").await;
        let read = start(
            &router,
            case,
            "shop",
            &format!("SELECT id, name FROM shop.{table}"),
        )
        .await;
        started.push((case, registered, read));
    }
    settle().await;

    for (case, registered, read) in started {
        succeeded(&router, case, &registered).await;
        let found = execution(&router, case, &read).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: a read of a format the engine cannot decode must not answer \
             rows, got {found:?}"
        );
        let reason = found["Status"]["StateChangeReason"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: a failed query must report why, got {found:?}"
        );
        let (status, refused) = athena(
            &router,
            "GetQueryResults",
            &json!({ "QueryExecutionId": read }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a failed read has no rows to report, got {status} with \
             {refused:?}"
        );
    }
}

/// A relation the data catalog does NOT hold is Athena's own not-found refusal: the/// not-found class it publishes, naming what could not be resolved and where in the
/// statement it appears.
#[tokio::test]
async fn a_relation_the_catalog_does_not_hold_is_refused_as_not_found() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router, "shop").await;
    glue_ok(
        &router,
        "setup",
        "CreateTable",
        &json!({
            "DatabaseName": "shop",
            "TableInput": {
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "id", "Type": "bigint" } ],
                    "Location": format!("s3://{BUCKET}/events/"),
                },
            },
        }),
    )
    .await;

    // (case, the statement, the fragment the refusal must carry)
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a table the database does not hold",
            "SELECT id FROM shop.not_a_table",
            "Table 'awsdatacatalog.shop.not_a_table' does not exist",
        ),
        (
            "an unqualified table the context database does not hold",
            "SELECT id FROM not_a_table",
            "Table 'awsdatacatalog.shop.not_a_table' does not exist",
        ),
        (
            "a table in a database the catalog does not hold",
            "SELECT id FROM nowhere.events",
            "Schema 'nowhere' does not exist",
        ),
    ];

    let mut started: Vec<(&str, &str, String)> = Vec::new();
    for (case, sql, expected) in &cases {
        started.push((case, expected, start(&router, case, "shop", sql).await));
    }
    // The control statement over the table the catalog DOES hold succeeds, so the
    // refusals above are about the names and not about the read path.
    let control = start(
        &router,
        "the control read",
        "shop",
        "SELECT count(*) FROM shop.events",
    )
    .await;
    settle().await;

    for (case, expected, id) in started {
        let message = failed(&router, case, &id, 2, Some(1301)).await;
        assert!(
            message.contains(expected),
            "case {case:?}: expected a refusal carrying {expected:?}, got {message:?}"
        );
        assert!(
            message.contains("line 1:"),
            "case {case:?}: a not-found refusal names where the name appears, got \
             {message:?}"
        );
    }

    succeeded(&router, "the control read", &control).await;
    let counted = only_value(
        "the control read",
        &rows_of(&router, "the control read", &control).await,
    );
    assert_eq!(
        counted, "0",
        "case {:?}: a registered table with no objects under it answers no rows, got \
         {counted:?}",
        "the control read"
    );
}
