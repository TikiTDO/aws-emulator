// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB cluster snapshot operations:
//! `DescribeDBClusterSnapshots`, `DeleteDBClusterSnapshot`, `CopyDBClusterSnapshot`,
//! `DescribeDBClusterSnapshotAttributes` and `ModifyDBClusterSnapshotAttribute`.
//!
//! The rule these pin: EVERY operation that names a DB cluster snapshot answers the
//! modelled `DBClusterSnapshotNotFoundFault` — HTTP 404, `text/xml`, an awsQuery
//! `<ErrorResponse>` whose `Error/Code` is `DBClusterSnapshotNotFoundFault` — when the
//! identifier it was given names no snapshot in the region the request was signed for,
//! and computes everything else from the snapshots that ARE stored. It is asserted over a
//! TABLE spanning the five operations and the identifier forms each accepts, with the
//! store both empty and holding other snapshots, so a 404 can never come from "the store
//! is empty".
//!
//! Every call goes over the wire, through the same `MultiServiceRouter` the production
//! router uses (`crate::db_instances::emulator`). The records the reads run against are
//! SEEDED through this service's own state API, so each row starts from exactly the stored
//! state it describes and the five implemented operations are exercised against real
//! stored state rather than an empty map. The snapshot a caller TAKES of a cluster is
//! driven through `CreateDBClusterSnapshot` over the wire instead, in
//! `crate::db_cluster_subject_operations`.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, DbClusterSnapshotRecord, MAX_SNAPSHOT_SHARED_ACCOUNTS,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, expect_error, expect_ok, optional_text, text, texts, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// One row of [`SNAPSHOT_OPERATIONS`]: the `Action`, the member that names the snapshot it
/// works on, and the other members the model requires.
type SnapshotOperation = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Every operation that names a DB cluster snapshot, as `(action, the member that names
/// the snapshot, the other members the model requires)`
/// (`smithy_query(query="operation", operation="CopyDBClusterSnapshot,…")`).
const SNAPSHOT_OPERATIONS: [SnapshotOperation; 5] = [
    (
        "DescribeDBClusterSnapshots",
        "DBClusterSnapshotIdentifier",
        &[],
    ),
    ("DeleteDBClusterSnapshot", "DBClusterSnapshotIdentifier", &[]),
    (
        "CopyDBClusterSnapshot",
        "SourceDBClusterSnapshotIdentifier",
        &[("TargetDBClusterSnapshotIdentifier", "the-copy")],
    ),
    (
        "DescribeDBClusterSnapshotAttributes",
        "DBClusterSnapshotIdentifier",
        &[],
    ),
    (
        "ModifyDBClusterSnapshotAttribute",
        "DBClusterSnapshotIdentifier",
        &[("AttributeName", "restore")],
    ),
];

/// The ARN a stored cluster snapshot carries.
fn snapshot_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:cluster-snapshot:{identifier}")
}

/// A complete, `available` cluster snapshot record of `cluster` — a snapshot that has
/// finished, which is what `CreateDBClusterSnapshot` leaves behind once the clock has
/// settled it.
fn stored_snapshot(region: &str, identifier: &str, cluster: &str) -> DbClusterSnapshotRecord {
    DbClusterSnapshotRecord {
        snapshot_identifier: identifier.to_string(),
        region: region.to_string(),
        db_cluster_identifier: cluster.to_string(),
        db_cluster_resource_id: format!("cluster-{cluster}"),
        engine: "aurora-postgresql".to_string(),
        engine_version: Some("15.4".to_string()),
        engine_mode: Some("provisioned".to_string()),
        database_name: Some("ledger".to_string()),
        // An Aurora cluster carries no `DBClusterInstanceClass` of its own.
        db_instance_class: None,
        allocated_storage: Some(1),
        holds_data: true,
        master_username: Some("postgres".to_string()),
        // A record built by hand holds no master credential and no copy of a cluster's
        // storage: both are taken from a live cluster by the operations that snapshot one.
        master_user_password: None,
        storage: None,
        availability_zones: vec![format!("{region}a"), format!("{region}b")],
        port: 5432,
        storage_type: Some("aurora".to_string()),
        storage_encrypted: false,
        kms_key_id: None,
        license_model: Some("postgresql-license".to_string()),
        vpc_id: state::region_vpc_id(region),
        backup_retention_period: Some(1),
        preferred_backup_window: Some("07:00-07:30".to_string()),
        iam_database_authentication_enabled: false,
        snapshot_type: "manual".to_string(),
        status: state::SNAPSHOT_STATUS_AVAILABLE.to_string(),
        pending_status: None,
        cluster_create_time: state::now_as_wire_timestamp(),
        snapshot_create_time: state::now_as_wire_timestamp(),
        source_snapshot_arn: None,
        restore_attribute: BTreeSet::new(),
        tags: Vec::new(),
    }
}

