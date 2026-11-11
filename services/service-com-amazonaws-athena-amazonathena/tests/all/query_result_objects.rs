// SPDX-License-Identifier: CC0-1.0

//! What a COMPLETED Athena query execution leaves in the object store: the results objects
//! it saves under the output location its submission resolved.
//!
//! The rule under test: a query whose run reported the statement done SAVES what it
//! produced under the output location that submission resolved, keyed by the execution's
//! own id — `<location>/<QueryExecutionId>.csv` with its `.metadata` sibling for a
//! statement whose results are rows, and `<location>/<QueryExecutionId>.txt` for one whose
//! result is the engine's text output — which is the same object `GetQueryExecution`
//! names in `ResultConfiguration.OutputLocation`. The location is the one in force at
//! SUBMIT time: the request's, the workgroup's where the request names none, and the
//! workgroup's where its configuration overrides the request's.
//!
//! Every suite asserts that rule over a TABLE of inputs rather than one request. The rows
//! differ along what the rule generalises over: where the location came from, the form the
//! prefix was written in, the class of statement submitted, and the values the run
//! produced.
//!
//! Athena and Amazon S3 are mounted on ONE shared engine and driven over the wire the
//! production front door uses — Athena through `POST /` with `X-Amz-Target`, S3 through its
//! own REST data plane — so what is asserted is what an SDK client observes of both
//! services.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    query_results::RESULT_METADATA_SUFFIX, QUERY_EXECUTION_COMPLETION, QUERY_STATE_SUCCEEDED,
    RESULT_OBJECT_ROWS_SUFFIX, RESULT_OBJECT_TEXT_SUFFIX,
};
use smithy_server_runtime::MultiServiceRouter;

/// The bucket every query in these suites writes its results into.
const BUCKET: &str = "athena-saved-results";

/// A second bucket, named by a request whose workgroup overrides it: nothing may be written
/// there.
const DECOY_BUCKET: &str = "athena-decoy-results";

/// How long past [`QUERY_EXECUTION_COMPLETION`] a test waits for a started query to be
/// readable in its terminal state.
const SETTLE_GRACE: Duration = Duration::from_millis(600);

/// How often a test asks whether a started query is terminal yet.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena and Amazon S3 mounted on ONE shared engine, the way the router mounts every
/// service: the object store Athena saves results into is the one S3's own operations read.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One Athena call, as a client makes it, and its HTTP status and parsed body.
async fn athena(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonAthena.{operation}"))
        .header("authorization", authorization("athena"))
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

/// One path-style request to the emulator's Amazon S3 front door, and its status and body
/// as text.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// `PUT /<bucket>` — `CreateBucket` — so a query's results have somewhere to land.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let (status, body) = s3(router, "PUT", &format!("/{bucket}")).await;
    assert!(
        (200..300).contains(&status),
        "setup: creating s3://{bucket} must succeed, got {status} with {body}"
    );
}

/// The keys `ListObjectsV2` lists under `prefix`, in the order the listing reports them.
async fn listed_keys(router: &MultiServiceRouter, bucket: &str, prefix: &str) -> Vec<String> {
    let (status, body) = s3(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={prefix}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListObjectsV2(s3://{bucket}/{prefix}) must answer 200, got {status} with {body}"
    );
    body.split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split_once("</Key>"))
        .map(|(key, _)| key.to_string())
        .collect()
}

/// `GET /<bucket>/<key>` — `GetObject` — answering `(status, body)`.
async fn object(router: &MultiServiceRouter, bucket: &str, key: &str) -> (u16, String) {
    s3(router, "GET", &format!("/{bucket}/{key}")).await
}

/// Start one query and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, answer) = athena(router, "StartQueryExecution", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartQueryExecution({request}) must be accepted, got {status} with \
         {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: StartQueryExecution must answer with an id, got {answer:?}")
        })
        .to_string()
}

