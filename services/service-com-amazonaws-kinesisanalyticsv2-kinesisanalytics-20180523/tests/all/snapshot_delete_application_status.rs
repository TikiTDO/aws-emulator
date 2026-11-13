// SPDX-License-Identifier: CC0-1.0

//! Which snapshot a `DeleteApplicationSnapshot` request identifies — and nothing about the
//! application's lifecycle status — is what decides the answer.
//!
//! `snapshot_deletion.rs` drives the delete against an application that is RUNNING and
//! already holds a snapshot. This suite pins the other end of that dimension: an
//! application a client has only CREATED (READY, never started, so it can hold no snapshot
//! at all) refuses a snapshot it holds no record of with exactly the same modelled error,
//! HTTP 400 `ResourceNotFoundException`, and exactly the same sentence as a RUNNING one or
//! one stopped back to READY — and with exactly the code and sentence
//! `DescribeApplicationSnapshot` gives that same miss, since both operations resolve one
//! `SnapshotName` against one application's snapshots
//! (`smithy_query(query="operation",operation="DeleteApplicationSnapshot,DescribeApplicationSnapshot")`).
//!
//! Every status this suite asserts on is read back from `DescribeApplication` and polled
//! until the clock has made it due, so a slow host waits longer rather than failing.

use std::time::Duration;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_startable, describe, error_code, error_message, listed_snapshots, router};

/// The runtime the snapshot operations are served for.
const FLINK_RUNTIME: &str = "FLINK-1_20";

/// How long a case waits for a status the clock owes before giving up. Generous on
/// purpose: the condition is polled, so the wait tracks the host rather than bounding it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before reading the status again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Requests that identify no snapshot, spanning the modelled `SnapshotName` form (len
/// 1..256, pattern `^[a-zA-Z0-9_.-]+$`) and the `SnapshotCreationTimestamp` the request
/// carries (`smithy_query(query="operation",operation="DeleteApplicationSnapshot")`): the
/// name is what is not found, so the timestamp supplied changes nothing.
///
/// (case, `SnapshotName`, `SnapshotCreationTimestamp`)
fn requests_naming_no_snapshot() -> Vec<(&'static str, String, i64)> {
    vec![
        ("shortest-name-at-epoch", "q".to_string(), 0),
        (
            "punctuated-name",
            "cleanup.candidate_7-v2".to_string(),
            1_767_225_600,
        ),
        ("digits-name-far-future", "20300102".to_string(), 4_102_444_800),
        ("longest-name", "x".repeat(256), 1_700_000_000),
    ]
}

/// Drive one lifecycle call — `StartApplication`, `StopApplication` — on `application`,
/// asserting it was accepted.
async fn served(router: &MultiServiceRouter, case: &str, operation: &str, application: &str) {
    let (status, body) = call(router, operation, &json!({"ApplicationName": application})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {operation} to be served, got {status} {body}"
    );
}

/// The `ApplicationStatus` `DescribeApplication` reports for `application` right now.
async fn reported_status(router: &MultiServiceRouter, case: &str, application: &str) -> String {
    let detail = describe(router, application).await;
    detail["ApplicationStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an ApplicationStatus, got {detail}"))
        .to_string()
}

