// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for RDS restart persistence (`Persisted` snapshot/restore) and
//! host-backed PostgreSQL snapshot volume retention.
//!
//! Every control-plane call goes over the wire through `MultiServiceRouter` as a real
//! awsQuery POST, populates non-empty tuple-keyed regional maps, serializes the full state
//! through `serde_json::to_string` (exercising JSON map-key, `DateTime`, `Instant`, and
//! catalog-entry serialization), and restores into a fresh `InMemoryEngine` + router.

use std::sync::Arc;

use ::service_common::persist::Persisted as _;
use ::service_common::workload::WorkloadHandle;
use service_com_amazonaws_rds_amazonrdsv19::state::{
    data_plane::real_engine::{self, RealWorkload},
    reconcile_restored_real_engines, ComAmazonawsRdsAmazonrdsv19State as State,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, expect_error, expect_ok, text, texts, wait_for_settle, REGION};

/// Mount `EmulatedService` over `engine` exactly as `db_instances::emulator` does, while
/// returning the `Arc<InMemoryEngine>` so a test can snapshot and restore `State` across
/// two separate engines.
fn mounted(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Serialize `before`'s RDS state to JSON text (not just in-memory `Value`) and restore it
/// into a brand-new `InMemoryEngine`, verifying every map key is JSON-safe.
fn roundtrip_engine(
    before: &engine::InMemoryEngine,
) -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let json_text = {
        let state = before.state::<State>();
        let store = state.lock();
        let snapshot = store.snapshot();
        serde_json::to_string(&snapshot).expect("RDS snapshot serializes to JSON text")
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&json_text).expect("RDS snapshot parses from JSON text");
    let restored = State::restore(State::VERSION, parsed).expect("RDS state restores cleanly");

    let after = Arc::new(engine::InMemoryEngine::new());
    *after.state::<State>().lock() = restored;
    let router = mounted(Arc::clone(&after));
    (after, router)
}

