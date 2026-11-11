// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the pipeline EXECUTION plane: `StartPipelineExecution` sets an
//! execution running, the engine drives its stages and actions, and the reporting
//! operations report where it got to.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! case:
//!
//! * an execution does not stay `InProgress`: each stage's actions run in `runOrder`
//!   order, the artifacts are produced into the pipeline's `artifactStore`, and the
//!   execution reaches a terminal `PipelineExecutionStatus` that
//!   `GetPipelineExecution` / `ListPipelineExecutions` / `GetPipelineState` report;
//! * an action is only `Succeeded` when its real work was performed — an S3 source
//!   action's bytes reach the artifact store and an S3 deploy action's reach the
//!   delivered object — and work this emulator cannot perform fails the action with a
//!   reason, which fails the execution;
//! * an `Approval` action holds its stage until `PutApprovalResult` answers the token it
//!   published, so nothing downstream of it is delivered before that;
//! * a disabled inbound transition holds an execution out of the stage until
//!   `EnableStageTransition` releases it;
//! * `StopPipelineExecution` settles the execution at `Stopped` and nothing further is
//!   delivered; a second stop is refused;
//! * the pipeline's `executionMode` decides whether a start runs at once, waits, or
//!   supersedes the execution already waiting, and a start past the pipeline's
//!   concurrent-execution ceiling is refused;
//! * only the LATEST execution's failed stage can be retried, and a retry addressed at
//!   an older one is refused as not retryable;
//!
//! Every assertion goes through the router, over the wire each service speaks, so
//! nothing here can pass by reading a handler's internals.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for CodePipeline.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern. IAM is NOT mounted in this
/// suite, so `CreatePipeline`'s role check has no answer and the declaration is accepted
/// (only a definitive "no such role" refuses one — see `create_pipeline::role_is_absent`,
/// pinned by the `validation_and_messages` suite, which does mount IAM).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// How long a test waits for the clock-owed steps of one execution.
///
/// Each step becomes due `state::execution::ACTION_STEP` (300 ms) after the one before
/// it, so a three-stage pipeline needs a few seconds of polling; this is the ceiling at
/// which a test gives up and reports what it last saw.
///
/// `pub(crate)` with the harness below: the `stop_pipeline_execution` suite drives the
/// same execution plane through it rather than growing a second copy.
pub(crate) const WAIT: Duration = Duration::from_secs(20);

