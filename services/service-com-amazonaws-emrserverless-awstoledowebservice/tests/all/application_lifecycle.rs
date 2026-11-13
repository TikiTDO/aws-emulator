// SPDX-License-Identifier: CC0-1.0

//! The EMR Serverless application and job run lifecycle, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes. The rule
//! under test is the one the control plane states for its resources: an application and
//! a job run are whatever the create or submit stored, and every reporting operation
//! computes its answer from that record rather than from the request that asked.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::{
    DEFAULT_EXECUTION_TIMEOUT_MINUTES, JOB_RUN_STAGES, JOB_RUN_STAGE_TRANSITION,
    LIFECYCLE_TRANSITION, PAGE_LIMIT,
};
use service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService;

/// The region the test requests are signed for; it is the region every minted ARN
/// names.
const REGION: &str = "eu-west-1";

/// A SigV4 credential scope naming this service's signing name (`emr-serverless`,
/// from `smithy_query(query="service_info")`). It is what routes a request here;
/// request authentication is off, so the signature itself is never checked.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/emr-serverless/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every emulator ARN carries (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for a clock-owed transition to have fallen due.
pub(crate) fn wait_for_transition() {
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(250));
}

/// Long enough for EVERY stage the clock owes a submitted job run
/// (`JOB_RUN_STAGES`, one `JOB_RUN_STAGE_TRANSITION` apart) to have fallen due, so the
/// run has reached the terminal state the last of them names.
pub(crate) fn wait_for_job_run_to_settle() {
    let stages = u32::try_from(JOB_RUN_STAGES.len()).unwrap_or(1);
    std::thread::sleep(JOB_RUN_STAGE_TRANSITION * stages + Duration::from_millis(250));
}

/// One emulator: a router carrying only this service, over one shared engine, with the
/// engine handed back so a test can read or rewind the state the handlers share.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One emulator: a router carrying only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// What the emulator answered: the HTTP status, the `x-amzn-errortype` identity and
/// the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header, which is how restJson1 names the error shape a
    /// client branches on. `None` on a success, which carries no such header.
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        // restJson1 renders the error shape id in `__type`, which may be namespaced.
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or_default()
    }

    /// The error SHAPE a restJson1 client resolves from the `x-amzn-errortype`
    /// identity, or `None` when the answer carries no identity.
    ///
    /// The protocol admits three forms — `Shape`, `namespace#Shape` and
    /// `Shape:<namespace URI>` — and a client takes the name after the last `#` and
    /// before the first `:`, so this is exactly what a client's `catch` keys off.
    pub(crate) fn resolved_error_shape(&self) -> Option<&str> {
        self.error_type.as_deref().map(|identity| {
            let after_namespace = identity.rsplit('#').next().unwrap_or(identity);
            after_namespace.split(':').next().unwrap_or(after_namespace)
        })
    }

    /// The value at a JSON pointer, or `Value::Null`.
    pub(crate) fn at(&self, pointer: &str) -> Value {
        self.body.pointer(pointer).cloned().unwrap_or(Value::Null)
    }
}

/// One signed request, answered as the WHOLE HTTP response, so a suite can assert on the
/// status, the body BYTES and the response headers — the framing a parsed [`Answer`]
/// cannot show. [`call`] is this with the body parsed as JSON.
pub(crate) async fn call_http(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> ::http::Response<Bytes> {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "emr-serverless.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    router.dispatch(request).await
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let response = call_http(router, method, uri, body).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

pub(crate) async fn create_application(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/applications", Some(request)).await
}

pub(crate) async fn get_application(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/applications/{id}"), None).await
}

pub(crate) async fn list_applications(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/applications{query}"), None).await
}

pub(crate) async fn start_application(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "POST", &format!("/applications/{id}/start"), None).await
}

pub(crate) async fn stop_application(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "POST", &format!("/applications/{id}/stop"), None).await
}

pub(crate) async fn delete_application(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/applications/{id}"), None).await
}

pub(crate) async fn start_job_run(
    router: &MultiServiceRouter,
    id: &str,
    request: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/applications/{id}/jobruns"),
        Some(request),
    )
    .await
}

pub(crate) async fn get_job_run(
    router: &MultiServiceRouter,
    id: &str,
    job_run_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{id}/jobruns/{job_run_id}"),
        None,
    )
    .await
}

pub(crate) async fn list_job_runs(
    router: &MultiServiceRouter,
    id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{id}/jobruns{query}"),
        None,
    )
    .await
}

pub(crate) async fn cancel_job_run(
    router: &MultiServiceRouter,
    id: &str,
    job_run_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/applications/{id}/jobruns/{job_run_id}{query}"),
        None,
    )
    .await
}

