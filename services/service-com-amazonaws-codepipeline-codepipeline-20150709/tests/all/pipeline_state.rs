// SPDX-License-Identifier: CC0-1.0

//! `GetPipelineState` over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.GetPipelineState`).
//!
//! The rule under test: the reported state is a projection of the STORED pipeline —
//! one `StageState` per stored stage in declaration order, one `ActionState` per
//! stored action in declaration order, one condition state per declared condition set
//! naming that set's rules, an inbound transition for every stage an execution
//! transitions INTO (that is, every stage but the first), and no execution anywhere,
//! because within the span this state is read in — the span entering a stage is owed —
//! the run the pipeline's creation started has reached no stage, no action and no rule.
//! (What a run that HAS reached them reports is the execution plane's own suites.)
//!
//! The table below spans that class: the rows differ in stage count, in how many
//! actions each stage declares, in the character forms a `StageName` may take, and in
//! whether a stage declares entry/success/failure conditions. Every assertion goes
//! through the router, so nothing here can pass by reading a handler's internals.

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{build_stage, source_stage, Emulator, ROLE_ARN};

/// One row of the table: a label (also the pipeline name, which satisfies
/// `PipelineName`'s modelled pattern `^[A-Za-z0-9.@\-_]+$`) and the stages the
/// declaration carries.
struct Case {
    label: &'static str,
    stages: Vec<Value>,
}

/// A stage declaring an entry, a success and a failure condition.
///
/// `BeforeEntryConditions$conditions`, `SuccessConditions$conditions` and
/// `FailureConditions$conditions` are each modelled `len 1..1`, and
/// `Condition$rules` `len 1..5`, so one condition per set with between one and five
/// rules is the whole declarable range.
fn conditioned_stage(name: &str) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Deploy",
            "actionTypeId": {
                "category": "Deploy",
                "owner": "AWS",
                "provider": "S3",
                "version": "1",
            },
            "configuration": {"BucketName": "my-bucket", "Extract": "true"},
            "inputArtifacts": [{"name": "SourceOut"}],
        }],
        // `Result`'s modelled values are ROLLBACK, FAIL, RETRY and SKIP;
        // `RuleTypeId$category`'s only value is `Rule`.
        "beforeEntry": {"conditions": [{
            "result": "FAIL",
            "rules": [rule("EntryWindow", "DeploymentWindow")],
        }]},
        "onSuccess": {"conditions": [{
            "result": "FAIL",
            "rules": [rule("SuccessCheck", "VariableCheck"), rule("SecondCheck", "LambdaInvoke")],
        }]},
        "onFailure": {
            "result": "ROLLBACK",
            "conditions": [{
                "result": "ROLLBACK",
                "rules": [rule("FailureCheck", "VariableCheck")],
            }],
        },
    })
}

/// A stage declaring a condition set whose one condition names NO rules.
///
/// `Condition$rules` is an optional member (`len 1..5` when present), so a condition
/// carrying only a `result` is a declarable input — and the state reported for it
/// carries no rule states at all rather than an empty list.
fn rules_less_condition_stage(name: &str) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Deploy",
            "actionTypeId": {
                "category": "Deploy",
                "owner": "AWS",
                "provider": "S3",
                "version": "1",
            },
            "configuration": {"BucketName": "my-bucket", "Extract": "true"},
            "inputArtifacts": [{"name": "SourceOut"}],
        }],
        "onSuccess": {"conditions": [{"result": "SKIP"}]},
    })
}

/// One `RuleDeclaration`: `name` and `ruleTypeId` are its only required members.
fn rule(name: &str, provider: &str) -> Value {
    json!({
        "name": name,
        "ruleTypeId": {
            "category": "Rule",
            "owner": "AWS",
            "provider": provider,
            "version": "1",
        },
        "roleArn": ROLE_ARN,
    })
}

