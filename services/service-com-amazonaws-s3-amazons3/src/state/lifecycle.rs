// SPDX-License-Identifier: CC0-1.0

//! What a bucket's stored lifecycle configuration IS and which configurations S3 admits:
//! the published bounds on a configuration, the rejection one that breaks them earns, and
//! the validators the lifecycle write judges a configuration by.

use super::*;

/// How many rules one lifecycle configuration may carry: "An Amazon S3 Lifecycle
/// configuration can have up to 1,000 rules. This limit is not adjustable"
/// (`PutBucketLifecycleConfiguration`'s own documentation).
pub const MAX_LIFECYCLE_RULES: usize = 1_000;

/// The longest rule identifier a lifecycle rule may carry: "Unique identifier for the
/// rule. The value cannot be longer than 255 characters"
/// (`smithy_query(query="shape",shape="LifecycleRule")`, member `ID`). The `ID` shape
/// itself carries no `@length`, so nothing refuses a longer one before this check.
pub const MAX_LIFECYCLE_RULE_ID_CHARS: usize = 255;

/// How many noncurrent versions a lifecycle rule may ask to retain: "You can specify up to
/// 100 noncurrent versions to retain"
/// (`NoncurrentVersionExpiration$NewerNoncurrentVersions`'s own documentation).
pub const MAX_NEWER_NONCURRENT_VERSIONS: i32 = 100;

/// The action a current-version transition is named by in a refusal, and the member of it
/// that carries the day count — the two names S3's transition refusals are built from.
///
/// `Transition$Days` is "indicates the number of days after creation when objects are
/// transitioned to the specified storage class"
/// (`smithy_query(query="shape",shape="Transition")`), and a refusal about it names the
/// action and that member, so both come from the model rather than from one request.
pub const TRANSITION_ACTION: (&str, &str) = ("Transition", "Days");

/// The same pair for a NONCURRENT-version transition: the action
/// `NoncurrentVersionTransition` and its own day member `NoncurrentDays`
/// (`smithy_query(query="shape",shape="NoncurrentVersionTransition")`).
pub const NONCURRENT_VERSION_TRANSITION_ACTION: (&str, &str) =
    ("NoncurrentVersionTransition", "NoncurrentDays");

/// The default minimum-object-size behaviour a lifecycle configuration carries when the
/// request names no `TransitionDefaultMinimumObjectSize`.
///
/// One of that member's two values
/// (`smithy_query(query="shape",shape="TransitionDefaultMinimumObjectSize")`), and the one
/// whose documentation states the DEFAULT: "`all_storage_classes_128K` - Objects smaller
/// than 128 KB will not transition to any storage class by default."
pub const DEFAULT_TRANSITION_MINIMUM_OBJECT_SIZE: &str = "all_storage_classes_128K";

/// The storage classes a lifecycle transition may name, WARMEST FIRST, each with its
/// published minimum storage duration in days.
///
/// The ORDER is the one the Amazon S3 User Guide's transition chart allows a lifecycle
/// rule to follow — Standard-IA, then Intelligent-Tiering, then One Zone-IA, then Glacier
/// Instant Retrieval, then Glacier Flexible Retrieval, then Glacier Deep Archive — and
/// transitions in the other direction, from a colder class to a warmer one, are not
/// supported at all. The DAYS are each class's minimum storage duration from the same
/// guide's storage-class comparison (30 days for the two infrequent-access classes, 90 for
/// the two Glacier retrieval classes, 180 for Deep Archive, none for Intelligent-Tiering),
/// which is how long an object stays in a class before a later transition may move it on.
///
/// Every value is one of `TransitionStorageClass`'s enum values
/// (`smithy_query(query="shape",shape="TransitionStorageClass")`).
pub const TRANSITION_STORAGE_CLASS_CHAIN: [(&str, i32); 6] = [
    ("STANDARD_IA", 30),
    ("INTELLIGENT_TIERING", 0),
    ("ONEZONE_IA", 30),
    ("GLACIER_IR", 90),
    ("GLACIER", 90),
    ("DEEP_ARCHIVE", 180),
];

/// Where `storage_class` sits in [`TRANSITION_STORAGE_CLASS_CHAIN`] — the lower the rank,
/// the warmer the class — or `None` for a value that is not a transition storage class.
pub fn transition_storage_class_rank(storage_class: &str) -> Option<usize> {
    TRANSITION_STORAGE_CLASS_CHAIN
        .iter()
        .position(|(class, _)| *class == storage_class)
}

/// The published minimum storage duration of `storage_class`, in days, or `None` for a
/// value that is not a transition storage class. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
pub fn minimum_storage_duration_days(storage_class: &str) -> Option<i32> {
    TRANSITION_STORAGE_CLASS_CHAIN
        .iter()
        .find(|(class, _)| *class == storage_class)
        .map(|(_, days)| *days)
}

/// Seconds in one day, the unit a lifecycle `Date` is judged in: the member's time "is
/// always midnight UTC" (`LifecycleExpiration$Date`), so the value is a whole number of
/// these.
pub(super) const SECONDS_PER_DAY: i64 = 86_400;

/// Whether `date` is midnight UTC — the only time of day a lifecycle `Date` may carry:
/// "The date value must conform to the ISO 8601 format. The time is always midnight UTC"
/// (`LifecycleExpiration$Date`, and `Transition$Date` says the same).
pub(super) fn is_utc_midnight(date: &::aws_smithy_types::DateTime) -> bool {
    date.subsec_nanos() == 0 && date.secs().rem_euclid(SECONDS_PER_DAY) == 0
}

