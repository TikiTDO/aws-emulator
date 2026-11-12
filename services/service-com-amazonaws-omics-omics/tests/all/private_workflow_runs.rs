// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the private-workflow / run slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted record.
//! Anything `CreateWorkflow` or `StartRun` stored is what a later `GetWorkflow`, `GetRun`,
//! `ListWorkflows` or `ListRuns` reports; a replayed `requestId` answers with the resource
//! the first call created rather than a second one; a status the clock owes is the only
//! thing a read changes; and a record `DeleteRun`/`DeleteWorkflow` removed is a modelled
//! 404 everywhere afterwards.
//!
//! The last suite asserts the second rule this slice carries: every call leaves ONE
//! structured, attributable record on the `emulator::audit` target — above all the four
//! that change the store, whose effect is otherwise invisible after the fact — carrying
//! the operation, the identity the request presented, the request target, the resource and
//! the outcome, and none of the caller's own data.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::base64;
use serde_json::{json, Map, Value};
use service_com_amazonaws_omics_omics::state::{FIRST_RESOURCE_ID, MAX_TAGS};
use sha2::Digest;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; the ARNs must carry exactly this one.
///
/// This module owns the HTTP harness every suite in this crate drives the emulator
/// through, so the items the other suites reuse are `pub(crate)`.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// A role ARN satisfying the modelled `RunRoleArn` (len 1..128, `^arn:.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmicsWorkflowRole";
/// An output URI satisfying the modelled `RunOutputUri` (len 1..750).
const OUTPUT_URI: &str = "s3://omics-emulator-bucket/runs";

/// Every value the modelled `WorkflowEngine` enum admits.
/// Source: `smithy_query(query="shape",shape="WorkflowEngine")` -> CWL | NEXTFLOW | WDL |
/// WDL_LENIENT.
pub(crate) const WORKFLOW_ENGINES: &[&str] = &["CWL", "NEXTFLOW", "WDL", "WDL_LENIENT"];
/// Every value the modelled `StorageType` enum admits.
/// Source: `smithy_query(query="shape",shape="StorageType")` -> STATIC | DYNAMIC.
pub(crate) const STORAGE_TYPES: &[&str] = &["STATIC", "DYNAMIC"];
/// Every value the modelled `RunLogLevel` enum admits.
/// Source: `smithy_query(query="shape",shape="RunLogLevel")` -> OFF | FATAL | ERROR | ALL.
const RUN_LOG_LEVELS: &[&str] = &["OFF", "FATAL", "ERROR", "ALL"];
/// Every value the modelled `RunRetentionMode` enum admits.
/// Source: `smithy_query(query="shape",shape="RunRetentionMode")` -> RETAIN | REMOVE.
const RUN_RETENTION_MODES: &[&str] = &["RETAIN", "REMOVE"];
/// The published bounds of the modelled `storageCapacity` / `priority` range (0..100000).
/// Source: the `storageCapacity` and `priority` members of `CreateWorkflowRequest` and
/// `StartRunRequest`.
pub(crate) const CAPACITY_RANGE: (i64, i64) = (0, 100_000);

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_omics_omics::EmulatedService::new(
        engine,
    ));
    router
}

/// The access key id a call presents when a case does not name one of its own.
const DEFAULT_AKID: &str = "AKIDOMICSTEST";

/// A request signed for `omics` in [`REGION`] — the signal the router routes on and the
/// handlers read the region from — presenting `akid` as the SigV4 access key id, which is
/// the identity the service's audit record attributes the call to.
fn request_as(
    method: &str,
    uri: &str,
    body: Option<&Value>,
    akid: &str,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("omics.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/omics/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_as(router, method, uri, body, DEFAULT_AKID).await
}

/// The same call, presenting `akid` as the SigV4 access key id.
async fn call_as(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
    akid: &str,
) -> (u16, Value) {
    let response = router.dispatch(request_as(method, uri, body, akid)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.omics#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

pub(crate) async fn create_workflow(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/workflow", Some(body)).await
}

pub(crate) async fn get_workflow(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/workflow/{id}"), None).await
}

pub(crate) async fn delete_workflow(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/workflow/{id}"), None).await
}

async fn list_workflows(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/workflow{query}"), None).await
}

pub(crate) async fn start_run(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/run", Some(body)).await
}

pub(crate) async fn get_run(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/run/{id}"), None).await
}

async fn delete_run(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/run/{id}"), None).await
}

pub(crate) async fn list_runs(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/run{query}"), None).await
}

/// The `id` member of a 201 response, after asserting the status.
pub(crate) fn created_id(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
    body.get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no id, got {body:?}"))
        .to_string()
}

/// Assert `id` is the decimal the modelled `WorkflowId`/`RunId` pattern `^[0-9]+$`
/// (len 1..18) describes, and that it is one this emulator could have minted.
fn assert_numeric_id(case: &str, id: &str) {
    assert!(
        !id.is_empty() && id.len() <= 18 && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a 1..18 digit decimal id, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: id {id:?} does not parse as a number"));
    assert!(
        value >= FIRST_RESOURCE_ID,
        "case {case:?}: expected an id at or above {FIRST_RESOURCE_ID}, got {value}"
    );
}

/// Assert `arn` is this emulator's ARN for `resource/id`.
pub(crate) fn assert_arn(case: &str, arn: &str, resource: &str, id: &str) {
    let expected = format!("arn:aws:omics:{REGION}:{ACCOUNT}:{resource}/{id}");
    assert_eq!(
        arn, expected,
        "case {case:?}: expected ARN {expected}, got {arn:?}"
    );
}

/// Assert `uuid` satisfies the modelled `WorkflowUuid` pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
pub(crate) fn assert_uuid(case: &str, uuid: &str) {
    let groups: Vec<&str> = uuid.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected five hyphen-separated groups, got {uuid:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit()),
            "case {case:?}: expected a {width}-character hex group, got {group:?} in {uuid:?}"
        );
    }
}

/// The hex SHA-256 of `bytes`: the digest a workflow's definition must report.
pub(crate) fn expected_digest(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Assert `member` is present and is a date-time timestamp, and answer the instant it
/// names in epoch seconds.
///
/// Every timestamp member of this model declares `@timestampFormat("date-time")`, so the
/// wire form is an RFC 3339 STRING and not the epoch NUMBER the restJson1 protocol
/// defaults to; `tests/all/response_member_wire_form.rs` asserts that rule over the model and over every path
/// that reports an instant, and this helper holds every other suite to it.
pub(crate) fn assert_timestamp(case: &str, member: &str, body: &Value) -> f64 {
    let text = body
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {member} date-time string, got {body:?}")
        })
        .to_string();
    let instant = aws_smithy_types::DateTime::from_str(
        &text,
        aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} is not an RFC 3339 date-time: {error}")
    });
    let value = instant.as_secs_f64();
    assert!(
        value > 0.0,
        "case {case:?}: expected a {member} after the epoch, got {text:?}"
    );
    value
}

/// Every member of `expected` must appear in `actual` with exactly that value.
pub(crate) fn assert_members(case: &str, via: &str, expected: &Value, actual: &Value) {
    let expected = expected.as_object().expect("an object of expected members");
    for (member, want) in expected {
        let got = actual.get(member);
        assert_eq!(
            got,
            Some(want),
            "case {case:?} read via {via}: expected {member} = {want}, got {got:?} \
             in the whole response {actual:?}"
        );
    }
}

/// A tag map with `count` entries, each key and value inside the modelled `TagKey` /
/// `TagValue` lengths.
///
/// The one copy every suite in this crate builds its `tags` member from.
pub(crate) fn tag_map(count: usize) -> Value {
    let mut tags = Map::new();
    for n in 0..count {
        tags.insert(format!("key{n}"), Value::String(format!("value{n}")));
    }
    Value::Object(tags)
}

/// Assert `actual` carries none of `members`.
pub(crate) fn assert_absent(case: &str, members: &[&str], actual: &Value) {
    for member in members {
        assert!(
            actual.get(*member).is_none(),
            "case {case:?}: expected no {member}, got {:?} in {actual:?}",
            actual.get(*member)
        );
    }
}

/// The `items` list of a 200 List response, after asserting the status.
pub(crate) fn items(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
    body.get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no items, got {body:?}"))
        .clone()
}

/// The `id` of each item in a List page.
fn item_ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<no id>")
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CreateWorkflow -> GetWorkflow
// ---------------------------------------------------------------------------

/// The modelled `RunStatus` values a run holds while it is still in flight: the entries
/// of the progression before its terminal one, plus the transient `STOPPING`.
/// Source: `smithy_query(query="shape",shape="RunStatus")` -> PENDING | STARTING |
/// RUNNING | STOPPING | COMPLETED | DELETED | CANCELLED | FAILED.
const IN_FLIGHT_RUN_STATUSES: &[&str] = &["PENDING", "STARTING", "RUNNING", "STOPPING"];

