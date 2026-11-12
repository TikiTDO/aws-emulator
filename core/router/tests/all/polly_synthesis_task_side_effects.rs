// SPDX-License-Identifier: CC0-1.0

//! An asynchronous Amazon Polly synthesis task's side effects, through the WHOLE mounted
//! service set — the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a speech synthesis task reaches its terminal status WITH its
//! output in place — one non-empty object in the bucket `OutputS3BucketName` named, at the
//! key the task's own `OutputUri` reports and carrying the media type of the requested
//! `OutputFormat` — and a task started with an `SnsTopicArn` publishes exactly one
//! notification about the terminal status it reached, so a queue subscribed to that topic
//! (and granting the SNS service principal the send) receives one SNS `Notification`
//! envelope whose `Message` names the task and the status it ended in.**
//!
//! Three services have to agree for that to hold — Polly delivers, Amazon S3 holds the
//! object, SNS fans the notification out to SQS — and each hand-over Polly makes is an
//! in-process call addressed the way a client's own `s3`- or `sns`-scoped call is addressed,
//! so WHICH mounted model claims it is only decidable with every service mounted. The
//! crate-local suite
//! (`services/service-com-amazonaws-polly-parrot-v1/tests/all/synthesis_task_side_effects.rs`)
//! pins the object itself over a table of formats, prefixes and Regions, and the failure of a
//! task whose bucket is absent; what only this suite can reach is the same path over the
//! production service set, with the notification's own fan-out at the end of it.
//!
//! The rows differ along what the rule generalises over: the `OutputFormat` (and so the
//! object's media type), the key prefix, the Region the whole wiring lives in and how the
//! subscribed queue's policy scopes the grant.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, given_topic_with_subscriber, router, Grant,
};

/// How long a row waits for a task to reach a terminal status, and for its notification to
/// arrive, and how often it looks. Driven off what has HAPPENED rather than off one fixed
/// sleep, so a slow machine delays a row instead of breaking it — and bounded, so a row that
/// will never see its notification fails quickly instead of holding a worker.
const LOOKS: usize = 200;
const LOOK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// How many times a row polls the subscribed queue, each poll waiting
/// [`RECEIVE_WAIT_SECONDS`] for a message to arrive.
const DRAINS: usize = 20;
const RECEIVE_WAIT_SECONDS: u32 = 1;

/// The terminal statuses of Polly's `TaskStatus` enum: the two a task stops at.
const TERMINAL_STATUSES: [&str; 2] = ["completed", "failed"];

/// The `Type` an SNS notification envelope carries. Fixed text, so it is asserted as such.
const NOTIFICATION_TYPE: &str = "Notification";

/// One Amazon Polly call (restJson1, `POST`/`GET` under `/v1/`), with the status and the
/// parsed body it answered.
async fn polly(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://polly.{region}.amazonaws.com{path}"))
        .header("host", format!("polly.{region}.amazonaws.com"))
        .header("authorization", authorization("polly", region))
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One request against Amazon S3's REST surface, path-style, with the whole response.
async fn s3(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &str,
) -> http::Response<Bytes> {
    let host = format!("s3.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3", region))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    router.dispatch(request).await
}

