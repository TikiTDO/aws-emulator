// SPDX-License-Identifier: CC0-1.0

//! What a `PipelineDeclaration`'s ACTION TYPES and STAGE NAMES are held to, end to end
//! over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test, each asserted over a SET of inputs and over BOTH operations that
//! take a whole declaration, because one law may not be worded two ways:
//!
//! * an `actionTypeId` whose `owner` is `AWS` names an action type the service publishes —
//!   a closed set ([`BUILT_IN_ACTION_TYPES`], plus the published identities the catalog
//!   carries no detail for) — so a `category`/`provider`/`version` combination it does not
//!   carry is refused as a malformed ACTION declaration, while an id of an owner whose
//!   catalog is open (`Custom`, `ThirdParty`) is stored as sent;
//! * an action declares no more `inputArtifacts` or `outputArtifacts` than its action
//!   type's `ArtifactDetails$maximumCount` allows — which is what makes an artifact on a
//!   manual approval action, whose action type takes and produces none, impossible;
//! * a stage is addressed by its name, so a declaration naming two stages alike is refused
//!   as a malformed STAGE declaration naming the repeated stage, while the same action name
//!   in two differently-named stages is accepted.
//!
//! Every refusal is also shown to change nothing: the stored declaration keeps the version
//! and the structure it had.
//!
//! The harness comes from `tests/all/pipeline_lifecycle.rs`, so every assertion goes
//! through the same router the production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    action_type_identity, action_type_is_published, built_in_action_type,
    duplicate_stage_name_message, region_identity, AWS_ACTION_TYPES_WITHOUT_DETAIL,
    BUILT_IN_ACTION_TYPES,
};

use crate::pipeline_lifecycle::{Emulator, ROLE_ARN};

/// The two operations that take a whole `PipelineDeclaration`, so every declaration law
/// is asserted on both.
const DECLARATION_OPERATIONS: [&str; 2] = ["CreatePipeline", "UpdatePipeline"];

/// The name every row's fixture pipeline carries: `UpdatePipeline` replaces the structure
/// of a pipeline that already exists, so each row creates this one first.
const FIXTURE: &str = "demo";

/// The artifact the fixture's source stage produces, so an acting action of a row has a
/// produced name to consume.
const SOURCE_ARTIFACT: &str = "SourceOut";

/// An `ActionTypeId` as the wire carries it.
fn action_type(category: &str, owner: &str, provider: &str, version: &str) -> Value {
    json!({
        "category": category,
        "owner": owner,
        "provider": provider,
        "version": version,
    })
}

/// The fixture's source stage: a `Custom`-owned source action, whose catalog is open, so
/// no row's rule is about this stage. It produces [`SOURCE_ARTIFACT`].
fn source_stage() -> Value {
    json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": action_type("Source", "Custom", "MySource", "1"),
            "outputArtifacts": [{"name": SOURCE_ARTIFACT}],
        }],
    })
}

/// One acting stage carrying a single action of `id`, with the artifact lists given.
fn acting_stage(stage: &str, action: &str, id: Value, inputs: &[&str], outputs: &[&str]) -> Value {
    let mut declaration = json!({
        "name": action,
        "actionTypeId": id,
    });
    if !inputs.is_empty() {
        declaration["inputArtifacts"] =
            Value::Array(inputs.iter().map(|name| json!({"name": name})).collect());
    }
    if !outputs.is_empty() {
        declaration["outputArtifacts"] =
            Value::Array(outputs.iter().map(|name| json!({"name": name})).collect());
    }
    json!({"name": stage, "actions": [declaration]})
}

/// A declaration named `name` over `stages`.
fn pipeline(name: &str, stages: Value) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// The name a `CreatePipeline` row declares: the operation mints a pipeline, so a row of
/// it may not reuse the stored fixture's name.
const FRESH: &str = "fresh";

/// The `(status, code, message)` of a refusal, through the one reader the shared harness
/// owns (`Emulator::refusal`).
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    emulator.refusal(operation, body).await
}