/// One emulator: the shared engine, CodePipeline and S3 mounted on it, and the router
/// that dispatches to both — the execution engine reaches S3 through the framework's
/// in-process callback, so both have to be there.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one engine both mounted services were built from, so a suite can read a store
    /// they SHARE — the account's management-event log, which the service that serves a
    /// call writes and CloudTrail reads.
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
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // The artifact store's customer managed key is a key of the KMS service: the
        // engine takes the data key each artifact object is encrypted under from it, so
        // without KMS mounted there is no key for that encryption to come from.
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// The shared engine this emulator's services were built from.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call one CodePipeline operation.
    pub(crate) async fn cp(&self, operation: &str, body: &Value) -> (u16, Value) {
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

    /// Call one CodePipeline operation that must succeed.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.cp(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// The `__type` of a CodePipeline error answer.
    pub(crate) async fn error_code(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.cp(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    /// One S3 request, path style, the way the emulated S3 service routes it.
    async fn s3(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, http::HeaderMap, bytes::Bytes) {
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
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), parts.headers, body)
    }

    /// Create a bucket with versioning enabled — what a CodePipeline source bucket is.
    pub(crate) async fn bucket(&self, name: &str) {
        let (status, _, body) = self.s3("PUT", &format!("/{name}"), &[], b"").await;
        assert_eq!(
            status, 200,
            "the bucket {name:?} must be created, got {status} with {body:?}"
        );
        let (status, _, body) = self
            .s3(
                "PUT",
                &format!("/{name}?versioning"),
                &[("content-type", "application/xml")],
                b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            )
            .await;
        assert_eq!(
            status, 200,
            "versioning must be enabled on {name:?}, got {status} with {body:?}"
        );
    }

    /// Write an object, answering the version id the write reported.
    pub(crate) async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) -> Option<String> {
        let (status, headers, answer) = self
            .s3("PUT", &format!("/{bucket}/{key}"), &[], body)
            .await;
        assert_eq!(
            status, 200,
            "writing s3://{bucket}/{key} must succeed, got {status} with {answer:?}"
        );
        headers
            .get("x-amz-version-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// Call one KMS operation that must succeed.
    ///
    /// KMS speaks `awsJson1_1` under the `TrentService` target prefix
    /// (`smithy_query query="service_info"` -> `service: TrentService`,
    /// `protocols: [awsJson1_1]`).
    pub(crate) async fn kms(&self, operation: &str, body: &Value) -> Value {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/kms/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method("POST")
            .uri("https://kms.us-east-1.amazonaws.com/")
            .header("host", "kms.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Read an object's metadata: the status and the response headers, which is where
    /// S3 reports the server-side encryption an object carries.
    pub(crate) async fn head_object(&self, bucket: &str, key: &str) -> (u16, http::HeaderMap) {
        let (status, headers, _) = self
            .s3("HEAD", &format!("/{bucket}/{key}"), &[], b"")
            .await;
        (status, headers)
    }

    /// Read an object: the status and the bytes.
    pub(crate) async fn get_object(&self, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        let (status, _, body) = self.s3("GET", &format!("/{bucket}/{key}"), &[], b"").await;
        (status, body.to_vec())
    }
}

/// A pipeline declaration from its stages.
pub(crate) fn declaration(name: &str, stages: Value, execution_mode: Option<&str>) -> Value {
    let mut pipeline = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": artifact_bucket(name)},
        "stages": stages,
    });
    if let Some(mode) = execution_mode {
        pipeline["executionMode"] = json!(mode);
    }
    json!({"pipeline": pipeline})
}

/// The artifact-store bucket one test pipeline uses.
///
/// A pipeline name may carry upper case and punctuation an Amazon S3 bucket name may not
/// ("bucket names can consist only of lowercase letters, numbers, dots (.), and hyphens
/// (-)"), so the name is folded into the character class S3 holds a bucket under. This is
/// the one place the suite derives the bucket from the pipeline name, so the create and the
/// `artifactStore` declaration name the same bucket.
pub(crate) fn artifact_bucket(name: &str) -> String {
    let folded: String = name
        .chars()
        .map(|character| {
            let lowered = character.to_ascii_lowercase();
            match lowered.is_ascii_lowercase() || lowered.is_ascii_digit() {
                true => lowered,
                false => '-',
            }
        })
        .collect();
    format!("{}-artifacts", folded.trim_matches('-'))
}

/// An S3 source stage producing `output` from `bucket`/`key`.
pub(crate) fn source_stage(bucket: &str, key: &str, output: &str) -> Value {
    json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": {
                "category": "Source", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {"S3Bucket": bucket, "S3ObjectKey": key},
            "outputArtifacts": [{"name": output}],
            "namespace": "SourceVariables",
        }],
    })
}

/// An S3 deploy stage delivering `input` to `bucket`/`key`.
pub(crate) fn deploy_stage(name: &str, input: &str, bucket: &str, key: &str, extract: bool) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Dep",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {
                "BucketName": bucket,
                "ObjectKey": key,
                "Extract": if extract { "true" } else { "false" },
            },
            "inputArtifacts": [{"name": input}],
        }],
    })
}

/// The id of the execution a pipeline's own CREATION started.
///
/// Creating a pipeline runs it once — `TriggerType` carries `CreatePipeline` for
/// exactly that run — so a pipeline has an execution before anything starts one, and
/// this is how a suite names it.
///
/// `pub(crate)` with the harness: the suites that drive one execution of a freshly
/// created pipeline drive THAT run rather than growing a second copy of this lookup.
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

