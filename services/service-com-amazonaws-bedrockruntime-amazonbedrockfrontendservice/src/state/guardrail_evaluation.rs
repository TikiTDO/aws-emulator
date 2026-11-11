// SPDX-License-Identifier: CC0-1.0

//! APPLYING a guardrail revision: how submitted content is evaluated against the
//! policies a revision configures ([`super::guardrail_registry`]), and what the
//! evaluation reports.
//!
//! One stage serves both surfaces that apply a guardrail the REQUEST named — the
//! `ApplyGuardrail` call, which reports the assessment itself, and the Converse-API
//! turn, which reports it as a trace and ends on the guardrail's stop reason — so the
//! two cannot disagree about what a guardrail does to the same text.
//!
//! WHAT EACH POLICY IS EVALUATED BY. Every verdict below is computed in process from
//! the submitted text; none is read out of the request:
//!
//! * the WORD policy matches the configured words as WORDS, case-insensitively, with
//!   the same linear-time engine the account-enforced guardrail matches its own word
//!   list with ([`super::enforced_guardrail`]), and places its managed word list with
//!   the profanity engine this emulator already analyses toxicity with;
//! * the CONTENT policy is scored by the very check the inline guardrail checks serve
//!   their content filter and prompt-attack recognizer from
//!   ([`super::guardrail_checks::evaluate`]), and a category is DETECTED where the
//!   score reaches the strength its filter is configured at — the published table of
//!   filter strength against detection confidence;
//! * the SENSITIVE-INFORMATION policy places entities with the PII recognizer this
//!   emulator serves Comprehend's PII family from, so the two services cannot
//!   disagree about what a value is, and places its regular expressions with the
//!   same linear-time engine;
//! * the TOPIC policy places text against each denied topic's NAME and the sample
//!   phrases it was configured with. A topic's DEFINITION is natural language for a
//!   model to read and no model runs here, so it is not what a topic is placed by;
//! * the CONTEXTUAL GROUNDING policy SCORES the guarded content against the grounding
//!   source and the query the request carried ([`grounding_score`],
//!   [`relevance_score`]).
//!
//! WHAT THE RESULT IS. A policy entry that matched carries the action it is
//! configured with: an entry that BLOCKS makes the guardrail intervene and the
//! content is answered with the revision's blocked messaging, an entry that
//! ANONYMIZES leaves the content answered with the matched values masked, and an
//! entry that does neither is reported as a detection and changes nothing. An
//! evaluation that places nothing acts not at all.

use std::collections::{BTreeMap, BTreeSet};

use regex::{Regex, RegexBuilder};
use rustrict::{Censor, Type};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::contains_pii_entities::detect_pii_spans;

use super::enforced_guardrail::{blocking_severity, is_whole_word, FILTER_PROMPT_ATTACK};
use super::guardrail_checks::{
    self, CheckedText, RequestedChecks, CONTENT_FILTER_RESULT_ORDER, MILD_SEVERITY,
    MODERATE_SEVERITY, PROMPT_ATTACK_RESULT_ORDER, SENSITIVE_INFORMATION_ENTITY_TYPES,
    SEVERE_SEVERITY,
};
use super::guardrail_registry::{
    self, DeniedTopic, GuardrailPolicies, PolicyAction, ResolvedGuardrail, ASSESSED_NONE,
    GROUNDING_FILTER_GROUNDING, GROUNDING_FILTER_RELEVANCE, MANAGED_WORDS_PROFANITY,
};
use super::ConverseTurn;
use crate::ops::types::{
    AppliedGuardrailDetails, ContentBlock, GuardrailAssessment, GuardrailContentBlock,
    GuardrailContentFilter, GuardrailContentPolicyAssessment, GuardrailContextualGroundingFilter,
    GuardrailContextualGroundingPolicyAssessment, GuardrailConverseContentBlock, GuardrailCoverage,
    GuardrailCustomWord, GuardrailImageCoverage, GuardrailInvocationMetrics, GuardrailManagedWord,
    GuardrailPiiEntityFilter, GuardrailRegexFilter, GuardrailSensitiveInformationPolicyAssessment,
    GuardrailTextCharactersCoverage, GuardrailTopic, GuardrailTopicPolicyAssessment,
    GuardrailTraceAssessment, GuardrailUsage, GuardrailWordPolicyAssessment, Message,
    SystemContentBlock,
};

/// `GuardrailAction` — what the guardrail did to the content it was applied to.
pub const ACTION_INTERVENED: &str = "GUARDRAIL_INTERVENED";
pub const ACTION_NONE: &str = "NONE";

/// `GuardrailOutputScope` — how much of the evaluation the caller asked to be told.
///
/// `INTERVENTIONS`, which is also what a request naming no scope gets, reports the
/// entries the evaluation PLACED; `FULL` reports every entry the policies evaluated,
/// detected or not, "for enhanced debugging" (`ApplyGuardrailRequest$outputScope`).
pub const OUTPUT_SCOPE_FULL: &str = "FULL";
pub const OUTPUT_SCOPE_INTERVENTIONS: &str = "INTERVENTIONS";

/// `GuardrailContentSource` — which side of an invocation the submitted content is.
pub const CONTENT_SOURCE_INPUT: &str = "INPUT";
pub const CONTENT_SOURCE_OUTPUT: &str = "OUTPUT";

/// `GuardrailContentQualifier` — what a submitted text block IS.
///
/// A block qualified [`QUALIFIER_GROUNDING_SOURCE`] is the reference material the
/// contextual grounding policy scores against, one qualified [`QUALIFIER_QUERY`] is
/// the question it scores relevance against, and everything else — a block qualified
/// [`QUALIFIER_GUARD_CONTENT`] and a block carrying no qualifier at all — is the
/// content the policies guard.
pub const QUALIFIER_GROUNDING_SOURCE: &str = "grounding_source";
pub const QUALIFIER_QUERY: &str = "query";
pub const QUALIFIER_GUARD_CONTENT: &str = "guard_content";

/// `GuardrailContentFilterConfidence` — the detection confidence a content filter
/// reports, on the same scale `GuardrailContentFilterStrength` configures it at.
pub const CONFIDENCE_NONE: &str = "NONE";
pub const CONFIDENCE_LOW: &str = "LOW";
pub const CONFIDENCE_MEDIUM: &str = "MEDIUM";
pub const CONFIDENCE_HIGH: &str = "HIGH";

/// `GuardrailOrigin` — how the guardrail came to be applied. A guardrail the request
/// itself named is applied because the request asked for it, rather than because the
/// account or the organization enforces it.
pub const GUARDRAIL_ORIGIN_REQUEST: &str = "REQUEST";

/// `GuardrailOwnership` — whose guardrail was applied. The guardrails this plane can
/// apply are the ones this account holds.
pub const GUARDRAIL_OWNERSHIP_SELF: &str = "SELF";

/// What `actionReason` says of content a guardrail BLOCKED.
///
/// Amazon Bedrock states the reason for the action it took in one sentence naming
/// that action; a call the guardrail only masked, or only placed detections in,
/// blocked nothing and carries no reason at all.
pub const ACTION_REASON_BLOCKED: &str = "Guardrail blocked.";

/// How far an evaluation reads one of a topic's sample phrases in the text before the
/// text is on that topic.
///
/// A sample phrase is a FORM of the topic rather than a string to find: a request
/// phrases the same thing in its own words, so a phrase is read where the text
/// carries this proportion of the words that phrase is made of. It is a proportion
/// rather than an exact match so a rephrasing is still placed, and high enough that a
/// phrase sharing only its ordinary words with the text is not.
pub const TOPIC_PHRASE_COVERAGE: f64 = 0.8;

