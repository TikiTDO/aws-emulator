// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreDBInstanceFromDBSnapshot`.
//!
//! The rule under test: restoring a DB snapshot creates a NEW DB instance whose settings
//! are the SOURCE's as the snapshot copied them — overridden, member by member, by the
//! ones this request names — and whose storage holds the DATA the snapshot captured, so a
//! client that connects to the restored instance's own engine endpoint reads the rows the
//! source held when the snapshot was taken and nothing written to either instance
//! afterwards.
//!
//! Every control-plane call goes over the wire (the awsQuery form POST the router keys on)
//! and every data-plane statement goes over the PostgreSQL wire, through the client the
//! engine-endpoint suite drives — so what these tests exercise is the protocol a stock
//! driver speaks, in one process.
//!
//! The tables span what the rule generalises over: the members a restore may override and
//! the ones it inherits, an engine this emulator serves a data plane for and one it does
//! not, both forms the source may be named by (identifier and ARN), both ways a DB
//! snapshot comes to exist (`CreateDBSnapshot` and the final snapshot `DeleteDBInstance`
//! takes), and each refusal the operation's model declares for a source or a target it
//! cannot serve.

use crate::db_instance_engine_endpoint::{
    available_instance, endpoint_of, postgres_login, PostgresSession, Rows, Transport, DATABASE,
    MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, emulator, expect_error, expect_ok, text, wait_for_settle};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One row of the override table: what the restore asks for beyond the source, and what
/// the restored instance must then report.
struct OverrideRow {
    label: &'static str,
    overrides: Params,
    expected: Params,
}

/// One row of the refusal table: the request, and the status and wire error `<Code>` it
/// must be refused with.
struct RefusalRow {
    label: &'static str,
    request: Params,
    status: u16,
    code: &'static str,
}

/// The table every data-plane case writes, and the rows the SNAPSHOT holds: a table a
/// client creates over the wire, so the restored instance's rows can only come from the
/// copy the snapshot took.
///
/// `pub(crate)` with the four helpers below: the DB CLUSTER restore suite proves the same
/// rule about a cluster's storage, so it drives the same ledger rather than keeping a second
/// copy of it.
pub(crate) const LEDGER_ROWS: [(i32, &str); 3] = [
    (1, "the first entry"),
    (2, "the second entry"),
    (3, "the third entry"),
];

/// Open a session on `endpoint` as the master user, against the instance's own database.
///
/// TLS, because RDS for PostgreSQL 15 and later force it by default and these instances are
/// created on the catalogue's default version. See [`LEDGER_ROWS`] for why this is shared.
pub(crate) async fn session_on(endpoint: &(String, u16), case: &str) -> PostgresSession {
    postgres_login(
        &endpoint.0,
        endpoint.1,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the endpoint must admit the master user, got {refusal:?}")
    })
}

/// Run one statement that returns no rows, and fail the case if the engine refused it.
pub(crate) async fn run(session: &mut PostgresSession, case: &str, sql: &str) {
    if let Err(refusal) = session.query(sql).await {
        panic!("case {case:?}: the engine must run {sql:?}, got {refusal:?}");
    }
}

