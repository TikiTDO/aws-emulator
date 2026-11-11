// SPDX-License-Identifier: CC0-1.0

//! `ExportComplianceInquiry`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones this operation states:
//!
//! * an export REPORTS ON A PERSISTED INQUIRY: it answers a presigned URL that names the
//!   inquiry it was exported from, at the endpoint the caller reached this emulator on,
//!   beside the tags that one stored record carries — whatever form the submission took,
//!   whatever export options the request named, and whatever status the clock has brought
//!   the inquiry to;
//! * exporting CREATES a document, so every call answers a URL of its own: two exports of
//!   one inquiry are two documents;
//! * `queryIdentifiers` selects which of the inquiry's queries the export covers, so an
//!   identifier that names no query of that inquiry is refused with the modelled
//!   `ValidationException` naming the member, while the identifiers it does own are
//!   served;
//! * an inquiry id the store does not hold is refused the way every other inquiry
//!   operation refuses it — the front door's 401 `UnauthorizedException`, which does not
//!   confirm the resource's absence;
//! * an export changes nothing a later reader of the inquiry sees.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for unless a case names another.
const REGION: &str = "eu-west-1";

/// The endpoint host every request of this suite addresses the emulator on. A document
/// URL this service announces names the endpoint the caller reached, so this is the host
/// the announced export URL must carry.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// The path prefix the export documents are published under (`crate::state`'s
/// `EXPORT_DOCUMENT_PATH_PREFIX`), spelled out here so a change to it has to be made
/// deliberately in both places.
const EXPORT_DOCUMENT_PATH_PREFIX: &str = "/v1/compliance-inquiry/download";

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// A SigV4 credential scope for `region` naming this service — what routes the request to
/// it.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

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

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `documentPresignedUrl` an export answered with.
    fn url(&self) -> &str {
        self.body["documentPresignedUrl"].as_str().unwrap_or_default()
    }

    /// The `tags` member of this answer, as a map, or `None` when it carries none.
    fn tags(&self) -> Option<BTreeMap<String, String>> {
        let object = self.body["tags"].as_object()?;
        Some(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        )
    }
}