/// The most entries of one policy an evaluation reports.
///
/// The EMULATOR's bound: one request cannot render an unbounded number of placements
/// into one response. It is far above the placements of any realistic submission, and
/// the entries past it are dropped rather than the request being refused.
pub const MAX_POLICY_FINDINGS: usize = 100;

/// The content one application of a guardrail reads, as the submitted blocks say what
/// each piece IS.
#[derive(Debug, Clone, Default)]
pub struct GuardedContent {
    /// The blocks the policies guard, in submission order.
    pub guarded: Vec<String>,
    /// The blocks qualified as the grounding source.
    pub grounding_sources: Vec<String>,
    /// The blocks qualified as the query.
    pub queries: Vec<String>,
    /// How many image blocks were submitted. No image engine runs in this emulator,
    /// so an image is reported as submitted and NOT guarded rather than as evaluated.
    pub images: usize,
}

impl GuardedContent {
    /// Every character of text submitted, whatever part of the request it is.
    #[must_use]
    pub fn characters(&self) -> usize {
        [&self.guarded, &self.grounding_sources, &self.queries]
            .into_iter()
            .flatten()
            .map(|text| text.chars().count())
            .sum()
    }

    /// This content cut to the ceilings one evaluation reads at.
    ///
    /// EVERY part of it is cut, not only the content to guard: the grounding source
    /// and the query are read by the contextual grounding policy, so their size is
    /// caller-chosen work too, and the surfaces that do not refuse an oversized
    /// submission outright (the Converse turns) are bounded by this. Each part is cut
    /// through the one budget this plane reads text through
    /// ([`guardrail_checks::budgeted_texts`]), in characters and in pieces.
    #[must_use]
    pub fn budgeted(&self) -> Self {
        let cut = |texts: &[String]| guardrail_checks::budgeted_texts(texts.iter().map(String::as_str));
        Self {
            guarded: cut(&self.guarded),
            grounding_sources: cut(&self.grounding_sources),
            queries: cut(&self.queries),
            images: self.images,
        }
    }

    /// Whether any block was qualified as the grounding source.
    #[must_use]
    pub fn has_grounding_source(&self) -> bool {
        !self.grounding_sources.is_empty()
    }

    /// Whether any block was qualified as the query.
    #[must_use]
    pub fn has_query(&self) -> bool {
        !self.queries.is_empty()
    }
}

/// The submitted blocks of an `ApplyGuardrail` call, read as what each piece IS.
///
/// A text block's qualifiers say what it is: the grounding source and the query feed
/// the contextual grounding policy, and every other block — one marked
/// [`QUALIFIER_GUARD_CONTENT`] and one carrying no qualifier at all — is content the
/// policies guard. An image carries no text, so it is counted and not read.
#[must_use]
pub fn submitted_content(blocks: &[GuardrailContentBlock]) -> GuardedContent {
    let mut content = GuardedContent::default();
    for block in blocks {
        match block {
            GuardrailContentBlock::Text(text) => push_qualified(
                &text.text,
                text.qualifiers.as_deref().unwrap_or_default(),
                &mut content,
            ),
            GuardrailContentBlock::Image(_) => content.images += 1,
        }
    }
    content
}

/// Add one submitted text block to the part of the content its QUALIFIERS say it is.
///
/// The one reading of a block's qualifiers, shared by the two surfaces that carry
/// qualified blocks in two shapes (`GuardrailTextBlock` on the apply call,
/// `GuardrailConverseTextBlock` in a Converse request's `guardContent`), so the same
/// block means the same thing on both.
fn push_qualified(text: &str, qualifiers: &[String], content: &mut GuardedContent) {
    let qualified = |named: &str| qualifiers.iter().any(|value| value.as_str() == named);
    if qualified(QUALIFIER_GROUNDING_SOURCE) {
        content.grounding_sources.push(text.to_string());
    } else if qualified(QUALIFIER_QUERY) {
        content.queries.push(text.to_string());
    } else {
        content.guarded.push(text.to_string());
    }
}

/// Why the submitted content cannot be scored against the guardrail's contextual
/// grounding policy, or `None` where it can.
///
/// A contextual grounding filter scores the guarded content against material the
/// REQUEST carries: a `GROUNDING` filter against the reference text the submission
/// qualifies as the grounding source, and a `RELEVANCE` filter against the question
/// it qualifies as the query. So a request whose guardrail configures one of those
/// filters and carries none of that material is asking for a score of nothing, and
/// is refused rather than answered with a score that stands for nothing.
#[must_use]
pub fn missing_grounding_material(
    policies: &GuardrailPolicies,
    content: &GuardedContent,
) -> Option<String> {
    let configured = |kind: &str| {
        policies
            .grounding_filters
            .iter()
            .any(|filter| filter.kind == kind && filter.action.is_some())
    };
    if !policies.evaluates_grounding() {
        return None;
    }
    if !content.has_grounding_source() {
        return Some(missing_material_message(MATERIAL_GROUNDING_SOURCE));
    }
    if configured(GROUNDING_FILTER_RELEVANCE) && !content.has_query() {
        return Some(missing_material_message(MATERIAL_QUERY));
    }
    None
}

/// The pieces of a contextual grounding evaluation, as the refusal below names them.
pub const MATERIAL_GROUNDING_SOURCE: &str = "grounding source";
pub const MATERIAL_QUERY: &str = "query";

/// What this plane answers a request whose guardrail configures a contextual
/// grounding policy and which carries none of the material named by `material`.
///
/// Amazon Bedrock names the piece that is MISSING and then states what such an
/// evaluation needs, so the caller learns both what was wrong and what to send; the
/// missing piece is the variable part.
#[must_use]
pub fn missing_material_message(material: &str) -> String {
    format!(
        "The provided request does not contain the {material}. Grounding source, query and \
         content to guard are required for Guardrails contextual grounding policy evaluation."
    )
}

/// What one application of a guardrail did, and what it found.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// [`ACTION_INTERVENED`] where a policy entry blocked or masked the content,
    /// [`ACTION_NONE`] otherwise.
    pub action: &'static str,
    /// Whether an entry that BLOCKS matched, which is what the blocked messaging is
    /// answered for.
    pub blocked: bool,
    /// `actionReason` — set only for content that was blocked.
    pub action_reason: Option<String>,
    /// The assessment, cut to the scope the request asked for.
    pub assessment: GuardrailAssessment,
    /// `outputs` — the text the guardrail answers with: the blocked messaging for
    /// content it blocked, the masked content for content it only anonymized, and
    /// nothing at all where it acted on nothing.
    pub outputs: Vec<String>,
    pub usage: GuardrailUsage,
    pub coverage: GuardrailCoverage,
}

