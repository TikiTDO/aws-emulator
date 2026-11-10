// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PromoteReadReplicaDBCluster`: the operation that "promotes a read
//! replica DB cluster to a standalone DB cluster".
//!
//! One rule is pinned here. A DB cluster can be promoted exactly while it IS a read replica
//! — it carries the replication source `CreateDBCluster$ReplicationSourceIdentifier` gave it
//! — and is up; the promotion clears that source, so the cluster replicates from nothing and
//! the source stops reporting it in `ReadReplicaIdentifiers`, it reports the transient
//! `promoting` status and reaches `available` on its own. A `DBClusterIdentifier` naming no
//! cluster of the region is the modelled `DBClusterNotFoundFault` (HTTP 404), and a cluster
//! that carries no replication source — never a replica, or already promoted — or that is not
//! up is the modelled `InvalidDBClusterStateFault` (HTTP 400), which leaves the cluster as it
//! was.
//!
//! The success side is asserted over a TABLE spanning the class the rule generalises over:
//! the two kinds of source the member accepts ("the Amazon Resource Name (ARN) of the source
//! DB instance or DB cluster"), two Aurora engines, and the three forms a member that names a
//! DB cluster accepts ("the user-supplied DB cluster identifier or the Amazon Resource Name
//! (ARN)", "this parameter isn't case-sensitive"). The refusal side spans the three states no
//! promotion can be performed in, and the three forms an absent identifier may be written in.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_cluster_subject_operations::{cluster_arn, create_available_cluster};
use crate::db_clusters::describe_one;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// An identifier no call below ever creates a cluster for.
const ABSENT: &str = "emu-promote-absent";

/// What a read replica cluster replicates FROM: `ReplicationSourceIdentifier` is "the Amazon
/// Resource Name (ARN) of the source DB instance or DB cluster if this DB cluster is created
/// as a read replica", so both kinds are rows of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Another DB cluster of the same region.
    Cluster,
    /// A DB instance of the same region.
    Instance,
}

/// The replica's identifier as written by the request: the spelling it is stored under.
fn as_written(identifier: &str) -> String {
    identifier.to_string()
}

/// The same identifier upper-cased — "this parameter isn't case-sensitive".
fn upper_cased(identifier: &str) -> String {
    identifier.to_ascii_uppercase()
}

/// The cluster's ARN form, the other form every member that names a DB cluster accepts.
fn arn_form(identifier: &str) -> String {
    cluster_arn(identifier)
}

/// One promotion row: the case, the kind of source the replica was created from, the engine
/// of the clusters, the two identifiers, and how the promote writes the replica's.
struct PromotionCase {
    case: &'static str,
    source: Source,
    engine: &'static str,
    source_identifier: &'static str,
    replica: &'static str,
    names_replica_by: fn(&str) -> String,
}

/// The class the promotion rule generalises over.
const PROMOTIONS: &[PromotionCase] = &[
    PromotionCase {
        case: "a replica of a cluster, named by identifier",
        source: Source::Cluster,
        engine: "aurora-mysql",
        source_identifier: "emu-promote-cluster-source",
        replica: "emu-promote-cluster-replica",
        names_replica_by: as_written,
    },
    PromotionCase {
        case: "a replica of a cluster on another engine, named by an upper-cased identifier",
        source: Source::Cluster,
        engine: "aurora-postgresql",
        source_identifier: "emu-promote-pg-source",
        replica: "emu-promote-pg-replica",
        names_replica_by: upper_cased,
    },
    PromotionCase {
        case: "a replica of a DB instance, named by the cluster's ARN",
        source: Source::Instance,
        engine: "aurora-mysql",
        source_identifier: "emu-promote-instance-source",
        replica: "emu-promote-instance-replica",
        names_replica_by: arn_form,
    },
];

/// `PromoteReadReplicaDBCluster` naming one cluster, however `named` spells it.
async fn promote(router: &MultiServiceRouter, named: &str) -> (u16, String) {
    call(
        router,
        "PromoteReadReplicaDBCluster",
        &[("DBClusterIdentifier", named)],
    )
    .await
}

/// `CreateDBCluster` for a read replica of `source`, without waiting out the span the create
/// owes, so a row can promote a cluster that is still coming up.
async fn create_replica(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    source: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBCluster",
        &[
            ("DBClusterIdentifier", identifier),
            ("Engine", engine),
            ("ReplicationSourceIdentifier", source),
        ],
    )
    .await
}

