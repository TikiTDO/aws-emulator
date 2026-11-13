// SPDX-License-Identifier: CC0-1.0

//! The server-defaulted `StartTime` of an added exclusion window.
//!
//! The rule under test: an optional input member the model documents as defaulting to the
//! current time is settled by the SERVICE, at the instant it serves the call, so the stored
//! resource carries a concrete value and every later read reports it. For this service that
//! member is `ExclusionWindow.StartTime` — "The start of the SLO time window exclusion.
//! Defaults to current time if not specified"
//! (`smithy_query(query="doc", shape="ExclusionWindow", member="StartTime")`) — written by
//! `BatchUpdateExclusionWindows` (PATCH `/exclusion-windows`) and read back by
//! `ListServiceLevelObjectiveExclusionWindows` (GET `/slo/{Id}/exclusion-windows`).
//!
//! Each rule is asserted over a TABLE of window configurations: the rows differ along what
//! the rule generalises over — which optional members the configuration states, the duration
//! unit, the duration, and (for a stated start time) where the instant sits relative to now.
//! The harness is the one `exclusion_windows` drives this surface with, so every suite speaks
//! to one set of helpers.

use serde_json::Value;

use crate::exclusion_windows::{
    batch, create_slo, keys_of, list, window, windows_of, DURATION_UNITS,
};
use crate::slo_lifecycle::router;

/// Now, in epoch seconds, as the wire renders a `Timestamp` on this protocol.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// The `StartTime` the one window of `page` reports, or `None` when it reports none.
fn reported_start_time(page: &Value) -> Option<f64> {
    page["ExclusionWindows"][0]["StartTime"].as_f64()
}

/// Window configurations that OMIT `StartTime`, spanning the duration units, the durations and
/// the other optional members the shape admits.
fn omitted_start_time_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minute, duration 1, no other optional member",
            window(DURATION_UNITS[0], 1, None, None, None),
        ),
        (
            "hour, duration 6, with a reason",
            window(DURATION_UNITS[1], 6, None, None, Some("migration")),
        ),
        (
            "day, cron recurrence, with a reason",
            window(
                DURATION_UNITS[2],
                2,
                None,
                Some("cron(0 0 ? * SUN *)"),
                Some("Weekly maintenance"),
            ),
        ),
        (
            "month, rate recurrence, no reason",
            window(DURATION_UNITS[3], 1, None, Some("rate(30 days)"), None),
        ),
        (
            "day, duration 365, reason at the length maximum",
            window(
                DURATION_UNITS[2],
                365,
                None,
                None,
                // `ExclusionReason` is `length 1..1024`
                // (`smithy_query(shape="ExclusionWindow")`, member `Reason`).
                Some(&"r".repeat(1024)),
            ),
        ),
    ]
}

/// A window added without `StartTime` is stored starting at the instant the add was served, so
/// a later read reports that instant rather than no start at all.
#[tokio::test]
async fn an_omitted_start_time_defaults_to_the_instant_the_add_was_served() {
    let router = router();
    for (index, (label, sent)) in omitted_start_time_table().into_iter().enumerate() {
        let name = format!("default-start-{index}");
        create_slo(&router, &name).await;

        let before = now_seconds();
        let (status, updated) =
            batch(&router, std::slice::from_ref(&name), std::slice::from_ref(&sent), &[]).await;
        let after = now_seconds();
        assert_eq!(
            status, 200,
            "case {label:?}: expected the add to succeed, got {status} with {updated}"
        );

        let (status, page) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the read to succeed, got {status} with {page}"
        );
        let Some(start_time) = reported_start_time(&page) else {
            panic!("case {label:?}: expected the stored window to report a StartTime, got {page}");
        };
        // The service may state the instant at a coarser resolution than this test's own
        // clock, so the bracket is the call's, widened by one second at the lower end.
        assert!(
            start_time > before - 1.0 && start_time <= after,
            "case {label:?}: expected a StartTime in the call's own interval \
             ({before}..={after}), got {start_time}"
        );

        // It is the stored window's own start time, not something the read made up: the same
        // read again reports the same instant.
        let (_, again) = list(&router, &name, "").await;
        assert_eq!(
            reported_start_time(&again),
            Some(start_time),
            "case {label:?}: expected the repeated read to report the same StartTime, got {again}"
        );

        // And the rest of the configuration is the one sent.
        for member in ["Window", "RecurrenceRule", "Reason"] {
            assert_eq!(
                page["ExclusionWindows"][0][member],
                sent[member],
                "case {label:?}: expected {member} as sent, got {page}"
            );
        }
    }
}

