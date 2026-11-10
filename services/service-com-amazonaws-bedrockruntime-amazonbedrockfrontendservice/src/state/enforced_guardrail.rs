// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-ENFORCED GUARDRAIL this data plane applies to inference.
//!
//! An account-level enforced guardrail is installed through the Bedrock CONTROL
//! plane (`PutEnforcedGuardrailConfiguration`), and the whole point of it is that
//! the guardrail is applied SERVER-SIDE to inference the caller never opted in
//! to: a request that names no guardrail at all is still evaluated against it.
//! So the configuration has to be readable on the inference path, and that is
//! what this module is.
//!
//! WHERE IT LIVES. The record sits in THIS service's store, keyed on this
//! service's state type like every other part of it, and the control plane
//! installs it here ([`install`]) on the call that stores the configuration —
//! the same direction `super::invocation_metrics` publishes an invocation's
//! metric data into the CloudWatch emulator's own store. The engine keys state
//! by type, so an enforcement the inference path can read has exactly one home,
//! and the two planes cannot drift into disagreeing about what is enforced.
//!
//! WHAT IT HOLDS. The guardrail revision being enforced, as the policies this
//! plane evaluates: the configured words and managed word lists of its word
//! policy, the filters of its content policy, the two blocked-messaging strings,
//! the models the configuration's model enforcement covers, and its
//! selective-guarding modes. A guardrail
//! version a configuration enforces is a NUMBERED version
//! (`GuardrailNumericalVersion`, never the DRAFT), and a numbered version is an
//! immutable snapshot of the guardrail's policies, so what is installed here is
//! exactly the revision inference evaluates for as long as the configuration
//! stands.
//!
//! HOW IT IS APPLIED. Before generation, the request's own input text is
//! evaluated against EVERY policy the enforced revision configures — a guardrail
//! blocks on any one of them, not only on the one it was most recently read for.
//! A word the word policy blocks on the input side, or a category the content
//! policy filters on the input side at or above the strength that filter is
//! configured at, means the guardrail INTERVENED, and the turn that is served
//! carries the guardrail's `blockedInputMessaging` rather than a completion
//! ("BLOCK – Block the content and replace it with blocked messaging",
//! `GuardrailWord$inputAction`). An input every policy passes is generated for,
//! and the generated text is then evaluated against the same policies on the
//! OUTPUT side, which answers `blockedOutputsMessaging` the same way. Either way the invocation
//! reports the action it took: the invoke surfaces carry it in the
//! service-injected [`GUARDRAIL_ACTION_FIELD`] member of the response body, and
//! the Converse surfaces in the modelled `StopReason`
//! [`STOP_REASON_GUARDRAIL_INTERVENED`].
//!
//! WHEN IT CHANGES. The enforcement stands until the control plane replaces it —
//! a further put of the configuration — or withdraws it, which the delete of the
//! configuration does. The guardrail version it holds is an immutable one, so a
//! later change to that guardrail's DRAFT changes nothing about what is enforced;
//! and deleting the guardrail itself leaves what the account installed in force,
//! because the configuration that installed it is still the one the account
//! holds.
//!
//! HOW MUCH WORK ONE EVALUATION DOES. The text it reads is bounded by the same
//! ceiling this service's inline guardrail checks read
//! ([`super::guardrail_checks::MAX_SUBMITTED_CHARACTERS`]), charged at least one
//! character per piece of the request, so neither the length of a prompt nor the
//! number of content blocks it is split into decides how much is read. The words
//! it looks for are ONE pattern over the installed word list — bounded in count
//! ([`MAX_ENFORCED_WORDS`]) and in size ([`MAX_ENFORCED_WORD_BYTES`]) when it is
//! installed — matched by the same linear-time engine the inline checks match
//! untrusted text with. So the scan is linear in that bounded text rather than
//! linear in it once per configured word — there is no product of the two — which
//! is what keeps this stage on the request path, beside the tokenizing and the
//! prompt digesting the same handlers already do there. The content policy is
//! evaluated by the very check the inline guardrail checks serve the content
//! filter from ([`super::guardrail_checks::evaluate`]), over that same bounded
//! text and once for every category the policy configures together, so one
//! evaluation reads the text once whatever the policy holds.

use rustrict::{Censor, Type};

use super::{
    count_text_tokens, invoke_model_response_body, ConverseTurn, InvokeTurn, ModelEntry,
    ModelFamily,
};
use crate::ops::types::{ContentBlock, Message, SystemContentBlock};

/// The member Amazon Bedrock itself adds to the body of a guarded invocation,
/// carrying the action its guardrail took. It is no model's response field and no
/// modelled member of this service's shapes — the service injects it into the
/// body — which is why it travels as a body member rather than as a modelled one,
/// exactly as [`super::INVOCATION_METRICS_FIELD`] does on a response stream.
pub const GUARDRAIL_ACTION_FIELD: &str = "amazon-bedrock-guardrailAction";

