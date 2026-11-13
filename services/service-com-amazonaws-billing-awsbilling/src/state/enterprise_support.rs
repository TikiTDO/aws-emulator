// SPDX-License-Identifier: CC0-1.0

//! The Enterprise Support contract of this emulator's consolidated billing
//! family: the pricing plans the Support profile's charge is computed by, and
//! the method that distributes that charge across the accounts the profile
//! covers — what `GetEnterpriseSupportChargeSummary` reads.
//!
//! The Support-eligible spend the charge is computed FROM is not held here. It
//! is the per-account-month spend in
//! [`ComAmazonawsBillingAwsbillingState::support_eligible_spend`] (see the
//! `support_charges` module) of the accounts the profile covers — the family
//! accounts subscribed to [`SUPPORT_PLAN_ENTERPRISE`] that month — so the
//! profile's total charge, the percentage it works out to and one account's
//! share of it are readings of one store and cannot disagree.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{
    apply_rate, epoch_secs, prorate, ComAmazonawsBillingAwsbillingState, SupportEligibleSpend,
    MINOR_UNITS_PER_UNIT,
};

/// `Proportional`, a `supportAllocationMethod` value.
///
/// From the model's `supportAllocationMethod` doc: "Proportional means support
/// charges are distributed to each account in proportion to its eligible
/// Spend."
pub const SUPPORT_ALLOCATION_PROPORTIONAL: &str = "Proportional";

/// `Fixed_Percentage`, the other `supportAllocationMethod` value: "support
/// charges are distributed across accounts according to pre-configured
/// percentages from the contract" (same doc).
pub const SUPPORT_ALLOCATION_FIXED_PERCENTAGE: &str = "Fixed_Percentage";

/// `AWSSupportEnterprise`, the Enterprise Support plan.
///
/// The plan a `support_eligible_spend` record carries is an `AWSSupport`-coded
/// plan name (see [`super::SUPPORT_PLAN_BUSINESS`] for the member doc that
/// lists the ones `supportPlanName` reports); this is the Enterprise plan's
/// code, and a record carrying it is an account-month inside the Enterprise
/// Support profile.
pub const SUPPORT_PLAN_ENTERPRISE: &str = "AWSSupportEnterprise";

/// The `pricingPlanId` of the standard Enterprise Support pricing plan — the
/// plan a profile is billed under when no negotiated plan replaces it.
pub const ENTERPRISE_STANDARD_PLAN_ID: &str = "enterprise-support-standard";

/// The `name` of that plan, the Support plan it prices.
pub const ENTERPRISE_STANDARD_PLAN_NAME: &str = "Enterprise Support";

/// The `resourceType` the modeled `ResourceNotFoundException` reports for a
/// billing month's Enterprise Support charge.
pub const ENTERPRISE_SUPPORT_CHARGE_RESOURCE_TYPE: &str = "enterprisesupportcharge";

/// One tier of an Enterprise Support pricing plan, as the modeled
/// `PricingPlanTier` reports it. Amounts are in minor units
/// ([`MINOR_UNITS_PER_UNIT`]) and rates in basis points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportPricingTier {
    /// The spend at which the tier starts (`tierMinimum`).
    pub tier_minimum_minor: i64,
    /// The spend at which it stops (`tierMaximum`); `None` for the last tier,
    /// which prices every unit above the tier before it.
    pub tier_maximum_minor: Option<i64>,
    /// The charge the tier adds whatever the spend inside it is
    /// (`baseCharge`).
    pub base_charge_minor: i64,
    /// The rate the tier charges on the spend it prices, in basis points
    /// (`additionalPercentageOfAggregateCharges`).
    pub additional_rate_basis_points: i64,
    /// The adjustment added to the tier's own charge
    /// (`aggregateChargesAdjustment`).
    pub aggregate_charges_adjustment_minor: i64,
    /// Whether the tier prices only the slice of the spend inside its own
    /// bounds (`incremental`), rather than the whole aggregate spend.
    pub incremental: bool,
}

/// An Enterprise Support pricing plan: the period it is in effect for, the
/// tiers a profile's Support-eligible spend is priced by, the monthly minimum
/// that charge cannot fall below, and the plan discount that reduces it.
///
/// The members mirror the modeled `PricingPlan`, so a reader reports what is
/// persisted rather than recomputing the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportPricingPlan {
    pub pricing_plan_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The first instant the plan is in effect.
    pub start_date: SystemTime,
    /// The instant it stops being in effect, when it has one; `None` is a plan
    /// still in effect.
    pub end_date: Option<SystemTime>,
    /// The plan's discount, in basis points (`planDiscountPercent`).
    pub plan_discount_basis_points: i64,
    /// Whether that discount also reduces a charge the plan minimum raised
    /// (`discountAppliesToMinimumCharge`).
    pub discount_applies_to_minimum_charge: bool,
    /// The monthly minimum charge (`minimumCharge`): a floor on the charge the
    /// tiers price, so a subscribed profile with little spend is still charged
    /// it.
    pub minimum_charge_minor: i64,
    /// Whether the plan prices spend in tiers (`tiered`).
    pub tiered: bool,
    /// The tiers, lowest first.
    pub tiers: Vec<EnterpriseSupportPricingTier>,
}

