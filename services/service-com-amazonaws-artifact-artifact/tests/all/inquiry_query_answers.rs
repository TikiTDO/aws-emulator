// SPDX-License-Identifier: CC0-1.0

//! The ANSWER one compliance-inquiry query carries, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the answer plane states:
//!
//! * a query is answered out of the published report catalog this service serves: the
//!   `response` a query reports is the passages of the catalog documents whose text carries
//!   the query's terms, each under the source it was read out of, and the `citations`
//!   beside it name those sources, quote those same passages and link to where this
//!   emulator publishes them;
//! * a query the catalog holds nothing bearing on — and one naming no term at all —
//!   reports NEITHER member: there is no passage to quote, so none is invented;
//! * the answer is released by the CLOCK and by no read: a query whose processing is still
//!   in progress reports neither member, every listing after the due instant reports the
//!   same answer, and reading it twice changes nothing;
//! * a citation's link resolves back to this emulator — it LOCATES the cited report's own
//!   read, at the version the passage was quoted from — while the document behind that
//!   report stays reachable only the way every other caller reaches it, by accepting the
//!   term governing it.
//!
//! The rows differ along what those rules generalise over: the TERMS the query names (a
//! series, a report name, prose from a published description, one term, several, none at
//! all, and terms no catalog document carries) and the FORM of the submission (a text
//! query, and an uploaded document whose section labels carry the terms).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint the test addresses the emulator on, which every URL this service announces
/// has to name for the announced URL to resolve.
const HOST: &str = "artifact.eu-west-1.amazonaws.com";

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The span after which an inquiry's processing is due to be over, plus slack. The handler
/// stores the due instant rather than sleeping, so only the test waits.
///
/// Must stay above `crate::state::INQUIRY_PROCESSING_SPAN` (500 ms at the time of
/// writing): if that span is ever raised, raise this with it.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio without
/// its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// The most sources one answer cites — the bound the answer plane states
/// (`crate::state::ANSWER_CITATION_CAPACITY`), spelled out here so a raised bound fails
/// this suite instead of redefining what it asserts.
const CITATION_CAPACITY: usize = 3;

/// `LongStringAttribute`'s published maximum (`@length(min: 1, max: 2048)`), which every
/// member of the answer targets.
const LONG_STRING_MAX: usize = 2048;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Bytes,
    signed: bool,
) -> Answer {
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .header("content-type", "application/json");
    if signed {
        request = request.header("authorization", SIGNED);
    }
    let request = request
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer { status, body }
}

/// Create one inquiry and return the `InquiryId` it minted.
async fn create(router: &MultiServiceRouter, body: &Value) -> String {
    let answer = send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
        true,
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "the create must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create must report an inquiry id, got {:?}", answer.body))
        .to_string()
}

/// The queries one inquiry reports.
async fn list_queries(router: &MultiServiceRouter, inquiry_id: &str) -> Vec<Value> {
    let answer = send(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}"),
        Bytes::new(),
        true,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing an inquiry's queries must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["queries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The catalog as the API reports it, so what this suite checks a citation against is
/// whatever this emulator publishes rather than a list written out here.
async fn catalog(router: &MultiServiceRouter) -> Vec<Value> {
    let answer = send(router, "GET", "/v1/report/list", Bytes::new(), true).await;
    let reports = answer.body["reports"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !reports.is_empty(),
        "the catalog must publish reports for a query to be answered out of, got {:?}",
        answer.body
    );
    reports
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query } })
}

/// A `CreateComplianceInquiry` body for an uploaded document processed section by section.
fn file_body(name: &str, sections: &[&str]) -> Value {
    json!({
        "name": name,
        "inquiryContent": { "fileContent": { "content": "cXVlc3Rpb25uYWlyZQ==", "fileSections": sections } },
    })
}

/// One submission, and what the catalog must have been found to hold for each of the
/// queries it submits.
struct Case {
    label: &'static str,
    body: Value,
    /// Per query, in `queryIdentifier` order: the text a cited source must carry when the
    /// catalog holds one bearing on that query, and `None` when it holds none at all.
    ///
    /// The expected text is a term of the query the catalog really publishes — the series,
    /// name or published description of a report — so the row says WHICH source answers it
    /// without restating the ranking.
    expected: Vec<Option<&'static str>>,
}

