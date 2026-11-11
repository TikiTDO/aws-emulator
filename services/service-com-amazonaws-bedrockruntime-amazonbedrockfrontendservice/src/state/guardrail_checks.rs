// SPDX-License-Identifier: CC0-1.0

//! The inline guardrail checks `InvokeGuardrailChecks` evaluates, and the text-unit
//! accounting it reports for them.
//!
//! Every check is served by an engine that runs IN PROCESS over the submitted
//! messages, so each score, offset and count below is computed from the request
//! rather than read out of it:
//!
//! * the SENSITIVE-INFORMATION check runs the PII recognizer this emulator already
//!   serves Comprehend's PII family from
//!   ([`detect_pii_spans`]), so the two services cannot disagree about what a value
//!   is, and the offsets a finding carries are that recognizer's own zero-based
//!   character offsets into the content block it was found in. The recognizer's
//!   entity catalog is this service's catalog under other names for seven entities,
//!   which [`SENSITIVE_INFORMATION_ENTITY_TYPES`] maps. A finding's confidence is
//!   answered per DETECTION, from how completely the detected value determines the
//!   entity ([`entity_confidence`]), so two detections of differing strength in one
//!   response carry differing confidences.
//! * the CONTENT-FILTER check scores every modelled category from the text. Three
//!   of them — the ones about language — are classified by the English
//!   profanity/abuse engine this emulator already analyses toxicity with
//!   (`rustrict`), and the two about CONDUCT, which that engine has no class for,
//!   are placed by a rule recognizer over the forms each of those categories is
//!   published as covering. No modelled category is answered by a constant: a
//!   category comes back at zero only where both the engine and the rules placed
//!   nothing of it in any message.
//! * the PROMPT-ATTACK check runs a rule recognizer over the same text. A prompt
//!   attack is an INSTRUCTION to the model, published as a small set of imperative
//!   forms, so each modelled category is recognised by the forms it is published in,
//!   and a finding's severity is the evidence class of the rule that placed it. The
//!   families overlap, and a message carries one attack, so the result ATTRIBUTES it
//!   ([`prompt_attack_findings`]): the family the message exercises most strongly
//!   among the ones asked about carries the severity, and a family read only below
//!   that is answered at zero.
//!
//! A check the request does not configure is never evaluated and never reported. A
//! check that IS configured answers about every category or entity type the request
//! named and about nothing else: the content filter and the prompt-attack check
//! report one entry per named category, carrying zero where they placed nothing of
//! it — which is what the recorded response of a request naming a whole category set
//! carries (`bedrock-runtime.guardrail-checks-results-mirror-requested-check-types`)
//! — and the sensitive-information check reports one entry per entity it placed,
//! because an entry of that check is a LOCATION in the text.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use rustrict::{Censor, Type};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::contains_pii_entities::{
    detect_pii_spans, Evidence,
};

/// The order the content filter reports its verdicts in.
///
/// It is the SERVICE's own order and not a model artefact: the model declares the
/// same five categories in another order
/// (`smithy_query(query="shape",shape="GuardrailChecksContentFilterCategory")`), the
/// generated value constant
/// ([`crate::ops::types::GUARDRAIL_CHECKS_CONTENT_FILTER_CATEGORY_VALUES`]) is
/// sorted, and the order a REFUSAL lists the same value set in
/// ([`super::CONTENT_FILTER_CATEGORY_ORDER`]) is a third one — so the order the
/// results come back in has to be stated once here rather than taken from any of
/// them. The MEMBERSHIP is the model's: the crate's own test pins this list to cover
/// exactly that value set, so a category the model adds or drops cannot go
/// unreported.
pub const CONTENT_FILTER_RESULT_ORDER: [&str; 5] =
    ["VIOLENCE", "MISCONDUCT", "HATE", "SEXUAL", "INSULTS"];

/// The order the prompt-attack check reports its verdicts in, stated here for the
/// same reason as the categories above and pinned against
/// [`crate::ops::types::GUARDRAIL_CHECKS_PROMPT_ATTACK_CATEGORY_VALUES`] by the same
/// test.
pub const PROMPT_ATTACK_RESULT_ORDER: [&str; 3] =
    ["JAILBREAK", "PROMPT_INJECTION", "PROMPT_LEAKAGE"];

/// How many characters one text unit covers.
///
/// Amazon Bedrock accounts guardrail evaluation in text units, and one text unit
/// covers up to 1,000 characters of text ("Guardrails" in Amazon Bedrock pricing),
/// so the units a check consumed is the submitted character count divided by this,
/// rounded up.
pub const CHARACTERS_PER_TEXT_UNIT: usize = 1000;

/// The most characters of submitted text one evaluation reads.
///
/// The EMULATOR's bound rather than an AWS one, and the thing that makes the work a
/// request can ask for FINITE: the three recognizers are linear in the text, so
/// without a ceiling here one request could occupy a blocking-pool slot for as long
/// as the caller cares to make its body. It is twenty-five text units
/// ([`CHARACTERS_PER_TEXT_UNIT`]), far above any conversation a client submits for an
/// inline check, and a request past it is refused before any analysis runs
/// ([`too_much_text_message`]). It bounds the number of content blocks too: the
/// modelled minimum length of a text block is one character, so a request cannot
/// carry more blocks than characters.
pub const MAX_SUBMITTED_CHARACTERS: usize = 25 * CHARACTERS_PER_TEXT_UNIT;

