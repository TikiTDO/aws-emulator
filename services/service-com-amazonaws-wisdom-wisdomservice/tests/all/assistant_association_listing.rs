// SPDX-License-Identifier: CC0-1.0

//! What `ListAssistantAssociations` REPORTS, driven over the wire through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a listing reports exactly the associations the named assistant holds — the stored
//!   records, each carrying the knowledge base its create was made against — in
//!   `assistantAssociationId` order, under either form of the parent's label, for every
//!   association count including NONE (an assistant that holds none answers the required
//!   list, empty, with no continuation token — not a not-found);
//! * `maxResults` is honoured at each published bound of its modelled range
//!   (`smithy_query(query="operation",operation="ListAssistantAssociations")`:
//!   `maxResults` `range 1..100`, `nextToken` `len 1..2048`) and a value one past either
//!   bound is refused with the modelled `ValidationException` (HTTP 400), as is a
//!   `nextToken` longer than its modelled maximum;
//! * the listing REPORTS: repeating it, and walking it a page at a time, leave what a
//!   later caller sees unchanged, while a `DeleteAssistantAssociation` between two pages
//!   drops exactly that one record and a token already handed out still resumes at the
//!   first surviving id at or after it.
//!
//! Every assertion goes over the wire and nothing reaches into the service's state struct,
//! so a response that is not computed from persisted state cannot pass. The harness below
//! is this suite's own, as each suite in this crate carries its own.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it out of the
/// SigV4 credential scope to build ARNs.
const REGION: &str = "ap-south-1";

