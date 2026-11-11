// SPDX-License-Identifier: CC0-1.0

//! The window a `CreateHarvestJob` scheduled is what drives the job's status, BOTH ends of
//! it, driven over HTTP through the same router the emulator boots.
//!
//! `ScheduleConfiguration` is when the job runs — `CreateHarvestJobRequest$
//! ScheduleConfiguration` ("The configuration for when the harvest job should run,
//! including start and end times") and `HarvestJob$ScheduleConfiguration` ("The
//! configuration for when the harvest job is scheduled to run"), from
//! `smithy_query(query="shape", shape="CreateHarvestJobRequest")` — so the rules asserted
//! here are:
//!
//! 1. A job is accepted `QUEUED` and stays `QUEUED` until the START of the window it was
//!    scheduled for; a job whose window is already open leaves `QUEUED` on its own, after
//!    the emulator's own non-zero span.
//! 2. A job leaves `IN_PROGRESS` at the END of that window, never before, and never
//!    without having been observable `IN_PROGRESS` — so a window that had already closed
//!    when the job was created still walks `QUEUED` -> `IN_PROGRESS` -> `FAILED` rather
//!    than skipping a status.
//! 3. The reporting operations (`GetHarvestJob`, `ListHarvestJobs`) only materialise what
//!    the clock already owed: while the status is unchanged, repeated reads report the same
//!    `ETag` and `ModifiedAt`, and the two readers never disagree about the status.
//!
//! Each rule is asserted over a TABLE of scheduled windows — ahead of now, open now,
//! opened in the past, closed in the past, and of several lengths — rather than over one
//! recorded request.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client (`channel_lifecycle`) and the
// one `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::Emulator;
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::HARVESTER_UNAVAILABLE_MESSAGE;

/// The hierarchy every row hangs its job off, all matching the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hjw-group";
const CHANNEL: &str = "hjw-channel";
const ENDPOINT: &str = "hjw-endpoint";
const MANIFEST: &str = "index";
const JOB: &str = "hjw-job";

/// The statuses of `HarvestJobStatus` this suite walks, in the order a job that is left
/// alone passes through them.
const QUEUED: &str = "QUEUED";
const IN_PROGRESS: &str = "IN_PROGRESS";
const FAILED: &str = "FAILED";

/// Comfortably longer than the span the emulator holds a fresh harvest job in `QUEUED`
/// before the clock owes it the next status, so a wait of this length materialises every
/// transition that was due — and a status still reported after it is one the job's own
/// schedule is holding it in.
const PAST_ANY_SPAN: Duration = Duration::from_millis(3_500);

/// How long a row waits for a transition its schedule puts in the future, and how often it
/// looks: short polls, so a status that lasts only one span is still observed. The limit is
/// the number of polls, which spans comfortably more than the two transition spans a job
/// created for a closed window crosses.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const POLL_LIMIT: u32 = 3_000;

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

/// The status `ListHarvestJobs` reports for the suite's job — the OTHER reporting path
/// through the same materialisation.
async fn listed_status(emulator: &Emulator, case: &str) -> String {
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
    let item = items
        .iter()
        .find(|item| item["HarvestJobName"] == json!(JOB))
        .unwrap_or_else(|| panic!("case {case:?}: the list must report the job: {body}"));
    item["Status"].as_str().unwrap_or_default().to_string()
}

/// A member of a job projection, as a string (`""` when absent).
fn member(job: &Value, name: &str) -> String {
    job[name].as_str().unwrap_or_default().to_string()
}

/// Create the suite's job over the window `[start, end)` (epoch seconds) and return the
/// create's own response.
async fn create_job(emulator: &Emulator, case: &str, start: f64, end: f64) -> Value {
    let mut body = harvest_body(start, end, &[MANIFEST]);
    body["HarvestJobName"] = json!(JOB);
    let (status, created) = emulator.call("POST", &collection(), Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateHarvestJob expected 200, got {status} {created}"
    );
    assert_eq!(
        member(&created, "Status"),
        QUEUED,
        "case {case:?}: a job is accepted QUEUED, got {created}"
    );
    // The window travels as epoch seconds, so each end is compared as a number rather
    // than by its rendering.
    for (name, expected) in [("StartTime", start), ("EndTime", end)] {
        let actual = created["ScheduleConfiguration"][name]
            .as_f64()
            .unwrap_or(f64::NAN);
        assert!(
            (actual - expected).abs() < 0.001,
            "case {case:?}: the create must report {name} {expected}, got {actual} in {created}"
        );
    }
    created
}