/// [`GUARDRAIL_ACTION_FIELD`] for an invocation whose guardrail intervened.
pub const ACTION_INTERVENED: &str = "INTERVENED";

/// [`GUARDRAIL_ACTION_FIELD`] for an invocation a guardrail was applied to and
/// found nothing in: enforcement is reported in BOTH directions, so a caller can
/// tell a guarded call that passed from an unguarded one.
pub const ACTION_NONE: &str = "NONE";

/// The modelled `StopReason` a Converse-API turn a guardrail blocked ends on.
pub const STOP_REASON_GUARDRAIL_INTERVENED: &str = "guardrail_intervened";

/// The `IncludedModelId` wildcard: the one value that member's pattern admits
/// beside a model id (`^(ALL|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63})…)$`), naming
/// every model rather than one.
pub const ALL_MODELS: &str = "ALL";

/// `GuardrailWordAction$BLOCK` — block the content and replace it with the
/// guardrail's blocked messaging.
pub const WORD_ACTION_BLOCK: &str = "BLOCK";

/// `GuardrailContentFilterAction$BLOCK` — the content policy's own spelling of
/// the same blocking action [`WORD_ACTION_BLOCK`] is the word policy's, read by
/// the one reading of a policy entry's action ([`blocks`]).
pub const FILTER_ACTION_BLOCK: &str = WORD_ACTION_BLOCK;

/// `GuardrailContentFilterType$PROMPT_ATTACK` — the one modelled content-filter
/// category that is about an INSTRUCTION to the model rather than about the
/// language a message uses, so it is placed by the prompt-attack recognizer
/// beside the content filter ([`super::guardrail_checks`]).
pub const FILTER_PROMPT_ATTACK: &str = "PROMPT_ATTACK";

/// `GuardrailFilterStrength` — the strengths a content filter can be configured
/// at, strongest first. `NONE` is named by its absence from
/// [`blocking_severity`]: a filter at that strength blocks nothing.
pub const FILTER_STRENGTH_HIGH: &str = "HIGH";
pub const FILTER_STRENGTH_MEDIUM: &str = "MEDIUM";
pub const FILTER_STRENGTH_LOW: &str = "LOW";

/// `GuardrailManagedWordsType$PROFANITY` — the one managed word list a guardrail
/// can configure.
pub const MANAGED_WORDS_PROFANITY: &str = "PROFANITY";

/// `SelectiveGuardingMode$SELECTIVE` — guard only the content the request itself
/// marked for guarding (`guardContent`), rather than the whole of that part of
/// the request.
pub const GUARDING_SELECTIVE: &str = "SELECTIVE";

/// The most configured words one enforcement is evaluated against.
///
/// The EMULATOR's bound on the work one invocation does, not an AWS quota: the
/// word policy is modelled `@length(min: 1, max: 10000)`, so the pattern an
/// evaluation matches with is bounded by that modelled ceiling and nothing past
/// it is installed.
pub const MAX_ENFORCED_WORDS: usize = 10_000;

/// The most bytes of configured word text one enforcement is evaluated against.
///
/// The EMULATOR's bound beside [`MAX_ENFORCED_WORDS`], and for the same reason:
/// it is the SIZE of the pattern an evaluation matches with, so the work of one
/// evaluation is bounded whatever the account configured — by count and by size
/// together, neither of which a request can grow. It is far above any word policy
/// a client configures (the modelled maximum word length is 100 characters), and
/// the words past it are not installed rather than the put being refused: this
/// bound is the emulator's own, not something to refuse a modelled configuration
/// over.
pub const MAX_ENFORCED_WORD_BYTES: usize = 64 * 1024;

/// One word a guardrail's word policy is configured with, as this plane applies
/// it: the text to look for, and whether a match blocks the input and the output.
///
/// `inputAction` / `outputAction` and `inputEnabled` / `outputEnabled` are what
/// decide those two flags ([`BlockedWord::new`]), so the policy's own vocabulary
/// is read once, here, rather than at each place a word is matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedWord {
    /// `GuardrailWord$text`, folded to lowercase: a word policy blocks a word
    /// however it is capitalised, so the fold is done ONCE here — when the
    /// enforcement is installed — rather than again for every piece of text of
    /// every request.
    pub text: String,
    pub blocks_input: bool,
    pub blocks_output: bool,
}

impl BlockedWord {
    /// A configured word as its policy entry means it.
    ///
    /// An entry that names no action blocks: `BLOCK` is the action a word filter
    /// takes on a match, and `NONE` is the one a configuration asks for to get
    /// detection without blocking ("`NONE` – Take no action but return detection
    /// information in the trace response", `GuardrailWord$inputAction`). An entry
    /// that says nothing about whether evaluation is enabled is evaluated: the
    /// member exists to turn an evaluation OFF ("Indicates whether guardrail
    /// evaluation is enabled on the input", `GuardrailWord$inputEnabled`).
    #[must_use]
    pub fn new(
        text: String,
        input_action: Option<&str>,
        input_enabled: Option<bool>,
        output_action: Option<&str>,
        output_enabled: Option<bool>,
    ) -> Self {
        Self {
            text: text.to_lowercase(),
            blocks_input: blocks(input_action, input_enabled),
            blocks_output: blocks(output_action, output_enabled),
        }
    }
}

