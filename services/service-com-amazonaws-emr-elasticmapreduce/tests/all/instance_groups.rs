// SPDX-License-Identifier: CC0-1.0

//! The instance groups of an EMR cluster, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! group is only ever read back through `ListInstanceGroups`, which is what makes these
//! tests evidence that the listing is computed from persisted state.
//!
//! THE RULE: `ListInstanceGroups` reports the instance groups the named cluster
//! persisted — the ones its `RunJobFlow` request configured, or the ones its uniform
//! `MasterInstanceType`/`SlaveInstanceType`/`InstanceCount` form implies — in creation
//! order, each as a projection of the stored configuration plus the id EMR minted and
//! the lifecycle the clock moves, in pages a `Marker` resumes; a cluster id the service
//! holds no cluster for, and a marker no group of that cluster carries, are refused with
//! the modelled 400 `InvalidRequestException`.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{
    assert_minted_id, assert_refused, at, kept_alive_request, Emr, REQUEST_TARGET,
};
use crate::security_configurations::audit_capture;

/// This suite's own access key id, which no other suite presents, so the process-wide
/// audit capture can tell its records from a sibling's.
const AUDIT_AKID: &str = "AKIDEMRINSTANCEGROUPS";

/// Longer than the service's clock-owed transition span (500 ms), so a group that was
/// `PROVISIONING`, and a cluster that was `TERMINATING`, has reached its next state by
/// the time we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The chosen bounds this suite takes against the group store (`crate::state`'s
/// `LIST_INSTANCE_GROUPS_PAGE_SIZE`, `MAX_INSTANCE_GROUPS_PER_CLUSTER`,
/// `MAX_EBS_VOLUMES_PER_INSTANCE` and `MAX_EBS_BLOCK_DEVICE_CONFIGS`).
const PAGE_SIZE: usize = 20;
const MAX_INSTANCE_GROUPS_PER_CLUSTER: usize = 50;
const MAX_EBS_VOLUMES_PER_INSTANCE: i64 = 25;
const MAX_EBS_BLOCK_DEVICE_CONFIGS: usize = 25;

/// Cluster ids no cluster carries: the shape a real id has, a prefix with nothing after
/// it, the empty string (`ClusterId` is `len 0..256`, so the boundary validator passes it
/// on to the handler) and a value that is no cluster id at all.
const UNKNOWN_CLUSTER_IDS: &[&str] = &["j-ZZZZZZZZZZZZZ", "j-", "", "not-a-cluster-id"];

/// What one reported `InstanceGroup` must carry. Every field is what the request asked
/// for, so a row states the request's own values rather than anything the emulator
/// invents.
#[derive(Clone)]
struct Expected {
    group_type: &'static str,
    instance_type: &'static str,
    count: i64,
    market: &'static str,
    bid_price: Option<&'static str>,
    name: Option<&'static str>,
    custom_ami_id: Option<&'static str>,
    /// `(device name, volume type)` per EBS volume the group requested, in order.
    ebs_devices: &'static [(&'static str, &'static str)],
    ebs_optimized: Option<bool>,
    configuration_classifications: &'static [&'static str],
    /// `(MinCapacity, MaxCapacity)` of the group's automatic scaling policy.
    auto_scaling: Option<(i64, i64)>,
}

impl Expected {
    /// A group with nothing optional configured.
    const fn group(group_type: &'static str, instance_type: &'static str, count: i64) -> Self {
        Self {
            group_type,
            instance_type,
            count,
            // "Valid values are ON_DEMAND or SPOT" (`InstanceGroup$Market`): a request
            // that names no market provisions On-Demand instances.
            market: "ON_DEMAND",
            bid_price: None,
            name: None,
            custom_ami_id: None,
            ebs_devices: &[],
            ebs_optimized: None,
            configuration_classifications: &[],
            auto_scaling: None,
        }
    }
}

