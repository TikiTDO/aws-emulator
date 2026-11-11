// SPDX-License-Identifier: CC0-1.0

//! A status change the CLOCK owed a harvest job is recorded with the instant it fell DUE,
//! never with the instant some client happened to look, driven over HTTP through the same
//! router the emulator boots.
//!
//! `ScheduleConfiguration` is when the job runs — `CreateHarvestJobRequest$
//! ScheduleConfiguration` ("The configuration for when the harvest job should run,
//! including start and end times"), from
//! `smithy_query(query="shape", shape="CreateHarvestJobRequest")` — and `ModifiedAt` is
//! "The date and time the harvest job was modified", from
//! `smithy_query(query="shape", shape="GetHarvestJobResponse")`. A job's status changes are
//! therefore modifications its own schedule makes, so:
//!
//! 1. Once the window's START has passed, `ModifiedAt` is that start instant — whatever
//!    delay the first read came after, and whichever reporting operation (`GetHarvestJob`,
//!    `ListHarvestJobs`) materialised it.
//! 2. Once the window's END has passed, `ModifiedAt` is that end instant, on the same
//!    terms.
//! 3. For a window that was already open when the job was created, the due instant is the
//!    emulator's own non-zero span after the create rather than a scheduled instant, so two
//!    identical jobs read after DIFFERENT delays report the same gap between `CreatedAt`
//!    and `ModifiedAt` — the stamp is a function of the job, not of the reader.
//!
//! Each rule is asserted over a TABLE of windows and read instants rather than over one
//! recorded request.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client (`channel_lifecycle`) and the
// one `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::Emulator;
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The hierarchy every row hangs its job off, all matching the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hjm-group";
const CHANNEL: &str = "hjm-channel";
const ENDPOINT: &str = "hjm-endpoint";
const MANIFEST: &str = "index";
const JOB: &str = "hjm-job";

/// The statuses of `HarvestJobStatus` these rows settle in.
const IN_PROGRESS: &str = "IN_PROGRESS";
const FAILED: &str = "FAILED";

/// How close two instants have to be to count as the same instant. Ten milliseconds is far
/// below the lag every row puts between a transition falling due and the read that
/// observes it, so a stamp taken from the read instant cannot slip through.
const SAME_INSTANT: f64 = 0.01;

/// The least a read lags the transition it observes, so a `ModifiedAt` stamped with the
/// READ instant would miss the due instant by at least this much.
const LEAST_READ_LAG: f64 = 0.5;

/// A fresh emulator holding the channel group, channel and origin endpoint a harvest job
/// needs, and no harvest job yet.
async fn emulator_with_endpoint(case: &str) -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": MANIFEST}],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateOriginEndpoint expected 200, got {status} {body}"
    );
    emulator
}

/// The job collection of the suite's endpoint.
fn collection() -> String {
    format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob")
}

/// Create the suite's job over the window `[start, end]` (epoch seconds) and return the
/// create's own response.
async fn create_job(emulator: &Emulator, case: &str, start: f64, end: f64) -> Value {
    let mut body = harvest_body(start, end, &[MANIFEST]);
    body["HarvestJobName"] = json!(JOB);
    let (status, created) = emulator.call("POST", &collection(), Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateHarvestJob expected 200, got {status} {created}"
    );
    created
}

/// `GetHarvestJob` on the suite's job.
async fn read_job(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = emulator
        .call("GET", &format!("{}/{JOB}", collection()), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetHarvestJob expected 200, got {status} {body}"
    );
    body
}

/// The suite's job as `ListHarvestJobs` projects it — the OTHER reporting path through the
/// same materialisation.
async fn listed_job(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = emulator
        .call(
            "GET",
            &format!(
                "/channelGroup/{GROUP}/harvestJob?originEndpointName={ENDPOINT}&maxResults=100"
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListHarvestJobs expected 200, got {status} {body}"
    );
    let items = body["Items"].as_array().cloned().unwrap_or_default();
    items
        .iter()
        .find(|item| item["HarvestJobName"] == json!(JOB))
        .unwrap_or_else(|| panic!("case {case:?}: the list must report the job: {body}"))
        .clone()
}

/// A member of a job projection, as a string (`""` when absent).
fn text(job: &Value, name: &str) -> String {
    job[name].as_str().unwrap_or_default().to_string()
}

/// A timestamp member of a job projection, as epoch seconds.
fn instant(job: &Value, name: &str, case: &str) -> f64 {
    job[name]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} timestamp, got {job}"))
}