/// Poll `GetPipelineExecution` until the execution reaches a terminal status.
pub(crate) async fn wait_for_terminal(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
            )
            .await;
        let status = last["pipelineExecution"]["status"].as_str().unwrap_or("");
        if matches!(
            status,
            "Succeeded" | "Failed" | "Stopped" | "Superseded" | "Cancelled"
        ) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pipeline {pipeline}: execution {execution} never reached a terminal status; last was {last}");
}

/// The status of one execution right now.
pub(crate) async fn status_of(emulator: &Emulator, pipeline: &str, execution: &str) -> String {
    let answer = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
        )
        .await;
    answer["pipelineExecution"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A zip archive of `entries`, built by the `zip` crate (the same crate the deploy
/// action reads one with).
///
/// `pub(crate)` with the harness: the `s3_deploy_object_key` suite drives the same
/// extraction path through it rather than growing a second copy.
pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        for (name, body) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("the entry must start");
            writer.write_all(body).expect("the entry must be written");
        }
        writer.finish().expect("the archive must close");
    }
    cursor.into_inner()
}

#[tokio::test]
async fn an_execution_reaches_a_terminal_status_and_delivers_what_its_actions_produce() {
    // One row per class this table covers: a plain delivery, an extracted delivery and
    // a source object that is not there. (An action whose provider this emulator cannot
    // run has its own table in
    // `an_action_this_emulator_cannot_run_fails_its_execution_with_the_reason`.)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, bool, bool, &str)> = vec![
        ("plain delivery", true, false, "Succeeded"),
        ("extracted delivery", true, true, "Succeeded"),
        ("missing source object", false, false, "Failed"),
    ];

    for (case, write_source, extract, expected) in rows {
        let emulator = Emulator::new();
        let pipeline = format!("pipe-{}", case.replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;

        let entries: [(&str, &[u8]); 2] = [("one.txt", b"first entry"), ("two.txt", b"second")];
        let payload = if extract {
            zip_of(&entries)
        } else {
            b"the source bytes".to_vec()
        };
        if write_source {
            emulator
                .put_object("source-bucket", "source.zip", &payload)
                .await;
        }

        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        deploy_stage(
                            "Deploy",
                            "SourceArtifact",
                            "deploy-bucket",
                            if extract { "unpacked" } else { "delivered.zip" },
                            extract,
                        ),
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .expect("the start reports an execution id")
            .to_string();

        let final_answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        let status = final_answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            status, expected,
            "case {case:?}: expected the execution to reach {expected}, got {final_answer}"
        );

        if expected == "Succeeded" && !extract {
            let (status, bytes) = emulator.get_object("deploy-bucket", "delivered.zip").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the delivered object must be there, got {status}"
            );
            assert_eq!(
                bytes, payload,
                "case {case:?}: the delivered bytes must be the source bytes"
            );
        }
        if expected == "Succeeded" && extract {
            for (name, body) in entries {
                let (status, bytes) = emulator
                    .get_object("deploy-bucket", &format!("unpacked/{name}"))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the extracted entry {name} must be delivered, got {status}"
                );
                assert_eq!(
                    bytes, body,
                    "case {case:?}: the extracted entry {name} must carry its own bytes"
                );
            }
        }
        if expected == "Failed" {
            let (status, _) = emulator.get_object("deploy-bucket", "delivered.zip").await;
            assert_eq!(
                status, 404,
                "case {case:?}: a failed execution delivers nothing, got {status}"
            );
            let summary = final_answer["pipelineExecution"]["statusSummary"]
                .as_str()
                .unwrap_or_default();
            assert!(
                !summary.is_empty(),
                "case {case:?}: a failed execution reports why, got {final_answer}"
            );
        }
    }
}

