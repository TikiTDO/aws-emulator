// SPDX-License-Identifier: CC0-1.0

//! Sampling rules: the stored rule, the rules a submitted one has to satisfy,
//! the account's service-managed catch-all, and the writes `CreateSamplingRule`
//! / `UpdateSamplingRule` / `DeleteSamplingRule` make.
//!
//! One concern, one file: every surface that reads or writes a sampling rule
//! answers from this module, so a rule may not reach through an update a value
//! it could not have been created with, and the ARN a reader is answered is the
//! one that selects the rule back.

use std::collections::BTreeMap;

use super::{
    ComAmazonawsXrayAwsxrayState, SamplingRuleLookupError, ARN_NAMESPACE, MAX_SAMPLING_RULES,
    SAMPLING_RULE_EXISTS_MESSAGE,
};
use crate::ops::types::{SamplingRateBoost, SamplingRule, SamplingRuleRecord};

/// A stored sampling rule: the rule as it was accepted — its `RuleName` and the
/// `RuleARN` minted for it filled in — plus when it was created and last
/// modified. `CreateSamplingRule` and `GetSamplingRules` both answer from this.
#[derive(Debug, Clone)]
pub struct StoredSamplingRule {
    /// The accepted rule, with `RuleName` and `RuleARN` as stored.
    pub rule: SamplingRule,
    /// When the rule was created, in seconds of epoch time.
    pub created_at: f64,
    /// When the rule was last modified, in seconds of epoch time.
    pub modified_at: f64,
    /// The tags the rule carries, keyed by tag key — what `TagResource` and
    /// `ListTagsForResource` reach through
    /// [`tags_for_resource`](ComAmazonawsXrayAwsxrayState::tags_for_resource).
    pub tags: BTreeMap<String, String>,
}

impl StoredSamplingRule {
    /// This record as the modelled `SamplingRuleRecord`.
    ///
    /// The ONE place a stored rule becomes a wire record, so every operation
    /// that answers a `SamplingRuleRecord` — `CreateSamplingRule`,
    /// `GetSamplingRules`, `UpdateSamplingRule`, `DeleteSamplingRule` — answers
    /// the same shape for the same rule.
    pub fn to_record(&self) -> SamplingRuleRecord {
        let mut rule = self.rule.clone();
        // `SamplingRule$Attributes` targets `AttributeMap`, modelled
        // `@length(min: 0, max: 5)`
        // (`smithy_query(query="shape",shape="AttributeMap")`): a map whose
        // minimum length is zero, so the EMPTY map is a legal value of the
        // member rather than an absent member. A rule that matches on no
        // attribute therefore serializes `Attributes` as `{}`, which is what an
        // attribute-matching client iterates, instead of omitting it.
        let attributes = rule.attributes.take().unwrap_or_default();
        rule.attributes = Some(attributes);
        SamplingRuleRecord {
            sampling_rule: Some(rule),
            created_at: Some(::aws_smithy_types::DateTime::from_secs_f64(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from_secs_f64(self.modified_at)),
        }
    }
}

/// Why a submitted `SamplingRule` cannot be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplingRuleRejection {
    /// The rule is refused for what it says: a bad selector, or a member value
    /// the service does not accept. A client-input refusal.
    Invalid(String),
    /// The store already holds [`MAX_SAMPLING_RULES`] rules. This is the one
    /// rejection `RuleLimitExceededException` reports.
    LimitExceeded(String),
}

impl SamplingRuleRejection {
    /// What to tell the caller.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::LimitExceeded(message) => message,
        }
    }
}