/// What one billing month's Support-eligible spend is charged under a pricing
/// plan: the charge the plan's tiers and minimum give it, the plan discount
/// that reduces it, and the total left after that discount.
///
/// Its default is no charge at all — what a month the profile was not billed
/// under any plan for carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnterpriseSupportCharge {
    /// The charge before the plan discount, in minor units.
    pub gross_minor: i64,
    /// The plan discount, as the positive magnitude of the reduction.
    pub discount_minor: i64,
    /// `gross_minor` less `discount_minor`, never below zero: a discount
    /// reduces a charge, it never turns one into a payment.
    pub total_minor: i64,
}

impl EnterpriseSupportPricingPlan {
    /// The standard Enterprise Support pricing plan.
    ///
    /// Source: the Support plan prices AWS publishes at
    /// <https://aws.amazon.com/premiumsupport/pricing/>. Enterprise Support
    /// costs the greater of 15,000 a month or a tiered percentage of monthly
    /// Support-eligible spend — 10% of the first 150,000, 7% of 150,000 to
    /// 500,000, 5% of 500,000 to 1,000,000 and 3% above 1,000,000 — with no
    /// negotiated discount, so a profile billed under it is charged the
    /// published schedule and nothing else.
    ///
    /// In effect from the epoch and with no end date, so it prices every
    /// billing month a caller can ask about until a negotiated plan is stored
    /// beside it.
    pub fn standard() -> Self {
        Self {
            pricing_plan_id: ENTERPRISE_STANDARD_PLAN_ID.to_string(),
            name: ENTERPRISE_STANDARD_PLAN_NAME.to_string(),
            description: Some(
                "The greater of the monthly minimum and the tiered percentage of monthly \
                 Support-eligible spend."
                    .to_string(),
            ),
            start_date: SystemTime::UNIX_EPOCH,
            end_date: None,
            plan_discount_basis_points: 0,
            discount_applies_to_minimum_charge: false,
            minimum_charge_minor: 15_000 * MINOR_UNITS_PER_UNIT,
            tiered: true,
            tiers: vec![
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 0,
                    tier_maximum_minor: Some(150_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 1_000,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 150_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: Some(500_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 700,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 500_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: Some(1_000_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 500,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 1_000_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: None,
                    base_charge_minor: 0,
                    additional_rate_basis_points: 300,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
            ],
        }
    }

    /// Whether the plan is in effect at `at_secs` seconds past the epoch: its
    /// start date is at or before that instant, and its end date — when it has
    /// one — after it.
    pub fn covers_instant(&self, at_secs: f64) -> bool {
        epoch_secs(self.start_date) <= at_secs
            && self
                .end_date
                .is_none_or(|end| at_secs < epoch_secs(end))
    }

    /// What the plan's tiers price `total_spend_minor` at, before the plan
    /// minimum and the plan discount.
    ///
    /// An incremental tier prices only the slice of the spend that falls
    /// inside its own bounds; a non-incremental one prices the whole spend,
    /// and only when the spend falls inside them. Either way a tier charges
    /// its base charge, plus its rate on the spend it prices, plus its
    /// aggregate adjustment — and a tier the spend does not reach prices
    /// nothing.
    pub fn tiered_charge_minor(&self, total_spend_minor: i64) -> i64 {
        let mut charge_minor = 0_i64;
        for tier in &self.tiers {
            if total_spend_minor <= tier.tier_minimum_minor && tier.tier_minimum_minor > 0 {
                continue;
            }
            let priced_minor = if tier.incremental {
                let upper_minor = tier
                    .tier_maximum_minor
                    .map_or(total_spend_minor, |maximum| maximum.min(total_spend_minor));
                (upper_minor.saturating_sub(tier.tier_minimum_minor)).max(0)
            } else if tier
                .tier_maximum_minor
                .is_none_or(|maximum| total_spend_minor <= maximum)
            {
                total_spend_minor
            } else {
                continue;
            };
            charge_minor = charge_minor
                .saturating_add(tier.base_charge_minor)
                .saturating_add(apply_rate(priced_minor, tier.additional_rate_basis_points))
                .saturating_add(tier.aggregate_charges_adjustment_minor);
        }
        charge_minor
    }

    /// The charge for `total_spend_minor` under this plan: the greater of what
    /// its tiers price the spend at and its monthly minimum, less the plan
    /// discount — which reduces a charge the minimum raised only when
    /// [`Self::discount_applies_to_minimum_charge`] says it does.
    pub fn charge(&self, total_spend_minor: i64) -> EnterpriseSupportCharge {
        let tiered_minor = self.tiered_charge_minor(total_spend_minor);
        let gross_minor = tiered_minor.max(self.minimum_charge_minor);
        let raised_by_minimum = gross_minor > tiered_minor;
        let discountable_minor = if raised_by_minimum && !self.discount_applies_to_minimum_charge {
            0
        } else {
            gross_minor
        };
        let discount_minor = apply_rate(discountable_minor, self.plan_discount_basis_points);
        EnterpriseSupportCharge {
            gross_minor,
            discount_minor,
            total_minor: gross_minor.saturating_sub(discount_minor).max(0),
        }
    }
}

/// The Enterprise Support contract the Support profile is billed under: how its
/// total charge is distributed across the accounts it covers, and the pricing
/// plans that charge is computed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportContract {
    /// The `supportAllocationMethod`: [`SUPPORT_ALLOCATION_PROPORTIONAL`] or
    /// [`SUPPORT_ALLOCATION_FIXED_PERCENTAGE`].
    pub allocation_method: String,
    /// The pre-configured share of the total Support charge each account is
    /// charged, in basis points.
    ///
    /// Read only under [`SUPPORT_ALLOCATION_FIXED_PERCENTAGE`]; an account the
    /// contract names no percentage for is charged none of the total.
    pub charge_allocation_basis_points: BTreeMap<String, i64>,
    /// The pricing plans the contract has been billed under. A month is priced
    /// by the one in effect for it ([`Self::effective_pricing_plan`]).
    pub pricing_plans: Vec<EnterpriseSupportPricingPlan>,
}

impl Default for EnterpriseSupportContract {
    fn default() -> Self {
        Self {
            // "Proportional means support charges are distributed to each
            // account in proportion to its eligible Spend" — the method that
            // needs no pre-configured percentage, so it is the one a contract
            // nobody has negotiated percentages for is billed under.
            allocation_method: SUPPORT_ALLOCATION_PROPORTIONAL.to_string(),
            charge_allocation_basis_points: BTreeMap::new(),
            pricing_plans: vec![EnterpriseSupportPricingPlan::standard()],
        }
    }
}

impl EnterpriseSupportContract {
    /// The plan in effect at `at_secs` seconds past the epoch — the most
    /// recently effective of the plans whose period covers that instant, so a
    /// negotiated plan stored over the standard one prices the months it
    /// covers.
    ///
    /// `None` when no stored plan covers the instant: the profile was not
    /// billed under this contract then, so there is no charge to report for
    /// it.
    pub fn effective_pricing_plan(&self, at_secs: f64) -> Option<&EnterpriseSupportPricingPlan> {
        self.pricing_plans
            .iter()
            .filter(|plan| plan.covers_instant(at_secs))
            .max_by(|left, right| {
                epoch_secs(left.start_date).total_cmp(&epoch_secs(right.start_date))
            })
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// The share of `total_charge_minor` the Enterprise Support contract
    /// charges `account_id`, given that account's Support-eligible spend and
    /// the whole profile's.
    ///
    /// Under the fixed-percentage method that is the percentage the contract
    /// names for the account; under the proportional method it is the charge in
    /// the proportion the account's spend is of the profile's. A charge cannot
    /// be distributed in proportion to spend when the profile accrued none, so
    /// a profile with no spend charges its whole minimum to the payer account
    /// the profile is billed to.
    pub fn allocated_support_charge_minor(
        &self,
        account_id: &str,
        total_charge_minor: i64,
        account_spend_minor: i64,
        profile_spend_minor: i64,
    ) -> i64 {
        let contract = &self.enterprise_support_contract;
        if contract.allocation_method == SUPPORT_ALLOCATION_FIXED_PERCENTAGE {
            let rate_basis_points = contract
                .charge_allocation_basis_points
                .get(account_id)
                .copied()
                .unwrap_or(0);
            return apply_rate(total_charge_minor, rate_basis_points);
        }
        if profile_spend_minor == 0 {
            return if self.is_billing_family_management_account(account_id) {
                total_charge_minor
            } else {
                0
            };
        }
        prorate(total_charge_minor, account_spend_minor, profile_spend_minor)
    }

    /// The Support-eligible spend records of one billing month that belong to
    /// the Enterprise Support profile `payer_account_id` pays for: the records
    /// of the accounts of its consolidated billing family that were subscribed
    /// to Enterprise Support that month, in ascending account order.
    ///
    /// The family traversal is the one every billing history in this service
    /// reads by
    /// ([`ComAmazonawsBillingAwsbillingState::history_covers_billed_account`]),
    /// and the subscription is the record's own plan: an account on another
    /// Support plan is billed under that plan, not inside this profile.
    pub fn enterprise_support_spend_for_month<'a>(
        &'a self,
        payer_account_id: &'a str,
        billing_month: &str,
    ) -> impl Iterator<Item = &'a SupportEligibleSpend> {
        self.support_eligible_spend_for_month(payer_account_id, billing_month)
            .map(|(_, spend)| spend)
            .filter(|spend| spend.support_plan_name == SUPPORT_PLAN_ENTERPRISE)
    }
}
