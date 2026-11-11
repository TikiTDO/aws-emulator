// SPDX-License-Identifier: CC0-1.0

//! The stage-CONDITION state `GetPipelineState` reports, and what
//! `OverrideStageCondition` does with it — end to end over the wire the service speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test:
//!
//! * a declared condition site reports the RUN of its conditions and their rules: the
//!   evaluation the condition plane recorded for the execution that reached the stage
//!   (`StageConditionState$latestExecution`, `ConditionState$latestExecution`,
//!   `RuleState$latestExecution`), and a site nothing has evaluated reports its declared
//!   conditions and rules with no run at all;
//! * an override acts on a GATE: the site whose met condition stopped this execution's
//!   run of the stage is overridable, and the stage then runs; every other request —
//!   another site, a site whose condition was not met, an unknown stage, an execution
//!   that is not the pipeline's latest, an unknown pipeline — is refused with the error
//!   the model declares for it, and changes nothing.
//!
//! The rows span what those rules generalise over: both condition sites a stage can
//! declare (`ConditionType` -> `BEFORE_ENTRY`, `ON_SUCCESS`), a condition that was met
//! and one that was not, and each refusal class the operation models.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service and every assertion goes through the
//! router.

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, status_of, wait_for_terminal,
    Emulator,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "condition-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The bucket the gated stage delivers into.
const DEPLOY_BUCKET: &str = "condition-deploy";

/// The key it delivers under.
const DEPLOY_KEY: &str = "gated.zip";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The bytes the source object carries.
const PAYLOAD: &[u8] = b"the gated bytes";

/// The `ConditionType` values the model declares
/// (`smithy_query query="shape" shape="ConditionType"` -> `BEFORE_ENTRY`,
/// `ON_SUCCESS`).
const BEFORE_ENTRY: &str = "BEFORE_ENTRY";
const ON_SUCCESS: &str = "ON_SUCCESS";

/// The refusal status every error this operation models carries (all 400 but
/// `ConflictException`).
const REFUSAL_STATUS: u16 = 400;

/// A `VariableCheck` rule comparing a declared pipeline variable against `value`.
///
/// `VariableCheck` is the one published rule provider whose verdict is a function of the
/// EXECUTION's own state, so it is the provider a test can make reach a verdict: its
/// configuration keys are the variable, the value and the operator (CodePipeline's rule
/// reference for that provider).
fn variable_check(name: &str, value: &str, operator: &str) -> Value {
    json!({
        "name": name,
        "ruleTypeId": {
            "category": "Rule",
            "owner": "AWS",
            "provider": "VariableCheck",
            "version": "1",
        },
        "configuration": {
            "Variable": "#{variables.Release}",
            "Value": value,
            "Operator": operator,
        },
    })
}

/// A condition site carrying one `VariableCheck` condition with `result`.
fn site(result: &str, rule: Value) -> Value {
    json!({"conditions": [{"result": result, "rules": [rule]}]})
}

/// Create a pipeline whose `Gated` stage declares `site_member` with `conditions`, and
/// answer the id of the execution a start of it runs.
async fn gated_pipeline(
    emulator: &Emulator,
    pipeline: &str,
    site_member: &str,
    conditions: Value,
) -> String {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
    let mut gated = deploy_stage("Gated", ARTIFACT, DEPLOY_BUCKET, DEPLOY_KEY, false);
    gated[site_member] = conditions;
    let mut body = declaration(
        pipeline,
        json!([source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT), gated]),
        None,
    );
    // Conditions are a V2 declaration's, and the rule compares a declared variable.
    body["pipeline"]["pipelineType"] = json!("V2");
    body["pipeline"]["variables"] = json!([{"name": "Release", "defaultValue": "4"}]);
    emulator.ok("CreatePipeline", &body).await;
    emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string()
}