/// An emulator holding the fixture pipeline at version 1, so a row drives
/// `UpdatePipeline` against a stored structure and can read that structure back.
async fn emulator_with_fixture() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline(FIXTURE, json!([
                source_stage(),
                acting_stage(
                    "Build",
                    "Compile",
                    action_type("Build", "Custom", "MyBuilder", "1"),
                    &[SOURCE_ARTIFACT],
                    &[],
                ),
            ]))}),
        )
        .await;
    emulator
}

/// What the store holds for `name`: its version and its stage names, so a refused call
/// can be shown to have changed neither — or `None` when the store holds no such
/// pipeline, which is what a refused `CreatePipeline` must leave behind.
async fn stored(emulator: &Emulator, name: &str) -> Option<(Value, Value)> {
    let (status, read) = emulator.call("GetPipeline", &json!({"name": name})).await;
    (status == 200).then(|| {
        (
            read["pipeline"]["version"].clone(),
            read["pipeline"]["stages"].clone(),
        )
    })
}

/// Drive one declaration through both declaration operations and assert the outcome.
///
/// `expected` is `None` for a declaration that must be STORED and `Some((code, clause))`
/// for one that must be refused with that error code and a message carrying that clause.
/// A refused call is also shown to leave the store as it was: the fixture keeps its
/// structure and version, and a refused create mints nothing.
async fn assert_declaration(case: &str, stages: Value, expected: Option<(&str, String)>) {
    for operation in DECLARATION_OPERATIONS {
        let emulator = emulator_with_fixture().await;
        // `CreatePipeline` mints a pipeline and `UpdatePipeline` replaces a stored one,
        // so each row drives the operation against the name that operation can take.
        let name = if operation == DECLARATION_OPERATIONS[0] {
            FRESH
        } else {
            FIXTURE
        };
        let before = stored(&emulator, name).await;
        let body = json!({"pipeline": pipeline(name, stages.clone())});
        match &expected {
            None => {
                let (status, answer) = emulator.call(operation, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: a declaration this rule admits is \
                     stored, got {status} with {answer}"
                );
                let after = stored(&emulator, name).await.unwrap_or_else(|| {
                    panic!("case {case:?} on {operation}: the pipeline must be readable")
                });
                assert_eq!(
                    after.1, answer["pipeline"]["stages"],
                    "case {case:?} on {operation}: the stored structure is the one the \
                     answer reported, got {}",
                    after.1
                );
            }
            Some((code, clause)) => {
                let (status, actual, message) = refusal(&emulator, operation, &body).await;
                assert_eq!(
                    (status, actual.as_str()),
                    (400, *code),
                    "case {case:?} on {operation}: expected a 400 {code}, got {status} \
                     {actual} ({message:?})"
                );
                assert!(
                    message.contains(clause.as_str()),
                    "case {case:?} on {operation}: the refusal names what was wrong: \
                     expected a message carrying {clause:?}, got {message:?}"
                );
                let after = stored(&emulator, name).await;
                assert_eq!(
                    after, before,
                    "case {case:?} on {operation}: a refused declaration changes nothing"
                );
            }
        }
    }
}

/// The error code a malformed ACTION declaration is refused with.
const BAD_ACTION: &str = "InvalidActionDeclarationException";

/// The error code a malformed STAGE declaration is refused with.
const BAD_STAGE: &str = "InvalidStageDeclarationException";

