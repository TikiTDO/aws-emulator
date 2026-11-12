// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the retention window of a speech synthesis task, driven over
//! HTTP through the same router the emulator serves with.
//!
//! RULE under test: a synthesis task is reported only while it is inside the retention
//! window the model documents on `StartSpeechSynthesisTask` ("The SpeechSynthesisTask
//! object is available for 72 hours after starting the asynchronous synthesis task") —
//! measured from the task's own `CreationTime`. Past it the service holds no such task,
//! so `GetSpeechSynthesisTask` answers `SynthesisTaskNotFoundException` (HTTP 400) and
//! `ListSpeechSynthesisTasks` neither reports it nor lets it take up a page slot.
//!
//! Each test checks the rule over a TABLE of ages that spans the class: inside the
//! window, on the published bound, and past it — never one age only. The harness and
//! the response readers come from `crate::lexicons_and_speech`, the task fixtures from
//! `crate::support`, and the list readers from `crate::synthesis_tasks`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_polly_parrot_v1::state::TASK_RETENTION;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, get_task, message_slot, seed_task, stored_task};
use crate::synthesis_tasks::{list, list_ids};

/// The ages a task is read back at, and whether the service still holds it.
///
/// The bound itself comes from the documented window rather than from a literal, so
/// the rows stay the published bound and its neighbours whatever the constant says: a
/// task is available FOR the window, so the instant the window has fully elapsed the
/// task is gone.
fn retention_cases() -> Vec<(&'static str, Duration, bool)> {
    vec![
        ("just started", Duration::ZERO, true),
        ("one second old", Duration::from_secs(1), true),
        ("an hour old", Duration::from_secs(3_600), true),
        (
            "a minute inside the window",
            TASK_RETENTION - Duration::from_secs(60),
            true,
        ),
        // The millisecond on either side of the bound is pinned deterministically by
        // the state's own unit test; over HTTP the row inside the window is kept a
        // whole minute clear of it, because the handler reads the clock after the test
        // does.
        ("exactly the whole window old", TASK_RETENTION, false),
        (
            "a millisecond past the window",
            TASK_RETENTION + Duration::from_millis(1),
            false,
        ),
        (
            "a day past the window",
            TASK_RETENTION + Duration::from_secs(86_400),
            false,
        ),
        ("a week past the window", Duration::from_secs(7 * 86_400), false),
        ("a year past the window", Duration::from_secs(365 * 86_400), false),
    ]
}

/// RULE: `GetSpeechSynthesisTask` reports a task while it is inside the retention
/// window and refuses it with `SynthesisTaskNotFoundException` (HTTP 400) once the
/// window has elapsed, exactly as it refuses a `TaskId` that names no task at all.
#[tokio::test]
async fn a_task_is_readable_only_inside_its_retention_window() {
    let polly = Polly::new();
    let now = SystemTime::now();

    for (case, age, retained) in retention_cases() {
        let task_id = fixture_task_id(&format!("retention-{case}"));
        seed_task(&polly, &stored_task(&task_id, now - age));

        let response = get_task(&polly, &task_id).await;
        if retained {
            assert_eq!(
                response.status(),
                200,
                "case {case:?}: a task inside the window must be reported, body {:?}",
                String::from_utf8_lossy(response.body())
            );
            assert_eq!(
                json(&response)["SynthesisTask"]["TaskId"].as_str(),
                Some(task_id.as_str()),
                "case {case:?}: the reported task must be the one read"
            );
        } else {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "SynthesisTaskNotFoundException".to_string()),
                "case {case:?}: an expired task must be refused, body {:?}",
                json(&response)
            );
            // The refusal is the service's one for a `TaskId` naming no task, message
            // slot and all: an expired task is indistinguishable from one never started.
            assert_eq!(
                message_slot(&response),
                Some(serde_json::Value::Null),
                "case {case:?}: wrong message slot, body {:?}",
                json(&response)
            );
        }
    }

    // A task that expires is gone for good: a second read answers the same way, and a
    // task still inside the window is untouched by the reads around it.
    for (case, _age, retained) in retention_cases() {
        let task_id = fixture_task_id(&format!("retention-{case}"));
        let expected = if retained { 200 } else { 400 };
        let response = get_task(&polly, &task_id).await;
        assert_eq!(
            response.status(),
            expected,
            "case {case:?}: a repeated read must answer the same, body {:?}",
            json(&response)
        );
    }
}

