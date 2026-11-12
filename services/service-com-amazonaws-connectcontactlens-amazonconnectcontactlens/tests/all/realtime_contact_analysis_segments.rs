// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRealtimeContactAnalysisSegments`.
//!
//! Every case drives a real HTTP request through the `MultiServiceRouter` — the same
//! path a socket-served request takes — so what is asserted is the wire behaviour, not
//! an internal function call. Segments are put into state through the seeding API on
//! `crate::state` (Contact Lens has no public write API: Amazon Connect's voice
//! analysis is the producer), and then read back through the operation.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connectcontactlens_amazonconnectcontactlens as contact_lens;

use contact_lens::ops::types::{
    Categories, CategoryDetails, CharacterOffsets, ExtractedInformation, ExtractedInformationValue,
    IssueDetected, PointOfInterest, PostContactSummary, RealtimeContactAnalysisSegment, Transcript,
    EXTRACTED_INFORMATION_FAILURE_CODE_VALUES, POST_CONTACT_SUMMARY_FAILURE_CODE_VALUES,
    POST_CONTACT_SUMMARY_STATUS_VALUES, SENTIMENT_VALUE_VALUES,
};
use contact_lens::state::{
    service_state, AnalysisSession, SessionKey, MAX_PAGE_SIZE, VOICE_DATA_RETENTION,
};