/// The groups `ListInstanceGroups` reports for `cluster_id`, every page of them:
/// following the `Marker` to its end is what collects the whole listing.
async fn list_all_groups(emr: &Emr, case: &str, cluster_id: &str) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "ClusterId": cluster_id });
        if let Some(token) = marker.as_ref() {
            request["Marker"] = json!(token);
        }
        let out = emr.ok("ListInstanceGroups", request).await;
        let page = out["InstanceGroups"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceGroups must answer InstanceGroups, got {out}")
        });
        assert!(
            page.len() <= PAGE_SIZE,
            "case {case:?}: a page holds at most {PAGE_SIZE} groups, got {}",
            page.len()
        );
        all.extend(page);
        match out["Marker"].as_str() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    all
}

/// Assert one reported group is the projection of what the request configured.
fn assert_group(case: &str, expected: &Expected, got: &Value) {
    let id = got["Id"].as_str().unwrap_or_default();
    assert_minted_id(case, "ig-", id);
    assert_eq!(
        got["InstanceGroupType"].as_str(),
        Some(expected.group_type),
        "case {case:?}: wrong InstanceGroupType, got {got}"
    );
    assert_eq!(
        got["InstanceType"].as_str(),
        Some(expected.instance_type),
        "case {case:?}: wrong InstanceType, got {got}"
    );
    assert_eq!(
        got["RequestedInstanceCount"].as_i64(),
        Some(expected.count),
        "case {case:?}: wrong RequestedInstanceCount, got {got}"
    );
    assert_eq!(
        got["Market"].as_str(),
        Some(expected.market),
        "case {case:?}: wrong Market, got {got}"
    );
    assert_eq!(
        got["BidPrice"].as_str(),
        expected.bid_price,
        "case {case:?}: wrong BidPrice, got {got}"
    );
    assert_eq!(
        got["Name"].as_str(),
        expected.name,
        "case {case:?}: wrong Name, got {got}"
    );
    assert_eq!(
        got["CustomAmiId"].as_str(),
        expected.custom_ami_id,
        "case {case:?}: wrong CustomAmiId, got {got}"
    );
    assert_eq!(
        got["EbsOptimized"].as_bool(),
        expected.ebs_optimized,
        "case {case:?}: wrong EbsOptimized, got {got}"
    );

    // One EbsBlockDevice per requested volume, in configuration order.
    let devices: Vec<(&str, &str)> = got["EbsBlockDevices"]
        .as_array()
        .map(|devices| {
            devices
                .iter()
                .map(|device| {
                    (
                        device["Device"].as_str().unwrap_or_default(),
                        at(device, &["VolumeSpecification", "VolumeType"])
                            .as_str()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        devices, expected.ebs_devices,
        "case {case:?}: wrong EbsBlockDevices, got {got}"
    );

    let classifications: Vec<&str> = got["Configurations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Classification"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        classifications, expected.configuration_classifications,
        "case {case:?}: wrong Configurations, got {got}"
    );

    match expected.auto_scaling {
        Some((min, max)) => {
            assert_eq!(
                at(got, &["AutoScalingPolicy", "Constraints", "MinCapacity"]).as_i64(),
                Some(min),
                "case {case:?}: wrong automatic scaling MinCapacity, got {got}"
            );
            assert_eq!(
                at(got, &["AutoScalingPolicy", "Constraints", "MaxCapacity"]).as_i64(),
                Some(max),
                "case {case:?}: wrong automatic scaling MaxCapacity, got {got}"
            );
            assert!(
                at(got, &["AutoScalingPolicy", "Rules"])
                    .as_array()
                    .is_some_and(|rules| rules.len() == 1),
                "case {case:?}: the submitted scaling rule must be reported, got {got}"
            );
        }
        None => assert!(
            got["AutoScalingPolicy"].is_null(),
            "case {case:?}: a group configured with no scaling policy reports none, got {got}"
        ),
    }

    // Nothing has reconfigured or resized this group, so it carries no configuration
    // version and no shrink policy.
    assert!(
        got["ConfigurationsVersion"].is_null() && got["ShrinkPolicy"].is_null(),
        "case {case:?}: an unmodified group carries no version and no shrink policy, got {got}"
    );
}

/// A `RunJobFlow` request whose `Instances` member is `instances`.
fn launch(name: &str, instances: Value) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": instances,
    })
}

/// The three-role `InstanceGroups` list case 4 launches with, and case 2 of the clock
/// test reads back.
fn configured_groups() -> Value {
    json!([
        { "Name": "primary", "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
        {
            "InstanceRole": "CORE",
            "InstanceType": "r5.xlarge",
            "InstanceCount": 2,
            "Market": "SPOT",
            "BidPrice": "0.10",
            "Configurations": [{
                "Classification": "spark-defaults",
                "Properties": { "spark.executor.memory": "2g" },
            }],
        },
        {
            "Name": "workers",
            "InstanceRole": "TASK",
            "InstanceType": "c5.xlarge",
            "InstanceCount": 3,
            "CustomAmiId": "ami-0123456789abcdef0",
            "EbsConfiguration": {
                "EbsOptimized": true,
                "EbsBlockDeviceConfigs": [
                    {
                        "VolumeSpecification": {
                            "VolumeType": "gp3",
                            "SizeInGB": 100,
                            "Iops": 3000,
                            "Throughput": 125,
                        },
                        "VolumesPerInstance": 2,
                    },
                    { "VolumeSpecification": { "VolumeType": "gp2", "SizeInGB": 32 } },
                ],
            },
            "AutoScalingPolicy": {
                "Constraints": { "MinCapacity": 1, "MaxCapacity": 10 },
                "Rules": [{
                    "Name": "scale-out",
                    "Action": {
                        "SimpleScalingPolicyConfiguration": {
                            "AdjustmentType": "CHANGE_IN_CAPACITY",
                            "ScalingAdjustment": 1,
                            "CoolDown": 300,
                        },
                    },
                    "Trigger": {
                        "CloudWatchAlarmDefinition": {
                            "ComparisonOperator": "LESS_THAN",
                            "MetricName": "YARNMemoryAvailablePercentage",
                            "Period": 300,
                            "Threshold": 15.0,
                            "EvaluationPeriods": 1,
                            "Statistic": "AVERAGE",
                            "Unit": "PERCENT",
                        },
                    },
                }],
            },
        },
    ])
}

/// The three groups [`configured_groups`] asks for, as they must be reported back.
fn configured_expectations() -> Vec<Expected> {
    vec![
        Expected {
            name: Some("primary"),
            ..Expected::group("MASTER", "m5.xlarge", 1)
        },
        Expected {
            market: "SPOT",
            bid_price: Some("0.10"),
            configuration_classifications: &["spark-defaults"],
            ..Expected::group("CORE", "r5.xlarge", 2)
        },
        Expected {
            name: Some("workers"),
            custom_ami_id: Some("ami-0123456789abcdef0"),
            // `/dev/sda` is the root volume, so the requested volumes start at
            // `/dev/sdb`: two of the first configuration, then one of the second.
            ebs_devices: &[
                ("/dev/sdb", "gp3"),
                ("/dev/sdc", "gp3"),
                ("/dev/sdd", "gp2"),
            ],
            ebs_optimized: Some(true),
            auto_scaling: Some((1, 10)),
            ..Expected::group("TASK", "c5.xlarge", 3)
        },
    ]
}

// ---------------------------------------------------------------------------
// RunJobFlow -> ListInstanceGroups
// ---------------------------------------------------------------------------

/// THE RULE above, over the `Instances` forms `RunJobFlow` accepts: an explicit
/// `InstanceGroups` list of every role, the uniform node-type form at one node, at
/// several nodes and with no core node type named, and an instance-fleet cluster, which
/// has no groups at all.
#[tokio::test]
async fn list_instance_groups_reports_every_group_its_cluster_was_launched_with() {
    struct Case {
        label: &'static str,
        instances: Value,
        expected: Vec<Expected>,
    }

    let emr = Emr::new();
    let cases = vec![
        Case {
            label: "uniform form, a single node",
            instances: json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            // "The number of Amazon EC2 instances in the cluster" is one, and that one
            // is the master.
            expected: vec![Expected::group("MASTER", "m5.xlarge", 1)],
        },
        Case {
            label: "uniform form, three nodes",
            instances: json!({
                "InstanceCount": 3,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.2xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            }),
            // One instance is the master; the core and task nodes hold the rest.
            expected: vec![
                Expected::group("MASTER", "m5.xlarge", 1),
                Expected::group("CORE", "m5.2xlarge", 2),
            ],
        },
        Case {
            label: "uniform form, no core node instance type named",
            instances: json!({ "InstanceCount": 2, "MasterInstanceType": "m5.4xlarge" }),
            // Nothing in the request says what to provision the core nodes as, so the
            // cluster carries only the master group.
            expected: vec![Expected::group("MASTER", "m5.4xlarge", 1)],
        },
        Case {
            label: "configured groups, all three roles",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": configured_groups(),
            }),
            expected: configured_expectations(),
        },
        Case {
            label: "instance fleets, which carry no instance groups",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                }],
            }),
            expected: Vec::new(),
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let label = case.label;
        let cluster_id = emr
            .run_job_flow(launch(label, case.instances.clone()))
            .await;
        let groups = list_all_groups(&emr, label, &cluster_id).await;
        assert_eq!(
            groups.len(),
            case.expected.len(),
            "case {label:?}: expected {} groups, got {groups:?}",
            case.expected.len()
        );
        for (expected, got) in case.expected.iter().zip(&groups) {
            assert_group(label, expected, got);

            // A group EMR has just accepted is not up: it is acquiring its instances, so
            // none of them is running yet and it has no ready instant.
            assert_eq!(
                at(got, &["Status", "State"]).as_str(),
                Some("PROVISIONING"),
                "case {label:?}: a new group must be observable as PROVISIONING, got {got}"
            );
            assert_eq!(
                got["RunningInstanceCount"].as_i64(),
                Some(0),
                "case {label:?}: a provisioning group runs no instances, got {got}"
            );
            assert!(
                at(got, &["Status", "Timeline", "CreationDateTime"])
                    .as_f64()
                    .is_some(),
                "case {label:?}: the group's creation instant must be recorded, got {got}"
            );
            assert!(
                at(got, &["Status", "Timeline", "ReadyDateTime"]).is_null()
                    && at(got, &["Status", "Timeline", "EndDateTime"]).is_null(),
                "case {label:?}: a provisioning group is neither ready nor ended, got {got}"
            );
            assert!(
                at(got, &["Status", "StateChangeReason"]).is_null(),
                "case {label:?}: nothing has changed this group's state, got {got}"
            );

            // Every minted group id is unique service-wide.
            let id = got["Id"].as_str().unwrap_or_default().to_string();
            assert!(
                minted.insert(id.clone()),
                "case {label:?}: group id {id:?} was minted twice"
            );
        }
    }
}

