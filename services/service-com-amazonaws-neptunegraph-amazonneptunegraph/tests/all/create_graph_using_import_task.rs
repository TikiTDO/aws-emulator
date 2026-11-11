// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateGraphUsingImportTask`: the one call that mints a graph
//! AND the import task that is to populate it, read back through `GetGraph` and
//! `GetImportTask` — driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * `CreateGraphUsingImportTask` persists TWO records from one call — a graph named by
//!   the request and an import task against it — and reports the stored task: a minted
//!   `^t-[a-z0-9]{10}$` task id, the `^g-[a-z0-9]{10}$` id of the graph it created,
//!   every import parameter the request chose, and the modelled `INITIALIZING` status.
//!   `GetImportTask` answers that task record and `GetGraph` that graph record, which
//!   reports `IMPORTING` while the task is in flight.
//! * `minProvisionedMemory` / `maxProvisionedMemory` BOUND a value the service chooses
//!   rather than being it, so the graph's `provisionedMemory` is the floor of the range
//!   the request allows — and the documented 128 m-NCUs when the request bounds it
//!   neither way.
//! * The import itself is the same load `StartImportTask` runs, so with no object
//!   store mounted beside this service the source cannot be read: the clock stops the
//!   task in the modelled `FAILED` status carrying what the run met, and the graph this
//!   call provisioned comes into service empty. What a task that DOES read its source
//!   loads is the sibling suite `import_task_run`.
//! * A request the model cannot refuse at the boundary is refused here as a
//!   `ValidationException` (400): a `graphName` outside the pattern whose negative
//!   lookahead no boundary regex compiles, a blank or oversized `source`, an `NTRIPLES`
//!   import with no `blankNodeHandling`, and a minimum above the maximum. A repeated
//!   `graphName` is a `ConflictException` (409).
//!
//! So each table below spans what its rule generalises over — the import format and its
//! parquet type, the form of the source location, which memory bounds the request
//! carries (including the published range's own ends), the replica count at each
//! published bound, the tag entry count, whether the Neptune import options are
//! supplied, and the IAM role's path form — rather than re-running one recorded request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ARN_NAMESPACE, DEFAULT_MIN_PROVISIONED_MEMORY, MAX_IMPORT_SOURCE_CHARS, PROVISIONING_SPAN,
    SERVICE_OWNED_KMS_KEY, UNBOUNDED_IMPORT_PROVISIONED_MEMORY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneImportRole";

/// The same pattern's `role/service-role` form, so the table spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneImport";

/// The smallest `ProvisionedMemory` the model allows (`range 16..24576`), which is also
/// `minProvisionedMemory`'s documented default.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The largest `ProvisionedMemory` the model allows (`range 16..24576`).
const MAX_PROVISIONED_MEMORY: i64 = 24_576;

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
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

/// Percent-encode one path segment, so the `/` inside an ARN does not split the URI
/// into another path segment on the non-greedy `{resourceArn}` label.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_task_id(value: &str) -> bool {
    has_id_shape(value, "t-")
}

/// Whether `value` matches the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
fn is_graph_id(value: &str) -> bool {
    has_id_shape(value, "g-")
}

/// Whether `value` is `prefix` followed by exactly ten `[a-z0-9]` characters.
fn has_id_shape(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// `POST /importtasks` with `body` — the wire form of `CreateGraphUsingImportTask`.
async fn create_using_import(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/importtasks", Some(body)).await
}

/// `GET /importtasks/{taskIdentifier}`.
async fn get_import(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/importtasks/{task_id}"), None).await
}

/// `GET /graphs/{graphIdentifier}`.
async fn get_graph(router: &MultiServiceRouter, graph_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{graph_id}"), None).await
}

/// Poll one resource until its `status` is `expected`. Nothing in the service sleeps
/// for the span the clock owes; only this test does.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reached {expected}, last status {}",
        body["status"]
    );
}

/// One row of the create-and-read-back table.
struct ImportCase {
    case: &'static str,
    graph_name: &'static str,
    role_arn: &'static str,
    source: &'static str,
    format: Option<&'static str>,
    parquet_type: Option<&'static str>,
    blank_node_handling: Option<&'static str>,
    /// The `importOptions` the request carries, with every optional member spelled out
    /// so the echoed value is compared exactly.
    import_options: Option<Value>,
    min_provisioned_memory: Option<i64>,
    max_provisioned_memory: Option<i64>,
    /// What the service must therefore choose as the graph's `provisionedMemory`.
    expected_provisioned_memory: i64,
    replica_count: Option<i64>,
    public_connectivity: Option<bool>,
    deletion_protection: Option<bool>,
    kms_key_identifier: Option<&'static str>,
    vector_search_dimension: Option<i64>,
    tags: BTreeMap<String, String>,
}

