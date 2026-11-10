// SPDX-License-Identifier: CC0-1.0

//! Athena's statements over the data catalog's DATABASES: the DDL that defines one, the
//! DDL that drops one, and the statement that lists them.
//!
//! The rule under test: a statement whose whole effect is on a DATABASE of the data
//! catalog is performed against that catalog — `AwsDataCatalog` IS the account's AWS Glue
//! Data Catalog — so
//!
//! * `CREATE DATABASE` (and its Presto spelling `CREATE SCHEMA`), in every clause form
//!   Athena's DDL engine takes, reaches State SUCCEEDED with StatementType DDL and
//!   SubstatementType CREATE_DATABASE, and leaves a database Glue's own `GetDatabase`
//!   reports, carrying the description, the location and the properties the statement
//!   declared;
//! * `DROP DATABASE` removes that same database, and a database that still holds tables is
//!   dropped only by a statement that asked for its contents to go with it;
//! * `SHOW DATABASES` reports the databases that catalog holds;
//! * a statement the catalog's metastore refuses — a database that is already there, one
//!   that is not, one that is not empty — FAILS carrying the error type Athena publishes
//!   for that class, the metastore's own message, and the DDL task's line as the state
//!   change reason, and leaves the catalog exactly as it was.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! grammar the statement is written in, the quoting of the name, which clauses it carries,
//! and which class of refusal the metastore answers with.
//!
//! Every call goes over the wire the production front door uses: Athena and Glue through
//! `POST /` with `X-Amz-Target`, on ONE `MultiServiceRouter` over ONE shared engine, so the
//! catalog Athena writes is the catalog Glue reads.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The name Athena reaches the account's AWS Glue Data Catalog by.
const GLUE_CATALOG: &str = "AwsDataCatalog";

/// The database a statement that names none runs in.
const CONTEXT_DATABASE: &str = "default";

/// Where a query's results are written, which every submission has to name.
const OUTPUT_LOCATION: &str = "s3://athena-database-ddl/results/";

/// How long past [`QUERY_EXECUTION_COMPLETION`] a test waits for a started query to be
/// readable in its terminal state.
const SETTLE_GRACE: Duration = Duration::from_millis(400);

/// The `AthenaError$ErrorCategory` of a failure the submitted statement caused: the
/// documented list is "1 - System, 2 - User, 3 - Other".
const ERROR_CATEGORY_USER: i64 = 2;

/// The `AthenaError$ErrorType` Athena publishes for a DDL task the metastore refused.
const ERROR_TYPE_DDL_TASK: i64 = 1006;

/// The `AthenaError$ErrorType` Athena publishes for a name the catalog does not hold.
const ERROR_TYPE_ENTITY_NOT_FOUND: i64 = 1301;

/// What the Hive DDL engine writes in front of the metastore exception a failed task
/// wrapped.
const DDL_TASK_FAILED: &str =
    "FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask.";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena and Glue mounted on ONE shared engine, the way the router mounts every service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
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

/// Start one statement and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, sql: &str) -> String {
    let (status, answer) = athena(
        router,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": CONTEXT_DATABASE, "Catalog": GLUE_CATALOG },
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

/// The `Database` Glue holds under `name`, or `None` when it holds none.
async fn stored_database(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, body) = glue(router, "GetDatabase", &json!({ "Name": name })).await;
    (status == 200).then(|| body["Database"].clone())
}

