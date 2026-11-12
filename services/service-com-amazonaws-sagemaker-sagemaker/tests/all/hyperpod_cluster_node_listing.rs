// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod's node listing: `BatchAddClusterNodes`
//! adds nodes to a cluster and `ListClusterNodes` reports them back, filtered, sorted and
//! paged, with `DescribeClusterNode` as the per-node reader each listing is checked
//! against.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * every node a cluster holds is reported exactly once, with the identity, group,
//!   type, status and launch time the per-node reader reports for it;
//! * a group's nodes are the instances it was created asking for, as many as the
//!   cluster's own realised count reports;
//! * `IncludeNodeLogicalIds` decides which nodes are reported (only those with an
//!   assigned `InstanceId` when it is absent or false), and every summary carries the
//!   node's `NodeLogicalId` whichever listing reported it;
//! * `InstanceGroupNameContains` and the `CreationTimeAfter`/`CreationTimeBefore` window
//!   select by substring and by inclusive bounds, combining by intersection;
//! * results follow the requested `SortBy`/`SortOrder`, defaulting to `CREATION_TIME`
//!   ascending, and a value outside either enum is refused;
//! * paging with `MaxResults` walks the whole ordered result exactly once;
//! * listing is a reporting call: beyond the status transition the clock already owed, a
//!   second identical listing answers the same;
//! * a cluster this account does not hold is refused with the modelled
//!   `ResourceNotFound`, whether it was named by name or by ARN.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// Longer than `state::TRANSIENT_STATE_SETTLES_AFTER` (300 ms), so the clock owes every
/// added node its `Pending` -> `Running` transition by the time the next call runs.
const PAST_SETTLE_DUE: Duration = Duration::from_millis(600);

/// Spacing between batches, so `LaunchTime` ordering is unambiguous and a filter bound
/// can be placed strictly between two batches.
const BATCH_SPACING: Duration = Duration::from_millis(5);

/// An instance group of a HyperPod cluster. `InstanceCount`, `InstanceGroupName` and
/// `ExecutionRole` are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification); the types come from `ClusterInstanceType`
/// (smithy_query shape=ClusterInstanceType).
fn instance_group(name: &str, instance_type: &str) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": 0,
        "InstanceType": instance_type,
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// Create a cluster holding one instance group per `(name, instance type)` row. Answers
/// its `ClusterArn`.
async fn create_cluster(emulator: &Emulator, name: &str, groups: &[(&str, &str)]) -> String {
    let instance_groups: Vec<Value> = groups
        .iter()
        .map(|(group, instance_type)| instance_group(group, instance_type))
        .collect();
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({ "ClusterName": name, "InstanceGroups": instance_groups }),
        )
        .await;
    str_field(&created, "ClusterArn")
}

/// Add `count` nodes to `group` of `cluster` through `BatchAddClusterNodes`. Answers the
/// `NodeLogicalId`s that call minted, in the order it answered them.
async fn add_nodes(emulator: &Emulator, cluster: &str, group: &str, count: i64) -> Vec<String> {
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{
                    "InstanceGroupName": group,
                    "IncrementTargetCountBy": count,
                }],
            }),
        )
        .await;
    let successful = added
        .get("Successful")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case add {count} to {group}: expected Successful in {added}"))
        .clone();
    assert_eq!(
        successful.len(),
        usize::try_from(count).expect("a positive node count"),
        "case add {count} to {group}: the add must answer one entry per node, got {added}"
    );
    successful
        .iter()
        .map(|entry| str_field(entry, "NodeLogicalId"))
        .collect()
}

/// The summaries a `ListClusterNodes` response carries.
fn summaries(listed: &Value) -> Vec<Value> {
    listed
        .get("ClusterNodeSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterNodeSummaries in {listed}"))
        .clone()
}

/// The `NodeLogicalId`s a `ListClusterNodes` response carries, in the order it carried
/// them. Every summary must carry one, which is what `IncludeNodeLogicalIds` asks for.
fn listed_logical_ids(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| str_field(summary, "NodeLogicalId"))
        .collect()
}