impl ImportCase {
    /// A row with only the members the operation requires, for the table to vary from.
    fn new(case: &'static str, graph_name: &'static str, source: &'static str) -> Self {
        Self {
            case,
            graph_name,
            role_arn: ROLE_ARN,
            source,
            format: None,
            parquet_type: None,
            blank_node_handling: None,
            import_options: None,
            min_provisioned_memory: None,
            max_provisioned_memory: None,
            expected_provisioned_memory: i64::from(UNBOUNDED_IMPORT_PROVISIONED_MEMORY),
            replica_count: None,
            public_connectivity: None,
            deletion_protection: None,
            kms_key_identifier: None,
            vector_search_dimension: None,
            tags: BTreeMap::new(),
        }
    }

    /// This row as the request body it is sent as.
    fn request(&self) -> Value {
        let mut request = json!({
            "graphName": self.graph_name,
            "roleArn": self.role_arn,
            "source": self.source,
            "tags": self.tags,
        });
        for (member, value) in [
            ("format", self.format.map(|v| json!(v))),
            ("parquetType", self.parquet_type.map(|v| json!(v))),
            (
                "blankNodeHandling",
                self.blank_node_handling.map(|v| json!(v)),
            ),
            ("importOptions", self.import_options.clone()),
            (
                "minProvisionedMemory",
                self.min_provisioned_memory.map(|v| json!(v)),
            ),
            (
                "maxProvisionedMemory",
                self.max_provisioned_memory.map(|v| json!(v)),
            ),
            ("replicaCount", self.replica_count.map(|v| json!(v))),
            (
                "publicConnectivity",
                self.public_connectivity.map(|v| json!(v)),
            ),
            (
                "deletionProtection",
                self.deletion_protection.map(|v| json!(v)),
            ),
            (
                "kmsKeyIdentifier",
                self.kms_key_identifier.map(|v| json!(v)),
            ),
            (
                "vectorSearchConfiguration",
                self.vector_search_dimension
                    .map(|dimension| json!({ "dimension": dimension })),
            ),
        ] {
            if let Some(value) = value {
                request[member] = value;
            }
        }
        request
    }
}