/// Wait until the wall clock reaches `target` (epoch seconds), so a row reads at an instant
/// its window is defined against rather than one its own setup time has shifted.
async fn sleep_until(target: f64) {
    let remaining = target - epoch_seconds_from_now(0.0);
    if remaining > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(remaining)).await;
    }
}

/// One row of the scheduled-instant table: a window, the instant the job is first read at,
/// and which end of the window the status it then reports became due at. Every offset is
/// in seconds from the one base instant the row computes.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    start_offset: f64,
    end_offset: f64,
    read_offset: f64,
    settled: &'static str,
    /// Whether the due instant is the window's END (otherwise its START).
    due_at_window_end: bool,
}

/// Windows that open AFTER the job is created, so the `QUEUED` -> `IN_PROGRESS` transition
/// falls due exactly at the scheduled start. The rows differ in where that start sits and
/// in how long the reader waited before looking. Every start is past the emulator's own
/// transition span, so the scheduled instant is the later of the two and therefore the one
/// the transition falls due at.
const OPENING_ROWS: &[Row] = &[
    Row {
        label: "a window that opens four seconds in, read a second after it opened",
        start_offset: 4.0,
        end_offset: 600.0,
        read_offset: 5.0,
        settled: IN_PROGRESS,
        due_at_window_end: false,
    },
    Row {
        label: "the same window, read three seconds after it opened instead",
        start_offset: 4.0,
        end_offset: 600.0,
        read_offset: 7.0,
        settled: IN_PROGRESS,
        due_at_window_end: false,
    },
    Row {
        label: "a window that opens four and a half seconds in, read two seconds later",
        start_offset: 4.5,
        end_offset: 90.0,
        read_offset: 6.5,
        settled: IN_PROGRESS,
        due_at_window_end: false,
    },
];

/// Windows that CLOSE after the job is created, so the `IN_PROGRESS` -> `FAILED` transition
/// falls due exactly at the scheduled end. The rows differ in the window's length and in
/// when the reader looked. Every window is longer than the emulator's own transition span,
/// so the scheduled end is the later of the two.
const CLOSING_ROWS: &[Row] = &[
    Row {
        label: "a four-second window, read two seconds after it closed",
        start_offset: 4.0,
        end_offset: 8.0,
        read_offset: 10.0,
        settled: FAILED,
        due_at_window_end: true,
    },
    Row {
        label: "a five-and-a-half-second window, read just after it closed",
        start_offset: 4.0,
        end_offset: 9.5,
        read_offset: 10.1,
        settled: FAILED,
        due_at_window_end: true,
    },
];

/// RULES 1 and 2 — the stamp is the scheduled instant the transition fell due at, not the
/// instant of the read that materialised it.
async fn a_due_transition_is_stamped_with_the_scheduled_instant(rows: &[Row]) {
    for row in rows {
        let label = row.label;
        let emulator = emulator_with_endpoint(label).await;
        // Rounded UP, so every offset below is at least that far ahead of the create: a
        // window whose start the create had already passed would fall due at the
        // emulator's own span instead, which is what RULE 3 covers.
        let base = epoch_seconds_from_now(0.0).ceil();
        let start = base + row.start_offset;
        let end = base + row.end_offset;
        let created = create_job(&emulator, label, start, end).await;
        let created_at = instant(&created, "CreatedAt", label);
        let due = if row.due_at_window_end { end } else { start };
        assert!(
            due - created_at >= LEAST_READ_LAG,
            "case {label:?}: the row's transition must fall due after the create at \
             {created_at}, due at {due}"
        );

        sleep_until(base + row.read_offset).await;
        // The wall clock the read happens at: what a stamp taken from the OBSERVATION
        // would carry instead.
        let observed_at = epoch_seconds_from_now(0.0);
        assert!(
            observed_at - due >= LEAST_READ_LAG,
            "case {label:?}: the row must read at least {LEAST_READ_LAG}s after the \
             transition fell due at {due}, read at {observed_at}"
        );
        let settled = read_job(&emulator, label).await;
        assert_eq!(
            text(&settled, "Status"),
            row.settled,
            "case {label:?}: expected {} by the time of the read, got {settled}",
            row.settled
        );

        let stamped = instant(&settled, "ModifiedAt", label);
        assert!(
            (stamped - due).abs() < SAME_INSTANT,
            "case {label:?}: expected ModifiedAt {due} (the instant the transition fell \
             due), got {stamped} in {settled}"
        );
        assert!(
            stamped >= created_at,
            "case {label:?}: expected ModifiedAt at or after CreatedAt {created_at}, got \
             {stamped}"
        );

        // The other reporting path materialises through the same helper, so it must report
        // the same stamp rather than restamping the job with its own instant.
        let listed = listed_job(&emulator, label).await;
        let listed_stamp = instant(&listed, "ModifiedAt", label);
        assert!(
            (listed_stamp - stamped).abs() < SAME_INSTANT,
            "case {label:?}: ListHarvestJobs must report ModifiedAt {stamped}, got \
             {listed_stamp} in {listed}"
        );
        assert_eq!(
            text(&listed, "Status"),
            row.settled,
            "case {label:?}: ListHarvestJobs must agree about the status, got {listed}"
        );

        // Reading again materialises nothing, so neither the stamp nor the tag moves.
        let again = read_job(&emulator, label).await;
        for name in ["Status", "ETag", "ModifiedAt"] {
            assert_eq!(
                again[name], settled[name],
                "case {label:?}: reading again must not move {name}, got {} where {} was \
                 reported",
                again[name], settled[name]
            );
        }
    }
}