/// How long a clock-owed transition is given to settle before a test gives up.
///
/// Generously above the spans the handlers actually owe, so a loaded machine cannot turn
/// an assertion into a timing failure; the polls below return as soon as the status is
/// there, so the slack costs nothing when the machine is idle.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often a poll re-reads while waiting for a status.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Read the workflow until it reports `expected`, and answer that response.
///
/// Polling rather than sleeping a fixed span: the status is owed to the clock, so what is
/// asserted is that it arrives, not when a single read happened to land.
async fn await_workflow_status(
    router: &MultiServiceRouter,
    id: &str,
    expected: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = get_workflow(router, id).await;
        assert_eq!(status, 200, "workflow {id}: expected 200, got {body:?}");
        if body.get("status").and_then(Value::as_str) == Some(expected) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "workflow {id}: expected status {expected} within {SETTLE_DEADLINE:?}, \
             last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the run until it reports `expected`, and answer that response.
async fn await_run_status(router: &MultiServiceRouter, id: &str, expected: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = get_run(router, id).await;
        assert_eq!(status, 200, "run {id}: expected 200, got {body:?}");
        if body.get("status").and_then(Value::as_str) == Some(expected) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {id}: expected status {expected} within {SETTLE_DEADLINE:?}, last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until every run in `ids` has reached the terminal status a run nothing executed
/// ends in — the only status `DeleteRun` admits.
async fn await_runs_finished(router: &MultiServiceRouter, ids: &[String]) {
    for id in ids {
        await_run_status(router, id, "FAILED").await;
    }
}

/// Attempt `DeleteRun` and assert the answer obeys "you can only delete a run that has
/// reached a COMPLETED, FAILED, or CANCELLED stage", whichever side of the clock the call
/// landed on. Answers `true` when the delete was refused.
///
/// A fixed expectation would be timing-sensitive: these attempts are made while the run
/// is still in flight, but the clock owes it a terminal status shortly after, so a
/// stalled runner could meet the finished run instead. Pairing the answer with what the
/// store then reports asserts the rule either way: a refusal must be a
/// `ConflictException` that left the record in place, and an acceptance must have removed
/// it.
async fn delete_run_obeys_the_rule(router: &MultiServiceRouter, id: &str) -> bool {
    let (code, body) = delete_run(router, id).await;
    match code {
        409 => {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "run {id}: a refused delete must be a ConflictException, got {body:?}"
            );
            let (read, run) = get_run(router, id).await;
            assert_eq!(
                read, 200,
                "run {id}: a refused delete must leave the record in place, got {run:?}"
            );
            true
        }
        202 => {
            let (read, run) = get_run(router, id).await;
            assert_eq!(
                read, 404,
                "run {id}: an accepted delete must remove the record, got {run:?}"
            );
            false
        }
        other => panic!(
            "run {id}: DeleteRun answered {other}, neither the 202 of an accepted delete \
             nor the 409 of a refused one: {body:?}"
        ),
    }
}

/// The same for `DeleteWorkflow` and its blocking-run rule: a refusal must be a
/// `ConflictException` that left the workflow in place, an acceptance must have removed
/// it. Answers `true` when the delete was refused.
async fn delete_workflow_obeys_the_rule(router: &MultiServiceRouter, id: &str) -> bool {
    let (code, body) = delete_workflow(router, id).await;
    match code {
        409 => {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "workflow {id}: a refused delete must be a ConflictException, got {body:?}"
            );
            let (read, workflow) = get_workflow(router, id).await;
            assert_eq!(
                read, 200,
                "workflow {id}: a refused delete must leave the record in place, \
                 got {workflow:?}"
            );
            true
        }
        202 => {
            let (read, workflow) = get_workflow(router, id).await;
            assert_eq!(
                read, 404,
                "workflow {id}: an accepted delete must remove the record, got {workflow:?}"
            );
            false
        }
        other => panic!(
            "workflow {id}: DeleteWorkflow answered {other}, neither the 202 of an accepted \
             delete nor the 409 of a refused one: {body:?}"
        ),
    }
}

/// A row of the workflow create/read-back table: a request body, and the members a
/// later `GetWorkflow` must report for it.
struct WorkflowCase {
    /// How the row is reported in a failure.
    case: &'static str,
    request: Value,
    /// Every member the read-back must carry, exactly.
    expected: Value,
    /// Members the read-back must NOT carry, because nothing stored them.
    absent: Vec<&'static str>,
}

/// The table. The rule is "a Get reports what the create stored", so the rows differ
/// along what the rule generalises over: which optional members are present, every value
/// of the `engine` and `storageType` catalogs, both published bounds of the
/// `storageCapacity` range, the number of `parameterTemplate` and `tags` entries, and the
/// form the definition arrived in (none, URI, ZIP, or both) — which is also what decides
/// whether a content `digest` can be reported at all.
fn workflow_cases() -> Vec<WorkflowCase> {
    let zip: &[u8] = b"PK\x03\x04 emulator workflow definition archive";
    let zip_b64 = base64::encode(zip);
    let mut cases = vec![
        // Only the one required member: nothing else may appear from nowhere.
        WorkflowCase {
            case: "requestId-only",
            request: json!({ "requestId": "req-minimal" }),
            expected: json!({ "type": "PRIVATE" }),
            absent: vec![
                "name",
                "description",
                "engine",
                "main",
                "definition",
                "digest",
                "parameterTemplate",
                "storageCapacity",
                "storageType",
                "accelerators",
                "tags",
            ],
        },
        // The low published bound of `storageCapacity`, an accelerator, one tag and one
        // parameter.
        WorkflowCase {
            case: "capacity-low-bound",
            request: json!({
                "requestId": "req-low",
                "name": "capacity-low-bound",
                "description": "a workflow at the low storage bound",
                "engine": "WDL",
                "main": "main.wdl",
                "definitionUri": "s3://omics-emulator-bucket/low.zip",
                "storageCapacity": CAPACITY_RANGE.0,
                "storageType": "STATIC",
                "accelerators": "GPU",
                "parameterTemplate": { "sample": { "description": "the sample", "optional": false } },
                "tags": { "team": "genomics" },
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": "capacity-low-bound",
                "description": "a workflow at the low storage bound",
                "engine": "WDL",
                "main": "main.wdl",
                "definition": "s3://omics-emulator-bucket/low.zip",
                "storageCapacity": CAPACITY_RANGE.0,
                "storageType": "STATIC",
                "accelerators": "GPU",
                "parameterTemplate": { "sample": { "description": "the sample", "optional": false } },
                "tags": { "team": "genomics" },
            }),
            // Only a URI, so this emulator never read the archive: no `digest` may be
            // reported for it.
            absent: vec!["digest"],
        },
        // The high published bound, several parameters and several tags.
        WorkflowCase {
            case: "capacity-high-bound",
            request: json!({
                "requestId": "req-high",
                "name": "capacity-high-bound",
                "engine": "NEXTFLOW",
                "main": "workflow-definition/main.nf",
                "definitionUri": "s3://omics-emulator-bucket/high.zip",
                "storageCapacity": CAPACITY_RANGE.1,
                "storageType": "DYNAMIC",
                "parameterTemplate": {
                    "reads": { "description": "input reads" },
                    "reference": { "description": "reference genome", "optional": true },
                    "threads": { "optional": true },
                },
                "tags": { "team": "genomics", "env": "test", "owner": "ci" },
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": "capacity-high-bound",
                "engine": "NEXTFLOW",
                "main": "workflow-definition/main.nf",
                "definition": "s3://omics-emulator-bucket/high.zip",
                "storageCapacity": CAPACITY_RANGE.1,
                "storageType": "DYNAMIC",
                "parameterTemplate": {
                    "reads": { "description": "input reads" },
                    "reference": { "description": "reference genome", "optional": true },
                    "threads": { "optional": true },
                },
                "tags": { "team": "genomics", "env": "test", "owner": "ci" },
            }),
            absent: vec!["accelerators", "digest"],
        },
        // The definition arriving as a ZIP blob rather than a URI: the archive's own
        // bytes ARE the definition's content, so the digest a client computes over the
        // archive it uploaded is what must be reported; there is no URI to report.
        WorkflowCase {
            case: "definition-zip-only",
            request: json!({
                "requestId": "req-zip",
                "name": "definition-zip-only",
                "engine": "CWL",
                "main": "main.cwl",
                "definitionZip": zip_b64,
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": "definition-zip-only",
                "engine": "CWL",
                "main": "main.cwl",
                "digest": expected_digest(zip),
            }),
            absent: vec!["definition"],
        },
        // Both forms present: the archive is the definition's content, so it is what the
        // digest covers, and the URI is still reported.
        WorkflowCase {
            case: "definition-zip-and-uri",
            request: json!({
                "requestId": "req-zip-and-uri",
                "name": "definition-zip-and-uri",
                "engine": "WDL_LENIENT",
                "definitionZip": zip_b64,
                "definitionUri": "s3://omics-emulator-bucket/both.zip",
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": "definition-zip-and-uri",
                "engine": "WDL_LENIENT",
                "definition": "s3://omics-emulator-bucket/both.zip",
                "digest": expected_digest(zip),
            }),
            absent: vec!["main"],
        },
        // A name using the punctuation the modelled `WorkflowName` pattern admits.
        WorkflowCase {
            case: "punctuated name",
            request: json!({
                "requestId": "req-punctuated",
                "name": "Prod [us-west-2] (tier.1): joint-genotyping",
                "engine": "WDL",
                "definitionUri": "s3://omics-emulator-bucket/punctuated.zip",
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": "Prod [us-west-2] (tier.1): joint-genotyping",
                "engine": "WDL",
                "definition": "s3://omics-emulator-bucket/punctuated.zip",
            }),
            absent: vec!["digest"],
        },
    ];
    // One row per remaining (engine, storageType) pair the catalogs admit, so every
    // published value of both enums is exercised and not only the ones above.
    for (index, engine) in WORKFLOW_ENGINES.iter().enumerate() {
        let storage_type = STORAGE_TYPES[index % STORAGE_TYPES.len()];
        let uri = format!("s3://omics-emulator-bucket/catalog-{engine}.zip");
        cases.push(WorkflowCase {
            case: "engine catalog row",
            request: json!({
                "requestId": format!("req-catalog-{engine}"),
                "name": format!("catalog-{engine}"),
                "engine": engine,
                "storageType": storage_type,
                "definitionUri": uri.clone(),
            }),
            expected: json!({
                "type": "PRIVATE",
                "name": format!("catalog-{engine}"),
                "engine": engine,
                "storageType": storage_type,
                "definition": uri.clone(),
            }),
            absent: vec!["digest"],
        });
    }
    cases
}