/// The rows of the create-and-read-back table: each differs from the others along what
/// the rules generalise over, not in its name alone.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase::new(
            "a bare CSV import from an S3 prefix, no bounds and no options",
            "csv-from-s3",
            "s3://import-bucket/csv/",
        ),
        ImportCase {
            format: Some("CSV"),
            min_provisioned_memory: Some(256),
            max_provisioned_memory: Some(1024),
            expected_provisioned_memory: 256,
            replica_count: Some(0),
            public_connectivity: Some(true),
            deletion_protection: Some(true),
            kms_key_identifier: Some(KMS_KEY_ARN),
            vector_search_dimension: Some(1536),
            tags: [("env".to_string(), "dev".to_string())]
                .into_iter()
                .collect(),
            ..ImportCase::new(
                "CSV with both memory bounds, no replica, every switch on",
                "csv-bounded",
                "s3://import-bucket/csv/2026-10-24",
            )
        },
        ImportCase {
            format: Some("OPEN_CYPHER"),
            role_arn: SERVICE_ROLE_ARN,
            max_provisioned_memory: Some(2048),
            expected_provisioned_memory: i64::from(DEFAULT_MIN_PROVISIONED_MEMORY),
            replica_count: Some(2),
            tags: [
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "graph".to_string()),
                ("cost-centre".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
            ..ImportCase::new(
                "openCypher with only a maximum, the replica bound, a service role",
                "opencypher-max-only",
                "s3://import-bucket/opencypher",
            )
        },
        ImportCase {
            format: Some("PARQUET"),
            parquet_type: Some("COLUMNAR"),
            min_provisioned_memory: Some(MIN_PROVISIONED_MEMORY),
            expected_provisioned_memory: MIN_PROVISIONED_MEMORY,
            ..ImportCase::new(
                "PARQUET with its columnar type, at the bottom of the memory range",
                "parquet-columnar",
                "s3://import-bucket/parquet",
            )
        },
        ImportCase {
            format: Some("NTRIPLES"),
            // "Must be provided when format is ntriples"
            // (smithy_query(query="doc",shape="CreateGraphUsingImportTaskInput",
            // member="blankNodeHandling")).
            blank_node_handling: Some("convertToIri"),
            min_provisioned_memory: Some(MAX_PROVISIONED_MEMORY),
            max_provisioned_memory: Some(MAX_PROVISIONED_MEMORY),
            expected_provisioned_memory: MAX_PROVISIONED_MEMORY,
            ..ImportCase::new(
                "RDF n-triples with blank-node handling, at the top of the memory range",
                "ntriples-converted",
                "s3://import-bucket/rdf/data.nt",
            )
        },
        ImportCase {
            // "A URL identifying to the location of the data to be imported. This can be
            // an Amazon S3 path, or can point to a Neptune database endpoint or
            // snapshot" (smithy_query(query="doc",
            // shape="CreateGraphUsingImportTaskInput",member="source")).
            import_options: Some(json!({
                "neptune": {
                    "s3ExportPath": "s3://staging-bucket/neptune-export",
                    "s3ExportKmsKeyId": KMS_KEY_ARN,
                    "preserveDefaultVertexLabels": true,
                    "preserveEdgeIds": false,
                },
            })),
            ..ImportCase::new(
                "a Neptune cluster endpoint with the Neptune import options",
                "from-neptune-cluster",
                "neptune-db://my-cluster.cluster-abcdefghijkl.us-east-1.neptune.amazonaws.com:8182",
            )
        },
        ImportCase {
            import_options: Some(json!({
                "neptune": {
                    "s3ExportPath": "s3://staging-bucket/snapshot-export",
                    "s3ExportKmsKeyId": KMS_KEY_ARN,
                    "preserveDefaultVertexLabels": false,
                    "preserveEdgeIds": true,
                },
            })),
            min_provisioned_memory: Some(512),
            expected_provisioned_memory: 512,
            ..ImportCase::new(
                "a Neptune snapshot ARN with the opposite Neptune import options",
                "from-neptune-snapshot",
                "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:my-neptune-snapshot",
            )
        },
    ]
}