/// Look again until `application` reports `settled`, or [`SETTLE_DEADLINE`] passes.
async fn await_status(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    settled: &str,
) {
    let give_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let observed = reported_status(router, case, application).await;
        if observed == settled {
            return;
        }
        assert!(
            tokio::time::Instant::now() < give_up_at,
            "case {case}: expected {application} to settle at {settled}, got {observed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `DeleteApplicationSnapshot` naming a snapshot the application holds no record of is
/// refused as the modelled `ResourceNotFoundException`, in the sentence that names the
/// snapshot asked for and the application it was looked for in, whatever lifecycle status
/// the application is in and whether or not it holds other snapshots. The refusal removes
/// and mints nothing, and `DescribeApplicationSnapshot` answers the same miss identically.
#[tokio::test]
async fn a_snapshot_miss_is_refused_the_same_way_whatever_status_the_application_is_in() {
    let router = router();

    // READY, never started: the state a client is in straight after CreateApplication, so
    // this application can hold no snapshot at all.
    let fresh = "snap-miss-ready";
    create_startable(&router, fresh, FLINK_RUNTIME).await;
    await_status(&router, "ready-never-started", fresh, "READY").await;

    // RUNNING, holding no snapshot: started and left to settle, nothing snapshotted.
    let running = "snap-miss-running";
    create_startable(&router, running, FLINK_RUNTIME).await;
    served(&router, "running-no-snapshots", "StartApplication", running).await;
    await_status(&router, "running-no-snapshots", running, "RUNNING").await;

    // READY again, holding one snapshot taken while it ran: the miss is about the NAME
    // asked for, not about an application that holds nothing.
    let stopped = "snap-miss-stopped";
    let held = "taken-while-running".to_string();
    create_startable(&router, stopped, FLINK_RUNTIME).await;
    served(&router, "ready-after-stop", "StartApplication", stopped).await;
    await_status(&router, "ready-after-stop", stopped, "RUNNING").await;
    let (status, taken) = call(
        &router,
        "CreateApplicationSnapshot",
        &json!({"ApplicationName": stopped, "SnapshotName": held}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ready-after-stop: CreateApplicationSnapshot got {status} {taken}"
    );
    served(&router, "ready-after-stop", "StopApplication", stopped).await;
    await_status(&router, "ready-after-stop", stopped, "READY").await;

    // (case, application, the status it is in, the snapshots it holds)
    let applications: [(&str, &str, &str, Vec<String>); 3] = [
        ("ready-never-started", fresh, "READY", Vec::new()),
        ("running-no-snapshots", running, "RUNNING", Vec::new()),
        (
            "ready-after-stop",
            stopped,
            "READY",
            vec![held.clone()],
        ),
    ];

    for (case, application, status, holds) in applications {
        let observed = reported_status(&router, case, application).await;
        assert_eq!(
            observed, status,
            "case {case}: expected the application to be {status}, got {observed:?}"
        );

        for (row, snapshot, timestamp) in requests_naming_no_snapshot() {
            let case = format!("{case}/{row}");
            let expected = format!(
                "Snapshot '{snapshot}' does not exist for application {application}."
            );

            let (http, refused) = call(
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
                (http, error_code(&refused).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: expected 400 ResourceNotFoundException, got {http} {refused}"
            );
            assert_eq!(
                error_message(&refused),
                expected,
                "case {case}: expected the refusal to name the snapshot asked for and the \
                 application it was looked for in, got {refused}"
            );

            // The reader of one snapshot by name refuses the same miss in the same words.
            let (read_http, described) = call(
                &router,
                "DescribeApplicationSnapshot",
                &json!({"ApplicationName": application, "SnapshotName": snapshot}),
            )
            .await;
            assert_eq!(
                (read_http, error_code(&described), error_message(&described)),
                (http, error_code(&refused), error_message(&refused)),
                "case {case}: expected DescribeApplicationSnapshot to word the same miss \
                 identically, got {read_http} {described}"
            );

            // A refused delete is not a write: the snapshots the application holds are
            // exactly the ones it held before.
            assert_eq!(
                listed_snapshots(&router, &case, application).await,
                holds,
                "case {case}: expected the refused delete to leave the stored snapshots \
                 untouched"
            );
            let settled = reported_status(&router, &case, application).await;
            assert_eq!(
                settled, status,
                "case {case}: expected the refused delete to leave the application {status}, \
                 got {settled:?}"
            );
        }
    }
}

/// Delete-if-exists is idempotent from a READY application a client has only created: the
/// first call refuses the snapshot as not found, and so does every call after it, with the
/// same status, code and sentence — never a 501 or any other unmodelled answer.
#[tokio::test]
async fn deleting_the_same_absent_snapshot_again_answers_the_same_refusal() {
    let router = router();
    let application = "snap-miss-repeated";
    create_startable(&router, application, FLINK_RUNTIME).await;
    await_status(&router, "repeated", application, "READY").await;

    for (row, snapshot, timestamp) in requests_naming_no_snapshot() {
        let mut answers = Vec::new();
        for attempt in 0..3 {
            let (http, body) = call(
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
                (http, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {row}/attempt {attempt}: expected 400 ResourceNotFoundException, got \
                 {http} {body}"
            );
            answers.push((http, error_code(&body), error_message(&body)));
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {row}: expected every attempt to answer the same refusal, got {answers:?}"
        );
    }
}