/// The lifecycle configuration a bucket carries: the rules it was put with, and the
/// minimum-object-size behaviour that put applied to them.
///
/// The MODELLED rules are stored whole, the way [`Bucket::replication`] stores its
/// container: `Rules` is exactly what the put carries and what
/// `GetBucketLifecycleConfiguration` reports back, so a read answers the configuration the
/// bucket actually carries rather than a re-derived copy of it. Each rule's `ID` is filled
/// in here if the request omitted it ([`ComAmazonawsS3Amazons3State::put_lifecycle_configuration`]),
/// so every later read reports the same identifier.
#[derive(Debug, Clone, Default)]
pub struct BucketLifecycle {
    /// The configuration's rules, in the order the request carried them.
    pub rules: Vec<crate::ops::types::LifecycleRule>,
    /// Which default minimum object size behaviour applies to the configuration — the
    /// value the put named, or [`DEFAULT_TRANSITION_MINIMUM_OBJECT_SIZE`] when it named
    /// none. Reported back by both the put and the read.
    pub transition_default_minimum_object_size: String,
}

/// Why a lifecycle configuration may not be stored on a bucket.
///
/// Each variant names the member that is wrong and the published constraint it breaks, so
/// the refusal's message states the rule the caller has to satisfy rather than one
/// request's wording. [`Self::code`] is the wire error code each one goes out with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleConfigurationRejection {
    /// A configuration naming no rule at all: `BucketLifecycleConfiguration$Rules` is a
    /// REQUIRED member, so a document naming none does not validate against the schema S3
    /// publishes.
    NoRules,
    /// More rules than [`MAX_LIFECYCLE_RULES`].
    TooManyRules {
        /// How many the document carried.
        count: usize,
    },
    /// An `ID` longer than [`MAX_LIFECYCLE_RULE_ID_CHARS`] characters.
    IdTooLong {
        /// Which rule carries it, counted from 1 as the document orders them.
        rule: usize,
        /// How many characters it carried.
        length: usize,
    },
    /// Two rules carrying the same `ID`, which is "unique identifier for the rule".
    DuplicateId {
        /// Which rule repeats an identifier an earlier rule already carries.
        rule: usize,
        /// The repeated identifier.
        id: String,
    },
    /// A rule naming BOTH the legacy `Prefix` and a `Filter`: the two are alternatives —
    /// "`Filter` is required if the `LifecycleRule` does not contain a `Prefix` element"
    /// (`LifecycleRule$Filter`).
    PrefixAndFilter {
        /// Which rule names both.
        rule: usize,
    },
    /// A rule naming NEITHER a `Prefix` nor a `Filter`, so nothing says which objects it
    /// applies to. Same sentence as [`Self::PrefixAndFilter`].
    NoPrefixOrFilter {
        /// Which rule names neither.
        rule: usize,
    },
    /// One configuration mixing a legacy `Prefix` rule with a `Filter` rule: a
    /// configuration is written against one version of the API or the other, and the
    /// `Prefix` form is the one "supported for backward compatibility"
    /// (`PutBucketLifecycleConfiguration`'s own documentation).
    MixedPrefixAndFilterRules {
        /// The first rule naming a legacy `Prefix`.
        prefix_rule: usize,
        /// The first rule naming a `Filter`.
        filter_rule: usize,
    },
    /// A `Filter` naming more than one predicate directly: "A `Filter` must have exactly
    /// one of `Prefix`, `Tag`, `ObjectSizeGreaterThan`, `ObjectSizeLessThan`, or `And`
    /// specified" (`LifecycleRule$Filter`) — several conditions travel inside `And`.
    FilterNamesSeveralPredicates {
        /// Which rule carries that filter.
        rule: usize,
        /// How many predicates it named.
        named: usize,
    },
    /// An object-size bound below zero, which is no object size: the members are a
    /// "minimum object size to which the rule applies" and a maximum.
    NegativeObjectSize {
        /// Which rule carries it.
        rule: usize,
        /// The member that carried it.
        member: &'static str,
        /// The value that is not an object size.
        bytes: i64,
    },
    /// An object-size range no object can be in, because the minimum is not below the
    /// maximum.
    ObjectSizeRangeEmpty {
        /// Which rule carries it.
        rule: usize,
        /// The `ObjectSizeGreaterThan` it named.
        greater_than: i64,
        /// The `ObjectSizeLessThan` it named.
        less_than: i64,
    },
    /// A rule naming no action at all: a lifecycle rule carries "one or more lifecycle
    /// transition and expiration actions that you want Amazon S3 to perform on the objects
    /// identified by the filter" (`PutBucketLifecycleConfiguration`'s own documentation).
    NoAction {
        /// Which rule names none.
        rule: usize,
    },
    /// An `Expiration` naming more than one of `Days`, `Date` and
    /// `ExpiredObjectDeleteMarker`, which are alternatives: the delete-marker form "cannot
    /// be specified with Days or Date" (`LifecycleExpiration$ExpiredObjectDeleteMarker`).
    SeveralExpirationPredicates {
        /// Which rule carries that expiration.
        rule: usize,
        /// How many of the three it named.
        named: usize,
    },
    /// A day count that is not a non-zero positive integer, where the member's own
    /// documentation requires one ("The value must be a non-zero positive integer").
    NonPositiveDays {
        /// The member that carried it.
        member: &'static str,
        /// The action that member belongs to.
        action: &'static str,
    },
    /// A `Transition$Days` below zero: that one "can be `0` or any positive integer".
    NegativeDays {
        /// The member that carried it.
        member: &'static str,
        /// The action that member belongs to.
        action: &'static str,
    },
    /// A `Date` whose time of day is not midnight UTC. See [`is_utc_midnight`].
    DateNotMidnight {
        /// Which rule carries it.
        rule: usize,
        /// The member that carried it.
        member: &'static str,
    },
    /// One rule mixing `Date`-based and `Days`-based actions: a rule is written against
    /// one clock or the other, since a day count is measured from each object's own
    /// creation while a date is the same instant for every object.
    MixedDateAndDays {
        /// Which rule mixes them.
        rule: usize,
    },
    /// A transition action naming no storage class, so nothing says where the object goes.
    TransitionWithoutStorageClass {
        /// Which rule carries it.
        rule: usize,
        /// The action that carried it.
        member: &'static str,
    },
    /// An action naming no day count and no date, so nothing says when it is due.
    ActionWithoutWhen {
        /// Which rule carries it.
        rule: usize,
        /// The action that carried it.
        member: &'static str,
    },
    /// Two transitions of one rule naming the same storage class, so one of them could
    /// never move the object anywhere.
    DuplicateTransitionStorageClass {
        /// Which rule carries them.
        rule: usize,
        /// The class named twice.
        storage_class: String,
    },
    /// A transition to a WARMER class than the one an earlier transition of the same rule
    /// already moved the object to. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
    TransitionToWarmerClass {
        /// The action both transitions belong to and the member carrying their day counts
        /// ([`TRANSITION_ACTION`] or [`NONCURRENT_VERSION_TRANSITION_ACTION`]).
        action: (&'static str, &'static str),
        /// The scope of the rule carrying them, as S3 describes a filter.
        filter: String,
        /// The COLDER class, the one the earlier transition names.
        colder: String,
        /// The WARMER class, the one the later transition names.
        warmer: String,
    },
    /// A transition due before the class the object is in has been held for its minimum
    /// storage duration. See [`TRANSITION_STORAGE_CLASS_CHAIN`].
    TransitionBeforeMinimumDuration {
        /// The action both transitions belong to and the member carrying their day counts.
        action: (&'static str, &'static str),
        /// The scope of the rule carrying them, as S3 describes a filter.
        filter: String,
        /// The class the object is in, whose minimum storage duration is the gap.
        earlier: String,
        /// The class the later transition names.
        later: String,
        /// How many days apart they have to be.
        minimum: i32,
    },
    /// A rule asking to retain more noncurrent versions than
    /// [`MAX_NEWER_NONCURRENT_VERSIONS`].
    TooManyNewerNoncurrentVersions {
        /// Which rule carries it.
        rule: usize,
        /// The count it named.
        count: i32,
    },
    /// A `Filter`'s `And` carrying fewer than [`MIN_LIFECYCLE_AND_PREDICATES`] predicates:
    /// the operator "is used in a Lifecycle Rule Filter to apply a logical AND to two or
    /// more predicates" (`LifecycleRuleAndOperator`'s own documentation), so a conjunction
    /// of one thing is not one.
    AndWithTooFewPredicates {
        /// Which rule carries it.
        rule: usize,
        /// How many predicates it named.
        named: usize,
    },
    /// Two tag predicates of one filter naming the same key: a tag predicate matches one
    /// key to one value, so two on the same key can never both hold.
    DuplicateTagKey {
        /// Which rule carries them.
        rule: usize,
        /// The key named twice.
        key: String,
    },
    /// A tag predicate whose key is outside the published bounds: shorter than one
    /// character, or longer than [`MAX_TAG_KEY_CHARS`].
    TagKeyOutsideBounds {
        /// Which rule carries it.
        rule: usize,
        /// How long the key is.
        chars: usize,
    },
    /// A tag predicate whose value is longer than [`MAX_TAG_VALUE_CHARS`] characters.
    TagValueTooLong {
        /// Which rule carries it.
        rule: usize,
        /// How long the value is.
        chars: usize,
    },
    /// An action that acts on something an object TAG does not describe, in a rule whose
    /// filter names a tag: an incomplete multipart upload carries no object tags yet, and a
    /// delete marker carries none at all, so neither action can be scoped by them.
    TagFilteredAction {
        /// Which rule carries it.
        rule: usize,
        /// The action that cannot be tag-scoped.
        action: &'static str,
    },
    /// An `Expiration` due no later than the last `Transition` of the same rule, so the
    /// object would be expired before — or on the same day as — a transition that is meant
    /// to move it first.
    ExpirationNotAfterTransition {
        /// The scope of the rule carrying them, as S3 describes a filter.
        filter: String,
    },
}

