// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an accepted import task RUNS: the data it reads out of
//! the account's object store, the elements it loads into the target graph, and the
//! progression and counts `GetImportTask` reports for that run — driven over the wire
//! (`restJson1`) through the same router the emulator serves from, with the emulator's
//! own Amazon S3 mounted beside this service on ONE engine exactly as the assembled
//! router mounts them.
//!
//! The rule every assertion here is about: an accepted import READS the data its
//! `source` names out of that object store in the `format` the request asked for,
//! LOADS the elements it holds into the graph it names, and reports that run — the
//! modelled stages `INITIALIZING`, `ANALYZING_DATA`, `IMPORTING` and then the terminal
//! `SUCCEEDED`, with `importTaskDetails` counting what the load wrote and no
//! `statusReason` to explain. A run that could not read its source, and one whose data
//! carried an error `failOnError` left it no choice about ("If set to true, the task
//! halts when an import error is encountered. If set to false, the task skips the data
//! that caused the error and continues if possible",
//! `smithy_query(query="doc",shape="StartImportTaskInput",member="failOnError")`),
//! stops `FAILED` and loads nothing.
//!
//! So the table spans what the rule generalises over: each `Format` the model declares
//! (`smithy_query(query="shape",shape="Format")` -> `CSV | NTRIPLES | OPEN_CYPHER |
//! PARQUET`), a source naming a prefix of several objects and one naming a single
//! object, node files and edge files, a multi-labelled node, typed and untyped
//! properties, the Neptune import options' own export path, both settings of
//! `failOnError` over the same unreadable row, a location the object store does not
//! hold, and a source that is no object-store location at all. Every element the load
//! writes is read back through `GetGraphSummary`, which computes its report from the
//! nodes and edges the graph really holds — so a task reporting `SUCCEEDED` is a graph
//! holding the data.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    COMPLETE_PROGRESS_PERCENTAGE, IMPORT_TASK_SPAN, MAX_IMPORT_COLUMNS, MAX_IMPORT_SOURCE_OBJECTS,
    MAX_LABELS_PER_NODE, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// The bucket every load's objects are staged in.
const BUCKET: &str = "neptune-import-fixtures";

/// An emulator holding this service and the emulator's own Amazon S3 on one engine, so
/// an object staged through Amazon S3's own `PutObject` is the object an import reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// An Amazon S3 client over this emulator's own in-process callback — the SAME
/// construction a load reads its source through
/// ([`service_common::s3_client::s3_client`], which `src/state/import_load.rs` uses),
/// so an object staged with this client is the object that load reads.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    service_common::s3_client::s3_client(&callback, REGION)
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create the bucket every load's objects are staged in, through Amazon S3's own
/// `CreateBucket`.
async fn create_bucket(router: &MultiServiceRouter) {
    let created = s3(router).create_bucket().bucket(BUCKET).send().await;
    assert!(
        created.is_ok(),
        "Amazon S3 must create the fixture bucket, got {created:?}"
    );
}

/// Stage one source object, through Amazon S3's own `PutObject`.
async fn stage(router: &MultiServiceRouter, key: &str, body: Vec<u8>) {
    let put = s3(router)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "Amazon S3 must hold the source object {key}, got {put:?}"
    );
}

/// `POST /graphs` — mint a graph, without waiting for it to come into service.
async fn create_graph(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        &[],
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case graph {name}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case graph {name}: no id in {created}"))
        .to_string()
}

/// Poll `path` until it reports `expected`, and answer that read. Nothing in the
/// service sleeps for the spans the clock owes; only this test does.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    let mut last = Value::Null;
    for _ in 0..80 {
        let (status, body) = call(router, "GET", path, &[], None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GET {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        last = body;
        tokio::time::sleep(IMPORT_TASK_SPAN / 4).await;
    }
    panic!("case {case:?}: GET {path} never reported {expected}, last read {last}");
}

/// `GET /summary` for one graph — the report computed from the nodes and edges the
/// graph really holds. The graph is named in the `graphIdentifier` HEADER
/// (`smithy_query(query="operation",operation="GetGraphSummary")` -> input).
async fn summary(router: &MultiServiceRouter, case: &str, graph_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        "/summary",
        &[("graphIdentifier", graph_id)],
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetGraphSummary, got {status}: {body}"
    );
    body["graphSummary"].clone()
}

