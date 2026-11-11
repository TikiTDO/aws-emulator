// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a malformed `PipelineDeclaration` is classified under, end to end over
//! the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! `CreatePipeline` and `UpdatePipeline` each model three declaration refusals —
//! `InvalidActionDeclarationException`, `InvalidStageDeclarationException` and
//! `InvalidStructureException` — and a client branching on the code cannot tell them
//! apart unless the fault's SCOPE decides which it gets. That is the one rule this suite
//! asserts, over a set of declarations per scope and over both operations, because one
//! law may not be worded two ways:
//!
//! * a fault of ONE ACTION's own declaration — the artifacts it names, its name within
//!   its stage — is an invalid ACTION declaration;
//! * a fault of ONE STAGE's own declaration — a name another stage also carries, no
//!   action at all — is an invalid STAGE declaration;
//! * a fault of the pipeline AS A WHOLE — too few stages, a first stage that is not the
//!   source stage, nothing acting on the revision, a V2-only feature on a V1
//!   declaration — is a malformed structure.
//!
//! The harness is `tests/all/pipeline_lifecycle.rs`'s, so every assertion goes through
//! the router the production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    duplicate_action_name_message, duplicate_stage_name_message, too_few_stages_message,
    FIRST_STAGE_NOT_SOURCE_ONLY_MESSAGE, NO_NON_SOURCE_ACTION_MESSAGE,
};

use crate::pipeline_lifecycle::{Emulator, ROLE_ARN};

/// The two operations that take a whole `PipelineDeclaration`: every classification rule
/// is asserted on both, so neither can classify a fault its own way.
const DECLARATION_OPERATIONS: [&str; 2] = ["CreatePipeline", "UpdatePipeline"];

/// The name the fixture pipeline carries, which the `UpdatePipeline` rows re-declare.
const FIXTURE: &str = "demo";

/// The three declaration refusals the model declares for both operations
/// (`smithy_query query="operation" operation="CreatePipeline,UpdatePipeline"`).
const ACTION_FAULT: &str = "InvalidActionDeclarationException";
const STAGE_FAULT: &str = "InvalidStageDeclarationException";
const STRUCTURE_FAULT: &str = "InvalidStructureException";

/// A declaration over `stages`, with the artifact store every declaration must name.
fn pipeline(name: &str, stages: Value) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// One stage carrying `actions`, which may be empty: `StageDeclaration$actions` is
/// `@required`, and an empty list satisfies that constraint.
fn stage(name: &str, actions: Vec<Value>) -> Value {
    json!({"name": name, "actions": actions})
}

/// One `Custom`-owned action of `category`, so nothing a refused or stored declaration
/// carries is work the execution plane performs while a case asserts about it.
fn action(name: &str, category: &str, run_order: i64) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": category,
            "owner": "Custom",
            "provider": "MyProvider",
            "version": "1",
        },
        "runOrder": run_order,
    })
}

/// `action` with the artifact lists it declares.
fn with_artifacts(mut action: Value, inputs: &[&str], outputs: &[&str]) -> Value {
    if !inputs.is_empty() {
        action["inputArtifacts"] = json!(inputs
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>());
    }
    if !outputs.is_empty() {
        action["outputArtifacts"] = json!(outputs
            .iter()
            .map(|name| json!({"name": name}))
            .collect::<Vec<Value>>());
    }
    action
}

/// A source stage whose single action produces `output`.
fn source_stage(name: &str, output: &str) -> Value {
    stage(
        name,
        vec![with_artifacts(action("Src", "Source", 1), &[], &[output])],
    )
}

/// The `(status, code, message)` of a refusal. The awsJson `__type` may carry the
/// model's namespace, so only the shape name is compared.
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, response) = emulator.call(operation, body).await;
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

/// An emulator holding the fixture pipeline, so an `UpdatePipeline` row re-declares a
/// structure that exists and can be shown to have survived the refusal.
async fn emulator_with_fixture() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline(
                FIXTURE,
                json!([
                    source_stage("Source", "SourceOut"),
                    stage("Build", vec![with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &[])]),
                ]),
            )}),
        )
        .await;
    emulator
}

