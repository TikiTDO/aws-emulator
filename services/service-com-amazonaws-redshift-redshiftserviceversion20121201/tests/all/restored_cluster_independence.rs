// SPDX-License-Identifier: CC0-1.0

//! What a cluster restored from a snapshot and the cluster the snapshot was taken
//! of are to EACH OTHER once the restore has happened.
//!
//! The rule under test: `RestoreFromClusterSnapshot` "creates a new cluster from a
//! snapshot" (`smithy_query(query="operation",
//! operation="RestoreFromClusterSnapshot")`) holding the data the snapshot
//! captured ("a snapshot contains a copy of the cluster data as of this exact
//! time", `smithy_query(query="doc", shape="Snapshot",
//! member="SnapshotCreateTime")`) — a new cluster, so what it serves is its OWN
//! copy of that data: after the restore each cluster is written independently of
//! the other, and the snapshot still holds the copy it captured, so it restores
//! the same data again into a further cluster.
//!
//! [`crate::snapshot_data`] asserts what a restore SERVES (the rows written before
//! the snapshot and none written after it) and owns the statement helpers; this
//! suite asserts the independence of the copies the restore leaves behind, over a
//! table whose rows differ along which cluster takes the later write and what kind
//! of write it is — a row appended, a row removed, a row changed, the table
//! dropped, and a table created that the other cluster never had.
//!
//! The control-plane harness (the signed awsQuery call, the Data API call) is
//! shared with [`crate::cluster_lifecycle`], which owns it.

use serde_json::{json, Value};

use crate::cluster_lifecycle::Emulator;
use crate::snapshot_data::{
    create_cluster, create_snapshot, query, restore, run_sql, settle, submit_sql,
};

/// The database every cluster in this suite is created with.
const DATABASE: &str = "devdb";

/// The table the seeded rows live in, named distinctly enough that a failure
/// naming it is unmistakably about this table.
const SEEDED_TABLE: &str = "snapshotted";

/// The statements that seed the data the snapshot captures, so every case starts
/// from the same two rows.
const SEED: &[&str] = &[
    "CREATE TABLE snapshotted (id INTEGER, val VARCHAR(16))",
    "INSERT INTO snapshotted (id, val) VALUES (1, 'one'), (2, 'two')",
];

/// The read-back of the seeded table, which the snapshot's own copy must keep
/// answering however either cluster is written afterwards.
const SEEDED_READ: &str = "SELECT id, val FROM snapshotted ORDER BY id";

/// The `Records` [`SEEDED_READ`] answers on a cluster holding exactly the seeded
/// rows.
fn seeded_rows() -> Value {
    json!([
        [{"longValue": 1}, {"stringValue": "one"}],
        [{"longValue": 2}, {"stringValue": "two"}]
    ])
}

/// Which of the two clusters a case writes to after the restore.
#[derive(Debug, Clone, Copy)]
enum Side {
    /// The cluster the snapshot was taken of.
    Source,
    /// The cluster the restore created.
    Restored,
}

impl Side {
    /// The other cluster — the one that must not see the write.
    fn other(self) -> Self {
        match self {
            Self::Source => Self::Restored,
            Self::Restored => Self::Source,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Source => "the source cluster",
            Self::Restored => "the restored cluster",
        }
    }
}

/// What a read answers on one of the two clusters.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Exactly these `Records`.
    Rows(fn() -> Value),
    /// The statement does not finish, and the failure names this table: the
    /// cluster holds no table of that name, which is how "the write that created
    /// (or dropped) the table never reached me" is observable.
    NoSuchTable(&'static str),
}

/// One row of the independence table.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// Which cluster takes the statements below after the restore.
    side: Side,
    /// The write, run after the restore against `side` only.
    writes: &'static [&'static str],
    /// The read-back, run against BOTH clusters.
    read: &'static str,
    /// What `read` answers on the cluster that took the write.
    written: Expect,
    /// What the same `read` answers on the cluster that did not.
    untouched: Expect,
}