#[tokio::test]
async fn an_action_this_emulator_cannot_run_fails_its_execution_with_the_reason() {
    // One row per action class no provider here can perform: nothing is reported as
    // having run, and the execution reaches the modelled Failed status.
    let rows = [
        ("a build action", "Build", "CodeBuild"),
        ("an invoke action", "Invoke", "Lambda"),
        ("a deploy through CloudFormation", "Deploy", "CloudFormation"),
    ];
    for (case, category, provider) in rows {
        let emulator = Emulator::new();
        let pipeline = format!("unrunnable-{provider}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"bytes")
            .await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        {
                            "name": "Work",
                            "actions": [{
                                "name": "Act",
                                "actionTypeId": {
                                    "category": category,
                                    "owner": "AWS",
                                    "provider": provider,
                                    "version": "1",
                                },
                                "inputArtifacts": [{"name": "SourceArtifact"}],
                            }],
                        },
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Failed"),
            "case {case:?}: an action nothing can run fails the execution, got {answer}"
        );

        let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
        let reported = state["stageStates"]
            .as_array()
            .expect("stage states are reported")
            .iter()
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .filter_map(|action| action["latestExecution"]["status"].as_str())
            .collect::<Vec<_>>();
        assert!(
            reported.contains(&"Failed"),
            "case {case:?}: the action itself is reported Failed, got {state}"
        );
    }
}

#[tokio::test]
async fn an_approval_action_holds_its_stage_until_the_token_is_answered() {
    let emulator = Emulator::new();
    let pipeline = "approval-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", b"approved bytes")
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    {
                        "name": "Gate",
                        "actions": [{
                            "name": "Approve",
                            "actionTypeId": {
                                "category": "Approval",
                                "owner": "AWS",
                                "provider": "Manual",
                                "version": "1",
                            },
                            "configuration": {"CustomData": "ship it?"},
                        }],
                    },
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "gated.zip", false),
                ]),
                None,
            ),
        )
        .await;
    // The execution under test is the one the pipeline's creation started: creating a
    // pipeline runs it once, and that run is what reaches the approval action.
    let execution = creation_run(&emulator, pipeline).await;

    // The approval token is published only once the action is REACHED.
    let deadline = Instant::now() + WAIT;
    let mut token = None;
    while Instant::now() < deadline && token.is_none() {
        let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
        token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!("Approve"))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if token.is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let token = token.expect("the approval action must publish a token");

    // Nothing downstream has run while the approval waits.
    let (status, _) = emulator.get_object("deploy-bucket", "gated.zip").await;
    assert_eq!(
        status, 404,
        "the artifact must not be delivered before the approval, got {status}"
    );
    assert_eq!(
        status_of(&emulator, pipeline, &execution).await,
        "InProgress",
        "the execution waits at the approval"
    );

    // A token that is not the one in force is refused, and the approval still waits.
    let (status, code) = emulator
        .error_code(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "actionName": "Approve",
                "result": {"summary": "stale", "status": "Approved"},
                "token": "00000000-0000-4000-8000-000000000000",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidApprovalTokenException"),
        "a stale token is refused"
    );

    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "actionName": "Approve",
                "result": {"summary": "looks good", "status": "Approved"},
                "token": token,
            }),
        )
        .await;

    let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        answer["pipelineExecution"]["status"].as_str(),
        Some("Succeeded"),
        "an approved execution runs on to the end, got {answer}"
    );
    let (status, bytes) = emulator.get_object("deploy-bucket", "gated.zip").await;
    assert_eq!(
        status, 200,
        "the artifact is delivered after the approval, got {status}"
    );
    assert_eq!(bytes, b"approved bytes".to_vec(), "the delivered bytes");

    // Answering the same approval again is refused: it already has a response.
    let (status, code) = emulator
        .error_code(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "actionName": "Approve",
                "result": {"summary": "again", "status": "Approved"},
                "token": token,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ApprovalAlreadyCompletedException"),
        "an approval that already answered is refused"
    );
}