/// One managed word list a guardrail's word policy is configured with: which list
/// it is, and whether it blocks the input and the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWordList {
    /// `GuardrailManagedWords$type` — today the model declares one value,
    /// [`MANAGED_WORDS_PROFANITY`].
    pub kind: String,
    pub blocks_input: bool,
    pub blocks_output: bool,
}

impl ManagedWordList {
    /// A managed word list as its policy entry means it, reading the action and
    /// the enabled members exactly as [`BlockedWord::new`] does.
    #[must_use]
    pub fn new(
        kind: String,
        input_action: Option<&str>,
        input_enabled: Option<bool>,
        output_action: Option<&str>,
        output_enabled: Option<bool>,
    ) -> Self {
        Self {
            kind,
            blocks_input: blocks(input_action, input_enabled),
            blocks_output: blocks(output_action, output_enabled),
        }
    }
}

/// Whether a policy entry with this action and enabled flag BLOCKS on that side.
fn blocks(action: Option<&str>, enabled: Option<bool>) -> bool {
    enabled.unwrap_or(true) && action.is_none_or(|action| action == WORD_ACTION_BLOCK)
}

/// One filter a guardrail's content policy is configured with, as this plane
/// applies it: which modelled category it filters, and the strength it filters
/// that category at on each side.
///
/// A side the filter does not BLOCK on carries no strength at all
/// ([`ContentFilter::new`] reads `inputAction` / `outputAction` and `inputEnabled`
/// / `outputEnabled` exactly as [`BlockedWord::new`] does), so the evaluation
/// below asks only about the categories the policy would block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFilter {
    /// `GuardrailContentFilter$type` — a value of the modelled
    /// `GuardrailContentFilterType` enum.
    pub category: String,
    /// `inputStrength` — the strength this category is filtered at on the input
    /// side, or `None` where this filter does not block that side.
    pub input_strength: Option<String>,
    /// `outputStrength` — the same for generated text.
    pub output_strength: Option<String>,
}

impl ContentFilter {
    /// A configured filter as its policy entry means it.
    #[must_use]
    pub fn new(
        category: String,
        input_strength: String,
        input_action: Option<&str>,
        input_enabled: Option<bool>,
        output_strength: String,
        output_action: Option<&str>,
        output_enabled: Option<bool>,
    ) -> Self {
        Self {
            category,
            input_strength: blocks(input_action, input_enabled).then_some(input_strength),
            output_strength: blocks(output_action, output_enabled).then_some(output_strength),
        }
    }

    /// The strength this filter blocks `category` at on one side.
    fn strength(&self, input: bool) -> Option<&str> {
        if input {
            self.input_strength.as_deref()
        } else {
            self.output_strength.as_deref()
        }
    }
}

/// The lowest severity a filter configured at `strength` blocks a category from,
/// or `None` for a strength that blocks nothing.
///
/// A filter's strength decides how much of what its detector places is blocked:
/// the strongest setting blocks content placed at any confidence, the middle one
/// blocks medium and high confidence, and the weakest only high confidence, while
/// `GuardrailFilterStrength$NONE` applies no filter at all ("Content filters" in
/// the Amazon Bedrock guardrails documentation, which publishes exactly that
/// table of filter strengths against detection confidence). The engine places a
/// category at one of the three levels the content-filter check reports
/// ([`super::guardrail_checks`]), so each strength is the lowest of those levels
/// it blocks.
fn blocking_severity(strength: &str) -> Option<f64> {
    use super::guardrail_checks::{MILD_SEVERITY, MODERATE_SEVERITY, SEVERE_SEVERITY};

    match strength {
        FILTER_STRENGTH_HIGH => Some(MILD_SEVERITY),
        FILTER_STRENGTH_MEDIUM => Some(MODERATE_SEVERITY),
        FILTER_STRENGTH_LOW => Some(SEVERE_SEVERITY),
        _ => None,
    }
}

