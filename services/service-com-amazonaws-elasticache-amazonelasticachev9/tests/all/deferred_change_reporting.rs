// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two changes ElastiCache reports as OUTSTANDING after it has
//! accepted them: a cache cluster moved onto another cache parameter group, and an auth
//! token being rotated.
//!
//! The rules under test:
//!
//! * a cache parameter group's values are read by the engine when a node STARTS, so a
//!   cluster moved onto another group goes on running the values its nodes were started
//!   with: the association moves at once and
//!   `CacheParameterGroupStatus$ParameterApplyStatus` reports `pending-reboot` with every
//!   node still owed one named in `CacheNodeIdsToReboot`, whatever `ApplyImmediately` says,
//!   while the cluster itself stays `available`. `RebootCacheCluster` is what applies them
//!   and puts the association back `in-sync`;
//! * an auth-token ROTATE adds the new password beside the old one — both are valid from
//!   then on — so the rotation is outstanding until the caller finalizes it:
//!   `ReplicationGroupPendingModifiedValues$AuthTokenStatus` reports `ROTATING` however the
//!   call was applied, and the `SET` that is "allowed only after ROTATE" is what closes the
//!   window and clears it.
//!
//! The tests are tables: the rows span `ApplyImmediately`, how many nodes the cluster has,
//! which engine it runs and whether the group it is moved onto is a different one at all, so
//! no single recorded request is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// What `CacheParameterGroupStatus$ParameterApplyStatus` reports: `in-sync` once the nodes
/// run the group's values, and `pending-reboot` while a node is still owed the restart that
/// loads them. Written out here rather than read off the constants the handler carries, so
/// the test pins the wire values.
const IN_SYNC: &str = "in-sync";
const PENDING_REBOOT: &str = "pending-reboot";

/// `AuthTokenUpdateStatus`'s value while a rotation is outstanding, and the two update
/// strategies this suite drives (`AuthTokenUpdateStrategyType`: `SET | ROTATE | DELETE`).
const ROTATING: &str = "ROTATING";
const ROTATE: &str = "ROTATE";
const SET: &str = "SET";

/// An `AuthToken` the service accepts: within the published length bounds and drawn from the
/// published character set (`AuthToken`: "Must be at least 16 characters and no more than
/// 128 characters in length").
const FIRST_TOKEN: &str = "First-Rotation-Token-1";
const SECOND_TOKEN: &str = "Second-Rotation-Token-2";
const THIRD_TOKEN: &str = "Third-Rotation-Token-3";

/// Create one cache parameter group of `family` and assert it was accepted.
async fn create_parameter_group(emulator: &Emulator, case: &str, name: &str, family: &str) {
    let created = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", name),
                ("CacheParameterGroupFamily", family),
                ("Description", "a group to move a cluster onto"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the parameter group must be creatable, got {} {}",
        created.status, created.body
    );
}

/// Wait until the cache cluster `id` reports the status the clock owes it after a create or
/// a reboot: `available`.
async fn await_available_cluster(emulator: &Emulator, case: &str, id: &str) -> Reply {
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the cache cluster {id} settling at available"),
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
        |reply| reply.text("CacheClusterStatus") == Some(state::STATUS_AVAILABLE),
    )
    .await
}

/// Create one cache cluster and let the clock settle it at `available`.
async fn create_settled_cluster(
    emulator: &Emulator,
    case: &str,
    id: &str,
    extra: &[(&str, &str)],
) {
    let mut pairs = params(&[("CacheClusterId", id)]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateCacheCluster", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the cluster must be creatable, got {} {}",
        created.status, created.body
    );
    await_available_cluster(emulator, case, id).await;
}

/// `DescribeCacheClusters` for one cluster, node detail and all.
async fn describe_cluster(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
        )
        .await
}

/// Wait until the replication group `id` reports the status the clock owes it: `available`.
async fn await_available_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the replication group {id} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await
}