#[tokio::test]
async fn control_plane_records_timestamps_tags_and_mint_counter_survive_json_restart() {
    let before_engine = Arc::new(engine::InMemoryEngine::new());
    let before = mounted(Arc::clone(&before_engine));

    expect_ok(
        "create subnet group",
        call(
            &before,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "persist-subnets"),
                ("DBSubnetGroupDescription", "durable subnet group"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
                ("SubnetIds.SubnetIdentifier.2", "subnet-0fedcba987654321f"),
            ],
        )
        .await,
    );

    expect_ok(
        "create parameter group",
        call(
            &before,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "persist-pg16"),
                ("DBParameterGroupFamily", "postgres16"),
                ("Description", "durable postgres parameters"),
            ],
        )
        .await,
    );
    expect_ok(
        "modify parameter group",
        call(
            &before,
            "ModifyDBParameterGroup",
            &[
                ("DBParameterGroupName", "persist-pg16"),
                ("Parameters.Parameter.1.ParameterName", "log_min_duration_statement"),
                ("Parameters.Parameter.1.ParameterValue", "250"),
                ("Parameters.Parameter.1.ApplyMethod", "immediate"),
            ],
        )
        .await,
    );

    expect_ok(
        "create cluster parameter group",
        call(
            &before,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "persist-aurora-pg16"),
                ("DBParameterGroupFamily", "aurora-postgresql16"),
                ("Description", "durable cluster parameters"),
            ],
        )
        .await,
    );

    expect_ok(
        "create option group",
        call(
            &before,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "persist-options"),
                ("EngineName", "postgres"),
                ("MajorEngineVersion", "16"),
                ("OptionGroupDescription", "durable option group"),
            ],
        )
        .await,
    );

    expect_ok(
        "create db instance",
        call(
            &before,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "persist-db"),
                ("Engine", "postgres"),
                ("EngineVersion", "16.4"),
                ("DBInstanceClass", "db.t4g.micro"),
                ("MasterUsername", "appuser"),
                ("MasterUserPassword", "SecretPass123!"),
                ("DBName", "appdb"),
                ("DBSubnetGroupName", "persist-subnets"),
                ("DBParameterGroupName", "persist-pg16"),
                ("Tags.Tag.1.Key", "env"),
                ("Tags.Tag.1.Value", "nightly"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let instance_before = expect_ok(
        "describe instance before restart",
        call(
            &before,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "persist-db")],
        )
        .await,
    );
    let dbi_before = text(&instance_before, "DbiResourceId");
    let arn_before = text(&instance_before, "DBInstanceArn");
    let created_before = text(&instance_before, "InstanceCreateTime");
    let address_before = text(&instance_before, "Address");
    let port_before = text(&instance_before, "Port");

    expect_ok(
        "create manual snapshot",
        call(
            &before,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", "persist-db"),
                ("DBSnapshotIdentifier", "persist-snap-1"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let snap_before = expect_ok(
        "describe snapshot before restart",
        call(
            &before,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "persist-snap-1")],
        )
        .await,
    );
    let snap_created_before = text(&snap_before, "SnapshotCreateTime");

    expect_ok(
        "create aurora cluster",
        call(
            &before,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "persist-cluster"),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", "clusteradmin"),
                ("MasterUserPassword", "ClusterPass123!"),
                ("DBSubnetGroupName", "persist-subnets"),
                ("DBClusterParameterGroupName", "persist-aurora-pg16"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let mint_before = before_engine.state::<State>().lock().mint_counter;
    assert!(mint_before > 0);

    // Roundtrip through JSON text into a brand-new engine + router.
    let (after_engine, after) = roundtrip_engine(&before_engine);
    assert_eq!(
        reconcile_restored_real_engines(&*after_engine, &after_engine.state::<State>())
            .await
            .expect("reconcile succeeds with no real workloads"),
        0
    );
    assert_eq!(after_engine.state::<State>().lock().mint_counter, mint_before);

    let instance_after = expect_ok(
        "describe instance after restart",
        call(
            &after,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "persist-db")],
        )
        .await,
    );
    assert_eq!(text(&instance_after, "DBInstanceStatus"), "available");
    assert_eq!(text(&instance_after, "DbiResourceId"), dbi_before);
    assert_eq!(text(&instance_after, "DBInstanceArn"), arn_before);
    assert_eq!(text(&instance_after, "InstanceCreateTime"), created_before);
    assert_eq!(text(&instance_after, "Address"), address_before);
    assert_eq!(text(&instance_after, "Port"), port_before);

    let tags_after = expect_ok(
        "list tags after restart",
        call(
            &after,
            "ListTagsForResource",
            &[("ResourceName", arn_before.as_str())],
        )
        .await,
    );
    assert_eq!(texts(&tags_after, "Key"), vec!["env"]);
    assert_eq!(texts(&tags_after, "Value"), vec!["nightly"]);

    let snap_after = expect_ok(
        "describe snapshot after restart",
        call(
            &after,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "persist-snap-1")],
        )
        .await,
    );
    assert_eq!(text(&snap_after, "Status"), "available");
    assert_eq!(text(&snap_after, "SnapshotCreateTime"), snap_created_before);

    let subnet_after = expect_ok(
        "describe subnet group after restart",
        call(
            &after,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", "persist-subnets")],
        )
        .await,
    );
    assert_eq!(text(&subnet_after, "DBSubnetGroupDescription"), "durable subnet group");

    let params_after = expect_ok(
        "describe modified parameter after restart",
        call(
            &after,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", "persist-pg16"),
                ("Source", "user"),
            ],
        )
        .await,
    );
    assert_eq!(texts(&params_after, "ParameterName"), vec!["log_min_duration_statement"]);
    assert_eq!(texts(&params_after, "ParameterValue"), vec!["250"]);

    let cluster_after = expect_ok(
        "describe cluster after restart",
        call(
            &after,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "persist-cluster")],
        )
        .await,
    );
    assert_eq!(text(&cluster_after, "Status"), "available");

    let events_after = expect_ok(
        "describe events after restart",
        call(
            &after,
            "DescribeEvents",
            &[
                ("SourceIdentifier", "persist-db"),
                ("SourceType", "db-instance"),
            ],
        )
        .await,
    );
    assert!(!texts(&events_after, "Message").is_empty());
}

