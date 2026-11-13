// SPDX-License-Identifier: CC0-1.0

//! `GetEnterpriseSupportChargeSummary`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: for a billing
//! month in the past it sums the Support-eligible usage, Reserved Instance and
//! Savings Plan spend persisted in
//! `ComAmazonawsBillingAwsbillingState::support_eligible_spend` for the
//! accounts of the caller's consolidated billing family that were subscribed to
//! Enterprise Support, charges that total by the pricing plan of
//! `ComAmazonawsBillingAwsbillingState::enterprise_support_contract` in effect
//! for the month — the greater of the plan's monthly minimum and what its tiers
//! price the total at, less the plan discount — reports the percentage that
//! charge works out to on that spend, and allocates the caller's own share of
//! it by the contract's allocation method. It writes nothing.
//!
//! The emulator runs no billing pipeline, so nothing accrues spend and the
//! store is empty on a fresh emulator: like the Business Support suite, the
//! tables below put the records in state directly (the same shared engine the
//! router hands every handler) and then assert what the API reports for them.
//! The rows differ along everything the rule generalises over: the profile's
//! spend (each published tier bound and one past it), which of the three kinds
//! of spend it is made of, the accounts it is spread over and whether they are
//! in the profile at all, the billing month, whether a bill is final, the
//! pricing plan in effect and its minimum and discount, and the allocation
//! method.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    billing_period, caller_account_id, EnterpriseSupportPricingPlan,
    EnterpriseSupportPricingTier, SupportEligibleSpend, SupportServiceSpend,
    ENTERPRISE_STANDARD_PLAN_ID, SPEND_ITEM_TYPE_USAGE, SUPPORT_ALLOCATION_FIXED_PERCENTAGE,
    SUPPORT_PLAN_BUSINESS, SUPPORT_PLAN_ENTERPRISE,
};
use service_com_amazonaws_billing_awsbilling::{ComAmazonawsBillingAwsbillingState, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// A second and third 12-digit account, added to the consolidated billing
/// family so the payer account's Support profile covers their bills.
const MEMBER_ACCOUNT_ID: &str = "123456789012";
const OTHER_MEMBER_ACCOUNT_ID: &str = "210987654321";

/// A 12-digit account that is NOT in the consolidated billing family, so no
/// bill of its own belongs to the payer account's Support profile.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// A billing month far enough in the past that the clock cannot move it, and
/// the two months either side of it, so a month's answer can be shown to carry
/// only its own bills and its own billing period.
const PAST_MONTH: &str = "2024-03";
const EARLIER_MONTH: &str = "2024-02";
const DECEMBER_MONTH: &str = "2023-12";

/// One minor unit of currency (a cent), the scale the stored amounts use.
const CENTS: i64 = 100;

/// The first instant of [`PAST_MONTH`], as seconds past the epoch — the instant
/// a plan that starts with that month is stored with.
const PAST_MONTH_START_SECS: u64 = 1_709_251_200;

/// One row of the pricing table: the case, the profile's Support-eligible spend
/// in whole currency units, and the `totalSupportCharge` and
/// `supportChargePercentage` the response must report for it.
type TierCase = (&'static str, i64, &'static str, &'static str);

/// One router over ONE shared engine — what `core/router` builds in production,
/// so every call in a test observes the same state.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Ask for one billing month's Enterprise Support charge summary.
async fn summary(router: &MultiServiceRouter, billing_month: &str) -> (u16, Value) {
    call(
        router,
        "GetEnterpriseSupportChargeSummary",
        &json!({ "billingMonth": billing_month }),
    )
    .await
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A whole number of currency units, in the minor units the store holds.
fn units(whole: i64) -> i64 {
    whole * CENTS
}

/// A month as the count of whole calendar months before it, so a month can be
/// named relative to today's.
fn month_index(year: i32, month: i32) -> i32 {
    year * 12 + month - 1
}

/// The billing month `offset` whole months from the one the clock is in now.
fn month_from_now(offset: i32) -> String {
    let (year, month) = billing_period(SystemTime::now()).expect("the clock names a month");
    let index = month_index(year, month) + offset;
    format!("{:04}-{:02}", index.div_euclid(12), index.rem_euclid(12) + 1)
}

/// One account-month of Support-eligible spend to persist: the Support plan the
/// account was subscribed to, and the three kinds of spend on its bill, in
/// whole currency units.
struct Seed {
    account_id: &'static str,
    billing_month: &'static str,
    support_plan_name: &'static str,
    usage: i64,
    reserved_instance: i64,
    savings_plan: i64,
    is_estimated_bill: bool,
}

impl Seed {
    /// An account-month of usage spend inside the Enterprise Support profile —
    /// the shape most rows need.
    fn enterprise(account_id: &'static str, billing_month: &'static str, usage: i64) -> Self {
        Self {
            account_id,
            billing_month,
            support_plan_name: SUPPORT_PLAN_ENTERPRISE,
            usage,
            reserved_instance: 0,
            savings_plan: 0,
            is_estimated_bill: false,
        }
    }

    fn record(&self) -> SupportEligibleSpend {
        SupportEligibleSpend {
            account_id: self.account_id.to_string(),
            billing_month: self.billing_month.to_string(),
            support_plan_name: self.support_plan_name.to_string(),
            currency_code: "USD".to_string(),
            by_service: if self.usage == 0 {
                Vec::new()
            } else {
                vec![SupportServiceSpend {
                    contributing_service: "Amazon Elastic Compute Cloud".to_string(),
                    item_type: SPEND_ITEM_TYPE_USAGE.to_string(),
                    description: None,
                    charge_amount_minor: units(self.usage),
                }]
            },
            discount: None,
            reserved_instance_spend_minor: units(self.reserved_instance),
            savings_plan_spend_minor: units(self.savings_plan),
            is_estimated_bill: self.is_estimated_bill,
        }
    }
}

/// Persist every seeded record, and put every account it names (bar the one
/// deliberately left outside) in the consolidated billing family, so the payer
/// account's Support profile covers their bills.
fn persist(engine: &Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for seed in seeds {
        if seed.account_id != OUTSIDE_ACCOUNT_ID {
            state
                .consolidated_billing_family
                .insert(seed.account_id.to_string(), "Linked Account".to_string());
        }
        assert!(
            state.record_support_eligible_spend(seed.record()),
            "case {:?}: the seeded spend must fit in the store",
            (seed.billing_month, seed.account_id)
        );
    }
}

/// How many spend records are resident, so a read can be shown not to write.
fn resident(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    state.support_eligible_spend.len()
}

/// A negotiated pricing plan in effect from `start_secs` on: one tier charging
/// `rate_basis_points` of the whole spend, the monthly minimum `minimum`, and a
/// plan discount of `discount_basis_points` that reduces a charge the minimum
/// raised only when `discount_applies_to_minimum_charge`.
fn negotiated_plan(
    start_secs: u64,
    minimum: i64,
    rate_basis_points: i64,
    discount_basis_points: i64,
    discount_applies_to_minimum_charge: bool,
) -> EnterpriseSupportPricingPlan {
    EnterpriseSupportPricingPlan {
        pricing_plan_id: "negotiated-plan".to_string(),
        name: "Enterprise Support (negotiated)".to_string(),
        description: None,
        start_date: SystemTime::UNIX_EPOCH + Duration::from_secs(start_secs),
        end_date: None,
        plan_discount_basis_points: discount_basis_points,
        discount_applies_to_minimum_charge,
        minimum_charge_minor: units(minimum),
        tiered: true,
        tiers: vec![EnterpriseSupportPricingTier {
            tier_minimum_minor: 0,
            tier_maximum_minor: None,
            base_charge_minor: 0,
            additional_rate_basis_points: rate_basis_points,
            aggregate_charges_adjustment_minor: 0,
            incremental: true,
        }],
    }
}

/// The published Enterprise Support price schedule, priced at each of its own
/// tier bounds and at amounts between and past them.
///
/// Source: the Support plan prices AWS publishes at
/// <https://aws.amazon.com/premiumsupport/pricing/>: Enterprise Support is the
/// greater of 15,000 a month and 10% of the first 150,000 of monthly
/// Support-eligible spend, 7% of 150,000 to 500,000, 5% of 500,000 to
/// 1,000,000 and 3% above 1,000,000. The percentage beside each charge is what
/// that charge works out to on the spend it was computed from.
#[tokio::test]
async fn prices_the_profiles_spend_by_each_published_tier() {
    let cases: Vec<TierCase> = vec![
        ("subscribed with no spend", 0, "15000.00", "0.00"),
        ("spend below the minimum charge", 100_000, "15000.00", "0.15"),
        ("the first tier bound exactly", 150_000, "15000.00", "0.10"),
        ("into the second tier", 200_000, "18500.00", "0.0925"),
        ("the second tier bound exactly", 500_000, "39500.00", "0.0790"),
        ("the third tier bound exactly", 1_000_000, "64500.00", "0.0645"),
        ("past the last published bound", 1_500_000, "79500.00", "0.0530"),
    ];

    for (case, spend, total_support_charge, percentage) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed::enterprise(caller_account_id(), PAST_MONTH, spend)],
        );

        let (status, body) = summary(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["totalSupportEligibleSpend"].as_str(),
            Some(format!("{spend}.00").as_str()),
            "case {case:?}: expected the profile's spend reported, got {body}"
        );
        assert_eq!(
            body["totalSupportCharge"].as_str(),
            Some(total_support_charge),
            "case {case:?}: expected the published schedule's charge, got {body}"
        );
        assert_eq!(
            body["supportChargePercentage"].as_str(),
            Some(percentage),
            "case {case:?}: expected the charge's percentage of the spend, got {body}"
        );
        // The profile's only account is the payer, so the whole charge is
        // allocated to it — including the minimum charge of a profile that
        // accrued no spend at all to distribute in proportion to.
        assert_eq!(
            body["supportCharge"].as_str(),
            Some(total_support_charge),
            "case {case:?}: expected the payer's share to be the whole charge, got {body}"
        );
        assert_eq!(
            body["supportDiscount"].as_str(),
            Some("0.00"),
            "case {case:?}: expected no discount under the standard plan, got {body}"
        );
    }
}