/// The `RuleName` a submitted rule is to be stored under, refusing a rule whose
/// members cannot describe one.
///
/// The rules come from the member documentation
/// (`smithy_query(query="shape",shape="SamplingRule")`): a rule is specified
/// "by either name or ARN, but not both", `Version` is "the version of the
/// sampling rule format (`1`)", and `ResourceARN` "matches the ARN of the Amazon
/// Web Services resource on which the service runs" — an ARN, or `*` for any.
/// Every modelled `@length` / `@range` bound is refused at the boundary before a
/// handler sees the request, so none of them is re-checked here.
pub fn sampling_rule_name(rule: &SamplingRule) -> Result<&str, SamplingRuleRejection> {
    let invalid =
        |message: String| -> SamplingRuleRejection { SamplingRuleRejection::Invalid(message) };
    let name = match (rule.rule_name.as_deref(), rule.rule_arn.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "SamplingRule must specify a rule by either RuleName or RuleARN, but not both"
                    .to_string(),
            ))
        }
        (None, _) => {
            return Err(invalid(
                "SamplingRule.RuleName is required to create a sampling rule; its RuleARN is minted by X-Ray".to_string(),
            ))
        }
        (Some(name), None) => name,
    };
    if rule.version != SAMPLING_RULE_FORMAT_VERSION {
        return Err(invalid(format!(
            "SamplingRule.Version must be {SAMPLING_RULE_FORMAT_VERSION}, the version of the sampling rule format, not {}",
            rule.version
        )));
    }
    validate_resource_arn(&rule.resource_arn)?;
    Ok(name)
}

/// Check a submitted `ResourceARN`: "matches the ARN of the Amazon Web Services
/// resource on which the service runs"
/// (`smithy_query(query="shape",shape="SamplingRule")`) — so what is judged is
/// ARN SYNTAX, and the one syntax rule a value can break here is the prefix
/// every ARN starts with. [`ANY_RESOURCE_ARN`] matches any resource, and a value
/// that names no resource at all (the empty string) constrains nothing, so
/// neither is an ARN that has to parse. Written once, because a rule may not
/// reach a value through `UpdateSamplingRule` that `CreateSamplingRule` refuses.
pub fn validate_resource_arn(resource_arn: &str) -> Result<(), SamplingRuleRejection> {
    if resource_arn.is_empty() || resource_arn == ANY_RESOURCE_ARN {
        return Ok(());
    }
    if !resource_arn.starts_with(ARN_PREFIX) {
        return Err(SamplingRuleRejection::Invalid(format!(
            "Invalid resource ARN: {resource_arn}. Malformed ARN - doesn't start with '{ARN_PREFIX}'"
        )));
    }
    Ok(())
}

/// The bounds the modelled `SamplingRule` members carry, as
/// `smithy_query(query="shape",shape="Priority,FixedRate,ReservoirSize")` states
/// them: `Priority` `@range(min: 1, max: 9999)`, `FixedRate` `@range(min: 0,
/// max: 1)`, `ReservoirSize` `@range(min: 0)`.
///
/// They are re-checked HERE for `UpdateSamplingRule`, whose own members target
/// the unconstrained `NullableInteger` / `NullableDouble`, so the boundary
/// refuses none of them: a rule may not reach through an update a value it could
/// not have been created with.
pub const SAMPLING_RULE_PRIORITY_MIN: i32 = 1;
/// The modelled maximum of `SamplingRule$Priority`.
pub const SAMPLING_RULE_PRIORITY_MAX: i32 = 9999;
/// The modelled minimum of `SamplingRule$FixedRate`.
pub const SAMPLING_RULE_FIXED_RATE_MIN: f64 = 0.0;
/// The modelled maximum of `SamplingRule$FixedRate`.
pub const SAMPLING_RULE_FIXED_RATE_MAX: f64 = 1.0;
/// The modelled minimum of `SamplingRule$ReservoirSize`.
pub const SAMPLING_RULE_RESERVOIR_SIZE_MIN: i32 = 0;

/// What X-Ray answers an `UpdateSamplingRule` carrying a value outside one of
/// the BOUNDS the modelled `SamplingRule` declares: the request as a whole is
/// not a usable update.
///
/// FIXED text: `SamplingRuleUpdate`'s own members carry no such bound for the
/// boundary to report (they target the unconstrained `NullableInteger` /
/// `NullableDouble`), so this refusal is the service's own and it states the
/// condition alone — neither the member that was out of range, nor the value it
/// carried, nor the bound reaches it.
///
/// It is the sentence for THAT class and not for every refusal of an update: a
/// `ResourceARN` that is not ARN syntax is a different class, judged by
/// [`validate_resource_arn`] and stated in the sentence that value earns
/// wherever it is submitted — through `CreateSamplingRule` as well, which is why
/// the check and its wording live in one place.
pub const INVALID_SAMPLING_RULE_UPDATE_MESSAGE: &str = "Invalid update request for sampling rule";