/// Poll a started query until it is terminal, asserting it SUCCEEDED, and answer its
/// `QueryExecution`.
///
/// POLLED rather than slept through: the suites start every row's query before reading any
/// of them back, so the span a completion is owed after elapses once for the whole table
/// instead of once per row.
async fn succeeded(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let attempts = ((QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).as_millis()
        / POLL_INTERVAL.as_millis())
        + 20;
    for _ in 0..attempts {
        let (status, body) = athena(
            router,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetQueryExecution({id}) must answer 200, got {status} with {body:?}"
        );
        let found = body["QueryExecution"].clone();
        if found["Status"]["State"].as_str() == Some(QUERY_STATE_SUCCEEDED) {
            return found;
        }
        if found["Status"]["StateChangeReason"].is_string() {
            panic!("case {case:?}: the query {id} did not succeed: {found:?}");
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the query {id} never reached SUCCEEDED");
}

/// Create the workgroup `name` with `configuration`.
async fn create_work_group(router: &MultiServiceRouter, name: &str, configuration: Value) {
    let (status, created) = athena(
        router,
        "CreateWorkGroup",
        &json!({ "Name": name, "Configuration": configuration }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Apply `updates` to the workgroup `name`.
async fn update_work_group(router: &MultiServiceRouter, name: &str, updates: Value) {
    let (status, updated) = athena(
        router,
        "UpdateWorkGroup",
        &json!({ "WorkGroup": name, "ConfigurationUpdates": updates }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the workgroup {name} must take the update {updates}, got {status} with \
         {updated:?}"
    );
}

/// Where one row of the location table expects its results, and what it submits.
struct Landing {
    /// What the row is.
    case: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// The `StartQueryExecution` request, less the statement.
    request: Value,
    /// The prefix the results object must land under, without a leading or trailing
    /// separator.
    prefix: &'static str,
    /// The extension the results object takes.
    suffix: &'static str,
    /// Whether a `.metadata` sibling must be saved beside it: a statement whose results
    /// are rows saves its metadata information with them.
    metadata: bool,
    /// The bytes the saved object must hold, where this row pins them. A statement that
    /// produced no rows saves an object with nothing in it; the suite below pins the bytes
    /// of a result set that has rows.
    body: Option<&'static str>,
}

/// A query whose run reported the statement done SAVES its results under the output
/// location the submission resolved, keyed by the execution's own id — which is the key
/// `GetQueryExecution` names in `ResultConfiguration.OutputLocation`.
///
/// The rows differ along where the resolved location came from (the request, the workgroup
/// behind a request that names none, the workgroup overriding one that does), the form the
/// prefix was written in (with and without its trailing separator, and nested), and the
/// class of statement submitted (results that are rows, and a result that is the engine's
/// text output).
#[tokio::test]
async fn a_succeeded_query_saves_its_results_under_the_resolved_output_location() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    create_bucket(&router, DECOY_BUCKET).await;

    // The workgroups the rows below run in: one supplying a location to a request that
    // names none, and one enforcing its own over whatever a request names.
    create_work_group(
        &router,
        "supplying",
        json!({ "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/supplied/") } }),
    )
    .await;
    create_work_group(
        &router,
        "enforcing",
        json!({
            "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/enforced/") },
            "EnforceWorkGroupConfiguration": true,
        }),
    )
    .await;

    let cases: Vec<Landing> = vec![
        Landing {
            case: "a projection writing to the location the request named",
            sql: "SELECT 'north' AS region",
            request: json!({
                "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/requested/") }
            }),
            prefix: "requested",
            suffix: RESULT_OBJECT_ROWS_SUFFIX,
            metadata: true,
            body: Some("\"region\"\n\"north\"\n"),
        },
        Landing {
            case: "a location written without its trailing separator",
            sql: "SELECT 'south' AS region",
            request: json!({
                "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/bare") }
            }),
            prefix: "bare",
            suffix: RESULT_OBJECT_ROWS_SUFFIX,
            metadata: true,
            body: Some("\"region\"\n\"south\"\n"),
        },
        Landing {
            case: "a nested location",
            sql: "SELECT 'east' AS region",
            request: json!({
                "ResultConfiguration": {
                    "OutputLocation": format!("s3://{BUCKET}/year/2026/month/01/")
                }
            }),
            prefix: "year/2026/month/01",
            suffix: RESULT_OBJECT_ROWS_SUFFIX,
            metadata: true,
            body: Some("\"region\"\n\"east\"\n"),
        },
        Landing {
            case: "a location the workgroup supplies to a request that names none",
            sql: "SELECT 'west' AS region",
            request: json!({ "WorkGroup": "supplying" }),
            prefix: "supplied",
            suffix: RESULT_OBJECT_ROWS_SUFFIX,
            metadata: true,
            body: Some("\"region\"\n\"west\"\n"),
        },
        Landing {
            case: "a workgroup location overriding the one the request named",
            sql: "SELECT 'central' AS region",
            request: json!({
                "WorkGroup": "enforcing",
                "ResultConfiguration": { "OutputLocation": format!("s3://{DECOY_BUCKET}/ignored/") },
            }),
            prefix: "enforced",
            suffix: RESULT_OBJECT_ROWS_SUFFIX,
            metadata: true,
            body: Some("\"region\"\n\"central\"\n"),
        },
        Landing {
            case: "a statement that defines a table, whose result is text output",
            sql: "CREATE TABLE saved_results (id integer)",
            request: json!({
                "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/defining/") }
            }),
            prefix: "defining",
            suffix: RESULT_OBJECT_TEXT_SUFFIX,
            metadata: false,
            // A definition statement produced no rows, so the text output saved for it
            // holds nothing: the object is there, and empty.
            body: Some(""),
        },
        Landing {
            case: "a utility statement, whose result is text output",
            sql: "EXPLAIN SELECT 1",
            request: json!({
                "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/utility/") }
            }),
            prefix: "utility",
            suffix: RESULT_OBJECT_TEXT_SUFFIX,
            metadata: false,
            // A plan's text is the engine's own, so the row pins the object rather than
            // the engine's wording (the suite below pins the bytes of a result set whose
            // values this emulator decides).
            body: None,
        },
    ];

    let mut started: Vec<(&Landing, String)> = Vec::new();
    for landing in &cases {
        let mut request = landing.request.clone();
        request["QueryString"] = json!(landing.sql);
        started.push((landing, start(&router, landing.case, &request).await));
    }

    for (landing, id) in &started {
        let case = landing.case;
        let found = succeeded(&router, case, id).await;
        let key = format!("{}/{id}{}", landing.prefix, landing.suffix);

        // The key the control plane NAMES is the key the data plane HOLDS.
        assert_eq!(
            found["ResultConfiguration"]["OutputLocation"].as_str(),
            Some(format!("s3://{BUCKET}/{key}").as_str()),
            "case {case:?}: expected the results object s3://{BUCKET}/{key}, got {found:?}"
        );

        let mut expected = vec![key.clone()];
        if landing.metadata {
            expected.push(format!("{key}{RESULT_METADATA_SUFFIX}"));
        }
        expected.sort();
        let listed = listed_keys(&router, BUCKET, landing.prefix).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing of s3://{BUCKET}/{} to hold {expected:?}, \
             got {listed:?}",
            landing.prefix
        );

        // And the object itself is readable at that key, through S3's own reader.
        let (status, body) = object(&router, BUCKET, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetObject(s3://{BUCKET}/{key}) must answer 200, got {status}"
        );
        if let Some(expected_body) = landing.body {
            assert_eq!(
                body, expected_body,
                "case {case:?}: expected the saved object to hold {expected_body:?}, got \
                 {body:?}"
            );
        }
    }

    // A location a workgroup's configuration overrode received nothing: the results went
    // where the resolved configuration said, and nowhere else.
    let decoy = listed_keys(&router, DECOY_BUCKET, "").await;
    assert!(
        decoy.is_empty(),
        "an overridden results location must stay empty, got {decoy:?}"
    );
}