/// The `(InstanceId, ContactId)` pair every seeded session is stored under.
///
/// `InstanceId` is a GUID because that is what an Amazon Connect instance is identified by:
/// an identifier of any other form resolves to no instance and is refused at the front door
/// (`src/service/instance_id.rs`, pinned in `tests/all/refusal_stages.rs`). `ContactId` is a
/// GUID too so the swapped-pair row below addresses a resolvable instance and still misses.
const INSTANCE: &str = "4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4";
const CONTACT: &str = "b5f87632-2a3b-4c5d-9e8f-0a1b2c3d4e5f";
/// A second GUID-shaped instance identifier, for the rows that miss on the instance half.
const OTHER_INSTANCE: &str = "7d9e1c3a-4b5c-4d6e-8f90-1a2b3c4d5e6f";
const URI: &str = "/realtime-contact-analysis/analysis-segments";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`connect`), which is the routing signal the router reads first. The signature
/// itself is never checked here: the test router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/connect/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// What one dispatched request answered: the status, the restJson1 error discriminator,
/// and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn segments(&self) -> Vec<Value> {
        self.body
            .get("Segments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The modeled `Message` member of an error body (the only member the error shapes
    /// in this service carry).
    fn message(&self) -> Option<String> {
        self.body
            .get("Message")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

fn build_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(contact_lens::EmulatedService::new(Arc::clone(engine)));
    router
}

/// The seeding path: register an analysis session on the shared engine the way the
/// producing side (Amazon Connect's voice analysis) would.
fn seed(engine: &Arc<engine::InMemoryEngine>, session: AnalysisSession) {
    service_state(&**engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .put_session(session);
}

fn complete(engine: &Arc<engine::InMemoryEngine>, key: &SessionKey) {
    let marked = service_state(&**engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .complete_session(key);
    assert!(marked, "complete_session must find the seeded session {key:?}");
}

async fn list(router: &MultiServiceRouter, body: &Value) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri(URI)
        .header("host", "contact-lens.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body: parsed,
    }
}

fn request(instance: &str, contact: &str, max_results: Option<i32>, token: Option<&str>) -> Value {
    let mut body = json!({ "InstanceId": instance, "ContactId": contact });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    body
}

/// Segment `index` of a synthetic transcript, with ascending offsets.
fn transcript_segment(index: usize) -> RealtimeContactAnalysisSegment {
    let begin = i32::try_from(index).expect("test index fits in i32") * 1_000;
    RealtimeContactAnalysisSegment {
        transcript: Some(Transcript {
            id: format!("t-{index}"),
            participant_id: "participant-1".to_string(),
            participant_role: if index.is_multiple_of(2) {
                "CUSTOMER"
            } else {
                "AGENT"
            }
            .to_string(),
            content: format!("utterance {index}"),
            begin_offset_millis: begin,
            end_offset_millis: begin + 900,
            sentiment: None,
            issues_detected: None,
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Pagination over the stored segments
// ---------------------------------------------------------------------------

/// Paging a session returns every stored segment exactly once, in stored order, with no
/// page larger than the effective limit — and the response omits `NextToken` exactly
/// when analysis has completed and every segment has been handed out.
///
/// The rows vary what the rule generalises over: how many segments are stored, the page
/// size (absent, each published bound, and sizes that divide the list evenly and
/// unevenly) and whether analysis is still running.
#[tokio::test]
async fn pagination_returns_every_stored_segment_exactly_once() {
    let rows: [(usize, Option<i32>, bool); 12] = [
        (0, None, true),
        (0, Some(1), false),
        (1, Some(1), true),
        (1, None, false),
        (3, Some(2), false),
        (3, Some(2), true),
        (5, Some(1), true),
        (7, Some(3), false),
        (10, None, true),
        (10, Some(MAX_PAGE_SIZE), true),
        (100, Some(MAX_PAGE_SIZE), true),
        (100, Some(7), true),
    ];

    for (count, max_results, completed) in rows {
        let case = format!("count={count} max_results={max_results:?} completed={completed}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..count).map(transcript_segment))
                .completed(completed),
        );

        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let page_limit = usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE))
            .expect("the page limit is positive");

        for page in 0..=count + 1 {
            let reply = list(
                &router,
                &request(INSTANCE, CONTACT, max_results, token.as_deref()),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {page} expected HTTP 200, got {} {:?}",
                reply.status, reply.body
            );
            let segments = reply.segments();
            assert!(
                segments.len() <= page_limit,
                "case {case}: page {page} expected at most {page_limit} segments, got {}",
                segments.len()
            );
            let drained = segments.is_empty();
            seen.extend(segments);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
            if drained {
                // A drained but still-running session keeps handing back a token so the
                // client can poll for segments that have not been produced yet.
                assert!(
                    !completed,
                    "case {case}: a completed, drained session must omit NextToken, got {token:?}"
                );
                break;
            }
        }

        assert_eq!(
            seen.len(),
            count,
            "case {case}: expected {count} segments across all pages, got {}",
            seen.len()
        );
        for (index, segment) in seen.iter().enumerate() {
            assert_eq!(
                segment["Transcript"]["Id"],
                json!(format!("t-{index}")),
                "case {case}: position {index} expected the stored segment t-{index}, got {segment:?}"
            );
        }
        assert_eq!(
            token.is_none(),
            completed,
            "case {case}: expected NextToken absent={completed}, got token {token:?}"
        );
    }
}

/// A `NextToken` resumes EXACTLY after the last returned segment: the page a token
/// yields is the stored slice starting at that boundary, whatever the boundary is.
#[tokio::test]
async fn a_next_token_resumes_exactly_after_the_last_returned_segment() {
    const COUNT: usize = 9;
    for first_page in 1..=COUNT {
        let case = format!("first_page={first_page}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..COUNT).map(transcript_segment))
                .completed(true),
        );

        let first = list(
            &router,
            &request(
                INSTANCE,
                CONTACT,
                Some(i32::try_from(first_page).expect("page size fits in i32")),
                None,
            ),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected HTTP 200, got {} {:?}",
            first.status, first.body
        );
        let token = first.next_token();
        assert_eq!(
            token.is_some(),
            first_page < COUNT,
            "case {case}: expected a NextToken only when segments remain, got {token:?}"
        );
        let Some(token) = token else { continue };

        let second = list(
            &router,
            &request(INSTANCE, CONTACT, Some(MAX_PAGE_SIZE), Some(&token)),
        )
        .await;
        assert_eq!(
            second.status, 200,
            "case {case}: resumed page expected HTTP 200, got {} {:?}",
            second.status, second.body
        );
        let resumed: Vec<String> = second
            .segments()
            .iter()
            .map(|segment| segment["Transcript"]["Id"].to_string())
            .collect();
        let expected: Vec<String> = (first_page..COUNT)
            .map(|index| json!(format!("t-{index}")).to_string())
            .collect();
        assert_eq!(
            resumed, expected,
            "case {case}: expected the resumed page to be {expected:?}, got {resumed:?}"
        );
    }
}

/// Listing is a read: repeating one request answers byte-identically, including the
/// opaque `NextToken`, so no caller's later read is changed by an earlier one.
#[tokio::test]
async fn repeating_a_request_answers_identically() {
    let rows: [(usize, Option<i32>, bool); 4] = [
        (0, None, false),
        (4, Some(2), false),
        (4, Some(2), true),
        (4, None, true),
    ];
    for (count, max_results, completed) in rows {
        let case = format!("count={count} max_results={max_results:?} completed={completed}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..count).map(transcript_segment))
                .completed(completed),
        );

        let body = request(INSTANCE, CONTACT, max_results, None);
        let first = list(&router, &body).await;
        let second = list(&router, &body).await;
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {case}: expected the second identical read to answer {:?}, got {:?}",
            first.body,
            second.body
        );
    }
}

// ---------------------------------------------------------------------------
// Segment fidelity
// ---------------------------------------------------------------------------

/// Every stored segment comes back as the value that was stored, for each shape the
/// model lets a `RealtimeContactAnalysisSegment` carry.
///
/// The enum catalogs iterated here are the model's own, re-exported by the service
/// crate from the Smithy model (`SentimentValue`, `PostContactSummaryStatus`,
/// `PostContactSummaryFailureCode`, `ExtractedInformationFailureCode`), so values the
/// issue never cited — `INVALID_ANALYSIS_CONFIGURATION`, `MAX_PACKAGE_FEATURE_ONLY` —
/// are covered too.
#[tokio::test]
async fn stored_segments_round_trip_in_their_modeled_shape() {
    let mut cases: Vec<(String, RealtimeContactAnalysisSegment, Value)> = Vec::new();

    cases.push((
        "transcript without sentiment".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-plain".to_string(),
                participant_id: "p-1".to_string(),
                participant_role: "CUSTOMER".to_string(),
                content: "hello there".to_string(),
                begin_offset_millis: 0,
                end_offset_millis: 1_200,
                sentiment: None,
                issues_detected: None,
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-plain",
                "ParticipantId": "p-1",
                "ParticipantRole": "CUSTOMER",
                "Content": "hello there",
                "BeginOffsetMillis": 0,
                "EndOffsetMillis": 1_200,
            }
        }),
    ));

    for sentiment in SENTIMENT_VALUE_VALUES {
        cases.push((
            format!("transcript with sentiment {sentiment}"),
            RealtimeContactAnalysisSegment {
                transcript: Some(Transcript {
                    id: format!("seg-{sentiment}"),
                    participant_id: "p-2".to_string(),
                    participant_role: "AGENT".to_string(),
                    content: format!("feeling {sentiment}"),
                    begin_offset_millis: 2_000,
                    end_offset_millis: 2_500,
                    sentiment: Some((*sentiment).to_string()),
                    issues_detected: None,
                }),
                ..Default::default()
            },
            json!({
                "Transcript": {
                    "Id": format!("seg-{sentiment}"),
                    "ParticipantId": "p-2",
                    "ParticipantRole": "AGENT",
                    "Content": format!("feeling {sentiment}"),
                    "BeginOffsetMillis": 2_000,
                    "EndOffsetMillis": 2_500,
                    "Sentiment": sentiment,
                }
            }),
        ));
    }

    cases.push((
        "transcript with issues detected".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-issues".to_string(),
                participant_id: "p-3".to_string(),
                participant_role: "CUSTOMER".to_string(),
                content: "this is broken and I want a refund".to_string(),
                begin_offset_millis: 3_000,
                end_offset_millis: 4_000,
                sentiment: Some("NEGATIVE".to_string()),
                issues_detected: Some(vec![
                    IssueDetected {
                        character_offsets: CharacterOffsets {
                            begin_offset_char: 0,
                            end_offset_char: 14,
                        },
                    },
                    IssueDetected {
                        character_offsets: CharacterOffsets {
                            begin_offset_char: 19,
                            end_offset_char: 33,
                        },
                    },
                ]),
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-issues",
                "ParticipantId": "p-3",
                "ParticipantRole": "CUSTOMER",
                "Content": "this is broken and I want a refund",
                "BeginOffsetMillis": 3_000,
                "EndOffsetMillis": 4_000,
                "Sentiment": "NEGATIVE",
                "IssuesDetected": [
                    { "CharacterOffsets": { "BeginOffsetChar": 0, "EndOffsetChar": 14 } },
                    { "CharacterOffsets": { "BeginOffsetChar": 19, "EndOffsetChar": 33 } },
                ],
            }
        }),
    ));

    cases.push((
        "categories with matched details".to_string(),
        RealtimeContactAnalysisSegment {
            categories: Some(Categories {
                matched_categories: vec!["Cancellation".to_string(), "Escalation".to_string()],
                matched_details: [
                    (
                        "Cancellation".to_string(),
                        CategoryDetails {
                            points_of_interest: vec![PointOfInterest {
                                begin_offset_millis: 1_000,
                                end_offset_millis: 2_000,
                            }],
                        },
                    ),
                    (
                        "Escalation".to_string(),
                        CategoryDetails {
                            points_of_interest: vec![
                                PointOfInterest {
                                    begin_offset_millis: 5_000,
                                    end_offset_millis: 6_000,
                                },
                                PointOfInterest {
                                    begin_offset_millis: 7_000,
                                    end_offset_millis: 7_500,
                                },
                            ],
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            }),
            ..Default::default()
        },
        json!({
            "Categories": {
                "MatchedCategories": ["Cancellation", "Escalation"],
                "MatchedDetails": {
                    "Cancellation": {
                        "PointsOfInterest": [{ "BeginOffsetMillis": 1_000, "EndOffsetMillis": 2_000 }]
                    },
                    "Escalation": {
                        "PointsOfInterest": [
                            { "BeginOffsetMillis": 5_000, "EndOffsetMillis": 6_000 },
                            { "BeginOffsetMillis": 7_000, "EndOffsetMillis": 7_500 },
                        ]
                    },
                },
            }
        }),
    ));

    cases.push((
        "categories with no match".to_string(),
        RealtimeContactAnalysisSegment {
            categories: Some(Categories {
                matched_categories: Vec::new(),
                matched_details: std::collections::HashMap::new(),
            }),
            ..Default::default()
        },
        json!({ "Categories": { "MatchedCategories": [], "MatchedDetails": {} } }),
    ));

    for status in POST_CONTACT_SUMMARY_STATUS_VALUES {
        // A COMPLETED summary carries its content and no failure code; a FAILED one is
        // enumerated against every failure code the model declares.
        if *status == "COMPLETED" {
            cases.push((
                "post contact summary COMPLETED".to_string(),
                RealtimeContactAnalysisSegment {
                    post_contact_summary: Some(PostContactSummary {
                        content: Some("The customer asked to cancel.".to_string()),
                        status: (*status).to_string(),
                        failure_code: None,
                    }),
                    ..Default::default()
                },
                json!({
                    "PostContactSummary": {
                        "Content": "The customer asked to cancel.",
                        "Status": status,
                    }
                }),
            ));
            continue;
        }
        for failure_code in POST_CONTACT_SUMMARY_FAILURE_CODE_VALUES {
            cases.push((
                format!("post contact summary {status}/{failure_code}"),
                RealtimeContactAnalysisSegment {
                    post_contact_summary: Some(PostContactSummary {
                        content: None,
                        status: (*status).to_string(),
                        failure_code: Some((*failure_code).to_string()),
                    }),
                    ..Default::default()
                },
                json!({
                    "PostContactSummary": { "Status": status, "FailureCode": failure_code }
                }),
            ));
        }
    }

    for failure_code in EXTRACTED_INFORMATION_FAILURE_CODE_VALUES {
        cases.push((
            format!("extracted information {failure_code}"),
            RealtimeContactAnalysisSegment {
                extracted_information: Some(ExtractedInformation {
                    extraction_definition_id: format!("def-{failure_code}"),
                    extraction_definition_name: "OrderNumber".to_string(),
                    extraction_definition_display_label: Some("Order no.".to_string()),
                    extracted_values: Some(vec![ExtractedInformationValue {
                        content: "A-1234".to_string(),
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 8_000,
                            end_offset_millis: 8_400,
                        }],
                    }]),
                    failure_code: Some((*failure_code).to_string()),
                }),
                ..Default::default()
            },
            json!({
                "ExtractedInformation": {
                    "ExtractionDefinitionId": format!("def-{failure_code}"),
                    "ExtractionDefinitionName": "OrderNumber",
                    "ExtractionDefinitionDisplayLabel": "Order no.",
                    "ExtractedValues": [{
                        "Content": "A-1234",
                        "PointsOfInterest": [{ "BeginOffsetMillis": 8_000, "EndOffsetMillis": 8_400 }],
                    }],
                    "FailureCode": failure_code,
                }
            }),
        ));
    }

    cases.push((
        "transcript and categories in one segment".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-both".to_string(),
                participant_id: "p-4".to_string(),
                participant_role: "AGENT".to_string(),
                content: "let me escalate that".to_string(),
                begin_offset_millis: 9_000,
                end_offset_millis: 9_900,
                sentiment: Some("NEUTRAL".to_string()),
                issues_detected: None,
            }),
            categories: Some(Categories {
                matched_categories: vec!["Escalation".to_string()],
                matched_details: [(
                    "Escalation".to_string(),
                    CategoryDetails {
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 9_000,
                            end_offset_millis: 9_900,
                        }],
                    },
                )]
                .into_iter()
                .collect(),
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-both",
                "ParticipantId": "p-4",
                "ParticipantRole": "AGENT",
                "Content": "let me escalate that",
                "BeginOffsetMillis": 9_000,
                "EndOffsetMillis": 9_900,
                "Sentiment": "NEUTRAL",
            },
            "Categories": {
                "MatchedCategories": ["Escalation"],
                "MatchedDetails": {
                    "Escalation": {
                        "PointsOfInterest": [{ "BeginOffsetMillis": 9_000, "EndOffsetMillis": 9_900 }]
                    }
                },
            },
        }),
    ));

    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments(cases.iter().map(|(_, segment, _)| segment.clone()))
            .completed(true),
    );

    let reply = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        reply.status, 200,
        "expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    let returned = reply.segments();
    assert_eq!(
        returned.len(),
        cases.len(),
        "expected {} segments, got {}",
        cases.len(),
        returned.len()
    );
    for (index, (case, _, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &returned[index], expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            returned[index]
        );
    }
}

