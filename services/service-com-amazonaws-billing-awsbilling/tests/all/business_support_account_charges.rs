// SPDX-License-Identifier: CC0-1.0

//! `ListBusinessSupportAccountCharges`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: it PRICES the
//! Support-eligible spend persisted in
//! `ComAmazonawsBillingAwsbillingState::support_eligible_spend` by the published
//! price schedule of the Support plan each account was subscribed to — the
//! greater of the plan's monthly minimum and the tiered percentage of that
//! account's spend, less the discount the record carries — and reports those
//! charges for one billing month: the accounts of the caller's consolidated
//! billing family, ascending by account, narrowed by the optional `accountId`
//! and paged by `maxResults`/`nextToken`, beside the month's own totals and
//! account count, which the model documents for ALL accounts in the month and
//! which therefore move with neither the filter nor the page. The current month
//! is estimated, an earlier one finalized, and a month that has not begun is
//! refused. It writes nothing.
//!
//! The emulator runs no billing pipeline, so nothing accrues spend and the
//! store is empty on a fresh emulator: like the credit-allocation suite, the
//! tables below put the records in state directly (the same shared engine the
//! router hands every handler) and then assert what the API reports for them.
//! The rows differ along everything the rule generalises over: the Support
//! plan, the usage basis (each published tier bound and one past it), the
//! number of contributing services, the currency, whether a discount applies
//! and of what kind, the account, the billing month, the page size and the
//! position of the page boundary.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    billing_period, caller_account_id, SupportDiscount, SupportEligibleSpend, SupportServiceSpend,
    DISCOUNT_TYPE_DISTRIBUTOR, DISCOUNT_TYPE_SPP, SPEND_ITEM_TYPE_USAGE, SUPPORT_PLAN_BASIC,
    SUPPORT_PLAN_BUSINESS, SUPPORT_PLAN_DEVELOPER,
};
use service_com_amazonaws_billing_awsbilling::{
    ComAmazonawsBillingAwsbillingState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// A second and third 12-digit account, added to the consolidated billing
/// family so the payer account's answer covers their bills.
const MEMBER_ACCOUNT_ID: &str = "123456789012";
const OTHER_MEMBER_ACCOUNT_ID: &str = "210987654321";

/// A 12-digit account that is NOT in the consolidated billing family, so no
/// bill of its own belongs to the payer account's answer.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// A billing month far enough in the past that the clock cannot move it: the
/// pricing, selection and paging rows use it, so only the estimated/finalized
/// rows depend on today's date.
const PAST_MONTH: &str = "2024-03";

/// A second past billing month, so a month's answer can be shown not to carry
/// another month's bills.
const EARLIER_MONTH: &str = "2024-02";

/// One minor unit of currency (a cent), the scale the stored amounts use.
const CENTS: i64 = 100;

/// One row of the discount table: the case, the discount rate in basis points,
/// its type and source, and the `discountPercentage`, `discountAmount` and
/// `totalCharge` the response must report for it.
type DiscountCase = (
    &'static str,
    i64,
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
    &'static str,
);

/// One row of the pricing table: the case, the Support plan, the usage basis in
/// minor units, the `totalCharge` the response must report, and how many tiers
/// it must break that charge into.
type TierCase = (&'static str, &'static str, i64, &'static str, usize);

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

/// One account-month of Support-eligible spend to persist: the plan, the
/// currency, the per-service spend in minor units and the discount, if any.
struct Seed {
    account_id: &'static str,
    billing_month: &'static str,
    support_plan_name: &'static str,
    currency_code: &'static str,
    /// (contributing service, description, spend in minor units)
    by_service: Vec<(&'static str, Option<&'static str>, i64)>,
    discount: Option<(i64, &'static str, Option<&'static str>)>,
}

impl Seed {
    /// A record of a single service's spend — the shape most rows need.
    fn one_service(
        account_id: &'static str,
        billing_month: &'static str,
        support_plan_name: &'static str,
        basis_minor: i64,
    ) -> Self {
        Self {
            account_id,
            billing_month,
            support_plan_name,
            currency_code: "USD",
            by_service: vec![("Amazon Elastic Compute Cloud", None, basis_minor)],
            discount: None,
        }
    }

    fn record(&self) -> SupportEligibleSpend {
        SupportEligibleSpend {
            account_id: self.account_id.to_string(),
            billing_month: self.billing_month.to_string(),
            support_plan_name: self.support_plan_name.to_string(),
            currency_code: self.currency_code.to_string(),
            by_service: self
                .by_service
                .iter()
                .map(|(service, description, minor)| SupportServiceSpend {
                    contributing_service: (*service).to_string(),
                    item_type: SPEND_ITEM_TYPE_USAGE.to_string(),
                    description: description.map(ToString::to_string),
                    charge_amount_minor: *minor,
                })
                .collect(),
            discount: self.discount.as_ref().map(|(rate, kind, source)| {
                SupportDiscount {
                    rate_basis_points: *rate,
                    discount_type: (*kind).to_string(),
                    discount_source: source.map(ToString::to_string),
                }
            }),
        }
    }
}

/// Persist every seeded record, and put every account it names (bar the one
/// deliberately left outside) in the consolidated billing family, so the payer
/// account's answer covers their bills.
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

/// The accounts a response reports, in the order it reports them.
fn reported_accounts(body: &Value) -> Vec<String> {
    body["accountCharges"]
        .as_array()
        .map(|charges| {
            charges
                .iter()
                .map(|charge| charge["accountId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The one charge entry a response reports for `account_id`.
fn charge_for<'a>(body: &'a Value, account_id: &str) -> &'a Value {
    body["accountCharges"]
        .as_array()
        .and_then(|charges| {
            charges
                .iter()
                .find(|charge| charge["accountId"].as_str() == Some(account_id))
        })
        .unwrap_or_else(|| panic!("case {account_id:?}: the response must report this account, got {body}"))
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

/// A whole number of currency units, in the minor units the store holds.
fn units(whole: i64) -> i64 {
    whole * CENTS
}

/// The Support charge each published tier bound — and each amount just past one
/// — prices to, for the plan the account is subscribed to.
///
/// Source: the Support plan prices AWS publishes at
/// <https://aws.amazon.com/premiumsupport/pricing/>: Business Support is the
/// greater of 100 a month or 10% of the first 10,000 of monthly
/// Support-eligible spend, 7% of 10,000 to 80,000, 5% of 80,000 to 250,000 and
/// 3% above 250,000; Developer Support the greater of 29 a month or 3% of that
/// spend; Basic Support nothing.
fn tier_cases() -> Vec<TierCase> {
    vec![
        // The monthly minimum is a floor, so a subscribed account with no
        // spend at all is still charged it, and prices no tier.
        ("business no spend", SUPPORT_PLAN_BUSINESS, 0, "100.00", 0),
        // 10% of 500 is 50, under the 100 minimum: the minimum is charged, and
        // the tier that priced the spend is still reported.
        ("business under the minimum", SUPPORT_PLAN_BUSINESS, units(500), "100.00", 1),
        // 10% of 1,000 is exactly the minimum.
        ("business at the minimum", SUPPORT_PLAN_BUSINESS, units(1_000), "100.00", 1),
        ("business over the minimum", SUPPORT_PLAN_BUSINESS, units(2_000), "200.00", 1),
        // The first published bound, and one cent past it: the cent is priced
        // by the second tier, whose rounded charge is nothing.
        ("business at 10K", SUPPORT_PLAN_BUSINESS, units(10_000), "1000.00", 1),
        ("business one cent past 10K", SUPPORT_PLAN_BUSINESS, units(10_000) + 1, "1000.00", 2),
        ("business past 10K", SUPPORT_PLAN_BUSINESS, units(10_100), "1007.00", 2),
        // 1,000 + 7% of 70,000.
        ("business at 80K", SUPPORT_PLAN_BUSINESS, units(80_000), "5900.00", 2),
        ("business past 80K", SUPPORT_PLAN_BUSINESS, units(80_100), "5905.00", 3),
        // 1,000 + 4,900 + 5% of 170,000.
        ("business at 250K", SUPPORT_PLAN_BUSINESS, units(250_000), "14400.00", 3),
        ("business past 250K", SUPPORT_PLAN_BUSINESS, units(250_100), "14403.00", 4),
        // Developer Support: one tier at 3%, with a 29 minimum.
        ("developer under the minimum", SUPPORT_PLAN_DEVELOPER, units(100), "29.00", 1),
        ("developer over the minimum", SUPPORT_PLAN_DEVELOPER, units(1_000), "30.00", 1),
        ("developer at a large spend", SUPPORT_PLAN_DEVELOPER, units(250_100), "7503.00", 1),
        // Basic Support is free, so it prices no tier and charges nothing.
        ("basic", SUPPORT_PLAN_BASIC, units(5_000), "0.00", 0),
    ]
}

#[tokio::test]
async fn prices_every_published_support_tier() {
    for (case, plan, basis_minor, expected_charge, expected_tiers) in tier_cases() {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed::one_service(
                caller_account_id(),
                PAST_MONTH,
                plan,
                basis_minor,
            )],
        );

        let (status, body) = call(
            &router,
            "ListBusinessSupportAccountCharges",
            &json!({ "billingMonth": PAST_MONTH }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} {body}");

        let charge = charge_for(&body, caller_account_id());
        assert_eq!(
            charge["totalCharge"].as_str(),
            Some(expected_charge),
            "case {case:?}: expected totalCharge {expected_charge}, got {charge}"
        );
        assert_eq!(
            charge["supportPlanName"].as_str(),
            Some(plan),
            "case {case:?}: expected supportPlanName {plan}, got {charge}"
        );
        let tiers = charge["tierCharges"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            tiers.len(),
            expected_tiers,
            "case {case:?}: expected {expected_tiers} tier charges, got {tiers:?}"
        );
        // The tiers partition the account's usage basis: their slices sum to
        // it, and nothing is priced twice.
        let sliced: i64 = tiers
            .iter()
            .map(|tier| {
                minor_units(tier["usageSlice"].as_str().unwrap_or_default(), case)
            })
            .sum();
        // A plan with a price schedule slices the WHOLE basis across its
        // tiers; a plan with none (Basic Support is free) prices no slice of
        // it.
        let expected_sliced = if expected_tiers == 0 { 0 } else { basis_minor };
        assert_eq!(
            sliced, expected_sliced,
            "case {case:?}: expected the tier slices to sum to {expected_sliced}, got {sliced}"
        );
        // The month's totals are this one account's charge and basis.
        assert_eq!(
            body["totalSupportCharge"].as_str(),
            Some(expected_charge),
            "case {case:?}: expected totalSupportCharge {expected_charge}, got {body}"
        );
        assert_eq!(
            charge["totalUsageBasis"].as_str(),
            body["totalSupportEligibleSpend"].as_str(),
            "case {case:?}: expected the month's eligible spend to be this account's, got {body}"
        );
        assert_eq!(
            body["accountCount"].as_i64(),
            Some(1),
            "case {case:?}: expected accountCount 1, got {body}"
        );
        assert_eq!(
            body["isEstimated"].as_bool(),
            Some(false),
            "case {case:?}: expected a past month to be finalized, got {body}"
        );
        assert_eq!(
            resident(&engine),
            1,
            "case {case:?}: a read must not write: expected 1 resident record, got {}",
            resident(&engine)
        );
    }
}

/// A modeled decimal amount back in minor units, so a reported amount can be
/// summed exactly.
fn minor_units(amount: &str, case: &str) -> i64 {
    let (whole, fraction) = amount
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: expected a decimal amount, got {amount:?}"));
    let negative = whole.starts_with('-');
    let magnitude = whole.trim_start_matches('-').parse::<i64>().unwrap_or_else(|error| {
        panic!("case {case:?}: expected a decimal amount, got {amount:?} ({error})")
    }) * CENTS
        + fraction.parse::<i64>().unwrap_or_else(|error| {
            panic!("case {case:?}: expected two fraction digits, got {amount:?} ({error})")
        });
    if negative {
        -magnitude
    } else {
        magnitude
    }
}

#[tokio::test]
async fn reports_the_spend_breakdown_each_charge_was_priced_from() {
    // Two contributing services, one with a description and one without, in a
    // currency other than the dollar: the usage basis is their sum, and every
    // line is reported as stored.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed {
            account_id: caller_account_id(),
            billing_month: PAST_MONTH,
            support_plan_name: SUPPORT_PLAN_BUSINESS,
            currency_code: "EUR",
            by_service: vec![
                ("Amazon Simple Storage Service", Some("S3 storage"), units(4_000)),
                ("AWS Lambda", None, units(2_500) + 37),
            ],
            discount: None,
        }],
    );

    let (status, body) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH }),
    )
    .await;
    assert_eq!(status, 200, "case \"two services\": expected HTTP 200, got {status} {body}");

    let charge = charge_for(&body, caller_account_id());
    assert_eq!(
        charge["totalUsageBasis"].as_str(),
        Some("6500.37"),
        "case \"two services\": expected the usage basis to be the sum of the lines, got {charge}"
    );
    // 10% of 6,500.37 — under no minimum once the spend is this large.
    assert_eq!(
        charge["totalCharge"].as_str(),
        Some("650.04"),
        "case \"two services\": expected 10% of the usage basis, got {charge}"
    );
    let lines = charge["supportEligibleSpendByService"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        lines.len(),
        2,
        "case \"two services\": expected both spend lines, got {lines:?}"
    );
    assert_eq!(
        lines[0]["contributingService"].as_str(),
        Some("Amazon Simple Storage Service"),
        "case \"two services\": expected the first stored line first, got {lines:?}"
    );
    assert_eq!(
        lines[0]["description"].as_str(),
        Some("S3 storage"),
        "case \"two services\": expected the stored description, got {lines:?}"
    );
    assert_eq!(
        lines[1].get("description"),
        None,
        "case \"two services\": expected no description on a line stored without one, got {lines:?}"
    );
    assert_eq!(
        lines[1]["chargeAmount"].as_str(),
        Some("2500.37"),
        "case \"two services\": expected the stored amount, got {lines:?}"
    );
    for line in &lines {
        assert_eq!(
            line["currency"].as_str(),
            Some("EUR"),
            "case \"two services\": expected the bill's currency, got {line}"
        );
        assert_eq!(
            line["itemType"].as_str(),
            Some(SPEND_ITEM_TYPE_USAGE),
            "case \"two services\": expected itemType {SPEND_ITEM_TYPE_USAGE}, got {line}"
        );
    }
    // Each tier reports the UTC calendar month it priced, as a half-open
    // period: 2024-03-01 through 2024-04-01.
    let tier = &charge["tierCharges"][0];
    assert_eq!(
        tier["chargePeriodStartDate"].as_f64(),
        Some(1_709_251_200.0),
        "case \"two services\": expected the month's first instant, got {tier}"
    );
    assert_eq!(
        tier["chargePeriodEndDate"].as_f64(),
        Some(1_711_929_600.0),
        "case \"two services\": expected the next month's first instant, got {tier}"
    );
    assert_eq!(
        tier["tierRate"].as_str(),
        Some("0.10"),
        "case \"two services\": expected the first tier's rate as a decimal, got {tier}"
    );
}

