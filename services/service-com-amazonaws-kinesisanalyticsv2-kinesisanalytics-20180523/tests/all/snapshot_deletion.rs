// SPDX-License-Identifier: CC0-1.0

//! Deleting one snapshot of an application, and how the snapshot operations report an
//! application they hold no record of.
//!
//! `DeleteApplicationSnapshot` identifies the snapshot to remove by the application it
//! belongs to, its `SnapshotName` and the `SnapshotCreationTimestamp` the snapshot readers
//! report for it. The suite asserts that over a TABLE of snapshot names spanning the
//! modelled `SnapshotName` form, and over every way a request can fail to identify a
//! stored snapshot — an application that does not exist, a snapshot name the application
//! holds no record of, one already deleted, and a creation timestamp that is not the
//! stored one — not over the one request a recording happens to carry. The last case is
//! driven over every snapshot operation, since they all resolve a request against one
//! application's snapshots.

use serde_json::{json, Value};

use crate::harness::{
    call, create_startable, error_code, error_message, listed_snapshots, router, ACCOUNT,
    SETTLE_WAIT,
};

/// Snapshot names spanning the modelled `SnapshotName` form — len 1..256, pattern
/// `^[a-zA-Z0-9_.-]+$` (`smithy_query(query="shape",shape="SnapshotName")`): the shortest
/// name, one of each punctuation the pattern admits, a digits-only name, a mixed-case
/// name, and the longest name the length constraint allows.
fn snapshot_names() -> Vec<(&'static str, String)> {
    vec![
        ("shortest", "s".to_string()),
        ("punctuation", "snap.1_2-3".to_string()),
        ("digits", "20260101".to_string()),
        ("mixed-case", "SnapShot-B".to_string()),
        ("longest", "z".repeat(256)),
    ]
}