#[tokio::test]
async fn opening_a_scheduled_window_stamps_the_job_with_that_start_instant() {
    a_due_transition_is_stamped_with_the_scheduled_instant(OPENING_ROWS).await;
}

#[tokio::test]
async fn closing_a_scheduled_window_stamps_the_job_with_that_end_instant() {
    a_due_transition_is_stamped_with_the_scheduled_instant(CLOSING_ROWS).await;
}

/// One row of the already-settled table: a window that had opened (or closed) before the
/// job was created, so the due instant is the emulator's own span after the create.
#[derive(Debug, Clone, Copy)]
struct SettledRow {
    label: &'static str,
    start_offset: f64,
    end_offset: f64,
    settled: &'static str,
}

/// Windows the create cannot be ahead of: the clock owes the transitions straight away, so
/// the only thing that may decide the stamp is the job's own create instant.
const SETTLED_ROWS: &[SettledRow] = &[
    SettledRow {
        label: "a window that opened an hour before the job and is still open",
        start_offset: -3600.0,
        end_offset: 3600.0,
        settled: IN_PROGRESS,
    },
    SettledRow {
        label: "a window that had closed an hour before the job",
        start_offset: -7200.0,
        end_offset: -3600.0,
        settled: FAILED,
    },
];

/// The delays two otherwise identical jobs are first read after. Both are past every span
/// the emulator owes such a job — two transition spans for a window that had already
/// closed — so each read observes the same settled status.
const UNEQUAL_READ_DELAYS: [Duration; 2] =
    [Duration::from_millis(6_500), Duration::from_millis(8_500)];

/// RULE 3 — with no scheduled instant to stamp, the gap between `CreatedAt` and
/// `ModifiedAt` is still the job's own: identical for a reader that looked early and one
/// that looked late.
#[tokio::test]
async fn the_stamp_of_a_job_whose_window_was_already_open_does_not_depend_on_the_reader() {
    for row in SETTLED_ROWS {
        let mut gaps: Vec<(Duration, f64)> = Vec::new();
        for delay in UNEQUAL_READ_DELAYS {
            let label = row.label;
            let emulator = emulator_with_endpoint(label).await;
            let base = epoch_seconds_from_now(0.0).ceil();
            let created = create_job(
                &emulator,
                label,
                base + row.start_offset,
                base + row.end_offset,
            )
            .await;
            let created_at = instant(&created, "CreatedAt", label);

            tokio::time::sleep(delay).await;
            let observed_at = epoch_seconds_from_now(0.0);
            let settled = read_job(&emulator, label).await;
            assert_eq!(
                text(&settled, "Status"),
                row.settled,
                "case {label:?} read after {delay:?}: expected {}, got {settled}",
                row.settled
            );
            let stamped = instant(&settled, "ModifiedAt", label);
            assert!(
                stamped > created_at,
                "case {label:?} read after {delay:?}: a materialised transition must move \
                 ModifiedAt past CreatedAt {created_at}, got {stamped}"
            );
            assert!(
                observed_at - stamped >= LEAST_READ_LAG,
                "case {label:?} read after {delay:?}: expected ModifiedAt at least \
                 {LEAST_READ_LAG}s before the read at {observed_at}, got {stamped}"
            );
            gaps.push((delay, stamped - created_at));
        }
        let (early_delay, early_gap) = gaps[0];
        let (late_delay, late_gap) = gaps[1];
        assert!(
            (early_gap - late_gap).abs() < SAME_INSTANT,
            "case {:?}: the gap from CreatedAt to ModifiedAt must not depend on the read \
             delay — {early_gap} after {early_delay:?} against {late_gap} after \
             {late_delay:?}",
            row.label
        );
    }
}
