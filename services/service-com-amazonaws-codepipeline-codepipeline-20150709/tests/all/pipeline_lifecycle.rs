// SPDX-License-Identifier: CC0-1.0

//! The CodePipeline pipeline-definition lifecycle and one execution, end to end over
//! the wire the service actually speaks: `awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`.
//!
//! Every assertion goes through the router, so nothing here can pass by reading a
//! handler's internals: a response that is not computed from the shared store fails.
//!
//! The harness here ([`Emulator`], [`declaration`] and the stage builders) is
//! `pub(crate)` so the other suites of this test target reuse it rather than growing a
//! second copy; see `tests/all/update_pipeline.rs`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern
/// (`^arn:aws(-[\w]+)*:iam::[0-9]{12}:role/`).
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// The canonical emulator account id every ARN carries
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
///
/// `pub(crate)` because the other suites in this test target drive the service
/// through this same harness rather than building a second one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The one shared engine this emulator's service reads and writes, for a case
    /// that has to put a resource in the store the handler reads without making the
    /// calls that would create it (filling a quota, say). Every assertion still goes
    /// through the router.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call `operation` with `body`, signed for `region` so the handler reads the
    /// region off the SigV4 credential scope the way a real SDK request carries it.
    pub(crate) async fn call_in(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        self.send(operation, body, Some(authorization.as_str())).await
    }

    /// Call `operation` in the default region.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    async fn send(
        &self,
        operation: &str,
        body: &Value,
        authorization: Option<&str>,
    ) -> (u16, Value) {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        } else {
            // No credential scope at all: the request still routes on X-Amz-Target.
            request = request.header("host", "codepipeline.us-east-1.amazonaws.com");
        }
        let request = request
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!("{operation}: response body is not JSON ({e}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// The `__type` an error response carries (the awsJson error code clients match
    /// on), or `None` for a success.
    pub(crate) async fn expect_error(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// The `(status, code, message)` of a refusal: the awsJson `__type` without the
    /// model's namespace, and the message the error carries.
    ///
    /// `pub(crate)` on the shared harness so the suites built on this `Emulator` read one
    /// refusal the same way rather than each growing a copy.
    pub(crate) async fn refusal(&self, operation: &str, body: &Value) -> (u16, String, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        let message = response
            .get("message")
            .or_else(|| response.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (status, code, message)
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only
/// RFC 3986's unreserved characters literal — what an awsQuery body carries.
///
/// `pub(crate)` on the shared harness: a suite that drives ANOTHER service of this
/// emulator over its query protocol (IAM, which this service's role check and its
/// authorization gate read) encodes a form value the same way, so the rule lives here
/// once.
pub(crate) fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The smallest declaration that is a pipeline: a source stage, and one stage that acts
/// on what it fetched.
///
/// Two stages because that is the floor a declaration is held to, and the second stage's
/// action is outside the `Source` category because a pipeline has to do something with
/// the revision its source stage brings in (`state::validate_declaration`).
pub(crate) fn declaration(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Build", 1)],
    })
}

/// The id of the execution a pipeline's own CREATION started.
///
/// Creating a pipeline RUNS it once — `TriggerType` carries `CreatePipeline` for
/// exactly that run — so every pipeline these suites create carries one execution
/// before anything starts another, and this is how a case names it.
///
/// `pub(crate)` with the harness: the suites built on this `Emulator` name that run
/// through this one lookup rather than growing a copy each.
pub(crate) async fn creation_run(emulator: &Emulator, pipeline: &str) -> String {
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["trigger"]["triggerType"] == json!("CreatePipeline"))
        .and_then(|summary| summary["pipelineExecutionId"].as_str())
        .unwrap_or_else(|| panic!("creating {pipeline} ran it once, got {listed}"))
        .to_string()
}

/// A source stage whose action is `Custom`-owned.
///
/// The owner matters to every suite that STARTS an execution of these fixtures: a
/// `Custom`-owned action's outcome is a job worker's to report (`PollForJobs` ->
/// `AcknowledgeJob` -> `PutJobSuccessResult`), so an execution that reaches it waits
/// there instead of being carried on by the clock — which is what lets these suites
/// assert about a running execution without racing the execution plane. The plane
/// itself, including what it does to an action whose work runs outside this emulator,
/// is `tests/all/execution_engine.rs`.
pub(crate) fn source_stage(name: &str) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Src",
            "actionTypeId": {
                "category": "Source",
                "owner": "Custom",
                "provider": "MySource",
                "version": "1",
            },
            "configuration": {"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"},
            "outputArtifacts": [{"name": "SourceOut"}],
        }],
    })
}

pub(crate) fn build_stage(name: &str, action_count: usize) -> Value {
    let actions: Vec<Value> = (0..action_count)
        .map(|index| {
            json!({
                "name": format!("Build{index}"),
                "actionTypeId": {
                    "category": "Build",
                    "owner": "AWS",
                    "provider": "CodeBuild",
                    "version": "1",
                },
                "configuration": {"ProjectName": format!("project-{index}")},
                "inputArtifacts": [{"name": "SourceOut"}],
                "runOrder": index + 1,
            })
        })
        .collect();
    json!({"name": name, "actions": actions})
}

