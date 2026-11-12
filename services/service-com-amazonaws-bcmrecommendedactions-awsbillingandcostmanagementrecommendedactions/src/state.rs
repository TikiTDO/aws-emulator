// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions service.
//!
//! The service holds exactly one kind of record: a *recommended action* that AWS
//! Billing and Cost Management computed for an account. Its model binds a single
//! operation, `ListRecommendedActions`, which only reports — there is no public
//! create, update or delete — so the store STARTS with the inventory this account
//! has ([`computed_recommendations`], the stand-in for the real service's own
//! recommendation generator) and is re-filled through the seeding seam below
//! ([`RecommendedActionsStore::seed`]).
//!
//! That starting value is installed when the emulator BUILDS this service
//! ([`install_service_state`], called from `EmulatedService::new`), not when a
//! request first reaches the store: the engine builds a service's state on first
//! access, so leaving it to the first access would make the inventory — and the
//! instant stamped on each of its records — a function of when the first caller
//! happened to read.
//!
//! Records are held in INSERTION ORDER: `ListRecommendedActions` is `@paginated`,
//! and a stable order is what makes its `nextToken` resume exactly where the
//! previous page stopped without duplicating or skipping a record.

use std::collections::BTreeMap;

/// Declare a Smithy `enum` shape as a Rust enum plus its wire spelling.
///
/// Every member list below is copied from this service's Smithy model; see the
/// doc comment on each enum for the `smithy_query` call that produced it. Keeping
/// them as Rust enums means a record in the store cannot hold a value the model
/// does not declare, and `from_wire` is the single membership test the request
/// members the model types as enums are validated against.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident => $wire:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every member the model declares, in model order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// This member's wire value.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }

            /// The member `wire` spells, or `None` when the enum has no such member.
            pub fn from_wire(wire: &str) -> Option<Self> {
                match wire { $($wire => Some(Self::$variant),)+ _ => None }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#ActionType` — the action a customer can
    /// take by adopting the recommendation.
    ///
    /// Members from `smithy_query(query="shape", shape="ActionType")`.
    ActionType {
        AddAlternateBillingContact => "ADD_ALTERNATE_BILLING_CONTACT",
        CreateAnomalyMonitor => "CREATE_ANOMALY_MONITOR",
        CreateBudget => "CREATE_BUDGET",
        EnableCostOptimizationHub => "ENABLE_COST_OPTIMIZATION_HUB",
        MigrateToGranularPermissions => "MIGRATE_TO_GRANULAR_PERMISSIONS",
        PaymentsDue => "PAYMENTS_DUE",
        PaymentsPastDue => "PAYMENTS_PAST_DUE",
        ReviewAnomalies => "REVIEW_ANOMALIES",
        ReviewBudgetAlerts => "REVIEW_BUDGET_ALERTS",
        ReviewBudgetsExceeded => "REVIEW_BUDGETS_EXCEEDED",
        ReviewExpiringRi => "REVIEW_EXPIRING_RI",
        ReviewExpiringSp => "REVIEW_EXPIRING_SP",
        ReviewFreetierUsageAlerts => "REVIEW_FREETIER_USAGE_ALERTS",
        ReviewFreetierCreditsRemaining => "REVIEW_FREETIER_CREDITS_REMAINING",
        ReviewFreetierDaysRemaining => "REVIEW_FREETIER_DAYS_REMAINING",
        ReviewSavingsOpportunityRecommendations => "REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS",
        UpdateExpiredPaymentMethod => "UPDATE_EXPIRED_PAYMENT_METHOD",
        UpdateInvalidPaymentMethod => "UPDATE_INVALID_PAYMENT_METHOD",
        UpdateTaxExemptionCertificate => "UPDATE_TAX_EXEMPTION_CERTIFICATE",
        UpdateTaxRegistrationNumber => "UPDATE_TAX_REGISTRATION_NUMBER",
    }
}