impl LifecycleConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Three codes from S3's published "Error responses" list, all HTTP 400, chosen the way
    /// [`CorsConfigurationRejection::code`] chooses between the first two: a document that
    /// does not satisfy the schema the API publishes — the rule COUNT included, since the
    /// published schema is what bounds a configuration at [`MAX_LIFECYCLE_RULES`] rules —
    /// is [`MALFORMED_XML_CODE`]; a single member whose VALUE is outside what that member
    /// accepts, or whose value cannot hold beside the value of another member of the same
    /// action, is [`INVALID_ARGUMENT_CODE`]; and a configuration whose members are each
    /// acceptable but which together ask for something S3 cannot carry out is
    /// [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRules
            | Self::TooManyRules { .. }
            | Self::PrefixAndFilter { .. }
            | Self::NoPrefixOrFilter { .. }
            | Self::FilterNamesSeveralPredicates { .. }
            | Self::SeveralExpirationPredicates { .. }
            | Self::TransitionWithoutStorageClass { .. }
            | Self::ActionWithoutWhen { .. }
            | Self::AndWithTooFewPredicates { .. } => MALFORMED_XML_CODE,
            Self::TagKeyOutsideBounds { .. } | Self::TagValueTooLong { .. } => {
                INVALID_REQUEST_CODE
            }
            Self::IdTooLong { .. }
            | Self::DuplicateId { .. }
            | Self::NonPositiveDays { .. }
            | Self::NegativeDays { .. }
            | Self::DateNotMidnight { .. }
            | Self::NegativeObjectSize { .. }
            | Self::TransitionToWarmerClass { .. }
            | Self::TransitionBeforeMinimumDuration { .. }
            | Self::ExpirationNotAfterTransition { .. }
            | Self::TooManyNewerNoncurrentVersions { .. } => INVALID_ARGUMENT_CODE,
            Self::MixedPrefixAndFilterRules { .. }
            | Self::ObjectSizeRangeEmpty { .. }
            | Self::NoAction { .. }
            | Self::MixedDateAndDays { .. }
            | Self::DuplicateTagKey { .. }
            | Self::TagFilteredAction { .. }
            | Self::DuplicateTransitionStorageClass { .. } => INVALID_REQUEST_CODE,
        }
    }

    /// The `<Message>` this refusal goes out with.
    ///
    /// A refusal S3 answers [`MALFORMED_XML_CODE`] with carries that code's own FIXED
    /// published text ([`MALFORMED_XML_MESSAGE`]) rather than anything about this
    /// configuration: the code says the document did not validate against the published
    /// schema, and the schema is the same for every document. Every other refusal states
    /// the rule the caller has to satisfy, which is what [`Display`](std::fmt::Display)
    /// writes.
    pub fn message(&self) -> String {
        if self.code() == MALFORMED_XML_CODE {
            return MALFORMED_XML_MESSAGE.to_string();
        }
        self.to_string()
    }
}