/// The `stages` a declaration is REPORTED with: the ones the request declares, with
/// each action's artifact-list members made present — empty for an action that
/// declares no artifact, which is how every reported action declaration carries them
/// (that rule has its own suite, `tests/all/action_artifact_lists.rs`).
///
/// Used by the suites that compare a reported structure against the one they sent, so
/// "as stored" stays strict about every other member.
pub(crate) fn reported_stages(declaration: &Value) -> Value {
    let mut stages = declaration["stages"].clone();
    for stage in stages.as_array_mut().into_iter().flatten() {
        for action in stage["actions"].as_array_mut().into_iter().flatten() {
            for member in ["inputArtifacts", "outputArtifacts"] {
                if action[member].is_null() {
                    action[member] = Value::Array(Vec::new());
                }
            }
        }
    }
    stages
}

pub(crate) fn stage_names(pipeline: &Value) -> Vec<String> {
    pipeline["stages"]
        .as_array()
        .expect("stages is a list")
        .iter()
        .map(|stage| stage["name"].as_str().expect("a stage name").to_string())
        .collect()
}

/// The full happy path the service exists for: create a pipeline, read it back,
/// list it, start an execution, read the execution back, list executions, delete,
/// and watch every read follow the state.
#[tokio::test]
async fn the_pipeline_lifecycle_is_served_from_state() {
    let emulator = Emulator::new();
    let declaration = json!({
        "name": "demo",
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Build", 2)],
    });

    let created = emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration}))
        .await;
    assert_eq!(
        created["pipeline"]["version"], 1,
        "a new pipeline is version 1, got {created}"
    );

    let fetched = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
    assert_eq!(
        stage_names(&fetched["pipeline"]),
        vec!["Source".to_string(), "Build".to_string()],
        "GetPipeline must report the stages as created, got {fetched}"
    );
    assert_eq!(
        fetched["pipeline"]["stages"][1]["actions"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "GetPipeline must report both Build actions, got {fetched}"
    );
    assert_eq!(
        fetched["pipeline"]["roleArn"], ROLE_ARN,
        "GetPipeline must report the roleArn as created, got {fetched}"
    );
    assert_eq!(
        fetched["metadata"]["pipelineArn"],
        json!(format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:demo")),
        "metadata must carry the pipeline ARN, got {fetched}"
    );
    for member in ["created", "updated"] {
        assert!(
            fetched["metadata"][member].is_number(),
            "metadata.{member} must be a timestamp, got {fetched}"
        );
    }

    let listed = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed["pipelines"][0]["name"], "demo",
        "ListPipelines must list the created pipeline, got {listed}"
    );
    assert_eq!(
        listed["pipelines"][0]["version"], 1,
        "the summary carries the stored version, got {listed}"
    );

    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": "demo"}))
        .await;
    let execution_id = started["pipelineExecutionId"]
        .as_str()
        .expect("an execution id")
        .to_string();

    let execution = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": "demo", "pipelineExecutionId": execution_id}),
        )
        .await;
    let execution = &execution["pipelineExecution"];
    assert_eq!(
        execution["pipelineExecutionId"],
        json!(execution_id),
        "the stored execution is the one the start call minted, got {execution}"
    );
    assert_eq!(
        execution["status"], "InProgress",
        "a freshly started execution is InProgress, got {execution}"
    );
    assert_eq!(
        execution["pipelineName"], "demo",
        "the execution names its pipeline, got {execution}"
    );
    assert_eq!(
        execution["pipelineVersion"], 1,
        "the execution runs the stored pipeline version, got {execution}"
    );
    assert_eq!(
        execution["trigger"]["triggerType"], "StartPipelineExecution",
        "the trigger records what started it, got {execution}"
    );

    let executions = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": "demo"}))
        .await;
    assert_eq!(
        executions["pipelineExecutionSummaries"][0]["pipelineExecutionId"],
        json!(execution_id),
        "the summary list reports the stored execution, got {executions}"
    );

    emulator
        .ok("DeletePipeline", &json!({"name": "demo"}))
        .await;
    let (status, code) = emulator.expect_error("GetPipeline", &json!({"name": "demo"})).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNotFoundException"),
        "a deleted pipeline is not found"
    );
    let listed = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed["pipelines"],
        json!([]),
        "a deleted pipeline is no longer listed, got {listed}"
    );
    let (status, code) = emulator
        .expect_error(
            "GetPipelineExecution",
            &json!({"pipelineName": "demo", "pipelineExecutionId": execution_id}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNotFoundException"),
        "a deleted pipeline's executions go with it"
    );
}

