// SPDX-License-Identifier: CC0-1.0

//! What `StartSpeechSynthesisTask` records, and what the task reads then report.
//!
//! RULE: a started synthesis task is HELD by the service, so the task reads report it
//! with the record the start call made from the request — its minted `TaskId`, the
//! engine, output format, text type, voice and billed character count the request
//! resolved to, the output location in the bucket the request named, the creation time,
//! and the optional members only where the request set them — until its retention
//! window runs out. The task is created in a non-terminal status and the CLOCK is what
//! moves it to its terminal one; a request the service refuses records no task at all.
//!
//! The rule is checked over a TABLE rather than the one request a fixture quoted: the
//! rows differ along what it generalises over — the output format (and so the output
//! object's suffix), the engine, the text type (and so what is billed), the voice and
//! its language, the key prefix, and the Region the call is signed for.

use std::time::Duration;

use bytes::Bytes;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::get_task;
use crate::synthesis_tasks::{list, task_ids};
use service_com_amazonaws_polly_parrot_v1::state::TASK_COMPLETION_DELAY;

/// The status a task is created in, out of the model's `TaskStatus` values
/// (`scheduled | inProgress | completed | failed`).
const CREATED_STATUS: &str = "scheduled";

/// The status the clock moves a started task to.
const TERMINAL_STATUS: &str = "completed";

/// The bucket the rows write their output to.
const OUTPUT_BUCKET: &str = "polly-output";

/// One `StartSpeechSynthesisTask` call, signed for `region`.
async fn start(
    polly: &Polly,
    region: &str,
    request: &serde_json::Value,
) -> http::Response<Bytes> {
    polly
        .call_in_region(region, "POST", "/v1/synthesisTasks", &request.to_string())
        .await
}

/// What one row expects the task record to carry.
struct Expected {
    /// The Region the call is signed for, which the output location names.
    region: &'static str,
    /// `SynthesisTask$OutputFormat`.
    output_format: &'static str,
    /// The suffix the output object carries for that format.
    suffix: &'static str,
    /// The key prefix the request asked for, as the object key begins with.
    key_prefix: &'static str,
    /// `SynthesisTask$Engine`.
    engine: &'static str,
    /// `SynthesisTask$TextType`.
    text_type: &'static str,
    /// `SynthesisTask$VoiceId`.
    voice_id: &'static str,
    /// `SynthesisTask$RequestCharacters` — the BILLED characters, which for `ssml`
    /// input excludes the markup.
    request_characters: i64,
}

/// Assert one reported `SynthesisTask` against what the row expects, except for its
/// status (which the clock owns) — and answer the `TaskId` it reported.
fn assert_record(case: &str, reported: &serde_json::Value, expected: &Expected) -> String {
    let task_id = reported["TaskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {reported:?}"))
        .to_string();
    assert!(
        !task_id.is_empty(),
        "case {case:?}: the task must carry a minted TaskId, got {reported:?}"
    );
    let expected_uri = format!(
        "https://s3.{}.amazonaws.com/{OUTPUT_BUCKET}/{}{task_id}.{}",
        expected.region, expected.key_prefix, expected.suffix
    );
    for (member, actual, want) in [
        ("OutputFormat", reported["OutputFormat"].as_str(), Some(expected.output_format)),
        ("Engine", reported["Engine"].as_str(), Some(expected.engine)),
        ("TextType", reported["TextType"].as_str(), Some(expected.text_type)),
        ("VoiceId", reported["VoiceId"].as_str(), Some(expected.voice_id)),
        ("OutputUri", reported["OutputUri"].as_str(), Some(expected_uri.as_str())),
    ] {
        assert_eq!(
            actual, want,
            "case {case:?}: member {member} is {actual:?}, expected {want:?}"
        );
    }
    assert_eq!(
        reported["RequestCharacters"].as_i64(),
        Some(expected.request_characters),
        "case {case:?}: wrong RequestCharacters, got {reported:?}"
    );
    assert!(
        reported["CreationTime"].as_f64().is_some_and(|t| t > 0.0),
        "case {case:?}: the task must report when it was started, got {reported:?}"
    );
    task_id
}