/// `POST /graphs/{graphIdentifier}/importtasks` with `body`, asserting the task is
/// accepted and reports the modelled `INITIALIZING` status, and answering its id.
async fn start_import(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    body: Value,
) -> String {
    let (status, started) = call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        &[],
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from StartImportTask of {body}, got {status}: {started}"
    );
    assert_eq!(
        started["status"],
        json!("INITIALIZING"),
        "case {case:?}: no element has been loaded when the call returns, expected \
         INITIALIZING, got {}",
        started["status"]
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
        .to_string()
}

/// The Gremlin CSV node file every `CSV` row below loads: the header columns Neptune's
/// Gremlin load format names (`~id`, `~label`, then `key:Type` properties), a
/// multi-labelled node, and a typed property beside an untyped one.
const GREMLIN_NODES: &str = "~id,~label,name:String,age:Int\n\
                             n1,person,Alice,33\n\
                             n2,person;employee,Bob,41\n";

/// The Gremlin CSV edge file: an edge names both of its endpoints (`~from`, `~to`) and
/// its own label.
const GREMLIN_EDGES: &str = "~id,~from,~to,~label,since:Int\ne1,n1,n2,knows,2019\n";

/// The openCypher node file: the same elements spelled with that format's own columns
/// (`:ID`, `:LABEL`).
const OPEN_CYPHER_NODES: &str = ":ID,:LABEL,name:String\nc1,city,Berlin\nc2,city,Paris\n";

/// The openCypher relationship file (`:START_ID`, `:END_ID`, `:TYPE`).
const OPEN_CYPHER_EDGES: &str = ":ID,:START_ID,:END_ID,:TYPE\nr1,c1,c2,route\n";

/// The RDF N-Triples document: `rdf:type` gives the subject a label, a literal object
/// sets a property, and a resource object joins the two nodes with an edge.
const NTRIPLES: &str = "<http://example.com/a> \
                        <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                        <http://example.com/Person> .\n\
                        <http://example.com/a> <http://example.com/name> \"Ada\" .\n\
                        <http://example.com/a> <http://example.com/knows> \
                        <http://example.com/b> .\n";

/// A Gremlin CSV node file whose `age:Int` column holds a value that is not an integer
/// in one row — the one unreadable row both `failOnError` rows below load.
const GREMLIN_NODES_WITH_A_BAD_ROW: &str = "~id,~label,age:Int\nn1,person,33\nn2,person,abc\n";

/// The bytes of a Parquet file holding `rows` under `columns`, every column a REQUIRED
/// UTF-8 string, which is how Neptune's columnar load format spells the same `~id` and
/// `~label` columns the Gremlin CSV format names.
fn parquet_object(columns: &[&str], rows: &[Vec<&str>]) -> Vec<u8> {
    use parquet::basic::{ConvertedType, Repetition, Type as PhysicalType};
    use parquet::data_type::{ByteArray, ByteArrayType};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::types::Type as SchemaType;

    let fields = columns
        .iter()
        .map(|column| {
            Arc::new(
                SchemaType::primitive_type_builder(column, PhysicalType::BYTE_ARRAY)
                    .with_repetition(Repetition::REQUIRED)
                    .with_converted_type(ConvertedType::UTF8)
                    .build()
                    .expect("the fixture column builds"),
            )
        })
        .collect::<Vec<_>>();
    let schema = SchemaType::group_type_builder("elements")
        .with_fields(fields)
        .build()
        .expect("the fixture schema builds");
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        Arc::new(schema),
        Arc::new(WriterProperties::builder().build()),
    )
    .expect("the fixture writer builds");
    {
        let mut row_group = writer.next_row_group().expect("a row group");
        for index in 0..columns.len() {
            let mut column = row_group
                .next_column()
                .expect("a column")
                .expect("the schema keeps every column");
            let values: Vec<ByteArray> = rows
                .iter()
                .map(|row| ByteArray::from(row[index].as_bytes().to_vec()))
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&values, None, None)
                .expect("the fixture values write");
            column.close().expect("the column closes");
        }
        row_group.close().expect("the row group closes");
    }
    writer
        .into_inner()
        .expect("the fixture file closes")
        .to_vec()
}

