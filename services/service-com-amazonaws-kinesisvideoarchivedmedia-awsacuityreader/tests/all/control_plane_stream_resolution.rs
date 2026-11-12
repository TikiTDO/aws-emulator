// SPDX-License-Identifier: CC0-1.0

//! The archived-media read plane over streams the CONTROL plane created.
//!
//! Every case drives one router carrying both emulated services over ONE shared
//! engine, so a stream created through `kinesisvideo` `CreateStream` is the very
//! record the archived-media reads resolve — through the same request walk
//! (route -> deserialize -> validate -> handle -> serialize) a socket-served
//! request takes.
//!
//! The rule under test: an archived-media read resolves the stream it addresses
//! in the Kinesis Video Streams registry and then applies its own per-operation
//! behaviour to that stream's archive — the retention gate re-read on every
//! call, the fragment index, the fragment-number check, the empty page — so
//! `ResourceNotFoundException` is only ever for an identifier the registry does
//! not hold.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use smithy_server_runtime::MultiServiceRouter;

use archived_media::state::{
    from_epoch_millis, service_state, ArchivedStream, StoredFragment, NO_DATA_RETENTION_MESSAGE,
    ONE_STREAM_IDENTIFIER_MESSAGE, STREAM_NOT_FOUND_MESSAGE,
};

/// The emulator endpoint every case addresses: this emulator, never a real AWS
/// host.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The base instant the archived fragments of this suite carry.
const T0_SECS: i64 = 1_761_000_000;

/// How far past [`T0_SECS`] the selectors in this suite reach.
const WINDOW_SECS: i64 = 1000;

/// A fragment number shaped like a real one: a long, increasing decimal, per
/// the modelled `FragmentNumberString` (`^[0-9]+$`).
const FRAGMENT_BASE: u64 = 9_134_385_233_318_143_200;

