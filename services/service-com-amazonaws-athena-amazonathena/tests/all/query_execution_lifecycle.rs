// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's query-execution lifecycle: `StartQueryExecution` runs
//! the submitted SQL on this service's in-process engine and persists what it produced,
//! and `GetQueryExecution`, `GetQueryResults`, `ListQueryExecutions`,
//! `StopQueryExecution` and `BatchGetQueryExecution` answer out of that record — driven
//! over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through the same
//! router the emulator boots, so every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one
//! request: the rows differ along whatever the rule generalises over — the statement's
//! type, the number and type of the columns it projects, how many rows it returns, the
//! page size, the workgroup, the shape of a bad request.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, query_engine, DEFAULT_WORK_GROUP, IDEMPOTENT_PARAMETERS_DO_NOT_MATCH,
    NO_OUTPUT_LOCATION, QUERY_EXECUTION_COMPLETION, QUERY_STATE_CANCELLED, QUERY_STATE_FAILED,
    QUERY_STATE_QUEUED, QUERY_STATE_RUNNING, QUERY_STATE_SUCCEEDED,
};
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service
/// shape (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols
/// build that header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: the harness turns request authentication off
/// below, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// Where the queries in these tests write their results. Any location satisfies the
/// documented rule that one must be given; nothing is written to it.
const OUTPUT_LOCATION: &str = "s3://athena-results/lifecycle/";

/// How long a test waits past [`QUERY_EXECUTION_COMPLETION`] for a started query to be
/// terminal: the span is owed from the submit, and this leaves room for the engine run
/// and the request walk around it.
const SETTLE_GRACE: Duration = Duration::from_millis(600);

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Invoke one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
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

/// The wire error code (`__type`) of an error body, with the shape-id prefix the
/// awsJson protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// Start one query and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, answer) = call(router, "StartQueryExecution", body).await;
    assert_eq!(
        status, 200,
        "StartQueryExecution({body}) must be accepted, got {status} with {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQueryExecution must answer with an id, got {answer:?}"))
        .to_string()
}

/// A `StartQueryExecution` body for `sql` with a results location.
fn query(sql: &str) -> Value {
    json!({
        "QueryString": sql,
        "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
    })
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "GetQueryExecution", &json!({ "QueryExecutionId": id })).await;
    assert_eq!(
        status, 200,
        "GetQueryExecution({id}) must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// The `Status.State` of the stored execution `id`.
async fn state_of(router: &MultiServiceRouter, id: &str) -> String {
    execution(router, id).await["Status"]["State"]
        .as_str()
        .unwrap_or("<no State>")
        .to_string()
}

/// Wait out the span a started query's completion is due after, then return the
/// execution once it is terminal.
///
/// The completion is owed [`QUERY_EXECUTION_COMPLETION`] after the submit, so this
/// sleeps rather than spins, and then polls a few times in case the host was busy.
async fn settled(router: &MultiServiceRouter, id: &str) -> Value {
    ::tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
    for _ in 0..20 {
        let found = execution(router, id).await;
        let state = found["Status"]["State"].as_str().unwrap_or_default();
        if state != QUERY_STATE_QUEUED && state != QUERY_STATE_RUNNING {
            return found;
        }
        ::tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the execution {id} never reached a terminal state");
}

/// Read every row of `id`'s result set, `page_size` rows at a time, following the
/// `NextToken` the previous page handed back. Returns the rows in order and how many
/// pages it took.
async fn all_rows(
    router: &MultiServiceRouter,
    id: &str,
    page_size: Option<usize>,
) -> (Vec<Value>, usize) {
    let mut rows: Vec<Value> = Vec::new();
    let mut pages = 0;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "QueryExecutionId": id });
        if let Some(page_size) = page_size {
            request["MaxResults"] = json!(page_size);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "GetQueryResults", &request).await;
        assert_eq!(
            status, 200,
            "GetQueryResults({request}) must answer 200, got {status} with {body:?}"
        );
        pages += 1;
        for row in body["ResultSet"]["Rows"].as_array().unwrap_or(&Vec::new()) {
            rows.push(row.clone());
        }
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() || pages > 64 {
            break;
        }
    }
    (rows, pages)
}

/// One row rendered the way `GetQueryResults` renders it: a `Datum` per value, and a
/// `Datum` with no value for SQL NULL.
fn row_of(values: &[Option<&str>]) -> Value {
    json!({
        "Data": values
            .iter()
            .map(|value| match value {
                Some(value) => json!({ "VarCharValue": value }),
                None => json!({}),
            })
            .collect::<Vec<Value>>()
    })
}

/// A `ClientRequestToken` of the modelled length (32 to 128 characters), built from
/// `seed` so each test row carries its own.
fn request_token(seed: &str) -> String {
    format!("{seed:-<40}")
}

/// One row of the start-and-read-back table.
struct QueryCase {
    /// What this row is about.
    case: &'static str,
    /// The SQL submitted.
    sql: &'static str,
    /// The `StatementType` the submitted SQL parses as.
    statement_type: &'static str,
    /// The columns the query projects, as `(Name, Type)` — `None` where the row does not
    /// assert them.
    columns: Option<Vec<(&'static str, &'static str)>>,
    /// The DATA rows the query produces, without the column-label row.
    rows: Option<Vec<Vec<Option<&'static str>>>>,
}