/// The table: every row is a declaration whose state must be reported stage for
/// stage and action for action.
fn cases() -> Vec<Case> {
    vec![
        // The smallest thing that is a pipeline: the source stage, and one stage that
        // acts on what it fetched (`state::validate_declaration`).
        Case {
            label: "smallest",
            stages: vec![source_stage("Source"), build_stage("Build", 1)],
        },
        // Two stages, the second with three actions.
        Case {
            label: "two-stages",
            stages: vec![source_stage("Source"), build_stage("Build", 3)],
        },
        // Four stages with different action counts, so stage order and action order
        // are both observable.
        Case {
            label: "four.stages",
            stages: vec![
                source_stage("Source"),
                build_stage("Build", 2),
                build_stage("Test", 1),
                build_stage("Deploy", 5),
            ],
        },
        // Every character class `StageName`'s pattern (^[A-Za-z0-9.@\-_]+$) allows.
        Case {
            label: "odd@names",
            stages: vec![
                source_stage("Stage.one"),
                build_stage("Stage@two", 1),
                build_stage("Stage-three", 2),
                build_stage("Stage_four", 1),
            ],
        },
        // A stage declaring entry, success and failure conditions, after one that
        // declares none.
        Case {
            label: "conditions_declared",
            stages: vec![source_stage("Source"), conditioned_stage("Deploy")],
        },
        // A declared condition that names no rules at all.
        Case {
            label: "condition_without_rules",
            stages: vec![
                source_stage("Source"),
                rules_less_condition_stage("Deploy"),
            ],
        },
    ]
}

fn declaration_of(case: &Case) -> Value {
    json!({
        "name": case.label,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": case.stages,
    })
}

