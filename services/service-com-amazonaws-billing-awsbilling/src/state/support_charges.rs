// SPDX-License-Identifier: CC0-1.0

//! The Support-eligible spend each linked account of this emulator's
//! consolidated billing family accrued in a billing month — the records
//! `ListBusinessSupportAccountCharges` and `GetEnterpriseSupportChargeSummary`
//! price.
//!
//! A Support charge is NOT stored here: it is COMPUTED from the spend below by
//! the tiered price schedule of the account's Support plan (see
//! `crate::ops::list_business_support_account_charges`) or, for an account
//! inside the Enterprise Support profile, by the profile's pricing plan (see
//! the `enterprise_support` module), so the per-account breakdown, the month's
//! totals and the account count are readings of one store and cannot disagree.
//!
//! Empty on a fresh emulator: spend accrues from a billing pipeline, which this
//! emulator does not run, so only a future operation that bills usage — or this
//! service's test suite, through
//! [`ComAmazonawsBillingAwsbillingState::record_support_eligible_spend`] —
//! writes it. Nothing synthesises a record.

use super::{ComAmazonawsBillingAwsbillingState, CreditAllocation};

/// `AWSSupportBusiness`, the Business Support plan.
///
/// From the model's `BusinessSupportAccountCharge$supportPlanName` doc: "Valid
/// values: `AWSSupportBusiness` (Business Support plan), `AWSSupportDeveloper`
/// (Developer Support plan), `AWSSupportEssential` (Basic Support plan)."
pub const SUPPORT_PLAN_BUSINESS: &str = "AWSSupportBusiness";

/// `AWSSupportDeveloper`, the Developer Support plan (see
/// [`SUPPORT_PLAN_BUSINESS`] for the member doc that lists the values).
pub const SUPPORT_PLAN_DEVELOPER: &str = "AWSSupportDeveloper";

/// `AWSSupportEssential`, the Basic Support plan (see
/// [`SUPPORT_PLAN_BUSINESS`] for the member doc that lists the values).
pub const SUPPORT_PLAN_BASIC: &str = "AWSSupportEssential";

/// Every Support plan name the model documents, in the order its
/// `supportPlanName` doc lists them.
pub const SUPPORT_PLAN_NAMES: &[&str] = &[
    SUPPORT_PLAN_BUSINESS,
    SUPPORT_PLAN_DEVELOPER,
    SUPPORT_PLAN_BASIC,
];

/// The `itemType` a spend line carries.
///
/// From the model's `BusinessSupportServiceSpend$itemType` doc: "The type of
/// the line item. Valid values: `Usage`."
pub const SPEND_ITEM_TYPE_USAGE: &str = "Usage";

/// A discount applied through a distributor arrangement.
///
/// From the model's `BusinessSupportDiscount$discountType` doc: "Valid values:
/// `Distributor_Discount` (a discount applied through a distributor
/// arrangement), `SPP_Discount` (a discount applied through the Solution
/// Provider Program)."
pub const DISCOUNT_TYPE_DISTRIBUTOR: &str = "Distributor_Discount";

/// A discount applied through the Solution Provider Program (see
/// [`DISCOUNT_TYPE_DISTRIBUTOR`] for the member doc that lists the values).
pub const DISCOUNT_TYPE_SPP: &str = "SPP_Discount";

/// How many minor units (cents) make up one unit of a currency, the scale every
/// stored amount here is held on.
///
/// Money is kept in minor units rather than a binary float so that a
/// percentage of it — a Support tier rate, a discount — is an exact integer
/// computation, and so a stored amount round-trips to the modeled decimal
/// string unchanged.
pub const MINOR_UNITS_PER_UNIT: i64 = 100;

/// One contributing service's Support-eligible spend on one account's bill, as
/// the modeled `BusinessSupportServiceSpend` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportServiceSpend {
    /// The Amazon Web Services service the spend is for.
    pub contributing_service: String,
    /// The line item's type — [`SPEND_ITEM_TYPE_USAGE`] is the only value the
    /// model documents.
    pub item_type: String,
    pub description: Option<String>,
    /// The spend, in minor units of the owning record's
    /// [`SupportEligibleSpend::currency_code`].
    pub charge_amount_minor: i64,
}

/// The discount that applies to one account's Support charge, as the modeled
/// `BusinessSupportDiscount` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportDiscount {
    /// The discount rate in basis points (hundredths of one percent), so the
    /// 12% the model's `discountPercentage` doc gives as its example is 1200.
    pub rate_basis_points: i64,
    /// A `discountType` value, e.g. [`DISCOUNT_TYPE_SPP`].
    pub discount_type: String,
    /// The program the discount came through, when one is named.
    pub discount_source: Option<String>,
}

