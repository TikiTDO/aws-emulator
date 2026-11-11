// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `ModifyDBCluster` WORDS the modelled state fault it refuses
//! with when the cluster it names is not in a state the modification can be performed in —
//! the refusal two overlapping modifications of one cluster make the loser see.
//!
//! The rule under test: `ModifyDBCluster` refuses a cluster that is not in a state the
//! operation can be performed in with `InvalidDBClusterStateFault` (HTTP 400, the
//! `awsQueryError` code `InvalidDBClusterStateFault` —
//! `smithy_query(query="operation", operation="ModifyDBCluster")`) carrying the sentence
//! that names the STATUS the cluster is in — the one thing that says whether waiting will
//! clear the refusal — and nothing else from the request or the store: not the
//! `DBClusterIdentifier` the request carried, and not the settings it asked to change.
//!
//! The table spans what the rule generalises over: WHICH status blocks the modification
//! (one still coming up, one applying another modification, one whose stop is running, one
//! whose stop has finished, one in a failover and one on its way out), both cluster TYPES
//! `CreateDBCluster` documents across four of the engines `Engine` publishes, the spelling
//! and length of the identifier the request carries (mixed case, and the published
//! 1-to-63/1-to-52-character bound of each cluster type), and which settings the refused
//! modification names (a number, a boolean, a window, a log-export list).
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    db_cluster_identifier_max_len, db_cluster_not_available_for_modification_message,
    DB_CLUSTER_STATUS_AVAILABLE, DB_CLUSTER_STATUS_CREATING, DB_CLUSTER_STATUS_DELETING,
    DB_CLUSTER_STATUS_FAILING_OVER, DB_CLUSTER_STATUS_MODIFYING, DB_CLUSTER_STATUS_STOPPED,
    DB_CLUSTER_STATUS_STOPPING, MIN_FAILOVER_DB_CLUSTER_INSTANCES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, subtrees, text, wait_for_settle, REGION,
};

/// The settings a Multi-AZ DB cluster requires, as `CreateDBCluster` documents them for
/// that cluster type ("This setting is required to create a Multi-AZ DB cluster").
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "100"),
    ("DBClusterInstanceClass", "db.m6gd.xlarge"),
    ("StorageType", "io1"),
    ("Iops", "1000"),
];

/// The `BackupRetentionPeriod` the loser of the overlapping pair names, and the `Port` the
/// WINNER names: the winner's change is one the cluster's instances must be restarted for,
/// which is what leaves the cluster `modifying` for the loser to meet, and the two members
/// differ so the row can check the refused call changed nothing. The winner names a
/// maintenance window beside the port, because a cluster whose engine this emulator serves
/// publishes the port its own engine listener answers on rather than the configured one, so
/// the port it set is not what the cluster reports back.
const WINNING_PORT: &str = "3307";
const WINNING_MAINTENANCE_WINDOW: &str = "sun:06:00-sun:06:30";
const LOSING_RETENTION: &str = "9";

/// How the test puts a cluster into a status the modification cannot be performed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocked {
    /// Straight after `CreateDBCluster`, inside the span the create owes.
    StillCreating,
    /// Another `ModifyDBCluster` is still applying — the overlapping pair.
    ApplyingAModification,
    /// `StopDBCluster` has been accepted and its span has not elapsed.
    Stopping,
    /// The stop has finished and the cluster sits in its settled status.
    Stopped,
    /// `FailoverDBCluster` has been accepted and its span has not elapsed.
    FailingOver,
    /// `DeleteDBCluster` has been accepted and the record is inside the span it owes.
    BeingDeleted,
}

impl Blocked {
    /// The `Status` the cluster reports when the refused modification arrives.
    fn status(self) -> &'static str {
        match self {
            Blocked::StillCreating => DB_CLUSTER_STATUS_CREATING,
            Blocked::ApplyingAModification => DB_CLUSTER_STATUS_MODIFYING,
            Blocked::Stopping => DB_CLUSTER_STATUS_STOPPING,
            Blocked::Stopped => DB_CLUSTER_STATUS_STOPPED,
            Blocked::FailingOver => DB_CLUSTER_STATUS_FAILING_OVER,
            Blocked::BeingDeleted => DB_CLUSTER_STATUS_DELETING,
        }
    }
}