impl std::fmt::Display for LifecycleConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRules => write!(
                f,
                "LifecycleConfiguration.Rules names no Rule; a lifecycle configuration \
                 carries at least one rule"
            ),
            Self::TooManyRules { count } => write!(
                f,
                "LifecycleConfiguration.Rules carries {count} rules, more than the \
                 {MAX_LIFECYCLE_RULES} rules one lifecycle configuration may hold"
            ),
            Self::IdTooLong { .. } => write!(
                f,
                "ID length should not exceed allowed limit of {MAX_LIFECYCLE_RULE_ID_CHARS}"
            ),
            Self::DuplicateId { .. } => write!(
                f,
                "Rule ID must be unique. Found same ID for more than one rule"
            ),
            Self::PrefixAndFilter { rule } => write!(
                f,
                "Rule {rule}: names both Prefix and Filter; a rule identifies its objects \
                 with one or the other"
            ),
            Self::NoPrefixOrFilter { rule } => write!(
                f,
                "Rule {rule}: names neither Prefix nor Filter; Filter is required when a \
                 rule carries no Prefix"
            ),
            Self::MixedPrefixAndFilterRules { .. } => write!(
                f,
                "Filter element can only be used in Lifecycle V2."
            ),
            Self::FilterNamesSeveralPredicates { rule, named } => write!(
                f,
                "Rule {rule}: Filter names {named} of Prefix, Tag, ObjectSizeGreaterThan, \
                 ObjectSizeLessThan and And; a filter names exactly one of them, and \
                 several conditions travel inside And"
            ),
            Self::NegativeObjectSize {
                rule,
                member,
                bytes,
            } => write!(
                f,
                "Rule {rule}: {member} is an object size in bytes and cannot be negative, \
                 got {bytes}"
            ),
            Self::ObjectSizeRangeEmpty {
                rule,
                greater_than,
                less_than,
            } => write!(
                f,
                "Rule {rule}: ObjectSizeGreaterThan {greater_than} is not below \
                 ObjectSizeLessThan {less_than}, so no object size matches the filter"
            ),
            Self::NoAction { .. } => write!(
                f,
                "At least one action needs to be specified in a rule"
            ),
            Self::SeveralExpirationPredicates { rule, named } => write!(
                f,
                "Rule {rule}: Expiration names {named} of Days, Date and \
                 ExpiredObjectDeleteMarker; an expiration names exactly one of them"
            ),
            Self::NonPositiveDays { member, action } => write!(
                f,
                "'{member}' for {action} action must be a positive integer"
            ),
            Self::NegativeDays { member, action } => write!(
                f,
                "'{member}' for {action} action must be 0 or a positive integer"
            ),
            Self::DateNotMidnight { .. } => write!(f, "'Date' must be at midnight GMT"),
            Self::MixedDateAndDays { rule } => write!(
                f,
                "Rule {rule}: mixes Date-based and Days-based actions; a rule measures \
                 every action from one of the two"
            ),
            Self::TransitionWithoutStorageClass { rule, member } => write!(
                f,
                "Rule {rule}: {member} names no StorageClass, so nothing says which \
                 storage class the object transitions to"
            ),
            Self::ActionWithoutWhen { rule, member } => write!(
                f,
                "Rule {rule}: {member} says when it is due with neither a day count nor a \
                 date"
            ),
            Self::DuplicateTransitionStorageClass {
                rule,
                storage_class,
            } => write!(
                f,
                "Rule {rule}: two transitions name storage class {storage_class:?}"
            ),
            Self::TransitionToWarmerClass {
                action: (action, days),
                filter,
                colder,
                warmer,
            } => write!(
                f,
                "'{days}' in the '{action}' action for StorageClass '{colder}' for filter \
                 '{filter}' must be greater than '{days}' in the '{action}' action for \
                 StorageClass '{warmer}' for filter '{filter}'"
            ),
            Self::TransitionBeforeMinimumDuration {
                action: (action, days),
                filter,
                earlier,
                later,
                minimum,
            } => write!(
                f,
                "'{days}' in the '{action}' action for StorageClass '{later}' for filter \
                 '{filter}' must be {minimum} days more than 'filter '{filter}'' in the \
                 '{action}' action for StorageClass '{earlier}'"
            ),
            Self::TooManyNewerNoncurrentVersions { rule, count } => write!(
                f,
                "Rule {rule}: NewerNoncurrentVersions retains {count} versions, more than \
                 the {MAX_NEWER_NONCURRENT_VERSIONS} noncurrent versions a rule may retain"
            ),
            // The code's own FIXED published text: an `And` of fewer than two predicates is
            // a document the published schema does not validate, and the schema is the same
            // for every document, so this variant's sentence is never read (see
            // [`LifecycleConfigurationRejection::message`]) and states the rule only.
            Self::AndWithTooFewPredicates { rule, named } => write!(
                f,
                "Rule {rule}: Filter And names {named} predicates, fewer than the \
                 {MIN_LIFECYCLE_AND_PREDICATES} a logical AND applies to"
            ),
            Self::DuplicateTagKey { .. } => {
                write!(f, "Duplicate Tag Keys are not allowed.")
            }
            Self::TagKeyOutsideBounds { .. } => write!(
                f,
                "A Tag's Key must be a length between 1 and {MAX_TAG_KEY_CHARS}."
            ),
            Self::TagValueTooLong { .. } => write!(
                f,
                "A Tag's Value must be a length between 0 and {MAX_TAG_VALUE_CHARS}."
            ),
            Self::TagFilteredAction { action, .. } => write!(
                f,
                "{action} cannot be specified with Tags."
            ),
            Self::ExpirationNotAfterTransition { filter } => write!(
                f,
                "'Days' in the Expiration action for filter '{filter}' must be greater than \
                 'Days' in the Transition action"
            ),
        }
    }
}

