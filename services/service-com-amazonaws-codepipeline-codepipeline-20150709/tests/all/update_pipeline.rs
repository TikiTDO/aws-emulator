// SPDX-License-Identifier: CC0-1.0

//! `UpdatePipeline`, end to end over the wire the service speaks (`awsJson1_1`).
//!
//! The rule under test: an accepted `UpdatePipeline` REPLACES the stored
//! `PipelineDeclaration` with the full structure the caller sent, stores it at the
//! stored version plus 1 (whatever `version` the sent declaration carried), keeps the
//! earlier versions readable, leaves the pipeline's identity — its name, ARN and
//! creation instant — alone, and answers with the declaration as stored.
//!
//! Every case is driven through the router and read back through `GetPipeline`,
//! `ListPipelines` and the execution plane, so nothing here can pass on an echoed
//! request: a response not computed from the shared store fails.

use serde_json::{json, Value};

// The harness, the base declaration and the stage builders are shared with the
// create/read suite rather than copied (`tests/all/pipeline_lifecycle.rs`).
use crate::pipeline_lifecycle::{
    build_stage, declaration, reported_stages, source_stage, Emulator, ROLE_ARN,
};

/// A cross-region `artifactStores` map, the other form of artifact store a
/// declaration can carry (`PipelineDeclaration$artifactStores`).
fn artifact_stores() -> Value {
    json!({
        "us-east-1": {"type": "S3", "location": "east"},
        "eu-west-1": {"type": "S3", "location": "west"},
    })
}