/// The guardrail the account enforces on inference, as the inference path applies
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnforcedGuardrail {
    /// `guardrailId` of the enforced guardrail, as the control plane resolved it.
    pub guardrail_id: String,
    /// The numbered `guardrailVersion` being enforced.
    pub guardrail_version: String,
    /// `blockedInputMessaging` — what the guardrail answers an input it blocked
    /// with.
    pub blocked_input_messaging: String,
    /// `blockedOutputsMessaging` — the same for generated text it blocked.
    pub blocked_outputs_messaging: String,
    /// The word policy's configured words, at most [`MAX_ENFORCED_WORDS`] of
    /// them.
    pub words: Vec<BlockedWord>,
    /// The word policy's managed word lists.
    pub managed_word_lists: Vec<ManagedWordList>,
    /// The content policy's filters, at most one per modelled category.
    pub content_filters: Vec<ContentFilter>,
    /// `modelEnforcement.includedModels` — the models the configuration is
    /// enforced on. EMPTY when the configuration carries no model enforcement at
    /// all, which is enforcement on every model ("If not present, the
    /// configuration is enforced on all models",
    /// `AccountEnforcedGuardrailInferenceInputConfiguration$modelEnforcement`).
    pub included_models: Vec<String>,
    /// `modelEnforcement.excludedModels` — the models excluded from it.
    pub excluded_models: Vec<String>,
    /// Whether `selectiveContentGuarding.system` asked for
    /// [`GUARDING_SELECTIVE`] guarding of the system prompt.
    pub system_guarded_selectively: bool,
    /// Whether `selectiveContentGuarding.messages` asked for it on the user
    /// messages.
    pub messages_guarded_selectively: bool,
}

impl EnforcedGuardrail {
    /// Whether this configuration is enforced on the model `model_id` names.
    ///
    /// A configuration with no model enforcement is enforced on every model, and
    /// one that has it is enforced on the models it includes — the wildcard
    /// [`ALL_MODELS`] standing for all of them — less the ones it excludes, which
    /// take precedence: the two lists are "Models to enforce the guardrail on"
    /// and "Models to exclude from enforcement of the guardrail"
    /// (`ModelEnforcement`).
    ///
    /// `model_id` is the id of the model the invocation RESOLVED to, so a request
    /// that named the model through an inference profile or an ARN is covered by
    /// a configuration naming the model itself.
    #[must_use]
    pub fn covers_model(&self, model_id: &str) -> bool {
        if self.excluded_models.iter().any(|excluded| excluded == model_id) {
            return false;
        }
        self.included_models.is_empty()
            || self
                .included_models
                .iter()
                .any(|included| included == ALL_MODELS || included == model_id)
    }

    /// The guardrail's `blockedInputMessaging` when `texts` — the request's input,
    /// piece by piece — carries something a policy of this guardrail blocks on the
    /// input side, and `None` when it passes every one of them.
    #[must_use]
    pub fn input_intervention(&self, texts: &[String]) -> Option<&str> {
        self.intervention(texts.iter().map(String::as_str), true)
            .then_some(self.blocked_input_messaging.as_str())
    }

    /// The same for generated text, answering `blockedOutputsMessaging`.
    #[must_use]
    pub fn output_intervention(&self, text: &str) -> Option<&str> {
        self.intervention(::std::iter::once(text), false)
            .then_some(self.blocked_outputs_messaging.as_str())
    }