#[tokio::test]
async fn sums_the_usage_reserved_instance_and_savings_plan_spend() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed {
                account_id: caller_account_id(),
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_ENTERPRISE,
                usage: 60_000,
                reserved_instance: 30_000,
                savings_plan: 10_000,
                is_estimated_bill: false,
            },
            Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 40_000),
            Seed {
                account_id: OTHER_MEMBER_ACCOUNT_ID,
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_ENTERPRISE,
                usage: 0,
                reserved_instance: 50_000,
                savings_plan: 10_000,
                is_estimated_bill: false,
            },
        ],
    );

    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"three kinds of spend\": expected HTTP 200, got {status} {body}"
    );
    // 100,000 of usage, 80,000 of Reserved Instance and 20,000 of Savings Plan
    // spend across the profile: 200,000 of Support-eligible spend, which the
    // published schedule charges 15,000 plus 7% of the 50,000 above the first
    // tier bound.
    for (member, expected) in [
        ("totalSupportEligibleUsageSpend", "100000.00"),
        ("totalSupportEligibleReservedInstanceSpend", "80000.00"),
        ("totalSupportEligibleSavingsPlanSpend", "20000.00"),
        ("totalSupportEligibleSpend", "200000.00"),
        ("totalSupportCharge", "18500.00"),
        ("supportChargePercentage", "0.0925"),
    ] {
        assert_eq!(
            body[member].as_str(),
            Some(expected),
            "case \"three kinds of spend\": expected {member} {expected}, got {body}"
        );
    }
    // The payer accrued 100,000 of the profile's 200,000, so proportional
    // allocation charges it half the total.
    assert_eq!(
        body["supportCharge"].as_str(),
        Some("9250.00"),
        "case \"three kinds of spend\": expected the payer's proportional share, got {body}"
    );

    // A payer with no spend of its own is charged none of a total its members'
    // spend is the whole of.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 200_000)],
    );
    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"payer with no spend\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["totalSupportCharge"].as_str(),
        Some("18500.00"),
        "case \"payer with no spend\": expected the profile's charge, got {body}"
    );
    assert_eq!(
        body["supportCharge"].as_str(),
        Some("0.00"),
        "case \"payer with no spend\": expected the payer charged none of it, got {body}"
    );
}