/// The saved results object holds the ROWS the run produced: the column labels first and
/// the data rows under them, every present value quoted and every NULL left as an empty
/// field — the same values a read of those results reports.
///
/// The rows differ along what the statement produced: one column or several, one row or
/// several, a value carrying the field separator, a value carrying the quote character, a
/// NULL beside the empty string, and a computed value.
#[tokio::test]
async fn the_saved_results_hold_the_rows_the_run_produced() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;

    // (case, the SQL, the CSV the results object must hold, the column labels the metadata
    // saved beside it must describe, in projection order).
    let cases: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "one column and one row",
            "SELECT 'sim2real' AS marker",
            "\"marker\"\n\"sim2real\"\n",
            &["marker"],
        ),
        (
            "several columns and several rows",
            "SELECT 'desk' AS item, 1 AS id UNION ALL SELECT 'lamp' AS item, 2 AS id",
            "\"item\",\"id\"\n\"desk\",\"1\"\n\"lamp\",\"2\"\n",
            &["item", "id"],
        ),
        (
            "a value carrying the field separator",
            "SELECT 'Paris, France' AS place",
            "\"place\"\n\"Paris, France\"\n",
            &["place"],
        ),
        (
            "a value carrying the quote character",
            "SELECT 'a \"b\" c' AS quoted",
            "\"quoted\"\n\"a \"\"b\"\" c\"\n",
            &["quoted"],
        ),
        (
            "a NULL beside the empty string",
            "SELECT NULL AS absent, '' AS blank",
            "\"absent\",\"blank\"\n,\"\"\n",
            &["absent", "blank"],
        ),
        (
            "a computed value",
            "SELECT 2 + 3 AS computed",
            "\"computed\"\n\"5\"\n",
            &["computed"],
        ),
    ];

    let mut started: Vec<(&str, String, &str, &[&str])> = Vec::new();
    for (case, sql, expected, labels) in &cases {
        let request = json!({
            "QueryString": sql,
            "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/rows/") },
        });
        started.push((case, start(&router, case, &request).await, expected, labels));
    }

    for (case, id, expected, labels) in &started {
        succeeded(&router, case, id).await;
        let key = format!("rows/{id}{RESULT_OBJECT_ROWS_SUFFIX}");
        let (status, body) = object(&router, BUCKET, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetObject(s3://{BUCKET}/{key}) must answer 200, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body, *expected,
            "case {case:?}: expected the results object to hold {expected:?}, got {body:?}"
        );

        // The metadata saved beside it describes that same result set, column by column.
        let (status, metadata) =
            object(&router, BUCKET, &format!("{key}{RESULT_METADATA_SUFFIX}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the metadata beside {key} must be readable, got {status} with \
             {metadata:?}"
        );
        let document: Value = serde_json::from_str(&metadata).unwrap_or(Value::Null);
        // Every column of the result set, under the label it was saved in the CSV under.
        let described: Vec<String> = document["ColumnInfo"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|column| {
                column["Label"]
                    .as_str()
                    .or_else(|| column["Name"].as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            described, *labels,
            "case {case:?}: expected the metadata to describe the columns {labels:?}, got \
             {metadata:?}"
        );
    }
}

/// The location a query's results are saved under is the workgroup's configuration in
/// force AT SUBMIT TIME, not the one the workgroup was created with: a workgroup whose
/// output location is removed runs no query at all, and one restored under a new prefix
/// sends the next query's results there — while everything already saved stays where it
/// was saved.
#[tokio::test]
async fn results_follow_the_workgroup_configuration_in_force_at_submit_time() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    let work_group = "lifecycle";
    create_work_group(
        &router,
        work_group,
        json!({ "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/first/") } }),
    )
    .await;

    // The statement every step submits: the same query, so what differs between the steps
    // is only the workgroup's configuration at the instant of the submit.
    let sql = "SELECT 'marker' AS marker";
    let submission = json!({ "QueryString": sql, "WorkGroup": work_group });

    // Step one: the location the workgroup was created with.
    let first = start(&router, "the created location", &submission).await;

    // Step two: the location removed. A query has nowhere to write its results, so it is
    // refused rather than run.
    update_work_group(
        &router,
        work_group,
        json!({ "ResultConfigurationUpdates": { "RemoveOutputLocation": true } }),
    )
    .await;
    let (status, refused) = athena(&router, "StartQueryExecution", &submission).await;
    assert_eq!(
        status, 400,
        "with its location removed the workgroup runs no query, got {status} with {refused:?}"
    );

    // Step three: the location restored under a NEW prefix.
    update_work_group(
        &router,
        work_group,
        json!({
            "ResultConfigurationUpdates": { "OutputLocation": format!("s3://{BUCKET}/second/") }
        }),
    )
    .await;
    let second = start(&router, "the restored location", &submission).await;

    // (case, the execution, the prefix its results must be saved under).
    let expected: Vec<(&str, &String, &str)> = vec![
        ("the created location", &first, "first"),
        ("the restored location", &second, "second"),
    ];
    for (case, id, prefix) in &expected {
        succeeded(&router, case, id).await;
        let key = format!("{prefix}/{id}{RESULT_OBJECT_ROWS_SUFFIX}");
        let listed = listed_keys(&router, BUCKET, prefix).await;
        assert_eq!(
            listed,
            vec![key.clone(), format!("{key}{RESULT_METADATA_SUFFIX}")],
            "case {case:?}: expected s3://{BUCKET}/{prefix} to hold {key} and its metadata, \
             got {listed:?}"
        );
        let (status, body) = object(&router, BUCKET, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetObject(s3://{BUCKET}/{key}) must answer 200, got {status}"
        );
        assert_eq!(
            body, "\"marker\"\n\"marker\"\n",
            "case {case:?}: expected the rows the run produced, got {body:?}"
        );
    }
}
