// SPDX-License-Identifier: CC0-1.0

//! `ApplyGuardrail`: a guardrail REVISION the control plane published is evaluated
//! against submitted content, policy by policy, end to end over the router.
//!
//! The rule under test: **applying a guardrail evaluates the submitted content
//! against every policy the named revision configures on the named side, reports each
//! entry it placed with what that entry does, and answers the revision's blocked
//! messaging where an entry that blocks matched.** Everything it reports — the
//! action, the per-policy entries, the text units each policy read and the coverage
//! of the submission — is computed from the request and the stored revision, so a
//! revision that configures a policy differently is answered differently.
//!
//! Each suite asserts that rule over a SET of inputs, differing along what the rule
//! generalises over: which policy is configured, what side it is configured on, what
//! the entry's action is, whether the text carries what the policy places, the output
//! scope asked for, and the version of the guardrail that is applied. The guardrail
//! is published through the very function the Bedrock control plane publishes one
//! with ([`state::guardrail_registry::publish`]), so what these suites apply is what
//! a created guardrail is; the two planes are driven together in
//! `core/router/tests/all/bedrock_guardrail_applied_across_planes.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::guardrail_registry::{
    DeniedTopic, FilterEntry, GroundingEntry, GuardrailPolicies, ManagedWordEntry, PiiEntry,
    PolicyAction, PublishedGuardrail, RegexEntry, Sides, WordEntry, DRAFT_VERSION,
    GROUNDING_FILTER_GROUNDING, GROUNDING_FILTER_RELEVANCE, MANAGED_WORDS_PROFANITY,
    TOPIC_TYPE_DENY,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_runtime_inference::{post_json, Call};

/// The guardrail every suite here applies.
const GUARDRAIL_ID: &str = "abcd1234efgh";

/// What the guardrail answers content it blocked on each side with.
const BLOCKED_INPUT: &str = "Sorry, the model cannot answer this question.";
const BLOCKED_OUTPUT: &str = "Sorry, the model cannot answer with this text.";

/// The emulator under test: the production router with this service mounted on a
/// shared engine, plus the engine itself so a row can publish the guardrail the
/// control plane would have published.
fn emulator_with_engine() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// The guardrail's ARN, as the control plane mints one
/// (`arn:aws:bedrock:<region>:<account>:guardrail/<id>`).
fn guardrail_arn() -> String {
    engine::emulator_arn("bedrock", "us-east-1", &format!("guardrail/{GUARDRAIL_ID}"))
}