/// THE RULE: a group AWS answers in a non-terminal state reaches its next state once the
/// clock makes the transition due — `PROVISIONING` becomes `RUNNING`, its requested
/// instances are then the running ones, and the instant is recorded as
/// `Timeline.ReadyDateTime` — and a read never moves it anywhere else. A policy attached
/// to a group follows it: `ATTACHING` while the group is coming up, `ATTACHED` once it is.
#[tokio::test]
async fn a_provisioning_instance_group_runs_when_the_clock_makes_it_due() {
    let emr = Emr::new();
    let cases = [
        (
            "uniform form",
            json!({
                "InstanceCount": 4,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            }),
        ),
        (
            "configured groups",
            json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": configured_groups(),
            }),
        ),
    ];

    let mut ids = Vec::new();
    for (label, instances) in &cases {
        let cluster_id = emr.run_job_flow(launch(label, instances.clone())).await;
        for group in list_all_groups(&emr, label, &cluster_id).await {
            assert_eq!(
                at(&group, &["Status", "State"]).as_str(),
                Some("PROVISIONING"),
                "case {label:?}: the transient state must be observable, got {group}"
            );
            if !group["AutoScalingPolicy"].is_null() {
                assert_eq!(
                    at(&group, &["AutoScalingPolicy", "Status", "State"]).as_str(),
                    Some("ATTACHING"),
                    "case {label:?}: a policy on a provisioning group is ATTACHING, got {group}"
                );
            }
        }
        ids.push(cluster_id);
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    for ((label, _), cluster_id) in cases.iter().zip(&ids) {
        let groups = list_all_groups(&emr, label, cluster_id).await;
        assert!(
            !groups.is_empty(),
            "case {label:?}: the cluster must still report its groups, got {groups:?}"
        );
        for group in &groups {
            assert_eq!(
                at(group, &["Status", "State"]).as_str(),
                Some("RUNNING"),
                "case {label:?}: the group must reach RUNNING, got {group}"
            );
            assert_eq!(
                group["RunningInstanceCount"].as_i64(),
                group["RequestedInstanceCount"].as_i64(),
                "case {label:?}: a running group runs the instances it requested, got {group}"
            );
            let ready = at(group, &["Status", "Timeline", "ReadyDateTime"]).as_f64();
            let created = at(group, &["Status", "Timeline", "CreationDateTime"]).as_f64();
            assert!(
                ready.is_some() && ready >= created,
                "case {label:?}: the ready instant must follow the creation one, got {group}"
            );
            if !group["AutoScalingPolicy"].is_null() {
                assert_eq!(
                    at(group, &["AutoScalingPolicy", "Status", "State"]).as_str(),
                    Some("ATTACHED"),
                    "case {label:?}: a policy on a running group is ATTACHED, got {group}"
                );
            }
        }

        // Reading the listing a second time reports exactly the same groups: a read
        // moves nothing.
        let again = list_all_groups(&emr, label, cluster_id).await;
        assert_eq!(
            again, groups,
            "case {label:?}: a second read must report the same groups"
        );
    }
}