/// Moving a cluster onto another parameter group leaves the change waiting for a reboot, and
/// the reboot is what applies it.
#[tokio::test]
async fn a_cluster_moved_onto_another_parameter_group_waits_for_a_reboot() {
    // The axis: the engine and the node count (so the list of nodes owed a reboot is not one
    // shape), and whether the change was asked for immediately or for the maintenance
    // window — the values apply at a node START either way.
    let rows: &[(&str, &str, &str, &str, &str, usize, &str)] = &[
        (
            "a two-node Memcached cluster, deferred",
            "pending-memcached",
            "memcached",
            "memcached1.6",
            "false",
            2,
            "2",
        ),
        (
            "a two-node Memcached cluster, applied immediately",
            "pending-memcached-now",
            "memcached",
            "memcached1.6",
            "true",
            2,
            "2",
        ),
        (
            "a single-node Valkey cluster, deferred",
            "pending-valkey",
            "valkey",
            "valkey7",
            "false",
            1,
            "1",
        ),
        (
            "a single-node Redis OSS cluster, ApplyImmediately omitted",
            "pending-redis",
            "redis",
            "redis7",
            "",
            1,
            "1",
        ),
    ];

    for (case, id, engine, family, apply, nodes, node_count) in rows {
        let emulator = Emulator::new();
        let first = format!("{id}-group-a");
        let second = format!("{id}-group-b");
        create_parameter_group(&emulator, case, &first, family).await;
        create_parameter_group(&emulator, case, &second, family).await;
        create_settled_cluster(
            &emulator,
            case,
            id,
            &[
                ("Engine", engine),
                ("CacheNodeType", "cache.m7g.large"),
                ("NumCacheNodes", node_count),
                ("CacheParameterGroupName", &first),
            ],
        )
        .await;

        // Before the move, the cluster runs the values its group holds: nothing is owed a
        // reboot.
        let before = describe_cluster(&emulator, id).await;
        assert_eq!(
            before.text("CacheParameterGroup/ParameterApplyStatus"),
            Some(IN_SYNC),
            "case {case:?}: expected the association to start in sync, got {}",
            before.body
        );

        let mut pairs = params(&[
            ("CacheClusterId", id),
            ("CacheParameterGroupName", &second),
        ]);
        if !apply.is_empty() {
            pairs.extend(params(&[("ApplyImmediately", apply)]));
        }
        let modified = emulator.call("ModifyCacheCluster", &pairs).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: the modify must answer 200, got {} {}",
            modified.status, modified.body
        );

        // THE RULE, on the answer and on a later read alike — both are built from the one
        // stored cluster, so they cannot disagree. A queued parameter change starts no
        // asynchronous work, so the cluster never leaves `available`.
        let described = describe_cluster(&emulator, id).await;
        let expected_nodes: Vec<String> = (1..=*nodes).map(|node| format!("{node:04}")).collect();
        for (source, reply) in [("the answer", &modified), ("a later describe", &described)] {
            assert_eq!(
                reply.text("CacheParameterGroup/CacheParameterGroupName"),
                Some(second.as_str()),
                "case {case:?} ({source}): expected the new group to be associated, got {}",
                reply.body
            );
            assert_eq!(
                reply.text("CacheParameterGroup/ParameterApplyStatus"),
                Some(PENDING_REBOOT),
                "case {case:?} ({source}): expected the change to wait for a reboot, got {}",
                reply.body
            );
            assert_eq!(
                reply.all("CacheParameterGroup/CacheNodeIdsToReboot/CacheNodeId"),
                expected_nodes,
                "case {case:?} ({source}): expected every node to be owed a reboot, got {}",
                reply.body
            );
            assert_eq!(
                reply.text("CacheClusterStatus"),
                Some(state::STATUS_AVAILABLE),
                "case {case:?} ({source}): a queued parameter change leaves the cluster \
                 available, got {}",
                reply.body
            );
        }

        // The reboot of the nodes owed one is what applies the values.
        let mut reboot = params(&[("CacheClusterId", id)]);
        for (index, node) in expected_nodes.iter().enumerate() {
            reboot.push((
                format!("CacheNodeIdsToReboot.CacheNodeId.{}", index + 1),
                node.clone(),
            ));
        }
        let rebooted = emulator.call("RebootCacheCluster", &reboot).await;
        assert_eq!(
            rebooted.status, 200,
            "case {case:?}: the reboot must answer 200, got {} {}",
            rebooted.status, rebooted.body
        );
        // The reboot is what applies the values: the association is in sync once the nodes
        // the reboot restarted are back, which is the condition the suite waits for.
        let settled = await_available_cluster(&emulator, case, id).await;
        assert_eq!(
            settled.text("CacheParameterGroup/ParameterApplyStatus"),
            Some(IN_SYNC),
            "case {case:?}: expected the reboot to apply the values, got {}",
            settled.body
        );
        assert!(
            settled
                .all("CacheParameterGroup/CacheNodeIdsToReboot/CacheNodeId")
                .is_empty(),
            "case {case:?}: expected no node to be owed a reboot afterwards, got {}",
            settled.body
        );
    }
}