/// One linked account's Support subscription and Support-eligible spend for one
/// billing month: the record a Support charge is priced from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportEligibleSpend {
    /// The 12-digit linked account the spend is billed to.
    pub account_id: String,
    /// The billing month, in the modeled `BusinessSupportBillingMonth`
    /// `YYYY-MM` form — the first segment of [`SupportEligibleSpend::key`].
    pub billing_month: String,
    /// The Support plan the account was subscribed to that month, one of
    /// [`SUPPORT_PLAN_NAMES`]. It is what the charge is priced by.
    pub support_plan_name: String,
    /// The ISO-4217 currency the account's bill is in, reported beside every
    /// spend line.
    pub currency_code: String,
    /// The Support-eligible spend, broken down by contributing service. Their
    /// sum is the account's usage basis.
    pub by_service: Vec<SupportServiceSpend>,
    /// The Support-eligible Reserved Instance spend on the account's bill, in
    /// minor units of [`SupportEligibleSpend::currency_code`].
    ///
    /// Reserved Instance fees are Support-eligible spend for an Enterprise
    /// Support profile (the `GetEnterpriseSupportChargeSummary` output reports
    /// them beside usage and Savings Plan spend), and not for a Business
    /// Support charge, whose `totalSupportEligibleSpend` the model documents as
    /// spend "from usage of Amazon Web Services". So the bill holds them and
    /// each reader takes the part its own charge is computed from.
    pub reserved_instance_spend_minor: i64,
    /// The Support-eligible Savings Plan spend on the account's bill, in minor
    /// units of [`SupportEligibleSpend::currency_code`] — Support-eligible for
    /// the same reason [`SupportEligibleSpend::reserved_instance_spend_minor`]
    /// is.
    pub savings_plan_spend_minor: i64,
    /// `true` when the account's bill for the month is not yet finalized, so
    /// the charges computed from this record are estimates — the same reading
    /// of a bill [`CreditAllocation::is_estimated_bill`] carries.
    pub is_estimated_bill: bool,
    /// The discount that reduces this account's Support charge, when one
    /// applies; `None` is an account with no discount, which reports no
    /// `supportDiscount` member.
    pub discount: Option<SupportDiscount>,
}

impl SupportEligibleSpend {
    /// The most account-month spend records this emulator keeps resident.
    ///
    /// The store grows with every account and month billed, so it needs a
    /// ceiling for the same reason `billing_views` does. An emulator resource
    /// bound, not a published AWS quota:
    /// [`ComAmazonawsBillingAwsbillingState::record_support_eligible_spend`]
    /// refuses a write past it rather than growing without limit.
    pub const MAX_RESIDENT: usize = 4096;

    /// The key this record is stored under: billing month, then account —
    /// together the one bill it is about.
    ///
    /// The parts are joined with the ASCII unit separator (`0x1F`), which sorts
    /// below every character the modeled members can carry, so one month's
    /// records are a contiguous key range walked in ascending account order.
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.billing_month, self.account_id)
    }

    /// The inclusive lower and exclusive upper bound of one billing month's
    /// records, so a reader seeks straight to a month instead of scanning the
    /// store.
    ///
    /// The key layout starts with the same fixed-width `YYYY-MM` month and
    /// separator [`CreditAllocation::key`] does, so the bounds are its rule.
    pub fn month_key_bounds(month: &str) -> (String, String) {
        CreditAllocation::month_key_bounds(month)
    }

    /// The account's Support-eligible spend for the month: the sum of its
    /// per-service spend, in minor units.
    ///
    /// Saturating, so no set of stored lines can overflow the sum.
    pub fn usage_basis_minor(&self) -> i64 {
        self.by_service
            .iter()
            .fold(0_i64, |sum, spend| {
                sum.saturating_add(spend.charge_amount_minor)
            })
    }

    /// The account's whole Support-eligible spend for the month: its usage
    /// basis, its Reserved Instance spend and its Savings Plan spend.
    ///
    /// The `GetEnterpriseSupportChargeSummary` output documents its
    /// `totalSupportEligibleSpend` as including "eligible spend from usage of
    /// Amazon Web Services, Reserved Instances, and Savings Plans", which is
    /// exactly these three parts. Saturating, for the same reason
    /// [`Self::usage_basis_minor`] is.
    pub fn total_support_eligible_spend_minor(&self) -> i64 {
        self.usage_basis_minor()
            .saturating_add(self.reserved_instance_spend_minor)
            .saturating_add(self.savings_plan_spend_minor)
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// Persist one account-month's Support-eligible spend under its
    /// [`SupportEligibleSpend::key`], replacing the record for the same account
    /// and billing month rather than duplicating it.
    ///
    /// Answers `false` without writing when the store is already at
    /// [`SupportEligibleSpend::MAX_RESIDENT`], so the caller refuses rather
    /// than growing the store without a bound. The only writer today is this
    /// service's test suite: spend accrues from a billing run, which this
    /// emulator does not perform, so a future operation that bills usage is
    /// what will call this in production.
    pub fn record_support_eligible_spend(&mut self, spend: SupportEligibleSpend) -> bool {
        let key = spend.key();
        if !self.support_eligible_spend.contains_key(&key)
            && self.support_eligible_spend.len() >= SupportEligibleSpend::MAX_RESIDENT
        {
            return false;
        }
        self.support_eligible_spend.insert(key, spend);
        true
    }

    /// The Support-eligible spend records of one billing month that belong to
    /// the billing history reported for `account_id`, in ascending account
    /// order.
    ///
    /// The family traversal is
    /// [`ComAmazonawsBillingAwsbillingState::history_covers_billed_account`],
    /// the same rule the credit-allocation history reads by: a record about a
    /// member account's bill belongs to the payer account's answer, and a
    /// record about an account outside the consolidated billing family belongs
    /// to nobody's.
    pub fn support_eligible_spend_for_month<'a>(
        &'a self,
        account_id: &'a str,
        billing_month: &str,
    ) -> impl Iterator<Item = (&'a String, &'a SupportEligibleSpend)> {
        let (lower, upper) = SupportEligibleSpend::month_key_bounds(billing_month);
        self.support_eligible_spend
            .range(lower..upper)
            .filter(move |(_, spend)| {
                self.history_covers_billed_account(account_id, &spend.account_id)
            })
    }
}
