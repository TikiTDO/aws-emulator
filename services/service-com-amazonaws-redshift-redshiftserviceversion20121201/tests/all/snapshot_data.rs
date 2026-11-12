// SPDX-License-Identifier: CC0-1.0

//! What a snapshot of a provisioned cluster HOLDS, and what a restore from it
//! serves: the cluster's databases, end to end over the awsQuery wire with the
//! Redshift Data API mounted beside the control plane on one engine.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over the one
//! sequence the issue quoted: a snapshot of a cluster carries that cluster's
//! databases as they stood when the snapshot was taken ("a snapshot contains a
//! copy of the cluster data as of this exact time",
//! `smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`),
//! and `RestoreFromClusterSnapshot` — which "creates a new cluster from a
//! snapshot" — makes the cluster it creates serve exactly those databases: every
//! table and row written before the snapshot, and none written after it.
//!
//! The rows differ along what that rule generalises over: the value and type of
//! the data, the number of rows, the number of tables, the database the data is
//! in, and what the source cluster did after the snapshot was taken. The same
//! rule is then asserted on the other two ways this service mints a snapshot of a
//! cluster — `CopyClusterSnapshot` and the final snapshot `DeleteCluster` takes —
//! and on a cluster no statement has ever run against, which holds no table to
//! restore.
//!
//! The control-plane harness (the signed awsQuery call, the Data API call) is
//! shared with [`crate::cluster_lifecycle`], which owns it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Emulator, REGION};

/// The node type every cluster in this suite is created with — one the published
/// catalog offers, as `CreateCluster` requires.
const NODE_TYPE: &str = "ra3.xlplus";

/// The admin user every cluster is created with.
const MASTER_USERNAME: &str = "admin";

/// The `DbUser` every statement authenticates as. Any name does: the Data API's
/// engine addresses a database by cluster and database name.
const DB_USER: &str = "awsuser";

/// The `WaitTimeSeconds` every statement asks for: inside the modelled range and
/// far longer than the SQL engine's own deadline, so what decides a statement's
/// outcome is the engine and never this request's patience on a loaded machine.
const WAIT_SECONDS: i32 = 10;

/// Create the cluster `identifier` holding the initial database `database`.
///
/// `pub(crate)` so a sibling suite about the same rule drives the Data API
/// through this harness instead of writing a second copy of it.
pub(crate) async fn create_cluster(
    emulator: &Emulator,
    case: &str,
    identifier: &str,
    database: &str,
) {
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", NODE_TYPE),
                ("MasterUsername", MASTER_USERNAME),
                ("DBName", database),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateCluster {identifier} expected 200, got {} with body {}",
        created.status, created.body
    );
}

/// Snapshot `cluster` under `snapshot`, and answer the snapshot's ARN.
pub(crate) async fn create_snapshot(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    snapshot: &str,
) -> String {
    let taken = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", snapshot),
                ("ClusterIdentifier", cluster),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case {case:?}: CreateClusterSnapshot {snapshot} expected 200, got {} with body {}",
        taken.status, taken.body
    );
    taken.text("SnapshotArn").unwrap_or_default()
}

/// Restore `snapshot` into the new cluster `identifier`.
pub(crate) async fn restore(emulator: &Emulator, case: &str, snapshot: &str, identifier: &str) {
    let restored = emulator
        .call(
            "RestoreFromClusterSnapshot",
            &[
                ("ClusterIdentifier", identifier),
                ("SnapshotIdentifier", snapshot),
            ],
        )
        .await;
    assert_eq!(
        restored.status, 200,
        "case {case:?}: RestoreFromClusterSnapshot into {identifier} expected 200, got {} \
         with body {}",
        restored.status, restored.body
    );
}

/// Run `sql` on `cluster`/`database` through the Data API, requiring it to
/// finish, and answer the statement's `Id`.
pub(crate) async fn run_sql(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    database: &str,
    sql: &str,
) -> String {
    let body = submit_sql(emulator, case, cluster, database, sql).await;
    assert_eq!(
        body["Status"],
        json!("FINISHED"),
        "case {case:?}: statement {sql:?} on {cluster}/{database} expected FINISHED, got \
         {body}"
    );
    body["Id"].as_str().unwrap_or_default().to_string()
}