/// The `(id, note)` rows of the ledger table, as a session reads them.
pub(crate) async fn ledger_of(session: &mut PostgresSession, case: &str) -> Vec<(i32, String)> {
    let answer: Rows = session
        .query("SELECT id, note FROM ledger ORDER BY id")
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case:?}: the ledger must be readable, got {refusal:?}")
        });
    answer
        .rows
        .iter()
        .map(|row| {
            let id = row
                .first()
                .and_then(Option::as_deref)
                .unwrap_or_default()
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: the id column must be an integer"));
            (
                id,
                row.get(1)
                    .and_then(Option::as_deref)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The rows [`LEDGER_ROWS`] stands for, as a reader sees them.
pub(crate) fn expected_ledger() -> Vec<(i32, String)> {
    LEDGER_ROWS
        .iter()
        .map(|(id, note)| (*id, (*note).to_string()))
        .collect()
}

/// Create the ledger and fill it with [`LEDGER_ROWS`].
pub(crate) async fn fill_ledger(session: &mut PostgresSession, case: &str) {
    run(
        session,
        case,
        "CREATE TABLE ledger (id INTEGER PRIMARY KEY, note TEXT NOT NULL)",
    )
    .await;
    for (id, note) in LEDGER_ROWS {
        run(
            session,
            case,
            &format!("INSERT INTO ledger (id, note) VALUES ({id}, '{note}')"),
        )
        .await;
    }
}

/// Restore `snapshot` into a new DB instance named `identifier`, with `extra` as the
/// members the request overrides, and answer with the envelope the restore returned.
async fn restore(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
    snapshot: &str,
    extra: &[(&str, String)],
) -> String {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "DBSnapshotIdentifier" => snapshot,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    expect_ok(
        case,
        call(router, "RestoreDBInstanceFromDBSnapshot", &request).await,
    )
}

/// `DescribeDBInstances` for one instance, once the clock has settled it.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_instance_carries_the_snapshots_settings_and_the_requests_overrides() {
    let router = emulator();
    // The source instances the snapshots below are taken from: one engine whose wire this
    // emulator serves, and one it serves none for — a restore is a DB instance either way.
    let sources = [
        ("postgres", "db.t3.micro", 5432),
        ("oracle-se2", "db.m5.large", 1521),
    ];
    for (engine, class, port) in sources {
        let source = format!("emu-restore-src-{}", engine.replace(['-', '.'], ""));
        let snapshot = format!("{source}-snap");
        let created = expect_ok(
            engine,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => source,
                    "Engine" => engine,
                    "DBInstanceClass" => class,
                    "AllocatedStorage" => 20,
                    "MasterUsername" => MASTER_USER,
                    "MasterUserPassword" => MASTER_PASSWORD,
                    "DBName" => DATABASE,
                    "BackupRetentionPeriod" => 5,
                    "PreferredBackupWindow" => "03:00-03:30",
                    "Tags.Tag.1.Key" => "owner",
                    "Tags.Tag.1.Value" => "the snapshot suite",
                ],
            )
            .await,
        );
        let source_version = text(&created, "EngineVersion");
        wait_for_settle();
        expect_ok(
            engine,
            call(
                &router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => source,
                    "DBSnapshotIdentifier" => snapshot,
                ],
            )
            .await,
        );
        wait_for_settle();

        // Each row names what the request overrides and what the restored instance must
        // then report; the members no row names are the source's, which the row after the
        // overrides asserts.
        let rows = [
            OverrideRow {
                label: "nothing overridden",
                overrides: Vec::new(),
                expected: params![
                    "DBInstanceClass" => class,
                    "AllocatedStorage" => 20,
                    "BackupRetentionPeriod" => 5,
                    "PreferredBackupWindow" => "03:00-03:30",
                    "MasterUsername" => MASTER_USER,
                    "Engine" => engine,
                    "EngineVersion" => source_version,
                    "MultiAZ" => "false",
                    "DBInstanceStatus" => "available",
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "a larger instance class",
                overrides: params!["DBInstanceClass" => "db.m5.xlarge"].to_vec(),
                expected: params![
                    "DBInstanceClass" => "db.m5.xlarge",
                    "AllocatedStorage" => 20,
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "more storage and a port of its own",
                overrides: params!["AllocatedStorage" => 50, "Port" => 6432].to_vec(),
                expected: params!["AllocatedStorage" => 50].to_vec(),
            },
            OverrideRow {
                label: "the backup settings replaced",
                overrides: params![
                    "BackupRetentionPeriod" => 0,
                    "PreferredBackupWindow" => "11:00-11:30",
                ]
                .to_vec(),
                expected: params![
                    "BackupRetentionPeriod" => 0,
                    "PreferredBackupWindow" => "11:00-11:30",
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "a Multi-AZ deployment with deletion protection",
                overrides: params!["MultiAZ" => true, "DeletionProtection" => true].to_vec(),
                expected: params!["MultiAZ" => "true", "DeletionProtection" => "true"].to_vec(),
            },
        ];
        for (index, row) in rows.into_iter().enumerate() {
            let OverrideRow {
                label,
                overrides,
                expected,
            } = row;
            let case = format!("{engine}: {label}");
            let target = format!("emu-restored-{}-{index}", engine.replace(['-', '.'], ""));
            let restored = restore(&router, &case, &target, &snapshot, &overrides).await;
            assert_eq!(
                text(&restored, "DBInstanceStatus"),
                "creating",
                "case {case:?}: a restored instance is still being created when the call \
                 returns, got {restored}"
            );
            wait_for_settle();
            let described = describe(&router, &case, &target).await;
            for (member, value) in &expected {
                assert_eq!(
                    &text(&described, member),
                    value,
                    "case {case:?}: expected {member} to be {value:?}, got \
                     {:?} in {described}",
                    text(&described, member)
                );
            }
            // The restored instance is its OWN resource: a new resource id, and an
            // endpoint of its own on the port the restore asked for.
            assert_ne!(
                text(&described, "DbiResourceId"),
                text(&created, "DbiResourceId"),
                "case {case:?}: a restored instance gets its own DbiResourceId, got the \
                 source's in {described}"
            );
            let wanted_port = overrides
                .iter()
                .find(|(member, _)| *member == "Port")
                .map_or(port.to_string(), |(_, value)| value.clone());
            // The port a request asked for is the instance's own; an engine this emulator
            // serves publishes the port its listener was assigned instead, which is why
            // only the unserved engine's endpoint is read for it.
            if engine == "oracle-se2" {
                let (address, published) = endpoint_of(&described);
                assert_eq!(
                    published.to_string(),
                    wanted_port,
                    "case {case:?}: the restored instance listens on the port the restore \
                     asked for, got {address}:{published}"
                );
            }
            // The tags the snapshot carried from its source, since this request named none.
            assert_eq!(
                text(&described, "Key"),
                "owner",
                "case {case:?}: with no new tags the restore takes the source's, got \
                 {described}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_instance_serves_the_rows_the_snapshot_held() {
    let router = emulator();
    let case = "restore the rows a CreateDBSnapshot captured";
    let source_endpoint = available_instance(&router, "emu-rows-source", "postgres", &[]).await;
    let mut source = session_on(&source_endpoint, case).await;
    fill_ledger(&mut source, case).await;

    expect_ok(
        case,
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-rows-source",
                "DBSnapshotIdentifier" => "emu-rows-snap",
            ],
        )
        .await,
    );
    wait_for_settle();
    // Written AFTER the snapshot was taken, so it is not part of what the snapshot holds:
    // a restore brings back the rows of that instant, not the source's rows now.
    run(
        &mut source,
        case,
        "INSERT INTO ledger (id, note) VALUES (99, 'after the snapshot')",
    )
    .await;

    // Both forms `DBSnapshotIdentifier` accepts name the same snapshot: its identifier,
    // and the ARN a shared manual snapshot is named by.
    let snapshot_arn = text(
        &expect_ok(
            case,
            call(
                &router,
                "DescribeDBSnapshots",
                &params!["DBSnapshotIdentifier" => "emu-rows-snap"],
            )
            .await,
        ),
        "DBSnapshotArn",
    );
    for (label, named) in [
        ("by identifier", "emu-rows-snap".to_string()),
        ("by ARN", snapshot_arn),
    ] {
        let case = format!("{case} {label}");
        let target = format!("emu-rows-restored-{}", label.replace(' ', ""));
        restore(&router, &case, &target, &named, &[]).await;
        wait_for_settle();
        let described = describe(&router, &case, &target).await;
        let restored_endpoint = endpoint_of(&described);
        assert_ne!(
            restored_endpoint.1, source_endpoint.1,
            "case {case:?}: the restored instance has an engine endpoint of its own, got \
             the source's port"
        );
        let mut restored = session_on(&restored_endpoint, &case).await;
        assert_eq!(
            ledger_of(&mut restored, &case).await,
            expected_ledger(),
            "case {case:?}: the restored instance must serve exactly the rows the snapshot \
             held"
        );
        // The restored storage is its own: a write to it is invisible to the source, and
        // the source's later row is still invisible here.
        run(
            &mut restored,
            &case,
            "INSERT INTO ledger (id, note) VALUES (7, 'only in the restore')",
        )
        .await;
        let source_ids: Vec<i32> = ledger_of(&mut source, &case)
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            source_ids,
            vec![1, 2, 3, 99],
            "case {case:?}: a write to the restored instance must not reach the source"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_final_snapshot_a_delete_takes_restores_the_rows_it_held() {
    let router = emulator();
    let case = "restore the final snapshot of a deleted instance";
    let source_endpoint = available_instance(&router, "emu-final-source", "postgres", &[]).await;
    let mut source = session_on(&source_endpoint, case).await;
    fill_ledger(&mut source, case).await;

    expect_ok(
        case,
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-final-source",
                "FinalDBSnapshotIdentifier" => "emu-final-snap",
            ],
        )
        .await,
    );
    wait_for_settle();
    // The instance is gone and its snapshot is the only copy of its rows left.
    expect_error(
        case,
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-final-source"],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );

    restore(&router, case, "emu-final-restored", "emu-final-snap", &[]).await;
    wait_for_settle();
    let described = describe(&router, case, "emu-final-restored").await;
    let mut restored = session_on(&endpoint_of(&described), case).await;
    assert_eq!(
        ledger_of(&mut restored, case).await,
        expected_ledger(),
        "case {case:?}: the final snapshot a delete took must restore the instance's rows"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_refuses_a_source_or_a_target_it_cannot_serve() {
    let router = emulator();
    available_instance(&router, "emu-refusal-source", "postgres", &[]).await;
    expect_ok(
        "take the snapshot the refusals name",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-refusal-source",
                "DBSnapshotIdentifier" => "emu-refusal-snap",
            ],
        )
        .await,
    );
    // Asked for while the snapshot is still being taken: the one state refusal, answered
    // before the clock settles the snapshot below.
    expect_error(
        "a snapshot that is still being taken",
        call(
            &router,
            "RestoreDBInstanceFromDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-refusal-early",
                "DBSnapshotIdentifier" => "emu-refusal-snap",
            ],
        )
        .await,
        400,
        "InvalidDBSnapshotState",
    );
    wait_for_settle();

    // Each row: the request, the status and the wire error code it must be refused with.
    let rows = [
        RefusalRow {
            label: "a snapshot this region holds none of",
            request: params![
                "DBInstanceIdentifier" => "emu-refusal-target",
                "DBSnapshotIdentifier" => "emu-no-such-snapshot",
            ]
            .to_vec(),
            status: 404,
            code: "DBSnapshotNotFound",
        },
        RefusalRow {
            label: "a DB cluster snapshot this region holds none of",
            request: params![
                "DBInstanceIdentifier" => "emu-refusal-target",
                "DBClusterSnapshotIdentifier" => "emu-no-such-cluster-snapshot",
            ]
            .to_vec(),
            status: 404,
            code: "DBClusterSnapshotNotFoundFault",
        },
        RefusalRow {
            label: "both sources at once",
            request: params![
                "DBInstanceIdentifier" => "emu-refusal-target",
                "DBSnapshotIdentifier" => "emu-refusal-snap",
                "DBClusterSnapshotIdentifier" => "emu-some-cluster-snapshot",
            ]
            .to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "neither source",
            request: params!["DBInstanceIdentifier" => "emu-refusal-target"].to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "an identifier the region already holds",
            request: params![
                "DBInstanceIdentifier" => "emu-refusal-source",
                "DBSnapshotIdentifier" => "emu-refusal-snap",
            ]
            .to_vec(),
            status: 400,
            code: "DBInstanceAlreadyExists",
        },
        RefusalRow {
            label: "a DB parameter group that does not exist",
            request: params![
                "DBInstanceIdentifier" => "emu-refusal-target",
                "DBSnapshotIdentifier" => "emu-refusal-snap",
                "DBParameterGroupName" => "emu-no-such-parameter-group",
            ]
            .to_vec(),
            status: 404,
            code: "DBParameterGroupNotFound",
        },
    ];
    for row in rows {
        let RefusalRow {
            label,
            request,
            status,
            code,
        } = row;
        expect_error(
            label,
            call(&router, "RestoreDBInstanceFromDBSnapshot", &request).await,
            status,
            code,
        );
        // A refused restore leaves nothing behind: the target identifier is still free.
        expect_error(
            label,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => "emu-refusal-target"],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }
}
