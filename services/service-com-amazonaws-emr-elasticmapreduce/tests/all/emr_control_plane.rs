// SPDX-License-Identifier: CC0-1.0

//! The EMR control-plane families that answer a resource id: the cluster-id mutations
//! (`ModifyCluster`, `CancelSteps`, `AddInstanceFleet`, `GetClusterSessionCredentials`),
//! `GetSession`, the Studio and studio-session-mapping families, the notebook-execution
//! readers, `SetUnhealthyNodeReplacement` and the retired `DescribeJobFlows`.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends. Nothing reaches into the service's
//! state struct: what was written is only ever read back through `DescribeCluster`,
//! `ListSteps`, `DescribeStep`, `DescribeStudio` or `GetStudioSessionMapping`.
//!
//! THE RULE every "unknown id" table below asserts: an EMR operation that names a
//! resource by id resolves that id against the service's own state and refuses an id no
//! resource carries with the error its model declares for bad user input — a 400
//! `InvalidRequestException`, or the 400 `ValidationException` channel for an operation
//! whose model declares no client error at all. It never answers 501, never succeeds
//! silently and never invents the resource.

use std::time::Duration;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{
    assert_minted_id, assert_refused, at, kept_alive_request, minimal_request, step_config, Emr,
};

/// Longer than the service's clock-owed transition span (500 ms), so a step that was
/// `CANCEL_PENDING` has reached `CANCELLED` by the time we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// Cluster ids no cluster carries: the shape a real id has, a shorter and a longer
/// body, a prefix with nothing after it, and the empty string (`ClusterId` is
/// `len 0..256`, so the boundary validator passes it on to the handler).
pub(crate) const UNKNOWN_CLUSTER_IDS: &[&str] = &[
    "j-ZZZZZZZZZZZZZ",
    "j-1",
    "j-000000000000000000000000",
    "j-",
    "",
    "not-a-cluster-id",
];

/// Studio ids no studio carries, spanning the same classes.
const UNKNOWN_STUDIO_IDS: &[&str] = &["es-NOPE", "es-ZZZZZZZZZZZZZ", "es-", "", "studio"];

/// The published and chosen bounds this suite takes against the stores the change
/// adds: the maximum number of Studios the emulator holds, the mappings one Studio
/// holds, and the instance fleets one cluster carries (`crate::state`'s `MAX_STUDIOS`,
/// `MAX_SESSION_MAPPINGS_PER_STUDIO` and `MAX_INSTANCE_FLEETS_PER_CLUSTER`).
const MAX_STUDIOS: usize = 100;
const MAX_SESSION_MAPPINGS_PER_STUDIO: usize = 500;
const MAX_INSTANCE_FLEETS_PER_CLUSTER: usize = 50;

/// The bound a batch of job flow ids is capped at: EMR publishes "The maximum number of
/// clusters allowed is 10" for `TerminateJobFlows` and nothing for the `Set*` operations
/// carrying the same `JobFlowIds` member, so the emulator takes the same number for all
/// of them.
pub(crate) const MAX_JOB_FLOW_IDS_PER_REQUEST: usize = 10;

/// A request body for each cluster-id mutation, with `<cluster>` standing in for the
/// cluster id under test.
fn cluster_id_requests(cluster_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "ModifyCluster",
            json!({ "ClusterId": cluster_id, "StepConcurrencyLevel": 2 }),
        ),
        (
            "CancelSteps",
            json!({ "ClusterId": cluster_id, "StepIds": ["s-NOPE"] }),
        ),
        (
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster_id,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        ),
        (
            "GetClusterSessionCredentials",
            json!({ "ClusterId": cluster_id }),
        ),
        (
            "GetSession",
            json!({ "ClusterId": cluster_id, "SessionId": "is-0000000000000" }),
        ),
    ]
}

/// A `RunJobFlow` request whose instances are instance FLEETS, which is what
/// `AddInstanceFleet` may be called on.
fn fleet_request(name: &str) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-7.1.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [{
                "InstanceFleetType": "MASTER",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
            }],
        },
    })
}

/// The members `CreateStudio` requires, with every network id in the form Amazon EC2
/// gives that resource ("Resource IDs", Amazon EC2 User Guide: the type prefix plus 8
/// or 17 lowercase hex characters) — and, because it authenticates through IAM Identity
/// Center, the `UserRole` such a Studio cannot be created without.
///
/// `pub(crate)` so another suite in this test target drives `CreateStudio` through the
/// same request rather than growing a second copy of it.
pub(crate) fn studio_request(name: &str) -> Value {
    json!({
        "Name": name,
        "AuthMode": "SSO",
        "VpcId": "vpc-0a1b2c3d",
        "SubnetIds": ["subnet-0a1b2c3d"],
        "ServiceRole": "EMR_Studio_ServiceRole",
        "UserRole": "EMR_Studio_UserRole",
        "WorkspaceSecurityGroupId": "sg-0a1b2c3d",
        "EngineSecurityGroupId": "sg-1a2b3c4d",
        "DefaultS3Location": "s3://emr-studio/backup/",
    })
}

// ---------------------------------------------------------------------------
// The cluster-id family
// ---------------------------------------------------------------------------

/// Every cluster-id operation refuses an id no cluster carries with
/// `InvalidRequestException`, for every shape of unknown id — and refuses it even while
/// OTHER clusters exist, so the refusal is a lookup and not an empty-store answer.
#[tokio::test]
async fn cluster_id_operations_refuse_an_unknown_cluster() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        for (operation, body) in cluster_id_requests(unknown) {
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(
                &format!("{operation} with cluster id {unknown:?}"),
                status,
                &payload,
                "InvalidRequestException",
            );
        }
    }

    // The live cluster is untouched by every refused call.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
}