// ---------------------------------------------------------------------------
// Unknown session, bad token, refused page size, retention
// ---------------------------------------------------------------------------

/// No stored session for the `(InstanceId, ContactId)` pair is `ResourceNotFoundException`
/// (HTTP 404) — never an empty HTTP 200. Both halves of the key are load-bearing, so a
/// miss on either (or a swapped pair) is a miss.
#[tokio::test]
async fn an_unknown_session_is_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..2).map(transcript_segment))
            .completed(true),
    );

    let misses = [
        ("unknown contact", INSTANCE, "contact-does-not-exist"),
        ("unknown instance", OTHER_INSTANCE, CONTACT),
        ("both unknown", OTHER_INSTANCE, "contact-9"),
        ("swapped pair", CONTACT, INSTANCE),
    ];
    for (case, instance, contact) in misses {
        let reply = list(&router, &request(instance, contact, None, None)).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.body.get("__type").and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the body's __type to name the error, got {:?}",
            reply.body
        );
    }

    // The control row: the seeded pair itself is served.
    let hit = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        hit.status, 200,
        "case \"seeded pair\": expected HTTP 200, got {} {:?}",
        hit.status, hit.body
    );
}

/// The message Contact Lens returns with `ResourceNotFoundException`. Fixed text: it is
/// pinned here as a literal, not read from the crate, so a handler that starts
/// interpolating the request into it fails this test.
const NOT_FOUND_MESSAGE: &str = "Real-time contact analysis not found.";