#[tokio::test]
async fn a_rejected_approval_fails_the_execution_and_withholds_the_downstream_stage() {
    let emulator = Emulator::new();
    let pipeline = "rejection-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", b"rejected bytes")
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    {
                        "name": "Gate",
                        "actions": [{
                            "name": "Approve",
                            "actionTypeId": {
                                "category": "Approval",
                                "owner": "AWS",
                                "provider": "Manual",
                                "version": "1",
                            },
                        }],
                    },
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "nope.zip", false),
                ]),
                None,
            ),
        )
        .await;
    // Again the creation run is the execution under test, for the same reason.
    let execution = creation_run(&emulator, pipeline).await;

    let deadline = Instant::now() + WAIT;
    let mut token = None;
    while Instant::now() < deadline && token.is_none() {
        let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
        token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!("Approve"))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if token.is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let token = token.expect("the approval action must publish a token");

    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "actionName": "Approve",
                "result": {"summary": "not now", "status": "Rejected"},
                "token": token,
            }),
        )
        .await;

    let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        answer["pipelineExecution"]["status"].as_str(),
        Some("Failed"),
        "a rejected approval fails the execution, got {answer}"
    );
    let (status, _) = emulator.get_object("deploy-bucket", "nope.zip").await;
    assert_eq!(
        status, 404,
        "the downstream stage is withheld, got {status}"
    );

    // The reviewer's summary is what the action reports.
    let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
    let summary = state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|stage| stage["actionStates"].as_array())
        .flatten()
        .find(|action| action["actionName"] == json!("Approve"))
        .and_then(|action| action["latestExecution"]["summary"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        summary, "not now",
        "the rejection's summary is recorded on the action, got {state}"
    );
}

#[tokio::test]
async fn an_artifact_archive_past_the_extraction_bounds_is_refused() {
    // The archive is caller content, so its expansion is bounded: an entry that would
    // take the extraction past the bound, and a payload that is not an archive at all,
    // both fail the action with their reason rather than being allocated or delivered.
    for (case, payload) in [
        (
            "a payload that is not an archive",
            b"not a zip at all".to_vec(),
        ),
        (
            "an archive that expands past the bound",
            zip_of(&[(
                "huge.bin",
                &vec![
                    0u8;
                    service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_EXTRACTED_BYTES
                        + 1
                ][..],
            )]),
        ),
    ] {
        let emulator = Emulator::new();
        let pipeline = format!("archive-{}", case.replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", &payload)
            .await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "out", true),
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Failed"),
            "case {case:?}: the execution must fail, got {answer}"
        );
        let summary = answer["pipelineExecution"]["statusSummary"]
            .as_str()
            .unwrap_or_default();
        assert!(
            summary.contains("extracted"),
            "case {case:?}: the reason names the extraction, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_disabled_stage_transition_holds_the_execution_at_that_transition() {
    // The rule generalises over the `StageTransitionType`: an Inbound hold keeps the
    // execution OUT of the stage it names, an Outbound hold keeps it from LEAVING that
    // stage; each is reported, and each is released by enabling that same transition.
    // `StageState$inboundTransitionState` is the only transition member the model binds,
    // so only an Inbound hold is reported there.
    for (case, transition_type, held_stage, reported_on_deploy) in [
        ("Inbound on the deploy stage", "Inbound", "Deploy", true),
        ("Outbound on the source stage", "Outbound", "Source", false),
    ] {
        let emulator = Emulator::new();
        let pipeline = format!("transition-{}", case.replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"held bytes")
            .await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "held.zip", false),
                    ]),
                    None,
                ),
            )
            .await;
        emulator
            .ok(
                "DisableStageTransition",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": held_stage,
                    "transitionType": transition_type,
                    "reason": "waiting for the window",
                }),
            )
            .await;

        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();

        // Long enough for every step this pipeline owes, had the transition been enabled.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(
            status_of(&emulator, &pipeline, &execution).await,
            "InProgress",
            "case {case:?}: a held execution does not finish"
        );
        let (status, _) = emulator.get_object("deploy-bucket", "held.zip").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a held execution delivers nothing, got {status}"
        );

        let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
        let deploy = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|stage| stage["stageName"] == json!("Deploy"))
            .cloned()
            .expect("the Deploy stage is reported");
        assert_eq!(
            deploy["inboundTransitionState"]["enabled"],
            json!(!reported_on_deploy),
            "case {case:?}: only an Inbound hold is reported on inboundTransitionState, \
             got {deploy}"
        );
        assert_eq!(
            deploy["inboundTransitionState"]["disabledReason"].is_string(),
            reported_on_deploy,
            "case {case:?}: the reason is reported with the hold it belongs to, got \
             {deploy}"
        );

        // Enabling the OTHER transition type releases nothing: the two are independent.
        let other = if transition_type == "Inbound" {
            "Outbound"
        } else {
            "Inbound"
        };
        emulator
            .ok(
                "EnableStageTransition",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": held_stage,
                    "transitionType": other,
                }),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            status_of(&emulator, &pipeline, &execution).await,
            "InProgress",
            "case {case:?}: enabling the other transition type releases nothing"
        );

        emulator
            .ok(
                "EnableStageTransition",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": held_stage,
                    "transitionType": transition_type,
                }),
            )
            .await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the released execution runs to the end, got {answer}"
        );
        let (status, bytes) = emulator.get_object("deploy-bucket", "held.zip").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the artifact is delivered once released"
        );
        assert_eq!(
            bytes,
            b"held bytes".to_vec(),
            "case {case:?}: the delivered bytes"
        );
    }
}