#[tokio::test]
async fn a_created_workflow_reads_back_from_state() {
    let router = emulator();
    let cases = workflow_cases();

    // Create them all first, so each read-back also proves the store keeps the rows
    // apart rather than answering with the most recent one.
    let mut created = Vec::new();
    let mut ids = BTreeSet::new();
    let mut uuids = BTreeSet::new();
    for workflow in &cases {
        let (status, body) = create_workflow(&router, &workflow.request).await;
        let id = created_id(workflow.case, status, &body);
        assert_numeric_id(workflow.case, &id);
        let arn = body.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_arn(workflow.case, arn, "workflow", &id);
        let uuid = body.get("uuid").and_then(Value::as_str).unwrap_or_default();
        assert_uuid(workflow.case, uuid);
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some("CREATING"),
            "case {:?}: a just-created workflow must report the transient status, got {body:?}",
            workflow.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {:?}: id {id} was already handed out",
            workflow.case
        );
        assert!(
            uuids.insert(uuid.to_string()),
            "case {:?}: uuid {uuid} was already handed out",
            workflow.case
        );
        created.push((id, arn.to_string(), uuid.to_string()));
    }

    for (workflow, (id, arn, uuid)) in cases.iter().zip(&created) {
        // Read until the clock has settled the CREATING -> ACTIVE transition the create
        // owes, then assert the whole record that read reported.
        let body = await_workflow_status(&router, id, "ACTIVE").await;
        assert_members(workflow.case, "GetWorkflow", &workflow.expected, &body);
        assert_absent(workflow.case, &workflow.absent, &body);
        assert_members(
            workflow.case,
            "GetWorkflow",
            &json!({
                "id": id,
                "arn": arn,
                "uuid": uuid,
                "status": "ACTIVE",
            }),
            &body,
        );
        assert_timestamp(workflow.case, "creationTime", &body);
    }
}

#[tokio::test]
async fn a_replayed_request_id_returns_the_same_workflow() {
    let router = emulator();
    // The rule generalises over the token's text and over how much of the rest of the
    // request changes on the replay: the token alone decides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "identical replay",
            json!({ "requestId": "token-1", "name": "alpha", "engine": "WDL" }),
            json!({ "requestId": "token-1", "name": "alpha", "engine": "WDL" }),
        ),
        (
            "replay with different members",
            json!({ "requestId": "token-2", "name": "beta", "engine": "CWL" }),
            json!({ "requestId": "token-2", "name": "beta-renamed", "engine": "NEXTFLOW" }),
        ),
        (
            "replay of a token that is all punctuation",
            json!({ "requestId": "::token-3::", "name": "gamma" }),
            json!({ "requestId": "::token-3::" }),
        ),
    ];

    let mut first_ids = Vec::new();
    for (case, first, _) in &rows {
        let (status, body) = create_workflow(&router, first).await;
        first_ids.push(created_id(case, status, &body));
    }

    for ((case, _, replay), expected_id) in rows.iter().zip(&first_ids) {
        let (status, body) = create_workflow(&router, replay).await;
        let id = created_id(case, status, &body);
        assert_eq!(
            &id, expected_id,
            "case {case:?}: a replayed requestId must return the original workflow id, \
             got {body:?}"
        );
    }

    // And no duplicate was stored: exactly one workflow per distinct token.
    let (status, body) = list_workflows(&router, "").await;
    let listed = items("replay listing", status, &body);
    assert_eq!(
        listed.len(),
        rows.len(),
        "a replayed requestId must not create a second workflow; got {:?}",
        item_ids(&listed)
    );
}