/// Re-applying the group a cluster already runs changes no value, so nothing is owed a
/// reboot for it.
#[tokio::test]
async fn re_applying_the_group_a_cluster_already_runs_owes_no_reboot() {
    let case = "the group the cluster already runs, in another case";
    let emulator = Emulator::new();
    let group = "same-group";
    create_parameter_group(&emulator, case, group, "redis7").await;
    create_settled_cluster(
        &emulator,
        case,
        "unchanged-cluster",
        &[
            ("Engine", "redis"),
            ("CacheNodeType", "cache.m7g.large"),
            ("NumCacheNodes", "1"),
            ("CacheParameterGroupName", group),
        ],
    )
    .await;

    // The axis: the same name, and the same name spelled in another case — a group is
    // resolved the one way, so both name the group the cluster already runs.
    for named in [group, "SAME-GROUP"] {
        let modified = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "unchanged-cluster"),
                    ("CacheParameterGroupName", named),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case:?} ({named}): the modify must answer 200, got {} {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.text("CacheParameterGroup/ParameterApplyStatus"),
            Some(IN_SYNC),
            "case {case:?} ({named}): expected the association to stay in sync, got {}",
            modified.body
        );
    }
}

/// An auth-token rotation is outstanding until the caller finalizes it, so the group reports
/// the grace window until a `SET` closes it.
#[tokio::test]
async fn an_auth_token_rotation_reports_the_grace_window_until_a_set_closes_it() {
    // The axis: whether the rotation was asked for immediately or for the maintenance
    // window — the second password is valid beside the first either way, so the rotation is
    // reported outstanding in both.
    for (case, apply) in [
        ("a rotation applied immediately", "true"),
        ("a rotation left for the maintenance window", "false"),
    ] {
        let emulator = Emulator::new();
        let id = "rotating-group";
        let created = emulator
            .call(
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", id),
                    ("ReplicationGroupDescription", "a group with a password"),
                    ("Engine", state::DEFAULT_ENGINE),
                    ("CacheNodeType", "cache.m7g.large"),
                    ("TransitEncryptionEnabled", "true"),
                    ("AuthToken", FIRST_TOKEN),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the group must be creatable, got {} {}",
            created.status, created.body
        );
        await_available_group(&emulator, case, id).await;

        let rotated = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", id),
                    ("AuthToken", SECOND_TOKEN),
                    ("AuthTokenUpdateStrategy", ROTATE),
                    ("ApplyImmediately", apply),
                ]),
            )
            .await;
        assert_eq!(
            rotated.status, 200,
            "case {case:?}: the rotation must be accepted, got {} {}",
            rotated.status, rotated.body
        );
        assert_eq!(
            rotated.text("PendingModifiedValues/AuthTokenStatus"),
            Some(ROTATING),
            "case {case:?}: expected the answer to report the rotation outstanding, got {}",
            rotated.body
        );
        assert!(
            !rotated.body.contains(SECOND_TOKEN),
            "case {case:?}: the password must never reach the wire, got {}",
            rotated.body
        );

        // The window stays open past the span the group's own change takes: the rotation
        // ends when the caller ends it, not when the clock does. So the suite waits for the
        // group to settle — the condition — and then reads the window.
        let described = await_available_group(&emulator, case, id).await;
        assert_eq!(
            (
                described.text("Status"),
                described.text("PendingModifiedValues/AuthTokenStatus"),
            ),
            (
                Some(state::STATUS_AVAILABLE),
                Some(ROTATING),
            ),
            "case {case:?}: expected the group available with the rotation still outstanding, \
             got {}",
            described.body
        );

        // The SET that is "allowed only after ROTATE" closes the window.
        let finalized = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", id),
                    ("AuthToken", THIRD_TOKEN),
                    ("AuthTokenUpdateStrategy", SET),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            finalized.status, 200,
            "case {case:?}: the finalizing set must be accepted, got {} {}",
            finalized.status, finalized.body
        );
        let closed = await_available_group(&emulator, case, id).await;
        assert_eq!(
            closed.text("PendingModifiedValues/AuthTokenStatus"),
            None,
            "case {case:?}: expected the window to be closed, got {}",
            closed.body
        );
        assert_eq!(
            closed.text("AuthTokenEnabled"),
            Some("true"),
            "case {case:?}: expected the group to keep a password, got {}",
            closed.body
        );
    }
}
