// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW this model reports a refusal that came back from the
//! KNOWLEDGE-BASE plane behind the endpoint, driven over HTTP through the same router the
//! emulator serves with.
//!
//! The rule under test: an operation of the ASSISTANT plane that takes a `knowledgeBaseId`
//! resolves it by calling the service that holds the knowledge bases, so a refusal that
//! call comes back with is reported as THAT service answered it — its own sentence followed
//! by the envelope naming the relayed call (the service, the status it answered with, that
//! call's own request id and how many attempts it took) — while an operation addressed to
//! the knowledge-base plane itself is answered by that service directly and reports the
//! bare sentence with no envelope.
//!
//! The rows differ along every axis the rule generalises over: which absent
//! `knowledgeBaseId` is named (ids no create minted, one whose knowledge base was created
//! and then deleted), how the parent assistant is addressed (by id and by ARN), what else
//! the create carries (nothing, tags, a `clientToken`), and — for the contrasting bare
//! form — which knowledge-base-plane operation answered. The request id inside the envelope
//! is a fact about the relayed CALL, so the rows assert its FORM and that two calls never
//! report one id, never a particular value.
//!
//! Everything goes over the wire, so a response that is not computed from persisted state
//! cannot pass.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The routing key this model answers under (`src/service.rs`'s `OWN_REGISTRATION_KEY`):
/// the shared `wisdom` endpoint identity is served by the Amazon Q in Connect crate, so
/// this legacy model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The sentence the service that holds the knowledge bases answers a `knowledgeBaseId` it
/// holds nothing under with, as the recorded wire answers of this endpoint word it.
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The service the knowledge-base lookup is relayed to, as the relayed envelope names it,
/// and the status it answers a knowledge base it holds nothing under with: the `@httpError`
/// of `ResourceNotFoundException`
/// (`smithy_query(query="operation",operation="GetKnowledgeBase")`).
const RELAYED_SERVICE: &str = "CloverKnowledge";
const RELAYED_STATUS: u16 = 404;

/// The trailing half of the envelope: the call behind a refusal of the caller's own input
/// is not retried, so it was attempted once.
const RELAYED_TAIL: &str = ") (SDK Attempt Count: 1)";

/// The leading half of the envelope, up to the relayed call's own request id.
fn relayed_head() -> String {
    format!(
        "{MESSAGE_KNOWLEDGE_BASE_NOT_FOUND} (Service: {RELAYED_SERVICE}, Status Code: \
         {RELAYED_STATUS}, Request ID: "
    )
}

/// One `KnowledgeBaseType` this model declares
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")` also declares `EXTERNAL` and
/// `QUICK_RESPONSES`; which one it is decides nothing about this rule).
const KNOWLEDGE_BASE_TYPE: &str = "CUSTOM";

/// The only `AssociationType` this model declares
/// (`smithy_query(query="shape",shape="AssociationType")`).
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

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

    fn message(&self) -> String {
        self.string("/message")
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process through
/// the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
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
            .uri(format!("https://wisdom.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.body().clone();
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

    async fn knowledge_base(&self, name: &str) -> String {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": KNOWLEDGE_BASE_TYPE })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.string("/knowledgeBase/knowledgeBaseId")
    }

    async fn create_association(&self, assistant_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/associations", label(assistant_label)),
            Some(body),
        )
        .await
    }
}

/// A `UuidOrArn` as it travels in a URL: an ARN's slashes are percent-encoded.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// A uuid-shaped id, so a row can name an id of the form this service mints without minting
/// one.
fn uuid(nibble: char) -> String {
    let run = |count: usize| std::iter::repeat_n(nibble, count).collect::<String>();
    format!("{}-{}-{}-{}-{}", run(8), run(4), run(4), run(4), run(12))
}

/// The request id the envelope reports, having asserted the whole refusal around it.
fn relayed_request_id(case: &str, reply: &Reply) -> String {
    let message = reply.message();
    let head = relayed_head();
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with {}",
        reply.status, reply.body
    );
    assert!(
        reply.error_type.starts_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
        reply.error_type,
        reply.body
    );
    assert!(
        message.starts_with(&head),
        "case {case:?}: expected the message to start {head:?}, got {message:?}"
    );
    assert!(
        message.ends_with(RELAYED_TAIL),
        "case {case:?}: expected the message to end {RELAYED_TAIL:?}, got {message:?}"
    );
    let request_id = message
        .strip_prefix(&head)
        .and_then(|rest| rest.strip_suffix(RELAYED_TAIL))
        .unwrap_or_default()
        .to_string();
    assert!(
        service_common::query_error_envelope::is_request_id(&request_id),
        "case {case:?}: expected the envelope to name the relayed call by a request id, got \
         {request_id:?} in {message:?}"
    );
    request_id
}

