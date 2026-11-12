// SPDX-License-Identifier: CC0-1.0

//! Fixtures the speech-synthesis-task suites share.
//!
//! One definition of what a started task looks like in the store, one way to seed it,
//! one way to read a task back and one reader for the `message` slot of an error
//! document, so `error_semantics` (the read path), `synthesis_tasks` (the list path),
//! `task_id_validity` (the two `TaskId` refusals) and `error_document_message` (the
//! rendering) cannot drift on any of them. The request harness and the JSON readers
//! live with the lexicon suite (`crate::lexicons_and_speech`), which every suite drives
//! the service through.
//!
//! The one PLS document whose replacement is measured against the lexeme quota lives
//! here for the same reason: `error_semantics` (the refusals and their messages),
//! `error_document_message` (the rendering) and `shared_pls_fixture` (the shared path
//! itself) all build it here, so a change to the document — its alphabet, its language,
//! how many lexemes it carries, how the replacement nests — reaches every suite at once.

use std::time::SystemTime;

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::state::{service_state, StoredSynthesisTask};

use crate::lexicons_and_speech::{json, Polly};

/// One `GetSpeechSynthesisTask` call.
pub(crate) async fn get_task(polly: &Polly, task_id: &str) -> http::Response<Bytes> {
    polly
        .call("GET", &format!("/v1/synthesisTasks/{task_id}"), "")
        .await
}

/// The `message` slot of an error document: the JSON value under the key the error
/// shape's member occupies, or `None` when the document carries NO such key at all —
/// the difference a `body["message"].as_str()` reader cannot see.
pub(crate) fn message_slot(response: &http::Response<Bytes>) -> Option<serde_json::Value> {
    let body = json(response);
    for key in ["message", "Message"] {
        if let Some(value) = body.get(key) {
            return Some(value.clone());
        }
    }
    None
}

/// A PLS lexicon with one lexeme whose `element` replacement is `length` characters.
pub(crate) fn pls_with_replacement(element: &str, length: usize) -> String {
    format!(
        "<lexicon version=\"1.0\" \
         xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
         alphabet=\"ipa\" xml:lang=\"en-US\">\
         <lexeme><grapheme>G</grapheme><{element}>{}</{element}></lexeme></lexicon>",
        "x".repeat(length)
    )
}

/// A `TaskId` in the form Amazon Polly mints one — a UUID in the canonical hyphenated
/// form — derived from `label`, so a fixture still reads as the case it is about and
/// two labels never collide.
///
/// A started task's id is minted, never chosen, so a fixture that stands in for a
/// started task uses the minted form rather than a prose id.
pub(crate) fn fixture_task_id(label: &str) -> String {
    ::uuid::Uuid::new_v5(&::uuid::Uuid::NAMESPACE_OID, label.as_bytes())
        .hyphenated()
        .to_string()
}

/// A synthesis task as a start call would have recorded it. A caller overrides the
/// members its case is about.
pub(crate) fn stored_task(task_id: &str, created: SystemTime) -> StoredSynthesisTask {
    StoredSynthesisTask {
        task_id: task_id.to_string(),
        engine: "standard".to_string(),
        output_format: "mp3".to_string(),
        output_uri: format!("s3://polly-output/{task_id}.mp3"),
        creation_time: created,
        request_characters: 11,
        sns_topic_arn: None,
        lexicon_names: Vec::new(),
        sample_rate: Some("22050".to_string()),
        speech_mark_types: Vec::new(),
        text_type: "text".to_string(),
        voice_id: "Joanna".to_string(),
        language_code: None,
        task_status: "inProgress".to_string(),
        task_status_reason: None,
        pending_status: None,
        // A seeded task stands for one whose output delivery is already behind it, so the
        // clock alone settles what it is owed; `crate::synthesis_task_side_effects` is the
        // suite that drives the delivery itself.
        output_delivery: None,
    }
}

/// Put a task in the store the way a start call would.
///
/// `StartSpeechSynthesisTask` records a task with the creation time and the status of
/// the instant it was called, so a suite whose rows fix a creation time, a status or a
/// pending transition of their own seeds the store through the shared engine instead.
/// `crate::speech_synthesis_task_creation` is the suite that drives the start call
/// itself.
pub(crate) fn seed_task(polly: &Polly, task: &StoredSynthesisTask) {
    let state = service_state(&**polly.engine()).expect("the in-memory engine carries the state");
    state
        .lock()
        .synthesis_tasks
        .insert(task.task_id.clone(), task.clone());
}