/// `ModifyCluster` writes the concurrency level and the extended-support flag, and
/// `DescribeCluster` reports what it wrote. The table spans the level's whole published
/// range (1..256, `ModifyCluster$StepConcurrencyLevel`: "a minimum of 1 step and a
/// maximum of 256 steps"), both `ExtendedSupport` values and the omitted member.
#[tokio::test]
async fn modify_cluster_writes_what_describe_cluster_reports() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("modified")).await;

    // Each row is applied in turn to the same cluster, so what a row does not change
    // must keep the value the row before it left.
    let mut expected_level = 1i64;
    let mut expected_support: Option<bool> = None;
    for (label, request, level, support) in [
        ("the documented minimum", json!({ "StepConcurrencyLevel": 1 }), Some(1), None),
        ("a level in the middle", json!({ "StepConcurrencyLevel": 7 }), Some(7), None),
        (
            "the documented maximum",
            json!({ "StepConcurrencyLevel": 256 }),
            Some(256),
            None,
        ),
        (
            "extended support on, level untouched",
            json!({ "ExtendedSupport": true }),
            None,
            Some(true),
        ),
        (
            "extended support off and a new level",
            json!({ "StepConcurrencyLevel": 3, "ExtendedSupport": false }),
            Some(3),
            Some(false),
        ),
        ("nothing to change", json!({}), None, None),
    ] {
        let mut body = request.clone();
        body["ClusterId"] = json!(&cluster_id);
        let out = emr.ok("ModifyCluster", body).await;
        if let Some(level) = level {
            expected_level = level;
        }
        if let Some(support) = support {
            expected_support = Some(support);
        }
        assert_eq!(
            out["StepConcurrencyLevel"].as_i64(),
            Some(expected_level),
            "case {label:?}: ModifyCluster must answer the level the cluster now carries, \
             got {out}"
        );
        assert_eq!(
            out["ExtendedSupport"].as_bool(),
            expected_support,
            "case {label:?}: ModifyCluster must answer the ExtendedSupport the cluster now \
             carries, got {out}"
        );

        let cluster = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            cluster["StepConcurrencyLevel"].as_i64(),
            Some(expected_level),
            "case {label:?}: DescribeCluster must report the written level, got {cluster}"
        );
        assert_eq!(
            cluster["ExtendedSupport"].as_bool(),
            expected_support,
            "case {label:?}: DescribeCluster must report the written ExtendedSupport, \
             got {cluster}"
        );
    }

    // Outside the published range the request is refused and the cluster keeps the
    // level it holds — one case past each bound and one well past.
    for level in [0, -1, 257, 10_000] {
        let (status, payload) = emr
            .call(
                "ModifyCluster",
                json!({ "ClusterId": &cluster_id, "StepConcurrencyLevel": level }),
            )
            .await;
        assert_refused(
            &format!("StepConcurrencyLevel {level}"),
            status,
            &payload,
            "InvalidRequestException",
        );
        let cluster = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            cluster["StepConcurrencyLevel"].as_i64(),
            Some(expected_level),
            "case {level}: a refused ModifyCluster must change nothing, got {cluster}"
        );
    }
}