impl std::error::Error for LifecycleConfigurationRejection {}

/// Whether `rules` may be stored as a bucket's lifecycle configuration, reporting the first
/// published rule they break.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current
/// lifecycle configuration exactly as it was — the order every other bucket subresource put
/// keeps. The counts are judged before the per-rule values, so a configuration past the
/// published rule ceiling is refused for that rather than for whichever rule happens to
/// come first.
pub fn validate_lifecycle_configuration(
    rules: &[crate::ops::types::LifecycleRule],
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    if rules.is_empty() {
        return Err(Rejection::NoRules);
    }
    if rules.len() > MAX_LIFECYCLE_RULES {
        return Err(Rejection::TooManyRules { count: rules.len() });
    }

    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut prefix_rule: Option<usize> = None;
    let mut filter_rule: Option<usize> = None;

    for (index, rule) in rules.iter().enumerate() {
        // Counted from 1, the way the document orders the rules, so a message names the
        // rule a caller can find in what it sent.
        let position = index + 1;

        if let Some(id) = rule.id.as_deref() {
            let length = id.chars().count();
            if length > MAX_LIFECYCLE_RULE_ID_CHARS {
                return Err(Rejection::IdTooLong {
                    rule: position,
                    length,
                });
            }
            if !ids.insert(id) {
                return Err(Rejection::DuplicateId {
                    rule: position,
                    id: id.to_string(),
                });
            }
        }

        match (rule.prefix.is_some(), rule.filter.is_some()) {
            (true, true) => return Err(Rejection::PrefixAndFilter { rule: position }),
            (false, false) => return Err(Rejection::NoPrefixOrFilter { rule: position }),
            (true, false) => prefix_rule = prefix_rule.or(Some(position)),
            (false, true) => filter_rule = filter_rule.or(Some(position)),
        }
        if let (Some(prefix_rule), Some(filter_rule)) = (prefix_rule, filter_rule) {
            return Err(Rejection::MixedPrefixAndFilterRules {
                prefix_rule,
                filter_rule,
            });
        }

        if let Some(filter) = &rule.filter {
            validate_lifecycle_filter(position, filter)?;
        }
        validate_lifecycle_actions(position, rule)?;
    }

    Ok(())
}