    /// Whether ANY policy of this guardrail blocks `texts` on this side: a word
    /// its word policy blocks, or a category its content policy filters at or
    /// above the strength that filter is configured at.
    ///
    /// The text is read against the ceiling this stage reads at ONCE
    /// ([`Self::budgeted`]), so the two policies evaluate the same bounded text
    /// and a request cannot grow the work by the number of policies configured.
    fn intervention<'a>(&self, texts: impl IntoIterator<Item = &'a str>, input: bool) -> bool {
        let pieces = Self::budgeted(texts);
        self.blocked(&pieces, input) || self.filtered(&pieces, input)
    }

    /// The pieces of text one evaluation reads, cut to the ceiling this stage
    /// reads at.
    ///
    /// The characters are charged over every piece of the request together and at
    /// least one per piece, so neither the length of the text nor the number of
    /// pieces it arrives in can grow the work one invocation does.
    fn budgeted<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut budget = super::guardrail_checks::MAX_SUBMITTED_CHARACTERS;
        let mut pieces: Vec<String> = Vec::new();
        for piece in texts {
            if budget == 0 {
                break;
            }
            let guarded: String = piece.chars().take(budget).collect();
            budget = budget.saturating_sub(guarded.chars().count().max(1));
            pieces.push(guarded);
        }
        pieces
    }

    /// Whether a category the content policy filters on this side is placed in
    /// `pieces` at or above the strength its filter is configured at.
    ///
    /// The categories are asked about TOGETHER, in one evaluation by the check the
    /// inline guardrail checks serve the content filter from
    /// ([`super::guardrail_checks::evaluate`]), so the two surfaces cannot
    /// disagree about what a message carries. `PROMPT_ATTACK` is the one modelled
    /// filter category that check places with its prompt-attack recognizer rather
    /// than with the content filter, so it is read off that result instead — at
    /// the strongest severity any of its families was placed at, since the filter
    /// is configured for the category as a whole.
    fn filtered(&self, pieces: &[String], input: bool) -> bool {
        use super::guardrail_checks::{
            evaluate, CheckedText, RequestedChecks, CONTENT_FILTER_RESULT_ORDER,
            PROMPT_ATTACK_RESULT_ORDER,
        };

        let configured: Vec<(&str, f64)> = self
            .content_filters
            .iter()
            .filter_map(|filter| {
                let least = blocking_severity(filter.strength(input)?)?;
                Some((filter.category.as_str(), least))
            })
            .collect();
        if configured.is_empty() || pieces.is_empty() {
            return false;
        }
        let named = |category: &str| configured.iter().any(|(named, _)| *named == category);
        let categories: ::std::collections::BTreeSet<String> = CONTENT_FILTER_RESULT_ORDER
            .into_iter()
            .filter(|category| named(category))
            .map(str::to_string)
            .collect();
        let attacks: ::std::collections::BTreeSet<String> = match named(FILTER_PROMPT_ATTACK) {
            true => PROMPT_ATTACK_RESULT_ORDER
                .into_iter()
                .map(str::to_string)
                .collect(),
            false => ::std::collections::BTreeSet::new(),
        };
        let requested = RequestedChecks {
            content_filter: (!categories.is_empty()).then_some(categories),
            prompt_attack: (!attacks.is_empty()).then_some(attacks),
            sensitive_information: None,
        };
        let texts: Vec<CheckedText> = pieces
            .iter()
            .enumerate()
            .map(|(index, text)| CheckedText {
                message_index: index,
                content_index: 0,
                text: text.clone(),
            })
            .collect();
        let placed = evaluate(&requested, &texts);
        let attack_severity = placed
            .prompt_attack
            .unwrap_or_default()
            .into_iter()
            .map(|finding| finding.severity_score)
            .max_by(f64::total_cmp);
        configured.into_iter().any(|(category, least)| {
            let severity = match category == FILTER_PROMPT_ATTACK {
                true => attack_severity,
                false => placed
                    .content_filter
                    .as_ref()
                    .and_then(|findings| {
                        findings
                            .iter()
                            .find(|finding| finding.category == category)
                            .map(|finding| finding.severity_score)
                    }),
            };
            severity.is_some_and(|severity| severity > 0.0 && severity >= least)
        })
    }

    /// Whether any word the policy blocks on this side occurs in `pieces`.
    ///
    /// A configured word matches as a WORD: a match whose neighbours are
    /// alphanumeric is part of a longer word and is not the word that was
    /// configured. Matching ignores case, each side of it folded once — the words
    /// when the enforcement was installed, the text here. A managed word list is
    /// matched by the recognizer for the list it names rather than by text this
    /// emulator would have to carry a copy of.
    ///
    /// Every word the policy blocks on this side goes into ONE pattern, matched by
    /// the linear-time engine this crate already matches untrusted text with, so
    /// the text is scanned once however many words are configured.
    fn blocked(&self, pieces: &[String], input: bool) -> bool {
        let side = |blocks_input: bool, blocks_output: bool| {
            if input {
                blocks_input
            } else {
                blocks_output
            }
        };
        let needles: Vec<&str> = self
            .words
            .iter()
            .filter(|word| side(word.blocks_input, word.blocks_output))
            .map(|word| word.text.as_str())
            .collect();
        let profanity = self.managed_word_lists.iter().any(|list| {
            list.kind == MANAGED_WORDS_PROFANITY && side(list.blocks_input, list.blocks_output)
        });
        let matcher = match needles.is_empty() {
            true => None,
            false => {
                // ONE pattern over every word the policy blocks on this side, each
                // word ESCAPED so a word's own characters are never read as pattern
                // syntax. The word-boundary rule is applied to the match rather than
                // written into the pattern, because a configured phrase may begin or
                // end with punctuation of its own.
                let pattern = needles
                    .iter()
                    .map(|needle| ::regex::escape(needle))
                    .collect::<Vec<String>>()
                    .join("|");
                match ::regex::Regex::new(&pattern) {
                    Ok(matcher) => Some(matcher),
                    // The word list is bounded in count and in bytes when it is
                    // installed, so there is no configuration this is reachable for;
                    // it is reported where a developer can see it rather than failing
                    // an invocation that was otherwise servable.
                    Err(error) => {
                        ::tracing::warn!(
                            %error,
                            words = needles.len(),
                            "the enforced guardrail's word policy could not be compiled"
                        );
                        None
                    }
                }
            }
        };
        if matcher.is_none() && !profanity {
            return false;
        }

        // The pieces this evaluation reads are already cut to the ceiling this
        // stage reads at ([`Self::budgeted`]).
        for guarded in pieces {
            let lowercase = guarded.to_lowercase();
            if matcher.as_ref().is_some_and(|matcher| {
                matcher
                    .find_iter(&lowercase)
                    .any(|found| is_whole_word(&lowercase, found.start(), found.end()))
            }) {
                return true;
            }
            if profanity && Censor::from_str(guarded).analyze().is(Type::PROFANE) {
                return true;
            }
        }
        false
    }
}