/// A `guardrailIdentifier` as it travels in the request URI: the label is one path
/// segment, so the separators of an ARN are percent-encoded, which is what an SDK
/// sends.
fn encoded(identifier: &str) -> String {
    identifier.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

/// A revision carrying only the two blocked-messaging strings, which every row then
/// configures the policy it is about.
fn revision() -> GuardrailPolicies {
    GuardrailPolicies {
        blocked_input_messaging: BLOCKED_INPUT.to_string(),
        blocked_outputs_messaging: BLOCKED_OUTPUT.to_string(),
        ..GuardrailPolicies::default()
    }
}

/// Publish `policies` as the guardrail's DRAFT, the way the control plane publishes a
/// created guardrail.
fn publish(engine: &engine::InMemoryEngine, policies: GuardrailPolicies) {
    publish_revisions(engine, BTreeMap::from([(DRAFT_VERSION.to_string(), policies)]));
}

/// Publish a whole set of revisions under the guardrail.
fn publish_revisions(
    engine: &engine::InMemoryEngine,
    revisions: BTreeMap<String, GuardrailPolicies>,
) {
    assert!(
        state::guardrail_registry::publish(
            engine,
            PublishedGuardrail {
                id: GUARDRAIL_ID.to_string(),
                arn: guardrail_arn(),
                revisions,
            }
        ),
        "the guardrail must be publishable on the engine the service holds"
    );
}

/// `ApplyGuardrail` on one revision, over the operation's own wire
/// (`POST /guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply`).
async fn apply(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    body: &Value,
) -> Call {
    post_json(
        router,
        &format!("/guardrail/{identifier}/version/{version}/apply"),
        body,
    )
    .await
}

/// The body of an apply of one piece of text on one side.
fn text_body(source: &str, text: &str, scope: Option<&str>) -> Value {
    let mut body = json!({
        "source": source,
        "content": [{ "text": { "text": text } }],
    });
    if let Some(scope) = scope {
        body["outputScope"] = json!(scope);
    }
    body
}

/// The single assessment an apply answers with.
fn assessment(call: &Call) -> Value {
    let body = call.json();
    let assessments = body["assessments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        assessments.len(),
        1,
        "one apply of one revision reports one assessment, got {body}"
    );
    assessments[0].clone()
}

/// The `action` an apply answered with.
fn action_of(call: &Call) -> String {
    call.json()["action"].as_str().unwrap_or_default().to_string()
}

/// The text the guardrail answered with, block by block.
fn outputs_of(call: &Call) -> Vec<String> {
    call.json()["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|output| output["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn ok(case: &str, call: &Call) {
    assert_eq!(
        call.status,
        200,
        "case {case:?}: the apply must be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
}

/// RULE: a word the word policy configures is placed as a WORD, whatever its case and
/// wherever it sits, and the placement carries what that entry DOES — blocking the
/// content, or reporting the detection and leaving it.
#[tokio::test]
async fn the_word_policy_places_each_configured_word_and_acts_as_it_is_configured() {
    // (case, the entry's configured action, the text, placed, blocked)
    let rows: [(&str, Option<&str>, &str, bool, bool); 8] = [
        ("the word alone", None, "zephyrine", true, true),
        (
            "inside a sentence",
            None,
            "please describe zephyrine for me",
            true,
            true,
        ),
        ("capitalised", None, "Zephyrine is the token", true, true),
        (
            "part of a longer word",
            None,
            "zephyrines are different",
            false,
            false,
        ),
        (
            "a configured phrase",
            None,
            "pass me the Red Pencil please",
            true,
            true,
        ),
        (
            "nothing configured in it",
            None,
            "a benign question about tokens",
            false,
            false,
        ),
        (
            "detection without blocking",
            Some("NONE"),
            "describe zephyrine",
            true,
            false,
        ),
        (
            "detection without blocking, nothing in it",
            Some("NONE"),
            "a benign question",
            false,
            false,
        ),
    ];
    for (case, configured, text, placed, blocked) in rows {
        let (engine, router) = emulator_with_engine();
        let sides = Sides::new(configured, None, configured, None, PolicyAction::Block);
        publish(
            &engine,
            GuardrailPolicies {
                words: vec![
                    WordEntry {
                        text: "zephyrine".to_string(),
                        sides,
                    },
                    WordEntry {
                        text: "red pencil".to_string(),
                        sides,
                    },
                ],
                ..revision()
            },
        );

        let call = apply(&router, GUARDRAIL_ID, DRAFT_VERSION, &text_body("INPUT", text, None)).await;
        ok(case, &call);
        let expected_action = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        let actual = action_of(&call);
        assert_eq!(
            actual, expected_action,
            "case {case:?}: expected action {expected_action}, got {actual:?} in {}",
            call.json()
        );
        let words = assessment(&call)["wordPolicy"]["customWords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            !words.is_empty(),
            placed,
            "case {case:?}: expected a word placement={placed}, got {words:?}"
        );
        if placed {
            let entry = &words[0];
            let matched = entry["match"].as_str().unwrap_or_default().to_lowercase();
            assert!(
                text.to_lowercase().contains(&matched),
                "case {case:?}: the placement must report the text it matched, got {entry}"
            );
            let expected_entry_action = if blocked { "BLOCKED" } else { "NONE" };
            assert_eq!(
                entry["action"].as_str().unwrap_or_default(),
                expected_entry_action,
                "case {case:?}: expected entry action {expected_entry_action}, got {entry}"
            );
            assert_eq!(
                entry["detected"].as_bool(),
                Some(true),
                "case {case:?}: a reported placement is a detection, got {entry}"
            );
        }
        let expected_outputs: Vec<String> = if blocked {
            vec![BLOCKED_INPUT.to_string()]
        } else {
            Vec::new()
        };
        let actual_outputs = outputs_of(&call);
        assert_eq!(
            actual_outputs, expected_outputs,
            "case {case:?}: expected outputs {expected_outputs:?}, got {actual_outputs:?}"
        );
    }
}

/// RULE: a policy entry is evaluated, and acts, only on the SIDE its configuration
/// enables — so the same text is blocked on one side and passed on the other, and the
/// messaging answered is that side's.
#[tokio::test]
async fn an_entry_acts_only_on_the_side_it_is_configured_on() {
    // (case, the side the entry blocks, the side applied, blocked, messaging)
    let rows: [(&str, bool, &str, bool, &str); 4] = [
        ("input entry, input applied", true, "INPUT", true, BLOCKED_INPUT),
        ("input entry, output applied", true, "OUTPUT", false, ""),
        (
            "output entry, output applied",
            false,
            "OUTPUT",
            true,
            BLOCKED_OUTPUT,
        ),
        ("output entry, input applied", false, "INPUT", false, ""),
    ];
    for (case, blocks_input, source, blocked, messaging) in rows {
        let (engine, router) = emulator_with_engine();
        let sides = Sides {
            input: blocks_input.then_some(PolicyAction::Block),
            output: (!blocks_input).then_some(PolicyAction::Block),
        };
        publish(
            &engine,
            GuardrailPolicies {
                words: vec![WordEntry {
                    text: "zephyrine".to_string(),
                    sides,
                }],
                ..revision()
            },
        );

        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &text_body(source, "tell me about zephyrine", None),
        )
        .await;
        ok(case, &call);
        let expected = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        let actual = action_of(&call);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected action {expected}, got {actual:?} in {}",
            call.json()
        );
        let expected_outputs: Vec<String> = if blocked {
            vec![messaging.to_string()]
        } else {
            Vec::new()
        };
        let actual_outputs = outputs_of(&call);
        assert_eq!(
            actual_outputs, expected_outputs,
            "case {case:?}: expected outputs {expected_outputs:?}, got {actual_outputs:?}"
        );
    }
}

/// RULE: the `INTERVENTIONS` output scope — which is also what a request naming no
/// scope gets — reports the entries the evaluation PLACED and drops the policies it
/// placed nothing of, while `FULL` reports every entry the policies evaluated with
/// whether it was placed.
#[tokio::test]
async fn the_output_scope_decides_whether_undetected_policies_are_reported() {
    // (case, the scope asked for, the policies the assessment must carry)
    let rows: [(&str, Option<&str>, &[&str]); 3] = [
        ("no scope named", None, &[]),
        ("interventions", Some("INTERVENTIONS"), &[]),
        (
            "full",
            Some("FULL"),
            &["topicPolicy", "contentPolicy", "sensitiveInformationPolicy"],
        ),
    ];
    for (case, scope, reported) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                topics: vec![DeniedTopic {
                    name: "Fiduciary Advice".to_string(),
                    kind: TOPIC_TYPE_DENY.to_string(),
                    examples: vec!["Where should I invest my savings?".to_string()],
                    sides: Sides::both(PolicyAction::Block),
                }],
                content_filters: vec![FilterEntry {
                    category: "VIOLENCE".to_string(),
                    input_strength: "HIGH".to_string(),
                    output_strength: "HIGH".to_string(),
                    sides: Sides::both(PolicyAction::Block),
                }],
                pii_entities: vec![PiiEntry {
                    entity_type: "EMAIL".to_string(),
                    sides: Sides::both(PolicyAction::Block),
                }],
                words: vec![WordEntry {
                    text: "zephyrine".to_string(),
                    sides: Sides::both(PolicyAction::Block),
                }],
                ..revision()
            },
        );

        // Text no configured policy places anything in.
        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &text_body("INPUT", "What time does the library open?", scope),
        )
        .await;
        ok(case, &call);
        let assessed = assessment(&call);
        let actual = action_of(&call);
        assert_eq!(
            actual, "NONE",
            "case {case:?}: nothing was placed, so the guardrail acted on nothing, got {actual:?}"
        );
        for policy in [
            "topicPolicy",
            "contentPolicy",
            "wordPolicy",
            "sensitiveInformationPolicy",
        ] {
            let present = assessed.get(policy).is_some_and(|value| !value.is_null());
            let expected = reported.contains(&policy);
            assert_eq!(
                present, expected,
                "case {case:?}: expected {policy} reported={expected}, got {assessed}"
            );
        }
        if scope == Some("FULL") {
            let topics = assessed["topicPolicy"]["topics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                topics.first().and_then(|topic| topic["detected"].as_bool()),
                Some(false),
                "case {case:?}: the full scope reports an unplaced entry as undetected, got \
                 {assessed}"
            );
            // The action an assessment reports is the action TAKEN, so an entry
            // nothing was placed of carries that nothing was done, whatever it is
            // configured to do.
            assert_eq!(
                topics.first().and_then(|topic| topic["action"].as_str()),
                Some("NONE"),
                "case {case:?}: an unplaced entry had no action taken on it, got {assessed}"
            );
        }
        // Whatever the scope, the assessment names the revision that was applied and
        // reports what the evaluation cost: they are facts about the evaluation
        // rather than entries it placed in the content.
        let applied = &assessed["appliedGuardrailDetails"];
        assert_eq!(
            applied["guardrailId"].as_str(),
            Some(GUARDRAIL_ID),
            "case {case:?}: the assessment must name the guardrail applied, got {assessed}"
        );
        assert_eq!(
            applied["guardrailVersion"].as_str(),
            Some(DRAFT_VERSION),
            "case {case:?}: the assessment must name the revision applied, got {assessed}"
        );
        assert_eq!(
            applied["guardrailArn"].as_str(),
            Some(guardrail_arn().as_str()),
            "case {case:?}: the assessment must name the guardrail's ARN, got {assessed}"
        );
        assert_eq!(
            applied["guardrailOrigin"][0].as_str(),
            Some("REQUEST"),
            "case {case:?}: a guardrail the request named is applied because it asked, got \
             {assessed}"
        );
        let metrics = &assessed["invocationMetrics"];
        assert_eq!(
            metrics["guardrailCoverage"],
            call.json()["guardrailCoverage"],
            "case {case:?}: the metrics report the coverage of this evaluation, got {assessed}"
        );
        assert_eq!(
            metrics["usage"],
            call.json()["usage"],
            "case {case:?}: the metrics report the units this evaluation read, got {assessed}"
        );
        assert!(
            metrics["guardrailProcessingLatency"]
                .as_i64()
                .is_some_and(|latency| latency >= 0),
            "case {case:?}: the metrics report how long the evaluation took, got {assessed}"
        );
    }
}