/// The table: each row is a DIFFERENT KIND of edit to the same stored pipeline —
/// stage count, stage identity, artifact-store form, V2 parameters, the explicitly
/// declared `pipelineType`/`executionMode`, the role, a stale `version` echoed back,
/// and a present-but-empty collection. The expected `pipelineType` /
/// `executionMode` are the ones the declaration resolves to, which is the rule
/// `PipelineDeclaration$pipelineType` and `$executionMode` document and is applied to
/// an update exactly as to a create.
#[tokio::test]
async fn an_update_replaces_the_stored_structure_at_the_next_version() {
    // (case, the declaration sent to UpdatePipeline, pipelineType, executionMode)
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "stages added",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 2), build_stage("Test", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "the source stage replaced by a different one",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                // A pipeline's first stage is its SOURCE stage (AWS: "the first stage
                // must contain at least one source action, and can only contain source
                // actions", enforced by `state::validate_declaration`), so a
                // replacement of the source stage is a different SOURCE stage.
                "stages": [source_stage("Origin"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "artifactStore replaced by a cross-region artifactStores map",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStores": artifact_stores(),
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "V2 parameters added, so the type resolves to V2",
            json!({
                "name": "demo",
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
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                "pipelineType": "V2",
                "executionMode": "PARALLEL",
            }),
            "V2",
            "PARALLEL",
        ),
        (
            "a different role and an encryption key on the store",
            json!({
                "name": "demo",
                "roleArn": "arn:aws:iam::123456789012:role/other",
                "artifactStore": {
                    "type": "S3",
                    "location": "other-bucket",
                    "encryptionKey": {"id": "alias/cp", "type": "KMS"},
                },
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "the declaration read back from GetPipeline, carrying its stale version",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                // The version the pipeline had when it was read: the service owns
                // this member, so the update does not take it from the request.
                "version": 1,
                "pipelineType": "V1",
                "executionMode": "SUPERSEDED",
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "a present-but-empty artifactStores map beside the artifactStore",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "artifactStores": {},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
            }),
            "V1",
            "SUPERSEDED",
        ),
        (
            "a present-but-empty variables list is no V2 parameter",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source"), build_stage("Build", 1)],
                "variables": [],
            }),
            "V1",
            "SUPERSEDED",
        ),
    ];

    for (case, updated, pipeline_type, execution_mode) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
            .await;
        let before = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;

        let answered = emulator
            .ok("UpdatePipeline", &json!({"pipeline": updated}))
            .await;
        let reported = answered["pipeline"].clone();

        assert_eq!(
            reported["version"],
            json!(2),
            "case {case:?}: an update stores the next version, got {reported}"
        );
        assert_eq!(
            reported["name"],
            json!("demo"),
            "case {case:?}: the update names the pipeline it replaced, got {reported}"
        );
        // The WHOLE stored declaration, member for member: the structure the caller
        // sent, with only the three members the service owns resolved. Comparing the
        // whole value is what makes "as sent" strict — a member the row does not
        // send must be ABSENT from the answer, and a member the service invented
        // would show up here.
        let mut expected = updated.clone();
        expected["version"] = json!(2);
        expected["pipelineType"] = json!(pipeline_type);
        expected["executionMode"] = json!(execution_mode);
        // An action's artifact lists are reported present, empty where the action
        // declares none, so the sent stages take that form here too.
        expected["stages"] = reported_stages(&updated);
        assert_eq!(
            reported, expected,
            "case {case:?}: the stored declaration must be the sent structure with \
             version, pipelineType and executionMode resolved, got {reported}"
        );

        // The update is PERSISTED: the read reports the same declaration, not the
        // one the create stored.
        let after = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
        assert_eq!(
            after["pipeline"], reported,
            "case {case:?}: GetPipeline must report the updated declaration"
        );
        assert_ne!(
            after["pipeline"], before["pipeline"],
            "case {case:?}: the stored declaration must have changed, got {after}"
        );

        // The pipeline's identity is fixed at create time.
        assert_eq!(
            after["metadata"]["pipelineArn"], before["metadata"]["pipelineArn"],
            "case {case:?}: an update must not move the ARN, got {after}"
        );
        assert_eq!(
            after["metadata"]["created"], before["metadata"]["created"],
            "case {case:?}: an update must not move the creation instant, got {after}"
        );
        let (updated_at, created_at) = (
            after["metadata"]["updated"].as_f64(),
            after["metadata"]["created"].as_f64(),
        );
        assert!(
            updated_at >= created_at,
            "case {case:?}: updated must not precede created, got {after}"
        );

        // The earlier version is still readable, and still the structure the create
        // stored; the version past the newest one was never created.
        let at_one = emulator
            .ok("GetPipeline", &json!({"name": "demo", "version": 1}))
            .await;
        assert_eq!(
            at_one["pipeline"], before["pipeline"],
            "case {case:?}: version 1 must still read the created structure"
        );
        let at_two = emulator
            .ok("GetPipeline", &json!({"name": "demo", "version": 2}))
            .await;
        assert_eq!(
            at_two["pipeline"], reported,
            "case {case:?}: version 2 must read the updated structure"
        );
        let (status, code) = emulator
            .expect_error("GetPipeline", &json!({"name": "demo", "version": 3}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineVersionNotFoundException"),
            "case {case:?}: version 3 was never created"
        );

        // Every other reader of the stored pipeline follows the update too.
        let listed = emulator.ok("ListPipelines", &json!({})).await;
        let summary = listed["pipelines"][0].clone();
        assert_eq!(
            (
                summary["name"].clone(),
                summary["version"].clone(),
                summary["pipelineType"].clone(),
                summary["executionMode"].clone(),
            ),
            (
                json!("demo"),
                json!(2),
                json!(pipeline_type),
                json!(execution_mode),
            ),
            "case {case:?}: the summary must agree with the updated pipeline, got {summary}"
        );

        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": "demo"}))
            .await;
        let execution_id = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an execution id, got {started}"))
            .to_string();
        let execution = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": "demo", "pipelineExecutionId": execution_id}),
            )
            .await;
        let execution = execution["pipelineExecution"].clone();
        assert_eq!(
            execution["pipelineVersion"],
            json!(2),
            "case {case:?}: an execution started after the update runs the new version, got {execution}"
        );
        assert_eq!(
            execution["executionMode"],
            json!(execution_mode),
            "case {case:?}: the execution inherits the updated executionMode, got {execution}"
        );
    }
}