/// A router carrying this service, with `snapshots` — `(identifier, cluster)` — already
/// stored in `region`.
fn emulator_holding(region: &str, snapshots: &[(&str, &str)]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let store = state::service_state(&*engine).expect("this service's store");
        let mut store = store.lock();
        for (identifier, cluster) in snapshots {
            store.db_cluster_snapshots.insert(
                (region.to_string(), (*identifier).to_string()),
                stored_snapshot(region, identifier, cluster),
            );
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// An identifier that names no stored snapshot is the modelled 404 on EVERY operation that
/// names one — whatever form the identifier takes, and whether or not other snapshots
/// exist.
#[tokio::test]
async fn every_cluster_snapshot_operation_refuses_an_identifier_that_names_no_snapshot() {
    // `DBClusterSnapshotIdentifier` is bounded at 255 characters; an identifier at that
    // published bound still names nothing.
    let longest = format!("s{}", "x".repeat(254));

    // (case, the identifier to ask for)
    let missing: [(&str, &str); 5] = [
        ("a plain name", "no-such-snapshot"),
        // The identifier "is stored as a lowercase string", so case cannot turn a miss
        // into a hit.
        ("the same name upper-cased", "NO-SUCH-SNAPSHOT"),
        ("one character past a stored name", "alpha-snapshotx"),
        ("an ARN form", "arn:aws:rds:us-west-2:000000000000:cluster-snapshot:nope"),
        ("the longest allowed identifier", longest.as_str()),
    ];

    // Every request id the refusals carried: an awsQuery error envelope names ONE request,
    // so no two of them may be the same.
    let mut request_ids = BTreeSet::new();

    // Both store states: nothing stored at all, and two snapshots stored that simply are
    // not the one asked for.
    for seeded in [false, true] {
        let stored: &[(&str, &str)] = if seeded {
            &[("alpha-snapshot", "alpha"), ("beta-snapshot", "beta")]
        } else {
            &[]
        };
        let router = emulator_holding(REGION, stored);
        for (action, naming_member, extra) in SNAPSHOT_OPERATIONS {
            for (form, identifier) in missing {
                let case = format!("{action} with {form} (store seeded: {seeded})");
                let mut params: Vec<(&str, &str)> = vec![(naming_member, identifier)];
                params.extend_from_slice(extra);
                let (status, body) = call(&router, action, &params).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the modelled DBClusterSnapshotNotFoundFault's \
                     404, got {status} with {body}"
                );
                assert_eq!(
                    text(&body, "Code"),
                    "DBClusterSnapshotNotFoundFault",
                    "case {case:?}: got {body}"
                );
                assert_eq!(
                    text(&body, "Type"),
                    "Sender",
                    "case {case:?}: a client error is a Sender fault, got {body}"
                );
                assert!(
                    text(&body, "Message").contains(identifier),
                    "case {case:?}: the message must name the identifier the request asked \
                     for, got {body}"
                );
                // The envelope element an SDK reads back as the error's request id, the
                // counterpart of a successful response's `ResponseMetadata/RequestId`.
                let request_id = text(&body, "RequestId");
                assert_eq!(
                    request_id.split('-').map(str::len).collect::<Vec<_>>(),
                    vec![8, 4, 4, 4, 12],
                    "case {case:?}: expected an 8-4-4-4-12 request id, got {body}"
                );
                assert!(
                    request_ids.insert(request_id.clone()),
                    "case {case:?}: every response names its own request, but \
                     {request_id:?} was already seen"
                );
            }
        }
    }
}

/// A stored cluster snapshot is reported, copied, shared, unshared and deleted, and every
/// answer is read back from state.
#[tokio::test]
async fn a_stored_cluster_snapshot_is_reported_copied_shared_and_deleted() {
    let router = emulator_holding(
        REGION,
        &[("alpha-snapshot", "alpha"), ("beta-snapshot", "beta")],
    );

    // 1. Both snapshots are reported, in identifier order.
    let all = expect_ok(
        "describe all",
        call(&router, "DescribeDBClusterSnapshots", &[("", "")][..0]).await,
    );
    assert_eq!(
        texts(&all, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string(), "beta-snapshot".to_string()],
        "case \"describe all\": expected both stored snapshots in order, got {all}"
    );

    // 2. One snapshot, named case-insensitively, projects the stored record's own facts.
    let one = expect_ok(
        "describe one",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "ALPHA-SNAPSHOT")],
        )
        .await,
    );
    assert_eq!(
        texts(&one, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string()],
        "case \"describe one\": expected exactly the named snapshot, got {one}"
    );
    assert_eq!(
        text(&one, "DBClusterSnapshotArn"),
        snapshot_arn(REGION, "alpha-snapshot"),
        "case \"describe one\": expected the regional snapshot ARN, got {one}"
    );
    assert_eq!(
        text(&one, "DBClusterIdentifier"),
        "alpha",
        "case \"describe one\": the snapshot names the cluster it was taken from, got {one}"
    );
    assert_eq!(
        text(&one, "Status"),
        "available",
        "case \"describe one\": got {one}"
    );
    assert_eq!(
        text(&one, "PercentProgress"),
        "100",
        "case \"describe one\": a finished snapshot is complete, got {one}"
    );
    assert_eq!(
        optional_text(&one, "SourceDBClusterSnapshotArn"),
        None,
        "case \"describe one\": a snapshot taken from a cluster was copied from nothing, \
         got {one}"
    );

    // The same snapshot by ARN, and narrowed by the cluster it belongs to.
    for (case, member, value) in [
        (
            "describe by arn",
            "DBClusterSnapshotIdentifier",
            snapshot_arn(REGION, "alpha-snapshot"),
        ),
        ("describe by cluster", "DBClusterIdentifier", "ALPHA".to_string()),
        (
            "describe by resource id",
            "DbClusterResourceId",
            "cluster-alpha".to_string(),
        ),
    ] {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[(member, value.as_str())],
            )
            .await,
        );
        assert_eq!(
            texts(&body, "DBClusterSnapshotIdentifier"),
            vec!["alpha-snapshot".to_string()],
            "case {case:?}: got {body}"
        );
    }

    // A cluster that holds no snapshot is an empty page, because this operation models no
    // cluster-not-found error.
    let unknown_cluster = expect_ok(
        "describe by unknown cluster",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("DBClusterIdentifier", "no-such-cluster")],
        )
        .await,
    );
    assert!(
        texts(&unknown_cluster, "DBClusterSnapshotIdentifier").is_empty(),
        "case \"describe by unknown cluster\": expected an empty page, got {unknown_cluster}"
    );

    // The two selectors are documented as mutually exclusive.
    expect_error(
        "both selectors",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[
                ("DBClusterIdentifier", "alpha"),
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
            ],
        )
        .await,
        400,
        "ValidationException",
    );

    // 3. A copy is a new record that points back at its source, and AWS answers it in the
    // `copying` status: the clock owes it `available`.
    let copy = expect_ok(
        "copy",
        call(
            &router,
            "CopyDBClusterSnapshot",
            &[
                ("SourceDBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("TargetDBClusterSnapshotIdentifier", "ALPHA-COPY"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&copy, "DBClusterSnapshotIdentifier"),
        "alpha-copy",
        "case \"copy\": the identifier is stored as a lowercase string, got {copy}"
    );
    assert_eq!(
        text(&copy, "Status"),
        "copying",
        "case \"copy\": a copy AWS returns in progress must not be reported finished, \
         got {copy}"
    );
    assert_eq!(
        text(&copy, "PercentProgress"),
        "0",
        "case \"copy\": a copy still being made has made no progress, got {copy}"
    );
    assert_eq!(
        text(&copy, "SourceDBClusterSnapshotArn"),
        snapshot_arn(REGION, "alpha-snapshot"),
        "case \"copy\": the copy must name the snapshot it came from, got {copy}"
    );
    assert_eq!(
        text(&copy, "DBClusterIdentifier"),
        "alpha",
        "case \"copy\": a copy is still a snapshot of the source's cluster, got {copy}"
    );

    // While it is copying, the state errors that go with that status hold.
    expect_error(
        "delete a copy still copying",
        call(
            &router,
            "DeleteDBClusterSnapshot",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await,
        400,
        "InvalidDBClusterSnapshotStateFault",
    );
    expect_error(
        "share a copy still copying",
        call(
            &router,
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-copy"),
                ("AttributeName", "restore"),
            ],
        )
        .await,
        400,
        "InvalidDBClusterSnapshotStateFault",
    );

    // The clock makes it available; a describe materialises the transition.
    crate::db_instances::wait_for_settle();
    let after_copy = expect_ok(
        "describe after copy",
        call(&router, "DescribeDBClusterSnapshots", &[("", "")][..0]).await,
    );
    assert_eq!(
        texts(&after_copy, "DBClusterSnapshotIdentifier"),
        vec![
            "alpha-copy".to_string(),
            "alpha-snapshot".to_string(),
            "beta-snapshot".to_string()
        ],
        "case \"describe after copy\": the copy must be in state, got {after_copy}"
    );
    assert_eq!(
        texts(&after_copy, "Status"),
        vec![
            "available".to_string(),
            "available".to_string(),
            "available".to_string()
        ],
        "case \"describe after copy\": the clock owed the copy `available`, got {after_copy}"
    );

    // Copying onto an identifier already in state is the modelled already-exists fault.
    expect_error(
        "duplicate copy",
        call(
            &router,
            "CopyDBClusterSnapshot",
            &[
                ("SourceDBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("TargetDBClusterSnapshotIdentifier", "alpha-copy"),
            ],
        )
        .await,
        400,
        "DBClusterSnapshotAlreadyExistsFault",
    );

    // 4. The `restore` attribute starts empty, takes the accounts a caller authorizes, and
    // is read back by the describe.
    let attributes = expect_ok(
        "attributes before sharing",
        call(
            &router,
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await,
    );
    assert_eq!(
        texts(&attributes, "AttributeName"),
        vec!["restore".to_string()],
        "case \"attributes before sharing\": `restore` is always reported, got {attributes}"
    );
    assert!(
        texts(&attributes, "AttributeValue").is_empty(),
        "case \"attributes before sharing\": a snapshot nobody is authorized for carries \
         no values, got {attributes}"
    );

    let shared = expect_ok(
        "share",
        call(
            &router,
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
                ("ValuesToAdd.AttributeValue.2", "all"),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&shared, "AttributeValue"),
        vec!["111122223333".to_string(), "all".to_string()],
        "case \"share\": the answer must report the attribute as it now stands, got {shared}"
    );
    let read_back = expect_ok(
        "attributes after sharing",
        call(
            &router,
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await,
    );
    assert_eq!(
        texts(&read_back, "AttributeValue"),
        vec!["111122223333".to_string(), "all".to_string()],
        "case \"attributes after sharing\": the write must be in state, got {read_back}"
    );
    // The copy does not inherit the source's authorizations.
    let copy_attributes = expect_ok(
        "attributes of the copy",
        call(
            &router,
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await,
    );
    assert!(
        texts(&copy_attributes, "AttributeValue").is_empty(),
        "case \"attributes of the copy\": `restore` is not copied, got {copy_attributes}"
    );

    // `public` now selects the one snapshot whose `restore` attribute carries `all`.
    // (case, SnapshotType, the snapshots it must select)
    let type_rows: [(&str, &str, &[&str]); 4] = [
        (
            "manual",
            "manual",
            &["alpha-copy", "alpha-snapshot", "beta-snapshot"],
        ),
        ("public", "public", &["alpha-snapshot"]),
        ("automated", "automated", &[]),
        ("shared", "shared", &[]),
    ];
    for (case, snapshot_type, expected) in type_rows {
        let page = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[("SnapshotType", snapshot_type)],
            )
            .await,
        );
        let reported = texts(&page, "DBClusterSnapshotIdentifier");
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?}, got {reported:?} in {page}"
        );
    }
    expect_error(
        "an unmodelled SnapshotType",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("SnapshotType", "semi-automated")],
        )
        .await,
        400,
        "ValidationException",
    );

    // Authorizing the same account twice changes nothing, and a removal takes it away.
    let unshared = expect_ok(
        "unshare",
        call(
            &router,
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
                ("ValuesToRemove.AttributeValue.1", "all"),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&unshared, "AttributeValue"),
        vec!["111122223333".to_string()],
        "case \"unshare\": got {unshared}"
    );

    // 5. Deleting a snapshot answers with it and removes it from state; a second delete
    // finds nothing.
    let deleted = expect_ok(
        "delete the copy",
        call(
            &router,
            "DeleteDBClusterSnapshot",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "DBClusterSnapshotIdentifier"),
        "alpha-copy",
        "case \"delete the copy\": got {deleted}"
    );
    let after_delete = expect_ok(
        "describe after delete",
        call(&router, "DescribeDBClusterSnapshots", &[("", "")][..0]).await,
    );
    assert_eq!(
        texts(&after_delete, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string(), "beta-snapshot".to_string()],
        "case \"describe after delete\": the deleted snapshot must be gone, got {after_delete}"
    );
    expect_error(
        "delete twice",
        call(
            &router,
            "DeleteDBClusterSnapshot",
            &[("DBClusterSnapshotIdentifier", "alpha-copy")],
        )
        .await,
        404,
        "DBClusterSnapshotNotFoundFault",
    );
}

