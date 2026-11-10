// SPDX-License-Identifier: CC0-1.0

//! Stage transitions, end to end over the wire the service speaks: `awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`.
//!
//! The rule under test: a stage transition's enablement is state of the pipeline,
//! addressed by `(pipelineName, stageName, transitionType)`, so the state a read
//! reports for a transition is exactly what the LAST `DisableStageTransition` /
//! `EnableStageTransition` call for THAT transition left — every other transition of
//! the pipeline untouched — and a call naming a pipeline or a stage the store does
//! not hold is refused with the modelled not-found error and persists nothing.
//!
//! Every assertion goes through the router, so nothing here can pass by reading a
//! handler's internals.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern
/// (`^arn:aws(-[\w]+)*:iam::[0-9]{12}:role/`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// Every value `StageTransitionType` declares
/// (`smithy_query query="shape" shape="StageTransitionType"` -> `Inbound`,
/// `Outbound`).
const TRANSITION_TYPES: [&str; 2] = ["Inbound", "Outbound"];

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=0123456789abcdef",
            )
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

    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// The `(status, __type)` of a refusal — the awsJson error code clients match on.
    async fn expect_error(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    async fn create(&self, name: &str, stages: &[&str]) {
        self.ok("CreatePipeline", &json!({"pipeline": declaration(name, stages)}))
            .await;
    }

    async fn pipeline_state(&self, name: &str) -> Value {
        self.ok("GetPipelineState", &json!({"name": name})).await
    }
}

/// A valid declaration whose stages are `stages`, in that order: the first stage
/// sources an artifact and each later stage consumes it.
fn declaration(name: &str, stages: &[&str]) -> Value {
    let stages: Vec<Value> = stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            if index == 0 {
                json!({
                    "name": stage,
                    "actions": [{
                        "name": format!("{stage}Source"),
                        "actionTypeId": {
                            "category": "Source",
                            "owner": "AWS",
                            "provider": "S3",
                            "version": "1",
                        },
                        "configuration": {
                            "S3Bucket": "my-bucket",
                            "S3ObjectKey": "source.zip",
                        },
                        "outputArtifacts": [{"name": "SourceOut"}],
                    }],
                })
            } else {
                json!({
                    "name": stage,
                    "actions": [{
                        "name": format!("{stage}Build"),
                        "actionTypeId": {
                            "category": "Build",
                            "owner": "AWS",
                            "provider": "CodeBuild",
                            "version": "1",
                        },
                        "configuration": {"ProjectName": format!("project-{index}")},
                        "inputArtifacts": [{"name": "SourceOut"}],
                        "runOrder": 1,
                    }],
                })
            }
        })
        .collect();
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// The `stageStates` entry for `stage`, or `None` when the reader reported no such
/// stage.
fn stage_state(state: &Value, stage: &str) -> Option<Value> {
    state["stageStates"]
        .as_array()?
        .iter()
        .find(|reported| reported["stageName"] == json!(stage))
        .cloned()
}

/// The `inboundTransitionState` the reader reports for `stage`, or `None` when the
/// member is absent.
fn inbound_transition(state: &Value, stage: &str) -> Option<Value> {
    let stage_state = stage_state(state, stage)
        .unwrap_or_else(|| panic!("no stageStates entry for {stage} in {state}"));
    match stage_state.get("inboundTransitionState") {
        None | Some(Value::Null) => None,
        Some(transition) => Some(transition.clone()),
    }
}