/// A database definition in every form Athena's DDL engine takes it is PERFORMED against
/// the Glue Data Catalog: the execution succeeds as a DDL statement of its own kind, and
/// the database it defined is one Glue reports with the clauses the statement declared.
#[tokio::test]
async fn a_database_definition_is_performed_against_the_data_catalog() {
    let router = emulator();

    // (case, the statement, the database it defines, its description, its location, its
    // properties). The rows differ along the grammar the statement is written in, the
    // quoting of the name and which clauses it carries.
    type Case = (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<(&'static str, &'static str)>,
    );
    let cases: Vec<Case> = vec![
        (
            "a bare definition",
            "CREATE DATABASE ddl_plain",
            "ddl_plain",
            None,
            None,
            None,
        ),
        (
            "the Presto spelling of the same object",
            "CREATE SCHEMA ddl_schema",
            "ddl_schema",
            None,
            None,
            None,
        ),
        (
            "a name the statement folds",
            "CREATE DATABASE IF NOT EXISTS DDL_Folded",
            "ddl_folded",
            None,
            None,
            None,
        ),
        (
            "a back-quoted name with every clause the grammar takes",
            "CREATE DATABASE `ddl-clauses` COMMENT 'the shop' \
             LOCATION 's3://athena-database-ddl/shop/' \
             WITH DBPROPERTIES ('owner' = 'shop')",
            "ddl-clauses",
            Some("the shop"),
            Some("s3://athena-database-ddl/shop"),
            Some(("owner", "shop")),
        ),
    ];

    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, sql, ..) in &cases {
        started.push((case, start(&router, case, sql).await));
    }
    settle().await;

    for ((case, sql, database, description, location, property), (_, id)) in
        cases.iter().zip(&started)
    {
        let found = execution(&router, case, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {case:?}: {sql} must reach {QUERY_STATE_SUCCEEDED}, got {found:?}"
        );
        assert_eq!(
            found["StatementType"].as_str(),
            Some("DDL"),
            "case {case:?}: a definition is a DDL statement, got {found:?}"
        );
        assert_eq!(
            found["SubstatementType"].as_str(),
            Some("CREATE_DATABASE"),
            "case {case:?}: unexpected substatement type in {found:?}"
        );
        assert!(
            found["Statistics"]["QueryPlanningTimeInMillis"].is_i64(),
            "case {case:?}: a terminal query reports what planning it took, got {found:?}"
        );

        let stored = stored_database(&router, database).await.unwrap_or_else(|| {
            panic!("case {case:?}: the Data Catalog must hold {database} after {sql}")
        });
        assert_eq!(
            stored["Name"].as_str(),
            Some(*database),
            "case {case:?}: unexpected stored database in {stored:?}"
        );
        assert_eq!(
            stored["Description"].as_str(),
            *description,
            "case {case:?}: unexpected description in {stored:?}"
        );
        assert_eq!(
            stored["LocationUri"].as_str(),
            *location,
            "case {case:?}: unexpected location in {stored:?}"
        );
        if let Some((key, value)) = property {
            assert_eq!(
                stored["Parameters"][key].as_str(),
                Some(*value),
                "case {case:?}: unexpected parameters in {stored:?}"
            );
        }
    }
}

/// What the catalog's metastore REFUSES fails the query with that class's own error type,
/// its own message and the DDL task's own line — and leaves the catalog as it was; what it
/// performs succeeds and is visible to Glue.
#[tokio::test]
async fn the_metastore_refusals_carry_athenas_own_classification() {
    let router = emulator();

    // Two databases: one empty, and one holding a table, which is what makes a drop
    // without CASCADE a refusal.
    let setup = start(&router, "setup", "CREATE DATABASE ddl_empty").await;
    let held = start(&router, "setup", "CREATE DATABASE ddl_held").await;
    settle().await;
    for (case, id) in [("empty", &setup), ("held", &held)] {
        let found = execution(&router, case, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "setup {case:?}: the database must be created, got {found:?}"
        );
    }
    let (status, created) = glue(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "ddl_held",
            "TableInput": { "Name": "items" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the table must be created, got {status} with {created:?}"
    );

    // (case, the statement, the error type it reports, its message, its state change
    // reason). The rows differ along WHICH refusal the metastore answers with: an object
    // already there, one that is not there, and one that cannot go while it holds
    // tables.
    let cases: Vec<(&str, &str, i64, String, String)> = vec![
        (
            "a database that is already there",
            "CREATE DATABASE ddl_empty",
            ERROR_TYPE_DDL_TASK,
            "Database ddl_empty already exists".to_string(),
            format!(
                "{DDL_TASK_FAILED} AlreadyExistsException(message:Database ddl_empty \
                 already exists)"
            ),
        ),
        (
            "a database that still holds tables",
            "DROP DATABASE ddl_held",
            ERROR_TYPE_DDL_TASK,
            "Cannot drop database ddl_held as its not empty.".to_string(),
            format!(
                "{DDL_TASK_FAILED} MetaException(message:Unable to drop database: \
                 InvalidOperationException(message:Database ddl_held is not empty.))"
            ),
        ),
        (
            "a database the catalog does not hold",
            "DROP SCHEMA ddl_absent",
            ERROR_TYPE_ENTITY_NOT_FOUND,
            "Database does not exist: ddl_absent".to_string(),
            "FAILED: SemanticException [Error 10072]: Database does not exist: ddl_absent"
                .to_string(),
        ),
    ];

    let mut refused: Vec<(&str, String)> = Vec::new();
    for (case, sql, ..) in &cases {
        refused.push((case, start(&router, case, sql).await));
    }
    settle().await;

    for ((case, sql, error_type, message, reason), (_, id)) in cases.iter().zip(&refused) {
        let found = execution(&router, case, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: {sql} must reach {QUERY_STATE_FAILED}, got {found:?}"
        );
        assert_eq!(
            found["Status"]["AthenaError"]["ErrorCategory"].as_i64(),
            Some(ERROR_CATEGORY_USER),
            "case {case:?}: the statement caused the failure, got {found:?}"
        );
        assert_eq!(
            found["Status"]["AthenaError"]["ErrorType"].as_i64(),
            Some(*error_type),
            "case {case:?}: unexpected error type in {found:?}"
        );
        assert_eq!(
            found["Status"]["AthenaError"]["ErrorMessage"].as_str(),
            Some(message.as_str()),
            "case {case:?}: unexpected error message in {found:?}"
        );
        assert_eq!(
            found["Status"]["StateChangeReason"].as_str(),
            Some(reason.as_str()),
            "case {case:?}: unexpected state change reason in {found:?}"
        );
    }

    // Nothing the refusals addressed changed: the database that was there still is, with
    // the table it held.
    let (status, table) = glue(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "ddl_held", "Name": "items" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the refused drop must leave the table alone, got {status} with {table:?}"
    );

    // What the metastore PERFORMS: a definition it tolerates the presence of, a drop that
    // takes the tables with it, and a drop that tolerates an absence.
    let performed: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "a definition it tolerates the presence of",
            "CREATE DATABASE IF NOT EXISTS ddl_empty",
            Some("ddl_empty"),
        ),
        (
            "a drop that takes the tables with it",
            "DROP DATABASE ddl_held CASCADE",
            None,
        ),
        (
            "a drop that tolerates an absence",
            "DROP DATABASE IF EXISTS ddl_absent",
            None,
        ),
    ];
    let mut accepted: Vec<(&str, String)> = Vec::new();
    for (case, sql, _) in &performed {
        accepted.push((case, start(&router, case, sql).await));
    }
    settle().await;
    for ((case, sql, still_held), (_, id)) in performed.iter().zip(&accepted) {
        let found = execution(&router, case, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {case:?}: {sql} must reach {QUERY_STATE_SUCCEEDED}, got {found:?}"
        );
        if let Some(database) = still_held {
            assert!(
                stored_database(&router, database).await.is_some(),
                "case {case:?}: {database} must still be in the catalog"
            );
        }
    }
    assert!(
        stored_database(&router, "ddl_held").await.is_none(),
        "the dropped database must be gone from the catalog"
    );
}