/// The import parameters a request carries are persisted on the task and reported back
/// by `GetImportTask`; the graph parameters are persisted on the graph the same call
/// creates and reported back by `GetGraph`, with the `provisionedMemory` the bounds
/// imply and the tags the request supplied.
#[tokio::test]
async fn create_graph_using_import_task_persists_both_records_and_reads_them_back() {
    let router = emulator();
    let mut minted_tasks = BTreeSet::new();
    let mut minted_graphs = BTreeSet::new();

    for row in import_cases() {
        let case = row.case;
        let request = row.request();
        let (status, created) = create_using_import(&router, request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateGraphUsingImportTask, got {status}: \
             {created}"
        );

        let task_id = created["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {created}"))
            .to_string();
        assert!(
            is_task_id(&task_id),
            "case {case:?}: expected a taskId matching ^t-[a-z0-9]{{10}}$, got {task_id:?}"
        );
        assert!(
            minted_tasks.insert(task_id.clone()),
            "case {case:?}: taskId {task_id:?} was minted twice"
        );

        let graph_id = created["graphId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no graphId in {created}"))
            .to_string();
        assert!(
            is_graph_id(&graph_id),
            "case {case:?}: expected a graphId matching ^g-[a-z0-9]{{10}}$, got {graph_id:?}"
        );
        assert!(
            minted_graphs.insert(graph_id.clone()),
            "case {case:?}: graphId {graph_id:?} was minted twice"
        );

        assert_eq!(
            created["status"],
            json!("INITIALIZING"),
            "case {case:?}: no data has moved when the call returns, expected \
             INITIALIZING, got {}",
            created["status"]
        );
        for (member, expected) in [
            ("source", json!(row.source)),
            ("roleArn", json!(row.role_arn)),
            ("format", row.format.map_or(Value::Null, |v| json!(v))),
            (
                "parquetType",
                row.parquet_type.map_or(Value::Null, |v| json!(v)),
            ),
            (
                "importOptions",
                row.import_options.clone().unwrap_or(Value::Null),
            ),
        ] {
            assert_eq!(
                created[member], expected,
                "case {case:?}: expected {member} {expected}, got {}",
                created[member]
            );
        }

        // The round trip through the task: an independent read reports the record that
        // was stored, not the response that was rendered.
        let (read_status, read) = get_import(&router, &task_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetImportTask, got {read_status}: {read}"
        );
        for member in [
            "taskId",
            "graphId",
            "source",
            "roleArn",
            "format",
            "parquetType",
            "importOptions",
        ] {
            assert_eq!(
                read[member], created[member],
                "case {case:?}: GetImportTask's {member} must be the stored value {}, \
                 got {}",
                created[member], read[member]
            );
        }
        assert_eq!(
            read["attemptNumber"],
            json!(1),
            "case {case:?}: a task nothing retries has been attempted once, got {}",
            read["attemptNumber"]
        );

        // The round trip through the graph the same call created.
        let (graph_status, graph) = get_graph(&router, &graph_id).await;
        assert_eq!(
            graph_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
        );
        for (member, expected) in [
            ("id", json!(graph_id)),
            ("name", json!(row.graph_name)),
            ("provisionedMemory", json!(row.expected_provisioned_memory)),
            ("replicaCount", json!(row.replica_count.unwrap_or(1))),
            (
                "publicConnectivity",
                json!(row.public_connectivity.unwrap_or(false)),
            ),
            (
                "deletionProtection",
                json!(row.deletion_protection.unwrap_or(false)),
            ),
            (
                // The graph is encrypted either way: with the key the request named, or
                // with the service-owned key its sentinel names.
                "kmsKeyIdentifier",
                row.kms_key_identifier
                    .map_or_else(|| json!(SERVICE_OWNED_KMS_KEY), |v| json!(v)),
            ),
            (
                "vectorSearchConfiguration",
                row.vector_search_dimension
                    .map_or(Value::Null, |d| json!({ "dimension": d })),
            ),
            // The graph is populated from `source`, never restored from a snapshot.
            ("sourceSnapshotId", Value::Null),
        ] {
            assert_eq!(
                graph[member], expected,
                "case {case:?}: expected the graph's {member} to be {expected}, got {}",
                graph[member]
            );
        }
        // The graph loads data for as long as the task it was created with runs, and
        // comes into service once that task has stopped — which a slow test machine may
        // already have reached by this read.
        let graph_lifecycle_status = graph["status"].as_str().unwrap_or_default().to_string();
        assert!(
            ["IMPORTING", "AVAILABLE"].contains(&graph_lifecycle_status.as_str()),
            "case {case:?}: a graph created with an import is IMPORTING while the task \
             runs and AVAILABLE after, got {graph_lifecycle_status:?}"
        );

        // The tags supplied at create time are stored ON THE GRAPH, which is what the
        // member documents, so the tagging surface reads them back through the graph's
        // own ARN.
        let graph_arn = engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("graph/{graph_id}"));
        let (tags_status, tags_body) =
            call(&router, "GET", &format!("/tags/{}", encode(&graph_arn)), None).await;
        assert_eq!(
            tags_status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {tags_status}: \
             {tags_body}"
        );
        assert_eq!(
            tags_body["tags"],
            json!(row.tags),
            "case {case:?}: expected the supplied tags {}, got {}",
            json!(row.tags),
            tags_body["tags"]
        );
    }
}