/// RULE: a `knowledgeBaseId` the knowledge-base plane holds nothing under is refused on the
/// assistant plane with that plane's refusal RELAYED — its sentence, then the envelope
/// naming the call — for every way a create can name an absent knowledge base.
#[tokio::test]
async fn an_absent_knowledge_base_is_refused_with_the_relayed_envelope() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("relayed-kb-assistant").await;
    let deleted = emulator.knowledge_base("relayed-kb-deleted").await;
    let removed = emulator
        .send("DELETE", &format!("/knowledgeBases/{deleted}"), None)
        .await;
    assert!(
        removed.status == 204 || removed.status == 200,
        "case \"delete the knowledge base\": expected a success, got {} with {}",
        removed.status,
        removed.body
    );

    let rows: Vec<(&str, String, Value)> = vec![
        (
            "an id no create minted",
            assistant_id.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": uuid('a') },
            }),
        ),
        (
            "another id no create minted",
            assistant_id.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": uuid('b') },
            }),
        ),
        (
            "a knowledge base that was deleted",
            assistant_id.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": deleted.clone() },
            }),
        ),
        (
            "the parent addressed by ARN",
            assistant_arn.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": uuid('c') },
            }),
        ),
        (
            "a create carrying tags",
            assistant_id.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": uuid('d') },
                "tags": { "env": "test", "team": "support" },
            }),
        ),
        (
            "a create carrying a clientToken",
            assistant_id.clone(),
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": uuid('e') },
                "clientToken": "relayed-kb-token",
            }),
        ),
    ];

    let mut request_ids = Vec::new();
    for (case, parent, body) in rows {
        let requested = body
            .pointer("/association/knowledgeBaseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reply = emulator.create_association(&parent, body).await;
        request_ids.push(relayed_request_id(case, &reply));
        // The requested label travels in the member the model provides for naming it, so
        // the message does not have to quote it.
        assert_eq!(
            reply.string("/resourceName"),
            requested,
            "case {case:?}: expected resourceName to name the requested knowledge base, got {}",
            reply.body
        );
    }

    // The envelope's request id is a fact about the relayed CALL, so no two of these
    // refusals report one id.
    for (index, id) in request_ids.iter().enumerate() {
        assert!(
            !request_ids[..index].contains(id),
            "case \"one id per relayed call\": expected every refusal its own request id, got \
             {id:?} twice in {request_ids:?}"
        );
    }
}

/// RULE: the envelope is reported for the RELAYED lookup only. A refusal the knowledge-base
/// plane answers directly carries the bare sentence, and a knowledge base that plane DOES
/// hold is associated rather than refused.
#[tokio::test]
async fn a_refusal_that_relays_no_call_carries_the_bare_sentence() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("bare-sentence-assistant").await;
    let absent = uuid('9');

    let direct: Vec<(&str, &str, String)> = vec![
        ("GetKnowledgeBase", "GET", format!("/knowledgeBases/{absent}")),
        (
            "DeleteKnowledgeBase",
            "DELETE",
            format!("/knowledgeBases/{absent}"),
        ),
        (
            "ListContents",
            "GET",
            format!("/knowledgeBases/{absent}/contents"),
        ),
    ];
    for (case, method, path) in direct {
        let reply = emulator.send(method, &path, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.message(),
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            "case {case:?}: expected the bare sentence, got {}",
            reply.body
        );
    }

    let held = emulator.knowledge_base("bare-sentence-knowledge-base").await;
    let created = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": held },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"a knowledge base the plane holds\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/assistantAssociation/associationData/knowledgeBaseAssociation/knowledgeBaseId"),
        held,
        "case \"a knowledge base the plane holds\": expected the association to name it, got {}",
        created.body
    );
}