/// A `CopyDBClusterSnapshot` whose source ARN names ANOTHER region finds no source: this
/// store keeps a snapshot under the region it lives in, and a cross-region source is not
/// in it. The call answers the modelled 404 — never a panic, and never a copy of a
/// snapshot it could not read — and the message names the region it looked in, which is
/// the ARN's own.
#[tokio::test]
async fn a_copy_whose_source_arn_names_another_region_is_the_modelled_not_found() {
    let router = emulator_holding(REGION, &[("alpha-snapshot", "alpha")]);

    // (case, the source the request names, the region the message must name)
    let rows: [(&str, String, &str); 4] = [
        // The stored snapshot's own identifier, under another region's ARN: the
        // identifier exists, the region does not hold it.
        (
            "a stored identifier under another region's ARN",
            snapshot_arn("us-east-1", "alpha-snapshot"),
            "us-east-1",
        ),
        (
            "a stored identifier under a third region's ARN",
            snapshot_arn("eu-west-1", "alpha-snapshot"),
            "eu-west-1",
        ),
        (
            "an identifier nothing holds, under another region's ARN",
            snapshot_arn("ap-south-1", "no-such-snapshot"),
            "ap-south-1",
        ),
        // A plain identifier naming nothing is looked for in the request's own region.
        (
            "a plain identifier nothing holds",
            "no-such-snapshot".to_string(),
            REGION,
        ),
    ];

    for (case, source, expected_region) in rows {
        let body = expect_error(
            case,
            call(
                &router,
                "CopyDBClusterSnapshot",
                &[
                    ("SourceDBClusterSnapshotIdentifier", source.as_str()),
                    ("TargetDBClusterSnapshotIdentifier", "the-copy"),
                ],
            )
            .await,
            404,
            "DBClusterSnapshotNotFoundFault",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(source.as_str()) && message.contains(expected_region),
            "case {case:?}: the message must name the source asked for and the region it \
             was looked for in ({expected_region}), got {message:?}"
        );
        // Nothing was written: the target the refused copy named must not exist.
        expect_error(
            case,
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", "the-copy")],
            )
            .await,
            404,
            "DBClusterSnapshotNotFoundFault",
        );
    }

    // The same ARN form IN the request's own region names the stored snapshot, so the
    // refusals above are about the region and not about the ARN form.
    let copied = expect_ok(
        "the source ARN in the request's own region",
        call(
            &router,
            "CopyDBClusterSnapshot",
            &[
                (
                    "SourceDBClusterSnapshotIdentifier",
                    snapshot_arn(REGION, "alpha-snapshot").as_str(),
                ),
                ("TargetDBClusterSnapshotIdentifier", "the-copy"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&copied, "SourceDBClusterSnapshotArn"),
        snapshot_arn(REGION, "alpha-snapshot"),
        "case \"the source ARN in the request's own region\": the copy must name the \
         snapshot it was made from, got {copied}"
    );
}

/// RDS is regional: a snapshot stored for one region is invisible to a request signed for
/// another, on every operation that names one.
#[tokio::test]
async fn cluster_snapshots_are_scoped_to_the_region_the_request_is_signed_for() {
    let router = emulator_holding(REGION, &[("alpha-snapshot", "alpha")]);

    // Two other regions, so the rule is not "one other region behaves differently".
    for other_region in ["us-east-1", "eu-west-1"] {
        for (action, naming_member, extra) in SNAPSHOT_OPERATIONS {
            let case = format!("{action} signed for {other_region}");
            let mut params: Vec<(&str, &str)> = vec![(naming_member, "alpha-snapshot")];
            params.extend_from_slice(extra);
            expect_error(
                &case,
                call_in(&router, other_region, action, &params).await,
                404,
                "DBClusterSnapshotNotFoundFault",
            );
        }
        let page = expect_ok(
            &format!("describe all signed for {other_region}"),
            call_in(
                &router,
                other_region,
                "DescribeDBClusterSnapshots",
                &[("", "")][..0],
            )
            .await,
        );
        assert!(
            texts(&page, "DBClusterSnapshotIdentifier").is_empty(),
            "case \"describe all signed for {other_region}\": another region's snapshots \
             must not be reported, got {page}"
        );
    }

    // The region that owns it still sees it.
    let own = expect_ok(
        "describe in its own region",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await,
    );
    assert_eq!(
        texts(&own, "DBClusterSnapshotIdentifier"),
        vec!["alpha-snapshot".to_string()],
        "case \"describe in its own region\": got {own}"
    );
}

/// `ModifyDBClusterSnapshotAttribute` refuses what the model and its documentation do not
/// allow, over the class of each rule, and stops at the published sharing quota.
#[tokio::test]
async fn sharing_a_cluster_snapshot_refuses_values_outside_the_documented_rule() {
    let router = emulator_holding(REGION, &[("alpha-snapshot", "alpha")]);

    // (case, the members to send besides the snapshot identifier)
    let refused: [(&str, &[(&str, &str)]); 6] = [
        (
            // `AttributeName`: `restore` is the only attribute AWS defines.
            "an attribute other than restore",
            &[("AttributeName", "share")],
        ),
        (
            "the right attribute name in the wrong case",
            &[("AttributeName", "RESTORE")],
        ),
        (
            // A value must be an account id or `all`.
            "a value that is not an account id",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "not-an-account"),
            ],
        ),
        (
            "an account id one digit short",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "11112222333"),
            ],
        ),
        (
            "an account id one digit long",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "1111222233334"),
            ],
        ),
        (
            // The result would depend on which list were applied first.
            "the same value added and removed",
            &[
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "111122223333"),
                ("ValuesToRemove.AttributeValue.1", "111122223333"),
            ],
        ),
    ];

    for (case, members) in refused {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterSnapshotIdentifier", "alpha-snapshot")];
        params.extend_from_slice(members);
        expect_error(
            case,
            call(&router, "ModifyDBClusterSnapshotAttribute", &params).await,
            400,
            "ValidationException",
        );
    }

    // Nothing above reached the store: the attribute is still empty.
    let attributes = expect_ok(
        "attributes after the refusals",
        call(
            &router,
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await,
    );
    assert!(
        texts(&attributes, "AttributeValue").is_empty(),
        "case \"attributes after the refusals\": a refused call must store nothing, \
         got {attributes}"
    );

    // One account per published slot is authorized, and the one past the limit is the
    // modelled quota fault; see `state::MAX_SNAPSHOT_SHARED_ACCOUNTS`.
    for index in 0..MAX_SNAPSHOT_SHARED_ACCOUNTS {
        let account = format!("{:012}", 100_000_000_000_u64 + index as u64);
        expect_ok(
            "fill the sharing quota",
            call(
                &router,
                "ModifyDBClusterSnapshotAttribute",
                &[
                    ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                    ("AttributeName", "restore"),
                    ("ValuesToAdd.AttributeValue.1", account.as_str()),
                ],
            )
            .await,
        );
    }
    expect_error(
        "one account past the quota",
        call(
            &router,
            "ModifyDBClusterSnapshotAttribute",
            &[
                ("DBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("AttributeName", "restore"),
                ("ValuesToAdd.AttributeValue.1", "999988887777"),
            ],
        )
        .await,
        400,
        // The wire code is the shape's `awsQueryError` code, which drops the `Fault`
        // suffix here (`smithy_query`: awsQueryError code `SharedSnapshotQuotaExceeded`).
        "SharedSnapshotQuotaExceeded",
    );
    let still = expect_ok(
        "attributes after the over-quota call",
        call(
            &router,
            "DescribeDBClusterSnapshotAttributes",
            &[("DBClusterSnapshotIdentifier", "alpha-snapshot")],
        )
        .await,
    );
    assert_eq!(
        texts(&still, "AttributeValue").len(),
        MAX_SNAPSHOT_SHARED_ACCOUNTS,
        "case \"attributes after the over-quota call\": the refused account must not have \
         been stored, got {still}"
    );
}

/// `ValuesToAdd` and `ValuesToRemove` publish no `@length`, so their entry COUNTS are the
/// caller's to choose, and every check this operation runs is per entry. A list past the
/// emulator's collection bound is refused before anything is checked or stored; a list AT
/// the bound is well formed, so it is the documented sharing quota that answers it.
#[tokio::test]
async fn an_over_size_attribute_value_list_is_refused_before_anything_is_applied() {
    let router = emulator_holding(REGION, &[("alpha-snapshot", "alpha")]);

    // (case, the member carrying the list, how many entries, the wire error code; "" for
    // a request inside both bounds, which is served)
    let rows: [(&str, &str, usize, &str); 4] = [
        (
            "ValuesToAdd one entry past the collection bound",
            "ValuesToAdd",
            state::MAX_COLLECTION_ENTRIES + 1,
            "ValidationException",
        ),
        (
            "ValuesToRemove one entry past the collection bound",
            "ValuesToRemove",
            state::MAX_COLLECTION_ENTRIES + 1,
            "ValidationException",
        ),
        (
            // At the bound the request is well formed, so the DOCUMENTED quota on the
            // accounts a snapshot ends up shared with is what refuses it.
            "ValuesToAdd at the collection bound",
            "ValuesToAdd",
            state::MAX_COLLECTION_ENTRIES,
            "SharedSnapshotQuotaExceeded",
        ),
        (
            // Inside both bounds: adding is accepted, which is what makes the refusals
            // above about the counts and not about the values.
            "ValuesToAdd inside both bounds",
            "ValuesToAdd",
            10,
            "",
        ),
    ];

    for (case, member, entries, expected_code) in rows {
        // Distinct, well-formed 12-digit account ids, so the rows differ only in COUNT.
        let mut owned: Vec<(String, String)> = vec![
            (
                "DBClusterSnapshotIdentifier".to_string(),
                "alpha-snapshot".to_string(),
            ),
            ("AttributeName".to_string(), "restore".to_string()),
        ];
        owned.extend((0..entries).map(|index| {
            (
                format!("{member}.AttributeValue.{}", index + 1),
                format!("{:012}", 100_000_000_000_u64 + index as u64),
            )
        }));
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let response = call(&router, "ModifyDBClusterSnapshotAttribute", &params).await;
        if expected_code.is_empty() {
            let body = expect_ok(case, response);
            assert_eq!(
                texts(&body, "AttributeValue").len(),
                entries,
                "case {case:?}: every value sent must be authorized, got {body}"
            );
            continue;
        }
        expect_error(case, response, 400, expected_code);
    }
}

/// One row of the tag table: the case, the `Tags` request members it sends as
/// `(name, value)` pairs, and whether the copy is accepted.
type TagRow = (&'static str, Vec<(String, String)>, bool);

/// `TagList` publishes no `@length` and the tags are STORED on the copy, so the list is
/// bounded: AWS's tagging limits (50 tags, a 128-character key, a 256-character value) are
/// enforced before anything is kept, a list inside them is stored and reported back, and a
/// refused copy stores nothing at all.
#[tokio::test]
async fn a_copy_refuses_a_tag_list_the_emulator_will_not_store() {
    let router = emulator_holding(REGION, &[("alpha-snapshot", "alpha")]);

    // (case, the tag members to send, accepted)
    let longest_key = "k".repeat(state::MAX_TAG_KEY_LEN);
    let over_long_key = "k".repeat(state::MAX_TAG_KEY_LEN + 1);
    let longest_value = "v".repeat(state::MAX_TAG_VALUE_LEN);
    let over_long_value = "v".repeat(state::MAX_TAG_VALUE_LEN + 1);

    // One tag entry per published slot, and one past it.
    let at_the_limit: Vec<(String, String)> = (0..state::MAX_TAGS_PER_RESOURCE)
        .flat_map(|index| {
            [
                (format!("Tags.Tag.{}.Key", index + 1), format!("k{index}")),
                (format!("Tags.Tag.{}.Value", index + 1), format!("v{index}")),
            ]
        })
        .collect();
    let mut past_the_limit = at_the_limit.clone();
    past_the_limit.push((
        format!("Tags.Tag.{}.Key", state::MAX_TAGS_PER_RESOURCE + 1),
        "one-too-many".to_string(),
    ));
    past_the_limit.push((
        format!("Tags.Tag.{}.Value", state::MAX_TAGS_PER_RESOURCE + 1),
        "v".to_string(),
    ));

    let one_tag = |key: &str, value: &str| -> Vec<(String, String)> {
        vec![
            ("Tags.Tag.1.Key".to_string(), key.to_string()),
            ("Tags.Tag.1.Value".to_string(), value.to_string()),
        ]
    };

    // (case, the tag members, whether the copy is accepted)
    let rows: [TagRow; 6] = [
        ("no tags at all", Vec::new(), true),
        ("tags at the published count", at_the_limit, true),
        ("one tag past the published count", past_the_limit, false),
        (
            "a key at the published length",
            one_tag(&longest_key, "v"),
            true,
        ),
        (
            "a key one character past it",
            one_tag(&over_long_key, "v"),
            false,
        ),
        (
            "a value one character past it",
            one_tag("k", &over_long_value),
            false,
        ),
    ];

    for (index, (case, tags, accepted)) in rows.into_iter().enumerate() {
        // Each row copies to its own target, so one row cannot be refused for a reason
        // another row created.
        let target = format!("the-copy-{index}");
        let mut owned: Vec<(String, String)> = vec![
            (
                "SourceDBClusterSnapshotIdentifier".to_string(),
                "alpha-snapshot".to_string(),
            ),
            ("TargetDBClusterSnapshotIdentifier".to_string(), target.clone()),
        ];
        owned.extend(tags.iter().cloned());
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let response = call(&router, "CopyDBClusterSnapshot", &params).await;
        if accepted {
            let body = expect_ok(case, response);
            assert_eq!(
                texts(&body, "Key").len(),
                tags.len() / 2,
                "case {case:?}: every tag sent must be stored on the copy, got {body}"
            );
        } else {
            expect_error(case, response, 400, "ValidationException");
            // A refused copy stores nothing: the target it named must not exist.
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBClusterSnapshots",
                    &[("DBClusterSnapshotIdentifier", target.as_str())],
                )
                .await,
                404,
                "DBClusterSnapshotNotFoundFault",
            );
        }
    }

    // The longest accepted value is stored verbatim, so the bound admits what it says.
    let copied = expect_ok(
        "a value at the published length",
        call(
            &router,
            "CopyDBClusterSnapshot",
            &[
                ("SourceDBClusterSnapshotIdentifier", "alpha-snapshot"),
                ("TargetDBClusterSnapshotIdentifier", "the-copy-tagged"),
                ("Tags.Tag.1.Key", "k"),
                ("Tags.Tag.1.Value", longest_value.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&copied, "Value"),
        vec![longest_value],
        "case \"a value at the published length\": got {copied}"
    );
}

/// `Filters` selects over the four names this operation documents, on the identifier and
/// the ARN forms each accepts, case-sensitively — and a name it does not document is a 400
/// rather than a filter quietly ignored, which would report snapshots the caller excluded.
#[tokio::test]
async fn filters_select_over_the_documented_names_and_refuse_any_other() {
    let router = emulator_holding(
        REGION,
        &[("alpha-snapshot", "alpha"), ("beta-snapshot", "beta")],
    );

    /// The `Filters.Filter.1` request members for one filter and its values.
    fn filter_members(name: &str, values: &[&str]) -> Vec<(String, String)> {
        let mut members = vec![("Filters.Filter.1.Name".to_string(), name.to_string())];
        members.extend(values.iter().enumerate().map(|(index, value)| {
            (
                format!("Filters.Filter.1.Values.Value.{}", index + 1),
                (*value).to_string(),
            )
        }));
        members
    }

    // (case, filter name, filter values, the snapshot identifiers the page must carry).
    // The names and the value forms each accepts come from the member's own documentation
    // (`smithy_query(query="doc",shape="DescribeDBClusterSnapshotsMessage",member="Filters")`):
    // db-cluster-id takes cluster identifiers and cluster ARNs, db-cluster-snapshot-id
    // snapshot identifiers (and their ARNs), engine the engine name, snapshot-type the
    // snapshot type — and "Filter names and values are case-sensitive."
    let rows: [(&str, &str, &[&str], &[&str]); 10] = [
        ("db-cluster-id by identifier", "db-cluster-id", &["alpha"], &["alpha-snapshot"]),
        (
            "db-cluster-id by cluster ARN",
            "db-cluster-id",
            &[&format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:beta")],
            &["beta-snapshot"],
        ),
        (
            "db-cluster-id with both values",
            "db-cluster-id",
            &["alpha", "beta"],
            &["alpha-snapshot", "beta-snapshot"],
        ),
        (
            "db-cluster-id in the wrong case matches nothing",
            "db-cluster-id",
            &["ALPHA"],
            &[],
        ),
        (
            "db-cluster-snapshot-id by identifier",
            "db-cluster-snapshot-id",
            &["beta-snapshot"],
            &["beta-snapshot"],
        ),
        (
            "db-cluster-snapshot-id by snapshot ARN",
            "db-cluster-snapshot-id",
            &[&format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:alpha-snapshot")],
            &["alpha-snapshot"],
        ),
        (
            // Every stored snapshot carries the same engine, so this one selects both.
            "engine the stored snapshots carry",
            "engine",
            &["aurora-postgresql"],
            &["alpha-snapshot", "beta-snapshot"],
        ),
        ("another engine", "engine", &["mysql"], &[]),
        (
            "snapshot-type manual",
            "snapshot-type",
            &["manual"],
            &["alpha-snapshot", "beta-snapshot"],
        ),
        ("snapshot-type automated", "snapshot-type", &["automated"], &[]),
    ];

    for (case, name, values, expected) in rows {
        let owned = filter_members(name, values);
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(member, value)| (member.as_str(), value.as_str()))
            .collect();
        let body = expect_ok(case, call(&router, "DescribeDBClusterSnapshots", &params).await);
        assert_eq!(
            texts(&body, "DBClusterSnapshotIdentifier"),
            expected
                .iter()
                .map(|identifier| (*identifier).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: got {body}"
        );
    }

    // A name the member does not document, and the same name in the wrong case: both are
    // refused, never served as an unfiltered page.
    for (case, name) in [
        ("an unsupported filter name", "db-cluster-snapshot-arn"),
        ("a supported name in the wrong case", "DB-Cluster-Id"),
        ("an empty filter name", ""),
    ] {
        let owned = filter_members(name, &["alpha"]);
        let params: Vec<(&str, &str)> = owned
            .iter()
            .map(|(member, value)| (member.as_str(), value.as_str()))
            .collect();
        expect_error(
            case,
            call(&router, "DescribeDBClusterSnapshots", &params).await,
            400,
            "InvalidParameterValue",
        );
    }
}

/// `MaxRecords` holds its published bounds and the `Marker` round-trips over the stored
/// cluster snapshots.
#[tokio::test]
async fn max_records_holds_its_bounds_and_the_marker_pages_the_cluster_snapshots() {
    // One more snapshot than the smallest page, so a first page cannot be the last.
    let total = *state::MAX_RECORDS_RANGE.start() as usize + 2;
    let stored: Vec<(String, String)> = (0..total)
        .map(|index| (format!("snapshot-{index:03}"), "alpha".to_string()))
        .collect();
    let rows: Vec<(&str, &str)> = stored
        .iter()
        .map(|(identifier, cluster)| (identifier.as_str(), cluster.as_str()))
        .collect();
    let router = emulator_holding(REGION, &rows);

    // (case, MaxRecords, accepted)
    let bounds: [(&str, i32, bool); 4] = [
        (
            "one below the minimum",
            *state::MAX_RECORDS_RANGE.start() - 1,
            false,
        ),
        ("the published minimum", *state::MAX_RECORDS_RANGE.start(), true),
        ("the published maximum", *state::MAX_RECORDS_RANGE.end(), true),
        (
            "one above the maximum",
            *state::MAX_RECORDS_RANGE.end() + 1,
            false,
        ),
    ];
    for (case, max_records, accepted) in bounds {
        let value = max_records.to_string();
        let (status, body) = call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("MaxRecords", value.as_str())],
        )
        .await;
        assert_eq!(
            status == 200,
            accepted,
            "case {case:?}: expected accepted == {accepted}, got {status} with {body}"
        );
    }

    // The first page carries the smallest allowed number of records and a marker; the
    // second starts at it and carries none.
    let page_size = state::MAX_RECORDS_RANGE.start().to_string();
    let first = expect_ok(
        "first page",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("MaxRecords", page_size.as_str())],
        )
        .await,
    );
    let first_ids = texts(&first, "DBClusterSnapshotIdentifier");
    assert_eq!(
        first_ids.len(),
        *state::MAX_RECORDS_RANGE.start() as usize,
        "case \"first page\": expected a full page, got {first}"
    );
    let marker = text(&first, "Marker");
    let second = expect_ok(
        "second page",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[
                ("MaxRecords", page_size.as_str()),
                ("Marker", marker.as_str()),
            ],
        )
        .await,
    );
    let second_ids = texts(&second, "DBClusterSnapshotIdentifier");
    assert_eq!(
        second_ids.len(),
        total - *state::MAX_RECORDS_RANGE.start() as usize,
        "case \"second page\": expected the rest of the snapshots, got {second}"
    );
    assert!(
        second_ids.iter().all(|id| *id >= marker),
        "case \"second page\": every record must lie at or beyond the marker {marker:?}, \
         got {second_ids:?}"
    );
    assert_eq!(
        optional_text(&second, "Marker"),
        None,
        "case \"second page\": the last page carries no marker, got {second}"
    );
}