/// One row of the load table: the objects it stages, the request it sends, and what the
/// run it describes writes.
struct LoadCase {
    case: &'static str,
    /// The objects staged under this row's own prefix, as (key suffix, bytes).
    objects: Vec<(&'static str, Vec<u8>)>,
    /// The `source` the request names, relative to the bucket.
    source: String,
    /// The members the request carries beside `source` and `roleArn`.
    extra: Value,
    /// The statements the load writes: one per label, one per property, and one per
    /// edge.
    statements: i64,
    /// The distinct values the load interns: element ids, labels, property keys and
    /// property values.
    dictionary_entries: i64,
    nodes: i64,
    edges: i64,
    /// The distinct node labels the loaded graph holds.
    node_labels: Vec<&'static str>,
}

/// An accepted import reads the data its source holds, loads it into the graph, and
/// reports that run: the terminal `SUCCEEDED`, the whole of the work done, the counts
/// the load measured, and no reason to explain — over a row per bulk-load format and
/// source form.
#[tokio::test]
async fn an_accepted_import_loads_the_data_its_source_holds() {
    let router = emulator();
    create_bucket(&router).await;

    let cases = [
        LoadCase {
            case: "Gremlin CSV node and edge files under one prefix",
            objects: vec![
                ("nodes.csv", GREMLIN_NODES.as_bytes().to_vec()),
                ("edges.csv", GREMLIN_EDGES.as_bytes().to_vec()),
            ],
            source: format!("s3://{BUCKET}/csv/"),
            extra: json!({"format": "CSV"}),
            // n1: one label and two properties; n2: two labels and two properties;
            // e1: the edge and one property.
            statements: 9,
            // n1, n2, e1, person, employee, knows, name, age, since, "Alice", 33,
            // "Bob", 41, 2019.
            dictionary_entries: 14,
            nodes: 2,
            edges: 1,
            node_labels: vec!["employee", "person"],
        },
        LoadCase {
            case: "openCypher node and relationship files under one prefix",
            objects: vec![
                ("nodes.csv", OPEN_CYPHER_NODES.as_bytes().to_vec()),
                ("edges.csv", OPEN_CYPHER_EDGES.as_bytes().to_vec()),
            ],
            source: format!("s3://{BUCKET}/opencypher/"),
            extra: json!({"format": "OPEN_CYPHER"}),
            // Two nodes with a label and a property each, and one edge.
            statements: 5,
            // c1, c2, r1, city, route, name, "Berlin", "Paris".
            dictionary_entries: 8,
            nodes: 2,
            edges: 1,
            node_labels: vec!["city"],
        },
        LoadCase {
            case: "a columnar object named directly, not by its prefix",
            objects: vec![(
                "parquet/part-0.parquet",
                parquet_object(
                    &["~id", "~label", "name"],
                    &[
                        vec!["p1", "product", "Widget"],
                        vec!["p2", "product", "Gadget"],
                    ],
                ),
            )],
            source: format!("s3://{BUCKET}/parquet/part-0.parquet"),
            extra: json!({"format": "PARQUET", "parquetType": "COLUMNAR"}),
            // Two nodes with a label and a property each.
            statements: 4,
            // p1, p2, product, name, "Widget", "Gadget".
            dictionary_entries: 6,
            nodes: 2,
            edges: 0,
            node_labels: vec!["product"],
        },
        LoadCase {
            case: "RDF N-Triples",
            objects: vec![("data.nt", NTRIPLES.as_bytes().to_vec())],
            source: format!("s3://{BUCKET}/ntriples/"),
            // `blankNodeHandling` "Must be provided when format is ntriples".
            extra: json!({"format": "NTRIPLES", "blankNodeHandling": "convertToIri"}),
            // The subject's label and property, and the edge to the object.
            statements: 3,
            // The two subject/object IRIs, the edge's own id, the type IRI the label
            // is, the predicate the edge's label is, the predicate the property key
            // is, and "Ada".
            dictionary_entries: 7,
            nodes: 2,
            edges: 1,
            node_labels: vec!["http://example.com/Person"],
        },
        LoadCase {
            case: "a Neptune source read through the import options' export path",
            objects: vec![("neptune-export/nodes.csv", GREMLIN_NODES.as_bytes().to_vec())],
            source: "my-cluster.cluster-abc123.us-east-1.neptune.amazonaws.com".to_string(),
            extra: json!({
                "format": "CSV",
                "importOptions": {
                    "neptune": {
                        "s3ExportPath": format!("s3://{BUCKET}/neptune-export/"),
                        "s3ExportKmsKeyId": "alias/aws/s3",
                    },
                },
            }),
            // The two nodes of the Gremlin node file, with no edge file beside it.
            statements: 7,
            // n1, n2, person, employee, name, age, "Alice", 33, "Bob", 41.
            dictionary_entries: 10,
            nodes: 2,
            edges: 0,
            node_labels: vec!["employee", "person"],
        },
    ];

    // Every row's objects are staged and its graph minted first: a graph comes into
    // service on the clock, so the rows provision together rather than one span apiece.
    let mut graphs: Vec<String> = Vec::new();
    for (index, row) in cases.iter().enumerate() {
        for (key, bytes) in &row.objects {
            let prefix = row
                .source
                .strip_prefix(&format!("s3://{BUCKET}/"))
                .unwrap_or_default();
            let key = if key.starts_with(prefix) && !prefix.is_empty() {
                (*key).to_string()
            } else {
                format!("{prefix}{key}")
            };
            stage(&router, &key, bytes.clone()).await;
        }
        graphs.push(create_graph(&router, &format!("import-run-{index}")).await);
    }
    for (row, graph_id) in cases.iter().zip(&graphs) {
        wait_for_status(&router, row.case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    }

    // Then every row's import is accepted, so the runs overlap instead of this test
    // waiting out one run per row.
    let mut tasks: Vec<String> = Vec::new();
    for (row, graph_id) in cases.iter().zip(&graphs) {
        let mut body = json!({"roleArn": ROLE_ARN, "source": row.source});
        for (member, value) in row.extra.as_object().expect("the extra members") {
            body[member] = value.clone();
        }
        tasks.push(start_import(&router, row.case, graph_id, body).await);
    }

    for ((row, graph_id), task_id) in cases.iter().zip(&graphs).zip(&tasks) {
        let case = row.case;
        let settled =
            wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "SUCCEEDED").await;
        assert_eq!(
            settled["statusReason"],
            Value::Null,
            "case {case:?}: a run that loaded its data has nothing to explain, got {}",
            settled["statusReason"]
        );
        let details = &settled["importTaskDetails"];
        assert_eq!(
            details["status"],
            json!("SUCCEEDED"),
            "case {case:?}: the details report the status the task is in, got {}",
            details["status"]
        );
        assert_eq!(
            details["progressPercentage"],
            json!(COMPLETE_PROGRESS_PERCENTAGE),
            "case {case:?}: a completed run has done all of its work, got {}",
            details["progressPercentage"]
        );
        for (member, expected) in [
            ("statementCount", row.statements),
            ("dictionaryEntryCount", row.dictionary_entries),
        ] {
            assert_eq!(
                details[member],
                json!(expected),
                "case {case:?}: expected {member} {expected} for the data the load wrote, \
                 got {}",
                details[member]
            );
        }
        assert_eq!(
            details["errorCount"],
            json!(0),
            "case {case:?}: every row of this source reads, so no error was met, got {}",
            details["errorCount"]
        );
        assert_eq!(
            details["errorDetails"],
            Value::Null,
            "case {case:?}: a run that met no error has no error to detail, got {}",
            details["errorDetails"]
        );
        assert!(
            details["startTime"].is_number(),
            "case {case:?}: the details report when the run started, got {}",
            details["startTime"]
        );
        assert!(
            details["timeElapsedSeconds"].as_i64().unwrap_or(-1) >= 0,
            "case {case:?}: the details report how long the run has been going, got {}",
            details["timeElapsedSeconds"]
        );

        // The graph is serving again, and what it holds is what the load wrote: the
        // summary is computed from the graph's own nodes and edges.
        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        let graph_summary = summary(&router, case, graph_id).await;
        for (member, expected) in [("numNodes", row.nodes), ("numEdges", row.edges)] {
            assert_eq!(
                graph_summary[member],
                json!(expected),
                "case {case:?}: expected {member} {expected} in the loaded graph, got {}",
                graph_summary[member]
            );
        }
        assert_eq!(
            graph_summary["nodeLabels"],
            json!(row.node_labels),
            "case {case:?}: expected the labels the load wrote, got {}",
            graph_summary["nodeLabels"]
        );
    }
}

/// `failOnError` decides the fate of a row the load cannot read: the default halts the
/// task, so it stops `FAILED` and the graph stays empty, while `false` skips that row
/// and the rest of the data is loaded — with the error counted and detailed either way.
#[tokio::test]
async fn fail_on_error_decides_the_fate_of_a_row_the_load_cannot_read() {
    let router = emulator();
    create_bucket(&router).await;
    stage(
        &router,
        "bad-row/nodes.csv",
        GREMLIN_NODES_WITH_A_BAD_ROW.as_bytes().to_vec(),
    )
    .await;

    // (case, the failOnError the request carries, the terminal status, the statements
    // the load writes, the nodes the graph ends up holding)
    let cases: Vec<(&str, Option<bool>, &str, i64, i64)> = vec![
        ("failOnError omitted, which defaults to true", None, "FAILED", 0, 0),
        ("failOnError true", Some(true), "FAILED", 0, 0),
        // n1 keeps its label and its readable property; n2 keeps its label and loses
        // only the value that could not be read.
        ("failOnError false", Some(false), "SUCCEEDED", 3, 2),
    ];

    let mut graphs: Vec<String> = Vec::new();
    for index in 0..cases.len() {
        graphs.push(create_graph(&router, &format!("fail-on-error-{index}")).await);
    }
    for (row, graph_id) in cases.iter().zip(&graphs) {
        wait_for_status(&router, row.0, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    }

    let mut tasks: Vec<String> = Vec::new();
    for ((case, fail_on_error, _, _, _), graph_id) in cases.iter().zip(&graphs) {
        let mut body = json!({
            "roleArn": ROLE_ARN,
            "source": format!("s3://{BUCKET}/bad-row/"),
            "format": "CSV",
        });
        if let Some(fail_on_error) = fail_on_error {
            body["failOnError"] = json!(fail_on_error);
        }
        tasks.push(start_import(&router, case, graph_id, body).await);
    }

    for (((case, _, terminal, statements, nodes), graph_id), task_id) in
        cases.iter().zip(&graphs).zip(&tasks)
    {
        let settled =
            wait_for_status(&router, case, &format!("/importtasks/{task_id}"), terminal).await;
        let details = &settled["importTaskDetails"];
        assert_eq!(
            details["errorCount"],
            json!(1),
            "case {case:?}: one row of this source does not read, got {}",
            details["errorCount"]
        );
        assert!(
            details["errorDetails"].is_string(),
            "case {case:?}: a run that met an error says what the first one was, got {}",
            details["errorDetails"]
        );
        assert_eq!(
            details["statementCount"],
            json!(statements),
            "case {case:?}: expected statementCount {statements} for what the load wrote, \
             got {}",
            details["statementCount"]
        );
        if *terminal == "FAILED" {
            assert!(
                settled["statusReason"].is_string(),
                "case {case:?}: a task that stopped must say why, got {}",
                settled["statusReason"]
            );
            assert!(
                details["progressPercentage"].as_i64().unwrap_or(-1)
                    < i64::from(COMPLETE_PROGRESS_PERCENTAGE),
                "case {case:?}: a run that did not finish has not done all of its work, \
                 got {}",
                details["progressPercentage"]
            );
        } else {
            assert_eq!(
                settled["statusReason"],
                Value::Null,
                "case {case:?}: a run that skipped the row and finished has nothing to \
                 explain, got {}",
                settled["statusReason"]
            );
        }

        // What the graph holds afterwards is what the load wrote: nothing at all for a
        // run that halted, and the rows it could read for one that skipped.
        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        let graph_summary = summary(&router, case, graph_id).await;
        assert_eq!(
            graph_summary["numNodes"],
            json!(nodes),
            "case {case:?}: expected numNodes {nodes} in the graph the load wrote, got {}",
            graph_summary["numNodes"]
        );
    }
}

/// A source the object store does not hold is a run that could not read anything: the
/// task stops `FAILED` saying so, reports no count of work nothing did, and the graph
/// it was to load stays empty and comes back into service.
#[tokio::test]
async fn a_source_that_cannot_be_read_stops_the_task_failed_and_loads_nothing() {
    let router = emulator();
    create_bucket(&router).await;
    stage(&router, "held/nodes.csv", GREMLIN_NODES.as_bytes().to_vec()).await;

    // (case, the source the request names)
    let cases: Vec<(&str, String)> = vec![
        (
            "a prefix of the bucket that holds nothing",
            format!("s3://{BUCKET}/not-staged/"),
        ),
        (
            "a bucket the object store does not hold",
            "s3://no-such-import-bucket/data/".to_string(),
        ),
        (
            "a location that is no object-store location at all",
            "https://example.com/data/".to_string(),
        ),
        (
            "a Neptune endpoint with no import options naming an export path",
            "my-cluster.cluster-abc123.us-east-1.neptune.amazonaws.com".to_string(),
        ),
    ];

    let mut graphs: Vec<String> = Vec::new();
    for index in 0..cases.len() {
        graphs.push(create_graph(&router, &format!("unreadable-source-{index}")).await);
    }
    for ((case, _), graph_id) in cases.iter().zip(&graphs) {
        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    }

    let mut tasks: Vec<String> = Vec::new();
    for ((case, source), graph_id) in cases.iter().zip(&graphs) {
        tasks.push(
            start_import(
                &router,
                case,
                graph_id,
                json!({"roleArn": ROLE_ARN, "source": source, "format": "CSV"}),
            )
            .await,
        );
    }

    for (((case, _), graph_id), task_id) in cases.iter().zip(&graphs).zip(&tasks) {
        let settled =
            wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "FAILED").await;
        assert!(
            settled["statusReason"].is_string(),
            "case {case:?}: a task that could not read its source must say so, got {}",
            settled["statusReason"]
        );
        let details = &settled["importTaskDetails"];
        for member in ["statementCount", "dictionaryEntryCount", "errorCount"] {
            assert_eq!(
                details[member],
                json!(0),
                "case {case:?}: nothing was read, so {member} counts nothing, got {}",
                details[member]
            );
        }

        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        let graph_summary = summary(&router, case, graph_id).await;
        for member in ["numNodes", "numEdges"] {
            assert_eq!(
                graph_summary[member],
                json!(0),
                "case {case:?}: a run that loaded nothing leaves the graph empty, got {}",
                graph_summary[member]
            );
        }
    }
}

/// `CreateGraphUsingImportTask` runs the same load: the graph it mints comes into
/// service holding what that load wrote, and the task it minted reports the run.
#[tokio::test]
async fn create_graph_using_import_task_brings_its_graph_up_holding_what_the_load_wrote() {
    let router = emulator();
    create_bucket(&router).await;
    stage(&router, "created/nodes.csv", GREMLIN_NODES.as_bytes().to_vec()).await;
    stage(&router, "created/edges.csv", GREMLIN_EDGES.as_bytes().to_vec()).await;

    let case = "a graph created with its own load";
    let (status, created) = call(
        &router,
        "POST",
        "/importtasks",
        &[],
        Some(json!({
            "graphName": "created-with-its-load",
            "roleArn": ROLE_ARN,
            "source": format!("s3://{BUCKET}/created/"),
            "format": "CSV",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphUsingImportTask, got {status}: {created}"
    );
    let task_id = created["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {created}"))
        .to_string();
    let graph_id = created["graphId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graphId in {created}"))
        .to_string();

    // The graph is IMPORTING for as long as its only load runs, so a caller reading it
    // straight after the call sees that rather than a graph claiming to be ready.
    let (early_status, early) = call(&router, "GET", &format!("/graphs/{graph_id}"), &[], None).await;
    assert_eq!(
        early_status, 200,
        "case {case:?}: expected 200 from GetGraph, got {early_status}: {early}"
    );
    assert_eq!(
        early["status"],
        json!("IMPORTING"),
        "case {case:?}: a graph read while its load runs is IMPORTING, got {}",
        early["status"]
    );

    let settled =
        wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "SUCCEEDED").await;
    assert_eq!(
        settled["graphId"],
        json!(graph_id.clone()),
        "case {case:?}: the task loads the graph the same call created, got {}",
        settled["graphId"]
    );
    assert_eq!(
        settled["importTaskDetails"]["progressPercentage"],
        json!(COMPLETE_PROGRESS_PERCENTAGE),
        "case {case:?}: a completed run has done all of its work, got {}",
        settled["importTaskDetails"]["progressPercentage"]
    );

    // The graph comes into service holding the elements that load wrote, so the pair of
    // records agrees: the graph is ready and its load did run.
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let graph_summary = summary(&router, case, &graph_id).await;
    for (member, expected) in [("numNodes", 2), ("numEdges", 1)] {
        assert_eq!(
            graph_summary[member],
            json!(expected),
            "case {case:?}: expected {member} {expected} in the created graph, got {}",
            graph_summary[member]
        );
    }
    let node_properties: BTreeMap<String, i64> = graph_summary["nodeProperties"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_object())
                .flat_map(|entry| {
                    entry
                        .iter()
                        .map(|(key, value)| (key.clone(), value.as_i64().unwrap_or_default()))
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        node_properties.get("name").copied(),
        Some(2),
        "case {case:?}: both loaded nodes carry the name property, got {node_properties:?}"
    );
}

/// A node takes at most the labels this emulator loads onto one, and the labels past
/// that ceiling are errors of the row the load skips rather than silent growth — the
/// same treatment a row's properties past their own ceiling get.
#[tokio::test]
async fn a_node_takes_at_most_the_labels_one_element_may_carry() {
    let router = emulator();
    create_bucket(&router).await;

    // One node row carrying three labels more than one node may hold, each distinct, so
    // the only thing wrong with the row is how many labels it names.
    let over = MAX_LABELS_PER_NODE + 3;
    let labels: Vec<String> = (0..over).map(|index| format!("label{index:03}")).collect();
    let object = format!("~id,~label\nn1,{}\n", labels.join(";"));
    stage(&router, "labels/nodes.csv", object.into_bytes()).await;

    let case = "a node row naming more labels than one node may hold";
    let graph_id = create_graph(&router, "label-ceiling").await;
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let task_id = start_import(
        &router,
        case,
        &graph_id,
        json!({
            "roleArn": ROLE_ARN,
            "source": format!("s3://{BUCKET}/labels/"),
            "format": "CSV",
            // The labels past the ceiling are import errors, so the run completes only
            // where the request asks for the ones it can read to be skipped.
            "failOnError": false,
        }),
    )
    .await;

    let settled =
        wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "SUCCEEDED").await;
    assert_eq!(
        settled["importTaskDetails"]["errorCount"],
        json!(1),
        "case {case:?}: the labels past the ceiling are ONE error of the row that named \
         them, got {}",
        settled["importTaskDetails"]["errorCount"]
    );

    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let graph_summary = summary(&router, case, &graph_id).await;
    assert_eq!(
        graph_summary["numNodes"],
        json!(1),
        "case {case:?}: the row names one node, got {}",
        graph_summary["numNodes"]
    );
    assert_eq!(
        graph_summary["numNodeLabels"],
        json!(MAX_LABELS_PER_NODE),
        "case {case:?}: expected the {MAX_LABELS_PER_NODE} labels one node may carry, got {}",
        graph_summary["numNodeLabels"]
    );
}

/// A load object past one of the ceilings this emulator reads an import under stops the
/// run: the task reports `FAILED` with a reason naming the bound it broke, and the graph
/// it was to load stays empty — over a row per ceiling a small object can reach.
#[tokio::test]
async fn an_object_past_a_ceiling_this_emulator_reads_under_stops_the_run() {
    let router = emulator();
    create_bucket(&router).await;

    // (case, key, the object staged under this row's own prefix)
    // A header declaring more columns than one element can be stored with, and a source
    // holding more objects than one import reads: both are small objects that reach a
    // bound the service fixes.
    let wide_header = (0..=MAX_IMPORT_COLUMNS)
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let wide = format!("~id,{wide_header}\nn1,{}\n", ",".repeat(MAX_IMPORT_COLUMNS));
    stage(&router, "wide/nodes.csv", wide.into_bytes()).await;
    for index in 0..=MAX_IMPORT_SOURCE_OBJECTS {
        stage(
            &router,
            &format!("many/nodes-{index:03}.csv"),
            GREMLIN_NODES.as_bytes().to_vec(),
        )
        .await;
    }

    for (case, prefix) in [
        ("an object declaring more columns than one element can hold", "wide"),
        ("a source holding more objects than one import reads", "many"),
    ] {
        let graph_id = create_graph(&router, &format!("ceiling-{prefix}")).await;
        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        let task_id = start_import(
            &router,
            case,
            &graph_id,
            json!({
                "roleArn": ROLE_ARN,
                "source": format!("s3://{BUCKET}/{prefix}/"),
                "format": "CSV",
            }),
        )
        .await;

        let settled =
            wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "FAILED").await;
        assert!(
            settled["statusReason"].is_string(),
            "case {case:?}: a run stopped by a ceiling must say which bound it broke, got {}",
            settled["statusReason"]
        );
        assert_eq!(
            settled["importTaskDetails"]["statementCount"],
            json!(0),
            "case {case:?}: a run that stopped wrote no statement, got {}",
            settled["importTaskDetails"]["statementCount"]
        );

        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        let graph_summary = summary(&router, case, &graph_id).await;
        for member in ["numNodes", "numEdges"] {
            assert_eq!(
                graph_summary[member],
                json!(0),
                "case {case:?}: a run that stopped leaves the graph empty, got {}",
                graph_summary[member]
            );
        }
    }
}