/// A stored declaration round-trips unchanged, whatever its shape: the rule is
/// "GetPipeline reports the declaration as stored", so the table varies the parts a
/// declaration can differ in — stage count, action count, artifact-store form,
/// V2 parameters, tags — not just the name.
#[tokio::test]
async fn every_declaration_round_trips_as_stored() {
    // (case, declaration, expected pipelineType, expected executionMode)
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the smallest declaration there is",
            declaration("single"),
            // No V2-only parameter, so the type resolves to V1.
            "V1",
            "SUPERSEDED",
        ),
        (
            "three stages, several actions",
            json!({
                "name": "three",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 3), build_stage("Test", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "cross-region artifactStores map",
            json!({
                "name": "cross-region",
                "roleArn": ROLE_ARN,
                "artifactStores": {
                    "us-east-1": {"type": "S3", "location": "east"},
                    "eu-west-1": {"type": "S3", "location": "west"},
                },
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "V2 parameters make it a V2 pipeline",
            json!({
                "name": "with-variables",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                "variables": [{"name": "Env", "defaultValue": "test"}],
                "triggers": [{
                    "providerType": "CodeStarSourceConnection",
                    "gitConfiguration": {"sourceActionName": "Src"},
                }],
            }),
            "V2",
            "SUPERSEDED",
        ),
        (
            "an explicit pipelineType and executionMode are kept",
            json!({
                "name": "explicit",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                "pipelineType": "V2",
                "executionMode": "QUEUED",
                "variables": [{"name": "Env", "defaultValue": "test"}],
            }),
            "V2",
            "QUEUED",
        ),
        (
            "an encryption key on the artifact store",
            json!({
                "name": "encrypted",
                "roleArn": ROLE_ARN,
                "artifactStore": {
                    "type": "S3",
                    "location": "my-bucket",
                    "encryptionKey": {"id": "alias/cp", "type": "KMS"},
                },
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
    ];

    for (case, declaration, pipeline_type, execution_mode) in cases {
        let emulator = Emulator::new();
        let name = declaration["name"].as_str().expect("a name").to_string();
        let created = emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": declaration, "tags": [{"key": "team", "value": "build"}]}),
            )
            .await;
        assert_eq!(
            created["tags"],
            json!([{"key": "team", "value": "build"}]),
            "case {case:?}: CreatePipeline reports the tags applied, got {created}"
        );

        let fetched = emulator.ok("GetPipeline", &json!({"name": name})).await;
        let stored = &fetched["pipeline"];
        assert_eq!(
            stored["stages"],
            reported_stages(&declaration),
            "case {case:?}: the stages must come back exactly as stored, got {stored}"
        );
        assert_eq!(
            stored["artifactStore"], declaration["artifactStore"],
            "case {case:?}: artifactStore must come back as stored, got {stored}"
        );
        assert_eq!(
            stored["artifactStores"], declaration["artifactStores"],
            "case {case:?}: artifactStores must come back as stored, got {stored}"
        );
        assert_eq!(
            stored["version"], 1,
            "case {case:?}: a new pipeline is version 1, got {stored}"
        );
        assert_eq!(
            stored["pipelineType"], pipeline_type,
            "case {case:?}: unexpected resolved pipelineType, got {stored}"
        );
        assert_eq!(
            stored["executionMode"], execution_mode,
            "case {case:?}: unexpected resolved executionMode, got {stored}"
        );
        assert_eq!(
            created["pipeline"], *stored,
            "case {case:?}: CreatePipeline and GetPipeline must report one declaration"
        );

        // The one summary this pipeline has agrees with what GetPipeline reports.
        let listed = emulator.ok("ListPipelines", &json!({})).await;
        let summary = &listed["pipelines"][0];
        assert_eq!(
            (
                summary["name"].clone(),
                summary["version"].clone(),
                summary["pipelineType"].clone(),
                summary["executionMode"].clone()
            ),
            (
                json!(name),
                json!(1),
                json!(pipeline_type),
                json!(execution_mode)
            ),
            "case {case:?}: the summary must agree with the stored pipeline, got {summary}"
        );
    }
}

/// A declaration whose structure cannot describe a pipeline is refused with the
/// error shape the model declares for that defect — never accepted.
#[tokio::test]
async fn a_structurally_impossible_declaration_is_refused() {
    // (case, declaration, expected awsJson error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no stages at all",
            json!({
                "name": "empty",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [],
            }),
            "InvalidStructureException",
        ),
        (
            "a stage with no actions",
            json!({
                "name": "empty-stage",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [{"name": "Source", "actions": []}, build_stage("Build", 1)],
            }),
            "InvalidStageDeclarationException",
        ),
        (
            "a middle stage with no actions",
            json!({
                "name": "empty-second-stage",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [
                    source_stage("Source"),
                    {"name": "Gate", "actions": []},
                    build_stage("Build", 1),
                ],
            }),
            "InvalidStageDeclarationException",
        ),
        (
            // The requirement is the service's own, stated over the input rather than
            // as a defect of the pipeline's structure
            // (`state::MISSING_ARTIFACT_STORE_MESSAGE`).
            "neither artifactStore nor artifactStores",
            json!({
                "name": "no-store",
                "roleArn": ROLE_ARN,
                "stages": [source_stage("Source")],
            }),
            "ValidationException",
        ),
        (
            "both artifactStore and artifactStores",
            json!({
                "name": "both-stores",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "artifactStores": {"us-east-1": {"type": "S3", "location": "my-bucket"}},
                "stages": [source_stage("Source")],
            }),
            "InvalidStructureException",
        ),
    ];

    for (case, declaration, expected) in cases {
        let emulator = Emulator::new();
        let name = declaration["name"].as_str().expect("a name").to_string();
        let (status, code) = emulator
            .expect_error("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );
        // A refused create stores nothing.
        let (status, code) = emulator.expect_error("GetPipeline", &json!({"name": name})).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: a refused create must store no pipeline"
        );
    }
}

/// A declaration the model's own constraints reject never reaches the store: the
/// required members and their patterns are refused at the boundary with
/// `ValidationException`, as the model declares for this operation.
#[tokio::test]
async fn a_declaration_the_model_rejects_never_reaches_the_store() {
    // (case, declaration)
    let cases: Vec<(&str, Value)> = vec![
        (
            "no roleArn",
            json!({
                "name": "no-role",
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source")],
            }),
        ),
        (
            "a roleArn that is not an IAM role ARN",
            json!({
                "name": "bad-role",
                "roleArn": "not-an-arn",
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source")],
            }),
        ),
        (
            "no stages member at all",
            json!({
                "name": "no-stages",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
            }),
        ),
        (
            "a name outside PipelineName's pattern",
            json!({
                "name": "not a pipeline name",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source")],
            }),
        ),
    ];

    for (case, declaration) in cases {
        let emulator = Emulator::new();
        let (status, code) = emulator
            .expect_error("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: unexpected refusal"
        );
        let listed = emulator.ok("ListPipelines", &json!({})).await;
        assert_eq!(
            listed["pipelines"],
            json!([]),
            "case {case:?}: a refused create must store nothing, got {listed}"
        );
    }
}

/// A name is in use once, and the second create fails without touching the first.
#[tokio::test]
async fn a_name_in_use_is_refused_and_the_stored_pipeline_is_untouched() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;

    let mut second = declaration("demo");
    second["stages"] = json!([source_stage("Different"), build_stage("Build", 1)]);
    let (status, code) = emulator
        .expect_error("CreatePipeline", &json!({"pipeline": second}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNameInUseException"),
        "re-creating a stored name must be refused"
    );

    let fetched = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
    assert_eq!(
        stage_names(&fetched["pipeline"]),
        vec!["Source".to_string(), "Build".to_string()],
        "the refused create must not overwrite the stored declaration, got {fetched}"
    );
}

/// `GetPipeline` with a `version` serves the stored version, and a version that was
/// never created is not found.
#[tokio::test]
async fn get_pipeline_resolves_a_requested_version() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;

    let at_one = emulator
        .ok("GetPipeline", &json!({"name": "demo", "version": 1}))
        .await;
    let current = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
    assert_eq!(
        at_one["pipeline"], current["pipeline"],
        "version 1 is the only version a created pipeline has, got {at_one}"
    );

    // Each version past the only stored one is absent, not an empty success.
    for version in [2, 7, 1000] {
        let (status, code) = emulator
            .expect_error("GetPipeline", &json!({"name": "demo", "version": version}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineVersionNotFoundException"),
            "version {version} was never created"
        );
    }
}

/// `ListPipelines` pages the stored pipelines in name order: every page is at most
/// `maxResults` long, the pages concatenate to the whole set exactly once, and the
/// last page carries no token. The table spans both published bounds of
/// `MaxPipelines` (`@range(min: 1, max: 1000)`) and sizes either side of the page.
#[tokio::test]
async fn list_pipelines_pages_the_whole_store_in_name_order() {
    // (case, pipeline count, maxResults)
    let cases: [(&str, usize, Option<i64>); 7] = [
        ("empty store", 0, None),
        ("no maxResults returns everything", 5, None),
        ("the minimum page size", 4, Some(1)),
        ("a page smaller than the store", 5, Some(2)),
        ("a page exactly the store size", 3, Some(3)),
        ("a page larger than the store", 3, Some(10)),
        ("the maximum page size", 3, Some(1000)),
    ];

    for (case, count, max_results) in cases {
        let emulator = Emulator::new();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            // Deliberately created out of order: the listing is name-ordered, not
            // creation-ordered.
            let name = format!("pipeline-{:02}", (count - index) * 7 % 100);
            emulator
                .ok("CreatePipeline", &json!({"pipeline": declaration(&name)}))
                .await;
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let page = emulator.ok("ListPipelines", &request).await;
            let names: Vec<String> = page["pipelines"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: pipelines is a list, got {page}"))
                .iter()
                .map(|summary| summary["name"].as_str().expect("a name").to_string())
                .collect();
            if let Some(max) = max_results {
                assert!(
                    i64::try_from(names.len()).expect("a small page") <= max,
                    "case {case:?}: a page must hold at most maxResults, got {names:?}"
                );
            }
            seen.extend(names);
            next_token = page["nextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every stored pipeline once, in name order"
        );
    }
}

/// A `nextToken` this listing did not issue is refused rather than silently served
/// as a first page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;

    // Not base64 at all, base64 of text carrying no cursor kind ("demo"), and a
    // cursor of the other listing's kind (base64 of "execution:demo").
    for token in ["not-a-token", "ZGVtbw==", "ZXhlY3V0aW9uOmRlbW8="] {
        let (status, code) = emulator
            .expect_error("ListPipelines", &json!({"nextToken": token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "token {token:?} was never issued by ListPipelines"
        );
    }
}

/// Every start mints a distinct, stored, v4-shaped execution id, and the same
/// `clientRequestToken` names the execution it already minted.
#[tokio::test]
async fn start_pipeline_execution_mints_stored_ids_and_honours_its_idempotency_token() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;

    // (case, clientRequestToken)
    let cases: [(&str, Option<&str>); 5] = [
        ("no token", None),
        ("token A", Some("token-A")),
        ("token A again", Some("token-A")),
        ("token B", Some("token-B")),
        ("no token again", None),
    ];

    let mut ids: Vec<String> = Vec::new();
    for (case, token) in cases {
        let mut request = json!({"name": "demo"});
        if let Some(token) = token {
            request["clientRequestToken"] = json!(token);
        }
        let started = emulator.ok("StartPipelineExecution", &request).await;
        let id = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {started}"))
            .to_string();
        assert!(
            is_execution_id(&id),
            "case {case:?}: {id:?} does not match PipelineExecutionId's modelled pattern"
        );
        // Every id the start call reports is readable from state.
        let execution = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": "demo", "pipelineExecutionId": id}),
            )
            .await;
        assert_eq!(
            execution["pipelineExecution"]["status"], "InProgress",
            "case {case:?}: the stored execution is InProgress, got {execution}"
        );
        ids.push(id);
    }

    assert_eq!(
        ids[1], ids[2],
        "repeating clientRequestToken token-A must return the same execution id"
    );
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        4,
        "four distinct requests must mint four executions, got {ids:?}"
    );
}

/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, the modelled
/// `PipelineExecutionId` pattern, checked without pulling in a regex engine.
fn is_execution_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The variables an execution resolves come from the declaration and the overrides:
/// a declared default when nothing overrides it, the override when one is given,
/// and a refusal for an override the pipeline declares no variable for.
#[tokio::test]
async fn execution_variables_resolve_against_the_declaration() {
    let emulator = Emulator::new();
    let declaration = json!({
        "name": "vars",
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Build", 1)],
        "variables": [
            {"name": "Env", "defaultValue": "test"},
            {"name": "Release", "defaultValue": "none"},
        ],
    });
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration}))
        .await;

    // (case, overrides, expected resolved variables)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no overrides: every declared default resolves",
            json!([]),
            json!([
                {"name": "Env", "resolvedValue": "test"},
                {"name": "Release", "resolvedValue": "none"},
            ]),
        ),
        (
            "one override",
            json!([{"name": "Env", "value": "prod"}]),
            json!([
                {"name": "Env", "resolvedValue": "prod"},
                {"name": "Release", "resolvedValue": "none"},
            ]),
        ),
        (
            "every variable overridden, in the other order",
            json!([
                {"name": "Release", "value": "1.2.3"},
                {"name": "Env", "value": "prod"},
            ]),
            json!([
                {"name": "Env", "resolvedValue": "prod"},
                {"name": "Release", "resolvedValue": "1.2.3"},
            ]),
        ),
    ];

    for (case, overrides, expected) in cases {
        let mut request = json!({"name": "vars"});
        if !overrides.as_array().expect("a list").is_empty() {
            request["variables"] = overrides;
        }
        let started = emulator.ok("StartPipelineExecution", &request).await;
        let id = started["pipelineExecutionId"].as_str().expect("an id");
        let execution = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": "vars", "pipelineExecutionId": id}),
            )
            .await;
        assert_eq!(
            execution["pipelineExecution"]["variables"], expected,
            "case {case:?}: unexpected resolved variables, got {execution}"
        );
    }

    let (status, code) = emulator
        .expect_error(
            "StartPipelineExecution",
            &json!({"name": "vars", "variables": [{"name": "Unknown", "value": "x"}]}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an override the pipeline declares no variable for cannot be resolved"
    );

    // A declared variable with no defaultValue must be given a value at start time;
    // the execution cannot be started without one, and is not started with the
    // variable missing from its resolved set.
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": {
                "name": "needs-value",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                "variables": [{"name": "Env", "description": "no default"}],
            }}),
        )
        .await;
    let (status, code) = emulator
        .expect_error("StartPipelineExecution", &json!({"name": "needs-value"}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a declared variable with no default and no override cannot be resolved"
    );
    let executions = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({"pipelineName": "needs-value"}),
        )
        .await;
    assert_eq!(
        executions["pipelineExecutionSummaries"],
        json!([]),
        "the refused start must not have recorded an execution, got {executions}"
    );
    let started = emulator
        .ok(
            "StartPipelineExecution",
            &json!({"name": "needs-value", "variables": [{"name": "Env", "value": "prod"}]}),
        )
        .await;
    let id = started["pipelineExecutionId"].as_str().expect("an id");
    let execution = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": "needs-value", "pipelineExecutionId": id}),
        )
        .await;
    assert_eq!(
        execution["pipelineExecution"]["variables"],
        json!([{"name": "Env", "resolvedValue": "prod"}]),
        "giving the value resolves it, got {execution}"
    );
}

