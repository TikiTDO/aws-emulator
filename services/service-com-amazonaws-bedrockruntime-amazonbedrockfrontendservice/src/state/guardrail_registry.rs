// SPDX-License-Identifier: CC0-1.0

//! The guardrail REVISIONS this data plane can apply, as the Bedrock control plane
//! published them.
//!
//! WHY THEY LIVE HERE. A guardrail is a CONTROL-plane resource, but applying one is a
//! data-plane call: `ApplyGuardrail` names a guardrail revision by identifier and
//! version and evaluates submitted content against the policies that revision
//! configures. So the policies have to be readable on the inference path, and — as
//! with the account-enforced guardrail ([`super::enforced_guardrail`]) — the one home
//! for them is this service's own store: the engine keys state by type, the control
//! plane writes a revision here on the call that creates, updates, versions or
//! deletes it, and the two planes cannot drift into disagreeing about what a
//! guardrail blocks.
//!
//! WHAT A REVISION HOLDS. Only what EVALUATION needs: the two blocked-messaging
//! strings, and each policy as a list of entries carrying, per side, what that entry
//! DOES on a match ([`PolicyAction`]) — block the content, anonymize it, or report the
//! detection and take no action. An entry whose evaluation the configuration turned
//! off on a side is not evaluated on that side at all, which is the absence of an
//! action there. Everything a guardrail resource otherwise carries — its name, tags,
//! KMS key, timestamps and status — is the control plane's and stays there.
//!
//! HOW MUCH ONE EVALUATION COSTS. The lists an evaluation walks are bounded when they
//! are published: the word list by the same count and byte ceilings the enforced
//! guardrail bounds its own by ([`super::enforced_guardrail::MAX_ENFORCED_WORDS`]),
//! and the other policies by [`MAX_PUBLISHED_ENTRIES`] entries each. The number of
//! guardrails held is bounded by the control plane's own account quota, since this
//! registry holds one record per guardrail that plane holds.

use std::collections::BTreeMap;

use super::enforced_guardrail::{MAX_ENFORCED_WORDS, MAX_ENFORCED_WORD_BYTES};

/// `GuardrailVersion`'s one non-numeric value: the guardrail's working revision.
pub const DRAFT_VERSION: &str = "DRAFT";

/// The actions a guardrail policy entry is CONFIGURED with, as the control plane's
/// own `GuardrailWordAction` / `GuardrailContentFilterAction` /
/// `GuardrailTopicAction` / `GuardrailSensitiveInformationAction` /
/// `GuardrailContextualGroundingAction` spell them.
pub const CONFIG_ACTION_BLOCK: &str = "BLOCK";
pub const CONFIG_ACTION_NONE: &str = "NONE";
pub const CONFIG_ACTION_ANONYMIZE: &str = "ANONYMIZE";

/// The actions an ASSESSMENT reports, as this service's own
/// `GuardrailWordPolicyAction` / `GuardrailContentPolicyAction` /
/// `GuardrailTopicPolicyAction` / `GuardrailSensitiveInformationPolicyAction` /
/// `GuardrailContextualGroundingPolicyAction` enums spell them: the configured
/// vocabulary above is the imperative, these are the past tense of it.
pub const ASSESSED_BLOCKED: &str = "BLOCKED";
pub const ASSESSED_NONE: &str = "NONE";
pub const ASSESSED_ANONYMIZED: &str = "ANONYMIZED";

/// `GuardrailManagedWordsType$PROFANITY` — the one managed word list a guardrail can
/// configure.
pub const MANAGED_WORDS_PROFANITY: &str = "PROFANITY";

/// `GuardrailTopicType$DENY` — the one topic type a guardrail can configure, and the
/// one value the assessment's own `GuardrailTopicType` declares.
pub const TOPIC_TYPE_DENY: &str = "DENY";

/// `GuardrailContextualGroundingFilterType` — the two things a contextual grounding
/// filter scores: how far the guarded content is supported by the grounding source,
/// and how far it answers the query.
pub const GROUNDING_FILTER_GROUNDING: &str = "GROUNDING";
pub const GROUNDING_FILTER_RELEVANCE: &str = "RELEVANCE";

/// The most entries of one policy an evaluation walks.
///
/// The EMULATOR's bound on the work one request can be made to do, not an AWS quota:
/// every policy here is walked once per evaluation, so the published list is cut to
/// this rather than the publication being refused — the control plane's own record of
/// the guardrail is unaffected. It is far above every modelled policy ceiling (a
/// topic policy is `@length(max: 30)`, a content policy's filters one per category, a
/// sensitive-information policy one per entity type), so no configuration a caller
/// can make reaches it.
pub const MAX_PUBLISHED_ENTRIES: usize = 256;