/// The not-found refusal carries the service's FIXED message: the same sentence for every
/// missing session, with nothing from the request (or from stored state) interpolated into
/// it, and no member besides `Message` in the body.
///
/// The rows vary what the rule generalises over — which half of the key misses, the form
/// and length of the `ContactId` (len 1..256, pattern `\S`, per
/// `smithy_query(query="operation",operation="ListRealtimeContactAnalysisSegments")`), and
/// whether the session never existed at all or has aged out of the retention window. Every
/// row names a resolvable instance, because an `InstanceId` that resolves to none never
/// reaches this stage (`tests/all/refusal_stages.rs` pins that one).
#[tokio::test]
async fn the_not_found_message_is_the_services_fixed_text() {
    let longest = "l".repeat(256);
    let rows: [(&str, &str, &str, bool); 9] = [
        ("unknown contact", INSTANCE, "contact-does-not-exist", false),
        ("unknown instance", OTHER_INSTANCE, CONTACT, false),
        ("both unknown", OTHER_INSTANCE, "contact-9", false),
        ("swapped pair", CONTACT, INSTANCE, false),
        (
            "an instance arn naming the instance",
            "arn:aws:connect:us-east-1:000000000000:instance/7d9e1c3a-4b5c-4d6e-8f90-1a2b3c4d5e6f",
            CONTACT,
            false,
        ),
        ("single-character contact id", INSTANCE, "c", false),
        ("maximum-length contact id", INSTANCE, &longest, false),
        ("punctuated contact id", INSTANCE, "contact%9#d", false),
        ("session aged out of retention", INSTANCE, CONTACT, true),
    ];

    for (case, instance, contact, aged_out) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        // The control session always exists; for the aged-out row it is the requested
        // pair itself, placed past the retention window.
        let started_at = if aged_out {
            SystemTime::now()
                .checked_sub(VOICE_DATA_RETENTION + Duration::from_secs(60))
                .expect("a representable start time")
        } else {
            SystemTime::now()
        };
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..2).map(transcript_segment))
                .completed(true)
                .started_at(started_at),
        );

        let reply = list(&router, &request(instance, contact, None, None)).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?}: expected the fixed message {NOT_FOUND_MESSAGE:?}, got {:?}",
            reply.message()
        );
        assert!(
            reply.body.get("Segments").is_none() && reply.body.get("NextToken").is_none(),
            "case {case:?}: the error body must carry no output member, got {:?}",
            reply.body
        );
    }

    // The control row: a session inside the window is served, so the refusal above is
    // the missing session and not a blanket 404.
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..2).map(transcript_segment))
            .completed(true),
    );
    let hit = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        hit.status, 200,
        "case \"seeded pair\": expected HTTP 200, got {} {:?}",
        hit.status, hit.body
    );
    assert_eq!(
        hit.message(),
        None,
        "case \"seeded pair\": a served page carries no Message, got {:?}",
        hit.body
    );
}