/// Whether the match spanning `start..end` of `haystack` is a word of its own:
/// a neighbouring alphanumeric character makes it part of a longer word, which is
/// not the word the policy configured.
fn is_whole_word(haystack: &str, start: usize, end: usize) -> bool {
    let before = haystack.get(..start).and_then(|text| text.chars().next_back());
    let after = haystack.get(end..).and_then(|text| text.chars().next());
    !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
}

/// Install `enforced` as the guardrail this account enforces on inference,
/// replacing whatever stood before it; `None` withdraws it.
///
/// The Bedrock CONTROL plane calls this on the request that stores the account's
/// enforced guardrail configuration, so the configuration a caller installed is
/// the one the next invocation is evaluated against. Answers whether the store
/// was reached: an emulator assembled without this data plane enforces nothing,
/// and the control plane's own record of the configuration is unaffected either
/// way.
///
/// Call it with NO store of the calling service held, so two services' stores are
/// never locked at once.
pub fn install<Eng: Send + Sync + 'static>(
    engine: &Eng,
    enforced: Option<EnforcedGuardrail>,
) -> bool {
    let Some(state) = super::service_state(engine) else {
        return false;
    };
    state.lock().enforced_guardrail = enforced.map(|mut enforced| {
        // The word list an evaluation is matched with is bounded here, where it is
        // written, in count and in bytes: what one invocation's guardrail stage
        // costs is then fixed by the enforcement rather than by any later request.
        enforced.words.truncate(MAX_ENFORCED_WORDS);
        let mut bytes = 0usize;
        enforced.words.retain(|word| {
            bytes = bytes.saturating_add(word.text.len());
            bytes <= MAX_ENFORCED_WORD_BYTES
        });
        enforced
    });
    true
}

/// The guardrail this account enforces on an invocation of `model_id`, or `None`
/// when the account enforces none or the installed one does not cover that model.
#[must_use]
pub fn for_model<Eng: Send + Sync + 'static>(
    engine: &Eng,
    model_id: &str,
) -> Option<EnforcedGuardrail> {
    super::service_state(engine)?
        .lock()
        .enforced_guardrail
        .clone()
        .filter(|enforced| enforced.covers_model(model_id))
}

/// Report the guardrail action `action` on a response body or a stream chunk.
///
/// A no-op for a body that is not a JSON object, which no response family's
/// envelope is.
pub fn report_action(body: &mut serde_json::Value, action: &str) {
    if let Some(object) = body.as_object_mut() {
        object.insert(
            GUARDRAIL_ACTION_FIELD.to_string(),
            serde_json::Value::String(action.to_string()),
        );
    }
}

/// The guardrail stage of an invoke-API request: the turn to serve, and the action
/// to report, under the guardrail this account enforces on `model`.
///
/// `turn` is the inference as it was served ([`super::invoke_turn`]); the turn
/// that comes back carries the guardrail's blocked messaging instead when the
/// guardrail intervened. The action is `None` for a model no enforced
/// configuration covers, which is what leaves an unguarded invocation's body
/// without the service-injected member at all.
pub fn guard_invoke_turn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    model: &ModelEntry,
    family: ModelFamily,
    body: &[u8],
    turn: InvokeTurn,
) -> (InvokeTurn, Option<&'static str>) {
    let Some(enforced) = for_model(engine, &model.model_id) else {
        return (turn, None);
    };
    // The body of an invoke request carries the prompt itself, and the Converse
    // API's `guardContent` marking \u2014 the thing a selective guarding mode narrows
    // to \u2014 is no part of that body, so there is nothing for it to narrow and the
    // whole prompt is what is evaluated. An account-level control is not quietly
    // not applied because the configuration asked for selective guarding of a
    // part this surface does not carry.
    let prompt = vec![super::prompt_text_of_body(body)];
    if let Some(messaging) = enforced.input_intervention(&prompt) {
        return (
            turn.intervened(family, &model.model_id, messaging),
            Some(ACTION_INTERVENED),
        );
    }
    if let Some(messaging) = enforced.output_intervention(&turn.text) {
        return (
            turn.intervened(family, &model.model_id, messaging),
            Some(ACTION_INTERVENED),
        );
    }
    (turn, Some(ACTION_NONE))
}

/// The guardrail stage of a Converse-API request: the turn to serve under the
/// guardrail this account enforces on `model`.
///
/// The Converse surfaces report the action in the turn itself — a blocked turn
/// carries the guardrail's messaging and ends on
/// [`STOP_REASON_GUARDRAIL_INTERVENED`] — so there is no separate member to
/// inject here.
pub fn guard_converse_turn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    model: &ModelEntry,
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    turn: ConverseTurn,
) -> ConverseTurn {
    let Some(enforced) = for_model(engine, &model.model_id) else {
        return turn;
    };
    let texts = guarded_texts(&enforced, system, messages);
    if let Some(messaging) = enforced.input_intervention(&texts) {
        return turn.intervened(messaging);
    }
    if let Some(messaging) = enforced.output_intervention(&turn.text) {
        return turn.intervened(messaging);
    }
    turn
}

