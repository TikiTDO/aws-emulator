// SPDX-License-Identifier: CC0-1.0

//! `PollForJobs` end to end over the wire the service actually speaks: `awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`.
//!
//! The four rules the handler is, each asserted over a SET of inputs rather than over
//! one recorded call:
//!
//! 1. `PollForJobs` is valid only for an action type whose `ActionOwner` is `Custom`;
//!    any other owner is refused `ValidationException` (HTTP 400).
//! 2. A `Custom` action type exists only because this account registered it, so an
//!    unregistered id is refused `ActionTypeNotFoundException` (HTTP 400).
//! 3. `queryParam` must agree with the registered action type's queryable
//!    configuration property, and selects the jobs whose action configuration maps that
//!    property to the value supplied.
//! 4. A poll reports the jobs waiting for that action type, oldest first, at most
//!    `maxBatchSize` of them, each handed out with a fresh `nonce` and then withheld
//!    from later polls until that claim lapses — and every nonce a dispatch published
//!    claims the job until one of them has.
//!
//! `GetJobDetails` reads one of those jobs back by its id — the round trip for the job
//! the execution plane queued — and refuses an id that names no job with
//! `JobNotFoundException` (HTTP 400); it is implemented in this change for that reason.
//!
//! The jobs themselves come from the execution plane: a pipeline is created with an
//! action whose action type is `Custom`, and the execution reaching it is what queues
//! the job a worker polls for. Where a rule is about the SHAPE of the answer rather
//! than about an execution (the batch-size and `queryParam` tables), the queue is
//! seeded through `ComAmazonawsCodepipelineCodepipeline20150709State::enqueue_job`, its
//! one write path. Everything else goes through `MultiServiceRouter::dispatch`, so
//! nothing here can pass by reading a handler's internals.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State, JobRecord, ARTIFACT_CREDENTIAL_LIFETIME,
    JOB_POLL_LEASE,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The operation under test.
const POLL: &str = "PollForJobs";

/// The canonical emulator account id every request is signed by
/// (`emulator_server::EMULATOR_ACCOUNT_ID`), and therefore the account a job names.
const ACCOUNT_ID: &str = "000000000000";

/// The `ActionOwner` values the model declares
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `Custom`,
/// `ThirdParty`).
const OWNER_CUSTOM: &str = "Custom";
const OWNER_AWS: &str = "AWS";
const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// The two refusals this operation models, with the status the model publishes.
const VALIDATION: &str = "ValidationException";
const ACTION_TYPE_NOT_FOUND: &str = "ActionTypeNotFoundException";
const REFUSAL_STATUS: u16 = 400;

/// A `roleArn` satisfying `RoleArn`'s modelled pattern. IAM is not mounted here, so
/// `CreatePipeline`'s role check has no answer and the declaration is accepted.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// How long a test waits for the clock-owed steps of one execution: each becomes due
/// `state::execution::ACTION_STEP` (300 ms) after the one before it.
const WAIT: Duration = Duration::from_secs(20);