/// `ApplicationId` / `JobRunId`'s modelled `@pattern` `^[0-9a-z]+$` with length 1..64,
/// checked by hand (this crate has no regex dependency) rather than through the crate's
/// own predicate, so a wrong predicate in `src/` cannot make this test pass. Source:
/// `smithy_query(query="shape",shape="Application")`.
pub(crate) fn matches_resource_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// `ApplicationArn`'s modelled `@pattern`
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+$`
/// with length 60..1024.
fn matches_application_arn(arn: &str, region: &str) -> bool {
    let expected_prefix = format!("arn:aws:emr-serverless:{region}:{ACCOUNT_ID}:/applications/");
    let Some(id) = arn.strip_prefix(&expected_prefix) else {
        return false;
    };
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric())
        && (60..=1024).contains(&arn.len())
}

/// `JobArn`'s modelled `@pattern`: an application ARN followed by
/// `/jobruns/<jobRunId>`.
fn matches_job_run_arn(arn: &str, region: &str) -> bool {
    let Some((application_part, job_run_id)) = arn.rsplit_once("/jobruns/") else {
        return false;
    };
    matches_application_arn(application_part, region)
        && !job_run_id.is_empty()
        && job_run_id.chars().all(|c| c.is_ascii_alphanumeric())
        && (60..=1024).contains(&arn.len())
}

/// `RequestIdentityUserArn`'s modelled `@pattern`
/// `^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$` with length 20..2048.
pub(crate) fn matches_request_identity_arn(arn: &str) -> bool {
    let Some(rest) = arn
        .strip_prefix("arn:aws:iam::")
        .or_else(|| arn.strip_prefix("arn:aws:sts::"))
    else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let account_ok = account.is_empty()
        || (account.len() == 12 && account.chars().all(|c| c.is_ascii_digit()));
    account_ok
        && !resource.is_empty()
        && resource.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '+' | '=' | ',' | '.' | '@' | '-')
        })
        && (20..=2048).contains(&arn.len())
}

/// `NextToken`'s modelled `@pattern` `^[A-Za-z0-9_=-]+$` with length 1..1024.
fn matches_next_token(token: &str) -> bool {
    (1..=1024).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '=' | '-'))
}

/// A minimal `CreateApplication` body: only the three required members, with the
/// client token the caller names.
pub(crate) fn minimal_create(token: &str) -> Value {
    json!({
        "clientToken": token,
        "releaseLabel": "emr-7.1.0",
        "type": "SPARK",
    })
}

/// A `sparkSubmit` job driver (the `JobDriver` union's `sparkSubmit` member).
fn spark_submit_driver(entry_point: &str) -> Value {
    json!({ "sparkSubmit": { "entryPoint": entry_point } })
}

/// An execution role ARN satisfying `IAMRoleArn`'s pattern.
pub(crate) const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/EMRServerlessRole";

/// A minimal `StartJobRun` body.
pub(crate) fn minimal_job_run(token: &str) -> Value {
    json!({
        "clientToken": token,
        "executionRoleArn": EXECUTION_ROLE,
        "jobDriver": spark_submit_driver("s3://bucket/app.py"),
    })
}

/// Create an application and return its id, waiting out the create transition so the
/// application is `CREATED`.
pub(crate) async fn created_application(router: &MultiServiceRouter, token: &str) -> String {
    let answer = create_application(router, minimal_create(token)).await;
    assert_eq!(
        answer.status, 200,
        "case {token:?}: CreateApplication expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer.at("/applicationId").as_str().unwrap_or("").to_string();
    wait_for_transition();
    id
}

/// Create an application and bring it to `STARTED`.
pub(crate) async fn started_application(router: &MultiServiceRouter, token: &str) -> String {
    let id = created_application(router, token).await;
    let started = start_application(router, &id).await;
    assert_eq!(
        started.status, 200,
        "case {token:?}: StartApplication expected 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_transition();
    id
}

/// The whole lifecycle the issue asks for, in order, with every answer computed from
/// the persisted record.
#[tokio::test]
async fn the_application_and_job_run_lifecycle_round_trips_through_state() {
    let router = emulator();

    let created = create_application(
        &router,
        json!({
            "clientToken": "tok-lifecycle",
            "releaseLabel": "emr-7.1.0",
            "type": "SPARK",
            "name": "demo",
            "tags": { "team": "data" },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateApplication expected 200, got {} with {:?}",
        created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        matches_resource_id(&application_id),
        "case lifecycle: applicationId must match ^[0-9a-z]+$, got {application_id:?}"
    );
    let application_arn = created.at("/arn").as_str().unwrap_or_default().to_string();
    assert!(
        matches_application_arn(&application_arn, REGION),
        "case lifecycle: arn must match ApplicationArn's pattern, got {application_arn:?}"
    );
    assert_eq!(
        created.at("/name"),
        json!("demo"),
        "case lifecycle: CreateApplication returns the stored name, got {:?}",
        created.at("/name")
    );

    wait_for_transition();

    let fetched = get_application(&router, &application_id).await;
    assert_eq!(
        fetched.status, 200,
        "GetApplication expected 200, got {} with {:?}",
        fetched.status, fetched.body
    );
    for (member, expected) in [
        ("/application/applicationId", json!(application_id)),
        ("/application/arn", json!(application_arn)),
        ("/application/name", json!("demo")),
        ("/application/releaseLabel", json!("emr-7.1.0")),
        // The engine type is reported in the service's canonical casing, not the
        // casing the create asked in.
        ("/application/type", json!("Spark")),
        ("/application/state", json!("CREATED")),
        ("/application/tags/team", json!("data")),
    ] {
        assert_eq!(
            fetched.at(member),
            expected,
            "case lifecycle: GetApplication{member} expected {expected:?}, got {:?}",
            fetched.at(member)
        );
    }

    let started = start_application(&router, &application_id).await;
    assert_eq!(
        started.status, 200,
        "StartApplication expected 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_transition();
    let after_start = get_application(&router, &application_id).await;
    assert_eq!(
        after_start.at("/application/state"),
        json!("STARTED"),
        "case lifecycle: the started application reports STARTED, got {:?}",
        after_start.at("/application/state")
    );
    assert!(
        after_start.at("/application/updatedAt").as_f64()
            >= fetched.at("/application/updatedAt").as_f64(),
        "case lifecycle: updatedAt must not go backwards, got {:?} after {:?}",
        after_start.at("/application/updatedAt"),
        fetched.at("/application/updatedAt")
    );

    let submitted = start_job_run(
        &router,
        &application_id,
        json!({
            "clientToken": "job-1",
            "executionRoleArn": EXECUTION_ROLE,
            "jobDriver": spark_submit_driver("s3://bucket/app.py"),
            "name": "nightly",
            "mode": "BATCH",
        }),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        matches_resource_id(&job_run_id),
        "case lifecycle: jobRunId must match ^[0-9a-z]+$, got {job_run_id:?}"
    );
    let job_run_arn = submitted.at("/arn").as_str().unwrap_or_default().to_string();
    assert!(
        matches_job_run_arn(&job_run_arn, REGION),
        "case lifecycle: arn must match JobArn's pattern, got {job_run_arn:?}"
    );
    assert_eq!(
        submitted.at("/applicationId"),
        json!(application_id),
        "case lifecycle: StartJobRun reports its own application, got {:?}",
        submitted.at("/applicationId")
    );

    let run = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        run.status, 200,
        "GetJobRun expected 200, got {} with {:?}",
        run.status, run.body
    );
    for (member, expected) in [
        ("/jobRun/applicationId", json!(application_id)),
        ("/jobRun/jobRunId", json!(job_run_id)),
        ("/jobRun/arn", json!(job_run_arn)),
        ("/jobRun/name", json!("nightly")),
        ("/jobRun/mode", json!("BATCH")),
        ("/jobRun/executionRole", json!(EXECUTION_ROLE)),
        // The release label is the APPLICATION's, never the request's.
        ("/jobRun/releaseLabel", json!("emr-7.1.0")),
        ("/jobRun/attempt", json!(1)),
        (
            "/jobRun/jobDriver/sparkSubmit/entryPoint",
            json!("s3://bucket/app.py"),
        ),
    ] {
        assert_eq!(
            run.at(member),
            expected,
            "case lifecycle: GetJobRun{member} expected {expected:?}, got {:?}",
            run.at(member)
        );
    }
    let created_by = run.at("/jobRun/createdBy");
    assert!(
        matches_request_identity_arn(created_by.as_str().unwrap_or_default()),
        "case lifecycle: createdBy must match RequestIdentityUserArn's pattern, got {created_by:?}"
    );

    let listed_runs = list_job_runs(&router, &application_id, "").await;
    assert_eq!(
        listed_runs.at("/jobRuns/0/id"),
        json!(job_run_id),
        "case lifecycle: ListJobRuns reports the submitted run, got {:?}",
        listed_runs.body
    );
    assert_eq!(
        listed_runs.at("/jobRuns/0/releaseLabel"),
        json!("emr-7.1.0"),
        "case lifecycle: the summary carries the stored release label, got {:?}",
        listed_runs.at("/jobRuns/0/releaseLabel")
    );

    let cancelled = cancel_job_run(&router, &application_id, &job_run_id, "").await;
    assert_eq!(
        cancelled.status, 200,
        "CancelJobRun expected 200, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        cancelled.at("/jobRunId"),
        json!(job_run_id),
        "case lifecycle: CancelJobRun names the run it cancelled, got {:?}",
        cancelled.body
    );
    wait_for_transition();
    let after_cancel = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        after_cancel.at("/jobRun/state"),
        json!("CANCELLED"),
        "case lifecycle: the cancelled run settles to CANCELLED, got {:?}",
        after_cancel.at("/jobRun/state")
    );

    let stopped = stop_application(&router, &application_id).await;
    assert_eq!(
        stopped.status, 200,
        "StopApplication expected 200, got {} with {:?}",
        stopped.status, stopped.body
    );
    wait_for_transition();
    let after_stop = get_application(&router, &application_id).await;
    assert_eq!(
        after_stop.at("/application/state"),
        json!("STOPPED"),
        "case lifecycle: the stopped application settles to STOPPED, got {:?}",
        after_stop.at("/application/state")
    );

    let deleted = delete_application(&router, &application_id).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteApplication expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    // The deleted application is RETAINED in the terminal state of the
    // `ApplicationState` enum, so a client polling it observes `TERMINATED` instead of
    // the state it last had.
    let gone = get_application(&router, &application_id).await;
    assert_eq!(
        gone.status, 200,
        "case lifecycle: GetApplication after the delete expected 200, got {} with {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.at("/application/state"),
        json!("TERMINATED"),
        "case lifecycle: the deleted application reports TERMINATED, got {:?}",
        gone.at("/application/state")
    );
    let remaining = list_applications(&router, "?states=TERMINATED").await;
    assert_eq!(
        remaining.at("/applications/0/id"),
        json!(application_id),
        "case lifecycle: the terminated application is listed under its state, got {:?}",
        remaining.body
    );
}

/// Whatever a create stores, a later read reports unchanged — across values, sizes,
/// types, counts and forms of the optional members.
#[tokio::test]
async fn get_application_reports_exactly_what_the_create_stored() {
    // The `type` values are `EngineType` (a free string, len 1..64) and the two
    // engines EMR Serverless documents; the `architecture` values are the whole
    // `Architecture` enum (`ARCHITECTURE_VALUES`); the capacity strings satisfy
    // `CpuSize` / `MemorySize` / `DiskSize`'s patterns.
    let cases: Vec<(&str, Value)> = vec![
        ("only the required members", minimal_create("tok-minimal")),
        (
            "a name and one tag",
            json!({
                "clientToken": "tok-named",
                "releaseLabel": "emr-6.15.0",
                "type": "HIVE",
                "name": "etl.nightly",
                "tags": { "team": "data" },
            }),
        ),
        (
            "three tags and the ARM64 architecture",
            json!({
                "clientToken": "tok-arm",
                "releaseLabel": "emr-7.2.0",
                "type": "SPARK",
                "architecture": "ARM64",
                "tags": { "a": "1", "b": "2", "c": "3" },
            }),
        ),
        (
            "the X86_64 architecture and a maximum capacity",
            json!({
                "clientToken": "tok-x86",
                "releaseLabel": "emr-7.0.0",
                "type": "SPARK",
                "architecture": "X86_64",
                "maximumCapacity": { "cpu": "400vCPU", "memory": "3000GB", "disk": "20000GB" },
            }),
        ),
        (
            "two initial capacity entries with worker configurations",
            json!({
                "clientToken": "tok-capacity",
                "releaseLabel": "emr-7.1.0",
                "type": "SPARK",
                "initialCapacity": {
                    "Driver": {
                        "workerCount": 1,
                        "workerConfiguration": { "cpu": "2vCPU", "memory": "4GB" },
                    },
                    "Executor": {
                        "workerCount": 4,
                        "workerConfiguration": {
                            "cpu": "4vCPU",
                            "memory": "8GB",
                            "disk": "20GB",
                            "diskType": "SHUFFLE_OPTIMIZED",
                        },
                    },
                },
            }),
        ),
        (
            "auto start and auto stop given explicitly",
            json!({
                "clientToken": "tok-auto",
                "releaseLabel": "emr-7.1.0",
                "type": "SPARK",
                "autoStartConfiguration": { "enabled": false },
                "autoStopConfiguration": { "enabled": false, "idleTimeoutMinutes": 120 },
            }),
        ),
        (
            "a runtime configuration list and a network configuration",
            json!({
                "clientToken": "tok-runtime",
                "releaseLabel": "emr-7.1.0",
                "type": "SPARK",
                "runtimeConfiguration": [
                    { "classification": "spark-defaults", "properties": { "spark.executor.cores": "2" } },
                ],
                "networkConfiguration": {
                    "subnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                    "securityGroupIds": ["sg-0a1b2c3d"],
                },
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in cases {
        let created = create_application(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateApplication expected 200, got {} with {:?}",
            created.status, created.body
        );
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            matches_resource_id(&id),
            "case {case:?}: applicationId must match ^[0-9a-z]+$, got {id:?}"
        );

        let fetched = get_application(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetApplication expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );

        // Every member the request named reads back identical.
        let members = request.as_object().expect("the request is an object");
        for (member, expected) in members {
            if member == "clientToken" {
                // `clientToken` is an idempotency key, not a member of `Application`.
                continue;
            }
            let pointer = format!("/application/{member}");
            if member == "autoStartConfiguration" || member == "autoStopConfiguration" {
                // Checked below together with their documented defaults.
                continue;
            }
            if member == "type" {
                // The engine type reads back in the service's canonical casing, which
                // `an_engine_type_reads_back_in_the_services_own_casing` pins; here it
                // only has to name the same engine.
                let actual = fetched.at(&pointer);
                assert!(
                    actual.as_str().is_some_and(|reported| reported
                        .eq_ignore_ascii_case(expected.as_str().unwrap_or_default())),
                    "case {case:?}: GetApplication{pointer} expected the engine \
                     {expected:?} named, got {actual:?}"
                );
                continue;
            }
            assert_eq!(
                &fetched.at(&pointer),
                expected,
                "case {case:?}: GetApplication{pointer} expected {expected:?}, got {:?}",
                fetched.at(&pointer)
            );
        }

        // `AutoStartConfig$enabled` ("Defaults to true"),
        // `AutoStopConfig$enabled` ("Defaults to true") and
        // `AutoStopConfig$idleTimeoutMinutes` ("Defaults to 15 minutes") are reported
        // with the values the request gave, or with those documented defaults.
        let expected_auto_start = members
            .get("autoStartConfiguration")
            .and_then(|config| config.get("enabled"))
            .cloned()
            .unwrap_or(json!(true));
        assert_eq!(
            fetched.at("/application/autoStartConfiguration/enabled"),
            expected_auto_start,
            "case {case:?}: autoStartConfiguration.enabled expected {expected_auto_start:?}, got {:?}",
            fetched.at("/application/autoStartConfiguration/enabled")
        );
        let expected_auto_stop = members
            .get("autoStopConfiguration")
            .and_then(|config| config.get("enabled"))
            .cloned()
            .unwrap_or(json!(true));
        assert_eq!(
            fetched.at("/application/autoStopConfiguration/enabled"),
            expected_auto_stop,
            "case {case:?}: autoStopConfiguration.enabled expected {expected_auto_stop:?}, got {:?}",
            fetched.at("/application/autoStopConfiguration/enabled")
        );
        let expected_idle = members
            .get("autoStopConfiguration")
            .and_then(|config| config.get("idleTimeoutMinutes"))
            .cloned()
            .unwrap_or(json!(15));
        assert_eq!(
            fetched.at("/application/autoStopConfiguration/idleTimeoutMinutes"),
            expected_idle,
            "case {case:?}: autoStopConfiguration.idleTimeoutMinutes expected {expected_idle:?}, got {:?}",
            fetched.at("/application/autoStopConfiguration/idleTimeoutMinutes")
        );

        // And the summary `ListApplications` serves projects the same record.
        let listed = list_applications(&router, "").await;
        let summary = listed
            .body
            .get("applications")
            .and_then(Value::as_array)
            .expect("ListApplications returns an applications list")
            .iter()
            .find(|entry| entry["id"] == json!(id))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the application must be listed"));
        for (summary_member, application_member) in [
            ("arn", "arn"),
            ("name", "name"),
            ("releaseLabel", "releaseLabel"),
            ("type", "type"),
            ("state", "state"),
            ("architecture", "architecture"),
            ("createdAt", "createdAt"),
            ("updatedAt", "updatedAt"),
        ] {
            let expected = fetched.at(&format!("/application/{application_member}"));
            let actual = summary
                .get(summary_member)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: the summary's {summary_member} must equal the \
                 application's {application_member} ({expected:?}), got {actual:?}"
            );
        }
    }
}

/// A created application is not usable the instant the create returns: it reports the
/// transient state to a caller acting straight away, and the clock owes it the settled
/// one without any further call. The same holds for a start and for a stop.
#[tokio::test]
async fn every_lifecycle_call_reports_its_transient_state_until_the_clock_settles_it() {
    let router = emulator();

    // (case, the transient state, the settled state, how to start the transition)
    let started_at = Instant::now();
    let created = create_application(&router, minimal_create("tok-transient")).await;
    let id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let immediately = get_application(&router, &id).await;
    if started_at.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            immediately.at("/application/state"),
            json!("CREATING"),
            "case create: a caller acting inside the transition span sees CREATING, got {:?}",
            immediately.at("/application/state")
        );
    }
    wait_for_transition();
    assert_eq!(
        get_application(&router, &id).await.at("/application/state"),
        json!("CREATED"),
        "case create: the clock owes the application CREATED"
    );

    let started_at = Instant::now();
    start_application(&router, &id).await;
    let immediately = get_application(&router, &id).await;
    if started_at.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            immediately.at("/application/state"),
            json!("STARTING"),
            "case start: a caller acting inside the transition span sees STARTING, got {:?}",
            immediately.at("/application/state")
        );
    }
    wait_for_transition();
    assert_eq!(
        get_application(&router, &id).await.at("/application/state"),
        json!("STARTED"),
        "case start: the clock owes the application STARTED"
    );

    let started_at = Instant::now();
    stop_application(&router, &id).await;
    let immediately = get_application(&router, &id).await;
    if started_at.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            immediately.at("/application/state"),
            json!("STOPPING"),
            "case stop: a caller acting inside the transition span sees STOPPING, got {:?}",
            immediately.at("/application/state")
        );
    }
    wait_for_transition();
    assert_eq!(
        get_application(&router, &id).await.at("/application/state"),
        json!("STOPPED"),
        "case stop: the clock owes the application STOPPED"
    );
}

/// `clientToken` is `@idempotencyToken`: the same token names the same resource, a
/// different one names a new resource. Asserted for both operations that carry one.
#[tokio::test]
async fn the_same_client_token_names_the_same_resource() {
    let router = emulator();

    // Applications: repeating a token must not mint a second application.
    let tokens = ["tok-a", "tok-b", "tok-a", "tok.c_1-2", "tok-b"];
    let mut minted: Vec<String> = Vec::new();
    for token in tokens {
        let answer = create_application(&router, minimal_create(token)).await;
        assert_eq!(
            answer.status, 200,
            "case {token:?}: CreateApplication expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        minted.push(
            answer
                .at("/applicationId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_eq!(
        minted[0], minted[2],
        "case tok-a: the repeated token must name the first application, got {:?} then {:?}",
        minted[0], minted[2]
    );
    assert_eq!(
        minted[1], minted[4],
        "case tok-b: the repeated token must name the first application, got {:?} then {:?}",
        minted[1], minted[4]
    );
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        3,
        "three distinct tokens must have minted three applications, got {minted:?}"
    );
    let listed = list_applications(&router, "").await;
    assert_eq!(
        listed
            .body
            .get("applications")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3),
        "the store must hold one application per distinct token, got {:?}",
        listed.body
    );

    // Job runs: the same, scoped to the application the run belongs to.
    let application_id = started_application(&router, "tok-jobs").await;
    let job_tokens = ["job-a", "job-b", "job-a"];
    let mut job_ids: Vec<String> = Vec::new();
    for token in job_tokens {
        let answer = start_job_run(&router, &application_id, minimal_job_run(token)).await;
        assert_eq!(
            answer.status, 200,
            "case {token:?}: StartJobRun expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        job_ids.push(
            answer
                .at("/jobRunId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_eq!(
        job_ids[0], job_ids[2],
        "case job-a: the repeated token must name the first job run, got {:?} then {:?}",
        job_ids[0], job_ids[2]
    );
    assert_ne!(
        job_ids[0], job_ids[1],
        "case job-b: a different token must name a new job run, got {job_ids:?}"
    );
    let runs = list_job_runs(&router, &application_id, "").await;
    assert_eq!(
        runs.body
            .get("jobRuns")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2),
        "the application must hold one job run per distinct token, got {:?}",
        runs.body
    );
}

/// An id that names nothing is refused, with the status and code the model declares for
/// that operation — never a 200 with an empty body and never a 501.
#[tokio::test]
async fn an_id_that_names_nothing_is_refused() {
    let router = emulator();
    let application_id = started_application(&router, "tok-notfound").await;
    let submitted = start_job_run(&router, &application_id, minimal_job_run("job-1")).await;
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Every operation whose model declares `ResourceNotFoundException` (HTTP 404)
    // answers it for an unknown id.
    let unknown = "doesnotexist";
    for (case, method, uri) in [
        (
            "GetApplication",
            "GET",
            format!("/applications/{unknown}"),
        ),
        (
            "StartApplication",
            "POST",
            format!("/applications/{unknown}/start"),
        ),
        (
            "StopApplication",
            "POST",
            format!("/applications/{unknown}/stop"),
        ),
        (
            "DeleteApplication",
            "DELETE",
            format!("/applications/{unknown}"),
        ),
        (
            "GetJobRun on an unknown application",
            "GET",
            format!("/applications/{unknown}/jobruns/{job_run_id}"),
        ),
        (
            "GetJobRun on an unknown job run",
            "GET",
            format!("/applications/{application_id}/jobruns/{unknown}"),
        ),
        (
            "CancelJobRun on an unknown job run",
            "DELETE",
            format!("/applications/{application_id}/jobruns/{unknown}"),
        ),
    ] {
        let answer = call(&router, method, &uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // `StartJobRun` against an unknown application is the same refusal.
    let orphan = start_job_run(&router, unknown, minimal_job_run("job-orphan")).await;
    assert_eq!(
        orphan.status, 404,
        "case StartJobRun on an unknown application: expected 404, got {} with {:?}",
        orphan.status, orphan.body
    );
    assert_eq!(
        orphan.error_code(),
        "ResourceNotFoundException",
        "case StartJobRun on an unknown application: expected ResourceNotFoundException, got {:?}",
        orphan.body
    );

    // `ListJobRuns` declares no `ResourceNotFoundException`
    // (`smithy_query(query="operation",operation="ListJobRuns")` lists only
    // `InternalServerException` and `ValidationException`), so its refusal is the
    // modelled 400.
    let listed = list_job_runs(&router, unknown, "").await;
    assert_eq!(
        listed.status, 400,
        "case ListJobRuns on an unknown application: expected 400, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.error_code(),
        "ValidationException",
        "case ListJobRuns on an unknown application: expected ValidationException, got {:?}",
        listed.body
    );
}

/// `DeleteApplication` documents its precondition — "An application has to be in a
/// stopped or created state in order to be deleted" — judged against the state the
/// application has SETTLED on: it admits a transition on its way to `CREATED` or
/// `STOPPED` and refuses one on its way to `STARTED`. An accepted delete leaves the
/// application readable in the terminal `TERMINATED` state.
#[tokio::test]
async fn delete_application_admits_the_states_that_settle_on_the_ones_it_documents() {
    // One row per `ApplicationState` a client can put an application into: the state it
    // reports, the state it settles on and the status the delete must answer. Source:
    // the `ApplicationState` enum (`CREATING`, `CREATED`, `STARTING`, `STARTED`,
    // `STOPPING`, `STOPPED`, `TERMINATED`), every value of which is a row here.
    let cases = [
        ("CREATING", "CREATED", 200),
        ("CREATED", "CREATED", 200),
        ("STARTING", "STARTED", 400),
        ("STARTED", "STARTED", 400),
        ("STOPPING", "STOPPED", 200),
        ("STOPPED", "STOPPED", 200),
        ("TERMINATED", "TERMINATED", 400),
    ];

    for (state, settles_on, expected_status) in cases {
        let router = emulator();
        let created = create_application(&router, minimal_create("tok-delete")).await;
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        match state {
            "CREATING" => {}
            "CREATED" => wait_for_transition(),
            "STARTING" => {
                wait_for_transition();
                start_application(&router, &id).await;
            }
            "STARTED" => {
                wait_for_transition();
                start_application(&router, &id).await;
                wait_for_transition();
            }
            "STOPPING" => {
                wait_for_transition();
                stop_application(&router, &id).await;
            }
            "STOPPED" => {
                wait_for_transition();
                stop_application(&router, &id).await;
                wait_for_transition();
            }
            "TERMINATED" => {
                wait_for_transition();
                delete_application(&router, &id).await;
            }
            other => panic!("unhandled state {other}"),
        }

        let observed = get_application(&router, &id).await;
        assert_eq!(
            observed.at("/application/state"),
            json!(state),
            "case {state:?}: the application must be in that state before the delete, got {:?}",
            observed.at("/application/state")
        );

        let deleted = delete_application(&router, &id).await;
        assert_eq!(
            deleted.status, expected_status,
            "case {state:?}: DeleteApplication on an application settling on \
             {settles_on:?} expected {expected_status}, got {} with {:?}",
            deleted.status, deleted.body
        );
        let after = get_application(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {state:?}: the application is still readable after the delete, got {} \
             with {:?}",
            after.status, after.body
        );
        if expected_status == 400 {
            assert_eq!(
                deleted.error_code(),
                "ValidationException",
                "case {state:?}: expected ValidationException, got {:?}",
                deleted.body
            );
            assert_eq!(
                after.at("/application/state"),
                json!(state),
                "case {state:?}: a refused delete must leave the state alone, got {:?}",
                after.at("/application/state")
            );
        } else {
            assert_eq!(
                after.at("/application/state"),
                json!("TERMINATED"),
                "case {state:?}: an accepted delete must report TERMINATED, got {:?}",
                after.at("/application/state")
            );
        }
    }
}

/// `StopApplication` documents the precondition the model cannot express: "All
/// scheduled and running jobs must be completed or cancelled before stopping an
/// application."
#[tokio::test]
async fn stop_application_requires_its_job_runs_to_have_finished() {
    let router = emulator();
    let application_id = started_application(&router, "tok-stop").await;
    start_job_run(&router, &application_id, minimal_job_run("job-1")).await;

    let refused = stop_application(&router, &application_id).await;
    assert_eq!(
        refused.status, 400,
        "case an unfinished job run: StopApplication expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case an unfinished job run: expected ValidationException, got {:?}",
        refused.body
    );
    assert_eq!(
        get_application(&router, &application_id)
            .await
            .at("/application/state"),
        json!("STARTED"),
        "case an unfinished job run: the refused stop must leave the application STARTED"
    );

    // Once the job run has finished, the stop is accepted.
    wait_for_job_run_to_settle();
    let accepted = stop_application(&router, &application_id).await;
    assert_eq!(
        accepted.status, 200,
        "case every job run finished: StopApplication expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
}

/// This emulator records job runs but runs no Spark or Hive workload, so a submitted
/// job run ends in the modelled failure state with a reason that says so — never in a
/// success nothing computed.
#[tokio::test]
async fn a_submitted_job_run_ends_in_the_modelled_failure_state() {
    let router = emulator();
    let application_id = started_application(&router, "tok-exec").await;
    let submitted = start_job_run(&router, &application_id, minimal_job_run("job-1")).await;
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let immediately = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        immediately.at("/jobRun/state"),
        json!("SUBMITTED"),
        "a job run is SUBMITTED when it is accepted, got {:?}",
        immediately.at("/jobRun/state")
    );
    assert_eq!(
        immediately.at("/jobRun/startedAt"),
        Value::Null,
        "nothing ran, so there is no start instant, got {:?}",
        immediately.at("/jobRun/startedAt")
    );
    assert_eq!(
        immediately.at("/jobRun/totalResourceUtilization"),
        Value::Null,
        "nothing ran, so there is no resource utilisation, got {:?}",
        immediately.at("/jobRun/totalResourceUtilization")
    );

    wait_for_job_run_to_settle();
    let settled = get_job_run(&router, &application_id, &job_run_id).await;
    assert_eq!(
        settled.at("/jobRun/state"),
        json!("FAILED"),
        "a job run nothing executed must end FAILED, got {:?}",
        settled.at("/jobRun/state")
    );
    let details = settled.at("/jobRun/stateDetails");
    assert!(
        details
            .as_str()
            .is_some_and(|text| text.contains("executes no Spark or Hive workload")),
        "the failure must say why nothing ran, got {details:?}"
    );
    assert_ne!(
        settled.at("/jobRun/endedAt"),
        Value::Null,
        "a finished job run carries the instant it ended, got {:?}",
        settled.at("/jobRun/endedAt")
    );
    assert_eq!(
        settled.at("/jobRun/totalExecutionDurationSeconds"),
        Value::Null,
        "no duration is reported for work nothing ran, got {:?}",
        settled.at("/jobRun/totalExecutionDurationSeconds")
    );
}

/// `JobRun$executionTimeoutMinutes` documents its default: "If you didn't specify a
/// timeout, this value defaults to 720 minutes." Any value the caller does give is kept.
#[tokio::test]
async fn a_job_runs_execution_timeout_is_the_one_given_or_the_documented_default() {
    // The rows span the modelled `@range(min: 0, max: 1000000)`: both published bounds,
    // a value inside it, and the absent member that takes the default.
    let cases: Vec<(&str, Option<i64>, i64)> = vec![
        ("absent", None, DEFAULT_EXECUTION_TIMEOUT_MINUTES),
        ("the range's minimum", Some(0), 0),
        ("a value inside the range", Some(45), 45),
        ("the range's maximum", Some(1_000_000), 1_000_000),
    ];

    let router = emulator();
    let application_id = started_application(&router, "tok-timeout").await;
    for (index, (case, given, expected)) in cases.into_iter().enumerate() {
        let mut request = minimal_job_run(&format!("job-timeout-{index}"));
        if let Some(minutes) = given {
            request["executionTimeoutMinutes"] = json!(minutes);
        }
        let submitted = start_job_run(&router, &application_id, request).await;
        assert_eq!(
            submitted.status, 200,
            "case {case:?}: StartJobRun expected 200, got {} with {:?}",
            submitted.status, submitted.body
        );
        let job_run_id = submitted
            .at("/jobRunId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let run = get_job_run(&router, &application_id, &job_run_id).await;
        assert_eq!(
            run.at("/jobRun/executionTimeoutMinutes"),
            json!(expected),
            "case {case:?}: executionTimeoutMinutes expected {expected}, got {:?}",
            run.at("/jobRun/executionTimeoutMinutes")
        );
    }
}

/// `JobRun$jobDriver` is required on everything that reports a job run, so a submit
/// naming no driver is refused rather than stored.
#[tokio::test]
async fn start_job_run_without_a_job_driver_is_refused() {
    let router = emulator();
    let application_id = started_application(&router, "tok-nodriver").await;

    let refused = start_job_run(
        &router,
        &application_id,
        json!({ "clientToken": "job-nodriver", "executionRoleArn": EXECUTION_ROLE }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case no jobDriver: expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case no jobDriver: expected ValidationException, got {:?}",
        refused.body
    );
    let runs = list_job_runs(&router, &application_id, "").await;
    assert_eq!(
        runs.at("/jobRuns"),
        json!([]),
        "case no jobDriver: the refused submit must store nothing, got {:?}",
        runs.body
    );

    // Both members of the `JobDriver` union are accepted.
    for (case, driver) in [
        ("sparkSubmit", spark_submit_driver("s3://bucket/app.py")),
        (
            "hive",
            json!({ "hive": { "query": "SELECT 1", "initQueryFile": "s3://bucket/init.q" } }),
        ),
    ] {
        let accepted = start_job_run(
            &router,
            &application_id,
            json!({
                "clientToken": format!("job-driver-{case}"),
                "executionRoleArn": EXECUTION_ROLE,
                "jobDriver": driver,
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: StartJobRun expected 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        let job_run_id = accepted
            .at("/jobRunId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let run = get_job_run(&router, &application_id, &job_run_id).await;
        assert_eq!(
            run.at(&format!("/jobRun/jobDriver/{case}")),
            driver[case],
            "case {case:?}: the stored driver must read back unchanged, got {:?}",
            run.at("/jobRun/jobDriver")
        );
    }
}

/// `AutoStartConfig` documents that an application "automatically start[s] on job
/// submission", defaulting to true; an application configured otherwise refuses a
/// submit while it is not running.
#[tokio::test]
async fn a_job_submission_starts_the_application_only_when_auto_start_is_enabled() {
    let cases = [
        ("auto start left to its documented default", None, 200),
        ("auto start enabled explicitly", Some(true), 200),
        ("auto start disabled", Some(false), 400),
    ];

    for (case, enabled, expected_status) in cases {
        let router = emulator();
        let mut request = minimal_create("tok-autostart");
        if let Some(enabled) = enabled {
            request["autoStartConfiguration"] = json!({ "enabled": enabled });
        }
        create_application(&router, request).await;
        wait_for_transition();
        let id = list_applications(&router, "").await.at("/applications/0/id");
        let id = id.as_str().unwrap_or_default().to_string();

        let submitted = start_job_run(&router, &id, minimal_job_run("job-1")).await;
        assert_eq!(
            submitted.status, expected_status,
            "case {case:?}: StartJobRun expected {expected_status}, got {} with {:?}",
            submitted.status, submitted.body
        );
        if expected_status == 200 {
            assert_eq!(
                get_application(&router, &id).await.at("/application/state"),
                json!("STARTING"),
                "case {case:?}: the submit must have started the application"
            );
        } else {
            assert_eq!(
                submitted.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                submitted.body
            );
            assert_eq!(
                get_application(&router, &id).await.at("/application/state"),
                json!("CREATED"),
                "case {case:?}: a refused submit must leave the application as it was"
            );
        }
    }
}

/// `CancelJobRun` moves a running job run to `CANCELLING` and the clock owes it
/// `CANCELLED`; a repeat cancel of a run that is already `CANCELLED` is the idempotent
/// success (the refusal of a run that finished some other way is asserted in
/// `idempotency_and_idle_lifecycle`). The `shutdownGracePeriodInSeconds` the caller asks
/// for is the wait that applies.
#[tokio::test]
async fn cancel_job_run_moves_a_live_run_and_refuses_a_finished_one() {
    let router = emulator();
    let application_id = started_application(&router, "tok-cancel").await;

    let submitted = start_job_run(&router, &application_id, minimal_job_run("job-live")).await;
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cancelled = cancel_job_run(&router, &application_id, &job_run_id, "").await;
    assert_eq!(
        cancelled.status, 200,
        "case a live run: CancelJobRun expected 200, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state"),
        json!("CANCELLING"),
        "case a live run: the run reports CANCELLING straight after the cancel"
    );
    wait_for_transition();
    assert_eq!(
        get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state"),
        json!("CANCELLED"),
        "case a live run: the clock owes the run CANCELLED"
    );

    // `@idempotent`: cancelling a run that is ALREADY cancelled is the success the
    // operation promises, so the repeat answers 200 with the same ids and moves nothing.
    let again = cancel_job_run(&router, &application_id, &job_run_id, "").await;
    assert_eq!(
        again.status, 200,
        "case an already-cancelled run: CancelJobRun expected 200, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        (again.at("/applicationId"), again.at("/jobRunId")),
        (json!(application_id), json!(job_run_id)),
        "case an already-cancelled run: the repeat must echo the same ids, got {:?}",
        again.body
    );
    assert_eq!(
        get_job_run(&router, &application_id, &job_run_id)
            .await
            .at("/jobRun/state"),
        json!("CANCELLED"),
        "case an already-cancelled run: the repeat must not move the state"
    );

    // `shutdownGracePeriodInSeconds` is "the duration in seconds to wait before
    // forcefully terminating the job after cancellation is requested", so a grace
    // period longer than the emulator's own span keeps the run CANCELLING past it.
    let slow = start_job_run(&router, &application_id, minimal_job_run("job-grace")).await;
    let slow_id = slow.at("/jobRunId").as_str().unwrap_or_default().to_string();
    let grace = cancel_job_run(
        &router,
        &application_id,
        &slow_id,
        "?shutdownGracePeriodInSeconds=120",
    )
    .await;
    assert_eq!(
        grace.status, 200,
        "case a long grace period: CancelJobRun expected 200, got {} with {:?}",
        grace.status, grace.body
    );
    wait_for_transition();
    assert_eq!(
        get_job_run(&router, &application_id, &slow_id)
            .await
            .at("/jobRun/state"),
        json!("CANCELLING"),
        "case a long grace period: the run is still CANCELLING inside the wait the caller asked for"
    );
}

/// Both list operations page over a deterministic order: concatenating the pages
/// yields every stored record exactly once, whatever the page size.
#[tokio::test]
async fn paging_a_list_neither_skips_nor_duplicates_a_record() {
    let router = emulator();

    // Nine applications, so several page sizes divide the set unevenly.
    let mut expected_applications: Vec<String> = Vec::new();
    for index in 0..9 {
        let answer =
            create_application(&router, minimal_create(&format!("tok-page-{index}"))).await;
        expected_applications.push(
            answer
                .at("/applicationId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    expected_applications.sort();

    // The rows span the modelled `@range(min: 1, max: 50)`: both published bounds, two
    // sizes that divide the set unevenly, one that divides it exactly, and the absent
    // member that serves the whole page limit.
    for (case, query) in [
        ("the range's minimum", "?maxResults=1".to_string()),
        ("a page smaller than the set", "?maxResults=2".to_string()),
        ("a page dividing the set exactly", "?maxResults=3".to_string()),
        ("a page larger than the set", "?maxResults=20".to_string()),
        (
            "the range's maximum",
            format!("?maxResults={PAGE_LIMIT}"),
        ),
        ("no maxResults", String::new()),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..32 {
            let uri = match next.as_deref() {
                None => query.clone(),
                Some(token) => {
                    if query.is_empty() {
                        format!("?nextToken={token}")
                    } else {
                        format!("{query}&nextToken={token}")
                    }
                }
            };
            let page = list_applications(&router, &uri).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: ListApplications expected 200, got {} with {:?}",
                page.status, page.body
            );
            for entry in page
                .body
                .get("applications")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                seen.push(entry["id"].as_str().unwrap_or_default().to_string());
            }
            match page.at("/nextToken").as_str() {
                Some(token) => {
                    assert!(
                        matches_next_token(token),
                        "case {case:?}: nextToken must match ^[A-Za-z0-9_=-]+$, got {token:?}"
                    );
                    next = Some(token.to_string());
                }
                None => {
                    next = None;
                    break;
                }
            }
        }
        assert!(
            next.is_none(),
            "case {case:?}: paging must terminate, got a cursor after 32 pages"
        );
        assert_eq!(
            seen, expected_applications,
            "case {case:?}: the pages must concatenate to every application exactly once, \
             got {seen:?}"
        );
    }

    // A token this service did not mint is refused, not read as a resume point.
    for (case, token) in [
        ("not a cursor at all", "nonsense"),
        ("the right prefix with a bad body", "v1_zz"),
    ] {
        let answer = list_applications(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // The same for job runs, scoped to one application.
    let application_id = started_application(&router, "tok-page-runs").await;
    let mut expected_runs: Vec<String> = Vec::new();
    for index in 0..5 {
        let answer = start_job_run(
            &router,
            &application_id,
            minimal_job_run(&format!("job-page-{index}")),
        )
        .await;
        expected_runs.push(
            answer
                .at("/jobRunId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    expected_runs.sort();
    for (case, page_size) in [("one at a time", 1), ("two at a time", 2), ("all at once", 50)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..32 {
            let uri = match next.as_deref() {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let page = list_job_runs(&router, &application_id, &uri).await;
            for entry in page
                .body
                .get("jobRuns")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                seen.push(entry["id"].as_str().unwrap_or_default().to_string());
            }
            match page.at("/nextToken").as_str() {
                Some(token) => next = Some(token.to_string()),
                None => {
                    next = None;
                    break;
                }
            }
        }
        assert!(
            next.is_none(),
            "case {case:?}: paging job runs must terminate"
        );
        assert_eq!(
            seen, expected_runs,
            "case {case:?}: the pages must concatenate to every job run exactly once, got {seen:?}"
        );
    }
}

/// A list reports only the records that pass its filters, and a job run list only the
/// runs of the application it names.
#[tokio::test]
async fn a_list_reports_only_the_records_its_filters_admit() {
    let router = emulator();

    // Two applications in different states, so `states` has something to separate.
    let created_only = created_application(&router, "tok-filter-created").await;
    let running = started_application(&router, "tok-filter-started").await;

    // Rows spanning the `ApplicationStateSet` filter: one state, the other state, both,
    // a state nothing is in, and the absent filter. Source: the `ApplicationState`
    // enum.
    let cases: Vec<(&str, &str, Vec<&String>)> = vec![
        ("one state", "?states=CREATED", vec![&created_only]),
        ("the other state", "?states=STARTED", vec![&running]),
        (
            "both states",
            "?states=CREATED&states=STARTED",
            vec![&created_only, &running],
        ),
        ("a state nothing is in", "?states=TERMINATED", vec![]),
        ("no filter", "", vec![&created_only, &running]),
    ];
    for (case, query, expected) in cases {
        let mut expected: Vec<String> = expected.into_iter().cloned().collect();
        expected.sort();
        let page = list_applications(&router, query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListApplications expected 200, got {} with {:?}",
            page.status, page.body
        );
        let mut seen: Vec<String> = page
            .body
            .get("applications")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }

    // Job runs: one BATCH and one STREAMING run on the started application, and one run
    // on a second application that must never appear in the first one's list. The
    // second application is created FIRST, so the runs below are not waiting on it.
    let other_application = started_application(&router, "tok-filter-other").await;
    let other_run = start_job_run(&router, &other_application, minimal_job_run("job-other"))
        .await
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let batch = start_job_run(
        &router,
        &running,
        json!({
            "clientToken": "job-batch",
            "executionRoleArn": EXECUTION_ROLE,
            "jobDriver": spark_submit_driver("s3://bucket/batch.py"),
            "mode": "BATCH",
        }),
    )
    .await
    .at("/jobRunId")
    .as_str()
    .unwrap_or_default()
    .to_string();
    let streaming = start_job_run(
        &router,
        &running,
        json!({
            "clientToken": "job-streaming",
            "executionRoleArn": EXECUTION_ROLE,
            "jobDriver": spark_submit_driver("s3://bucket/stream.py"),
            "mode": "STREAMING",
        }),
    )
    .await
    .at("/jobRunId")
    .as_str()
    .unwrap_or_default()
    .to_string();

    // Settle both runs into two DIFFERENT terminal states, so the `states` rows below
    // separate them without depending on when this test runs: the streaming run is
    // cancelled, the batch run is left to the clock.
    cancel_job_run(&router, &running, &streaming, "").await;
    wait_for_job_run_to_settle();

    // Rows spanning the `mode` filter (the whole `JobRunMode` enum: `BATCH`,
    // `STREAMING`) and the `states` filter (values from the `JobRunState` enum).
    let run_cases: Vec<(&str, &str, Vec<&String>)> = vec![
        ("mode BATCH", "?mode=BATCH", vec![&batch]),
        ("mode STREAMING", "?mode=STREAMING", vec![&streaming]),
        ("state FAILED", "?states=FAILED", vec![&batch]),
        ("state CANCELLED", "?states=CANCELLED", vec![&streaming]),
        (
            "both states",
            "?states=FAILED&states=CANCELLED",
            vec![&batch, &streaming],
        ),
        ("a state nothing is in", "?states=SUCCESS", vec![]),
        ("no filter", "", vec![&batch, &streaming]),
    ];
    for (case, query, expected) in run_cases {
        let mut expected: Vec<String> = expected.into_iter().cloned().collect();
        expected.sort();
        let page = list_job_runs(&router, &running, query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListJobRuns expected 200, got {} with {:?}",
            page.status, page.body
        );
        let mut seen: Vec<String> = page
            .body
            .get("jobRuns")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
        assert!(
            !seen.contains(&other_run) || expected.contains(&other_run),
            "case {case:?}: another application's job run must never be listed, got {seen:?}"
        );
    }

    // `createdAtAfter` / `createdAtBefore` bound the creation instant.
    let created_at = get_job_run(&router, &running, &batch)
        .await
        .at("/jobRun/createdAt")
        .as_f64()
        .expect("createdAt is an epoch-seconds number");
    for (case, query, expect_present) in [
        (
            "a lower bound before the run",
            format!("?createdAtAfter={}", urlencode(&rfc3339(created_at - 60.0))),
            true,
        ),
        (
            "a lower bound after the run",
            format!("?createdAtAfter={}", urlencode(&rfc3339(created_at + 60.0))),
            false,
        ),
        (
            "an upper bound after the run",
            format!(
                "?createdAtBefore={}",
                urlencode(&rfc3339(created_at + 60.0))
            ),
            true,
        ),
        (
            "an upper bound before the run",
            format!(
                "?createdAtBefore={}",
                urlencode(&rfc3339(created_at - 60.0))
            ),
            false,
        ),
    ] {
        let page = list_job_runs(&router, &running, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListJobRuns expected 200, got {} with {:?}",
            page.status, page.body
        );
        let seen: Vec<String> = page
            .body
            .get("jobRuns")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            seen.contains(&batch),
            expect_present,
            "case {case:?}: expected the run to be {}present, got {seen:?}",
            if expect_present { "" } else { "ab" }
        );
    }
}

/// Every operation this slice leaves unimplemented still answers HTTP 501, so a client
/// can tell "nobody has written it yet" from a wrong answer.
#[tokio::test]
async fn the_operations_this_slice_does_not_implement_still_answer_501() {
    let router = emulator();
    let application_id = started_application(&router, "tok-stubs").await;

    for (case, method, uri, body) in [
        (
            "GetResourceDashboard",
            "GET",
            format!("/applications/{application_id}/dashboard"),
            None,
        ),
    ] {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: an unimplemented operation must answer 501, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// `Date`'s `httpQuery` binding is an RFC-3339 date-time (restJson1's default
/// timestamp format outside a JSON body), so a creation-time bound travels in that
/// form even though the same member comes back as epoch seconds in a response body.
pub(crate) fn rfc3339(epoch_seconds: f64) -> String {
    aws_smithy_types::DateTime::from_secs_f64(epoch_seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("the instant formats as RFC-3339")
}

/// Percent-encode a path segment, so an ARN can travel in one.
pub(crate) fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
