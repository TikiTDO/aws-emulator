// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `DBClusterSnapshot` ENVELOPE every operation that answers with
//! a DB cluster snapshot carries.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so each assertion reads the XML the emulator itself produced.
//!
//! The rules under test:
//!
//! * A snapshot reports the data at rest it holds the way the CLUSTER it was taken from
//!   reports its own: `StorageEncryptionType` is the source cluster's own reading, so the
//!   two can never disagree about whether the data is unencrypted, encrypted under the key
//!   the service applies itself, or encrypted under a KMS key.
//! * A record of a CLUSTER reports the cluster engine's own license model: an Aurora or
//!   Neptune engine is the licensed product, so its snapshots — and the automated backup of
//!   the same cluster — report the ENGINE'S name, while every other engine's cluster reports
//!   the licence that engine family publishes.
//! * The source cluster's custom-data-group flag is carried onto its snapshots, and the full
//!   size of the data a snapshot holds is reported once the snapshot has been TAKEN — a
//!   snapshot still being made reports no size at all, and one that captured no data reports
//!   zero.
//! * ONE projection: the create that took the snapshot, the read that polls it and the
//!   delete that removed it report the same values for all of them.
//!
//! The table spans what those rules generalise over: the two cluster TYPES
//! `CreateDBCluster` documents (Aurora DB clusters and Multi-AZ DB clusters), four engines
//! across the three control planes this front door serves, a request that leaves storage
//! encryption to the service and one that asks for it, and a source cluster with and without
//! a DB instance in it (so the snapshot both captures storage and captures none). The size a
//! snapshot reports is driven over the engine's own PostgreSQL wire: a client writes rows,
//! snapshots the cluster, writes more and snapshots again, so the size is read against data
//! this test knows the extent of.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_cluster_snapshot_restore::cluster_endpoint;
use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instance_engine_endpoint::{DATABASE, MASTER_PASSWORD, MASTER_USER};
use crate::db_instances::{call, emulator, expect_ok, optional_text, subtrees, text};
// The ledger the snapshot-restore suites write and read over the PostgreSQL wire: the size a
// snapshot reports is the size of exactly that data, so this suite drives the same table
// rather than keeping a second copy of the harness.
use crate::db_snapshot_restore::{fill_ledger, run, session_on, LEDGER_ROWS};

/// How long a poll loop is given before this suite calls a transition unobservable.
///
/// Generous next to the span a transition this service owes becomes due after, so a loaded
/// test machine cannot fail the suite.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The status a snapshot that has been taken, and a cluster that is ready to be snapshotted,
/// report.
const AVAILABLE: &str = "available";

/// The instance class a cluster member is created with: one the orderable-options report
/// offers for a cluster of every engine this front door serves.
const CLUSTER_MEMBER_CLASS: &str = "db.r5.large";

/// One row of the snapshot-envelope table.
struct SnapshotCase {
    case: &'static str,
    /// The `DBClusterIdentifier` of the cluster the snapshot is taken from.
    cluster: &'static str,
    engine: &'static str,
    /// The parameters the create carries beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the cluster has a DB INSTANCE in it, which is what decides whether the
    /// snapshot captures any storage at all.
    with_instance: bool,
    /// The `LicenseModel` the snapshot must report, or `None` for an engine this service
    /// publishes no license model for.
    license_model: Option<&'static str>,
    /// The `StorageEncryptionType` the snapshot must report, from the value list
    /// `DBClusterSnapshot$StorageEncryptionType` publishes (`none` | `sse-rds` | `sse-kms`).
    storage_encryption_type: &'static str,
}