/// RULE: an `AWS`-owned `actionTypeId` names an action type the service publishes, and
/// that set is closed, so an id outside it is refused as a malformed action declaration —
/// while an id whose owner's catalog is open is stored as sent.
#[tokio::test]
async fn an_aws_owned_action_type_the_service_does_not_publish_is_refused() {
    // The rows differ along what the rule generalises over: the category, the provider,
    // the version and the OWNER of the id — including two entries the issue did not cite
    // (`Deploy/AWS/CodeDeploy`, which the catalog carries, and `Deploy/AWS/CloudFormation`,
    // a published identity it carries no detail for).
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a provider no AWS action type has",
            action_type("Deploy", "AWS", "NoSuchProvider", "1"),
            false,
        ),
        (
            "another category, another absent provider",
            action_type("Build", "AWS", "NotAProvider", "1"),
            false,
        ),
        (
            "a real provider in a category it is not published in",
            action_type("Deploy", "AWS", "CodeBuild", "1"),
            false,
        ),
        (
            "a published provider at a version it does not publish",
            action_type("Deploy", "AWS", "CodeDeploy", "2"),
            false,
        ),
        (
            "a provider that is one character long",
            action_type("Deploy", "AWS", "X", "1"),
            false,
        ),
        (
            "a published AWS action type",
            action_type("Deploy", "AWS", "CodeDeploy", "1"),
            true,
        ),
        (
            "another published AWS action type",
            action_type("Invoke", "AWS", "StepFunctions", "1"),
            true,
        ),
        (
            "a published AWS action type the catalog carries no detail for",
            action_type("Deploy", "AWS", "CloudFormation", "1"),
            true,
        ),
        (
            "a Custom-owned id the account has not registered",
            action_type("Deploy", "Custom", "NoSuchProvider", "1"),
            true,
        ),
        (
            "a ThirdParty-owned id no partner has registered",
            action_type("Deploy", "ThirdParty", "NoSuchPartner", "1"),
            true,
        ),
    ];

    for (case, id, accepted) in rows {
        let stages = json!([
            source_stage(),
            acting_stage("Act", "Work", id.clone(), &[SOURCE_ARTIFACT], &[]),
        ]);
        let expected = (!accepted).then(|| {
            let identity = action_type_identity(&ActionTypeId {
                category: id["category"].as_str().unwrap_or_default().to_string(),
                owner: id["owner"].as_str().unwrap_or_default().to_string(),
                provider: id["provider"].as_str().unwrap_or_default().to_string(),
                version: id["version"].as_str().unwrap_or_default().to_string(),
            });
            // The refusal states the identity the request named, the action that named
            // it and the Region it is not available in: every row is signed for the
            // harness's default Region.
            (
                BAD_ACTION,
                format!(
                    "{identity} in action 'Work' is not available in region '{}'",
                    region_identity("us-east-1")
                ),
            )
        });
        assert_declaration(case, stages, expected).await;
    }

    // The sentence itself, spelled out once: its fixed text exactly, with the identity the
    // request named, the action that named it and the Region the request was signed for —
    // so a drift in the builder's own wording is caught rather than re-asserted.
    let emulator = emulator_with_fixture().await;
    let (_, _, message) = refusal(
        &emulator,
        DECLARATION_OPERATIONS[1],
        &json!({"pipeline": pipeline(
            FIXTURE,
            json!([
                source_stage(),
                acting_stage(
                    "Act",
                    "Work",
                    action_type("Deploy", "AWS", "NoSuchProvider", "1"),
                    &[SOURCE_ARTIFACT],
                    &[],
                ),
            ]),
        )}),
    )
    .await;
    assert_eq!(
        message,
        "ActionType (Category: 'Deploy', Provider: 'NoSuchProvider', Owner: 'AWS', \
         Version: '1') in action 'Work' is not available in region 'US_EAST_1'",
        "expected the sentence this refusal states, got {message:?}"
    );
}

