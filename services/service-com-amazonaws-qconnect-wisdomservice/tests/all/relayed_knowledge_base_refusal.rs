// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW this endpoint reports a refusal that came back from the
//! KNOWLEDGE-BASE plane behind it, driven over HTTP through the same router the production
//! emulator mounts.
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
//! `knowledgeBaseId` is named (several ids no create minted, one whose knowledge base was
//! created and then deleted), how the parent assistant is addressed (by id and by ARN),
//! what else the create carries (nothing, tags, a `clientToken`), whether the assistant
//! already holds an association, and — for the contrasting bare form — which
//! knowledge-base-plane operation answered. The request id inside the envelope is a fact
//! about the relayed CALL, so the rows assert its FORM and that two calls never report one
//! id, never a particular value.
//!
//! Everything goes over the wire, so a response that is not computed from persisted state
//! cannot pass. The harness is the one this surface is already driven with
//! (`idempotency_contents_and_refusal_fidelity.rs`).

use serde_json::{json, Value};

use super::idempotency_contents_and_refusal_fidelity::{Emulator, Reply};

/// The sentence the service that holds the knowledge bases answers a `knowledgeBaseId` it
/// holds nothing under with, as the recorded wire answers of this endpoint word it.
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The sentence this endpoint answers a request addressed to an assistant this account does
/// not hold with — the assistant plane's OWN resource, so no call is relayed for it.
const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";

/// The service the knowledge-base lookup is relayed to, as the relayed envelope names it,
/// and the status it answers a knowledge base it holds nothing under with: the `@httpError`
/// of `ResourceNotFoundException`
/// (`smithy_query(query="operation",operation="GetKnowledgeBase")`).
const RELAYED_SERVICE: &str = "CloverKnowledge";
const RELAYED_STATUS: u16 = 404;

/// The two fixed halves of the envelope, around the request id of the relayed call.
fn relayed_head() -> String {
    format!(
        "{MESSAGE_KNOWLEDGE_BASE_NOT_FOUND} (Service: {RELAYED_SERVICE}, Status Code: \
         {RELAYED_STATUS}, Request ID: "
    )
}
const RELAYED_TAIL: &str = ") (SDK Attempt Count: 1)";

/// The only `KnowledgeBaseType` a knowledge base of these rows is created as
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")` also declares `EXTERNAL` and
/// `QUICK_RESPONSES`; which one it is decides nothing about this rule).
const KNOWLEDGE_BASE_TYPE: &str = "CUSTOM";

/// A uuid-shaped id, so a row can name an id of the form this service mints without
/// minting one.
fn uuid(nibble: char) -> String {
    let run = |count: usize| std::iter::repeat_n(nibble, count).collect::<String>();
    format!("{}-{}-{}-{}-{}", run(8), run(4), run(4), run(4), run(12))
}

/// Create one knowledge base and hand back its id.
async fn knowledge_base(emulator: &Emulator, name: &str) -> String {
    let reply = emulator
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

/// A `UuidOrArn` as it travels in a URL: an ARN's slashes are percent-encoded.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

async fn create_association(emulator: &Emulator, assistant_label: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/assistants/{}/associations", label(assistant_label)),
            Some(body),
        )
        .await
}

/// The request id the envelope reports, for a message that carries the envelope at all.
fn relayed_request_id(case: &str, reply: &Reply) -> String {
    let message = reply.message();
    let head = relayed_head();
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
        reply.code(),
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

    // One knowledge base that EXISTS, so a row can hold an association already, and one
    // that existed and was deleted — an id that resolved once and resolves nothing now.
    let held = knowledge_base(&emulator, "relayed-kb-held").await;
    let deleted = knowledge_base(&emulator, "relayed-kb-deleted").await;
    let removed = emulator
        .send("DELETE", &format!("/knowledgeBases/{deleted}"), None)
        .await;
    assert!(
        removed.status == 204 || removed.status == 200,
        "case \"delete the knowledge base\": expected a success, got {} with {}",
        removed.status,
        removed.body
    );

    // An association the assistant already holds, so the last row's refusal is answered by
    // an assistant that is not empty.
    let first = create_association(
        &emulator,
        &assistant_id,
        json!({
            "associationType": "KNOWLEDGE_BASE",
            "association": { "knowledgeBaseId": held },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case \"an association the assistant holds\": expected HTTP 200, got {} with {}",
        first.status, first.body
    );

    let rows: Vec<(&str, String, Value)> = vec![
        (
            "an id no create minted",
            assistant_id.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": uuid('a') },
            }),
        ),
        (
            "another id no create minted",
            assistant_id.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": uuid('b') },
            }),
        ),
        (
            "a knowledge base that was deleted",
            assistant_id.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": deleted.clone() },
            }),
        ),
        (
            "the parent addressed by ARN",
            assistant_arn.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": uuid('c') },
            }),
        ),
        (
            "a create carrying tags",
            assistant_id.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
                "association": { "knowledgeBaseId": uuid('d') },
                "tags": { "env": "test", "team": "support" },
            }),
        ),
        (
            "a create carrying a clientToken, on an assistant that already holds one",
            assistant_id.clone(),
            json!({
                "associationType": "KNOWLEDGE_BASE",
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
        let reply = create_association(&emulator, &parent, body).await;
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

/// RULE: the envelope is reported for the RELAYED lookup only. A refusal the assistant
/// plane answers out of its OWN store, and one the knowledge-base plane answers directly,
/// carry the bare sentence with nothing appended.
#[tokio::test]
async fn a_refusal_that_relays_no_call_carries_the_bare_sentence() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("bare-sentence-assistant").await;
    let absent_assistant = uuid('f');
    let absent_knowledge_base = uuid('9');

    // The assistant plane's own resource: no call is relayed to resolve it.
    let own = create_association(
        &emulator,
        &absent_assistant,
        json!({
            "associationType": "KNOWLEDGE_BASE",
            "association": { "knowledgeBaseId": absent_knowledge_base },
        }),
    )
    .await;
    assert_eq!(
        own.message(),
        MESSAGE_ASSISTANT_NOT_FOUND,
        "case \"an assistant this account does not hold\": expected the bare sentence, got {}",
        own.body
    );

    // The knowledge-base plane, addressed directly: it answers for itself, so there is no
    // relayed call to name.
    let direct: Vec<(&str, &str, String)> = vec![
        (
            "GetKnowledgeBase",
            "GET",
            format!("/knowledgeBases/{absent_knowledge_base}"),
        ),
        (
            "DeleteKnowledgeBase",
            "DELETE",
            format!("/knowledgeBases/{absent_knowledge_base}"),
        ),
        (
            "ListContents",
            "GET",
            format!("/knowledgeBases/{absent_knowledge_base}/contents"),
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

    // And the assistant plane still resolves a knowledge base that IS held, so the relayed
    // call is made rather than always refused.
    let kb = knowledge_base(&emulator, "bare-sentence-knowledge-base").await;
    let created = create_association(
        &emulator,
        &assistant_id,
        json!({
            "associationType": "KNOWLEDGE_BASE",
            "association": { "knowledgeBaseId": kb },
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
        kb,
        "case \"a knowledge base the plane holds\": expected the association to name it, got {}",
        created.body
    );
}