/// Submit `sql` on `cluster`/`database` and answer the statement as the Data API
/// reports it, WITHOUT requiring it to finish: a read of a table the restored
/// cluster does not hold fails, and that failure is what proves the data is not
/// there.
pub(crate) async fn submit_sql(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    database: &str,
    sql: &str,
) -> Value {
    let (status, body) = emulator
        .call_data_api(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": cluster,
                "Database": database,
                "DbUser": DB_USER,
                "Sql": sql,
                "WaitTimeSeconds": WAIT_SECONDS,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ExecuteStatement {sql:?} on {cluster}/{database} expected 200, got \
         {status} with {body}"
    );
    body
}

/// The `Records` `sql` answers on `cluster`/`database`.
pub(crate) async fn query(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    database: &str,
    sql: &str,
) -> Value {
    let id = run_sql(emulator, case, cluster, database, sql).await;
    let (status, body) = emulator
        .call_data_api("GetStatementResult", &json!({ "Id": id }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStatementResult for {sql:?} expected 200, got {status} with {body}"
    );
    body["Records"].clone()
}

/// Wait out the clock-owed `creating` -> `available` transition a snapshot is
/// minted with: only an available snapshot can be restored from.
pub(crate) async fn settle() {
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
}

/// One row of the snapshot-data table: what was written before the snapshot, what
/// the source did after it, the read-back, and what the restored cluster must
/// answer it with.
#[derive(Debug, Clone, Copy)]
struct DataCase {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The cluster's initial database, which the writes and the read-back run in.
    database: &'static str,
    /// The statements that write the data the snapshot has to capture.
    writes: &'static [&'static str],
    /// Statements run against the SOURCE cluster AFTER the snapshot was taken, so
    /// the row also shows that a restore serves the snapshot and not the source as
    /// it stands now.
    after_snapshot: &'static [&'static str],
    /// The read-back, run against the restored cluster and against the source.
    read: &'static str,
    /// The `Records` the read-back must answer on the RESTORED cluster.
    expected: fn() -> Value,
    /// The `Records` the same read-back answers on the SOURCE cluster, which has
    /// also taken `after_snapshot`.
    source_expected: fn() -> Value,
}