/// RULE: a task past the retention window is not in the report — whatever its status,
/// and whether or not the report filters on one.
#[tokio::test]
async fn an_expired_task_drops_out_of_the_report() {
    let polly = Polly::new();
    let now = SystemTime::now();

    // Every value of the model's `TaskStatus` enum
    // (`smithy_query(query="shape", shape="TaskStatus")`): one fresh task and one
    // expired task in each, so no status is retained or dropped on its own.
    let statuses = ["completed", "failed", "inProgress", "scheduled"];
    for status in statuses {
        for (label, age) in [
            ("fresh", Duration::from_secs(60)),
            ("expired", TASK_RETENTION + Duration::from_secs(60)),
        ] {
            let mut task = stored_task(&format!("{status}-{label}"), now - age);
            task.task_status = status.to_string();
            seed_task(&polly, &task);
        }
    }

    let mut expected: Vec<String> = statuses
        .iter()
        .map(|status| format!("{status}-fresh"))
        .collect();
    expected.sort();
    let mut reported = list_ids(&polly, "").await;
    reported.sort();
    assert_eq!(
        reported, expected,
        "an unfiltered report must carry exactly the tasks inside the window, got {reported:?}"
    );

    for status in statuses {
        assert_eq!(
            list_ids(&polly, &format!("?Status={status}")).await,
            vec![format!("{status}-fresh")],
            "Status={status}: the expired task of that status must not be reported"
        );
    }
}

/// RULE: an expired task occupies no page slot and shifts no cursor — paging a store
/// that holds expired tasks visits exactly the retained ones, once each, in the report's
/// own order (newest first).
#[tokio::test]
async fn expired_tasks_take_up_no_page_slot() {
    let polly = Polly::new();
    let now = SystemTime::now();

    // Retained and expired tasks interleaved in creation order, so an expired task
    // sits at the start, in the middle and at the end of the order a page walks.
    let retained = 5u64;
    for index in 0..retained {
        seed_task(
            &polly,
            &stored_task(
                &format!("Keep{index}"),
                now - Duration::from_secs(retained - index),
            ),
        );
        seed_task(
            &polly,
            &stored_task(
                &format!("Gone{index}"),
                now - (TASK_RETENTION + Duration::from_secs(retained - index)),
            ),
        );
    }
    let expected: Vec<String> = (0..retained)
        .rev()
        .map(|index| format!("Keep{index}"))
        .collect();

    // Both published bounds of `MaxResults` (1 and 100), sizes in between, and a
    // request that bounds nothing.
    for max_results in [Some(1usize), Some(2), Some(5), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (max_results, &token) {
                (Some(max), None) => format!("?MaxResults={max}"),
                (Some(max), Some(next)) => format!("?MaxResults={max}&NextToken={next}"),
                (None, None) => String::new(),
                (None, Some(next)) => format!("?NextToken={next}"),
            };
            let response = list(&polly, &query).await;
            assert_eq!(
                response.status(),
                200,
                "MaxResults={max_results:?} page {pages}: body {:?}",
                String::from_utf8_lossy(response.body())
            );
            let body = json(&response);
            let ids = crate::synthesis_tasks::task_ids(&body);
            assert!(
                !ids.is_empty(),
                "MaxResults={max_results:?} page {pages}: an expired task must not fill a page"
            );
            if let Some(max) = max_results {
                assert!(
                    ids.len() <= max,
                    "MaxResults={max}: page {pages} carries {} tasks",
                    ids.len()
                );
            }
            seen.extend(ids);
            pages += 1;
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                pages < 20,
                "MaxResults={max_results:?}: pagination did not terminate"
            );
        }
        assert_eq!(
            seen, expected,
            "MaxResults={max_results:?}: the pages together must be the retained tasks once, newest first"
        );
    }
}

/// RULE: reading a task changes nothing a caller could not have observed by waiting —
/// a read inside the window neither expires the task nor extends its window, so the
/// task is still gone once the window has elapsed, however often it was polled.
#[tokio::test]
async fn reading_a_task_neither_expires_it_nor_extends_its_window() {
    let polly = Polly::new();
    let now = SystemTime::now();

    let fresh_id = fixture_task_id("polled-fresh");
    seed_task(&polly, &stored_task(&fresh_id, now));
    let edge_id = fixture_task_id("polled-edge");
    seed_task(
        &polly,
        &stored_task(&edge_id, now - (TASK_RETENTION - Duration::from_secs(5))),
    );

    for round in 0..5 {
        for task_id in [&fresh_id, &edge_id] {
            let response = get_task(&polly, task_id).await;
            assert_eq!(
                response.status(),
                200,
                "round {round} task {task_id}: polling must not expire a task inside the window, body {:?}",
                json(&response)
            );
        }
        let reported = list_ids(&polly, "").await;
        assert_eq!(
            reported.len(),
            2,
            "round {round}: both tasks inside the window must stay reported, got {reported:?}"
        );
    }

    // The same task, seeded as it will be once its window has run out: the polling
    // above bought it no extra time.
    seed_task(
        &polly,
        &stored_task(&edge_id, now - (TASK_RETENTION + Duration::from_secs(5))),
    );
    let response = get_task(&polly, &edge_id).await;
    assert_eq!(
        (response.status().as_u16(), error_code(&response)),
        (400, "SynthesisTaskNotFoundException".to_string()),
        "a polled task must still expire on its own creation time, body {:?}",
        json(&response)
    );
    assert_eq!(
        list_ids(&polly, "").await,
        vec![fresh_id.clone()],
        "only the task inside the window may remain"
    );
}