/// RULE: a contextual grounding filter SCORES the guarded content against the
/// material the request carries for it — the grounding source, and the query — and
/// acts on content that scores BELOW the threshold it is configured with.
#[tokio::test]
async fn contextual_grounding_scores_the_content_and_acts_below_the_threshold() {
    const SOURCE: &str = "The Mars Reconnaissance Orbiter reached Mars in 2006 and still \
                          photographs the surface.";
    const QUERY: &str = "When did the Mars Reconnaissance Orbiter reach Mars?";
    // (case, the threshold, the answer, blocked)
    let rows: [(&str, f64, &str, bool); 4] = [
        (
            "an answer taken from the source",
            0.8,
            "The Mars Reconnaissance Orbiter reached Mars in 2006.",
            false,
        ),
        (
            "an answer the source does not support",
            0.8,
            "Voyager flew past Neptune during a storm.",
            true,
        ),
        (
            "the same unsupported answer, under a threshold nothing fails",
            0.0,
            "Voyager flew past Neptune during a storm.",
            false,
        ),
        (
            "a supported answer, under a threshold only a perfect score passes",
            1.0,
            "The orbiter photographs the martian surface.",
            true,
        ),
    ];
    for (case, threshold, answer, blocked) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                grounding_filters: vec![GroundingEntry {
                    kind: GROUNDING_FILTER_GROUNDING.to_string(),
                    threshold,
                    action: Some(PolicyAction::Block),
                }],
                ..revision()
            },
        );

        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &json!({
                "source": "OUTPUT",
                "outputScope": "FULL",
                "content": [
                    { "text": { "text": SOURCE, "qualifiers": ["grounding_source"] } },
                    { "text": { "text": QUERY, "qualifiers": ["query"] } },
                    { "text": { "text": answer, "qualifiers": ["guard_content"] } },
                ],
            }),
        )
        .await;
        ok(case, &call);
        let assessed = assessment(&call);
        let filters = assessed["contextualGroundingPolicy"]["filters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            filters.len(),
            1,
            "case {case:?}: the configured filter must be reported, got {assessed}"
        );
        let filter = &filters[0];
        let score = filter["score"].as_f64().unwrap_or(-1.0);
        assert!(
            (0.0..=1.0).contains(&score),
            "case {case:?}: the score must be a modelled 0..1 score, got {filter}"
        );
        assert_eq!(
            filter["threshold"].as_f64(),
            Some(threshold),
            "case {case:?}: the filter reports the threshold it is configured with, got {filter}"
        );
        assert_eq!(
            filter["detected"].as_bool(),
            Some(score < threshold),
            "case {case:?}: a filter acts exactly below its threshold, got {filter}"
        );
        // The assessment reports the action TAKEN: the configured one where the
        // filter acted, and none where it did not.
        let expected_entry_action = if score < threshold { "BLOCKED" } else { "NONE" };
        assert_eq!(
            filter["action"].as_str(),
            Some(expected_entry_action),
            "case {case:?}: expected entry action {expected_entry_action}, got {filter}"
        );
        let expected = if blocked {
            "GUARDRAIL_INTERVENED"
        } else {
            "NONE"
        };
        let actual = action_of(&call);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected action {expected} for score {score}, got {actual:?}"
        );
        // The units the policy read are charged to the policy that read them, and to
        // no other.
        let usage = call.json()["usage"].clone();
        assert_eq!(
            usage["contextualGroundingPolicyUnits"].as_i64(),
            Some(1),
            "case {case:?}: the configured policy read the submitted text, got {usage}"
        );
        assert_eq!(
            usage["wordPolicyUnits"].as_i64(),
            Some(0),
            "case {case:?}: a policy the revision does not configure reads nothing, got {usage}"
        );
    }
}

