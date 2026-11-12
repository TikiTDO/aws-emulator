// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSpeechSynthesisTasks`, driven over HTTP through the same
//! router the emulator serves with.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs that
//! spans the class the rule covers — every `TaskStatus` rather than one, several
//! `MaxResults` including both published bounds, several creation orders including a
//! tie, a transition the clock owes on both sides of its due instant.
//!
//! The harness and the response readers come from `crate::lexicons_and_speech`, the
//! task fixtures from `crate::support`: a row that fixes a creation time, a status or a
//! pending transition of its own seeds the shared store directly, the way
//! `StartSpeechSynthesisTask` writes it (and `crate::speech_synthesis_task_creation`
//! drives that call end to end).

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::state::{PendingTaskStatus, TASK_COMPLETION_DELAY};

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, seed_task, stored_task};

/// One `ListSpeechSynthesisTasks` call with the given query string.
pub(crate) async fn list(polly: &Polly, query: &str) -> http::Response<Bytes> {
    polly
        .call("GET", &format!("/v1/synthesisTasks{query}"), "")
        .await
}

/// The task ids one page reports, in page order.
pub(crate) fn task_ids(body: &serde_json::Value) -> Vec<String> {
    body["SynthesisTasks"]
        .as_array()
        .expect("SynthesisTasks is a list")
        .iter()
        .map(|task| task["TaskId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The task ids one successful page reports, in page order.
pub(crate) async fn list_ids(polly: &Polly, query: &str) -> Vec<String> {
    let response = list(polly, query).await;
    assert_eq!(
        response.status(),
        200,
        "query {query:?}: body {:?}",
        String::from_utf8_lossy(response.body())
    );
    task_ids(&json(&response))
}

/// RULE: `ListSpeechSynthesisTasks` reports the stored tasks ordered by their creation
/// date, MOST RECENTLY CREATED FIRST, each one carrying the members the store holds for it.
#[tokio::test]
async fn tasks_are_reported_in_creation_order_with_their_stored_members() {
    let polly = Polly::new();
    assert_eq!(
        list_ids(&polly, "").await,
        Vec::<String>::new(),
        "an untouched store has no tasks to report"
    );

    // Seeded in an order unrelated to their creation dates, and including a pair
    // created at the same instant so the tie-break is exercised too.
    let now = SystemTime::now();
    for (task_id, created_ago_secs) in [
        ("third", 10u64),
        ("first", 900),
        ("tie-b", 100),
        ("second", 500),
        ("tie-a", 100),
    ] {
        seed_task(
            &polly,
            &stored_task(task_id, now - Duration::from_secs(created_ago_secs)),
        );
    }

    assert_eq!(
        list_ids(&polly, "").await,
        vec!["third", "tie-b", "tie-a", "second", "first"],
        "tasks must be ordered by creation date, newest first, with the task id breaking a tie"
    );

    // Every member of the reported task is the stored one.
    let mut full = stored_task("fullTask", now - Duration::from_secs(30));
    full.task_status = "failed".to_string();
    full.task_status_reason = Some("the output bucket is not writable".to_string());
    full.engine = "neural".to_string();
    full.output_format = "ogg_vorbis".to_string();
    full.sample_rate = Some("16000".to_string());
    full.request_characters = 4_242;
    full.text_type = "ssml".to_string();
    full.voice_id = "Lupe".to_string();
    full.language_code = Some("es-US".to_string());
    full.lexicon_names = vec!["Alpha".to_string(), "Beta".to_string()];
    full.speech_mark_types = vec!["word".to_string()];
    full.sns_topic_arn = Some("arn:aws:sns:us-east-1:000000000000:polly-status".to_string());
    let created_epoch = full
        .creation_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a creation time after the epoch")
        .as_secs_f64();
    seed_task(&polly, &full);

    let body = json(&list(&polly, "?Status=failed").await);
    let reported = body["SynthesisTasks"]
        .as_array()
        .expect("SynthesisTasks is a list")
        .iter()
        .find(|task| task["TaskId"].as_str() == Some("fullTask"))
        .cloned()
        .unwrap_or_else(|| panic!("the seeded task was not reported: {body:?}"));
    for (member, expected) in [
        ("TaskId", serde_json::json!("fullTask")),
        ("TaskStatus", serde_json::json!("failed")),
        (
            "TaskStatusReason",
            serde_json::json!(full.task_status_reason.clone()),
        ),
        ("Engine", serde_json::json!("neural")),
        ("OutputFormat", serde_json::json!("ogg_vorbis")),
        ("OutputUri", serde_json::json!(full.output_uri.clone())),
        ("SampleRate", serde_json::json!("16000")),
        ("RequestCharacters", serde_json::json!(4_242)),
        ("TextType", serde_json::json!("ssml")),
        ("VoiceId", serde_json::json!("Lupe")),
        ("LanguageCode", serde_json::json!("es-US")),
        ("LexiconNames", serde_json::json!(["Alpha", "Beta"])),
        ("SpeechMarkTypes", serde_json::json!(["word"])),
        ("SnsTopicArn", serde_json::json!(full.sns_topic_arn.clone())),
    ] {
        assert_eq!(
            reported[member], expected,
            "member {member}: expected {expected:?}, got {:?}",
            reported[member]
        );
    }
    let reported_epoch = reported["CreationTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("CreationTime must be an epoch timestamp: {reported:?}"));
    assert!(
        (reported_epoch - created_epoch).abs() < 0.002,
        "member CreationTime: expected {created_epoch}, got {reported_epoch}"
    );
}

/// RULE: `Status` reports exactly the tasks in that status, for every status the model
/// declares; without it every task is reported.
#[tokio::test]
async fn the_status_filter_reports_exactly_the_tasks_in_that_status() {
    let polly = Polly::new();
    // Every value of the model's `TaskStatus` enum
    // (`smithy_query(query="shape", shape="TaskStatus")`), two tasks each.
    let statuses = ["completed", "failed", "inProgress", "scheduled"];
    let now = SystemTime::now();
    for status in statuses {
        for copy in 0..2u64 {
            let mut task = stored_task(
                &format!("{status}-{copy}"),
                now - Duration::from_secs(60 + copy),
            );
            task.task_status = status.to_string();
            seed_task(&polly, &task);
        }
    }

    for status in statuses {
        let reported: BTreeSet<String> = list_ids(&polly, &format!("?Status={status}"))
            .await
            .into_iter()
            .collect();
        let expected: BTreeSet<String> = (0..2u64).map(|copy| format!("{status}-{copy}")).collect();
        assert_eq!(
            reported, expected,
            "Status={status}: expected {expected:?}, got {reported:?}"
        );
    }

    assert_eq!(
        list_ids(&polly, "").await.len(),
        statuses.len() * 2,
        "an unfiltered report must carry every stored task"
    );

    // A status outside the enum never reaches the handler.
    let bogus = list(&polly, "?Status=inflight").await;
    assert_eq!(
        bogus.status(),
        400,
        "a Status outside the modeled enum must be refused, body {:?}",
        String::from_utf8_lossy(bogus.body())
    );
}

/// RULE: the report is paged — a page carries at most `MaxResults` tasks, a `NextToken`
/// appears only while a further task exists, and following the tokens visits every task
/// exactly once in creation order, newest first.
#[tokio::test]
async fn pagination_visits_every_task_exactly_once() {
    let polly = Polly::new();
    let total = 25u64;
    let now = SystemTime::now();
    for index in 0..total {
        // `Task00` is the oldest: the older the task, the longer ago it was created.
        seed_task(
            &polly,
            &stored_task(
                &format!("Task{index:02}"),
                now - Duration::from_secs(total - index),
            ),
        );
    }
    // Newest first, so the highest-numbered task is the first one reported.
    let expected: Vec<String> = (0..total)
        .rev()
        .map(|index| format!("Task{index:02}"))
        .collect();

    // Both published bounds of `MaxResults` (1 and 100), sizes in between, and a
    // request that names no bound at all (`None`), which is therefore not paged.
    for max_results in [Some(1usize), Some(2), Some(7), Some(24), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        let mut token: Option<String> = None;
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
            let ids = task_ids(&body);
            assert!(
                !ids.is_empty(),
                "MaxResults={max_results:?} page {pages}: a page reached through a NextToken must not be empty"
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
                pages < 40,
                "MaxResults={max_results:?}: pagination did not terminate"
            );
        }
        assert_eq!(
            seen, expected,
            "MaxResults={max_results:?}: the pages together must be every task once, newest first"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "MaxResults={max_results:?}: a task was reported twice in {seen:?}"
        );
    }

    // A `MaxResults` outside the modeled 1..100 range never reaches the handler.
    for bad in ["0", "101", "-1"] {
        let response = list(&polly, &format!("?MaxResults={bad}")).await;
        assert_eq!(
            response.status(),
            400,
            "MaxResults={bad}: expected a refusal, body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }
}

/// RULE: a `NextToken` resumes the listing it was issued for and no other — a token
/// this service did not mint, or one minted under a different `Status` filter, is
/// `InvalidNextTokenException` (HTTP 400) rather than a page of the wrong tasks.
#[tokio::test]
async fn a_task_page_token_only_resumes_its_own_listing() {
    let polly = Polly::new();
    let now = SystemTime::now();
    for index in 0..4u64 {
        let mut task = stored_task(&format!("Task{index}"), now - Duration::from_secs(10 + index));
        task.task_status = if index % 2 == 0 { "completed" } else { "failed" }.to_string();
        seed_task(&polly, &task);
    }

    let first = json(&list(&polly, "?MaxResults=1").await);
    let token = first["NextToken"]
        .as_str()
        .expect("four tasks do not fit in a one-task page")
        .to_string();
    let page_one = task_ids(&first);
    let page_two = list_ids(&polly, &format!("?MaxResults=1&NextToken={token}")).await;
    assert!(
        page_one != page_two,
        "the second page repeats the first: {page_one:?}"
    );

    for mismatched in [
        format!("?Status=completed&NextToken={token}"),
        format!("?Status=failed&NextToken={token}"),
        "?NextToken=not-a-token".to_string(),
        "?NextToken=zzzz".to_string(),
        "?NextToken=ff".to_string(),
        // Well-formed hex, but not a cursor this operation ever minted.
        format!("?NextToken={}", hex::encode("1\u{1f}x")),
        // The same, with all three fields a minted cursor carries: the shape alone is
        // not what makes a token one this service issued (see
        // `crate::task_page_token_minting` for that rule's own table).
        format!("?NextToken={}", hex::encode("0\u{1f}\u{1f}")),
    ] {
        let response = list(&polly, &mismatched).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidNextTokenException".to_string()),
            "query {mismatched:?}: a foreign token must be refused, body {:?}",
            json(&response)
        );
    }
}

/// RULE: a report materialises only the status transition the clock has ALREADY made
/// due, never one that is not yet due, and the two task reads agree on what it reports.
#[tokio::test]
async fn a_report_settles_only_the_transition_the_clock_already_owed() {
    let polly = Polly::new();
    let now = SystemTime::now();

    // Due in the past, so the report must show the status the clock owed.
    let due_id = fixture_task_id("due");
    let mut due = stored_task(&due_id, now - Duration::from_secs(30));
    due.task_status = "inProgress".to_string();
    due.pending_status = Some(PendingTaskStatus {
        due_at: now - TASK_COMPLETION_DELAY,
        status: "completed".to_string(),
        reason: None,
    });
    seed_task(&polly, &due);

    // Due far in the future, so the report must leave it where it is however often it
    // is polled.
    let pending_id = fixture_task_id("pending");
    let mut pending = stored_task(&pending_id, now - Duration::from_secs(20));
    pending.task_status = "scheduled".to_string();
    pending.pending_status = Some(PendingTaskStatus {
        due_at: now + Duration::from_secs(3_600),
        status: "completed".to_string(),
        reason: Some("not due yet".to_string()),
    });
    seed_task(&polly, &pending);

    // A task already terminal stays terminal, with its reason.
    let failed_id = fixture_task_id("failed");
    let mut failed = stored_task(&failed_id, now - Duration::from_secs(10));
    failed.task_status = "failed".to_string();
    failed.task_status_reason = Some("the output bucket is not writable".to_string());
    seed_task(&polly, &failed);

    let expected = [
        (due_id.as_str(), "completed", None),
        (pending_id.as_str(), "scheduled", None),
        (
            failed_id.as_str(),
            "failed",
            Some("the output bucket is not writable"),
        ),
    ];
    // Several reports, filtered and paged, and the answer must not drift between them.
    for round in 0..3 {
        for query in ["", "?MaxResults=1", "?Status=scheduled"] {
            assert_eq!(
                list(&polly, query).await.status(),
                200,
                "round {round} query {query:?}: a report must succeed"
            );
        }
        let body = json(&list(&polly, "").await);
        let reported = body["SynthesisTasks"]
            .as_array()
            .expect("SynthesisTasks is a list")
            .clone();
        for (task_id, status, reason) in expected {
            let task = reported
                .iter()
                .find(|task| task["TaskId"].as_str() == Some(task_id))
                .unwrap_or_else(|| panic!("round {round}: {task_id} was not reported: {body:?}"));
            assert_eq!(
                task["TaskStatus"].as_str(),
                Some(status),
                "round {round} task {task_id}: wrong TaskStatus, got {task:?}"
            );
            assert_eq!(
                task["TaskStatusReason"].as_str(),
                reason,
                "round {round} task {task_id}: wrong TaskStatusReason, got {task:?}"
            );
        }
    }

    // The per-task read agrees with the report, so neither reader invents a status.
    for (task_id, status, _) in expected {
        let response = polly
            .call("GET", &format!("/v1/synthesisTasks/{task_id}"), "")
            .await;
        assert_eq!(
            response.status(),
            200,
            "task {task_id}: GetSpeechSynthesisTask failed, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            json(&response)["SynthesisTask"]["TaskStatus"].as_str(),
            Some(status),
            "task {task_id}: the two reads must report the same status"
        );
    }
}