#[tokio::test]
async fn subtracts_the_discount_a_bill_carries() {
    // (case, discount rate in basis points, type, source, expected percentage,
    //  expected discount amount, expected total charge). The gross charge on
    //  20,000 of spend is 10% of the first 10,000 plus 7% of the next 10,000,
    //  i.e. 1,700.
    let cases: Vec<DiscountCase> = vec![
        (
            "twelve percent through the solution provider program",
            1_200,
            DISCOUNT_TYPE_SPP,
            Some("Solution Provider Program"),
            "0.12",
            "-204.00",
            "1496.00",
        ),
        (
            "five percent through a distributor, no source",
            500,
            DISCOUNT_TYPE_DISTRIBUTOR,
            None,
            "0.05",
            "-85.00",
            "1615.00",
        ),
        (
            "a fraction of a percent",
            25,
            DISCOUNT_TYPE_SPP,
            Some("Solution Provider Program"),
            "0.0025",
            "-4.25",
            "1695.75",
        ),
    ];

    for (case, rate, kind, source, percentage, expected_discount, expected_charge) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed {
                account_id: caller_account_id(),
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_BUSINESS,
                currency_code: "USD",
                by_service: vec![("Amazon Elastic Compute Cloud", None, units(20_000))],
                discount: Some((rate, kind, source)),
            }],
        );

        let (status, body) = call(
            &router,
            "ListBusinessSupportAccountCharges",
            &json!({ "billingMonth": PAST_MONTH }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} {body}");

        let charge = charge_for(&body, caller_account_id());
        let discount = &charge["supportDiscount"];
        assert_eq!(
            discount["discountAmount"].as_str(),
            Some(expected_discount),
            "case {case:?}: expected discountAmount {expected_discount}, got {discount}"
        );
        assert_eq!(
            discount["discountPercentage"].as_str(),
            Some(percentage),
            "case {case:?}: expected discountPercentage {percentage}, got {discount}"
        );
        assert_eq!(
            discount["discountType"].as_str(),
            Some(kind),
            "case {case:?}: expected discountType {kind}, got {discount}"
        );
        assert_eq!(
            discount.get("discountSource").and_then(Value::as_str),
            source,
            "case {case:?}: expected discountSource {source:?}, got {discount}"
        );
        assert_eq!(
            charge["totalCharge"].as_str(),
            Some(expected_charge),
            "case {case:?}: expected totalCharge {expected_charge}, got {charge}"
        );
        // The month's total is the discounted charge, not the gross one.
        assert_eq!(
            body["totalSupportCharge"].as_str(),
            Some(expected_charge),
            "case {case:?}: expected totalSupportCharge {expected_charge}, got {body}"
        );
    }

    // A bill with no discount reports no `supportDiscount` member at all.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::one_service(
            caller_account_id(),
            PAST_MONTH,
            SUPPORT_PLAN_BUSINESS,
            units(20_000),
        )],
    );
    let (_, body) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH }),
    )
    .await;
    let charge = charge_for(&body, caller_account_id());
    assert_eq!(
        charge.get("supportDiscount"),
        None,
        "case \"no discount\": expected no supportDiscount member, got {charge}"
    );
}