#[tokio::test]
async fn counts_only_the_profiles_accounts_and_the_months_own_bills() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            // In the profile: a family account subscribed to Enterprise Support.
            Seed::enterprise(caller_account_id(), PAST_MONTH, 150_000),
            // Outside it: a family account on another Support plan...
            Seed {
                account_id: MEMBER_ACCOUNT_ID,
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_BUSINESS,
                usage: 100_000,
                reserved_instance: 0,
                savings_plan: 0,
                is_estimated_bill: false,
            },
            // ...and an Enterprise Support account outside the billing family.
            Seed::enterprise(OUTSIDE_ACCOUNT_ID, PAST_MONTH, 100_000),
            // Another month's bill, which this month's answer must not carry.
            Seed::enterprise(caller_account_id(), EARLIER_MONTH, 500_000),
        ],
    );

    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"one profile account\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["totalSupportEligibleSpend"].as_str(),
        Some("150000.00"),
        "case \"one profile account\": expected only the profile's own spend, got {body}"
    );
    assert_eq!(
        body["totalSupportCharge"].as_str(),
        Some("15000.00"),
        "case \"one profile account\": expected the charge on that spend, got {body}"
    );

    // The other month is priced from its own bill alone.
    let (status, earlier) = summary(&router, EARLIER_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"another month\": expected HTTP 200, got {status} {earlier}"
    );
    assert_eq!(
        earlier["totalSupportEligibleSpend"].as_str(),
        Some("500000.00"),
        "case \"another month\": expected that month's spend, got {earlier}"
    );
    assert_eq!(
        earlier["totalSupportCharge"].as_str(),
        Some("39500.00"),
        "case \"another month\": expected the charge on that spend, got {earlier}"
    );

    // A month in which no account of the family was subscribed to Enterprise
    // Support carries no Support charge at all — not the plan's minimum.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed {
            account_id: caller_account_id(),
            billing_month: PAST_MONTH,
            support_plan_name: SUPPORT_PLAN_BUSINESS,
            usage: 100_000,
            reserved_instance: 0,
            savings_plan: 0,
            is_estimated_bill: false,
        }],
    );
    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"nobody subscribed\": expected HTTP 200, got {status} {body}"
    );
    for member in [
        "totalSupportEligibleSpend",
        "totalSupportCharge",
        "supportCharge",
    ] {
        assert_eq!(
            body[member].as_str(),
            Some("0.00"),
            "case \"nobody subscribed\": expected {member} 0.00, got {body}"
        );
    }
    assert_eq!(
        body["supportChargePercentage"].as_str(),
        Some("0.00"),
        "case \"nobody subscribed\": expected no percentage, got {body}"
    );
}