/// `body` with `IncludeNodeLogicalIds` set, so a listing is addressed by the identifier
/// every node has for its whole lifecycle.
fn with_logical_ids(body: &Value) -> Value {
    let mut body = body.clone();
    body["IncludeNodeLogicalIds"] = Value::Bool(true);
    body
}

/// Every node of `cluster`, listed by walking `NextToken` to the end so the assertion is
/// about the whole result rather than one page.
async fn all_nodes(emulator: &Emulator, body: &Value) -> Vec<Value> {
    let mut body = body.clone();
    let mut seen = Vec::new();
    loop {
        let listed = emulator.ok("ListClusterNodes", &body).await;
        seen.extend(summaries(&listed));
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => body["NextToken"] = Value::String(next.to_string()),
            None => break,
        }
        assert!(
            seen.len() < 1_000,
            "paging did not terminate, saw {} summaries",
            seen.len()
        );
    }
    seen
}

/// The summary of the node `logical_id` names, from a listing that carries logical ids.
fn summary_of(case: &str, listed: &[Value], logical_id: &str) -> Value {
    let matches: Vec<&Value> = listed
        .iter()
        .filter(|summary| str_field(summary, "NodeLogicalId") == logical_id)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "case {case:?}: node {logical_id:?} must be listed exactly once, got {matches:?}"
    );
    matches[0].clone()
}

/// An `InstanceId` in the modelled form: `ClusterNodeId` is pattern
/// `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$` (smithy_query operation=DescribeClusterNode).
fn assert_instance_id(case: &str, instance_id: &str) {
    let digits = instance_id.strip_prefix("i-").unwrap_or_else(|| {
        panic!("case {case:?}: expected an InstanceId starting \"i-\", got {instance_id:?}")
    });
    assert!(
        (digits.len() == 8 || digits.len() == 17)
            && digits.chars().all(|glyph| glyph.is_ascii_hexdigit()),
        "case {case:?}: an InstanceId's tail must match [a-f0-9]{{8}} or [a-f0-9]{{17}}, got \
         {digits:?}"
    );
}

/// One row of the provisioning table below: the case name, the `(instance group, instance
/// type, declared instance count)` rows the cluster is created with, and the
/// `(instance group, node count)` batches added to it afterwards.
type ProvisioningRow<'a> = (&'a str, Vec<(&'a str, &'a str, i64)>, Vec<(&'a str, i64)>);

/// Create a cluster whose groups declare an instance count of their own. Answers its
/// `ClusterArn`.
async fn create_cluster_with_counts(
    emulator: &Emulator,
    name: &str,
    groups: &[(&str, &str, i64)],
) -> String {
    let instance_groups: Vec<Value> = groups
        .iter()
        .map(|(group, instance_type, instance_count)| {
            let mut spec = instance_group(group, instance_type);
            spec["InstanceCount"] = json!(instance_count);
            spec
        })
        .collect();
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({ "ClusterName": name, "InstanceGroups": instance_groups }),
        )
        .await;
    str_field(&created, "ClusterArn")
}

