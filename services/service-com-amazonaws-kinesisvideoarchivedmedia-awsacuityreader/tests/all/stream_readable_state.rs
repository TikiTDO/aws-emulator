// SPDX-License-Identifier: CC0-1.0

//! WHEN a stream the Kinesis Video Streams control plane holds is readable on
//! the archived-media plane.
//!
//! THE RULE UNDER TEST: an archived-media read resolves the stream it addresses
//! in the Kinesis Video Streams registry and serves it only while that registry
//! holds the record in the one status that serves reads, `ACTIVE`. A record the
//! registry holds in any other modelled `StreamInfo$Status` — and a record whose
//! removal the clock has already made due, whether or not a control-plane call
//! has run since — is as unreadable as an identifier the registry never held:
//! HTTP 404, `ResourceNotFoundException`, with the fixed sentence "The requested
//! stream is not found or not active." in `Message` and the error type in the
//! `x-amzn-errortype` header.
//!
//! The tables span what the rule generalises over: the status the registry holds
//! the record in (all four values of the modelled enum —
//! `smithy_query(query="shape",shape="Status",model_path="services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/smithy-model/com-amazonaws-kinesisvideo-kinesisvideo-20170930.json")`),
//! how the request addresses its stream (`StreamName`, `StreamARN`) and which
//! read is made (all six stream-addressed operations of this plane).
//!
//! Every case drives one router carrying both emulated services over ONE shared
//! engine, through the same request walk a socket-served request takes, so a
//! stream created through `CreateStream` is the very record a read resolves.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use smithy_server_runtime::MultiServiceRouter;

use archived_media::state::{from_epoch_millis, service_state, ArchivedStream, StoredFragment};

/// The emulator endpoint every case addresses: this emulator, never a real AWS
/// host.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The base instant the archived fragments of this suite carry.
const T0_SECS: i64 = 1_761_000_000;

/// How far past [`T0_SECS`] every selector in this suite reaches. Inside
/// `GetImages`' own 300-second window ceiling, so one window serves all six
/// reads.
const WINDOW_SECS: i64 = 100;

/// A fragment number shaped like a real one: a long, increasing decimal, per the
/// modelled `FragmentNumberString` (`^[0-9]+$`).
const FRAGMENT_BASE: u64 = 9_134_385_233_318_143_200;

/// AWS's fixed sentence for an archived-media read whose stream does not resolve:
/// it states both halves of the condition and names neither the stream nor the
/// identifier form the request used.
const NOT_FOUND_OR_NOT_ACTIVE: &str = "The requested stream is not found or not active.";

/// How long the suite is prepared to keep looking for a condition the clock owes
/// — far longer than any span this emulator uses, so a loaded host still passes.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two looks at a condition the clock owes.
const LOOK_AGAIN: Duration = Duration::from_millis(10);

/// One router over one engine, carrying the archived-media reader AND the
/// Kinesis Video Streams control plane.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// What one call answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The `x-amzn-errortype` header, empty when the answer carried none.
    error_type_header: String,
    body: serde_json::Value,
}