/// `CancelSteps` answers one `CancelStepsInfo` per requested step id, in request order:
/// `SUBMITTED` for a step it could act on and `FAILED` with a reason for one it could
/// not. A submitted cancel is visible through `DescribeStep`/`ListSteps` as
/// `CANCEL_PENDING`, and the clock then owes the step `CANCELLED`.
#[tokio::test]
async fn cancel_steps_reports_one_status_per_requested_step() {
    let emr = Emr::new();
    let steps: Vec<Value> = (0..4)
        .map(|index| {
            step_config(
                &format!("step {index}"),
                "s3://jars/work.jar",
                json!([index.to_string()]),
                Value::Null,
            )
        })
        .collect();
    let cluster_id = emr
        .run_job_flow(json!({
            "Name": "cancelled",
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
            "Steps": steps,
        }))
        .await;
    let listed = emr.list_steps(json!({ "ClusterId": &cluster_id })).await;
    // ListSteps reports newest first, so reverse to recover submission order.
    let mut step_ids: Vec<String> = listed["Steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| step["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    step_ids.reverse();
    assert_eq!(
        step_ids.len(),
        4,
        "the fixture must persist four steps, got {listed}"
    );

    // The rows differ in how many ids are asked for, whether they are known, and
    // whether an id repeats inside one request ("CancelSteps is idempotent", so a
    // repeat is still submitted).
    for (label, requested, expected) in [
        ("one known step", vec![step_ids[0].clone()], vec!["SUBMITTED"]),
        (
            "two known steps",
            vec![step_ids[1].clone(), step_ids[2].clone()],
            vec!["SUBMITTED", "SUBMITTED"],
        ),
        (
            "a known step repeated",
            vec![step_ids[3].clone(), step_ids[3].clone()],
            vec!["SUBMITTED", "SUBMITTED"],
        ),
        (
            "an unknown step",
            vec!["s-NOPE".to_string()],
            vec!["FAILED"],
        ),
        (
            "known and unknown together",
            vec![step_ids[0].clone(), "s-NOPE".to_string()],
            vec!["SUBMITTED", "FAILED"],
        ),
        ("no ids at all", Vec::new(), Vec::new()),
    ] {
        let out = emr
            .ok(
                "CancelSteps",
                json!({ "ClusterId": &cluster_id, "StepIds": requested }),
            )
            .await;
        let info: Vec<Value> = out["CancelStepsInfoList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            info.len(),
            expected.len(),
            "case {label:?}: one CancelStepsInfo per requested id, got {out}"
        );
        for ((entry, wanted), asked) in info.iter().zip(&expected).zip(&requested) {
            assert_eq!(
                entry["StepId"].as_str(),
                Some(asked.as_str()),
                "case {label:?}: the entries must follow request order, got {out}"
            );
            assert_eq!(
                entry["Status"].as_str(),
                Some(*wanted),
                "case {label:?}: expected {wanted} for {asked}, got {out}"
            );
            let has_reason = entry["Reason"].as_str().is_some();
            assert_eq!(
                has_reason,
                *wanted == "FAILED",
                "case {label:?}: only a FAILED entry carries a Reason, got {out}"
            );
        }
    }

    // Every cancelled step is CANCEL_PENDING now …
    for step_id in &step_ids {
        let out = emr
            .ok(
                "DescribeStep",
                json!({ "ClusterId": &cluster_id, "StepId": step_id }),
            )
            .await;
        assert_eq!(
            at(&out, &["Step", "Status", "State"]).as_str(),
            Some("CANCEL_PENDING"),
            "case {step_id:?}: a submitted cancel must be observable, got {out}"
        );
    }

    // … and the clock owes it CANCELLED, with the end instant it was cancelled at.
    tokio::time::sleep(PAST_TRANSITION).await;
    for step_id in &step_ids {
        let out = emr
            .ok(
                "DescribeStep",
                json!({ "ClusterId": &cluster_id, "StepId": step_id }),
            )
            .await;
        assert_eq!(
            at(&out, &["Step", "Status", "State"]).as_str(),
            Some("CANCELLED"),
            "case {step_id:?}: the clock owes a cancelled step CANCELLED, got {out}"
        );
        assert!(
            !at(&out, &["Step", "Status", "Timeline", "EndDateTime"]).is_null(),
            "case {step_id:?}: a cancelled step has an EndDateTime, got {out}"
        );
    }

    // A step that has already reached CANCELLED can no longer be cancelled.
    let out = emr
        .ok(
            "CancelSteps",
            json!({ "ClusterId": &cluster_id, "StepIds": [&step_ids[0]] }),
        )
        .await;
    assert_eq!(
        at(&out, &["CancelStepsInfoList"])[0]["Status"].as_str(),
        Some("FAILED"),
        "case \"already cancelled\": a step past cancelling must answer FAILED, got {out}"
    );
}

/// `AddInstanceFleet` mints a fleet id for a cluster whose instances are fleets, and
/// answers that id with the cluster's own id and ARN. A cluster that uses instance
/// GROUPS, and one that is no longer running, are refused.
#[tokio::test]
async fn add_instance_fleet_mints_a_fleet_id_for_a_fleet_cluster() {
    let emr = Emr::new();
    let fleet_cluster = emr.run_job_flow(fleet_request("fleets")).await;
    let group_cluster = emr.run_job_flow(kept_alive_request("groups")).await;
    let expected_arn = emr.describe_cluster(&fleet_cluster).await["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rows differ in fleet type, in which capacities are asked for, and in how
    // many instance types the fleet may draw from.
    let mut minted = Vec::new();
    for (label, fleet) in [
        (
            "on-demand task fleet, one instance type",
            json!({
                "InstanceFleetType": "TASK",
                "TargetOnDemandCapacity": 2,
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
            }),
        ),
        (
            "spot core fleet, three instance types, named",
            json!({
                "Name": "core-spot",
                "InstanceFleetType": "CORE",
                "TargetSpotCapacity": 6,
                "InstanceTypeConfigs": [
                    { "InstanceType": "m5.xlarge", "WeightedCapacity": 2 },
                    { "InstanceType": "m5.2xlarge", "WeightedCapacity": 4 },
                    { "InstanceType": "r5.xlarge", "WeightedCapacity": 2 },
                ],
            }),
        ),
        (
            "mixed capacities",
            json!({
                "InstanceFleetType": "TASK",
                "TargetOnDemandCapacity": 1,
                "TargetSpotCapacity": 1,
                "InstanceTypeConfigs": [{ "InstanceType": "c5.xlarge" }],
            }),
        ),
    ] {
        let out = emr
            .ok(
                "AddInstanceFleet",
                json!({ "ClusterId": &fleet_cluster, "InstanceFleet": fleet }),
            )
            .await;
        let fleet_id = out["InstanceFleetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: an InstanceFleetId is required, got {out}"))
            .to_string();
        assert_minted_id(label, "if-", &fleet_id);
        assert_eq!(
            out["ClusterId"].as_str(),
            Some(fleet_cluster.as_str()),
            "case {label:?}: the cluster id must be echoed, got {out}"
        );
        assert_eq!(
            out["ClusterArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {label:?}: the ARN must be the cluster's own, got {out}"
        );
        minted.push(fleet_id);
    }
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every added fleet must carry its own id, got {minted:?}"
    );

    // A cluster built from instance groups is not a fleet cluster.
    let (status, payload) = emr
        .call(
            "AddInstanceFleet",
            json!({
                "ClusterId": &group_cluster,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        )
        .await;
    assert_refused(
        "instance-group cluster",
        status,
        &payload,
        "InvalidRequestException",
    );

    // "Adds an instance fleet to a running cluster": one that is shutting down is not.
    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [&fleet_cluster] }),
    )
    .await;
    let (status, payload) = emr
        .call(
            "AddInstanceFleet",
            json!({
                "ClusterId": &fleet_cluster,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        )
        .await;
    assert_refused(
        "terminating cluster",
        status,
        &payload,
        "InvalidRequestException",
    );
}

/// `SetUnhealthyNodeReplacement` writes the setting on every cluster it names, and
/// `DescribeCluster` reports it. An unknown job flow id is refused with the 400
/// `ValidationException` channel (the operation models no client error) and changes
/// NOTHING, however many known ids the same request carries.
#[tokio::test]
async fn set_unhealthy_node_replacement_writes_what_describe_cluster_reports() {
    let emr = Emr::new();
    // The rows differ in the value the cluster was LAUNCHED with, so the write is
    // asserted both as a change and as a no-op.
    let launched_off = emr
        .run_job_flow(json!({
            "Name": "off",
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
                "UnhealthyNodeReplacement": false,
            },
        }))
        .await;
    let launched_on = emr
        .run_job_flow(json!({
            "Name": "on",
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
                "UnhealthyNodeReplacement": true,
            },
        }))
        .await;
    let transient = emr.run_job_flow(minimal_request("transient")).await;

    for (label, ids, requested) in [
        ("one cluster, switched on", vec![launched_off.clone()], true),
        ("one cluster, switched off", vec![launched_on.clone()], false),
        (
            "three clusters at once, all on",
            vec![launched_off.clone(), launched_on.clone(), transient.clone()],
            true,
        ),
        (
            "the same three, all off",
            vec![launched_off.clone(), launched_on.clone(), transient.clone()],
            false,
        ),
    ] {
        emr.ok(
            "SetUnhealthyNodeReplacement",
            json!({ "JobFlowIds": ids, "UnhealthyNodeReplacement": requested }),
        )
        .await;
        for id in &ids {
            let cluster = emr.describe_cluster(id).await;
            assert_eq!(
                cluster["UnhealthyNodeReplacement"].as_bool(),
                Some(requested),
                "case {label:?}: DescribeCluster must report the written value for {id}, \
                 got {cluster}"
            );
        }
    }

    // An unknown id refuses the whole request, so the known cluster beside it keeps
    // the value it holds.
    for unknown in UNKNOWN_CLUSTER_IDS {        let (status, payload) = emr
            .call(
                "SetUnhealthyNodeReplacement",
                json!({
                    "JobFlowIds": [&launched_off, unknown],
                    "UnhealthyNodeReplacement": true,
                }),
            )
            .await;
        assert_refused(
            &format!("job flow id {unknown:?}"),
            status,
            &payload,
            "ValidationException",
        );
        let cluster = emr.describe_cluster(&launched_off).await;
        assert_eq!(
            cluster["UnhealthyNodeReplacement"].as_bool(),
            Some(false),
            "case {unknown:?}: a refused request must change nothing, got {cluster}"
        );
    }

    // A batch of job flow ids is capped: EMR publishes "The maximum number of clusters
    // allowed is 10" for `TerminateJobFlows`' identical member and nothing for this
    // one, so the emulator takes that number here too (CHOSEN by analogy). A batch past
    // it is refused before any cluster is touched, and a batch at the bound is accepted.
    let at_the_bound: Vec<&String> = (0..MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| &launched_off)
        .collect();
    emr.ok(
        "SetUnhealthyNodeReplacement",
        json!({ "JobFlowIds": at_the_bound, "UnhealthyNodeReplacement": true }),
    )
    .await;
    let past_the_bound: Vec<&String> = (0..=MAX_JOB_FLOW_IDS_PER_REQUEST)
        .map(|_| &launched_off)
        .collect();
    let (status, payload) = emr
        .call(
            "SetUnhealthyNodeReplacement",
            json!({ "JobFlowIds": past_the_bound, "UnhealthyNodeReplacement": false }),
        )
        .await;
    assert_refused(
        "one job flow id past the maximum",
        status,
        &payload,
        "ValidationException",
    );
    let cluster = emr.describe_cluster(&launched_off).await;
    assert_eq!(
        cluster["UnhealthyNodeReplacement"].as_bool(),
        Some(true),
        "case \"past the maximum\": a refused batch must change nothing, got {cluster}"
    );
}