/// One emulator: the shared engine, CodePipeline and S3 mounted on it, and the router
/// that dispatches to both — the execution engine moves artifacts through S3, so both
/// have to be there.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { engine, router }
    }

    /// This service's store, the way a handler reaches it.
    fn store(&self) -> engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State> {
        self.engine
            .state::<ComAmazonawsCodepipelineCodepipeline20150709State>()
    }

    async fn cp(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method("POST")
            .uri("https://codepipeline.us-east-1.amazonaws.com/")
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// One CodePipeline call that must succeed.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.cp(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// One S3 request, path style, the way the emulated S3 service routes it.
    async fn s3(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8]) -> u16 {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
            .header("host", "s3.us-east-1.amazonaws.com")
            .header("authorization", authorization);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a valid request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Create a versioned bucket — what a CodePipeline source or artifact bucket is.
    async fn bucket(&self, name: &str) {
        let status = self.s3("PUT", &format!("/{name}"), &[], b"").await;
        assert_eq!(status, 200, "the bucket {name:?} must be created, got {status}");
        let status = self
            .s3(
                "PUT",
                &format!("/{name}?versioning"),
                &[("content-type", "application/xml")],
                b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            )
            .await;
        assert_eq!(
            status, 200,
            "versioning must be enabled on {name:?}, got {status}"
        );
    }

    /// Write one object.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let status = self.s3("PUT", &format!("/{bucket}/{key}"), &[], body).await;
        assert_eq!(
            status, 200,
            "writing s3://{bucket}/{key} must succeed, got {status}"
        );
    }

    /// Register a custom action type with `properties` as its configuration
    /// properties, and answer the `ActionTypeId` it is identified by.
    async fn register(&self, category: &str, provider: &str, properties: Value) -> ActionTypeId {
        let mut body = json!({
            "category": category,
            "provider": provider,
            "version": "1",
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
        });
        if let Some(declared) = properties.as_array() {
            if !declared.is_empty() {
                body["configurationProperties"] = properties;
            }
        }
        self.ok("CreateCustomActionType", &body).await;
        action_type(category, OWNER_CUSTOM, provider, "1")
    }

    /// Queue `count` jobs for `action_type`, oldest first, each carrying
    /// `configuration`, and answer the ids as queued.
    fn queue_jobs(
        &self,
        action_type: &ActionTypeId,
        configurations: &[Vec<(&str, &str)>],
    ) -> Vec<String> {
        let now = SystemTime::now();
        let mut queued = Vec::new();
        for (index, configuration) in configurations.iter().enumerate() {
            let mut job = JobRecord::new(uuid_like(index), action_type.clone(), now);
            for (name, value) in configuration {
                job.configuration
                    .insert((*name).to_string(), (*value).to_string());
            }
            queued.push(job.job_id.clone());
            let accepted = self.store().lock().enqueue_job(action_type, job);
            assert!(
                accepted,
                "seeding a job for {action_type:?}: expected the store to accept it, got refused"
            );
        }
        queued
    }

    /// Call `PollForJobs`.
    async fn poll(
        &self,
        action_type: &ActionTypeId,
        max_batch_size: Option<i32>,
        query_param: Option<(&str, &str)>,
    ) -> (u16, Value) {
        let mut body = json!({"actionTypeId": action_type_json(action_type)});
        if let Some(size) = max_batch_size {
            body["maxBatchSize"] = json!(size);
        }
        if let Some((name, value)) = query_param {
            body["queryParam"] = json!({name: value});
        }
        self.cp(POLL, &body).await
    }

    /// Poll until a job is reported, or give up after [`WAIT`].
    async fn poll_until_job(&self, action_type: &ActionTypeId) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let (status, response) = self.poll(action_type, None, None).await;
            assert_eq!(
                status, 200,
                "polling for a job: expected HTTP 200, got {status} with {response}"
            );
            if let Some(job) = reported_jobs(&response).into_iter().next() {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "polling for a job: no job was queued within {WAIT:?}; last answer {response}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The `ActionExecutionStatus` `GetPipelineState` reports for one action.
    async fn action_status(&self, pipeline: &str, stage: &str, action: &str) -> Option<String> {
        let state = self.ok("GetPipelineState", &json!({"name": pipeline})).await;
        state["stageStates"]
            .as_array()?
            .iter()
            .find(|reported| reported["stageName"] == json!(stage))?["actionStates"]
            .as_array()?
            .iter()
            .find(|reported| reported["actionName"] == json!(action))?["latestExecution"]["status"]
            .as_str()
            .map(str::to_string)
    }

    /// Drive the execution forward until `action` is reported `InProgress`, without
    /// polling for its job (a poll would claim it).
    async fn wait_for_action_in_progress(&self, pipeline: &str, stage: &str, action: &str) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if self.action_status(pipeline, stage, action).await.as_deref() == Some("InProgress") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "the action {action:?} of stage {stage:?} was not InProgress within {WAIT:?}, it is \
             {:?}",
            self.action_status(pipeline, stage, action).await
        );
    }
}

/// An `ActionTypeId`'s four identifying fields, every one `@required`.
fn action_type(category: &str, owner: &str, provider: &str, version: &str) -> ActionTypeId {
    ActionTypeId {
        category: category.to_string(),
        owner: owner.to_string(),
        provider: provider.to_string(),
        version: version.to_string(),
    }
}

fn action_type_json(id: &ActionTypeId) -> Value {
    json!({
        "category": id.category,
        "owner": id.owner,
        "provider": id.provider,
        "version": id.version,
    })
}

/// One configuration property as `CreateCustomActionType` declares it. A queryable
/// property "must be both required and not secret"
/// (`state::validate_configuration_properties`).
fn property(name: &str, queryable: bool) -> Value {
    json!({
        "name": name,
        "required": queryable,
        "key": false,
        "secret": false,
        "queryable": queryable,
    })
}

/// A value matching `JobId`'s modelled pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), distinct per
/// `index`.
fn uuid_like(index: usize) -> String {
    format!("{index:08x}-0000-4000-8000-{index:012x}")
}

/// Whether `value` is a uuid in the form `JobId` and `PipelineExecutionId` model.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(length, group)| {
                group.len() == *length && group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// `value` with its first hex digit changed, so the result is still a `JobId` but names
/// another job.
fn flip_first_digit(value: &str) -> String {
    let mut flipped = value.to_string();
    let replacement = if value.starts_with('0') { '1' } else { '0' };
    flipped.replace_range(0..1, &replacement.to_string());
    flipped
}