/// The most characters of one configured regular expression this plane compiles.
///
/// The EMULATOR's bound, beside [`MAX_PUBLISHED_ENTRIES`]: the pattern comes from the
/// account's configuration and is compiled by the linear-time engine this crate
/// matches untrusted text with, so the cost of one evaluation is fixed by the
/// publication rather than by the request. A longer pattern is not published.
pub const MAX_PUBLISHED_REGEX_CHARACTERS: usize = 1024;

/// What this plane answers a request naming a guardrail revision this account does
/// not hold.
///
/// The sentence Amazon Bedrock answers a request whose guardrail revision does not
/// exist with: it names the two parts of the request that address a revision — the
/// identifier and the version — without repeating either value, so the wording is the
/// whole requirement, and the same sentence stands for a guardrail that is not there
/// and for a version of it that was never cut. It is a failure of the request's own
/// INPUT, which is what an identifier naming no guardrail of this account is, so the
/// surfaces that apply a guardrail answer it with their modelled
/// `ValidationException`.
pub const GUARDRAIL_NOT_FOUND: &str =
    "The guardrail identifier or version provided in the request does not exist.";

/// What one policy entry DOES where it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Block the content and answer the guardrail's blocked messaging.
    Block,
    /// Mask the matched value in the content the guardrail answers with.
    Anonymize,
    /// Take no action, and report the detection — what a configuration asks for to
    /// get detection without blocking.
    Detect,
}

impl PolicyAction {
    /// The action an entry takes on one side, or `None` where the configuration
    /// turned that side's evaluation OFF.
    ///
    /// An entry that says nothing about whether evaluation is enabled is evaluated:
    /// the member exists to turn an evaluation off. An entry naming no action on a
    /// side takes `default`, which is the policy-wide action where its shape carries
    /// one (a sensitive-information entry's `action`) and blocking where it does not.
    #[must_use]
    pub fn of(action: Option<&str>, enabled: Option<bool>, default: PolicyAction) -> Option<Self> {
        if !enabled.unwrap_or(true) {
            return None;
        }
        Some(match action {
            Some(CONFIG_ACTION_BLOCK) => Self::Block,
            Some(CONFIG_ACTION_ANONYMIZE) => Self::Anonymize,
            Some(CONFIG_ACTION_NONE) => Self::Detect,
            None => default,
            // A value outside the vocabulary this plane reads is taken as the
            // policy's own action rather than silently turning the entry off, and is
            // reported where a developer can see that the configuration named
            // something this plane does not model.
            Some(unmodelled) => {
                ::tracing::warn!(
                    action = %unmodelled,
                    "a guardrail policy entry names an action this plane does not read"
                );
                default
            }
        })
    }

    /// How an assessment reports this action.
    #[must_use]
    pub fn assessed(self) -> &'static str {
        match self {
            Self::Block => ASSESSED_BLOCKED,
            Self::Anonymize => ASSESSED_ANONYMIZED,
            Self::Detect => ASSESSED_NONE,
        }
    }

    /// Whether a match of this entry blocks the content.
    #[must_use]
    pub fn blocks(self) -> bool {
        matches!(self, Self::Block)
    }

    /// Whether a match of this entry masks the matched value.
    #[must_use]
    pub fn anonymizes(self) -> bool {
        matches!(self, Self::Anonymize)
    }
}

/// What a policy entry does on each side of an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sides {
    pub input: Option<PolicyAction>,
    pub output: Option<PolicyAction>,
}

impl Sides {
    /// The two sides of a policy entry as its configuration states them.
    #[must_use]
    pub fn new(
        input_action: Option<&str>,
        input_enabled: Option<bool>,
        output_action: Option<&str>,
        output_enabled: Option<bool>,
        default: PolicyAction,
    ) -> Self {
        Self {
            input: PolicyAction::of(input_action, input_enabled, default),
            output: PolicyAction::of(output_action, output_enabled, default),
        }
    }

    /// Both sides evaluated under one action: the shape of an entry whose
    /// configuration states no side of its own.
    #[must_use]
    pub fn both(action: PolicyAction) -> Self {
        Self {
            input: Some(action),
            output: Some(action),
        }
    }

    /// What this entry does on the side an evaluation is running, or `None` where it
    /// is not evaluated there.
    #[must_use]
    pub fn on(&self, input: bool) -> Option<PolicyAction> {
        if input {
            self.input
        } else {
            self.output
        }
    }
}