/// Check an update's values against those bounds, before anything is written.
///
/// A value outside one of them makes the whole update unusable, which is what
/// the refusal states ([`INVALID_SAMPLING_RULE_UPDATE_MESSAGE`]): one sentence
/// for the request, whichever of those members carried the value and whatever it
/// was. A `ResourceARN` is the other class, and keeps its own sentence.
pub fn validate_sampling_rule_update(
    update: &SamplingRuleUpdateDraft,
) -> Result<(), SamplingRuleRejection> {
    let unusable = || -> SamplingRuleRejection {
        SamplingRuleRejection::Invalid(INVALID_SAMPLING_RULE_UPDATE_MESSAGE.to_string())
    };
    if let Some(resource_arn) = update.resource_arn.as_deref() {
        validate_resource_arn(resource_arn)?;
    }
    if let Some(priority) = update.priority {
        if !(SAMPLING_RULE_PRIORITY_MIN..=SAMPLING_RULE_PRIORITY_MAX).contains(&priority) {
            return Err(unusable());
        }
    }
    if let Some(fixed_rate) = update.fixed_rate {
        if !(SAMPLING_RULE_FIXED_RATE_MIN..=SAMPLING_RULE_FIXED_RATE_MAX).contains(&fixed_rate) {
            return Err(unusable());
        }
    }
    if let Some(reservoir_size) = update.reservoir_size {
        if reservoir_size < SAMPLING_RULE_RESERVOIR_SIZE_MIN {
            return Err(unusable());
        }
    }
    Ok(())
}

/// The only `SamplingRule$Version` X-Ray accepts: "the version of the sampling
/// rule format (`1`)".
pub const SAMPLING_RULE_FORMAT_VERSION: i32 = 1;

/// The `SamplingRule$ResourceARN` that matches any resource.
pub const ANY_RESOURCE_ARN: &str = "*";

/// What every Amazon Resource Name begins with — the one piece of ARN syntax a
/// submitted `ResourceARN` is judged against ([`validate_resource_arn`]).
pub const ARN_PREFIX: &str = "arn:";

/// The `SamplingRule` matcher value that matches anything: the wildcard every
/// matcher of the service-managed catch-all rule carries.
pub const ANY_MATCHER: &str = "*";

/// The name of the sampling rule every X-Ray account holds without anyone
/// creating it.
///
/// X-Ray seeds each account with one service-managed catch-all rule, which
/// `GetSamplingRules` always lists and which no `DeleteSamplingRule` removes;
/// every X-Ray SDK reads it at startup as its fallback sampling policy. The
/// model does not describe it — `GetSamplingRulesResult$SamplingRuleRecords` is
/// just "rule definitions and metadata" — so the rule is recorded service
/// behaviour, written down here once.
pub const DEFAULT_SAMPLING_RULE_NAME: &str = "Default";

/// What X-Ray answers a `DeleteSamplingRule` that selects the account's
/// service-managed rule.
///
/// FIXED text: the rule is protected for WHAT it is, so the refusal states that
/// condition and interpolates nothing — neither the selector the request used
/// nor the reserved name itself — whichever of the two selectors named it.
pub const DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE: &str = "Cannot delete default sampling rule";

/// The service-managed rule's `Priority`, deliberately PAST the `@range(min: 1,
/// max: 9999)` a caller may set on a rule of its own
/// (`smithy_query(query="shape",shape="Priority")`), so no created rule can ever
/// sort behind the account's catch-all.
pub const DEFAULT_SAMPLING_RULE_PRIORITY: i32 = 10_000;