/// The awsJson error code (`__type`) and message an error response carries.
fn error_of(operation: &str, response: &Value) -> (String, String) {
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .to_string();
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// The jobs a successful poll reported, in order.
fn reported_jobs(response: &Value) -> Vec<Value> {
    response
        .get("jobs")
        .map(|jobs| {
            jobs.as_array()
                .unwrap_or_else(|| panic!("jobs is not a list: {response}"))
                .clone()
        })
        .unwrap_or_default()
}

/// The ids a successful poll reported, in order.
fn reported_ids(response: &Value) -> Vec<String> {
    reported_jobs(response)
        .iter()
        .map(|job| {
            job.get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a job without an id: {job}"))
                .to_string()
        })
        .collect()
}

/// A pipeline whose second stage is a CUSTOM action: an S3 source produces
/// `SourceArtifact`, then the custom action consumes it and is to produce
/// `BuildArtifact`.
fn custom_action_pipeline(
    name: &str,
    action_type: &ActionTypeId,
    configuration: Value,
) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": artifact_bucket(name)},
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Src",
                    "actionTypeId": {
                        "category": "Source", "owner": OWNER_AWS, "provider": "S3", "version": "1"
                    },
                    "configuration": {"S3Bucket": "source-bucket", "S3ObjectKey": "source.zip"},
                    "outputArtifacts": [{"name": "SourceArtifact"}],
                }],
            },
            {
                "name": "Work",
                "actions": [{
                    "name": "Custom",
                    "actionTypeId": action_type_json(action_type),
                    "configuration": configuration,
                    "inputArtifacts": [{"name": "SourceArtifact"}],
                    "outputArtifacts": [{"name": "BuildArtifact"}],
                }],
            },
        ],
    }})
}

fn artifact_bucket(name: &str) -> String {
    format!("{name}-artifacts")
}

/// Create the buckets, the source object, the custom action type and the pipeline;
/// answer the action type and the id of the execution the pipeline's creation started.
///
/// Creating a pipeline RUNS it once — `TriggerType` carries `CreatePipeline` for that
/// run — so the execution whose custom action queues the job each case below polls for
/// is the one the create began.
async fn running_pipeline(
    emulator: &Emulator,
    pipeline: &str,
    properties: Value,
    configuration: Value,
) -> (ActionTypeId, String) {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.put_object("source-bucket", "source.zip", b"job bytes").await;
    let action_type = emulator.register("Build", "MyWorker", properties).await;
    emulator
        .ok(
            "CreatePipeline",
            &custom_action_pipeline(pipeline, &action_type, configuration),
        )
        .await;
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    let execution = listed["pipelineExecutionSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["trigger"]["triggerType"] == json!("CreatePipeline"))
        .and_then(|summary| summary["pipelineExecutionId"].as_str())
        .unwrap_or_else(|| panic!("creating {pipeline} ran it once, got {listed}"))
        .to_string();
    (action_type, execution)
}

/// Rule 1 and rule 2, over the class of action types a custom-action poll cannot name:
/// every owner that is not `Custom` (differing in category, provider and version too),
/// and a `Custom` id this account never registered.
#[tokio::test]
async fn an_action_type_that_is_not_a_registered_custom_action_is_refused() {
    // (case, the action type polled for, the refusal expected)
    let rows: [(&str, ActionTypeId, &str); 7] = [
        (
            "an AWS-owned action type is run by AWS",
            action_type("Source", OWNER_AWS, "S3", "1"),
            VALIDATION,
        ),
        (
            "an AWS-owned action type of another category",
            action_type("Deploy", OWNER_AWS, "CodeDeploy", "1"),
            VALIDATION,
        ),
        (
            "a ThirdParty action type is its partner's",
            action_type("Source", OWNER_THIRD_PARTY, "GitHub", "1"),
            VALIDATION,
        ),
        (
            "a ThirdParty action type of another version",
            action_type("Test", OWNER_THIRD_PARTY, "ExampleScm", "2"),
            VALIDATION,
        ),
        (
            "a Custom action type that was never created",
            action_type("Build", OWNER_CUSTOM, "NoSuchWorker", "1"),
            ACTION_TYPE_NOT_FOUND,
        ),
        (
            "a Custom action type whose category was never created",
            action_type("Test", OWNER_CUSTOM, "MyWorker", "1"),
            ACTION_TYPE_NOT_FOUND,
        ),
        (
            "a Custom action type whose version was never created",
            action_type("Build", OWNER_CUSTOM, "MyWorker", "2"),
            ACTION_TYPE_NOT_FOUND,
        ),
    ];

    let emulator = Emulator::new();
    // One registered custom action type, so "not registered" is about the id the
    // request named rather than about an empty registry.
    let registered = emulator.register("Build", "MyWorker", json!([])).await;
    // A job waiting changes nothing about either refusal.
    emulator.queue_jobs(&registered, &[vec![]]);

    for (case, polled, expected_code) in rows {
        let (status, response) = emulator.poll(&polled, None, None).await;
        let (code, message) = error_of(POLL, &response);
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with {response}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code}, got {code:?} with {response}"
        );
        // The refusal is about the action type the REQUEST named, so it names that
        // action type's own fields.
        for field in [
            polled.category.as_str(),
            polled.owner.as_str(),
            polled.provider.as_str(),
            polled.version.as_str(),
        ] {
            assert!(
                message.contains(field),
                "case {case:?}: expected the message to name {field:?}, got {message:?}"
            );
        }
    }
}

