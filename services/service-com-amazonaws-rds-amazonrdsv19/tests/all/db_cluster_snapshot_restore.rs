// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreDBClusterFromSnapshot`.
//!
//! The rule under test: restoring a snapshot creates a NEW DB cluster whose configuration is
//! the SOURCE's as the snapshot copied it — overridden, member by member, by the ones this
//! request names — whose storage holds the DATA the snapshot captured, and which is answered
//! `creating` and reaches `available` on its own; every resource the request names is
//! resolved first, and a restore that is refused leaves nothing behind.
//!
//! Every control-plane call goes over the wire (the awsQuery form POST the router keys on)
//! and every data-plane statement over the PostgreSQL wire, through the client the
//! engine-endpoint suite drives — so what these tests exercise is the protocol a stock
//! driver speaks, in one process.
//!
//! The tables span what the rule generalises over: both cluster TYPES (an Aurora DB cluster
//! and a Multi-AZ DB cluster), the members a restore may override and the ones it inherits,
//! both forms a DB cluster snapshot may be named by (identifier and ARN), the DB snapshot a
//! restore may name by ARN instead, and each refusal class the operation answers — the two
//! snapshot 404s, the snapshot state fault, the already-exists conflict, the two referenced
//! groups' 404s and the parameter refusals.

use crate::db_cluster_engine_endpoint::MULTI_AZ_SETTINGS;
use crate::db_clusters::describe_one;
use crate::db_instance_engine_endpoint::{DATABASE, MASTER_PASSWORD, MASTER_USER};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle,
};
// The ledger the DB SNAPSHOT restore suite writes and reads over the PostgreSQL wire: the
// rule about what a restore brings back is the same one for a cluster's storage, so this
// suite drives that same table rather than keeping a second copy of the harness.
use crate::db_snapshot_restore::{expected_ledger, fill_ledger, ledger_of, run, session_on};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One source cluster the override table is run against: one of the two cluster TYPES.
struct SourceCluster {
    label: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The members this cluster's own type requires beyond the shared ones.
    extra: &'static [(&'static str, &'static str)],
    /// What the snapshot of this cluster must carry over to a restore that overrides
    /// nothing: `(member, value)` pairs read off the restored cluster.
    inherited: &'static [(&'static str, &'static str)],
}

/// One row of the override table: what the restore asks for beyond the snapshot, and what
/// the restored cluster must then report.
struct OverrideRow {
    label: &'static str,
    overrides: Params,
    expected: Params,
}

/// One row of the refusal table: the request, and the status and wire error `<Code>` it must
/// be refused with.
struct RefusalRow {
    label: &'static str,
    request: Params,
    status: u16,
    code: &'static str,
}

/// The two cluster types a restore serves, each created with the settings its own type
/// requires.
const SOURCES: &[SourceCluster] = &[
    SourceCluster {
        label: "an Aurora DB cluster",
        identifier: "emu-restore-src-aurora",
        engine: "aurora-postgresql",
        extra: &[],
        // An Aurora cluster's storage "adjusts as needed", so it reports the one unit of
        // allocated storage this service publishes for such a cluster.
        inherited: &[("AllocatedStorage", "1")],
    },
    SourceCluster {
        label: "a Multi-AZ DB cluster",
        identifier: "emu-restore-src-multiaz",
        engine: "postgres",
        extra: MULTI_AZ_SETTINGS,
        // The storage and the instance class the snapshot copied off the Multi-AZ cluster,
        // which the restore names neither of.
        inherited: &[
            ("AllocatedStorage", "400"),
            ("DBClusterInstanceClass", "db.m5d.large"),
            ("StorageType", "gp3"),
        ],
    },
];

/// Create `identifier` as a cluster of `engine` with the master credential these tests log in
/// with, wait for it to settle, and answer with the envelope `DescribeDBClusters` reports.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut request: Vec<(&str, String)> = params![
        "DBClusterIdentifier" => identifier,
        "Engine" => engine,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
        "DatabaseName" => DATABASE,
        "BackupRetentionPeriod" => 5,
        "PreferredBackupWindow" => "03:00-03:30",
        "Tags.Tag.1.Key" => "owner",
        "Tags.Tag.1.Value" => "the cluster restore suite",
    ]
    .to_vec();
    request.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    expect_ok(case, call(router, "CreateDBCluster", &request).await);
    wait_for_settle();
    describe_one(router, case, identifier).await
}