/// RULE: a cluster's nodes are the instances its groups were created asking for —
/// `InstanceCount` is "the number of instances to add to the instance group" — so once
/// the cluster is provisioned each group holds that many `Running` nodes, which is the
/// same count `DescribeCluster` reports realised in that group's `CurrentCount`; nodes a
/// call added to a group raised its count, so they are part of it rather than minted a
/// second time.
#[tokio::test]
async fn the_instances_a_group_was_created_asking_for_are_the_nodes_it_holds() {
    // Rows differ along what the rule generalises over: the count each group declares
    // (none, one, several), the number of groups, and whether a batch raised a group's
    // count after the create.
    let rows: Vec<ProvisioningRow> = vec![
        ("no-instances", vec![("idle", "ml.c5.large", 0)], vec![]),
        ("one-instance", vec![("solo", "ml.c5.large", 1)], vec![]),
        ("many-instances", vec![("pool", "ml.m5.large", 4)], vec![]),
        (
            "two-groups-different-counts",
            vec![
                ("controller", "ml.c5.large", 1),
                ("workers", "ml.g5.xlarge", 3),
            ],
            vec![],
        ),
        (
            "declared-and-added",
            vec![("pool", "ml.m5.large", 2), ("idle", "ml.t3.medium", 0)],
            vec![("pool", 2)],
        ),
    ];

    for (case, groups, adds) in rows {
        let emulator = Emulator::new();
        create_cluster_with_counts(&emulator, case, &groups).await;
        for (group, count) in &adds {
            add_nodes(&emulator, case, group, *count).await;
        }
        tokio::time::sleep(PAST_SETTLE_DUE).await;

        let listed = all_nodes(&emulator, &with_logical_ids(&json!({ "ClusterName": case }))).await;
        let described = emulator
            .ok("DescribeCluster", &json!({ "ClusterName": case }))
            .await;
        let details = described
            .get("InstanceGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected InstanceGroups in {described}"))
            .clone();

        for (group, instance_type, declared) in &groups {
            let added: i64 = adds
                .iter()
                .filter(|(name, _)| name == group)
                .map(|(_, count)| *count)
                .sum();
            let expected = declared + added;
            let held: Vec<&Value> = listed
                .iter()
                .filter(|summary| str_field(summary, "InstanceGroupName") == *group)
                .collect();
            assert_eq!(
                i64::try_from(held.len()).expect("a small node count"),
                expected,
                "case {case:?}: group {group:?} holds the instances it asked for, got {listed:?}"
            );
            for summary in &held {
                assert_eq!(
                    str_field(summary, "InstanceType"),
                    *instance_type,
                    "case {case:?}: an instance of {group:?} runs its group's type, got \
                     {summary:?}"
                );
                assert_eq!(
                    str_field(&summary["InstanceStatus"], "Status"),
                    "Running",
                    "case {case:?}: an instance of a provisioned group is Running, got {summary:?}"
                );
                assert_instance_id(case, &str_field(summary, "InstanceId"));
            }

            // The listing and the cluster's own realised count cannot disagree.
            let group_details = details
                .iter()
                .find(|detail| str_field(detail, "InstanceGroupName") == *group)
                .unwrap_or_else(|| panic!("case {case:?}: group {group:?} must be described"));
            assert_eq!(
                group_details.get("CurrentCount").and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: group {group:?} must report one realised count to both readers, \
                 got {group_details:?} against {} listed",
                held.len()
            );
        }
    }
}