/// THE RULE: a group does not outlive its cluster — once the cluster the clock owed a
/// termination to has reached `TERMINATED`, its groups have `ENDED` for the modelled
/// reason `CLUSTER_TERMINATED`, run nothing, and carry the instant they ended.
#[tokio::test]
async fn instance_groups_end_when_their_cluster_is_terminated() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("doomed")).await;
    let before = list_all_groups(&emr, "terminated cluster", &cluster_id).await;
    assert_eq!(
        before.len(),
        2,
        "case \"terminated cluster\": the launch implies a master and a core group, got {before:?}"
    );

    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [&cluster_id] }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let after = list_all_groups(&emr, "terminated cluster", &cluster_id).await;
    let ids_before: Vec<&str> = before
        .iter()
        .map(|group| group["Id"].as_str().unwrap_or_default())
        .collect();
    let ids_after: Vec<&str> = after
        .iter()
        .map(|group| group["Id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids_after, ids_before,
        "case \"terminated cluster\": the same groups must still be reported"
    );
    for group in &after {
        assert_eq!(
            at(group, &["Status", "State"]).as_str(),
            Some("ENDED"),
            "case \"terminated cluster\": the group must have ENDED, got {group}"
        );
        assert_eq!(
            at(group, &["Status", "StateChangeReason", "Code"]).as_str(),
            Some("CLUSTER_TERMINATED"),
            "case \"terminated cluster\": wrong state change reason, got {group}"
        );
        assert_eq!(
            group["RunningInstanceCount"].as_i64(),
            Some(0),
            "case \"terminated cluster\": an ended group runs nothing, got {group}"
        );
        assert!(
            at(group, &["Status", "Timeline", "EndDateTime"])
                .as_f64()
                .is_some(),
            "case \"terminated cluster\": the end instant must be recorded, got {group}"
        );
    }
}