/// Rule 4's first half, end to end: an execution reaching a custom action queues the
/// job a worker polls for, and the job reports what the worker needs — from the stored
/// declaration and the execution, never from the poll.
#[tokio::test]
async fn an_execution_reaching_a_custom_action_queues_a_job_for_its_worker() {
    let emulator = Emulator::new();
    let pipeline = "custom-job-pipe";
    let (action_type, execution) = running_pipeline(
        &emulator,
        pipeline,
        json!([]),
        json!({"Command": "build", "Timeout": "5"}),
    )
    .await;

    let job = emulator.poll_until_job(&action_type).await;
    let id = job["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid(id),
        "case \"a queued job\": expected a uuid job id, got {id:?}"
    );
    let nonce = job["nonce"].as_str().unwrap_or_default();
    assert!(
        (1..=50).contains(&nonce.len()),
        "case \"a queued job\": expected a nonce of 1..50 characters, got {nonce:?}"
    );
    assert_eq!(
        job["accountId"].as_str(),
        Some(ACCOUNT_ID),
        "case \"a queued job\": expected the account the pipeline lives in, got {job}"
    );
    assert_eq!(
        job["data"]["actionTypeId"],
        action_type_json(&action_type),
        "case \"a queued job\": expected the polled action type, got {job}"
    );
    assert_eq!(
        job["data"]["actionConfiguration"]["configuration"],
        json!({"Command": "build", "Timeout": "5"}),
        "case \"a queued job\": expected the action's declared configuration, got {job}"
    );
    let context = &job["data"]["pipelineContext"];
    assert_eq!(
        (
            context["pipelineName"].as_str(),
            context["stage"]["name"].as_str(),
            context["action"]["name"].as_str(),
            context["pipelineExecutionId"].as_str(),
        ),
        (
            Some(pipeline),
            Some("Work"),
            Some("Custom"),
            Some(execution.as_str()),
        ),
        "case \"a queued job\": expected the pipeline context of the action that queued it, \
         got {context}"
    );
    assert_eq!(
        context["pipelineArn"].as_str(),
        Some(format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:{pipeline}").as_str()),
        "case \"a queued job\": expected the pipeline's ARN, got {context}"
    );
    assert!(
        context["action"]["actionExecutionId"].as_str().is_some(),
        "case \"a queued job\": expected the action execution's id, got {context}"
    );

    // The artifact the worker reads is the one the source action produced, at the
    // location it produced it; the one it is to write is in the same artifact store.
    let input = &job["data"]["inputArtifacts"][0];
    assert_eq!(
        (
            input["name"].as_str(),
            input["location"]["type"].as_str(),
            input["location"]["s3Location"]["bucketName"].as_str(),
        ),
        (
            Some("SourceArtifact"),
            Some("S3"),
            Some(artifact_bucket(pipeline).as_str()),
        ),
        "case \"a queued job\": expected the produced input artifact, got {job}"
    );
    let output = &job["data"]["outputArtifacts"][0];
    assert_eq!(
        (
            output["name"].as_str(),
            output["location"]["s3Location"]["bucketName"].as_str(),
        ),
        (Some("BuildArtifact"), Some(artifact_bucket(pipeline).as_str())),
        "case \"a queued job\": expected the artifact the job is to produce, got {job}"
    );
    // The worker is handed the credentials it reads and writes those artifacts with:
    // minted by the dispatch, never chosen by this service, and all three members the
    // model requires of a session. Only the MEMBER is named in the failure text — a
    // vended secret is not printed.
    let credentials = &job["data"]["artifactCredentials"];
    for member in ["accessKeyId", "secretAccessKey", "sessionToken"] {
        assert!(
            credentials[member]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "case \"a queued job\": expected a non-empty artifactCredentials.{member} on a \
             job that moves artifacts, got {:?}",
            credentials[member].as_str().map(str::len)
        );
    }

    // Read back: the job the poll reported is the job the store holds, reached through
    // `GetJobDetails` by the id the worker was given. `JobDetails` carries no nonce —
    // that is the claim one poll granted, not a property of the job — so the rest of
    // the job must be identical.
    let details = emulator
        .ok("GetJobDetails", &json!({"jobId": id}))
        .await;
    assert_eq!(
        (
            details["jobDetails"]["id"].as_str(),
            details["jobDetails"]["accountId"].as_str(),
        ),
        (Some(id), Some(ACCOUNT_ID)),
        "case \"read back\": expected the polled job's id and account, got {details}"
    );
    assert_eq!(
        details["jobDetails"]["data"], job["data"],
        "case \"read back\": expected the same job data the poll reported, got {details}"
    );
    assert!(
        details["jobDetails"].get("nonce").is_none(),
        "case \"read back\": JobDetails models no nonce, got {details}"
    );

    // An id that names no job is refused, whatever its form.
    for (case, job_id) in [
        ("an id no job carries", uuid_like(99)),
        ("the polled id with one digit changed", flip_first_digit(id)),
    ] {
        let (status, response) = emulator.cp("GetJobDetails", &json!({"jobId": job_id})).await;
        let (code, message) = error_of("GetJobDetails", &response);
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "JobNotFoundException"),
            "case {case:?}: expected HTTP {REFUSAL_STATUS} JobNotFoundException, got {response}"
        );
        assert!(
            message.contains(&job_id),
            "case {case:?}: expected the message to name {job_id:?}, got {message:?}"
        );
    }

    // The action waits for the worker: nothing moved it, and the execution has not
    // settled behind its back.
    assert_eq!(
        emulator.action_status(pipeline, "Work", "Custom").await,
        Some("InProgress".to_string()),
        "case \"a queued job\": the custom action waits for its worker"
    );
}