/// One row of the identity table below: the case name, the `(instance group, instance
/// type)` rows the cluster is created with, and the `(instance group, node count)`
/// batches added to it.
type IdentityRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<(&'a str, i64)>);

/// RULE: `ListClusterNodes` reports every node the cluster holds exactly once, each
/// summary carrying the identity and placement the cluster stored for that node — the
/// group it was added to, that group's instance type, the id its `Pending` -> `Running`
/// transition minted, its launch time and its current status — so a listing and the
/// per-node reader never disagree.
#[tokio::test]
async fn every_node_of_a_cluster_is_listed_once_with_the_identity_its_reader_reports() {
    // Rows differ along what the rule generalises over: the number of groups, the number
    // of nodes added to each, the instance types, and whether a group was left empty.
    let rows: Vec<IdentityRow> = vec![
        (
            "one-group-one-node",
            vec![("solo", "ml.c5.large")],
            vec![("solo", 1)],
        ),
        (
            "one-group-many-nodes",
            vec![("pool", "ml.m5.xlarge")],
            vec![("pool", 5)],
        ),
        (
            "two-groups-one-empty",
            vec![("filled", "ml.t3.medium"), ("empty", "ml.c5.xlarge")],
            vec![("filled", 2)],
        ),
        (
            "three-groups-repeated-adds",
            vec![
                ("controller", "ml.c5.large"),
                ("workers", "ml.g5.xlarge"),
                ("storage", "ml.i3en.large"),
            ],
            vec![("controller", 1), ("workers", 3), ("storage", 1), ("workers", 2)],
        ),
    ];

    for (case, groups, adds) in rows {
        let emulator = Emulator::new();
        create_cluster(&emulator, case, &groups).await;

        // The group each added node belongs to, by logical id.
        let mut expected: Vec<(String, String)> = Vec::new();
        for (group, count) in &adds {
            for logical_id in add_nodes(&emulator, case, group, *count).await {
                expected.push((logical_id, (*group).to_string()));
            }
        }
        tokio::time::sleep(PAST_SETTLE_DUE).await;

        let listed = all_nodes(&emulator, &with_logical_ids(&json!({ "ClusterName": case }))).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {case:?}: every added node must be listed once, got {listed:?}"
        );

        let mut instance_ids: BTreeSet<String> = BTreeSet::new();
        for (logical_id, group) in &expected {
            let summary = summary_of(case, &listed, logical_id);
            let instance_type = groups
                .iter()
                .find(|(name, _)| name == group)
                .map(|(_, instance_type)| *instance_type)
                .expect("the group under test is one the cluster was created with");
            assert_eq!(
                str_field(&summary, "InstanceGroupName"),
                *group,
                "case {case:?}: node {logical_id:?} belongs to {group:?}, got {summary:?}"
            );
            assert_eq!(
                str_field(&summary, "InstanceType"),
                instance_type,
                "case {case:?}: node {logical_id:?} runs its group's type, got {summary:?}"
            );
            assert_eq!(
                str_field(&summary["InstanceStatus"], "Status"),
                "Running",
                "case {case:?}: node {logical_id:?} is Running once the clock owes it, got \
                 {summary:?}"
            );
            let launch_time = summary
                .get("LaunchTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected a LaunchTime in {summary:?}"));
            assert!(
                launch_time > 0.0,
                "case {case:?}: node {logical_id:?} must carry its launch time, got {launch_time}"
            );

            let instance_id = str_field(&summary, "InstanceId");
            assert_instance_id(case, &instance_id);
            assert!(
                instance_ids.insert(instance_id.clone()),
                "case {case:?}: every node gets its own InstanceId, but {instance_id:?} repeated"
            );

            // The listing agrees with the per-node reader, member for member.
            let described = emulator
                .ok(
                    "DescribeClusterNode",
                    &json!({ "ClusterName": case, "NodeLogicalId": logical_id }),
                )
                .await;
            let details = described
                .get("NodeDetails")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected NodeDetails in {described}"));
            for member in ["InstanceId", "InstanceGroupName", "InstanceType"] {
                assert_eq!(
                    str_field(&summary, member),
                    str_field(&details, member),
                    "case {case:?}: node {logical_id:?} must report one {member} to both \
                     readers, got {summary:?} and {details:?}"
                );
            }
            assert_eq!(
                str_field(&summary["InstanceStatus"], "Status"),
                str_field(&details["InstanceStatus"], "Status"),
                "case {case:?}: node {logical_id:?} must report one status to both readers, got \
                 {summary:?} and {details:?}"
            );
        }
    }
}