/// Polling an unknown session repeatedly answers the SAME refusal every time: the message
/// is state-independent, so it cannot drift between calls.
#[tokio::test]
async fn repeated_polls_of_an_unknown_session_answer_identically() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..2).map(transcript_segment)),
    );

    for poll in 0..4 {
        let reply = list(&router, &request(INSTANCE, "never-a-contact", None, None)).await;
        assert_eq!(
            (reply.status, reply.message().as_deref()),
            (404, Some(NOT_FOUND_MESSAGE)),
            "case \"poll {poll}\": expected 404 with the fixed message, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// A `NextToken` the service did not mint for THIS contact is `InvalidRequestException`
/// (HTTP 400): the token is resolved against stored cursors, never parsed.
#[tokio::test]
async fn a_next_token_with_no_live_cursor_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let other: SessionKey = (INSTANCE.to_string(), "contact-2".to_string());
    for contact in [CONTACT, "contact-2"] {
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, contact).with_segments((0..3).map(transcript_segment)),
        );
    }

    // A token minted for the OTHER contact, and one for a session that was replaced.
    let foreign = list(&router, &request(INSTANCE, &other.1, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    let replaced = list(&router, &request(INSTANCE, CONTACT, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..3).map(transcript_segment)),
    );

    let long_token = "x".repeat(131_070);
    let rejected: [(&str, &str); 5] = [
        ("unstructured garbage", "garbage"),
        ("never minted", "ffffffffffffffffffffffffffffffff"),
        ("minted for another contact", &foreign),
        ("cursor dropped with its session", &replaced),
        ("maximum-length token", &long_token),
    ];
    for (case, token) in rejected {
        let reply = list(&router, &request(INSTANCE, CONTACT, None, Some(token))).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
    }

    // The control row: a token this contact's own cursor backs is accepted.
    let fresh = list(&router, &request(INSTANCE, CONTACT, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    let reply = list(&router, &request(INSTANCE, CONTACT, Some(1), Some(&fresh))).await;
    assert_eq!(
        reply.status, 200,
        "case \"live cursor\": expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.segments()
            .first()
            .map(|segment| segment["Transcript"]["Id"].clone()),
        Some(json!("t-1")),
        "case \"live cursor\": expected the page to resume at t-1, got {:?}",
        reply.segments()
    );
}

/// `MaxResults` outside the modeled 1..100 range is refused with HTTP 400 before the
/// handler runs; each published bound is served.
#[tokio::test]
async fn max_results_outside_the_modeled_range_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..3).map(transcript_segment))
            .completed(true),
    );

    // The published bounds (1 and 100), and one value past each, plus the extremes.
    let rows: [(i32, u16); 6] = [
        (1, 200),
        (MAX_PAGE_SIZE, 200),
        (0, 400),
        (MAX_PAGE_SIZE + 1, 400),
        (-1, 400),
        (i32::MAX, 400),
    ];
    for (max_results, expected) in rows {
        let reply = list(&router, &request(INSTANCE, CONTACT, Some(max_results), None)).await;
        assert_eq!(
            reply.status, expected,
            "case MaxResults={max_results}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// Voice data is retained for 24 hours (the modeled operation doc), so a session whose
/// analysis started at or before that bound behaves as absent.
#[tokio::test]
async fn a_session_past_the_retention_window_behaves_as_absent() {
    let rows: [(&str, Duration, bool, u16); 5] = [
        ("just started", Duration::ZERO, false, 200),
        (
            "one minute inside the window",
            VOICE_DATA_RETENTION - Duration::from_secs(60),
            false,
            200,
        ),
        ("exactly at the window", VOICE_DATA_RETENTION, false, 404),
        (
            "one minute past the window",
            VOICE_DATA_RETENTION + Duration::from_secs(60),
            false,
            404,
        ),
        ("started in the future", Duration::from_secs(600), true, 200),
    ];

    for (case, offset, in_future, expected) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        let now = SystemTime::now();
        let started_at = if in_future {
            now.checked_add(offset).expect("a representable start time")
        } else {
            now.checked_sub(offset).expect("a representable start time")
        };
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..2).map(transcript_segment))
                .completed(true)
                .started_at(started_at),
        );

        let reply = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// Segments produced after a page was read show up on the next call, and completing the