/// Whether one rule's `Filter` names a set of objects, reporting the first published rule it
/// breaks. See [`validate_lifecycle_configuration`].
pub(super) fn validate_lifecycle_filter(
    position: usize,
    filter: &crate::ops::types::LifecycleRuleFilter,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    let named = [
        filter.prefix.is_some(),
        filter.tag.is_some(),
        filter.object_size_greater_than.is_some(),
        filter.object_size_less_than.is_some(),
        filter.and.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count();
    // An EMPTY filter is not a refusal: "If the Filter element is left empty, the Lifecycle
    // Rule applies to all objects in the bucket" (`LifecycleRuleFilter`'s own
    // documentation).
    if named > 1 {
        return Err(Rejection::FilterNamesSeveralPredicates {
            rule: position,
            named,
        });
    }

    // The conjunction "is used in a Lifecycle Rule Filter to apply a logical AND to two or
    // more predicates" (`LifecycleRuleAndOperator`): a prefix, each tag and each object-size
    // bound are one predicate each, so an `And` carrying fewer than
    // [`MIN_LIFECYCLE_AND_PREDICATES`] is a conjunction of one thing, which is not one.
    if let Some(and) = &filter.and {
        let predicates = usize::from(and.prefix.is_some())
            + and.tags.as_ref().map_or(0, Vec::len)
            + usize::from(and.object_size_greater_than.is_some())
            + usize::from(and.object_size_less_than.is_some());
        if predicates < MIN_LIFECYCLE_AND_PREDICATES {
            return Err(Rejection::AndWithTooFewPredicates {
                rule: position,
                named: predicates,
            });
        }
    }

    // Every tag predicate the filter names, wherever it names it: the filter's own `Tag` and
    // each tag inside its `And` are the same predicate — "a tag key and value" an object
    // must carry — so neither may be judged a way of its own.
    let tags: Vec<crate::ops::types::Tag> = lifecycle_filter_tags(filter).cloned().collect();
    for tag in &tags {
        // The published bounds every tag of this service is judged by
        // ([`MAX_TAG_KEY_CHARS`] and [`MAX_TAG_VALUE_CHARS`]), so a predicate cannot name a
        // tag no object could carry. The key's floor is one character — `Tag$Key` targets
        // `ObjectKey`, `length {min 1}` — and the value's is none.
        let key_chars = tag.key.chars().count();
        if key_chars == 0 || key_chars > MAX_TAG_KEY_CHARS {
            return Err(Rejection::TagKeyOutsideBounds {
                rule: position,
                chars: key_chars,
            });
        }
        let value_chars = tag.value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(Rejection::TagValueTooLong {
                rule: position,
                chars: value_chars,
            });
        }
    }
    // Read through the crate's one reading of the tag-predicate uniqueness rule, which the
    // S3 Intelligent-Tiering filter is judged by too.
    if let Some(repeated) = first_repeated_tag_key(&tags) {
        return Err(Rejection::DuplicateTagKey {
            rule: position,
            key: repeated.to_string(),
        });
    }

    validate_lifecycle_object_sizes(
        position,
        filter.object_size_greater_than,
        filter.object_size_less_than,
    )?;
    if let Some(and) = &filter.and {
        validate_lifecycle_object_sizes(
            position,
            and.object_size_greater_than,
            and.object_size_less_than,
        )?;
    }

    Ok(())
}

/// Whether a filter's object-size bounds describe a size some object can have.
pub(super) fn validate_lifecycle_object_sizes(
    position: usize,
    greater_than: Option<i64>,
    less_than: Option<i64>,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    for (member, bytes) in [
        ("ObjectSizeGreaterThan", greater_than),
        ("ObjectSizeLessThan", less_than),
    ] {
        if let Some(bytes) = bytes {
            if bytes < 0 {
                return Err(Rejection::NegativeObjectSize {
                    rule: position,
                    member,
                    bytes,
                });
            }
        }
    }
    if let (Some(greater_than), Some(less_than)) = (greater_than, less_than) {
        if greater_than >= less_than {
            return Err(Rejection::ObjectSizeRangeEmpty {
                rule: position,
                greater_than,
                less_than,
            });
        }
    }

    Ok(())
}