#[tokio::test]
async fn reports_the_billing_period_and_bill_date_of_the_month() {
    // (case, billing month, the month's first instant, the next month's first
    // instant) — the half-open UTC calendar period the month's charges accrue
    // in, including a February of a leap year and a December, whose period ends
    // in the following year.
    let cases: [(&str, &str, f64, f64); 3] = [
        ("March", PAST_MONTH, 1_709_251_200.0, 1_711_929_600.0),
        ("leap February", EARLIER_MONTH, 1_706_745_600.0, 1_709_251_200.0),
        ("December", DECEMBER_MONTH, 1_701_388_800.0, 1_704_067_200.0),
    ];

    let (router, engine) = emulator();
    for (_, month, _, _) in cases {
        persist(&engine, &[Seed::enterprise(caller_account_id(), month, 1)]);
    }

    for (case, month, start, end) in cases {
        let (status, body) = summary(&router, month).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["billingMonth"].as_str(),
            Some(month),
            "case {case:?}: expected the requested month echoed, got {body}"
        );
        assert_eq!(
            body["payerAccountId"].as_str(),
            Some(caller_account_id()),
            "case {case:?}: expected the payer account of the profile, got {body}"
        );
        assert_eq!(
            body["billingPeriodStartDate"].as_f64(),
            Some(start),
            "case {case:?}: expected the month's first instant, got {body}"
        );
        assert_eq!(
            body["billingPeriodEndDate"].as_f64(),
            Some(end),
            "case {case:?}: expected the next month's first instant, got {body}"
        );
        // The bill for a billing month is generated when its billing period
        // closes.
        assert_eq!(
            body["billDate"].as_f64(),
            Some(end),
            "case {case:?}: expected the bill dated to the period's close, got {body}"
        );
    }
}