#[tokio::test]
async fn too_many_tags_are_refused() {
    let router = emulator();
    // The bound is MAX_TAGS, documented on the `tags` member ("You can define up to 50
    // tags for the workflow"), so the row at the bound is accepted and the row one past
    // it is refused.
    for (case, count, expect_ok) in [
        ("at the tag bound", MAX_TAGS, true),
        ("one past the tag bound", MAX_TAGS + 1, false),
    ] {
        let (status, body) = create_workflow(
            &router,
            &json!({ "requestId": format!("tags-{count}"), "tags": tag_map(count) }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_workflow_is_not_found() {
    let router = emulator();
    // One real workflow exists, so the 404s below are about the id, not about an empty
    // store.
    let (status, body) = create_workflow(&router, &json!({ "requestId": "only" })).await;
    let existing = created_id("only", status, &body);

    // The rule generalises over the id's width and value: anything the store does not
    // hold is not found, however it is shaped.
    for id in ["0", "1", "999999", "999999999999999999", "1000001"] {
        assert_ne!(id, existing, "the fixture must not collide with the real id");
        let (status, body) = get_workflow(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetWorkflow, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = delete_workflow(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from DeleteWorkflow, got {body:?}"
        );

        let (status, body) = get_run(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetRun, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = delete_run(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from DeleteRun, got {body:?}"
        );
    }

    // A workflow of a type this store does not hold is also not found, even by a real
    // id: every workflow CreateWorkflow mints is PRIVATE.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workflow/{existing}?type=READY2RUN"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a READY2RUN lookup of a PRIVATE workflow must be 404, got {body:?}"
    );
}

#[tokio::test]
async fn a_workflow_settles_to_active_on_the_clock() {
    let router = emulator();
    let (status, body) = create_workflow(&router, &json!({ "requestId": "clock" })).await;
    let id = created_id("clock", status, &body);

    // The transient status is observable: a read taken straight after the create still
    // sees it, so the span is never zero.
    let (_, before) = get_workflow(&router, &id).await;
    assert_eq!(
        before.get("status").and_then(Value::as_str),
        Some("CREATING"),
        "a workflow read before its transition is due must still be CREATING, got {before:?}"
    );

    let after = await_workflow_status(&router, &id, "ACTIVE").await;
    // Reading does not move it again, and does not restamp the creation time.
    let (_, again) = get_workflow(&router, &id).await;
    assert_eq!(
        again.get("status"),
        after.get("status"),
        "a second read must report the same status, got {again:?}"
    );
    assert_eq!(
        again.get("creationTime"),
        after.get("creationTime"),
        "a read must not restamp creationTime, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// StartRun -> GetRun
// ---------------------------------------------------------------------------

/// Create a workflow that must succeed, and return its id.
pub(crate) async fn workflow_for_runs(router: &MultiServiceRouter, request_id: &str) -> String {
    let (status, body) = create_workflow(
        router,
        &json!({
            "requestId": request_id,
            "name": format!("wf-{request_id}"),
            "engine": "WDL",
            "main": "main.wdl",
            "definitionUri": format!("s3://omics-emulator-bucket/{request_id}.zip"),
            "storageCapacity": 1200,
            "storageType": "STATIC",
            "accelerators": "GPU",
        }),
    )
    .await;
    created_id(request_id, status, &body)
}

#[tokio::test]
async fn an_export_request_is_refused_rather_than_answered() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "export").await;
    let body = start_run_body(&workflow_id, "export-run", &json!({}));
    let (status, response) = start_run(&router, &body).await;
    let run_id = created_id("export run", status, &response);

    // An export asks for a presigned URL to an exported object; this emulator serves no
    // object store, so the request is refused with 501 `NotImplemented` instead of a 200
    // that would read as an export which succeeded and produced nothing. The rule
    // generalises over every value the modelled export lists admit and over asking for
    // more than one.
    // Source: `smithy_query(query="shape",shape="WorkflowExport")` -> DEFINITION |
    // README, and `smithy_query(query="shape",shape="RunExport")` -> DEFINITION.
    let rows: Vec<(&str, String)> = vec![
        (
            "workflow export DEFINITION",
            format!("/workflow/{workflow_id}?export=DEFINITION"),
        ),
        (
            "workflow export README",
            format!("/workflow/{workflow_id}?export=README"),
        ),
        (
            "workflow export both",
            format!("/workflow/{workflow_id}?export=DEFINITION&export=README"),
        ),
        (
            "run export DEFINITION",
            format!("/run/{run_id}?export=DEFINITION"),
        ),
    ];
    for (case, uri) in &rows {
        let (status, body) = call(&router, "GET", uri, None).await;
        assert_eq!(
            status, 501,
            "case {case:?}: an export must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body:?}"
        );
    }

    // A read that asks for no export is served in full from state.
    for (case, uri) in [
        ("workflow with no export", format!("/workflow/{workflow_id}")),
        ("run with no export", format!("/run/{run_id}")),
    ] {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read without an export must succeed, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_run_records_the_workflows_canonical_id() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "canonical").await;

    // `WorkflowId` is `^[0-9]+$`, so leading zeros name the same workflow. The rule
    // generalises over how many: whatever the request spelled, the stored run reports
    // the workflow's own canonical decimal, and `DeleteWorkflow` sees the run as
    // blocking under that id.
    let mut run_ids = Vec::new();
    for zeros in ["0", "00", "000000"] {
        let spelled = format!("{zeros}{workflow_id}");
        let body = start_run_body(&spelled, &format!("canonical-{zeros}"), &json!({}));
        let (status, response) = start_run(&router, &body).await;
        let id = created_id(&spelled, status, &response);
        let (_, run) = get_run(&router, &id).await;
        assert_eq!(
            run.get("workflowId").and_then(Value::as_str),
            Some(workflow_id.as_str()),
            "case {spelled:?}: a run must report the workflow's canonical id, got {run:?}"
        );
        run_ids.push(id);
    }

    // The workflow is held by those runs however the delete spells its id. Each attempt
    // asserts the rule on whichever side of the clock it lands: refused while a run is
    // in flight, accepted once none is.
    let mut held = true;
    for zeros in ["0", "00", "000000"] {
        if !held {
            break;
        }
        let spelled = format!("{zeros}{workflow_id}");
        for delete_as in [workflow_id.clone(), spelled.clone()] {
            held = delete_workflow_obeys_the_rule(&router, &delete_as).await;
            if !held {
                break;
            }
        }
    }

    // Only a finished run can be deleted, so wait for the clock to take them there.
    await_runs_finished(&router, &run_ids).await;
    for id in &run_ids {
        let (status, body) = delete_run(&router, id).await;
        assert_eq!(
            status, 202,
            "run {id}: a finished run must be deletable, got {body:?}"
        );
    }

    // With no run left, the workflow is deletable — or already gone, if the clock had
    // finished every run before one of the attempts above landed.
    let (status, body) = delete_workflow(&router, &workflow_id).await;
    assert!(
        status == 202 || status == 404,
        "expected the workflow to be deletable or already gone once no run is live, \
         got {status} {body:?}"
    );
    let (status, body) = get_workflow(&router, &workflow_id).await;
    assert_eq!(status, 404, "the workflow must be gone, got {body:?}");
}

#[tokio::test]
async fn inputs_naming_a_resource_this_emulator_does_not_hold_are_refused() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "unheld").await;

    // Each of these members names a resource this emulator does not hold: a run
    // configuration, a run group or a workflow version (every create for them is still a
    // 501), a run cache no `CreateRunCache` ever minted, or a workflow shared from
    // another account (`CreateShare`/`AcceptShare` are still 501). Accepting the request
    // and storing the reference would report a run bound to something nothing resolved,
    // so the request is refused. The rule generalises over which member names it and
    // over the value.
    let rows: Vec<(&str, Value)> = vec![
        (
            "configurationName",
            json!({ "configurationName": "prod-config" }),
        ),
        (
            "another configurationName",
            json!({ "configurationName": "A1" }),
        ),
        (
            "workflowOwnerId of another account",
            json!({ "workflowOwnerId": "123456789012" }),
        ),
        (
            "workflowOwnerId of yet another account",
            json!({ "workflowOwnerId": "999999999999" }),
        ),
        ("runGroupId", json!({ "runGroupId": "4242424" })),
        ("cacheId", json!({ "cacheId": "7777777" })),
        (
            "workflowVersionName",
            json!({ "workflowVersionName": "v1.2" }),
        ),
    ];
    for (case, extra) in &rows {
        let body = start_run_body(&workflow_id, &format!("unheld-{case}"), extra);
        let (status, response) = start_run(&router, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from StartRun, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // The account this emulator DOES serve is not a foreign owner, so naming it works.
    let body = start_run_body(
        &workflow_id,
        "unheld-own-account",
        &json!({ "workflowOwnerId": ACCOUNT }),
    );
    let (status, response) = start_run(&router, &body).await;
    created_id("own account owner", status, &response);

    // `ListRuns` refuses a filter naming a resource this emulator does not hold the same
    // way, rather than answering an empty page that would read as "it exists and has no
    // runs". The rule generalises over the member and over the value.
    for (member, value) in [
        ("batchId", "1"),
        ("batchId", "1234567"),
        ("batchId", "999999999999999999"),
        ("runGroupId", "1"),
        ("runGroupId", "4242424"),
    ] {
        let (status, body) = list_runs(&router, &format!("?{member}={value}")).await;
        assert_eq!(
            status, 404,
            "{member} {value}: expected 404 from ListRuns, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{member} {value}: expected ResourceNotFoundException, got {body:?}"
        );
    }
    // And a listing that names neither still enumerates the store.
    let (status, body) = list_runs(&router, "").await;
    let page = items("no unheld filter", status, &body);
    assert!(
        !page.is_empty(),
        "a listing without an unheld filter must still enumerate the runs, got {page:?}"
    );

    // `GetWorkflow` narrows on the owner the same way.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workflow/{workflow_id}?workflowOwnerId=123456789012"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a workflow sought under another owner must be 404, got {body:?}"
    );
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workflow/{workflow_id}?workflowOwnerId={ACCOUNT}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a workflow sought under its own account must be 200, got {body:?}"
    );
}

#[tokio::test]
async fn run_parameters_outside_the_workflows_template_are_refused() {
    let router = emulator();
    // The workflow declares exactly two parameters, so its template is the input surface
    // every run must stay inside: "The run cannot include any parameters that are not
    // defined in the parameter template."
    let (status, body) = create_workflow(
        &router,
        &json!({
            "requestId": "templated",
            "name": "templated",
            "engine": "WDL",
            "storageCapacity": 100,
            "parameterTemplate": {
                "reads": { "description": "input reads" },
                "threads": { "description": "worker count", "optional": true },
            },
        }),
    )
    .await;
    let templated = created_id("templated", status, &body);
    // A workflow with no template declares no surface, so it constrains nothing.
    let untemplated = workflow_for_runs(&router, "untemplated").await;

    // (case, workflow, parameters, expected status)
    let rows: Vec<(&str, &String, Value, u16)> = vec![
        ("no parameters at all", &templated, Value::Null, 201),
        ("an empty parameters object", &templated, json!({}), 201),
        (
            "one defined parameter",
            &templated,
            json!({ "reads": ["s3://bucket/a.fastq"] }),
            201,
        ),
        (
            "both defined parameters",
            &templated,
            json!({ "reads": ["s3://bucket/a.fastq"], "threads": 4 }),
            201,
        ),
        (
            "one undefined parameter",
            &templated,
            json!({ "sample": "NA12878" }),
            400,
        ),
        (
            "a defined and an undefined parameter",
            &templated,
            json!({ "reads": ["s3://bucket/a.fastq"], "sample": "NA12878" }),
            400,
        ),
        (
            "several undefined parameters",
            &templated,
            json!({ "sample": "NA12878", "region": "chr1" }),
            400,
        ),
        // The same parameters against a workflow that declared no template.
        (
            "anything against an untemplated workflow",
            &untemplated,
            json!({ "sample": "NA12878", "region": "chr1" }),
            201,
        ),
    ];

    for (case, workflow, parameters, expected) in &rows {
        let mut extra = Map::new();
        if !parameters.is_null() {
            extra.insert("parameters".to_string(), parameters.clone());
        }
        let body = start_run_body(workflow, &format!("param-{case}"), &Value::Object(extra));
        let (status, response) = start_run(&router, &body).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {response:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
        }
    }

    // The rule holds for a DUPLICATE too: the inherited document is checked against the
    // workflow the duplicate actually runs on, not against the source run's workflow.
    let source = start_run_body(
        &untemplated,
        "param-duplicate-source",
        &json!({ "parameters": { "sample": "NA12878" } }),
    );
    let (status, response) = start_run(&router, &source).await;
    let source_id = created_id("duplicate source", status, &response);
    let (status, response) = start_run(
        &router,
        &json!({
            "runId": source_id,
            "workflowId": templated,
            "roleArn": ROLE_ARN,
            "outputUri": OUTPUT_URI,
            "requestId": "param-duplicate",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a duplicate must not inherit parameters the new workflow does not define, \
         got {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response:?}"
    );
}

/// A row of the run start/read-back table.
struct RunCase {
    case: &'static str,
    /// The members added to the three required ones.
    extra: Value,
    /// Every member the read-back must carry, exactly.
    expected: Value,
}

/// The table. The rule is "a GetRun reports what the StartRun stored", so the rows differ
/// along what it generalises over: which optional members are present, both published
/// bounds of the `priority` range, every value of the `logLevel` and `retentionMode`
/// catalogs, every `storageType`, and the shape of the `parameters` document.
fn run_cases() -> Vec<RunCase> {
    let mut cases = vec![
        // Only the required members plus the workflow: the documented defaults must be
        // stored, and the workflow's own storage settings inherited.
        RunCase {
            case: "required-only",
            extra: json!({}),
            expected: json!({
                "workflowType": "PRIVATE",
                "roleArn": ROLE_ARN,
                "outputUri": OUTPUT_URI,
                "storageType": "STATIC",
                "retentionMode": "RETAIN",
                "storageCapacity": 1200,
                "accelerators": "GPU",
            }),
        },
        // The low published bound of `priority`, an empty-object parameters document,
        // one tag and a cache behaviour.
        RunCase {
            case: "priority-low-bound",
            extra: json!({
                "name": "priority-low",
                "priority": CAPACITY_RANGE.0,
                "parameters": {},
                "storageCapacity": 0,
                "storageType": "DYNAMIC",
                "logLevel": "OFF",
                "retentionMode": "REMOVE",
                "cacheBehavior": "CACHE_ON_FAILURE",
                "tags": { "stage": "unit" },
            }),
            expected: json!({
                "name": "priority-low",
                "priority": CAPACITY_RANGE.0,
                "parameters": {},
                "storageCapacity": 0,
                "storageType": "DYNAMIC",
                "logLevel": "OFF",
                "retentionMode": "REMOVE",
                "cacheBehavior": "CACHE_ON_FAILURE",
                "tags": { "stage": "unit" },
            }),
        },
        // The high published bound, and a nested parameters document carrying every JSON
        // kind the modelled `RunParameters` document admits.
        RunCase {
            case: "priority-high-bound",
            extra: json!({
                "name": "priority-high",
                "priority": CAPACITY_RANGE.1,
                "storageCapacity": CAPACITY_RANGE.1,
                "logLevel": "ALL",
                "parameters": {
                    "reads": ["s3://bucket/a.fastq", "s3://bucket/b.fastq"],
                    "threads": 8,
                    "ratio": 0.75,
                    "strict": true,
                    "notes": null,
                    "nested": { "left": 1, "right": { "deep": "yes" } },
                },
            }),
            expected: json!({
                "name": "priority-high",
                "priority": CAPACITY_RANGE.1,
                "storageCapacity": CAPACITY_RANGE.1,
                "logLevel": "ALL",
                "parameters": {
                    "reads": ["s3://bucket/a.fastq", "s3://bucket/b.fastq"],
                    "threads": 8,
                    "ratio": 0.75,
                    "strict": true,
                    "notes": null,
                    "nested": { "left": 1, "right": { "deep": "yes" } },
                },
            }),
        },
    ];
    // One row per remaining (logLevel, retentionMode, storageType) combination, so every
    // published value of all three catalogs is exercised.
    for (index, log_level) in RUN_LOG_LEVELS.iter().enumerate() {
        let retention = RUN_RETENTION_MODES[index % RUN_RETENTION_MODES.len()];
        let storage_type = STORAGE_TYPES[index % STORAGE_TYPES.len()];
        cases.push(RunCase {
            case: "log-level catalog row",
            extra: json!({
                "name": format!("catalog-{log_level}"),
                "logLevel": log_level,
                "retentionMode": retention,
                "storageType": storage_type,
            }),
            expected: json!({
                "name": format!("catalog-{log_level}"),
                "logLevel": log_level,
                "retentionMode": retention,
                "storageType": storage_type,
            }),
        });
    }
    cases
}

/// A `StartRun` body: the three members every row needs, plus the row's own.
pub(crate) fn start_run_body(workflow_id: &str, request_id: &str, extra: &Value) -> Value {
    let mut body = Map::new();
    body.insert("workflowId".to_string(), json!(workflow_id));
    body.insert("roleArn".to_string(), json!(ROLE_ARN));
    body.insert("outputUri".to_string(), json!(OUTPUT_URI));
    body.insert("requestId".to_string(), json!(request_id));
    for (member, value) in extra.as_object().expect("extra members are an object") {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

#[tokio::test]
async fn a_started_run_reads_back_from_state() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "runs-workflow").await;
    let cases = run_cases();

    let mut created = Vec::new();
    let mut ids = BTreeSet::new();
    for (index, run) in cases.iter().enumerate() {
        let body = start_run_body(&workflow_id, &format!("run-req-{index}"), &run.extra);
        let (status, response) = start_run(&router, &body).await;
        let id = created_id(run.case, status, &response);
        assert_numeric_id(run.case, &id);
        let arn = response.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_arn(run.case, arn, "run", &id);
        let uuid = response
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_uuid(run.case, uuid);
        assert_eq!(
            response.get("status").and_then(Value::as_str),
            Some("PENDING"),
            "case {:?}: a just-started run must report the first status, got {response:?}",
            run.case
        );
        assert_eq!(
            response.get("runOutputUri").and_then(Value::as_str),
            Some(format!("{OUTPUT_URI}/{id}").as_str()),
            "case {:?}: runOutputUri must place this run under its own outputUri, \
             got {response:?}",
            run.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {:?}: id {id} was already handed out",
            run.case
        );
        created.push((id, arn.to_string(), uuid.to_string()));
    }

    for (run, (id, arn, uuid)) in cases.iter().zip(&created) {
        let (status, body) = get_run(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRun, got {body:?}",
            run.case
        );
        assert_members(run.case, "GetRun", &run.expected, &body);
        assert_members(
            run.case,
            "GetRun",
            &json!({
                "id": id,
                "arn": arn,
                "uuid": uuid,
                "workflowId": workflow_id,
                "workflowType": "PRIVATE",
                "roleArn": ROLE_ARN,
                "outputUri": OUTPUT_URI,
                "runOutputUri": format!("{OUTPUT_URI}/{id}"),
                "definition": "s3://omics-emulator-bucket/runs-workflow.zip",
            }),
            &body,
        );
        assert_timestamp(run.case, "creationTime", &body);
        // Nothing executed, so nothing claims to have: no engine version, no log
        // location, no resource digests.
        assert_absent(
            run.case,
            &["engineVersion", "logLocation", "resourceDigests"],
            &body,
        );
    }
}

#[tokio::test]
async fn the_default_run_storage_type_applies_when_no_one_names_one() {
    let router = emulator();
    // The workflow names no storageType, so the documented run default applies; a
    // workflow that names one lends it to the run; and the request beats both.
    let (status, body) = create_workflow(
        &router,
        &json!({ "requestId": "no-storage-type", "name": "no-storage-type" }),
    )
    .await;
    let bare_workflow = created_id("no-storage-type", status, &body);
    let typed_workflow = workflow_for_runs(&router, "typed-workflow").await;

    for (case, workflow, requested, expected) in [
        (
            "no storageType anywhere",
            &bare_workflow,
            None,
            // The value `storageType`'s own documentation names as the default.
            "STATIC",
        ),
        (
            "storageType from the workflow",
            &typed_workflow,
            None,
            "STATIC",
        ),
        (
            "storageType from the request",
            &typed_workflow,
            Some("DYNAMIC"),
            "DYNAMIC",
        ),
    ] {
        let mut extra = Map::new();
        // A STATIC run needs a capacity from somewhere, and the bare workflow lends
        // none, so this request carries one; it is not what the row is about.
        extra.insert("storageCapacity".to_string(), json!(500));
        if let Some(requested) = requested {
            extra.insert("storageType".to_string(), json!(requested));
        }
        let body = start_run_body(workflow, &format!("storage-{case}"), &Value::Object(extra));
        let (status, response) = start_run(&router, &body).await;
        let id = created_id(case, status, &response);
        let (_, run) = get_run(&router, &id).await;
        assert_eq!(
            run.get("storageType").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected storageType {expected}, got {run:?}"
        );
    }
}

#[tokio::test]
async fn a_static_run_needs_a_storage_capacity_from_somewhere() {
    let router = emulator();
    // From the `storageCapacity` documentation: "The storage capacity is not required
    // for runs with a DYNAMIC storage type" — so a run whose storage type resolves to
    // STATIC must have a capacity, from the request or from the workflow's default.
    let (status, body) = create_workflow(
        &router,
        &json!({ "requestId": "bare", "name": "bare" }),
    )
    .await;
    let bare = created_id("bare", status, &body);
    let with_capacity = workflow_for_runs(&router, "with-capacity").await;

    // (case, workflow, request members, expected status)
    let rows: Vec<(&str, &String, Value, u16)> = vec![
        ("STATIC with no capacity anywhere", &bare, json!({}), 400),
        (
            "STATIC with a capacity on the request",
            &bare,
            json!({ "storageCapacity": 1 }),
            201,
        ),
        (
            "STATIC with the capacity at the low published bound",
            &bare,
            json!({ "storageCapacity": CAPACITY_RANGE.0 }),
            201,
        ),
        (
            "STATIC with the capacity from the workflow",
            &with_capacity,
            json!({}),
            201,
        ),
        (
            "DYNAMIC needs no capacity",
            &bare,
            json!({ "storageType": "DYNAMIC" }),
            201,
        ),
        (
            "explicit STATIC with no capacity anywhere",
            &bare,
            json!({ "storageType": "STATIC" }),
            400,
        ),
    ];

    for (case, workflow, extra, expected) in &rows {
        let body = start_run_body(workflow, &format!("capacity-{case}"), extra);
        let (status, response) = start_run(&router, &body).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {response:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_duplicated_run_inherits_the_source_runs_defaults() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "duplicate").await;
    let source_body = start_run_body(
        &workflow_id,
        "duplicate-source",
        &json!({
            "name": "source",
            "priority": 42,
            "logLevel": "ERROR",
            "retentionMode": "REMOVE",
            "storageType": "DYNAMIC",
            "storageCapacity": 321,
            "cacheBehavior": "CACHE_ALWAYS",
            "parameters": { "reads": ["s3://bucket/x.fastq"] },
            "tags": { "origin": "source" },
        }),
    );
    let (status, response) = start_run(&router, &source_body).await;
    let source_id = created_id("duplicate source", status, &response);

    // `runId` duplicates "using the run's default values", which the caller may then
    // override. The rule generalises over WHICH members the duplicate overrides, so the
    // table covers inheriting everything, overriding one member, and overriding several.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "inherit everything",
            json!({}),
            json!({
                "workflowId": workflow_id.clone(),
                "name": "source",
                "priority": 42,
                "logLevel": "ERROR",
                "retentionMode": "REMOVE",
                "storageType": "DYNAMIC",
                "storageCapacity": 321,
                "cacheBehavior": "CACHE_ALWAYS",
                "parameters": { "reads": ["s3://bucket/x.fastq"] },
                "tags": { "origin": "source" },
            }),
        ),
        (
            "override one member",
            json!({ "name": "duplicate" }),
            json!({
                "name": "duplicate",
                "priority": 42,
                "logLevel": "ERROR",
                "parameters": { "reads": ["s3://bucket/x.fastq"] },
            }),
        ),
        (
            "override several members",
            json!({
                "priority": 7,
                "logLevel": "ALL",
                "parameters": { "reads": ["s3://bucket/y.fastq"] },
                "tags": { "origin": "duplicate" },
            }),
            json!({
                "name": "source",
                "priority": 7,
                "logLevel": "ALL",
                "parameters": { "reads": ["s3://bucket/y.fastq"] },
                "tags": { "origin": "duplicate" },
                "storageType": "DYNAMIC",
                "storageCapacity": 321,
            }),
        ),
    ];

    for (case, overrides, expected) in &rows {
        let mut body = Map::new();
        body.insert("runId".to_string(), json!(source_id));
        body.insert("roleArn".to_string(), json!(ROLE_ARN));
        body.insert("outputUri".to_string(), json!(OUTPUT_URI));
        body.insert("requestId".to_string(), json!(format!("dup-{case}")));
        for (member, value) in overrides.as_object().expect("an object") {
            body.insert(member.clone(), value.clone());
        }
        let (status, response) = start_run(&router, &Value::Object(body)).await;
        let id = created_id(case, status, &response);
        assert_ne!(
            id, source_id,
            "case {case:?}: a duplicate must be its own run"
        );
        let (_, run) = get_run(&router, &id).await;
        assert_members(case, "GetRun", expected, &run);
        assert_eq!(
            run.get("runId").and_then(Value::as_str),
            Some(source_id.as_str()),
            "case {case:?}: a duplicate must report the run it copied, got {run:?}"
        );
    }
}

#[tokio::test]
async fn starting_a_run_on_an_unknown_workflow_is_not_found() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "exists").await;

    // The rule generalises over what is missing: the workflow id, the type it is sought
    // under, or the run a duplicate was asked to copy.
    let rows: Vec<(&str, Value)> = vec![
        ("unknown workflowId", json!({ "workflowId": "999999" })),
        (
            "unknown wide workflowId",
            json!({ "workflowId": "999999999999999999" }),
        ),
        (
            "real workflowId sought as READY2RUN",
            json!({ "workflowId": workflow_id.clone(), "workflowType": "READY2RUN" }),
        ),
        ("unknown runId to duplicate", json!({ "runId": "888888" })),
    ];

    for (case, extra) in &rows {
        let mut body = Map::new();
        body.insert("roleArn".to_string(), json!(ROLE_ARN));
        body.insert("outputUri".to_string(), json!(OUTPUT_URI));
        body.insert("requestId".to_string(), json!(format!("missing-{case}")));
        for (member, value) in extra.as_object().expect("an object") {
            body.insert(member.clone(), value.clone());
        }
        let (status, response) = start_run(&router, &Value::Object(body)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from StartRun, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // A request that names neither a workflow nor a run to duplicate is a bad request,
    // not a missing resource.
    let (status, response) = start_run(
        &router,
        &json!({
            "roleArn": ROLE_ARN,
            "outputUri": OUTPUT_URI,
            "requestId": "no-target",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a StartRun naming no workflow must be 400, got {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response:?}"
    );
}

#[tokio::test]
async fn a_replayed_request_id_returns_the_same_run() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "idempotent-runs").await;

    for (case, replay_extra) in [
        ("identical replay", json!({ "name": "same" })),
        ("replay with different members", json!({ "name": "other" })),
    ] {
        let first = start_run_body(
            &workflow_id,
            &format!("run-token-{case}"),
            &json!({ "name": "same" }),
        );
        let (status, body) = start_run(&router, &first).await;
        let expected_id = created_id(case, status, &body);

        let replay = start_run_body(&workflow_id, &format!("run-token-{case}"), &replay_extra);
        let (status, body) = start_run(&router, &replay).await;
        let id = created_id(case, status, &body);
        assert_eq!(
            id, expected_id,
            "case {case:?}: a replayed requestId must return the original run id, got {body:?}"
        );

        // The stored run is still the one the first call described.
        let (_, run) = get_run(&router, &id).await;
        assert_eq!(
            run.get("name").and_then(Value::as_str),
            Some("same"),
            "case {case:?}: a replay must not rewrite the stored run, got {run:?}"
        );
    }
}

#[tokio::test]
async fn a_run_advances_through_its_statuses_on_the_clock() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "clock-runs").await;
    let body = start_run_body(&workflow_id, "clock-run", &json!({ "name": "clock" }));
    let (status, response) = start_run(&router, &body).await;
    let id = created_id("clock run", status, &response);

    // The transient state is observable: a read taken straight after the start still
    // sees the run in flight, so the span the clock owes is never zero. The assertion is
    // on the CLASS of status rather than on `PENDING` exactly, so a stall between the
    // start and this read cannot turn "not finished yet" into a timing failure.
    let (_, pending) = get_run(&router, &id).await;
    let in_flight = pending.get("status").and_then(Value::as_str).unwrap_or_default();
    assert!(
        IN_FLIGHT_RUN_STATUSES.contains(&in_flight),
        "a run read straight after StartRun must still be in flight (one of \
         {IN_FLIGHT_RUN_STATUSES:?}), got {pending:?}"
    );
    assert_absent(
        "clock run",
        &["stopTime", "failureReason", "statusMessage"],
        &pending,
    );

    // Then read until the clock has taken it through every step the sequence owes.
    let done = await_run_status(&router, &id, "FAILED").await;
    // The literal the emulator is REQUIRED to report, not whatever the implementation
    // happens to carry: no workflow engine is emulated, so no run can have succeeded,
    // and the modelled `RunStatus` value a run that never executed ends in is FAILED —
    // with the reason and the message saying exactly why.
    assert_members(
        "clock run",
        "GetRun",
        &json!({
            "status": "FAILED",
            // The literal text, not the handler's constant: a run that never executed
            // must SAY so, and the wording is checked here rather than echoed.
            "failureReason": "No workflow engine is emulated, so the run never executed.",
            "statusMessage": "This emulator records runs but executes no WDL, Nextflow or CWL \
                              workflow engine, so the run ended without running and wrote no \
                              outputs.",
        }),
        &done,
    );
    let reason = done
        .get("failureReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !reason.is_empty() && reason.len() <= 64,
        "failureReason must fit the modelled RunFailureReason length 1..64, got {} \
         characters: {reason:?}",
        reason.len()
    );
    let start_time = assert_timestamp("clock run", "startTime", &done);
    let stop_time = assert_timestamp("clock run", "stopTime", &done);
    assert!(
        start_time <= stop_time,
        "expected startTime {start_time} at or before stopTime {stop_time}"
    );

    // The timestamps are the instants the steps were DUE, so polling again later
    // reports exactly the same values.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (_, again) = get_run(&router, &id).await;
    assert_members(
        "clock run",
        "GetRun",
        &json!({
            "status": "FAILED",
            "startTime": done.get("startTime").cloned().unwrap_or(Value::Null),
            "stopTime": done.get("stopTime").cloned().unwrap_or(Value::Null),
        }),
        &again,
    );

    // Nothing ran, so nothing claims to have: no engine version, no log streams, no
    // image digests, even once the run has reached its terminal status.
    assert_absent(
        "clock run",
        &["engineVersion", "logLocation", "resourceDigests"],
        &again,
    );
}

// ---------------------------------------------------------------------------
// ListRuns / ListWorkflows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_runs_pages_over_the_persisted_runs() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "paging").await;

    let mut expected_ids = Vec::new();
    for n in 0..5u32 {
        let body = start_run_body(
            &workflow_id,
            &format!("page-{n}"),
            &json!({ "name": format!("run-{n}") }),
        );
        let (status, response) = start_run(&router, &body).await;
        expected_ids.push(created_id("paging", status, &response));
    }

    // The rule generalises over the page size: the pages concatenate to the whole list
    // in creation order, whatever size is asked for, and only a page with more behind it
    // carries a nextToken.
    for page_size in [1usize, 2, 3, 5, 100] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&startingToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = list_runs(&router, &query).await;
            let page = items(&format!("page size {page_size}"), status, &body);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page carried {} items",
                page.len()
            );
            seen.extend(item_ids(&page));
            pages += 1;
            assert!(
                pages <= expected_ids.len() + 1,
                "page size {page_size}: paging did not terminate"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected_ids,
            "page size {page_size}: expected every run in creation order"
        );
    }

    // The last page of an exactly-filled list carries no token.
    let (status, body) = list_runs(&router, "?maxResults=5").await;
    let page = items("exact fit", status, &body);
    assert_eq!(page.len(), 5, "expected all five runs, got {page:?}");
    assert!(
        body.get("nextToken").is_none(),
        "a page with nothing behind it must carry no nextToken, got {body:?}"
    );

    // A token this operation never issued is a bad request, not a page.
    let (status, body) = list_runs(&router, "?startingToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "an unissued startingToken must be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body:?}"
    );
}