/// The `StageState` `GetPipelineState` reports for one stage.
async fn stage_state(emulator: &Emulator, pipeline: &str, stage: &str) -> Value {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|reported| reported["stageName"] == json!(stage))
        .cloned()
        .unwrap_or_else(|| panic!("no stage state for {stage}: {state}"))
}

/// RULE: a declared condition site reports the run of each of its conditions and rules
/// once the execution reached the stage, and reports the declaration alone while nothing
/// has evaluated it.
///
/// The rows differ along the site and along whether the condition was MET: a rule whose
/// comparison does not hold reaches a FAILING verdict (the condition is met and its
/// `Result` applies), and one whose comparison holds reaches a passing one.
#[tokio::test]
async fn a_declared_condition_site_reports_the_run_of_its_rules() {
    // (case, the site member, the ConditionType it is, whether the rule's comparison
    //  holds, the RuleExecutionStatus the rule's run carries)
    let rows: Vec<(&str, &str, &str, bool, &str)> = vec![
        (
            "an entry condition whose rule failed",
            "beforeEntry",
            BEFORE_ENTRY,
            false,
            "Failed",
        ),
        (
            "an entry condition whose rule passed",
            "beforeEntry",
            BEFORE_ENTRY,
            true,
            "Succeeded",
        ),
        (
            "a success condition whose rule failed",
            "onSuccess",
            ON_SUCCESS,
            false,
            "Failed",
        ),
        (
            "a success condition whose rule passed",
            "onSuccess",
            ON_SUCCESS,
            true,
            "Succeeded",
        ),
    ];

    for (case, member, condition_type, holds, rule_status) in rows {
        let emulator = Emulator::new();
        let pipeline = format!("cond-state-{}", case.replace(' ', "-"));
        // The variable's declared value is "4": `EQ 4` holds, `EQ 99` does not.
        let value = if holds { "4" } else { "99" };
        let execution = gated_pipeline(
            &emulator,
            &pipeline,
            member,
            site("FAIL", variable_check("Check", value, "EQ")),
        )
        .await;
        wait_for_terminal(&emulator, &pipeline, &execution).await;

        let reported = stage_state(&emulator, &pipeline, "Gated").await;
        let reported_site = match condition_type {
            BEFORE_ENTRY => &reported["beforeEntryConditionState"],
            _ => &reported["onSuccessConditionState"],
        };
        assert_eq!(
            reported_site["conditionStates"][0]["ruleStates"][0]["ruleName"],
            json!("Check"),
            "case {case:?}: the declared rule is reported by name, got {reported}"
        );
        let run = &reported_site["conditionStates"][0]["ruleStates"][0]["latestExecution"];
        assert_eq!(
            run["status"],
            json!(rule_status),
            "case {case:?}: expected the rule's run to report {rule_status}, got {reported}"
        );
        assert!(
            run["ruleExecutionId"].is_string() && run["lastStatusChange"].is_number(),
            "case {case:?}: a rule's run carries its own id and instant, got {run}"
        );
        // The condition and the SET roll that verdict up: the condition a rule failed at
        // is `Failed`, and one whose rules all passed is `Succeeded`.
        assert_eq!(
            reported_site["conditionStates"][0]["latestExecution"]["status"],
            json!(rule_status),
            "case {case:?}: expected the condition to roll its rule up, got {reported}"
        );
        assert_eq!(
            reported_site["latestExecution"]["status"],
            json!(rule_status),
            "case {case:?}: expected the condition set to roll its conditions up, got \
             {reported}"
        );

        // The other site, which this stage does not declare, is not reported at all —
        // and the stage the execution came through declares neither.
        let absent = match condition_type {
            BEFORE_ENTRY => "onSuccessConditionState",
            _ => "beforeEntryConditionState",
        };
        assert!(
            reported.get(absent).is_none(),
            "case {case:?}: a site the stage does not declare is not reported, got {reported}"
        );
        let source = stage_state(&emulator, &pipeline, "Source").await;
        assert!(
            source.get("beforeEntryConditionState").is_none()
                && source.get("onSuccessConditionState").is_none(),
            "case {case:?}: a stage declaring no condition reports no condition state, got \
             {source}"
        );
    }
}

