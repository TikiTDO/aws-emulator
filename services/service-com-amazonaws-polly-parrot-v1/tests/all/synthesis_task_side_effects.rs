// SPDX-License-Identifier: CC0-1.0

//! What a COMPLETED speech synthesis task does outside this service, driven end to end
//! through the same router the emulator serves with, with Amazon S3 mounted beside Polly on
//! one engine — so "the object is there" is answered by the service that owns the bucket.
//!
//! RULE: a synthesis task reaches its terminal status WITH its output in place — exactly one
//! object in the bucket `OutputS3BucketName` named, at the key the task's own `OutputUri`
//! reports (which carries the `OutputS3KeyPrefix` the request asked for), non-empty and
//! carrying the media type of the requested `OutputFormat`; for `json` with
//! `SpeechMarkTypes` that object is the newline-delimited speech-mark stream of the same
//! utterance, carrying the mark types the request asked for. A task whose output cannot be
//! written does not report a completion: it reaches `failed`, with a reason naming the
//! bucket it could not write to.
//!
//! RULE: `ListSpeechSynthesisTasks` reports the tasks newest first, so the first task of the
//! first page is the most recently created one and the listed order agrees with the
//! `CreationTime` each task itself reports.
//!
//! Both rules are checked over a TABLE rather than the one request a trace quotes: the rows
//! differ along the `OutputFormat` (and so the object's suffix, its media type and whether
//! its body is audio or a mark stream), the key prefix (none, a bare prefix, and one spelling
//! a folder), the sample rate and the Region the call is signed for.

use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;

use crate::lexicons_and_speech::{json, Polly};
use crate::support::get_task;
use crate::synthesis_tasks::list;

/// The bucket the rows write their output to.
const OUTPUT_BUCKET: &str = "polly-task-output";

/// How long a row waits for a task to reach a terminal status, and how often it looks.
/// Driven off what has HAPPENED rather than off one fixed sleep, so a slow machine delays a
/// row instead of breaking it.
const TERMINAL_WAIT: Duration = Duration::from_secs(20);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The terminal statuses of the model's `TaskStatus` enum
/// (`smithy_query(query="shape", shape="TaskStatus")`): the two a task stops at.
const TERMINAL_STATUSES: [&str; 2] = ["completed", "failed"];

/// One row: a start request, and what the object it must leave behind looks like.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The Region the call is signed for, which the output location names.
    region: &'static str,
    /// `OutputFormat`.
    output_format: &'static str,
    /// `SampleRate`, where the row names one.
    sample_rate: Option<&'static str>,
    /// `SpeechMarkTypes`, which only `json` output carries.
    marks: &'static [&'static str],
    /// `OutputS3KeyPrefix`, where the row names one.
    key_prefix: Option<&'static str>,
    /// The media type the written object must carry.
    ///
    /// From `SynthesizeSpeechOutput$ContentType`'s own documentation, which is the one
    /// mapping of `OutputFormat` to a media type this service has: mp3 → audio/mpeg,
    /// ogg_vorbis and ogg_opus → audio/ogg, pcm → audio/pcm, mu-law → audio/mulaw, a-law →
    /// audio/alaw, json → application/x-json-stream.
    content_type: &'static str,
    /// The suffix the object's key must carry for that format.
    suffix: &'static str,
}

/// The rows: one started task each, spanning the formats, the prefixes and the Regions.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "mp3 with no prefix at all",
            region: "us-east-1",
            output_format: "mp3",
            sample_rate: None,
            marks: &[],
            key_prefix: None,
            content_type: "audio/mpeg",
            suffix: "mp3",
        },
        Row {
            case: "pcm at a named sample rate, under a folder-like prefix, in another Region",
            region: "eu-west-1",
            output_format: "pcm",
            sample_rate: Some("8000"),
            marks: &[],
            key_prefix: Some("synth/2026/"),
            content_type: "audio/pcm",
            suffix: "pcm",
        },
        Row {
            case: "ogg_vorbis under a bare prefix, which is a key prefix and not a folder",
            region: "us-east-1",
            output_format: "ogg_vorbis",
            sample_rate: Some("22050"),
            marks: &[],
            key_prefix: Some("custom-prefix"),
            content_type: "audio/ogg",
            suffix: "ogg",
        },
        Row {
            case: "a-law, the companded format, under a deep prefix",
            region: "us-west-2",
            output_format: "alaw",
            sample_rate: None,
            marks: &[],
            key_prefix: Some("deep/nested/path/"),
            content_type: "audio/alaw",
            suffix: "alaw",
        },
        Row {
            case: "json speech marks, which are a stream rather than audio",
            region: "us-east-1",
            output_format: "json",
            sample_rate: None,
            marks: &["sentence", "word"],
            key_prefix: Some("marks-"),
            content_type: "application/x-json-stream",
            suffix: "marks",
        },
    ]
}