/// One topic a guardrail's topic policy denies.
///
/// The DEFINITION a topic carries is natural language for a model to read, and no
/// model runs in this emulator, so what is published is the topic's NAME and the
/// sample phrases it was configured with — the two parts of a topic a recognizer can
/// place text against (see [`super::guardrail_evaluation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeniedTopic {
    pub name: String,
    /// `GuardrailTopic$type` — `DENY` for every topic a guardrail configures.
    pub kind: String,
    /// `GuardrailTopicConfig$examples` — the sample phrases of the topic.
    pub examples: Vec<String>,
    pub sides: Sides,
}

/// One filter a guardrail's content policy configures: the modelled category it
/// filters and the strength it filters that category at on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterEntry {
    /// A value of the modelled `GuardrailContentFilterType` enum.
    pub category: String,
    /// `inputStrength` — `GuardrailFilterStrength`, the strength the category is
    /// filtered at on the input side.
    pub input_strength: String,
    /// `outputStrength` — the same for the output side.
    pub output_strength: String,
    pub sides: Sides,
}

impl FilterEntry {
    /// The strength this filter is configured at on one side.
    #[must_use]
    pub fn strength(&self, input: bool) -> &str {
        if input {
            &self.input_strength
        } else {
            &self.output_strength
        }
    }
}

/// One word a guardrail's word policy configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordEntry {
    /// `GuardrailWord$text`, as configured.
    pub text: String,
    pub sides: Sides,
}

/// One managed word list a guardrail's word policy configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWordEntry {
    /// `GuardrailManagedWords$type` — today [`MANAGED_WORDS_PROFANITY`].
    pub kind: String,
    pub sides: Sides,
}

/// One entity type a guardrail's sensitive-information policy configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiiEntry {
    /// A value of the modelled `GuardrailPiiEntityType` enum.
    pub entity_type: String,
    pub sides: Sides,
}

/// One regular expression a guardrail's sensitive-information policy configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexEntry {
    pub name: String,
    pub pattern: String,
    pub sides: Sides,
}

/// One filter a guardrail's contextual grounding policy configures: what it scores
/// and the score below which it acts.
///
/// A contextual grounding filter carries ONE action and one enabled flag rather than
/// a pair per side (`GuardrailContextualGroundingFilterConfig`), so it is evaluated
/// under that one action wherever it is evaluated.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundingEntry {
    /// [`GROUNDING_FILTER_GROUNDING`] or [`GROUNDING_FILTER_RELEVANCE`].
    pub kind: String,
    /// `threshold` — the modelled 0..1 score the filter acts below.
    pub threshold: f64,
    /// `None` where the configuration turned this filter off.
    pub action: Option<PolicyAction>,
}

/// One guardrail revision, as this data plane applies it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuardrailPolicies {
    /// `blockedInputMessaging` — what the guardrail answers an input it blocked with.
    pub blocked_input_messaging: String,
    /// `blockedOutputsMessaging` — the same for content on the output side.
    pub blocked_outputs_messaging: String,
    pub topics: Vec<DeniedTopic>,
    pub content_filters: Vec<FilterEntry>,
    pub words: Vec<WordEntry>,
    pub managed_word_lists: Vec<ManagedWordEntry>,
    pub pii_entities: Vec<PiiEntry>,
    pub regexes: Vec<RegexEntry>,
    pub grounding_filters: Vec<GroundingEntry>,
}

impl GuardrailPolicies {
    /// Whether the revision configures a topic policy that is evaluated on this side.
    #[must_use]
    pub fn evaluates_topics(&self, input: bool) -> bool {
        self.topics.iter().any(|topic| topic.sides.on(input).is_some())
    }

    /// Whether it configures a content policy that is evaluated on this side.
    #[must_use]
    pub fn evaluates_content(&self, input: bool) -> bool {
        self.content_filters
            .iter()
            .any(|filter| filter.sides.on(input).is_some())
    }

    /// Whether it configures a word policy that is evaluated on this side.
    #[must_use]
    pub fn evaluates_words(&self, input: bool) -> bool {
        self.words
            .iter()
            .any(|word| word.sides.on(input).is_some())
            || self
                .managed_word_lists
                .iter()
                .any(|list| list.sides.on(input).is_some())
    }

    /// Whether it configures PII entity types that are evaluated on this side.
    #[must_use]
    pub fn evaluates_pii(&self, input: bool) -> bool {
        self.pii_entities
            .iter()
            .any(|entity| entity.sides.on(input).is_some())
    }