/// Apply the revision `applied` resolved to, to `content`, on one side of an
/// invocation.
///
/// `input` says which side: a policy entry is evaluated, and acts, only on the sides
/// its configuration enables. `full_scope` reports every entry the policies
/// evaluated rather than only the ones they placed.
///
/// Synchronous, CPU-bound work with no await of its own, over text whose length the
/// caller has already bounded.
#[must_use]
pub fn apply(
    applied: &ResolvedGuardrail,
    content: &GuardedContent,
    input: bool,
    full_scope: bool,
) -> Verdict {
    let started = ::std::time::Instant::now();
    let policies = &applied.policies;
    // What the caller submitted, which is what the coverage reports, against what
    // this evaluation READS: every part of the submission is cut to the ceilings one
    // evaluation reads at, so no part of a request can grow the work it does.
    let total_characters = content.characters();
    let content = content.budgeted();
    let guarded = &content.guarded;
    let guarded_characters: usize = guarded.iter().map(|text| text.chars().count()).sum();

    let topics = topic_findings(policies, guarded, input);
    let filters = content_filter_findings(policies, guarded, input);
    let words = word_findings(policies, guarded, input);
    let managed = managed_word_findings(policies, guarded, input);
    let (pii, pii_masking) = pii_findings(policies, guarded, input);
    let (regexes, regex_masking) = regex_findings(policies, guarded, input);
    let grounding = grounding_findings(policies, &content);

    // What the evaluation DID: a placement whose entry blocks makes the guardrail
    // block the content, one whose entry anonymizes leaves the content masked, and a
    // placement reported without an action of its own changes nothing.
    let blocked = topics.iter().any(Finding::blocks)
        || filters.iter().any(Finding::blocks)
        || words.iter().any(Finding::blocks)
        || managed.iter().any(Finding::blocks)
        || pii.iter().any(Finding::blocks)
        || regexes.iter().any(Finding::blocks)
        || grounding.iter().any(Finding::blocks);
    let anonymized = !blocked
        && (pii.iter().any(Finding::anonymizes) || regexes.iter().any(Finding::anonymizes));

    let units = guardrail_checks::text_units(guarded_characters);
    let charged = |evaluated: bool| if evaluated { units } else { 0 };
    let usage = GuardrailUsage {
        topic_policy_units: charged(policies.evaluates_topics(input)),
        content_policy_units: charged(policies.evaluates_content(input)),
        word_policy_units: charged(policies.evaluates_words(input)),
        sensitive_information_policy_units: charged(policies.evaluates_pii(input)),
        // Amazon Bedrock charges the regular-expression filters of a
        // sensitive-information policy at no cost ("Guardrails" in Amazon Bedrock
        // pricing), so the units they read are reported as the FREE ones.
        sensitive_information_policy_free_units: charged(policies.evaluates_regexes(input)),
        contextual_grounding_policy_units: charged(policies.evaluates_grounding()),
        content_policy_image_units: None,
        automated_reasoning_policy_units: None,
        automated_reasoning_policies: None,
    };
    let coverage = GuardrailCoverage {
        text_characters: Some(GuardrailTextCharactersCoverage {
            guarded: Some(count_i32(guarded_characters)),
            total: Some(count_i32(total_characters)),
        }),
        // Reported only where images were submitted at all, and reported as not
        // guarded: this emulator runs no image engine, so claiming an image was
        // evaluated would be claiming a verdict nothing reached.
        images: (content.images > 0).then(|| GuardrailImageCoverage {
            guarded: Some(0),
            total: Some(count_i32(content.images)),
        }),
    };

    let assessment = GuardrailAssessment {
        topic_policy: topic_assessment(&topics, full_scope),
        content_policy: content_assessment(&filters, full_scope),
        word_policy: word_assessment(&words, &managed),
        sensitive_information_policy: sensitive_information_assessment(
            policies, &pii, &regexes, input, full_scope,
        ),
        contextual_grounding_policy: grounding_assessment(&grounding),
        automated_reasoning_policy: None,
        // What the evaluation COST, beside what it found: the time it took, and the
        // units and coverage of the text it read. They are the metrics of this
        // evaluation rather than something it placed in the content, so they are
        // reported whatever output scope was asked for.
        invocation_metrics: Some(GuardrailInvocationMetrics {
            guardrail_processing_latency: Some(super::latency_ms(started.elapsed())),
            usage: Some(usage.clone()),
            guardrail_coverage: Some(coverage.clone()),
        }),
        applied_guardrail_details: Some(AppliedGuardrailDetails {
            guardrail_id: Some(applied.id.clone()),
            guardrail_version: Some(applied.version.clone()),
            guardrail_arn: Some(applied.arn.clone()),
            // The guardrail was named by the REQUEST, which is how it came to be
            // applied, and it is this account's own guardrail — the registry holds
            // the guardrails this account created.
            guardrail_origin: Some(vec![GUARDRAIL_ORIGIN_REQUEST.to_string()]),
            guardrail_ownership: Some(GUARDRAIL_OWNERSHIP_SELF.to_string()),
        }),
    };

    // The text the guardrail answers with. Blocking replaces the content whole;
    // masking leaves it, with every value an anonymizing entry matched replaced by
    // the name of what it is.
    let outputs = if blocked {
        vec![policies.blocked_messaging(input).to_string()]
    } else if anonymized {
        let mut maskings = pii_masking;
        maskings.extend(regex_masking);
        masked(guarded, &maskings)
    } else {
        Vec::new()
    };

    Verdict {
        action: if blocked || anonymized {
            ACTION_INTERVENED
        } else {
            ACTION_NONE
        },
        blocked,
        action_reason: blocked.then(|| ACTION_REASON_BLOCKED.to_string()),
        assessment,
        outputs,
        usage,
        coverage,
    }
}

/// What one entry of one policy, as this evaluation placed it: whether it matched,
/// and what it does where it does.
trait Finding {
    /// Whether the evaluation placed this entry.
    fn detected(&self) -> bool;
    /// What the entry does where it is placed.
    fn action(&self) -> PolicyAction;
    /// Whether this placement blocks the content.
    fn blocks(&self) -> bool {
        self.detected() && self.action().blocks()
    }
    /// Whether this placement masks the value it matched.
    fn anonymizes(&self) -> bool {
        self.detected() && self.action().anonymizes()
    }
    /// The action the assessment reports for this entry: an entry the evaluation
    /// placed carries what it DID, and one it placed nothing of carries that nothing
    /// was done — the assessment reports the action TAKEN, not the action the entry
    /// is configured with.
    fn assessed(&self) -> String {
        match self.detected() {
            true => self.action().assessed().to_string(),
            false => ASSESSED_NONE.to_string(),
        }
    }
}

/// A count as the modelled integer member carries it.
fn count_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

// ---------------------------------------------------------------------------
// The topic policy
// ---------------------------------------------------------------------------

/// One denied topic, as this evaluation placed it.
struct TopicFinding {
    name: String,
    kind: String,
    detected: bool,
    action: PolicyAction,
}

impl Finding for TopicFinding {
    fn detected(&self) -> bool {
        self.detected
    }
    fn action(&self) -> PolicyAction {
        self.action
    }
}