/// Assert `read` answers `expect` on `cluster`.
async fn assert_read(emulator: &Emulator, case: &str, cluster: &str, read: &str, expect: Expect) {
    match expect {
        Expect::Rows(rows) => {
            let records = query(emulator, case, cluster, DATABASE, read).await;
            assert_eq!(
                records,
                rows(),
                "case {case:?}: {read:?} on {cluster} expected {expected}, got {records}",
                expected = rows()
            );
        }
        Expect::NoSuchTable(table) => {
            let body = submit_sql(emulator, case, cluster, DATABASE, read).await;
            assert_eq!(
                body["Status"],
                json!("FAILED"),
                "case {case:?}: {read:?} on {cluster}, which holds no table {table:?}, \
                 expected FAILED, got {body}"
            );
            // Why it failed, which is the part that says the table is the one
            // missing rather than the statement failing for some other reason.
            let id = body["Id"].as_str().unwrap_or_default().to_string();
            let (status, described) = emulator
                .call_data_api("DescribeStatement", &json!({ "Id": id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeStatement for {read:?} expected 200, got {status} \
                 with {described}"
            );
            let reported = described["Error"].as_str().unwrap_or_default();
            assert!(
                reported.contains(table),
                "case {case:?}: the failure of {read:?} on {cluster} must name the table \
                 {table:?} that cluster does not hold, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn a_restored_cluster_and_its_source_are_written_independently() {
    let cases = [
        Case {
            label: "a row appended on the restored cluster",
            side: Side::Restored,
            writes: &["INSERT INTO snapshotted (id, val) VALUES (3, 'three')"],
            read: SEEDED_READ,
            written: Expect::Rows(|| {
                json!([
                    [{"longValue": 1}, {"stringValue": "one"}],
                    [{"longValue": 2}, {"stringValue": "two"}],
                    [{"longValue": 3}, {"stringValue": "three"}]
                ])
            }),
            untouched: Expect::Rows(seeded_rows),
        },
        Case {
            label: "a row removed on the source cluster",
            side: Side::Source,
            writes: &["DELETE FROM snapshotted WHERE id = 1"],
            read: SEEDED_READ,
            written: Expect::Rows(|| json!([[{"longValue": 2}, {"stringValue": "two"}]])),
            untouched: Expect::Rows(seeded_rows),
        },
        Case {
            label: "a row changed on the restored cluster",
            side: Side::Restored,
            writes: &["UPDATE snapshotted SET val = 'changed' WHERE id = 2"],
            read: SEEDED_READ,
            written: Expect::Rows(|| {
                json!([
                    [{"longValue": 1}, {"stringValue": "one"}],
                    [{"longValue": 2}, {"stringValue": "changed"}]
                ])
            }),
            untouched: Expect::Rows(seeded_rows),
        },
        Case {
            label: "the table dropped on the restored cluster",
            side: Side::Restored,
            writes: &["DROP TABLE snapshotted"],
            read: SEEDED_READ,
            written: Expect::NoSuchTable(SEEDED_TABLE),
            untouched: Expect::Rows(seeded_rows),
        },
        Case {
            label: "a table created on the source cluster",
            side: Side::Source,
            writes: &[
                "CREATE TABLE added_later (id INTEGER)",
                "INSERT INTO added_later (id) VALUES (9)",
            ],
            read: "SELECT id FROM added_later ORDER BY id",
            written: Expect::Rows(|| json!([[{"longValue": 9}]])),
            untouched: Expect::NoSuchTable("added_later"),
        },
    ];

    let emulator = Emulator::with_data_api();
    for (index, case) in cases.iter().enumerate() {
        let source = format!("snapindep-src-{index}");
        let target = format!("snapindep-dst-{index}");
        let again = format!("snapindep-again-{index}");
        let snapshot = format!("snapindep-snap-{index}");

        create_cluster(&emulator, case.label, &source, DATABASE).await;
        for sql in SEED {
            run_sql(&emulator, case.label, &source, DATABASE, sql).await;
        }
        create_snapshot(&emulator, case.label, &source, &snapshot).await;
        settle().await;
        restore(&emulator, case.label, &snapshot, &target).await;

        let written = match case.side {
            Side::Source => source.clone(),
            Side::Restored => target.clone(),
        };
        let untouched = match case.side.other() {
            Side::Source => source.clone(),
            Side::Restored => target.clone(),
        };
        for sql in case.writes {
            run_sql(&emulator, case.label, &written, DATABASE, sql).await;
        }

        // The cluster that took the write serves it...
        assert_read(&emulator, case.label, &written, case.read, case.written).await;
        // ... and the other one, which the restore left as a copy of its own,
        // serves what it held before: the write reached one cluster only.
        assert_read(
            &emulator,
            &format!("{} ({} took the write)", case.label, case.side.label()),
            &untouched,
            case.read,
            case.untouched,
        )
        .await;

        // And the snapshot still holds the copy it captured, neither consumed by
        // the first restore nor altered by what either cluster did afterwards: it
        // restores the same data again into a further cluster.
        restore(&emulator, case.label, &snapshot, &again).await;
        let records = query(&emulator, case.label, &again, DATABASE, SEEDED_READ).await;
        assert_eq!(
            records,
            seeded_rows(),
            "case {label:?}: {SEEDED_READ:?} on {again}, restored a second time from the same \
             snapshot, expected {expected}, got {records}",
            label = case.label,
            expected = seeded_rows()
        );
    }
}