    /// Whether it configures regular expressions that are evaluated on this side.
    #[must_use]
    pub fn evaluates_regexes(&self, input: bool) -> bool {
        self.regexes
            .iter()
            .any(|regex| regex.sides.on(input).is_some())
    }

    /// Whether it configures a contextual grounding filter that is evaluated.
    #[must_use]
    pub fn evaluates_grounding(&self) -> bool {
        self.grounding_filters
            .iter()
            .any(|filter| filter.action.is_some())
    }

    /// The messaging this revision answers content it blocked on one side with.
    #[must_use]
    pub fn blocked_messaging(&self, input: bool) -> &str {
        if input {
            &self.blocked_input_messaging
        } else {
            &self.blocked_outputs_messaging
        }
    }

    /// Cut every policy list to the ceilings one evaluation is bounded by.
    fn bounded(mut self) -> Self {
        self.words.truncate(MAX_ENFORCED_WORDS);
        let mut bytes = 0usize;
        self.words.retain(|word| {
            bytes = bytes.saturating_add(word.text.len());
            bytes <= MAX_ENFORCED_WORD_BYTES
        });
        self.topics.truncate(MAX_PUBLISHED_ENTRIES);
        self.content_filters.truncate(MAX_PUBLISHED_ENTRIES);
        self.managed_word_lists.truncate(MAX_PUBLISHED_ENTRIES);
        self.pii_entities.truncate(MAX_PUBLISHED_ENTRIES);
        self.regexes
            .retain(|regex| regex.pattern.chars().count() <= MAX_PUBLISHED_REGEX_CHARACTERS);
        self.regexes.truncate(MAX_PUBLISHED_ENTRIES);
        self.grounding_filters.truncate(MAX_PUBLISHED_ENTRIES);
        self
    }
}

/// A guardrail this data plane can apply: the identity a request resolves to, and
/// every revision of it the control plane published.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublishedGuardrail {
    /// `guardrailId`, as the control plane minted it.
    pub id: String,
    /// `guardrailArn` of that same guardrail.
    pub arn: String,
    /// Each revision under the `guardrailVersion` that names it:
    /// [`DRAFT_VERSION`], or the decimal version number.
    pub revisions: BTreeMap<String, GuardrailPolicies>,
}

/// The revision one request resolved to, with the identity of the guardrail it
/// belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGuardrail {
    pub id: String,
    pub arn: String,
    pub version: String,
    pub policies: GuardrailPolicies,
}

/// Publish `guardrail` — every revision of it — as the data plane applies it,
/// replacing whatever stood under its id.
///
/// The Bedrock CONTROL plane calls this on each request that creates, updates or
/// versions a guardrail, so the policies a caller configured are the ones the next
/// apply is evaluated against. Answers whether the store was reached: an emulator
/// assembled without this data plane applies nothing, and the control plane's own
/// record is unaffected either way.
///
/// Call it with NO store of the calling service held, so two services' stores are
/// never locked at once.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, guardrail: PublishedGuardrail) -> bool {
    let Some(state) = super::service_state(engine) else {
        return false;
    };
    let id = guardrail.id.clone();
    let bounded = PublishedGuardrail {
        revisions: guardrail
            .revisions
            .into_iter()
            .map(|(version, policies)| (version, policies.bounded()))
            .collect(),
        ..guardrail
    };
    state.lock().guardrails.insert(id, bounded);
    true
}

/// Withdraw the guardrail `guardrail_id` names, which the control plane's delete of
/// it does: a guardrail that is gone applies to nothing.
pub fn withdraw<Eng: Send + Sync + 'static>(engine: &Eng, guardrail_id: &str) -> bool {
    let Some(state) = super::service_state(engine) else {
        return false;
    };
    state.lock().guardrails.remove(guardrail_id);
    true
}