#[tokio::test]
async fn estimates_a_month_while_any_bill_it_prices_is_not_final() {
    // (case, billing month, whether the member's bill is an estimate, the
    // isEstimated the month must report).
    let cases: [(&str, &str, bool, bool); 2] = [
        ("every bill final", PAST_MONTH, false, false),
        ("one bill still an estimate", EARLIER_MONTH, true, true),
    ];

    for (case, month, estimated_member, expected) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[
                Seed::enterprise(caller_account_id(), month, 100_000),
                Seed {
                    account_id: MEMBER_ACCOUNT_ID,
                    billing_month: month,
                    support_plan_name: SUPPORT_PLAN_ENTERPRISE,
                    usage: 100_000,
                    reserved_instance: 0,
                    savings_plan: 0,
                    is_estimated_bill: estimated_member,
                },
            ],
        );

        let (status, body) = summary(&router, month).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["isEstimated"].as_bool(),
            Some(expected),
            "case {case:?}: expected isEstimated {expected}, got {body}"
        );
    }
}

#[tokio::test]
async fn allocates_the_charge_by_the_contracts_method() {
    // The payer accrued 20,000 of the profile's 200,000 of spend, which the
    // published schedule charges 18,500: a tenth of it by the proportional
    // method, and the contract's own 25% under the fixed-percentage one.
    // (case, allocation method, the payer's share the response must report).
    let cases: [(&str, Option<&str>, &str); 2] = [
        ("proportional to the account's spend", None, "1850.00"),
        (
            "the contract's fixed percentage",
            Some(SUPPORT_ALLOCATION_FIXED_PERCENTAGE),
            "4625.00",
        ),
    ];

    for (case, allocation_method, expected) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[
                Seed::enterprise(caller_account_id(), PAST_MONTH, 20_000),
                Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 180_000),
            ],
        );
        if let Some(method) = allocation_method {
            let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
            let mut state = state.lock();
            let contract = &mut state.enterprise_support_contract;
            contract.allocation_method = method.to_string();
            contract
                .charge_allocation_basis_points
                .insert(caller_account_id().to_string(), 2_500);
            contract
                .charge_allocation_basis_points
                .insert(MEMBER_ACCOUNT_ID.to_string(), 7_500);
        }

        let (status, body) = summary(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["totalSupportCharge"].as_str(),
            Some("18500.00"),
            "case {case:?}: expected the profile's charge unchanged by the method, got {body}"
        );
        assert_eq!(
            body["supportCharge"].as_str(),
            Some(expected),
            "case {case:?}: expected the payer's share {expected}, got {body}"
        );
    }
}