#[tokio::test]
async fn real_workload_final_snapshot_retains_volume_reference_across_restart_until_snapshot_deleted() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = mounted(Arc::clone(&engine));

    expect_ok(
        "create db instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "real-vol-db"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t4g.micro"),
                ("MasterUsername", "postgres"),
                ("MasterUserPassword", "SecretPass123!"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let dbi_id = {
        let described = expect_ok(
            "describe real-vol-db",
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "real-vol-db")],
            )
            .await,
        );
        text(&described, "DbiResourceId")
    };
    let expected_volume = real_engine::volume_name(&dbi_id);

    // Attach a tracked RealWorkload record for `real-vol-db` to exercise the real-engine
    // final-snapshot path in `DeleteDBInstance` and verify `real_volume` retention across
    // JSON persistence without requiring a live Docker daemon in unit CI.
    {
        let state = engine.state::<State>();
        let mut store = state.lock();
        store.real_workloads.insert(
            (REGION.to_string(), "real-vol-db".to_string()),
            RealWorkload {
                handle: WorkloadHandle {
                    kind: real_engine::WORKLOAD_KIND.to_string(),
                    id: "synthetic-container-id".to_string(),
                    adopted: false,
                },
                volume: expected_volume.clone(),
            },
        );
    }

    // Previously `DeleteDBInstance` refused any `FinalDBSnapshotIdentifier` when
    // `real_workloads` held an entry and unconditionally deleted the volume (`keep_data = false`).
    expect_ok(
        "delete real-backed instance with final snapshot",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "real-vol-db"),
                ("FinalDBSnapshotIdentifier", "real-vol-final-snap"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let (after_engine, after_router) = roundtrip_engine(&engine);
    {
        let state = after_engine.state::<State>();
        let store = state.lock();
        assert!(
            !store
                .real_workloads
                .contains_key(&(REGION.to_string(), "real-vol-db".to_string())),
            "deleted instance must leave real_workloads"
        );
        let snap = store
            .db_snapshots
            .get(&(REGION.to_string(), "real-vol-final-snap".to_string()))
            .expect("final snapshot survives JSON restart");
        assert_eq!(
            snap.real_volume.as_deref(),
            Some(expected_volume.as_str()),
            "final snapshot must retain the real-engine PGDATA volume name across restart"
        );
    }

    expect_ok(
        "delete final snapshot after restart",
        call(
            &after_router,
            "DeleteDBSnapshot",
            &[("DBSnapshotIdentifier", "real-vol-final-snap")],
        )
        .await,
    );
    expect_error(
        "final snapshot is gone after delete",
        call(
            &after_router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "real-vol-final-snap")],
        )
        .await,
        404,
        "DBSnapshotNotFound",
    );
}

#[tokio::test]
async fn skip_final_snapshot_delete_and_recreate_across_restart_mints_fresh_resource_id_and_volume() {
    let before_engine = Arc::new(engine::InMemoryEngine::new());
    let before = mounted(Arc::clone(&before_engine));

    expect_ok(
        "create first incarnation",
        call(
            &before,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "ephemeral-db"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t4g.micro"),
                ("MasterUsername", "postgres"),
                ("MasterUserPassword", "SecretPass123!"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let first_dbi = text(
        &expect_ok(
            "describe first incarnation",
            call(
                &before,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "ephemeral-db")],
            )
            .await,
        ),
        "DbiResourceId",
    );

    expect_ok(
        "delete with SkipFinalSnapshot=true",
        call(
            &before,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "ephemeral-db"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let (_after_engine, after) = roundtrip_engine(&before_engine);

    expect_ok(
        "recreate same identifier after restart",
        call(
            &after,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "ephemeral-db"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t4g.micro"),
                ("MasterUsername", "postgres"),
                ("MasterUserPassword", "SecretPass123!"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let second_dbi = text(
        &expect_ok(
            "describe second incarnation",
            call(
                &after,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "ephemeral-db")],
            )
            .await,
        ),
        "DbiResourceId",
    );

    assert_ne!(
        first_dbi, second_dbi,
        "recreating a deleted DBInstanceIdentifier after restart must mint a distinct DbiResourceId"
    );
    assert_ne!(
        real_engine::volume_name(&first_dbi),
        real_engine::volume_name(&second_dbi),
        "fresh DbiResourceId maps to a disjoint real-engine PGDATA volume"
    );
}