#[tokio::test]
async fn a_page_token_is_only_valid_for_the_listing_that_issued_it() {
    let router = emulator();
    // Two workflows and two runs, so each listing has a further page and therefore
    // mints a token.
    let workflow_id = workflow_for_runs(&router, "token-a").await;
    let _second_workflow = workflow_for_runs(&router, "token-b").await;
    for n in 0..2u32 {
        let body = start_run_body(&workflow_id, &format!("token-run-{n}"), &json!({}));
        let (status, response) = start_run(&router, &body).await;
        created_id("token run", status, &response);
    }

    let (status, body) = list_runs(&router, "?maxResults=1").await;
    items("run page", status, &body);
    let run_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ListRuns nextToken, got {body:?}"))
        .to_string();
    let (status, body) = list_workflows(&router, "?maxResults=1").await;
    items("workflow page", status, &body);
    let workflow_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ListWorkflows nextToken, got {body:?}"))
        .to_string();

    // Each token resumes its OWN listing.
    let (status, body) = list_runs(&router, &format!("?startingToken={run_token}")).await;
    assert_eq!(status, 200, "a ListRuns token must resume ListRuns, got {body:?}");
    let (status, body) =
        list_workflows(&router, &format!("?startingToken={workflow_token}")).await;
    assert_eq!(
        status, 200,
        "a ListWorkflows token must resume ListWorkflows, got {body:?}"
    );

    // And is refused by the other, rather than silently resuming the wrong collection.
    for (case, query, call_runs) in [
        (
            "workflow token given to ListRuns",
            format!("?startingToken={workflow_token}"),
            true,
        ),
        (
            "run token given to ListWorkflows",
            format!("?startingToken={run_token}"),
            false,
        ),
    ] {
        let (status, body) = if call_runs {
            list_runs(&router, &query).await
        } else {
            list_workflows(&router, &query).await
        };
        assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn list_runs_honours_its_filters() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "filters").await;

    // Three runs differing in the members the filters key on.
    let mut ids = Vec::new();
    for name in ["alpha", "beta", "alpha"] {
        let body = start_run_body(
            &workflow_id,
            &format!("filter-{}", ids.len()),
            &json!({ "name": name }),
        );
        let (status, response) = start_run(&router, &body).await;
        ids.push(created_id("filters", status, &response));
    }

    // The rule generalises over which filter is applied and over whether it matches
    // nothing, some rows or all of them.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter", String::new(), ids.clone()),
        (
            "name matching two",
            "?name=alpha".to_string(),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "name matching one",
            "?name=beta".to_string(),
            vec![ids[1].clone()],
        ),
        ("name matching none", "?name=gamma".to_string(), vec![]),
        (
            // No run ever reaches CANCELLED here, so this row is a filter that matches
            // nothing whatever the clock has done.
            "status matching none",
            "?status=CANCELLED".to_string(),
            vec![],
        ),
        // No run this slice stores belongs to a run group, and the group itself does not
        // exist, so the filter is refused rather than answered with an empty page (see
        // the unheld-reference test).
    ];

    for (case, query, expected) in &rows {
        let (status, body) = list_runs(&router, query).await;
        let page = items(case, status, &body);
        assert_eq!(
            &item_ids(&page),
            expected,
            "case {case:?}: unexpected items for query {query:?}"
        );
    }

    // A status filter that DOES match, asserted once the clock has taken every run to
    // its terminal status, so the expected set does not depend on when the call landed.
    await_runs_finished(&router, &ids).await;
    for (case, query, expected) in [
        ("status matching every run", "?status=FAILED", ids.clone()),
        ("status no run is in any more", "?status=PENDING", vec![]),
        (
            "status and name together",
            "?status=FAILED&name=beta",
            vec![ids[1].clone()],
        ),
    ] {
        let (status, body) = list_runs(&router, query).await;
        let page = items(case, status, &body);
        assert_eq!(
            item_ids(&page),
            expected,
            "case {case:?}: unexpected items for query {query:?}"
        );
    }
}