/// A STATED `StartTime` is stored exactly as sent: the default settles an omission, it never
/// re-times an instant the caller chose.
#[tokio::test]
async fn a_stated_start_time_is_stored_as_sent() {
    let router = router();
    // Instants spanning the epoch itself, the past, the present second and the future.
    let table: Vec<(&str, i64)> = vec![
        ("the epoch", 0),
        ("a past instant", 1_700_000_000),
        ("now", now_seconds() as i64),
        ("a future instant", now_seconds() as i64 + 86_400),
    ];
    for (index, (label, stated)) in table.into_iter().enumerate() {
        let name = format!("stated-start-{index}");
        create_slo(&router, &name).await;
        let sent = window(
            DURATION_UNITS[index % DURATION_UNITS.len()],
            index as i64 + 1,
            Some(stated),
            None,
            Some("stated start time"),
        );

        let (status, updated) =
            batch(&router, std::slice::from_ref(&name), std::slice::from_ref(&sent), &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the add to succeed, got {status} with {updated}"
        );
        let (_, page) = list(&router, &name, "").await;
        assert_eq!(
            reported_start_time(&page),
            Some(stated as f64),
            "case {label:?}: expected the StartTime sent ({stated}) to be stored as sent, got {page}"
        );
    }
}

/// The defaulted start time is part of the window's identity: a removal names the window by
/// the configuration the read reports for it, so re-sending the configuration with `StartTime`
/// still omitted matches no stored window and leaves the set as it was.
#[tokio::test]
async fn the_defaulted_start_time_is_what_a_removal_names_the_window_by() {
    let router = router();
    for (index, (label, sent)) in omitted_start_time_table().into_iter().enumerate() {
        let name = format!("default-removal-{index}");
        create_slo(&router, &name).await;
        let (status, updated) =
            batch(&router, std::slice::from_ref(&name), std::slice::from_ref(&sent), &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the add to succeed, got {status} with {updated}"
        );

        let (_, page) = list(&router, &name, "").await;
        let stored = windows_of(&page);
        assert_eq!(
            stored.len(),
            1,
            "case {label:?}: expected the SLO to hold the one window added, got {page}"
        );

        // The configuration as SENT no longer names a stored window: it states no start time,
        // and every stored window has one. Naming no window takes nothing away, so the call
        // succeeds and the set is still the one the read reported.
        let (status, answer) =
            batch(&router, std::slice::from_ref(&name), &[], std::slice::from_ref(&sent)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a removal that omits StartTime to be a no-op, \
             got {status} with {answer}"
        );
        let (_, page) = list(&router, &name, "").await;
        assert_eq!(
            keys_of(&page),
            keys_of(&json_page(&stored)),
            "case {label:?}: expected the unmatched removal to leave the set as it was, got {page}"
        );

        // The configuration the READ reports does name it, so the window is addressable.
        let (status, removed) = batch(&router, std::slice::from_ref(&name), &[], &stored).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the removal by the reported configuration to succeed, \
             got {status} with {removed}"
        );
        let (_, page) = list(&router, &name, "").await;
        assert_eq!(
            keys_of(&page),
            Vec::new(),
            "case {label:?}: expected an empty set after the removal, got {page}"
        );
    }
}

/// One call settles the default ONCE: every SLO the request names gains a window starting at
/// the same instant, so one add is one window however many SLOs it is applied to.
#[tokio::test]
async fn one_call_settles_one_start_time_for_every_slo_it_names() {
    let router = router();
    let names: Vec<String> = (0..3).map(|slot| format!("shared-start-{slot}")).collect();
    for name in &names {
        create_slo(&router, name).await;
    }
    let sent = window(DURATION_UNITS[1], 4, None, None, Some("one batch"));

    let (status, updated) = batch(&router, &names, std::slice::from_ref(&sent), &[]).await;
    assert_eq!(
        status, 200,
        "case shared: expected the add to succeed, got {status} with {updated}"
    );

    let mut instants: Vec<Option<f64>> = Vec::new();
    for name in &names {
        let (status, page) = list(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the read to succeed, got {status} with {page}"
        );
        instants.push(reported_start_time(&page));
    }
    assert!(
        instants[0].is_some() && instants.iter().all(|instant| *instant == instants[0]),
        "case shared: expected one concrete StartTime for every SLO of the call, got {instants:?}"
    );
}

/// `windows` as a list page reports them, so a set read earlier can be compared with
/// [`keys_of`] against one read later.
fn json_page(windows: &[Value]) -> Value {
    serde_json::json!({ "ExclusionWindows": windows })
}