/// RULE: an override releases the gate a met condition put on the stage — the site is
/// not evaluated again for that run, the stage runs, and the condition's run is reported
/// `Overridden`.
#[tokio::test]
async fn overriding_the_gate_lets_the_stage_run() {
    let emulator = Emulator::new();
    let pipeline = "cond-override";
    // A comparison that does not hold, so the condition is MET and its FAIL result
    // stops the stage.
    let execution = gated_pipeline(
        &emulator,
        pipeline,
        "beforeEntry",
        site("FAIL", variable_check("Check", "99", "EQ")),
    )
    .await;
    let blocked = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        blocked["pipelineExecution"]["status"],
        json!("Failed"),
        "the met entry condition stops the stage, got {blocked}"
    );
    let (status, _) = emulator.get_object(DEPLOY_BUCKET, DEPLOY_KEY).await;
    assert_eq!(
        status, 404,
        "a stage the gate stopped delivers nothing, got {status}"
    );

    let answer = emulator
        .ok(
            "OverrideStageCondition",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gated",
                "pipelineExecutionId": execution,
                "conditionType": BEFORE_ENTRY,
            }),
        )
        .await;
    assert_eq!(
        answer,
        Value::Object(serde_json::Map::new()),
        "the override answers the empty output the model declares, got {answer}"
    );

    let released = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        released["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the overridden execution runs the stage it was held out of, got {released}"
    );
    let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, DEPLOY_KEY).await;
    assert_eq!(
        status, 200,
        "the released stage performs its action's work, got {status}"
    );
    assert_eq!(
        bytes, PAYLOAD,
        "the released stage delivers the artifact's own bytes"
    );
    let reported = stage_state(&emulator, pipeline, "Gated").await;
    assert_eq!(
        reported["beforeEntryConditionState"]["conditionStates"][0]["latestExecution"]["status"],
        json!("Overridden"),
        "the overridden condition reports the status of a gate a caller decided, got {reported}"
    );
    // The rule's OWN run keeps the verdict it reached: the override decided the gate,
    // not the comparison.
    assert_eq!(
        reported["beforeEntryConditionState"]["conditionStates"][0]["ruleStates"][0]
            ["latestExecution"]["status"],
        json!("Failed"),
        "the rule's run keeps its verdict, got {reported}"
    );

    // A second override of the same site has no gate left to act on.
    let (status, code) = emulator
        .error_code(
            "OverrideStageCondition",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gated",
                "pipelineExecutionId": execution,
                "conditionType": BEFORE_ENTRY,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (REFUSAL_STATUS, "ConditionNotOverridableException"),
        "a gate already overridden is not overridable again"
    );
}

/// RULE, the other site: an `ON_SUCCESS` gate is overridden the same way — the stage has
/// already run its actions, so the override lets the execution past the gate without
/// re-running what already ran.
#[tokio::test]
async fn overriding_a_success_gate_lets_the_execution_past_it() {
    let emulator = Emulator::new();
    let pipeline = "cond-override-success";
    let execution = gated_pipeline(
        &emulator,
        pipeline,
        "onSuccess",
        site("FAIL", variable_check("Check", "99", "EQ")),
    )
    .await;
    let blocked = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        blocked["pipelineExecution"]["status"],
        json!("Failed"),
        "the met success condition stops the stage after its actions ran, got {blocked}"
    );
    // The stage's action DID run before the gate tripped: its delivery is there.
    let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, DEPLOY_KEY).await;
    assert_eq!(
        status, 200,
        "a success condition is evaluated after the stage's actions ran, got {status}"
    );
    assert_eq!(bytes, PAYLOAD, "the stage delivered the artifact's own bytes");
    let ran = stage_state(&emulator, pipeline, "Gated").await;
    let action_execution_id = ran["actionStates"][0]["latestExecution"]["actionExecutionId"].clone();
    assert!(
        action_execution_id.is_string(),
        "the stage's action reports the run it performed, got {ran}"
    );

    emulator
        .ok(
            "OverrideStageCondition",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gated",
                "pipelineExecutionId": execution,
                "conditionType": ON_SUCCESS,
            }),
        )
        .await;
    let released = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        released["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the overridden execution carries on past the gate, got {released}"
    );
    let reported = stage_state(&emulator, pipeline, "Gated").await;
    assert_eq!(
        reported["onSuccessConditionState"]["conditionStates"][0]["latestExecution"]["status"],
        json!("Overridden"),
        "the overridden condition reports the status of a gate a caller decided, got {reported}"
    );
    // Nothing re-ran: the action still reports the ONE run it performed.
    assert_eq!(
        reported["actionStates"][0]["latestExecution"]["actionExecutionId"], action_execution_id,
        "an override decides the condition, it does not re-run what already ran, got {reported}"
    );
    assert_eq!(
        reported["actionStates"][0]["latestExecution"]["status"],
        json!("Succeeded"),
        "the action keeps the outcome it reached, got {reported}"
    );
}