/// One row of the table: a scheduled window, and the status the job must report once
/// [`PAST_ANY_SPAN`] has passed since it was created.
#[derive(Debug, Clone, Copy)]
struct Window {
    label: &'static str,
    /// The window's start and end, as offsets in seconds from the instant the row runs.
    start_offset: f64,
    end_offset: f64,
    settled: &'static str,
    /// Whether the window opens late enough to be seen opening WHILE the row runs: the row
    /// then polls on and asserts the job reaches `IN_PROGRESS` of its own accord.
    opens_during_the_row: bool,
}

/// Windows whose END is far enough out that the row only ever judges the START side. The
/// rows differ in which side of now the window opens and how long it is.
const OPENING_WINDOWS: &[Window] = &[
    Window {
        label: "a window that opens an hour from now",
        start_offset: 3600.0,
        end_offset: 7200.0,
        settled: QUEUED,
        opens_during_the_row: false,
    },
    Window {
        label: "a window that opens a minute from now",
        start_offset: 60.0,
        end_offset: 3660.0,
        settled: QUEUED,
        opens_during_the_row: false,
    },
    Window {
        label: "a one-second window that opens a minute from now",
        start_offset: 60.0,
        end_offset: 61.0,
        settled: QUEUED,
        opens_during_the_row: false,
    },
    Window {
        label: "a window that opens six seconds from now",
        start_offset: 6.0,
        end_offset: 606.0,
        settled: QUEUED,
        opens_during_the_row: true,
    },
    Window {
        label: "a window that is open as the job is created",
        start_offset: 0.0,
        end_offset: 3600.0,
        settled: IN_PROGRESS,
        opens_during_the_row: false,
    },
    Window {
        label: "a window that opened an hour ago and is still open",
        start_offset: -3600.0,
        end_offset: 3600.0,
        settled: IN_PROGRESS,
        opens_during_the_row: false,
    },
];

/// RULE 1 — the START of the scheduled window is what takes a job out of `QUEUED`.
#[tokio::test]
async fn a_harvest_job_stays_queued_until_the_window_it_was_scheduled_for_opens() {
    for row in OPENING_WINDOWS {
        let label = row.label;
        let emulator = emulator_with_endpoint(label).await;
        let now = epoch_seconds_from_now(0.0).floor();
        let created = create_job(
            &emulator,
            label,
            now + row.start_offset,
            now + row.end_offset,
        )
        .await;

        // The span is never zero: a caller reading straight after the create still sees
        // the status the create reported, whatever its window.
        let first = read_job(&emulator, label).await;
        assert_eq!(
            member(&first, "Status"),
            QUEUED,
            "case {label:?}: a read straight after the create must still report QUEUED, got \
             {first}"
        );
        assert_eq!(
            first["ScheduleConfiguration"], created["ScheduleConfiguration"],
            "case {label:?}: the read must report the window the create stored, got {first}"
        );

        tokio::time::sleep(PAST_ANY_SPAN).await;
        let settled = read_job(&emulator, label).await;
        assert_eq!(
            member(&settled, "Status"),
            row.settled,
            "case {label:?}: expected {} once a second has passed, got {settled}",
            row.settled
        );
        assert_eq!(
            listed_status(&emulator, label).await,
            row.settled,
            "case {label:?}: ListHarvestJobs must agree with GetHarvestJob about the status"
        );

        // RULE 3 — a read materialises only what the clock owed: with the next transition
        // still in the future, reading again reports the same version of the job.
        let again = read_job(&emulator, label).await;
        for name in ["Status", "ETag", "ModifiedAt", "ErrorMessage"] {
            assert_eq!(
                again[name], settled[name],
                "case {label:?}: reading again must not move {name}, got {} where {} was \
                 reported",
                again[name], settled[name]
            );
        }

        if row.opens_during_the_row {
            // Nobody touches the job: the clock alone takes it into the window it was
            // scheduled for, out of the QUEUED every read above found it in.
            let seen = poll_statuses(&emulator, label, IN_PROGRESS).await;
            assert_eq!(
                seen,
                vec![QUEUED.to_string(), IN_PROGRESS.to_string()],
                "case {label:?}: the job must sit QUEUED until its window opens and then \
                 report IN_PROGRESS, saw {seen:?}"
            );
        }
    }
}