const SNAPSHOT_CASES: &[SnapshotCase] = &[
    SnapshotCase {
        case: "an Aurora MySQL cluster, storage encryption left to the service",
        cluster: "emu-snapenv-aurora-mysql",
        engine: "aurora-mysql",
        extra: &[],
        with_instance: false,
        license_model: Some("aurora-mysql"),
        storage_encryption_type: "sse-rds",
    },
    SnapshotCase {
        case: "an Aurora MySQL cluster created with encrypted storage",
        cluster: "emu-snapenv-aurora-mysql-kms",
        engine: "aurora-mysql",
        extra: &[("StorageEncrypted", "true")],
        with_instance: false,
        license_model: Some("aurora-mysql"),
        storage_encryption_type: "sse-kms",
    },
    SnapshotCase {
        case: "an Aurora PostgreSQL cluster with a member DB instance",
        cluster: "emu-snapenv-aurora-postgresql",
        engine: "aurora-postgresql",
        extra: &[],
        with_instance: true,
        license_model: Some("aurora-postgresql"),
        storage_encryption_type: "sse-rds",
    },
    SnapshotCase {
        case: "a Neptune cluster",
        cluster: "emu-snapenv-neptune",
        engine: "neptune",
        extra: &[],
        with_instance: false,
        license_model: Some("neptune"),
        storage_encryption_type: "sse-rds",
    },
    SnapshotCase {
        case: "a DocumentDB cluster, whose engine publishes no license model",
        cluster: "emu-snapenv-docdb",
        engine: "docdb",
        extra: &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "pw1-Secret"),
        ],
        with_instance: false,
        license_model: None,
        storage_encryption_type: "sse-rds",
    },
    SnapshotCase {
        case: "a Multi-AZ DB cluster, whose storage is not encrypted at all",
        cluster: "emu-snapenv-multiaz",
        engine: "postgres",
        // `Iops` and `StorageType` are required to create a Multi-AZ DB cluster, as every
        // other suite that creates one names them.
        extra: &[
            ("AllocatedStorage", "100"),
            ("DBClusterInstanceClass", "db.m6gd.xlarge"),
            ("StorageType", "io1"),
            ("Iops", "1000"),
        ],
        with_instance: false,
        license_model: Some("postgresql-license"),
        storage_encryption_type: "none",
    },
    SnapshotCase {
        case: "a Multi-AZ DB cluster created with encrypted storage",
        cluster: "emu-snapenv-multiaz-kms",
        engine: "postgres",
        extra: &[
            ("AllocatedStorage", "100"),
            ("DBClusterInstanceClass", "db.m6gd.xlarge"),
            ("StorageType", "io1"),
            ("Iops", "1000"),
            ("StorageEncrypted", "true"),
        ],
        with_instance: false,
        license_model: Some("postgresql-license"),
        storage_encryption_type: "sse-kms",
    },
];

/// `CreateDBCluster` for one row.
async fn create_cluster(router: &MultiServiceRouter, row: &SnapshotCase) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", row.cluster),
        ("Engine", row.engine),
    ];
    params.extend_from_slice(row.extra);
    call(router, "CreateDBCluster", &params).await
}

/// The one `<DBClusterSnapshot>` subtree a response carries.
fn the_snapshot(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBClusterSnapshot");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBClusterSnapshot, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// `DescribeDBClusterSnapshots` for one snapshot, polled until it reports
/// `available` — the status a snapshot that has been TAKEN carries.
///
/// A poll loop rather than a wait of a fixed length: the loop ends on the CONDITION, so it
/// cannot depend on how fast the host runs, and only reporting calls are made inside it.
async fn snapshot_once_taken(router: &MultiServiceRouter, case: &str, snapshot: &str) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let body = expect_ok(
            case,
            call(
                router,
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", snapshot)],
            )
            .await,
        );
        let reported = the_snapshot(case, &body);
        if text(&reported, "Status") == AVAILABLE {
            return reported;
        }
        last = reported;
        sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the snapshot never reached {AVAILABLE} in {POLL_DEADLINE:?}; \
         last answer: {last}"
    )
}

/// `DescribeDBClusters` for one cluster, polled until it reports `available` — the status a
/// cluster has to be in for a snapshot of it to be accepted.
///
/// A poll loop for the same reason as [`snapshot_once_taken`]: the loop ends on the
/// CONDITION rather than after a span, so how fast the host runs cannot decide it.
async fn cluster_once_available(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let reported = describe_one(router, case, identifier).await;
        if text(&reported, "Status") == AVAILABLE {
            return reported;
        }
        last = reported;
        sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the cluster never reached {AVAILABLE} in {POLL_DEADLINE:?}; \
         last answer: {last}"
    )
}