/// Whether one rule's ACTIONS are a set of things S3 can do to the objects the rule
/// identifies, reporting the first published rule they break.
///
/// A rule's actions are judged together rather than one at a time, because three of the
/// published rules are about the whole set: a rule carries at least one action, measures
/// every one of them from a date or from a day count but not from both, and moves an object
/// through the storage classes in one direction only. See
/// [`validate_lifecycle_configuration`].
pub(super) fn validate_lifecycle_actions(
    position: usize,
    rule: &crate::ops::types::LifecycleRule,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    // Two actions act on something an object TAG does not describe: an incomplete multipart
    // upload carries no object tags yet, and a delete marker carries none at all. A rule
    // whose filter names a tag therefore cannot carry either — the action could never be
    // scoped the way the filter asks.
    let tag_filtered = rule
        .filter
        .as_ref()
        .is_some_and(|filter| lifecycle_filter_tags(filter).next().is_some());
    if tag_filtered {
        if rule.abort_incomplete_multipart_upload.is_some() {
            return Err(Rejection::TagFilteredAction {
                rule: position,
                action: "AbortIncompleteMultipartUpload",
            });
        }
        if rule
            .expiration
            .as_ref()
            .and_then(|expiration| expiration.expired_object_delete_marker)
            == Some(true)
        {
            return Err(Rejection::TagFilteredAction {
                rule: position,
                action: "ExpiredObjectDeleteMarker",
            });
        }
    }

    let mut actions = 0usize;
    let mut days_based = false;
    let mut date_based = false;
    // The day the rule's expiration is due on, on whichever of the two scales the rule is
    // written against — the same scale the transitions below are collected on, since a rule
    // mixing the two is refused.
    let mut expiration_day: Option<i64> = None;

    if let Some(expiration) = &rule.expiration {
        let named = [
            expiration.days.is_some(),
            expiration.date.is_some(),
            expiration.expired_object_delete_marker.is_some(),
        ]
        .into_iter()
        .filter(|named| *named)
        .count();
        if named > 1 {
            return Err(Rejection::SeveralExpirationPredicates {
                rule: position,
                named,
            });
        }
        actions += named;

        if let Some(days) = expiration.days {
            if days <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "Days",
                    action: "Expiration",
                });
            }
            days_based = true;
            expiration_day = Some(i64::from(days));
        }
        if let Some(date) = &expiration.date {
            if !is_utc_midnight(date) {
                return Err(Rejection::DateNotMidnight {
                    rule: position,
                    member: "Expiration.Date",
                });
            }
            date_based = true;
            expiration_day = Some(date.secs() / SECONDS_PER_DAY);
        }
    }

    // Each transition as (the day it is due, the class it moves the object to). A
    // days-based rule counts from each object's creation and a date-based one from the
    // epoch; a rule mixing the two is refused below, so the numbers in here are always on
    // one of the two scales and the gaps between them are days either way.
    let mut transitions: Vec<(i64, &str)> = Vec::new();
    for transition in rule.transitions.iter().flatten() {
        actions += 1;
        let Some(storage_class) = transition.storage_class.as_deref() else {
            return Err(Rejection::TransitionWithoutStorageClass {
                rule: position,
                member: "Transition",
            });
        };
        match (transition.days, &transition.date) {
            (Some(_), Some(_)) => return Err(Rejection::MixedDateAndDays { rule: position }),
            (None, None) => {
                return Err(Rejection::ActionWithoutWhen {
                    rule: position,
                    member: "Transition",
                })
            }
            (Some(days), None) => {
                if days < 0 {
                    return Err(Rejection::NegativeDays {
                        member: "Days",
                        action: "Transition",
                    });
                }
                // No FLOOR is checked here. A transition's `Days` "can be 0 or any
                // positive integer" (`Transition$Days`), and the 30-day minimum storage
                // duration of the infrequent-access classes is what an object already IN
                // one of them is charged for, not a bound on when a rule may move an object
                // into it — so a transition to `STANDARD_IA` one day after creation is a
                // configuration S3 stores. The duration is still what separates one
                // transition from the NEXT ([`validate_transition_order`]).
                days_based = true;
                transitions.push((i64::from(days), storage_class));
            }
            (None, Some(date)) => {
                if !is_utc_midnight(date) {
                    return Err(Rejection::DateNotMidnight {
                        rule: position,
                        member: "Transition.Date",
                    });
                }
                date_based = true;
                transitions.push((date.secs() / SECONDS_PER_DAY, storage_class));
            }
        }
    }

    if let Some(expiration) = &rule.noncurrent_version_expiration {
        let mut named = 0usize;
        if let Some(days) = expiration.noncurrent_days {
            if days <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NoncurrentDays",
                    action: "NoncurrentVersionExpiration",
                });
            }
            named += 1;
        }
        if let Some(count) = expiration.newer_noncurrent_versions {
            if count <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NewerNoncurrentVersions",
                    action: "NoncurrentVersionExpiration",
                });
            }
            if count > MAX_NEWER_NONCURRENT_VERSIONS {
                return Err(Rejection::TooManyNewerNoncurrentVersions {
                    rule: position,
                    count,
                });
            }
            named += 1;
        }
        actions += named;
    }

    // The noncurrent transitions are measured in days a version has been noncurrent, which
    // is a scale of its own rather than the rule's date-or-days one, so they are ordered
    // against each other and nothing else.
    let mut noncurrent_transitions: Vec<(i64, &str)> = Vec::new();
    for transition in rule.noncurrent_version_transitions.iter().flatten() {
        actions += 1;
        let Some(storage_class) = transition.storage_class.as_deref() else {
            return Err(Rejection::TransitionWithoutStorageClass {
                rule: position,
                member: "NoncurrentVersionTransition",
            });
        };
        let Some(days) = transition.noncurrent_days else {
            return Err(Rejection::ActionWithoutWhen {
                rule: position,
                member: "NoncurrentVersionTransition",
            });
        };
        if days <= 0 {
            return Err(Rejection::NonPositiveDays {
                member: "NoncurrentDays",
                action: "NoncurrentVersionTransition",
            });
        }
        if let Some(count) = transition.newer_noncurrent_versions {
            if count <= 0 {
                return Err(Rejection::NonPositiveDays {
                    member: "NewerNoncurrentVersions",
                    action: "NoncurrentVersionTransition",
                });
            }
            if count > MAX_NEWER_NONCURRENT_VERSIONS {
                return Err(Rejection::TooManyNewerNoncurrentVersions {
                    rule: position,
                    count,
                });
            }
        }
        noncurrent_transitions.push((i64::from(days), storage_class));
    }

    if let Some(abort) = &rule.abort_incomplete_multipart_upload {
        let Some(days) = abort.days_after_initiation else {
            return Err(Rejection::ActionWithoutWhen {
                rule: position,
                member: "AbortIncompleteMultipartUpload",
            });
        };
        if days <= 0 {
            return Err(Rejection::NonPositiveDays {
                member: "DaysAfterInitiation",
                action: "AbortIncompleteMultipartUpload",
            });
        }
        actions += 1;
    }

    if actions == 0 {
        return Err(Rejection::NoAction { rule: position });
    }
    if days_based && date_based {
        return Err(Rejection::MixedDateAndDays { rule: position });
    }

    let filter = lifecycle_rule_scope(rule);
    validate_transition_order(position, TRANSITION_ACTION, &filter, transitions.clone())?;
    validate_transition_order(
        position,
        NONCURRENT_VERSION_TRANSITION_ACTION,
        &filter,
        noncurrent_transitions,
    )?;

    // A transition MOVES the object and an expiration REMOVES it, so an expiration due on or
    // before the last transition would take the object away before that transition could
    // ever run: the expiration has to fall strictly after every transition of the same rule.
    // Both are on the one scale the rule is written against, the mixed case having been
    // refused above.
    if let (Some(expiration), Some(transition)) = (
        expiration_day,
        transitions.iter().map(|(day, _)| *day).max(),
    ) {
        if expiration <= transition {
            return Err(Rejection::ExpirationNotAfterTransition {
                filter: filter.clone(),
            });
        }
    }

    Ok(())
}