/// A cancelled import loads nothing: the elements its run had read are released with
/// it, so the graph it was filling stays as empty as the task found it.
#[tokio::test]
async fn a_cancelled_import_loads_nothing() {
    let router = emulator();
    create_bucket(&router).await;
    stage(
        &router,
        "cancelled/nodes.csv",
        GREMLIN_NODES.as_bytes().to_vec(),
    )
    .await;

    let case = "a load cancelled before it landed";
    let graph_id = create_graph(&router, "cancelled-load").await;
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let task_id = start_import(
        &router,
        case,
        &graph_id,
        json!({
            "roleArn": ROLE_ARN,
            "source": format!("s3://{BUCKET}/cancelled/"),
            "format": "CSV",
        }),
    )
    .await;

    let (cancel_status, cancelled) = call(
        &router,
        "DELETE",
        &format!("/importtasks/{task_id}"),
        &[],
        None,
    )
    .await;
    assert_eq!(
        cancel_status, 200,
        "case {case:?}: expected 200 from CancelImportTask, got {cancel_status}: {cancelled}"
    );
    assert_eq!(
        cancelled["status"],
        json!("CANCELLING"),
        "case {case:?}: an accepted cancellation reports CANCELLING, got {}",
        cancelled["status"]
    );

    let settled =
        wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "CANCELLED").await;
    let details = &settled["importTaskDetails"];
    for member in ["statementCount", "dictionaryEntryCount"] {
        assert_eq!(
            details[member],
            json!(0),
            "case {case:?}: a cancelled load wrote nothing, so {member} is 0, got {}",
            details[member]
        );
    }

    // Long enough that the load the cancellation stopped would have landed by now.
    tokio::time::sleep(IMPORT_TASK_SPAN * 4 + PROVISIONING_SPAN).await;
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let graph_summary = summary(&router, case, &graph_id).await;
    for member in ["numNodes", "numEdges"] {
        assert_eq!(
            graph_summary[member],
            json!(0),
            "case {case:?}: a cancelled load leaves the graph empty, got {}",
            graph_summary[member]
        );
    }
}