/// Rule 4's second half, as the invariant it is: the job a poll reports is HELD for
/// that worker — a poll in the meantime is reported nothing — and once the claim lapses
/// the job is available again, handed out with a new nonce.
#[tokio::test]
async fn a_polled_job_is_held_until_its_claim_lapses() {
    let emulator = Emulator::new();
    let pipeline = "lease-pipe";
    let (action_type, _) =
        running_pipeline(&emulator, pipeline, json!([]), json!({"Command": "build"})).await;

    let first = emulator.poll_until_job(&action_type).await;
    let (id, first_nonce) = (
        first["id"].as_str().unwrap_or_default().to_string(),
        first["nonce"].as_str().unwrap_or_default().to_string(),
    );

    let (status, response) = emulator.poll(&action_type, None, None).await;
    assert_eq!(
        status, 200,
        "case \"a second poll inside the claim\": expected HTTP 200, got {status} with {response}"
    );
    assert!(
        reported_jobs(&response).is_empty(),
        "case \"a second poll inside the claim\": expected the job to be withheld, got {response}"
    );

    // Nothing sleeps in the handler: the claim lapses by the passage of time alone.
    tokio::time::sleep(JOB_POLL_LEASE + Duration::from_millis(100)).await;
    let (status, response) = emulator.poll(&action_type, None, None).await;
    assert_eq!(
        status, 200,
        "case \"a poll after the claim lapsed\": expected HTTP 200, got {status} with {response}"
    );
    let again = reported_jobs(&response);
    assert_eq!(
        again.iter().map(|job| job["id"].as_str()).collect::<Vec<_>>(),
        vec![Some(id.as_str())],
        "case \"a poll after the claim lapsed\": expected the same job again, got {response}"
    );
    let second_nonce = again[0]["nonce"].as_str().unwrap_or_default();
    assert_ne!(
        second_nonce, first_nonce,
        "case \"a poll after the claim lapsed\": expected a fresh nonce, got {second_nonce:?} \
         twice"
    );
}