/// RULE: an action declares no more artifacts on either list than its action type's
/// published maximum, so an artifact on an action type that takes or produces none is
/// refused — and an action type with no published bounds admits any list the service's own
/// per-action cap allows.
#[tokio::test]
async fn an_action_declares_no_more_artifacts_than_its_action_type_takes() {
    // The bounds the rows are written against come from the catalog itself, so a case
    // states what the action type publishes rather than a figure copied by hand.
    let bound = |category: &str, provider: &str, outputs: bool| -> i32 {
        let entry = BUILT_IN_ACTION_TYPES
            .iter()
            .find(|entry| {
                entry.category == category && entry.owner == "AWS" && entry.provider == provider
            })
            .unwrap_or_else(|| panic!("the catalog publishes {category}/AWS/{provider}"));
        if outputs {
            entry.output_artifacts.1
        } else {
            entry.input_artifacts.1
        }
    };
    // (case, category, owner, provider, input count, output count, accepted)
    let rows: Vec<(&str, &str, &str, &str, usize, usize, bool)> = vec![
        (
            "an output artifact on a manual approval",
            "Approval",
            "AWS",
            "Manual",
            0,
            1,
            false,
        ),
        (
            "an input artifact on a manual approval",
            "Approval",
            "AWS",
            "Manual",
            1,
            0,
            false,
        ),
        (
            "no artifact at all on a manual approval",
            "Approval",
            "AWS",
            "Manual",
            0,
            0,
            true,
        ),
        (
            "the one input artifact an S3 deploy takes",
            "Deploy",
            "AWS",
            "S3",
            1,
            0,
            true,
        ),
        (
            "one input artifact past what an S3 deploy takes",
            "Deploy",
            "AWS",
            "S3",
            2,
            0,
            false,
        ),
        (
            "an output artifact on an S3 deploy, which produces none",
            "Deploy",
            "AWS",
            "S3",
            1,
            1,
            false,
        ),
        (
            "the five output artifacts a CodeBuild action produces",
            "Build",
            "AWS",
            "CodeBuild",
            1,
            5,
            true,
        ),
        (
            "one output artifact past what a CodeBuild action produces",
            "Build",
            "AWS",
            "CodeBuild",
            1,
            6,
            false,
        ),
        (
            "artifacts on an action type with no published bounds",
            "Deploy",
            "Custom",
            "MyDeployer",
            1,
            3,
            true,
        ),
    ];

    for (case, category, owner, provider, inputs, outputs, accepted) in rows {
        // The first input is the artifact the source stage produces; any further one is a
        // name nothing produces, which the arity rule is refused ahead of.
        let input_names: Vec<String> = (0..inputs)
            .map(|index| {
                if index == 0 {
                    SOURCE_ARTIFACT.to_string()
                } else {
                    format!("Extra{index}")
                }
            })
            .collect();
        let output_names: Vec<String> =
            (0..outputs).map(|index| format!("Out{index}")).collect();
        let inputs_ref: Vec<&str> = input_names.iter().map(String::as_str).collect();
        let outputs_ref: Vec<&str> = output_names.iter().map(String::as_str).collect();
        let stages = json!([
            source_stage(),
            acting_stage(
                "Act",
                "Work",
                action_type(category, owner, provider, "1"),
                &inputs_ref,
                &outputs_ref,
            ),
        ]);
        let expected = (!accepted).then(|| {
            // The refusal names WHICH artifacts were over their bound and the published
            // maximum they exceeded.
            let over_outputs = outputs > bound(category, provider, true) as usize;
            let artifacts = if over_outputs { "output" } else { "input" };
            (
                BAD_ACTION,
                format!("allows at most {} {artifacts} artifact(s)", bound(category, provider, over_outputs)),
            )
        });
        assert_declaration(case, stages, expected).await;
    }

    // The sentence itself, spelled out once, so the whole message is pinned rather than
    // only the clause naming the artifacts: the action type's identity, the action that
    // declared them, which artifacts they are, the published bound and the count declared.
    let emulator = emulator_with_fixture().await;
    let (_, _, message) = refusal(
        &emulator,
        DECLARATION_OPERATIONS[1],
        &json!({"pipeline": pipeline(
            FIXTURE,
            json!([
                source_stage(),
                acting_stage(
                    "Act",
                    "Approve",
                    action_type("Approval", "AWS", "Manual", "1"),
                    &[],
                    &["ApprovalOutput"],
                ),
            ]),
        )}),
    )
    .await;
    assert_eq!(
        message,
        "ActionType (Category: 'Approval', Provider: 'Manual', Owner: 'AWS', \
         Version: '1') in action 'Approve' allows at most 0 output artifact(s), but the \
         action declares 1",
        "expected the sentence this refusal states, got {message:?}"
    );
}