impl ActionType {
    /// The `(feature, severity)` pair the service publishes for this action type.
    ///
    /// The model types `RecommendedAction$type`, `$feature` and `$severity` as three
    /// independent enums and says nothing about how they pair up, but the pairing is
    /// not account data: the service publishes a fixed catalog of recommended action
    /// types, each belonging to ONE Billing and Cost Management feature and reported
    /// at ONE severity tier (the AWS Billing and Cost Management user guide's
    /// recommended-actions catalog). Expressing it once here is what keeps a record
    /// this service produces from naming a pair the service does not publish:
    /// `INFO` is an opportunity to set something up or an amount to look at (an
    /// invoice that is merely due included), `WARNING` is something already needing
    /// attention (expiring, exceeded, triggered), and `CRITICAL` is reserved for a
    /// payment that is past due or cannot be charged at all.
    pub const fn published_pair(self) -> (Feature, Severity) {
        match self {
            Self::AddAlternateBillingContact => (Feature::Account, Severity::Info),
            Self::CreateAnomalyMonitor => (Feature::CostAnomalyDetection, Severity::Info),
            Self::CreateBudget => (Feature::Budgets, Severity::Info),
            Self::EnableCostOptimizationHub => (Feature::CostOptimizationHub, Severity::Info),
            Self::MigrateToGranularPermissions => (Feature::Iam, Severity::Warning),
            Self::PaymentsDue => (Feature::Payments, Severity::Info),
            Self::PaymentsPastDue => (Feature::Payments, Severity::Critical),
            Self::ReviewAnomalies => (Feature::CostAnomalyDetection, Severity::Warning),
            Self::ReviewBudgetAlerts => (Feature::Budgets, Severity::Warning),
            Self::ReviewBudgetsExceeded => (Feature::Budgets, Severity::Warning),
            Self::ReviewExpiringRi => (Feature::Reservations, Severity::Warning),
            Self::ReviewExpiringSp => (Feature::SavingsPlans, Severity::Warning),
            Self::ReviewFreetierUsageAlerts => (Feature::FreeTier, Severity::Warning),
            Self::ReviewFreetierCreditsRemaining => (Feature::FreeTier, Severity::Info),
            Self::ReviewFreetierDaysRemaining => (Feature::FreeTier, Severity::Info),
            Self::ReviewSavingsOpportunityRecommendations => {
                (Feature::CostOptimizationHub, Severity::Info)
            }
            Self::UpdateExpiredPaymentMethod => (Feature::Payments, Severity::Critical),
            Self::UpdateInvalidPaymentMethod => (Feature::Payments, Severity::Critical),
            Self::UpdateTaxExemptionCertificate => (Feature::TaxSettings, Severity::Warning),
            Self::UpdateTaxRegistrationNumber => (Feature::TaxSettings, Severity::Warning),
        }
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#Severity` — how urgent the action is.
    ///
    /// Members from `smithy_query(query="shape", shape="Severity")`.
    Severity {
        Info => "INFO",
        Warning => "WARNING",
        Critical => "CRITICAL",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#Feature` — the Billing and Cost
    /// Management feature the action belongs to.
    ///
    /// Members from `smithy_query(query="shape", shape="Feature")`.
    Feature {
        Account => "ACCOUNT",
        Budgets => "BUDGETS",
        CostAnomalyDetection => "COST_ANOMALY_DETECTION",
        CostOptimizationHub => "COST_OPTIMIZATION_HUB",
        FreeTier => "FREE_TIER",
        Iam => "IAM",
        Payments => "PAYMENTS",
        Reservations => "RESERVATIONS",
        SavingsPlans => "SAVINGS_PLANS",
        TaxSettings => "TAX_SETTINGS",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#FilterName` — the record field an
    /// `ActionFilter` filters on.
    ///
    /// Members from `smithy_query(query="shape", shape="FilterName")`.
    FilterName {
        Feature => "FEATURE",
        Severity => "SEVERITY",
        Type => "TYPE",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#MatchOption` — whether an
    /// `ActionFilter` keeps or excludes the records it matches.
    ///
    /// Members from `smithy_query(query="shape", shape="MatchOption")`.
    MatchOption {
        Equals => "EQUALS",
        NotEquals => "NOT_EQUALS",
    }
}

impl FilterName {
    /// The order this service LISTS this enum's members in when a refusal states
    /// the value set a rejected value failed ([`member_strings`]).
    ///
    /// Carried per enum because the listing order is a property of the service's
    /// message and not of the model: it is neither the declaration order the model
    /// publishes ([`FilterName::ALL`]) nor any sorting of it — this enum and
    /// [`MatchOption`] are listed in orders that no single rule over the model
    /// produces. Every member [`FilterName::ALL`] declares appears here exactly
    /// once, which is what makes a listing a re-ordering of the published set and
    /// not a different set (`tests/all/validation_message_wording.rs` pins it).
    pub const VALUE_SET_ORDER: &'static [Self] = &[Self::Feature, Self::Type, Self::Severity];
}

impl MatchOption {
    /// The order this service lists this enum's members in when a refusal states
    /// its value set; see [`FilterName::VALUE_SET_ORDER`] for why each enum carries
    /// its own.
    pub const VALUE_SET_ORDER: &'static [Self] = &[Self::NotEquals, Self::Equals];
}

/// The wire members of an enum, as the value set a refusal states them — the one
/// mapping behind every value set this service names in a `ValidationException`.
///
/// Only the request members the model TYPES as enums are stated this way: a member
/// whose target is a plain string closes no value set, so what a caller sends in it
/// is compared against the stored records rather than checked against a list.
pub fn member_strings<E: std::fmt::Display>(all: &[E]) -> Vec<String> {
    all.iter().map(std::string::ToString::to_string).collect()
}

/// One recommended action held for an account: every member of the wire
/// `RecommendedAction` shape, persisted.
///
/// `context` is a `BTreeMap` so the map a response carries is rendered in a
/// stable order, and `last_updated_timestamp` is the ISO-8601 instant the action's
/// status last changed (the model types `RecommendedAction$lastUpdatedTimeStamp`
/// as a plain string, so it is stored and returned verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAction {
    /// `RecommendedAction$id`.
    pub id: String,
    /// `RecommendedAction$type`.
    pub action_type: ActionType,
    /// `RecommendedAction$accountId` — 12 digits (`^[0-9]{12}$`).
    pub account_id: String,
    /// `RecommendedAction$severity`.
    pub severity: Severity,
    /// `RecommendedAction$feature`.
    pub feature: Feature,
    /// `RecommendedAction$context`.
    pub context: BTreeMap<String, String>,
    /// `RecommendedAction$nextSteps`.
    pub next_steps: Vec<String>,
    /// `RecommendedAction$lastUpdatedTimeStamp`.
    pub last_updated_timestamp: String,
}

/// The instant a record was last recomputed, as `RecommendedAction$lastUpdatedTimeStamp`
/// carries it: the member is modelled as a plain `String`, and AWS renders these
/// instants ISO-8601 in UTC.
fn iso8601(at: std::time::SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One record of the inventory the service computed for this account, built from
/// the action it recommends.
///
/// The record's `feature` and `severity` are NOT arguments: they are the pair
/// [`ActionType::published_pair`] publishes for the type, so the inventory cannot
/// report a type in a feature or a severity tier the service does not publish for
/// it.
fn computed(action_type: ActionType, next_step: &str, computed_at: &str) -> StoredAction {
    let (feature, severity) = action_type.published_pair();
    StoredAction {
        id: boot_id(action_type),
        action_type,
        account_id: ::engine::emulator_account_id().to_string(),
        severity,
        feature,
        context: BTreeMap::from([(
            "feature".to_string(),
            feature.as_str().to_string(),
        )]),
        next_steps: vec![next_step.to_string()],
        last_updated_timestamp: computed_at.to_string(),
    }
}

/// The `RecommendedAction$id` the boot inventory gives the record it holds for an
/// action type.
///
/// The model types the member as a plain `String` and fixes no form, so this is the
/// emulator's own value; deriving it from the type keeps the ids unique, stable
/// across runs and readable.
fn boot_id(action_type: ActionType) -> String {
    format!(
        "ra-{}",
        action_type.as_str().to_ascii_lowercase().replace('_', "-")
    )
}

/// The next step the inventory's record for an action type reports
/// (`RecommendedAction$nextSteps`): what a customer does to adopt the
/// recommendation.
///
/// Each row is ONE recommendation this emulator's account is in the condition for,
/// with that condition stated above it. The service computes a recommendation from a
/// condition it detects, so a type whose condition this account is NOT in carries no
/// record: nothing is past due and the payment method on file is valid (no `PAYMENTS`
/// recommendation), no budget exists so none can be exceeded or have alerted, no
/// anomaly monitor exists so no anomaly has been detected, the account is not on Free
/// Tier, and no tax document on file is expiring. Rows are in model order, which is
/// the insertion order the store keeps.
const BOOT_INVENTORY: &[(ActionType, &str)] = &[
    // No alternate billing contact is on the account.
    (
        ActionType::AddAlternateBillingContact,
        "Add an alternate billing contact for this account.",
    ),
    // No cost anomaly monitor is configured.
    (
        ActionType::CreateAnomalyMonitor,
        "Create a cost anomaly monitor so unexpected spend is detected.",
    ),
    // No budget exists — the baseline recommendation of a fresh account.
    (
        ActionType::CreateBudget,
        "Create a budget to track and control your costs.",
    ),
    // Cost Optimization Hub is not enabled.
    (
        ActionType::EnableCostOptimizationHub,
        "Enable Cost Optimization Hub to see your consolidated savings opportunities.",
    ),
    // The account's billing policies still name the coarse billing permissions.
    (
        ActionType::MigrateToGranularPermissions,
        "Migrate your billing IAM policies to the fine-grained billing permissions.",
    ),
    // Reserved Instances the account holds expire soon.
    (
        ActionType::ReviewExpiringRi,
        "Review the Reserved Instances that expire soon.",
    ),
    // Savings Plans the account holds expire soon.
    (
        ActionType::ReviewExpiringSp,
        "Review the Savings Plans that expire soon.",
    ),
    // The account's usage carries savings opportunities to look at.
    (
        ActionType::ReviewSavingsOpportunityRecommendations,
        "Review the savings opportunities in Cost Optimization Hub.",
    ),
];

/// The recommended actions this emulator holds when it starts.
///
/// `ListRecommendedActions` only REPORTS, and the service models no create
/// operation because AWS itself computes the recommendations — so an inventory
/// filled ONLY through [`RecommendedActionsStore::seed`] reports nothing at all to
/// a caller that never seeded it, and an exclusion-style query (`NOT_EQUALS`) over
/// it can never return the complement it exists to return. The store therefore
/// starts with the set this account has, the way the real service's generator
/// produces one; `seed` and `clear` stay the seam that installs another.
///
/// The inventory carries one record per recommendation this account's condition
/// makes due ([`BOOT_INVENTORY`]), in model order, each carrying the
/// `(feature, severity)` pair the service publishes for its type
/// ([`ActionType::published_pair`]) and the account the emulator presents
/// ([`engine::emulator_account_id`]). It spans the baseline recommendations a fresh
/// account in good standing carries — a budget to create, an anomaly monitor to
/// create, Cost Optimization Hub to enable, an alternate billing contact to add,
/// billing permissions to migrate, commitments expiring and savings opportunities to
/// look at — so a filter over a value the account carries answers the way the API's
/// filtering contract requires: the `EQUALS` side and the `NOT_EQUALS` side of the
/// partition are both non-empty, instead of a type the account should have returning
/// nothing. The inventory is fixed rather than derived from traffic: this emulator
/// observes no billing usage to derive one from.
fn computed_recommendations() -> Vec<StoredAction> {
    // The instant the inventory was produced, which is the instant this store is
    // built — the emulator's own start, because `install_service_state` builds it on
    // the boot path. It is NOT a reader's clock: a record nobody changed keeps the
    // timestamp it was computed with however long the emulator runs before it is
    // first listed.
    let computed_at = iso8601(std::time::SystemTime::now());
    BOOT_INVENTORY
        .iter()
        .map(|&(action_type, next_step)| computed(action_type, next_step, &computed_at))
        .collect()
}

/// The recommended actions this emulator holds, in insertion order.
#[derive(Debug)]
pub struct RecommendedActionsStore {
    actions: Vec<StoredAction>,
}

impl Default for RecommendedActionsStore {
    /// The inventory the service computed for this account — see
    /// [`computed_recommendations`]. The store's INITIAL value, installed on the
    /// emulator's boot path by [`install_service_state`] rather than by whichever
    /// request reaches the store first, so `ListRecommendedActions` stays a pure
    /// read: two callers see the same records whichever of them called first, and
    /// neither one's clock decides what they report.
    fn default() -> Self {
        Self {
            actions: computed_recommendations(),
        }
    }
}

impl RecommendedActionsStore {
    /// Install recommended actions, keeping insertion order.
    ///
    /// THE SEEDING SEAM. The service models no create operation — AWS itself
    /// computes these records — so this is how the emulator's inventory is
    /// populated. An action whose `id` is already held is REPLACED in place, so
    /// re-seeding an updated record neither duplicates it nor moves it to the end
    /// of the page order.
    pub fn seed<I: IntoIterator<Item = StoredAction>>(&mut self, actions: I) {
        for action in actions {
            match self.actions.iter_mut().find(|held| held.id == action.id) {
                Some(held) => *held = action,
                None => self.actions.push(action),
            }
        }
    }

    /// Drop every held action — the seam a caller uses to model the service having
    /// recomputed its recommendations down to none.
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// The held actions, in insertion order.
    pub fn actions(&self) -> &[StoredAction] {
        &self.actions
    }

    /// How many actions are held.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether no action is held.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Per-service state for com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState {
    /// The recommended actions `ListRecommendedActions` reports.
    pub recommended_actions: RecommendedActionsStore,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>)
}

/// Build this service's store NOW, so its initial value is fixed when the emulator
/// is built rather than when a request first reads it.
///
/// The engine builds a service's state on first access, and this service binds one
/// operation, a report. Left to first access, the initial value — including the
/// instant [`computed_recommendations`] stamps on every record it produces — would
/// be decided by whichever request got there first: an emulator listed an hour
/// after it started would report its unchanged inventory as computed just now, and
/// two emulators holding the identical inventory would report different instants.
/// Called from `EmulatedService::new`, which the emulator runs once per service
/// while it boots, before any request is served.
///
/// Idempotent and lock-free for later callers: the engine keeps the state it built
/// first, so calling this again (or a reader that somehow got there first) observes
/// that one store. A `None` answer — an engine this crate's store does not live in —
/// leaves nothing to install, and the handler states it as a server error.
pub fn install_service_state<Eng: Send + Sync + 'static>(engine: &Eng) {
    // The handle itself is of no use here: resolving it is what builds the store,
    // and the handlers reach it through `service_state` per request. So the handle
    // is dropped deliberately, and a `None` needs no answer at this seam.
    drop(service_state(engine));
}