/// RULE, the refusal half: a request that names no gate is refused with the error the
/// model declares for what it got wrong, and changes nothing about the execution.
///
/// The rows span the refusal classes: the site the stage declares no condition for, the
/// site that is not the one holding the stage, a condition that was not met at all, an
/// unknown stage, an execution that is not the pipeline's latest, and an unknown
/// pipeline.
#[tokio::test]
async fn a_request_that_names_no_gate_is_refused() {
    let emulator = Emulator::new();
    let pipeline = "cond-refusals";
    // The gate this pipeline really has: an entry condition that was met.
    let execution = gated_pipeline(
        &emulator,
        pipeline,
        "beforeEntry",
        site("FAIL", variable_check("Check", "99", "EQ")),
    )
    .await;
    wait_for_terminal(&emulator, pipeline, &execution).await;

    // A second pipeline whose gated stage declares no condition at all, so a request
    // about its stage names a site the declaration does not carry.
    let ungated = "cond-ungated";
    emulator.bucket(&artifact_bucket(ungated)).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                ungated,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Gated", ARTIFACT, DEPLOY_BUCKET, "ungated.zip", false),
                ]),
                None,
            ),
        )
        .await;
    let ungated_execution = emulator
        .ok("StartPipelineExecution", &json!({"name": ungated}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();
    wait_for_terminal(&emulator, ungated, &ungated_execution).await;
    // One more execution of the same pipeline, so the one above is no longer its latest.
    let newer = emulator
        .ok("StartPipelineExecution", &json!({"name": ungated}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();
    wait_for_terminal(&emulator, ungated, &newer).await;

    // (case, pipelineName, stageName, pipelineExecutionId, conditionType, the error)
    let rows: Vec<(&str, &str, &str, &str, &str, &str)> = vec![
        (
            "the site that is not the one holding the stage",
            pipeline,
            "Gated",
            &execution,
            ON_SUCCESS,
            "ConditionNotOverridableException",
        ),
        (
            "a stage no condition stopped",
            pipeline,
            "Source",
            &execution,
            BEFORE_ENTRY,
            "ConditionNotOverridableException",
        ),
        (
            "a stage whose declaration carries no condition",
            ungated,
            "Gated",
            &newer,
            BEFORE_ENTRY,
            "ConditionNotOverridableException",
        ),
        (
            "a stage the pipeline does not declare",
            pipeline,
            "NoSuchStage",
            &execution,
            BEFORE_ENTRY,
            "StageNotFoundException",
        ),
        (
            "an execution that is not the pipeline's latest",
            ungated,
            "Gated",
            &ungated_execution,
            BEFORE_ENTRY,
            "NotLatestPipelineExecutionException",
        ),
        (
            "a pipeline the account does not hold",
            "no-such-pipeline",
            "Gated",
            &execution,
            BEFORE_ENTRY,
            "PipelineNotFoundException",
        ),
    ];

    for (case, pipeline_name, stage_name, execution_id, condition_type, expected) in rows {
        let before = status_of(&emulator, pipeline, &execution).await;
        let (status, code) = emulator
            .error_code(
                "OverrideStageCondition",
                &json!({
                    "pipelineName": pipeline_name,
                    "stageName": stage_name,
                    "pipelineExecutionId": execution_id,
                    "conditionType": condition_type,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, expected),
            "case {case:?}: unexpected refusal"
        );
        assert_eq!(
            status_of(&emulator, pipeline, &execution).await,
            before,
            "case {case:?}: a refused override must change nothing"
        );
    }
    // And the gate the refusals did not touch is still there to be overridden.
    emulator
        .ok(
            "OverrideStageCondition",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gated",
                "pipelineExecutionId": execution,
                "conditionType": BEFORE_ENTRY,
            }),
        )
        .await;
}

/// RULE: `ConditionNotOverridableException` carries ONE fixed sentence with the request's
/// own values in it — the `conditionType` and the `stageName` it named, and the PIPELINE
/// it named them on in the single-quoted slot — so nothing of it is about one particular
/// refusal and no per-run execution id reaches it.
#[tokio::test]
async fn the_not_overridable_refusal_states_one_sentence_over_the_request() {
    let emulator = Emulator::new();
    // Two gated pipelines, differing in the site their condition is declared at, so a row
    // can ask for a site the stage does not hold as well as one whose condition was never
    // met.
    let entry_gated = "cond-wording-entry";
    let entry_execution = gated_pipeline(
        &emulator,
        entry_gated,
        "beforeEntry",
        site("FAIL", variable_check("Check", "99", "EQ")),
    )
    .await;
    wait_for_terminal(&emulator, entry_gated, &entry_execution).await;

    // The rows differ along every value the sentence interpolates: the pipeline, the
    // stage, and the condition type — and along which not-overridable class the request
    // falls in (a site the stage does not hold, a stage no condition stopped).
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "the site the stage does not hold",
            entry_gated,
            "Gated",
            ON_SUCCESS,
        ),
        (
            "a stage no condition stopped, at the entry site",
            entry_gated,
            "Source",
            BEFORE_ENTRY,
        ),
        (
            "a stage no condition stopped, at the success site",
            entry_gated,
            "Source",
            ON_SUCCESS,
        ),
    ];

    for (case, pipeline_name, stage_name, condition_type) in rows {
        let (status, response) = emulator
            .cp(
                "OverrideStageCondition",
                &json!({
                    "pipelineName": pipeline_name,
                    "stageName": stage_name,
                    "pipelineExecutionId": entry_execution,
                    "conditionType": condition_type,
                }),
            )
            .await;
        let code = response["__type"].as_str().unwrap_or_default();
        let message = response["message"]
            .as_str()
            .or_else(|| response["Message"].as_str())
            .unwrap_or_default();
        assert_eq!(
            (status, code),
            (REFUSAL_STATUS, "ConditionNotOverridableException"),
            "case {case:?}: unexpected refusal, got {status} {code} {message:?}"
        );
        // The sentence, spelled here rather than read from the crate: the fixed text is
        // the service's, and the three substitutions are the request's own values. The
        // single-quoted slot carries the PIPELINE the request named — the value the
        // recorded refusal of this class carries there, on two independently recorded
        // calls whose own execution id appears nowhere in the sentence.
        let expected = format!(
            "The condition type {condition_type} in stage {stage_name} cannot be \
             overridden now. Either it contains no failed or in-progress rule, the \
             execution '{pipeline_name}' is already superseded, or the pipeline \
             definition has changed since the rules(s) failed."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal's one sentence, got {message:?}"
        );
        assert!(
            !message.contains(entry_execution.as_str()),
            "case {case:?}: the per-run execution id is not part of the sentence, got \
             {message:?}"
        );
    }
}
