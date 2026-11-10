// SPDX-License-Identifier: CC0-1.0

//! What `GetRecommendations` reports as the analysed profile span while the aggregation a
//! posted agent profile is owed has not fallen due yet, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: every read that reports a profiling group's profile
//! DATA counts a posted agent profile only once, as of the instant of the call, the
//! service has collected it into an aggregated profile — the window a profile lands in is
//! still open when the post returns, and its aggregate falls due a span after that window
//! closes. So the span `GetRecommendations` reports its analysis is about is the earliest
//! and latest receipt times of the AGGREGATED profiles the group retains inside the
//! requested window, and a window retaining no such profile covers nothing, leaving the
//! requested bounds as the analysed span — the same profile `GetProfile` answers its
//! no-data 404 for and `ListProfileTimes` reports no profile time for.
//!
//! Rows vary what the rule generalises over: how many profiles the group holds, which of
//! them are aggregated and which still pending, where each receipt sits relative to the
//! requested window (inside, on a bound, outside), how wide and where the window is, and
//! the size of the posted payload.
//!
//! The emulator, the HTTP plumbing and the receipt placement are the shared
//! [`crate::profiling_group_lifecycle`] and [`crate::profile_times`] helpers, not a fork
//! of them.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::{Answer, Harness, json_profile, servable_window};

/// The fixed text a 404 about absent profile DATA carries
/// (`state::NO_PROFILES_IN_TIME_RANGE`), restated here so this suite pins the wire text
/// rather than importing the value it compares against.
const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// A window wide enough to contain any instant these cases place a receipt at, in the
/// ISO 8601 form the modelled `date-time` query members carry.
const WIDE_START: &str = "2000-01-01T00:00:00Z";
const WIDE_END: &str = "2099-01-01T00:00:00Z";

/// 2020-06-01T00:00:00Z — a receipt whose aggregation fell due years ago, the kind of
/// instant the rest of this crate's analysis cases place profiles at.
const LONG_AGGREGATED: i64 = 1_590_969_600;

/// The length of the finest aggregation window this service aggregates at, in whole
/// seconds, from the service's own
/// [`state::finest_aggregation_period_seconds`](service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::finest_aggregation_period_seconds)
/// — so these cases place receipts by the window length the emulator aggregates at rather
/// than by a number written out here.
fn finest_period() -> i64 {
    service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::finest_aggregation_period_seconds()
}

/// The clock's current instant in whole seconds — what a post over HTTP stamps a receipt
/// with.
fn clock_seconds() -> i64 {
    DateTime::from(std::time::SystemTime::now()).secs()
}

/// The start of the finest aggregation window containing `at`, aligned to UTC.
fn window_start(at: i64) -> i64 {
    at - at.rem_euclid(finest_period())
}

/// An instant as the ISO 8601 form the modelled `date-time` query members use.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