/// Each denied topic evaluated on this side, with whether the text is on it.
///
/// The text is read ONCE for the whole policy rather than once per topic: every
/// topic NAME goes into the one word matcher both guardrail paths scan with
/// ([`matched_words`]), and the words each piece carries are collected once and then
/// compared with each topic's sample phrases. So the work is linear in the submitted
/// text plus the configured topics, never their product.
fn topic_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> Vec<TopicFinding> {
    let evaluated: Vec<(&DeniedTopic, PolicyAction)> = policies
        .topics
        .iter()
        .filter_map(|topic| Some((topic, topic.sides.on(input)?)))
        .collect();
    if evaluated.is_empty() {
        return Vec::new();
    }
    let names: Vec<&str> = evaluated
        .iter()
        .map(|(topic, _)| topic.name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    // A topic's name is matched as a phrase of its own, so at most one placement per
    // configured name is what there is to find.
    let named: BTreeSet<String> = matched_words(&names, texts, names.len().max(1))
        .into_iter()
        .map(|matched| matched.to_lowercase())
        .collect();
    let carried: Vec<BTreeSet<String>> = texts.iter().map(|text| words(text)).collect();
    evaluated
        .into_iter()
        .map(|(topic, action)| TopicFinding {
            name: topic.name.clone(),
            kind: topic.kind.clone(),
            detected: named.contains(&topic.name.trim().to_lowercase())
                || topic.examples.iter().any(|example| {
                    let phrase = words(example);
                    carried
                        .iter()
                        .any(|words| coverage(&phrase, words) >= TOPIC_PHRASE_COVERAGE)
                }),
            action,
        })
        .collect()
}

/// The topic-policy assessment: the topics this evaluation reports.
fn topic_assessment(
    findings: &[TopicFinding],
    full_scope: bool,
) -> Option<GuardrailTopicPolicyAssessment> {
    let topics: Vec<GuardrailTopic> = findings
        .iter()
        .filter(|finding| full_scope || finding.detected)
        .map(|finding| GuardrailTopic {
            name: finding.name.clone(),
            r#type: finding.kind.clone(),
            action: finding.assessed(),
            detected: Some(finding.detected),
        })
        .collect();
    (!topics.is_empty()).then_some(GuardrailTopicPolicyAssessment { topics })
}

// ---------------------------------------------------------------------------
// The content policy
// ---------------------------------------------------------------------------

/// One content filter, as this evaluation placed it.
struct FilterFinding {
    category: String,
    /// The strength the filter is configured at on the side being evaluated.
    strength: String,
    /// The confidence the engine placed the category at.
    confidence: &'static str,
    detected: bool,
    action: PolicyAction,
}

impl Finding for FilterFinding {
    fn detected(&self) -> bool {
        self.detected
    }
    fn action(&self) -> PolicyAction {
        self.action
    }
}

/// Each configured content filter evaluated on this side, with the confidence its
/// category was placed at and whether that reaches the strength it filters at.
fn content_filter_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> Vec<FilterFinding> {
    let scored = content_filter_scores(policies, texts, input);
    policies
        .content_filters
        .iter()
        .filter_map(|filter| {
            let action = filter.sides.on(input)?;
            let severity = scored.get(filter.category.as_str()).copied().unwrap_or(0.0);
            let strength = filter.strength(input);
            Some(FilterFinding {
                category: filter.category.clone(),
                strength: strength.to_string(),
                confidence: confidence(severity),
                // A filter's strength decides how much of what the engine placed is
                // filtered: the strongest setting filters a category placed at any
                // confidence and the weakest only one placed at the highest, while
                // `NONE` filters nothing at all.
                detected: severity > 0.0
                    && blocking_severity(strength).is_some_and(|least| severity >= least),
                action,
            })
        })
        .collect()
}

/// The severity the engine placed on each category a filter of this side asks about.
///
/// The categories are asked about TOGETHER, in one evaluation by the check the inline
/// guardrail checks serve their content filter from, so the two surfaces cannot
/// disagree about what a message carries. `PROMPT_ATTACK` is the one modelled
/// category that check places with its prompt-attack recognizer rather than with the
/// content filter, so it is read off that result instead — at the strongest severity
/// any of its families was placed at, the filter being configured for the category as
/// a whole.
fn content_filter_scores(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> BTreeMap<&'static str, f64> {
    let configured: Vec<&str> = policies
        .content_filters
        .iter()
        .filter(|filter| filter.sides.on(input).is_some())
        .map(|filter| filter.category.as_str())
        .collect();
    category_severities(&configured, texts)
}

/// The severity the engine placed on each of `categories` in `texts`.
///
/// The ONE reading of the content-filter categories a guardrail policy configures,
/// shared with the account-enforced guardrail ([`super::enforced_guardrail`]) so the
/// two guardrail paths cannot place a category differently in the same text. The
/// categories are asked about TOGETHER, so the text is read once however many are
/// configured; a category the engine placed nothing of is absent from the result.
pub(super) fn category_severities(
    categories: &[&str],
    texts: &[String],
) -> BTreeMap<&'static str, f64> {
    let mut scores: BTreeMap<&'static str, f64> = BTreeMap::new();
    if categories.is_empty() || texts.is_empty() {
        return scores;
    }
    let named = |category: &str| categories.contains(&category);
    let filtered: BTreeSet<String> = CONTENT_FILTER_RESULT_ORDER
        .into_iter()
        .filter(|category| named(category))
        .map(str::to_string)
        .collect();
    let attacks: BTreeSet<String> = match named(FILTER_PROMPT_ATTACK) {
        true => PROMPT_ATTACK_RESULT_ORDER
            .into_iter()
            .map(str::to_string)
            .collect(),
        false => BTreeSet::new(),
    };
    let requested = RequestedChecks {
        content_filter: (!filtered.is_empty()).then_some(filtered),
        prompt_attack: (!attacks.is_empty()).then_some(attacks),
        sensitive_information: None,
    };
    let checked: Vec<CheckedText> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| CheckedText {
            message_index: index,
            content_index: 0,
            text: text.clone(),
        })
        .collect();
    let placed = guardrail_checks::evaluate(&requested, &checked);
    for finding in placed.content_filter.unwrap_or_default() {
        scores.insert(finding.category, finding.severity_score);
    }
    if let Some(strongest) = placed
        .prompt_attack
        .unwrap_or_default()
        .into_iter()
        .map(|finding| finding.severity_score)
        .max_by(f64::total_cmp)
    {
        scores.insert(FILTER_PROMPT_ATTACK, strongest);
    }
    scores
}

/// The confidence a placement at `severity` is reported at: the three levels the
/// engine distinguishes, and `NONE` for a category it placed nothing of.
fn confidence(severity: f64) -> &'static str {
    if severity >= SEVERE_SEVERITY {
        CONFIDENCE_HIGH
    } else if severity >= MODERATE_SEVERITY {
        CONFIDENCE_MEDIUM
    } else if severity >= MILD_SEVERITY {
        CONFIDENCE_LOW
    } else {
        CONFIDENCE_NONE
    }
}

/// The content-policy assessment: the filters this evaluation reports.
fn content_assessment(
    findings: &[FilterFinding],
    full_scope: bool,
) -> Option<GuardrailContentPolicyAssessment> {
    let filters: Vec<GuardrailContentFilter> = findings
        .iter()
        .filter(|finding| full_scope || finding.detected)
        .map(|finding| GuardrailContentFilter {
            r#type: finding.category.clone(),
            confidence: finding.confidence.to_string(),
            filter_strength: Some(finding.strength.clone()),
            action: finding.assessed(),
            detected: Some(finding.detected),
        })
        .collect();
    (!filters.is_empty()).then_some(GuardrailContentPolicyAssessment { filters })
}

// ---------------------------------------------------------------------------
// The word policy
// ---------------------------------------------------------------------------

/// One word the policy matched. A word policy reports only what it MATCHED, so a
/// finding of it is always a detection.
struct WordFinding {
    matched: String,
    /// The managed word list the match came from, or `None` for a configured word.
    list: Option<String>,
    action: PolicyAction,
}

impl Finding for WordFinding {
    fn detected(&self) -> bool {
        true
    }
    fn action(&self) -> PolicyAction {
        self.action
    }
}