/// session is what finally drops `NextToken` — the round trip through stored state.
#[tokio::test]
async fn segments_appended_after_a_read_appear_on_the_next_call() {
    let engine = build_engine();
    let router = build_router(&engine);
    let key: SessionKey = (INSTANCE.to_string(), CONTACT.to_string());
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..2).map(transcript_segment)),
    );

    let first = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        first.segments().len(),
        2,
        "case \"before append\": expected 2 segments, got {:?}",
        first.segments()
    );
    let token = first
        .next_token()
        .expect("case \"before append\": an open session must hand back a NextToken");

    let appended = service_state(&*engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .append_segments(&key, (2..5).map(transcript_segment));
    assert!(appended, "append_segments must find the seeded session");

    let second = list(&router, &request(INSTANCE, CONTACT, None, Some(&token))).await;
    let resumed: Vec<Value> = second
        .segments()
        .iter()
        .map(|segment| segment["Transcript"]["Id"].clone())
        .collect();
    assert_eq!(
        resumed,
        vec![json!("t-2"), json!("t-3"), json!("t-4")],
        "case \"after append\": expected the newly produced segments, got {resumed:?}"
    );
    assert!(
        second.next_token().is_some(),
        "case \"after append\": a running session must still hand back a NextToken, got {:?}",
        second.body
    );

    complete(&engine, &key);
    let third = list(
        &router,
        &request(
            INSTANCE,
            CONTACT,
            None,
            second.next_token().as_deref(),
        ),
    )
    .await;
    assert!(
        third.segments().is_empty(),
        "case \"after completion\": expected no further segments, got {:?}",
        third.segments()
    );
    assert_eq!(
        third.next_token(),
        None,
        "case \"after completion\": a completed, drained session must omit NextToken, got {:?}",
        third.body
    );
}