/// One router over one engine, carrying the archived-media reader AND the
/// Kinesis Video Streams control plane.
///
/// The two services share the signing name `kinesisvideo` and are told apart by
/// their first URI path segment, so one router serves both — and both reach the
/// same store, which is what makes a created stream readable here.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
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

    /// POST one restJson1 call and return its status, its error code and its
    /// parsed JSON body (`Null` for an empty or binary body).
    async fn call(&self, path: &str, body: serde_json::Value) -> (u16, String, serde_json::Value) {
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
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let code = parsed
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(header_code.as_str())
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        (status, code, parsed)
    }

    /// Create `name` through the control plane with `retention` hours, wait for
    /// it to be readable, and answer its `StreamARN`.
    async fn create_stream(&self, name: &str, retention: i32) -> String {
        let (status, _, body) = self
            .call(
                "/createStream",
                serde_json::json!({ "StreamName": name, "DataRetentionInHours": retention }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case create {name:?} retention {retention}: expected HTTP 200, got {status} with \
             {body:?}"
        );
        let arn = body
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case create {name:?}: no StreamARN in {body:?}"))
            .to_string();
        self.await_active(name).await;
        arn
    }

    /// Wait until the control plane reports `name` as `ACTIVE`, the one status an
    /// archived-media read serves, as a client of the real service does before
    /// reading a stream it just created ("CreateStream is an asynchronous
    /// operation").
    ///
    /// Waits for the CONDITION, not for a span: it looks again until the status
    /// holds, under a deadline far longer than the transition needs.
    async fn await_active(&self, name: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            last = self
                .describe_stream(name)
                .await
                .get("Status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if last == "ACTIVE" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("case create {name:?}: expected Status ACTIVE before the deadline, got {last:?}");
    }

    /// The stream's `StreamInfo`, as the control plane reports it.
    async fn describe_stream(&self, name: &str) -> serde_json::Value {
        let (status, _, body) = self
            .call("/describeStream", serde_json::json!({ "StreamName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case describe {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        body.get("StreamInfo")
            .cloned()
            .unwrap_or_else(|| panic!("case describe {name:?}: no StreamInfo in {body:?}"))
    }

    /// The retention `DescribeStream` reports for `name`.
    async fn reported_retention(&self, name: &str) -> i64 {
        let info = self.describe_stream(name).await;
        info.get("DataRetentionInHours")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case describe {name:?}: no DataRetentionInHours in {info:?}"))
    }

    /// Move `name`'s retention by `change` hours in the direction `operation`
    /// names, quoting the version the control plane currently reports.
    async fn update_retention(
        &self,
        name: &str,
        operation: &str,
        change: i32,
    ) -> (u16, serde_json::Value) {
        let info = self.describe_stream(name).await;
        let version = info
            .get("Version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case update {name:?}: no Version in {info:?}"))
            .to_string();
        let (status, _, body) = self
            .call(
                "/updateDataRetention",
                serde_json::json!({
                    "StreamName": name,
                    "CurrentVersion": version,
                    "Operation": operation,
                    "DataRetentionChangeInHours": change,
                }),
            )
            .await;
        (status, body)
    }

    /// Put `count` fragments into `stream`'s ARCHIVE, 10 seconds apart from
    /// [`T0_SECS`].
    ///
    /// There is no ingest OPERATION yet, so the archive is seeded through the
    /// reader's own state API — on the same engine the router serves from.
    /// `archived_retention` is what the ARCHIVE's own record carries, which the
    /// registry's record overrides wherever the two disagree.
    fn seed_archive(&self, stream: &str, archived_retention: u32, count: u64) {
        let state = service_state(&*self.engine).expect("the archived-media store");
        let mut state = state.lock();
        state.put_stream(ArchivedStream::new(
            stream,
            REGION,
            SystemTime::now(),
            archived_retention,
        ));
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

/// The `Message` of a failure body (`message` on the framework's own
/// validation envelope).
fn message(body: &serde_json::Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `base` with every member of `extra` added.
fn with_members(base: &serde_json::Value, extra: &serde_json::Value) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().expect("an object of members") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(object)
}

/// The timestamp range every selector in this suite asks for.
fn window() -> serde_json::Value {
    serde_json::json!({ "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + WINDOW_SECS })
}

/// The two operations the retention gate guards, with a body that reaches that
/// gate once the stream-naming member is added.
fn retention_gated_operations() -> Vec<(&'static str, serde_json::Value)> {
    vec![
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
    ]
}

/// Every archived-media read, with a body that succeeds for a retaining stream
/// holding the seeded archive once the stream-naming member is added.
fn every_read() -> Vec<(&'static str, serde_json::Value)> {
    let mut reads = vec![
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
    ];
    reads.extend(retention_gated_operations());
    reads
}

/// RULE: a stream the control plane holds is RESOLVED by every archived-media
/// read, whichever identifier form addresses it, so the per-operation behaviour
/// runs instead of a missing-resource refusal — and the retention the gate
/// reads is the REGISTRY's, not a value held beside the archive.
#[tokio::test]
async fn the_retention_gate_reads_the_registered_streams_retention() {
    let harness = Harness::new();

    // Rows differ along the retention the stream was CREATED with: 0 (retains
    // nothing) and three retaining values, the smallest included.
    for retention in [0i32, 1, 24, 8760] {
        let name = format!("probe-retention-{retention}");
        let arn = harness.create_stream(&name, retention).await;
        // The archive's own record deliberately disagrees with the registry:
        // the stream is the control plane's resource, so its record decides.
        harness.seed_archive(&name, if retention == 0 { 72 } else { 0 }, 0);

        for (path, extra) in retention_gated_operations() {
            for (form, addressing) in [
                (
                    "StreamName",
                    serde_json::json!({ "StreamName": name.clone() }),
                ),
                ("StreamARN", serde_json::json!({ "StreamARN": arn.clone() })),
            ] {
                let (status, code, body) = harness
                    .call(path, with_members(&addressing, &extra))
                    .await;
                let case = format!("{path} / {form} / retention {retention}");
                if retention == 0 {
                    assert_eq!(
                        status, 400,
                        "case {case}: expected HTTP 400, got {status} with {body:?}"
                    );
                    assert_eq!(
                        code, "NoDataRetentionException",
                        "case {case}: expected NoDataRetentionException, got {code:?}"
                    );
                    assert_eq!(
                        message(&body),
                        NO_DATA_RETENTION_MESSAGE,
                        "case {case}: expected {NO_DATA_RETENTION_MESSAGE:?}, got {:?}",
                        message(&body)
                    );
                } else {
                    // Past the gate: a retaining stream with an empty archive
                    // is refused for having no fragments to serve, not for
                    // being absent.
                    assert_eq!(
                        status, 404,
                        "case {case}: expected HTTP 404, got {status} with {body:?}"
                    );
                    assert_eq!(
                        code, "ResourceNotFoundException",
                        "case {case}: expected ResourceNotFoundException, got {code:?}"
                    );
                    let reported = message(&body);
                    assert!(
                        reported.starts_with("No fragments found in the stream"),
                        "case {case}: expected a no-fragments refusal, got {reported:?}"
                    );
                    assert_ne!(
                        reported, STREAM_NOT_FOUND_MESSAGE,
                        "case {case}: a registered stream must not be reported absent"
                    );
                }
            }
        }
    }
}

/// RULE: the retention gate is re-read from the stream's CURRENT state on every
/// call, so a retention the control plane moved is observed by the next read —
/// in both directions, and by every retention-gated operation.
#[tokio::test]
async fn a_moved_retention_is_observed_by_the_next_read() {
    let harness = Harness::new();
    let name = "probe-retention-transition";
    harness.create_stream(name, 0).await;

    // One step of the transition: the change applied before it, the retention
    // it leaves, and whether the retention gate refuses afterwards.
    struct Step {
        case: &'static str,
        change: Option<(&'static str, i32)>,
        retention: i64,
        gated: bool,
    }

    let steps = vec![
        Step {
            case: "as created",
            change: None,
            retention: 0,
            gated: true,
        },
        Step {
            case: "after an increase of 1",
            change: Some(("INCREASE_DATA_RETENTION", 1)),
            retention: 1,
            gated: false,
        },
        Step {
            case: "after a further increase of 23",
            change: Some(("INCREASE_DATA_RETENTION", 23)),
            retention: 24,
            gated: false,
        },
        Step {
            case: "after a decrease of 23",
            change: Some(("DECREASE_DATA_RETENTION", 23)),
            retention: 1,
            gated: false,
        },
        Step {
            case: "back at zero",
            change: Some(("DECREASE_DATA_RETENTION", 1)),
            retention: 0,
            gated: true,
        },
    ];

    for Step {
        case,
        change,
        retention,
        gated,
    } in steps
    {
        if let Some((operation, hours)) = change {
            let (status, body) = harness.update_retention(name, operation, hours).await;
            assert_eq!(
                status, 200,
                "case {case}: UpdateDataRetention expected HTTP 200, got {status} with {body:?}"
            );
        }
        let reported = harness.reported_retention(name).await;
        assert_eq!(
            reported, retention,
            "case {case}: expected DataRetentionInHours {retention}, got {reported}"
        );
        for (path, extra) in retention_gated_operations() {
            let (status, code, body) = harness
                .call(
                    path,
                    with_members(&serde_json::json!({ "StreamName": name }), &extra),
                )
                .await;
            let (expected_status, expected_code) = if gated {
                (400, "NoDataRetentionException")
            } else {
                (404, "ResourceNotFoundException")
            };
            assert_eq!(
                status, expected_status,
                "case {case} / {path}: expected HTTP {expected_status}, got {status} with {body:?}"
            );
            assert_eq!(
                code, expected_code,
                "case {case} / {path}: expected {expected_code}, got {code:?}"
            );
        }
    }
}

/// RULE: a registered stream with nothing archived in the selector's window is
/// a SUCCESS for `ListFragments` — HTTP 200 with `Fragments` present and empty
/// — and the page otherwise carries exactly what the archive holds in it.
#[tokio::test]
async fn list_fragments_answers_an_empty_page_for_a_registered_stream() {
    let harness = Harness::new();
    let selector = serde_json::json!({
        "FragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": window(),
        },
    });

    // Rows differ along how many fragments the archive holds, and along the
    // retention the stream carries — neither bears on this success.
    for (case, retention, archived) in [
        ("zero retention, empty archive", 0i32, 0u64),
        ("retaining, empty archive", 24, 0),
        ("retaining, one fragment", 24, 1),
        ("retaining, five fragments", 24, 5),
    ] {
        let name = format!("probe-list-{retention}-{archived}");
        let arn = harness.create_stream(&name, retention).await;
        harness.seed_archive(&name, 0, archived);
        for (form, addressing) in [
            (
                "StreamName",
                serde_json::json!({ "StreamName": name.clone() }),
            ),
            ("StreamARN", serde_json::json!({ "StreamARN": arn.clone() })),
        ] {
            let (status, _, body) = harness
                .call("/listFragments", with_members(&addressing, &selector))
                .await;
            assert_eq!(
                status, 200,
                "case {case} / {form}: expected HTTP 200, got {status} with {body:?}"
            );
            let fragments = body
                .get("Fragments")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case} / {form}: Fragments must be present, got {body:?}")
                });
            assert_eq!(
                fragments.len() as u64,
                archived,
                "case {case} / {form}: expected {archived} fragment(s), got {fragments:?}"
            );
        }
    }
}

/// RULE: `GetMediaForFragmentList` resolves the stream and then checks the
/// fragment numbers against that stream's archive, refusing
/// `InvalidArgumentException` (400) and LISTING every number that names no
/// fragment, in request order.
#[tokio::test]
async fn get_media_for_fragment_list_lists_the_fragment_numbers_it_cannot_find() {
    let harness = Harness::new();
    let name = "probe-fragment-numbers";
    harness.create_stream(name, 24).await;
    harness.seed_archive(name, 24, 3);

    // Rows differ along how many of the requested numbers are archived and
    // where the unknown ones sit in the request.
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "every number archived",
            vec![fragment_number(0), fragment_number(2)],
            Vec::new(),
        ),
        (
            "one unknown number",
            vec!["1".to_string()],
            vec!["1".to_string()],
        ),
        (
            "two unknown numbers",
            vec!["1".to_string(), "20".to_string()],
            vec!["1".to_string(), "20".to_string()],
        ),
        (
            "an unknown number between archived ones",
            vec![fragment_number(0), "300".to_string(), fragment_number(1)],
            vec!["300".to_string()],
        ),
        (
            "a number far outside the archive",
            vec!["99999999999999999999999999999999999999".to_string()],
            vec!["99999999999999999999999999999999999999".to_string()],
        ),
    ];

    for (case, requested, unknown) in cases {
        let (status, code, body) = harness
            .call(
                "/getMediaForFragmentList",
                serde_json::json!({ "StreamName": name, "Fragments": requested }),
            )
            .await;
        if unknown.is_empty() {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {code:?}"
        );
        let expected = format!("Fragment numbers are invalid : [{}]", unknown.join(", "));
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            message(&body)
        );
    }
}

/// RULE: `ResourceNotFoundException` is for an identifier the registry does not
/// hold, and a request that does not name exactly one stream is
/// `InvalidArgumentException` — one rule, applied by every archived-media read.
#[tokio::test]
async fn an_unregistered_identifier_is_the_only_missing_resource() {
    let harness = Harness::new();
    let name = "probe-addressing";
    let arn = harness.create_stream(name, 24).await;
    harness.seed_archive(name, 24, 2);

    // (the case, the addressing members, the expected status, error code and
    // message). An empty expectation means the call succeeds.
    let addressing: Vec<(&str, serde_json::Value, u16, &str, &str)> = vec![
        (
            "the registered name",
            serde_json::json!({ "StreamName": name }),
            200,
            "",
            "",
        ),
        (
            "the registered ARN",
            serde_json::json!({ "StreamARN": arn.clone() }),
            200,
            "",
            "",
        ),
        (
            "an unregistered name",
            serde_json::json!({ "StreamName": "probe-absent" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND_MESSAGE,
        ),
        (
            "an unregistered ARN",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000"
            }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND_MESSAGE,
        ),
        (
            "both identifiers",
            serde_json::json!({ "StreamName": name, "StreamARN": arn.clone() }),
            400,
            "InvalidArgumentException",
            ONE_STREAM_IDENTIFIER_MESSAGE,
        ),
        (
            "neither identifier",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
            ONE_STREAM_IDENTIFIER_MESSAGE,
        ),
    ];

    for (path, extra) in &every_read() {
        for (case, members, status, code, expected) in &addressing {
            let (got_status, got_code, body) =
                harness.call(path, with_members(members, extra)).await;
            assert_eq!(
                got_status, *status,
                "case {path} / {case}: expected HTTP {status}, got {got_status} with {body:?}"
            );
            if expected.is_empty() {
                continue;
            }
            assert_eq!(
                got_code, *code,
                "case {path} / {case}: expected {code}, got {got_code:?}"
            );
            assert_eq!(
                message(&body),
                *expected,
                "case {path} / {case}: expected {expected:?}, got {:?}",
                message(&body)
            );
        }
    }
}

/// RULE: `UpdateDataRetention` adds or subtracts the hours it is given from the
/// stream's current retention, refuses a result outside the published 0..87600
/// hours, and refuses a `CurrentVersion` that is not the stored one — leaving
/// the stored retention untouched in both refusals.
#[tokio::test]
async fn update_data_retention_moves_the_stored_retention_within_its_bounds() {
    let harness = Harness::new();

    // Rows differ along the direction, the magnitude, and whether the result is
    // inside the published range ("the minimum value for data retention is 0
    // and the maximum value is 87600 (ten years)").
    let cases: Vec<(&str, i32, &str, i32, u16, i64)> = vec![
        (
            "an increase from zero",
            0,
            "INCREASE_DATA_RETENTION",
            1,
            200,
            1,
        ),
        (
            "an increase to the maximum",
            87_599,
            "INCREASE_DATA_RETENTION",
            1,
            200,
            87_600,
        ),
        (
            "an increase past the maximum",
            87_600,
            "INCREASE_DATA_RETENTION",
            1,
            400,
            87_600,
        ),
        ("a decrease to zero", 5, "DECREASE_DATA_RETENTION", 5, 200, 0),
        (
            "a decrease below zero",
            5,
            "DECREASE_DATA_RETENTION",
            6,
            400,
            5,
        ),
        (
            "a decrease within range",
            48,
            "DECREASE_DATA_RETENTION",
            24,
            200,
            24,
        ),
    ];

    for (case, initial, operation, change, expected_status, expected_retention) in cases {
        let name = format!("probe-update-{initial}-{change}-{operation}");
        harness.create_stream(&name, initial).await;
        let (status, body) = harness.update_retention(&name, operation, change).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        let reported = harness.reported_retention(&name).await;
        assert_eq!(
            reported, expected_retention,
            "case {case}: expected DataRetentionInHours {expected_retention}, got {reported}"
        );
    }

    // A version that is not the stored one is refused, and changes nothing.
    let name = "probe-update-stale-version";
    harness.create_stream(name, 1).await;
    let (status, body) = harness
        .update_retention(name, "INCREASE_DATA_RETENTION", 1)
        .await;
    assert_eq!(
        status, 200,
        "case stale version setup: expected HTTP 200, got {status} with {body:?}"
    );
    let (status, code, body) = harness
        .call(
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": name,
                // A well-formed `Version` the record never carried: the update
                // above minted a fresh one.
                "CurrentVersion": "staleversion0",
                "Operation": "INCREASE_DATA_RETENTION",
                "DataRetentionChangeInHours": 1,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case stale version: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        code, "VersionMismatchException",
        "case stale version: expected VersionMismatchException, got {code:?}"
    );
    let reported = harness.reported_retention(name).await;
    assert_eq!(
        reported, 2,
        "case stale version: expected the retention to stay 2, got {reported}"
    );
}