/// A source no object store holds cannot be read, so the clock stops the task in the
/// modelled `FAILED` status with a reason that says what the run met — and the graph
/// the call provisioned comes into service, empty, rather than claiming a load that did
/// not happen.
#[tokio::test]
async fn the_import_a_create_starts_stops_failed_and_the_graph_comes_into_service() {
    let router = emulator();

    // (case, graphName, format, source) — no object store holds any of them.
    let cases = [
        (
            "CSV from an S3 prefix",
            "ends-failed-csv",
            "CSV",
            "s3://import-bucket/csv",
        ),
        (
            "PARQUET from an S3 object",
            "ends-failed-parquet",
            "PARQUET",
            "s3://import-bucket/parquet/part-0.parquet",
        ),
    ];

    for (case, graph_name, format, source) in cases {
        let (status, created) = create_using_import(
            &router,
            json!({
                "graphName": graph_name,
                "roleArn": ROLE_ARN,
                "source": source,
                "format": format,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );
        let task_id = created["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {created}"))
            .to_string();
        let graph_id = created["graphId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no graphId in {created}"))
            .to_string();

        // Read straight after the create call: the transient statuses are observable,
        // so a caller never sees either record skip them.
        let (early_status, early) = get_import(&router, &task_id).await;
        assert_eq!(
            early_status, 200,
            "case {case:?}: expected 200 from GetImportTask, got {early_status}: {early}"
        );
        assert_eq!(
            early["status"],
            json!("INITIALIZING"),
            "case {case:?}: a task read straight after the create call is still \
             INITIALIZING, got {}",
            early["status"]
        );
        assert_eq!(
            early["statusReason"],
            Value::Null,
            "case {case:?}: an accepted task has no reason to report yet, got {}",
            early["statusReason"]
        );
        let (early_graph_status, early_graph) = get_graph(&router, &graph_id).await;
        assert_eq!(
            early_graph_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {early_graph_status}: \
             {early_graph}"
        );
        assert_eq!(
            early_graph["status"],
            json!("IMPORTING"),
            "case {case:?}: a graph read straight after the create call is still \
             IMPORTING, got {}",
            early_graph["status"]
        );

        let stopped =
            wait_for_status(&router, case, &format!("/importtasks/{task_id}"), "FAILED").await;
        let reason = stopped["statusReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: a FAILED task must say why, got {}",
            stopped["statusReason"]
        );
        for member in ["taskId", "graphId", "source", "roleArn", "format"] {
            assert_eq!(
                stopped[member], created[member],
                "case {case:?}: stopping must not change {member}, expected {}, got {}",
                created[member], stopped[member]
            );
        }

        // The graph the call provisioned is a real control-plane record, so it comes
        // into service — and reports no data of its own, because none was imported.
        let settled_graph =
            wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
        assert_eq!(
            settled_graph["statusReason"],
            Value::Null,
            "case {case:?}: a graph in service has nothing to explain, got {}",
            settled_graph["statusReason"]
        );
        assert_eq!(
            settled_graph["id"],
            json!(graph_id),
            "case {case:?}: expected the graph {graph_id:?}, got {}",
            settled_graph["id"]
        );
    }
}