#[tokio::test]
async fn stopping_an_execution_settles_it_and_a_second_stop_is_refused() {
    let emulator = Emulator::new();
    let pipeline = "stop-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", b"stopped bytes")
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "stopped.zip", false),
                ]),
                None,
            ),
        )
        .await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();

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
    assert_eq!(
        status_of(&emulator, pipeline, &execution).await,
        "Stopped",
        "an abandoning stop settles at once"
    );

    // Nothing runs after the stop, however long the clock runs on.
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(
        status_of(&emulator, pipeline, &execution).await,
        "Stopped",
        "a stopped execution stays stopped"
    );
    let (status, _) = emulator.get_object("deploy-bucket", "stopped.zip").await;
    assert_eq!(
        status, 404,
        "a stopped execution delivers nothing, got {status}"
    );

    let (status, code) = emulator
        .error_code(
            "StopPipelineExecution",
            &json!({
                "pipelineName": pipeline,
                "pipelineExecutionId": execution,
                "abandon": false,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineExecutionNotStoppableException"),
        "an execution that already stopped cannot be stopped again"
    );
}

#[tokio::test]
async fn the_execution_mode_decides_what_a_second_start_does() {
    // SUPERSEDED (the documented default) runs one execution and supersedes the one
    // already waiting; QUEUED runs them one after another; PARALLEL runs them together.
    for (case, mode) in [
        ("the default mode", None),
        ("SUPERSEDED", Some("SUPERSEDED")),
        ("QUEUED", Some("QUEUED")),
        ("PARALLEL", Some("PARALLEL")),
    ] {
        let emulator = Emulator::new();
        let pipeline = format!("mode-{}", case.replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"mode bytes")
            .await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "mode.zip", false),
                    ]),
                    mode,
                ),
            )
            .await;

        let mut ids = Vec::new();
        for _ in 0..3 {
            let started = emulator
                .ok("StartPipelineExecution", &json!({"name": pipeline}))
                .await;
            ids.push(started["pipelineExecutionId"].as_str().unwrap().to_string());
        }

        let superseded = mode.is_none() || mode == Some("SUPERSEDED");
        if superseded {
            // The middle execution never runs: the third start supersedes it.
            let status = status_of(&emulator, &pipeline, &ids[1]).await;
            assert_eq!(
                status, "Superseded",
                "case {case:?}: the queued execution is superseded, got {status}"
            );
        }
        for (index, id) in ids.iter().enumerate() {
            let status = status_of(&emulator, &pipeline, id).await;
            if status == "Superseded" {
                continue;
            }
            let answer = wait_for_terminal(&emulator, &pipeline, id).await;
            assert_eq!(
                answer["pipelineExecution"]["status"].as_str(),
                Some("Succeeded"),
                "case {case:?}: execution {index} must finish, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn a_start_past_the_pipelines_concurrency_ceiling_is_refused() {
    let emulator = Emulator::new();
    let pipeline = "ceiling-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", b"ceiling bytes")
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "ceiling.zip", false),
                ]),
                Some("QUEUED"),
            ),
        )
        .await;

    let ceiling =
        service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE;
    let mut accepted = 0usize;
    let mut refusals = Vec::new();
    for _ in 0..ceiling + 5 {
        let (status, answer) = emulator
            .cp("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        if status == 200 {
            accepted += 1;
        } else {
            refusals.push(
                answer["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('#')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    assert!(
        accepted <= ceiling,
        "no more than the ceiling of {ceiling} executions is accepted, got {accepted}"
    );
    assert!(
        !refusals.is_empty(),
        "a burst past the ceiling must be refused, got {accepted} successes and no refusal"
    );
    assert!(
        refusals
            .iter()
            .all(|code| code == "ConcurrentPipelineExecutionsLimitExceededException"),
        "the refusal is the modelled concurrency error, got {refusals:?}"
    );
}

#[tokio::test]
async fn an_artifact_store_whose_key_kms_will_not_issue_fails_with_that_reason() {
    // "You can configure a customer managed key to encrypt the artifacts in the artifact
    // store": encrypting an artifact object under that key is a USE of the key, so KMS
    // decides whether it can be used. A key the service does not hold cannot issue the
    // data key the artifact's encryption takes, and an artifact that cannot be encrypted
    // as the store declares is one that cannot be produced — the action fails with that
    // reason rather than reporting a success that claims an encryption nothing performed.
    // (The usable-key rule, and the key whose state changes under a running pipeline,
    // are the table in `tests/all/artifact_store_encryption.rs`.)
    let emulator = Emulator::new();
    let pipeline = "cmk-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", b"cmk bytes")
        .await;
    let absent_key = format!(
        "arn:aws:kms:us-east-1:000000000000:key/{}",
        "11111111-2222-4333-8444-555555555555"
    );
    let mut body = declaration(
        pipeline,
        json!([
            source_stage("source-bucket", "source.zip", "SourceArtifact"),
            deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "cmk.zip", false),
        ]),
        None,
    );
    body["pipeline"]["artifactStore"]["encryptionKey"] = json!({
        "id": absent_key,
        "type": "KMS",
    });
    emulator.ok("CreatePipeline", &body).await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();
    let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        answer["pipelineExecution"]["status"].as_str(),
        Some("Failed"),
        "an artifact store whose key cannot be used fails the execution, got {answer}"
    );
    let summary = answer["pipelineExecution"]["statusSummary"]
        .as_str()
        .unwrap_or_default();
    assert!(
        summary.contains("InternalError. Error reference code: "),
        "the reason is CodePipeline's internal-error shape: what KMS said about the key \
         is not relayed to the caller, got {summary:?}"
    );
    let (status, _) = emulator.head_object("deploy-bucket", "cmk.zip").await;
    assert_eq!(
        status, 404,
        "nothing is delivered when the artifact could not be encrypted, got {status}"
    );
}