/// The text of a Converse-API request the enforcement is evaluated over.
///
/// Each part of the request has its own guarding mode, and a part guarded
/// [`GUARDING_SELECTIVE`]ly contributes only the content the request MARKED for
/// guarding — its `guardContent` blocks — while a part guarded comprehensively
/// (which is also how a part the configuration says nothing about is guarded)
/// contributes all of its text. That is the Converse API's own rule for guarded
/// content: "If you include guardContent blocks in the messages field, the
/// guardrail operates only on those messages. If you include no guardContent
/// blocks, the guardrail operates on all messages in the request"
/// (`Converse$guardrailConfig`).
fn guarded_texts(
    enforced: &EnforcedGuardrail,
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    for block in system.map(Vec::as_slice).unwrap_or_default() {
        match block {
            SystemContentBlock::guardContent(guard) => super::push_guard_text(guard, &mut pieces),
            SystemContentBlock::Text(text) if !enforced.system_guarded_selectively => {
                pieces.push(text.clone())
            }
            SystemContentBlock::Text(_) | SystemContentBlock::cachePoint(_) => {}
        }
    }
    for message in messages.map(Vec::as_slice).unwrap_or_default() {
        for block in &message.content {
            match block {
                ContentBlock::guardContent(guard) => super::push_guard_text(guard, &mut pieces),
                block if !enforced.messages_guarded_selectively => {
                    super::push_content_text(block, &mut pieces)
                }
                _ => {}
            }
        }
    }
    pieces
}

impl InvokeTurn {
    /// The turn a guardrail intervention serves: the guardrail's `messaging` in
    /// the family's own response envelope, accounted for as the output this call
    /// answered with.
    ///
    /// The input tokens stay the ones the request was: the prompt was read and
    /// evaluated whether or not it was generated for.
    #[must_use]
    pub fn intervened(self, family: ModelFamily, model_id: &str, messaging: &str) -> Self {
        let output_tokens = count_text_tokens(messaging);
        Self {
            response: invoke_model_response_body(
                family,
                model_id,
                &self.completion_id,
                messaging,
                self.input_tokens,
                output_tokens,
                false,
            ),
            text: messaging.to_string(),
            stopped_at_token_cap: false,
            output_tokens,
            ..self
        }
    }
}