/// One row: a cluster, the status it is in when the modification arrives, and the
/// modification that is refused.
struct RefusalCase {
    case: &'static str,
    /// The `DBClusterIdentifier` the create names; stored folded to lower case.
    identifier: String,
    engine: &'static str,
    /// The parameters the create carries beyond `DBClusterIdentifier` and `Engine`.
    create_extra: Vec<(&'static str, String)>,
    blocked: Blocked,
    /// The settings the REFUSED modification names.
    modification: Vec<(&'static str, String)>,
}

impl RefusalCase {
    /// The member DB instances a failover needs: the writer the role moves FROM and the
    /// replica it moves TO, which is the smallest membership a failover is accepted for.
    fn member_identifiers(&self) -> Vec<String> {
        (0..MIN_FAILOVER_DB_CLUSTER_INSTANCES)
            .map(|index| format!("{}-m{index}", self.identifier))
            .collect()
    }
}

/// `(name, value)` pairs owned so a row can hold them.
fn owned(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs
        .iter()
        .map(|(name, value)| (*name, (*value).to_string()))
        .collect()
}

/// A `DBClusterIdentifier` of exactly the length its member admits for `engine`'s cluster
/// type ("Must contain from 1 to 63 (for Aurora DB clusters) or 1 to 52 (for Multi-AZ DB
/// clusters) letters, numbers, or hyphens"), so the published bound is one of the rows.
fn identifier_at_the_published_bound(engine: &str, prefix: &str) -> String {
    let max = db_cluster_identifier_max_len(engine);
    let mut identifier = prefix.to_string();
    identifier.extend(std::iter::repeat_n('a', max - prefix.len()));
    assert_eq!(
        identifier.len(),
        max,
        "the row at the published bound must be {max} characters, got {}",
        identifier.len()
    );
    identifier
}

/// The table: one row per status that blocks a modification, across both cluster types.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "an Aurora Serverless v2 cluster already applying another modification",
            identifier: "emu-modstate-serverless".to_string(),
            engine: "aurora-postgresql",
            create_extra: owned(&[
                ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
                ("ServerlessV2ScalingConfiguration.MaxCapacity", "1.0"),
            ]),
            blocked: Blocked::ApplyingAModification,
            modification: owned(&[("BackupRetentionPeriod", LOSING_RETENTION)]),
        },        RefusalCase {
            case: "a cluster still coming up, named in mixed case",
            identifier: "EMU-ModState-Creating".to_string(),
            engine: "aurora-mysql",
            create_extra: Vec::new(),
            blocked: Blocked::StillCreating,
            modification: owned(&[("Port", "3307")]),
        },
        RefusalCase {
            case: "a Multi-AZ DB cluster still coming up, at its 52-character bound",
            identifier: identifier_at_the_published_bound("mysql", "emu-modstate-multiaz-"),
            engine: "mysql",
            create_extra: owned(MULTI_AZ_SETTINGS),
            blocked: Blocked::StillCreating,
            modification: owned(&[("BackupRetentionPeriod", "35")]),
        },
        RefusalCase {
            case: "a cluster whose stop is still running",
            identifier: "emu-modstate-stopping".to_string(),
            engine: "aurora-mysql",
            create_extra: Vec::new(),
            blocked: Blocked::Stopping,
            modification: owned(&[("DeletionProtection", "true")]),
        },
        RefusalCase {
            case: "a DocumentDB cluster whose stop has finished",
            identifier: "emu-modstate-stopped".to_string(),
            engine: "docdb",
            create_extra: owned(&[
                ("MasterUsername", "emuadmin"),
                ("MasterUserPassword", "pw1-Secret"),
            ]),
            blocked: Blocked::Stopped,
            modification: owned(&[("PreferredBackupWindow", "04:00-04:30")]),
        },
        RefusalCase {
            case: "a cluster in a failover",
            identifier: "emu-modstate-failover".to_string(),
            engine: "aurora-postgresql",
            create_extra: Vec::new(),
            blocked: Blocked::FailingOver,
            modification: owned(&[(
                "CloudwatchLogsExportConfiguration.EnableLogTypes.member.1",
                "audit",
            )]),
        },
        RefusalCase {
            case: "a cluster on its way out, at its 63-character bound",
            identifier: identifier_at_the_published_bound("aurora-mysql", "emu-modstate-deleting-"),
            engine: "aurora-mysql",
            create_extra: Vec::new(),
            blocked: Blocked::BeingDeleted,
            modification: owned(&[("BackupRetentionPeriod", "3")]),
        },
    ]
}