/// The cursor rule both listings share: a page resumes at the first item still in
/// the listing's order at or after the cursor's key. `ListPipelines` is ordered by
/// that key, so a pipeline deleted mid-paging still has a position and paging
/// continues; an execution that is no longer retained has none, so its cursor is
/// refused.
#[tokio::test]
async fn a_stale_cursor_resumes_where_the_order_still_has_a_position() {
    let emulator = Emulator::new();
    for name in ["aaa", "bbb", "ccc"] {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration(name)}))
            .await;
    }
    let first = emulator
        .ok("ListPipelines", &json!({"maxResults": 1}))
        .await;
    let token = first["nextToken"].as_str().expect("a token").to_string();

    // The pipeline the cursor names is deleted before the next page is asked for.
    emulator.ok("DeletePipeline", &json!({"name": "bbb"})).await;
    let second = emulator
        .ok("ListPipelines", &json!({"nextToken": token}))
        .await;
    assert_eq!(
        second["pipelines"]
            .as_array()
            .map(|page| page
                .iter()
                .map(|p| p["name"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()),
        Some(vec!["ccc".to_string()]),
        "paging continues at the next stored name, got {second}"
    );

    // An execution's cursor, after the pipeline it belongs to is gone: the key has
    // no position in any order, so the token is refused.
    emulator
        .ok("StartPipelineExecution", &json!({"name": "aaa"}))
        .await;
    emulator
        .ok("StartPipelineExecution", &json!({"name": "aaa"}))
        .await;
    let page = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({"pipelineName": "aaa", "maxResults": 1}),
        )
        .await;
    let execution_token = page["nextToken"].as_str().expect("a token").to_string();
    emulator.ok("DeletePipeline", &json!({"name": "aaa"})).await;
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("aaa")}))
        .await;
    let (status, code) = emulator
        .expect_error(
            "ListPipelineExecutions",
            &json!({"pipelineName": "aaa", "nextToken": execution_token}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidNextTokenException"),
        "a cursor naming an execution that is no longer retained is refused"
    );
}