/// RULE: the identities the service publishes come from ONE answer to "does the service
/// publish this id?": the catalog `ListActionTypes` reports and the identities it carries
/// no reportable detail for, which name no id in common — so no id is published twice and
/// none is published by neither table while the documented provider list names it.
#[test]
fn the_two_published_action_type_tables_are_disjoint_and_both_resolve() {
    for (category, provider) in AWS_ACTION_TYPES_WITHOUT_DETAIL {
        let id = ActionTypeId {
            category: category.to_string(),
            owner: "AWS".to_string(),
            provider: provider.to_string(),
            version: "1".to_string(),
        };
        assert!(
            built_in_action_type(&id).is_none(),
            "case {category}/{provider}: an identity the catalog carries detail for must \
             not be listed as carrying none"
        );
        assert!(
            action_type_is_published(&id),
            "case {category}/{provider}: a listed identity must resolve as published"
        );
    }
    for entry in BUILT_IN_ACTION_TYPES {
        let id = entry.key().to_id();
        assert!(
            action_type_is_published(&id),
            "case {}/{}: a catalog entry must resolve as published",
            entry.category,
            entry.provider
        );
    }
}

/// RULE: a stage is addressed by its name, so a declaration naming two stages alike is
/// refused as a malformed stage declaration naming the repeated stage — ahead of anything
/// else those indistinguishable stages get wrong.
#[tokio::test]
async fn a_declaration_naming_two_stages_alike_is_refused() {
    let act = |stage: &str, action: &str| {
        acting_stage(
            stage,
            action,
            action_type("Build", "Custom", "MyBuilder", "1"),
            &[SOURCE_ARTIFACT],
            &[],
        )
    };
    let approval = |stage: &str| {
        acting_stage(
            stage,
            "Approve",
            action_type("Approval", "AWS", "Manual", "1"),
            &[],
            &[],
        )
    };
    // The rows differ along which stage is repeated, how far apart the two copies are,
    // and what the repeated stage holds — including the first stage's own name, where the
    // second copy also breaks the source-stage law the duplicate is refused ahead of.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        ("every stage named once", json!([source_stage(), act("Build", "Compile")]), None),
        (
            "three stages, every name distinct",
            json!([source_stage(), act("Build", "Compile"), approval("Approve")]),
            None,
        ),
        (
            "one action name reused in two differently-named stages",
            json!([source_stage(), act("Build", "Work"), act("Test", "Work")]),
            None,
        ),
        (
            "two adjacent stages named alike",
            json!([source_stage(), act("Build", "Compile"), act("Build", "Again")]),
            Some("Build"),
        ),
        (
            "the source stage's name used again at the end",
            json!([source_stage(), approval("Approve"), source_stage()]),
            Some("Source"),
        ),
        (
            "a name used three times",
            json!([
                source_stage(),
                act("Build", "One"),
                act("Build", "Two"),
                act("Build", "Three"),
            ]),
            Some("Build"),
        ),
    ];

    for (case, stages, repeated) in rows {
        let expected =
            repeated.map(|stage| (BAD_STAGE, duplicate_stage_name_message(stage)));
        assert_declaration(case, stages, expected).await;
    }

    // The sentence itself, spelled out once: its fixed text exactly, over the stage name
    // the declaration repeated.
    assert_eq!(
        duplicate_stage_name_message("Source"),
        "Stage name 'Source' is used more than once",
        "expected the sentence this refusal states"
    );
    let emulator = emulator_with_fixture().await;
    let (_, _, message) = refusal(
        &emulator,
        DECLARATION_OPERATIONS[1],
        &json!({"pipeline": pipeline(
            FIXTURE,
            json!([source_stage(), approval("Approve"), source_stage()]),
        )}),
    )
    .await;
    assert_eq!(
        message, "Stage name 'Source' is used more than once",
        "expected the sentence this refusal states, got {message:?}"
    );
}