#[tokio::test]
async fn prices_a_month_by_the_pricing_plan_in_effect_for_it() {
    // A negotiated plan in effect from PAST_MONTH on: 5% of the whole spend,
    // a 20,000 monthly minimum and a 10% plan discount.
    // (case, billing month, spend in whole units, whether the plan's discount
    //  reaches a charge its minimum raised, the plan id, the total charge, the
    //  discount reported).
    let cases: [(&str, &str, i64, bool, &str, &str, &str); 4] = [
        (
            "the negotiated plan's own month",
            PAST_MONTH,
            1_000_000,
            false,
            "negotiated-plan",
            // 5% of 1,000,000 is 50,000, less the plan's 10% discount.
            "45000.00",
            "-5000.00",
        ),
        (
            "a month before the negotiated plan",
            EARLIER_MONTH,
            1_000_000,
            false,
            ENTERPRISE_STANDARD_PLAN_ID,
            // The published schedule: 15,000 + 24,500 + 25,000.
            "64500.00",
            "0.00",
        ),
        (
            "a charge the plan minimum raised",
            PAST_MONTH,
            100_000,
            false,
            "negotiated-plan",
            "20000.00",
            "0.00",
        ),
        (
            "a minimum the plan discount reaches",
            PAST_MONTH,
            100_000,
            true,
            "negotiated-plan",
            "18000.00",
            "-2000.00",
        ),
    ];

    for (case, month, spend, discount_applies_to_minimum, plan_id, total, discount) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed::enterprise(caller_account_id(), month, spend)],
        );
        {
            let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
            let mut state = state.lock();
            state
                .enterprise_support_contract
                .pricing_plans
                .push(negotiated_plan(
                    PAST_MONTH_START_SECS,
                    20_000,
                    500,
                    1_000,
                    discount_applies_to_minimum,
                ));
        }

        let (status, body) = summary(&router, month).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["supportEffectivePricingPlan"]["pricingPlanId"].as_str(),
            Some(plan_id),
            "case {case:?}: expected the plan in effect for the month, got {body}"
        );
        assert_eq!(
            body["totalSupportCharge"].as_str(),
            Some(total),
            "case {case:?}: expected the charge that plan prices, got {body}"
        );
        assert_eq!(
            body["supportDiscount"].as_str(),
            Some(discount),
            "case {case:?}: expected the plan discount reported, got {body}"
        );
    }
}

#[tokio::test]
async fn reports_the_effective_plan_the_charge_was_computed_by() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 200_000)],
    );

    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"standard plan\": expected HTTP 200, got {status} {body}"
    );
    let plan = &body["supportEffectivePricingPlan"];
    assert_eq!(
        plan["pricingPlanId"].as_str(),
        Some(ENTERPRISE_STANDARD_PLAN_ID),
        "case \"standard plan\": expected the standard plan, got {plan}"
    );
    assert_eq!(
        plan["minimumCharge"].as_str(),
        Some("15000.00"),
        "case \"standard plan\": expected the published monthly minimum, got {plan}"
    );
    assert_eq!(
        plan["planDiscountPercent"].as_str(),
        Some("0.00"),
        "case \"standard plan\": expected no negotiated discount, got {plan}"
    );
    assert_eq!(
        plan["discountAppliesToMinimumCharge"].as_bool(),
        Some(false),
        "case \"standard plan\": expected the discount not to reach the minimum, got {plan}"
    );
    assert_eq!(
        plan["tiered"].as_str(),
        Some("true"),
        "case \"standard plan\": expected a tiered plan, got {plan}"
    );
    // The four published tiers, each reporting the bounds and the rate the
    // charge beside it was computed from.
    let tiers = plan["tiers"].as_array().cloned().unwrap_or_default();
    let expected: [(&str, Option<&str>, &str); 4] = [
        ("0.00", Some("150000.00"), "0.10"),
        ("150000.00", Some("500000.00"), "0.07"),
        ("500000.00", Some("1000000.00"), "0.05"),
        ("1000000.00", None, "0.03"),
    ];
    assert_eq!(
        tiers.len(),
        expected.len(),
        "case \"standard plan\": expected {} published tiers, got {tiers:?}",
        expected.len()
    );
    for (index, (minimum, maximum, rate)) in expected.into_iter().enumerate() {
        let tier = &tiers[index];
        assert_eq!(
            tier["tierMinimum"].as_str(),
            Some(minimum),
            "case \"tier {index}\": expected the tier to start at {minimum}, got {tier}"
        );
        assert_eq!(
            tier["tierMaximum"].as_str(),
            maximum,
            "case \"tier {index}\": expected the tier to stop at {maximum:?}, got {tier}"
        );
        assert_eq!(
            tier["additionalPercentageOfAggregateCharges"].as_str(),
            Some(rate),
            "case \"tier {index}\": expected the published rate {rate}, got {tier}"
        );
        assert_eq!(
            tier["incremental"].as_bool(),
            Some(true),
            "case \"tier {index}\": expected the tier to price its own slice, got {tier}"
        );
        assert_eq!(
            tier["baseCharge"].as_str(),
            Some("0.00"),
            "case \"tier {index}\": expected no base charge, got {tier}"
        );
        assert_eq!(
            tier["aggregateChargesAdjustment"].as_str(),
            Some("0.00"),
            "case \"tier {index}\": expected no adjustment, got {tier}"
        );
    }
}