#[tokio::test]
async fn a_restored_cluster_serves_the_databases_the_snapshot_captured() {
    let cases = [
        DataCase {
            label: "integers and text in three rows",
            database: "devdb",
            writes: &[
                "CREATE TABLE people (id INTEGER, name VARCHAR(16))",
                "INSERT INTO people (id, name) VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')",
            ],
            after_snapshot: &["INSERT INTO people (id, name) VALUES (4, 'edsger')"],
            read: "SELECT id, name FROM people ORDER BY id",
            expected: || {
                json!([
                    [{"longValue": 1}, {"stringValue": "ada"}],
                    [{"longValue": 2}, {"stringValue": "alan"}],
                    [{"longValue": 3}, {"stringValue": "grace"}]
                ])
            },
            source_expected: || {
                json!([
                    [{"longValue": 1}, {"stringValue": "ada"}],
                    [{"longValue": 2}, {"stringValue": "alan"}],
                    [{"longValue": 3}, {"stringValue": "grace"}],
                    [{"longValue": 4}, {"stringValue": "edsger"}]
                ])
            },
        },
        DataCase {
            label: "two tables, and a third created after the snapshot",
            database: "analytics",
            writes: &[
                "CREATE TABLE orders (id INTEGER)",
                "CREATE TABLE items (id INTEGER)",
                "INSERT INTO orders (id) VALUES (1), (2)",
                "INSERT INTO items (id) VALUES (7)",
            ],
            after_snapshot: &["CREATE TABLE refunds (id INTEGER)"],
            read: "SELECT (SELECT COUNT(*) FROM orders), (SELECT COUNT(*) FROM items)",
            expected: || json!([[{"longValue": 2}, {"longValue": 1}]]),
            source_expected: || json!([[{"longValue": 2}, {"longValue": 1}]]),
        },
        DataCase {
            label: "a hundred rows, none of them deleted by the source afterwards",
            database: "devdb",
            writes: &[
                "CREATE TABLE series (n INTEGER)",
                "INSERT INTO series (n) WITH RECURSIVE counter(n) AS \
                 (SELECT 1 UNION ALL SELECT n + 1 FROM counter WHERE n < 100) \
                 SELECT n FROM counter",
            ],
            after_snapshot: &["DELETE FROM series WHERE n > 10"],
            read: "SELECT COUNT(*), MAX(n) FROM series",
            expected: || json!([[{"longValue": 100}, {"longValue": 100}]]),
            source_expected: || json!([[{"longValue": 10}, {"longValue": 10}]]),
        },
        DataCase {
            label: "a row of every value kind",
            database: "reporting",
            writes: &[
                "CREATE TABLE mixed (i INTEGER, r REAL, t VARCHAR(8), b BLOB, n INTEGER)",
                "INSERT INTO mixed (i, r, t, b, n) VALUES (7, 1.5, 'x', x'6162', NULL)",
            ],
            after_snapshot: &[],
            read: "SELECT i, r, t, b, n FROM mixed",
            expected: || {
                json!([[
                    {"longValue": 7},
                    {"doubleValue": 1.5},
                    {"stringValue": "x"},
                    {"blobValue": "YWI="},
                    {"isNull": true}
                ]])
            },
            source_expected: || {
                json!([[
                    {"longValue": 7},
                    {"doubleValue": 1.5},
                    {"stringValue": "x"},
                    {"blobValue": "YWI="},
                    {"isNull": true}
                ]])
            },
        },
        DataCase {
            label: "a table the source drops after the snapshot",
            database: "devdb",
            writes: &[
                "CREATE TABLE ledger (entry VARCHAR(8))",
                "INSERT INTO ledger (entry) VALUES ('kept')",
                "CREATE TABLE audit (entry VARCHAR(8))",
                "INSERT INTO audit (entry) VALUES ('also-kept')",
            ],
            after_snapshot: &["DROP TABLE audit"],
            read: "SELECT entry FROM ledger ORDER BY entry",
            expected: || json!([[{"stringValue": "kept"}]]),
            source_expected: || json!([[{"stringValue": "kept"}]]),
        },
    ];

    let emulator = Emulator::with_data_api();
    for (index, case) in cases.iter().enumerate() {
        let source = format!("snapdata-src-{index}");
        let target = format!("snapdata-dst-{index}");
        let snapshot = format!("snapdata-snap-{index}");

        create_cluster(&emulator, case.label, &source, case.database).await;
        for sql in case.writes {
            run_sql(&emulator, case.label, &source, case.database, sql).await;
        }

        create_snapshot(&emulator, case.label, &source, &snapshot).await;
        for sql in case.after_snapshot {
            run_sql(&emulator, case.label, &source, case.database, sql).await;
        }
        settle().await;

        restore(&emulator, case.label, &snapshot, &target).await;

        // The point of the restore: the restored cluster's own SQL engine serves
        // the snapshot's rows.
        let restored_records = query(&emulator, case.label, &target, case.database, case.read).await;
        assert_eq!(
            restored_records,
            (case.expected)(),
            "case {label:?}: {read:?} on the restored cluster {target} expected {expected}, \
             got {restored_records}",
            label = case.label,
            read = case.read,
            expected = (case.expected)()
        );
        // And the source is untouched by the restore: it still serves what it
        // holds now, including the writes it took after the snapshot.
        let source_records = query(&emulator, case.label, &source, case.database, case.read).await;
        assert_eq!(
            source_records,
            (case.source_expected)(),
            "case {label:?}: {read:?} on the source cluster {source} expected {expected}, got \
             {source_records}",
            label = case.label,
            read = case.read,
            expected = (case.source_expected)()
        );
    }
}

#[tokio::test]
async fn a_snapshot_of_a_cluster_no_statement_has_run_against_restores_no_table() {
    let case = "a cluster no statement has run against";
    let emulator = Emulator::with_data_api();
    create_cluster(&emulator, case, "snapdata-empty-src", "devdb").await;
    create_snapshot(&emulator, case, "snapdata-empty-src", "snapdata-empty-snap").await;
    settle().await;
    restore(&emulator, case, "snapdata-empty-snap", "snapdata-empty-dst").await;

    // Nothing was ever written, so there is nothing to restore and the read of a
    // table that was never created fails on the restored cluster's own engine —
    // rather than answering rows no statement ever wrote.
    let body = submit_sql(
        &emulator,
        case,
        "snapdata-empty-dst",
        "devdb",
        "SELECT 1 FROM people",
    )
    .await;
    assert_eq!(
        body["Status"],
        json!("FAILED"),
        "case {case:?}: a read of a table the snapshot never held expected FAILED, got {body}"
    );
}