/// The `name` of every element of `list`, in order. Used on both the declaration and
/// the reported state, so the two are compared as sequences.
fn names(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list, got {list}"))
        .iter()
        .map(|entry| {
            entry[key]
                .as_str()
                .unwrap_or_else(|| panic!("expected a {key} on {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_state_reports_every_stored_stage_and_action_in_order() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let declaration = declaration_of(&case);
        let created = emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration}))
            .await;
        let stored = created["pipeline"].clone();
        let metadata = emulator
            .ok("GetPipeline", &json!({"name": label}))
            .await["metadata"]
            .clone();

        let state = emulator
            .ok("GetPipelineState", &json!({"name": label}))
            .await;

        assert_eq!(
            state["pipelineName"].as_str(),
            Some(label),
            "case {label:?}: expected the stored pipeline name, got {state}"
        );
        assert_eq!(
            state["pipelineVersion"], stored["version"],
            "case {label:?}: expected the stored version {}, got {}",
            stored["version"], state["pipelineVersion"]
        );
        assert_eq!(
            state["created"], metadata["created"],
            "case {label:?}: expected the stored creation instant {}, got {}",
            metadata["created"], state["created"]
        );
        assert_eq!(
            state["updated"], metadata["updated"],
            "case {label:?}: expected the stored update instant {}, got {}",
            metadata["updated"], state["updated"]
        );

        let declared_stages = stored["stages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: stages is a list, got {stored}"))
            .clone();
        let reported_stages = state["stageStates"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected stageStates, got {state}"))
            .clone();
        assert_eq!(
            names(&state["stageStates"], "stageName"),
            names(&stored["stages"], "name"),
            "case {label:?}: expected one stage state per stored stage, in order, got {state}"
        );

        for (position, (declared, reported)) in declared_stages
            .iter()
            .zip(reported_stages.iter())
            .enumerate()
        {
            let stage = declared["name"].as_str().unwrap_or_default();
            assert_eq!(
                names(&reported["actionStates"], "actionName"),
                names(&declared["actions"], "name"),
                "case {label:?} stage {stage:?}: expected one action state per stored \
                 action, in order, got {reported}"
            );

            // EVERY stage reports the state of the transition INTO it, the first
            // stage's included: a transition no call has disabled is enabled and
            // carries no disable metadata.
            let transition = reported
                .get("inboundTransitionState")
                .unwrap_or_else(|| panic!(
                    "case {label:?} stage {stage:?}: expected an inbound transition \
                     state, got {reported}"
                ));
            assert_eq!(
                transition["enabled"].as_bool(),
                Some(true),
                "case {label:?} stage {stage:?}: a transition no call has disabled is \
                 enabled, got {transition}"
            );
            assert!(
                transition.get("disabledReason").is_none()
                    && transition.get("lastChangedAt").is_none()
                    && transition.get("lastChangedBy").is_none(),
                "case {label:?} stage {stage:?}: an enabled transition carries no \
                 disable metadata, got {transition}"
            );

            // Creating a pipeline RUNS it once, and ENTERING a stage is itself a
            // clock-owed step (`state::execution::ACTION_STEP`), so at the instant this
            // state is read — within that span of the create — the run that creation
            // started has entered NO stage yet: it is waiting at the first one, which
            // reports it as the execution inbound to it, and nothing has reached any
            // stage at all. A revision only `PutActionRevision` records is not reported
            // anywhere either.
            let waited_at_by_the_creation_run = position == 0;
            for absent in ["latestExecution", "inboundExecution", "inboundExecutions"] {
                if waited_at_by_the_creation_run && absent == "inboundExecution" {
                    continue;
                }
                assert!(
                    reported.get(absent).is_none(),
                    "case {label:?} stage {stage:?}: nothing ran this stage, so {absent} \
                     must be absent, got {reported}"
                );
            }
            for action in reported["actionStates"].as_array().expect("action states") {
                for absent in ["currentRevision", "entityUrl", "revisionUrl", "latestExecution"] {
                    assert!(
                        action.get(absent).is_none(),
                        "case {label:?} stage {stage:?}: nothing ran this action, so \
                         {absent} must be absent, got {action}"
                    );
                }
            }

            // Each declared condition set is reported with its rules, by name, and a
            // set the stage does not declare is not reported at all.
            for (declared_key, reported_key) in [
                ("beforeEntry", "beforeEntryConditionState"),
                ("onSuccess", "onSuccessConditionState"),
                ("onFailure", "onFailureConditionState"),
            ] {
                let Some(conditions) = declared
                    .get(declared_key)
                    .and_then(|set| set.get("conditions"))
                else {
                    assert!(
                        reported.get(reported_key).is_none(),
                        "case {label:?} stage {stage:?}: {declared_key} is not declared, so \
                         {reported_key} must be absent, got {reported}"
                    );
                    continue;
                };
                let condition_set_state = reported.get(reported_key).unwrap_or_else(|| {
                    panic!(
                        "case {label:?} stage {stage:?}: {declared_key} is declared, so \
                         {reported_key} must be reported, got {reported}"
                    )
                });
                assert!(
                    condition_set_state.get("latestExecution").is_none(),
                    "case {label:?} stage {stage:?}: nothing evaluated {declared_key}, so \
                     its latestExecution must be absent, got {condition_set_state}"
                );
                let declared_conditions =
                    conditions.as_array().expect("conditions is a list").clone();
                let reported_conditions = condition_set_state["conditionStates"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {label:?} stage {stage:?}: expected condition states for \
                             {declared_key}, got {condition_set_state}"
                        )
                    })
                    .clone();
                assert_eq!(
                    reported_conditions.len(),
                    declared_conditions.len(),
                    "case {label:?} stage {stage:?}: expected one condition state per \
                     declared {declared_key} condition, got {condition_set_state}"
                );
                for (declared_condition, reported_condition) in
                    declared_conditions.iter().zip(reported_conditions.iter())
                {
                    // `Condition$rules` is optional: a condition that names rules is
                    // reported with one rule state per rule, in order, and one that
                    // names none carries no rule states at all.
                    match declared_condition.get("rules") {
                        Some(declared_rules) => assert_eq!(
                            names(&reported_condition["ruleStates"], "ruleName"),
                            names(declared_rules, "name"),
                            "case {label:?} stage {stage:?}: expected one rule state per \
                             declared {declared_key} rule, in order, got {reported_condition}"
                        ),
                        None => assert!(
                            reported_condition.get("ruleStates").is_none(),
                            "case {label:?} stage {stage:?}: the {declared_key} condition \
                             declares no rules, so ruleStates must be absent, got \
                             {reported_condition}"
                        ),
                    }
                    for rule_state in reported_condition["ruleStates"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        assert!(
                            rule_state.get("latestExecution").is_none()
                                && rule_state.get("currentRevision").is_none(),
                            "case {label:?} stage {stage:?}: nothing ran this rule, so it \
                             carries no execution, got {rule_state}"
                        );
                    }
                }
            }
        }
    }
}