/// THE RULE's pagination half: a cluster holding more groups than one page reports
/// answers the first [`PAGE_SIZE`] of them plus the `Marker` of the first group it did
/// not report, and that marker resumes the listing exactly there — so following it walks
/// every group of the cluster, in creation order, once.
#[tokio::test]
async fn list_instance_groups_pages_on_marker_and_refuses_one_no_group_carries() {
    let emr = Emr::new();
    // One master group plus enough task groups to need a second page.
    let overflow = 3;
    let total = PAGE_SIZE + overflow;
    let mut configs = vec![json!({
        "InstanceRole": "MASTER",
        "InstanceType": "m5.xlarge",
        "InstanceCount": 1,
    })];
    for index in 1..total {
        configs.push(json!({
            "Name": format!("task-{index}"),
            "InstanceRole": "TASK",
            "InstanceType": "m5.xlarge",
            "InstanceCount": 1,
        }));
    }
    let cluster_id = emr
        .run_job_flow(launch(
            "paged",
            json!({ "KeepJobFlowAliveWhenNoSteps": true, "InstanceGroups": configs }),
        ))
        .await;

    let first = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": &cluster_id }))
        .await;
    assert_eq!(
        first["InstanceGroups"].as_array().map(Vec::len),
        Some(PAGE_SIZE),
        "case \"first page\": a full page holds {PAGE_SIZE} groups, got {first}"
    );
    let marker = first["Marker"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"first page\": a short listing must carry a Marker, got {first}"))
        .to_string();

    let second = emr
        .ok(
            "ListInstanceGroups",
            json!({ "ClusterId": &cluster_id, "Marker": &marker }),
        )
        .await;
    assert_eq!(
        second["InstanceGroups"].as_array().map(Vec::len),
        Some(overflow),
        "case \"second page\": the rest of the groups must follow, got {second}"
    );
    assert!(
        second["Marker"].is_null(),
        "case \"second page\": the last page carries no Marker, got {second}"
    );
    assert_eq!(
        second["InstanceGroups"][0]["Id"].as_str(),
        Some(marker.as_str()),
        "case \"second page\": the marker names the group the page resumes at, got {second}"
    );

    // Following the marker walks every group exactly once, in creation order — the task
    // groups' own names are what pin that order.
    let walked = list_all_groups(&emr, "paged", &cluster_id).await;
    assert_eq!(
        walked.len(),
        total,
        "case \"paged\": every group must be walked once, got {} of {total}",
        walked.len()
    );
    let names: Vec<Option<&str>> = walked
        .iter()
        .map(|group| group["Name"].as_str())
        .collect();
    let mut expected_names: Vec<Option<&str>> = vec![None];
    let owned: Vec<String> = (1..total).map(|index| format!("task-{index}")).collect();
    expected_names.extend(owned.iter().map(|name| Some(name.as_str())));
    assert_eq!(
        names, expected_names,
        "case \"paged\": the groups must be reported in creation order"
    );

    // A marker no group of this cluster carries is refused, not silently restarted.
    for marker in [
        "ig-ZZZZZZZZZZZZZ",
        "ig-",
        "",
        "not-a-group-id",
        cluster_id.as_str(),
    ] {
        let (status, payload) = emr
            .call(
                "ListInstanceGroups",
                json!({ "ClusterId": &cluster_id, "Marker": marker }),
            )
            .await;
        assert_refused(
            &format!("marker {marker:?}"),
            status,
            &payload,
            "InvalidRequestException",
        );
    }
}