/// An execution the store does not hold is not found — for every id shape a caller
/// can ask about — and neither is one belonging to another pipeline.
#[tokio::test]
async fn an_unknown_execution_or_pipeline_is_not_found() {
    let emulator = Emulator::new();
    for name in ["demo", "other"] {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration(name)}))
            .await;
    }
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": "other"}))
        .await;
    let other_id = started["pipelineExecutionId"].as_str().expect("an id");

    // (case, pipelineName, pipelineExecutionId, expected error)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an id nothing minted",
            "demo",
            "00000000-0000-0000-0000-000000000000",
            "PipelineExecutionNotFoundException",
        ),
        (
            "another well-formed id nothing minted",
            "demo",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            "PipelineExecutionNotFoundException",
        ),
        (
            "an execution of another pipeline",
            "demo",
            other_id,
            "PipelineExecutionNotFoundException",
        ),
        (
            "a pipeline that does not exist",
            "missing",
            other_id,
            "PipelineNotFoundException",
        ),
    ];

    for (case, pipeline_name, execution_id, expected) in cases {
        let (status, code) = emulator
            .expect_error(
                "GetPipelineExecution",
                &json!({
                    "pipelineName": pipeline_name,
                    "pipelineExecutionId": execution_id,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );
    }

    for operation in ["StartPipelineExecution", "ListPipelineExecutions"] {
        let body = if operation == "StartPipelineExecution" {
            json!({"name": "missing"})
        } else {
            json!({"pipelineName": "missing"})
        };
        let (status, code) = emulator.expect_error(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "{operation} on an unknown pipeline"
        );
    }
}

/// `ListPipelineExecutions` reports the pipeline's executions newest first and pages
/// them, whatever the page size.
#[tokio::test]
async fn list_pipeline_executions_reports_newest_first_and_pages() {
    for page_size in [None, Some(1), Some(2), Some(4), Some(100)] {
        let emulator = Emulator::new();
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
            .await;
        // Creating the pipeline RAN it once, so the oldest execution of the five this
        // listing pages over is that run (`TriggerType` carries `CreatePipeline`).
        let mut started: Vec<String> = vec![creation_run(&emulator, "demo").await];
        for index in 0..4 {
            let response = emulator
                .ok(
                    "StartPipelineExecution",
                    &json!({"name": "demo", "clientRequestToken": format!("token-{index}")}),
                )
                .await;
            started.push(
                response["pipelineExecutionId"]
                    .as_str()
                    .expect("an id")
                    .to_string(),
            );
        }
        let newest_first: Vec<String> = started.iter().rev().cloned().collect();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({"pipelineName": "demo"});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let page = emulator.ok("ListPipelineExecutions", &request).await;
            let ids: Vec<String> = page["pipelineExecutionSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("page_size {page_size:?}: expected a list, got {page}"))
                .iter()
                .map(|summary| {
                    summary["pipelineExecutionId"]
                        .as_str()
                        .expect("an id")
                        .to_string()
                })
                .collect();
            if let Some(size) = page_size {
                assert!(
                    i64::try_from(ids.len()).expect("a small page") <= size,
                    "page_size {page_size:?}: a page must hold at most maxResults, got {ids:?}"
                );
            }
            seen.extend(ids);
            next_token = page["nextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, newest_first,
            "page_size {page_size:?}: executions must page newest first, exactly once each"
        );
    }
}

/// The `succeededInStage` filter is refused as not implemented rather than answered
/// with a list that ignores it: this emulator records no stage-level outcome to
/// filter on.
#[tokio::test]
async fn a_stage_filter_is_refused_as_not_implemented() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;
    let (status, code) = emulator
        .expect_error(
            "ListPipelineExecutions",
            &json!({
                "pipelineName": "demo",
                "filter": {"succeededInStage": {"stageName": "Source"}},
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (501, "NotImplemented"),
        "a filter whose answer the emulator cannot compute must not be ignored"
    );
}

/// Deleting a pipeline takes its executions and its idempotency tokens with it, and
/// leaves every other pipeline alone.
#[tokio::test]
async fn delete_pipeline_removes_only_that_pipeline_and_its_executions() {
    let emulator = Emulator::new();
    for name in ["keep", "drop"] {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration(name)}))
            .await;
        emulator
            .ok(
                "StartPipelineExecution",
                &json!({"name": name, "clientRequestToken": "shared-token"}),
            )
            .await;
    }
    let kept = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": "keep"}))
        .await;
    let kept_id = kept["pipelineExecutionSummaries"][0]["pipelineExecutionId"]
        .as_str()
        .expect("an id")
        .to_string();
    // Every execution the pipeline about to be deleted holds — the run its creation
    // started and the one the start above began — so what must not survive it is named
    // rather than assumed.
    let dropped = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": "drop"}))
        .await;
    let dropped_ids: Vec<Value> = dropped["pipelineExecutionSummaries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["pipelineExecutionId"].clone())
        .collect();

    emulator.ok("DeletePipeline", &json!({"name": "drop"})).await;

    let listed = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed["pipelines"]
            .as_array()
            .map(|pipelines| pipelines
                .iter()
                .map(|p| p["name"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()),
        Some(vec!["keep".to_string()]),
        "only the deleted pipeline is gone, got {listed}"
    );
    let still_there = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": "keep", "pipelineExecutionId": kept_id}),
        )
        .await;
    assert_eq!(
        still_there["pipelineExecution"]["pipelineExecutionId"],
        json!(kept_id),
        "the surviving pipeline keeps its execution, got {still_there}"
    );

    // The model declares no not-found error for DeletePipeline, so deleting what is
    // not there succeeds and changes nothing.
    emulator.ok("DeletePipeline", &json!({"name": "drop"})).await;
    let listed_again = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed, listed_again,
        "a repeated delete must change nothing"
    );

    // Re-creating the deleted name starts over at version 1 with its own history.
    let recreated = emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("drop")}))
        .await;
    assert_eq!(
        recreated["pipeline"]["version"], 1,
        "a re-created pipeline starts at version 1, got {recreated}"
    );
    let executions = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": "drop"}))
        .await;
    let surviving: Vec<&Value> = executions["pipelineExecutionSummaries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|summary| dropped_ids.contains(&summary["pipelineExecutionId"]))
        .collect();
    assert!(
        surviving.is_empty(),
        "the deleted pipeline's executions did not survive, got {executions}"
    );
    // What the re-created pipeline does hold is the run its own creation started,
    // which is a history of its own rather than the deleted pipeline's.
    assert_eq!(
        executions["pipelineExecutionSummaries"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the re-created pipeline holds its own creation run alone, got {executions}"
    );
}

/// The store is bounded in both directions a caller can grow it.
///
/// Past the account's pipeline limit `CreatePipeline` answers the modelled
/// `LimitExceededException`, and past the retained execution window a new execution
/// drops the oldest one — so neither map grows without limit, and the window keeps
/// exactly the most recent executions.
#[tokio::test]
async fn the_store_is_bounded_per_request() {
    // Both figures are the service crate's own bounds, read from `crate::state` so
    // the test cannot drift from them.
    let max_pipelines = service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_PIPELINES_PER_ACCOUNT;
    let max_history = service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_EXECUTION_HISTORY_PER_PIPELINE;

    let emulator = Emulator::new();
    for index in 0..max_pipelines {
        let name = format!("pipeline-{index:05}");
        let (status, response) = emulator
            .call("CreatePipeline", &json!({"pipeline": declaration(&name)}))
            .await;
        assert_eq!(
            status, 200,
            "pipeline {index} is inside the limit of {max_pipelines}, got {response}"
        );
    }
    let (status, code) = emulator
        .expect_error(
            "CreatePipeline",
            &json!({"pipeline": declaration("one-too-many")}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "pipeline {} exceeds the account limit",
        max_pipelines + 1
    );
    // A refused create leaves the limit intact, so deleting one makes room again.
    emulator
        .ok("DeletePipeline", &json!({"name": "pipeline-00000"}))
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": declaration("one-too-many")}),
        )
        .await;

    // The execution window: start two more than it holds, on its own emulator.
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..max_history + 2 {
        let response = emulator
            .ok(
                "StartPipelineExecution",
                &json!({"name": "demo", "clientRequestToken": format!("token-{index:05}")}),
            )
            .await;
        ids.push(
            response["pipelineExecutionId"]
                .as_str()
                .expect("an id")
                .to_string(),
        );
    }

    // The two oldest fell out of the window, with the tokens that named them.
    for (index, evicted) in ids[..2].iter().enumerate() {
        let (status, code) = emulator
            .expect_error(
                "GetPipelineExecution",
                &json!({"pipelineName": "demo", "pipelineExecutionId": evicted}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineExecutionNotFoundException"),
            "execution {index} is outside the retained window of {max_history}"
        );
        let restarted = emulator
            .ok(
                "StartPipelineExecution",
                &json!({"name": "demo", "clientRequestToken": format!("token-{index:05}")}),
            )
            .await;
        assert_ne!(
            restarted["pipelineExecutionId"],
            json!(evicted),
            "an evicted execution's idempotency token must not keep naming it"
        );
    }

    // The newest is still there, and the window never holds more than its bound.
    let newest = ids.last().expect("an id");
    let execution = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": "demo", "pipelineExecutionId": newest}),
        )
        .await;
    assert_eq!(
        execution["pipelineExecution"]["pipelineExecutionId"],
        json!(newest),
        "the newest execution must still be readable, got {execution}"
    );
    let mut retained = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"pipelineName": "demo", "maxResults": 100});
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListPipelineExecutions", &request).await;
        retained += page["pipelineExecutionSummaries"]
            .as_array()
            .map_or(0, Vec::len);
        next_token = page["nextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        retained, max_history,
        "the retained history must stay at its bound of {max_history}"
    );
}