/// `GetClusterSessionCredentials` is DECLINED for a cluster that exists too: this
/// emulator launches no instances and serves no username/password cluster endpoint, so
/// it refuses with the modelled `InvalidRequestException` rather than handing back a
/// credential for a service nothing runs. This pins the decline, so it cannot quietly
/// become a fabricated credential.
#[tokio::test]
async fn get_cluster_session_credentials_declines_an_existing_cluster() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("credentials")).await;

    // The rows differ in whether a runtime role is named, which is the only other
    // member the operation takes.
    for (label, body) in [
        ("no execution role", json!({ "ClusterId": &cluster_id })),
        (
            "a runtime role ARN",
            json!({
                "ClusterId": &cluster_id,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
        ),
    ] {
        let (status, payload) = emr.call("GetClusterSessionCredentials", body).await;
        assert_refused(label, status, &payload, "InvalidRequestException");
        assert!(
            payload.get("Credentials").is_none() && payload.get("ExpiresAt").is_none(),
            "case {label:?}: no credential may be handed back, got {payload}"
        );
    }
}

/// Every store this change adds is BOUNDED, so a caller cannot grow the emulator
/// without end: the Studio store, one Studio's session mappings and one cluster's
/// instance fleets each refuse the create past their maximum with
/// `InvalidRequestException`, and what is already held is untouched.
#[tokio::test]
async fn the_new_stores_refuse_a_create_past_their_maximum() {
    let emr = Emr::new();

    // One Studio's session mappings, each under its own identity. This studio is
    // created first, because the section below fills the studio store.
    let studio_id = emr.ok("CreateStudio", studio_request("mapped")).await["StudioId"]
        .as_str()
        .expect("a StudioId")
        .to_string();
    for index in 0..MAX_SESSION_MAPPINGS_PER_STUDIO {
        emr.ok(
            "CreateStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityName": format!("analyst-{index}"),
                "IdentityType": "USER",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            }),
        )
        .await;
    }
    let (status, payload) = emr
        .call(
            "CreateStudioSessionMapping",
            json!({
                "StudioId": &studio_id,
                "IdentityName": "one-too-many",
                "IdentityType": "USER",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            }),
        )
        .await;
    assert_refused(
        "one session mapping past the maximum",
        status,
        &payload,
        "InvalidRequestException",
    );
    // The mapping the studio already holds is untouched.
    let mapping = emr
        .ok(
            "GetStudioSessionMapping",
            json!({ "StudioId": &studio_id, "IdentityName": "analyst-0", "IdentityType": "USER" }),
        )
        .await;
    assert_eq!(
        at(&mapping, &["SessionMapping", "IdentityName"]).as_str(),
        Some("analyst-0"),
        "a refused create must leave the held mappings, got {mapping}"
    );

    // The Studio store: fill it to its maximum (one studio is already held), then one
    // more.
    for index in 1..MAX_STUDIOS {
        let out = emr
            .ok("CreateStudio", studio_request(&format!("studio {index}")))
            .await;
        assert!(
            out["StudioId"].as_str().is_some(),
            "case {index}: a create inside the bound must succeed, got {out}"
        );
    }
    let (status, payload) = emr
        .call("CreateStudio", studio_request("one too many"))
        .await;
    assert_refused(
        "one studio past the maximum",
        status,
        &payload,
        "InvalidRequestException",
    );
    assert!(
        payload.get("StudioId").is_none(),
        "a refused create hands back no studio, got {payload}"
    );
    // The studios already held are untouched.
    let studio = emr.ok("DescribeStudio", json!({ "StudioId": &studio_id })).await;
    assert_eq!(
        at(&studio, &["Studio", "StudioId"]).as_str(),
        Some(studio_id.as_str()),
        "a refused create must leave the held studios, got {studio}"
    );

    // One cluster's instance fleets: the cluster launches with one fleet, so the
    // remaining room is one short of the maximum.
    let cluster_id = emr.run_job_flow(fleet_request("bounded")).await;
    for index in 1..MAX_INSTANCE_FLEETS_PER_CLUSTER {
        let out = emr
            .ok(
                "AddInstanceFleet",
                json!({
                    "ClusterId": &cluster_id,
                    "InstanceFleet": {
                        "InstanceFleetType": "TASK",
                        "TargetOnDemandCapacity": 1,
                        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                    },
                }),
            )
            .await;
        assert!(
            out["InstanceFleetId"].as_str().is_some(),
            "case {index}: an add inside the bound must succeed, got {out}"
        );
    }
    let (status, payload) = emr
        .call(
            "AddInstanceFleet",
            json!({
                "ClusterId": &cluster_id,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        )
        .await;
    assert_refused(
        "one fleet past the maximum",
        status,
        &payload,
        "InvalidRequestException",
    );
    // The cluster itself is untouched by the refusal.
    let cluster = emr.describe_cluster(&cluster_id).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(cluster_id.as_str()),
        "a refused add must leave the cluster readable, got {cluster}"
    );

    // A launch asking for more fleets than a cluster carries is refused too.
    let fleets: Vec<Value> = (0..=MAX_INSTANCE_FLEETS_PER_CLUSTER)
        .map(|_| {
            json!({
                "InstanceFleetType": "TASK",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
            })
        })
        .collect();
    let (status, payload) = emr
        .call(
            "RunJobFlow",
            json!({
                "Name": "too many fleets",
                "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
                "Instances": { "KeepJobFlowAliveWhenNoSteps": true, "InstanceFleets": fleets },
            }),
        )
        .await;
    assert_refused(
        "a launch past the fleet maximum",
        status,
        &payload,
        "ValidationException",
    );
}