/// How long a caller waits for an evaluation to answer before the request is failed.
///
/// Orders of magnitude above what evaluating text at the size ceiling costs, so it
/// never fires for a request the recognizers are simply working through. It bounds
/// what the CALLER waits for, not the work: a blocking task cannot be cancelled from
/// outside, so when the budget fires the evaluation runs to its natural end and
/// releases its blocking-pool slot then. What bounds the WORK is the refusal of a
/// request carrying more than [`MAX_SUBMITTED_CHARACTERS`] of text, applied before
/// any analysis starts.
///
/// ONE budget for every surface that runs these recognizers — the inline checks and
/// the guardrail a request applies — because the work is the same recognizers over
/// text this same ceiling bounds: neither surface can hold a blocking-pool slot for
/// longer than the other.
pub const EVALUATION_TIME_BUDGET: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// The most pieces of submitted text one evaluation reads.
///
/// The EMULATOR's bound beside [`MAX_SUBMITTED_CHARACTERS`], and the other half of
/// what makes the work a request can ask for FINITE: every configured policy entry is
/// read against every piece, so the work is the PRODUCT of the piece count and the
/// configuration, and bounding the characters alone does not bound it — a request can
/// carry thousands of empty or one-character pieces. It is far above any submission a
/// client makes (a conversation, or the content blocks of one apply call), and the
/// pieces past it are not read ([`budgeted_texts`]); the surface that takes the
/// pieces as its own submission refuses a request past it outright
/// ([`too_many_blocks_message`]).
pub const MAX_SUBMITTED_BLOCKS: usize = 1_000;

/// The service's message for a request carrying more pieces of text than one
/// evaluation reads.
///
/// Both numbers come from the request and from the bound it broke, never from a
/// recorded run.
#[must_use]
pub fn too_many_blocks_message(blocks: usize) -> String {
    format!(
        "The request carries {blocks} content blocks, which exceeds the maximum of \
         {MAX_SUBMITTED_BLOCKS} content blocks for one request."
    )
}

/// The pieces of text one evaluation reads, cut to the ceilings it reads at
/// ([`MAX_SUBMITTED_CHARACTERS`] and [`MAX_SUBMITTED_BLOCKS`]).
///
/// The characters are charged over every piece together and at least one per piece,
/// and the pieces are counted, so neither the length of the text, nor the number of
/// pieces it arrives in, nor the two together can grow the work one request does.
/// Every stage that evaluates text it did not first refuse for being too long reads
/// it through this.
#[must_use]
pub fn budgeted_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut budget = MAX_SUBMITTED_CHARACTERS;
    let mut pieces: Vec<String> = Vec::new();
    for piece in texts {
        if budget == 0 || pieces.len() >= MAX_SUBMITTED_BLOCKS {
            break;
        }
        let guarded: String = piece.chars().take(budget).collect();
        budget = budget.saturating_sub(guarded.chars().count().max(1));
        pieces.push(guarded);
    }
    pieces
}

/// The service's message for a request carrying more text than one evaluation reads.
///
/// Both numbers come from the request and from the bound it broke, never from a
/// recorded run.
#[must_use]
pub fn too_much_text_message(characters: usize) -> String {
    format!(
        "The messages carry {characters} characters, which exceeds the maximum of \
         {MAX_SUBMITTED_CHARACTERS} characters for one request."
    )
}

/// The most sensitive-information findings one evaluation reports.
///
/// The EMULATOR's bound rather than an AWS one: one request cannot render an
/// unbounded number of findings into one response. Findings past it are dropped and
/// the result says so ([`SensitiveInformationFindings::truncated`]) rather than being
/// refused, and the bound is far above the entity count of any realistic message.
pub const MAX_SENSITIVE_INFORMATION_FINDINGS: usize = 100;

/// The text units `characters` of submitted text consume.
///
/// Every configured check reads the whole submitted text, so each one consumes the
/// same units; the modelled minimum length of a content block makes that at least
/// one unit.
#[must_use]
pub fn text_units(characters: usize) -> i32 {
    let units = characters.div_ceil(CHARACTERS_PER_TEXT_UNIT).max(1);
    i32::try_from(units).unwrap_or(i32::MAX)
}

/// One text block of a request, with where it sits in the request.
///
/// `message_index` and `content_index` are zero-based, the scale the modelled
/// `messageIndex` / `contentIndex` members carry (`range` min 0).
#[derive(Debug, Clone)]
pub struct CheckedText {
    pub message_index: usize,
    pub content_index: usize,
    pub text: String,
}

/// The checks one request configured: for each check type, the categories or entity
/// types it named, or `None` for a check it did not configure at all.
///
/// The named values are a SET: the request may name the same category or entity type
/// more than once, and each is answered once, so what a repeated name cannot do is
/// multiply the work of a scan.
#[derive(Debug, Clone, Default)]
pub struct RequestedChecks {
    pub content_filter: Option<BTreeSet<String>>,
    pub prompt_attack: Option<BTreeSet<String>>,
    pub sensitive_information: Option<BTreeSet<String>>,
}

impl RequestedChecks {
    /// Whether the request configured no check at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content_filter.is_none()
            && self.prompt_attack.is_none()
            && self.sensitive_information.is_none()
    }
}

/// One category a check was asked about, with the severity it placed on it.
///
/// A check answers one of these per category the request named — zero severity for a
/// category it placed nothing of — so a client reading the results learns the
/// verdict for everything it asked about.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryFinding {
    /// A value of the modelled category enum of the check that answered it.
    pub category: &'static str,
    /// The severity of the placement, in the modelled 0..1 range, or zero where the
    /// check placed nothing of this category.
    pub severity_score: f64,
}

/// One sensitive-information entity the recognizer placed in the submitted text.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityFinding {
    /// A value of the modelled `GuardrailChecksSensitiveInformationEntityType` enum.
    pub entity_type: &'static str,
    pub confidence_score: f64,
    /// Zero-based character offsets into the content block the entity was found in,
    /// `end_offset` one past its last character.
    pub begin_offset: i32,
    pub end_offset: i32,
    pub message_index: i32,
    pub content_index: i32,
}