/// RULE: a redelivery does not take the job away from the worker that was handed it
/// first. Every nonce a poll published for a job still WAITING to be claimed claims
/// it, whichever dispatch published it, and the acknowledgement is what closes the
/// set: once one of them has claimed the job, none of them claims it any more.
///
/// The rows differ along which of the outstanding dispatches acknowledges — the first,
/// a middle one, the newest — because the rule is about the whole set and not about
/// one position in it.
#[tokio::test]
async fn every_nonce_a_poll_published_claims_the_job_until_one_of_them_has() {
    /// How many times each row lets the job be re-dispatched without acknowledging it.
    const DISPATCHES: usize = 3;

    for claiming in 0..DISPATCHES {
        let case = format!("the nonce of dispatch {} claims the job", claiming + 1);
        let emulator = Emulator::new();
        let pipeline = format!("nonce-pipe-{claiming}");
        let (action_type, _) =
            running_pipeline(&emulator, &pipeline, json!([]), json!({"Command": "build"})).await;

        // The same job, dispatched `DISPATCHES` times: nothing acknowledges it, so
        // each lapsed claim makes it available again with a nonce of its own.
        let first = emulator.poll_until_job(&action_type).await;
        let job_id = first["id"].as_str().unwrap_or_default().to_string();
        let mut nonces = vec![first["nonce"].as_str().unwrap_or_default().to_string()];
        while nonces.len() < DISPATCHES {
            // Nothing sleeps in the handler: the claim lapses by the passage of time.
            tokio::time::sleep(JOB_POLL_LEASE + Duration::from_millis(100)).await;
            let (status, response) = emulator.poll(&action_type, None, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from a poll, got {status} with {response}"
            );
            let jobs = reported_jobs(&response);
            assert_eq!(
                jobs.iter().map(|job| job["id"].as_str()).collect::<Vec<_>>(),
                vec![Some(job_id.as_str())],
                "case {case:?}: an unacknowledged job is dispatched again, got {response}"
            );
            nonces.push(jobs[0]["nonce"].as_str().unwrap_or_default().to_string());
        }
        let distinct: BTreeSet<&String> = nonces.iter().collect();
        assert_eq!(
            distinct.len(),
            nonces.len(),
            "case {case:?}: each dispatch publishes its own nonce, got {nonces:?}"
        );

        // The chosen dispatch's nonce claims the job, whichever one it is.
        let acknowledged = emulator
            .ok(
                "AcknowledgeJob",
                &json!({"jobId": job_id, "nonce": nonces[claiming]}),
            )
            .await;
        assert_eq!(
            acknowledged["status"],
            json!("InProgress"),
            "case {case:?}: the acknowledgement reports the status the job now carries, \
             got {acknowledged}"
        );

        // And the claim closes the set: every nonce the job ever published — the one
        // that claimed it included — and any nonce it never published are refused with
        // the error the operation models, worded over the job the nonce was presented
        // for.
        let refused: Vec<(String, String)> = nonces
            .iter()
            .enumerate()
            .map(|(index, nonce)| (format!("dispatch {}", index + 1), nonce.clone()))
            .chain(std::iter::once((
                "a nonce no dispatch published".to_string(),
                flip_first_digit(&nonces[claiming]),
            )))
            .collect();
        for (which, nonce) in refused {
            let (status, response) = emulator
                .cp("AcknowledgeJob", &json!({"jobId": job_id, "nonce": nonce}))
                .await;
            let (code, message) = error_of("AcknowledgeJob", &response);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidNonceException"),
                "case {case:?}, {which}: a claimed job is claimed by nobody else, got \
                 {status} {response}"
            );
            assert_eq!(
                message,
                format!("Invalid nonce for job id {job_id}"),
                "case {case:?}, {which}: unexpected refusal wording"
            );
        }
    }
}

/// Rule 4's cap, exhaustively over how many jobs are waiting and how many the caller
/// asked for: a poll is handed the first `min(queued, maxBatchSize)` of them, oldest
/// first, and an absent `maxBatchSize` asks for every one.
#[tokio::test]
async fn a_poll_is_capped_by_max_batch_size() {
    // `MaxBatchSize` is modelled `range 1..`, so 1 is the smallest value a request can
    // carry; one past the queue length covers "asked for more than there is".
    let batch_sizes: [Option<i32>; 6] = [None, Some(1), Some(2), Some(3), Some(4), Some(5)];

    for queued_count in 0..=4usize {
        for max_batch_size in batch_sizes {
            let case = format!("{queued_count} queued, maxBatchSize {max_batch_size:?}");
            let emulator = Emulator::new();
            let action_type = emulator.register("Build", "MyWorker", json!([])).await;
            let configurations = vec![Vec::new(); queued_count];
            let queued = emulator.queue_jobs(&action_type, &configurations);

            let (status, response) = emulator.poll(&action_type, max_batch_size, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );
            let limit = max_batch_size.map_or(queued_count, |size| size as usize);
            let expected: Vec<String> = queued
                .iter()
                .take(limit.min(queued_count))
                .cloned()
                .collect();
            let actual = reported_ids(&response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the first {} queued jobs oldest first, got {actual:?}",
                expected.len()
            );
            // An empty poll reports an empty LIST, not an absent member.
            if expected.is_empty() {
                assert_eq!(
                    response.get("jobs"),
                    Some(&json!([])),
                    "case {case:?}: expected an empty jobs list, got {response}"
                );
            }
        }
    }
}