/// The rows: a query differs from the next along the type of statement it is, how many
/// columns it projects and of which types, how many rows it returns, and whether its
/// answer is a literal or something the engine had to compute.
fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            case: "a one-column projection",
            sql: "SELECT 1 AS one",
            statement_type: query_engine::STATEMENT_TYPE_DML,
            columns: Some(vec![("one", "integer")]),
            rows: Some(vec![vec![Some("1")]]),
        },
        QueryCase {
            case: "an expression the engine has to evaluate",
            sql: "SELECT 2 + 3 * 4 AS computed, length('abcde') AS width",
            statement_type: query_engine::STATEMENT_TYPE_DML,
            columns: Some(vec![("computed", "integer"), ("width", "integer")]),
            rows: Some(vec![vec![Some("14"), Some("5")]]),
        },
        QueryCase {
            case: "a projection of one value of each type",
            sql: "SELECT 7 AS i, 4294967296 AS big, 2.5 AS d, 'text' AS s, \
                  x'deadbeef' AS b, NULL AS u",
            statement_type: query_engine::STATEMENT_TYPE_DML,
            columns: Some(vec![
                ("i", "integer"),
                ("big", "bigint"),
                ("d", "double"),
                ("s", "varchar"),
                ("b", "varbinary"),
                ("u", "unknown"),
            ]),
            rows: Some(vec![vec![
                Some("7"),
                Some("4294967296"),
                Some("2.5"),
                Some("text"),
                Some("deadbeef"),
                None,
            ]]),
        },
        QueryCase {
            case: "several rows, ordered by the engine",
            sql: "SELECT n FROM (SELECT 3 AS n UNION ALL SELECT 1 UNION ALL SELECT 2) \
                  ORDER BY n",
            statement_type: query_engine::STATEMENT_TYPE_DML,
            columns: Some(vec![("n", "integer")]),
            rows: Some(vec![
                vec![Some("1")],
                vec![Some("2")],
                vec![Some("3")],
            ]),
        },
        QueryCase {
            case: "a query whose answer is no rows at all",
            sql: "SELECT 1 AS n WHERE 1 = 0",
            statement_type: query_engine::STATEMENT_TYPE_DML,
            // The column is still reported, with the type the values it produced imply —
            // and it produced none, so there is no type to report for it.
            columns: Some(vec![("n", "unknown")]),
            rows: Some(vec![]),
        },
        QueryCase {
            case: "a definition statement",
            sql: "CREATE TABLE lifecycle_ddl (id integer, name varchar)",
            statement_type: query_engine::STATEMENT_TYPE_DDL,
            columns: Some(vec![]),
            rows: Some(vec![]),
        },
        QueryCase {
            case: "a definition statement that writes the rows a query produced",
            sql: "CREATE TABLE lifecycle_ctas AS SELECT 1 AS n UNION ALL SELECT 2",
            // `CREATE TABLE AS SELECT` manipulates data as well as defining a table,
            // which is the class Athena puts it in.
            statement_type: query_engine::STATEMENT_TYPE_DML,
            columns: Some(vec![]),
            rows: Some(vec![]),
        },
        QueryCase {
            case: "a utility statement",
            sql: "EXPLAIN SELECT 1",
            statement_type: query_engine::STATEMENT_TYPE_UTILITY,
            columns: None,
            rows: None,
        },
    ]
}

/// A started query is PERSISTED, and every member a read reports comes from that
/// record: the query text byte for byte, the statement type the SQL parses as, the
/// workgroup it belongs to, the resolved results location, and the rows the engine
/// computed. Its state walks QUEUED and then the terminal state its run reached.
#[tokio::test]
async fn a_started_query_is_persisted_and_read_back_with_the_rows_the_engine_computed() {
    let router = router();
    let before = SystemTime::now();

    // Started first, all of them, so the one span the completion is owed after is waited
    // out once rather than per row.
    let cases = query_cases();
    let mut started: Vec<(usize, String)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let id = start(&router, &query(case.sql)).await;
        // A caller reading straight back still sees a transient state — QUEUED, or
        // RUNNING once the queued span has passed — and `GetQueryResults` refuses while
        // the query has not finished.
        let state = state_of(&router, &id).await;
        assert!(
            state == QUERY_STATE_QUEUED || state == QUERY_STATE_RUNNING,
            "case {:?}: a query read straight back must be {QUERY_STATE_QUEUED} or \
             {QUERY_STATE_RUNNING}, got {state}",
            case.case
        );
        let (status, refused) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        assert_eq!(
            status, 400,
            "case {:?}: GetQueryResults on an unfinished query must answer 400, got \
             {status} with {refused:?}",
            case.case
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {:?}: expected InvalidRequestException, got {refused:?}",
            case.case
        );
        let message = refused["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(QUERY_STATE_QUEUED) || message.contains(QUERY_STATE_RUNNING),
            "case {:?}: the refusal must name the state the query is in, got {message:?}",
            case.case
        );
        started.push((index, id));
    }

    for (index, id) in &started {
        let case = &cases[*index];
        let found = settled(&router, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {:?}: expected {QUERY_STATE_SUCCEEDED}, got {found:?}",
            case.case
        );
        assert_eq!(
            found["QueryExecutionId"].as_str(),
            Some(id.as_str()),
            "case {:?}: the execution must report the id it was minted under, got {found:?}",
            case.case
        );
        assert_eq!(
            found["Query"].as_str(),
            Some(case.sql),
            "case {:?}: the execution must report the submitted query, got {found:?}",
            case.case
        );
        assert_eq!(
            found["StatementType"].as_str(),
            Some(case.statement_type),
            "case {:?}: expected statement type {}, got {found:?}",
            case.case,
            case.statement_type
        );
        assert_eq!(
            found["WorkGroup"].as_str(),
            Some(DEFAULT_WORK_GROUP),
            "case {:?}: a query naming no workgroup belongs to {DEFAULT_WORK_GROUP}, got \
             {found:?}",
            case.case
        );
        // Once the query has run, the results location names the OBJECT the run wrote
        // inside the configured directory: named after the execution, with the extension
        // its statement's results take.
        let suffix = if case.statement_type == query_engine::STATEMENT_TYPE_DML {
            state::RESULT_OBJECT_ROWS_SUFFIX
        } else {
            state::RESULT_OBJECT_TEXT_SUFFIX
        };
        let object = format!("{OUTPUT_LOCATION}{id}{suffix}");
        assert_eq!(
            found["ResultConfiguration"]["OutputLocation"].as_str(),
            Some(object.as_str()),
            "case {:?}: expected the result object {object}, got {found:?}",
            case.case
        );
        assert_eq!(
            found["Status"]["StateChangeReason"],
            Value::Null,
            "case {:?}: a query that succeeded has no state-change reason, got {found:?}",
            case.case
        );

        // The timestamps are the record's own instants, in order and in the span the
        // test ran in.
        let submitted = found["Status"]["SubmissionDateTime"].as_f64();
        let completed = found["Status"]["CompletionDateTime"].as_f64();
        let floor = before
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        let submitted = submitted.unwrap_or_else(|| {
            panic!(
                "case {:?}: a submitted query must report SubmissionDateTime, got {found:?}",
                case.case
            )
        });
        let completed = completed.unwrap_or_else(|| {
            panic!(
                "case {:?}: a terminal query must report CompletionDateTime, got {found:?}",
                case.case
            )
        });
        assert!(
            submitted >= floor && completed >= submitted,
            "case {:?}: expected {floor} <= SubmissionDateTime <= CompletionDateTime, got \
             {submitted} and {completed}",
            case.case
        );
        assert!(
            completed - submitted >= QUERY_EXECUTION_COMPLETION.as_secs_f64(),
            "case {:?}: a completion is due {QUERY_EXECUTION_COMPLETION:?} after the \
             submit, got {} s",
            case.case,
            completed - submitted
        );

        // The statistics are measurements, so they are reported and ordered rather than
        // guessed at.
        let statistics = &found["Statistics"];
        let total = statistics["TotalExecutionTimeInMillis"].as_i64();
        let queue = statistics["QueryQueueTimeInMillis"].as_i64();
        let engine = statistics["EngineExecutionTimeInMillis"].as_i64();
        assert!(
            total.zip(queue).is_some_and(|(total, queue)| total >= queue && queue > 0),
            "case {:?}: expected 0 < queue time <= total time, got {statistics:?}",
            case.case
        );
        assert!(
            engine.is_some_and(|engine| engine >= 0),
            "case {:?}: a terminal query must report its engine time, got {statistics:?}",
            case.case
        );

        let (rows, _) = all_rows(&router, id, None).await;
        let (status, results) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetQueryResults must answer 200, got {status} with {results:?}",
            case.case
        );

        if let Some(columns) = &case.columns {
            let reported: Vec<(String, String)> = results["ResultSet"]["ResultSetMetadata"]
                ["ColumnInfo"]
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
            let expected: Vec<(String, String)> = columns
                .iter()
                .map(|(name, kind)| ((*name).to_string(), (*kind).to_string()))
                .collect();
            assert_eq!(
                reported, expected,
                "case {:?}: expected columns {expected:?}, got {reported:?}",
                case.case
            );
        }

        if let Some(data) = &case.rows {
            // A DML query's results are read with the column labels as the first row;
            // a query of another type has no such row.
            let mut expected: Vec<Value> = Vec::new();
            if case.statement_type == query_engine::STATEMENT_TYPE_DML
                && !case.columns.as_ref().is_none_or(|columns| columns.is_empty())
            {
                let labels: Vec<Option<&str>> = case
                    .columns
                    .as_ref()
                    .map(|columns| columns.iter().map(|(name, _)| Some(*name)).collect())
                    .unwrap_or_default();
                expected.push(row_of(&labels));
            }
            for row in data {
                expected.push(row_of(row));
            }
            assert_eq!(
                rows, expected,
                "case {:?}: expected rows {expected:?}, got {rows:?}",
                case.case
            );

            // Read again: a result set is stored, so a repeat call reports the same rows.
            let (again, _) = all_rows(&router, id, None).await;
            assert_eq!(
                again, rows,
                "case {:?}: a repeat GetQueryResults must report the same rows, got \
                 {again:?} after {rows:?}",
                case.case
            );
        }
    }
}