/// RULE: a request whose guardrail configures a contextual grounding policy and which
/// carries none of the material that policy scores against is refused
/// `400 ValidationException` — a score of nothing is not a verdict.
#[tokio::test]
async fn an_apply_without_the_grounding_material_is_refused() {
    // (case, the filters configured, the content, refused, the qualifier the refusal
    // must name)
    let rows: [(&str, &[&str], Value, bool, &str); 5] = [
        (
            "grounding configured, no grounding source",
            &[GROUNDING_FILTER_GROUNDING],
            json!([{ "text": { "text": "An answer with no source.", "qualifiers": ["guard_content"] } }]),
            true,
            "grounding source",
        ),
        (
            "grounding configured, only a query",
            &[GROUNDING_FILTER_GROUNDING],
            json!([
                { "text": { "text": "Which orbiter?", "qualifiers": ["query"] } },
                { "text": { "text": "An answer with no source.", "qualifiers": ["guard_content"] } },
            ]),
            true,
            "grounding source",
        ),
        (
            "relevance configured, a source but no query",
            &[GROUNDING_FILTER_RELEVANCE],
            json!([
                { "text": { "text": "The orbiter reached Mars in 2006.", "qualifiers": ["grounding_source"] } },
                { "text": { "text": "It reached Mars in 2006.", "qualifiers": ["guard_content"] } },
            ]),
            true,
            "query",
        ),
        (
            "both configured, both carried",
            &[GROUNDING_FILTER_GROUNDING, GROUNDING_FILTER_RELEVANCE],
            json!([
                { "text": { "text": "The orbiter reached Mars in 2006.", "qualifiers": ["grounding_source"] } },
                { "text": { "text": "When did it reach Mars?", "qualifiers": ["query"] } },
                { "text": { "text": "It reached Mars in 2006.", "qualifiers": ["guard_content"] } },
            ]),
            false,
            "",
        ),
        (
            "no contextual grounding policy at all",
            &[],
            json!([{ "text": { "text": "An answer with no source." } }]),
            false,
            "",
        ),
    ];
    for (case, configured, content, refused, qualifier) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                grounding_filters: configured
                    .iter()
                    .map(|kind| GroundingEntry {
                        kind: (*kind).to_string(),
                        threshold: 0.5,
                        action: Some(PolicyAction::Block),
                    })
                    .collect(),
                ..revision()
            },
        );

        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &json!({ "source": "OUTPUT", "content": content }),
        )
        .await;
        if !refused {
            ok(case, &call);
            continue;
        }
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        let message = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message,
            state::guardrail_evaluation::missing_material_message(qualifier),
            "case {case:?}: the refusal must name the material the request is missing \
             ({qualifier}), got {message:?}"
        );
        assert!(
            message.contains(qualifier),
            "case {case:?}: the refusal must name the material the request is missing \
             ({qualifier}), got {message:?}"
        );
    }
}