/// Assert the members a snapshot projects off the cluster it was taken from, whichever
/// operation answered with it.
fn assert_copied_members(
    case: &str,
    envelope: &str,
    snapshot: &str,
    row: &SnapshotCase,
    cluster: &str,
) {
    let case = format!("{case} ({envelope})");

    let reported = optional_text(snapshot, "LicenseModel");
    assert_eq!(
        reported.as_deref(),
        row.license_model,
        "case {case:?}: expected LicenseModel {:?}, got {reported:?} in {snapshot}",
        row.license_model
    );

    // The encryption type is asserted against the row AND against the source cluster's own
    // envelope, so the snapshot cannot report data at rest differently from the storage it
    // was copied from.
    let reported = optional_text(snapshot, "StorageEncryptionType");
    assert_eq!(
        reported.as_deref(),
        Some(row.storage_encryption_type),
        "case {case:?}: expected StorageEncryptionType {:?}, got {reported:?} in {snapshot}",
        row.storage_encryption_type
    );
    let of_the_cluster = text(cluster, "StorageEncryptionType");
    assert_eq!(
        reported.as_deref(),
        Some(of_the_cluster.as_str()),
        "case {case:?}: the snapshot reports the cluster's own StorageEncryptionType \
         {of_the_cluster:?}, got {reported:?} in {snapshot}"
    );

    // The flag is the source cluster's, read off that cluster's envelope rather than
    // restated here.
    let reported = optional_text(snapshot, "CustomDataGroupPresent");
    let of_the_cluster = text(cluster, "CustomDataGroupPresent");
    assert_eq!(
        reported.as_deref(),
        Some(of_the_cluster.as_str()),
        "case {case:?}: the snapshot carries the cluster's CustomDataGroupPresent \
         {of_the_cluster:?}, got {reported:?} in {snapshot}"
    );
}

#[tokio::test]
async fn a_cluster_snapshot_envelope_projects_the_cluster_it_was_taken_from() {
    let router = emulator();

    for row in SNAPSHOT_CASES {
        let snapshot = format!("{}-s1", row.cluster);
        expect_ok(row.case, create_cluster(&router, row).await);
        if row.with_instance {
            let member = format!("{}-m0", row.cluster);
            expect_ok(
                row.case,
                call(
                    &router,
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", member.as_str()),
                        ("Engine", row.engine),
                        ("DBInstanceClass", CLUSTER_MEMBER_CLASS),
                        ("DBClusterIdentifier", row.cluster),
                    ],
                )
                .await,
            );
        }
        let cluster = cluster_once_available(&router, row.case, row.cluster).await;

        // The create answers the snapshot it stored: the copied members are already there,
        // and the size is not — the snapshot is still being taken.
        let taken = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", row.cluster),
                    ("DBClusterSnapshotIdentifier", snapshot.as_str()),
                ],
            )
            .await,
        );
        let taken = the_snapshot(row.case, &taken);
        assert_copied_members(row.case, "CreateDBClusterSnapshot", &taken, row, &cluster);
        let reported = optional_text(&taken, "FullSnapshotSizeInBytes");
        assert_eq!(
            reported, None,
            "case {:?}: a snapshot still being taken reports no size, got {reported:?} in \
             {taken}",
            row.case
        );

        // The read-back a waiter polls reports the same members, plus the size of the data
        // the finished snapshot holds — zero for one taken of a cluster nothing was ever
        // written to.
        let read_back = snapshot_once_taken(&router, row.case, &snapshot).await;
        assert_copied_members(
            row.case,
            "DescribeDBClusterSnapshots",
            &read_back,
            row,
            &cluster,
        );
        let reported = optional_text(&read_back, "FullSnapshotSizeInBytes");
        assert_eq!(
            reported.as_deref(),
            Some("0"),
            "case {:?}: a snapshot that holds no data reports zero bytes, got {reported:?} \
             in {read_back}",
            row.case
        );

        // The delete answers the snapshot as it stood, with the same copied members: it is
        // the last record a client can capture for the snapshot.
        let removed = expect_ok(
            row.case,
            call(
                &router,
                "DeleteDBClusterSnapshot",
                &[("DBClusterSnapshotIdentifier", snapshot.as_str())],
            )
            .await,
        );
        let removed = the_snapshot(row.case, &removed);
        assert_copied_members(row.case, "DeleteDBClusterSnapshot", &removed, row, &cluster);
    }
}