/// `CreateDBCluster` for one row.
async fn create_cluster(router: &MultiServiceRouter, row: &RefusalCase) -> (u16, String) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.clone()),
        ("Engine", row.engine.to_string()),
    ];
    request.extend(row.create_extra.iter().cloned());
    call(router, "CreateDBCluster", &request).await
}

/// `ModifyDBCluster` naming `identifier` and carrying `settings`.
async fn modify_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    settings: &[(&'static str, String)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> =
        vec![("DBClusterIdentifier", identifier.to_string())];
    request.extend(settings.iter().cloned());
    call(router, "ModifyDBCluster", &request).await
}

/// The one `DBCluster` subtree `DescribeDBClusters` reports for `identifier`.
async fn describe_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    let found = subtrees(&body, "DBCluster");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBCluster, got {} in {body}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

#[tokio::test]
async fn a_modification_of_a_cluster_in_a_blocking_state_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    let cases = refusal_cases();

    // Every cluster whose blocking call needs it `available` first, plus the member
    // instances a failover has to have to move the writer role between. One wait settles
    // them all.
    for row in &cases {
        if row.blocked == Blocked::StillCreating {
            continue;
        }
        expect_ok(row.case, create_cluster(&router, row).await);
        if row.blocked == Blocked::FailingOver {
            for (index, member) in row.member_identifiers().iter().enumerate() {
                let zone = char::from(b'a' + u8::try_from(index).unwrap_or(0));
                expect_ok(
                    row.case,
                    call(
                        &router,
                        "CreateDBInstance",
                        &[
                            ("DBInstanceIdentifier", member.as_str()),
                            ("Engine", row.engine),
                            ("DBInstanceClass", "db.serverless"),
                            ("DBClusterIdentifier", row.identifier.as_str()),
                            ("AvailabilityZone", format!("{REGION}{zone}").as_str()),
                        ],
                    )
                    .await,
                );
            }
        }
    }
    wait_for_settle();

    // The stops that must have FINISHED when the modification arrives, settled together.
    let mut stopped_any = false;
    for row in &cases {
        if row.blocked == Blocked::Stopped {
            expect_ok(
                row.case,
                call(
                    &router,
                    "StopDBCluster",
                    &[("DBClusterIdentifier", row.identifier.as_str())],
                )
                .await,
            );
            stopped_any = true;
        }
    }
    if stopped_any {
        wait_for_settle();
    }

    // One row at a time, with no wait inside the loop, so the transient status each
    // blocking call leaves is still the one the refused modification meets.
    for row in &cases {
        let case = row.case;
        match row.blocked {
            Blocked::StillCreating => {
                expect_ok(case, create_cluster(&router, row).await);
            }
            Blocked::ApplyingAModification => {
                expect_ok(
                    case,
                    modify_cluster(
                        &router,
                        &row.identifier,
                        &owned(&[
                            ("Port", WINNING_PORT),
                            ("PreferredMaintenanceWindow", WINNING_MAINTENANCE_WINDOW),
                        ]),
                    )
                    .await,
                );
            }
            Blocked::Stopping => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "StopDBCluster",
                        &[("DBClusterIdentifier", row.identifier.as_str())],
                    )
                    .await,
                );
            }
            Blocked::Stopped => {}
            Blocked::FailingOver => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "FailoverDBCluster",
                        &[("DBClusterIdentifier", row.identifier.as_str())],
                    )
                    .await,
                );
            }
            Blocked::BeingDeleted => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "DeleteDBCluster",
                        &[
                            ("DBClusterIdentifier", row.identifier.as_str()),
                            ("SkipFinalSnapshot", "true"),
                        ],
                    )
                    .await,
                );
            }
        }

        // The cluster really is in the status this row is about: the refusal below is the
        // state precondition answering, not some other check.
        let reported = describe_cluster(&router, case, &row.identifier).await;
        let status = text(&reported, "Status");
        assert_eq!(
            status,
            row.blocked.status(),
            "case {case:?}: expected the cluster to be {:?} when the modification arrives, \
             got {status:?}",
            row.blocked.status()
        );

        let refused = expect_error(
            case,
            modify_cluster(&router, &row.identifier, &row.modification).await,
            400,
            "InvalidDBClusterStateFault",
        );
        let message = text(&refused, "Message");
        let expected = db_cluster_not_available_for_modification_message(row.blocked.status());
        assert_eq!(
            message, expected,
            "case {case:?}: expected the wording {expected:?}, got {message:?}"
        );
        let folded = row.identifier.to_ascii_lowercase();
        assert!(
            !message.to_ascii_lowercase().contains(&folded),
            "case {case:?}: the refusal must not interpolate the cluster identifier, got \
             {message:?}"
        );
        assert!(
            message.contains(row.blocked.status()),
            "case {case:?}: the refusal must name the cluster status {:?}, got {message:?}",
            row.blocked.status()
        );

        // Nothing the refused modification named reached the store: it is refused on the
        // cluster's STATE, and a refusal changes nothing a later caller sees.
        let after = describe_cluster(&router, case, &row.identifier).await;
        let status_after = text(&after, "Status");
        assert_eq!(
            status_after,
            row.blocked.status(),
            "case {case:?}: expected the refused modification to leave the status alone, \
             got {status_after:?}"
        );
    }

    // The refusal was about the STATE: once the clock has settled the overlapping pair's
    // cluster, the winner's value is the one it kept and a further modification is taken.
    wait_for_settle();
    let overlapping = cases
        .iter()
        .find(|row| row.blocked == Blocked::ApplyingAModification)
        .expect("the table carries an overlapping-modification row");
    let settled = describe_cluster(&router, overlapping.case, &overlapping.identifier).await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, DB_CLUSTER_STATUS_AVAILABLE,
        "case {:?}: expected the cluster to settle back to {DB_CLUSTER_STATUS_AVAILABLE}, \
         got {status:?}",
        overlapping.case
    );
    let retention = text(&settled, "BackupRetentionPeriod");
    assert_ne!(
        retention, LOSING_RETENTION,
        "case {:?}: the refused modification must have written nothing, got {retention:?}",
        overlapping.case
    );
    let window = text(&settled, "PreferredMaintenanceWindow");
    assert_eq!(
        window, WINNING_MAINTENANCE_WINDOW,
        "case {:?}: expected the accepted modification's value \
         {WINNING_MAINTENANCE_WINDOW:?} to be the one the cluster kept, got {window:?}",
        overlapping.case
    );
    let accepted = expect_ok(
        overlapping.case,
        modify_cluster(&router, &overlapping.identifier, &overlapping.modification).await,
    );
    // The same modification is taken now the cluster is available — and since it needs no
    // downtime, it leaves the cluster available rather than moving it.
    let status = text(&accepted, "Status");
    assert_eq!(
        status, DB_CLUSTER_STATUS_AVAILABLE,
        "case {:?}: expected the same modification to be taken once the cluster is \
         available, got {status:?}",
        overlapping.case
    );
}