/// The seeds the month, filter and paging tests share: three family accounts
/// billed in one month, one of them also billed in an earlier month, and one
/// account outside the family.
fn family_seeds() -> Vec<Seed> {
    vec![
        Seed::one_service(
            caller_account_id(),
            PAST_MONTH,
            SUPPORT_PLAN_BUSINESS,
            units(20_000),
        ),
        Seed::one_service(
            MEMBER_ACCOUNT_ID,
            PAST_MONTH,
            SUPPORT_PLAN_DEVELOPER,
            units(1_000),
        ),
        Seed::one_service(
            OTHER_MEMBER_ACCOUNT_ID,
            PAST_MONTH,
            SUPPORT_PLAN_BASIC,
            units(500),
        ),
        Seed::one_service(
            MEMBER_ACCOUNT_ID,
            EARLIER_MONTH,
            SUPPORT_PLAN_BUSINESS,
            units(70_000),
        ),
        Seed::one_service(
            OUTSIDE_ACCOUNT_ID,
            PAST_MONTH,
            SUPPORT_PLAN_BUSINESS,
            units(123_456),
        ),
    ]
}

#[tokio::test]
async fn reports_one_month_of_the_billing_family() {
    let (router, engine) = emulator();
    persist(&engine, &family_seeds());

    // The month asked for: the three family accounts, ascending by account id,
    // and neither the earlier month's bill nor the outside account's.
    let (status, body) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH }),
    )
    .await;
    assert_eq!(status, 200, "case \"whole month\": expected HTTP 200, got {status} {body}");
    assert_eq!(
        reported_accounts(&body),
        vec![
            caller_account_id().to_string(),
            MEMBER_ACCOUNT_ID.to_string(),
            OTHER_MEMBER_ACCOUNT_ID.to_string(),
        ],
        "case \"whole month\": expected the family's accounts ascending, got {body}"
    );
    assert_eq!(
        body["billingMonth"].as_str(),
        Some(PAST_MONTH),
        "case \"whole month\": expected the requested month echoed, got {body}"
    );
    assert_eq!(
        body["accountCount"].as_i64(),
        Some(3),
        "case \"whole month\": expected accountCount 3, got {body}"
    );
    // 1,700 (10% of the first 10,000 plus 7% of the next 10,000) + 30 (3% of
    // 1,000) + 0 (Basic Support is free).
    assert_eq!(
        body["totalSupportCharge"].as_str(),
        Some("1730.00"),
        "case \"whole month\": expected the month's summed charges, got {body}"
    );
    assert_eq!(
        body["totalSupportEligibleSpend"].as_str(),
        Some("21500.00"),
        "case \"whole month\": expected the month's summed eligible spend, got {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case \"whole month\": expected no further page, got {body}"
    );

    // The `accountId` filter narrows the LIST only: the month's totals and
    // account count are documented for all accounts in the month.
    let (status, filtered) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH, "accountId": MEMBER_ACCOUNT_ID }),
    )
    .await;
    assert_eq!(status, 200, "case \"filtered\": expected HTTP 200, got {status} {filtered}");
    assert_eq!(
        reported_accounts(&filtered),
        vec![MEMBER_ACCOUNT_ID.to_string()],
        "case \"filtered\": expected only the named account, got {filtered}"
    );
    assert_eq!(
        filtered["accountCount"].as_i64(),
        body["accountCount"].as_i64(),
        "case \"filtered\": expected the month's account count unchanged, got {filtered}"
    );
    assert_eq!(
        filtered["totalSupportCharge"].as_str(),
        body["totalSupportCharge"].as_str(),
        "case \"filtered\": expected the month's total charge unchanged, got {filtered}"
    );

    // An account outside the consolidated billing family is nobody else's to
    // read, filter or not.
    let (status, outside) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH, "accountId": OUTSIDE_ACCOUNT_ID }),
    )
    .await;
    assert_eq!(status, 200, "case \"outside the family\": expected HTTP 200, got {status} {outside}");
    assert_eq!(
        reported_accounts(&outside),
        Vec::<String>::new(),
        "case \"outside the family\": expected no charges, got {outside}"
    );

    // The earlier month carries only its own bill.
    let (status, earlier) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": EARLIER_MONTH }),
    )
    .await;
    assert_eq!(status, 200, "case \"earlier month\": expected HTTP 200, got {status} {earlier}");
    assert_eq!(
        reported_accounts(&earlier),
        vec![MEMBER_ACCOUNT_ID.to_string()],
        "case \"earlier month\": expected only that month's bill, got {earlier}"
    );
    // 1,000 (10% of 10,000) + 4,200 (7% of 60,000).
    assert_eq!(
        earlier["totalSupportCharge"].as_str(),
        Some("5200.00"),
        "case \"earlier month\": expected that month's charge, got {earlier}"
    );

    // A month nothing was billed in is an empty month, not a refusal.
    let (status, empty) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": "2023-07" }),
    )
    .await;
    assert_eq!(status, 200, "case \"empty month\": expected HTTP 200, got {status} {empty}");
    assert_eq!(
        reported_accounts(&empty),
        Vec::<String>::new(),
        "case \"empty month\": expected no charges, got {empty}"
    );
    assert_eq!(
        (
            empty["accountCount"].as_i64(),
            empty["totalSupportCharge"].as_str(),
            empty["totalSupportEligibleSpend"].as_str(),
        ),
        (Some(0), Some("0.00"), Some("0.00")),
        "case \"empty month\": expected a zeroed month, got {empty}"
    );

    // Five reads, nothing written.
    assert_eq!(
        resident(&engine),
        family_seeds().len(),
        "case \"reads do not write\": expected the seeded records only, got {}",
        resident(&engine)
    );
}