/// RULE: `IncludeNodeLogicalIds` decides which nodes a listing reports — only those with
/// an assigned `InstanceId` when it is absent or false, every node whatever its
/// provisioning status when it is true — and every summary a listing reports carries the
/// node's own `NodeLogicalId`, whichever of the two listings reported it.
#[tokio::test]
async fn include_node_logical_ids_decides_which_nodes_are_reported() {
    // Rows differ along the number of nodes still provisioning, which is the axis the
    // flag selects on.
    for node_count in [1_i64, 2, 4] {
        let case = format!("nodes={node_count}");
        let emulator = Emulator::new();
        let name = format!("provisioning-{node_count}");
        create_cluster(&emulator, &name, &[("pool", "ml.c5.large")]).await;
        let added = add_nodes(&emulator, &name, "pool", node_count).await;

        // Straight after the add every node is still `Pending` with no `InstanceId`, so
        // the default listing reports none of them and the opt-in listing reports all.
        for (flag, body) in [
            ("absent", json!({ "ClusterName": name })),
            (
                "false",
                json!({ "ClusterName": name, "IncludeNodeLogicalIds": false }),
            ),
        ] {
            let listed = emulator.ok("ListClusterNodes", &body).await;
            assert!(
                summaries(&listed).is_empty(),
                "case {case} IncludeNodeLogicalIds={flag}: a node with no InstanceId must not be \
                 listed, got {listed:?}"
            );
        }

        let listed = emulator
            .ok(
                "ListClusterNodes",
                &json!({ "ClusterName": name, "IncludeNodeLogicalIds": true }),
            )
            .await;
        let mut reported = listed_logical_ids(&listed);
        reported.sort();
        let mut expected = added.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: IncludeNodeLogicalIds=true must report every node, got {listed:?}"
        );
        for summary in summaries(&listed) {
            assert_eq!(
                str_field(&summary["InstanceStatus"], "Status"),
                "Pending",
                "case {case}: a node that has not reached Running is Pending, got {summary:?}"
            );
            let instance_id = summary
                .get("InstanceId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                instance_id.is_empty(),
                "case {case}: a node gets its InstanceId with the Running transition, got \
                 {summary:?}"
            );
        }

        // Once the clock owes the transition, both listings report every node, each
        // summary carrying the node's `NodeLogicalId`.
        tokio::time::sleep(PAST_SETTLE_DUE).await;
        for (flag, body) in [
            ("absent", json!({ "ClusterName": name })),
            (
                "true",
                json!({ "ClusterName": name, "IncludeNodeLogicalIds": true }),
            ),
        ] {
            let listed = emulator.ok("ListClusterNodes", &body).await;
            let reported = summaries(&listed);
            assert_eq!(
                reported.len(),
                added.len(),
                "case {case} IncludeNodeLogicalIds={flag}: every settled node must be listed, \
                 got {listed:?}"
            );
            for summary in &reported {
                assert!(
                    added.contains(&str_field(summary, "NodeLogicalId")),
                    "case {case} IncludeNodeLogicalIds={flag}: every summary carries the node's \
                     own NodeLogicalId, got {summary:?}"
                );
                assert_instance_id(&case, &str_field(summary, "InstanceId"));
            }
        }
    }
}

/// The cluster the filter, sort and page cases read: three groups, with nodes added in
/// four batches spaced in time. Answers the batches in creation order, each as its
/// `(instance group name, the logical ids it minted, their launch time)`.
async fn seed_nodes(emulator: &Emulator, cluster: &str) -> Vec<(String, Vec<String>, f64)> {
    create_cluster(
        emulator,
        cluster,
        &[
            ("alpha", "ml.c5.large"),
            ("beta-pool", "ml.m5.large"),
            ("gamma", "ml.t3.medium"),
        ],
    )
    .await;

    let batches: Vec<(&str, i64)> = vec![("alpha", 1), ("beta-pool", 2), ("gamma", 1), ("alpha", 1)];
    let mut added: Vec<(String, Vec<String>)> = Vec::new();
    for (group, count) in batches {
        added.push((
            group.to_string(),
            add_nodes(emulator, cluster, group, count).await,
        ));
        tokio::time::sleep(BATCH_SPACING).await;
    }
    tokio::time::sleep(PAST_SETTLE_DUE).await;

    // The launch times the service recorded, read back from the listing.
    let listed = all_nodes(emulator, &with_logical_ids(&json!({ "ClusterName": cluster }))).await;
    added
        .into_iter()
        .map(|(group, logical_ids)| {
            let launch_time = summary_of("seed", &listed, &logical_ids[0])
                .get("LaunchTime")
                .and_then(Value::as_f64)
                .expect("a seeded node is listed with a LaunchTime");
            (group, logical_ids, launch_time)
        })
        .collect()
}