/// The structure the fixture currently stores, so a refused call can be shown to have
/// changed nothing.
async fn stored_stages(emulator: &Emulator) -> Value {
    emulator.ok("GetPipeline", &json!({"name": FIXTURE})).await["pipeline"]["stages"].clone()
}

/// Drive every row of a table of malformed declarations through both operations and
/// assert the refusal each one carries, plus that nothing stored changed.
async fn each_row_is_refused(rows: &[(&str, Value, &str, String)]) {
    for operation in DECLARATION_OPERATIONS {
        for (case, stages, expected_code, expected_message) in rows {
            let emulator = emulator_with_fixture().await;
            let before = stored_stages(&emulator).await;
            let name = if operation == "UpdatePipeline" {
                FIXTURE
            } else {
                "fresh"
            };
            let (status, code, message) = refusal(
                &emulator,
                operation,
                &json!({"pipeline": pipeline(name, stages.clone())}),
            )
            .await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, *expected_code, expected_message.as_str()),
                "case {case:?} on {operation}: expected the refusal of that fault's scope, \
                 got {status} {code} {message:?}"
            );
            assert_eq!(
                stored_stages(&emulator).await,
                before,
                "case {case:?} on {operation}: a refused declaration changes nothing"
            );
        }
    }
}

/// RULE: a fault of ONE action's own declaration — an artifact it consumes that no
/// earlier action produces, an artifact name a second action also produces, a name its
/// own stage uses twice — is refused as an invalid ACTION declaration.
#[tokio::test]
async fn a_fault_of_one_actions_declaration_is_refused_as_an_action_fault() {
    // The rows differ along what the rule generalises over: WHICH artifact member is at
    // fault, where in the declaration the faulty action sits, and whether the artifact
    // it names exists nowhere, exists only later, or exists twice.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "an input artifact nothing produces",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![with_artifacts(action("Compile", "Build", 1), &["NoSuchArtifact"], &[])],
                ),
            ]),
            ACTION_FAULT,
            "action Compile in stage Build declares the input artifact NoSuchArtifact, \
             which no earlier action outputs"
                .to_string(),
        ),
        (
            "an input artifact produced only in a LATER stage",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![with_artifacts(action("Compile", "Build", 1), &["ShipOut"], &[])],
                ),
                stage(
                    "Ship",
                    vec![with_artifacts(action("Deploy", "Deploy", 1), &["SourceOut"], &["ShipOut"])],
                ),
            ]),
            ACTION_FAULT,
            "action Compile in stage Build declares the input artifact ShipOut, which no \
             earlier action outputs"
                .to_string(),
        ),
        (
            "an input artifact produced at the SAME runOrder",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![
                        with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &["BuildOut"]),
                        with_artifacts(action("Test", "Test", 1), &["BuildOut"], &[]),
                    ],
                ),
            ]),
            ACTION_FAULT,
            "action Test in stage Build declares the input artifact BuildOut, which no \
             earlier action outputs"
                .to_string(),
        ),
        (
            "two stages producing one artifact name",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &["SourceOut"])],
                ),
            ]),
            ACTION_FAULT,
            "the output artifact SourceOut is declared by more than one action; an \
             artifact name is produced exactly once"
                .to_string(),
        ),
        (
            "two actions of ONE stage producing one artifact name",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![
                        with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &["BuildOut"]),
                        with_artifacts(action("Again", "Build", 1), &["SourceOut"], &["BuildOut"]),
                    ],
                ),
            ]),
            ACTION_FAULT,
            "the output artifact BuildOut is declared by more than one action; an \
             artifact name is produced exactly once"
                .to_string(),
        ),
        (
            "one stage using one action name twice",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![
                        action("Compile", "Build", 1),
                        action("Compile", "Test", 2),
                    ],
                ),
            ]),
            ACTION_FAULT,
            duplicate_action_name_message("Compile", "Build"),
        ),
    ];

    each_row_is_refused(&rows).await;
}