/// RULE: the revision a request applies is the one its identifier and version name —
/// either spelling of the identifier resolves the same guardrail, and an identifier
/// or version naming a revision this account does not hold is refused
/// `400 ValidationException`.
#[tokio::test]
async fn an_apply_resolves_the_revision_its_identifier_and_version_name() {
    let (engine, router) = emulator_with_engine();
    // The DRAFT blocks one word; version 1 blocks another. Applying each must apply
    // ITS policies.
    let word = |text: &str| GuardrailPolicies {
        words: vec![WordEntry {
            text: text.to_string(),
            sides: Sides::both(PolicyAction::Block),
        }],
        ..revision()
    };
    publish_revisions(
        &engine,
        BTreeMap::from([
            (DRAFT_VERSION.to_string(), word("zephyrine")),
            ("1".to_string(), word("quillon")),
        ]),
    );
    let arn = encoded(&guardrail_arn());

    // (case, identifier, version, text, status, action)
    let rows: Vec<(&str, String, &str, &str, u16, &str)> = vec![
        (
            "the draft, by the bare id",
            GUARDRAIL_ID.to_string(),
            DRAFT_VERSION,
            "about zephyrine",
            200,
            "GUARDRAIL_INTERVENED",
        ),
        (
            "the draft does not carry version 1's word",
            GUARDRAIL_ID.to_string(),
            DRAFT_VERSION,
            "about quillon",
            200,
            "NONE",
        ),
        (
            "version 1, by the bare id",
            GUARDRAIL_ID.to_string(),
            "1",
            "about quillon",
            200,
            "GUARDRAIL_INTERVENED",
        ),
        (
            "version 1, by the ARN",
            arn.clone(),
            "1",
            "about quillon",
            200,
            "GUARDRAIL_INTERVENED",
        ),
        (
            "the draft, by the ARN",
            arn,
            DRAFT_VERSION,
            "about zephyrine",
            200,
            "GUARDRAIL_INTERVENED",
        ),
        (
            "a version that was never cut",
            GUARDRAIL_ID.to_string(),
            "7",
            "about zephyrine",
            400,
            "",
        ),
        (
            "a guardrail this account does not hold",
            "ffff0000ffff".to_string(),
            DRAFT_VERSION,
            "about zephyrine",
            400,
            "",
        ),
    ];
    for (case, identifier, version, text, status, expected) in rows {
        let call = apply(
            &router,
            &identifier,
            version,
            &text_body("INPUT", text, None),
        )
        .await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        if status == 400 {
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                call.error_code()
            );
            assert_eq!(
                call.json()["message"].as_str(),
                Some(state::guardrail_registry::GUARDRAIL_NOT_FOUND),
                "case {case:?}: the refusal must say that the revision the request named is \
                 not held, got {}",
                call.json()
            );
            continue;
        }
        let actual = action_of(&call);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected action {expected}, got {actual:?} in {}",
            call.json()
        );
    }
}

