// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `farm -> queue -> job` slice of AWS Deadline Cloud.
//!
//! Every test drives the service the way the router does in production: a buffered
//! `http::Request` through `MultiServiceRouter::dispatch`, so routing, deserialization,
//! constraint validation, the handler and serialization all run. Each test builds its own
//! emulator, and all the calls of one test share that emulator's single engine, which is
//! what makes "create then read it back over a separate request" a real assertion.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A minimal OpenJD-shaped job template written in JSON, carrying `name` at the top level.
fn json_template(name: &str) -> String {
    json!({
        "specificationVersion": "jobtemplate-2023-09",
        "name": name,
        "steps": [],
    })
    .to_string()
}

/// The same template written in YAML.
fn yaml_template(name: &str) -> String {
    format!("specificationVersion: jobtemplate-2023-09\nname: {name}\nsteps: []\n")
}

/// One emulator: the shared engine plus the deadline service mounted on a router.
///
/// `pub(crate)` so the other suites in this test target drive the service the same way
/// instead of each building a second copy of this harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests sign nothing, so they assert behaviour, not signatures.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_deadline_deadline::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request and return its status and parsed JSON body (`Value::Null` for
    /// an empty body).
    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://deadline.us-east-1.amazonaws.com{path}"))
            // The routing key every front door reads: the 4th field of the SigV4
            // credential scope is this service's signing name.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20231012/us-east-1/deadline/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "deadline.us-east-1.amazonaws.com");
        if let Some(token) = client_token {
            builder = builder.header("x-amz-client-token", token);
        }
        let payload = match &body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Bytes::from(serde_json::to_vec(value).expect("serializable body"))
            }
            None => Bytes::new(),
        };
        let request = builder.body(payload).expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn get(&self, path: &str) -> (u16, Value) {
        self.call("GET", path, None, None).await
    }

    pub(crate) async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.call("POST", path, Some(body), None).await
    }

    async fn delete(&self, path: &str) -> (u16, Value) {
        self.call("DELETE", path, None, None).await
    }

    /// Create a farm and return its id, failing the test if the create did not succeed.
    pub(crate) async fn farm(&self, display_name: &str) -> String {
        let (status, body) = self
            .post("/2023-10-12/farms", json!({"displayName": display_name}))
            .await;
        assert_eq!(
            status, 200,
            "CreateFarm {display_name}: expected 200, got {status} with {body}"
        );
        id_of(&body, "farmId")
    }

    /// Create a queue under `farm_id` and return its id.
    pub(crate) async fn queue(&self, farm_id: &str, display_name: &str) -> String {
        let (status, body) = self
            .post(
                &format!("/2023-10-12/farms/{farm_id}/queues"),
                json!({"displayName": display_name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateQueue {display_name}: expected 200, got {status} with {body}"
        );
        id_of(&body, "queueId")
    }

    /// Create a job under `farm_id`/`queue_id` from a JSON template and return its id.
    pub(crate) async fn job(
        &self,
        farm_id: &str,
        queue_id: &str,
        name: &str,
        priority: i64,
    ) -> String {
        let (status, body) = self
            .post(
                &format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"),
                json!({
                    "template": json_template(name),
                    "templateType": "JSON",
                    "priority": priority,
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateJob {name}: expected 201, got {status} with {body}"
        );
        id_of(&body, "jobId")
    }
}

pub(crate) fn id_of(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected member {member} in {body}"))
        .to_string()
}

pub(crate) fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected string member {member} in {body}"))
        .to_string()
}

/// The error code a modeled failure carries on restJson1: the body's `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Does `id` satisfy the modeled label pattern `^<prefix>-[0-9a-f]{32}$`?
///
/// `pub(crate)` so every suite in this test target checks a minted id the same way.
pub(crate) fn is_resource_id(id: &str, prefix: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{prefix}-")) else {
        return false;
    };
    suffix.len() == 32 && suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// A well-formed id no test ever created: `<prefix>-` followed by the 32 hex characters
/// of `suffix`, as the modeled `^<prefix>-[0-9a-f]{32}$` label patterns require, so the
/// request passes the boundary validator and the handler is what decides the outcome.
///
/// `pub(crate)` so every suite in this test target names an absent resource the same way.
pub(crate) fn absent_id(prefix: &str, suffix: &str) -> String {
    assert_eq!(suffix.len(), 32, "an id suffix is 32 hex characters");
    format!("{prefix}-{suffix}")
}

#[tokio::test]
async fn create_farm_then_get_farm_returns_what_was_created() {
    let emulator = Emulator::new();

    // The rule: GetFarm reports the values CreateFarm persisted. The rows differ along
    // every member CreateFarm accepts — the display name, an empty vs. a filled
    // description, the cost scale factor at both published bounds and in between, a KMS
    // key, and tags.
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({"displayName": "farm-a"})),
        (
            "described",
            json!({"displayName": "farm-b", "description": "the second farm"}),
        ),
        (
            "cost scale factor at the lower bound",
            json!({"displayName": "farm-c", "costScaleFactor": 0.0}),
        ),
        (
            "cost scale factor at the upper bound",
            json!({"displayName": "farm-d", "costScaleFactor": 100.0}),
        ),
        (
            "discounted, keyed and tagged",
            json!({
                "displayName": "farm-e",
                "description": "",
                "costScaleFactor": 0.5,
                "kmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "tags": {"team": "lighting"},
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, created) = emulator.post("/2023-10-12/farms", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateFarm to answer 200, got {status} with {created}"
        );
        let farm_id = id_of(&created, "farmId");
        assert!(
            is_resource_id(&farm_id, "farm"),
            "case {case:?}: expected a ^farm-[0-9a-f]{{32}}$ id, got {farm_id:?}"
        );

        let (status, farm) = emulator.get(&format!("/2023-10-12/farms/{farm_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetFarm to answer 200, got {status} with {farm}"
        );
        assert_eq!(
            string_at(&farm, "farmId"),
            farm_id,
            "case {case:?}: GetFarm reported another farm's id, body {farm}"
        );
        assert_eq!(
            string_at(&farm, "displayName"),
            string_at(&request, "displayName"),
            "case {case:?}: expected the created displayName, got {farm}"
        );
        // `description` defaults to "" and `costScaleFactor` to 1, per the model.
        let expected_description = request
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_eq!(
            farm.get("description").and_then(Value::as_str),
            Some(expected_description),
            "case {case:?}: expected description {expected_description:?}, got {farm}"
        );
        let expected_factor = request
            .get("costScaleFactor")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        assert_eq!(
            farm.get("costScaleFactor").and_then(Value::as_f64),
            Some(expected_factor),
            "case {case:?}: expected costScaleFactor {expected_factor}, got {farm}"
        );
        assert_eq!(
            farm.get("kmsKeyArn").and_then(Value::as_str),
            request.get("kmsKeyArn").and_then(Value::as_str),
            "case {case:?}: expected the created kmsKeyArn, got {farm}"
        );
        assert!(
            !string_at(&farm, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {farm}"
        );
        assert!(
            // `createdAt` is modelled `@timestampFormat("date-time")`, so its wire form is
            // an RFC 3339 string rather than restJson1's default epoch-seconds number
            // (`date_time_wire_form` owns that rule for every path of this service). What
            // this slice owns is that the value is present and does not move between reads.
            farm.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {farm}"
        );
        // Nothing has updated the farm, so the update members stay absent.
        assert!(
            farm.get("updatedAt").is_none() && farm.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {farm}"
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator.get(&format!("/2023-10-12/farms/{farm_id}")).await;
        assert_eq!(
            again, farm,
            "case {case:?}: two reads of one farm disagreed: {farm} then {again}"
        );
    }
}

#[tokio::test]
async fn create_queue_then_get_queue_returns_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("queues").await;

    // The rule: GetQueue reports the values CreateQueue persisted under that farm, and a
    // new queue is IDLE. The rows differ along the optional members and the list sizes.
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({"displayName": "queue-a"})),
        (
            "described with a role",
            json!({
                "displayName": "queue-b",
                "description": "the second queue",
                "roleArn": "arn:aws:iam::000000000000:role/DeadlineQueue",
            }),
        ),
        (
            "budget action and empty lists",
            json!({
                "displayName": "queue-c",
                "defaultBudgetAction": "STOP_SCHEDULING_AND_COMPLETE_TASKS",
                "requiredFileSystemLocationNames": [],
                "allowedStorageProfileIds": [],
            }),
        ),
        (
            "budget action and filled lists",
            json!({
                "displayName": "queue-d",
                "defaultBudgetAction": "STOP_SCHEDULING_AND_CANCEL_TASKS",
                "requiredFileSystemLocationNames": ["assets", "scratch"],
                "allowedStorageProfileIds": ["sp-0123456789abcdef0123456789abcdef"],
                "jobRunAsUser": {"runAs": "QUEUE_CONFIGURED_USER",
                                 "posix": {"user": "jobuser", "group": "jobgroup"}},
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, created) = emulator
            .post(
                &format!("/2023-10-12/farms/{farm_id}/queues"),
                request.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateQueue to answer 200, got {status} with {created}"
        );
        let queue_id = id_of(&created, "queueId");
        assert!(
            is_resource_id(&queue_id, "queue"),
            "case {case:?}: expected a ^queue-[0-9a-f]{{32}}$ id, got {queue_id:?}"
        );

        let (status, queue) = emulator
            .get(&format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetQueue to answer 200, got {status} with {queue}"
        );
        assert_eq!(
            string_at(&queue, "farmId"),
            farm_id,
            "case {case:?}: expected the queue's own farmId, got {queue}"
        );
        assert_eq!(
            string_at(&queue, "queueId"),
            queue_id,
            "case {case:?}: GetQueue reported another queue's id, body {queue}"
        );
        assert_eq!(
            string_at(&queue, "displayName"),
            string_at(&request, "displayName"),
            "case {case:?}: expected the created displayName, got {queue}"
        );
        assert_eq!(
            string_at(&queue, "status"),
            "IDLE",
            "case {case:?}: expected a new queue to be IDLE, got {queue}"
        );
        // `defaultBudgetAction` defaults to NONE, per the model.
        let expected_action = request
            .get("defaultBudgetAction")
            .and_then(Value::as_str)
            .unwrap_or("NONE");
        assert_eq!(
            string_at(&queue, "defaultBudgetAction"),
            expected_action,
            "case {case:?}: expected defaultBudgetAction {expected_action}, got {queue}"
        );
        assert_eq!(
            queue.get("roleArn").and_then(Value::as_str),
            request.get("roleArn").and_then(Value::as_str),
            "case {case:?}: expected the created roleArn, got {queue}"
        );
        assert_eq!(
            queue.get("requiredFileSystemLocationNames"),
            request.get("requiredFileSystemLocationNames"),
            "case {case:?}: expected the created requiredFileSystemLocationNames, got {queue}"
        );
        assert_eq!(
            queue.get("allowedStorageProfileIds"),
            request.get("allowedStorageProfileIds"),
            "case {case:?}: expected the created allowedStorageProfileIds, got {queue}"
        );
        assert!(
            !string_at(&queue, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {queue}"
        );
    }
}

#[tokio::test]
async fn create_job_then_get_job_returns_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("jobs").await;
    let queue_id = emulator.queue(&farm_id, "jobs").await;

    // The rule: GetJob reports the values CreateJob persisted, and the job's name is the
    // override when there is one, otherwise the name its template declares. The rows
    // differ along the template language, the name source, the priority (both published
    // bounds and a middle value) and the optional counts.
    struct Case {
        case: &'static str,
        request: Value,
        expected_name: &'static str,
        expected_priority: i64,
    }
    let cases = vec![
        Case {
            case: "JSON template names the job",
            request: json!({
                "template": json_template("render-from-json"),
                "templateType": "JSON",
                "priority": 50,
            }),
            expected_name: "render-from-json",
            expected_priority: 50,
        },
        Case {
            case: "YAML template with the name overridden",
            request: json!({
                "template": yaml_template("render-from-yaml"),
                "templateType": "YAML",
                "nameOverride": "named-by-the-request",
                "priority": 0,
            }),
            expected_name: "named-by-the-request",
            expected_priority: 0,
        },
        Case {
            case: "nameOverride wins over the template name",
            request: json!({
                "template": json_template("render-from-json"),
                "templateType": "JSON",
                "priority": 100,
                "nameOverride": "the-override",
                "descriptionOverride": "a job with an overridden name",
            }),
            expected_name: "the-override",
            expected_priority: 100,
        },
        Case {
            case: "no template, only an override",
            request: json!({"priority": 7, "nameOverride": "template-less"}),
            expected_name: "template-less",
            expected_priority: 7,
        },
        Case {
            case: "explicit retry and worker counts",
            request: json!({
                "template": yaml_template("bounded"),
                "templateType": "YAML",
                "nameOverride": "bounded",
                "priority": 42,
                "maxFailedTasksCount": 0,
                "maxRetriesPerTask": 11,
                "maxWorkerCount": -1,
                "targetTaskRunStatus": "SUSPENDED",
            }),
            expected_name: "bounded",
            expected_priority: 42,
        },
    ];

    for Case {
        case,
        request,
        expected_name,
        expected_priority,
    } in cases
    {
        let (status, created) = emulator
            .post(
                &format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"),
                request.clone(),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateJob to answer 201, got {status} with {created}"
        );
        let job_id = id_of(&created, "jobId");
        assert!(
            is_resource_id(&job_id, "job"),
            "case {case:?}: expected a ^job-[0-9a-f]{{32}}$ id, got {job_id:?}"
        );

        let (status, job) = emulator
            .get(&format!(
                "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetJob to answer 200, got {status} with {job}"
        );
        assert_eq!(
            string_at(&job, "jobId"),
            job_id,
            "case {case:?}: GetJob reported another job's id, body {job}"
        );
        assert_eq!(
            string_at(&job, "name"),
            expected_name,
            "case {case:?}: expected name {expected_name:?}, got {job}"
        );
        assert_eq!(
            job.get("priority").and_then(Value::as_i64),
            Some(expected_priority),
            "case {case:?}: expected priority {expected_priority}, got {job}"
        );
        // `maxFailedTasksCount` defaults to 20, `maxRetriesPerTask` to 5 and
        // `targetTaskRunStatus` to READY, per the model.
        let expected_failed = request
            .get("maxFailedTasksCount")
            .and_then(Value::as_i64)
            .unwrap_or(20);
        assert_eq!(
            job.get("maxFailedTasksCount").and_then(Value::as_i64),
            Some(expected_failed),
            "case {case:?}: expected maxFailedTasksCount {expected_failed}, got {job}"
        );
        let expected_retries = request
            .get("maxRetriesPerTask")
            .and_then(Value::as_i64)
            .unwrap_or(5);
        assert_eq!(
            job.get("maxRetriesPerTask").and_then(Value::as_i64),
            Some(expected_retries),
            "case {case:?}: expected maxRetriesPerTask {expected_retries}, got {job}"
        );
        let expected_target = request
            .get("targetTaskRunStatus")
            .and_then(Value::as_str)
            .unwrap_or("READY");
        assert_eq!(
            job.get("targetTaskRunStatus").and_then(Value::as_str),
            Some(expected_target),
            "case {case:?}: expected targetTaskRunStatus {expected_target}, got {job}"
        );
        assert_eq!(
            job.get("maxWorkerCount").and_then(Value::as_i64),
            request.get("maxWorkerCount").and_then(Value::as_i64),
            "case {case:?}: expected the created maxWorkerCount, got {job}"
        );
        assert_eq!(
            job.get("description").and_then(Value::as_str),
            request.get("descriptionOverride").and_then(Value::as_str),
            "case {case:?}: expected the created descriptionOverride, got {job}"
        );
        assert!(
            !string_at(&job, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {job}"
        );
    }
}

#[tokio::test]
async fn a_job_reaches_create_complete_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("lifecycle").await;
    let queue_id = emulator.queue(&farm_id, "lifecycle").await;
    let job_id = emulator.job(&farm_id, &queue_id, "settles", 50).await;
    let path = format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}");

    // A caller reading the job straight back still sees the non-terminal status the
    // create left it in.
    let (status, job) = emulator.get(&path).await;
    assert_eq!(status, 200, "GetJob: expected 200, got {status} with {job}");
    assert_eq!(
        string_at(&job, "lifecycleStatus"),
        "CREATE_IN_PROGRESS",
        "immediately after CreateJob: expected CREATE_IN_PROGRESS, got {job}"
    );

    // The transition is owed by the clock, not by any call: wait past the span and the
    // next read reports it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (status, job) = emulator.get(&path).await;
    assert_eq!(status, 200, "GetJob: expected 200, got {status} with {job}");
    assert_eq!(
        string_at(&job, "lifecycleStatus"),
        "CREATE_COMPLETE",
        "after the create span: expected CREATE_COMPLETE, got {job}"
    );
    assert!(
        !string_at(&job, "lifecycleStatusMessage").is_empty(),
        "expected a lifecycleStatusMessage, got {job}"
    );

    // ListJobs reports the same settled status as GetJob.
    let (status, listed) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"
        ))
        .await;
    assert_eq!(status, 200, "ListJobs: expected 200, got {status}: {listed}");
    let summary = listed["jobs"]
        .as_array()
        .and_then(|jobs| jobs.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        summary.get("lifecycleStatus").and_then(Value::as_str),
        Some("CREATE_COMPLETE"),
        "ListJobs: expected the settled lifecycle status, got {listed}"
    );
}

#[tokio::test]
async fn minted_ids_are_unique_across_creates() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("ids").await;
    let queue_id = emulator.queue(&farm_id, "ids").await;

    let mut ids = Vec::new();
    for index in 0..12 {
        ids.push(emulator.farm(&format!("farm-{index}")).await);
        ids.push(emulator.queue(&farm_id, &format!("queue-{index}")).await);
        ids.push(
            emulator
                .job(&farm_id, &queue_id, &format!("job-{index}"), 50)
                .await,
        );
    }
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        count,
        "expected {count} distinct minted ids, got {} after dedup",
        ids.len()
    );
}

