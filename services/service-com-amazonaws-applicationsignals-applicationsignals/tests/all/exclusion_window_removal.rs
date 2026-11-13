// SPDX-License-Identifier: CC0-1.0

//! The removal half of `BatchUpdateExclusionWindows` (PATCH `/exclusion-windows`), and the
//! resolution the service settles an omitted `StartTime` at.
//!
//! The rules under test:
//!
//! * each list of a batch update is a SET operation on the window set of every SLO its
//!   `SloIds` names — an addition the SLO already holds changes nothing, and a removal naming
//!   a configuration it does not hold takes nothing away — so a request whose removals match
//!   nothing is a SUCCESS that reports those ids in `SloIds` beside an empty `Errors`, with
//!   the set left exactly as it was, and a per-item failure (an id naming no SLO) is reported
//!   in `Errors` BESIDE the ids the request was applied to;
//! * an instant the service settles itself is stated at the resolution the wire carries it
//!   at — epoch seconds to the millisecond — so a window added without a `StartTime` reports
//!   where the exclusion began rather than the top of the second it began in.
//!
//! Each rule is asserted over a TABLE: the rows differ along what the rule generalises over —
//! which member the unmatched removal differs in, how many entries each list carries, how
//! many SLOs one call names and which id form names them, and whether the SLO holds windows
//! at all.
//!
//! The harness is the one `exclusion_windows` drives this surface with, so every suite speaks
//! to one set of helpers.

use serde_json::Value;

use crate::exclusion_windows::{
    applied_ids, batch, create_slo, keys_of, list, slo_arn, window, window_table, windows_of,
    DURATION_UNITS, MAX_WINDOWS_PER_SLO,
};
use crate::slo_lifecycle::router;

/// Now, in epoch seconds, as the wire renders a `Timestamp` on this protocol.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// How far from a whole second an instant must sit for the case below to be able to tell a
/// sub-second answer from a whole-second one: a hundredth of a second, ten times the
/// millisecond the service states an instant to.
const CLEAR_OF_THE_BOUNDARY: f64 = 0.01;

/// Now, once the clock reads an instant whose fractional second is clear of a second boundary
/// ([`CLEAR_OF_THE_BOUNDARY`]).
///
/// The rule under test is about the SUB-SECOND part of an instant the service settles itself,
/// so the call has to be made at an instant that HAS one: an answer truncated to the whole
/// second would then fall measurably before this sample, which is what the case asserts it
/// does not. The CONDITION is polled for rather than waited out — it holds on the first look
/// in all but a hundredth of runs — and the deadline is generous.
fn now_clear_of_a_second_boundary() -> f64 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let now = now_seconds();
        let into_the_second = now.fract();
        if (CLEAR_OF_THE_BOUNDARY..1.0 - CLEAR_OF_THE_BOUNDARY).contains(&into_the_second) {
            return now;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case setup: the clock never read an instant clear of a second boundary"
        );
        std::thread::yield_now();
    }
}

/// The configuration every row below stores before it asks for a removal: one window stating
/// every optional member, so a row can differ from it in any single one.
fn stored_window() -> Value {
    window(
        DURATION_UNITS[1],
        2,
        Some(1_762_000_000),
        Some("cron(0 0 ? * SUN *)"),
        Some("scheduled maintenance"),
    )
}

/// One row: a removal list that names no window the SLO holds.
struct Row {
    label: &'static str,
    /// The windows the SLO holds before the removal is asked for.
    stored: Vec<Value>,
    /// The removal list the batch sends.
    remove: Vec<Value>,
}