/// One row of the other-ways-to-mint-a-snapshot table: how the snapshot the
/// restore reads is minted, and the identifier it ends up under.
#[derive(Debug, Clone, Copy)]
enum Minted {
    /// `CopyClusterSnapshot` of a snapshot taken by `CreateClusterSnapshot`: a
    /// copy of a snapshot holds the same data as the snapshot it copies.
    Copy,
    /// The final snapshot `DeleteCluster` takes of the cluster it deletes.
    FinalSnapshot,
}

#[tokio::test]
async fn every_way_a_snapshot_of_a_cluster_is_minted_carries_the_clusters_data() {
    let cases = [
        (Minted::Copy, "a copy of a snapshot"),
        (Minted::FinalSnapshot, "a cluster's final snapshot"),
    ];

    let emulator = Emulator::with_data_api();
    for (index, (minted, label)) in cases.iter().enumerate() {
        let source = format!("snapmint-src-{index}");
        let target = format!("snapmint-dst-{index}");
        let snapshot = format!("snapmint-snap-{index}");

        create_cluster(&emulator, label, &source, "devdb").await;
        for sql in [
            "CREATE TABLE stock (sku VARCHAR(8), count INTEGER)",
            "INSERT INTO stock (sku, count) VALUES ('aa', 2), ('bb', 5)",
        ] {
            run_sql(&emulator, label, &source, "devdb", sql).await;
        }

        let restored_from = match minted {
            Minted::Copy => {
                // The AUTOMATED snapshot the copy is taken of. Nothing mints one
                // through the API — `CreateClusterSnapshot` produces a manual
                // snapshot and this service copies only automated ones — so the
                // record is written straight into the same store the handlers
                // read, carrying a REAL capture of the source cluster's
                // databases taken through the same seam the create takes it
                // through.
                let original = format!("snapmint-orig-{index}");
                let data = state::capture_cluster_data(&emulator.engine, source.clone())
                    .await
                    .unwrap_or_else(|failure| {
                        panic!("case {label:?}: the cluster's databases must copy: {failure}")
                    });
                let store =
                    state::service_state(&*emulator.engine).expect("the Redshift store is there");
                let stored_cluster = store
                    .lock()
                    .cluster(&source)
                    .cloned()
                    .unwrap_or_else(|| panic!("case {label:?}: the source cluster is stored"));
                let mut automated = state::snapshot_of(
                    &stored_cluster,
                    &original,
                    aws_smithy_types::DateTime::from(std::time::SystemTime::now()),
                    state::MANUAL_RETENTION_INDEFINITE,
                    Vec::new(),
                    REGION,
                );
                automated.snapshot_type = Some(state::SNAPSHOT_TYPE_AUTOMATED.to_string());
                automated.status = Some(state::STATUS_AVAILABLE.to_string());
                store.lock().snapshots.push(state::StoredSnapshot {
                    snapshot: automated,
                    // Already past, so the record needs no clock-owed transition.
                    available_at: std::time::Instant::now(),
                    source: Some(stored_cluster),
                    data: std::sync::Arc::new(data),
                });

                let copied = emulator
                    .call(
                        "CopyClusterSnapshot",
                        &[
                            ("SourceSnapshotIdentifier", &original),
                            ("TargetSnapshotIdentifier", &snapshot),
                        ],
                    )
                    .await;
                assert_eq!(
                    copied.status, 200,
                    "case {label:?}: CopyClusterSnapshot expected 200, got {} with body {}",
                    copied.status, copied.body
                );
                snapshot.clone()
            }
            Minted::FinalSnapshot => {
                let deleted = emulator
                    .call(
                        "DeleteCluster",
                        &[
                            ("ClusterIdentifier", &source),
                            ("SkipFinalClusterSnapshot", "false"),
                            ("FinalClusterSnapshotIdentifier", &snapshot),
                        ],
                    )
                    .await;
                assert_eq!(
                    deleted.status, 200,
                    "case {label:?}: DeleteCluster expected 200, got {} with body {}",
                    deleted.status, deleted.body
                );
                snapshot.clone()
            }
        };
        settle().await;

        restore(&emulator, label, &restored_from, &target).await;
        let records = query(
            &emulator,
            label,
            &target,
            "devdb",
            "SELECT sku, count FROM stock ORDER BY sku",
        )
        .await;
        let expected = json!([
            [{"stringValue": "aa"}, {"longValue": 2}],
            [{"stringValue": "bb"}, {"longValue": 5}]
        ]);
        assert_eq!(
            records,
            expected,
            "case {label:?}: the cluster restored from {restored_from} expected {expected}, \
             got {records}"
        );
    }
}