#[tokio::test]
async fn a_repeated_client_token_creates_one_resource() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotency").await;
    let queue_id = emulator.queue(&farm_id, "idempotency").await;

    // The rule: a create replayed with the same `X-Amz-Client-Token` answers with the
    // resource the first call made; a different token makes a different resource. The rows
    // are the three creates this slice implements.
    let cases: Vec<(&str, String, Value, &str)> = vec![
        (
            "CreateFarm",
            "/2023-10-12/farms".to_string(),
            json!({"displayName": "token-farm"}),
            "farmId",
        ),
        (
            "CreateQueue",
            format!("/2023-10-12/farms/{farm_id}/queues"),
            json!({"displayName": "token-queue"}),
            "queueId",
        ),
        (
            "CreateJob",
            format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"),
            json!({"priority": 50, "nameOverride": "token-job"}),
            "jobId",
        ),
    ];

    for (case, path, request, member) in cases {
        let (_, first) = emulator
            .call("POST", &path, Some(request.clone()), Some("token-one"))
            .await;
        let (_, replay) = emulator
            .call("POST", &path, Some(request.clone()), Some("token-one"))
            .await;
        let (_, other) = emulator
            .call("POST", &path, Some(request.clone()), Some("token-two"))
            .await;

        assert_eq!(
            id_of(&first, member),
            id_of(&replay, member),
            "case {case:?}: the same client token made a second resource: {first} then {replay}"
        );
        assert_ne!(
            id_of(&first, member),
            id_of(&other, member),
            "case {case:?}: two different client tokens shared one resource: {first} and {other}"
        );
    }
}