#[tokio::test]
async fn a_cluster_automated_backup_reports_the_same_license_model_as_the_clusters_snapshots() {
    let router = emulator();

    for row in SNAPSHOT_CASES {
        expect_ok(row.case, create_cluster(&router, row).await);
        let backups = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBClusterAutomatedBackups",
                &[("DBClusterIdentifier", row.cluster)],
            )
            .await,
        );
        let reported = optional_text(&backups, "LicenseModel");
        assert_eq!(
            reported.as_deref(),
            row.license_model,
            "case {:?}: the automated backup of a cluster reports the engine's license \
             model {:?}, got {reported:?} in {backups}",
            row.case,
            row.license_model
        );
    }
}

/// The cluster envelope and its snapshots agree about the engine the snapshot was taken
/// from, whatever version the cluster was created with: the snapshot copies the cluster's
/// own `EngineVersion` rather than re-deriving a default of its own.
#[tokio::test]
async fn a_cluster_snapshot_copies_the_source_clusters_engine_version() {
    let router = emulator();

    // (case, the `EngineVersion` the create names, or `None` to leave it to the service).
    let rows: [(&str, Option<&str>); 3] = [
        ("no version named, so the engine's default", None),
        ("an exact version of the 8.0 line", Some("8.0.mysql_aurora.3.05.2")),
        ("an exact version of the 5.7 line", Some("5.7.mysql_aurora.2.11.6")),
    ];

    for (index, (case, version)) in rows.iter().enumerate() {
        let cluster = format!("emu-snapenv-version-{index}");
        let snapshot = format!("{cluster}-s1");
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", cluster.as_str()),
            ("Engine", "aurora-mysql"),
        ];
        if let Some(version) = version {
            params.push(("EngineVersion", version));
        }
        let created =
            the_cluster(case, &expect_ok(case, call(&router, "CreateDBCluster", &params).await));
        let provisioned = text(&created, "EngineVersion");
        if let Some(version) = version {
            assert_eq!(
                provisioned, *version,
                "case {case:?}: the cluster runs the version the create named, got \
                 {provisioned:?} in {created}"
            );
        }

        let ready = cluster_once_available(&router, case, &cluster).await;
        assert_eq!(
            text(&ready, "EngineVersion"),
            provisioned,
            "case {case:?}: the cluster keeps the version it was created with: {ready}"
        );
        let taken = the_snapshot(
            case,
            &expect_ok(
                case,
                call(
                    &router,
                    "CreateDBClusterSnapshot",
                    &[
                        ("DBClusterIdentifier", cluster.as_str()),
                        ("DBClusterSnapshotIdentifier", snapshot.as_str()),
                    ],
                )
                .await,
            ),
        );
        let reported = text(&taken, "EngineVersion");
        assert_eq!(
            reported, provisioned,
            "case {case:?}: the snapshot reports the cluster's engine version \
             {provisioned:?}, got {reported:?} in {taken}"
        );

        let read_back = snapshot_once_taken(&router, case, &snapshot).await;
        let reported = text(&read_back, "EngineVersion");
        assert_eq!(
            reported, provisioned,
            "case {case:?}: the snapshot read back reports the cluster's engine version \
             {provisioned:?}, got {reported:?} in {read_back}"
        );
    }
}

/// The row a client writes AFTER the first snapshot was taken, so it is part of what a later
/// snapshot holds and of no earlier one.
const LATER_ROW: (i32, &str) = (99, "written after the first snapshot");