/// The configured words the policy matched in `texts`.
///
/// A configured word matches as a WORD: a match whose neighbours are alphanumeric is
/// part of a longer word and is not the word that was configured. Matching ignores
/// case, and the match is reported as the text carried it.
fn word_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> Vec<WordFinding> {
    let configured: Vec<(String, PolicyAction)> = policies
        .words
        .iter()
        .filter_map(|word| Some((word.text.to_lowercase(), word.sides.on(input)?)))
        .collect();
    let needles: Vec<&str> = configured.iter().map(|(word, _)| word.as_str()).collect();
    matched_words(&needles, texts, MAX_POLICY_FINDINGS)
        .into_iter()
        .filter_map(|matched| {
            let folded = matched.to_lowercase();
            let (_, action) = configured.iter().find(|(word, _)| *word == folded)?;
            Some(WordFinding {
                matched,
                list: None,
                action: *action,
            })
        })
        .collect()
}

/// The words of `needles` that occur in `texts`, as the text carried them, at most
/// `limit` of them and each reported once.
///
/// The ONE word matcher both guardrail paths scan with, so a word the account
/// enforces and a word a request's guardrail configures are matched by the same rule:
/// a configured word matches as a WORD — a match whose neighbours are alphanumeric is
/// part of a longer word and is not the word that was configured — and matching
/// ignores case.
///
/// Every needle goes into ONE pattern, each ESCAPED so a word's own characters are
/// never read as pattern syntax, matched by the linear-time engine this crate already
/// matches untrusted text with: the text is scanned once however many words are
/// configured. The word-boundary rule is applied to the match rather than written
/// into the pattern, because a configured phrase may begin or end with punctuation of
/// its own.
pub(super) fn matched_words(needles: &[&str], texts: &[String], limit: usize) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if needles.is_empty() || limit == 0 {
        return found;
    }
    let pattern = needles
        .iter()
        .map(|needle| regex::escape(needle))
        .collect::<Vec<String>>()
        .join("|");
    // The word list is bounded in count and in bytes when it is published, so there
    // is no configuration a pattern this size cannot be built from; a pattern that
    // does not compile matches nothing and is reported where a developer can see it
    // rather than failing a request that was otherwise servable.
    let Some(matcher) = insensitive(&pattern) else {
        return found;
    };
    for text in texts {
        for matched in matcher.find_iter(text) {
            if !is_whole_word(text, matched.start(), matched.end()) {
                continue;
            }
            // A configured word is ONE placement however often, and however
            // capitalised, the text carries it; it is reported as the text first
            // carried it.
            if found
                .iter()
                .any(|reported| reported.eq_ignore_ascii_case(matched.as_str()))
            {
                continue;
            }
            found.push(matched.as_str().to_string());
            if found.len() >= limit {
                return found;
            }
        }
    }
    found
}

/// The managed-word-list placements in `texts`.
///
/// The list a guardrail can configure is the PROFANITY one, placed by the engine this
/// emulator already analyses toxicity with rather than by a copy of that list carried
/// here: each word of the text is read on its own, so the placement reports the word
/// it was.
fn managed_word_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> Vec<WordFinding> {
    let mut found: Vec<WordFinding> = Vec::new();
    for list in &policies.managed_word_lists {
        let Some(action) = list.sides.on(input) else {
            continue;
        };
        if list.kind != MANAGED_WORDS_PROFANITY {
            continue;
        }
        for text in texts {
            for word in text.split(|character: char| !character.is_alphanumeric()) {
                if word.is_empty() || !Censor::from_str(word).analyze().is(Type::PROFANE) {
                    continue;
                }
                if found
                    .iter()
                    .any(|reported| reported.matched == word && reported.list.as_deref() == Some(list.kind.as_str()))
                {
                    continue;
                }
                if found.len() >= MAX_POLICY_FINDINGS {
                    return found;
                }
                found.push(WordFinding {
                    matched: word.to_string(),
                    list: Some(list.kind.clone()),
                    action,
                });
            }
        }
    }
    found
}