/// The submissions this suite covers.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a query naming a report series and its standard",
            body: text_body("Certification", "Which ISO 27001 certification applies?"),
            expected: vec![Some("ISO 27001")],
        },
        Case {
            label: "a query naming prose from a published description",
            body: text_body(
                "Card data",
                "Who assessed the cardholder data environment?",
            ),
            expected: vec![Some("cardholder")],
        },
        Case {
            label: "a query naming one term",
            body: text_body("Outposts", "Outposts"),
            expected: vec![Some("Outposts")],
        },
        Case {
            label: "a query naming terms no catalog document carries",
            body: text_body(
                "Unknown",
                "zzqqxx wibblewobble nonexistentcomplianceterm",
            ),
            expected: vec![None],
        },
        Case {
            label: "a query naming no term at all",
            body: text_body("Punctuation", "?!"),
            expected: vec![None],
        },
        Case {
            label: "an uploaded document whose sections name a series and nothing",
            body: file_body(
                "Vendor questionnaire",
                &["SOC 2 Type 2 controls", "zzqqxx wibblewobble"],
            ),
            expected: vec![Some("SOC"), None],
        },
    ]
}

/// The report the catalog publishes under `name`, as the listing reports it.
fn published<'reports>(reports: &'reports [Value], name: &str) -> &'reports Value {
    reports
        .iter()
        .find(|report| report["name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!("a cited source must be a report the catalog publishes, got {name:?}")
        })
}

/// The path and query of a URL this service announced, as a caller fetches it.
fn fetchable(url: &str) -> String {
    let after_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("an announced URL must carry a scheme, got {url:?}"));
    let (host, path) = after_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("an announced URL must carry a path, got {url:?}"));
    assert_eq!(
        host, HOST,
        "a citation must link to this emulator so it resolves, got {url:?}"
    );
    format!("/{path}")
}

#[tokio::test]
async fn an_answer_quotes_the_catalog_sources_the_query_matched() {
    let router = emulator();
    let reports = catalog(&router).await;

    for case in cases() {
        let label = case.label;
        let id = create(&router, &case.body).await;
        // The answer is released at the inquiry's due instant, so the listing that reads it
        // is taken after that span.
        std::thread::sleep(PAST_THE_PROCESSING_SPAN);
        let queries = list_queries(&router, &id).await;
        assert_eq!(
            queries.len(),
            case.expected.len(),
            "case {label:?}: expected {} queries, got {queries:?}",
            case.expected.len()
        );

        for (position, expected) in case.expected.iter().enumerate() {
            let query = &queries[position];
            let response = query["response"].as_str();
            let citations = query["citations"].as_array();

            let Some(expected_source_text) = expected else {
                assert!(
                    response.is_none() && citations.is_none(),
                    "case {label:?}: query {position} has no source in the catalog, so it \
                     reports neither a response nor a citation, got {query:?}"
                );
                continue;
            };

            let response = response.unwrap_or_else(|| {
                panic!(
                    "case {label:?}: query {position} must report the answer retrieved for \
                     it, got {query:?}"
                )
            });
            let citations = citations.cloned().unwrap_or_default();
            assert!(
                !citations.is_empty() && citations.len() <= CITATION_CAPACITY,
                "case {label:?}: query {position} must cite between one and \
                 {CITATION_CAPACITY} sources, got {citations:?}"
            );
            assert!(
                response.chars().count() <= LONG_STRING_MAX
                    && !response.contains(['<', '>'])
                    && !response.is_empty(),
                "case {label:?}: query {position} must report a response inside \
                 LongStringAttribute's bounds, got {response:?}"
            );

            // The response IS the cited passages, in citation order: each source's label
            // and the passage quoted under it are in the text, and the later citation's
            // passage comes after the earlier one's.
            let mut quoted_at = 0usize;
            let mut labels: Vec<&str> = Vec::new();
            for citation in &citations {
                let source_label = citation["sourceLabel"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a citation must label its source, got {citation:?}")
                });
                let source_content = citation["sourceContent"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a citation must quote its source, got {citation:?}")
                });
                let source_link = citation["sourceLink"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a citation must link to its source, got {citation:?}")
                });
                labels.push(source_label);

                // The source is a report this catalog really publishes, and the passage is
                // text that report really carries.
                let report = published(&reports, source_label);
                let description = report["description"].as_str().unwrap_or_default();
                let name = report["name"].as_str().unwrap_or_default();
                assert!(
                    description.contains(source_content) || name.contains(source_content),
                    "case {label:?}: a cited passage must be text the source publishes, \
                     got {source_content:?} for {source_label:?}"
                );
                let report_id = report["id"].as_str().unwrap_or_default();
                assert!(
                    source_link.contains(report_id),
                    "case {label:?}: a citation must link to the document it quoted, got \
                     {source_link:?} for {report_id:?}"
                );

                let at = response.find(source_content).unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: the response is the cited passages, so it must \
                         carry {source_content:?}, got {response:?}"
                    )
                });
                assert!(
                    at >= quoted_at,
                    "case {label:?}: the response states its sources in citation order, \
                     got {source_content:?} at {at} after {quoted_at}"
                );
                quoted_at = at;
                assert!(
                    response.contains(source_label),
                    "case {label:?}: a quoted passage is stated under the source it was \
                     read out of, got {response:?} without {source_label:?}"
                );
            }

            // The sources are the ones the query's own terms were found in, which is what
            // the row names.
            assert!(
                citations.iter().any(|citation| {
                    let label = citation["sourceLabel"].as_str().unwrap_or_default();
                    let content = citation["sourceContent"].as_str().unwrap_or_default();
                    label.contains(expected_source_text) || content.contains(expected_source_text)
                }),
                "case {label:?}: query {position} must be answered out of the source \
                 carrying {expected_source_text:?}, got {citations:?}"
            );
            labels.sort_unstable();
            let distinct = labels.len();
            labels.dedup();
            assert_eq!(
                labels.len(),
                distinct,
                "case {label:?}: each source is cited once, got {citations:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_answer_is_released_by_the_clock_and_then_never_changes() {
    let router = emulator();
    let id = create(
        &router,
        &text_body("Certification", "Which ISO 27001 certification applies?"),
    )
    .await;

    // Inside the processing span the query reports no answer: the response is released
    // when the processing that produced it is over, not when somebody looks. The handler
    // stores the due instant rather than sleeping, so the read that follows the create is
    // inside the span.
    let inside = list_queries(&router, &id).await;
    let query = &inside[0];
    assert_eq!(
        query["status"].as_str(),
        Some("PROCESSING"),
        "a query read inside the inquiry's processing span is still in progress, got {query:?}"
    );
    assert!(
        query["response"].is_null() && query["citations"].is_null(),
        "a query whose processing is still in progress reports no answer, got {query:?}"
    );

    std::thread::sleep(PAST_THE_PROCESSING_SPAN);
    let first = list_queries(&router, &id).await;
    assert!(
        first[0]["response"].as_str().is_some(),
        "past the processing span the query must report its answer, got {:?}",
        first[0]
    );

    // A read changes nothing a later caller sees.
    let second = list_queries(&router, &id).await;
    assert_eq!(
        (first[0]["response"].clone(), first[0]["citations"].clone()),
        (second[0]["response"].clone(), second[0]["citations"].clone()),
        "two listings must report the same answer, got {:?} then {:?}",
        first[0],
        second[0]
    );
}