/// The endpoint host those requests address.
const ENDPOINT_HOST: &str = "wisdom.ap-south-1.amazonaws.com";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` endpoint identity is served by the Amazon
/// Q in Connect crate, so this legacy model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The only `AssociationType` this model declares
/// (`smithy_query(query="shape",shape="AssociationType")`), written out rather than
/// imported from the implementation so the assertions pin the modelled value.
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The error code this model declares for input it validates itself.
const VALIDATION: &str = "ValidationException";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn array(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn object(&self, pointer: &str) -> Map<String, Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// The continuation token this answer carries, if it carries one.
    fn next_token(&self) -> Option<String> {
        self.body
            .pointer("/nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The ids of the summaries this answer reports, in the order it reported them.
    fn listed_ids(&self) -> Vec<String> {
        self.array("/assistantAssociationSummaries")
            .iter()
            .map(|summary| {
                summary
                    .pointer("/assistantAssociationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// An emulator with just this service mounted on a fresh engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what this suite is about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{uri}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize the request body"),
                ))
                .expect("build a request with a JSON body"),
            None => builder.body(Bytes::new()).expect("build a bodyless request"),
        };
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// An assistant, as its create persisted it: `(id, arn)`.
    async fn assistant(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/assistants",
                Some(json!({ "name": name, "type": "AGENT" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"assistant {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/assistant/assistantId"),
            reply.string("/assistant/assistantArn"),
        )
    }

    /// A `CUSTOM` knowledge base, as its create persisted it: `(id, arn)`.
    async fn knowledge_base(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": "CUSTOM" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/knowledgeBase/knowledgeBaseId"),
            reply.string("/knowledgeBase/knowledgeBaseArn"),
        )
    }

    /// Associate one knowledge base with one assistant and answer the whole
    /// `assistantAssociation` the create reported.
    async fn associate(&self, assistant_id: &str, knowledge_base_id: &str) -> Map<String, Value> {
        let reply = self
            .send(
                "POST",
                &format!("/assistants/{assistant_id}/associations"),
                Some(json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"associate {knowledge_base_id}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.object("/assistantAssociation")
    }

    async fn delete_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "DELETE",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn get_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn list_associations(&self, assistant_label: &str, query: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{}/associations{query}", label(assistant_label)),
            None,
        )
        .await
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded, or
/// they would be path separators naming another route.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// One assistant of a table row: its labels and the associations it was given, each as
/// `(association id, knowledge base id, knowledge base ARN)` in creation order.
struct Holder {
    case: String,
    assistant_id: String,
    assistant_arn: String,
    associations: Vec<(String, String, String)>,
}

/// Give each row's assistant its own associations, so the listings of several assistants
/// that exist at once can be compared against the records each one holds.
async fn holders(emulator: &Emulator, counts: &[usize]) -> Vec<Holder> {
    let mut built = Vec::new();
    for count in counts {
        let (assistant_id, assistant_arn) = emulator
            .assistant(&format!("listing-holder-of-{count}"))
            .await;
        let mut associations = Vec::new();
        for index in 0..*count {
            let (knowledge_base_id, knowledge_base_arn) = emulator
                .knowledge_base(&format!("listing-kb-{count}-{index}"))
                .await;
            let created = emulator.associate(&assistant_id, &knowledge_base_id).await;
            let association_id = created
                .get("assistantAssociationId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            associations.push((association_id, knowledge_base_id, knowledge_base_arn));
        }
        built.push(Holder {
            case: format!("an assistant holding {count} association(s)"),
            assistant_id,
            assistant_arn,
            associations,
        });
    }
    built
}

/// RULE: a listing reports exactly the associations the named assistant holds, as the
/// stored records and in `assistantAssociationId` order, for every count including none.
#[tokio::test]
async fn a_listing_reports_exactly_the_associations_the_assistant_holds() {
    let emulator = Emulator::new();
    // The counts differ along what the rule generalises over: none, one, a few, and more
    // than one page's worth at the smallest page size tested below.
    let holders = holders(&emulator, &[0, 1, 2, 5]).await;

    for holder in &holders {
        let mut expected: Vec<String> = holder
            .associations
            .iter()
            .map(|(association_id, _, _)| association_id.clone())
            .collect();
        expected.sort();

        for (addressing, assistant_label) in [
            ("by id", holder.assistant_id.clone()),
            ("by arn", holder.assistant_arn.clone()),
        ] {
            let case = format!("{} addressed {addressing}", holder.case);
            let reply = emulator.list_associations(&assistant_label, "").await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {}",
                reply.status, reply.body
            );
            assert!(
                reply
                    .body
                    .pointer("/assistantAssociationSummaries")
                    .is_some_and(Value::is_array),
                "case {case:?}: expected the required summary list, got {}",
                reply.body
            );
            assert_eq!(
                reply.listed_ids(),
                expected,
                "case {case:?}: expected every association once in id order, got {}",
                reply.body
            );
            assert_eq!(
                reply.next_token(),
                None,
                "case {case:?}: expected no continuation token on a single page, got {:?}",
                reply.next_token()
            );

            // Each summary carries the knowledge base its create was made against, and the
            // parent the route addressed.
            for summary in reply.array("/assistantAssociationSummaries") {
                let association_id = summary
                    .pointer("/assistantAssociationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let (_, knowledge_base_id, knowledge_base_arn) = holder
                    .associations
                    .iter()
                    .find(|(candidate, _, _)| *candidate == association_id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {association_id} is not one of this assistant's")
                    });
                assert_eq!(
                    (
                        summary.pointer("/assistantId"),
                        summary.pointer("/assistantArn"),
                        summary.pointer("/associationType"),
                        summary.pointer("/associationData/knowledgeBaseAssociation/knowledgeBaseId"),
                        summary
                            .pointer("/associationData/knowledgeBaseAssociation/knowledgeBaseArn"),
                    ),
                    (
                        Some(&json!(holder.assistant_id)),
                        Some(&json!(holder.assistant_arn)),
                        Some(&json!(TYPE_KNOWLEDGE_BASE)),
                        Some(&json!(knowledge_base_id)),
                        Some(&json!(knowledge_base_arn)),
                    ),
                    "case {case:?}: expected the stored record of {association_id}, got {summary}"
                );
            }
        }
    }

    // One assistant's listing never carries another's association.
    let every_listed: Vec<String> = {
        let mut seen = Vec::new();
        for holder in &holders {
            seen.extend(
                emulator
                    .list_associations(&holder.assistant_id, "")
                    .await
                    .listed_ids(),
            );
        }
        seen
    };
    let mut unique = every_listed.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        every_listed.len(),
        "case \"across assistants\": expected each association in exactly one listing, got \
         {every_listed:?}"
    );
}

/// RULE: `maxResults` and `nextToken` are honoured at each published bound of their
/// modelled ranges, and a value one past a bound is the modelled `ValidationException`.
#[tokio::test]
async fn the_paging_members_are_honoured_at_their_published_bounds() {
    let emulator = Emulator::new();
    let holders = holders(&emulator, &[3]).await;
    let holder = &holders[0];
    let mut expected: Vec<String> = holder
        .associations
        .iter()
        .map(|(association_id, _, _)| association_id.clone())
        .collect();
    expected.sort();

    // `maxResults` is `range 1..100`: each published bound, and one past each.
    for (case, max_results, status) in [
        ("maxResults at the lower bound", 1, 200),
        ("maxResults at the upper bound", 100, 200),
        ("maxResults one below the lower bound", 0, 400),
        ("maxResults one past the upper bound", 101, 400),
    ] {
        let reply = emulator
            .list_associations(&holder.assistant_id, &format!("?maxResults={max_results}"))
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        if status == 400 {
            assert_eq!(
                reply.error_type, VALIDATION,
                "case {case:?}: expected the modelled {VALIDATION}, got {:?} with {}",
                reply.error_type, reply.body
            );
        } else {
            let page_size = usize::try_from(max_results).expect("a modelled page size");
            assert_eq!(
                reply.listed_ids(),
                expected[..expected.len().min(page_size)].to_vec(),
                "case {case:?}: expected the first page of {expected:?}, got {}",
                reply.body
            );
        }
    }

    // `nextToken` is `len 1..2048`: a token at either published length is read as a
    // continuation, one past the maximum is refused for its length.
    for (case, token_length, status) in [
        ("nextToken at the shortest modelled length", 1, 200),
        ("nextToken at the longest modelled length", 2048, 200),
        ("nextToken one past the longest modelled length", 2049, 400),
    ] {
        // `0` is the first character of the modelled id alphabet, so a token made of it
        // continues from the start of the order — except for an id whose shortest group
        // runs out first, which the expectation below takes account of rather than
        // depending on which ids the mint happened to hand out.
        let token = "0".repeat(token_length);
        let reply = emulator
            .list_associations(&holder.assistant_id, &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        if status == 400 {
            assert_eq!(
                reply.error_type, VALIDATION,
                "case {case:?}: expected the modelled {VALIDATION}, got {:?} with {}",
                reply.error_type, reply.body
            );
        } else {
            // A continuation resumes at the first id at or after the token it carries.
            let continued: Vec<String> = expected
                .iter()
                .filter(|association_id| association_id.as_str() >= token.as_str())
                .cloned()
                .collect();
            assert_eq!(
                reply.listed_ids(),
                continued,
                "case {case:?}: expected the listing from the token on, got {}",
                reply.body
            );
        }
    }
}

/// RULE: the listing is a REPORT — repeating it or walking it page by page leaves what a
/// later caller sees unchanged — and it follows the records, so a delete between two pages
/// drops exactly that record while a token already handed out resumes at the first
/// surviving id at or after it.
#[tokio::test]
async fn the_listing_reports_the_records_and_changes_nothing() {
    let emulator = Emulator::new();
    let holders = holders(&emulator, &[3]).await;
    let holder = &holders[0];
    let assistant_id = holder.assistant_id.clone();
    let mut expected: Vec<String> = holder
        .associations
        .iter()
        .map(|(association_id, _, _)| association_id.clone())
        .collect();
    expected.sort();

    // Two identical listings answer the same thing, and so does one taken after a partial
    // walk: nothing is consumed by reading.
    let first = emulator.list_associations(&assistant_id, "").await;
    let again = emulator.list_associations(&assistant_id, "").await;
    assert_eq!(
        (first.status, &first.body),
        (again.status, &again.body),
        "case \"a repeated listing\": expected the same answer, got {} then {}",
        first.body,
        again.body
    );

    let page = emulator
        .list_associations(&assistant_id, "?maxResults=1")
        .await;
    assert_eq!(
        page.listed_ids(),
        expected[..1].to_vec(),
        "case \"the first page of one\": expected {:?}, got {}",
        &expected[..1],
        page.body
    );
    let token = page.next_token().unwrap_or_else(|| {
        panic!(
            "case \"the first page of one\": expected a token, got {}",
            page.body
        )
    });
    assert_eq!(
        token, expected[1],
        "case \"the first page of one\": expected the next page's first id as the token, got \
         {token:?}"
    );
    let whole = emulator.list_associations(&assistant_id, "").await;
    assert_eq!(
        whole.listed_ids(),
        expected,
        "case \"after a partial walk\": expected the whole listing unchanged, got {}",
        whole.body
    );

    // A delete between two pages drops exactly that record, and the token handed out
    // before it resumes at the first surviving id at or after it.
    let removed = expected[1].clone();
    let deleted = emulator.delete_association(&assistant_id, &removed).await;
    // `DeleteAssistantAssociation` is modelled with the empty-body success status
    // (`smithy_query(query="operation",operation="DeleteAssistantAssociation")`: HTTP 204).
    assert_eq!(
        deleted.status, 204,
        "case \"delete the token's own association\": expected HTTP 204, got {} with {}",
        deleted.status, deleted.body
    );
    let resumed = emulator
        .list_associations(&assistant_id, &format!("?nextToken={token}"))
        .await;
    assert_eq!(
        resumed.listed_ids(),
        expected[2..].to_vec(),
        "case \"resume on a token whose association was deleted\": expected {:?}, got {}",
        &expected[2..],
        resumed.body
    );
    let survivors: Vec<String> = expected
        .iter()
        .filter(|association_id| **association_id != removed)
        .cloned()
        .collect();
    let after = emulator.list_associations(&assistant_id, "").await;
    assert_eq!(
        after.listed_ids(),
        survivors,
        "case \"after the delete\": expected {survivors:?}, got {}",
        after.body
    );
    let gone = emulator.get_association(&assistant_id, &removed).await;
    assert_eq!(
        (gone.status, gone.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a read of the deleted association\": expected HTTP 404 \
         ResourceNotFoundException, got {} {:?} ({})",
        gone.status,
        gone.error_type,
        gone.body
    );

    // Every survivor still reads back as the listing reports it, so the listings above
    // changed no record.
    for summary in after.array("/assistantAssociationSummaries") {
        let association_id = summary
            .pointer("/assistantAssociationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let read = emulator.get_association(&assistant_id, &association_id).await;
        assert_eq!(
            (read.status, Value::Object(read.object("/assistantAssociation"))),
            (200, summary.clone()),
            "case \"survivor {association_id}\": expected the record the read answers with, got \
             {} with {}",
            read.status,
            read.body
        );
    }
}