/// The bucket a row writes its output to.
///
/// One per row, because a bucket name is global and a row's bucket lives in that row's own
/// Region: `OutputS3BucketName` matches `^[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9]$`, which this
/// spelling satisfies.
fn bucket_for(index: usize) -> String {
    format!("{OUTPUT_BUCKET}-{index}")
}

/// Create `bucket` in `region` through the object store itself.
///
/// Outside `us-east-1` a create has to name the Region it is making the bucket in —
/// "If you don't specify a Region, the bucket is created in the US East (N. Virginia) Region
/// (us-east-1) by default" (`CreateBucketConfiguration$LocationConstraint`) — so the
/// constraint travels with every create the rows make outside it.
async fn create_bucket(polly: &Polly, case: &str, region: &str, bucket: &str) {
    let mut create = polly.s3(region).create_bucket().bucket(bucket);
    if region != "us-east-1" {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    create
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the bucket must be created: {error}"));
}

/// The request one row starts its task with.
fn request(row: &Row, bucket: &str) -> Value {
    let mut body = serde_json::json!({
        "Text": "Hello world. The emulator speaks.",
        "VoiceId": "Joanna",
        "OutputFormat": row.output_format,
        "OutputS3BucketName": bucket,
    });
    if let Some(prefix) = row.key_prefix {
        body["OutputS3KeyPrefix"] = Value::from(prefix);
    }
    if let Some(rate) = row.sample_rate {
        body["SampleRate"] = Value::from(rate);
    }
    if !row.marks.is_empty() {
        body["SpeechMarkTypes"] = Value::from(row.marks.to_vec());
    }
    body
}

/// Start one task, which must be accepted, and answer the `SynthesisTask` it reported.
async fn start(polly: &Polly, case: &str, region: &str, body: &Value) -> Value {
    let response: http::Response<Bytes> = polly
        .call_in_region(region, "POST", "/v1/synthesisTasks", &body.to_string())
        .await;
    assert_eq!(
        response.status(),
        200,
        "case {case:?}: the task must be created, body {:?}",
        json(&response)
    );
    json(&response)["SynthesisTask"].clone()
}

/// Poll the task until it reaches a terminal status, and answer the record it then reports.
async fn awaited_terminal(polly: &Polly, case: &str, task_id: &str) -> Value {
    let looks = TERMINAL_WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = Value::Null;
    for _ in 0..looks {
        let response = get_task(polly, task_id).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: the started task must read back, body {:?}",
            json(&response)
        );
        reported = json(&response)["SynthesisTask"].clone();
        if TERMINAL_STATUSES.contains(&reported["TaskStatus"].as_str().unwrap_or_default()) {
            return reported;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the task never reached a terminal status, last report {reported}")
}

/// The key the task's `OutputUri` names, inside `bucket`.
///
/// `SynthesisTask$OutputUri` is the "Pathway for the output speech file", so the key is what
/// follows the bucket in that pathway — read out of the URI the task itself reported rather
/// than rebuilt here, which is the whole point: a consumer handed the `OutputUri` reads
/// exactly this key.
fn key_of(case: &str, output_uri: &str, bucket: &str) -> String {
    let (_, key) = output_uri
        .split_once(&format!("/{bucket}/"))
        .unwrap_or_else(|| panic!("case {case:?}: OutputUri {output_uri:?} names no key in {bucket:?}"));
    key.to_string()
}

#[tokio::test]
async fn a_completed_task_leaves_its_output_in_the_named_bucket() {
    let polly = Polly::with_s3();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let bucket = bucket_for(index);
        let bucket = bucket.as_str();
        let s3 = polly.s3(row.region);
        create_bucket(&polly, case, row.region, bucket).await;

        let started = start(&polly, case, row.region, &request(&row, bucket)).await;
        let task_id = started["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {started}"))
            .to_string();
        let terminal = awaited_terminal(&polly, case, &task_id).await;
        assert_eq!(
            terminal["TaskStatus"].as_str(),
            Some("completed"),
            "case {case:?}: a task whose bucket exists must complete, got {terminal}"
        );

        // The key the task reports is where the object is — and it carries the prefix the
        // request asked for and the suffix of the requested format.
        let key = key_of(
            case,
            terminal["OutputUri"].as_str().unwrap_or_default(),
            bucket,
        );
        assert!(
            key.starts_with(row.key_prefix.unwrap_or_default()),
            "case {case:?}: the key {key:?} must carry the requested prefix {:?}",
            row.key_prefix
        );
        assert!(
            key.ends_with(&format!(".{}", row.suffix)),
            "case {case:?}: the key {key:?} must name the format's own suffix {:?}",
            row.suffix
        );

        // Exactly ONE object under the prefix, and it is that key: a completed task writes
        // its output once, where it said it would.
        let listed = s3
            .list_objects_v2()
            .bucket(bucket)
            .prefix(row.key_prefix.unwrap_or_default())
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the prefix must list: {error}"));
        let keys: Vec<&str> = listed
            .contents()
            .iter()
            .filter_map(::aws_sdk_s3::types::Object::key)
            .collect();
        assert_eq!(
            keys,
            vec![key.as_str()],
            "case {case:?}: the prefix must hold exactly the object the task reported"
        );
        let size = listed
            .contents()
            .first()
            .and_then(::aws_sdk_s3::types::Object::size)
            .unwrap_or_default();
        assert!(
            size > 0,
            "case {case:?}: the written object must carry the task's output, got {size} bytes"
        );

        // Its media type is the one the requested format documents, and its body is what
        // that format carries.
        let object = s3
            .get_object()
            .bucket(bucket)
            .key(&key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetObject({key:?}) must answer: {error}"));
        assert_eq!(
            object.content_type(),
            Some(row.content_type),
            "case {case:?}: the object must carry the format's media type"
        );
        let body = object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the body must be readable: {error}"))
            .into_bytes();
        assert_eq!(
            i64::try_from(body.len()).unwrap_or_default(),
            size,
            "case {case:?}: the listing and the object must agree on its size"
        );

        if !row.marks.is_empty() {
            // `json` output IS a newline-delimited speech-mark stream, and the records
            // carry the types the request asked for.
            let text = String::from_utf8_lossy(&body);
            let records: Vec<Value> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).unwrap_or_else(|error| {
                        panic!("case {case:?}: line {line:?} is not a mark record: {error}")
                    })
                })
                .collect();
            assert!(
                !records.is_empty(),
                "case {case:?}: the marks object must carry records, got {text:?}"
            );
            for wanted in row.marks {
                assert!(
                    records
                        .iter()
                        .any(|record| record["type"].as_str() == Some(*wanted)),
                    "case {case:?}: the stream must carry a {wanted:?} mark, got {records:?}"
                );
            }
        }
    }
}

