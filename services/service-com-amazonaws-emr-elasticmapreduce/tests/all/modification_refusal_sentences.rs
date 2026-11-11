// SPDX-License-Identifier: CC0-1.0

//! What Amazon EMR SAYS when it refuses a modification, and what a fleet resize leaves
//! behind, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! fleet, a group and a policy are only ever read back through `ListInstanceFleets`,
//! `ListInstanceGroups` and `GetManagedScalingPolicy`.
//!
//! THE RULE every table below asserts: a refusal Amazon EMR words with FIXED text is
//! answered with exactly that text — nothing of the request and nothing of the store is
//! appended to it — while a refusal whose text AWS states with values takes those values
//! from the request; and the error identity is the one the refused operation MODELS. The
//! rows span the operations that share each refusal class, and the values the class holds
//! over.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::MAX_INSTANCE_TYPES_PER_FLEET;

use crate::job_flow_lifecycle::{assert_refused, at, Emr, PAST_TRANSITION};
use crate::refusal_messages::message;

/// The error identity the request-validation channel answers, which the operations that
/// model no client error of their own are refused under.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error identity this service models for a bad request, which an operation that
/// declares it answers instead (`smithy_query(query="operation",
/// operation="ModifyCluster,ModifyInstanceFleet,AddTags")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The one sentence the legacy job-flow surface refuses a change to a cluster on its way
/// out with. Entirely fixed text: no cluster id, and no state.
const NOT_MODIFIABLE_SENTENCE: &str =
    "A job flow that is shutting down, terminated, or finished may not be modified.";

/// The one sentence `PutAutoScalingPolicy` refuses the group that runs the primary node
/// with. Entirely fixed text: no group id, and no role.
const MASTER_GROUP_SENTENCE: &str =
    "Auto Scaling policy should not be specified for Master Instance Groups.";

/// The one sentence a managed scaling policy whose lower capacity boundary is above its
/// upper one is refused with. Entirely fixed text: neither capacity appears.
const INVERTED_COMPUTE_LIMITS_SENTENCE: &str = "Invalid configuration for managed scaling. \
                                                The specified max capacity must be greater \
                                                than min capacity.";

/// The sentence a step concurrency level NO step execution mode could take is refused
/// with: no band, because there is no band to aim for.
const BEYOND_EVERY_BAND_SENTENCE: &str = "Invalid step concurrency level.";

/// The sentence a level above the cluster's own ceiling but inside the widest one a mode
/// raises it to is refused with: the level it could not take, the ceiling and the mode that
/// ceiling belongs to, then the mode that would take it and that mode's own ceiling.
fn above_the_band_sentence(level: i64) -> String {
    format!(
        "StepConcurrencyLevel {level} exceeds the maximum (256) for StepExecutionMode \
         STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps."
    )
}

/// The documented band a `StepConcurrencyLevel` lies in ("a minimum of 1 step and a
/// maximum of 256 steps", `ModifyCluster$StepConcurrencyLevel` doc) and the widest ceiling
/// any step execution mode raises it to (the service's
/// `step_execution_mode::DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL`).
const STEP_CONCURRENCY_BAND: (i64, i64) = (1, 256);
const WIDEST_STEP_CONCURRENCY_CEILING: i64 = 6400;

/// A `RunJobFlow` request launching one instance GROUP of each role, kept alive so every
/// refusal below is about the request rather than about a cluster that went away.
fn group_launch(name: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE,
        "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceGroups": [
                { "Name": "primary", "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                { "Name": "core", "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 2 },
                { "Name": "workers", "InstanceRole": "TASK", "InstanceType": "c5.xlarge", "InstanceCount": 3 },
            ],
        },
    })
}

/// A `RunJobFlow` request launching one instance FLEET of each node type, each asking for
/// On-Demand units, so a resize can be asked for and read back.
fn fleet_launch(name: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE,
        "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [
                {
                    "Name": "primary",
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
                {
                    "Name": "core",
                    "InstanceFleetType": "CORE",
                    "TargetOnDemandCapacity": 2,
                    "TargetSpotCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge", "WeightedCapacity": 1 }],
                },
            ],
        },
    })
}