/// RULE: a fault of ONE stage's own declaration — a name another stage also carries, no
/// action at all — is refused as an invalid STAGE declaration, whatever the laws stated
/// over the pipeline's stages would also have made of the repeated stage.
#[tokio::test]
async fn a_fault_of_one_stage_is_refused_as_a_stage_fault() {
    // The rows differ along what the rule generalises over: WHICH stages share a name,
    // how far apart they are, what the repeat declares, and — for the empty-stage rows
    // — where the actionless stage sits.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "two adjacent stages with one name",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Build", vec![with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &[])]),
                stage("Build", vec![action("Test", "Test", 1)]),
            ]),
            STAGE_FAULT,
            duplicate_stage_name_message("Build"),
        ),
        (
            "the first stage's name used again two stages later",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Approval", vec![action("Approve", "Approval", 1)]),
                source_stage("Source", "SecondOut"),
            ]),
            STAGE_FAULT,
            duplicate_stage_name_message("Source"),
        ),
        (
            "three stages all named alike",
            json!([
                source_stage("Gate", "SourceOut"),
                stage("Gate", vec![action("Approve", "Approval", 1)]),
                stage("Gate", vec![action("Test", "Test", 1)]),
            ]),
            STAGE_FAULT,
            duplicate_stage_name_message("Gate"),
        ),
        (
            "a middle stage with no action",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Gate", Vec::new()),
                stage("Build", vec![with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &[])]),
            ]),
            STAGE_FAULT,
            "stage Gate must include at least one action in actions".to_string(),
        ),
        (
            "the first stage with no action",
            json!([
                stage("Source", Vec::new()),
                stage("Build", vec![action("Compile", "Build", 1)]),
            ]),
            STAGE_FAULT,
            "stage Source must include at least one action in actions".to_string(),
        ),
    ];

    each_row_is_refused(&rows).await;
}

/// RULE: a fault of the pipeline AS A WHOLE stays a malformed STRUCTURE — the scope of
/// the fault is what picks the refusal, so these are the control rows of the two tables
/// above.
#[tokio::test]
async fn a_fault_of_the_pipeline_as_a_whole_is_refused_as_a_structure_fault() {
    let mut rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "one stage only",
            json!([source_stage("Source", "SourceOut")]),
            STRUCTURE_FAULT,
            too_few_stages_message(1),
        ),
        (
            "a first stage that is not the source stage",
            json!([
                stage("Gate", vec![action("Approve", "Approval", 1)]),
                stage("Build", vec![action("Compile", "Build", 1)]),
            ]),
            STRUCTURE_FAULT,
            FIRST_STAGE_NOT_SOURCE_ONLY_MESSAGE.to_string(),
        ),
        (
            "nothing acting on the revision",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Mirror", vec![action("Again", "Source", 1)]),
            ]),
            STRUCTURE_FAULT,
            NO_NON_SOURCE_ACTION_MESSAGE.to_string(),
        ),
        (
            "a source action in a later stage",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Build", vec![action("Compile", "Build", 1)]),
                stage("More", vec![action("Again", "Source", 1)]),
            ]),
            STRUCTURE_FAULT,
            "a Source action can only appear in the first stage, but action Again is in \
             stage More"
                .to_string(),
        ),
    ];
    each_row_is_refused(&rows).await;
    rows.clear();

    // The V2-only features, one row each: the refusal names the feature the declaration
    // carries, and two different features are two different messages — so a client that
    // reads "upgrade this pipeline for its variables" out of one of them can. Each
    // sentence is spelled here rather than read from the crate: the wording is the
    // service's contract, not this implementation's.
    for (case, member, value, expected_message) in [
        (
            "variables on a V1 declaration",
            "variables",
            json!([{"name": "Flavor", "defaultValue": "vanilla"}]),
            "Pipeline variable can only be used with V2 pipelines",
        ),
        (
            "triggers on a V1 declaration",
            "triggers",
            json!([{
                "providerType": "CodeStarSourceConnection",
                "gitConfiguration": {"sourceActionName": "Src"},
            }]),
            "Pipeline trigger can only be used with V2 pipelines",
        ),
        (
            "the QUEUED execution mode on a V1 declaration",
            "executionMode",
            json!("QUEUED"),
            "QUEUED or PARALLEL mode can only be used with V2 pipelines",
        ),
        (
            "the PARALLEL execution mode on a V1 declaration",
            "executionMode",
            json!("PARALLEL"),
            "QUEUED or PARALLEL mode can only be used with V2 pipelines",
        ),
    ] {
        for operation in DECLARATION_OPERATIONS {
            let emulator = emulator_with_fixture().await;
            let name = if operation == "UpdatePipeline" {
                FIXTURE
            } else {
                "fresh"
            };
            let mut declaration = pipeline(
                name,
                json!([
                    source_stage("Source", "SourceOut"),
                    stage("Build", vec![action("Compile", "Build", 1)]),
                ]),
            );
            declaration["pipelineType"] = json!("V1");
            declaration[member] = value.clone();
            let (status, code, message) =
                refusal(&emulator, operation, &json!({"pipeline": declaration})).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, STRUCTURE_FAULT, expected_message),
                "case {case:?} on {operation}: expected the V2-only refusal naming the \
                 feature, got {status} {code} {message:?}"
            );
        }
    }

    // The controls for the same gate: the mode a V1 pipeline MAY state (the default one
    // its `executionMode` documents), and each V2-only feature on a declaration that
    // states V2 — so what the refusals above turn on is the pipeline type and not the
    // member being present.
    for (case, pipeline_type, member, value) in [
        ("the default execution mode on V1", "V1", "executionMode", json!("SUPERSEDED")),
        ("the QUEUED execution mode on V2", "V2", "executionMode", json!("QUEUED")),
        ("the PARALLEL execution mode on V2", "V2", "executionMode", json!("PARALLEL")),
        (
            "variables on V2",
            "V2",
            "variables",
            json!([{"name": "Flavor", "defaultValue": "vanilla"}]),
        ),
    ] {
        let emulator = Emulator::new();
        let mut declaration = pipeline(
            "accepted",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Build", vec![action("Compile", "Build", 1)]),
            ]),
        );
        declaration["pipelineType"] = json!(pipeline_type);
        declaration[member] = value;
        let (status, response) = emulator
            .call("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the declaration to be accepted, got {status} with \
             {response}"
        );
    }
}