/// RULE (the other half): a task whose output cannot be written reports no completion — it
/// reaches `failed`, carrying a reason that names the bucket, and writes nothing anywhere.
#[tokio::test]
async fn a_task_whose_bucket_is_absent_fails_rather_than_completing() {
    let polly = Polly::with_s3();
    // One bucket that exists, so the rows below differ only in whether their own
    // destination does.
    let s3 = polly.s3("us-east-1");
    create_bucket(&polly, "the control bucket", "us-east-1", OUTPUT_BUCKET).await;

    for (case, bucket, expected) in [
        ("a bucket nothing created", "polly-absent-bucket", "failed"),
        ("the bucket that exists", OUTPUT_BUCKET, "completed"),
    ] {
        let row = Row {
            case,
            region: "us-east-1",
            output_format: "mp3",
            sample_rate: None,
            marks: &[],
            key_prefix: Some("fallible/"),
            content_type: "audio/mpeg",
            suffix: "mp3",
        };
        let started = start(&polly, case, row.region, &request(&row, bucket)).await;
        let task_id = started["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {started}"))
            .to_string();
        let terminal = awaited_terminal(&polly, case, &task_id).await;
        assert_eq!(
            terminal["TaskStatus"].as_str(),
            Some(expected),
            "case {case:?}: wrong terminal status, got {terminal}"
        );
        let reason = terminal["TaskStatusReason"].as_str();
        if expected == "failed" {
            assert!(
                reason.is_some_and(|reason| reason.contains(bucket)),
                "case {case:?}: a failed task must state which bucket it could not write to, \
                 got {reason:?}"
            );
            // And nothing landed in the bucket that does exist, so the failure is the
            // write it names and not a write that went elsewhere.
            let listed = s3
                .list_objects_v2()
                .bucket(OUTPUT_BUCKET)
                .send()
                .await
                .expect("the control bucket lists");
            assert!(
                listed.contents().is_empty(),
                "case {case:?}: a task that could not write must write nothing, got {:?}",
                listed.contents()
            );
        } else {
            assert_eq!(
                reason, None,
                "case {case:?}: a completed task carries no failure reason, got {reason:?}"
            );
        }
    }
}