#[tokio::test]
async fn pages_the_month_by_max_results() {
    for page_size in [1_i64, 2, 3] {
        let case = format!("maxResults {page_size}");
        let (router, engine) = emulator();
        persist(&engine, &family_seeds());

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "billingMonth": PAST_MONTH, "maxResults": page_size });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListBusinessSupportAccountCharges", &request).await;
            assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} {body}");
            let accounts = reported_accounts(&body);
            assert!(
                accounts.len() as i64 <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {accounts:?}"
            );
            // Every page reports the month's own totals, not the page's.
            assert_eq!(
                body["accountCount"].as_i64(),
                Some(3),
                "case {case:?}: expected the month's account count on every page, got {body}"
            );
            assert_eq!(
                body["totalSupportCharge"].as_str(),
                Some("1730.00"),
                "case {case:?}: expected the month's total on every page, got {body}"
            );
            seen.extend(accounts);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: expected the walk to end, got {seen:?} after {pages} pages"
            );
            next_token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen,
            vec![
                caller_account_id().to_string(),
                MEMBER_ACCOUNT_ID.to_string(),
                OTHER_MEMBER_ACCOUNT_ID.to_string(),
            ],
            "case {case:?}: expected the pages to partition the month in order, got {seen:?}"
        );
        assert_eq!(
            resident(&engine),
            family_seeds().len(),
            "case {case:?}: a paged read must not write, got {} records",
            resident(&engine)
        );
    }
}