#[tokio::test]
async fn list_farms_pages_over_every_farm() {
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for index in 0..7 {
        created.push(emulator.farm(&format!("page-{index}")).await);
    }
    created.sort();

    // The rule: paging returns every farm exactly once, in at most `maxResults` per page,
    // and stops handing back a token at the end. The rows are page sizes that divide the
    // set evenly, unevenly, and exceed it, plus both published bounds of maxResults.
    for page_size in [1_usize, 2, 3, 7, 50, 100] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = emulator.get(&format!("/2023-10-12/farms{query}")).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected ListFarms to answer 200, got {status}: {body}"
            );
            let farms = body["farms"]
                .as_array()
                .unwrap_or_else(|| panic!("page size {page_size}: expected farms in {body}"))
                .clone();
            assert!(
                farms.len() <= page_size,
                "page size {page_size}: expected at most {page_size} farms, got {}",
                farms.len()
            );
            for farm in &farms {
                seen.push(string_at(farm, "farmId"));
            }
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, created,
            "page size {page_size}: expected every farm exactly once, got {seen:?}"
        );
    }

    // An unknown token is a client error, not an empty page.
    let (status, body) = emulator
        .get("/2023-10-12/farms?nextToken=farm-00000000000000000000000000000000")
        .await;
    assert_eq!(
        status, 400,
        "unknown nextToken: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "unknown nextToken: expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn list_jobs_reports_only_the_jobs_of_its_own_queue() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("scoping").await;
    let left = emulator.queue(&farm_id, "left").await;
    let right = emulator.queue(&farm_id, "right").await;

    let left_jobs = vec![
        emulator.job(&farm_id, &left, "left-one", 10).await,
        emulator.job(&farm_id, &left, "left-two", 20).await,
    ];
    let right_jobs = vec![emulator.job(&farm_id, &right, "right-one", 30).await];

    for (case, queue_id, expected) in [
        ("left queue", &left, &left_jobs),
        ("right queue", &right, &right_jobs),
    ] {
        let (status, body) = emulator
            .get(&format!(
                "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListJobs to answer 200, got {status} with {body}"
        );
        let mut listed: Vec<String> = body["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected jobs in {body}"))
            .iter()
            .map(|job| string_at(job, "jobId"))
            .collect();
        listed.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            listed, want,
            "case {case:?}: expected exactly this queue's jobs {want:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_this_emulator_does_not_hold_is_a_404() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("present").await;
    let queue_id = emulator.queue(&farm_id, "present").await;
    let job_id = emulator.job(&farm_id, &queue_id, "present", 50).await;

    let absent_farm = "farm-00000000000000000000000000000000";
    let absent_queue = "queue-00000000000000000000000000000000";
    let absent_job = "job-00000000000000000000000000000000";

    // A second farm, so "a queue of another farm" is a real case rather than an absent id.
    let other_farm = emulator.farm("other").await;

    // The rule: a path that names a resource this emulator does not hold — at any level of
    // farm -> queue -> job, including a child reached through the wrong parent — answers
    // ResourceNotFoundException (404), never 501 and never a canned resource.
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "GetFarm, unknown farm",
            "GET",
            format!("/2023-10-12/farms/{absent_farm}"),
        ),
        (
            "DeleteFarm, unknown farm",
            "DELETE",
            format!("/2023-10-12/farms/{absent_farm}"),
        ),
        (
            "GetQueue, unknown farm",
            "GET",
            format!("/2023-10-12/farms/{absent_farm}/queues/{queue_id}"),
        ),
        (
            "GetQueue, unknown queue",
            "GET",
            format!("/2023-10-12/farms/{farm_id}/queues/{absent_queue}"),
        ),
        (
            "GetQueue, queue of another farm",
            "GET",
            format!("/2023-10-12/farms/{other_farm}/queues/{queue_id}"),
        ),
        (
            "DeleteQueue, queue of another farm",
            "DELETE",
            format!("/2023-10-12/farms/{other_farm}/queues/{queue_id}"),
        ),
        (
            "GetJob, unknown job",
            "GET",
            format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{absent_job}"),
        ),
        (
            "GetJob, job of another queue",
            "GET",
            format!("/2023-10-12/farms/{farm_id}/queues/{absent_queue}/jobs/{job_id}"),
        ),
        (
            "ListJobs, unknown queue",
            "GET",
            format!("/2023-10-12/farms/{farm_id}/queues/{absent_queue}/jobs"),
        ),
    ];

    for (case, method, path) in cases {
        let (status, body) = emulator.call(method, &path, None, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The same rule on the creates, which reach a parent they must find first.
    let creates: Vec<(&str, String, Value)> = vec![
        (
            "CreateQueue, unknown farm",
            format!("/2023-10-12/farms/{absent_farm}/queues"),
            json!({"displayName": "nowhere"}),
        ),
        (
            "CreateJob, unknown farm",
            format!("/2023-10-12/farms/{absent_farm}/queues/{queue_id}/jobs"),
            json!({"priority": 50, "nameOverride": "nowhere"}),
        ),
        (
            "CreateJob, unknown queue",
            format!("/2023-10-12/farms/{farm_id}/queues/{absent_queue}/jobs"),
            json!({"priority": 50, "nameOverride": "nowhere"}),
        ),
        (
            "CreateJob, unknown sourceJobId",
            format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"),
            json!({"priority": 50, "nameOverride": "nowhere", "sourceJobId": absent_job}),
        ),
    ];

    for (case, path, request) in creates {
        let (status, body) = emulator.post(&path, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn delete_closes_the_lifecycle() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("deletable").await;
    let queue_id = emulator.queue(&farm_id, "deletable").await;
    let job_id = emulator.job(&farm_id, &queue_id, "deletable", 50).await;
    let kept_farm = emulator.farm("kept").await;

    let (status, body) = emulator
        .delete(&format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteQueue: expected 200, got {status} with {body}"
    );

    // The queue is gone, and so are the jobs that were reachable only through it.
    for (case, path) in [
        (
            "GetQueue after DeleteQueue",
            format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"),
        ),
        (
            "GetJob after DeleteQueue",
            format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}"),
        ),
    ] {
        let (status, body) = emulator.get(&path).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
    }

    let (status, body) = emulator.delete(&format!("/2023-10-12/farms/{farm_id}")).await;
    assert_eq!(
        status, 200,
        "DeleteFarm: expected 200, got {status} with {body}"
    );
    let (status, body) = emulator.get(&format!("/2023-10-12/farms/{farm_id}")).await;
    assert_eq!(
        status, 404,
        "GetFarm after DeleteFarm: expected 404, got {status} with {body}"
    );

    // Deleting one farm leaves every other farm where it was.
    let (status, body) = emulator.get("/2023-10-12/farms").await;
    assert_eq!(status, 200, "ListFarms: expected 200, got {status}: {body}");
    let remaining: Vec<String> = body["farms"]
        .as_array()
        .unwrap_or_else(|| panic!("expected farms in {body}"))
        .iter()
        .map(|farm| string_at(farm, "farmId"))
        .collect();
    assert_eq!(
        remaining,
        vec![kept_farm.clone()],
        "expected only the kept farm {kept_farm} to remain, got {remaining:?}"
    );
}

#[tokio::test]
async fn a_job_with_no_name_anywhere_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("validation").await;
    let queue_id = emulator.queue(&farm_id, "validation").await;
    let path = format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs");

    // The rule: a job must end up with a name, and a template can only be read in the
    // language the request says it is written in.
    let cases: Vec<(&str, Value)> = vec![
        ("no template and no nameOverride", json!({"priority": 50})),
        (
            "a template that declares no name",
            json!({
                "priority": 50,
                "template": "{\"specificationVersion\":\"jobtemplate-2023-09\"}",
                "templateType": "JSON",
            }),
        ),
        (
            "a template that is not the JSON it claims to be",
            json!({"priority": 50, "template": "not json at all {", "templateType": "JSON"}),
        ),
        (
            "a JSON template that declares an empty name",
            json!({
                "priority": 50,
                "template": "{\"name\":\"\",\"steps\":[]}",
                "templateType": "JSON",
            }),
        ),
        (
            "a template with no templateType",
            json!({"priority": 50, "template": json_template("unreadable")}),
        ),
        (
            // Nesting is the one dimension a document can turn into stack depth, and
            // serde_json refuses past its own recursion limit rather than growing the
            // stack, so the refusal is a client error like any other unreadable template.
            "a JSON template nested deeper than the parser reads",
            json!({
                "priority": 50,
                "template": format!("{}1{}", "[".repeat(2000), "]".repeat(2000)),
                "templateType": "JSON",
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.post(&path, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_yaml_template_needs_a_name_override() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("yaml").await;
    let queue_id = emulator.queue(&farm_id, "yaml").await;
    let path = format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs");

    // The rule: a job's name comes from `nameOverride`, else from the template — and a
    // YAML template cannot be read in this build (no YAML parser), so the one case that
    // would need that read answers the scaffold's 501 rather than inventing a name or
    // refusing a request the real service accepts. Every other YAML case works. The rows
    // differ along what names the job: nothing, an override, and a source job.
    let (status, body) = emulator
        .post(
            &path,
            json!({
                "priority": 50,
                "template": yaml_template("named-in-the-template"),
                "templateType": "YAML",
            }),
        )
        .await;
    assert_eq!(
        status, 501,
        "a YAML template with no nameOverride: expected the scaffold's 501, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "a YAML template with no nameOverride: expected NotImplemented, got {body}"
    );

    // With a name in the request, the YAML template needs no reading: the job is created
    // and the template is stored as given.
    let (status, created) = emulator
        .post(
            &path,
            json!({
                "priority": 50,
                "template": yaml_template("named-in-the-template"),
                "templateType": "YAML",
                "nameOverride": "named-by-the-request",
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "a YAML template with nameOverride: expected 201, got {status} with {created}"
    );
    let job_id = id_of(&created, "jobId");
    let (status, job) = emulator.get(&format!("{path}/{job_id}")).await;
    assert_eq!(status, 200, "GetJob: expected 200, got {status} with {job}");
    assert_eq!(
        string_at(&job, "name"),
        "named-by-the-request",
        "expected the overridden name, got {job}"
    );

    // And a job copied from that one inherits its stored name, so no template is read for
    // it either.
    let (status, copied) = emulator
        .post(&path, json!({"priority": 50, "sourceJobId": job_id}))
        .await;
    assert_eq!(
        status, 201,
        "a job copied from a YAML-templated job: expected 201, got {status} with {copied}"
    );
    let copy_id = id_of(&copied, "jobId");
    let (_, copy) = emulator.get(&format!("{path}/{copy_id}")).await;
    assert_eq!(
        string_at(&copy, "name"),
        "named-by-the-request",
        "expected the source job's stored name, got {copy}"
    );
}

#[tokio::test]
async fn deleting_a_resource_releases_its_client_token() {
    let emulator = Emulator::new();

    // The rule: a client token remembers a resource only while that resource exists, so
    // replaying the token after the resource is deleted creates a new one rather than
    // answering with an id nothing holds.
    let (status, first) = emulator
        .call(
            "POST",
            "/2023-10-12/farms",
            Some(json!({"displayName": "recreated"})),
            Some("token-release"),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFarm: expected 200, got {status} with {first}"
    );
    let first_id = id_of(&first, "farmId");

    let (status, body) = emulator
        .delete(&format!("/2023-10-12/farms/{first_id}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteFarm: expected 200, got {status} with {body}"
    );

    let (status, second) = emulator
        .call(
            "POST",
            "/2023-10-12/farms",
            Some(json!({"displayName": "recreated"})),
            Some("token-release"),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFarm replayed after the delete: expected 200, got {status} with {second}"
    );
    let second_id = id_of(&second, "farmId");
    assert_ne!(
        first_id, second_id,
        "expected a new farm after the first was deleted, got {first_id} twice"
    );
    let (status, farm) = emulator
        .get(&format!("/2023-10-12/farms/{second_id}"))
        .await;
    assert_eq!(
        status, 200,
        "GetFarm on the recreated farm: expected 200, got {status} with {farm}"
    );
}

#[tokio::test]
async fn a_create_past_this_emulators_stored_bounds_is_refused() {
    let emulator = Emulator::new();

    // The rule: every create is a caller-driven insert into a store only a delete empties,
    // so each level of the hierarchy is bounded and the create past the bound answers the
    // modeled ServiceQuotaExceededException (402) instead of growing the store. The rows
    // are the three levels plus the job-template byte budget, each driven to its own bound.

    // Farms: the store holds MAX_FARMS (500) of them.
    for index in 0..500 {
        let (status, body) = emulator
            .post(
                "/2023-10-12/farms",
                json!({"displayName": format!("bounded-{index}")}),
            )
            .await;
        assert_eq!(
            status, 200,
            "farm {index}: expected 200 below the bound, got {status} with {body}"
        );
    }
    let (status, body) = emulator
        .post("/2023-10-12/farms", json!({"displayName": "one-too-many"}))
        .await;
    assert_eq!(
        status, 402,
        "farm past the bound: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "farm past the bound: expected ServiceQuotaExceededException, got {body}"
    );

    // Deleting one makes room for exactly one more, so the bound is a live count and not
    // a one-way counter.
    let (_, listed) = emulator.get("/2023-10-12/farms?maxResults=1").await;
    let victim = string_at(&listed["farms"][0], "farmId");
    let (status, body) = emulator.delete(&format!("/2023-10-12/farms/{victim}")).await;
    assert_eq!(
        status, 200,
        "DeleteFarm: expected 200, got {status} with {body}"
    );
    let (status, body) = emulator
        .post("/2023-10-12/farms", json!({"displayName": "room-again"}))
        .await;
    assert_eq!(
        status, 200,
        "farm after a delete freed room: expected 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_queue_or_job_create_past_its_bound_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("bounded").await;

    // Queues: one farm holds MAX_QUEUES_PER_FARM (500).
    let queues_path = format!("/2023-10-12/farms/{farm_id}/queues");
    let mut first_queue = String::new();
    for index in 0..500 {
        let (status, body) = emulator
            .post(&queues_path, json!({"displayName": format!("q-{index}")}))
            .await;
        assert_eq!(
            status, 200,
            "queue {index}: expected 200 below the bound, got {status} with {body}"
        );
        if index == 0 {
            first_queue = id_of(&body, "queueId");
        }
    }
    let (status, body) = emulator
        .post(&queues_path, json!({"displayName": "one-too-many"}))
        .await;
    assert_eq!(
        status, 402,
        "queue past the bound: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "queue past the bound: expected ServiceQuotaExceededException, got {body}"
    );

    // Jobs: one queue holds MAX_JOBS_PER_QUEUE (1000).
    let jobs_path = format!("{queues_path}/{first_queue}/jobs");
    for index in 0..1000 {
        let (status, body) = emulator
            .post(
                &jobs_path,
                json!({"priority": 50, "nameOverride": format!("j-{index}")}),
            )
            .await;
        assert_eq!(
            status, 201,
            "job {index}: expected 201 below the bound, got {status} with {body}"
        );
    }
    let (status, body) = emulator
        .post(
            &jobs_path,
            json!({"priority": 50, "nameOverride": "one-too-many"}),
        )
        .await;
    assert_eq!(
        status, 402,
        "job past the bound: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "job past the bound: expected ServiceQuotaExceededException, got {body}"
    );
}

#[tokio::test]
async fn stored_job_templates_share_a_byte_budget_that_a_delete_releases() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("templates").await;
    let queue_id = emulator.queue(&farm_id, "templates").await;
    let jobs_path = format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs");

    // One template just under the modeled maximum (`JobTemplate, len 1..1000000`), so a
    // handful of them reaches MAX_STORED_TEMPLATE_BYTES (8 MiB).
    let padding = "x".repeat(900_000);
    let big_template = |name: &str| {
        json!({
            "specificationVersion": "jobtemplate-2023-09",
            "name": name,
            "description": padding,
            "steps": [],
        })
        .to_string()
    };

    let mut accepted = 0;
    let mut refused = None;
    for index in 0..12 {
        let (status, body) = emulator
            .post(
                &jobs_path,
                json!({
                    "priority": 50,
                    "template": big_template(&format!("big-{index}")),
                    "templateType": "JSON",
                }),
            )
            .await;
        if status == 201 {
            accepted += 1;
            continue;
        }
        refused = Some((status, body));
        break;
    }

    let (status, body) = refused.unwrap_or_else(|| {
        panic!("expected the template byte budget to refuse a create within 12 attempts")
    });
    assert_eq!(
        status, 402,
        "template past the byte budget: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "template past the byte budget: expected ServiceQuotaExceededException, got {body}"
    );
    assert!(
        accepted > 0,
        "expected the budget to accept at least one 900 KB template, accepted {accepted}"
    );

    // Deleting the queue releases the bytes its jobs held, so a fresh queue can store
    // templates again.
    let (status, body) = emulator
        .delete(&format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteQueue: expected 200, got {status} with {body}"
    );
    let next_queue = emulator.queue(&farm_id, "templates-again").await;
    let (status, body) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/queues/{next_queue}/jobs"),
            json!({
                "priority": 50,
                "template": big_template("after-the-release"),
                "templateType": "JSON",
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "a template after the delete released the budget: expected 201, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_maximally_nested_megabyte_template_is_refused_not_fatal() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("nesting").await;
    let queue_id = emulator.queue(&farm_id, "nesting").await;
    let path = format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs");

    // The rule: a template is read with its nesting bounded, so the deepest template the
    // modeled length allows answers a client error and the process keeps serving. The rows
    // are ~1 MB of pure nesting in each shape JSON can take — the shape that would
    // otherwise drive a recursive-descent parser through the stack. (A YAML template is
    // never read in this build; see `a_yaml_template_needs_a_name_override`.)
    let half = 499_000;
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "JSON arrays",
            "JSON",
            format!("{}1{}", "[".repeat(half), "]".repeat(half)),
        ),
        (
            "JSON objects",
            "JSON",
            format!(
                "{}1{}",
                "{\"a\":".repeat(half / 5),
                "}".repeat(half / 5)
            ),
        ),
    ];

    for (case, template_type, template) in cases {
        assert!(
            template.len() <= 1_000_000,
            "case {case:?}: the template must stay inside the modeled maximum, got {}",
            template.len()
        );
        let (status, body) = emulator
            .post(
                &path,
                json!({"priority": 50, "template": template, "templateType": template_type}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // The emulator is still serving after those requests, which is the point.
    let (status, body) = emulator
        .post(
            &path,
            json!({
                "priority": 50,
                "template": json_template("still-serving"),
                "templateType": "JSON",
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "after the nested templates: expected the service to still create jobs, got {status} \
         with {body}"
    );
}

#[tokio::test]
async fn a_job_created_from_another_job_inherits_its_template() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("copies").await;
    let queue_id = emulator.queue(&farm_id, "copies").await;
    let source = emulator.job(&farm_id, &queue_id, "the-source", 50).await;

    let (status, created) = emulator
        .post(
            &format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs"),
            json!({"priority": 25, "sourceJobId": source}),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateJob from a source job: expected 201, got {status} with {created}"
    );
    let job_id = id_of(&created, "jobId");
    assert_ne!(
        job_id, source,
        "a job created from another job must be its own resource, got {job_id}"
    );

    let (status, job) = emulator
        .get(&format!(
            "/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}"
        ))
        .await;
    assert_eq!(status, 200, "GetJob: expected 200, got {status} with {job}");
    assert_eq!(
        string_at(&job, "name"),
        "the-source",
        "expected the source job's name, got {job}"
    );
    assert_eq!(
        job.get("sourceJobId").and_then(Value::as_str),
        Some(source.as_str()),
        "expected the sourceJobId to be reported, got {job}"
    );
    assert_eq!(
        job.get("priority").and_then(Value::as_i64),
        Some(25),
        "expected this request's own priority, got {job}"
    );
}
