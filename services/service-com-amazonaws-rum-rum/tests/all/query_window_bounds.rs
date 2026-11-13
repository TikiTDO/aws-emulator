// SPDX-License-Identifier: CC0-1.0

//! The TIME WINDOW `GetAppMonitorData` answers a query against.
//!
//! The rule under test: a bound the caller SENT selects events at exactly the instants it
//! names — in the unit `TimeRange` carries, which recorded behaviour pins as MILLISECONDS
//! since the epoch, the same unit a reported event's `event_timestamp` is in — and the "If
//! you omit this, the time range extends to the time that this operation is performed"
//! reading of `TimeRange$Before` belongs to the member's modelled `@default` alone — the
//! value the deserializer fills in for a request that left the member out.
//! `QueryTimestamp` is a `long` with no `@range`
//! (`smithy_query(query="shape", shape="TimeRange,QueryTimestamp")`), so an instant before
//! the epoch is a bound a client may send, and a window that covers no stored event
//! reports none rather than being widened to now.
//!
//! The rows span what that rule generalises over — the value of each bound and whether
//! `Before` was sent at all: windows wholly before the epoch (one of them covering
//! nothing), a window of a single pre-epoch instant, an inverted one, windows spanning the
//! epoch and reaching into the future, bounds a single MILLISECOND inside and outside an
//! event's instant, a `Before` of the modelled default, and an omitted `Before` from two
//! different starts.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator};

/// The monitor every case below queries.
const MONITOR: &str = "window-monitor";

/// `2024-01-01T00:00:00Z` in seconds since the epoch: an ordinary instant in the past.
const PAST: i64 = 1_704_067_200;

/// `9999-12-31T23:59:59Z` in seconds since the epoch: the last instant RFC 3339 can spell,
/// and an instant no clock this emulator reads has reached.
const FUTURE: i64 = 253_402_300_799;

/// One second before `0001-01-01T00:00:00Z`, the first instant RFC 3339 can spell: an
/// extreme a client may deliver (`RumEvent$timestamp` carries no `@range`), and a bound the
/// window still has to place it by.
const FAR_PAST: i64 = -62_135_596_801;

/// How many milliseconds one second is: an event's instant travels as the epoch SECONDS
/// restJson1 carries a `Timestamp` as, while the window's bounds are epoch MILLISECONDS, so
/// a row names one of the fixture's instants by converting it ([`millis`]).
const MILLIS_PER_SECOND: i64 = 1_000;

/// The window bound at the whole second `seconds`, in the milliseconds `TimeRange` carries.
fn millis(seconds: i64) -> i64 {
    seconds * MILLIS_PER_SECOND
}

/// The instants the fixture events occupy, in seconds since the epoch and in the order they
/// are delivered (which is the order a query reports them in: oldest first).
///
/// Each event's index in this list, 1-based, is what a row's expectation names.
fn instants() -> Vec<i64> {
    vec![FAR_PAST, -300, -200, -100, 0, PAST, FUTURE]
}

/// The `id` of the fixture event at `index`: 36 characters matching `RumEvent$id`'s
/// pattern `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
fn id_of(index: usize) -> String {
    format!("{index:08x}-1234-4234-8234-123456789012")
}

/// One monitor holding one event at each of [`instants`], ready to query.
async fn seeded() -> MultiServiceRouter {
    let (router, _engine) = emulator();
    let monitor_id = create(
        &router,
        "seed",
        json!({"Name": MONITOR, "Domain": "window.example.com"}),
    )
    .await;
    let events: Vec<Value> = instants()
        .into_iter()
        .enumerate()
        .map(|(position, instant)| {
            json!({
                "id": id_of(position + 1),
                "timestamp": instant,
                "type": "com.amazon.rum.page_view_event",
                "details": "{\"pageId\":\"/home\"}",
            })
        })
        .collect();
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(json!({
            "BatchId": "11111111-2222-4333-8444-555555555555",
            "AppMonitorDetails": {"name": MONITOR, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {
                "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
            },
            "RumEvents": events,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"seed\": expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    router
}

/// The `id` of each event `GetAppMonitorData` reports for the window `[after, before]`,
/// both in milliseconds since the epoch, in the order it reported them. `before` of `None`
/// leaves the member out, which is what its modelled default stands for.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    after: i64,
    before: Option<i64>,
) -> Vec<String> {
    let mut time_range = json!({"After": after});
    if let Some(before) = before {
        time_range["Before"] = json!(before);
    }
    let reply = call(
        router,
        "POST",
        &format!("/appmonitor/{MONITOR}/data"),
        Some(json!({"TimeRange": time_range})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    answered["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Events array, got {answered}"))
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected each event to be a JSON string, got {entry}")
            });
            let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected each event to carry a JSON document, got \
                     {text:?} ({error})"
                )
            });
            document["event_id"].as_str().unwrap_or_default().to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_window_the_request_names_is_the_window_it_is_answered_against() {
    let router = seeded().await;

    // (case, After, Before, the 1-based fixture indices the window covers), every bound in
    // milliseconds since the epoch.
    let table: Vec<(&str, i64, Option<i64>, Vec<usize>)> = vec![
        ("a window wholly before the epoch", millis(-300), Some(millis(-100)), vec![2, 3, 4]),
        // The window the issue's request named: it covers no stored event, so the query
        // reports none instead of running on to now.
        ("a negative window covering no event", millis(-250), Some(millis(-210)), vec![]),
        ("one pre-epoch instant", millis(-200), Some(millis(-200)), vec![3]),
        ("an inverted negative window", millis(-100), Some(millis(-300)), vec![]),
        (
            "a negative window up to the millisecond before the epoch",
            millis(-300),
            Some(-1),
            vec![2, 3, 4],
        ),
        // One millisecond is the resolution of a bound, so a window that stops just short
        // of an event's instant leaves it out and one that starts just past it does too.
        (
            "a window ending one millisecond before an event's instant",
            millis(-300),
            Some(millis(-200) - 1),
            vec![2],
        ),
        (
            "a window starting one millisecond after an event's instant",
            millis(-300) + 1,
            Some(millis(-100)),
            vec![3, 4],
        ),
        (
            "a window of the single millisecond an event is at",
            millis(-100),
            Some(millis(-100)),
            vec![4],
        ),
        (
            "one instant far before every year RFC 3339 can spell",
            millis(FAR_PAST),
            Some(millis(FAR_PAST)),
            vec![1],
        ),
        (
            "the whole pre-epoch span",
            millis(FAR_PAST),
            Some(-1),
            vec![1, 2, 3, 4],
        ),
        ("a window spanning the epoch", millis(-100), Some(millis(PAST)), vec![4, 5, 6]),
        (
            "a window reaching into the future",
            millis(PAST),
            Some(millis(FUTURE)),
            vec![6, 7],
        ),
        // `Before` at its modelled `@default` is what an omitted member carries, so these
        // two rows are the same request: the window ends NOW, which is after every
        // fixture instant but the future one.
        (
            "a Before of the modelled default",
            millis(-300),
            Some(0),
            vec![2, 3, 4, 5, 6],
        ),
        ("an omitted Before", millis(-300), None, vec![2, 3, 4, 5, 6]),
        ("an omitted Before from the epoch", 0, None, vec![5, 6]),
    ];

    for (case, after, before, expected) in table {
        let ids = reported(&router, case, after, before).await;
        assert_eq!(
            ids,
            expected.iter().copied().map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected the events the window [{after}, {before:?}] covers, \
             got {ids:?}"
        );
    }
}