/// One scaling rule the model admits, so a policy's refusal is about the group or the
/// constraints rather than about the rule's own members.
fn rule(name: &str) -> Value {
    json!({
        "Name": name,
        "Action": {
            "SimpleScalingPolicyConfiguration": {
                "AdjustmentType": "CHANGE_IN_CAPACITY",
                "ScalingAdjustment": 1,
            },
        },
        "Trigger": {
            "CloudWatchAlarmDefinition": {
                "ComparisonOperator": "GREATER_THAN",
                "MetricName": "YARNMemoryAvailablePercentage",
                "Period": 300,
                "Threshold": 15.0,
            },
        },
    })
}

/// An `AutoScalingPolicy` request member.
fn auto_scaling_policy(min: i64, max: i64, rules: Vec<Value>) -> Value {
    json!({
        "Constraints": { "MinCapacity": min, "MaxCapacity": max },
        "Rules": rules,
    })
}

/// A `ManagedScalingPolicy` request member with the compute limits a row asks for.
fn managed_scaling_policy(unit_type: &str, minimum: i64, maximum: i64) -> Value {
    json!({
        "ComputeLimits": {
            "UnitType": unit_type,
            "MinimumCapacityUnits": minimum,
            "MaximumCapacityUnits": maximum,
        },
    })
}