/// `PUT /<bucket>` — `CreateBucket`, naming the Region outside `us-east-1`, which is where
/// "the bucket is created in the US East (N. Virginia) Region (us-east-1) by default"
/// (`CreateBucketConfiguration$LocationConstraint`).
async fn create_bucket(router: &MultiServiceRouter, case: &str, region: &str, bucket: &str) {
    let body = if region == "us-east-1" {
        String::new()
    } else {
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{region}</LocationConstraint></CreateBucketConfiguration>"
        )
    };
    let answer = s3(router, region, "PUT", &format!("/{bucket}"), &body).await;
    assert_eq!(
        answer.status().as_u16(),
        200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {}",
        String::from_utf8_lossy(answer.body())
    );
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`), which must answer 200.
///
/// The suite that owns the topic-and-queue fixture drains a queue into the PAYLOADS it
/// carries; this rule is about the ENVELOPE around one, so the receive is made here.
async fn sqs(router: &MultiServiceRouter, case: &str, region: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{region}.amazonaws.com"))
        .header("authorization", authorization("sqs", region))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Take every message waiting on the queue at `url`, as the SNS envelopes they are, deleting
/// what it hands over.
async fn drain_envelopes(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        region,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": RECEIVE_WAIT_SECONDS}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut envelopes = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        envelopes.push(serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: the body is not an SNS envelope ({error}): {body}")
        }));
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                case,
                region,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    envelopes
}

/// One row: the wiring, the request and what the object it leaves behind must carry.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The Region the whole wiring lives in.
    region: &'static str,
    /// `OutputFormat`.
    output_format: &'static str,
    /// `OutputS3KeyPrefix`.
    key_prefix: &'static str,
    /// The media type the written object must carry, from
    /// `SynthesizeSpeechOutput$ContentType`'s own documentation (json →
    /// application/x-json-stream).
    content_type: &'static str,
    /// `SpeechMarkTypes`, the mark types the stream must carry.
    marks: &'static [&'static str],
    /// How the subscribed queue's policy grants SNS the send.
    grant: Grant,
}

#[tokio::test]
async fn a_completed_synthesis_task_writes_its_output_and_notifies_its_topic() {
    // Both rows ask for the `json` speech-mark stream rather than audio: what only this
    // suite can reach is the cross-service wiring (the object in the bucket, the
    // notification through SNS to a subscribed queue), and the media type and body of every
    // OutputFormat — audio included — are the crate-local suite's own table. Keeping the
    // renderer out of this shared test target is deliberate: this binary holds hundreds of
    // timing-sensitive cross-service suites, and a row here that costs CPU costs it to all
    // of them. The rows differ along the Region the whole wiring lives in, the key prefix,
    // the mark types asked for and how the queue grants SNS the send.
    let rows: &[Row] = &[
        Row {
            case: "a speech-mark stream under a prefix, with the grant scoped to the topic",
            region: "us-west-2",
            output_format: "json",
            key_prefix: "synth/",
            content_type: "application/x-json-stream",
            marks: &["sentence"],
            grant: Grant::ScopedToTopic,
        },
        Row {
            case: "both mark types under another prefix, an unscoped grant, another Region",
            region: "eu-west-1",
            output_format: "json",
            key_prefix: "marks/",
            content_type: "application/x-json-stream",
            marks: &["sentence", "word"],
            grant: Grant::Unscoped,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let bucket = "polly-task-side-effects";
        create_bucket(&router, case, row.region, bucket).await;
        let (topic, queue_url) =
            given_topic_with_subscriber(&router, case, row.region, "polly-task-status", row.grant)
                .await;

        let mut request = json!({
            "Text": "Hi there.",
            "VoiceId": "Joanna",
            "OutputFormat": row.output_format,
            "OutputS3BucketName": bucket,
            "OutputS3KeyPrefix": row.key_prefix,
            "SnsTopicArn": topic.clone(),
        });
        if !row.marks.is_empty() {
            request["SpeechMarkTypes"] = json!(row.marks.to_vec());
        }
        let (status, started) = polly(
            &router,
            row.region,
            "POST",
            "/v1/synthesisTasks",
            &request.to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the task must be created, got {status} with {started}"
        );
        let task_id = started["SynthesisTask"]["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no TaskId in {started}"))
            .to_string();

        // Polled to its terminal status, exactly as a client does.
        let mut reported = Value::Null;
        for _ in 0..LOOKS {
            let (status, body) = polly(
                &router,
                row.region,
                "GET",
                &format!("/v1/synthesisTasks/{task_id}"),
                "",
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the task must read back, got {status} with {body}"
            );
            reported = body["SynthesisTask"].clone();
            if TERMINAL_STATUSES.contains(&reported["TaskStatus"].as_str().unwrap_or_default()) {
                break;
            }
            tokio::time::sleep(LOOK_INTERVAL).await;
        }
        assert_eq!(
            reported["TaskStatus"].as_str(),
            Some("completed"),
            "case {case:?}: the task must complete, last report {reported}"
        );

        // The object is where the task says it is, non-empty, with the format's media type.
        let output_uri = reported["OutputUri"].as_str().unwrap_or_default().to_string();
        let key = output_uri
            .split_once(&format!("/{bucket}/"))
            .map(|(_, key)| key.to_string())
            .unwrap_or_else(|| {
                panic!("case {case:?}: OutputUri {output_uri:?} names no key in {bucket:?}")
            });
        assert!(
            key.starts_with(row.key_prefix),
            "case {case:?}: the key {key:?} must carry the requested prefix {:?}",
            row.key_prefix
        );
        let object = s3(&router, row.region, "GET", &format!("/{bucket}/{key}"), "").await;
        assert_eq!(
            object.status().as_u16(),
            200,
            "case {case:?}: GetObject({key:?}) must answer the task's own output, got {}",
            String::from_utf8_lossy(object.body())
        );
        assert!(
            !object.body().is_empty(),
            "case {case:?}: the written object must carry the task's output"
        );
        assert_eq!(
            object
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some(row.content_type),
            "case {case:?}: the object must carry the format's media type"
        );
        // The body IS the speech-mark stream the request asked for: one JSON record per
        // line, carrying each requested mark type.
        let body = String::from_utf8_lossy(object.body()).into_owned();
        let records: Vec<Value> = body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: line {line:?} is not a mark record: {error}")
                })
            })
            .collect();
        for wanted in row.marks {
            assert!(
                records
                    .iter()
                    .any(|record| record["type"].as_str() == Some(*wanted)),
                "case {case:?}: the stream must carry a {wanted:?} mark, got {records:?}"
            );
        }

        // And exactly one notification about the terminal status reached the queue
        // subscribed to the topic the start call named.
        let mut delivered: Vec<Value> = Vec::new();
        for _ in 0..DRAINS {
            if !delivered.is_empty() {
                break;
            }
            delivered.extend(drain_envelopes(&router, case, row.region, &queue_url).await);
        }
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the completed task must notify its topic exactly once, got \
             {delivered:?}"
        );
        let envelope = &delivered[0];
        assert_eq!(
            envelope["Type"].as_str(),
            Some(NOTIFICATION_TYPE),
            "case {case:?}: the queue must receive an SNS notification envelope, got {envelope}"
        );
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the envelope must name the topic the task was started with, got \
             {envelope}"
        );
        let message = envelope["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&task_id),
            "case {case:?}: the notification must name the task {task_id:?}, got {message:?}"
        );
        assert!(
            message.contains("completed"),
            "case {case:?}: the notification must carry the terminal status, got {message:?}"
        );
        let payload: Value = serde_json::from_str(message).unwrap_or_else(|error| {
            panic!("case {case:?}: the notification is not a JSON document ({error}): {message}")
        });
        assert_eq!(
            (payload["taskId"].as_str(), payload["taskStatus"].as_str()),
            (Some(task_id.as_str()), Some("completed")),
            "case {case:?}: the notification must carry the task and its status, got {payload}"
        );

        // Nothing is delivered twice: a second look, after the first was drained, finds the
        // queue empty.
        let again = drain_envelopes(&router, case, row.region, &queue_url).await;
        assert!(
            again.is_empty(),
            "case {case:?}: one terminal status is one notification, got {again:?}"
        );
    }
}