impl Reply {
    /// The error code the answer states, from the body's `__type` or, failing
    /// that, the `x-amzn-errortype` header.
    fn code(&self) -> String {
        self.body
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(self.error_type_header.as_str())
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message`/`message` the answer states, whichever member carries it.
    fn message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether this answer is the plane's unresolvable-stream refusal.
    fn is_not_found_or_not_active(&self) -> bool {
        self.status == 404 && self.code() == "ResourceNotFoundException"
            && self.message() == NOT_FOUND_OR_NOT_ACTIVE
    }
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call to this emulator and report what it answered.
    async fn call(&self, path: &str, body: serde_json::Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json")
            // The SigV4 credential scope routes the request and names its
            // region; the signature itself is not checked here.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        Reply {
            status,
            error_type_header,
            body,
        }
    }

    /// Create `name` through the control plane, wait for it to be readable, and
    /// answer its `StreamARN`.
    async fn create_stream(&self, name: &str) -> String {
        let reply = self
            .call(
                "/createStream",
                serde_json::json!({ "StreamName": name, "DataRetentionInHours": 24 }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case create {name:?}: expected HTTP 200, got {reply:?}"
        );
        let arn = reply.body["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case create {name:?}: no StreamARN in {reply:?}"))
            .to_string();
        self.await_status(name, control_plane::state::STATUS_ACTIVE)
            .await;
        arn
    }

    /// Wait until the control plane reports `name` in `status`, as a client does
    /// before reading a stream it just created ("CreateStream is an asynchronous
    /// operation").
    ///
    /// Waits for the CONDITION, not for a span: it looks again until the status
    /// holds, under a deadline far longer than the transition needs.
    async fn await_status(&self, name: &str, status: &str) {
        let deadline = Instant::now() + DEADLINE;
        let mut last = String::new();
        while Instant::now() < deadline {
            let reply = self
                .call("/describeStream", serde_json::json!({ "StreamName": name }))
                .await;
            last = reply.body["StreamInfo"]["Status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if last == status {
                return;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
        panic!("case create {name:?}: expected Status {status} before the deadline, got {last:?}");
    }

    /// Hold the registry's record for `name` in `status`, with no transition and
    /// no removal owed, so one row observes one status.
    fn hold_in_status(&self, name: &str, status: &str) {
        let registry = control_plane::state::service_state(&*self.engine)
            .expect("the control-plane store");
        let mut registry = registry.lock();
        let stream = registry
            .streams
            .get_mut(name)
            .unwrap_or_else(|| panic!("case hold {name:?} in {status}: no such registered stream"));
        stream.status = status.to_string();
        stream.activates_at = None;
        stream.deletes_at = None;
    }

    /// Whether the registry still holds a record for `name`.
    fn registry_holds(&self, name: &str) -> bool {
        let registry = control_plane::state::service_state(&*self.engine)
            .expect("the control-plane store");
        let held = registry.lock().streams.contains_key(name);
        held
    }

    /// Put `count` fragments into `stream`'s archive, 10 seconds apart from
    /// [`T0_SECS`], so a readable stream has media for every read to answer from.
    fn seed_archive(&self, stream: &str, count: u64) {
        let state = service_state(&*self.engine).expect("the archived-media store");
        let mut state = state.lock();
        state.put_stream(ArchivedStream::new(stream, REGION, SystemTime::now(), 24));
        for index in 0..count {
            let offset = 10 * i64::try_from(index).expect("a small index");
            let at = from_epoch_millis((T0_SECS + offset) * 1000);
            assert!(
                state.append_fragment(
                    stream,
                    StoredFragment::new(
                        fragment_number(index),
                        at,
                        at,
                        2000,
                        vec![b'a' + u8::try_from(index).expect("a small index"); 16],
                    ),
                ),
                "case seed {stream:?} fragment {index}: the archive must accept it"
            );
        }
    }
}

/// The `index`-th seeded fragment number.
fn fragment_number(index: u64) -> String {
    (FRAGMENT_BASE + index).to_string()
}

/// The timestamp range every selector in this suite asks for.
fn window() -> serde_json::Value {
    serde_json::json!({ "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + WINDOW_SECS })
}

/// Every stream-addressed read of this plane, with a body that reaches the
/// operation's own verdict once the stream-naming member is added.
fn every_read() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "/listFragments",
            serde_json::json!({
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": window(),
                },
            }),
        ),
        (
            "/getMediaForFragmentList",
            serde_json::json!({ "Fragments": [fragment_number(0)] }),
        ),
        (
            "/getClip",
            serde_json::json!({
                "ClipFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": window(),
                },
            }),
        ),
        (
            "/getHLSStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": window(),
                },
            }),
        ),
        (
            "/getDASHStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": window(),
                },
            }),
        ),
        (
            "/getImages",
            serde_json::json!({
                "ImageSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + WINDOW_SECS,
                "Format": "JPEG",
            }),
        ),
    ]
}

/// `base` with every member of `extra` added.
fn with_members(base: &serde_json::Value, extra: &serde_json::Value) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().expect("an object of members") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(object)
}

/// RULE: `ACTIVE` is the only registry status an archived-media read serves. A
/// record the registry holds in any other modelled status is refused as the
/// missing resource it is — 404 `ResourceNotFoundException`, the fixed
/// not-found-or-not-active sentence, the error type on the header — by every
/// read, in both addressing forms.
///
/// The rows differ along the status the registry holds the record in (every
/// value of the modelled `Status` enum), the addressing form and the read.
#[tokio::test]
async fn only_an_active_registered_stream_is_read() {
    let harness = Harness::new();

    // (the case, the status held, whether a read of it is served). Every value of
    // the modelled `StreamInfo$Status` enum, the one that serves reads included.
    let statuses: [(&str, &str, bool); 4] = [
        ("the status that serves reads", control_plane::state::STATUS_ACTIVE, true),
        ("created and not yet active", control_plane::state::STATUS_CREATING, false),
        ("being updated", control_plane::state::STATUS_UPDATING, false),
        ("marked for deletion", control_plane::state::STATUS_DELETING, false),
    ];

    for (case, status, served) in statuses {
        let name = format!("probe-status-{}", status.to_lowercase());
        let arn = harness.create_stream(&name).await;
        harness.seed_archive(&name, 3);
        harness.hold_in_status(&name, status);

        for (path, body) in every_read() {
            for (form, addressing) in [
                (
                    "StreamName",
                    serde_json::json!({ "StreamName": name.clone() }),
                ),
                ("StreamARN", serde_json::json!({ "StreamARN": arn.clone() })),
            ] {
                let reply = harness.call(path, with_members(&body, &addressing)).await;
                if served {
                    assert!(
                        !reply.is_not_found_or_not_active(),
                        "case {case} / {path} by {form}: a stream the registry holds ACTIVE must \
                         be read, got {reply:?}"
                    );
                    assert_eq!(
                        reply.status, 200,
                        "case {case} / {path} by {form}: expected HTTP 200, got {reply:?}"
                    );
                } else {
                    assert_eq!(
                        reply.status, 404,
                        "case {case} / {path} by {form}: expected HTTP 404, got {reply:?}"
                    );
                    assert_eq!(
                        reply.code(),
                        "ResourceNotFoundException",
                        "case {case} / {path} by {form}: expected ResourceNotFoundException, got \
                         {reply:?}"
                    );
                    assert_eq!(
                        reply.message(),
                        NOT_FOUND_OR_NOT_ACTIVE,
                        "case {case} / {path} by {form}: expected the fixed sentence, got \
                         {reply:?}"
                    );
                    assert_eq!(
                        reply.error_type_header, "ResourceNotFoundException",
                        "case {case} / {path} by {form}: expected the error type on the header, \
                         got {reply:?}"
                    );
                }
            }
        }
    }
}

/// RULE (the other half): the state a read resolves against is the registry's as
/// of THIS call, so a deletion the clock has already made due is observed by the
/// read itself — no control-plane call has to run in between for the stream to
/// stop being readable, and a client polling a deleted stream is refused rather
/// than served for as long as nobody touches the control plane.
#[tokio::test]
async fn a_deleted_stream_stops_being_read_once_its_removal_falls_due() {
    let harness = Harness::new();
    let name = "probe-deleted-stream";
    let arn = harness.create_stream(name).await;

    // Readable first, so the refusal below is the delete's effect and not a
    // stream that was never there.
    let listed = harness
        .call(
            "/listFragments",
            with_members(
                &every_read()[0].1,
                &serde_json::json!({ "StreamName": name }),
            ),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case before the delete: expected HTTP 200, got {listed:?}"
    );

    let deleted = harness
        .call("/deleteStream", serde_json::json!({ "StreamARN": arn }))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected HTTP 200, got {deleted:?}"
    );

    // From the delete onwards every read refuses, through the whole life of the
    // record: while the registry still holds it as deleting, and after the
    // removal the delete made due — which the read itself materialises, since
    // nothing but archived-media calls run in this loop.
    let deadline = Instant::now() + DEADLINE;
    let mut removed = false;
    while Instant::now() < deadline {
        for (path, body) in every_read() {
            for (form, addressing) in [
                ("StreamName", serde_json::json!({ "StreamName": name })),
                ("StreamARN", serde_json::json!({ "StreamARN": arn })),
            ] {
                let reply = harness.call(path, with_members(&body, &addressing)).await;
                assert!(
                    reply.is_not_found_or_not_active(),
                    "case deleted / {path} by {form}: expected 404 ResourceNotFoundException with \
                     the fixed sentence, got {reply:?}"
                );
            }
        }
        if !harness.registry_holds(name) {
            removed = true;
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert!(
        removed,
        "case deleted: the reads alone must settle the removal the delete made due, but the \
         registry still holds {name:?}"
    );
}