/// The group id of each role the launched cluster carries, read back through
/// `ListInstanceGroups`.
async fn group_ids(emr: &Emr, case: &str, cluster_id: &str) -> BTreeMap<String, String> {
    let out = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": cluster_id }))
        .await;
    out["InstanceGroups"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceGroups must answer InstanceGroups, got {out}")
        })
        .iter()
        .map(|group| {
            (
                group["InstanceGroupType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                group["Id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The fleets `ListInstanceFleets` reports for `cluster_id`, keyed by the name they were
/// launched with.
async fn fleets_by_name(emr: &Emr, case: &str, cluster_id: &str) -> BTreeMap<String, Value> {
    let out = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
        .await;
    out["InstanceFleets"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceFleets must answer InstanceFleets, got {out}")
        })
        .iter()
        .map(|fleet| {
            (
                fleet["Name"].as_str().unwrap_or_default().to_string(),
                fleet.clone(),
            )
        })
        .collect()
}

/// The `(TargetOnDemandCapacity, TargetSpotCapacity)` one reported fleet carries.
fn targets(case: &str, fleet: &Value) -> (i64, i64) {
    (
        fleet["TargetOnDemandCapacity"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: a fleet reports both targets, got {fleet}")),
        fleet["TargetSpotCapacity"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: a fleet reports both targets, got {fleet}")),
    )
}

/// Assert a refusal carries `code`, says exactly `says`, and that none of `absent` — the
/// values the emulator must NOT have appended to a fixed sentence — appears in it.
fn assert_says(case: &str, status: u16, payload: &Value, code: &str, says: &str, absent: &[&str]) {
    assert_refused(case, status, payload, code);
    let said = message(case, payload);
    assert_eq!(
        said, says,
        "case {case:?}: expected {says:?}, got {said:?} in {payload}"
    );
    for value in absent {
        assert!(
            !said.contains(value),
            "case {case:?}: the sentence is fixed text, so {value:?} must not appear in \
             {said:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A cluster on its way out: one fixed sentence, on every operation that states it
// ---------------------------------------------------------------------------

/// THE RULE: an operation of the legacy job-flow surface refuses a change to a cluster
/// that is shutting down or gone with that surface's FIXED sentence — the cluster id the
/// request named and the state the store holds it in are no part of it. Rows span the
/// operations that share the class and both states a cluster on its way out is in.
#[tokio::test]
async fn a_change_to_a_cluster_on_its_way_out_is_refused_with_one_fixed_sentence() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(group_launch("on its way out")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let ids = group_ids(&emr, "on its way out", &cluster_id).await;
    let core_id = ids
        .get("CORE")
        .expect("the launch carries a core group")
        .clone();

    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
        .await;

    // TERMINATING straight after the call, TERMINATED once the clock makes it due.
    for state in ["TERMINATING", "TERMINATED"] {
        for (operation, request) in [
            (
                "AddJobFlowSteps",
                json!({
                    "JobFlowId": &cluster_id,
                    "Steps": [{
                        "Name": "after the end",
                        "HadoopJarStep": { "Jar": "command-runner.jar" },
                    }],
                }),
            ),
            (
                "ModifyInstanceGroups",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceGroups": [{ "InstanceGroupId": &core_id, "InstanceCount": 4 }],
                }),
            ),
        ] {
            let case = format!("{operation} on a {state} cluster");
            let (status, payload) = emr.call(operation, request).await;
            assert_says(
                &case,
                status,
                &payload,
                VALIDATION_EXCEPTION,
                NOT_MODIFIABLE_SENTENCE,
                &[&cluster_id, state],
            );
        }
        tokio::time::sleep(PAST_TRANSITION).await;
    }
}

// ---------------------------------------------------------------------------
// The primary instance group: one fixed sentence, whatever the policy carries
// ---------------------------------------------------------------------------

/// THE RULE: `PutAutoScalingPolicy` refuses the group that runs the primary node with its
/// FIXED sentence — the group id and the role are no part of it — and it refuses it
/// BEFORE it reads the submitted policy, so a request that gets the group and the policy
/// both wrong is refused for the group. Rows span the policy forms a caller sends: a
/// well-formed one, one with no rule at all, and ones whose capacity boundaries cannot
/// hold together.
#[tokio::test]
async fn the_primary_instance_group_is_refused_one_fixed_sentence_whatever_the_policy_carries() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(group_launch("primary group")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let ids = group_ids(&emr, "primary group", &cluster_id).await;
    let master_id = ids.get("MASTER").expect("the launch carries a master group");
    let core_id = ids.get("CORE").expect("the launch carries a core group");

    let policies = [
        ("a well-formed policy", auto_scaling_policy(1, 10, vec![rule("scale-out")])),
        ("a policy with no rule", auto_scaling_policy(1, 10, Vec::new())),
        (
            "a policy whose boundaries are inverted",
            auto_scaling_policy(10, 1, vec![rule("scale-out")]),
        ),
        (
            "a policy whose lower boundary is negative",
            auto_scaling_policy(-1, 10, vec![rule("scale-out")]),
        ),
    ];

    for (label, policy) in &policies {
        let case = format!("PutAutoScalingPolicy on the primary group with {label}");
        let (status, payload) = emr
            .call(
                "PutAutoScalingPolicy",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceGroupId": master_id,
                    "AutoScalingPolicy": policy,
                }),
            )
            .await;
        assert_says(
            &case,
            status,
            &payload,
            VALIDATION_EXCEPTION,
            MASTER_GROUP_SENTENCE,
            &[master_id, "MASTER"],
        );
    }

    // The OTHER half of the ordering: a group that CAN carry a policy is refused for the
    // policy, so the sentence above is about the group rather than about every put.
    let case = "PutAutoScalingPolicy on the core group with inverted boundaries";
    let (status, payload) = emr
        .call(
            "PutAutoScalingPolicy",
            json!({
                "ClusterId": &cluster_id,
                "InstanceGroupId": core_id,
                "AutoScalingPolicy": auto_scaling_policy(10, 1, vec![rule("scale-out")]),
            }),
        )
        .await;
    assert_refused(case, status, &payload, VALIDATION_EXCEPTION);
    assert_ne!(
        message(case, &payload),
        MASTER_GROUP_SENTENCE,
        "case {case:?}: a group that may carry a policy is refused for the POLICY, got \
         {payload}"
    );
}

// ---------------------------------------------------------------------------
// Managed scaling compute limits: one fixed sentence for every inverted range
// ---------------------------------------------------------------------------

/// THE RULE: compute limits whose lower boundary is above the upper one are refused with
/// Amazon EMR's FIXED sentence for the class — neither capacity appears in it — wherever a
/// managed scaling policy is attached, and nothing is stored. Rows span both `UnitType`
/// values and the distances the two boundaries can be apart, and the single-instant case
/// (a minimum EQUAL to the maximum) is the other half of the rule: it is a range, so it is
/// stored and reported.
#[tokio::test]
async fn inverted_managed_scaling_compute_limits_are_refused_with_one_fixed_sentence() {
    let emr = Emr::new();

    // `com.amazonaws.emr#ComputeLimitsUnitType`, every value the model declares
    // (`smithy_query(query="shape", shape="ComputeLimitsUnitType")`).
    let unit_types = ["InstanceFleetUnits", "Instances", "VCPU"];

    for unit_type in unit_types {
        for (minimum, maximum) in [(10, 4), (10, 2), (1, 0), (6400, 1), (2, -1)] {
            let case = format!("PutManagedScalingPolicy {unit_type} {minimum}..{maximum}");
            let cluster_id = emr.run_job_flow(group_launch("inverted limits")).await;
            let (status, payload) = emr
                .call(
                    "PutManagedScalingPolicy",
                    json!({
                        "ClusterId": &cluster_id,
                        "ManagedScalingPolicy": managed_scaling_policy(
                            unit_type, minimum, maximum,
                        ),
                    }),
                )
                .await;
            assert_says(
                &case,
                status,
                &payload,
                VALIDATION_EXCEPTION,
                INVERTED_COMPUTE_LIMITS_SENTENCE,
                &[&minimum.to_string(), &maximum.to_string()],
            );

            // Refused means not stored.
            let got = emr
                .ok("GetManagedScalingPolicy", json!({ "ClusterId": &cluster_id }))
                .await;
            assert!(
                at(&got, &["ManagedScalingPolicy"]).is_null(),
                "case {case:?}: a refused policy must not be attached, got {got}"
            );

            // The same class, at the other place a policy is attached.
            let case = format!("RunJobFlow {unit_type} {minimum}..{maximum}");
            let mut request = group_launch("inverted limits at launch");
            request["ManagedScalingPolicy"] =
                managed_scaling_policy(unit_type, minimum, maximum);
            let (status, payload) = emr.call("RunJobFlow", request).await;
            assert_says(
                &case,
                status,
                &payload,
                VALIDATION_EXCEPTION,
                INVERTED_COMPUTE_LIMITS_SENTENCE,
                &[&minimum.to_string(), &maximum.to_string()],
            );
        }

        // A minimum EQUAL to the maximum is a range the activity can stay inside, so it
        // is stored and read back.
        let case = format!("PutManagedScalingPolicy {unit_type} 4..4");
        let cluster_id = emr.run_job_flow(group_launch("equal limits")).await;
        emr.ok(
            "PutManagedScalingPolicy",
            json!({
                "ClusterId": &cluster_id,
                "ManagedScalingPolicy": managed_scaling_policy(unit_type, 4, 4),
            }),
        )
        .await;
        let got = emr
            .ok("GetManagedScalingPolicy", json!({ "ClusterId": &cluster_id }))
            .await;
        assert_eq!(
            at(
                &got,
                &["ManagedScalingPolicy", "ComputeLimits", "MinimumCapacityUnits"],
            )
            .as_i64(),
            Some(4),
            "case {case:?}: a stored policy must read back, got {got}"
        );
    }
}

// ---------------------------------------------------------------------------
// StepConcurrencyLevel: the class decides which of the two sentences is answered
// ---------------------------------------------------------------------------

/// THE RULE: a `StepConcurrencyLevel` outside the band the cluster's step execution mode
/// allows is refused under the error identity `ModifyCluster` MODELS, with the sentence of
/// the class it falls into — the level, the ceiling it exceeded and the mode that would take
/// it for a level another mode would take, and a sentence naming no band at all, and nothing
/// of the request, for one no mode could take. Rows walk each published bound of both bands,
/// one past each, and far past both.
#[tokio::test]
async fn an_out_of_range_step_concurrency_level_is_refused_per_class() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(group_launch("concurrency")).await;
    let (low, high) = STEP_CONCURRENCY_BAND;
    let widest = WIDEST_STEP_CONCURRENCY_CEILING;

    for (level, says, echoes_the_level) in [
        (low - 1, BEYOND_EVERY_BAND_SENTENCE.to_string(), false),
        (-1, BEYOND_EVERY_BAND_SENTENCE.to_string(), false),
        (i64::from(i32::MIN), BEYOND_EVERY_BAND_SENTENCE.to_string(), false),
        (high + 1, above_the_band_sentence(high + 1), true),
        (high + 2, above_the_band_sentence(high + 2), true),
        (widest - 1, above_the_band_sentence(widest - 1), true),
        (widest, above_the_band_sentence(widest), true),
        (widest + 1, BEYOND_EVERY_BAND_SENTENCE.to_string(), false),
        (i64::from(i32::MAX), BEYOND_EVERY_BAND_SENTENCE.to_string(), false),
    ] {
        let case = format!("ModifyCluster with StepConcurrencyLevel {level}");
        let (status, payload) = emr
            .call(
                "ModifyCluster",
                json!({ "ClusterId": &cluster_id, "StepConcurrencyLevel": level }),
            )
            .await;
        // The class that names no band names nothing of the request either, so the level
        // must not have been appended to it.
        let absent: Vec<String> = if echoes_the_level {
            Vec::new()
        } else {
            vec![level.to_string()]
        };
        let absent: Vec<&str> = absent.iter().map(String::as_str).collect();
        assert_says(
            &case,
            status,
            &payload,
            INVALID_REQUEST_EXCEPTION,
            &says,
            &absent,
        );

        // A refused modification changes nothing: the cluster keeps the level it holds.
        let described = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            described["StepConcurrencyLevel"].as_i64(),
            Some(low),
            "case {case:?}: a refused modification must leave the level alone, got \
             {described}"
        );
    }

    // Both published bounds of the cluster's own band are inside it.
    for level in [low, high, (low + high) / 2] {
        let case = format!("ModifyCluster with StepConcurrencyLevel {level}");
        emr.ok(
            "ModifyCluster",
            json!({ "ClusterId": &cluster_id, "StepConcurrencyLevel": level }),
        )
        .await;
        let described = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            described["StepConcurrencyLevel"].as_i64(),
            Some(level),
            "case {case:?}: the stored level must read back, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// ModifyInstanceFleet: the resize it applies, and the capacities it refuses
// ---------------------------------------------------------------------------

/// THE RULE: `ModifyInstanceFleet` applies the submitted targets to the fleet the request
/// names and `ListInstanceFleets` reports them, leaving the cluster's other fleets alone;
/// a member the configuration omits asks for no change. Rows span which markets a resize
/// asks units of, the direction it runs, and the zero both markets may be asked for.
#[tokio::test]
async fn a_submitted_target_capacity_is_what_the_fleet_requests() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(fleet_launch("resized")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let core_id = fleets_by_name(&emr, "resized", &cluster_id).await["core"]["Id"]
        .as_str()
        .expect("a listed fleet carries an Id")
        .to_string();

    // `(case, the modification, the targets the core fleet must then report)`. The fleet
    // launched asking for 2 On-Demand and 1 Spot unit, and each row starts from what the
    // row before it left.
    let rows: Vec<(&str, Value, (i64, i64))> = vec![
        ("up, On-Demand only", json!({ "TargetOnDemandCapacity": 5 }), (5, 1)),
        ("up, Spot only", json!({ "TargetSpotCapacity": 4 }), (5, 4)),
        (
            "both markets at once",
            json!({ "TargetOnDemandCapacity": 3, "TargetSpotCapacity": 7 }),
            (3, 7),
        ),
        ("down to the documented floor", json!({ "TargetOnDemandCapacity": 0 }), (0, 7)),
        (
            "no units of either market",
            json!({ "TargetOnDemandCapacity": 0, "TargetSpotCapacity": 0 }),
            (0, 0),
        ),
        ("far up", json!({ "TargetSpotCapacity": 1_000 }), (0, 1_000)),
        ("nothing but the fleet id", json!({}), (0, 1_000)),
    ];

    for (case, modification, expected) in rows {
        let mut fleet = modification.clone();
        fleet["InstanceFleetId"] = json!(&core_id);
        let out = emr
            .ok(
                "ModifyInstanceFleet",
                json!({ "ClusterId": &cluster_id, "InstanceFleet": fleet }),
            )
            .await;
        assert!(
            out.as_object().is_some_and(serde_json::Map::is_empty) || out.is_null(),
            "case {case:?}: ModifyInstanceFleet answers a body with no members, got {out}"
        );

        let listed = fleets_by_name(&emr, case, &cluster_id).await;
        assert_eq!(
            targets(case, &listed["core"]),
            expected,
            "case {case:?}: the fleet must request what the resize asked for, got {:?}",
            listed["core"]
        );
        // The fleet beside it is untouched: it still asks for what it launched with.
        assert_eq!(
            targets(case, &listed["primary"]),
            (1, 0),
            "case {case:?}: the other fleet must be left alone, got {:?}",
            listed["primary"]
        );
    }

    // An instance type the fleet did not hold a configuration for is added to it, and one
    // it did holds the submitted configuration from then on.
    let case = "an added instance type";
    emr.ok(
        "ModifyInstanceFleet",
        json!({
            "ClusterId": &cluster_id,
            "InstanceFleet": {
                "InstanceFleetId": &core_id,
                "InstanceTypeConfigs": [
                    { "InstanceType": "m5.xlarge", "WeightedCapacity": 4 },
                    { "InstanceType": "r5.2xlarge", "WeightedCapacity": 8 },
                ],
            },
        }),
    )
    .await;
    let listed = fleets_by_name(&emr, case, &cluster_id).await;
    let specifications: Vec<(&str, Option<i64>)> = listed["core"]["InstanceTypeSpecifications"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["InstanceType"].as_str().unwrap_or_default(),
                        entry["WeightedCapacity"].as_i64(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        specifications,
        vec![("m5.xlarge", Some(4)), ("r5.2xlarge", Some(8))],
        "case {case:?}: the submitted configurations must be what the fleet holds, got {:?}",
        listed["core"]
    );

    // The stored list is BOUNDED (the service's `MAX_INSTANCE_TYPES_PER_FLEET`): a merge
    // that would take the fleet past the ceiling is refused, and the fleet keeps what it
    // held. The two types above are already on it.
    let case = "one instance type more than a fleet holds";
    let past_the_ceiling: Vec<Value> = (0..MAX_INSTANCE_TYPES_PER_FLEET)
        .map(|at| json!({ "InstanceType": format!("z{at}.xlarge"), "WeightedCapacity": 1 }))
        .collect();
    let (status, payload) = emr
        .call(
            "ModifyInstanceFleet",
            json!({
                "ClusterId": &cluster_id,
                "InstanceFleet": {
                    "InstanceFleetId": &core_id,
                    "InstanceTypeConfigs": past_the_ceiling,
                },
            }),
        )
        .await;
    assert_refused(case, status, &payload, INVALID_REQUEST_EXCEPTION);
    let listed = fleets_by_name(&emr, case, &cluster_id).await;
    assert_eq!(
        listed["core"]["InstanceTypeSpecifications"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "case {case:?}: a refused merge must leave the fleet's configurations alone, got {:?}",
        listed["core"]
    );
}

/// THE RULE: a target capacity below zero names no number of Amazon EC2 units, so it is
/// refused under the error identity `ModifyInstanceFleet` MODELS, with the sentence that
/// names the rejected value and the fleet the request named — and the fleet's capacities
/// are left exactly as they were. Rows span which of the two members carries the bad
/// value, the magnitudes a client sends, and a request that gets both wrong (refused for
/// the first member the model declares).
#[tokio::test]
async fn a_negative_fleet_target_capacity_is_refused_and_changes_nothing() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(fleet_launch("negative targets")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let core_id = fleets_by_name(&emr, "negative targets", &cluster_id).await["core"]["Id"]
        .as_str()
        .expect("a listed fleet carries an Id")
        .to_string();

    // `(the modification's capacities, the value the sentence must name)`.
    let rows: Vec<(Value, i64)> = vec![
        (json!({ "TargetOnDemandCapacity": -5, "TargetSpotCapacity": 0 }), -5),
        (json!({ "TargetOnDemandCapacity": -1 }), -1),
        (json!({ "TargetSpotCapacity": -1 }), -1),
        (json!({ "TargetOnDemandCapacity": 3, "TargetSpotCapacity": -7 }), -7),
        // Both wrong: the sentence names the FIRST member the model declares.
        (json!({ "TargetOnDemandCapacity": -2, "TargetSpotCapacity": -9 }), -2),
        (json!({ "TargetOnDemandCapacity": -6400 }), -6400),
        (json!({ "TargetSpotCapacity": i64::from(i32::MIN) }), i64::from(i32::MIN)),
    ];

    for (capacities, rejected) in rows {
        let case = format!("ModifyInstanceFleet with {capacities}");
        let mut fleet = capacities.clone();
        fleet["InstanceFleetId"] = json!(&core_id);
        let (status, payload) = emr
            .call(
                "ModifyInstanceFleet",
                json!({ "ClusterId": &cluster_id, "InstanceFleet": fleet }),
            )
            .await;
        assert_says(
            &case,
            status,
            &payload,
            INVALID_REQUEST_EXCEPTION,
            &format!(
                "Invalid instance fleet capacity '{rejected}' specified for instance fleet \
                 '{core_id}', must be a non-negative number."
            ),
            &[],
        );

        // Refused means unchanged: the fleet still asks for what it launched with.
        let listed = fleets_by_name(&emr, &case, &cluster_id).await;
        assert_eq!(
            targets(&case, &listed["core"]),
            (2, 1),
            "case {case:?}: a refused resize must leave the fleet alone, got {:?}",
            listed["core"]
        );
    }
}

/// THE RULE's other refusals: a resize is refused — with the identity the operation models
/// and nothing written — for a cluster this account holds none of, for an `InstanceFleetId`
/// that cluster carries no fleet for, and for a cluster on its way out, which answers the
/// legacy surface's fixed sentence. Rows span the id forms a caller sends.
#[tokio::test]
async fn a_resize_of_a_fleet_the_service_cannot_reach_is_refused() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(fleet_launch("unreachable")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let core_id = fleets_by_name(&emr, "unreachable", &cluster_id).await["core"]["Id"]
        .as_str()
        .expect("a listed fleet carries an Id")
        .to_string();

    // A cluster id of the right class that names no cluster: the cluster-id sentence.
    for unknown in ["j-ZZZZZZZZZZZZZ", "j-", ""] {
        let case = format!("ModifyInstanceFleet on cluster {unknown:?}");
        let (status, payload) = emr
            .call(
                "ModifyInstanceFleet",
                json!({
                    "ClusterId": unknown,
                    "InstanceFleet": {
                        "InstanceFleetId": &core_id,
                        "TargetOnDemandCapacity": 3,
                    },
                }),
            )
            .await;
        assert_says(
            &case,
            status,
            &payload,
            INVALID_REQUEST_EXCEPTION,
            &format!("Cluster id '{unknown}' is not valid."),
            &[],
        );
    }

    // A fleet id the cluster carries no fleet for.
    for unknown in ["if-ZZZZZZZZZZZZZ", "if-", "", "not-a-fleet-id"] {
        let case = format!("ModifyInstanceFleet on fleet {unknown:?}");
        let (status, payload) = emr
            .call(
                "ModifyInstanceFleet",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceFleet": {
                        "InstanceFleetId": unknown,
                        "TargetOnDemandCapacity": 3,
                    },
                }),
            )
            .await;
        assert_says(
            &case,
            status,
            &payload,
            INVALID_REQUEST_EXCEPTION,
            &format!("Instance fleet id '{unknown}' is not valid."),
            &[],
        );
    }

    // A cluster on its way out: the legacy surface's fixed sentence, as every other
    // modification of such a cluster answers.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&cluster_id] }))
        .await;
    for state in ["TERMINATING", "TERMINATED"] {
        let case = format!("ModifyInstanceFleet on a {state} cluster");
        let (status, payload) = emr
            .call(
                "ModifyInstanceFleet",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceFleet": {
                        "InstanceFleetId": &core_id,
                        "TargetOnDemandCapacity": 3,
                    },
                }),
            )
            .await;
        assert_says(
            &case,
            status,
            &payload,
            INVALID_REQUEST_EXCEPTION,
            NOT_MODIFIABLE_SENTENCE,
            &[&cluster_id, state],
        );
        tokio::time::sleep(PAST_TRANSITION).await;
    }
}