/// The controls: a declaration whose every artifact reference resolves to an EARLIER
/// action and whose stage names are distinct is accepted and stored whole, so what the
/// refusals above turn on is the fault and not the shape of these declarations.
#[tokio::test]
async fn a_declaration_whose_scopes_are_all_sound_is_accepted_and_stored() {
    let rows: Vec<(&str, Value)> = vec![
        (
            "an artifact produced by an earlier stage",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &["BuildOut"])],
                ),
                stage(
                    "Ship",
                    vec![with_artifacts(action("Deploy", "Deploy", 1), &["BuildOut"], &[])],
                ),
            ]),
        ),
        (
            "an artifact produced at a LOWER runOrder of the same stage",
            json!([
                source_stage("Source", "SourceOut"),
                stage(
                    "Build",
                    vec![
                        with_artifacts(action("Compile", "Build", 1), &["SourceOut"], &["BuildOut"]),
                        with_artifacts(action("Test", "Test", 2), &["BuildOut"], &[]),
                    ],
                ),
            ]),
        ),
        (
            "one action name reused in another stage, and stage names all distinct",
            json!([
                source_stage("Source", "SourceOut"),
                stage("Build", vec![with_artifacts(action("Run", "Build", 1), &["SourceOut"], &[])]),
                stage("Ship", vec![action("Run", "Deploy", 1)]),
            ]),
        ),
    ];

    for (case, stages) in rows {
        let emulator = Emulator::new();
        emulator
            .ok("CreatePipeline", &json!({"pipeline": pipeline("accepted", stages.clone())}))
            .await;
        let stored = emulator.ok("GetPipeline", &json!({"name": "accepted"})).await;
        let declared_stages: Vec<String> = stages
            .as_array()
            .into_iter()
            .flatten()
            .map(|stage| stage["name"].as_str().unwrap_or_default().to_string())
            .collect();
        let reported: Vec<String> = stored["pipeline"]["stages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|stage| stage["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, declared_stages,
            "case {case:?}: the accepted declaration is stored whole, got {stored}"
        );
    }
}