/// The sentence Amazon RDS answers a promotion of a cluster that is no read replica with. It
/// is FIXED text: neither the identifier the request carried nor the cluster's status is in
/// it.
const NOT_A_READ_REPLICA: &str = "You cannot promote a DB cluster that is not a read replica.";

/// Assert the modelled state fault for a cluster that carries no replication source at all.
async fn expect_not_a_replica_refusal(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let refused = expect_error(
        case,
        promote(router, identifier).await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message, NOT_A_READ_REPLICA,
        "case {case:?}: expected the message {NOT_A_READ_REPLICA:?}, got {message:?}"
    );
}

/// Assert the modelled state fault for a read replica whose lifecycle status cannot take a
/// promotion: the refusal names the cluster as the request wrote it and `because`, the part
/// of the sentence that says which status that is.
async fn expect_status_refusal(
    router: &MultiServiceRouter,
    case: &str,
    named: &str,
    because: &str,
) {
    let refused = expect_error(
        case,
        promote(router, named).await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    assert!(
        message.contains(named) && message.contains(because),
        "case {case:?}: expected the refusal to name the cluster as {named:?} and {because:?}, \
         got {message:?}"
    );
}

#[tokio::test]
async fn a_read_replica_db_cluster_is_promoted_to_a_standalone_cluster() {
    let router = emulator();

    for row in PROMOTIONS {
        let case = row.case;
        // The source the replica replicates from, and the ARN the replica reports for it:
        // the member is resolved to the ARN of the record it names, whichever kind that is.
        let source_arn = match row.source {
            Source::Cluster => {
                create_available_cluster(&router, row.source_identifier, row.engine, &[]).await;
                cluster_arn(row.source_identifier)
            }
            Source::Instance => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "CreateDBInstance",
                        &[
                            ("DBInstanceIdentifier", row.source_identifier),
                            ("Engine", "mysql"),
                            ("DBInstanceClass", "db.t3.micro"),
                            ("AllocatedStorage", "20"),
                        ],
                    )
                    .await,
                );
                wait_for_settle();
                format!(
                    "arn:aws:rds:{REGION}:{ACCOUNT}:db:{}",
                    row.source_identifier
                )
            }
        };
        expect_ok(
            case,
            create_replica(&router, row.replica, row.engine, row.source_identifier).await,
        );
        wait_for_settle();

        let replica = describe_one(&router, case, row.replica).await;
        assert_eq!(
            optional_text(&replica, "ReplicationSourceIdentifier").as_deref(),
            Some(source_arn.as_str()),
            "case {case:?}: expected the replica to name the source it replicates from, got \
             {replica}"
        );
        if row.source == Source::Cluster {
            let source = describe_one(&router, case, row.source_identifier).await;
            let replicas = texts(&source, "ReadReplicaIdentifier");
            assert!(
                replicas.iter().any(|named| named == row.replica),
                "case {case:?}: expected the source to report the replica before the \
                 promotion, got {replicas:?}"
            );
        }

        let named = (row.names_replica_by)(row.replica);
        let promoted = expect_ok(case, promote(&router, &named).await);
        let status = text(&promoted, "Status");
        assert_eq!(
            status, "promoting",
            "case {case:?}: expected the accepted promotion to report the transient status, \
             got {status:?} in {promoted}"
        );
        let source_of_promoted = optional_text(&promoted, "ReplicationSourceIdentifier");
        assert_eq!(
            source_of_promoted, None,
            "case {case:?}: expected the promoted cluster to replicate from nothing, got \
             {source_of_promoted:?} in {promoted}"
        );

        // Inside the span the promotion owes, the promotion is what a read of the cluster
        // reports — and the source already reports no replica, because the membership is read
        // off these records rather than copied onto them.
        let reading = describe_one(&router, case, row.replica).await;
        let status = text(&reading, "Status");
        assert_eq!(
            status, "promoting",
            "case {case:?}: expected the read inside the span to report the transient status, \
             got {status:?}"
        );
        let source_of_reading = optional_text(&reading, "ReplicationSourceIdentifier");
        assert_eq!(
            source_of_reading, None,
            "case {case:?}: expected the read inside the span to report no replication \
             source, got {source_of_reading:?}"
        );
        if row.source == Source::Cluster {
            let source = describe_one(&router, case, row.source_identifier).await;
            let replicas = texts(&source, "ReadReplicaIdentifier");
            assert!(
                !replicas.iter().any(|named| named == row.replica),
                "case {case:?}: expected the source to stop reporting the promoted cluster, \
                 got {replicas:?}"
            );
        }

        wait_for_settle();
        let settled = describe_one(&router, case, row.replica).await;
        let status = text(&settled, "Status");
        assert_eq!(
            status, "available",
            "case {case:?}: expected the cluster to reach available on its own, got {status:?}"
        );
        let source_of_settled = optional_text(&settled, "ReplicationSourceIdentifier");
        assert_eq!(
            source_of_settled, None,
            "case {case:?}: expected the standalone cluster to stay standalone, got \
             {source_of_settled:?}"
        );

        // A cluster this promotion already made standalone is no read replica, so a second
        // promotion of it is the modelled state fault rather than a promotion that changes
        // nothing.
        expect_not_a_replica_refusal(&router, case, row.replica).await;
    }
}