/// RULE: the sensitive-information policy places values with the entity types it
/// configures, and acts as each entry says: an entry that BLOCKS answers the blocked
/// messaging, one that ANONYMIZES answers the content with the value masked, and one
/// configured for neither reports the placement and leaves the content.
#[tokio::test]
async fn the_sensitive_information_policy_places_values_and_acts_as_configured() {
    const TEXT: &str = "Write to dana.reed@example.com about the order.";
    // (case, the entry's action, the action answered, the text answered with)
    let rows: [(&str, PolicyAction, &str, &[&str]); 3] = [
        (
            "an entry that blocks",
            PolicyAction::Block,
            "GUARDRAIL_INTERVENED",
            &[BLOCKED_INPUT],
        ),
        (
            "an entry that anonymizes",
            PolicyAction::Anonymize,
            "GUARDRAIL_INTERVENED",
            &["Write to {EMAIL} about the order."],
        ),
        (
            "an entry that only reports",
            PolicyAction::Detect,
            "NONE",
            &[],
        ),
    ];
    for (case, configured, expected_action, expected_outputs) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                pii_entities: vec![PiiEntry {
                    entity_type: "EMAIL".to_string(),
                    sides: Sides::both(configured),
                }],
                ..revision()
            },
        );

        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &text_body("INPUT", TEXT, None),
        )
        .await;
        ok(case, &call);
        let actual = action_of(&call);
        assert_eq!(
            actual, expected_action,
            "case {case:?}: expected action {expected_action}, got {actual:?} in {}",
            call.json()
        );
        let entries = assessment(&call)["sensitiveInformationPolicy"]["piiEntities"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the value in the text must be placed once, got {}",
            call.json()
        );
        assert_eq!(
            entries[0]["match"].as_str(),
            Some("dana.reed@example.com"),
            "case {case:?}: the placement reports the value it matched, got {}",
            entries[0]
        );
        assert_eq!(
            entries[0]["type"].as_str(),
            Some("EMAIL"),
            "case {case:?}: the placement reports the entity type that placed it, got {}",
            entries[0]
        );
        let actual_outputs = outputs_of(&call);
        let expected_outputs: Vec<String> =
            expected_outputs.iter().map(|text| (*text).to_string()).collect();
        assert_eq!(
            actual_outputs, expected_outputs,
            "case {case:?}: expected outputs {expected_outputs:?}, got {actual_outputs:?}"
        );
    }
}