/// What the sensitive-information check found.
#[derive(Debug, Clone, Default)]
pub struct SensitiveInformationFindings {
    pub findings: Vec<EntityFinding>,
    /// Whether [`MAX_SENSITIVE_INFORMATION_FINDINGS`] cut findings the recognizer
    /// had placed.
    pub truncated: bool,
}

/// What one evaluation found, per configured check.
#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    pub content_filter: Option<Vec<CategoryFinding>>,
    pub prompt_attack: Option<Vec<CategoryFinding>>,
    pub sensitive_information: Option<SensitiveInformationFindings>,
    /// The text units each configured check consumed.
    pub text_units: i32,
}

/// One served inline guardrail evaluation, as the service's own log of them
/// ([`super::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::guardrail_check_evaluations`])
/// keeps it: which checks the request configured, what each of them consumed, and
/// how much they found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationLogEntry {
    /// The check types the request configured, in the order
    /// [`super::GUARDRAIL_CHECK_TYPES`] lists them.
    pub checks: Vec<&'static str>,
    /// The text units each configured check consumed.
    pub text_units: i32,
    /// How many findings the evaluation reported across those checks.
    pub findings: usize,
    /// Epoch milliseconds at which the evaluation was served.
    pub at_millis: i64,
}

impl Evaluation {
    /// The check types this evaluation ran, in the order
    /// [`super::GUARDRAIL_CHECK_TYPES`] lists them.
    #[must_use]
    pub fn checks(&self) -> Vec<&'static str> {
        let ran = [
            self.content_filter.is_some(),
            self.prompt_attack.is_some(),
            self.sensitive_information.is_some(),
        ];
        super::GUARDRAIL_CHECK_TYPES
            .into_iter()
            .zip(ran)
            .filter(|(_, ran)| *ran)
            .map(|(check, _)| check)
            .collect()
    }

    /// How many placements this evaluation reported across its checks: a category
    /// result whose severity is above zero is a placement, one at zero is the
    /// answer that the check placed nothing of that category.
    #[must_use]
    pub fn findings(&self) -> usize {
        let placed = |findings: &Option<Vec<CategoryFinding>>| {
            findings.as_ref().map_or(0, |findings| {
                findings
                    .iter()
                    .filter(|finding| finding.severity_score > 0.0)
                    .count()
            })
        };
        placed(&self.content_filter)
            + placed(&self.prompt_attack)
            + self
                .sensitive_information
                .as_ref()
                .map_or(0, |found| found.findings.len())
    }

    /// This evaluation as the log entry recording it, served at `at_millis`.
    #[must_use]
    pub fn log_entry(&self, at_millis: i64) -> EvaluationLogEntry {
        EvaluationLogEntry {
            checks: self.checks(),
            text_units: self.text_units,
            findings: self.findings(),
            at_millis,
        }
    }
}

/// Run every configured check over `texts`.
///
/// Synchronous, CPU-bound work with no await of its own: a request-time caller runs
/// it off the async worker, having first refused a request carrying more than
/// [`MAX_SUBMITTED_CHARACTERS`], which is what bounds the work done here.
#[must_use]
pub fn evaluate(requested: &RequestedChecks, texts: &[CheckedText]) -> Evaluation {
    let characters: usize = submitted_characters(texts);
    Evaluation {
        content_filter: requested
            .content_filter
            .as_ref()
            .map(|categories| content_filter_findings(categories, texts)),
        prompt_attack: requested
            .prompt_attack
            .as_ref()
            .map(|categories| prompt_attack_findings(categories, texts)),
        sensitive_information: requested
            .sensitive_information
            .as_ref()
            .map(|entity_types| sensitive_information_findings(entity_types, texts)),
        text_units: text_units(characters),
    }
}

/// How many characters of text `texts` carries: what the bound
/// [`MAX_SUBMITTED_CHARACTERS`] is applied to, and what the text units are computed
/// from.
#[must_use]
pub fn submitted_characters(texts: &[CheckedText]) -> usize {
    texts.iter().map(|block| block.text.chars().count()).sum()
}

// ---------------------------------------------------------------------------
// The category recognizers
// ---------------------------------------------------------------------------

/// One rule of a category recognizer: a pattern over one message's text, the
/// modelled category a match places, and the severity it places it at.
///
/// The content filter and the prompt-attack check are both category checks over the
/// same text, so both read this one rule type through [`placed_severity`]. What
/// differs between them is the rule TABLE and the scale each check states its
/// severities on, which is why the severity is a number here rather than an evidence
/// class: each table turns its own evidence classes into its own scale as it is
/// built.
struct CategoryRule {
    category: &'static str,
    severity: f64,
    pattern: Regex,
}

/// The strongest severity any rule of `category` placed in any of `texts`, or `None`
/// when no rule of that category matched anywhere.
///
/// The request asks one question per category about the CONVERSATION, not one per
/// message, so a category placed in any message is placed for the request.
fn placed_severity(rules: &[CategoryRule], category: &str, texts: &[CheckedText]) -> Option<f64> {
    rules
        .iter()
        .filter(|rule| rule.category == category)
        .filter(|rule| texts.iter().any(|block| rule.pattern.is_match(&block.text)))
        .map(|rule| rule.severity)
        .max_by(f64::total_cmp)
}

// ---------------------------------------------------------------------------
// The content filter
// ---------------------------------------------------------------------------

/// How much text one rule allows between two parts of the same sentence.
///
/// A rule reads ONE sentence, so its parts are joined by this rather than by `.*`:
/// the span cannot cross a sentence boundary and is bounded, which keeps a rule from
/// joining two unrelated sentences into a finding and keeps its cost linear in the
/// text.
const WITHIN_SENTENCE: &str = r"[^.!?\n]{0,40}?";

/// The forms a speaker states an intention in.
const STATED_INTENT: &str =
    r"(?:will|'ll|am going to|going to|gonna|am about to|intend to|plan to|want to|would)";