/// Repeated updates step the version by exactly one each time, and every version
/// keeps the structure it was stored with — the invariant behind reading a pipeline
/// "at" a version.
#[tokio::test]
async fn each_update_steps_the_version_by_one_and_every_version_keeps_its_structure() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;

    // version number -> the declaration stored at it, starting with the create.
    let mut expected: Vec<(i64, Value)> = vec![(1, created["pipeline"].clone())];

    for step in 1..=5u64 {
        let mut sent = declaration("demo");
        // Each update is a distinguishable structure: `step` build actions in a
        // stage named for the step.
        sent["stages"] = json!([
            source_stage("Source"),
            build_stage(&format!("Build{step}"), usize::try_from(step).expect("a small step")),
        ]);
        let answered = emulator
            .ok("UpdatePipeline", &json!({"pipeline": sent}))
            .await;
        let version = i64::try_from(step + 1).expect("a small version");
        assert_eq!(
            answered["pipeline"]["version"],
            json!(version),
            "step {step}: update {step} must store version {version}, got {answered}"
        );
        expected.push((version, answered["pipeline"].clone()));
    }

    for (version, declaration_at) in &expected {
        let read = emulator
            .ok("GetPipeline", &json!({"name": "demo", "version": version}))
            .await;
        assert_eq!(
            read["pipeline"], *declaration_at,
            "version {version} must still read the structure stored at it, got {read}"
        );
    }

    let current = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
    let (newest_version, newest) = expected.last().expect("at least the create");
    assert_eq!(
        current["pipeline"], *newest,
        "the current declaration is the newest version, got {current}"
    );
    assert_eq!(
        current["pipeline"]["version"],
        json!(newest_version),
        "the current version is the last one stored, got {current}"
    );

    let (status, code) = emulator
        .expect_error(
            "GetPipeline",
            &json!({"name": "demo", "version": newest_version + 1}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineVersionNotFoundException"),
        "no version past the newest one was ever stored"
    );
}

