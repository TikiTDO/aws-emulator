// SPDX-License-Identifier: CC0-1.0

//! A guardrail CREATED in the Amazon Bedrock control plane is the guardrail the
//! Bedrock Runtime data plane applies — on `ApplyGuardrail` and on a Converse turn
//! that names it — and the two surfaces agree about what it does to the same text.
//!
//! The rule under test: **the policies a guardrail revision configures are evaluated
//! by ONE stage, so applying a guardrail to a piece of text and sending that same
//! text through a request that names the guardrail reach the same verdict; and what
//! is evaluated is whatever the control plane last stored for that revision.** Two
//! crates have to agree for that to hold (the control plane owns the guardrail
//! resource, the data plane applies it), so the suite lives here: a crate-local test
//! can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: which policy the revision
//! configures, whether the text carries what that policy places, which revision is
//! applied (the DRAFT, and a numbered version cut from it), and what the control
//! plane has since done to the guardrail (updated it, cut a version of it, deleted
//! it).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The signing name the whole Amazon Bedrock family shares; the request's path is
/// what decides which member of it claims a request.
const BEDROCK: &str = "bedrock";

/// The host a data-plane request addresses, which is how AWS separates the two
/// endpoints.
const RUNTIME_HOST: &str = "bedrock-runtime";

/// A model this emulator serves the Converse API for.
const MODEL_ID: &str = "amazon.nova-lite-v1:0";

/// What the guardrail under test answers content it blocked on each side with.
const BLOCKED_INPUT: &str = "Sorry, the model cannot answer this question.";
const BLOCKED_OUTPUTS: &str = "Sorry, the model cannot answer with this text.";

/// The word the guardrail's word policy blocks.
const BLOCKED_WORD: &str = "zephyrine";