/// Who an act is aimed at, where a rule needs the act to have a target.
///
/// The speaker is deliberately not one of them: an act aimed at the speaker reads as
/// a REPORT of someone else's conduct as often as content carrying that conduct.
const PERSON: &str = r"(?:you|your|yourself|him|his|her|hers|them|their|themselves|everyone|anyone|someone|somebody)";

/// The severity reported for a category the engine placed at each of the three
/// severity levels it distinguishes.
///
/// `GuardrailChecksContentFilterResultEntry$severityScore` is modelled over 0..1
/// (`smithy_query(query="shape",shape="GuardrailChecksContentFilterResultEntry")`),
/// and the engine's verdict for a category is one of three ordered levels rather
/// than a continuous number — so each level is reported as the midpoint of its third
/// of that range. The levels are the same three the engine hands this emulator's
/// toxicity analysis, and the calibration is stated here in the type the member is
/// modelled as: this member is a `Double`, so the midpoints are computed in `f64`
/// rather than widened from the `Float` that member carries.
///
/// They are `pub` because the ENFORCED guardrail reads them
/// ([`super::enforced_guardrail`]): a configured filter blocks a category from a
/// level upwards, so the level a severity stands for has to be the same number on
/// both sides of that comparison.
pub const MILD_SEVERITY: f64 = 1.0 / 6.0;
pub const MODERATE_SEVERITY: f64 = 0.5;
pub const SEVERE_SEVERITY: f64 = 5.0 / 6.0;

/// The modelled content-filter categories the profanity/abuse engine classifies,
/// each with the engine category that classifies it.
///
/// The engine classifies English text as offensive (slurs and discriminatory
/// language), mean (demeaning, insulting) and sexual, which are the modelled
/// `HATE`, `INSULTS` and `SEXUAL` filters — the three categories that are about the
/// LANGUAGE a message uses. The two remaining modelled categories are about the
/// CONDUCT a message states, asks for or endorses, which no profanity class
/// describes, so they are placed by [`content_filter_rules`] instead; every modelled
/// category is read by one of the two.
const CONTENT_FILTER_DETECTORS: [(&str, Type); 3] = [
    ("HATE", Type::OFFENSIVE),
    ("INSULTS", Type::MEAN),
    ("SEXUAL", Type::SEXUAL),
];

/// The engine category that classifies `category`, or `None` for a modelled
/// category the engine has no class for, which [`content_filter_rules`] places
/// instead.
fn content_filter_detector(category: &str) -> Option<Type> {
    CONTENT_FILTER_DETECTORS
        .into_iter()
        .find(|(named, _)| *named == category)
        .map(|(_, detector)| detector)
}

/// The severity the analysis places on `category`, or `None` when the engine placed
/// no content of that category in the text.
fn category_severity(analysis: Type, category: Type) -> Option<f64> {
    if analysis.is(category & Type::SEVERE) {
        Some(SEVERE_SEVERITY)
    } else if analysis.is(category & Type::MODERATE) {
        Some(MODERATE_SEVERITY)
    } else if analysis.is(category & Type::MILD) {
        Some(MILD_SEVERITY)
    } else {
        None
    }
}

/// How a rule established that a message carries what its category covers, and
/// therefore how much severity a match of it places.
///
/// The two classes are EVIDENCE classes rather than decoration: which one placed a
/// category says what the recognizer actually read in the message, so two messages
/// that carry a category the same way are scored the same rather than by a per-match
/// probability. Both category checks read these classes; each states what severity
/// each class carries on ITS own scale, because the content filter reports the three
/// levels the profanity engine beside it reports and the prompt-attack check reports
/// its own two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleEvidence {
    /// The message states the thing itself: an act of the category aimed at someone,
    /// a request for instructions in one, or the imperative an attack family is
    /// published as.
    Stated,
    /// The message only alludes to it: it endorses the conduct, names its proceeds,
    /// or uses a phrase of the attack family, with no act or imperative of its own.
    Alluded,
}

/// Acts of physical pain, hurt or injury: what the `VIOLENCE` filter covers —
/// threats to inflict, or glorification of, physical harm toward a person, group or
/// thing.
const VIOLENT_ACT: &str = r"(?:kill|murder|shoot|stab|strangle|choke|behead|lynch|assault|maim|mutilate|torture|hurt|harm|injure|beat|bash|punch|slap|attack|bomb|blow up|burn down|massacre|break)";

/// The acts above whose plain reading is physical harm whatever sentence they sit
/// in, so a rule may read one aimed at a person on its own. The ones left out —
/// `attack`, `beat`, `break`, `hurt`, `harm` — have an everyday reading too, and are
/// read only by a rule that also reads the stated intention or the part of the body
/// the act is aimed at.
const UNAMBIGUOUS_VIOLENT_ACT: &str =
    r"(?:kill|murder|shoot|stab|strangle|choke|behead|lynch|assault|maim|mutilate|torture|beat up|punch)";

/// Acts of criminal activity, or of harming, defrauding or taking advantage of a
/// person, group or institution: what the `MISCONDUCT` filter covers.
const ILLICIT_ACT: &str = r"(?:pick (?:a|the|his|her|their)?\s*locks?|hotwire|break into|breaking into|steal|stealing|shoplift(?:ing)?|rob|robbing|burglari[sz]e|launder (?:money|cash)|money laundering|counterfeit|forge|forging|hack into|hacking into|phish(?:ing)?|smuggle|smuggling|evade tax(?:es)?|tax evasion|buy drugs|sell drugs|cook meth|make meth|defraud|scam|embezzle|insider trading|bribe|extort|blackmail|kidnap|skim (?:credit )?cards?|clone (?:a|the) (?:credit|debit) card|disable (?:the|an|his|her|their) alarm|bypass (?:the|an) alarm|fake (?:ids?|passports?|invoices?|receipts?))";