async fn send(router: &MultiServiceRouter, request: ::http::Request<Bytes>) -> Answer {
    let uri = request.uri().to_string();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `CreateComplianceInquiry`, signed for `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/v1/compliance-inquiry/create")
        .header("host", ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", signed_for(region))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, request).await
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    create_in(router, REGION, body).await
}

/// `ExportComplianceInquiry`, signed for `region`.
async fn export_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/v1/compliance-inquiry/export")
        .header("host", ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", signed_for(region))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, request).await
}

async fn export(router: &MultiServiceRouter, body: &Value) -> Answer {
    export_in(router, REGION, body).await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"
        ))
        .header("host", ENDPOINT)
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, request).await
}

/// What following a presigned URL answered: the status, the document bytes and the two
/// headers a client reads off a download.
struct Download {
    status: u16,
    content_type: Option<String>,
    content_length: Option<String>,
    body: Vec<u8>,
}

/// Follow a presigned URL exactly as a client does: `method` against the host and path
/// the URL names, and with NO credential of any kind — a presigned URL is followed
/// unsigned.
async fn follow(router: &MultiServiceRouter, method: &str, url: &str) -> Download {
    let (host, path) = split_url(url);
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let content_length = header("content-length");
    Download {
        status,
        content_type,
        content_length,
        body: response.into_body().to_vec(),
    }
}

/// The host and the path-with-query of an absolute URL.
fn split_url(url: &str) -> (&str, &str) {
    let rest = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"))
        .1;
    match rest.find('/') {
        Some(mark) => (&rest[..mark], &rest[mark..]),
        None => (rest, "/"),
    }
}

/// The id of the inquiry a create answered with.
fn created_id(answer: &Answer) -> String {
    answer.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries no inquiry id: {:?}", answer.body))
        .to_string()
}

/// The tags a case submitted, in the shape [`Answer::tags`] reports — `None` for an
/// untagged submission, because an untagged resource reports no `tags` member at all.
fn submitted_tags(tags: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
    if tags.is_empty() {
        return None;
    }
    Some(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

/// A `tags` object for a create body, or `Value::Null` for an untagged submission.
fn tags_object(tags: &[(&str, &str)]) -> Value {
    if tags.is_empty() {
        return Value::Null;
    }
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(name: &str, query: &str, tags: &[(&str, &str)]) -> Value {
    create_body(name, json!({ "query": query }), tags)
}

/// A `CreateComplianceInquiry` body for an uploaded document of `content` bytes.
fn file_body_of(name: &str, content: &str, sections: &[&str], tags: &[(&str, &str)]) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    create_body(name, json!({ "fileContent": file }), tags)
}

/// A `CreateComplianceInquiry` body for an uploaded document.
fn file_body(name: &str, sections: &[&str], tags: &[(&str, &str)]) -> Value {
    file_body_of(name, FILE_CONTENT, sections, tags)
}

fn create_body(name: &str, inquiry_content: Value, tags: &[(&str, &str)]) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    let tags = tags_object(tags);
    if !tags.is_null() {
        body["tags"] = tags;
    }
    body
}

/// An `ExportComplianceInquiry` body for `inquiry_id` under the named options. `None`
/// leaves the member out of the request, which is how a caller names no value for it.
fn export_body(
    inquiry_id: &str,
    query_identifiers: Option<&[i32]>,
    include_citations: Option<bool>,
) -> Value {
    let mut body = json!({ "complianceInquiryId": inquiry_id });
    if let Some(identifiers) = query_identifiers {
        body["queryIdentifiers"] = json!(identifiers);
    }
    if let Some(citations) = include_citations {
        body["includeCitations"] = json!(citations);
    }
    body
}

/// One submission plus one set of export options, and what the export must report.
struct Case {
    label: &'static str,
    /// The `CreateComplianceInquiry` body the inquiry is created from.
    body: Value,
    tags: &'static [(&'static str, &'static str)],
    /// The `queryIdentifiers` the export names, or `None` to name none. A TEXT inquiry
    /// owns exactly the one query its content carried (identifier 1); an upload owns one
    /// per unit of work it was submitted in — the sections it named, or the document as a
    /// whole — and an upload carrying no bytes owns none.
    query_identifiers: Option<&'static [i32]>,
    /// The `includeCitations` the export names, or `None` to name none.
    include_citations: Option<bool>,
    /// The `queryIdentifiers` the exported DOCUMENT must cover: the ones the request
    /// named, deduplicated, or every query the inquiry owns when it named none.
    covered: &'static [i32],
    /// The region the case signs both calls for.
    region: &'static str,
    /// Whether the case waits past the processing span before exporting, so the inquiry
    /// is terminal rather than `PROCESSING` — an export reports on the inquiry either
    /// way.
    after_terminal: bool,
}

/// The export cases this suite covers. The rows differ along everything the rule
/// generalises over: the form of the submission (a query versus an uploaded document),
/// how many tags the inquiry carries, whether the export names `queryIdentifiers` and
/// which, whether it names `includeCitations` and which value, the region the request is
/// signed for, and whether the inquiry has reached a terminal status yet.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a query, no options, after the processing span",
            body: text_body("Data residency", "Where is customer data stored?", &[]),
            tags: &[],
            query_identifiers: None,
            include_citations: None,
            covered: &[1],
            region: REGION,
            after_terminal: true,
        },
        Case {
            label: "a query, the one identifier it owns, citations on",
            body: text_body(
                "Encryption at rest",
                "Is customer data encrypted at rest?",
                &[("team", "compliance")],
            ),
            tags: &[("team", "compliance")],
            query_identifiers: Some(&[1]),
            include_citations: Some(true),
            covered: &[1],
            region: REGION,
            after_terminal: true,
        },
        Case {
            label: "a query, that identifier twice, citations off, another region",
            body: text_body(
                "Subprocessors",
                "Which subprocessors handle customer content?",
                &[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")],
            ),
            tags: &[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")],
            query_identifiers: Some(&[1, 1]),
            include_citations: Some(false),
            covered: &[1],
            region: "ap-southeast-3",
            after_terminal: true,
        },
        Case {
            label: "a query, exported inside the processing span",
            body: text_body("Backups", "How often are backups taken?", &[("env", "dev")]),
            tags: &[("env", "dev")],
            query_identifiers: None,
            include_citations: Some(true),
            covered: &[1],
            region: "us-east-1",
            after_terminal: false,
        },
        Case {
            label: "an uploaded document, no sections, no options",
            body: file_body("Vendor questionnaire", &[], &[]),
            tags: &[],
            query_identifiers: None,
            include_citations: None,
            covered: &[1],
            region: REGION,
            after_terminal: true,
        },
        Case {
            label: "an uploaded document, two sections, an empty identifier list",
            body: file_body(
                "Annual audit workbook",
                &["Section A", "Section B"],
                &[("env", "staging")],
            ),
            tags: &[("env", "staging")],
            query_identifiers: Some(&[]),
            include_citations: Some(false),
            // An empty list names no identifier, so the export covers every query the
            // inquiry owns — one per submitted section.
            covered: &[1, 2],
            region: "us-west-2",
            after_terminal: true,
        },
        Case {
            label: "an uploaded document, three sections, the second of them",
            body: file_body(
                "Customer DDQ",
                &["Sheet 1", "Sheet 2", "Sheet 3"],
                &[],
            ),
            tags: &[],
            query_identifiers: Some(&[2]),
            include_citations: Some(true),
            covered: &[2],
            region: REGION,
            after_terminal: true,
        },
        Case {
            label: "an upload carrying no bytes, so no query to cover",
            body: file_body_of("Empty workbook", "", &[], &[]),
            tags: &[],
            query_identifiers: None,
            include_citations: None,
            covered: &[],
            region: REGION,
            after_terminal: true,
        },
    ]
}

#[tokio::test]
async fn an_export_answers_a_url_naming_the_inquiry_it_was_taken_from() {
    for case in cases() {
        let label = case.label;
        // One emulator per case, so the processing span one case waits out cannot
        // decide what another observes.
        let router = emulator();
        let created = create_in(&router, case.region, &case.body).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: the create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let id = created_id(&created);

        if case.after_terminal {
            std::thread::sleep(PAST_THE_PROCESSING_SPAN);
        }

        let body = export_body(&id, case.query_identifiers, case.include_citations);
        let exported = export_in(&router, case.region, &body).await;
        assert_eq!(
            exported.status, 200,
            "case {label:?}: ExportComplianceInquiry must answer HTTP 200, got {} with {:?}",
            exported.status, exported.body
        );

        let url = exported.url().to_string();
        let expected_prefix =
            format!("https://{ENDPOINT}{EXPORT_DOCUMENT_PATH_PREFIX}/{id}/export/");
        assert!(
            url.starts_with(&expected_prefix),
            "case {label:?}: expected a URL under {expected_prefix:?}, got {url:?}"
        );
        assert_eq!(
            exported.tags(),
            submitted_tags(case.tags),
            "case {label:?}: the export must report the inquiry's own tags, got {:?}",
            exported.body["tags"]
        );

        // The tags come off the one stored record, so the export and a metadata read
        // cannot disagree about them.
        let read = get_metadata(&router, &id).await;
        assert_eq!(
            exported.tags(),
            read.tags(),
            "case {label:?}: the export's tags must be the stored inquiry's, got {:?} \
             against {:?}",
            exported.body["tags"],
            read.body["tags"]
        );

        // A presigned URL is a promise that it can be FOLLOWED: the document it leads to
        // is served, unsigned, and reports the inquiry it was exported from.
        let fetched = follow(&router, "GET", &url).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: the presigned URL must serve the document, got {} with {:?}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.content_type.as_deref(),
            Some("application/json"),
            "case {label:?}: expected the document's own media type, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.content_length.as_deref(),
            Some(fetched.body.len().to_string().as_str()),
            "case {label:?}: content-length must be the document's length, got {:?} for \
             {} bytes",
            fetched.content_length,
            fetched.body.len()
        );
        let document: Value = serde_json::from_slice(&fetched.body).unwrap_or_else(|error| {
            panic!(
                "case {label:?}: the document must be JSON ({error}): {:?}",
                String::from_utf8_lossy(&fetched.body)
            )
        });
        let detail = read.body["complianceInquiryDetail"].clone();
        for (in_document, in_detail) in [
            ("complianceInquiryId", "id"),
            ("arn", "arn"),
            ("name", "name"),
            ("status", "status"),
            ("statusMessage", "statusMessage"),
            ("supportMode", "supportMode"),
            ("inputSource", "inputSource"),
        ] {
            assert_eq!(
                document[in_document], detail[in_detail],
                "case {label:?}: the document's {in_document} must be the inquiry's \
                 {in_detail}, got {:?} against {:?}",
                document[in_document], detail[in_detail]
            );
        }
        // The document is read by a person rather than by the SDK, so its timestamps are
        // rendered as RFC 3339 date-times rather than as the epoch-seconds the restJson1
        // wire carries.
        for member in ["createdAt", "updatedAt"] {
            let rendered = document[member].as_str().unwrap_or_default();
            assert!(
                rendered.contains('T') && rendered.ends_with('Z'),
                "case {label:?}: expected {member} as an RFC 3339 date-time, got {:?}",
                document[member]
            );
        }
        assert_eq!(
            document["queryIdentifiers"],
            json!(case.covered),
            "case {label:?}: expected the document to cover {:?}, got {:?}",
            case.covered,
            document["queryIdentifiers"]
        );
        let queries = document["queries"].as_array().unwrap_or_else(|| {
            panic!(
                "case {label:?}: the document must carry a queries list, got {:?}",
                document["queries"]
            )
        });
        assert_eq!(
            queries.len(),
            case.covered.len(),
            "case {label:?}: expected {} covered queries, got {queries:?}",
            case.covered.len()
        );
        for query in queries {
            // "When true, include citations in the exported document": the member is
            // there when the request asked for it, and absent when it did not.
            assert_eq!(
                query.get("citations").is_some(),
                case.include_citations.unwrap_or(false),
                "case {label:?}: citations were asked for: {:?}; got {query:?}",
                case.include_citations
            );
            // The document reports the answer the query reports: the response and the
            // sources it was drawn from together, or neither.
            if case.include_citations.unwrap_or(false) {
                let citations = query["citations"].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    query["response"].is_null(),
                    citations.is_empty(),
                    "case {label:?}: the exported response and the sources it was drawn \
                     from are reported together, got {query:?}"
                );
                for citation in &citations {
                    assert!(
                        citation["sourceLabel"].as_str().is_some_and(|label| !label
                            .is_empty())
                            && citation["sourceContent"]
                                .as_str()
                                .is_some_and(|content| !content.is_empty())
                            && citation["sourceLink"]
                                .as_str()
                                .is_some_and(|link| link.starts_with("https://")),
                        "case {label:?}: an exported citation names its source, quotes it \
                         and links to it, got {citation:?}"
                    );
                }
            }
        }
        assert_eq!(
            document["includeCitations"],
            json!(case.include_citations.unwrap_or(false)),
            "case {label:?}: the document must state the export options it was taken \
             under, got {:?}",
            document["includeCitations"]
        );
    }
}

#[tokio::test]
async fn following_a_document_url_is_a_read_that_changes_nothing() {
    let router = emulator();
    let created = create(
        &router,
        &text_body("Downloaded twice", "Where is customer data stored?", &[]),
    )
    .await;
    let id = created_id(&created);
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let exported = export(&router, &export_body(&id, None, Some(true))).await;
    let url = exported.url().to_string();

    let first = follow(&router, "GET", &url).await;
    let second = follow(&router, "GET", &url).await;
    assert_eq!(
        (first.status, second.status),
        (200, 200),
        "both downloads must be served, got {} and {}",
        first.status,
        second.status
    );
    assert_eq!(
        first.body, second.body,
        "a download must not change the document a later caller sees, got {:?} against \
         {:?}",
        String::from_utf8_lossy(&first.body),
        String::from_utf8_lossy(&second.body)
    );

    // A HEAD reports the document's metadata without its bytes.
    let head = follow(&router, "HEAD", &url).await;
    assert_eq!(
        head.status, 200,
        "a HEAD of the document must be served, got {} with {:?}",
        head.status,
        String::from_utf8_lossy(&head.body)
    );
    assert!(
        head.body.is_empty(),
        "a HEAD carries no body, got {:?}",
        String::from_utf8_lossy(&head.body)
    );
    assert_eq!(
        head.content_length.as_deref(),
        Some(first.body.len().to_string().as_str()),
        "a HEAD must report the document's length, got {:?} against {} bytes",
        head.content_length,
        first.body.len()
    );

    // And a method the document plane does not serve is refused rather than served.
    let posted = follow(&router, "POST", &url).await;
    assert_eq!(
        posted.status, 405,
        "a POST to a document URL must be refused, got {} with {:?}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
}

#[tokio::test]
async fn a_document_url_that_names_no_export_of_this_caller_s_is_refused() {
    let router = emulator();
    let first = create(
        &router,
        &text_body("First", "Where is customer data stored?", &[]),
    )
    .await;
    let first_id = created_id(&first);
    let second = create(
        &router,
        &text_body("Second", "Is customer data encrypted at rest?", &[]),
    )
    .await;
    let second_id = created_id(&second);
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let served = export(&router, &export_body(&first_id, None, None)).await;
    let url = served.url().to_string();
    let (host, path_and_query) = split_url(&url);
    let (path, query) = path_and_query.split_once('?').unwrap_or_else(|| {
        panic!("{url:?} must carry the token that authorises the fetch")
    });
    let export_id = path
        .rsplit('/')
        .nth(1)
        .expect("the URL carries an export id")
        .to_string();
    let object = path.rsplit('/').next().expect("the URL carries an object");

    // One row per way a path can name no document this caller was handed, each presenting
    // the token the announced URL carried, with the refusal the plane answers under. That
    // token authorises ONE export under the inquiry it was taken from and nothing else, so
    // an export id this store never minted and the right export crossed with another
    // inquiry are refused as unauthorised rather than reported absent — a 404 there would
    // let a caller enumerate which exports exist. Only a path the token authorises is
    // answered 404, for an object the export does not publish.
    let refused = [
        (
            "an export id that was never minted",
            format!(
                "{EXPORT_DOCUMENT_PATH_PREFIX}/{first_id}/export/export-dead0000beef0000/\
                 {object}?{query}"
            ),
            403,
        ),
        (
            "the export under another inquiry",
            format!("{EXPORT_DOCUMENT_PATH_PREFIX}/{second_id}/export/{export_id}/{object}?{query}"),
            403,
        ),
        (
            "the export under another object name",
            format!(
                "{EXPORT_DOCUMENT_PATH_PREFIX}/{first_id}/export/{export_id}/\
                 other-report.json?{query}"
            ),
            404,
        ),
    ];
    for (label, candidate, expected) in refused {
        let answer = follow(&router, "GET", &format!("https://{host}{candidate}")).await;
        assert_eq!(
            answer.status, expected,
            "case {label:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The URL the export handed out is still served, which is what says the refusals
    // above judge the path rather than the class of request.
    let fetched = follow(&router, "GET", &url).await;
    assert_eq!(
        fetched.status, 200,
        "the published URL must still be served, got {} with {:?}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
}

#[tokio::test]
async fn every_export_of_one_inquiry_is_its_own_document() {
    let router = emulator();
    let created = create(
        &router,
        &text_body("Repeat export", "Where is customer data stored?", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let id = created_id(&created);
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    // The rows differ along the options, so the invariant holds both for repeats of one
    // request and for exports taken under different options.
    let requests = [
        ("no options", export_body(&id, None, None)),
        ("no options again", export_body(&id, None, None)),
        ("citations on", export_body(&id, None, Some(true))),
        ("the one identifier it owns", export_body(&id, Some(&[1]), None)),
    ];
    let mut urls: BTreeSet<String> = BTreeSet::new();
    for (label, body) in requests {
        let exported = export(&router, &body).await;
        assert_eq!(
            exported.status, 200,
            "case {label:?}: expected HTTP 200, got {} with {:?}",
            exported.status, exported.body
        );
        let url = exported.url().to_string();
        assert!(
            urls.insert(url.clone()),
            "case {label:?}: each export call produces its own document, so its URL must \
             be new; got {url:?} twice"
        );
    }
}

#[tokio::test]
async fn query_identifiers_the_inquiry_does_not_own_are_refused() {
    // The clause the service states for this refusal, naming the member and the
    // requirement its value failed, without echoing the refused identifiers.
    const CLAUSE: &str = "Value at 'queryIdentifiers' failed to satisfy constraint: every query \
                          identifier must name a query of the compliance inquiry";

    let router = emulator();
    let text = create(
        &router,
        &text_body("Owned queries", "Where is customer data stored?", &[]),
    )
    .await;
    let text_id = created_id(&text);
    let file = create(&router, &file_body_of("Empty questionnaire", "", &[], &[])).await;
    let file_id = created_id(&file);
    let upload = create(&router, &file_body("Questionnaire", &["Sheet 1"], &[])).await;
    let upload_id = created_id(&upload);
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    // `queryIdentifiers` is a list of `Integer`, which the model bounds in no other way
    // (`smithy_query(query="shape",shape="QueryIdentifiersList")`), so the rows walk the
    // identifiers around the one a TEXT inquiry owns — below it, above it, both bounds of
    // the type — and an inquiry that owns no query at all.
    let refused = [
        ("the identifier below the first", &text_id, vec![0]),
        ("the identifier after the last", &text_id, vec![2]),
        ("a negative identifier", &text_id, vec![-1]),
        ("the type's floor", &text_id, vec![i32::MIN]),
        ("the type's ceiling", &text_id, vec![i32::MAX]),
        ("one owned and one not", &text_id, vec![1, 2]),
        (
            "one past the sections an upload was submitted in",
            &upload_id,
            vec![2],
        ),
        ("an inquiry that owns no query", &file_id, vec![1]),
    ];
    for (label, id, identifiers) in refused {
        let answer = export(&router, &export_body(id, Some(&identifiers), None)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: an identifier naming no query must be refused with HTTP 400, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?}: expected the modelled ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {label:?}: expected the fieldValidationFailed reason, got {:?}",
            answer.body["reason"]
        );
        assert_eq!(
            answer.body["fieldList"][0]["name"].as_str(),
            Some("queryIdentifiers"),
            "case {label:?}: the refusal must name the member that carried the value, got \
             {:?}",
            answer.body["fieldList"]
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(format!("1 validation error detected: {CLAUSE}").as_str()),
            "case {label:?}: expected the validation envelope stating the clause, got {:?}",
            answer.body["message"]
        );
        assert!(
            answer.body["documentPresignedUrl"].is_null(),
            "case {label:?}: a refused export must serve no URL, got {:?}",
            answer.body["documentPresignedUrl"]
        );
    }

    // The identifier the TEXT inquiry DOES own is served, which is what says the refusal
    // judges the identifier rather than the member.
    let served = export(&router, &export_body(&text_id, Some(&[1]), None)).await;
    assert_eq!(
        served.status, 200,
        "the identifier the inquiry owns must be served, got {} with {:?}",
        served.status, served.body
    );
}

#[tokio::test]
async fn an_inquiry_id_the_store_does_not_hold_is_refused_as_unauthorized() {
    let router = emulator();
    let created = create(
        &router,
        &text_body("Control", "Where is customer data stored?", &[]),
    )
    .await;
    let known = created_id(&created);

    // Well-formed `InquiryId`s (`^compliance-inquiry-[a-zA-Z0-9]{16}$`) the store does
    // not hold: the gateway authorizes the TARGET, so an id this account holds no inquiry
    // for is refused the way every other inquiry operation refuses it — the front door's
    // 401, which does not confirm the resource's absence.
    let unknown = [
        "compliance-inquiry-0000000000000000",
        "compliance-inquiry-ffffffffffffffff",
        "compliance-inquiry-A1b2C3d4E5f6G7h8",
    ];
    for id in unknown {
        assert_ne!(id, known, "the unknown ids must not be the control's");
        let answer = export(&router, &export_body(id, None, None)).await;
        assert_eq!(
            answer.status, 401,
            "case {id:?}: an inquiry this account does not hold must be refused with HTTP \
             401, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "UnauthorizedException",
            "case {id:?}: expected the front door's UnauthorizedException, got {:?}",
            answer.body
        );
        assert!(
            answer.body["documentPresignedUrl"].is_null(),
            "case {id:?}: a refused export must serve no URL, got {:?}",
            answer.body["documentPresignedUrl"]
        );
    }

    let control = export(&router, &export_body(&known, None, None)).await;
    assert_eq!(
        control.status, 200,
        "the stored inquiry must still export, got {} with {:?}",
        control.status, control.body
    );
}

#[tokio::test]
async fn an_export_changes_nothing_a_later_reader_of_the_inquiry_sees() {
    for case in cases() {
        let label = case.label;
        let router = emulator();
        let created = create_in(&router, case.region, &case.body).await;
        let id = created_id(&created);
        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let before = get_metadata(&router, &id).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the inquiry must read back, got {} with {:?}",
            before.status, before.body
        );
        let detail = before.body["complianceInquiryDetail"].clone();

        // Two exports, so a second call cannot be what leaves the record alone.
        for attempt in 0..2 {
            let body = export_body(&id, case.query_identifiers, case.include_citations);
            let exported = export_in(&router, case.region, &body).await;
            assert_eq!(
                exported.status, 200,
                "case {label:?} attempt {attempt}: expected HTTP 200, got {} with {:?}",
                exported.status, exported.body
            );
        }

        let after = get_metadata(&router, &id).await;
        assert_eq!(
            after.body["complianceInquiryDetail"], detail,
            "case {label:?}: exporting must not change the inquiry a later reader sees, \
             got {:?} against {detail:?}",
            after.body["complianceInquiryDetail"]
        );
        assert_eq!(
            after.tags(),
            before.tags(),
            "case {label:?}: exporting must not change the inquiry's tags, got {:?} \
             against {:?}",
            after.body["tags"],
            before.body["tags"]
        );
    }
}