/// Create the workgroup `name`, with no configuration of its own.
async fn create_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(router, "CreateWorkGroup", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "the workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// The same `ClientRequestToken` answers with the SAME id and creates no second
/// execution; a token reused under changed parameters is refused.
#[tokio::test]
async fn a_repeated_client_request_token_returns_the_same_id_and_creates_no_second_execution() {
    let router = router();
    // The workgroups the rows below run in, and the one a row changes a token's
    // parameters to: a query runs only in a workgroup this account has.
    for name in ["token-work-group", "another-work-group"] {
        create_work_group(&router, name).await;
    }

    // (case, the query the token was first used with)
    let cases: Vec<(&str, Value)> = vec![
        ("a projection", query("SELECT 1 AS one")),
        (
            "a definition statement",
            query("CREATE TABLE idempotent_ddl (id integer)"),
        ),
        (
            "a query carrying execution parameters",
            json!({
                "QueryString": "SELECT ? AS p",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                "ExecutionParameters": ["11"],
            }),
        ),
        (
            "a query in a workgroup of its own",
            json!({
                "QueryString": "SELECT 2 AS two",
                "WorkGroup": "token-work-group",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
        ),
    ];

    let mut minted: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in &cases {
        let mut first = request.clone();
        first["ClientRequestToken"] = json!(request_token(case));
        let id = start(&router, &first).await;
        // Repeated twice, so the answer is the stored id and not merely the id of the
        // call that happened to run most recently.
        for attempt in 0..2 {
            let repeated = start(&router, &first).await;
            assert_eq!(
                repeated, id,
                "case {case:?}: repeat {attempt} of one token must answer the same id"
            );
        }
        minted.push((case, id, first));
    }

    // One execution per token, whatever the repeats: every id in the history is one of
    // the minted ones, and each was minted once.
    let (status, listed) = call(&router, "ListQueryExecutions", &json!({ "MaxResults": 50 })).await;
    assert_eq!(status, 200, "ListQueryExecutions must answer 200: {listed:?}");
    let listed_ids: Vec<String> = listed["QueryExecutionIds"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect();
    let expected_in_default: Vec<&String> = minted
        .iter()
        .filter(|(case, _, _)| *case != "a query in a workgroup of its own")
        .map(|(_, id, _)| id)
        .collect();
    assert_eq!(
        listed_ids.len(),
        expected_in_default.len(),
        "one execution per token: expected {} ids in {DEFAULT_WORK_GROUP}, got {listed_ids:?}",
        expected_in_default.len()
    );
    for id in expected_in_default {
        assert!(
            listed_ids.contains(id),
            "the execution {id} a token minted must be in the history, got {listed_ids:?}"
        );
    }

    // "An error is returned if a parameter, such as QueryString, has changed." Each row
    // changes ONE parameter of the request the token was first used with.
    let (case, _, first) = minted.first().expect("the table has rows");
    for (changed, mutate) in changed_parameters() {
        let mut altered = first.clone();
        mutate(&mut altered);
        let (status, refused) = call(&router, "StartQueryExecution", &altered).await;
        assert_eq!(
            status, 400,
            "case {case:?} with {changed:?} changed: expected 400, got {status} with \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?} with {changed:?} changed: expected InvalidRequestException, got \
             {refused:?}"
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(IDEMPOTENT_PARAMETERS_DO_NOT_MATCH),
            "case {case:?} with {changed:?} changed: unexpected message in {refused:?}"
        );
    }
}

/// The parameters a reused `ClientRequestToken` may not have changed, one mutation each.
#[allow(clippy::type_complexity)]
fn changed_parameters() -> Vec<(&'static str, Box<dyn Fn(&mut Value)>)> {
    vec![
        (
            "QueryString",
            Box::new(|request: &mut Value| request["QueryString"] = json!("SELECT 2 AS two")),
        ),
        (
            "WorkGroup",
            Box::new(|request: &mut Value| request["WorkGroup"] = json!("another-work-group")),
        ),
        (
            "QueryExecutionContext",
            Box::new(|request: &mut Value| {
                request["QueryExecutionContext"] = json!({ "Database": "elsewhere" })
            }),
        ),
        (
            "ExecutionParameters",
            Box::new(|request: &mut Value| request["ExecutionParameters"] = json!(["5"])),
        ),
        (
            "ResultConfiguration",
            Box::new(|request: &mut Value| {
                request["ResultConfiguration"] = json!({ "OutputLocation": "s3://somewhere-else/" })
            }),
        ),
        (
            "ResultReuseConfiguration",
            Box::new(|request: &mut Value| {
                request["ResultReuseConfiguration"] = json!({
                    "ResultReuseByAgeConfiguration": { "Enabled": true, "MaxAgeInMinutes": 60 }
                })
            }),
        ),
    ]
}

/// `GetQueryResults` pages the stored result set: `MaxResults` bounds a page, the
/// `NextToken` resumes exactly where the previous page ended, and the pages concatenate
/// to the whole result set whatever the page size.
#[tokio::test]
async fn get_query_results_pages_by_max_results_and_resumes_from_next_token() {
    let router = router();
    // Five data rows, so a page size of 1, 2, 3, 5, 6 and the maximum all differ in how
    // many pages they take over the six rows the results carry (the labels and the five).
    let id = start(
        &router,
        &query(
            "SELECT n, n * n AS square FROM (SELECT 1 AS n UNION ALL SELECT 2 \
             UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5) ORDER BY n",
        ),
    )
    .await;
    settled(&router, &id).await;

    let whole = vec![
        row_of(&[Some("n"), Some("square")]),
        row_of(&[Some("1"), Some("1")]),
        row_of(&[Some("2"), Some("4")]),
        row_of(&[Some("3"), Some("9")]),
        row_of(&[Some("4"), Some("16")]),
        row_of(&[Some("5"), Some("25")]),
    ];

    // (page size, how many pages the whole result set takes at it)
    for (page_size, pages_expected) in [(1, 6), (2, 3), (3, 2), (5, 2), (6, 1), (1000, 1)] {
        let (rows, pages) = all_rows(&router, &id, Some(page_size)).await;
        assert_eq!(
            rows, whole,
            "page size {page_size}: the pages must concatenate to the whole result set, \
             got {rows:?}"
        );
        assert_eq!(
            pages, pages_expected,
            "page size {page_size}: expected {pages_expected} pages, got {pages}"
        );
    }

    // The first page of `MaxResults` rows carries exactly that many, and a token.
    let (status, first) = call(
        &router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": id, "MaxResults": 2 }),
    )
    .await;
    assert_eq!(status, 200, "the first page must answer 200: {first:?}");
    assert_eq!(
        first["ResultSet"]["Rows"].as_array().map(Vec::len),
        Some(2),
        "MaxResults 2 must bound the page to two rows, got {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must hand back a token, got {first:?}"))
        .to_string();

    // A token names a position in ONE execution's results: offered to another execution
    // it names no page at all.
    let other = start(&router, &query("SELECT 1 AS one")).await;
    settled(&router, &other).await;
    for (case, request) in [
        (
            "a token minted for another execution",
            json!({ "QueryExecutionId": other, "NextToken": token }),
        ),
        (
            "a token nothing minted",
            json!({ "QueryExecutionId": id, "NextToken": "not-a-token-this-store-minted" }),
        ),
        (
            "a token of the right shape with a forged authenticator",
            json!({ "QueryExecutionId": id, "NextToken": "0".repeat(token.len()) }),
        ),
    ] {
        let (status, refused) = call(&router, "GetQueryResults", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
    }
}

/// `ListQueryExecutions` lists the ids actually persisted, newest first, filtered by
/// workgroup and paged by `MaxResults`.
#[tokio::test]
async fn list_query_executions_reports_the_persisted_ids_newest_first_per_work_group() {
    let router = router();

    // Two workgroups a query ran in, so the filter is asserted on a history that holds
    // both, plus one that exists and no query ran in; several queries each, so the order
    // is asserted on more than one pair.
    for name in ["listing-work-group", "empty-work-group"] {
        create_work_group(&router, name).await;
    }
    let mut in_default: Vec<String> = Vec::new();
    let mut in_other: Vec<String> = Vec::new();
    for index in 0..3 {
        in_default.push(start(&router, &query(&format!("SELECT {index} AS n"))).await);
        in_other.push(
            start(
                &router,
                &json!({
                    "QueryString": format!("SELECT {index} AS n"),
                    "WorkGroup": "listing-work-group",
                    "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                }),
            )
            .await,
        );
    }
    let newest_first = |ids: &[String]| -> Vec<String> {
        ids.iter().rev().cloned().collect::<Vec<String>>()
    };

    // (case, the workgroup the request named, the ids it must report)
    let cases: Vec<(&str, Option<&str>, Vec<String>)> = vec![
        (
            "no workgroup named, so the primary workgroup",
            None,
            newest_first(&in_default),
        ),
        (
            "the primary workgroup named",
            Some(DEFAULT_WORK_GROUP),
            newest_first(&in_default),
        ),
        (
            "another workgroup named",
            Some("listing-work-group"),
            newest_first(&in_other),
        ),
        (
            "a workgroup that exists and no query ran in",
            Some("empty-work-group"),
            Vec::new(),
        ),
    ];

    for (case, work_group, expected) in &cases {
        let mut request = json!({});
        if let Some(work_group) = work_group {
            request["WorkGroup"] = json!(work_group);
        }
        let (status, body) = call(&router, "ListQueryExecutions", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
        let reported: Vec<String> = body["QueryExecutionIds"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        if expected.is_empty() {
            // `QueryExecutionIdList` holds at least one id, so an empty page reports no
            // member rather than an empty array.
            assert_eq!(
                body["QueryExecutionIds"],
                Value::Null,
                "case {case:?}: an empty page must omit QueryExecutionIds, got {body:?}"
            );
        }
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {case:?}: a page that reported everything must hand back no token, got \
             {body:?}"
        );
    }

    // Paged: every page size walks the same history in the same order.
    for page_size in [1, 2, 3] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListQueryExecutions", &request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected 200, got {status} with {body:?}"
            );
            pages += 1;
            let page: Vec<String> = body["QueryExecutionIds"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect();
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must hold at most that many ids, got {page:?}"
            );
            walked.extend(page);
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() || pages > 16 {
                break;
            }
        }
        assert_eq!(
            walked,
            newest_first(&in_default),
            "page size {page_size}: the pages must concatenate to the whole history"
        );
    }

    // `MaxResults` of 0 is inside the modelled range (0..50) and asks for no ids.
    let (status, none) = call(
        &router,
        "ListQueryExecutions",
        &json!({ "MaxResults": 0, "WorkGroup": DEFAULT_WORK_GROUP }),
    )
    .await;
    assert_eq!(status, 200, "MaxResults 0 must answer 200: {none:?}");
    assert_eq!(
        none["QueryExecutionIds"],
        Value::Null,
        "MaxResults 0 asks for no ids, got {none:?}"
    );
    assert_eq!(
        none["NextToken"],
        Value::Null,
        "a page of no ids must hand back no token, or a caller following it never \
         progresses, got {none:?}"
    );

    // A listing token is scoped to the workgroup it was minted for.
    let (_, first) = call(
        &router,
        "ListQueryExecutions",
        &json!({ "MaxResults": 1, "WorkGroup": DEFAULT_WORK_GROUP }),
    )
    .await;
    let token = first["NextToken"]
        .as_str()
        .expect("a truncated listing hands back a token")
        .to_string();
    for (case, request) in [
        (
            "a token minted for another workgroup",
            json!({ "WorkGroup": "listing-work-group", "NextToken": token }),
        ),
        (
            "a token nothing minted",
            json!({ "WorkGroup": DEFAULT_WORK_GROUP, "NextToken": "made-up-token" }),
        ),
    ] {
        let (status, refused) = call(&router, "ListQueryExecutions", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
    }
}

/// A query runs in a workgroup this account HAS: one `CreateWorkGroup` registered, or
/// Athena's own default workgroup, which every account has. A name no workgroup answers
/// to is refused, by the operation that starts a query and by the one that lists a
/// workgroup's history.
#[tokio::test]
async fn a_work_group_this_account_does_not_have_is_refused() {
    let router = router();
    create_work_group(&router, "registered-work-group").await;

    // (case, the workgroup named, whether it is one this account has)
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        ("no workgroup named, so the default one", None, true),
        ("the default workgroup named", Some(DEFAULT_WORK_GROUP), true),
        (
            "a registered workgroup",
            Some("registered-work-group"),
            true,
        ),
        ("a name nothing registered", Some("no-such-work-group"), false),
        (
            "a registered name differing only in case",
            Some("Registered-Work-Group"),
            false,
        ),
        (
            "a name that is a prefix of a registered one",
            Some("registered-work-grou"),
            false,
        ),
    ];

    for (case, work_group, held) in &cases {
        let mut request = query("SELECT 1 AS one");
        let mut listing = json!({});
        if let Some(work_group) = work_group {
            request["WorkGroup"] = json!(work_group);
            listing["WorkGroup"] = json!(work_group);
        }
        for (operation, body) in [
            ("StartQueryExecution", &request),
            ("ListQueryExecutions", &listing),
        ] {
            let (status, answer) = call(&router, operation, body).await;
            if *held {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} must be served, got {status} with {answer:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must answer 400, got {status} with {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidRequestException",
                "case {case:?}: {operation} must answer InvalidRequestException, got \
                 {answer:?}"
            );
            assert_eq!(
                answer["Message"].as_str(),
                Some(state::WORK_GROUP_NOT_FOUND),
                "case {case:?}: {operation} must state the workgroup miss, got {answer:?}"
            );
        }
    }
}

/// `StopQueryExecution` moves a query that has not finished to CANCELLED with its
/// completion time, leaves a query that already finished exactly as it was, and refuses
/// an id this account holds no execution for.
#[tokio::test]
async fn stopping_a_query_cancels_it_and_a_finished_query_is_left_as_it_is() {
    let router = router();

    // Rows differ along the state the query is in when the stop arrives.
    let running = start(&router, &query("SELECT 1 AS one")).await;
    let (status, stopped) = call(
        &router,
        "StopQueryExecution",
        &json!({ "QueryExecutionId": running }),
    )
    .await;
    assert_eq!(
        status, 200,
        "stopping a running query must answer 200, got {status} with {stopped:?}"
    );
    let found = execution(&router, &running).await;
    assert_eq!(
        found["Status"]["State"].as_str(),
        Some(QUERY_STATE_CANCELLED),
        "a stopped query must be {QUERY_STATE_CANCELLED}, got {found:?}"
    );
    assert_eq!(
        found["Status"]["StateChangeReason"].as_str(),
        Some(state::QUERY_CANCELLED_BY_USER),
        "a cancelled query must report why, got {found:?}"
    );
    assert!(
        found["Status"]["CompletionDateTime"].as_f64().is_some(),
        "a cancelled query must report its completion time, got {found:?}"
    );
    // A stop is idempotent, and a cancelled query stays cancelled.
    let (status, again) = call(
        &router,
        "StopQueryExecution",
        &json!({ "QueryExecutionId": running }),
    )
    .await;
    assert_eq!(status, 200, "a repeated stop must answer 200: {again:?}");
    assert_eq!(
        state_of(&router, &running).await,
        QUERY_STATE_CANCELLED,
        "a repeated stop must leave the query cancelled"
    );
    // There are no results to read from a query that did not succeed.
    let (status, refused) = call(
        &router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": running }),
    )
    .await;
    assert_eq!(
        status, 400,
        "GetQueryResults on a cancelled query must answer 400, got {refused:?}"
    );
    assert_eq!(
        refused["Message"].as_str(),
        Some(state::query_did_not_succeed(QUERY_STATE_CANCELLED).as_str()),
        "the refusal must name the final state, got {refused:?}"
    );

    // A query that already finished is not interrupted by a stop.
    let finished = start(&router, &query("SELECT 2 AS two")).await;
    settled(&router, &finished).await;
    let (status, stopped) = call(
        &router,
        "StopQueryExecution",
        &json!({ "QueryExecutionId": finished }),
    )
    .await;
    assert_eq!(
        status, 200,
        "stopping a finished query must answer 200, got {status} with {stopped:?}"
    );
    assert_eq!(
        state_of(&router, &finished).await,
        QUERY_STATE_SUCCEEDED,
        "a finished query must stay {QUERY_STATE_SUCCEEDED} after a stop"
    );
    let (status, results) = call(
        &router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": finished }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a finished query's results must still be readable, got {results:?}"
    );

    // An id no execution was minted under.
    for case in [
        "does-not-exist",
        "11111111-2222-3333-4444-555555555555",
        &"x".repeat(128),
    ] {
        for operation in ["StopQueryExecution", "GetQueryExecution", "GetQueryResults"] {
            let (status, refused) =
                call(&router, operation, &json!({ "QueryExecutionId": case })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must answer 400, got {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidRequestException",
                "case {case:?}: {operation} must answer InvalidRequestException, got \
                 {refused:?}"
            );
            assert_eq!(
                refused["Message"].as_str(),
                Some(state::query_execution_not_found(case).as_str()),
                "case {case:?}: {operation} must report the id it could not find, got \
                 {refused:?}"
            );
        }
    }
}

/// One row of the batch-get table: the ids asked for, and how they must be partitioned.
struct BatchCase {
    /// What this row is about.
    case: &'static str,
    /// The ids the request carries.
    asked: Vec<String>,
    /// The ids that must come back as executions, in order.
    executions: Vec<String>,
    /// The ids that must come back as unprocessed, in order.
    unprocessed: Vec<String>,
}

/// `BatchGetQueryExecution` partitions the ids it is given by what state holds: an
/// execution for each id there is one for, an unprocessed entry for each there is not.
#[tokio::test]
async fn batch_get_query_execution_partitions_known_from_unknown_ids() {
    let router = router();
    let first = start(&router, &query("SELECT 1 AS one")).await;
    let second = start(&router, &query("SELECT 2 AS two")).await;

    // Rows differ along how many ids are asked for and how many of them exist.
    let cases: Vec<BatchCase> = vec![
        BatchCase {
            case: "one id that exists",
            asked: vec![first.clone()],
            executions: vec![first.clone()],
            unprocessed: Vec::new(),
        },
        BatchCase {
            case: "one id that does not",
            asked: vec!["no-such-execution".to_string()],
            executions: Vec::new(),
            unprocessed: vec!["no-such-execution".to_string()],
        },
        BatchCase {
            case: "a mix, in the order asked for",
            asked: vec![second.clone(), "unknown-a".to_string(), first.clone()],
            executions: vec![second.clone(), first.clone()],
            unprocessed: vec!["unknown-a".to_string()],
        },
        BatchCase {
            case: "every id unknown",
            asked: vec!["unknown-b".to_string(), "unknown-c".to_string()],
            executions: Vec::new(),
            unprocessed: vec!["unknown-b".to_string(), "unknown-c".to_string()],
        },
    ];

    for BatchCase {
        case,
        asked,
        executions,
        unprocessed,
    } in &cases
    {
        let (status, body) = call(
            &router,
            "BatchGetQueryExecution",
            &json!({ "QueryExecutionIds": asked }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
        let reported: Vec<String> = body["QueryExecutions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|found| found["QueryExecutionId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            &reported, executions,
            "case {case:?}: expected executions {executions:?}, got {reported:?}"
        );
        let missing: Vec<String> = body["UnprocessedQueryExecutionIds"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|entry| entry["QueryExecutionId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            &missing, unprocessed,
            "case {case:?}: expected unprocessed {unprocessed:?}, got {missing:?}"
        );
        for entry in body["UnprocessedQueryExecutionIds"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(state::ERROR_CODE_QUERY_EXECUTION_NOT_FOUND),
                "case {case:?}: an unprocessed id must carry the error code, got {entry:?}"
            );
            let id = entry["QueryExecutionId"].as_str().unwrap_or_default();
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(state::query_execution_not_found(id).as_str()),
                "case {case:?}: an unprocessed id must carry why, got {entry:?}"
            );
        }
        // Every stored execution reports the query it ran, so the partition really is
        // read from state rather than echoed from the request.
        for found in body["QueryExecutions"].as_array().unwrap_or(&Vec::new()) {
            assert!(
                found["Query"].as_str().is_some_and(|query| query.starts_with("SELECT")),
                "case {case:?}: a reported execution must carry its query, got {found:?}"
            );
        }
    }
}

/// A query has to have somewhere to write its results: the request's own location, or
/// the workgroup's, and a query with neither is refused.
#[tokio::test]
async fn a_query_with_no_results_location_is_refused_and_a_work_group_can_supply_one() {
    let router = router();

    // A workgroup that supplies a location, and one that enforces its own over the
    // request's.
    for (name, enforce, location) in [
        ("supplying-work-group", false, "s3://from-the-work-group/"),
        ("enforcing-work-group", true, "s3://enforced/"),
    ] {
        let (status, created) = call(
            &router,
            "CreateWorkGroup",
            &json!({
                "Name": name,
                "Configuration": {
                    "ResultConfiguration": { "OutputLocation": location },
                    "EnforceWorkGroupConfiguration": enforce,
                },
            }),
        )
        .await;
        assert_eq!(status, 200, "the workgroup {name} must be created: {created:?}");
    }

    // (case, the request, the location the stored execution must report — or None when
    // the request must be refused)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the request names a location",
            query("SELECT 1 AS one"),
            Some(OUTPUT_LOCATION),
        ),
        (
            "the request names none and neither does its workgroup",
            json!({ "QueryString": "SELECT 1 AS one" }),
            None,
        ),
        (
            "the request names an empty location",
            json!({
                "QueryString": "SELECT 1 AS one",
                "ResultConfiguration": { "OutputLocation": "" },
            }),
            None,
        ),
        (
            "the request names none and its workgroup does",
            json!({ "QueryString": "SELECT 1 AS one", "WorkGroup": "supplying-work-group" }),
            Some("s3://from-the-work-group/"),
        ),
        (
            "the request names one and its workgroup does not enforce its own",
            json!({
                "QueryString": "SELECT 1 AS one",
                "WorkGroup": "supplying-work-group",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
            Some(OUTPUT_LOCATION),
        ),
        (
            "the request names one and its workgroup enforces its own",
            json!({
                "QueryString": "SELECT 1 AS one",
                "WorkGroup": "enforcing-work-group",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
            Some("s3://enforced/"),
        ),
    ];

    for (case, request, expected) in &cases {
        let (status, body) = call(&router, "StartQueryExecution", request).await;
        match expected {
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidRequestException",
                    "case {case:?}: expected InvalidRequestException, got {body:?}"
                );
                assert_eq!(
                    body["Message"].as_str(),
                    Some(NO_OUTPUT_LOCATION),
                    "case {case:?}: unexpected message in {body:?}"
                );
            }
            Some(location) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} with {body:?}"
                );
                let id = body["QueryExecutionId"].as_str().unwrap_or_default();
                let found = execution(&router, id).await;
                assert_eq!(
                    found["ResultConfiguration"]["OutputLocation"].as_str(),
                    Some(*location),
                    "case {case:?}: expected the resolved location {location}, got {found:?}"
                );
            }
        }
    }
}

/// SQL the engine will not run FAILS the query: the execution reaches FAILED carrying
/// the reason, and its results are not readable.
///
/// The reported `ErrorCategory` follows whose doing the failure was: the documented
/// `AthenaError$ErrorCategory` list is "1 - System, 2 - User, 3 - Other", so SQL the
/// engine refused is the caller's (2) and a bound this emulator holds a run to is
/// neither that nor the engine failing (3).
#[tokio::test]
async fn sql_the_engine_refuses_fails_the_query_with_its_own_reason() {
    let router = router();

    // (case, the SQL, the ErrorCategory it must report). Rows differ along WHY the query
    // fails: a name that is not there, a column that is not, syntax the engine cannot
    // compute, a statement count a query execution does not run, and the host-reaching
    // constructs the engine's own confinement refuses. The category follows WHOSE doing
    // the failure was: SQL the engine refused is the caller's (2), while what this
    // emulator's own confinement or bounds stopped is neither that nor the engine failing
    // (3). Text the GRAMMAR cannot read at all is not here: such a submission is refused
    // before any execution is minted (see the rows below).
    let cases: Vec<(&str, &str, i64)> = vec![
        ("a table no catalog holds", "SELECT * FROM not_a_table", 2),
        (
            "a column the table does not have",
            "SELECT missing FROM (SELECT 1 AS n)",
            2,
        ),
        ("more than one statement", "SELECT 1; SELECT 2", 3),
        ("syntax the engine cannot parse", "SELECT FROM WHERE", 2),
        (
            "attaching a host file as a database",
            "ATTACH DATABASE 'host.db' AS stolen",
            3,
        ),
        (
            "loading a host library as an extension",
            "SELECT load_extension('libevil.so')",
            // The engine reports a DENIED FUNCTION as a plain statement error rather
            // than as an authorization denial, and the category follows the code the
            // engine reported rather than its message.
            2,
        ),
    ];

    let mut started: Vec<(&str, String, i64)> = Vec::new();
    for (case, sql, category) in &cases {
        started.push((case, start(&router, &query(sql)).await, *category));
    }

    for (case, id, category) in &started {
        let found = settled(&router, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: expected {QUERY_STATE_FAILED}, got {found:?}"
        );
        let reason = found["Status"]["StateChangeReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: a failed query must report why, got {found:?}"
        );
        assert!(
            !reason.contains('/') && !reason.contains('\\'),
            "case {case:?}: a reason must not carry a host path, got {reason:?}"
        );
        assert_eq!(
            found["Status"]["AthenaError"]["ErrorCategory"].as_i64(),
            Some(*category),
            "case {case:?}: expected error category {category}, got {found:?}"
        );
        assert_eq!(
            found["Status"]["AthenaError"]["Retryable"].as_bool(),
            Some(false),
            "case {case:?}: the FAILED state is terminal with no automatic retry, got \
             {found:?}"
        );
        let (status, refused) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: results of a failed query must not be readable, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(state::query_did_not_succeed(QUERY_STATE_FAILED).as_str()),
            "case {case:?}: unexpected message in {refused:?}"
        );
    }

    // Text the grammar cannot read as a statement is refused at SUBMISSION, so no
    // execution is minted for it and the engine is never handed it: the rows differ along
    // what the text is — syntax of no statement at all, and host-reaching statements of
    // another engine's dialect.
    for (case, sql) in [
        (
            "a database name the DDL grammar does not read",
            "CREATE DATABASE bad-name",
        ),
        (
            "writing the database out to a host file",
            "VACUUM INTO 'host-copy.db'",
        ),
    ] {
        let (status, refused) = call(&router, "StartQueryExecution", &query(sql)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: text the grammar cannot read must be refused, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            refused["ErrorCode"].as_str(),
            Some("MALFORMED_QUERY"),
            "case {case:?}: unexpected refusal class in {refused:?}"
        );
        assert_eq!(
            refused["QueryExecutionId"].as_str(),
            None,
            "case {case:?}: a refused submission mints no execution, got {refused:?}"
        );
    }
}

/// A query that outgrows the bounds the emulator runs the engine under ends FAILED
/// inside them: a result set larger than it retains, and work longer than it allows.
///
/// A truncated result reported as a whole one would be worse than a failure, so the
/// query fails rather than paging a short answer.
#[tokio::test]
async fn a_query_that_outgrows_the_engine_bounds_ends_failed_inside_them() {
    let router = router();

    // (case, SQL, the bound it must reach)
    let cases: Vec<(&str, String)> = vec![
        (
            "more rows than one result set retains",
            format!(
                "WITH RECURSIVE counted(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM \
                 counted WHERE n < {}) SELECT n FROM counted",
                query_engine::MAX_RESULT_ROWS + 1
            ),
        ),
        (
            "work with no end to it",
            "WITH RECURSIVE forever(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM forever) \
             SELECT count(*) FROM forever"
                .to_string(),
        ),
    ];

    for (case, sql) in &cases {
        let started = SystemTime::now();
        let id = start(&router, &query(sql)).await;
        let elapsed = started.elapsed().unwrap_or_default();
        assert!(
            elapsed <= query_engine::MAX_QUERY_DEADLINE + Duration::from_secs(5),
            "case {case:?}: the engine must be stopped by its own deadline, and the call \
             took {elapsed:?}"
        );
        let found = settled(&router, &id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: expected {QUERY_STATE_FAILED}, got {found:?}"
        );
        assert!(
            found["Status"]["StateChangeReason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: a failed query must report why, got {found:?}"
        );
        // A bound this emulator holds a run to is neither the caller's SQL being wrong
        // nor the engine failing, which is what category 3 ("Other") names.
        assert_eq!(
            found["Status"]["AthenaError"]["ErrorCategory"].as_i64(),
            Some(3),
            "case {case:?}: an emulator bound is category 3, got {found:?}"
        );
        let (status, refused) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a query that reached a bound has no results to read, got \
             {status} with {refused:?}"
        );
    }
}

/// A query writes to the database its context names, and a later query in the same
/// database reads what it wrote: the rows a result set reports come from state, and
/// `UpdateCount` is the number of rows the writing statement really wrote.
#[tokio::test]
async fn what_one_query_writes_a_later_query_in_the_same_database_reads() {
    let router = router();
    let context = json!({ "Database": "sales", "Catalog": "AwsDataCatalog" });
    let run = |sql: String| {
        let body = json!({
            "QueryString": sql,
            "QueryExecutionContext": context.clone(),
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        });
        body
    };

    // Written first, read afterwards, and each statement is its own query execution.
    let create = start(
        &router,
        &run("CREATE TABLE orders (id integer, item varchar, paid boolean, total real)"
            .to_string()),
    )
    .await;
    // (case, the write statement, the UpdateCount it must report — `None` where the
    // engine counts no rows for it)
    let inserts: Vec<(&str, &str, Option<i64>)> = vec![
        (
            "one row",
            "INSERT INTO orders VALUES (1, 'desk', 1, 249.5)",
            Some(1),
        ),
        (
            "three rows at once",
            "INSERT INTO orders VALUES (2, 'lamp', 0, 19.0), (3, 'chair', 1, 89.25), \
             (4, 'rug', 0, 150.0)",
            Some(3),
        ),
        (
            "a table defined from the rows a query produced",
            "CREATE TABLE paid_orders AS SELECT id, total FROM orders WHERE paid = 1",
            None,
        ),
    ];
    let mut insert_ids: Vec<(&str, String, Option<i64>)> = Vec::new();
    for (case, sql, expected) in &inserts {
        insert_ids.push((case, start(&router, &run((*sql).to_string())).await, *expected));
    }
    let select = start(
        &router,
        &run("SELECT id, item, paid, total FROM orders ORDER BY id".to_string()),
    )
    .await;
    let aggregate = start(
        &router,
        &run("SELECT count(*) AS rows_held, sum(total) AS takings FROM orders".to_string()),
    )
    .await;
    // The table the CREATE TABLE AS SELECT defined holds the rows that query produced.
    let from_ctas = start(
        &router,
        &run("SELECT id FROM paid_orders ORDER BY id".to_string()),
    )
    .await;
    // A query in ANOTHER database sees none of it: each database is its own.
    let elsewhere = start(
        &router,
        &json!({
            "QueryString": "SELECT id FROM orders",
            "QueryExecutionContext": { "Database": "warehouse" },
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;

    assert_eq!(
        settled(&router, &create).await["Status"]["State"].as_str(),
        Some(QUERY_STATE_SUCCEEDED),
        "the definition statement must succeed"
    );
    for (case, id, expected) in &insert_ids {
        let found = settled(&router, id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {case:?}: the insert must succeed, got {found:?}"
        );
        let (status, results) =
            call(&router, "GetQueryResults", &json!({ "QueryExecutionId": id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {results:?}"
        );
        assert_eq!(
            results["UpdateCount"].as_i64(),
            *expected,
            "case {case:?}: expected {expected:?} rows written, got {results:?}"
        );
        assert_eq!(
            results["ResultSet"]["Rows"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a write returns no rows, got {results:?}"
        );
    }

    settled(&router, &select).await;
    let (rows, _) = all_rows(&router, &select, None).await;
    assert_eq!(
        rows,
        vec![
            row_of(&[Some("id"), Some("item"), Some("paid"), Some("total")]),
            row_of(&[Some("1"), Some("desk"), Some("true"), Some("249.5")]),
            row_of(&[Some("2"), Some("lamp"), Some("false"), Some("19.0")]),
            row_of(&[Some("3"), Some("chair"), Some("true"), Some("89.25")]),
            row_of(&[Some("4"), Some("rug"), Some("false"), Some("150.0")]),
        ],
        "the select must report the rows the inserts wrote"
    );

    settled(&router, &aggregate).await;
    let (aggregated, _) = all_rows(&router, &aggregate, None).await;
    assert_eq!(
        aggregated,
        vec![
            row_of(&[Some("rows_held"), Some("takings")]),
            row_of(&[Some("4"), Some("507.75")]),
        ],
        "the aggregate must be computed over the stored rows"
    );

    settled(&router, &from_ctas).await;
    let (defined, _) = all_rows(&router, &from_ctas, None).await;
    assert_eq!(
        defined,
        vec![
            row_of(&[Some("id")]),
            row_of(&[Some("1")]),
            row_of(&[Some("3")]),
        ],
        "the table the CREATE TABLE AS SELECT defined must hold the rows it selected"
    );

    let found = settled(&router, &elsewhere).await;
    assert_eq!(
        found["Status"]["State"].as_str(),
        Some(QUERY_STATE_FAILED),
        "a query in another database must not see this database's table, got {found:?}"
    );
}

/// One row of the execution-parameters table.
struct ParameterCase {
    /// What this row is about.
    case: &'static str,
    /// The SQL submitted, with one `?` per parameter it expects.
    sql: &'static str,
    /// The `ExecutionParameters` the request carries, as the SQL literals a caller
    /// writes them as.
    parameters: Vec<&'static str>,
    /// The values the single row must carry, or `None` where the engine must refuse the
    /// query.
    expected: Option<Vec<Option<&'static str>>>,
}

/// `ExecutionParameters` are "applied sequentially to the parameters in the query in the
/// order in which the parameters occur": each value is BOUND to one placeholder.
#[tokio::test]
async fn execution_parameters_are_bound_to_the_query_in_order() {
    let router = router();

    // Rows differ along how many parameters are bound and of which type, and one row
    // leaves the count wrong on purpose.
    let cases: Vec<ParameterCase> = vec![
        ParameterCase {
            case: "one integer parameter",
            sql: "SELECT ? AS p",
            parameters: vec!["11"],
            expected: Some(vec![Some("11")]),
        },
        ParameterCase {
            case: "one quoted string parameter",
            sql: "SELECT ? AS p",
            parameters: vec!["'a name'"],
            expected: Some(vec![Some("a name")]),
        },
        ParameterCase {
            case: "parameters applied in order",
            sql: "SELECT ? AS first, ? AS second, ? AS third",
            parameters: vec!["1", "'two'", "3.5"],
            expected: Some(vec![Some("1"), Some("two"), Some("3.5")]),
        },
        ParameterCase {
            case: "a parameter the query computes with",
            sql: "SELECT ? + 1 AS incremented",
            parameters: vec!["41"],
            expected: Some(vec![Some("42")]),
        },
        ParameterCase {
            case: "more parameters than the query has placeholders",
            sql: "SELECT ? AS p",
            parameters: vec!["1", "2"],
            expected: None,
        },
    ];

    let mut started: Vec<(usize, String)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let id = start(
            &router,
            &json!({
                "QueryString": case.sql,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                "ExecutionParameters": case.parameters,
            }),
        )
        .await;
        started.push((index, id));
    }

    for (index, id) in &started {
        let ParameterCase {
            case,
            sql: _,
            parameters,
            expected,
        } = &cases[*index];
        let found = settled(&router, id).await;
        match expected {
            None => {
                assert_eq!(
                    found["Status"]["State"].as_str(),
                    Some(QUERY_STATE_FAILED),
                    "case {case:?}: the engine must refuse it, got {found:?}"
                );
                continue;
            }
            Some(values) => {
                assert_eq!(
                    found["Status"]["State"].as_str(),
                    Some(QUERY_STATE_SUCCEEDED),
                    "case {case:?}: expected {QUERY_STATE_SUCCEEDED}, got {found:?}"
                );
                let reported: Vec<String> = found["ExecutionParameters"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect();
                assert_eq!(
                    reported,
                    parameters
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect::<Vec<String>>(),
                    "case {case:?}: the stored execution must report its parameters, got \
                     {found:?}"
                );
                let (rows, _) = all_rows(&router, id, None).await;
                let data = rows.get(1).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    data,
                    row_of(values),
                    "case {case:?}: expected the bound values {values:?}, got {rows:?}"
                );
            }
        }
    }
}