/// The fewest predicates a lifecycle filter's `And` may carry: the operator "is used in a
/// Lifecycle Rule Filter to apply a logical AND to two or more predicates"
/// (`LifecycleRuleAndOperator`'s own documentation).
pub const MIN_LIFECYCLE_AND_PREDICATES: usize = 2;

/// Every TAG predicate `filter` names, wherever it names it: the filter's own `Tag` and each
/// tag inside its `And`.
///
/// One reading for both, because they are the same predicate — a tag key and value an object
/// must carry — so a filter cannot be judged one way at the top level and another inside a
/// conjunction.
pub(super) fn lifecycle_filter_tags(
    filter: &crate::ops::types::LifecycleRuleFilter,
) -> impl Iterator<Item = &crate::ops::types::Tag> {
    filter.tag.as_ref().into_iter().chain(
        filter
            .and
            .as_ref()
            .and_then(|and| and.tags.as_ref())
            .into_iter()
            .flatten(),
    )
}

/// How S3 describes the set of objects one lifecycle rule applies to, which is what its
/// transition refusals name the rule by.
///
/// Recorded behavior: a rule filtering on the prefix `tiered/` is reported as
/// `(prefix=tiered/)`, so the description is the rule's predicates inside one pair of
/// parentheses, the prefix first and named even when the rule carries none (a rule with an
/// empty filter "applies to all objects in the bucket", `LifecycleRuleFilter`). The prefix
/// is wherever the rule states it — the legacy `Prefix` member, the `Filter`'s own `Prefix`,
/// or the `Prefix` inside its `And` — and each tag the filter names follows it, since a
/// tag-filtered rule applies to a different set than the prefix alone describes.
pub(super) fn lifecycle_rule_scope(rule: &crate::ops::types::LifecycleRule) -> String {
    let filter = rule.filter.as_ref();
    let and = filter.and_then(|filter| filter.and.as_ref());
    let prefix = rule
        .prefix
        .as_deref()
        .or_else(|| filter.and_then(|filter| filter.prefix.as_deref()))
        .or_else(|| and.and_then(|and| and.prefix.as_deref()));
    // A rule that states no prefix at all describes no prefix predicate, so the description
    // names none; one that states the element and leaves it EMPTY states a prefix every key
    // matches, which the description names as the empty value it is.
    let mut described = match prefix {
        Some(prefix) => format!("(prefix={prefix}"),
        None => "(".to_string(),
    };
    // Every tag the rule's filter names, wherever it names it, through the one reading of
    // that ([`lifecycle_filter_tags`]).
    let tags = filter.into_iter().flat_map(lifecycle_filter_tags);
    for (index, tag) in tags.enumerate() {
        if prefix.is_some() || index > 0 {
            described.push_str(", ");
        }
        described.push_str(&format!("tag={}={}", tag.key, tag.value));
    }
    described.push(')');
    described
}

/// Whether `transitions` — each as (the day it is due, the class it moves the object to) —
/// move an object through the storage classes in the one direction S3 supports, holding it
/// in each class for that class's minimum storage duration first.
///
/// See [`TRANSITION_STORAGE_CLASS_CHAIN`] for both published rules.
pub(super) fn validate_transition_order(
    position: usize,
    action: (&'static str, &'static str),
    filter: &str,
    mut transitions: Vec<(i64, &str)>,
) -> Result<(), LifecycleConfigurationRejection> {
    use LifecycleConfigurationRejection as Rejection;

    // Due order, not document order: the rule is about the sequence the object actually
    // moves through, and a configuration may list its transitions in any order.
    transitions.sort_by_key(|(day, _)| *day);
    for pair in transitions.windows(2) {
        // Destructured rather than indexed, so nothing here can be out of bounds.
        let [(earlier_day, earlier_class), (later_day, later_class)] = pair else {
            continue;
        };
        if earlier_class == later_class {
            return Err(Rejection::DuplicateTransitionStorageClass {
                rule: position,
                storage_class: later_class.to_string(),
            });
        }
        // An unranked class is one `TransitionStorageClass` does not name, which the
        // boundary validator refuses before any of this runs; nothing is claimed about its
        // place in the chain here.
        let (Some(earlier_rank), Some(later_rank)) = (
            transition_storage_class_rank(earlier_class),
            transition_storage_class_rank(later_class),
        ) else {
            continue;
        };
        if later_rank < earlier_rank {
            return Err(Rejection::TransitionToWarmerClass {
                action,
                filter: filter.to_string(),
                colder: earlier_class.to_string(),
                warmer: later_class.to_string(),
            });
        }
        let minimum = minimum_storage_duration_days(earlier_class).unwrap_or_default();
        let gap = later_day.saturating_sub(*earlier_day);
        if gap < i64::from(minimum) {
            return Err(Rejection::TransitionBeforeMinimumDuration {
                action,
                filter: filter.to_string(),
                earlier: earlier_class.to_string(),
                later: later_class.to_string(),
                minimum,
            });
        }
    }

    Ok(())
}