#[tokio::test]
async fn list_workflows_pages_and_filters_over_the_persisted_workflows() {
    let router = emulator();
    let mut ids = Vec::new();
    for (n, name) in ["one", "two", "three", "two"].iter().enumerate() {
        let (status, body) = create_workflow(
            &router,
            &json!({ "requestId": format!("list-{n}"), "name": name, "engine": "WDL" }),
        )
        .await;
        ids.push(created_id("list workflows", status, &body));
    }

    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter", String::new(), ids.clone()),
        (
            "name matching two",
            "?name=two".to_string(),
            vec![ids[1].clone(), ids[3].clone()],
        ),
        ("name matching none", "?name=four".to_string(), vec![]),
        ("type PRIVATE", "?type=PRIVATE".to_string(), ids.clone()),
        ("type READY2RUN", "?type=READY2RUN".to_string(), vec![]),
        (
            "first page of one",
            "?maxResults=1".to_string(),
            vec![ids[0].clone()],
        ),
        (
            "first page of three",
            "?maxResults=3".to_string(),
            ids[..3].to_vec(),
        ),
    ];

    for (case, query, expected) in &rows {
        let (status, body) = list_workflows(&router, query).await;
        let page = items(case, status, &body);
        assert_eq!(
            &item_ids(&page),
            expected,
            "case {case:?}: unexpected items for query {query:?}"
        );
    }

    // Paging a filtered list still walks the whole match set.
    let (status, body) = list_workflows(&router, "?name=two&maxResults=1").await;
    let page = items("filtered page", status, &body);
    assert_eq!(item_ids(&page), vec![ids[1].clone()]);
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a nextToken, got {body:?}"))
        .to_string();
    let (status, body) =
        list_workflows(&router, &format!("?name=two&maxResults=1&startingToken={token}")).await;
    let page = items("filtered second page", status, &body);
    assert_eq!(item_ids(&page), vec![ids[3].clone()]);
}