#[tokio::test]
async fn a_source_revision_override_delivers_the_version_it_pins() {
    // `SourceRevisionOverride` pins the revision a source action reads; for an S3 source
    // that is the object VERSION, so the delivered bytes are that version's and not the
    // newest.
    let emulator = Emulator::new();
    let pipeline = "pinned-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    let first = emulator
        .put_object("source-bucket", "source.zip", b"the old revision")
        .await
        .expect("a versioned write reports a version id");
    emulator
        .put_object("source-bucket", "source.zip", b"the new revision")
        .await
        .expect("a versioned write reports a version id");

    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "pinned.zip", false),
                ]),
                None,
            ),
        )
        .await;

    for (case, pinned, expected) in [
        ("pinned to the old version", Some(first.as_str()), &b"the old revision"[..]),
        ("not pinned at all", None, &b"the new revision"[..]),
    ] {
        let mut request = json!({"name": pipeline});
        if let Some(version) = pinned {
            request["sourceRevisions"] = json!([{
                "actionName": "Src",
                "revisionType": "S3_OBJECT_VERSION_ID",
                "revisionValue": version,
            }]);
        }
        let started = emulator.ok("StartPipelineExecution", &request).await;
        let execution = started["pipelineExecutionId"].as_str().unwrap().to_string();
        let answer = wait_for_terminal(&emulator, pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the execution must finish, got {answer}"
        );
        let (status, bytes) = emulator.get_object("deploy-bucket", "pinned.zip").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the artifact must be delivered, got {status}"
        );
        assert_eq!(
            bytes, expected,
            "case {case:?}: the delivered bytes are the pinned revision's"
        );
    }
}

