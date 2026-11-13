// SPDX-License-Identifier: CC0-1.0

//! The AppFlow flow RUN and the flow lifecycle around it, driven over the wire.
//!
//! The rules under test:
//!
//! * a run of a flow whose source and destination are Amazon S3 locations TRANSFERS
//!   data — it reads the records at the source location out of the one object store in
//!   this emulator, maps each one by the flow's own `tasks`, writes the result at the
//!   destination location and reports the bytes and records it moved — so what a later
//!   `GetObject` reads out of the destination bucket is what the run put there, and
//!   `DescribeFlowExecutionRecords` reports `Successful` with the volume that moved;
//! * a run AppFlow is not allowed to perform moves nothing: a bucket whose policy does
//!   not grant the AppFlow service principal the actions the run takes, and a location
//!   whose bucket does not exist, both end the run in the service's modelled failure
//!   state with the reason, and nothing is written at the destination;
//! * `StartFlow` and `StopFlow` arm and disarm the trigger of a flow that runs ITSELF
//!   (`Draft` -> `Active` -> `Suspended` -> `Active`), each refusing as a state conflict
//!   when there is nothing left to arm or disarm, and an unforced `DeleteFlow` refuses a
//!   flow that is in use;
//! * a request refused before any handler ran is stated in AWS's constraint envelope,
//!   with a `@pattern` requirement quoted as the expression the service validates
//!   against.
//!
//! Both services are mounted on ONE shared engine, exactly as the router mounts them, so
//! what is exercised is the hand-off between the two: the AppFlow calls as the
//! `restJson1` requests a client sends, the source writes and destination reads as the
//! path-style Amazon S3 requests a client sends.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::flow_lifecycle::{
    appflow_bucket_policy, filter_task, map_task, on_demand_trigger, scheduled_trigger,
    DESTINATION_POLICY_ACTIONS, EVERY_RUN_ACTION, SOURCE_POLICY_ACTIONS,
};

/// The endpoint every request here reaches the emulator on, and the Region every request
/// is signed for.
const HOST: &str = "127.0.0.1:9297";
const REGION: &str = "us-east-1";

/// Longer than the span a run takes to settle (`EXECUTION_SETTLES_AFTER`), so a call made
/// after this wait must observe the terminal status the clock owed the run.
///
/// Waited with `std::thread::sleep`, not `tokio::time::sleep`: this workspace enables
/// tokio's `macros`/`rt` features only, so the timer driver is not compiled in.
const AFTER_RUN_SETTLES: Duration = Duration::from_millis(900);

/// What a dispatched request answered: the HTTP status, the wire error code (empty on
/// success) and the body as it arrived.
struct Answer {
    status: u16,
    error: String,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON, or `Value::Null` when it is not JSON at all.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn member(&self, name: &str) -> Value {
        self.json().get(name).cloned().unwrap_or(Value::Null)
    }

    fn string(&self, name: &str) -> String {
        self.member(name).as_str().unwrap_or_default().to_string()
    }
}