/// Read the job until it reports `target`, and return every status seen on the way (the
/// first element is the status at the first read).
async fn poll_statuses(emulator: &Emulator, case: &str, target: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut tag = String::new();
    for _ in 0..POLL_LIMIT {
        let job = read_job(emulator, case).await;
        let status = member(&job, "Status");
        // RULE 3, polled: while the status stands, the reads that observe it report one
        // unchanged version of the job — a read commits nothing of its own.
        if seen.last() == Some(&status) {
            assert_eq!(
                member(&job, "ETag"),
                tag,
                "case {case:?}: a read that moved no status must not rotate the ETag, got {job}"
            );
        }
        tag = member(&job, "ETag");
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if status == target {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the job never reached {target}, saw {seen:?}");
}

/// Windows that have CLOSED, or close, while the row runs: the rows differ in how long ago
/// the window closed, how long it was, and whether it closes before or after the job is
/// created.
const CLOSING_WINDOWS: &[Window] = &[
    Window {
        label: "a window that closed an hour before the job was created",
        start_offset: -7200.0,
        end_offset: -3600.0,
        settled: FAILED,
        opens_during_the_row: false,
    },
    Window {
        label: "a one-second window that closed a day before the job was created",
        start_offset: -86401.0,
        end_offset: -86400.0,
        settled: FAILED,
        opens_during_the_row: false,
    },
    Window {
        label: "a window that opens as the job is created and closes a second later",
        start_offset: 0.0,
        end_offset: 1.0,
        settled: FAILED,
        opens_during_the_row: false,
    },
    Window {
        label: "a window that opens a second from now and closes a second after that",
        start_offset: 1.0,
        end_offset: 2.0,
        settled: FAILED,
        opens_during_the_row: false,
    },
];

/// RULE 2 — the END of the scheduled window is what takes a job out of `IN_PROGRESS`, and
/// no status is skipped on the way.
#[tokio::test]
async fn a_harvest_job_walks_every_status_of_the_window_it_was_scheduled_for() {
    for row in CLOSING_WINDOWS {
        let label = row.label;
        let emulator = emulator_with_endpoint(label).await;
        let now = epoch_seconds_from_now(0.0).floor();
        create_job(
            &emulator,
            label,
            now + row.start_offset,
            now + row.end_offset,
        )
        .await;

        let seen = poll_statuses(&emulator, label, row.settled).await;
        assert_eq!(
            seen,
            vec![
                QUEUED.to_string(),
                IN_PROGRESS.to_string(),
                FAILED.to_string()
            ],
            "case {label:?}: the job must be observed in each status in turn, saw {seen:?}"
        );

        // The failure says what did not happen, and is reported by both readers.
        let failed = read_job(&emulator, label).await;
        assert_eq!(
            member(&failed, "ErrorMessage"),
            HARVESTER_UNAVAILABLE_MESSAGE,
            "case {label:?}: a job that ended must report why, got {failed}"
        );
        assert_eq!(
            listed_status(&emulator, label).await,
            FAILED,
            "case {label:?}: ListHarvestJobs must agree that the job ended"
        );

        // A job the clock has ended is terminal, so cancelling it is the modelled refusal
        // — the status the CANCEL path materialises is the same one the readers see.
        let (status, refused) = emulator
            .call("PUT", &format!("{}/{JOB}", collection()), None)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: cancelling an ended job expected 400, got {status} {refused}"
        );
        assert_eq!(
            member(&refused, "ValidationExceptionType"),
            "HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION",
            "case {label:?}: expected HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION, got {refused}"
        );
    }
}

/// A job whose window has not opened is still a LIVE job: it can be cancelled, which is
/// what makes the `QUEUED` the rows above assert a real status rather than a stalled one.
#[tokio::test]
async fn a_job_whose_window_has_not_opened_can_still_be_cancelled() {
    let label = "a window that opens an hour from now, cancelled before it does";
    let emulator = emulator_with_endpoint(label).await;
    let now = epoch_seconds_from_now(0.0).floor();
    create_job(&emulator, label, now + 3600.0, now + 7200.0).await;

    tokio::time::sleep(PAST_ANY_SPAN).await;
    let (status, cancelled) = emulator
        .call("PUT", &format!("{}/{JOB}", collection()), None)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CancelHarvestJob expected 200, got {status} {cancelled}"
    );
    let read = read_job(&emulator, label).await;
    assert_eq!(
        member(&read, "Status"),
        "CANCELLED",
        "case {label:?}: the cancelled job must report CANCELLED, got {read}"
    );

    // Cancelled is terminal: the window opening later cannot move it on.
    tokio::time::sleep(PAST_ANY_SPAN).await;
    let read_again = read_job(&emulator, label).await;
    assert_eq!(
        member(&read_again, "Status"),
        "CANCELLED",
        "case {label:?}: a terminal status is not moved by the clock, got {read_again}"
    );
}