#[tokio::test]
async fn only_the_latest_executions_failed_stage_can_be_retried() {
    let emulator = Emulator::new();
    let pipeline = "retry-pipe";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                // No source object is written, so the source stage fails.
                json!([
                    source_stage("source-bucket", "absent.zip", "SourceArtifact"),
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "retried.zip", false),
                ]),
                Some("PARALLEL"),
            ),
        )
        .await;

    let mut ids = Vec::new();
    for _ in 0..2 {
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let id = started["pipelineExecutionId"].as_str().unwrap().to_string();
        wait_for_terminal(&emulator, pipeline, &id).await;
        ids.push(id);
    }

    // The older execution is not retryable at all: it has been superseded, so the stage
    // holds no failed action a retry could re-run.
    let (status, response) = emulator
        .cp(
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Source",
                "pipelineExecutionId": ids[0],
                "retryMode": "FAILED_ACTIONS",
            }),
        )
        .await;
    let code = response["__type"].as_str().unwrap_or_default();
    assert_eq!(
        (status, code),
        (400, "StageNotRetryableException"),
        "retrying an older execution is refused, got {response}"
    );
    assert_eq!(
        response["message"].as_str().unwrap_or_default(),
        format!(
            "The stage named Source in the {pipeline} pipeline can't be retried now. Either \
             it contains no failed actions, the execution '{}' is already superseded, or the \
             pipeline definition has changed since the action(s) failed.",
            ids[0]
        ),
        "the refusal states the three conditions, got {response}"
    );

    // The latest one is: with the source object in place, the retried stage succeeds.
    emulator
        .put_object("source-bucket", "absent.zip", b"now present")
        .await;
    emulator
        .ok(
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Source",
                "pipelineExecutionId": ids[1],
                "retryMode": "FAILED_ACTIONS",
            }),
        )
        .await;
    let answer = wait_for_terminal(&emulator, pipeline, &ids[1]).await;
    assert_eq!(
        answer["pipelineExecution"]["status"].as_str(),
        Some("Succeeded"),
        "the retried stage runs again and succeeds, got {answer}"
    );

    // A stage that did not fail is not retryable.
    let (status, code) = emulator
        .error_code(
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Source",
                "pipelineExecutionId": ids[1],
                "retryMode": "FAILED_ACTIONS",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StageNotRetryableException"),
        "retrying a stage that did not fail is refused"
    );
}