/// RULE: the `ConverseStream` surface applies the guardrail its request names
/// exactly as `Converse` does — a blocked turn streams that side's blocked messaging
/// and stops on the guardrail's stop reason, and the metadata event carries the
/// assessments where the request asked for the trace.
#[tokio::test]
async fn the_stream_surface_applies_the_guardrail_its_request_names() {
    // (case, the prompt, blocked)
    let rows: [(&str, &str, bool); 2] = [
        ("the configured word in the prompt", "tell me about zephyrine", true),
        (
            "nothing the policy configures",
            "tell me about page tokens",
            false,
        ),
    ];
    for (case, prompt, blocked) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                words: vec![WordEntry {
                    text: "zephyrine".to_string(),
                    sides: Sides::both(PolicyAction::Block),
                }],
                ..revision()
            },
        );

        let call = post_json(
            &router,
            "/model/amazon.nova-lite-v1:0/converse-stream",
            &json!({
                "messages": [{ "role": "user", "content": [{ "text": prompt }] }],
                "guardrailConfig": {
                    "guardrailIdentifier": GUARDRAIL_ID,
                    "guardrailVersion": DRAFT_VERSION,
                    "trace": "enabled",
                },
            }),
        )
        .await;
        ok(case, &call);
        let frames = crate::bedrock_runtime_streaming::decode_frames(case, &call.body);
        let event = |name: &str| {
            frames
                .iter()
                .find(|(event_type, _)| event_type == name)
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stream carried no {name} event: {frames:?}")
                })
        };
        // The turn arrives as the increments a client appends, so its text is the
        // deltas reassembled.
        let streamed: String = frames
            .iter()
            .filter(|(event_type, _)| event_type == "contentBlockDelta")
            .map(|(_, payload)| {
                payload
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let stop = event("messageStop");
        let expected_stop = if blocked {
            "guardrail_intervened"
        } else {
            "end_turn"
        };
        assert_eq!(
            stop["stopReason"].as_str(),
            Some(expected_stop),
            "case {case:?}: expected the stream to stop on {expected_stop}, got {stop}"
        );
        if blocked {
            assert_eq!(
                streamed, BLOCKED_INPUT,
                "case {case:?}: a blocked turn streams the guardrail's messaging, got \
                 {streamed:?}"
            );
        } else {
            assert!(
                !streamed.is_empty() && streamed != BLOCKED_INPUT,
                "case {case:?}: a turn the guardrail passed streams the turn itself, got \
                 {streamed:?}"
            );
        }
        // The trace the request asked for, carried in the metadata event.
        let metadata = event("metadata");
        let placed = metadata["trace"]["guardrail"]["inputAssessment"][GUARDRAIL_ID]["wordPolicy"]
            ["customWords"]
            .as_array()
            .map(|words| !words.is_empty())
            .unwrap_or_default();
        assert_eq!(
            placed, blocked,
            "case {case:?}: the trace must report what the word policy placed, got {}",
            metadata["trace"]
        );
    }
}