/// The word-policy assessment.
///
/// A word policy reports what it MATCHED under either output scope: the full scope
/// "doesn't apply to word filters" (`ApplyGuardrailRequest$outputScope`), so a
/// configured word the text does not carry is reported by neither scope.
fn word_assessment(
    words: &[WordFinding],
    managed: &[WordFinding],
) -> Option<GuardrailWordPolicyAssessment> {
    if words.is_empty() && managed.is_empty() {
        return None;
    }
    Some(GuardrailWordPolicyAssessment {
        custom_words: words
            .iter()
            .map(|finding| GuardrailCustomWord {
                r#match: finding.matched.clone(),
                action: finding.action.assessed().to_string(),
                detected: Some(true),
            })
            .collect(),
        managed_word_lists: managed
            .iter()
            .map(|finding| GuardrailManagedWord {
                r#match: finding.matched.clone(),
                r#type: finding.list.clone().unwrap_or_default(),
                action: finding.action.assessed().to_string(),
                detected: Some(true),
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// The sensitive-information policy
// ---------------------------------------------------------------------------

/// One value the sensitive-information policy matched: by entity type, or by one of
/// the configured regular expressions.
struct ValueFinding {
    /// The value as the text carried it.
    matched: String,
    /// The entity type that placed it, or the regular expression's name.
    name: String,
    /// The pattern, for a value a regular expression placed.
    pattern: Option<String>,
    action: PolicyAction,
}

impl Finding for ValueFinding {
    fn detected(&self) -> bool {
        true
    }
    fn action(&self) -> PolicyAction {
        self.action
    }
}

/// One value an anonymizing entry matched, and what it is masked by.
#[derive(Debug, Clone)]
struct Masking {
    /// Which guarded block the value sits in.
    text: usize,
    /// Zero-based character offsets into that block, `end` one past the last
    /// character.
    begin: usize,
    end: usize,
    placeholder: String,
}

/// What the PII entity types the policy configures placed in `texts`, with the values
/// an anonymizing entry matched.
fn pii_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> (Vec<ValueFinding>, Vec<Masking>) {
    let mut findings: Vec<ValueFinding> = Vec::new();
    let mut maskings: Vec<Masking> = Vec::new();
    let configured: Vec<(&str, PolicyAction)> = policies
        .pii_entities
        .iter()
        .filter_map(|entity| Some((entity.entity_type.as_str(), entity.sides.on(input)?)))
        .collect();
    if configured.is_empty() {
        return (findings, maskings);
    }
    for (index, text) in texts.iter().enumerate() {
        for span in detect_pii_spans(text) {
            let Some(entity_type) = modelled_entity_type(span.entity_type) else {
                continue;
            };
            let Some((_, action)) = configured
                .iter()
                .find(|(configured, _)| *configured == entity_type)
            else {
                continue;
            };
            if findings.len() >= MAX_POLICY_FINDINGS {
                return (findings, maskings);
            }
            let value: String = text
                .chars()
                .skip(span.begin)
                .take(span.end.saturating_sub(span.begin))
                .collect();
            if action.anonymizes() {
                maskings.push(Masking {
                    text: index,
                    begin: span.begin,
                    end: span.end,
                    placeholder: format!("{{{entity_type}}}"),
                });
            }
            findings.push(ValueFinding {
                matched: value,
                name: entity_type.to_string(),
                pattern: None,
                action: *action,
            });
        }
    }
    (findings, maskings)
}

/// This service's entity type for a span the shared recognizer reported, or `None`
/// for a recognizer type a guardrail's PII catalog does not name.
///
/// The catalog a guardrail's sensitive-information policy is configured from
/// (`GuardrailPiiEntityType`) is the one the inline guardrail checks answer about, so
/// the mapping onto the recognizer's own names is read from there rather than copied.
fn modelled_entity_type(recognized: &str) -> Option<&'static str> {
    SENSITIVE_INFORMATION_ENTITY_TYPES
        .into_iter()
        .find(|(_, comprehend)| *comprehend == recognized)
        .map(|(modelled, _)| modelled)
}

/// What the regular expressions the policy configures matched in `texts`, with the
/// values an anonymizing entry matched.
fn regex_findings(
    policies: &GuardrailPolicies,
    texts: &[String],
    input: bool,
) -> (Vec<ValueFinding>, Vec<Masking>) {
    let mut findings: Vec<ValueFinding> = Vec::new();
    let mut maskings: Vec<Masking> = Vec::new();
    for entry in &policies.regexes {
        let Some(action) = entry.sides.on(input) else {
            continue;
        };
        // The pattern is the ACCOUNT's, matched by the linear-time engine this crate
        // matches untrusted text with: no configuration can make one request
        // backtrack. A pattern that does not compile matches nothing and is reported
        // where a developer can see it rather than failing a servable request.
        let matcher = match Regex::new(&entry.pattern) {
            Ok(matcher) => matcher,
            Err(error) => {
                ::tracing::warn!(
                    %error,
                    name = %entry.name,
                    "a guardrail regex filter could not be compiled"
                );
                continue;
            }
        };
        for (index, text) in texts.iter().enumerate() {
            for found in matcher.find_iter(text) {
                if findings.len() >= MAX_POLICY_FINDINGS {
                    return (findings, maskings);
                }
                if action.anonymizes() {
                    maskings.push(Masking {
                        text: index,
                        begin: text[..found.start()].chars().count(),
                        end: text[..found.end()].chars().count(),
                        placeholder: format!("{{{}}}", entry.name),
                    });
                }
                findings.push(ValueFinding {
                    matched: found.as_str().to_string(),
                    name: entry.name.clone(),
                    pattern: Some(entry.pattern.clone()),
                    action,
                });
            }
        }
    }
    (findings, maskings)
}

/// The sensitive-information assessment.
///
/// The entity-type entries are reported the way the scope says: the full scope "does
/// apply to … sensitive information with filters that can detect personally
/// identifiable information (PII)" (`ApplyGuardrailRequest$outputScope`), so a
/// configured entity type the text carries nothing of is reported there and nowhere
/// else. A REGEX is reported only where it matched, that same sentence excluding
/// "regex in sensitive information filters" from the full scope.
fn sensitive_information_assessment(
    policies: &GuardrailPolicies,
    pii: &[ValueFinding],
    regexes: &[ValueFinding],
    input: bool,
    full_scope: bool,
) -> Option<GuardrailSensitiveInformationPolicyAssessment> {
    let mut pii_entities: Vec<GuardrailPiiEntityFilter> = pii
        .iter()
        .map(|finding| GuardrailPiiEntityFilter {
            r#match: finding.matched.clone(),
            r#type: finding.name.clone(),
            action: finding.action.assessed().to_string(),
            detected: Some(true),
        })
        .collect();
    if full_scope {
        pii_entities.extend(undetected_pii(policies, pii, input));
    }
    let regexes: Vec<GuardrailRegexFilter> = regexes
        .iter()
        .map(|finding| GuardrailRegexFilter {
            name: Some(finding.name.clone()),
            r#match: Some(finding.matched.clone()),
            regex: finding.pattern.clone(),
            action: finding.action.assessed().to_string(),
            detected: Some(true),
        })
        .collect();
    if pii_entities.is_empty() && regexes.is_empty() {
        return None;
    }
    Some(GuardrailSensitiveInformationPolicyAssessment {
        pii_entities,
        regexes,
    })
}

/// The entity types the policy evaluates on this side but placed nothing of, which
/// the FULL output scope reports beside the placements.
fn undetected_pii(
    policies: &GuardrailPolicies,
    placed: &[ValueFinding],
    input: bool,
) -> Vec<GuardrailPiiEntityFilter> {
    policies
        .pii_entities
        .iter()
        .filter_map(|entry| {
            entry.sides.on(input)?;
            if placed
                .iter()
                .any(|finding| finding.name == entry.entity_type)
            {
                return None;
            }
            Some(GuardrailPiiEntityFilter {
                // The entry is reported as evaluated and not placed, so there is no
                // value to carry: this member is `@required`, and no value was
                // matched.
                r#match: String::new(),
                r#type: entry.entity_type.clone(),
                // Nothing was placed, so nothing was done.
                action: ASSESSED_NONE.to_string(),
                detected: Some(false),
            })
        })
        .collect()
}

/// `texts` with every value an anonymizing entry matched replaced by the name of what
/// it is, which is the content a guardrail that masked rather than blocked answers
/// with.
fn masked(texts: &[String], maskings: &[Masking]) -> Vec<String> {
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let mut spans: Vec<&Masking> = maskings
                .iter()
                .filter(|masking| masking.text == index)
                .collect();
            spans.sort_by_key(|masking| (masking.begin, masking.end));
            let characters: Vec<char> = text.chars().collect();
            let mut answered = String::new();
            let mut at = 0usize;
            for masking in spans {
                if masking.begin < at || masking.end > characters.len() {
                    continue;
                }
                answered.extend(characters[at..masking.begin].iter());
                answered.push_str(&masking.placeholder);
                at = masking.end;
            }
            answered.extend(characters[at.min(characters.len())..].iter());
            answered
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The contextual grounding policy
// ---------------------------------------------------------------------------

/// One contextual grounding filter, as this evaluation scored it.
struct GroundingFinding {
    /// `GuardrailContextualGroundingFilterType` — what the filter scores.
    kind: String,
    /// The 0..1 score it is configured to act below.
    threshold: f64,
    /// The score this evaluation computed.
    score: f64,
    action: PolicyAction,
}

impl Finding for GroundingFinding {
    fn detected(&self) -> bool {
        // A grounding filter acts on content that scores BELOW the threshold it is
        // configured with: the threshold is the least score it admits.
        self.score < self.threshold
    }
    fn action(&self) -> PolicyAction {
        self.action
    }
}

/// Each contextual grounding filter the policy configures, scored against the
/// submitted content.
///
/// A filter scores the guarded content against material the SUBMISSION carries for
/// it, so a filter whose material is no part of this evaluation has nothing to score
/// and is not reported: that is what keeps the input side of a guarded turn — which
/// submits the prompt and no answer to score — out of a policy about the answer. The
/// surfaces that require that material refuse a submission without it up front
/// ([`missing_grounding_material`]), so a configured filter is scored wherever the
/// submission was admitted as one it applies to.
///
/// The words of each part of the submission are read ONCE here, however many filters
/// the policy configures, so the work is linear in the submitted text rather than in
/// the product of the text and the configuration. `content` is already cut to the
/// ceilings one evaluation reads at ([`GuardedContent::budgeted`]).
fn grounding_findings(
    policies: &GuardrailPolicies,
    content: &GuardedContent,
) -> Vec<GroundingFinding> {
    if policies.grounding_filters.is_empty() {
        return Vec::new();
    }
    let answer = words_of(&content.guarded);
    let sources = words_of(&content.grounding_sources);
    let queries = words_of(&content.queries);
    policies
        .grounding_filters
        .iter()
        .filter_map(|filter| {
            let action = filter.action?;
            let score = match filter.kind.as_str() {
                // How far the answer is SUPPORTED by the source, and how far it
                // ANSWERS the query ([`grounding_score`], [`relevance_score`]).
                GROUNDING_FILTER_GROUNDING if content.has_grounding_source() => {
                    coverage(&answer, &sources)
                }
                GROUNDING_FILTER_RELEVANCE if content.has_query() => {
                    coverage(&queries, &answer)
                }
                _ => return None,
            };
            Some(GroundingFinding {
                kind: filter.kind.clone(),
                threshold: filter.threshold,
                score,
                action,
            })
        })
        .collect()
}

/// How far the guarded content is SUPPORTED by the grounding source: the proportion
/// of the words the content is made of that the source carries.
///
/// Content every word of which the source carries is fully grounded; content the
/// source carries none of is grounded in nothing. Content with no words at all is
/// supported by nothing and scores zero, rather than being read as perfectly
/// grounded.
#[must_use]
pub fn grounding_score(sources: &[String], guarded: &[String]) -> f64 {
    coverage(&words_of(guarded), &words_of(sources))
}

/// How far the guarded content ANSWERS the query: the proportion of the words the
/// query is made of that the content carries.
#[must_use]
pub fn relevance_score(queries: &[String], guarded: &[String]) -> f64 {
    coverage(&words_of(queries), &words_of(guarded))
}

/// The contextual-grounding assessment.
///
/// A contextual grounding filter answers a SCORE of the submission rather than a
/// detection in it, so every filter the policy evaluated is reported under either
/// output scope: there is always a score, and the filter's own `detected` says
/// whether that score fell below the threshold it is configured with.
fn grounding_assessment(
    findings: &[GroundingFinding],
) -> Option<GuardrailContextualGroundingPolicyAssessment> {
    let filters: Vec<GuardrailContextualGroundingFilter> = findings
        .iter()
        .map(|finding| GuardrailContextualGroundingFilter {
            r#type: finding.kind.clone(),
            threshold: finding.threshold,
            score: finding.score,
            action: finding.assessed(),
            detected: Some(finding.detected()),
        })
        .collect();
    (!filters.is_empty()).then_some(GuardrailContextualGroundingPolicyAssessment {
        filters: Some(filters),
    })
}

// ---------------------------------------------------------------------------
// The Converse-API surfaces
// ---------------------------------------------------------------------------

/// The guardrail a Converse-API request NAMED, with the content it submitted to it.
///
/// Resolving the guardrail and reading the request apart are cheap and need this
/// service's store; evaluating the policies against the text is neither, so the two
/// are separate: a caller resolves on the request path ([`converse_guardrail`]) and
/// runs the evaluation off it ([`guard_turn`]).
#[derive(Debug, Clone)]
pub struct ConverseGuardrail {
    applied: guardrail_registry::ResolvedGuardrail,
    /// What the request submitted, each piece under what its qualifiers say it is.
    submitted: GuardedContent,
    /// Whether the request asked for the assessments, and for the whole of them.
    traced: bool,
    full_scope: bool,
}

/// The guardrail a Converse-API request named in its own guardrail configuration, or
/// `None` where it named none.
///
/// `Err` is a request this guardrail cannot be applied to at all: one naming a
/// revision this account does not hold, or one whose guardrail needs contextual
/// grounding material the request does not carry.
///
/// The configuration is taken apart by the caller rather than passed whole, because
/// the two Converse surfaces carry it in two shapes (`GuardrailConfiguration` and
/// `GuardrailStreamConfiguration`) that say the same three things about the guardrail
/// to apply.
///
/// Call it with no store of this service held.
pub fn converse_guardrail<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: Option<&str>,
    version: Option<&str>,
    trace: &str,
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
) -> Result<Option<ConverseGuardrail>, String> {
    // Both identifier members carry the empty string as their default, so a
    // configuration that names no guardrail asks for none.
    let Some(identifier) = identifier.filter(|identifier| !identifier.is_empty()) else {
        return Ok(None);
    };
    let version = version
        .filter(|version| !version.is_empty())
        .unwrap_or(guardrail_registry::DRAFT_VERSION);
    let Some(applied) = guardrail_registry::resolve(engine, identifier, version) else {
        return Err(guardrail_registry::GUARDRAIL_NOT_FOUND.to_string());
    };
    // What the REQUEST submitted, read the way the apply call reads its own blocks:
    // the blocks qualified as the grounding source and the query are the material a
    // contextual grounding filter scores against rather than content to guard, so a
    // guardrail that needs that material and was given none cannot be applied at all.
    let submitted = converse_content(system, messages);
    if let Some(message) = missing_grounding_material(&applied.policies, &submitted) {
        return Err(message);
    }
    let full_scope = trace == GUARDRAIL_TRACE_ENABLED_FULL;
    Ok(Some(ConverseGuardrail {
        applied,
        submitted,
        traced: trace == GUARDRAIL_TRACE_ENABLED || full_scope,
        full_scope,
    }))
}

/// The guardrail stage of a Converse-API request that named a guardrail: the turn to
/// serve under it, and the trace to report.
///
/// The same policies `ApplyGuardrail` evaluates are evaluated here, so a guardrail
/// cannot block one surface and pass the other for the same text: the request's own
/// content is evaluated on the INPUT side and the text the turn serves on the OUTPUT
/// side, and a turn either side blocked carries that side's blocked messaging and
/// ends on the guardrail's stop reason. A policy that MASKS rather than blocks leaves
/// the turn, with the values it matched masked in the text it serves.
///
/// The trace comes back only where the request asked for one, and is reported under
/// the guardrail's own id — the key a Converse trace carries its assessments under.
///
/// Synchronous, CPU-bound work with no await of its own, over text
/// [`crate::state::guardrail_checks::budgeted_texts`] bounds: a request-time caller
/// runs it off the async worker, as the apply surface does.
#[must_use]
pub fn guard_turn(
    guardrail: ConverseGuardrail,
    turn: ConverseTurn,
) -> (ConverseTurn, Option<GuardrailTraceAssessment>) {
    let ConverseGuardrail {
        applied,
        submitted,
        traced,
        full_scope,
    } = guardrail;
    let policies = &applied.policies;
    let generated = turn.text.clone();
    let on_input = apply(
        &applied,
        &GuardedContent {
            guarded: submitted.guarded,
            images: submitted.images,
            ..GuardedContent::default()
        },
        true,
        full_scope,
    );
    // The OUTPUT side guards the text the turn serves, and it is that text a
    // contextual grounding filter scores: the answer against the source the request
    // carried, and against the question it asked.
    let on_output = apply(
        &applied,
        &GuardedContent {
            guarded: vec![generated.clone()],
            grounding_sources: submitted.grounding_sources,
            queries: submitted.queries,
            images: 0,
        },
        false,
        full_scope,
    );

    let trace = traced.then(|| GuardrailTraceAssessment {
        // The text the guardrail assessed on the output side, which is what the
        // trace reports beside the assessments.
        model_output: Some(vec![generated]),
        input_assessment: Some(::std::collections::HashMap::from([(
            applied.id.clone(),
            on_input.assessment.clone(),
        )])),
        output_assessments: Some(::std::collections::HashMap::from([(
            applied.id.clone(),
            vec![on_output.assessment.clone()],
        )])),
        action_reason: on_input
            .action_reason
            .clone()
            .or_else(|| on_output.action_reason.clone()),
    });

    if on_input.blocked {
        return (turn.intervened(policies.blocked_messaging(true)), trace);
    }
    if on_output.blocked {
        return (turn.intervened(policies.blocked_messaging(false)), trace);
    }
    // A policy that masks rather than blocks leaves the turn served, with the values
    // it matched masked in the text it carries.
    if let Some(masked) = on_output.outputs.first() {
        return (turn.anonymized(masked), trace);
    }
    (turn, trace)
}

/// `GuardrailTrace` — whether the request asked for the guardrail's assessments, and
/// for the whole of them.
pub const GUARDRAIL_TRACE_ENABLED: &str = "enabled";
pub const GUARDRAIL_TRACE_ENABLED_FULL: &str = "enabled_full";

/// What a Converse-API request submitted to the guardrail it named.
///
/// "If you include guardContent blocks in the messages field, the guardrail operates
/// only on those messages. If you include no guardContent blocks, the guardrail
/// operates on all messages in the request" (`Converse$guardrailConfig`) — so a
/// request that MARKED content for guarding submits exactly that content, and one
/// that marked none submits all of its text.
///
/// A marked block's QUALIFIERS say what it is, read by the same rule the apply call
/// reads its own blocks by ([`push_qualified`]): a block qualified as the grounding
/// source or the query is the material the contextual grounding policy scores
/// against, so it is no more guarded content here than it is there. A request that
/// marked nothing carries no such material and submits text to guard alone.
fn converse_content(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
) -> GuardedContent {
    let mut marked: Vec<&GuardrailConverseContentBlock> = Vec::new();
    for block in system.map(Vec::as_slice).unwrap_or_default() {
        if let SystemContentBlock::guardContent(guard) = block {
            marked.push(guard);
        }
    }
    for message in messages.map(Vec::as_slice).unwrap_or_default() {
        for block in &message.content {
            if let ContentBlock::guardContent(guard) = block {
                marked.push(guard);
            }
        }
    }
    if !marked.is_empty() {
        let mut content = GuardedContent::default();
        for guard in marked {
            match guard {
                GuardrailConverseContentBlock::Text(text) => push_qualified(
                    &text.text,
                    text.qualifiers.as_deref().unwrap_or_default(),
                    &mut content,
                ),
                GuardrailConverseContentBlock::Image(_) => content.images += 1,
            }
        }
        return content;
    }
    let mut guarded: Vec<String> = Vec::new();
    for block in system.map(Vec::as_slice).unwrap_or_default() {
        match block {
            SystemContentBlock::Text(text) => guarded.push(text.clone()),
            SystemContentBlock::guardContent(guard) => super::push_guard_text(guard, &mut guarded),
            SystemContentBlock::cachePoint(_) => {}
        }
    }
    for message in messages.map(Vec::as_slice).unwrap_or_default() {
        for block in &message.content {
            super::push_content_text(block, &mut guarded);
        }
    }
    GuardedContent {
        guarded,
        ..GuardedContent::default()
    }
}

impl ConverseTurn {
    /// The turn a guardrail MASKED rather than blocked: the text it serves with every
    /// value an anonymizing policy entry matched replaced by the name of what it is.
    ///
    /// Nothing else about the turn changes — it was generated for, it requested
    /// whatever tool it requested, and it ended for whatever reason it ended — so
    /// masking is not an intervention the stop reason reports.
    #[must_use]
    pub fn anonymized(self, text: &str) -> Self {
        Self {
            output_tokens: super::count_text_tokens(text),
            text: text.to_string(),
            ..self
        }
    }
}

// ---------------------------------------------------------------------------
// The word measures the topic and grounding recognizers read text with
// ---------------------------------------------------------------------------

/// The words `text` is made of: its runs of alphanumeric characters, folded to
/// lowercase, each counted once.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The words a set of pieces of text is made of, together.
fn words_of(texts: &[String]) -> BTreeSet<String> {
    texts.iter().flat_map(|text| words(text)).collect()
}

/// What proportion of `needles` `haystack` carries: one where it carries all of them,
/// zero where it carries none — and zero for an empty `needles`, which nothing
/// carries anything of.
fn coverage(needles: &BTreeSet<String>, haystack: &BTreeSet<String>) -> f64 {
    if needles.is_empty() {
        return 0.0;
    }
    let carried = needles.iter().filter(|word| haystack.contains(*word)).count();
    carried as f64 / needles.len() as f64
}

/// `pattern`, matched case-insensitively, or `None` for a pattern that does not
/// compile.
fn insensitive(pattern: &str) -> Option<Regex> {
    match RegexBuilder::new(pattern).case_insensitive(true).build() {
        Ok(matcher) => Some(matcher),
        Err(error) => {
            ::tracing::warn!(%error, "a guardrail pattern could not be compiled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::guardrail_registry::{Sides, TOPIC_TYPE_DENY};

    /// RULE: the grounding score is the proportion of the guarded content's words the
    /// source carries, and the relevance score the proportion of the query's words
    /// the content carries.
    #[test]
    fn contextual_grounding_scores_the_content_against_the_source_and_the_query() {
        let source = vec!["The capital of France is Paris.".to_string()];
        let query = vec!["What is the capital of France?".to_string()];
        // (case, content, grounding score)
        let rows: [(&str, &str, f64); 4] = [
            ("every word is in the source", "Paris is the capital.", 1.0),
            ("no word is in the source", "Berlin hosts many museums", 0.0),
            ("one word of three is", "Paris hosts museums", 1.0 / 3.0),
            ("no words at all", "", 0.0),
        ];
        for (case, content, expected) in rows {
            let guarded = vec![content.to_string()];
            let actual = grounding_score(&source, &guarded);
            assert!(
                (actual - expected).abs() < 1e-9,
                "case {case:?}: expected grounding {expected}, got {actual}"
            );
            let relevance = relevance_score(&query, &guarded);
            assert!(
                (0.0..=1.0).contains(&relevance),
                "case {case:?}: the relevance score must be a modelled 0..1 score, got {relevance}"
            );
        }
    }

    /// RULE: a denied topic is placed by its own name, as a phrase, or by the sample
    /// phrases it was configured with — and a longer word that merely contains the
    /// name is not it.
    #[test]
    fn a_denied_topic_is_placed_by_its_name_or_its_examples() {
        let topic = DeniedTopic {
            name: "Fiduciary Advice".to_string(),
            kind: TOPIC_TYPE_DENY.to_string(),
            examples: vec!["Where should I invest my retirement savings?".to_string()],
            sides: Sides::both(PolicyAction::Block),
        };
        // (case, text, on topic)
        let rows: [(&str, &str, bool); 6] = [
            ("the topic named", "Can you give me fiduciary advice?", true),
            ("the topic shouted", "FIDUCIARY ADVICE please", true),
            (
                "an example, rephrased",
                "where should i invest the retirement savings",
                true,
            ),
            (
                "an example, worded differently",
                "tell me a joke about accountants",
                false,
            ),
            ("part of a longer word", "fiduciaryadvice", false),
            (
                "nothing of the topic",
                "what is the capital of France",
                false,
            ),
        ];
        for (case, text, expected) in rows {
            let policies = GuardrailPolicies {
                topics: vec![topic.clone()],
                ..GuardrailPolicies::default()
            };
            let placed = topic_findings(&policies, &[text.to_string()], true);
            let actual = placed.first().is_some_and(|finding| finding.detected);
            assert_eq!(
                actual, expected,
                "case {case:?}: {text:?} expected on-topic={expected}, got {actual}"
            );
        }
    }
}