/// The logical ids of the batches `which` selects, sorted — the set a filter must answer.
fn ids_of(batches: &[(String, Vec<String>, f64)], which: &[usize]) -> Vec<String> {
    let mut ids: Vec<String> = which
        .iter()
        .flat_map(|index| batches[*index].1.clone())
        .collect();
    ids.sort();
    ids
}

/// RULE: `InstanceGroupNameContains` keeps the nodes whose instance group's name contains
/// the substring, `CreationTimeAfter`/`CreationTimeBefore` keep those launched inside the
/// inclusive window, and the filters combine by intersection.
#[tokio::test]
async fn list_cluster_nodes_filters_by_instance_group_and_the_creation_window() {
    let emulator = Emulator::new();
    let cluster = "filtered";
    let batches = seed_nodes(&emulator, cluster).await;
    // Batch order: alpha, beta-pool, gamma, alpha. A bound placed strictly between two
    // batches splits them unambiguously, whatever precision the wire's epoch-seconds
    // double has.
    let first = batches[0].2;
    let last = batches[3].2;
    let split_after_first = (batches[0].2 + batches[1].2) / 2.0;
    let split_before_last = (batches[2].2 + batches[3].2) / 2.0;

    // Rows differ along the axis each filter generalises over: the substring (a prefix,
    // an infix, a suffix, a whole name, a hyphen, one that matches nothing) and the
    // window (open at each end, closed on both, and one that excludes everything).
    let cases: Vec<(&str, Value, Vec<usize>)> = vec![
        ("no-filter", json!({}), vec![0, 1, 2, 3]),
        (
            "whole-name",
            json!({ "InstanceGroupNameContains": "alpha" }),
            vec![0, 3],
        ),
        (
            "prefix",
            json!({ "InstanceGroupNameContains": "beta" }),
            vec![1],
        ),
        (
            "infix",
            json!({ "InstanceGroupNameContains": "a-p" }),
            vec![1],
        ),
        (
            "suffix",
            json!({ "InstanceGroupNameContains": "pool" }),
            vec![1],
        ),
        (
            "shared-letter",
            json!({ "InstanceGroupNameContains": "a" }),
            vec![0, 1, 2, 3],
        ),
        (
            "no-match",
            json!({ "InstanceGroupNameContains": "delta" }),
            vec![],
        ),
        (
            "after-the-whole-set",
            json!({ "CreationTimeAfter": first - 1.0 }),
            vec![0, 1, 2, 3],
        ),
        (
            "after-the-first-batch",
            json!({ "CreationTimeAfter": split_after_first }),
            vec![1, 2, 3],
        ),
        (
            "before-the-first-split",
            json!({ "CreationTimeBefore": split_after_first }),
            vec![0],
        ),
        (
            "window-around-the-middle",
            json!({
                "CreationTimeAfter": split_after_first,
                "CreationTimeBefore": split_before_last,
            }),
            vec![1, 2],
        ),
        (
            // The wire carries an epoch-seconds double, which rounds the stored instant,
            // so a bound copied out of a response is within a microsecond of a node's
            // launch time rather than equal to it: this row places the closed window a
            // second clear of each end instead.
            "closed-window-over-the-whole-set",
            json!({ "CreationTimeAfter": first - 1.0, "CreationTimeBefore": last + 1.0 }),
            vec![0, 1, 2, 3],
        ),
        (
            "empty-window",
            json!({ "CreationTimeAfter": last + 60.0, "CreationTimeBefore": last + 120.0 }),
            vec![],
        ),
        (
            "group-and-window-intersect",
            json!({
                "InstanceGroupNameContains": "alpha",
                "CreationTimeAfter": split_after_first,
            }),
            vec![3],
        ),
        (
            "group-and-window-intersect-to-nothing",
            json!({
                "InstanceGroupNameContains": "gamma",
                "CreationTimeBefore": split_after_first,
            }),
            vec![],
        ),
    ];

    for (case, mut body, which) in cases {
        body["ClusterName"] = Value::String(cluster.to_string());
        let listed = all_nodes(&emulator, &with_logical_ids(&body)).await;
        let mut reported: Vec<String> = listed
            .iter()
            .map(|summary| str_field(summary, "NodeLogicalId"))
            .collect();
        reported.sort();
        assert_eq!(
            reported,
            ids_of(&batches, &which),
            "case {case:?}: filtered nodes, got {listed:?}"
        );
    }
}