// ---------------------------------------------------------------------------
// The tag operations: the resource is resolved before the call's payload is judged
// ---------------------------------------------------------------------------

/// THE RULE: a tag operation resolves the resource its `ResourceId` names BEFORE it judges
/// what the call carries, so an id naming no resource of this account is refused for the
/// resource — naming the id — whatever payload travels with it, and a present but EMPTY
/// `Tags` list on a resource that DOES exist is refused for naming no tag to add. Rows
/// span both tag operations and the id forms a caller sends.
#[tokio::test]
async fn a_tag_call_is_refused_for_its_resource_before_its_payload() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(group_launch("tagged")).await;

    // `(operation, the payload member it carries)` — including the empty list, which is
    // the payload the resource must still be resolved ahead of.
    let calls: Vec<(&str, Value)> = vec![
        ("AddTags", json!({ "Tags": [{ "Key": "stack", "Value": "Prod" }] })),
        ("AddTags", json!({ "Tags": [] })),
        ("RemoveTags", json!({ "TagKeys": ["stack"] })),
    ];

    for unknown in ["j-ZZZZZZZZZZZZZ", "j-1", "", "not-a-resource-id"] {
        for (operation, payload) in &calls {
            let case = format!("{operation} on {unknown:?} with {payload}");
            let mut request = payload.clone();
            request["ResourceId"] = json!(unknown);
            let (status, answered) = emr.call(operation, request).await;
            assert_says(
                &case,
                status,
                &answered,
                INVALID_REQUEST_EXCEPTION,
                &format!("Cluster id '{unknown}' is not valid."),
                &[],
            );
        }
    }

    // On a resource that DOES exist, the empty list is what is refused.
    let case = "AddTags with an empty Tags list on a live cluster";
    let (status, payload) = emr
        .call("AddTags", json!({ "ResourceId": &cluster_id, "Tags": [] }))
        .await;
    assert_says(
        case,
        status,
        &payload,
        INVALID_REQUEST_EXCEPTION,
        "Tags cannot be null or empty.",
        &[&cluster_id],
    );

    // And a list that names a tag is merged, so the refusal above is the empty list
    // rather than the operation.
    emr.ok(
        "AddTags",
        json!({
            "ResourceId": &cluster_id,
            "Tags": [{ "Key": "stack", "Value": "Prod" }],
        }),
    )
    .await;
    let described = emr.describe_cluster(&cluster_id).await;
    assert_eq!(
        at(&described, &["Tags"]),
        &json!([{ "Key": "stack", "Value": "Prod" }]),
        "case \"a tag that names a tag\": the merged set must read back, got {described}"
    );
}