/// The state follows the store: a pipeline that was never created, and one that was
/// deleted, are both not found — with the modelled 400, not an empty state.
#[tokio::test]
async fn an_unknown_or_deleted_pipeline_has_no_state() {
    let emulator = Emulator::new();
    for case in cases() {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration_of(&case)}))
            .await;
    }

    for name in ["missing", "smallest.gone", "SMALLEST"] {
        let (status, code) = emulator
            .expect_error("GetPipelineState", &json!({"name": name}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "name {name:?}: an unknown pipeline has no state"
        );
    }

    emulator
        .ok("DeletePipeline", &json!({"name": "two-stages"}))
        .await;
    let (status, code) = emulator
        .expect_error("GetPipelineState", &json!({"name": "two-stages"}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineNotFoundException"),
        "a deleted pipeline has no state"
    );
    // Deleting one pipeline leaves the others reportable.
    let survivor = emulator
        .ok("GetPipelineState", &json!({"name": "smallest"}))
        .await;
    assert_eq!(
        survivor["pipelineName"].as_str(),
        Some("smallest"),
        "deleting one pipeline must not remove another's state, got {survivor}"
    );
}

/// Reporting the state changes nothing a later caller sees: the same request answers
/// the same thing, and reading the state never starts an execution. What a started
/// execution DOES publish is the stage and action state the execution plane records
/// (`crate::state::driver`), which the execution-engine suite covers.
#[tokio::test]
async fn reporting_the_state_changes_nothing() {
    let emulator = Emulator::new();
    let case = Case {
        label: "repeatable",
        stages: vec![source_stage("Source"), build_stage("Build", 2)],
    };
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration_of(&case)}))
        .await;

    let request = json!({"name": "repeatable"});
    let first = emulator.ok("GetPipelineState", &request).await;
    let second = emulator.ok("GetPipelineState", &request).await;
    assert_eq!(
        first, second,
        "a read must report the same state twice, got {first} then {second}"
    );

    emulator
        .ok("StartPipelineExecution", &json!({"name": "repeatable"}))
        .await;
    // The declaration half of the answer is unchanged by a start: the same stages, in
    // the same order, with the same action names and transition states. Only the
    // execution members the start really created are added to it.
    let after_start = emulator.ok("GetPipelineState", &request).await;
    let shape_of = |answer: &Value| {
        answer["stageStates"]
            .as_array()
            .expect("stage states are reported")
            .iter()
            .map(|stage| {
                (
                    stage["stageName"].clone(),
                    stage["actionStates"]
                        .as_array()
                        .map(|actions| {
                            actions
                                .iter()
                                .map(|action| action["actionName"].clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    stage["inboundTransitionState"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape_of(&after_start),
        shape_of(&first),
        "a start must not change the stages, their action names or their transitions, \
         got {after_start}"
    );

    let executions = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({"pipelineName": "repeatable"}),
        )
        .await;
    assert_eq!(
        executions["pipelineExecutionSummaries"]
            .as_array()
            .map(Vec::len),
        // The two the calls above began: the run the pipeline's creation started and
        // the one the start did. Reporting the state begins none.
        Some(2),
        "reporting the state must not start an execution, got {executions}"
    );
}