/// The table: a removal entry differing from the stored window in each member of the
/// configuration in turn, lists of more than one entry (up to the modelled maximum), and an
/// SLO holding no windows at all.
fn unmatched_removals() -> Vec<Row> {
    let stored = stored_window();
    vec![
        Row {
            label: "differs in Window.Duration",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                3,
                Some(1_762_000_000),
                Some("cron(0 0 ? * SUN *)"),
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "differs in Window.DurationUnit",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[2],
                2,
                Some(1_762_000_000),
                Some("cron(0 0 ? * SUN *)"),
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "differs in StartTime",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                2,
                Some(1_762_000_001),
                Some("cron(0 0 ? * SUN *)"),
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "omits the StartTime the stored window states",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                2,
                None,
                Some("cron(0 0 ? * SUN *)"),
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "differs in RecurrenceRule.Expression",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                2,
                Some(1_762_000_000),
                Some("rate(7 days)"),
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "omits the RecurrenceRule the stored window states",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                2,
                Some(1_762_000_000),
                None,
                Some("scheduled maintenance"),
            )],
        },
        Row {
            label: "differs in Reason",
            stored: vec![stored.clone()],
            remove: vec![window(
                DURATION_UNITS[1],
                2,
                Some(1_762_000_000),
                Some("cron(0 0 ? * SUN *)"),
                Some("unscheduled maintenance"),
            )],
        },
        Row {
            label: "an unrelated configuration",
            stored: vec![stored.clone()],
            remove: vec![window(DURATION_UNITS[0], 1, None, None, None)],
        },
        Row {
            label: "two entries, neither matching",
            stored: vec![stored.clone()],
            remove: vec![
                window(DURATION_UNITS[0], 1, None, None, None),
                window(DURATION_UNITS[3], 1, Some(0), None, Some("month")),
            ],
        },
        Row {
            label: "a removal list at the modelled maximum, none matching",
            stored: vec![stored],
            // `RemoveExclusionWindows` is `len ..10`
            // (`smithy_query(query="operation",operation="BatchUpdateExclusionWindows")`).
            remove: (0..MAX_WINDOWS_PER_SLO)
                .map(|index| {
                    window(
                        DURATION_UNITS[index % DURATION_UNITS.len()],
                        index as i64 + 100,
                        None,
                        None,
                        None,
                    )
                })
                .collect(),
        },
        Row {
            label: "the SLO holds no window at all",
            stored: Vec::new(),
            remove: vec![window(DURATION_UNITS[0], 1, None, None, None)],
        },
    ]
}

/// A removal that names no window the SLO holds is a successful no-op: the id is reported in
/// `SloIds`, `Errors` is empty, and the SLO's window set is exactly the one a read reported
/// before the call.
#[tokio::test]
async fn a_removal_that_matches_no_stored_window_is_a_reported_no_op() {
    let router = router();
    for (index, row) in unmatched_removals().into_iter().enumerate() {
        let name = format!("unmatched-removal-{index}");
        create_slo(&router, &name).await;
        if !row.stored.is_empty() {
            let (status, updated) =
                batch(&router, std::slice::from_ref(&name), &row.stored, &[]).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the setup add to succeed, got {status} with {updated}",
                row.label
            );
        }
        let (_, before) = list(&router, &name, "").await;

        let (status, answer) = batch(&router, std::slice::from_ref(&name), &[], &row.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for a removal matching no stored window, got {status} \
             with {answer}",
            row.label
        );
        assert_eq!(
            applied_ids(&answer),
            vec![name.clone()],
            "case {:?}: expected the id to be reported as processed, got {answer}",
            row.label
        );
        assert_eq!(
            answer["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: expected no per-SLO errors, got {answer}",
            row.label
        );

        let (_, after) = list(&router, &name, "").await;
        assert_eq!(
            keys_of(&after),
            keys_of(&before),
            "case {:?}: expected the window set to be unchanged, got {after} after {before}",
            row.label
        );
    }
}

/// A removal list applies the entries that DO match and ignores the rest, in the same call as
/// the additions: what the SLO ends up holding is its old set minus the matches plus the
/// additions.
#[tokio::test]
async fn a_batch_applies_the_removals_that_match_beside_the_ones_that_do_not() {
    let router = router();
    let name = "mixed-removal-slo";
    create_slo(&router, name).await;

    let sent: Vec<Value> = window_table()
        .into_iter()
        .take(3)
        .map(|(_, value)| value)
        .collect();
    let (status, updated) = batch(&router, &[name.to_string()], &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case setup: expected the setup add to succeed, got {status} with {updated}"
    );
    let (_, page) = list(&router, name, "").await;
    let stored = windows_of(&page);
    assert_eq!(
        stored.len(),
        sent.len(),
        "case setup: expected the three windows added, got {page}"
    );

    // One entry the read reported (so it matches) and one that matches nothing, plus an
    // addition — a reconciling caller's whole desired change in one call. The addition states
    // its own `StartTime`, so the configuration it names is the same one on a repeat.
    let addition = window(
        DURATION_UNITS[0],
        7,
        Some(1_762_000_500),
        None,
        Some("added while removing"),
    );
    let remove = vec![
        stored[1].clone(),
        window(DURATION_UNITS[0], 999, None, None, Some("never stored")),
    ];
    let (status, answer) = batch(
        &router,
        &[name.to_string()],
        std::slice::from_ref(&addition),
        &remove,
    )
    .await;
    assert_eq!(
        status, 200,
        "case mixed: expected HTTP 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["Errors"].as_array().map(Vec::len),
        Some(0),
        "case mixed: expected no per-SLO errors, got {answer}"
    );

    let mut expected: Vec<Value> = vec![stored[0].clone(), stored[2].clone()];
    expected.push(addition.clone());
    let (_, page) = list(&router, name, "").await;
    assert_eq!(
        keys_of(&page),
        crate::exclusion_windows::keys_sent(&expected),
        "case mixed: expected the matched removal gone, the unmatched one ignored and the \
         addition stored, got {page}"
    );

    // And repeating the very same call converges: the addition is already held and the
    // removal already gone, so nothing changes and nothing fails.
    let (status, answer) = batch(
        &router,
        &[name.to_string()],
        std::slice::from_ref(&addition),
        &remove,
    )
    .await;
    assert_eq!(
        status, 200,
        "case repeat: expected the repeated reconciliation to succeed, got {status} with {answer}"
    );
    let (_, again) = list(&router, name, "").await;
    assert_eq!(
        keys_of(&again),
        keys_of(&page),
        "case repeat: expected the repeated call to change nothing, got {again}"
    );
}