/// Every content-filter rule, compiled once.
///
/// The CATEGORIES are the two modelled filters the profanity engine has no class
/// for; the FORMS are what each of them is published as covering — physical harm to
/// a person, group or thing (`VIOLENCE`), and criminal activity or the defrauding of
/// a person, group or institution (`MISCONDUCT`) — each stated as the forms a
/// message carries that conduct in: a stated intention, an imperative, a request for
/// instructions, or an endorsement of it.
fn content_filter_rules() -> &'static [CategoryRule] {
    static RULES: OnceLock<Vec<CategoryRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |category: &'static str, evidence: RuleEvidence, pattern: String| {
            CategoryRule {
                category,
                // The content filter reports on the profanity engine's three-level
                // scale, so a stated act is the severe level and an allusion the
                // moderate one.
                severity: match evidence {
                    RuleEvidence::Stated => SEVERE_SEVERITY,
                    RuleEvidence::Alluded => MODERATE_SEVERITY,
                },
                pattern: Regex::new(&pattern).expect("a content-filter rule is a valid regex"),
            }
        };
        vec![
            // A stated intention to inflict harm on someone: the threat form.
            rule(
                "VIOLENCE",
                RuleEvidence::Stated,
                format!(r"(?i)\b(?:i|we|he|she|they)\b{WITHIN_SENTENCE}\b{STATED_INTENT}\b{WITHIN_SENTENCE}\b{VIOLENT_ACT}\b{WITHIN_SENTENCE}\b{PERSON}\b"),
            ),
            // An imperative, or an act aimed at a person with no other framing.
            rule(
                "VIOLENCE",
                RuleEvidence::Stated,
                format!(r"(?i)\b{UNAMBIGUOUS_VIOLENT_ACT}\s+{PERSON}\b"),
            ),
            // Harm aimed at a part of someone's body.
            rule(
                "VIOLENCE",
                RuleEvidence::Stated,
                format!(r"(?i)\b(?:break|breaking|snap|smash|crush|cut off|rip off|burn)\b{WITHIN_SENTENCE}\b(?:your|his|her|their|the)\s+(?:legs?|arms?|kneecaps?|bones?|neck|jaw|face|ribs?|skull|fingers?|teeth)\b"),
            ),
            // Asking how to inflict harm, or how to build the means of it.
            rule(
                "VIOLENCE",
                RuleEvidence::Stated,
                format!(r"(?i)\bhow\s+(?:to|do i|can i|would i|does one)\b{WITHIN_SENTENCE}\b{VIOLENT_ACT}\b{WITHIN_SENTENCE}\b{PERSON}\b"),
            ),
            rule(
                "VIOLENCE",
                RuleEvidence::Stated,
                format!(r"(?i)\bhow\s+(?:to|do i|can i|would i|does one)\b{WITHIN_SENTENCE}\b(?:make|build|assemble)\b{WITHIN_SENTENCE}\b(?:bombs?|explosives?|napalm|nerve agents?|silencers?)\b"),
            ),
            // Glorifying harm, or wishing it on someone, with no act of the
            // speaker's own.
            rule(
                "VIOLENCE",
                RuleEvidence::Alluded,
                r"(?i)\b(?:deserve[sd]? to (?:die|be (?:killed|shot|beaten|hurt))|should (?:be (?:killed|shot|beaten|executed|lynched)|die)|hope (?:you|he|she|they) (?:dies?|gets? hurt|gets? shot)|glad (?:he|she|they) (?:died|got shot|was killed|were killed))\b".to_string(),
            ),
            // Seeking or giving instructions for an illicit act.
            rule(
                "MISCONDUCT",
                RuleEvidence::Stated,
                format!(r"(?i)\b(?:how to|how do i|how can i|how would i|step by step|steps? to|teach me|show me how|explain how|walk me through|best way to|help me)\b{WITHIN_SENTENCE}\b{ILLICIT_ACT}\b"),
            ),
            // A stated intention to commit one.
            rule(
                "MISCONDUCT",
                RuleEvidence::Stated,
                format!(r"(?i)\b(?:i|we)\b{WITHIN_SENTENCE}\b{STATED_INTENT}\b{WITHIN_SENTENCE}\b{ILLICIT_ACT}\b"),
            ),
            // Naming the proceeds or the scheme, with no act of the speaker's own.
            rule(
                "MISCONDUCT",
                RuleEvidence::Alluded,
                r"(?i)\b(?:stolen (?:credit cards?|goods|cars?)|fake (?:ids?|passports?|documents?)|counterfeit (?:bills?|money|notes?|currency)|money laundering|insider trading|black market|untraceable (?:guns?|handguns?|weapons?)|burner phones?)\b".to_string(),
            ),
        ]
    })
}