#[tokio::test]
async fn refuses_a_month_that_is_not_in_the_past() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 200_000)],
    );

    // The modeled `EnterpriseSupportBillingMonth` is "a month in the past", so
    // the current month — whose billing period has not closed — and every later
    // one are refused rather than answered.
    for offset in [0_i32, 1, 2, 13] {
        let month = month_from_now(offset);
        let case = format!("{offset} months from now");
        let (status, body) = summary(&router, &month).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {month}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"].as_str(),
            Some("billingMonth"),
            "case {case:?}: expected the refusal to name billingMonth, got {body}"
        );
    }

    // ...and a month before this one is answered.
    let (status, body) = summary(&router, &month_from_now(-1)).await;
    assert_eq!(
        status, 200,
        "case \"last month\": expected HTTP 200, got {status} {body}"
    );
}

#[tokio::test]
async fn refuses_a_month_no_pricing_plan_covers() {
    // A contract with no plan in effect for the month has no charge to report
    // for it: a plan that ended covers the months before it ended and no
    // later one, and a contract with no plan at all covers none.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed::enterprise(caller_account_id(), PAST_MONTH, 200_000),
            Seed::enterprise(caller_account_id(), EARLIER_MONTH, 200_000),
        ],
    );
    {
        // One plan, which stopped being in effect when PAST_MONTH began.
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        let mut plan = negotiated_plan(0, 20_000, 500, 0, false);
        plan.end_date = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(PAST_MONTH_START_SECS));
        state.enterprise_support_contract.pricing_plans = vec![plan];
    }

    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 404,
        "case \"after the plan ended\": expected HTTP 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"after the plan ended\": expected a ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(PAST_MONTH),
        "case \"after the plan ended\": expected the refusal to name the month, got {body}"
    );

    // The month the plan was in effect for is still priced by it.
    let (status, earlier) = summary(&router, EARLIER_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"while the plan was in effect\": expected HTTP 200, got {status} {earlier}"
    );
    assert_eq!(
        earlier["totalSupportCharge"].as_str(),
        Some("20000.00"),
        "case \"while the plan was in effect\": expected that plan's charge, got {earlier}"
    );

    // A contract with no plan at all covers no month.
    {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        state.enterprise_support_contract.pricing_plans.clear();
    }
    let (status, body) = summary(&router, EARLIER_MONTH).await;
    assert_eq!(
        status, 404,
        "case \"no plan at all\": expected HTTP 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"no plan at all\": expected a ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn refuses_a_caller_that_is_not_the_payer_account_of_the_profile() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 200_000)],
    );
    {
        // The family is paid for by another account, so the caller pays for
        // nobody and the profile's data is not its to read.
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        state.billing_family_management_account_id = MEMBER_ACCOUNT_ID.to_string();
    }

    let (status, body) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        status, 403,
        "case \"not the payer account\": expected HTTP 403, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case \"not the payer account\": expected an AccessDeniedException, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("GetEnterpriseSupportChargeSummary")),
        "case \"not the payer account\": expected the refusal to name the action, got {body}"
    );
}

#[tokio::test]
async fn reports_without_writing() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed::enterprise(caller_account_id(), PAST_MONTH, 200_000),
            Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 100_000),
        ],
    );
    let before = resident(&engine);

    let (first_status, first) = summary(&router, PAST_MONTH).await;
    let (second_status, second) = summary(&router, PAST_MONTH).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"two reads\": expected both reads answered, got {first} {second}"
    );
    assert_eq!(
        first, second,
        "case \"two reads\": expected a second read to see what the first did, got {first} {second}"
    );
    assert_eq!(
        resident(&engine),
        before,
        "case \"two reads\": expected a read to persist nothing"
    );
}