/// Rule 3, over the class of polls a registered action type's queryable property
/// accepts and refuses: the property it declares (or declares none), what the poll
/// supplies, and which of the waiting jobs that selects.
#[tokio::test]
async fn query_param_must_agree_with_the_action_types_queryable_property() {
    /// (case, the action type's configuration properties, the queryParam polled with,
    /// either the job ids expected or the refusal expected)
    type Row = (
        &'static str,
        Value,
        Option<(&'static str, &'static str)>,
        Result<Vec<usize>, &'static str>,
    );
    // The jobs seeded for every row: two carry `alpha` and one carries `beta`.
    let configurations = [
        vec![("Project", "alpha")],
        vec![("Project", "beta")],
        vec![("Project", "alpha")],
    ];
    let rows: [Row; 7] = [
        (
            "no queryable property, and no queryParam",
            json!([property("Plain", false)]),
            None,
            Ok(vec![0, 1, 2]),
        ),
        (
            "no configuration property at all, and no queryParam",
            json!([]),
            None,
            Ok(vec![0, 1, 2]),
        ),
        (
            "no queryable property, but a queryParam",
            json!([property("Plain", false)]),
            Some(("Plain", "alpha")),
            Err(VALIDATION),
        ),
        (
            "a queryable property, and no queryParam",
            json!([property("Project", true)]),
            None,
            Err(VALIDATION),
        ),
        (
            "a queryable property, but another key",
            json!([property("Project", true)]),
            Some(("Other", "alpha")),
            Err(VALIDATION),
        ),
        (
            "a queryable property, matched by two jobs",
            json!([property("Project", true)]),
            Some(("Project", "alpha")),
            Ok(vec![0, 2]),
        ),
        (
            "a queryable property, matched by one job",
            json!([property("Project", true)]),
            Some(("Project", "beta")),
            Ok(vec![1]),
        ),
    ];

    for (case, properties, query_param, expected) in rows {
        let emulator = Emulator::new();
        let action_type = emulator.register("Build", "MyWorker", properties).await;
        let queued = emulator.queue_jobs(&action_type, &configurations);

        let (status, response) = emulator.poll(&action_type, None, query_param).await;
        match expected {
            Ok(selected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {response}"
                );
                let expected_ids: Vec<String> =
                    selected.iter().map(|index| queued[*index].clone()).collect();
                let actual = reported_ids(&response);
                assert_eq!(
                    actual, expected_ids,
                    "case {case:?}: expected the jobs {expected_ids:?}, got {actual:?}"
                );
            }
            Err(expected_code) => {
                let (code, message) = error_of(POLL, &response);
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with {response}"
                );
                assert_eq!(
                    code, expected_code,
                    "case {case:?}: expected error code {expected_code}, got {code:?} with \
                     {message:?}"
                );
                assert!(
                    message.contains("queryParam"),
                    "case {case:?}: expected the message to name the member it is about, got \
                     {message:?}"
                );
            }
        }
    }
}

/// A poll hands out work that is still OWED: an action abandoned by a stop is not
/// waiting for a worker any more, so its job is not handed out — and the poll that
/// finds that out reports no job rather than failing.
#[tokio::test]
async fn a_job_whose_action_was_abandoned_is_not_handed_out() {
    let emulator = Emulator::new();
    let pipeline = "stopped-job-pipe";
    let (action_type, execution) =
        running_pipeline(&emulator, pipeline, json!([]), json!({"Command": "build"})).await;

    // Reached WITHOUT polling, so the job is queued and unclaimed.
    emulator
        .wait_for_action_in_progress(pipeline, "Work", "Custom")
        .await;
    let queued: Vec<String> = emulator
        .store()
        .lock()
        .jobs
        .values()
        .flatten()
        .map(|job| job.job_id.clone())
        .collect();
    assert_eq!(
        queued.len(),
        1,
        "case \"an abandoned action\": the custom action must have queued its job, got \
         {queued:?}"
    );
    // It is a job while the action waits: the reader finds it.
    emulator
        .ok("GetJobDetails", &json!({"jobId": queued[0]}))
        .await;

    emulator
        .ok(
            "StopPipelineExecution",
            &json!({
                "pipelineName": pipeline,
                "pipelineExecutionId": execution,
                "abandon": true,
                "reason": "no longer needed",
            }),
        )
        .await;

    let (status, response) = emulator.poll(&action_type, None, None).await;
    assert_eq!(
        status, 200,
        "case \"an abandoned action\": expected HTTP 200, got {status} with {response}"
    );
    assert!(
        reported_jobs(&response).is_empty(),
        "case \"an abandoned action\": expected no job to be handed out, got {response}"
    );
    let left = emulator.store().lock().jobs.values().flatten().count();
    assert_eq!(
        left, 0,
        "case \"an abandoned action\": the job is forgotten with the action, got {left} queued"
    );

    // And the same fact through the reader: the job is gone, not merely unpollable.
    let (status, response) = emulator
        .cp("GetJobDetails", &json!({"jobId": queued[0]}))
        .await;
    let (code, _) = error_of("GetJobDetails", &response);
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "JobNotFoundException"),
        "case \"an abandoned action\": expected the job to be gone, got {response}"
    );
}