/// The pipeline ARN carries the region the request was signed for, and the emulator's
/// canonical account id — never a region or account read out of the response of
/// another call.
#[tokio::test]
async fn the_pipeline_arn_carries_the_requests_region() {
    // Each region is a published AWS region; the default is the one
    // `emulator_server::emulator_arn` documents for a request that names none.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call_in(region, "CreatePipeline", &json!({"pipeline": declaration("demo")}))
            .await;
        assert_eq!(status, 200, "region {region}: create failed with {created}");
        let fetched = emulator
            .call_in(region, "GetPipeline", &json!({"name": "demo"}))
            .await
            .1;
        assert_eq!(
            fetched["metadata"]["pipelineArn"],
            json!(format!("arn:aws:codepipeline:{region}:{ACCOUNT_ID}:demo")),
            "region {region}: unexpected pipeline ARN, got {fetched}"
        );
    }

    // An unsigned request carries no credential scope: the ARN falls back to the
    // documented default region.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .send("CreatePipeline", &json!({"pipeline": declaration("demo")}), None)
        .await;
    assert_eq!(status, 200, "unsigned create failed with {created}");
    let (_, fetched) = emulator
        .send("GetPipeline", &json!({"name": "demo"}), None)
        .await;
    assert_eq!(
        fetched["metadata"]["pipelineArn"],
        json!(format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:demo")),
        "an unsigned request mints an ARN in the default region, got {fetched}"
    );
}

/// A read never changes what a later caller sees: repeating every reporting
/// operation reports the same thing, and starts no execution.
#[tokio::test]
async fn reads_do_not_change_state() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": "demo"}))
        .await;
    let id = started["pipelineExecutionId"].as_str().expect("an id");

    let reads: Vec<(&str, Value)> = vec![
        ("GetPipeline", json!({"name": "demo"})),
        ("ListPipelines", json!({})),
        ("ListPipelineExecutions", json!({"pipelineName": "demo"})),
        (
            "GetPipelineExecution",
            json!({"pipelineName": "demo", "pipelineExecutionId": id}),
        ),
    ];
    for (operation, request) in reads {
        let first = emulator.ok(operation, &request).await;
        let second = emulator.ok(operation, &request).await;
        assert_eq!(
            first, second,
            "{operation}: a read must report the same thing twice"
        );
    }

    let executions = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": "demo"}))
        .await;
    assert_eq!(
        executions["pipelineExecutionSummaries"]
            .as_array()
            .map(Vec::len),
        // The two the calls above began: the run the pipeline's creation started and
        // the one the start did. A read adds none.
        Some(2),
        "reading must not start an execution, got {executions}"
    );
}