#[tokio::test]
async fn refuses_a_token_it_did_not_issue() {
    let (router, engine) = emulator();
    persist(&engine, &family_seeds());

    let (status, body) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": PAST_MONTH, "nextToken": "notatokenthisserviceissued" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"forged token\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"forged token\": expected a ValidationException, got {body}"
    );
    assert_eq!(
        body["fieldList"][0]["name"].as_str(),
        Some("nextToken"),
        "case \"forged token\": expected the refusal to name nextToken, got {body}"
    );
}

#[tokio::test]
async fn estimates_the_current_month_and_refuses_one_that_has_not_begun() {
    let (router, engine) = emulator();
    let current = month_from_now(0);
    {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        assert!(
            state.record_support_eligible_spend(SupportEligibleSpend {
                account_id: caller_account_id().to_string(),
                billing_month: current.clone(),
                support_plan_name: SUPPORT_PLAN_BUSINESS.to_string(),
                currency_code: "USD".to_string(),
                by_service: vec![SupportServiceSpend {
                    contributing_service: "Amazon Elastic Compute Cloud".to_string(),
                    item_type: SPEND_ITEM_TYPE_USAGE.to_string(),
                    description: None,
                    charge_amount_minor: units(20_000),
                }],
                discount: None,
            }),
            "case \"current month\": the seeded spend must fit in the store"
        );
    }

    let (status, body) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": current }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"current month\": expected HTTP 200, got {status} {body}"
    );
    // Guard against the calendar turning over mid-test: the month asked for is
    // only the current one while it still is.
    if month_from_now(0) == current {
        assert_eq!(
            body["isEstimated"].as_bool(),
            Some(true),
            "case \"current month\": expected the current month's charges to be estimated, got {body}"
        );
        assert_eq!(
            charge_for(&body, caller_account_id())["totalCharge"].as_str(),
            Some("1700.00"),
            "case \"current month\": expected the month's charge priced, got {body}"
        );
    }

    // A past month is finalized...
    let (status, past) = call(
        &router,
        "ListBusinessSupportAccountCharges",
        &json!({ "billingMonth": month_from_now(-2) }),
    )
    .await;
    assert_eq!(status, 200, "case \"past month\": expected HTTP 200, got {status} {past}");
    assert_eq!(
        past["isEstimated"].as_bool(),
        Some(false),
        "case \"past month\": expected a past month to be finalized, got {past}"
    );

    // ...and a month that has not begun is neither estimated nor finalized, so
    // it is refused rather than answered with an empty month.
    for offset in [1_i32, 2, 13] {
        let future = month_from_now(offset);
        let case = format!("{offset} months ahead");
        let (status, body) = call(
            &router,
            "ListBusinessSupportAccountCharges",
            &json!({ "billingMonth": future }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {future}, got {status} {body}"
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

    assert_eq!(
        resident(&engine),
        1,
        "case \"reads do not write\": expected the one seeded record, got {}",
        resident(&engine)
    );
}