/// One call reports every id it was applied to in `SloIds` BESIDE one `Errors` entry per id it
/// was not, whichever id form the modelled `ServiceLevelObjectiveId` admits each was named by.
#[tokio::test]
async fn an_unmatched_removal_is_reported_beside_a_per_item_failure() {
    let router = router();
    let stored = ["converging-by-name", "converging-by-arn"];
    for name in &stored {
        create_slo(&router, name).await;
    }
    let absent = "converging-absent";
    let ids = vec![
        stored[0].to_string(),
        slo_arn(stored[1]),
        absent.to_string(),
    ];

    let remove = vec![window(DURATION_UNITS[0], 1, Some(1_700_000_000), None, None)];
    let (status, answer) = batch(&router, &ids, &[], &remove).await;
    assert_eq!(
        status, 200,
        "case mixed-ids: expected HTTP 200 while at least one id was applied, got {status} with \
         {answer}"
    );
    assert_eq!(
        applied_ids(&answer),
        vec![ids[0].clone(), ids[1].clone()],
        "case mixed-ids: expected the stored ids as the caller spelled them, got {answer}"
    );
    let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        1,
        "case mixed-ids: expected one error for the id naming no SLO, got {answer}"
    );
    assert_eq!(
        errors[0]["SloId"].as_str(),
        Some(absent),
        "case mixed-ids: expected the error to name the absent id as sent, got {errors:?}"
    );
    assert_eq!(
        errors[0]["ErrorCode"].as_str(),
        Some("ResourceNotFoundException"),
        "case mixed-ids: expected the not-found code for the absent id, got {errors:?}"
    );
}

/// A `StartTime` the service settles itself is stated to the millisecond: it is an exact
/// number of milliseconds, and it falls inside the call's own interval rather than at the top
/// of the second the call fell in — which the call being made clear of a second boundary
/// ([`now_clear_of_a_second_boundary`]) is what makes the two tellable apart.
#[tokio::test]
async fn a_defaulted_start_time_is_stated_to_the_millisecond() {
    let router = router();
    // Every row of the table that omits `StartTime`, so the service is what settles it.
    let rows: Vec<(&str, Value)> = window_table()
        .into_iter()
        .filter(|(_, value)| value["StartTime"].is_null())
        .collect();
    assert!(
        !rows.is_empty(),
        "case setup: expected the window table to carry a row that omits StartTime"
    );

    for (index, (label, sent)) in rows.into_iter().enumerate() {
        let name = format!("millisecond-start-{index}");
        create_slo(&router, &name).await;

        let before = now_clear_of_a_second_boundary();
        let (status, updated) =
            batch(&router, std::slice::from_ref(&name), std::slice::from_ref(&sent), &[]).await;
        let after = now_seconds();
        assert_eq!(
            status, 200,
            "case {label:?}: expected the add to succeed, got {status} with {updated}"
        );

        let (_, page) = list(&router, &name, "").await;
        let Some(start_time) = page["ExclusionWindows"][0]["StartTime"].as_f64() else {
            panic!("case {label:?}: expected the stored window to report a StartTime, got {page}");
        };

        // The sub-millisecond part is dropped, so the value is a whole number of
        // milliseconds.
        let millis = start_time * 1_000.0;
        assert!(
            (millis - millis.round()).abs() < 1e-3,
            "case {label:?}: expected a StartTime stated to the millisecond, got {start_time}"
        );
        // And it is the instant the call was served at, to the millisecond it was truncated
        // to: at or after the millisecond the sample before the call fell in, and never after
        // the sample taken once the call had answered. The sample sits at least
        // `CLEAR_OF_THE_BOUNDARY` into its second, so an answer truncated to the whole second
        // (the sub-second part dropped) falls below this bound rather than inside it.
        assert!(
            start_time >= before - 0.001 && start_time <= after,
            "case {label:?}: expected a StartTime in the call's own interval \
             ({before}..={after}), got {start_time}"
        );
    }
}