/// AppFlow and Amazon S3 on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One request to the service `service` signs as, with `body` as its own bytes.
    async fn signed(&self, service: &str, method: &str, path: &str, body: &[u8]) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let error = parts
            .headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.rsplit('/').next().unwrap_or(value).to_string())
            .or_else(|| {
                serde_json::from_slice::<Value>(&body)
                    .ok()?
                    .get("__type")?
                    .as_str()
                    .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
            })
            .unwrap_or_default();
        Answer {
            status: parts.status.as_u16(),
            error,
            body,
        }
    }

    /// One AppFlow operation, by the URI its `@http` trait binds it to.
    async fn appflow(&self, path: &str, body: &Value) -> Answer {
        self.signed(
            "appflow",
            "POST",
            path,
            &serde_json::to_vec(body).expect("the request body serializes"),
        )
        .await
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`.
    ///
    /// The create names NO `LocationConstraint`: the unspecified constraint stands for
    /// us-east-1, the Region this harness signs for, and S3 refuses a create that names
    /// it explicitly.
    async fn create_bucket(&self, bucket: &str) {
        let answer = self
            .signed("s3", "PUT", &format!("/{bucket}"), b"")
            .await;
        assert_eq!(
            answer.status,
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// `PUT /<bucket>?policy` — Amazon S3's `PutBucketPolicy`, which answers 204 with no
    /// body.
    async fn put_bucket_policy(&self, bucket: &str, policy: &Value) {
        let answer = self
            .signed(
                "s3",
                "PUT",
                &format!("/{bucket}?policy"),
                policy.to_string().as_bytes(),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "put policy on {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// `PUT /<bucket>/<key>` — Amazon S3's `PutObject`.
    async fn put_object(&self, bucket: &str, key: &str, body: &str) {
        let answer = self
            .signed("s3", "PUT", &format!("/{bucket}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            answer.status,
            200,
            "put {bucket}/{key}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// `GET /<bucket>/<key>` — Amazon S3's `GetObject`.
    async fn object(&self, bucket: &str, key: &str) -> Answer {
        self.signed("s3", "GET", &format!("/{bucket}/{key}"), b"").await
    }

    /// `DELETE /<bucket>/<key>` — Amazon S3's `DeleteObject`.
    async fn delete_object(&self, bucket: &str, key: &str) {
        let answer = self
            .signed("s3", "DELETE", &format!("/{bucket}/{key}"), b"")
            .await;
        assert_eq!(
            answer.status,
            204,
            "delete {bucket}/{key}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// `DELETE /<bucket>` — Amazon S3's `DeleteBucket`.
    async fn delete_bucket(&self, bucket: &str) {
        let answer = self.signed("s3", "DELETE", &format!("/{bucket}"), b"").await;
        assert_eq!(
            answer.status,
            204,
            "delete bucket {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// `DELETE /<bucket>?policy` — Amazon S3's `DeleteBucketPolicy`.
    async fn delete_bucket_policy(&self, bucket: &str) {
        let answer = self
            .signed("s3", "DELETE", &format!("/{bucket}?policy"), b"")
            .await;
        assert_eq!(
            answer.status,
            204,
            "delete policy on {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// Stand up `buckets` with the bucket policy AppFlow asks a caller to place on a
    /// location it reads or writes, so a flow naming them can be created at all.
    async fn stand_up(&self, buckets: &[&str]) {
        for bucket in buckets {
            self.create_bucket(bucket).await;
            self.put_bucket_policy(bucket, &appflow_bucket_policy(bucket, &EVERY_RUN_ACTION))
                .await;
        }
    }
}

/// An Amazon S3 source naming a bucket and the prefix the flow's files sit under.
fn s3_source_at(bucket: &str, prefix: &str, input_file_type: Option<&str>) -> Value {
    let mut properties = json!({ "bucketName": bucket, "bucketPrefix": prefix });
    if let Some(file_type) = input_file_type {
        properties["s3InputFormatConfig"] = json!({ "s3InputFileType": file_type });
    }
    json!({
        "connectorType": "S3",
        "sourceConnectorProperties": { "S3": properties }
    })
}

/// An Amazon S3 destination naming a bucket and the prefix the run writes under.
fn s3_destination_at(bucket: &str, prefix: &str, output_file_type: Option<&str>) -> Value {
    let mut properties = json!({ "bucketName": bucket, "bucketPrefix": prefix });
    if let Some(file_type) = output_file_type {
        properties["s3OutputFormatConfig"] = json!({ "fileType": file_type });
    }
    json!({
        "connectorType": "S3",
        "destinationConnectorProperties": { "S3": properties }
    })
}

/// A `Map_all` task, optionally naming the fields it does NOT map.
fn map_all_task(excluded: &[&str]) -> Value {
    let mut task = json!({ "taskType": "Map_all", "sourceFields": [] });
    if !excluded.is_empty() {
        task["taskProperties"] = json!({ "EXCLUDE_SOURCE_FIELDS_LIST": excluded.join(",") });
    }
    task
}

/// What the destination file is expected to hold.
#[derive(Debug)]
enum Expected {
    /// One JSON record per line, in this order.
    JsonLines(Vec<Value>),
    /// This exact CSV text.
    Csv(&'static str),
    /// An Apache Parquet file, which is identified by its own magic rather than by text.
    Parquet,
}

/// One row of the transfer table.
struct Transfer {
    case: &'static str,
    /// The objects the source bucket holds, as (key, bytes).
    source: &'static [(&'static str, &'static str)],
    input_file_type: Option<&'static str>,
    output_file_type: Option<&'static str>,
    tasks: Value,
    /// The records the run is expected to report having processed.
    records: u64,
    expected: Expected,
}

/// The transfer table. The rows differ along everything the rule generalises over: the
/// shape and the file type of the source files, how many records and files there are, the
/// output file type, and which of the three mapping rules the flow's tasks state.
fn transfers() -> Vec<Transfer> {
    vec![
        Transfer {
            case: "a json array of two records, every field mapped by a task of its own",
            source: &[("inbound/orders.json", r#"[{"id":"1","name":"ada"},{"id":"2","name":"bob"}]"#)],
            input_file_type: Some("JSON"),
            output_file_type: Some("JSON"),
            tasks: json!([map_task("id", "id"), map_task("name", "name")]),
            records: 2,
            expected: Expected::JsonLines(vec![
                json!({ "id": "1", "name": "ada" }),
                json!({ "id": "2", "name": "bob" }),
            ]),
        },
        Transfer {
            case: "one record per line, three of them, every field mapped",
            source: &[(
                "inbound/events.json",
                "{\"id\":\"1\"}\n{\"id\":\"2\"}\n{\"id\":\"3\"}\n",
            )],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([map_all_task(&[])]),
            records: 3,
            expected: Expected::JsonLines(vec![
                json!({ "id": "1" }),
                json!({ "id": "2" }),
                json!({ "id": "3" }),
            ]),
        },
        Transfer {
            case: "a single json record whose values are not all strings",
            source: &[("inbound/one.json", r#"{"id":"7","amount":42,"paid":true}"#)],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([map_all_task(&[])]),
            records: 1,
            expected: Expected::JsonLines(vec![json!({ "amount": 42, "id": "7", "paid": true })]),
        },
        Transfer {
            case: "two source files, one record each, read in key order",
            source: &[
                ("inbound/a.json", r#"{"id":"a"}"#),
                ("inbound/b.json", r#"{"id":"b"}"#),
            ],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([map_all_task(&[])]),
            records: 2,
            expected: Expected::JsonLines(vec![json!({ "id": "a" }), json!({ "id": "b" })]),
        },
        Transfer {
            case: "a csv source to a csv destination, with one field renamed",
            source: &[("inbound/people.csv", "id,name\n1,ada\n2,bob\n")],
            input_file_type: Some("CSV"),
            output_file_type: Some("CSV"),
            tasks: json!([map_task("name", "full_name")]),
            records: 2,
            expected: Expected::Csv("full_name,id\nada,1\nbob,2\n"),
        },
        Transfer {
            case: "a filter task projects the one field the run reads",
            source: &[("inbound/wide.json", r#"{"id":"1","name":"ada","note":"kept out"}"#)],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([filter_task(&["id"])]),
            records: 1,
            expected: Expected::JsonLines(vec![json!({ "id": "1" })]),
        },
        Transfer {
            case: "a map_all task excludes one field",
            source: &[("inbound/wide.json", r#"{"id":"1","secret":"hidden"}"#)],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([map_all_task(&["secret"])]),
            records: 1,
            expected: Expected::JsonLines(vec![json!({ "id": "1" })]),
        },
        Transfer {
            case: "a json source to a parquet destination",
            source: &[("inbound/orders.json", r#"[{"id":"1"},{"id":"2"}]"#)],
            input_file_type: None,
            output_file_type: Some("PARQUET"),
            tasks: json!([map_all_task(&[])]),
            records: 2,
            expected: Expected::Parquet,
        },
        Transfer {
            case: "a source prefix holding no file at all",
            source: &[],
            input_file_type: None,
            output_file_type: None,
            tasks: json!([map_all_task(&[])]),
            records: 0,
            expected: Expected::JsonLines(Vec::new()),
        },
    ]
}

/// A run of an Amazon S3 to Amazon S3 flow transfers the records at the source location
/// to the destination location, and reports the volume it moved.
#[tokio::test]
async fn an_s3_to_s3_run_transfers_the_source_records_to_the_destination() {
    for row in transfers() {
        let case = row.case;
        let emulator = Emulator::new();
        let source_bucket = "flow-source";
        let destination_bucket = "flow-destination";
        emulator.create_bucket(source_bucket).await;
        emulator.create_bucket(destination_bucket).await;
        emulator
            .put_bucket_policy(
                source_bucket,
                &appflow_bucket_policy(source_bucket, &SOURCE_POLICY_ACTIONS),
            )
            .await;
        emulator
            .put_bucket_policy(
                destination_bucket,
                &appflow_bucket_policy(destination_bucket, &DESTINATION_POLICY_ACTIONS),
            )
            .await;
        let mut source_bytes = 0u64;
        for (key, body) in row.source {
            emulator.put_object(source_bucket, key, body).await;
            source_bytes += body.len() as u64;
        }

        let flow_name = "transfer-flow";
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at(source_bucket, "inbound", row.input_file_type),
                    "destinationFlowConfigList": [s3_destination_at(
                        destination_bucket, "outbound", row.output_file_type)],
                    "tasks": row.tasks,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.json()
        );

        let started = emulator
            .appflow("/start-flow", &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected StartFlow to succeed, got {} {:?}",
            started.status,
            started.json()
        );
        let execution_id = started.string("executionId");
        assert!(
            !execution_id.is_empty(),
            "case {case:?}: expected StartFlow to report a run id, got {:?}",
            started.json()
        );

        std::thread::sleep(AFTER_RUN_SETTLES);
        let records = emulator
            .appflow(
                "/describe-flow-execution-records",
                &json!({ "flowName": flow_name }),
            )
            .await;
        let reported = records.member("flowExecutions");
        let run = reported
            .as_array()
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            run.get("executionStatus").and_then(Value::as_str),
            Some("Successful"),
            "case {case:?}: expected the run to succeed, got {run:?}"
        );
        let result = run.get("executionResult").cloned().unwrap_or(Value::Null);
        assert_eq!(
            result.get("recordsProcessed").and_then(Value::as_u64),
            Some(row.records),
            "case {case:?}: expected {} records processed, got {result:?}",
            row.records
        );
        assert_eq!(
            result.get("bytesProcessed").and_then(Value::as_u64),
            Some(source_bytes),
            "case {case:?}: expected {source_bytes} bytes processed, got {result:?}"
        );
        assert!(
            result.get("errorInfo").is_none_or(Value::is_null),
            "case {case:?}: expected a successful run to report no error, got {result:?}"
        );

        // What the run WROTE, read back through Amazon S3's own data plane at the key the
        // destination prefix, the flow and the run name.
        let extension = row.output_file_type.unwrap_or("JSON").to_ascii_lowercase();
        let key = format!("outbound/{flow_name}/{execution_id}/{execution_id}.{extension}");
        let written = emulator.object(destination_bucket, &key).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected the run to have written {key}, got {} {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        assert_eq!(
            result.get("bytesWritten").and_then(Value::as_u64),
            Some(written.body.len() as u64),
            "case {case:?}: expected the reported bytes to be the bytes written, got {result:?}"
        );
        match row.expected {
            Expected::JsonLines(expected) => {
                let lines: Vec<Value> = String::from_utf8_lossy(&written.body)
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
                    .collect();
                assert_eq!(
                    lines, expected,
                    "case {case:?}: expected the destination file to hold the mapped records, \
                     got {lines:?}"
                );
            }
            Expected::Csv(expected) => {
                let text = String::from_utf8_lossy(&written.body).to_string();
                assert_eq!(
                    text, expected,
                    "case {case:?}: expected the destination file to hold the mapped rows, got \
                     {text:?}"
                );
            }
            Expected::Parquet => {
                assert!(
                    written.body.starts_with(b"PAR1") && written.body.ends_with(b"PAR1"),
                    "case {case:?}: expected an Apache Parquet file, got {} bytes starting {:?}",
                    written.body.len(),
                    String::from_utf8_lossy(&written.body[..written.body.len().min(8)])
                );
            }
        }
    }
}

/// What stops being usable AFTER the flow was defined, so the run has nowhere to read or
/// write when it is started.
#[derive(Debug, Clone, Copy)]
enum Breakage {
    SourceBucketRemoved,
    DestinationBucketRemoved,
    SourcePolicyRemoved,
    DestinationPolicyRemoved,
    /// The policy is replaced by one granting the actions of the OTHER end, so the grant
    /// exists but not for what this end of the transfer does.
    SourcePolicyGrantsTheOtherEndsActions,
    DestinationPolicyGrantsTheOtherEndsActions,
}

/// A run writes EVERY destination the definition names, in the file type each one
/// declares, and reports the bytes of all of them together. The rows differ along the
/// number of destinations and the file types they ask for.
#[tokio::test]
async fn a_run_writes_every_destination_the_definition_names() {
    for (case, file_types) in [
        ("one destination", vec!["JSON"]),
        ("two destinations in the same file type", vec!["JSON", "JSON"]),
        ("two destinations in different file types", vec!["JSON", "CSV"]),
        (
            "three destinations, one of each file type",
            vec!["JSON", "CSV", "PARQUET"],
        ),
    ] {
        let emulator = Emulator::new();
        let source_bucket = "fanout-source";
        let buckets: Vec<String> = (0..file_types.len())
            .map(|index| format!("fanout-destination-{index}"))
            .collect();
        let mut stood_up: Vec<&str> = vec![source_bucket];
        stood_up.extend(buckets.iter().map(String::as_str));
        emulator.stand_up(&stood_up).await;
        emulator
            .put_object(source_bucket, "inbound/orders.json", r#"{"id":"1"}"#)
            .await;

        let destinations: Vec<Value> = buckets
            .iter()
            .zip(&file_types)
            .map(|(bucket, file_type)| s3_destination_at(bucket, "outbound", Some(file_type)))
            .collect();
        let flow_name = "fanout-flow";
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at(source_bucket, "inbound", None),
                    "destinationFlowConfigList": destinations,
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.json()
        );
        let started = emulator
            .appflow("/start-flow", &json!({ "flowName": flow_name }))
            .await;
        let execution_id = started.string("executionId");
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected StartFlow to succeed, got {} {:?}",
            started.status,
            started.json()
        );

        std::thread::sleep(AFTER_RUN_SETTLES);
        let records = emulator
            .appflow(
                "/describe-flow-execution-records",
                &json!({ "flowName": flow_name }),
            )
            .await;
        let run = records
            .member("flowExecutions")
            .as_array()
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            run.get("executionStatus").and_then(Value::as_str),
            Some("Successful"),
            "case {case:?}: expected the run to succeed, got {run:?}"
        );

        // Every destination holds the file the run wrote for it, and the reported bytes
        // are those files together.
        let mut written_bytes = 0u64;
        for (bucket, file_type) in buckets.iter().zip(&file_types) {
            let key = format!(
                "outbound/{flow_name}/{execution_id}/{execution_id}.{}",
                file_type.to_ascii_lowercase()
            );
            let written = emulator.object(bucket, &key).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected {bucket} to hold {key}, got {} {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert!(
                !written.body.is_empty(),
                "case {case:?}: expected {bucket} to hold the record the run moved, got an \
                 empty file"
            );
            written_bytes += written.body.len() as u64;
        }
        let result = run.get("executionResult").cloned().unwrap_or(Value::Null);
        assert_eq!(
            result.get("bytesWritten").and_then(Value::as_u64),
            Some(written_bytes),
            "case {case:?}: expected the reported bytes to be every destination file \
             together, got {result:?}"
        );
    }
}

/// A run of a flow with an endpoint this emulator does not connect to performs no
/// transfer, and says so: the modelled failure state with the reason, and true zeros
/// rather than a volume nothing moved. The rows differ along which end is that endpoint.
#[tokio::test]
async fn a_run_with_an_endpoint_this_emulator_does_not_reach_performs_nothing() {
    let not_performed = "This emulator does not connect to source or destination connector \
                         endpoints so the flow run was not performed.";
    for (case, source, destinations) in [
        (
            "the destination is an EventBridge event bus",
            s3_source_at("unreached-in", "inbound", None),
            json!([{
                "connectorType": "EventBridge",
                "destinationConnectorProperties": { "EventBridge": { "object": "orders" } }
            }]),
        ),
        (
            "one of two destinations is an EventBridge event bus",
            s3_source_at("unreached-in", "inbound", None),
            json!([
                s3_destination_at("unreached-out", "outbound", None),
                {
                    "connectorType": "EventBridge",
                    "destinationConnectorProperties": { "EventBridge": { "object": "orders" } }
                }
            ]),
        ),
    ] {
        let emulator = Emulator::new();
        emulator.stand_up(&["unreached-in", "unreached-out"]).await;
        emulator
            .put_object("unreached-in", "inbound/orders.json", r#"{"id":"1"}"#)
            .await;
        let flow_name = "unreached-flow";
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": source,
                    "destinationFlowConfigList": destinations,
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.json()
        );
        let started = emulator
            .appflow("/start-flow", &json!({ "flowName": flow_name }))
            .await;
        let execution_id = started.string("executionId");
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected StartFlow to record the run, got {} {:?}",
            started.status,
            started.json()
        );

        std::thread::sleep(AFTER_RUN_SETTLES);
        let records = emulator
            .appflow(
                "/describe-flow-execution-records",
                &json!({ "flowName": flow_name }),
            )
            .await;
        let run = records
            .member("flowExecutions")
            .as_array()
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or(Value::Null);
        let result = run.get("executionResult").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                run.get("executionStatus").and_then(Value::as_str),
                result
                    .get("errorInfo")
                    .and_then(|info| info.get("executionMessage"))
                    .and_then(Value::as_str)
            ),
            (Some("Error"), Some(not_performed)),
            "case {case:?}: expected a run nothing performed to say so, got {run:?}"
        );
        for counter in ["bytesProcessed", "bytesWritten", "recordsProcessed"] {
            assert_eq!(
                result.get(counter).and_then(Value::as_u64),
                Some(0),
                "case {case:?}: expected {counter} to be zero, got {result:?}"
            );
        }
        // And nothing was written at the Amazon S3 destination of the second row either:
        // a run that cannot serve every destination serves none.
        let key = format!("outbound/{flow_name}/{execution_id}/{execution_id}.json");
        let written = emulator.object("unreached-out", &key).await;
        assert_eq!(
            written.status, 404,
            "case {case:?}: expected nothing written, got {} {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
    }
}

/// One row of the refused-run table.
struct RefusedRun {
    case: &'static str,
    breakage: Breakage,
    /// The sentence the run's `errorInfo` is expected to carry.
    reason: &'static str,
}

/// A run AppFlow cannot perform ends in the modelled failure state with the reason, and
/// writes nothing at the destination. Each row breaks a location AFTER the flow was
/// defined — so what is exercised is the run reading the state at the moment it runs — and
/// the rows differ along which end of the transfer became unusable and how: its bucket is
/// gone, its policy is gone, or its policy grants the actions of the other end rather than
/// the ones this end needs.
#[tokio::test]
async fn a_run_the_locations_do_not_allow_moves_nothing() {
    let rows = [
        RefusedRun {
            case: "the source bucket is gone",
            breakage: Breakage::SourceBucketRemoved,
            reason: "The Amazon S3 bucket the source location names does not exist.",
        },
        RefusedRun {
            case: "the destination bucket is gone",
            breakage: Breakage::DestinationBucketRemoved,
            reason: "The Amazon S3 bucket the destination location names does not exist.",
        },
        RefusedRun {
            case: "the source bucket carries no policy any more",
            breakage: Breakage::SourcePolicyRemoved,
            reason: "Access denied reading the source location. Grant the AppFlow service \
                     principal appflow.amazonaws.com access to that bucket with a bucket policy.",
        },
        RefusedRun {
            case: "the destination bucket carries no policy any more",
            breakage: Breakage::DestinationPolicyRemoved,
            reason: "Access denied writing the destination location. Grant the AppFlow service \
                     principal appflow.amazonaws.com access to that bucket with a bucket policy.",
        },
        RefusedRun {
            case: "the source policy grants the destination's actions instead of the source's",
            breakage: Breakage::SourcePolicyGrantsTheOtherEndsActions,
            reason: "Access denied reading the source location. Grant the AppFlow service \
                     principal appflow.amazonaws.com access to that bucket with a bucket policy.",
        },
        RefusedRun {
            case: "the destination policy grants the source's actions instead of its own",
            breakage: Breakage::DestinationPolicyGrantsTheOtherEndsActions,
            reason: "Access denied writing the destination location. Grant the AppFlow service \
                     principal appflow.amazonaws.com access to that bucket with a bucket policy.",
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let source_bucket = "refused-source";
        let destination_bucket = "refused-destination";
        let source_key = "inbound/orders.json";
        emulator.stand_up(&[source_bucket, destination_bucket]).await;
        emulator
            .put_object(source_bucket, source_key, r#"{"id":"1"}"#)
            .await;

        let flow_name = "refused-flow";
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at(source_bucket, "inbound", None),
                    "destinationFlowConfigList": [s3_destination_at(destination_bucket, "outbound", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed while both locations are usable, \
             got {} {:?}",
            created.status,
            created.json()
        );

        // Now take the location away, which is what the run has to notice.
        let destination_still_there = !matches!(row.breakage, Breakage::DestinationBucketRemoved);
        match row.breakage {
            Breakage::SourceBucketRemoved => {
                emulator.delete_object(source_bucket, source_key).await;
                emulator.delete_bucket(source_bucket).await;
            }
            Breakage::DestinationBucketRemoved => {
                emulator.delete_bucket(destination_bucket).await;
            }
            Breakage::SourcePolicyRemoved => {
                emulator.delete_bucket_policy(source_bucket).await;
            }
            Breakage::DestinationPolicyRemoved => {
                emulator.delete_bucket_policy(destination_bucket).await;
            }
            Breakage::SourcePolicyGrantsTheOtherEndsActions => {
                emulator
                    .put_bucket_policy(
                        source_bucket,
                        &appflow_bucket_policy(source_bucket, &DESTINATION_POLICY_ACTIONS),
                    )
                    .await;
            }
            Breakage::DestinationPolicyGrantsTheOtherEndsActions => {
                emulator
                    .put_bucket_policy(
                        destination_bucket,
                        &appflow_bucket_policy(destination_bucket, &SOURCE_POLICY_ACTIONS),
                    )
                    .await;
            }
        }

        let started = emulator
            .appflow("/start-flow", &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected StartFlow to record the run, got {} {:?}",
            started.status,
            started.json()
        );
        let execution_id = started.string("executionId");

        std::thread::sleep(AFTER_RUN_SETTLES);
        let records = emulator
            .appflow(
                "/describe-flow-execution-records",
                &json!({ "flowName": flow_name }),
            )
            .await;
        let run = records
            .member("flowExecutions")
            .as_array()
            .and_then(|runs| runs.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            run.get("executionStatus").and_then(Value::as_str),
            Some("Error"),
            "case {case:?}: expected the run to fail, got {run:?}"
        );
        let result = run.get("executionResult").cloned().unwrap_or(Value::Null);
        assert_eq!(
            result
                .get("errorInfo")
                .and_then(|info| info.get("executionMessage"))
                .and_then(Value::as_str),
            Some(row.reason),
            "case {case:?}: expected the run to report why it could not be performed, got \
             {result:?}"
        );
        for counter in ["bytesProcessed", "bytesWritten", "recordsProcessed"] {
            assert_eq!(
                result.get(counter).and_then(Value::as_u64),
                Some(0),
                "case {case:?}: expected {counter} to be zero for a run that moved nothing, got \
                 {result:?}"
            );
        }
        if destination_still_there {
            let key = format!("outbound/{flow_name}/{execution_id}/{execution_id}.json");
            let written = emulator.object(destination_bucket, &key).await;
            assert_eq!(
                written.status, 404,
                "case {case:?}: expected nothing at the destination, got {} {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }
    }
}

/// `StartFlow` and `StopFlow` arm and disarm the trigger of a flow that runs itself, and
/// each refuses when there is nothing left to arm or disarm. An unforced `DeleteFlow`
/// refuses a flow that is in use and succeeds once its trigger is disarmed.
#[tokio::test]
async fn a_self_triggered_flow_is_activated_deactivated_and_resumed() {
    let emulator = Emulator::new();
    let flow_name = "scheduled-lifecycle-flow";
    emulator.stand_up(&["lifecycle-in", "lifecycle-out"]).await;
    let created = emulator
        .appflow(
            "/create-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": scheduled_trigger("rate(6hours)"),
                "sourceFlowConfig": s3_source_at("lifecycle-in", "inbound", None),
                "destinationFlowConfigList": [s3_destination_at("lifecycle-out", "outbound", None)],
                "tasks": [map_all_task(&[])],
            }),
        )
        .await;
    assert_eq!(
        created.string("flowStatus"),
        "Draft",
        "expected a scheduled flow to be created with its schedule unarmed, got {:?}",
        created.json()
    );

    // Each step names the status the flow is left in, and the status DescribeFlow then
    // reports for it: the two are the same value read twice, through two calls.
    for (case, path, expected) in [
        ("activating the schedule", "/start-flow", "Active"),
        ("deactivating the schedule", "/stop-flow", "Suspended"),
        ("activating it again", "/start-flow", "Active"),
        ("deactivating it again", "/stop-flow", "Suspended"),
    ] {
        let answer = emulator
            .appflow(path, &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            (answer.status, answer.string("flowStatus").as_str()),
            (200, expected),
            "case {case:?}: expected {expected}, got {} {:?}",
            answer.status,
            answer.json()
        );
        let described = emulator
            .appflow("/describe-flow", &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            described.string("flowStatus"),
            expected,
            "case {case:?}: expected DescribeFlow to report {expected}, got {:?}",
            described.json()
        );
    }

    // Deactivating a flow whose trigger is already disarmed is a state conflict, and
    // leaves the status where it was.
    let refused = emulator
        .appflow("/stop-flow", &json!({ "flowName": flow_name }))
        .await;
    assert_eq!(
        (refused.status, refused.error.as_str(), refused.string("message").as_str()),
        (
            409,
            "ConflictException",
            format!("Conflict executing request: Flow is already deactivated: {flow_name}").as_str()
        ),
        "expected deactivating a suspended flow to conflict, got {} {:?}",
        refused.status,
        refused.json()
    );

    // A suspended flow is not in use, so the unforced delete AppFlow refuses for an
    // active one succeeds here.
    let deleted = emulator
        .appflow("/delete-flow", &json!({ "flowName": flow_name }))
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected deleting a suspended flow to succeed, got {} {:?}",
        deleted.status,
        deleted.json()
    );
}

/// One row of the state-conflict table.
struct StateConflict {
    case: &'static str,
    /// The trigger the flow is created with.
    trigger: Value,
    /// Whether the flow's trigger is armed before the call under test.
    activate: bool,
    path: &'static str,
    request: Value,
    message: &'static str,
}

/// A call that cannot be served in the flow's current state is refused as a conflict, with
/// the sentence AppFlow words for that conflict, and the flow is left exactly as it was.
/// The rows differ along which call is refused and which state refuses it.
#[tokio::test]
async fn a_call_the_flow_state_cannot_serve_is_refused_as_a_conflict() {
    let flow_name = "conflict-flow";
    let rows = [
        StateConflict {
            case: "deactivating a flow that runs only on demand",
            trigger: on_demand_trigger(),
            activate: false,
            path: "/stop-flow",
            request: json!({ "flowName": flow_name }),
            message: "Conflict executing request: Flow is already deactivated: conflict-flow",
        },
        StateConflict {
            case: "activating a schedule that is already armed",
            trigger: scheduled_trigger("rate(1hours)"),
            activate: true,
            path: "/start-flow",
            request: json!({ "flowName": flow_name }),
            message: "Conflict executing request: Flow is already activated: conflict-flow",
        },
        StateConflict {
            case: "deleting an active flow without asking for the delete to be forced",
            trigger: scheduled_trigger("rate(1hours)"),
            activate: true,
            path: "/delete-flow",
            request: json!({ "flowName": flow_name }),
            message: "Conflict executing request: Flow is in active state, please set forceDelete \
                      to true or deactivate the flow: conflict-flow",
        },
        StateConflict {
            case: "deleting an on-demand flow, which is active from the moment it exists",
            trigger: on_demand_trigger(),
            activate: false,
            path: "/delete-flow",
            request: json!({ "flowName": flow_name, "forceDelete": false }),
            message: "Conflict executing request: Flow is in active state, please set forceDelete \
                      to true or deactivate the flow: conflict-flow",
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.stand_up(&["conflict-in", "conflict-out"]).await;
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": row.trigger,
                    "sourceFlowConfig": s3_source_at("conflict-in", "inbound", None),
                    "destinationFlowConfigList": [s3_destination_at("conflict-out", "outbound", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.json()
        );
        let before = if row.activate {
            emulator
                .appflow("/start-flow", &json!({ "flowName": flow_name }))
                .await
                .string("flowStatus")
        } else {
            created.string("flowStatus")
        };

        let refused = emulator.appflow(row.path, &row.request).await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected a conflict, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            row.message,
            "case {case:?}: expected AppFlow's conflict sentence, got {:?}",
            refused.json()
        );
        let described = emulator
            .appflow("/describe-flow", &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            described.string("flowStatus"),
            before,
            "case {case:?}: expected the refused call to leave the flow at {before}, got {:?}",
            described.json()
        );
    }
}

/// A flow definition naming a connector profile the account does not hold is refused as a
/// resource that is not found, and no flow is left behind. The rows differ along where in
/// the definition the profile is named.
#[tokio::test]
async fn a_definition_naming_an_unknown_connector_profile_is_not_found() {
    for (case, source, destinations, missing) in [
        (
            "the source names a profile nothing created",
            json!({
                "connectorType": "Salesforce",
                "connectorProfileName": "no-such-salesforce-profile",
                "sourceConnectorProperties": { "Salesforce": { "object": "Account" } }
            }),
            json!([s3_destination_at("profile-out", "outbound", None)]),
            "no-such-salesforce-profile",
        ),
        (
            "the destination names a profile nothing created",
            s3_source_at("profile-in", "inbound", None),
            json!([{
                "connectorType": "Salesforce",
                "connectorProfileName": "no-such-destination-profile",
                "destinationConnectorProperties": {
                    "Salesforce": { "object": "Account" }
                }
            }]),
            "no-such-destination-profile",
        ),
    ] {
        let emulator = Emulator::new();
        emulator.stand_up(&["profile-in", "profile-out"]).await;
        let flow_name = "profile-flow";
        let refused = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": source,
                    "destinationFlowConfigList": destinations,
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the create to be refused, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            format!("Connector Profile with name {missing} does not exist."),
            "case {case:?}: expected the refusal to name the profile, got {:?}",
            refused.json()
        );

        // Nothing was left behind: the flow is absent from both the read of one flow and
        // the listing.
        let described = emulator
            .appflow("/describe-flow", &json!({ "flowName": flow_name }))
            .await;
        assert_eq!(
            (described.status, described.string("message").as_str()),
            (404, format!("There is no flow with the name {flow_name}").as_str()),
            "case {case:?}: expected no flow to have been created, got {} {:?}",
            described.status,
            described.json()
        );
        let listed = emulator.appflow("/list-flows", &json!({})).await;
        assert_eq!(
            listed.member("flows").as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected the account to hold no flow, got {:?}",
            listed.member("flows")
        );
    }
}

/// One row of the unusable-location table: what the definition names, and why AppFlow
/// could not use it.
struct UnusableLocation {
    case: &'static str,
    /// The buckets that exist, with the actions each one's policy grants (`None` for a
    /// bucket that carries no policy at all).
    buckets: &'static [(&'static str, Option<&'static [&'static str]>)],
    source_bucket: &'static str,
    destination_bucket: &'static str,
    message: String,
}

/// How AppFlow names the flow-definition request a refusal refused, in the envelope the
/// refusal's message opens with.
const CREATE_FLOW_REQUEST: &str = "Create Flow";
const UPDATE_FLOW_REQUEST: &str = "Update Flow";

/// AppFlow's message for a flow-definition request it refused because the policy on a
/// bucket the definition names does not grant the AppFlow service principal every action
/// a run takes on that end of the transfer.
///
/// The fixed text is the service's own and is written out once, here; everything else is
/// taken from the request and from the bucket's policy state — the bucket the definition
/// named, the actions that bucket's policy still leaves ungranted (lowercased, in the
/// order a run takes them), the service principal the grants have to name, and the two
/// resource ARNs derived from the bucket name: the bucket itself and every object in it.
fn missing_actions_refusal(request: &str, bucket: &str, missing: &[&str]) -> String {
    format!(
        "{request} request failed: [The provided S3 Bucket [{bucket}] is missing the \
         following permitted actions: [{missing}]\n. These policies must be applied to the \
         [appflow.amazonaws.com] service principal and must be attached to all the \
         following resources: [arn:aws:s3:::{bucket}, arn:aws:s3:::{bucket}/*]]",
        missing = missing.join(", "),
    )
}

/// A definition naming an Amazon S3 location AppFlow could not use is refused when the
/// flow is DEFINED, and no flow is left behind. The rows differ along which end names the
/// unusable location and why: the bucket does not exist, it carries no policy, or its
/// policy grants the actions of the other end rather than the ones this end needs.
#[tokio::test]
async fn a_definition_naming_an_unusable_s3_location_is_refused() {
    let rows = [
        UnusableLocation {
            case: "the source bucket does not exist",
            buckets: &[("usable-out", Some(&EVERY_RUN_ACTION))],
            source_bucket: "absent-in",
            destination_bucket: "usable-out",
            message: "Amazon S3 bucket absent-in does not exist. Create the bucket and try again."
                .to_string(),
        },
        UnusableLocation {
            case: "the destination bucket does not exist",
            buckets: &[("usable-in", Some(&EVERY_RUN_ACTION))],
            source_bucket: "usable-in",
            destination_bucket: "absent-out",
            message: "Amazon S3 bucket absent-out does not exist. Create the bucket and try \
                      again."
                .to_string(),
        },
        UnusableLocation {
            case: "the source bucket carries no policy",
            buckets: &[
                ("unpolicied-in", None),
                ("usable-out", Some(&EVERY_RUN_ACTION)),
            ],
            source_bucket: "unpolicied-in",
            destination_bucket: "usable-out",
            message: missing_actions_refusal(
                CREATE_FLOW_REQUEST,
                "unpolicied-in",
                &["s3:listbucket", "s3:getobject"],
            ),
        },
        UnusableLocation {
            case: "the destination bucket carries no policy",
            buckets: &[
                ("usable-in", Some(&EVERY_RUN_ACTION)),
                ("unpolicied-out", None),
            ],
            source_bucket: "usable-in",
            destination_bucket: "unpolicied-out",
            message: missing_actions_refusal(
                CREATE_FLOW_REQUEST,
                "unpolicied-out",
                &["s3:putobject"],
            ),
        },
        UnusableLocation {
            case: "the source policy grants the destination's actions instead of the source's",
            buckets: &[
                ("mispolicied-in", Some(&DESTINATION_POLICY_ACTIONS)),
                ("usable-out", Some(&EVERY_RUN_ACTION)),
            ],
            source_bucket: "mispolicied-in",
            destination_bucket: "usable-out",
            message: missing_actions_refusal(
                CREATE_FLOW_REQUEST,
                "mispolicied-in",
                &["s3:listbucket", "s3:getobject"],
            ),
        },
        UnusableLocation {
            case: "the destination policy grants the source's actions instead of its own",
            buckets: &[
                ("usable-in", Some(&EVERY_RUN_ACTION)),
                ("mispolicied-out", Some(&SOURCE_POLICY_ACTIONS)),
            ],
            source_bucket: "usable-in",
            destination_bucket: "mispolicied-out",
            message: missing_actions_refusal(
                CREATE_FLOW_REQUEST,
                "mispolicied-out",
                &["s3:putobject"],
            ),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        for (bucket, actions) in row.buckets {
            emulator.create_bucket(bucket).await;
            if let Some(actions) = actions {
                emulator
                    .put_bucket_policy(bucket, &appflow_bucket_policy(bucket, actions))
                    .await;
            }
        }

        let flow_name = "unusable-location-flow";
        let refused = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at(row.source_bucket, "inbound", None),
                    "destinationFlowConfigList": [s3_destination_at(
                        row.destination_bucket, "outbound", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the create to be refused, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            row.message,
            "case {case:?}: expected the refusal to name the location, got {:?}",
            refused.json()
        );
        let listed = emulator.appflow("/list-flows", &json!({})).await;
        assert_eq!(
            listed.member("flows").as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected no flow to have been created, got {:?}",
            listed.member("flows")
        );
    }
}

/// `UpdateFlow` stores a definition under the SAME rules `CreateFlow` does: a definition
/// naming a location AppFlow could not use is refused there too, and the flow keeps the
/// definition it had. The rows differ along what the re-definition names.
#[tokio::test]
async fn re_defining_a_flow_onto_an_unusable_s3_location_is_refused() {
    for (case, bucket, message) in [
        (
            "a bucket that does not exist",
            "never-created-bucket",
            "Amazon S3 bucket never-created-bucket does not exist. Create the bucket and try \
             again."
                .to_string(),
        ),
        (
            "a bucket that carries no policy",
            "unpolicied-target",
            missing_actions_refusal(UPDATE_FLOW_REQUEST, "unpolicied-target", &["s3:putobject"]),
        ),
        (
            "a bucket whose policy grants only what a SOURCE needs",
            "read-only-target",
            missing_actions_refusal(UPDATE_FLOW_REQUEST, "read-only-target", &["s3:putobject"]),
        ),
    ] {
        let emulator = Emulator::new();
        emulator.stand_up(&["update-source", "update-target"]).await;
        emulator.create_bucket("unpolicied-target").await;
        emulator.create_bucket("read-only-target").await;
        emulator
            .put_bucket_policy(
                "read-only-target",
                &appflow_bucket_policy("read-only-target", &SOURCE_POLICY_ACTIONS),
            )
            .await;
        let flow_name = "update-location-flow";
        let created = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at("update-source", "inbound", None),
                    "destinationFlowConfigList": [s3_destination_at(
                        "update-target", "outbound", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.json()
        );

        let refused = emulator
            .appflow(
                "/update-flow",
                &json!({
                    "flowName": flow_name,
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at("update-source", "inbound", None),
                    "destinationFlowConfigList": [s3_destination_at(bucket, "outbound", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the update to be refused, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            message,
            "case {case:?}: expected the refusal to name the location, got {:?}",
            refused.json()
        );

        // The refused update changed nothing: the flow still names the location it was
        // created with.
        let described = emulator
            .appflow("/describe-flow", &json!({ "flowName": flow_name }))
            .await;
        let destination = described
            .member("destinationFlowConfigList")
            .as_array()
            .and_then(|list| list.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            destination
                .get("destinationConnectorProperties")
                .and_then(|properties| properties.get("S3"))
                .and_then(|s3| s3.get("bucketName"))
                .and_then(Value::as_str),
            Some("update-target"),
            "case {case:?}: expected the stored definition to be untouched, got {destination:?}"
        );
    }
}

/// One row of the missing-grant table: what each end's bucket policy grants, and the
/// bucket and the actions AppFlow's refusal has to name.
struct PolicyGap {
    case: &'static str,
    source_bucket: &'static str,
    source_granted: Granted,
    destination_bucket: &'static str,
    destination_granted: Granted,
    /// The bucket the refusal names, and the actions it enumerates as missing.
    named_bucket: &'static str,
    missing: &'static [&'static str],
}

/// What a row's bucket policy grants the AppFlow service principal.
enum Granted {
    /// No bucket policy at all.
    Nothing,
    /// An `Allow` of exactly these actions.
    Allow(&'static [&'static str]),
    /// An `Allow` of every action a run takes, with these ones explicitly denied.
    AllowAllExcept(&'static [&'static str]),
}

impl Granted {
    /// The policy document to place on `bucket`, or `None` to leave it unpolicied.
    fn document(&self, bucket: &str) -> Option<Value> {
        match self {
            Granted::Nothing => None,
            Granted::Allow(actions) => Some(appflow_bucket_policy(bucket, actions)),
            Granted::AllowAllExcept(denied) => {
                let mut policy = appflow_bucket_policy(bucket, &EVERY_RUN_ACTION);
                policy["Statement"]
                    .as_array_mut()
                    .expect("the policy carries its statements")
                    .push(json!({
                        "Sid": "DenyAppFlow",
                        "Effect": "Deny",
                        "Principal": { "Service": "appflow.amazonaws.com" },
                        "Action": denied,
                        "Resource": [
                            format!("arn:aws:s3:::{bucket}"),
                            format!("arn:aws:s3:::{bucket}/*"),
                        ],
                    }));
                Some(policy)
            }
        }
    }
}

/// A definition naming a bucket whose policy does not grant the AppFlow service principal
/// every action a run takes on that end is refused in AppFlow's own words: the
/// request-failed envelope naming the call, the bucket the definition named, the actions
/// that bucket's policy still leaves ungranted, the principal they have to be granted to
/// and the two resources the grant has to cover.
///
/// The rows differ along which end carries the gap, how the gap arises (no policy at all,
/// a policy granting the other end's actions, a partial grant, an explicit `Deny` over an
/// otherwise complete grant), how many actions are missing and the shape of the bucket
/// name the ARNs are derived from — so what is pinned is the template and the values it
/// takes from the request and from the policy state, not one case's text.
#[tokio::test]
async fn a_bucket_policy_missing_a_run_action_is_refused_in_appflows_words() {
    // The actions a run takes on each end, which are what a policy has to grant: a source
    // is LISTED and READ, a destination is WRITTEN.
    const LIST_ONLY: [&str; 1] = ["s3:ListBucket"];
    const READ_ONLY: [&str; 1] = ["s3:GetObject"];

    let rows = [
        PolicyGap {
            case: "the source bucket carries no policy at all",
            source_bucket: "gap-src-none",
            source_granted: Granted::Nothing,
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap-src-none",
            missing: &["s3:listbucket", "s3:getobject"],
        },
        PolicyGap {
            case: "the source policy grants the listing but not the read",
            source_bucket: "gap-src-list",
            source_granted: Granted::Allow(&LIST_ONLY),
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap-src-list",
            missing: &["s3:getobject"],
        },
        PolicyGap {
            case: "the source policy grants the read but not the listing",
            source_bucket: "gap-src-read",
            source_granted: Granted::Allow(&READ_ONLY),
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap-src-read",
            missing: &["s3:listbucket"],
        },
        PolicyGap {
            case: "the source policy denies the read it otherwise grants",
            source_bucket: "gap-src-denied-read",
            source_granted: Granted::AllowAllExcept(&READ_ONLY),
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap-src-denied-read",
            missing: &["s3:getobject"],
        },
        PolicyGap {
            case: "the source policy denies both actions it otherwise grants",
            source_bucket: "gap-src-denied-both",
            source_granted: Granted::AllowAllExcept(&SOURCE_POLICY_ACTIONS),
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap-src-denied-both",
            missing: &["s3:listbucket", "s3:getobject"],
        },
        PolicyGap {
            case: "a dotted source bucket name the resource ARNs are derived from",
            source_bucket: "gap.src.dotted-10",
            source_granted: Granted::Nothing,
            destination_bucket: "gap-dst-ok",
            destination_granted: Granted::Allow(&EVERY_RUN_ACTION),
            named_bucket: "gap.src.dotted-10",
            missing: &["s3:listbucket", "s3:getobject"],
        },
        PolicyGap {
            case: "the destination bucket carries no policy at all",
            source_bucket: "gap-src-ok",
            source_granted: Granted::Allow(&EVERY_RUN_ACTION),
            destination_bucket: "gap-dst-none",
            destination_granted: Granted::Nothing,
            named_bucket: "gap-dst-none",
            missing: &["s3:putobject"],
        },
        PolicyGap {
            case: "the destination policy grants only what a source needs",
            source_bucket: "gap-src-ok",
            source_granted: Granted::Allow(&EVERY_RUN_ACTION),
            destination_bucket: "gap-dst-read",
            destination_granted: Granted::Allow(&SOURCE_POLICY_ACTIONS),
            named_bucket: "gap-dst-read",
            missing: &["s3:putobject"],
        },
        PolicyGap {
            case: "the destination policy denies the write it otherwise grants",
            source_bucket: "gap-src-ok",
            source_granted: Granted::Allow(&EVERY_RUN_ACTION),
            destination_bucket: "gap-dst-denied-write",
            destination_granted: Granted::AllowAllExcept(&["s3:PutObject"]),
            named_bucket: "gap-dst-denied-write",
            missing: &["s3:putobject"],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        for (bucket, granted) in [
            (row.source_bucket, &row.source_granted),
            (row.destination_bucket, &row.destination_granted),
        ] {
            emulator.create_bucket(bucket).await;
            if let Some(document) = granted.document(bucket) {
                emulator.put_bucket_policy(bucket, &document).await;
            }
        }

        let refused = emulator
            .appflow(
                "/create-flow",
                &json!({
                    "flowName": "policy-gap-flow",
                    "triggerConfig": on_demand_trigger(),
                    "sourceFlowConfig": s3_source_at(row.source_bucket, "input", Some("CSV")),
                    "destinationFlowConfigList": [s3_destination_at(
                        row.destination_bucket, "output", None)],
                    "tasks": [map_all_task(&[])],
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {} {:?}",
            refused.status,
            refused.json()
        );
        let expected = missing_actions_refusal(CREATE_FLOW_REQUEST, row.named_bucket, row.missing);
        assert_eq!(
            refused.string("message"),
            expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {:?}",
            refused.string("message")
        );
    }
}

/// A definition naming more destinations than one run of this emulator writes is refused
/// where it is stored, by both operations that store one. The rows differ along the count,
/// on each side of the published ceiling.
#[tokio::test]
async fn a_definition_past_the_destination_ceiling_is_refused() {
    let ceiling = service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::MAX_DESTINATIONS_PER_RUN;
    for (case, count, refused) in [
        ("one destination", 1, false),
        ("as many destinations as a run writes", ceiling, false),
        ("one destination too many", ceiling + 1, true),
        ("far past the ceiling", ceiling * 4, true),
    ] {
        let emulator = Emulator::new();
        emulator.stand_up(&["ceiling-in", "ceiling-out"]).await;
        let destinations: Vec<Value> = (0..count)
            .map(|_| s3_destination_at("ceiling-out", "outbound", None))
            .collect();
        let flow_name = "destination-ceiling-flow";
        let definition = json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source_at("ceiling-in", "inbound", None),
            "destinationFlowConfigList": destinations,
            "tasks": [map_all_task(&[])],
        });
        let expected_message = format!(
            "A flow transfers to at most {ceiling} destinations and this definition names {count}."
        );

        let created = emulator.appflow("/create-flow", &definition).await;
        if refused {
            assert_eq!(
                (created.status, created.error.as_str(), created.string("message").as_str()),
                (400, "ValidationException", expected_message.as_str()),
                "case {case:?}: expected CreateFlow to refuse the definition, got {} {:?}",
                created.status,
                created.json()
            );
            // And the same definition is refused as a RE-definition, so neither operation
            // stores one the other would not.
            let accepted = emulator
                .appflow(
                    "/create-flow",
                    &json!({
                        "flowName": flow_name,
                        "triggerConfig": on_demand_trigger(),
                        "sourceFlowConfig": s3_source_at("ceiling-in", "inbound", None),
                        "destinationFlowConfigList": [s3_destination_at(
                            "ceiling-out", "outbound", None)],
                        "tasks": [map_all_task(&[])],
                    }),
                )
                .await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: expected a one-destination flow to be created, got {} {:?}",
                accepted.status,
                accepted.json()
            );
            let updated = emulator.appflow("/update-flow", &definition).await;
            assert_eq!(
                (updated.status, updated.error.as_str(), updated.string("message").as_str()),
                (400, "ValidationException", expected_message.as_str()),
                "case {case:?}: expected UpdateFlow to refuse the definition, got {} {:?}",
                updated.status,
                updated.json()
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected a definition within the ceiling to be stored, got {} {:?}",
                created.status,
                created.json()
            );
        }
    }
}

/// A second flow of the same name is refused with AppFlow's conflict sentence, whose only
/// variable part is the name the request carried. The rows differ along the name.
#[tokio::test]
async fn a_second_flow_of_the_same_name_conflicts() {
    for flow_name in ["orders-flow", "Orders_Flow-2", "a.b-c_d"] {
        let emulator = Emulator::new();
        emulator.stand_up(&["duplicate-in", "duplicate-out"]).await;
        let create = json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source_at("duplicate-in", "inbound", None),
            "destinationFlowConfigList": [s3_destination_at("duplicate-out", "outbound", None)],
            "tasks": [map_all_task(&[])],
        });
        let created = emulator.appflow("/create-flow", &create).await;
        assert_eq!(
            created.status, 200,
            "case {flow_name:?}: expected the first create to succeed, got {} {:?}",
            created.status,
            created.json()
        );
        let refused = emulator.appflow("/create-flow", &create).await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (409, "ConflictException"),
            "case {flow_name:?}: expected the second create to conflict, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            format!("Conflict executing request: Flow with name {flow_name} already exists."),
            "case {flow_name:?}: expected AppFlow's conflict sentence, got {:?}",
            refused.json()
        );
    }
}

/// A value outside a modelled `@pattern` is refused in AWS's constraint envelope, with the
/// expression quoted as the SERVICE validates it: without the anchors a whole-value
/// pattern wraps it in, and — for the name class AppFlow validates more narrowly than its
/// published model — as the service's own expression. The rows differ along the member,
/// the operation and the rejected value.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_the_service_validates() {
    let emulator = Emulator::new();
    // The expressions are the model's own
    // (`smithy_query(query="shape", shape="FlowName,ConnectorProfileName")`).
    for (case, path, request, member, value, pattern) in [
        (
            "a flow name starting outside the name class",
            "/describe-flow",
            json!({ "flowName": "-leading-hyphen" }),
            "flowName",
            "-leading-hyphen",
            r"[a-zA-Z0-9][\w!@.-]+",
        ),
        (
            "a flow name carrying a space",
            "/start-flow",
            json!({ "flowName": "two words" }),
            "flowName",
            "two words",
            r"[a-zA-Z0-9][\w!@.-]+",
        ),
        (
            "a flow name carrying a character the class admits nowhere",
            "/delete-flow",
            json!({ "flowName": "orders$flow" }),
            "flowName",
            "orders$flow",
            r"[a-zA-Z0-9][\w!@.-]+",
        ),
    ] {
        let refused = emulator.appflow(path, &request).await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the request to be refused, got {} {:?}",
            refused.status,
            refused.json()
        );
        assert_eq!(
            refused.string("message"),
            format!(
                "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {pattern}"
            ),
            "case {case:?}: expected AWS's constraint sentence, got {:?}",
            refused.json()
        );
    }

    // A member whose service-side expression is the published one: the anchors are
    // dropped and nothing else changes.
    let refused = emulator
        .appflow(
            "/create-connector-profile",
            &json!({
                "connectorProfileName": "two words",
                "connectorType": "Salesforce",
                "connectionMode": "Public",
                "connectorProfileConfig": {
                    "connectorProfileProperties": { "Salesforce": {} }
                }
            }),
        )
        .await;
    let message = refused.string("message");
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (400, "ValidationException"),
        "expected a connector profile name outside its pattern to be refused, got {} {:?}",
        refused.status,
        refused.json()
    );
    assert!(
        message.contains(r"Member must satisfy regular expression pattern: [\w/!@#+=.-]+"),
        "expected the published expression without its anchors, got {message:?}"
    );
    assert!(
        !message.contains(r"^[\w/!@#+=.-]+$"),
        "expected no anchored expression in the refusal, got {message:?}"
    );
}