/// THE RULE's refusal half: a cluster id the service holds no cluster for is refused with
/// the modelled 400 `InvalidRequestException` rather than answered with an empty listing,
/// and a launch past the bounds the group store takes is refused too.
#[tokio::test]
async fn list_instance_groups_refuses_an_unknown_cluster() {
    let emr = Emr::new();
    // A cluster that DOES exist, so the refusals below are about the id and not about an
    // empty store.
    let cluster_id = emr.run_job_flow(kept_alive_request("live")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        let (status, payload) = emr
            .call("ListInstanceGroups", json!({ "ClusterId": unknown }))
            .await;
        assert_refused(
            &format!("cluster id {unknown:?}"),
            status,
            &payload,
            "InvalidRequestException",
        );
    }

    // The live cluster is untouched by the refusals.
    let groups = list_all_groups(&emr, "live", &cluster_id).await;
    assert_eq!(
        groups.len(),
        2,
        "case \"live\": a refused listing must leave the cluster's groups, got {groups:?}"
    );

    // A launch asking for more groups than a cluster carries is refused.
    let too_many: Vec<Value> = (0..=MAX_INSTANCE_GROUPS_PER_CLUSTER)
        .map(|_| {
            json!({
                "InstanceRole": "TASK",
                "InstanceType": "m5.xlarge",
                "InstanceCount": 1,
            })
        })
        .collect();
    let (status, payload) = emr
        .call(
            "RunJobFlow",
            launch(
                "too many groups",
                json!({ "KeepJobFlowAliveWhenNoSteps": true, "InstanceGroups": too_many }),
            ),
        )
        .await;
    assert_refused(
        "a launch past the instance group maximum",
        status,
        &payload,
        "ValidationException",
    );

    // So is one asking for more EBS volumes than an instance carries, more block device
    // configurations than a group names, or a negative count; the bounds themselves are
    // accepted. Each row states the volumes per configuration, so the total is the sum.
    for (label, volumes_per_config, accepted) in [
        ("one configuration at the volume bound", vec![MAX_EBS_VOLUMES_PER_INSTANCE], true),
        ("two configurations summing to the bound", vec![20, 5], true),
        ("configuration count at the bound", vec![1; MAX_EBS_BLOCK_DEVICE_CONFIGS], true),
        ("one configuration past the volume bound", vec![MAX_EBS_VOLUMES_PER_INSTANCE + 1], false),
        ("two configurations summing past the bound", vec![20, 6], false),
        ("far past the volume bound", vec![10_000], false),
        (
            "one configuration past the count bound",
            vec![1; MAX_EBS_BLOCK_DEVICE_CONFIGS + 1],
            false,
        ),
        ("a negative volume count", vec![-1], false),
    ] {
        let expected_devices: i64 = volumes_per_config.iter().sum();
        let configs: Vec<Value> = volumes_per_config
            .iter()
            .map(|volumes| {
                json!({
                    "VolumeSpecification": { "VolumeType": "gp2", "SizeInGB": 32 },
                    "VolumesPerInstance": volumes,
                })
            })
            .collect();
        let request = launch(
            label,
            json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                // The primary group every launch states its capacity on, carrying the
                // volume configuration this row is about.
                "InstanceGroups": [{
                    "InstanceRole": "MASTER",
                    "InstanceType": "m5.xlarge",
                    "InstanceCount": 1,
                    "EbsConfiguration": { "EbsBlockDeviceConfigs": configs },
                }],
            }),
        );
        let (status, payload) = emr.call("RunJobFlow", request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: the bound itself must be accepted, got {status} with {payload}"
            );
            let launched = payload["JobFlowId"].as_str().unwrap_or_default().to_string();
            let groups = list_all_groups(&emr, label, &launched).await;
            assert_eq!(
                groups[0]["EbsBlockDevices"].as_array().map(Vec::len),
                Some(expected_devices as usize),
                "case {label:?}: every requested volume must be mapped, got {groups:?}"
            );
        } else {
            assert_refused(label, status, &payload, "ValidationException");
        }
    }
}