impl ConverseTurn {
    /// The turn a guardrail intervention serves: the guardrail's `messaging` as
    /// the whole of the assistant's content, ending on
    /// [`STOP_REASON_GUARDRAIL_INTERVENED`].
    ///
    /// A blocked turn requests no tool, reaches no generation cap and halts on
    /// no stop sequence: nothing was generated for the request at all.
    #[must_use]
    pub fn intervened(self, messaging: &str) -> Self {
        Self {
            text: messaging.to_string(),
            tool_use: None,
            output_tokens: count_text_tokens(messaging),
            truncated: false,
            halted_at_stop_sequence: false,
            guardrail_intervened: true,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    //! The parts of the rule that need no served request: which models a
    //! configuration covers, and what its word policy blocks.

    use super::*;

    /// One configured word that blocks both ways.
    fn word(text: &str) -> BlockedWord {
        BlockedWord::new(text.to_string(), None, None, None, None)
    }

    /// RULE: a configuration with no model enforcement covers every model; one
    /// that has it covers the models it includes — `ALL` standing for all of them
    /// — less the ones it excludes.
    #[test]
    fn model_enforcement_decides_which_models_are_covered() {
        let enforced = |included: &[&str], excluded: &[&str]| EnforcedGuardrail {
            included_models: included.iter().map(|id| (*id).to_string()).collect(),
            excluded_models: excluded.iter().map(|id| (*id).to_string()).collect(),
            ..EnforcedGuardrail::default()
        };
        // (case, configuration, model, covered)
        let rows: Vec<(&str, EnforcedGuardrail, &str, bool)> = vec![
            (
                "no model enforcement at all",
                enforced(&[], &[]),
                "amazon.nova-lite-v1:0",
                true,
            ),
            (
                "the model itself is included",
                enforced(&["amazon.nova-lite-v1:0"], &[]),
                "amazon.nova-lite-v1:0",
                true,
            ),
            (
                "another model is included",
                enforced(&["anthropic.claude-3-haiku-20240307-v1:0"], &[]),
                "amazon.nova-lite-v1:0",
                false,
            ),
            (
                "the wildcard includes it",
                enforced(&[ALL_MODELS], &[]),
                "meta.llama3-8b-instruct-v1:0",
                true,
            ),
            (
                "the wildcard, less this model",
                enforced(&[ALL_MODELS], &["meta.llama3-8b-instruct-v1:0"]),
                "meta.llama3-8b-instruct-v1:0",
                false,
            ),
            (
                "included and excluded at once: the exclusion decides",
                enforced(&["amazon.nova-lite-v1:0"], &["amazon.nova-lite-v1:0"]),
                "amazon.nova-lite-v1:0",
                false,
            ),
            (
                "one of several included models",
                enforced(
                    &["amazon.titan-text-premier-v1:0", "amazon.nova-pro-v1:0"],
                    &["cohere.command-text-v14"],
                ),
                "amazon.nova-pro-v1:0",
                true,
            ),
        ];
        for (case, enforced, model_id, expected) in rows {
            let actual = enforced.covers_model(model_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: {model_id} coverage expected {expected}, got {actual}"
            );
        }
    }

    /// RULE: a configured word is blocked as a word, whatever its case and
    /// wherever in the text it sits, and a longer word that merely contains it is
    /// not it.
    #[test]
    fn a_configured_word_is_matched_as_a_word() {
        let enforced = EnforcedGuardrail {
            words: vec![word("zephyrine"), word("red pencil")],
            ..EnforcedGuardrail::default()
        };
        // (case, text, blocked)
        let rows: [(&str, &str, bool); 10] = [
            ("the word alone", "zephyrine", true),
            ("inside a sentence", "please describe zephyrine for me", true),
            ("capitalised", "Zephyrine is the token", true),
            ("shouted", "ZEPHYRINE", true),
            ("punctuated", "what about (zephyrine)?", true),
            ("part of a longer word", "zephyrines are different", false),
            ("prefixed", "prezephyrine", false),
            ("a longer word, then the word itself", "zephyrinex zephyrine", true),
            ("a configured phrase", "pass me the Red Pencil please", true),
            ("nothing configured in it", "a benign question", false),
        ];
        for (case, text, expected) in rows {
            let actual = enforced
                .input_intervention(&[text.to_string()])
                .is_some();
            assert_eq!(
                actual, expected,
                "case {case:?}: {text:?} expected blocked={expected}, got {actual}"
            );
        }
    }

    /// RULE: which side a word blocks is its own `inputAction`/`inputEnabled` and
    /// `outputAction`/`outputEnabled`: an entry naming no action blocks, one
    /// naming `NONE` does not, and a side whose evaluation is disabled is not
    /// evaluated.
    #[test]
    fn a_words_action_and_enabled_members_decide_which_side_it_blocks() {
        /// One policy entry, and which sides it must block.
        struct Case {
            case: &'static str,
            input_action: Option<&'static str>,
            input_enabled: Option<bool>,
            output_action: Option<&'static str>,
            output_enabled: Option<bool>,
            blocks_input: bool,
            blocks_output: bool,
        }
        let entry = |case,
                     input_action,
                     input_enabled,
                     output_action,
                     output_enabled,
                     blocks_input,
                     blocks_output| Case {
            case,
            input_action,
            input_enabled,
            output_action,
            output_enabled,
            blocks_input,
            blocks_output,
        };
        let rows = [
            entry("nothing named", None, None, None, None, true, true),
            entry(
                "BLOCK both ways",
                Some("BLOCK"),
                None,
                Some("BLOCK"),
                None,
                true,
                true,
            ),
            entry(
                "NONE on the input",
                Some("NONE"),
                None,
                Some("BLOCK"),
                None,
                false,
                true,
            ),
            entry(
                "NONE on the output",
                Some("BLOCK"),
                None,
                Some("NONE"),
                None,
                true,
                false,
            ),
            entry(
                "input evaluation off",
                None,
                Some(false),
                None,
                Some(true),
                false,
                true,
            ),
            entry(
                "both evaluations off",
                None,
                Some(false),
                None,
                Some(false),
                false,
                false,
            ),
        ];
        for row in rows {
            let case = row.case;
            let enforced = EnforcedGuardrail {
                words: vec![BlockedWord::new(
                    "zephyrine".to_string(),
                    row.input_action,
                    row.input_enabled,
                    row.output_action,
                    row.output_enabled,
                )],
                blocked_input_messaging: "blocked in".to_string(),
                blocked_outputs_messaging: "blocked out".to_string(),
                ..EnforcedGuardrail::default()
            };
            let text = "the word zephyrine is here".to_string();
            assert_eq!(
                enforced.input_intervention(::std::slice::from_ref(&text)),
                row.blocks_input.then_some("blocked in"),
                "case {case:?}: the input side must block={}",
                row.blocks_input
            );
            assert_eq!(
                enforced.output_intervention(&text),
                row.blocks_output.then_some("blocked out"),
                "case {case:?}: the output side must block={}",
                row.blocks_output
            );
        }
    }
}