/// A request the model cannot refuse at the boundary is refused here, with the status
/// and error code the rule it broke calls for.
#[tokio::test]
async fn a_create_using_import_outside_the_rules_is_refused() {
    let router = emulator();

    // One stored graph, so the name-uniqueness row has something to conflict with.
    let (seeded, seed_body) = create_using_import(
        &router,
        json!({
            "graphName": "already-taken",
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/seed",
        }),
    )
    .await;
    assert_eq!(
        seeded, 201,
        "case seeding the conflict row: expected 201, got {seeded}: {seed_body}"
    );

    // A `source` one character past the bound the store puts on it, the same bound
    // `StartImportTask` enforces (`crate::state::MAX_IMPORT_SOURCE_CHARS`).
    let prefix = "s3://import-bucket/";
    let past_bound = format!(
        "{prefix}{}",
        "d".repeat(MAX_IMPORT_SOURCE_CHARS - prefix.len() + 1)
    );
    let at_bound = format!(
        "{prefix}{}",
        "d".repeat(MAX_IMPORT_SOURCE_CHARS - prefix.len())
    );

    // (case, the request body, the status, the wire error code — empty when accepted)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            // The `(?!g-)` clause: a name may not take the graph-id prefix.
            "a graphName taking the reserved g- prefix",
            json!({"graphName": "g-mygraph", "roleArn": ROLE_ARN, "source": "s3://b"}),
            400,
            "ValidationException",
        ),
        (
            "a graphName with an uppercase letter",
            json!({"graphName": "MyGraph", "roleArn": ROLE_ARN, "source": "s3://b"}),
            400,
            "ValidationException",
        ),
        (
            "a graphName with two consecutive hyphens",
            json!({"graphName": "my--graph", "roleArn": ROLE_ARN, "source": "s3://b"}),
            400,
            "ValidationException",
        ),
        (
            "a graphName ending in a hyphen",
            json!({"graphName": "my-graph-", "roleArn": ROLE_ARN, "source": "s3://b"}),
            400,
            "ValidationException",
        ),
        (
            "a blank source, which locates nothing",
            json!({"graphName": "blank-source", "roleArn": ROLE_ARN, "source": "   "}),
            400,
            "ValidationException",
        ),
        (
            "a source one character past the store's bound",
            json!({"graphName": "oversized-source", "roleArn": ROLE_ARN, "source": past_bound}),
            400,
            "ValidationException",
        ),
        (
            "a source at the store's bound",
            json!({"graphName": "source-at-bound", "roleArn": ROLE_ARN, "source": at_bound}),
            201,
            "",
        ),
        (
            "an NTRIPLES import with no blankNodeHandling",
            json!({
                "graphName": "ntriples-unhandled",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/data.nt",
                "format": "NTRIPLES",
            }),
            400,
            "ValidationException",
        ),
        (
            // The rule is about the PAIR, not about the format.
            "an NTRIPLES import with blankNodeHandling",
            json!({
                "graphName": "ntriples-handled",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/data.nt",
                "format": "NTRIPLES",
                "blankNodeHandling": "convertToIri",
            }),
            201,
            "",
        ),
        (
            "a minimum above the maximum, which admits no value",
            json!({
                "graphName": "inverted-bounds",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/csv",
                "minProvisionedMemory": 2048,
                "maxProvisionedMemory": 1024,
            }),
            400,
            "ValidationException",
        ),
        (
            // The pass-role decision is taken before the shape check, so a value that
            // names no role the caller may pass is the authorization failure it is.
            "a role ARN that names no IAM role",
            json!({
                "graphName": "bad-role",
                "roleArn": "not-an-arn",
                "source": "s3://import-bucket/csv",
            }),
            403,
            "AccessDeniedException",
        ),
        (
            "a format outside the modelled enum",
            json!({
                "graphName": "bad-format",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/csv",
                "format": "YAML",
            }),
            400,
            "ValidationException",
        ),
        (
            "a replicaCount past the modelled range",
            json!({
                "graphName": "too-many-replicas",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/csv",
                "replicaCount": 3,
            }),
            400,
            "ValidationException",
        ),
        (
            "a graphName another graph already holds",
            json!({
                "graphName": "already-taken",
                "roleArn": ROLE_ARN,
                "source": "s3://import-bucket/again",
            }),
            409,
            "ConflictException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = create_using_import(&router, request.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({request}): expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {body}"
        );
    }
}

/// A refused create persists NEITHER record: no graph takes the name and no task is
/// minted, so a caller can retry the corrected request.
#[tokio::test]
async fn a_refused_create_using_import_persists_nothing() {
    let router = emulator();

    let (refused, body) = create_using_import(
        &router,
        json!({
            "graphName": "retryable",
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/data.nt",
            "format": "NTRIPLES",
        }),
    )
    .await;
    assert_eq!(
        refused, 400,
        "case an NTRIPLES import with no blankNodeHandling: expected 400, got \
         {refused}: {body}"
    );

    // The name is free, so the corrected request is accepted.
    let (accepted, created) = create_using_import(
        &router,
        json!({
            "graphName": "retryable",
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/data.nt",
            "format": "NTRIPLES",
            "blankNodeHandling": "convertToIri",
        }),
    )
    .await;
    assert_eq!(
        accepted, 201,
        "case the corrected request: expected 201, got {accepted}: {created}"
    );

    // And exactly one graph and one task came out of the two calls: the accepted
    // create's records, with nothing left over from the refusal.
    let task_id = created["taskId"]
        .as_str()
        .expect("case the corrected request: a taskId")
        .to_string();
    let (read_status, read) = get_import(&router, &task_id).await;
    assert_eq!(
        read_status, 200,
        "case the accepted task: expected 200 from GetImportTask, got {read_status}: \
         {read}"
    );
    assert_eq!(
        read["graphId"],
        created["graphId"],
        "case the accepted task: expected graphId {}, got {}",
        created["graphId"],
        read["graphId"]
    );
    let graph_id = created["graphId"]
        .as_str()
        .expect("case the corrected request: a graphId")
        .to_string();
    let (graph_status, graph) = get_graph(&router, &graph_id).await;
    assert_eq!(
        graph_status, 200,
        "case the accepted graph: expected 200 from GetGraph, got {graph_status}: {graph}"
    );
    assert_eq!(
        graph["name"],
        json!("retryable"),
        "case the accepted graph: the name the refused call did not take must be held \
         by this graph, got {}",
        graph["name"]
    );
}