/// The rows: one request each, with what the task record must then carry.
fn rows() -> Vec<(&'static str, serde_json::Value, Expected)> {
    vec![
        (
            "plain text to mp3, no prefix",
            serde_json::json!({
                "Text": "Hello from the emulator.",
                "VoiceId": "Matthew",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "mp3",
                suffix: "mp3",
                key_prefix: "",
                // `StartSpeechSynthesisTaskInput$Engine` defaults to the standard
                // engine when the request names none.
                engine: "standard",
                // `TextType` "defaults to plain text".
                text_type: "text",
                voice_id: "Matthew",
                request_characters: 24,
            },
        ),
        (
            "a neural voice, an Ogg format and a key prefix, in another Region",
            serde_json::json!({
                "Text": "Twelve chars",
                "VoiceId": "Joanna",
                "Engine": "neural",
                "OutputFormat": "ogg_opus",
                "OutputS3BucketName": OUTPUT_BUCKET,
                "OutputS3KeyPrefix": "audio/2026/",
            }),
            Expected {
                region: "eu-west-1",
                output_format: "ogg_opus",
                suffix: "ogg",
                key_prefix: "audio/2026/",
                engine: "neural",
                text_type: "text",
                voice_id: "Joanna",
                request_characters: 12,
            },
        ),
        (
            "ssml input, whose markup is not billed",
            serde_json::json!({
                "Text": "<speak>Hello there.</speak>",
                "TextType": "ssml",
                "VoiceId": "Matthew",
                "OutputFormat": "pcm",
                "SampleRate": "8000",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "pcm",
                suffix: "pcm",
                key_prefix: "",
                engine: "standard",
                text_type: "ssml",
                voice_id: "Matthew",
                // "SSML tags are not counted as billed characters": only "Hello there."
                request_characters: 12,
            },
        ),
        (
            "speech marks, which ride the json output",
            serde_json::json!({
                "Text": "Hello world. Bye.",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": ["sentence", "word"],
                "OutputS3BucketName": OUTPUT_BUCKET,
                "OutputS3KeyPrefix": "marks-",
            }),
            Expected {
                region: "us-east-1",
                output_format: "json",
                suffix: "marks",
                key_prefix: "marks-",
                engine: "standard",
                text_type: "text",
                voice_id: "Joanna",
                request_characters: 17,
            },
        ),
        (
            "a bilingual voice with its other language",
            serde_json::json!({
                "Text": "Namaste",
                "VoiceId": "Aditi",
                "LanguageCode": "hi-IN",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            Expected {
                region: "us-east-1",
                output_format: "mp3",
                suffix: "mp3",
                key_prefix: "",
                engine: "standard",
                text_type: "text",
                voice_id: "Aditi",
                request_characters: 7,
            },
        ),
    ]
}

/// RULE: the start call records the task, both task reads report that record, and the
/// identifiers it mints are distinct.
#[tokio::test]
async fn a_started_task_is_held_and_reported_by_both_reads() {
    let polly = Polly::new();
    let mut started: Vec<(String, String)> = Vec::new();

    for (case, request, expected) in rows() {
        let response = start(&polly, expected.region, &request).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the task must be created, body {:?}",
            json(&response)
        );
        let body = json(&response);
        let task_id = assert_record(case, &body["SynthesisTask"], &expected);
        assert_eq!(
            body["SynthesisTask"]["TaskStatus"].as_str(),
            Some(CREATED_STATUS),
            "case {case:?}: a just-started task is not synthesized yet, got {body:?}"
        );

        // The same record reads back through GetSpeechSynthesisTask: the task is HELD,
        // not just echoed by the call that made it.
        let read = get_task(&polly, &task_id).await;
        assert_eq!(
            read.status(),
            200,
            "case {case:?}: the started task must read back, body {:?}",
            json(&read)
        );
        assert_record(case, &json(&read)["SynthesisTask"], &expected);
        started.push((case.to_string(), task_id));
    }

    // Every identifier differs, or the rows above would be one task read five times.
    for (index, (case, task_id)) in started.iter().enumerate() {
        assert!(
            started
                .iter()
                .enumerate()
                .all(|(other, (_, id))| other == index || id != task_id),
            "case {case:?}: the minted TaskId {task_id:?} was handed out twice"
        );
    }

    // And the listing reports every one of them.
    let listed = task_ids(&json(&list(&polly, "").await));
    for (case, task_id) in &started {
        assert!(
            listed.contains(task_id),
            "case {case:?}: the listing must report the started task {task_id:?}, \
             got {listed:?}"
        );
    }
    assert_eq!(
        listed.len(),
        started.len(),
        "the listing must report exactly the started tasks, got {listed:?}"
    );
}

/// RULE: the status of a started task is the CLOCK's: it is non-terminal when the task
/// is created and terminal once the completion the record carries has fallen due — and
/// the reads report the same status, because they read the same record.
#[tokio::test]
async fn the_clock_completes_a_started_task() {
    // The destination bucket exists, so the completion this test is about is the one that
    // delivers its output: `crate::synthesis_task_side_effects` is the suite about the
    // object itself, and about the task whose bucket is not there.
    let polly = Polly::with_s3();
    polly
        .s3("us-east-1")
        .create_bucket()
        .bucket(OUTPUT_BUCKET)
        .send()
        .await
        .expect("the destination bucket is created");
    let request = serde_json::json!({
        "Text": "Hello from the emulator.",
        "VoiceId": "Matthew",
        "OutputFormat": "mp3",
        "OutputS3BucketName": OUTPUT_BUCKET,
    });
    let response = start(&polly, "us-east-1", &request).await;
    assert_eq!(
        response.status(),
        200,
        "the task must be created, body {:?}",
        json(&response)
    );
    let body = json(&response);
    let task_id = body["SynthesisTask"]["TaskId"]
        .as_str()
        .expect("a created task reports its id")
        .to_string();
    assert_eq!(
        body["SynthesisTask"]["TaskStatus"].as_str(),
        Some(CREATED_STATUS),
        "a just-started task is not synthesized yet, got {body:?}"
    );

    // Past the instant the record says the completion fell due, both reads report it —
    // and a status filter for it now matches, which is the listing reading the same
    // record.
    tokio::time::sleep(TASK_COMPLETION_DELAY + Duration::from_millis(50)).await;
    let read = get_task(&polly, &task_id).await;
    assert_eq!(
        json(&read)["SynthesisTask"]["TaskStatus"].as_str(),
        Some(TERMINAL_STATUS),
        "case {task_id:?}: the completion the clock owed must be reported, body {:?}",
        json(&read)
    );
    let listed = json(&list(&polly, &format!("?Status={TERMINAL_STATUS}")).await);
    assert_eq!(
        task_ids(&listed),
        vec![task_id.clone()],
        "case {task_id:?}: the listing must report the completed task, got {listed:?}"
    );
}

/// RULE: a request the service refuses records NOTHING — one row per refusal the
/// asynchronous path answers, and the store is empty after all of them.
#[tokio::test]
async fn a_refused_request_records_no_task() {
    let polly = Polly::new();
    // The asynchronous API's own text bounds: "the maximum is 200,000 characters, of
    // which no more than 100,000 can be billed characters"
    // (`TextLengthExceededException`). One past each bound is refused.
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "speech marks with an audio format",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "SpeechMarkTypes": ["word"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "MarksNotSupportedForFormatException",
        ),
        (
            "ssml speech marks over plain text",
            serde_json::json!({
                "Text": "hello world",
                "TextType": "text",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": ["ssml"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "SsmlMarksNotSupportedForTextTypeException",
        ),
        (
            "one character past the billed bound",
            serde_json::json!({
                "Text": "a".repeat(100_001),
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "TextLengthExceededException",
        ),
        (
            "one character past the total bound",
            serde_json::json!({
                // Markup is not billed, so this text is inside the billed bound and
                // past the total one.
                "Text": format!("<speak>{}</speak>", "a".repeat(199_987)),
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "TextLengthExceededException",
        ),
        (
            "a lexicon this Region does not store",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "LexiconNames": ["Missing"],
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            404,
            "LexiconNotFoundException",
        ),
        (
            "a sample rate the format does not accept",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "pcm",
                "SampleRate": "22050",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "InvalidSampleRateException",
        ),
        (
            "an ssml document that is not well-formed",
            serde_json::json!({
                "Text": "<speak>unclosed",
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "InvalidSsmlException",
        ),
        (
            "an engine the voice does not support",
            serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Aditi",
                "Engine": "neural",
                "OutputFormat": "mp3",
                "OutputS3BucketName": OUTPUT_BUCKET,
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in &rows {
        let response = start(&polly, "us-east-1", request).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (*status, (*code).to_string()),
            "case {case:?}: wrong refusal, body {:?}",
            json(&response)
        );
    }

    let listed = task_ids(&json(&list(&polly, "").await));
    assert!(
        listed.is_empty(),
        "no refused request may record a task, got {listed:?}"
    );

    // A request inside every bound still starts a task, so the rows above are not
    // passing on a path that refuses everything.
    let accepted = start(
        &polly,
        "us-east-1",
        &serde_json::json!({
            "Text": "a".repeat(100_000),
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
            "OutputS3BucketName": OUTPUT_BUCKET,
        }),
    )
    .await;
    assert_eq!(
        accepted.status(),
        200,
        "a request at the billed bound must be accepted, body {:?}",
        json(&accepted)
    );
    assert_eq!(
        json(&accepted)["SynthesisTask"]["RequestCharacters"].as_i64(),
        Some(100_000),
        "the billed count must be the text's own, body {:?}",
        json(&accepted)
    );
}