/// THE RULE: launching a cluster CHANGES this service's state, so `RunJobFlow` leaves
/// exactly ONE structured audit record — on its refusal branches too — naming the
/// operation, the resource (the minted cluster id when one was minted, else the name the
/// request asked for), the presented access key id, the request target and the outcome,
/// and carrying no part of the request's own configuration. `ListInstanceGroups` only
/// reports, so it leaves none.
#[tokio::test]
async fn launching_a_cluster_emits_one_audit_record_and_listing_its_groups_emits_none() {
    let capture = audit_capture();
    let emr = Emr::with_akid(AUDIT_AKID);

    // Harness self-test, so a failure below can never be mistaken for a missing record.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = capture.drain_audit_records(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "the capture harness must see an audit record emitted on this thread, got {probe:?}"
    );

    // A value that must never reach the log, carried in the part of the request that
    // describes the cluster rather than names it.
    let secret = "not-in-the-log";
    let configured = json!({
        "KeepJobFlowAliveWhenNoSteps": true,
        "InstanceGroups": [{
            "Name": secret,
            "InstanceRole": "MASTER",
            "InstanceType": "m5.xlarge",
            "InstanceCount": 1,
            "Configurations": [{
                "Classification": "spark-defaults",
                "Properties": { "spark.executor.extraJavaOptions": secret },
            }],
        }],
    });
    let too_many_groups: Vec<Value> = (0..=MAX_INSTANCE_GROUPS_PER_CLUSTER)
        .map(|_| json!({ "InstanceRole": "TASK", "InstanceType": "m5.xlarge", "InstanceCount": 1 }))
        .collect();

    // (label, the cluster name the request carries, its Instances member, the outcome it
    // must record). `None` for the expected resource means "the id the launch minted".
    let audited = [
        (
            "a launch that persists a cluster",
            "audited-launch",
            configured.clone(),
            "cluster-launched",
            None,
        ),
        (
            "a launch past the instance group maximum",
            "refused-bound",
            json!({ "KeepJobFlowAliveWhenNoSteps": true, "InstanceGroups": too_many_groups }),
            "validation-refused",
            Some("refused-bound"),
        ),
        (
            "a launch carrying groups and fleets at once",
            "refused-both",
            json!({
                "InstanceGroups": [{
                    "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1,
                }],
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                }],
            }),
            "validation-refused",
            Some("refused-both"),
        ),
        (
            "a launch whose group asks for too many EBS volumes",
            "refused-volumes",
            json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [{
                    "InstanceRole": "CORE",
                    "InstanceType": "m5.xlarge",
                    "InstanceCount": 1,
                    "EbsConfiguration": {
                        "EbsBlockDeviceConfigs": [{
                            "VolumeSpecification": { "VolumeType": "gp2", "SizeInGB": 32 },
                            "VolumesPerInstance": MAX_EBS_VOLUMES_PER_INSTANCE + 1,
                        }],
                    },
                }],
            }),
            "validation-refused",
            Some("refused-volumes"),
        ),
    ];

    let mut launched = String::new();
    for (label, name, instances, outcome, expected_resource) in audited {
        let (_, payload) = emr
            .call("RunJobFlow", launch(name, instances.clone()))
            .await;
        let records = capture.drain_audit_records(AUDIT_AKID);
        assert_eq!(
            records.len(),
            1,
            "case {label:?}: expected exactly one audit record, captured {records:?}"
        );
        let record = &records[0];
        let resource = match expected_resource {
            Some(resource) => resource.to_string(),
            None => {
                launched = payload["JobFlowId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: no JobFlowId in {payload}"))
                    .to_string();
                launched.clone()
            }
        };
        for (field, expected) in [
            ("operation", "RunJobFlow"),
            ("resource", resource.as_str()),
            ("outcome", outcome),
            ("access_key_id", AUDIT_AKID),
            ("request_target", REQUEST_TARGET),
        ] {
            assert_eq!(
                record.field(field),
                Some(expected),
                "case {label:?}: expected {field}={expected:?}, got {record:?}"
            );
        }
        // Metadata only: no part of the submitted cluster configuration is logged.
        let rendered = format!("{record:?}");
        assert!(
            !rendered.contains(secret),
            "case {label:?}: the record must carry no part of the configuration, got {rendered}"
        );
    }

    // Reporting calls record nothing: there is no state change to attribute.
    let groups = list_all_groups(&emr, "audited-launch", &launched).await;
    assert_eq!(
        groups.len(),
        1,
        "case \"audited-launch\": the launched cluster must report its group, got {groups:?}"
    );
    let (status, _) = emr
        .call("ListInstanceGroups", json!({ "ClusterId": "j-NOSUCHCLUSTER" }))
        .await;
    assert_eq!(status, 400, "an unknown cluster id must still be refused");
    let (status, _) = emr
        .call(
            "ListInstanceGroups",
            json!({ "ClusterId": &launched, "Marker": "ig-NOSUCHGROUP" }),
        )
        .await;
    assert_eq!(status, 400, "an unknown marker must still be refused");
    let leaked = capture.drain_audit_records(AUDIT_AKID);
    assert!(
        leaked.is_empty(),
        "case \"reporting calls\": a reporting operation must emit no audit record, \
         captured {leaked:?}"
    );
}