// ---------------------------------------------------------------------------
// DeleteRun / DeleteWorkflow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_run_removes_it_from_every_reader() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "deletes").await;

    let mut ids = Vec::new();
    for n in 0..3u32 {
        let body = start_run_body(
            &workflow_id,
            &format!("delete-{n}"),
            &json!({ "name": format!("run-{n}") }),
        );
        let (status, response) = start_run(&router, &body).await;
        ids.push(created_id("deletes", status, &response));
    }

    // Only a finished run can be deleted: "You can only delete a run that has reached a
    // COMPLETED, FAILED, or CANCELLED stage". Each attempt asserts the rule on whichever
    // side of the clock it lands, and the runs it refused are the ones that must still be
    // in the store.
    let mut ids: Vec<String> = {
        let mut surviving = Vec::new();
        for id in &ids {
            if delete_run_obeys_the_rule(&router, id).await {
                surviving.push(id.clone());
            }
        }
        surviving
    };
    let (status, body) = list_runs(&router, "").await;
    let page = items("after refused deletes", status, &body);
    assert_eq!(
        item_ids(&page),
        ids,
        "a refused delete must leave every run it refused in the store"
    );

    await_runs_finished(&router, &ids).await;

    // The rule generalises over which run is deleted: the middle one first, then what is
    // left, and the rest keep their order and their values.
    while !ids.is_empty() {
        let id = ids.remove(ids.len() / 2);
        let (status, body) = delete_run(&router, &id).await;
        assert_eq!(
            status, 202,
            "deleting run {id}: expected 202, got {body:?}"
        );

        let (status, body) = get_run(&router, &id).await;
        assert_eq!(
            status, 404,
            "deleting run {id}: a later GetRun must be 404, got {body:?}"
        );
        assert_eq!(error_code(&body), "ResourceNotFoundException");

        let (status, body) = delete_run(&router, &id).await;
        assert_eq!(
            status, 404,
            "deleting run {id} twice: the second must be 404, got {body:?}"
        );

        let (status, body) = list_runs(&router, "").await;
        let page = items("after delete", status, &body);
        assert_eq!(
            item_ids(&page),
            ids,
            "deleting run {id}: ListRuns must show exactly the remaining runs"
        );
    }
}

#[tokio::test]
async fn a_workflow_with_non_terminal_runs_cannot_be_deleted() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "conflict").await;
    let body = start_run_body(&workflow_id, "conflict-run", &json!({ "name": "busy" }));
    let (status, response) = start_run(&router, &body).await;
    let run_id = created_id("conflict run", status, &response);

    // While the run has not reached a terminal status, the workflow is in use. The
    // attempt asserts the rule on whichever side of the clock it lands.
    if delete_workflow_obeys_the_rule(&router, &workflow_id).await {
        // It was refused, so once the clock finishes the run the workflow is free.
        await_run_status(&router, &run_id, "FAILED").await;
        let (status, body) = delete_workflow(&router, &workflow_id).await;
        assert_eq!(
            status, 202,
            "a workflow whose runs have finished must be deletable, got {body:?}"
        );
    }

    // The workflow is gone from every reader; the run record it ran is untouched.
    let (status, body) = get_workflow(&router, &workflow_id).await;
    assert_eq!(status, 404, "a deleted workflow must be 404, got {body:?}");
    assert_eq!(error_code(&body), "ResourceNotFoundException");
    let (status, body) = list_workflows(&router, "").await;
    let page = items("after workflow delete", status, &body);
    assert!(
        page.is_empty(),
        "a deleted workflow must not be listed, got {page:?}"
    );
    let (status, run) = get_run(&router, &run_id).await;
    assert_eq!(status, 200, "the run record outlives its workflow");
    assert_eq!(
        run.get("workflowId").and_then(Value::as_str),
        Some(workflow_id.as_str()),
        "the run still reports the workflow it ran, got {run:?}"
    );

    // Deleting a run frees a workflow just as finishing it does — and the run itself can
    // only be deleted once it has finished.
    let other = workflow_for_runs(&router, "conflict-two").await;
    let body = start_run_body(&other, "conflict-run-two", &json!({}));
    let (status, response) = start_run(&router, &body).await;
    let other_run = created_id("conflict run two", status, &response);
    let workflow_held = delete_workflow_obeys_the_rule(&router, &other).await;
    if delete_run_obeys_the_rule(&router, &other_run).await {
        await_run_status(&router, &other_run, "FAILED").await;
        let (status, body) = delete_run(&router, &other_run).await;
        assert_eq!(status, 202, "expected 202 once the run finished, got {body:?}");
    }
    if workflow_held {
        let (status, body) = delete_workflow(&router, &other).await;
        assert_eq!(
            status, 202,
            "with its only run deleted the workflow must be deletable, got {body:?}"
        );
    }
    let (status, body) = get_workflow(&router, &other).await;
    assert_eq!(status, 404, "the workflow must be gone, got {body:?}");
}

// ---------------------------------------------------------------------------
// Every call leaves a structured, attributable access record
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than pulling in a formatting
/// subscriber crate: the test needs one line per event with its target and fields, which
/// is a dozen lines of `Visit`, and a new dev-dependency would put an entry in the
/// workspace lockfile this change deliberately leaves untouched.
#[derive(Clone, Default)]
struct CapturedRecords(Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The access key id the audit case signs with, so it can pick its own records out of a
/// process-wide log that sibling tests also write to.
const AUDIT_AKID: &str = "AKIDAUDITCASE";

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in parallel
/// on other threads — so a thread-local subscriber is raced away as soon as another
/// thread re-evaluates a handler's callsite with no subscriber of its own. A global
/// default is always interested, which makes the capture deterministic; [`AUDIT_AKID`] is
/// what keeps a sibling test's records out of this one's assertions.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// Values a row submitted that must NEVER reach the log: the record is metadata about
/// the call, not a copy of what the caller sent.
const WITHHELD_FROM_AUDIT: &[&str] = &[
    "audit-create",
    "audit-start",
    "audit-missing",
    "audit-tags",
    "audit-version",
    ROLE_ARN,
    OUTPUT_URI,
    "audited-tag-value",
];

/// One row of the audit table: a call to make, and the record it must leave.
struct AuditCase {
    /// How the row is reported in a failure.
    case: &'static str,
    /// The operation the record must name.
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The `outcome` token the record must carry.
    outcome: &'static str,
    resource: ExpectedResource,
}

/// What the `resource` field of a row's record must name.
enum ExpectedResource {
    /// The id the call's own response reported (a create or a start).
    FromResponse,
    /// A resource the row named in its request.
    Named(String),
    /// No single resource: a listing, or a refusal made before any lookup.
    Nothing,
}

/// Assert the one record a row produced carries the metadata the audit rule requires and
/// none of the caller's own data.
fn assert_audit_record(
    case: &str,
    record: &str,
    operation: &str,
    request_target: &str,
    outcome: &str,
    resource: &str,
) {
    for (field, value) in [
        // The sigv4 signing name of this service, from its Smithy model.
        ("service", "omics"),
        ("operation", operation),
        // The identity the request PRESENTED, which is what the record attributes the
        // call to.
        ("access_key_id", AUDIT_AKID),
        ("request_target", request_target),
        ("resource", resource),
        ("outcome", outcome),
    ] {
        // Asserted as the `field=value` pair, not as a bare substring, so a value cannot
        // satisfy the check from the wrong field.
        let pair = format!("{field}={value}");
        assert!(
            record.contains(&pair),
            "case {case:?}: the audit record must carry {pair:?}, got {record:?}"
        );
    }
    for withheld in WITHHELD_FROM_AUDIT {
        assert!(
            !record.contains(withheld),
            "case {case:?}: the audit record must stay metadata-only, but it carries \
             {withheld:?}: {record:?}"
        );
    }
}

/// The captured records that are THIS case's: on the `emulator::audit` target and
/// presenting [`AUDIT_AKID`]. A sibling test running in parallel writes to the same
/// global capture, so selecting by the presented identity is what keeps the assertions
/// about this case's own calls.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ")
                && record.contains(&format!("access_key_id={AUDIT_AKID}"))
        })
        .cloned()
        .collect()
}

/// The single record one call emitted, under [`AUDIT_AKID`].
async fn audited_call(
    captured: &CapturedRecords,
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value, String) {
    let _ = captured.drain();
    let (status, response) = call_as(router, method, uri, body, AUDIT_AKID).await;
    let records = audit_records(&captured.drain());
    // One call, one record: a second would mean a branch audited twice.
    assert_eq!(
        records.len(),
        1,
        "case {case:?} (HTTP {status}, body {response:?}): expected exactly one audit \
         record, got {records:?}"
    );
    (status, response, records[0].clone())
}