/// A name no stored pipeline carries is refused — an update creates nothing — and
/// the pipelines that ARE stored are left untouched.
#[tokio::test]
async fn an_update_of_a_name_that_is_not_stored_is_refused() {
    // (case, the name to update, whether a pipeline named "demo" is stored first)
    let cases: [(&str, &str, bool); 3] = [
        ("an empty store", "ghost", false),
        ("another name is stored", "ghost", true),
        ("the name was deleted", "demo", false),
    ];

    for (case, name, create_demo) in cases {
        let emulator = Emulator::new();
        if create_demo {
            emulator
                .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
                .await;
        }
        if name == "demo" {
            // Stored, then deleted: the name resolved once and no longer does.
            emulator
                .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
                .await;
            emulator
                .ok("DeletePipeline", &json!({"name": "demo"}))
                .await;
        }
        let before = emulator.ok("ListPipelines", &json!({})).await;

        let mut sent = declaration(name);
        sent["stages"] = json!([build_stage("Build", 1)]);
        let (status, code) = emulator
            .expect_error("UpdatePipeline", &json!({"pipeline": sent}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: updating the unstored name {name:?} must be refused"
        );

        let after = emulator.ok("ListPipelines", &json!({})).await;
        assert_eq!(
            after["pipelines"], before["pipelines"],
            "case {case:?}: a refused update must store nothing, got {after}"
        );
        let (status, code) = emulator
            .expect_error("GetPipeline", &json!({"name": name}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: the refused update must not have created {name:?}"
        );
    }
}

/// A persisted stage transition belongs to the stage the STORED structure declares:
/// an update that no longer declares a stage drops that stage's transition record,
/// so a later structure reusing the name starts from the default (enabled, with no
/// reason) instead of inheriting a disabling aimed at a different structure.
#[tokio::test]
async fn an_update_drops_the_transitions_of_the_stages_it_no_longer_declares() {
    let emulator = Emulator::new();
    let with_build = json!({
        "name": "demo",
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [
            source_stage("Source"),
            build_stage("Build", 1),
            build_stage("Test", 1),
        ],
    });
    emulator
        .ok("CreatePipeline", &json!({"pipeline": with_build}))
        .await;

    // Disable the Build stage's inbound transition, and see it reported.
    emulator
        .ok(
            "DisableStageTransition",
            &json!({
                "pipelineName": "demo",
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "holding the deploy",
            }),
        )
        .await;
    let state = emulator.ok("GetPipelineState", &json!({"name": "demo"})).await;
    assert_eq!(
        state["stageStates"][1]["inboundTransitionState"]["enabled"],
        json!(false),
        "the disabled transition must be reported before the update, got {state}"
    );

    // Update to a structure WITHOUT the Build stage, then back to one with it.
    let without_build = json!({
        "name": "demo",
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Test", 1)],
    });
    emulator
        .ok("UpdatePipeline", &json!({"pipeline": without_build}))
        .await;
    emulator
        .ok("UpdatePipeline", &json!({"pipeline": with_build}))
        .await;

    let state = emulator.ok("GetPipelineState", &json!({"name": "demo"})).await;
    let reported = state["stageStates"][1]["inboundTransitionState"].clone();
    assert_eq!(
        reported["enabled"],
        json!(true),
        "a stage the update re-declared must start enabled, got {state}"
    );
    assert_eq!(
        reported["disabledReason"],
        Value::Null,
        "no reason from the dropped stage may survive the update, got {state}"
    );
    assert_eq!(
        state["stageStates"][0]["inboundTransitionState"]["enabled"],
        json!(true),
        "the first stage reports its inbound transition, enabled, got {state}"
    );
}

/// A declaration whose structure cannot describe a pipeline is refused with the
/// error shape the model declares for that defect, and the stored pipeline keeps the
/// version and the structure it had.
#[tokio::test]
async fn a_structurally_impossible_update_is_refused_and_changes_nothing() {
    // (case, the declaration sent to UpdatePipeline, expected awsJson error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no stages at all",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [],
            }),
            "InvalidStructureException",
        ),
        (
            "a stage with no actions",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [{"name": "Source", "actions": []}, build_stage("Build", 1)],
            }),
            "InvalidStageDeclarationException",
        ),
        (
            "a middle stage with no actions",
            json!({
                "name": "demo",
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
                "name": "demo",
                "roleArn": ROLE_ARN,
                "stages": [source_stage("Source")],
            }),
            "ValidationException",
        ),
        (
            "both artifactStore and artifactStores",
            json!({
                "name": "demo",
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "artifactStores": artifact_stores(),
                "stages": [source_stage("Source")],
            }),
            "InvalidStructureException",
        ),
        (
            // Refused at the boundary by the model's own constraints, before the
            // handler runs: `RoleArn` carries a pattern.
            "a roleArn that is not an IAM role ARN",
            json!({
                "name": "demo",
                "roleArn": "not-an-arn",
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [source_stage("Source")],
            }),
            "ValidationException",
        ),
    ];

    for (case, sent, expected) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
            .await;
        let before = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;

        let (status, code) = emulator
            .expect_error("UpdatePipeline", &json!({"pipeline": sent}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );

        let after = emulator.ok("GetPipeline", &json!({"name": "demo"})).await;
        assert_eq!(
            after["pipeline"], before["pipeline"],
            "case {case:?}: a refused update must leave the stored declaration alone"
        );
        assert_eq!(
            after["pipeline"]["version"],
            json!(1),
            "case {case:?}: a refused update must not step the version, got {after}"
        );
        let (status, code) = emulator
            .expect_error("GetPipeline", &json!({"name": "demo", "version": 2}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineVersionNotFoundException"),
            "case {case:?}: a refused update must store no new version"
        );
    }
}