/// `TransitionState$enabled` as the wire carries it. The member is modelled
/// `@default false`, so an absent member is the default rather than "unknown".
fn enabled(transition: &Value) -> bool {
    transition
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Every stage's `inboundTransitionState` enablement, in reported order: `None`
/// where the member is absent.
fn inbound_enablement(state: &Value, stages: &[&str]) -> Vec<Option<bool>> {
    stages
        .iter()
        .map(|stage| inbound_transition(state, stage).map(|transition| enabled(&transition)))
        .collect()
}

/// Disabling ONE transition is reported as the disabled state of exactly that
/// transition, over every (stage, transitionType) pair of pipelines of different
/// stage counts; enabling it again restores the enabled state.
///
/// `StageState` models an `inboundTransitionState` and no outbound counterpart, so
/// an `Outbound` transition is persisted but has no member to report: the case
/// asserts it changes no stage's inbound state rather than inventing one.
#[tokio::test]
async fn disabling_one_transition_changes_only_that_transitions_reported_state() {
    let shapes: Vec<Vec<&str>> = vec![
        vec!["Source", "Build"],
        vec!["Source", "Build", "Test", "Deploy"],
    ];
    let reason = "Waiting for manual approval";

    for stages in shapes {
        for transition_type in TRANSITION_TYPES {
            for target in &stages {
                let case = format!(
                    "{} stage(s), {transition_type} of {target}",
                    stages.len()
                );
                let emulator = Emulator::new();
                emulator.create("demo", &stages).await;

                emulator
                    .ok(
                        "DisableStageTransition",
                        &json!({
                            "pipelineName": "demo",
                            "stageName": target,
                            "transitionType": transition_type,
                            "reason": reason,
                        }),
                    )
                    .await;

                let state = emulator.pipeline_state("demo").await;
                for stage in &stages {
                    let disabled_here =
                        transition_type == "Inbound" && stage == target;
                    let reported = inbound_transition(&state, stage);
                    // EVERY stage reports its inbound transition state, the first
                    // stage's included: the member states whether the transition into
                    // the stage is enabled, which is a fact every stage has.
                    assert!(
                        reported.is_some(),
                        "case {case:?}: stage {stage} reports no \
                         inboundTransitionState, got {state}"
                    );
                    let Some(reported) = reported else { continue };
                    assert_eq!(
                        enabled(&reported),
                        !disabled_here,
                        "case {case:?}: stage {stage} enabled: expected {}, got {reported}",
                        !disabled_here
                    );
                    assert_eq!(
                        reported.get("disabledReason").and_then(Value::as_str),
                        disabled_here.then_some(reason),
                        "case {case:?}: stage {stage} disabledReason, got {reported}"
                    );
                    assert_eq!(
                        reported.get("lastChangedAt").is_some(),
                        disabled_here,
                        "case {case:?}: stage {stage} lastChangedAt is reported only \
                         for a transition a call changed, got {reported}"
                    );
                }

                emulator
                    .ok(
                        "EnableStageTransition",
                        &json!({
                            "pipelineName": "demo",
                            "stageName": target,
                            "transitionType": transition_type,
                        }),
                    )
                    .await;

                let state = emulator.pipeline_state("demo").await;
                assert!(
                    inbound_enablement(&state, &stages)
                        .iter()
                        .all(|reported| reported.is_none_or(|enabled| enabled)),
                    "case {case:?}: enabling restores every transition, got {state}"
                );
                if transition_type == "Inbound" {
                    let reported = inbound_transition(&state, target).unwrap_or_else(|| {
                        panic!("case {case:?}: a changed transition stays reported: {state}")
                    });
                    assert_eq!(
                        reported.get("disabledReason"),
                        None,
                        "case {case:?}: an enabled transition carries no reason, got {reported}"
                    );
                    assert!(
                        reported.get("lastChangedAt").is_some(),
                        "case {case:?}: the enable is itself a change, got {reported}"
                    );
                }
            }
        }
    }
}

/// Each transition of a pipeline is its own state: disabling several accumulates,
/// and the LAST call for one transition is what the reader reports for it.
#[tokio::test]
async fn transitions_are_independent_and_the_last_change_wins() {
    let stages = ["Source", "Build", "Test", "Deploy"];
    let emulator = Emulator::new();
    emulator.create("demo", &stages).await;

    for (stage, reason) in [("Build", "Build held (1)"), ("Deploy", "Deploy held")] {
        emulator
            .ok(
                "DisableStageTransition",
                &json!({
                    "pipelineName": "demo",
                    "stageName": stage,
                    "transitionType": "Inbound",
                    "reason": reason,
                }),
            )
            .await;
    }
    let state = emulator.pipeline_state("demo").await;
    assert_eq!(
        inbound_enablement(&state, &stages),
        vec![Some(true), Some(false), Some(true), Some(false)],
        "case \"two disabled, one untouched\": got {state}"
    );

    // Re-disabling one transition replaces its reason and leaves the other alone.
    emulator
        .ok(
            "DisableStageTransition",
            &json!({
                "pipelineName": "demo",
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "Build held (2)",
            }),
        )
        .await;
    let state = emulator.pipeline_state("demo").await;
    assert_eq!(
        inbound_transition(&state, "Build")
            .and_then(|transition| transition["disabledReason"].as_str().map(str::to_string)),
        Some("Build held (2)".to_string()),
        "case \"re-disabled\": the last reason is what is reported, got {state}"
    );
    assert_eq!(
        inbound_transition(&state, "Deploy")
            .and_then(|transition| transition["disabledReason"].as_str().map(str::to_string)),
        Some("Deploy held".to_string()),
        "case \"re-disabled\": another transition keeps its own reason, got {state}"
    );

    // Enabling one leaves the other disabled.
    emulator
        .ok(
            "EnableStageTransition",
            &json!({
                "pipelineName": "demo",
                "stageName": "Build",
                "transitionType": "Inbound",
            }),
        )
        .await;
    let state = emulator.pipeline_state("demo").await;
    assert_eq!(
        inbound_enablement(&state, &stages),
        vec![Some(true), Some(true), Some(true), Some(false)],
        "case \"one re-enabled\": got {state}"
    );
}

/// The reason is carried through exactly as given, across the forms and both
/// published bounds of `DisabledReason` (len 1..300, pattern
/// `^[a-zA-Z0-9!@ \(\)\.\*\?\-]+$`), and a reason outside them is refused at the
/// boundary without persisting anything.
#[tokio::test]
async fn the_stored_reason_spans_disabled_reasons_modelled_class() {
    // (case, reason) — every row inside the modelled length and pattern.
    let accepted: Vec<(&str, String)> = vec![
        ("one character (the minimum length)", "a".to_string()),
        ("a sentence", "Waiting for manual approval".to_string()),
        (
            "punctuation the pattern allows",
            "Held! (ticket OPS-1234) @ 10.30 - why? *see runbook*".to_string(),
        ),
        ("digits only", "1234567890".to_string()),
        ("300 characters (the maximum length)", "x".repeat(300)),
    ];
    for (case, reason) in accepted {
        let emulator = Emulator::new();
        emulator.create("demo", &["Source", "Build"]).await;
        emulator
            .ok(
                "DisableStageTransition",
                &json!({
                    "pipelineName": "demo",
                    "stageName": "Build",
                    "transitionType": "Inbound",
                    "reason": reason,
                }),
            )
            .await;
        let state = emulator.pipeline_state("demo").await;
        assert_eq!(
            inbound_transition(&state, "Build")
                .and_then(|transition| transition["disabledReason"].as_str().map(str::to_string)),
            Some(reason.clone()),
            "case {case:?}: the stored reason is the one given, got {state}"
        );
    }

    // (case, reason) — every row outside the modelled length or pattern.
    let refused: Vec<(&str, String)> = vec![
        ("empty (one below the minimum length)", String::new()),
        ("301 characters (one past the maximum)", "x".repeat(301)),
        (
            "a character outside the pattern",
            "waiting_for_approval".to_string(),
        ),
    ];
    for (case, reason) in refused {
        let emulator = Emulator::new();
        emulator.create("demo", &["Source", "Build"]).await;
        let (status, code) = emulator
            .expect_error(
                "DisableStageTransition",
                &json!({
                    "pipelineName": "demo",
                    "stageName": "Build",
                    "transitionType": "Inbound",
                    "reason": reason,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: unexpected refusal"
        );
        let state = emulator.pipeline_state("demo").await;
        assert_eq!(
            inbound_enablement(&state, &["Source", "Build"]),
            vec![Some(true), Some(true)],
            "case {case:?}: a refused call persists nothing, got {state}"
        );
    }
}

/// A transition call naming something the store does not hold is refused with the
/// modelled error for exactly that, and changes no stored transition — for both the
/// disabling and the enabling call.
#[tokio::test]
async fn a_transition_that_names_no_stored_target_is_refused() {
    // (case, pipelineName, stageName, transitionType, expected error code)
    let cases: [(&str, &str, &str, &str, &str); 5] = [
        (
            "unknown pipeline",
            "missing",
            "Build",
            "Inbound",
            "PipelineNotFoundException",
        ),
        (
            "unknown stage",
            "demo",
            "Nope",
            "Inbound",
            "StageNotFoundException",
        ),
        (
            "a stage of another pipeline",
            "demo",
            "Other",
            "Outbound",
            "StageNotFoundException",
        ),
        (
            "a transitionType outside the modelled enum",
            "demo",
            "Build",
            "Sideways",
            "ValidationException",
        ),
        (
            "a transitionType in the wrong case",
            "demo",
            "Build",
            "inbound",
            "ValidationException",
        ),
    ];

    for (case, pipeline_name, stage_name, transition_type, expected) in cases {
        for operation in ["DisableStageTransition", "EnableStageTransition"] {
            let emulator = Emulator::new();
            emulator.create("demo", &["Source", "Build"]).await;
            emulator.create("other", &["Other", "OtherBuild"]).await;
            let mut body = json!({
                "pipelineName": pipeline_name,
                "stageName": stage_name,
                "transitionType": transition_type,
            });
            if operation == "DisableStageTransition" {
                body["reason"] = json!("Held pending review");
            }
            let (status, code) = emulator.expect_error(operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, expected),
                "case {case:?} on {operation}: unexpected refusal"
            );
            let state = emulator.pipeline_state("demo").await;
            assert_eq!(
                inbound_enablement(&state, &["Source", "Build"]),
                vec![Some(true), Some(true)],
                "case {case:?} on {operation}: a refused call persists nothing, got {state}"
            );
        }
    }
}

/// `GetPipelineState` answers from the stored declaration: the stages in
/// declaration order with their action names, the stored version and instants, a
/// not-found refusal for a pipeline the store does not hold — and it writes
/// nothing, so a second read answers the same.
#[tokio::test]
async fn get_pipeline_state_reports_the_stored_pipeline_without_changing_it() {
    let emulator = Emulator::new();
    let stages = ["Source", "Build", "Deploy"];
    emulator.create("demo", &stages).await;

    let state = emulator.pipeline_state("demo").await;
    assert_eq!(
        state["pipelineName"],
        json!("demo"),
        "the reader names the pipeline it read, got {state}"
    );
    assert_eq!(
        state["pipelineVersion"],
        json!(1),
        "a new pipeline is version 1, got {state}"
    );
    for member in ["created", "updated"] {
        assert!(
            state[member].is_number(),
            "{member} must be a timestamp, got {state}"
        );
    }
    assert_eq!(
        state["stageStates"]
            .as_array()
            .map(|reported| reported
                .iter()
                .map(|stage| stage["stageName"].clone())
                .collect::<Vec<Value>>()),
        Some(stages.iter().map(|stage| json!(stage)).collect::<Vec<Value>>()),
        "the reader reports the stages in declaration order, got {state}"
    );
    for (index, stage) in stages.iter().enumerate() {
        let expected = if index == 0 {
            format!("{stage}Source")
        } else {
            format!("{stage}Build")
        };
        assert_eq!(
            stage_state(&state, stage).map(|reported| reported["actionStates"].clone()),
            Some(json!([{"actionName": expected}])),
            "stage {stage} reports its stored action names, got {state}"
        );
    }

    let again = emulator.pipeline_state("demo").await;
    assert_eq!(
        again, state,
        "a read must not change what the next caller sees"
    );

    let (status, code) = emulator
        .expect_error("GetPipelineState", &json!({"name": "missing"}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNotFoundException"),
        "an unknown pipeline has no state to report"
    );
}

/// A transition belongs to its pipeline: deleting the pipeline takes its stored
/// transitions with it, so a pipeline recreated under the same name starts with
/// every transition enabled.
#[tokio::test]
async fn deleting_a_pipeline_forgets_its_transitions() {
    let stages = ["Source", "Build"];
    let emulator = Emulator::new();
    emulator.create("demo", &stages).await;
    emulator
        .ok(
            "DisableStageTransition",
            &json!({
                "pipelineName": "demo",
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "Held pending review",
            }),
        )
        .await;
    emulator.ok("DeletePipeline", &json!({"name": "demo"})).await;
    emulator.create("demo", &stages).await;

    let state = emulator.pipeline_state("demo").await;
    assert_eq!(
        inbound_enablement(&state, &stages),
        vec![Some(true), Some(true)],
        "a recreated pipeline carries no transition the deleted one disabled, got {state}"
    );
    assert_eq!(
        inbound_transition(&state, "Build").and_then(|transition| transition
            .get("disabledReason")
            .cloned()),
        None,
        "nor the reason it was disabled with, got {state}"
    );
}