/// The emulator with the two services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is
/// what routes the request. The signature is never checked: request authentication is
/// off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One restJson1 exchange with one of the two services.
async fn call(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("{host}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(BEDROCK));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let bytes = body.map_or_else(Bytes::new, |body| {
        Bytes::from(serde_json::to_vec(body).expect("a serializable test body"))
    });
    let response = router
        .dispatch(request.body(bytes).expect("a well-formed request"))
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateGuardrail` with `policies` (the `*PolicyConfig` members of the request).
async fn create_guardrail(
    router: &MultiServiceRouter,
    name: &str,
    policies: Value,
) -> (u16, Value) {
    let mut body = json!({
        "name": name,
        "blockedInputMessaging": BLOCKED_INPUT,
        "blockedOutputsMessaging": BLOCKED_OUTPUTS,
    });
    for (member, value) in policies.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    call(router, BEDROCK, "POST", "/guardrails", Some(&body)).await
}

/// The word policy the suite's guardrail is created with: one configured word,
/// blocking both sides.
fn word_policy() -> Value {
    json!({ "wordPolicyConfig": { "wordsConfig": [{ "text": BLOCKED_WORD }] } })
}

/// `ApplyGuardrail` on one revision, over the data plane's own wire.
async fn apply_guardrail(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    source: &str,
    text: &str,
) -> (u16, Value) {
    call(
        router,
        RUNTIME_HOST,
        "POST",
        &format!("/guardrail/{identifier}/version/{version}/apply"),
        Some(&json!({
            "source": source,
            "content": [{ "text": { "text": text } }],
        })),
    )
    .await
}

/// `Converse` naming the guardrail in its own configuration, with the trace asked
/// for.
async fn converse_with_guardrail(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    text: &str,
) -> (u16, Value) {
    call(
        router,
        RUNTIME_HOST,
        "POST",
        &format!("/model/{MODEL_ID}/converse"),
        Some(&json!({
            "messages": [{ "role": "user", "content": [{ "text": text }] }],
            "guardrailConfig": {
                "guardrailIdentifier": identifier,
                "guardrailVersion": version,
                "trace": "enabled",
            },
        })),
    )
    .await
}

/// The text a Converse turn answered with.
fn converse_text(answered: &Value) -> String {
    answered["output"]["message"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a guardrail the control plane created is applied by the data plane, and the
/// two surfaces that apply it reach the same verdict for the same text.
#[tokio::test]
async fn a_created_guardrail_is_applied_consistently_by_both_surfaces() {
    let router = build_router();
    let (status, created) = create_guardrail(&router, "consistent-guardrail", word_policy()).await;
    assert_eq!(
        status, 202,
        "the guardrail must be created, got {status} / {created}"
    );
    let id = created["guardrailId"]
        .as_str()
        .expect("the control plane mints a guardrailId")
        .to_string();

    // (case, the prompt, blocked)
    let rows: [(&str, &str, bool); 5] = [
        ("the configured word alone", BLOCKED_WORD, true),
        (
            "the configured word in a sentence",
            "please tell me about zephyrine today",
            true,
        ),
        ("the configured word, capitalised", "What is Zephyrine?", true),
        (
            "a longer word that merely contains it",
            "zephyrines are a different thing",
            false,
        ),
        (
            "nothing the policy configures",
            "What time does the library open?",
            false,
        ),
    ];
    for (case, prompt, blocked) in rows {
        let (status, applied) = apply_guardrail(&router, &id, "DRAFT", "INPUT", prompt).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the apply must be served, got {status} / {applied}"
        );
        let expected_action = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        assert_eq!(
            applied["action"].as_str(),
            Some(expected_action),
            "case {case:?}: expected action {expected_action}, got {applied}"
        );

        let (status, answered) = converse_with_guardrail(&router, &id, "DRAFT", prompt).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the turn must be served, got {status} / {answered}"
        );
        let stop_reason = answered["stopReason"].as_str().unwrap_or_default();
        let intervened = stop_reason == "guardrail_intervened";
        assert_eq!(
            intervened, blocked,
            "case {case:?}: the two surfaces must agree; the apply said blocked={blocked} and \
             the turn stopped on {stop_reason:?}"
        );
        if blocked {
            // The turn a guardrail blocked carries the guardrail's blocked messaging,
            // which is the text the apply answers with too.
            assert_eq!(
                converse_text(&answered),
                BLOCKED_INPUT,
                "case {case:?}: the blocked turn must carry the blocked messaging, got {answered}"
            );
            assert_eq!(
                applied["outputs"][0]["text"].as_str(),
                Some(BLOCKED_INPUT),
                "case {case:?}: the apply must answer the same messaging, got {applied}"
            );
        }
        // The trace the request asked for reports the assessment under the
        // guardrail's own id, and the word policy is what placed the word.
        let assessed = &answered["trace"]["guardrail"]["inputAssessment"][&id];
        let placed = assessed["wordPolicy"]["customWords"]
            .as_array()
            .map(|words| !words.is_empty())
            .unwrap_or_default();
        assert_eq!(
            placed, blocked,
            "case {case:?}: the trace must report what the word policy placed, got {}",
            answered["trace"]
        );
    }
}

/// RULE: the revision applied is the one the control plane last stored for it — a
/// version cut from the DRAFT carries the DRAFT's policies at the instant it was cut,
/// an update replaces the DRAFT's policies, and a deleted guardrail applies to
/// nothing.
#[tokio::test]
async fn the_revision_applied_is_what_the_control_plane_last_stored() {
    let router = build_router();
    let (_, created) = create_guardrail(&router, "versioned-guardrail", word_policy()).await;
    let id = created["guardrailId"]
        .as_str()
        .expect("the control plane mints a guardrailId")
        .to_string();

    // A numbered version, cut from the DRAFT that blocks the word.
    let (status, cut) = call(
        &router,
        BEDROCK,
        "POST",
        &format!("/guardrails/{id}"),
        Some(&json!({ "description": "the policy as created" })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the version must be cut, got {status} / {cut}"
    );
    let version = cut["version"]
        .as_str()
        .expect("the cut answers the version it minted")
        .to_string();

    // The DRAFT is then updated to configure NO word policy at all.
    let (status, updated) = call(
        &router,
        BEDROCK,
        "PUT",
        &format!("/guardrails/{id}"),
        Some(&json!({
            "name": "versioned-guardrail",
            "blockedInputMessaging": BLOCKED_INPUT,
            "blockedOutputsMessaging": BLOCKED_OUTPUTS,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the update must be served, got {status} / {updated}"
    );

    // (case, the version applied, blocked)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the version cut before the update still blocks",
            version.clone(),
            true,
        ),
        (
            "the updated draft configures nothing to block",
            "DRAFT".to_string(),
            false,
        ),
    ];
    for (case, applied_version, blocked) in rows {
        let (status, applied) =
            apply_guardrail(&router, &id, &applied_version, "INPUT", BLOCKED_WORD).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the apply must be served, got {status} / {applied}"
        );
        let expected = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        assert_eq!(
            applied["action"].as_str(),
            Some(expected),
            "case {case:?}: expected action {expected}, got {applied}"
        );
    }

    // Deleting the guardrail leaves nothing to apply, on either surface.
    let (status, deleted) = call(
        &router,
        BEDROCK,
        "DELETE",
        &format!("/guardrails/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "the guardrail must be deletable, got {status} / {deleted}"
    );
    for version in ["DRAFT", version.as_str()] {
        let (status, refused) =
            apply_guardrail(&router, &id, version, "INPUT", BLOCKED_WORD).await;
        assert_eq!(
            status, 400,
            "case {version:?}: a deleted guardrail applies to nothing, got {status} / {refused}"
        );
    }
    let (status, refused) = converse_with_guardrail(&router, &id, "DRAFT", BLOCKED_WORD).await;
    assert_eq!(
        status, 400,
        "a turn naming a deleted guardrail names no guardrail of this account, got {status} / \
         {refused}"
    );
}

/// `Converse` naming the guardrail, with content blocks given verbatim (so a row can
/// mark content for guarding and qualify it).
async fn converse_with_content(
    router: &MultiServiceRouter,
    identifier: &str,
    content: Value,
) -> (u16, Value) {
    call(
        router,
        RUNTIME_HOST,
        "POST",
        &format!("/model/{MODEL_ID}/converse"),
        Some(&json!({
            "messages": [{ "role": "user", "content": content }],
            "guardrailConfig": {
                "guardrailIdentifier": identifier,
                "guardrailVersion": "DRAFT",
                "trace": "enabled",
            },
        })),
    )
    .await
}

/// RULE: a Converse turn's `guardContent` blocks are read by their QUALIFIERS, the
/// same way an apply call reads its own blocks — the grounding source and the query
/// are the material the contextual grounding policy scores the ANSWER against rather
/// than content to guard — and a turn whose guardrail needs that material and carries
/// none is refused instead of being scored against nothing.
#[tokio::test]
async fn a_guarded_turn_scores_its_answer_against_the_material_its_content_qualifies() {
    const SOURCE: &str = "The Mars Reconnaissance Orbiter reached Mars in 2006.";
    const QUERY: &str = "When did the orbiter reach Mars?";

    // (case, the threshold the filter is configured with, blocked)
    let rows: [(&str, f64, bool); 2] = [
        ("a threshold every score reaches", 0.0, false),
        ("a threshold only a fully grounded answer reaches", 1.0, true),
    ];
    for (case, threshold, blocked) in rows {
        let router = build_router();
        let (status, created) = create_guardrail(
            &router,
            "turn-grounding-guardrail",
            json!({
                "contextualGroundingPolicyConfig": {
                    "filtersConfig": [{ "type": "GROUNDING", "threshold": threshold }],
                },
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the guardrail must be created, got {status} / {created}"
        );
        let id = created["guardrailId"]
            .as_str()
            .expect("the control plane mints a guardrailId")
            .to_string();

        let (status, answered) = converse_with_content(
            &router,
            &id,
            json!([
                { "guardContent": { "text": { "text": SOURCE, "qualifiers": ["grounding_source"] } } },
                { "guardContent": { "text": { "text": QUERY, "qualifiers": ["query"] } } },
                { "text": "Tell me when the orbiter arrived." },
            ]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the turn must be served, got {status} / {answered}"
        );
        let stop_reason = answered["stopReason"].as_str().unwrap_or_default();
        assert_eq!(
            stop_reason == "guardrail_intervened",
            blocked,
            "case {case:?}: expected blocked={blocked}, got stop reason {stop_reason:?} in \
             {answered}"
        );
        // The filter scores the ANSWER: it is reported in the OUTPUT assessment, and
        // the input side — which carries no answer to score — reports no contextual
        // grounding policy at all.
        let trace = &answered["trace"]["guardrail"];
        let scored = &trace["outputAssessments"][&id][0]["contextualGroundingPolicy"]["filters"][0];
        assert_eq!(
            scored["threshold"].as_f64(),
            Some(threshold),
            "case {case:?}: the output assessment must report the configured filter, got {trace}"
        );
        assert!(
            scored["score"]
                .as_f64()
                .is_some_and(|score| (0.0..=1.0).contains(&score)),
            "case {case:?}: the filter must report the score it computed, got {trace}"
        );
        assert!(
            trace["inputAssessment"][&id]["contextualGroundingPolicy"].is_null(),
            "case {case:?}: the prompt carries no answer to score, so the input side reports no \
             contextual grounding policy, got {trace}"
        );
    }

    // The same guardrail, with none of the material it scores against: there is
    // nothing to score, so the turn is refused rather than served a verdict about
    // nothing.
    let router = build_router();
    let (_, created) = create_guardrail(
        &router,
        "turn-grounding-guardrail",
        json!({
            "contextualGroundingPolicyConfig": {
                "filtersConfig": [{ "type": "GROUNDING", "threshold": 0.5 }],
            },
        }),
    )
    .await;
    let id = created["guardrailId"]
        .as_str()
        .expect("the control plane mints a guardrailId")
        .to_string();
    let (status, refused) = converse_with_content(
        &router,
        &id,
        json!([{ "text": "Tell me when the orbiter arrived." }]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a turn carrying no grounding source cannot be scored, got {status} / {refused}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("grounding source"),
        "the refusal must name the material the request is missing, got {message:?}"
    );
}

/// RULE: the policies the data plane evaluates are whichever ones the revision
/// configures — so a guardrail created with a contextual grounding policy scores the
/// content it is applied to, and one created with a sensitive-information policy
/// masks the values it is configured to mask.
#[tokio::test]
async fn each_configured_policy_is_evaluated_by_the_data_plane() {
    let router = build_router();

    // The contextual grounding policy: an answer the source supports passes, one it
    // does not is blocked.
    let (status, grounded) = create_guardrail(
        &router,
        "grounding-guardrail",
        json!({
            "contextualGroundingPolicyConfig": {
                "filtersConfig": [{ "type": "GROUNDING", "threshold": 0.8 }],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the guardrail must be created, got {status} / {grounded}"
    );
    let grounding_id = grounded["guardrailId"]
        .as_str()
        .expect("the control plane mints a guardrailId")
        .to_string();

    // (case, the answer, blocked)
    let rows: [(&str, &str, bool); 2] = [
        (
            "an answer taken from the source",
            "The orbiter reached Mars in 2006.",
            false,
        ),
        (
            "an answer the source does not support",
            "Voyager flew past Neptune during a storm.",
            true,
        ),
    ];
    for (case, answer, blocked) in rows {
        let (status, applied) = call(
            &router,
            RUNTIME_HOST,
            "POST",
            &format!("/guardrail/{grounding_id}/version/DRAFT/apply"),
            Some(&json!({
                "source": "OUTPUT",
                "outputScope": "FULL",
                "content": [
                    { "text": { "text": "The orbiter reached Mars in 2006.",
                                "qualifiers": ["grounding_source"] } },
                    { "text": { "text": "When did the orbiter reach Mars?",
                                "qualifiers": ["query"] } },
                    { "text": { "text": answer, "qualifiers": ["guard_content"] } },
                ],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the apply must be served, got {status} / {applied}"
        );
        let expected = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        assert_eq!(
            applied["action"].as_str(),
            Some(expected),
            "case {case:?}: expected action {expected}, got {applied}"
        );
        let score = applied["assessments"][0]["contextualGroundingPolicy"]["filters"][0]["score"]
            .as_f64();
        assert!(
            score.is_some_and(|score| (0.0..=1.0).contains(&score)),
            "case {case:?}: the filter must report the score it computed, got {applied}"
        );
    }

    // The sensitive-information policy, configured to MASK: the value is reported and
    // the content comes back with it replaced.
    let (status, masking) = create_guardrail(
        &router,
        "masking-guardrail",
        json!({
            "sensitiveInformationPolicyConfig": {
                "piiEntitiesConfig": [{ "type": "EMAIL", "action": "ANONYMIZE" }],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the guardrail must be created, got {status} / {masking}"
    );
    let masking_id = masking["guardrailId"]
        .as_str()
        .expect("the control plane mints a guardrailId")
        .to_string();
    let (status, applied) = apply_guardrail(
        &router,
        &masking_id,
        "DRAFT",
        "INPUT",
        "Write to dana.reed@example.com about the order.",
    )
    .await;
    assert_eq!(
        status, 200,
        "the apply must be served, got {status} / {applied}"
    );
    assert_eq!(
        applied["action"].as_str(),
        Some("GUARDRAIL_INTERVENED"),
        "masking the content is an intervention, got {applied}"
    );
    assert_eq!(
        applied["outputs"][0]["text"].as_str(),
        Some("Write to {EMAIL} about the order."),
        "the masked content must carry the entity type in place of the value, got {applied}"
    );
    assert_eq!(
        applied["assessments"][0]["sensitiveInformationPolicy"]["piiEntities"][0]["action"]
            .as_str(),
        Some("ANONYMIZED"),
        "the placement must report the action the entry is configured with, got {applied}"
    );
}