/// `SHOW DATABASES` reports what the catalog holds: the databases of the catalog the
/// query ran against, in name order, narrowed by the pattern the statement gave.
#[tokio::test]
async fn show_databases_reports_the_catalog_s_own_databases() {
    let router = emulator();

    for name in ["show_alpha", "show_beta", "other_gamma"] {
        let (status, created) = glue(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": name } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: {name} must be created, got {status} with {created:?}"
        );
    }

    // (case, the statement, the names it must report). The rows differ along the spelling
    // of the statement and whether it narrows its answer.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "every database the catalog holds",
            "SHOW DATABASES",
            vec!["other_gamma", "show_alpha", "show_beta"],
        ),
        (
            "the Presto spelling of the same statement",
            "SHOW SCHEMAS",
            vec!["other_gamma", "show_alpha", "show_beta"],
        ),
        (
            "narrowed by a SQL pattern",
            "SHOW DATABASES LIKE 'show_%'",
            vec!["show_alpha", "show_beta"],
        ),
    ];

    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, sql, _) in &cases {
        started.push((case, start(&router, case, sql).await));
    }
    settle().await;

    for ((case, sql, expected), (_, id)) in cases.iter().zip(&started) {
        let found = execution(&router, case, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {case:?}: {sql} must reach {QUERY_STATE_SUCCEEDED}, got {found:?}"
        );
        assert_eq!(
            found["SubstatementType"].as_str(),
            Some("SHOW_DATABASES"),
            "case {case:?}: unexpected substatement type in {found:?}"
        );
        assert_eq!(
            found["StatementType"].as_str(),
            Some("UTILITY"),
            "case {case:?}: a reporting statement is a UTILITY statement, got {found:?}"
        );

        let (status, results) = athena(
            &router,
            "GetQueryResults",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetQueryResults must answer 200, got {status} with {results:?}"
        );
        let rows = results["ResultSet"]["Rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                row["Data"][0]["VarCharValue"]
                    .as_str()
                    .map(str::to_string)
            })
            // A result set whose first row is the column LABEL carries the names after
            // it; one that carries no label row carries only the names.
            .skip_while(|value| value == "database_name")
            .collect();
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected the databases {expected:?}, got {reported:?} in \
             {results:?}"
        );
    }
}