/// `GetSession` resolves the session id against the sessions its cluster carries: a
/// well-formed id reaches the lookup and is refused there with
/// `InvalidRequestException`, never answered with an invented session.
#[tokio::test]
async fn get_session_refuses_a_session_the_cluster_does_not_carry() {
    let emr = Emr::new();
    let cluster_id = emr
        .run_job_flow(json!({
            "Name": "session enabled",
            "ReleaseLabel": "emr-7.1.0",
            "SessionEnabled": true,
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
        }))
        .await;

    for session_id in ["is-0000000000000", "is-ZZZZZZZZZZZZZ", "is-a", "is-ad6bbdf71f480"] {
        let (status, payload) = emr
            .call(
                "GetSession",
                json!({ "ClusterId": &cluster_id, "SessionId": session_id }),
            )
            .await;
        assert_refused(
            &format!("session id {session_id:?}"),
            status,
            &payload,
            "InvalidRequestException",
        );
        assert!(
            payload.get("Session").is_none(),
            "case {session_id:?}: a refusal carries no Session, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeJobFlows
// ---------------------------------------------------------------------------

/// The retired `DescribeJobFlows` refuses every call with a 400 `ValidationException`,
/// whatever arguments it is given and whatever clusters exist — it never serves job-flow
/// data.
#[tokio::test]
async fn describe_job_flows_is_retired_whatever_the_arguments() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("described")).await;

    for (label, body) in [
        ("no arguments", json!({})),
        (
            "a known job flow id",
            json!({ "JobFlowIds": [&cluster_id] }),
        ),
        ("an unknown job flow id", json!({ "JobFlowIds": ["j-NOPE"] })),
        ("job flow states", json!({ "JobFlowStates": ["RUNNING"] })),
        (
            "a creation window",
            json!({ "CreatedAfter": 0, "CreatedBefore": 4_000_000_000i64 }),
        ),
    ] {
        let (status, payload) = emr.call("DescribeJobFlows", body).await;
        assert_refused(label, status, &payload, "ValidationException");
        assert!(
            payload.get("JobFlows").is_none(),
            "case {label:?}: the refusal must serve no job flows, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// EMR Studio
// ---------------------------------------------------------------------------

/// `CreateStudio` persists a studio built from the request and answers its minted id
/// and access URL; `DescribeStudio` answers the SAME studio back; `UpdateStudio`
/// changes only what it names; `DeleteStudio` removes it. The table spans both
/// `AuthMode` values, one to five subnets, and the optional members present or absent.
#[tokio::test]
async fn studios_are_created_read_back_updated_and_deleted() {
    let emr = Emr::new();

    let cases = vec![
        ("the smallest request", studio_request("minimal")),
        (
            // "Only specify a UserRole when you use IAM Identity Center
            // authentication." (`CreateStudio$UserRole` doc.) The model leaves UserRole
            // optional, so an IAM-authenticated Studio that names none is a well-formed
            // create.
            "IAM auth with no user role",
            json!({
                "Name": "iam studio, no user role",
                "AuthMode": "IAM",
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d"],
                "ServiceRole": "arn:aws:iam::000000000000:role/EMRStudio_Service",
                "WorkspaceSecurityGroupId": "sg-0a1b2c3d",
                "EngineSecurityGroupId": "sg-1a2b3c4d",
                "DefaultS3Location": "s3://emr-studio/iam/",
            }),
        ),
        (
            // An Identity Center Studio carries the user role its identities assume.
            "IAM Identity Center auth with a user role",
            json!({
                "Name": "sso studio",
                "AuthMode": "SSO",
                "VpcId": "vpc-00000000000000003",
                "SubnetIds": ["subnet-00000000000000003"],
                "ServiceRole": "EMR_Studio_ServiceRole",
                "UserRole": "arn:aws:iam::000000000000:role/EMRStudio_User",
                "WorkspaceSecurityGroupId": "sg-00000000000000003",
                "EngineSecurityGroupId": "sg-00000000000000004",
                "DefaultS3Location": "s3://emr-studio/sso/",
            }),
        ),
        (
            "IAM auth with a user role, description and tags",
            json!({
                "Name": "iam studio",
                "Description": "an IAM-authenticated studio",
                "AuthMode": "IAM",
                "VpcId": "vpc-00000000000000001",
                "SubnetIds": ["subnet-00000000000000001", "subnet-0a1b2c3d"],
                "ServiceRole": "arn:aws:iam::000000000000:role/EMRStudio_Service",
                "UserRole": "arn:aws:iam::000000000000:role/EMRStudio_User",
                "WorkspaceSecurityGroupId": "sg-00000000000000001",
                "EngineSecurityGroupId": "sg-00000000000000002",
                "DefaultS3Location": "s3://studio-backup",
                "Tags": [{ "Key": "team", "Value": "analytics" }],
                "EncryptionKeyArn": "arn:aws:kms:us-east-1:000000000000:key/abc",
            }),
        ),
        (
            "the maximum of five subnets, trusted identity propagation",
            json!({
                "Name": "five subnets",
                "AuthMode": "SSO",
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": [
                    "subnet-00000001", "subnet-00000002", "subnet-00000003",
                    "subnet-00000004", "subnet-00000005",
                ],
                "ServiceRole": "EMR_Studio_ServiceRole",
                "UserRole": "EMR_Studio_UserRole",
                "WorkspaceSecurityGroupId": "sg-0a1b2c3d",
                "EngineSecurityGroupId": "sg-1a2b3c4d",
                "DefaultS3Location": "s3://emr-studio/five/",
                "TrustedIdentityPropagationEnabled": true,
                "IdcUserAssignment": "REQUIRED",
                "IdcInstanceArn": "arn:aws:sso:::instance/ssoins-0123456789abcdef",
            }),
        ),
    ];

    let mut created = Vec::new();
    for (label, request) in &cases {
        let out = emr.ok("CreateStudio", request.clone()).await;
        let studio_id = out["StudioId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a StudioId is required, got {out}"))
            .to_string();
        assert_minted_id(label, "es-", &studio_id);
        let url = out["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a Url is required, got {out}"))
            .to_string();
        assert!(
            url.contains(&studio_id),
            "case {label:?}: the access URL must be the studio's own, got {url:?}"
        );

        let studio = emr
            .ok("DescribeStudio", json!({ "StudioId": &studio_id }))
            .await["Studio"]
            .clone();
        assert_eq!(
            studio["StudioId"].as_str(),
            Some(studio_id.as_str()),
            "case {label:?}: DescribeStudio must answer the studio asked for, got {studio}"
        );
        assert_eq!(
            studio["StudioArn"].as_str(),
            Some(
                format!(
                    "arn:aws:elasticmapreduce:us-east-1:{}:studio/{studio_id}",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {label:?}: the studio ARN must name the studio, got {studio}"
        );
        assert_eq!(
            studio["Url"].as_str(),
            Some(url.as_str()),
            "case {label:?}: the reported URL must be the one created, got {studio}"
        );
        assert!(
            !studio["CreationTime"].is_null(),
            "case {label:?}: a created studio records its creation time, got {studio}"
        );
        // Every member the request carried must come back as it was stored.
        for member in [
            "Name",
            "Description",
            "AuthMode",
            "VpcId",
            "SubnetIds",
            "ServiceRole",
            "UserRole",
            "WorkspaceSecurityGroupId",
            "EngineSecurityGroupId",
            "DefaultS3Location",
            "Tags",
            "EncryptionKeyArn",
            "IdcInstanceArn",
            "IdcUserAssignment",
            "TrustedIdentityPropagationEnabled",
        ] {
            assert_eq!(
                studio.get(member),
                request.get(member),
                "case {label:?}: {member} must round-trip, got {studio}"
            );
        }
        created.push(studio_id);
    }

    let mut unique = created.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        created.len(),
        "every studio must carry its own id, got {created:?}"
    );

    // UpdateStudio changes what it names and leaves the rest alone.
    let studio_id = created[0].clone();
    let before = emr
        .ok("DescribeStudio", json!({ "StudioId": &studio_id }))
        .await["Studio"]
        .clone();
    for (label, update) in [
        ("just the name", json!({ "Name": "renamed" })),
        (
            "description and backup location",
            json!({
                "Description": "now described",
                "DefaultS3Location": "s3://emr-studio/moved/",
            }),
        ),
        (
            "more subnets",
            json!({ "SubnetIds": ["subnet-0a1b2c3d", "subnet-00000009"] }),
        ),
        (
            "an encryption key",
            json!({ "EncryptionKeyArn": "arn:aws:kms:us-east-1:000000000000:key/zzz" }),
        ),
    ] {
        let mut body = update.clone();
        body["StudioId"] = json!(&studio_id);
        emr.ok("UpdateStudio", body).await;
        let after = emr
            .ok("DescribeStudio", json!({ "StudioId": &studio_id }))
            .await["Studio"]
            .clone();
        for (member, value) in update.as_object().expect("an update object") {
            assert_eq!(
                after.get(member),
                Some(value),
                "case {label:?}: {member} must be the updated value, got {after}"
            );
        }
        // What the update did not name keeps the value it had.
        for member in ["StudioId", "StudioArn", "AuthMode", "VpcId", "CreationTime"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {label:?}: {member} must not change, got {after}"
            );
        }
    }

    // DeleteStudio removes it, and the operations that address it then refuse.
    emr.ok("DeleteStudio", json!({ "StudioId": &studio_id }))
        .await;
    let (status, payload) = emr
        .call("DescribeStudio", json!({ "StudioId": &studio_id }))
        .await;
    assert_refused(
        "a deleted studio",
        status,
        &payload,
        "InvalidRequestException",
    );
    // The studios beside it are untouched.
    for other in &created[1..] {
        let studio = emr.ok("DescribeStudio", json!({ "StudioId": other })).await;
        assert_eq!(
            at(&studio, &["Studio", "StudioId"]).as_str(),
            Some(other.as_str()),
            "deleting one studio must leave the others, got {studio}"
        );
    }
}

/// `CreateStudio` resolves the network, role and backup location it is given BEFORE
/// creating anything, and refuses the request with `InvalidRequestException` when one
/// of them cannot name an existing resource. The rows span each member the rule covers,
/// each bound of the published subnet count, and both forms of EC2 resource id.
#[tokio::test]
async fn create_studio_refuses_an_unresolvable_network_role_or_location() {
    let emr = Emr::new();

    for (label, override_members) in [
        ("a VPC id that is not an EC2 id", json!({ "VpcId": "vpc-nope" })),
        (
            "a VPC id of the wrong length",
            json!({ "VpcId": "vpc-0a1b2c3d4" }),
        ),
        ("a VPC id with no body", json!({ "VpcId": "vpc-" })),
        ("a VPC id of another resource kind", json!({ "VpcId": "subnet-0a1b2c3d" })),
        (
            "a subnet id that is not an EC2 id",
            json!({ "SubnetIds": ["subnet-nope"] }),
        ),
        (
            "one good subnet and one bad",
            json!({ "SubnetIds": ["subnet-0a1b2c3d", "subnet-zzzzzzzz"] }),
        ),
        ("no subnets at all", json!({ "SubnetIds": [] })),
        (
            "one subnet past the documented maximum of five",
            json!({ "SubnetIds": [
                "subnet-00000001", "subnet-00000002", "subnet-00000003",
                "subnet-00000004", "subnet-00000005", "subnet-00000006",
            ] }),
        ),
        (
            "a workspace security group that is not an EC2 id",
            json!({ "WorkspaceSecurityGroupId": "sg-nope" }),
        ),
        (
            "an engine security group that is not an EC2 id",
            json!({ "EngineSecurityGroupId": "workspace" }),
        ),
        (
            "a backup location that is not an S3 URI",
            json!({ "DefaultS3Location": "emr-studio/backup/" }),
        ),
        (
            "a backup location with no bucket",
            json!({ "DefaultS3Location": "s3://" }),
        ),
        ("an empty service role", json!({ "ServiceRole": "" })),
    ] {
        let mut body = studio_request("refused");
        for (member, value) in override_members.as_object().expect("an override object") {
            body[member] = value.clone();
        }
        let (status, payload) = emr.call("CreateStudio", body).await;
        assert_refused(label, status, &payload, "InvalidRequestException");
        assert!(
            payload.get("StudioId").is_none(),
            "case {label:?}: a refused create hands back no studio, got {payload}"
        );
    }

    // The same request with every member resolvable is accepted, so the rows above
    // were refused for the member they changed and nothing else.
    let out = emr.ok("CreateStudio", studio_request("accepted")).await;
    assert!(
        out["StudioId"].as_str().is_some(),
        "the control request must be accepted, got {out}"
    );
}

/// Every operation that addresses a studio by id refuses an id no studio carries with
/// `InvalidRequestException`, for every shape of unknown id — and refuses it while
/// another studio exists, so the refusal is a lookup.
#[tokio::test]
async fn studio_id_operations_refuse_an_unknown_studio() {
    let emr = Emr::new();
    let live = emr.ok("CreateStudio", studio_request("live")).await["StudioId"]
        .as_str()
        .expect("a StudioId")
        .to_string();

    for unknown in UNKNOWN_STUDIO_IDS {
        for (operation, body) in [
            ("DescribeStudio", json!({ "StudioId": unknown })),
            (
                "UpdateStudio",
                json!({ "StudioId": unknown, "Name": "renamed" }),
            ),
            ("DeleteStudio", json!({ "StudioId": unknown })),
            (
                "CreateStudioSessionMapping",
                json!({
                    "StudioId": unknown,
                    "IdentityName": "analyst",
                    "IdentityType": "USER",
                    "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
                }),
            ),
            (
                "GetStudioSessionMapping",
                json!({ "StudioId": unknown, "IdentityName": "analyst", "IdentityType": "USER" }),
            ),
            (
                "UpdateStudioSessionMapping",
                json!({
                    "StudioId": unknown,
                    "IdentityName": "analyst",
                    "IdentityType": "USER",
                    "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
                }),
            ),
            (
                "DeleteStudioSessionMapping",
                json!({ "StudioId": unknown, "IdentityName": "analyst", "IdentityType": "USER" }),
            ),
        ] {
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(
                &format!("{operation} with studio id {unknown:?}"),
                status,
                &payload,
                "InvalidRequestException",
            );
        }
    }

    let studio = emr.ok("DescribeStudio", json!({ "StudioId": &live })).await;
    assert_eq!(
        at(&studio, &["Studio", "StudioId"]).as_str(),
        Some(live.as_str()),
        "the live studio must still read back, got {studio}"
    );
}

/// `CreateStudioSessionMapping` persists a mapping that `GetStudioSessionMapping`
/// answers back, `UpdateStudioSessionMapping` replaces only its session policy, and
/// `DeleteStudioSessionMapping` removes it. The table spans both `IdentityType` values
/// and both ways of naming the identity.
#[tokio::test]
async fn session_mappings_are_created_read_back_updated_and_deleted() {
    let emr = Emr::new();
    let studio_id = emr.ok("CreateStudio", studio_request("mapped")).await["StudioId"]
        .as_str()
        .expect("a StudioId")
        .to_string();

    let cases = vec![
        (
            "a user named by id",
            json!({
                "IdentityId": "94482488-3df9-4e8a-9f0a-example",
                "IdentityType": "USER",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            }),
        ),
        (
            "a user named by name",
            json!({
                "IdentityName": "analyst",
                "IdentityType": "USER",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioIntermediate",
            }),
        ),
        (
            "a group named by name",
            json!({
                "IdentityName": "data-science",
                "IdentityType": "GROUP",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioAdvanced",
            }),
        ),
        (
            "a group named by id",
            json!({
                "IdentityId": "11112222-3333-4444-5555-example",
                "IdentityType": "GROUP",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            }),
        ),
    ];

    for (label, request) in &cases {
        let mut body = request.clone();
        body["StudioId"] = json!(&studio_id);
        emr.ok("CreateStudioSessionMapping", body.clone()).await;

        // A second create of the same identity is a conflict, not a second mapping.
        let (status, payload) = emr.call("CreateStudioSessionMapping", body.clone()).await;
        assert_refused(
            &format!("{label} twice"),
            status,
            &payload,
            "InvalidRequestException",
        );

        let mut lookup = request.clone();
        lookup.as_object_mut().expect("an object").remove("SessionPolicyArn");
        lookup["StudioId"] = json!(&studio_id);
        let mapping = emr
            .ok("GetStudioSessionMapping", lookup.clone())
            .await["SessionMapping"]
            .clone();
        assert_eq!(
            mapping["StudioId"].as_str(),
            Some(studio_id.as_str()),
            "case {label:?}: the mapping belongs to its studio, got {mapping}"
        );
        for member in [
            "IdentityId",
            "IdentityName",
            "IdentityType",
            "SessionPolicyArn",
        ] {
            assert_eq!(
                mapping.get(member),
                request.get(member),
                "case {label:?}: {member} must round-trip, got {mapping}"
            );
        }
        assert!(
            !mapping["CreationTime"].is_null() && !mapping["LastModifiedTime"].is_null(),
            "case {label:?}: a created mapping records both instants, got {mapping}"
        );

        // The update replaces the policy and nothing else.
        let mut update = lookup.clone();
        update["SessionPolicyArn"] = json!("arn:aws:iam::aws:policy/EMRStudioAdvanced");
        emr.ok("UpdateStudioSessionMapping", update).await;
        let updated = emr
            .ok("GetStudioSessionMapping", lookup.clone())
            .await["SessionMapping"]
            .clone();
        assert_eq!(
            updated["SessionPolicyArn"].as_str(),
            Some("arn:aws:iam::aws:policy/EMRStudioAdvanced"),
            "case {label:?}: the session policy must be the updated one, got {updated}"
        );
        assert_eq!(
            updated["CreationTime"], mapping["CreationTime"],
            "case {label:?}: an update does not re-create the mapping, got {updated}"
        );

        // The delete removes it, and reading it back then refuses.
        emr.ok("DeleteStudioSessionMapping", lookup.clone()).await;
        let (status, payload) = emr.call("GetStudioSessionMapping", lookup.clone()).await;
        assert_refused(
            &format!("{label} after delete"),
            status,
            &payload,
            "InvalidRequestException",
        );
        let (status, payload) = emr.call("DeleteStudioSessionMapping", lookup).await;
        assert_refused(
            &format!("{label} deleted twice"),
            status,
            &payload,
            "InvalidRequestException",
        );
    }

    // "Either IdentityName or IdentityId must be specified, but not both."
    for (label, identity) in [
        ("neither identity member", json!({})),
        (
            "both identity members",
            json!({ "IdentityId": "94482488-3df9-4e8a-9f0a-example", "IdentityName": "analyst" }),
        ),
        ("an empty identity name", json!({ "IdentityName": "" })),
    ] {
        for operation in [
            "CreateStudioSessionMapping",
            "GetStudioSessionMapping",
            "UpdateStudioSessionMapping",
            "DeleteStudioSessionMapping",
        ] {
            let mut body = json!({
                "StudioId": &studio_id,
                "IdentityType": "USER",
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            });
            for (member, value) in identity.as_object().expect("an object") {
                body[member] = value.clone();
            }
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(
                &format!("{operation} with {label}"),
                status,
                &payload,
                "InvalidRequestException",
            );
        }
    }
}

/// `ListStudios` reports the Studios the account holds, as the summary the model
/// declares, and `ListStudioSessionMappings` reports the mappings of a Studio — both
/// paginated on `Marker`, both projections of what the create, update and delete
/// operations persisted.
///
/// The rows differ along what the listing generalises over: how many resources are held,
/// which Studio is asked for, and which `IdentityType` the mapping listing filters on.
#[tokio::test]
async fn the_studio_listings_report_what_the_studio_operations_persisted() {
    let emr = Emr::new();

    // An account that holds no Studio lists none, and reports no marker.
    let empty = emr.ok("ListStudios", json!({})).await;
    assert_eq!(
        at(&empty, &["Studios"]),
        &json!([]),
        "an account with no Studio lists none, got {empty}"
    );
    assert!(
        empty.get("Marker").is_none(),
        "a last page carries no marker, got {empty}"
    );

    // Three Studios, each with its own name, VPC and auth mode.
    let mut created: Vec<(String, Value)> = Vec::new();
    for (index, (name, auth_mode)) in [
        ("first studio", "IAM"),
        ("second studio", "SSO"),
        ("third studio", "IAM"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut request = studio_request(name);
        request["AuthMode"] = json!(auth_mode);
        request["Description"] = json!(format!("studio number {index}"));
        request["VpcId"] = json!(format!("vpc-0000000{index}"));
        let out = emr.ok("CreateStudio", request.clone()).await;
        created.push((
            out["StudioId"].as_str().expect("a StudioId").to_string(),
            request,
        ));
    }

    let listed = emr.ok("ListStudios", json!({})).await;
    let summaries = at(&listed, &["Studios"])
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("Studios must be a list, got {listed}"));
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created Studio must be listed once, got {listed}"
    );
    for (studio_id, request) in &created {
        let summary = summaries
            .iter()
            .find(|summary| summary["StudioId"].as_str() == Some(studio_id.as_str()))
            .unwrap_or_else(|| panic!("case {studio_id:?}: must be listed, got {listed}"));
        // The summary is the stored Studio's own members, and the detail members the
        // model keeps for DescribeStudio are not in it.
        for member in ["Name", "Description", "VpcId", "AuthMode"] {
            assert_eq!(
                summary.get(member),
                request.get(member),
                "case {studio_id:?}: {member} must be the stored one, got {summary}"
            );
        }
        let described = emr
            .ok("DescribeStudio", json!({ "StudioId": studio_id }))
            .await["Studio"]
            .clone();
        for member in ["Url", "CreationTime"] {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {studio_id:?}: {member} must agree with DescribeStudio, got {summary}"
            );
        }
        for member in ["SubnetIds", "ServiceRole", "StudioArn"] {
            assert!(
                summary.get(member).is_none(),
                "case {studio_id:?}: a summary carries no {member}, got {summary}"
            );
        }
    }

    // A deleted Studio leaves the listing, and the others stay.
    let (removed, _) = created.remove(1);
    emr.ok("DeleteStudio", json!({ "StudioId": &removed })).await;
    let after = emr.ok("ListStudios", json!({})).await;
    let remaining: Vec<&str> = at(&after, &["Studios"])
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["StudioId"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        remaining.len(),
        created.len(),
        "a deleted Studio leaves the listing, got {after}"
    );
    assert!(
        !remaining.contains(&removed.as_str()),
        "the deleted Studio must be gone, got {after}"
    );

    // A marker that names no listed Studio is refused rather than read as a resume point.
    for marker in ["es-NOPE", "", &removed] {
        let (status, payload) = emr.call("ListStudios", json!({ "Marker": marker })).await;
        assert_refused(
            &format!("ListStudios with marker {marker:?}"),
            status,
            &payload,
            "InvalidRequestException",
        );
    }

    // Session mappings: two Studios, each with mappings of both identity types.
    let studio_id = created[0].0.clone();
    let other_studio = created[1].0.clone();
    for (studio, identity, identity_type) in [
        (&studio_id, "analyst", "USER"),
        (&studio_id, "engineer", "USER"),
        (&studio_id, "data-science", "GROUP"),
        (&other_studio, "auditor", "USER"),
    ] {
        emr.ok(
            "CreateStudioSessionMapping",
            json!({
                "StudioId": studio,
                "IdentityName": identity,
                "IdentityType": identity_type,
                "SessionPolicyArn": "arn:aws:iam::aws:policy/EMRStudioBasic",
            }),
        )
        .await;
    }

    // (case, request, the identity names the listing must report)
    for (case, body, expected) in [
        (
            "one studio, no filter",
            json!({ "StudioId": &studio_id }),
            vec!["analyst", "data-science", "engineer"],
        ),
        (
            "one studio, users only",
            json!({ "StudioId": &studio_id, "IdentityType": "USER" }),
            vec!["analyst", "engineer"],
        ),
        (
            "one studio, groups only",
            json!({ "StudioId": &studio_id, "IdentityType": "GROUP" }),
            vec!["data-science"],
        ),
        (
            "the other studio",
            json!({ "StudioId": &other_studio }),
            vec!["auditor"],
        ),
        (
            "every studio",
            json!({}),
            vec!["analyst", "auditor", "data-science", "engineer"],
        ),
        (
            "every studio, groups only",
            json!({ "IdentityType": "GROUP" }),
            vec!["data-science"],
        ),
    ] {
        let listed = emr.ok("ListStudioSessionMappings", body.clone()).await;
        let mut reported: Vec<String> = at(&listed, &["SessionMappings"])
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["IdentityName"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {listed}"
        );
        // Two reads of one listing report the same page: a listing changes nothing.
        assert_eq!(
            emr.ok("ListStudioSessionMappings", body).await,
            listed,
            "case {case:?}: a repeated listing must report the same page"
        );
    }

    // A `StudioId` no Studio carries is refused, the way every Studio-scoped operation
    // refuses it.
    let (status, payload) = emr
        .call("ListStudioSessionMappings", json!({ "StudioId": &removed }))
        .await;
    assert_refused(
        "ListStudioSessionMappings for a deleted studio",
        status,
        &payload,
        "InvalidRequestException",
    );
}

// ---------------------------------------------------------------------------
// Notebook executions
// ---------------------------------------------------------------------------

/// Both notebook-execution operations resolve the execution id against the service's
/// own state and refuse an id no execution carries with `InvalidRequestException`.
#[tokio::test]
async fn notebook_execution_operations_refuse_an_unknown_execution() {
    let emr = Emr::new();
    // A cluster exists, so the refusal cannot be "the emulator holds nothing".
    emr.run_job_flow(kept_alive_request("notebooks")).await;

    for execution_id in [
        "ex-NOPE",
        "ex-00000000000000000000000000",
        "ex-",
        "",
        "notebook",
    ] {
        for operation in ["DescribeNotebookExecution", "StopNotebookExecution"] {
            let (status, payload) = emr
                .call(
                    operation,
                    json!({ "NotebookExecutionId": execution_id }),
                )
                .await;
            assert_refused(
                &format!("{operation} with execution id {execution_id:?}"),
                status,
                &payload,
                "InvalidRequestException",
            );
            assert!(
                payload.get("NotebookExecution").is_none(),
                "case {execution_id:?}: a refusal carries no NotebookExecution, got {payload}"
            );
        }
    }
}