/// The service-managed rule's `FixedRate`: it instruments 5% of the requests
/// that reach it once its reservoir is spent.
pub const DEFAULT_SAMPLING_RULE_FIXED_RATE: f64 = 0.05;

/// The service-managed rule's `ReservoirSize`: one request per second is
/// instrumented before the fixed rate applies.
pub const DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE: i32 = 1;

/// The account's service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule, as a
/// caller in `region` reads it.
///
/// DERIVED, never seeded into the store: the rule exists in every account
/// without being created, so there is no record to insert and no request can
/// remove it. Its ARN is a pure function of the reserved name and the region —
/// `sampling-rule/Default`, with none of the suffixes a minted id carries — so
/// two reads answer the same ARN while a caller signing for another region reads
/// that region's. Its `CreatedAt`/`ModifiedAt` are the epoch rather than a
/// reading of the clock, because nothing created it and nothing has modified it:
/// a derived rule that reported "now" would change on every read.
pub fn default_sampling_rule(region: &str) -> StoredSamplingRule {
    StoredSamplingRule {
        rule: SamplingRule {
            rule_name: Some(DEFAULT_SAMPLING_RULE_NAME.to_string()),
            rule_arn: Some(sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME)),
            resource_arn: ANY_RESOURCE_ARN.to_string(),
            priority: DEFAULT_SAMPLING_RULE_PRIORITY,
            fixed_rate: DEFAULT_SAMPLING_RULE_FIXED_RATE,
            reservoir_size: DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE,
            service_name: ANY_MATCHER.to_string(),
            service_type: ANY_MATCHER.to_string(),
            host: ANY_MATCHER.to_string(),
            http_method: ANY_MATCHER.to_string(),
            url_path: ANY_MATCHER.to_string(),
            version: SAMPLING_RULE_FORMAT_VERSION,
            // The catch-all matches on no attribute; the empty map is what a
            // reader sees ([`StoredSamplingRule::to_record`]).
            attributes: None,
            sampling_rate_boost: None,
        },
        created_at: 0.0,
        modified_at: 0.0,
        tags: BTreeMap::new(),
    }
}

/// What `UpdateSamplingRule` supplies: the members of a sampling rule a caller
/// may change.
///
/// A `None` member is one the request OMITTED and so leaves alone — "a document
/// specifying CHANGES to a sampling rule's configuration"
/// (`smithy_query(query="shape",shape="SamplingRuleUpdate")`). The rule's
/// identity (`RuleName`, `RuleARN`) and its `Version` are not here: the update
/// shape carries no `Version` and its two selectors NAME the rule rather than
/// re-point it.
#[derive(Debug, Clone, Default)]
pub struct SamplingRuleUpdateDraft {
    /// The updated `ResourceARN`, when the request carried one.
    pub resource_arn: Option<String>,
    /// The updated `Priority`, when the request carried one.
    pub priority: Option<i32>,
    /// The updated `FixedRate`, when the request carried one.
    pub fixed_rate: Option<f64>,
    /// The updated `ReservoirSize`, when the request carried one.
    pub reservoir_size: Option<i32>,
    /// The updated `ServiceName`, when the request carried one.
    pub service_name: Option<String>,
    /// The updated `ServiceType`, when the request carried one.
    pub service_type: Option<String>,
    /// The updated `Host`, when the request carried one.
    pub host: Option<String>,
    /// The updated `HTTPMethod`, when the request carried one.
    pub http_method: Option<String>,
    /// The updated `URLPath`, when the request carried one.
    pub url_path: Option<String>,
    /// The updated `Attributes`, when the request carried them.
    pub attributes: Option<std::collections::HashMap<String, String>>,
    /// The updated `SamplingRateBoost`, when the request carried one.
    pub sampling_rate_boost: Option<SamplingRateBoost>,
}