/// The revision `identifier` and `version` name, or `None` where this account holds
/// no such guardrail or that guardrail holds no such revision.
///
/// `identifier` is either spelling the member's pattern admits — the bare
/// `guardrailId` or the full `guardrailArn` — and `version` is read the way the
/// control plane reads it: [`DRAFT_VERSION`] names the working revision and a decimal
/// number names that numbered snapshot, so no other value names a revision.
#[must_use]
pub fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
    version: &str,
) -> Option<ResolvedGuardrail> {
    let state = super::service_state(engine)?;
    let store = state.lock();
    // The map is keyed by `guardrailId`, which is one of the two spellings the
    // member's pattern admits; the ARN is resolved against the stored record's own
    // ARN, so a guardrail is reached by either spelling and by nothing else.
    let guardrail = match store.guardrails.get(identifier) {
        Some(guardrail) => guardrail,
        None => store
            .guardrails
            .values()
            .find(|guardrail| guardrail.arn == identifier)?,
    };
    let policies = guardrail.revisions.get(version)?;
    Some(ResolvedGuardrail {
        id: guardrail.id.clone(),
        arn: guardrail.arn.clone(),
        version: version.to_string(),
        policies: policies.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One policy entry's configured action and enabled flag, and what that makes
    /// the entry do.
    struct SideCase {
        case: &'static str,
        action: Option<&'static str>,
        enabled: Option<bool>,
        /// The policy's own action, which an entry naming none takes.
        default: PolicyAction,
        expected: Option<PolicyAction>,
    }

    /// RULE: an entry's action and enabled members decide what it does on each side —
    /// an entry naming no action takes the policy's own action, one naming `NONE`
    /// reports without acting, and a side whose evaluation is disabled is not
    /// evaluated at all.
    #[test]
    fn a_policy_entrys_action_and_enabled_members_decide_each_side() {
        let entry = |case, action, enabled, default, expected| SideCase {
            case,
            action,
            enabled,
            default,
            expected,
        };
        let rows = [
            entry(
                "nothing named",
                None,
                None,
                PolicyAction::Block,
                Some(PolicyAction::Block),
            ),
            entry(
                "nothing named, anonymizing policy",
                None,
                None,
                PolicyAction::Anonymize,
                Some(PolicyAction::Anonymize),
            ),
            entry(
                "BLOCK named",
                Some(CONFIG_ACTION_BLOCK),
                None,
                PolicyAction::Anonymize,
                Some(PolicyAction::Block),
            ),
            entry(
                "ANONYMIZE named",
                Some(CONFIG_ACTION_ANONYMIZE),
                None,
                PolicyAction::Block,
                Some(PolicyAction::Anonymize),
            ),
            entry(
                "NONE named",
                Some(CONFIG_ACTION_NONE),
                None,
                PolicyAction::Block,
                Some(PolicyAction::Detect),
            ),
            entry(
                "evaluation off",
                None,
                Some(false),
                PolicyAction::Block,
                None,
            ),
            entry(
                "evaluation on, action named",
                Some(CONFIG_ACTION_BLOCK),
                Some(true),
                PolicyAction::Block,
                Some(PolicyAction::Block),
            ),
        ];
        for row in rows {
            let case = row.case;
            let expected = row.expected;
            let actual = PolicyAction::of(row.action, row.enabled, row.default);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a published revision is resolved by either spelling of the guardrail
    /// identifier and by the version that names it, and by nothing else.
    #[test]
    fn a_revision_is_resolved_by_identifier_and_version() {
        let engine = ::std::sync::Arc::new(::engine::InMemoryEngine::new());
        let mut revisions = BTreeMap::new();
        revisions.insert(DRAFT_VERSION.to_string(), GuardrailPolicies::default());
        revisions.insert(
            "1".to_string(),
            GuardrailPolicies {
                blocked_input_messaging: "blocked".to_string(),
                ..GuardrailPolicies::default()
            },
        );
        assert!(publish(
            &*engine,
            PublishedGuardrail {
                id: "abc123".to_string(),
                arn: "arn:aws:bedrock:us-east-1:000000000000:guardrail/abc123".to_string(),
                revisions,
            }
        ));

        // (case, identifier, version, resolves)
        let rows: [(&str, &str, &str, bool); 6] = [
            ("the bare id and the draft", "abc123", DRAFT_VERSION, true),
            ("the bare id and version 1", "abc123", "1", true),
            (
                "the ARN and version 1",
                "arn:aws:bedrock:us-east-1:000000000000:guardrail/abc123",
                "1",
                true,
            ),
            ("a version that was never cut", "abc123", "2", false),
            ("an empty version names none", "abc123", "", false),
            ("another guardrail's id", "def456", DRAFT_VERSION, false),
        ];
        for (case, identifier, version, expected) in rows {
            let actual = resolve(&*engine, identifier, version).is_some();
            assert_eq!(
                actual, expected,
                "case {case:?}: {identifier}/{version} expected resolved={expected}, got {actual}"
            );
        }

        assert!(withdraw(&*engine, "abc123"));
        let actual = resolve(&*engine, "abc123", DRAFT_VERSION);
        assert!(
            actual.is_none(),
            "case \"withdrawn\": a withdrawn guardrail resolves to nothing, got {actual:?}"
        );
    }
}