/// The content-filter result over `texts`: one entry per category the request
/// named, carrying the strongest severity either recognizer placed on that category
/// in ANY of the messages — the request asks one question per category about the
/// conversation, not one per message — and zero for a category neither placed
/// anything of.
///
/// Reported in [`CONTENT_FILTER_RESULT_ORDER`], so a request naming several
/// categories gets them back in one stable order whatever order it named them in,
/// and a category it named twice comes back once.
fn content_filter_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> Vec<CategoryFinding> {
    let analyses: Vec<Type> = texts
        .iter()
        .map(|block| Censor::from_str(&block.text).analyze())
        .collect();

    CONTENT_FILTER_RESULT_ORDER
        .into_iter()
        .filter(|category| requested.contains(*category))
        .map(|category| {
            let classified = content_filter_detector(category).and_then(|detector| {
                analyses
                    .iter()
                    .filter_map(|analysis| category_severity(*analysis, detector))
                    .max_by(f64::total_cmp)
            });
            // Whichever recognizer read the most of this category decides its
            // severity; zero is what BOTH reading nothing of it answers.
            let severity_score = [
                classified,
                placed_severity(content_filter_rules(), category, texts),
            ]
            .into_iter()
            .flatten()
            .max_by(f64::total_cmp)
            .unwrap_or(0.0);
            CategoryFinding {
                category,
                severity_score,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The prompt-attack check
// ---------------------------------------------------------------------------

/// The severity a prompt-attack finding carries at each of the two evidence classes
/// the check distinguishes ([`RuleEvidence`]).
///
/// `GuardrailChecksPromptAttackResultEntry$severityScore` is a `Double` over 0..1
/// documented as "higher values indicate greater severity"
/// (`smithy_query(query="shape",shape="GuardrailChecksPromptAttackResultEntry")`), so
/// the two ENDS of that range are the two verdicts a recognizer can reach: a message
/// that STATES the attack — it issues the published imperative of the family itself —
/// exercises the category decisively and is reported at the top of the range, and one
/// that only uses a phrase of the family, with no imperative of its own, a step below
/// it. A category the message exercises in neither way is reported at the bottom of
/// the range (zero) by [`prompt_attack_findings`]. The scale is this member's own
/// `f64` one: the modelled type is a 64-bit `Double`, so no severity here is carried
/// in a narrower float and widened on serialization, which cannot represent a value
/// of this range exactly.
const STATED_ATTACK_SEVERITY: f64 = 1.0;
const ALLUDED_ATTACK_SEVERITY: f64 = 0.8;

/// Every prompt-attack rule, compiled once, as the one rule type both category
/// checks are read through ([`CategoryRule`]).
///
/// The CATEGORIES are the model's own enum
/// (`GUARDRAIL_CHECKS_PROMPT_ATTACK_CATEGORY_VALUES`); the
/// FORMS are the published attack families each category names — an instruction
/// override (`PROMPT_INJECTION`), a request to shed the model's restrictions
/// (`JAILBREAK`) and a request to disclose the instructions themselves
/// (`PROMPT_LEAKAGE`) — each stated as the imperative forms those families are
/// published in. Every category has rules, so no modelled category is silent.
fn attack_rules() -> &'static [CategoryRule] {
    static RULES: OnceLock<Vec<CategoryRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |category: &'static str, evidence: RuleEvidence, pattern: &str| CategoryRule {
            category,
            severity: match evidence {
                RuleEvidence::Stated => STATED_ATTACK_SEVERITY,
                RuleEvidence::Alluded => ALLUDED_ATTACK_SEVERITY,
            },
            pattern: Regex::new(pattern).expect("a prompt-attack rule is a valid regex"),
        };
        vec![
            // Shedding the behaviour the model was configured to follow: the family
            // is published as a prompt designed to get the model past its own safety
            // and moderation behaviour, so a message that asks for that behaviour to
            // be dropped states this attack whichever part of it it names — the
            // safety behaviour itself, the instructions the model operates under, or
            // the restrictions it answers within — and the named personas the family
            // is published under allude to it.
            rule(
                "JAILBREAK",
                RuleEvidence::Stated,
                r"(?i)\b(?:bypass|disable|turn off|get around|work around|ignore|disregard|forget|override|drop)\b[^.!?\n]{0,40}?\b(?:guardrails?|safety|safeguards?|filters?|restrictions?|limitations?|polic(?:y|ies)|guidelines?|rules?|moderation)\b",
            ),
            rule(
                "JAILBREAK",
                RuleEvidence::Stated,
                r"(?i)\b(?:ignore|disregard|forget|override|replace)\b[^.!?\n]{0,40}?\b(?:instructions?|directions?|prompts?|system (?:prompt|message))\b",
            ),
            rule(
                "JAILBREAK",
                RuleEvidence::Stated,
                r"(?i)\b(?:pretend|act|behave|respond|answer)\b[^.!?\n]{0,40}?\b(?:no (?:rules|restrictions|limits|filters|guidelines)|without (?:any )?(?:rules|restrictions|limits|filters|guidelines)|unrestricted|uncensored|unfiltered)\b",
            ),
            rule(
                "JAILBREAK",
                RuleEvidence::Alluded,
                r"(?i)\b(?:jailbreak|do anything now|dan mode|developer mode|god mode)\b",
            ),
            // Putting the attacker's instructions in the place of the developer's:
            // the family is published as a prompt that overrides the instructions
            // the application gave the model with instructions of the attacker's
            // own, so what STATES it is the injected instruction — a message that
            // opens its own instruction channel in the content, or announces the
            // instructions that are to replace them. Telling the model to drop the
            // instructions it has, with no instruction of the attacker's own in it,
            // only uses this family's phrase: it is the form the rule above reads as
            // the jailbreak it states, and it is placed here as an allusion.
            rule(
                "PROMPT_INJECTION",
                RuleEvidence::Stated,
                r"(?im)^\s*(?:system|assistant)\s*:",
            ),
            rule(
                "PROMPT_INJECTION",
                RuleEvidence::Alluded,
                r"(?i)\b(?:ignore|disregard|forget|override|replace)\b[^.!?\n]{0,40}?\b(?:instructions?|directions?|prompts?|system (?:prompt|message))\b",
            ),
            rule(
                "PROMPT_INJECTION",
                RuleEvidence::Alluded,
                r"(?i)\bnew\s+(?:instructions?|rules?|directions?)\b",
            ),
            // Disclosing the instructions the model was given.
            rule(
                "PROMPT_LEAKAGE",
                RuleEvidence::Stated,
                r"(?i)\b(?:repeat|reveal|print|show|output|disclose|tell me|give me)\b[^.!?\n]{0,40}?\b(?:system prompt|system message|your instructions|your prompt|the prompt above|initial prompt|text above)\b",
            ),
            rule(
                "PROMPT_LEAKAGE",
                RuleEvidence::Stated,
                r"(?i)\bwhat\s+(?:are|were|is|was)\b[^.!?\n]{0,30}?\b(?:your (?:instructions|rules|guidelines)|system prompt|system message)\b",
            ),
            rule(
                "PROMPT_LEAKAGE",
                RuleEvidence::Alluded,
                r"(?i)\b(?:verbatim|word for word)\b[^.!?\n]{0,30}?\b(?:prompt|instructions?)\b",
            ),
        ]
    })
}