impl ComAmazonawsXrayAwsxrayState {
    /// Create one sampling rule, keyed by its `RuleName`.
    ///
    /// The collision check and the insert are ONE step on the locked store, so N
    /// concurrent calls for one name have exactly one winner: every loser sees
    /// the winner's record already present.
    pub fn insert_sampling_rule(
        &mut self,
        region: &str,
        mut rule: SamplingRule,
        tags: BTreeMap<String, String>,
        now: f64,
    ) -> Result<&StoredSamplingRule, SamplingRuleRejection> {
        let name = sampling_rule_name(&rule)?.to_string();
        if self.sampling_rule_name_is_taken(&name) {
            return Err(SamplingRuleRejection::Invalid(
                SAMPLING_RULE_EXISTS_MESSAGE.to_string(),
            ));
        }
        if self.created_sampling_rule_count() >= MAX_SAMPLING_RULES {
            return Err(SamplingRuleRejection::LimitExceeded(format!(
                "this emulator holds the maximum of {MAX_SAMPLING_RULES} sampling rules"
            )));
        }
        rule.rule_arn = Some(sampling_rule_arn(region, &name));
        rule.rule_name = Some(name.clone());
        self.sampling_rules.insert(
            name.clone(),
            StoredSamplingRule {
                rule,
                created_at: now,
                modified_at: now,
                tags,
            },
        );
        // The account's sampling-rule configuration just changed, which is what
        // `GetSamplingTargets` reports as `LastRuleModification` so a service
        // knows to re-read the rules.
        self.sampling.rules_modified_at = now;
        Ok(&self.sampling_rules[&name])
    }