/// Every call in this slice — above all the four that CHANGE the store — emits one
/// structured record on the `emulator::audit` target naming the operation, the identity
/// the request presented, the request target, the resource it resolved and the outcome.
///
/// A TABLE over the whole lifecycle, so each distinct outcome the slice can report is a
/// row: the creates, the idempotent replay, the reads, the listings, the refusals,
/// the "not found"s and the two deletes. Driven through the production router with the
/// capturing subscriber installed, so what is asserted is the record a real run emits.
#[tokio::test]
async fn every_call_emits_a_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    // The fixtures the read rows act on are created under the DEFAULT identity, so their
    // own records are not this case's.
    let workflow = workflow_for_runs(&router, "audit-fixture-workflow").await;
    let fixture_run_body = start_run_body(&workflow, "audit-fixture-run", &json!({}));
    let (status, response) = start_run(&router, &fixture_run_body).await;
    let run = created_id("the audit fixture run", status, &response);
    let (status, response) = call(
        &router,
        "POST",
        "/runCache",
        Some(&json!({
            "requestId": "audit-fixture-cache",
            "cacheS3Location": "s3://omics-emulator-cache/audit/",
        })),
    )
    .await;
    let cache = created_id("the audit fixture run cache", status, &response);

    let over_the_tag_bound = tag_map(MAX_TAGS + 1);

    // An id no store holds, spelled wider than anything this emulator mints.
    let absent = "999999999999999999";

    let cases: Vec<AuditCase> = vec![
        AuditCase {
            case: "a workflow created",
            operation: "CreateWorkflow",
            method: "POST",
            uri: "/workflow".to_string(),
            body: Some(json!({ "requestId": "audit-create", "name": "audited" })),
            outcome: "workflow-created",
            resource: ExpectedResource::FromResponse,
        },
        AuditCase {
            case: "the same create replayed",
            operation: "CreateWorkflow",
            method: "POST",
            uri: "/workflow".to_string(),
            body: Some(json!({ "requestId": "audit-create", "name": "audited" })),
            outcome: "no-change",
            resource: ExpectedResource::FromResponse,
        },
        AuditCase {
            case: "a create past the tag bound",
            operation: "CreateWorkflow",
            method: "POST",
            uri: "/workflow".to_string(),
            body: Some(json!({
                "requestId": "audit-tags",
                "tags": over_the_tag_bound,
            })),
            outcome: "validation-refused",
            resource: ExpectedResource::Nothing,
        },
        AuditCase {
            case: "a workflow read",
            operation: "GetWorkflow",
            method: "GET",
            uri: format!("/workflow/{workflow}"),
            body: None,
            outcome: "reported",
            resource: ExpectedResource::Named(workflow.clone()),
        },
        AuditCase {
            case: "a workflow id nothing holds",
            operation: "GetWorkflow",
            method: "GET",
            uri: format!("/workflow/{absent}"),
            body: None,
            outcome: "not-found",
            resource: ExpectedResource::Named(absent.to_string()),
        },
        AuditCase {
            case: "a workflow listing",
            operation: "ListWorkflows",
            method: "GET",
            uri: "/workflow".to_string(),
            body: None,
            outcome: "reported",
            resource: ExpectedResource::Nothing,
        },
        AuditCase {
            case: "a listing resumed from a token nothing issued",
            operation: "ListWorkflows",
            method: "GET",
            uri: "/workflow?startingToken=not-a-token".to_string(),
            body: None,
            outcome: "validation-refused",
            resource: ExpectedResource::Nothing,
        },
        AuditCase {
            case: "a run started",
            operation: "StartRun",
            method: "POST",
            uri: "/run".to_string(),
            body: Some(start_run_body(&workflow, "audit-start", &json!({}))),
            outcome: "run-started",
            resource: ExpectedResource::FromResponse,
        },
        AuditCase {
            case: "a run started on a workflow nothing holds",
            operation: "StartRun",
            method: "POST",
            uri: "/run".to_string(),
            body: Some(start_run_body(absent, "audit-missing", &json!({}))),
            outcome: "not-found",
            resource: ExpectedResource::Named(absent.to_string()),
        },
        AuditCase {
            case: "a run read",
            operation: "GetRun",
            method: "GET",
            uri: format!("/run/{run}"),
            body: None,
            outcome: "reported",
            resource: ExpectedResource::Named(run.clone()),
        },
        AuditCase {
            case: "a run id nothing holds",
            operation: "GetRun",
            method: "GET",
            uri: format!("/run/{absent}"),
            body: None,
            outcome: "not-found",
            resource: ExpectedResource::Named(absent.to_string()),
        },
        AuditCase {
            case: "a run listing",
            operation: "ListRuns",
            method: "GET",
            uri: "/run".to_string(),
            body: None,
            outcome: "reported",
            resource: ExpectedResource::Nothing,
        },
        AuditCase {
            case: "a run cache created",
            operation: "CreateRunCache",
            method: "POST",
            uri: "/runCache".to_string(),
            body: Some(json!({
                "requestId": "audit-cache",
                "cacheS3Location": "s3://omics-emulator-cache/audited/",
            })),
            outcome: "run-cache-created",
            resource: ExpectedResource::FromResponse,
        },
        AuditCase {
            case: "the same run cache create replayed",
            operation: "CreateRunCache",
            method: "POST",
            uri: "/runCache".to_string(),
            body: Some(json!({
                "requestId": "audit-cache",
                "cacheS3Location": "s3://omics-emulator-cache/audited/",
            })),
            outcome: "no-change",
            resource: ExpectedResource::FromResponse,
        },
        AuditCase {
            case: "a run cache read",
            operation: "GetRunCache",
            method: "GET",
            uri: format!("/runCache/{cache}"),
            body: None,
            outcome: "reported",
            resource: ExpectedResource::Named(cache.clone()),
        },
        AuditCase {
            case: "a run cache id nothing holds",
            operation: "GetRunCache",
            method: "GET",
            uri: format!("/runCache/{absent}"),
            body: None,
            outcome: "not-found",
            resource: ExpectedResource::Named(absent.to_string()),
        },
    ];

    let mut started: Vec<String> = vec![run.clone()];
    for row in cases {
        let AuditCase {
            case,
            operation,
            method,
            uri,
            body,
            outcome,
            resource: expected_resource,
        } = row;
        let (status, response, record) =
            audited_call(captured, &router, case, method, &uri, body.as_ref()).await;
        let resource = match &expected_resource {
            ExpectedResource::FromResponse => {
                let id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected an id in {response:?} (HTTP {status})")
                    })
                    .to_string();
                if operation == "StartRun" {
                    started.push(id.clone());
                }
                id
            }
            ExpectedResource::Named(id) => id.clone(),
            // `crate::state::AUDIT_NO_RESOURCE`.
            ExpectedResource::Nothing => "-".to_string(),
        };
        let target = format!("{method} {}", uri.split('?').next().unwrap_or(&uri));
        assert_audit_record(case, &record, operation, &target, outcome, &resource);
    }

    // The two version operations change the store too, so each leaves its own record.
    // They act on a workflow of their own, so the delete rows below are unaffected by the
    // version this creates.
    let versioned = workflow_for_runs(&router, "audit-versioned-workflow").await;
    let version_arn =
        format!("arn:aws:omics:{REGION}:{ACCOUNT}:workflow/{versioned}/version/av1");
    let case = "a workflow version created";
    let (status, body, record) = audited_call(
        captured,
        &router,
        case,
        "POST",
        &format!("/workflow/{versioned}/version"),
        Some(&json!({ "versionName": "av1", "requestId": "audit-version" })),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
    assert_audit_record(
        case,
        &record,
        "CreateWorkflowVersion",
        &format!("POST /workflow/{versioned}/version"),
        "workflow-version-created",
        &version_arn,
    );

    let case = "a workflow version deleted";
    let (status, body, record) = audited_call(
        captured,
        &router,
        case,
        "DELETE",
        &format!("/workflow/{versioned}/version/av1"),
        None,
    )
    .await;
    assert_eq!(status, 202, "case {case:?}: expected 202, got {body:?}");
    assert_audit_record(
        case,
        &record,
        "DeleteWorkflowVersion",
        &format!("DELETE /workflow/{versioned}/version/av1"),
        "workflow-version-deleted",
        &version_arn,
    );

    // The two deletes. Which outcome each is owed depends on which side of the clock the
    // call lands on — a run that has not finished cannot be deleted and its workflow is
    // still in use — so the record is asserted against the answer the call actually gave,
    // exactly as the delete rules themselves are asserted elsewhere in this file.
    let (status, _, record) = audited_call(
        captured,
        &router,
        "a workflow deleted while a run may still be in flight",
        "DELETE",
        &format!("/workflow/{workflow}"),
        None,
    )
    .await;
    let outcome = match status {
        409 => "conflict",
        202 => "workflow-deleted",
        other => panic!("DeleteWorkflow answered {other}, neither 202 nor 409"),
    };
    assert_audit_record(
        "a workflow deleted while a run may still be in flight",
        &record,
        "DeleteWorkflow",
        &format!("DELETE /workflow/{workflow}"),
        outcome,
        &workflow,
    );
    let workflow_still_held = status == 409;

    // Once the clock has finished every run, the delete rules admit both deletes, so each
    // record's outcome is the one a successful delete is owed.
    await_runs_finished(&router, &started).await;
    for id in &started {
        let case = "a finished run deleted";
        let (status, body, record) =
            audited_call(captured, &router, case, "DELETE", &format!("/run/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a finished run must be deletable, got {body:?}"
        );
        assert_audit_record(
            case,
            &record,
            "DeleteRun",
            &format!("DELETE /run/{id}"),
            "run-deleted",
            id,
        );
    }
    if workflow_still_held {
        let case = "the workflow deleted once its runs finished";
        let (status, body, record) = audited_call(
            captured,
            &router,
            case,
            "DELETE",
            &format!("/workflow/{workflow}"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 once every run finished, got {body:?}"
        );
        assert_audit_record(
            case,
            &record,
            "DeleteWorkflow",
            &format!("DELETE /workflow/{workflow}"),
            "workflow-deleted",
            &workflow,
        );
    }
}