/// The bytes the engine accounts one `(id INTEGER, note TEXT)` ledger row: eight for the
/// integer and the note's own length, which is how a stored value's size is read
/// (`state::data_plane::sql_engine::Cell::byte_cost`).
fn ledger_row_bytes(note: &str) -> i64 {
    8 + note.len() as i64
}

/// `FullSnapshotSizeInBytes` off a snapshot envelope, as the number it is.
fn reported_size(case: &str, snapshot: &str) -> i64 {
    let reported = text(snapshot, "FullSnapshotSizeInBytes");
    reported.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: FullSnapshotSizeInBytes must be a number, got {reported:?}")
    })
}

/// `CreateDBClusterSnapshot`, as the `<DBClusterSnapshot>` it answered.
async fn take_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    snapshot: &str,
) -> String {
    the_snapshot(
        case,
        &expect_ok(
            case,
            call(
                router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", cluster),
                    ("DBClusterSnapshotIdentifier", snapshot),
                ],
            )
            .await,
        ),
    )
}

/// The size a snapshot reports is the size of the DATA IT CAPTURED, measured at the instant
/// it was taken: it counts the rows a client wrote over the engine's own wire before the
/// snapshot, it does not change when the source is written to afterwards, and a snapshot
/// taken after those later writes is larger by exactly what they added.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_reports_the_size_of_the_data_it_captured() {
    let router = emulator();
    let case = "an Aurora PostgreSQL cluster whose rows a client wrote over the wire";
    let cluster = "emu-snapenv-sized";

    // A cluster a client can log in to: the master credential and the database the
    // engine-endpoint suites of this target connect with.
    expect_ok(
        case,
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", cluster),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", MASTER_USER),
                ("MasterUserPassword", MASTER_PASSWORD),
                ("DatabaseName", DATABASE),
            ],
        )
        .await,
    );
    let ready = cluster_once_available(&router, case, cluster).await;
    let endpoint = cluster_endpoint(case, &ready);
    let mut session = session_on(&endpoint, case).await;
    fill_ledger(&mut session, case).await;

    // The data the first snapshot captures includes exactly the ledger this client wrote, so
    // the size it reports accounts for those rows' values (the engine's own catalog of the
    // captured databases is part of the copy too, so the ledger is a floor, not the whole).
    let ledger_bytes: i64 = LEDGER_ROWS
        .iter()
        .map(|(_, note)| ledger_row_bytes(note))
        .sum();
    take_snapshot(&router, case, cluster, "emu-snapenv-sized-s1").await;
    let first = snapshot_once_taken(&router, case, "emu-snapenv-sized-s1").await;
    let first_size = reported_size(case, &first);
    assert!(
        first_size >= ledger_bytes,
        "case {case:?}: the snapshot reports the size of the data it captured, which holds \
         the {ledger_bytes} bytes of the ledger's values, got {first_size} in {first}"
    );

    // One more row, written after that snapshot was taken.
    run(
        &mut session,
        case,
        &format!(
            "INSERT INTO ledger (id, note) VALUES ({}, '{}')",
            LATER_ROW.0, LATER_ROW.1
        ),
    )
    .await;

    // The first snapshot's copy is immutable, so its size is the same on every later read.
    let again = snapshot_once_taken(&router, case, "emu-snapenv-sized-s1").await;
    let again_size = reported_size(case, &again);
    assert_eq!(
        again_size, first_size,
        "case {case:?}: a write to the source after the snapshot cannot change the size the \
         snapshot holds, got {again_size} where it was {first_size} in {again}"
    );

    // A snapshot taken now holds that row too, so it is larger by exactly its values.
    take_snapshot(&router, case, cluster, "emu-snapenv-sized-s2").await;
    let second = snapshot_once_taken(&router, case, "emu-snapenv-sized-s2").await;
    let second_size = reported_size(case, &second);
    let grown_by = ledger_row_bytes(LATER_ROW.1);
    assert_eq!(
        second_size,
        first_size + grown_by,
        "case {case:?}: a snapshot of the grown ledger is larger by the row that was added \
         ({grown_by} bytes), got {second_size} where the earlier one was {first_size} in \
         {second}"
    );
}