    /// The stored `RuleName` a `(RuleName, RuleARN)` pair selects, for a request
    /// addressed to `region`.
    ///
    /// `RuleName` and `RuleARN` are documented as "either name or ARN, but not
    /// both", so supplying neither or both is a refusal rather than a lookup. An
    /// ARN selects the rule whose ARN under THIS request's region it equals — an
    /// ARN naming another region names another region's rule — and it is matched
    /// by walking the stored rules, bounded by [`MAX_SAMPLING_RULES`], rather
    /// than through a second index, so there is only ever one copy of a rule.
    pub fn resolve_sampling_rule_name(
        &self,
        rule_name: Option<&str>,
        rule_arn: Option<&str>,
        region: &str,
    ) -> Result<String, SamplingRuleLookupError> {
        match (rule_name, rule_arn) {
            (None, None) => return Err(SamplingRuleLookupError::NoSelector),
            (Some(_), Some(_)) => return Err(SamplingRuleLookupError::BothSelectors),
            _ => {}
        }
        if let Some(name) = rule_name {
            if self.sampling_rules.contains_key(name) {
                return Ok(name.to_string());
            }
        }
        if let Some(arn) = rule_arn {
            if let Some(name) = self
                .sampling_rules
                .keys()
                .find(|name| sampling_rule_arn(region, name) == arn)
            {
                return Ok(name.clone());
            }
        }
        // Last the account's service-managed rule, which is derived rather than
        // stored ([`default_sampling_rule`]) and so is resolvable by name and by
        // its region's ARN without ever appearing in `sampling_rules`.
        if rule_name == Some(DEFAULT_SAMPLING_RULE_NAME)
            || rule_arn
                .is_some_and(|arn| arn == sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME))
        {
            return Ok(DEFAULT_SAMPLING_RULE_NAME.to_string());
        }
        Err(SamplingRuleLookupError::NotFound)
    }

    /// Whether `rule_name` already names a rule in the account, the
    /// service-managed [`DEFAULT_SAMPLING_RULE_NAME`] one included —
    /// `CreateSamplingRule`'s uniqueness check.
    pub fn sampling_rule_name_is_taken(&self, rule_name: &str) -> bool {
        rule_name == DEFAULT_SAMPLING_RULE_NAME || self.sampling_rules.contains_key(rule_name)
    }

    /// How many of the account's [`MAX_SAMPLING_RULES`] are spent: the rules it
    /// CREATED.
    ///
    /// A materialised record for the service-managed
    /// [`DEFAULT_SAMPLING_RULE_NAME`] rule is not one of them — nobody created
    /// it — so writing to that rule does not cost the account a rule it could
    /// otherwise create.
    pub fn created_sampling_rule_count(&self) -> usize {
        self.sampling_rules.len()
            - usize::from(self.sampling_rules.contains_key(DEFAULT_SAMPLING_RULE_NAME))
    }

    /// The account's [`DEFAULT_SAMPLING_RULE_NAME`] rule as a caller in `region`
    /// reads it: the configuration the stored record holds once a write has
    /// materialised one, else the derived rule — and in both cases the ARN
    /// DERIVED for `region`.
    ///
    /// The rule exists in every region without being created, so its ARN stays a
    /// pure function of the reserved name and the reader's region even after a
    /// write: the ARN a reader is answered is exactly the one that selects the
    /// rule for that reader
    /// ([`resolve_sampling_rule_name`](Self::resolve_sampling_rule_name)), which
    /// a stored ARN from whichever region wrote first would not be.
    pub fn default_sampling_rule_record(&self, region: &str) -> StoredSamplingRule {
        let mut record = self
            .sampling_rules
            .get(DEFAULT_SAMPLING_RULE_NAME)
            .cloned()
            .unwrap_or_else(|| default_sampling_rule(region));
        record.rule.rule_arn = Some(sampling_rule_arn(region, DEFAULT_SAMPLING_RULE_NAME));
        record
    }

    /// Every sampling rule a caller in `region` sees: the rules the account
    /// created, in `RuleName` order, and then the service-managed
    /// [`DEFAULT_SAMPLING_RULE_NAME`] rule.
    ///
    /// This is what `GetSamplingRules` lists, so the catch-all is visible on a
    /// fresh emulator exactly as it is in a fresh account, and an SDK reading
    /// the listing always finds a rule that matches every request. It is listed
    /// LAST, after the created rules, whether or not an update has materialised
    /// a record for it — which is also the order of its `Priority`, past every
    /// priority a created rule may carry. Bounded by [`MAX_SAMPLING_RULES`] + 1.
    pub fn visible_sampling_rules(&self, region: &str) -> Vec<StoredSamplingRule> {
        let mut visible: Vec<StoredSamplingRule> = self
            .sampling_rules
            .iter()
            .filter(|(name, _)| name.as_str() != DEFAULT_SAMPLING_RULE_NAME)
            .map(|(_, stored)| stored.clone())
            .collect();
        visible.push(self.default_sampling_rule_record(region));
        visible
    }

    /// The stored record for the rule named `rule_name`, for an operation that
    /// MUTATES it.
    ///
    /// The service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule is derived rather
    /// than stored, so the first write to it is what gives it a stored record —
    /// carrying the SAME derived members, so every selector that resolved it
    /// before still does. It is not a rule the account created, so materialising
    /// it takes none of the [`MAX_SAMPLING_RULES`] the account may create: the
    /// limit a caller observes is the same before and after that write. `None`
    /// means there is no record under that name to write to.
    pub(super) fn sampling_rule_mut(
        &mut self,
        region: &str,
        rule_name: &str,
    ) -> Option<&mut StoredSamplingRule> {
        if !self.sampling_rules.contains_key(rule_name) {
            if rule_name != DEFAULT_SAMPLING_RULE_NAME {
                return None;
            }
            self.sampling_rules.insert(
                DEFAULT_SAMPLING_RULE_NAME.to_string(),
                default_sampling_rule(region),
            );
        }
        self.sampling_rules.get_mut(rule_name)
    }

    /// Apply an `UpdateSamplingRule` to the rule `rule_name` names, answering
    /// the record as it now stands, or `None` when there is no record to write
    /// (see [`sampling_rule_mut`](Self::sampling_rule_mut)).
    ///
    /// Each member the update SUPPLIES replaces the stored one; each member it
    /// OMITS leaves the stored one as it is. The write lands in the STORE, so
    /// every later read of the rule — `GetSamplingRules`, `DeleteSamplingRule`,
    /// another update — reports the updated values rather than the ones it was
    /// created with, and `ModifiedAt` moves to `now` while `CreatedAt` does not.
    /// A rule's identity (its name and the ARN derived from it) and its tags are
    /// never touched, so no index needs maintenance here.
    pub fn apply_sampling_rule_update(
        &mut self,
        region: &str,
        rule_name: &str,
        update: SamplingRuleUpdateDraft,
        now: f64,
    ) -> Option<&StoredSamplingRule> {
        // The write is scoped, so the record can be answered by a fresh lookup
        // once the account's configuration instant has been stamped below.
        {
            let stored = self.sampling_rule_mut(region, rule_name)?;
            let rule = &mut stored.rule;
            if let Some(resource_arn) = update.resource_arn {
                rule.resource_arn = resource_arn;
            }
            if let Some(priority) = update.priority {
                rule.priority = priority;
            }
            if let Some(fixed_rate) = update.fixed_rate {
                rule.fixed_rate = fixed_rate;
            }
            if let Some(reservoir_size) = update.reservoir_size {
                rule.reservoir_size = reservoir_size;
            }
            if let Some(service_name) = update.service_name {
                rule.service_name = service_name;
            }
            if let Some(service_type) = update.service_type {
                rule.service_type = service_type;
            }
            if let Some(host) = update.host {
                rule.host = host;
            }
            if let Some(http_method) = update.http_method {
                rule.http_method = http_method;
            }
            if let Some(url_path) = update.url_path {
                rule.url_path = url_path;
            }
            if let Some(attributes) = update.attributes {
                rule.attributes = Some(attributes);
            }
            if let Some(sampling_rate_boost) = update.sampling_rate_boost {
                rule.sampling_rate_boost = Some(sampling_rate_boost);
            }
            stored.modified_at = now;
        }
        // The configuration a service reads changed, so every later
        // `GetSamplingTargets` tells it to re-read the rules.
        self.sampling.rules_modified_at = now;
        self.sampling_rules.get(rule_name)
    }

    /// Remove one sampling rule, answering the record that was removed.
    ///
    /// "Deletes a sampling rule" is the whole of the operation: the rule is gone
    /// from the store, so no later listing carries it and no later selector
    /// resolves it. The service-managed [`DEFAULT_SAMPLING_RULE_NAME`] rule is
    /// not removable — `DeleteSamplingRule` refuses it before reaching here —
    /// and removing its materialised record would only put the derived rule back
    /// in the listing.
    /// `now` is the instant the removal happened, supplied by the caller the
    /// same way the create and the update supply theirs, so every write to a
    /// rule stamps the account's configuration instant from ONE reading of the
    /// clock per request.
    pub fn remove_sampling_rule(
        &mut self,
        rule_name: &str,
        now: f64,
    ) -> Option<StoredSamplingRule> {
        let removed = self.sampling_rules.remove(rule_name)?;
        // A rule leaving the account is a change to the configuration a service
        // reads, exactly as creating or updating one is.
        self.sampling.rules_modified_at = now;
        Some(removed)
    }

    /// The rule `rule_name` names, as a READER in `region` sees it: the stored
    /// record, or the account's service-managed catch-all rule, which is derived
    /// rather than stored ([`default_sampling_rule`]).
    ///
    /// `None` means the account holds no rule under that name, which is what
    /// `GetSamplingTargets` reports as unprocessed statistics rather than
    /// serving an allocation for a rule nobody configured.
    pub fn visible_sampling_rule(
        &self,
        region: &str,
        rule_name: &str,
    ) -> Option<StoredSamplingRule> {
        if rule_name == DEFAULT_SAMPLING_RULE_NAME {
            return Some(self.default_sampling_rule_record(region));
        }
        self.sampling_rules.get(rule_name).cloned()
    }
}

/// A sampling rule's ARN, `arn:aws:xray:<region>:<account>:sampling-rule/<RuleName>`.
///
/// Derived from the name rather than minted with a unique suffix, because a
/// rule's name is unique in the account for as long as the rule exists — so an
/// ARN resolves back to a name without a second index.
pub fn sampling_rule_arn(region: &str, rule_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("sampling-rule/{rule_name}"),
    )
}