async fn get_recommendations(harness: &Harness, name: &str, start: i64, end: i64) -> Answer {
    harness
        .send(
            "GET",
            &format!(
                "/internal/profilingGroups/{name}/recommendations?startTime={}&endTime={}",
                iso(start),
                iso(end)
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

async fn list_profile_times(harness: &Harness, name: &str, period: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!(
                "/profilingGroups/{name}/profileTimes?startTime={WIDE_START}&endTime={WIDE_END}\
                 &period={period}"
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

/// One timestamp member of the response, as epoch seconds. restJson1 carries a
/// `Timestamp` either as epoch seconds or in the ISO 8601 form, and these cases are about
/// WHICH instant is reported, so both renderings read back the same way.
fn timestamp(answer: &Answer, member: &str, case: &str) -> f64 {
    let body = answer.json();
    let value = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} is required, got {body}"));
    if let Some(seconds) = value.as_f64() {
        return seconds;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a timestamp, got {value}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} must parse as a date-time: {error}")
        })
        .as_secs_f64()
}

/// Whether two epoch-second instants are the same instant as a response can express it:
/// the ISO 8601 `date-time` rendering a timestamp member carries is written to
/// MICROSECOND resolution, so a reported instant can sit up to one whole microsecond
/// from the instant the store holds, and an epoch-second value of this decade carried as
/// a 64-bit float resolves to only a fraction of a microsecond on top of that. Two
/// values within one microsecond-and-a-bit of each other are therefore the one reported
/// instant; anything a millisecond or more apart — the resolution a different instant
/// would differ by — is not.
fn same_instant(left: f64, right: f64) -> bool {
    (left - right).abs() < 2e-6
}

/// How many profile times one `ListProfileTimes` answer reports.
fn reported_times(answer: &Answer) -> usize {
    answer
        .json()
        .get("profileTimes")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("no profileTimes array in {}", answer.json()))
}

/// The receipt times one group's stored profiles carry right now, as epoch seconds —
/// read, never written, so a case can assert the span against the instants the store
/// actually holds.
fn stored_receipts(harness: &Harness, name: &str) -> Vec<f64> {
    harness
        .engine()
        .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
        .lock()
        .groups
        .get(name)
        .unwrap_or_else(|| panic!("no stored group named {name}"))
        .profiles
        .iter()
        .map(|profile| profile.received_at.as_secs_f64())
        .collect()
}

/// One row: the receipts a group's retained profiles are placed at, the window asked
/// about, and the span the analysis must report.
struct Row {
    label: &'static str,
    /// The stored receipt times, oldest first, as epoch seconds.
    receipts: Vec<i64>,
    /// The requested `[startTime, endTime]`.
    window: (i64, i64),
    /// The span the analysis must report, or `None` when the window covers no aggregated
    /// profile and the requested bounds must stand.
    expected: Option<(i64, i64)>,
}

fn rows() -> Vec<Row> {
    let now = clock_seconds();
    let period = finest_period();
    // A receipt stamped with the clock is in a window that is still OPEN, so its
    // aggregate is not due until that window closes plus the aggregation pass: pending at
    // the instant any read in this suite runs.
    let pending = now;
    // Two receipts whose window closed one and two whole windows ago, so their
    // aggregation fell due before this suite started, whatever the phase of the clock
    // inside the current window.
    let due_recent = window_start(now) - 2 * period;
    let due_older = window_start(now) - 3 * period;

    vec![
        Row {
            label: "one profile whose aggregation is not due",
            receipts: vec![pending],
            window: (pending - 60, pending + 60),
            expected: None,
        },
        Row {
            label: "three profiles, none of them aggregated yet",
            receipts: vec![pending, pending, pending],
            window: (pending - 60, pending + 60),
            expected: None,
        },
        Row {
            label: "one profile whose aggregation has fallen due",
            receipts: vec![due_recent],
            window: (due_recent - 60, pending + 60),
            expected: Some((due_recent, due_recent)),
        },
        Row {
            label: "an aggregated profile and a later pending one",
            receipts: vec![due_recent, pending],
            window: (due_recent - 60, pending + 60),
            expected: Some((due_recent, due_recent)),
        },
        Row {
            label: "two aggregated profiles and a later pending one",
            receipts: vec![due_older, due_recent, pending],
            window: (due_older - 60, pending + 60),
            expected: Some((due_older, due_recent)),
        },
        Row {
            label: "a window holding only the pending profile",
            receipts: vec![due_older, pending],
            window: (due_recent, pending + 60),
            expected: None,
        },
        Row {
            label: "a window closed exactly on the aggregated receipt",
            receipts: vec![due_recent, pending],
            window: (due_recent, due_recent + 1),
            expected: Some((due_recent, due_recent)),
        },
        Row {
            label: "a profile aggregated years ago beside a pending one",
            receipts: vec![LONG_AGGREGATED, pending],
            window: (LONG_AGGREGATED - 3_600, pending + 60),
            expected: Some((LONG_AGGREGATED, LONG_AGGREGATED)),
        },
    ]
}

#[tokio::test]
async fn the_analysed_span_counts_only_profiles_whose_aggregation_is_due() {
    let harness = Harness::new();

    // The rows are rebuilt for every case, so the receipt a row leaves PENDING is read
    // from the clock immediately before that row's own read: its aggregation then cannot
    // fall due (the window holding it is still open, and the aggregation pass runs after
    // that window closes) while the row runs.
    for index in 0..rows().len() {
        let row = rows().remove(index);
        let case = row.label;
        let name = format!("recommendations-pending-{index}");
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        // The payload differs in size and bytes from row to row, so no row's span
        // depends on the shape of what was posted.
        for post in 0..row.receipts.len() {
            let body = json_profile(
                1_600_000_000_000 + i64::try_from(post).unwrap_or(0),
                index * 8 + post,
            );
            assert_eq!(
                harness
                    .post_agent_profile(&name, "application/json", None, body)
                    .await
                    .status,
                204,
                "case {case:?}: post {post} must be stored"
            );
        }
        place_receipts(&harness, &name, &row.receipts);

        let (start, end) = row.window;
        let answer = get_recommendations(&harness, &name, start, end).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        assert_eq!(
            body.get("profilingGroupName")
                .and_then(serde_json::Value::as_str),
            Some(name.as_str()),
            "case {case:?}: profilingGroupName must come from the stored group, got {body}"
        );
        let (expected_start, expected_end) = row.expected.unwrap_or((start, end));
        assert_eq!(
            timestamp(&answer, "profileStartTime", case),
            expected_start as f64,
            "case {case:?}: expected profileStartTime {}, got {body}",
            iso(expected_start)
        );
        assert_eq!(
            timestamp(&answer, "profileEndTime", case),
            expected_end as f64,
            "case {case:?}: expected profileEndTime {}, got {body}",
            iso(expected_end)
        );

        // The read reported on the store without touching it: the same profiles are
        // retained, at the same instants, for the next caller.
        let receipts_after = stored_receipts(&harness, &name);
        let placed: Vec<f64> = row.receipts.iter().map(|at| *at as f64).collect();
        assert_eq!(
            receipts_after, placed,
            "case {case:?}: GetRecommendations must leave the retained profiles alone, \
             got {receipts_after:?}"
        );
    }
}

#[tokio::test]
async fn a_pending_profile_is_reported_by_no_profile_read_and_an_aggregated_one_by_all() {
    let harness = Harness::new();
    let name = "pending-span";
    assert_eq!(
        create_group(&harness, name).await.status,
        201,
        "create must succeed"
    );
    assert_eq!(
        harness
            .post_agent_profile(
                name,
                "application/json",
                None,
                json_profile(1_600_000_000_000, 0)
            )
            .await
            .status,
        204,
        "the post must succeed"
    );

    let now = clock_seconds();
    // A window wide enough to hold the receipt both before and after the aggregation is
    // let fall due, and narrow enough for `GetProfile`'s documented maximum range.
    let (start, end) = (now - 2 * 3_600, now + 60);

    // While the aggregation is pending the three reads agree: no profile data exists in
    // the window yet.
    let case = "aggregation pending";
    let pending = get_recommendations(&harness, name, start, end).await;
    assert_eq!(
        pending.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {}",
        pending.status,
        String::from_utf8_lossy(&pending.body)
    );
    assert_eq!(
        timestamp(&pending, "profileStartTime", case),
        start as f64,
        "case {case:?}: a window covering no aggregated profile must report its own start, \
         got {}",
        pending.json()
    );
    assert_eq!(
        timestamp(&pending, "profileEndTime", case),
        end as f64,
        "case {case:?}: a window covering no aggregated profile must report its own end, \
         got {}",
        pending.json()
    );
    let profile = harness.get_profile(name, &servable_window()).await;
    assert_eq!(
        profile.status,
        404,
        "case {case:?}: GetProfile must answer 404, got {} {}",
        profile.status,
        String::from_utf8_lossy(&profile.body)
    );
    assert_eq!(
        profile.error_message(),
        NO_PROFILES_IN_TIME_RANGE,
        "case {case:?}: expected the fixed no-data text, got {:?}",
        profile.error_message()
    );
    let times = list_profile_times(&harness, name, "PT5M").await;
    assert_eq!(
        reported_times(&times),
        0,
        "case {case:?}: ListProfileTimes must report no profile time, got {}",
        String::from_utf8_lossy(&times.body)
    );

    // Once the aggregation its window owes has fallen due, the SAME three reads agree the
    // other way: the profile is there, and the analysed span is its receipt instant.
    harness.complete_aggregation(name);
    let case = "aggregation due";
    let receipts = stored_receipts(&harness, name);
    let receipt = *receipts
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the group must retain the posted profile"));
    let due = get_recommendations(&harness, name, start, end).await;
    assert_eq!(
        due.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {}",
        due.status,
        String::from_utf8_lossy(&due.body)
    );
    for member in ["profileStartTime", "profileEndTime"] {
        let reported = timestamp(&due, member, case);
        assert!(
            same_instant(reported, receipt),
            "case {case:?}: expected the stored receipt {receipt} as {member}, got {reported} \
             in {}",
            due.json()
        );
    }
    let profile = harness.get_profile(name, &servable_window()).await;
    assert_eq!(
        profile.status,
        200,
        "case {case:?}: GetProfile must serve the aggregated profile, got {} {}",
        profile.status,
        String::from_utf8_lossy(&profile.body)
    );
    let times = list_profile_times(&harness, name, "PT5M").await;
    assert!(
        reported_times(&times) >= 1,
        "case {case:?}: ListProfileTimes must report the aggregated profile, got {}",
        String::from_utf8_lossy(&times.body)
    );
}