/// RULE: handing a job to a worker is what mints the temporary credentials that worker
/// reads and writes the pipeline's artifacts with — for a job that carries artifacts at
/// all — and every later read of the job reports the credentials that dispatch published.
///
/// The rows differ along what the rule generalises over: whether the job's action carries
/// artifacts (both lists, one of them, neither), which is what decides whether the worker
/// needs credentials for the artifact store.
#[tokio::test]
async fn a_dispatched_job_carries_the_artifact_credentials_its_worker_needs() {
    /// The three members `AWSSessionCredentials` requires, as the model names them.
    const SESSION_MEMBERS: [&str; 3] = ["accessKeyId", "secretAccessKey", "sessionToken"];

    // (case, the custom action's inputArtifacts, its outputArtifacts, credentials expected)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>, bool)> = vec![
        (
            "an action that reads and writes an artifact",
            vec!["SourceArtifact"],
            vec!["BuildArtifact"],
            true,
        ),
        (
            "an action that only reads one",
            vec!["SourceArtifact"],
            vec![],
            true,
        ),
        (
            "an action that only writes one",
            vec![],
            vec!["BuildArtifact"],
            true,
        ),
        ("an action that touches none", vec![], vec![], false),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, inputs, outputs, expected) in rows {
        let emulator = Emulator::new();
        let pipeline = "job-credentials";
        emulator.bucket(&artifact_bucket(pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"job bytes")
            .await;
        let action_type = emulator.register("Build", "MyWorker", json!([])).await;
        let mut declaration = custom_action_pipeline(
            pipeline,
            &action_type,
            json!({"Command": "build"}),
        );
        let artifacts = |names: &[&str]| -> Value {
            Value::Array(names.iter().map(|name| json!({"name": name})).collect())
        };
        declaration["pipeline"]["stages"][1]["actions"][0]["inputArtifacts"] =
            artifacts(&inputs);
        declaration["pipeline"]["stages"][1]["actions"][0]["outputArtifacts"] =
            artifacts(&outputs);
        emulator.ok("CreatePipeline", &declaration).await;

        let job = emulator.poll_until_job(&action_type).await;
        let id = job["id"].as_str().unwrap_or_default().to_string();
        let credentials = job["data"].get("artifactCredentials").cloned();
        if !expected {
            assert!(
                credentials.is_none(),
                "case {case:?}: a job whose action touches no artifact needs no \
                 credentials, got {job}"
            );
            continue;
        }
        let credentials = credentials.unwrap_or_else(|| {
            panic!("case {case:?}: expected artifactCredentials on the job, got {job}")
        });
        for member in SESSION_MEMBERS {
            assert!(
                credentials[member]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected a non-empty {member}, got {credentials}"
            );
        }
        // The credential is this dispatch's own: no two jobs are handed the same one.
        let access_key_id = credentials["accessKeyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            minted.insert(access_key_id.clone()),
            "case {case:?}: expected a credential of its own, got {access_key_id:?} again"
        );

        // Read back: the job the store holds reports the credentials THIS dispatch
        // published, not a fresh set per read.
        for attempt in 0..2 {
            let details = emulator.ok("GetJobDetails", &json!({"jobId": id})).await;
            assert_eq!(
                details["jobDetails"]["data"]["artifactCredentials"], credentials,
                "case {case:?}: read {attempt} must report the dispatched credentials, \
                 got {details}"
            );
        }

        // A job's credentials are minted ONCE: once its claim lapses and the job is
        // dispatched again, the worker is handed the same credentials and the store holds
        // no second one for it.
        tokio::time::sleep(JOB_POLL_LEASE + Duration::from_millis(100)).await;
        let again = emulator.poll_until_job(&action_type).await;
        assert_eq!(
            again["data"]["artifactCredentials"], credentials,
            "case {case:?}: a re-dispatched job keeps its credentials, got {again}"
        );
        let held = emulator.store().lock().artifact_credential_count();
        assert_eq!(
            held, 1,
            "case {case:?}: expected one minted credential for one job, got {held}"
        );

        // The release half of that bound: a credential whose life the clock has ended is
        // answered to the caller that holds the credential store — once, and the job stops
        // carrying it, so the store's entries stay tied to the jobs still in play. The
        // instant is computed, not slept for: the lifetime is an hour.
        let ended = SystemTime::now() + ARTIFACT_CREDENTIAL_LIFETIME + Duration::from_secs(1);
        let released = emulator.store().lock().due_artifact_credentials(ended);
        assert_eq!(
            released,
            vec![credentials["accessKeyId"].as_str().unwrap_or_default().to_string()],
            "case {case:?}: expected the due credential to be released, got {released:?}"
        );
        let after = emulator.store().lock().artifact_credential_count();
        assert_eq!(
            after, 0,
            "case {case:?}: a released credential is removed from the store, got {after}"
        );
        let again = emulator.store().lock().due_artifact_credentials(ended);
        assert!(
            again.is_empty(),
            "case {case:?}: a credential is released once, got {again:?}"
        );
        let details = emulator.ok("GetJobDetails", &json!({"jobId": id})).await;
        assert!(
            details["jobDetails"]["data"]
                .get("artifactCredentials")
                .is_none(),
            "case {case:?}: a job whose credentials were released carries none, got \
             {details}"
        );
    }
}