#[tokio::test]
async fn a_promotion_of_a_cluster_it_cannot_be_performed_on_is_refused() {
    let router = emulator();
    // A cluster stored throughout, so a 404 below is a decision about the identifier the
    // request named and never "this service holds no cluster at all". It replicates from
    // nothing, which is also the first state row.
    create_available_cluster(&router, "emu-promote-standalone", "aurora-postgresql", &[]).await;

    // The three forms a member that names a DB cluster accepts, all naming nothing.
    for named in [
        ABSENT.to_string(),
        ABSENT.to_ascii_uppercase(),
        cluster_arn(ABSENT),
    ] {
        let case = format!("an absent cluster named {named:?}");
        let body = expect_error(
            &case,
            promote(&router, &named).await,
            404,
            "DBClusterNotFoundFault",
        );
        let message = text(&body, "Message");
        let expected =
            format!("The source cluster could not be found or cannot be accessed: {named}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The source every replica below replicates from.
    create_available_cluster(&router, "emu-promote-src", "aurora-mysql", &[]).await;

    // Each case below is a cluster in a state no promotion can be performed in, asserted
    // while the cluster is in it, and the part of the refusal that says why: a cluster that
    // carries no replication source at all, one still being created, and one whose database
    // engine is not running.
    expect_not_a_replica_refusal(
        &router,
        "a cluster that never replicated",
        "emu-promote-standalone",
    )
    .await;

    expect_ok(
        "a replica that is still coming up",
        create_replica(&router, "emu-promote-creating", "aurora-mysql", "emu-promote-src").await,
    );
    expect_status_refusal(
        &router,
        "a replica that is still coming up",
        "emu-promote-creating",
        "status creating",
    )
    .await;

    create_available_cluster(
        &router,
        "emu-promote-stopped",
        "aurora-mysql",
        &[("ReplicationSourceIdentifier", "emu-promote-src")],
    )
    .await;
    expect_ok(
        "stop the replica",
        call(
            &router,
            "StopDBCluster",
            &[("DBClusterIdentifier", "emu-promote-stopped")],
        )
        .await,
    );
    wait_for_settle();
    expect_status_refusal(
        &router,
        "a replica that is stopped",
        "emu-promote-stopped",
        "status stopped",
    )
    .await;
    // The same refusal names the cluster in the SPELLING the request carried, whichever of
    // the forms the member accepts it used, as the not-found refusal above does.
    for named in [
        "EMU-Promote-Stopped".to_string(),
        cluster_arn("emu-promote-stopped"),
    ] {
        expect_status_refusal(
            &router,
            &format!("the stopped replica named {named:?}"),
            &named,
            "status stopped",
        )
        .await;
    }

    // A refused promotion changed nothing: the stopped replica still replicates from the
    // source it was created from, so it is still a replica once it is started again.
    let still_a_replica = describe_one(
        &router,
        "the stopped replica after its promotion was refused",
        "emu-promote-stopped",
    )
    .await;
    assert_eq!(
        optional_text(&still_a_replica, "ReplicationSourceIdentifier").as_deref(),
        Some(cluster_arn("emu-promote-src").as_str()),
        "expected the refused promotion to leave the cluster a read replica, got \
         {still_a_replica}"
    );
}