#[tokio::test]
async fn a_citation_links_to_the_source_its_passage_was_read_from() {
    let router = emulator();
    let reports = catalog(&router).await;
    let id = create(
        &router,
        &text_body("Audit", "Which independent auditor assessed the AWS controls?"),
    )
    .await;
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);
    let queries = list_queries(&router, &id).await;
    let citations = queries[0]["citations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !citations.is_empty(),
        "the query must cite the sources it was answered out of, got {:?}",
        queries[0]
    );

    for citation in &citations {
        let source_label = citation["sourceLabel"].as_str().unwrap_or_default();
        let source_link = citation["sourceLink"].as_str().unwrap_or_default();
        let report = published(&reports, source_label);
        // A citation LOCATES its source: following the link reads the cited report itself,
        // at the version the passage was quoted from. The document behind that report is
        // reached the way every other caller reaches it — by accepting its term — so the
        // link stands in for no acceptance and leads nowhere this emulator refuses to
        // serve.
        let followed = send(&router, "GET", &fetchable(source_link), Bytes::new(), true).await;
        assert_eq!(
            followed.status, 200,
            "case {source_label:?}: a citation's link must resolve back to this emulator, \
             got {} with {:?}",
            followed.status, followed.body
        );
        assert_eq!(
            (
                followed.body["reportDetails"]["id"].clone(),
                followed.body["reportDetails"]["name"].clone(),
                followed.body["reportDetails"]["version"].clone(),
            ),
            (
                report["id"].clone(),
                report["name"].clone(),
                report["version"].clone(),
            ),
            "case {source_label:?}: the link must read the cited report at the version the \
             passage came from, got {:?}",
            followed.body
        );
    }
}