/// Start `name` and take a snapshot under each of `snapshots`, returning the stored
/// `SnapshotCreationTimestamp` of each as the readers report it.
async fn running_application_with_snapshots(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    snapshots: &[String],
) -> Vec<i64> {
    create_startable(router, name, "FLINK-1_20").await;
    let (status, started) = call(router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(status, 200, "case {name}: StartApplication got {status} {started}");
    tokio::time::sleep(SETTLE_WAIT).await;

    let mut timestamps = Vec::new();
    for snapshot in snapshots {
        let (status, taken) = call(
            router,
            "CreateApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {snapshot}: CreateApplicationSnapshot got {status} {taken}"
        );
        timestamps.push(creation_timestamp(router, name, snapshot).await);
    }
    timestamps
}

/// The `SnapshotCreationTimestamp` the reader reports for one stored snapshot.
async fn creation_timestamp(
    router: &smithy_server_runtime::MultiServiceRouter,
    application: &str,
    snapshot: &str,
) -> i64 {
    let (status, described) = call(
        router,
        "DescribeApplicationSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": snapshot}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {snapshot}: DescribeApplicationSnapshot got {status} {described}"
    );
    described["SnapshotDetails"]["SnapshotCreationTimestamp"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {snapshot}: expected a stored creation timestamp, got {described}"))
}

/// A delete that identifies a stored snapshot removes THAT snapshot and leaves the rest:
/// the readers stop reporting it, and the snapshots not named are still reported.
#[tokio::test]
async fn deleting_a_snapshot_removes_only_the_snapshot_named() {
    let router = router();
    let application = "snapshot-delete-one";
    let cases = snapshot_names();
    let names: Vec<String> = cases.iter().map(|(_, name)| name.clone()).collect();
    let timestamps = running_application_with_snapshots(&router, application, &names).await;

    let mut remaining: Vec<String> = names.clone();
    for ((case, snapshot), timestamp) in cases.iter().zip(timestamps) {
        let (status, deleted) = call(
            &router,
            "DeleteApplicationSnapshot",
            &json!({
                "ApplicationName": application,
                "SnapshotName": snapshot,
                "SnapshotCreationTimestamp": timestamp,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DeleteApplicationSnapshot, got {status} {deleted}"
        );
        // The modelled output declares no members, so the success is the empty document.
        assert_eq!(
            deleted,
            json!({}),
            "case {case}: expected the empty DeleteApplicationSnapshotResponse, got {deleted}"
        );

        remaining.retain(|held| held != snapshot);
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            listed_snapshots(&router, case, application).await,
            expected,
            "case {case}: expected the list to report exactly the snapshots not deleted"
        );

        let (status, described) = call(
            &router,
            "DescribeApplicationSnapshot",
            &json!({"ApplicationName": application, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected the deleted snapshot to be gone, got {status} {described}"
        );
    }

    assert_eq!(
        listed_snapshots(&router, "all-deleted", application).await,
        Vec::<String>::new(),
        "case all-deleted: expected the application to report no snapshot"
    );
}

/// A request that does not identify a stored snapshot is refused with the modelled error
/// for what it got wrong: a `ResourceNotFoundException` when the application holds no
/// snapshot of that name — including one already deleted — an `InvalidArgumentException`
/// when the application itself is not stored, and an `InvalidArgumentException` when the
/// creation timestamp supplied is not the stored one.
#[tokio::test]
async fn a_delete_that_identifies_no_stored_snapshot_is_refused() {
    let router = router();
    let application = "snapshot-delete-refusals";
    let held = "held-snapshot".to_string();
    let timestamps =
        running_application_with_snapshots(&router, application, std::slice::from_ref(&held)).await;
    let timestamp = timestamps[0];

    // Deleted up front, so the "already deleted" row names a snapshot this application
    // really did hold.
    let gone = "gone-snapshot".to_string();
    let (status, taken) = call(
        &router,
        "CreateApplicationSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": gone}),
    )
    .await;
    assert_eq!(status, 200, "case gone: CreateApplicationSnapshot got {status} {taken}");
    let gone_timestamp = creation_timestamp(&router, application, &gone).await;
    let (status, deleted) = call(
        &router,
        "DeleteApplicationSnapshot",
        &json!({
            "ApplicationName": application,
            "SnapshotName": gone,
            "SnapshotCreationTimestamp": gone_timestamp,
        }),
    )
    .await;
    assert_eq!(status, 200, "case gone: the first delete got {status} {deleted}");

    // (case, request, the error code expected, the sentence expected)
    let cases: Vec<(&str, Value, &str, String)> = vec![
        (
            "absent-application",
            json!({
                "ApplicationName": "never-created",
                "SnapshotName": held,
                "SnapshotCreationTimestamp": timestamp,
            }),
            "InvalidArgumentException",
            format!("Application 'never-created' does not exist under account '{ACCOUNT}'."),
        ),
        (
            "never-minted-snapshot",
            json!({
                "ApplicationName": application,
                "SnapshotName": "never-minted",
                "SnapshotCreationTimestamp": timestamp,
            }),
            "ResourceNotFoundException",
            format!("Snapshot 'never-minted' does not exist for application {application}."),
        ),
        (
            "already-deleted-snapshot",
            json!({
                "ApplicationName": application,
                "SnapshotName": gone,
                "SnapshotCreationTimestamp": gone_timestamp,
            }),
            "ResourceNotFoundException",
            format!("Snapshot '{gone}' does not exist for application {application}."),
        ),
        (
            "timestamp-one-second-early",
            json!({
                "ApplicationName": application,
                "SnapshotName": held,
                "SnapshotCreationTimestamp": timestamp - 1,
            }),
            "InvalidArgumentException",
            format!(
                "SnapshotCreationTimestamp {} does not match the creation timestamp of \
                 snapshot {held} for application {application}",
                timestamp - 1
            ),
        ),
        (
            "timestamp-epoch",
            json!({
                "ApplicationName": application,
                "SnapshotName": held,
                "SnapshotCreationTimestamp": 0,
            }),
            "InvalidArgumentException",
            format!(
                "SnapshotCreationTimestamp 0 does not match the creation timestamp of \
                 snapshot {held} for application {application}"
            ),
        ),
        (
            "timestamp-far-future",
            json!({
                "ApplicationName": application,
                "SnapshotName": held,
                "SnapshotCreationTimestamp": timestamp + 86_400,
            }),
            "InvalidArgumentException",
            format!(
                "SnapshotCreationTimestamp {} does not match the creation timestamp of \
                 snapshot {held} for application {application}",
                timestamp + 86_400
            ),
        ),
    ];

    for (case, request, code, message) in cases {
        let (status, refused) = call(&router, "DeleteApplicationSnapshot", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, code),
            "case {case}: expected 400 {code}, got {status} {refused}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case}: expected the refusal to state what was not found, got {refused}"
        );

        // A refused delete removes nothing: the snapshot the application does hold is
        // still there to be read and, with the stored timestamp, still deletable.
        assert_eq!(
            listed_snapshots(&router, case, application).await,
            vec![held.clone()],
            "case {case}: expected the refused delete to leave the stored snapshot"
        );
    }

    let (status, deleted) = call(
        &router,
        "DeleteApplicationSnapshot",
        &json!({
            "ApplicationName": application,
            "SnapshotName": held,
            "SnapshotCreationTimestamp": timestamp,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case stored-timestamp: expected the stored timestamp to delete the snapshot, got \
         {status} {deleted}"
    );
    assert_eq!(
        listed_snapshots(&router, "stored-timestamp", application).await,
        Vec::<String>::new(),
        "case stored-timestamp: expected no snapshot to remain"
    );
}

/// Every snapshot operation refuses an application the service holds no record of as a
/// parameter that is not a valid argument — HTTP 400 `InvalidArgumentException` — stating
/// the name asked for and the account it was resolved in, rather than as the
/// `ResourceNotFoundException` the APPLICATION readers give the same miss.
///
/// The rows span the four operations that resolve a request against one application's
/// snapshots and the forms `ApplicationName` admits (len 1..128, `^[a-zA-Z0-9_.-]+$`), and
/// one application IS stored first, so what is asserted is absence and not a blanket
/// refusal. `CreateApplicationSnapshot` words the same refusal its own way, so its row
/// carries its own sentence.
#[tokio::test]
async fn the_snapshot_operations_refuse_an_absent_application_as_an_invalid_argument() {
    let router = router();
    create_startable(&router, "stored-for-snapshots", "FLINK-1_20").await;
    let (status, served) = call(
        &router,
        "ListApplicationSnapshots",
        &json!({"ApplicationName": "stored-for-snapshots"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case stored: an application that IS stored is served, got {status} {served}"
    );

    let longest = "n".repeat(128);
    for name in ["a", "probe.app_2-absent", longest.as_str()] {
        // (operation, request, the sentence this operation words the miss with)
        let rows: [(&str, Value, String); 4] = [
            (
                "CreateApplicationSnapshot",
                json!({"ApplicationName": name, "SnapshotName": "snap-1"}),
                format!("Application {name} does not exist under accountId {ACCOUNT}"),
            ),
            (
                "ListApplicationSnapshots",
                json!({"ApplicationName": name}),
                format!("Application '{name}' does not exist under account '{ACCOUNT}'."),
            ),
            (
                "DescribeApplicationSnapshot",
                json!({"ApplicationName": name, "SnapshotName": "snap-1"}),
                format!("Application '{name}' does not exist under account '{ACCOUNT}'."),
            ),
            (
                "DeleteApplicationSnapshot",
                json!({
                    "ApplicationName": name,
                    "SnapshotName": "snap-1",
                    "SnapshotCreationTimestamp": 1_767_225_600_i64,
                }),
                format!("Application '{name}' does not exist under account '{ACCOUNT}'."),
            ),
        ];

        for (operation, request, message) in rows {
            let (status, refused) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "InvalidArgumentException"),
                "case {operation}/{name:?}: expected 400 InvalidArgumentException, got \
                 {status} {refused}"
            );
            assert_eq!(
                error_message(&refused),
                message,
                "case {operation}/{name:?}: expected the refusal to name the absent \
                 application, got {refused}"
            );
        }
    }
}