/// RULE: results are ordered by the requested `SortBy` in the requested `SortOrder`,
/// defaulting to `CREATION_TIME` ascending, with the node's own identifier as the
/// tie-break; a value outside either enum is refused.
#[tokio::test]
async fn list_cluster_nodes_sorts_by_every_documented_key_and_order() {
    let emulator = Emulator::new();
    let cluster = "sorted";
    let batches = seed_nodes(&emulator, cluster).await;

    // Nodes added by one batch share a launch time, so creation order is the batch order
    // with the node identifier breaking each tie.
    let by_creation: Vec<String> = batches
        .iter()
        .flat_map(|(_, logical_ids, _)| {
            let mut tied = logical_ids.clone();
            tied.sort();
            tied
        })
        .collect();
    let by_creation_desc: Vec<String> = {
        // Descending reverses the KEY only: the tie-break stays ascending, so the two
        // nodes of one batch keep their order.
        let mut batched: Vec<Vec<String>> = batches
            .iter()
            .map(|(_, logical_ids, _)| {
                let mut tied = logical_ids.clone();
                tied.sort();
                tied
            })
            .collect();
        batched.reverse();
        batched.into_iter().flatten().collect()
    };
    let by_name: Vec<String> = {
        let mut sorted = by_creation.clone();
        sorted.sort();
        sorted
    };
    let by_name_desc: Vec<String> = by_name.iter().rev().cloned().collect();

    // Rows: every (SortBy, SortOrder) pair the model allows, plus the defaults each
    // absent member documents ("The default value is CREATION_TIME", "The default value
    // is Ascending" — smithy_query operation=ListClusterNodes).
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("defaults", json!({}), by_creation.clone()),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CREATION_TIME", "SortOrder": "Ascending" }),
            by_creation.clone(),
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CREATION_TIME", "SortOrder": "Descending" }),
            by_creation_desc,
        ),
        (
            "name-ascending",
            json!({ "SortBy": "NAME", "SortOrder": "Ascending" }),
            by_name.clone(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "NAME", "SortOrder": "Descending" }),
            by_name_desc,
        ),
        ("sort-by-only", json!({ "SortBy": "NAME" }), by_name),
        (
            "sort-order-only",
            json!({ "SortOrder": "Ascending" }),
            by_creation,
        ),
    ];

    for (case, mut body, expected) in cases {
        body["ClusterName"] = Value::String(cluster.to_string());
        let listed = emulator.ok("ListClusterNodes", &with_logical_ids(&body)).await;
        assert_eq!(
            listed_logical_ids(&listed),
            expected,
            "case {case:?}: ListClusterNodes order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // A value outside either enum is refused, not silently sorted some other way.
    for (case, member, value) in [
        ("unknown-sort-by", "SortBy", "CreationTime"),
        ("unknown-sort-order", "SortOrder", "ascending"),
    ] {
        let body = json!({ "ClusterName": cluster, member: value });
        let (status, parsed) = emulator.call("ListClusterNodes", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    let cluster = "paged";
    let batches = seed_nodes(&emulator, cluster).await;
    let ordered = ids_of(&batches, &[0, 1, 2, 3]);

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListClusterNodes), so 1 and
    // 100 are the published bounds.
    for page_size in [1_i64, 2, 3, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({
                "ClusterName": cluster,
                "IncludeNodeLogicalIds": true,
                "SortBy": "NAME",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListClusterNodes", &body).await;
            let page = listed_logical_ids(&listed);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page must hold at most MaxResults entries, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must walk the whole ordered result exactly once, got {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call(
            "ListClusterNodes",
            &json!({ "ClusterName": cluster, "NextToken": "not-a-token" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: `ListClusterNodes` is a reporting call: beyond materialising the status
/// transition the clock already owed, two identical listings of an unchanged cluster are
/// identical, and no listing consumes, hides or reorders a node.
#[tokio::test]
async fn listing_nodes_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let cluster = "reporting";
    let batches = seed_nodes(&emulator, cluster).await;
    let ordered = ids_of(&batches, &[0, 1, 2, 3]);

    // Rows: the listings a client actually issues, each repeated.
    let bodies = [
        json!({ "ClusterName": cluster, "IncludeNodeLogicalIds": true }),
        json!({
            "ClusterName": cluster,
            "IncludeNodeLogicalIds": true,
            "SortBy": "NAME",
            "SortOrder": "Descending",
        }),
        json!({ "ClusterName": cluster, "IncludeNodeLogicalIds": true, "MaxResults": 2 }),
        json!({
            "ClusterName": cluster,
            "IncludeNodeLogicalIds": true,
            "InstanceGroupNameContains": "alpha",
        }),
        json!({ "ClusterName": cluster }),
    ];
    for body in &bodies {
        let first = emulator.ok("ListClusterNodes", body).await;
        let second = emulator.ok("ListClusterNodes", body).await;
        assert_eq!(
            first, second,
            "case {body}: a second identical listing must answer the same, got {first:?} then \
             {second:?}"
        );
    }

    // Every node is still there, in the same order, after all that listing.
    let listed = all_nodes(
        &emulator,
        &json!({
            "ClusterName": cluster,
            "IncludeNodeLogicalIds": true,
            "SortBy": "NAME",
        }),
    )
    .await;
    let reported: Vec<String> = listed
        .iter()
        .map(|summary| str_field(summary, "NodeLogicalId"))
        .collect();
    assert_eq!(
        reported, ordered,
        "listing must not consume or reorder a node, got {listed:?}"
    );
}

/// RULE: the cluster is the listing's subject, so an identifier this account holds no
/// cluster for is refused with the modelled `ResourceNotFound` — by name or by ARN — and
/// a cluster that holds no node is reported as an empty list, never as a refusal.
#[tokio::test]
async fn an_identifier_no_cluster_answers_to_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    let arn = create_cluster(&emulator, "held", &[("pool", "ml.c5.large")]).await;

    // Rows: the two forms `ClusterNameOrArn` admits (smithy_query
    // operation=ListClusterNodes), each naming nothing this account holds — a name never
    // created, a one-character name, an ARN of the minted form carrying an id nothing
    // was minted with, and the held cluster's own ARN with another Region or account,
    // which addresses a different cluster.
    let held_id = arn
        .rsplit('/')
        .next()
        .expect("a ClusterArn ends in the minted id")
        .to_string();
    let rows: Vec<(&str, String)> = vec![
        ("never-created", "absent".to_string()),
        ("one-character-name", "z".to_string()),
        (
            "unknown-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/0123456789ab"),
        ),
        (
            "another-region",
            format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:cluster/{held_id}"),
        ),
        (
            "another-account",
            format!("arn:aws:sagemaker:us-east-1:210987654321:cluster/{held_id}"),
        ),
    ];
    for (case, identifier) in rows {
        let (status, parsed) = emulator
            .call("ListClusterNodes", &json!({ "ClusterName": identifier }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: error code, got {parsed:?}"
        );
    }

    // A cluster that was never given a node is listed as an empty result, by name and by
    // ARN, with no pagination token.
    for identifier in ["held".to_string(), arn] {
        let listed = emulator
            .ok(
                "ListClusterNodes",
                &json!({ "ClusterName": identifier, "IncludeNodeLogicalIds": true }),
            )
            .await;
        assert!(
            summaries(&listed).is_empty() && listed.get("NextToken").is_none(),
            "case {identifier:?}: a cluster with no node must be listed empty, got {listed:?}"
        );
    }
}