/// RULE: the work one request can ask for is bounded on BOTH its factors — the
/// characters of text submitted and the number of content blocks it arrives in — and
/// a request past either ceiling is refused rather than evaluated.
#[tokio::test]
async fn a_submission_past_either_ceiling_is_refused() {
    let (engine, router) = emulator_with_engine();
    publish(
        &engine,
        GuardrailPolicies {
            words: vec![WordEntry {
                text: "zephyrine".to_string(),
                sides: Sides::both(PolicyAction::Block),
            }],
            ..revision()
        },
    );
    let characters = state::guardrail_checks::MAX_SUBMITTED_CHARACTERS;
    let blocks = state::guardrail_checks::MAX_SUBMITTED_BLOCKS;

    // (case, the content submitted, the refusal message)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "one character past the text ceiling",
            json!([{ "text": { "text": "x".repeat(characters + 1) } }]),
            state::guardrail_checks::too_much_text_message(characters + 1),
        ),
        (
            "the grounding source past the text ceiling",
            json!([
                { "text": { "text": "x".repeat(characters), "qualifiers": ["grounding_source"] } },
                { "text": { "text": "y", "qualifiers": ["guard_content"] } },
            ]),
            state::guardrail_checks::too_much_text_message(characters + 1),
        ),
        (
            "one block past the block ceiling",
            Value::Array(
                (0..=blocks)
                    .map(|_| json!({ "text": { "text": "x" } }))
                    .collect(),
            ),
            state::guardrail_checks::too_many_blocks_message(blocks + 1),
        ),
    ];
    for (case, content, expected) in rows {
        let call = apply(
            &router,
            GUARDRAIL_ID,
            DRAFT_VERSION,
            &json!({ "source": "INPUT", "content": content }),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {}",
            call.status
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.json()["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: the refusal must name the ceiling it broke and by how much"
        );
    }

    // At the ceilings themselves, the submission is evaluated.
    let call = apply(
        &router,
        GUARDRAIL_ID,
        DRAFT_VERSION,
        &json!({
            "source": "INPUT",
            "content": Value::Array(
                (0..blocks).map(|_| json!({ "text": { "text": "x" } })).collect(),
            ),
        }),
    )
    .await;
    ok("at the block ceiling", &call);
}

/// RULE: the units an apply reports are the text units EACH configured policy read,
/// and a policy the revision does not configure reads none; the coverage reports the
/// characters that were guarded out of the characters submitted.
#[tokio::test]
async fn the_usage_and_coverage_report_what_each_policy_read() {
    const GUARDED: &str = "A short question about tokens.";
    const SOURCE: &str = "Reference material.";
    let (engine, router) = emulator_with_engine();
    publish(
        &engine,
        GuardrailPolicies {
            words: vec![WordEntry {
                text: "zephyrine".to_string(),
                sides: Sides::both(PolicyAction::Block),
            }],
            managed_word_lists: vec![ManagedWordEntry {
                kind: MANAGED_WORDS_PROFANITY.to_string(),
                sides: Sides::both(PolicyAction::Block),
            }],
            regexes: vec![RegexEntry {
                name: "order-id".to_string(),
                pattern: r"ORD-[0-9]{4}".to_string(),
                sides: Sides::both(PolicyAction::Block),
            }],
            ..revision()
        },
    );

    let call = apply(
        &router,
        GUARDRAIL_ID,
        DRAFT_VERSION,
        &json!({
            "source": "INPUT",
            "content": [
                { "text": { "text": SOURCE, "qualifiers": ["grounding_source"] } },
                { "text": { "text": GUARDED, "qualifiers": ["guard_content"] } },
            ],
        }),
    )
    .await;
    ok("usage", &call);
    let body = call.json();
    // (the member, the units it must report). A policy the revision configures read
    // the guarded text; one it does not configure read nothing. The regular
    // expressions of a sensitive-information policy are charged as the FREE units.
    let rows: [(&str, i64); 6] = [
        ("wordPolicyUnits", 1),
        ("sensitiveInformationPolicyFreeUnits", 1),
        ("topicPolicyUnits", 0),
        ("contentPolicyUnits", 0),
        ("sensitiveInformationPolicyUnits", 0),
        ("contextualGroundingPolicyUnits", 0),
    ];
    for (member, expected) in rows {
        let actual = body["usage"][member].as_i64();
        assert_eq!(
            actual,
            Some(expected),
            "case {member:?}: expected {expected} text units, got {}",
            body["usage"]
        );
    }
    let coverage = &body["guardrailCoverage"]["textCharacters"];
    assert_eq!(
        coverage["guarded"].as_i64(),
        Some(GUARDED.chars().count() as i64),
        "the guarded characters are the content the policies read, got {coverage}"
    );
    assert_eq!(
        coverage["total"].as_i64(),
        Some((GUARDED.chars().count() + SOURCE.chars().count()) as i64),
        "the total characters are every character submitted, got {coverage}"
    );
}