/// Take a snapshot of `cluster` under `snapshot`, and wait for it to settle.
async fn snapshot_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    cluster: &str,
    snapshot: &str,
) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBClusterSnapshot",
            &params![
                "DBClusterIdentifier" => cluster,
                "DBClusterSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// The ARN `DescribeDBClusterSnapshots` reports for one cluster snapshot.
async fn snapshot_arn(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    snapshot: &str,
) -> String {
    text(
        &expect_ok(
            case,
            call(
                router,
                "DescribeDBClusterSnapshots",
                &params!["DBClusterSnapshotIdentifier" => snapshot],
            )
            .await,
        ),
        "DBClusterSnapshotArn",
    )
}

/// Restore `snapshot` into a new cluster named `identifier`, with `extra` as the members the
/// request overrides, and answer with the envelope the restore returned.
async fn restore(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    snapshot: &str,
    extra: &[(&'static str, String)],
) -> String {
    let mut request: Vec<(&str, String)> = params![
        "DBClusterIdentifier" => identifier,
        "SnapshotIdentifier" => snapshot,
        "Engine" => engine,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    expect_ok(
        case,
        call(router, "RestoreDBClusterFromSnapshot", &request).await,
    )
}

/// The address a client connects to, as a CLUSTER envelope publishes it: a cluster reports
/// the host it is reached at and the port beside it, where a DB instance reports an
/// `Endpoint` structure carrying both.
fn cluster_endpoint(case: &str, envelope: &str) -> (String, u16) {
    let port = text(envelope, "Port");
    (
        text(envelope, "Endpoint"),
        port.parse().unwrap_or_else(|_| {
            panic!("case {case:?}: the cluster must publish a port, got {port:?}")
        }),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_cluster_carries_the_snapshots_settings_and_the_requests_overrides() {
    let router = emulator();
    for source in SOURCES {
        let snapshot = format!("{}-snap", source.identifier);
        let created = available_cluster(
            &router,
            source.label,
            source.identifier,
            source.engine,
            source.extra,
        )
        .await;
        let source_version = text(&created, "EngineVersion");
        snapshot_cluster(&router, source.label, source.identifier, &snapshot).await;

        // Each row names what the request overrides and what the restored cluster must then
        // report; the members no row names are the snapshot's, which the first row asserts.
        let mut rows = vec![
            OverrideRow {
                label: "nothing overridden",
                overrides: Vec::new(),
                expected: {
                    let mut expected: Params = params![
                        "Engine" => source.engine,
                        "EngineVersion" => source_version,
                        "DatabaseName" => DATABASE,
                        "MasterUsername" => MASTER_USER,
                        "BackupRetentionPeriod" => 5,
                        "PreferredBackupWindow" => "03:00-03:30",
                        "Status" => "available",
                        "DeletionProtection" => "false",
                    ]
                    .to_vec();
                    expected.extend(
                        source
                            .inherited
                            .iter()
                            .map(|(member, value)| (*member, (*value).to_string())),
                    );
                    expected
                },
            },
            OverrideRow {
                label: "the backup settings replaced",
                overrides: params![
                    "BackupRetentionPeriod" => 12,
                    "PreferredBackupWindow" => "11:00-11:30",
                ]
                .to_vec(),
                expected: params![
                    "BackupRetentionPeriod" => 12,
                    "PreferredBackupWindow" => "11:00-11:30",
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "a database name of its own, deletion protection and tag copying",
                overrides: params![
                    "DatabaseName" => "restored",
                    "DeletionProtection" => true,
                    "CopyTagsToSnapshot" => true,
                ]
                .to_vec(),
                expected: params![
                    "DatabaseName" => "restored",
                    "DeletionProtection" => "true",
                    "CopyTagsToSnapshot" => "true",
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "IAM database authentication and a log export turned on",
                overrides: params![
                    "EnableIAMDatabaseAuthentication" => true,
                    "EnableCloudwatchLogsExports.member.1" => "postgresql",
                ]
                .to_vec(),
                expected: params![
                    "IAMDatabaseAuthenticationEnabled" => "true",
                    // The one entry of `EnabledCloudwatchLogsExports`, which the awsQuery
                    // wire names `member`.
                    "member" => "postgresql",
                ]
                .to_vec(),
            },
            OverrideRow {
                label: "tags of its own, which replace the ones it would inherit",
                overrides: params![
                    "Tags.Tag.1.Key" => "purpose",
                    "Tags.Tag.1.Value" => "the restore",
                ]
                .to_vec(),
                expected: params!["Key" => "purpose", "Value" => "the restore"].to_vec(),
            },
        ];
        // One row per cluster TYPE, for a member only that type has: an Aurora cluster takes
        // its network type, a Multi-AZ DB cluster the instance class of its members.
        rows.push(if source.engine == "postgres" {
            OverrideRow {
                label: "a larger instance class for the cluster's instances",
                overrides: params!["DBClusterInstanceClass" => "db.m5d.4xlarge"].to_vec(),
                expected: params!["DBClusterInstanceClass" => "db.m5d.4xlarge"].to_vec(),
            }
        } else {
            OverrideRow {
                label: "a dual-stack network",
                overrides: params!["NetworkType" => "DUAL"].to_vec(),
                expected: params!["NetworkType" => "DUAL"].to_vec(),
            }
        });

        for (index, row) in rows.into_iter().enumerate() {
            let OverrideRow {
                label,
                overrides,
                expected,
            } = row;
            let case = format!("{}: {label}", source.label);
            let target = format!("emu-restored-{}-{index}", source.engine.replace('-', ""));
            let restored = restore(
                &router,
                &case,
                &target,
                source.engine,
                &snapshot,
                &overrides,
            )
            .await;
            assert_eq!(
                text(&restored, "Status"),
                "creating",
                "case {case:?}: a restored cluster is still being created when the call \
                 returns, got {restored}"
            );
            // The restored cluster is its OWN resource: its own immutable resource id, never
            // the source's.
            assert_ne!(
                text(&restored, "DbClusterResourceId"),
                text(&created, "DbClusterResourceId"),
                "case {case:?}: a restored cluster gets its own DbClusterResourceId, got the \
                 source's in {restored}"
            );
            wait_for_settle();
            let described = describe_one(&router, &case, &target).await;
            for (member, value) in &expected {
                let actual = optional_text(&described, member);
                assert_eq!(
                    actual.as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected {member} to be {value:?}, got {actual:?} in \
                     {described}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_cluster_serves_the_rows_the_snapshot_held() {
    let router = emulator();
    let case = "restore the rows a CreateDBClusterSnapshot captured";
    let created = available_cluster(
        &router,
        case,
        "emu-cluster-rows-source",
        "aurora-postgresql",
        &[],
    )
    .await;
    let source_endpoint = cluster_endpoint(case, &created);
    let mut source = session_on(&source_endpoint, case).await;
    fill_ledger(&mut source, case).await;

    snapshot_cluster(&router, case, "emu-cluster-rows-source", "emu-cluster-rows-snap").await;
    // Written AFTER the snapshot was taken, so it is not part of what the snapshot holds: a
    // restore brings back the rows of that instant, not the source's rows now.
    run(
        &mut source,
        case,
        "INSERT INTO ledger (id, note) VALUES (99, 'after the snapshot')",
    )
    .await;

    // Both forms `SnapshotIdentifier` accepts for a DB cluster snapshot name the same
    // snapshot: "either the name or the Amazon Resource Name (ARN)".
    let arn = snapshot_arn(&router, case, "emu-cluster-rows-snap").await;
    for (label, named) in [
        ("by identifier", "emu-cluster-rows-snap".to_string()),
        ("by ARN", arn),
    ] {
        let case = format!("{case} {label}");
        let target = format!("emu-cluster-rows-restored-{}", label.replace(' ', ""));
        restore(
            &router,
            &case,
            &target,
            "aurora-postgresql",
            &named,
            &[],
        )
        .await;
        wait_for_settle();
        let described = describe_one(&router, &case, &target).await;
        let restored_endpoint = cluster_endpoint(&case, &described);
        assert_ne!(
            restored_endpoint.1, source_endpoint.1,
            "case {case:?}: the restored cluster has an engine endpoint of its own, got the \
             source's port"
        );
        // The master credential is the SOURCE's, as the snapshot copied it: the restore mints
        // none, so this login is proof the snapshot carried it.
        let mut restored = session_on(&restored_endpoint, &case).await;
        assert_eq!(
            ledger_of(&mut restored, &case).await,
            expected_ledger(),
            "case {case:?}: the restored cluster must serve exactly the rows the snapshot held"
        );
        // The restored storage is its own: a write to it is invisible to the source, and the
        // source's later row is still invisible here.
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
            "case {case:?}: a write to the restored cluster must not reach the source"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_db_snapshot_named_by_arn_restores_into_a_cluster() {
    let router = emulator();
    let case = "restore an RDS for PostgreSQL DB snapshot into an Aurora PostgreSQL cluster";
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-cluster-src-instance",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
                "DBName" => DATABASE,
                "BackupRetentionPeriod" => 3,
            ],
        )
        .await,
    );
    wait_for_settle();
    let snapshot = expect_ok(
        case,
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-cluster-src-instance",
                "DBSnapshotIdentifier" => "emu-cluster-src-instance-snap",
            ],
        )
        .await,
    );
    let arn = text(&snapshot, "DBSnapshotArn");
    wait_for_settle();

    // "You can use only the ARN to specify a DB snapshot": the identifier alone names no DB
    // snapshot here, so it is the DB CLUSTER snapshot's own 404.
    expect_error(
        case,
        call(
            &router,
            "RestoreDBClusterFromSnapshot",
            &params![
                "DBClusterIdentifier" => "emu-cluster-from-db-snapshot",
                "SnapshotIdentifier" => "emu-cluster-src-instance-snap",
                "Engine" => "aurora-postgresql",
            ],
        )
        .await,
        404,
        "DBClusterSnapshotNotFoundFault",
    );
    let restored = restore(
        &router,
        case,
        "emu-cluster-from-db-snapshot",
        "aurora-postgresql",
        &arn,
        &[],
    )
    .await;
    assert_eq!(
        text(&restored, "Status"),
        "creating",
        "case {case:?}: a restored cluster is still being created when the call returns, got \
         {restored}"
    );
    wait_for_settle();
    let described = describe_one(&router, case, "emu-cluster-from-db-snapshot").await;
    for (member, value) in [
        ("Engine", "aurora-postgresql"),
        ("DatabaseName", DATABASE),
        ("MasterUsername", MASTER_USER),
        // "Default: Uses existing setting" — the retention the DB snapshot recorded.
        ("BackupRetentionPeriod", "3"),
        ("Status", "available"),
    ] {
        let actual = optional_text(&described, member);
        assert_eq!(
            actual.as_deref(),
            Some(value),
            "case {case:?}: expected {member} to be {value:?}, got {actual:?} in {described}"
        );
    }
    // An engine of ANOTHER database product is not compatible with the snapshot's.
    expect_error(
        "an engine of another product",
        call(
            &router,
            "RestoreDBClusterFromSnapshot",
            &params![
                "DBClusterIdentifier" => "emu-cluster-wrong-product",
                "SnapshotIdentifier" => arn,
                "Engine" => "aurora-mysql",
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_refuses_a_source_or_a_target_it_cannot_serve() {
    let router = emulator();
    available_cluster(
        &router,
        "the cluster the refusals name",
        "emu-cluster-refusal-src",
        "aurora-postgresql",
        &[],
    )
    .await;
    expect_ok(
        "take the snapshot the refusals name",
        call(
            &router,
            "CreateDBClusterSnapshot",
            &params![
                "DBClusterIdentifier" => "emu-cluster-refusal-src",
                "DBClusterSnapshotIdentifier" => "emu-cluster-refusal-snap",
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
            "RestoreDBClusterFromSnapshot",
            &params![
                "DBClusterIdentifier" => "emu-cluster-refusal-early",
                "SnapshotIdentifier" => "emu-cluster-refusal-snap",
                "Engine" => "aurora-postgresql",
            ],
        )
        .await,
        400,
        "InvalidDBClusterSnapshotStateFault",
    );
    wait_for_settle();

    // Each row: the request, the status and the wire error code it must be refused with.
    let rows = [
        RefusalRow {
            label: "a cluster snapshot this region holds none of",
            request: params!["SnapshotIdentifier" => "emu-no-such-cluster-snapshot"].to_vec(),
            status: 404,
            code: "DBClusterSnapshotNotFoundFault",
        },
        RefusalRow {
            label: "a DB snapshot ARN this region holds none of",
            request: params![
                "SnapshotIdentifier" =>
                    "arn:aws:rds:us-west-2:000000000000:snapshot:emu-no-such-db-snapshot",
            ]
            .to_vec(),
            status: 404,
            code: "DBSnapshotNotFound",
        },
        RefusalRow {
            label: "a DB subnet group that does not exist",
            request: params!["DBSubnetGroupName" => "emu-no-such-subnet-group"].to_vec(),
            status: 404,
            code: "DBSubnetGroupNotFoundFault",
        },
        RefusalRow {
            label: "a DB cluster parameter group that does not exist",
            request: params!["DBClusterParameterGroupName" => "emu-no-such-cluster-group"]
                .to_vec(),
            status: 404,
            code: "DBClusterParameterGroupNotFound",
        },
        RefusalRow {
            label: "a port below the published range",
            request: params!["Port" => 1149].to_vec(),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            label: "a backup retention period past the published maximum",
            request: params!["BackupRetentionPeriod" => 36].to_vec(),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            label: "a backtrack window on an engine that cannot backtrack",
            request: params!["BacktrackWindow" => 3600].to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "a storage type the cluster's engine does not take",
            request: params!["StorageType" => "io1"].to_vec(),
            status: 400,
            code: "StorageTypeNotSupported",
        },
        RefusalRow {
            label: "an instance class on an Aurora DB cluster",
            request: params!["DBClusterInstanceClass" => "db.m5d.large"].to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "one half of the VPC networking pair",
            request: params!["EnableVPCNetworking" => false].to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "the VPC networking pair without IAM database authentication",
            request: params![
                "EnableVPCNetworking" => false,
                "EnableInternetAccessGateway" => true,
            ]
            .to_vec(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            label: "an identifier the region already holds",
            request: params!["DBClusterIdentifier" => "emu-cluster-refusal-src"].to_vec(),
            status: 400,
            code: "DBClusterAlreadyExistsFault",
        },
    ];
    for row in rows {
        let RefusalRow {
            label,
            request,
            status,
            code,
        } = row;
        // The request every row starts from — a restore that would otherwise succeed — with
        // the row's own members replacing it member by member.
        let mut full: Params = params![
            "DBClusterIdentifier" => "emu-cluster-refusal-target",
            "SnapshotIdentifier" => "emu-cluster-refusal-snap",
            "Engine" => "aurora-postgresql",
        ]
        .to_vec();
        for (member, value) in request {
            match full.iter_mut().find(|(named, _)| *named == member) {
                Some(entry) => entry.1 = value,
                None => full.push((member, value)),
            }
        }
        expect_error(
            label,
            call(&router, "RestoreDBClusterFromSnapshot", &full).await,
            status,
            code,
        );
        // A refused restore leaves nothing behind: the target identifier is still free.
        expect_error(
            label,
            call(
                &router,
                "DescribeDBClusters",
                &params!["DBClusterIdentifier" => "emu-cluster-refusal-target"],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_that_turns_vpc_networking_off_reports_its_internet_access_gateway() {
    let router = emulator();
    let case = "restore an Aurora PostgreSQL cluster without a VPC network interface";
    available_cluster(
        &router,
        case,
        "emu-cluster-eni-source",
        "aurora-postgresql",
        &[],
    )
    .await;
    snapshot_cluster(&router, case, "emu-cluster-eni-source", "emu-cluster-eni-snap").await;

    // A restore that names neither member reaches its VPC's network and nothing else.
    let plain = restore(
        &router,
        case,
        "emu-cluster-eni-vpc",
        "aurora-postgresql",
        "emu-cluster-eni-snap",
        &[],
    )
    .await;
    // The two members the pair sets, with IAM database authentication, which they require.
    let gateway = restore(
        &router,
        case,
        "emu-cluster-eni-internet",
        "aurora-postgresql",
        "emu-cluster-eni-snap",
        &params![
            "EnableVPCNetworking" => false,
            "EnableInternetAccessGateway" => true,
            "EnableIAMDatabaseAuthentication" => true,
        ],
    )
    .await;
    for (label, envelope, expected) in [
        ("neither member named", &plain, ["true", "false"]),
        ("the pair named", &gateway, ["false", "true"]),
    ] {
        for (member, value) in ["VPCNetworkingEnabled", "InternetAccessGatewayEnabled"]
            .iter()
            .zip(expected)
        {
            let actual = optional_text(envelope, member);
            assert_eq!(
                actual.as_deref(),
                Some(value),
                "case {case:?} with {label}: expected {member} to be {value:?}, got \
                 {actual:?} in {envelope}"
            );
        }
    }
}