/// RULE: the listing is newest first, so a client reading the first element finds its most
/// recent task — and that order agrees with the `CreationTime` each task reports.
#[tokio::test]
async fn the_listing_reports_the_most_recently_started_task_first() {
    let polly = Polly::with_s3();
    create_bucket(&polly, "the ordered rows", "us-east-1", OUTPUT_BUCKET).await;

    // Three tasks started in sequence, each completed before the next is started, so their
    // creation order is unambiguous.
    let mut started: Vec<String> = Vec::new();
    for index in 0..3 {
        let case = format!("task {index}");
        let row = Row {
            case: "sequence",
            region: "us-east-1",
            output_format: "mp3",
            sample_rate: None,
            marks: &[],
            key_prefix: Some("ordered/"),
            content_type: "audio/mpeg",
            suffix: "mp3",
        };
        let task = start(&polly, &case, row.region, &request(&row, OUTPUT_BUCKET)).await;
        let task_id = task["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {task}"))
            .to_string();
        awaited_terminal(&polly, &case, &task_id).await;
        started.push(task_id);
    }

    let body = json(&list(&polly, "").await);
    let reported = body["SynthesisTasks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let listed: Vec<&str> = reported
        .iter()
        .map(|task| task["TaskId"].as_str().unwrap_or_default())
        .collect();
    let newest_first: Vec<&str> = started.iter().rev().map(String::as_str).collect();
    assert_eq!(
        listed, newest_first,
        "the listing must report the most recently started task first, got {listed:?}"
    );

    // ... and the order it served agrees with what the tasks themselves report, so the
    // direction is a property of the records and not of the response alone.
    let times: Vec<f64> = reported
        .iter()
        .map(|task| task["CreationTime"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] >= pair[1]),
        "the listed order must be descending CreationTime, got {times:?}"
    );

    // One page at a time walks the same order: the first page is the newest task, and the
    // cursor resumes in that direction rather than starting over.
    let first = json(&list(&polly, "?MaxResults=1").await);
    assert_eq!(
        first["SynthesisTasks"][0]["TaskId"].as_str(),
        Some(newest_first[0]),
        "the first page must be the newest task, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .expect("three tasks do not fit in a one-task page")
        .to_string();
    let second = json(&list(&polly, &format!("?MaxResults=1&NextToken={token}")).await);
    assert_eq!(
        second["SynthesisTasks"][0]["TaskId"].as_str(),
        Some(newest_first[1]),
        "the page after the newest task must be the next-newest, got {second}"
    );
}