/// The prompt-attack result over `texts`: one entry per category the request named,
/// in [`PROMPT_ATTACK_RESULT_ORDER`].
///
/// A message carries ONE prompt attack, and the families the modelled categories
/// name overlap — the same words can be read as more than one of them — so the check
/// ATTRIBUTES the attack rather than answering each category on its own: the entry
/// for the family the message exercises most strongly, among the categories the
/// request named, carries the severity its rules placed, and every other named
/// category is answered at zero, the same answer a message that attacks in no way at
/// all gets. Where two named families are exercised equally strongly — each stated
/// by its own form, as a message that both sheds the model's behaviour and asks for
/// its instructions does — both are reported: the attribution zeroes a family read
/// only BELOW the strongest, not one read as strongly.
fn prompt_attack_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> Vec<CategoryFinding> {
    let placed: Vec<(&'static str, Option<f64>)> = PROMPT_ATTACK_RESULT_ORDER
        .into_iter()
        .filter(|category| requested.contains(*category))
        .map(|category| (category, placed_severity(attack_rules(), category, texts)))
        .collect();

    // The attack is attributed to the family it fits best among the ones asked
    // about: the strongest evidence any of their rules placed anywhere in the
    // conversation.
    let attributed = placed
        .iter()
        .filter_map(|(_, severity)| *severity)
        .max_by(f64::total_cmp);

    placed
        .into_iter()
        // The family the attack is attributed to carries what its rules placed; a
        // family read only below that is not what this attack is, so it is answered
        // as not exercised.
        .map(|(category, severity)| CategoryFinding {
            category,
            severity_score: severity
                .filter(|placed| attributed.is_some_and(|strongest| *placed >= strongest))
                .unwrap_or(0.0),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The sensitive-information check
// ---------------------------------------------------------------------------

/// This service's sensitive-information entity types, each paired with the entity
/// type the shared PII recognizer reports for the same value.
///
/// The keys are the modelled catalog
/// (`GUARDRAIL_CHECKS_SENSITIVE_INFORMATION_ENTITY_TYPE_VALUES`,
/// pinned against this table by the crate's own test); the values are Comprehend's
/// catalog, which names the same entity without a country prefix for seven of them.
/// Comprehend recognises types this catalog does not name (a date, the Indian
/// identifiers), and a span of one of those is reported by neither check here,
/// because this service models no type for it.
pub const SENSITIVE_INFORMATION_ENTITY_TYPES: [(&str, &str); 31] = [
    ("ADDRESS", "ADDRESS"),
    ("AGE", "AGE"),
    ("AWS_ACCESS_KEY", "AWS_ACCESS_KEY"),
    ("AWS_SECRET_KEY", "AWS_SECRET_KEY"),
    ("CA_HEALTH_NUMBER", "CA_HEALTH_NUMBER"),
    ("CA_SOCIAL_INSURANCE_NUMBER", "CA_SOCIAL_INSURANCE_NUMBER"),
    ("CREDIT_DEBIT_CARD_CVV", "CREDIT_DEBIT_CVV"),
    ("CREDIT_DEBIT_CARD_EXPIRY", "CREDIT_DEBIT_EXPIRY"),
    ("CREDIT_DEBIT_CARD_NUMBER", "CREDIT_DEBIT_NUMBER"),
    ("DRIVER_ID", "DRIVER_ID"),
    ("EMAIL", "EMAIL"),
    (
        "INTERNATIONAL_BANK_ACCOUNT_NUMBER",
        "INTERNATIONAL_BANK_ACCOUNT_NUMBER",
    ),
    ("IP_ADDRESS", "IP_ADDRESS"),
    ("LICENSE_PLATE", "LICENSE_PLATE"),
    ("MAC_ADDRESS", "MAC_ADDRESS"),
    ("NAME", "NAME"),
    ("PASSWORD", "PASSWORD"),
    ("PHONE", "PHONE"),
    ("PIN", "PIN"),
    ("SWIFT_CODE", "SWIFT_CODE"),
    (
        "UK_NATIONAL_HEALTH_SERVICE_NUMBER",
        "UK_NATIONAL_HEALTH_SERVICE_NUMBER",
    ),
    (
        "UK_NATIONAL_INSURANCE_NUMBER",
        "UK_NATIONAL_INSURANCE_NUMBER",
    ),
    (
        "UK_UNIQUE_TAXPAYER_REFERENCE_NUMBER",
        "UK_UNIQUE_TAXPAYER_REFERENCE_NUMBER",
    ),
    ("URL", "URL"),
    ("USERNAME", "USERNAME"),
    ("US_BANK_ACCOUNT_NUMBER", "BANK_ACCOUNT_NUMBER"),
    ("US_BANK_ROUTING_NUMBER", "BANK_ROUTING"),
    (
        "US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER",
        "US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER",
    ),
    ("US_PASSPORT_NUMBER", "PASSPORT_NUMBER"),
    ("US_SOCIAL_SECURITY_NUMBER", "SSN"),
    (
        "VEHICLE_IDENTIFICATION_NUMBER",
        "VEHICLE_IDENTIFICATION_NUMBER",
    ),
];

/// The confidence this check reports for a detection, by how completely the detected
/// value itself determines the entity it was reported as.
///
/// `GuardrailChecksSensitiveInformationResultEntry$confidenceScore` is a `Double`
/// over 0..1 documented as the confidence score FOR THE DETECTION, higher values
/// indicating greater confidence
/// (`smithy_query(query="shape",\
/// shape="GuardrailChecksSensitiveInformationResultEntry")`), so it is answered per
/// detected entity, from what the recognizer established about THAT value rather than
/// from the entity type it was reported as. Three things determine a value:
///
/// * [`DETERMINED_ENTITY_CONFIDENCE`] — the value is fully determined: it carries the
///   check its defining standard gives it and that check passed, or its form is a
///   token nothing else takes (an address with a domain, a URI, a key with its
///   documented prefix), so the value alone says what it is.
/// * [`NUMBER_ENTITY_CONFIDENCE`] — the value is a bare number: digits and
///   separators, identified only by how many digits it has and how they are grouped,
///   and it carries as many as the published identifier of its kind does (a
///   telephone number carrying its country or area code, which ITU-T E.164 makes
///   part of the number).
/// * [`PARTIAL_ENTITY_CONFIDENCE`] — the value is weaker than that: a short run of
///   digits, which is only part of such an identifier (a subscriber number with no
///   area code) and which ordinary numbers take too, or a value whose own form is
///   ambiguous and which was admitted only because the surrounding text named what it
///   is.
///
/// Two detections of differing strength in one response therefore carry differing
/// scores. The scale is this member's own `f64` one: the sibling service that shares
/// the recognizer models its score as a `Float` and calibrates its own classes on
/// that scale, and widening one of those 32-bit values into this `Double` would
/// serialize an artefact of the narrower type (no value of this range but zero and
/// one is exactly representable in 32 bits), so this member's levels are stated here
/// in the type it is modelled as and carried in it end to end.
const DETERMINED_ENTITY_CONFIDENCE: f64 = 1.0;
const NUMBER_ENTITY_CONFIDENCE: f64 = 0.8;
const PARTIAL_ENTITY_CONFIDENCE: f64 = 0.6;

/// How many digits an identifier written as a bare number carries before the number
/// determines it on its own.
///
/// A telephone number is the published identifier that is written this way and
/// nothing else: ITU-T E.164 makes the country code part of the number and the NANP
/// the area code, which puts a complete one at ten digits, while the subscriber
/// number alone is seven. Fewer digits than this is part of an identifier rather than
/// one.
const DETERMINING_DIGITS: usize = 10;

/// The confidence a detection of `value`, established by `evidence`, carries on the
/// scale above.
fn entity_confidence(evidence: Evidence, value: &str) -> f64 {
    match evidence {
        // The value's own check passed, so nothing about the value is left open.
        Evidence::Validated => DETERMINED_ENTITY_CONFIDENCE,
        // The text had to name what the value is, so the value does not determine it.
        Evidence::Context => PARTIAL_ENTITY_CONFIDENCE,
        // A distinctive form with no check of its own: how much the form determines
        // depends on whether it is a token or a number.
        Evidence::Format => {
            let digits = value.chars().filter(char::is_ascii_digit).count();
            let bare_number = value.chars().all(|character| {
                character.is_ascii_digit() || NUMBER_SEPARATORS.contains(&character)
            });
            if !bare_number {
                DETERMINED_ENTITY_CONFIDENCE
            } else if digits >= DETERMINING_DIGITS {
                NUMBER_ENTITY_CONFIDENCE
            } else {
                PARTIAL_ENTITY_CONFIDENCE
            }
        }
    }
}

/// What a number written for people to read carries between its digits: the
/// separators E.164 and the NANP write a telephone number with, and the ones the
/// other numeric identifiers are grouped by.
const NUMBER_SEPARATORS: [char; 7] = [' ', '-', '.', '(', ')', '+', '/'];

/// This service's entity type for a span the recognizer reported, or `None` for a
/// recognizer type this service models no entity for.
fn modelled_entity_type(recognized: &str) -> Option<&'static str> {
    SENSITIVE_INFORMATION_ENTITY_TYPES
        .into_iter()
        .find(|(_, comprehend)| *comprehend == recognized)
        .map(|(modelled, _)| modelled)
}

/// The sensitive-information findings over `texts`, for the entity types the request
/// named.
///
/// Findings are in request order — by message, then by content block, then by where
/// in the block the entity starts — and are cut at
/// [`MAX_SENSITIVE_INFORMATION_FINDINGS`], where the scan STOPS rather than carrying
/// on over the blocks it has left.
fn sensitive_information_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> SensitiveInformationFindings {
    let mut findings: Vec<EntityFinding> = Vec::new();
    for block in texts {
        for span in detect_pii_spans(&block.text) {
            let Some(entity_type) = modelled_entity_type(span.entity_type) else {
                continue;
            };
            if !requested.contains(entity_type) {
                continue;
            }
            if findings.len() >= MAX_SENSITIVE_INFORMATION_FINDINGS {
                return SensitiveInformationFindings {
                    findings,
                    truncated: true,
                };
            }
            // The value the recognizer placed, sliced out of the block by the
            // finding's own character offsets: what it is made of is what decides
            // how much it determines the entity.
            let value: String = block
                .text
                .chars()
                .skip(span.begin)
                .take(span.end.saturating_sub(span.begin))
                .collect();
            findings.push(EntityFinding {
                entity_type,
                confidence_score: entity_confidence(span.evidence, &value),
                begin_offset: offset_i32(span.begin),
                end_offset: offset_i32(span.end),
                message_index: offset_i32(block.message_index),
                content_index: offset_i32(block.content_index),
            });
        }
    }
    SensitiveInformationFindings {
        findings,
        truncated: false,
    }
}

/// An offset or index as the modelled integer member carries it.
fn offset_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
