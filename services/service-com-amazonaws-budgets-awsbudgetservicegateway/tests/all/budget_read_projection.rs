// SPDX-License-Identifier: CC0-1.0

//! What a budget READ reports, beyond what the caller sent: the members the service
//! fills in on the write path (`TimePeriod`, a `COST` budget's `CostTypes`,
//! `HealthStatus`, a notification's `NotificationState`), the `LastUpdatedTime` it
//! stored, and the `BudgetLimit` it derives from `PlannedBudgetLimits`.
//!
//! Every test states a RULE and asserts it over a table; the request the issue recorded
//! is one row. The harness (`router`, `call`, `account`, …) is the one
//! `tests/all/budget_lifecycle.rs` already builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Spend, BUDGET_TYPE_VALUES, COMPARISON_OPERATOR_VALUES, HEALTH_STATUS_VALUE_VALUES,
    NOTIFICATION_STATE_VALUES, NOTIFICATION_TYPE_VALUES, TIME_UNIT_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    default_time_period_start, planned_limit_in_force,
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, HEALTH_STATUS_HEALTHY,
    NOTIFICATION_STATE_OK,
};

/// The service's own constants must BE the values the model and the recording carry —
/// pinned here so the assertions below can safely spell the literals out.
#[test]
fn the_services_own_status_constants_are_the_modelled_values() {
    assert_eq!(
        HEALTH_STATUS_HEALTHY, "HEALTHY",
        "case health-status-constant: expected the HealthStatusValue member HEALTHY"
    );
    assert_eq!(
        NOTIFICATION_STATE_OK, "OK",
        "case notification-state-constant: expected the NotificationState member OK"
    );
}

use super::budget_lifecycle::{
    caller_account, call, limit_amount_for, router, router_with_engine, same,
};
use super::recorded_response_members::without;

/// `06/15/87 00:00 UTC` — the end date `TimePeriod$End` documents for a budget created
/// without one ("If you didn't specify an end date, Amazon Web Services set your end
/// date to `06/15/87 00:00 UTC`"), written as the date it is.
const DEFAULT_END_CIVIL: &str = "2087-06-15T00:00:00Z";

/// Seconds in a day — the midnight-UTC grid every defaulted `TimePeriod` boundary sits on.
const DAY_SECS: i64 = 86_400;

/// Now, in epoch seconds.
///
/// `pub(crate)` so the sibling suites in this test target read the clock through the one
/// helper rather than copying it.
pub(crate) fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test host's clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch that fits in i64")
}

/// `secs` rendered as `YYYY-MM-DDThh:mm:ssZ`, which is what the calendar assertions
/// below read dates off.
///
/// `pub(crate)` so the sibling suites in this test target render an instant through the
/// one helper rather than copying it.
pub(crate) fn civil(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("{secs} must render as a date-time: {error}"))
}

/// The instant `YYYY-MM-DDThh:mm:ssZ` names, in epoch seconds — the shared helper, so
/// this suite and `tests/all/budget_validation_rules.rs` read dates the same way.
use super::budget_lifecycle::civil_secs;

/// The `YYYY-MM-DD` date part of a rendered instant.
fn civil_day(secs: i64) -> String {
    civil(secs)
        .split('T')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The midnight-UTC start of the month containing `secs` — the grid a monthly
/// `PlannedBudgetLimits` key sits on ("The `key` is the start of the month, `UTC` in
/// epoch seconds"). Computed with the service's own boundary helper, which the fixed
/// table in `the_defaulted_start_of_a_known_instant_is_the_period_boundary` pins
/// instant by instant.
fn month_start(secs: i64) -> i64 {
    let at = UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).expect("a positive epoch"));
    default_time_period_start("MONTHLY", at).secs()
}

/// The start of the month `months` before (negative) or after (positive) the month
/// containing now. Walked one month at a time off the month grid — stepping 31 days on
/// from the first of a month always lands in the next month, never past it — so no
/// month is skipped whatever its length.
///
/// `pub(crate)` so the sibling suites in this test target build planned-limit keys off
/// the one helper rather than copying the calendar walk.
pub(crate) fn month_start_offset(months: i32) -> i64 {
    let mut start = month_start(now_secs());
    for _ in 0..months.abs() {
        start = if months < 0 {
            month_start(start - 1)
        } else {
            month_start(start + 31 * DAY_SECS)
        };
    }
    start
}

/// One row of the skipped-key table: the case, the planned entries as
/// `(key, amount)` pairs, and the amount the derivation must report for them.
type SkippedKeyRow = (
    &'static str,
    Vec<(String, &'static str)>,
    Option<&'static str>,
);

/// RULE: a `PlannedBudgetLimits` key that names no period is skipped when the limit in
/// force is derived, rather than making the read fail. Asserted against the projection
/// helper directly: the write paths refuse such a key now
/// (`tests/all/budget_validation_rules.rs`), so a stored budget can only carry one from a
/// store written before that rule — which a read still has to project.
#[test]
fn a_planned_key_that_names_no_period_is_skipped_by_the_derivation() {
    let spend = |amount: &str| Spend {
        amount: amount.to_string(),
        unit: "USD".to_string(),
    };
    // Rows differ along the form of the unusable key and whether a usable one remains.
    let rows: Vec<SkippedKeyRow> = vec![
        (
            "a key that is not a number, beside one that is",
            vec![
                ("not-an-epoch".to_string(), "5"),
                (month_start_offset(-1).to_string(), "100"),
            ],
            Some("100"),
        ),
        (
            "an empty key, beside one that is a period",
            vec![
                (String::new(), "5"),
                (month_start_offset(-1).to_string(), "7"),
            ],
            Some("7"),
        ),
        (
            "no usable key at all",
            vec![("not-an-epoch".to_string(), "5")],
            None,
        ),
    ];
    for (case, entries, expected) in rows {
        let planned: std::collections::HashMap<String, Spend> = entries
            .into_iter()
            .map(|(key, amount)| (key, spend(amount)))
            .collect();
        let in_force = planned_limit_in_force(&planned, SystemTime::now());
        let amount = in_force.as_ref().map(|limit| limit.amount.as_str());
        assert_eq!(
            amount, expected,
            "case {case:?}: expected the limit in force to be {expected:?}, got {amount:?}"
        );
    }
}

/// A timestamp member of a read response, in epoch seconds (fractions included: the
/// awsJson wire form of a timestamp carries them, and two writes inside the same second
/// differ only there).
fn epoch_secs(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// As [`epoch_secs`], truncated to whole seconds for the calendar assertions below.
fn epoch_whole(value: &Value, member: &str) -> Option<i64> {
    epoch_secs(value, member).map(|seconds| seconds as i64)
}

/// Create `budget` for `account_id`, then read it back through BOTH read paths and
/// assert they agree. Returns the `Budget` as read.
async fn create_and_read(
    router: &MultiServiceRouter,
    account_id: &str,
    budget: &Value,
    case: &str,
) -> Value {
    let name = budget["BudgetName"]
        .as_str()
        .expect("every row names its budget")
        .to_string();
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": &name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
    );
    let read = body["Budget"].clone();
    let (status, body) = call(
        router,
        "DescribeBudgets",
        &json!({"AccountId": account_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgets to answer 200, got {status} {body}"
    );
    let listed = body["Budgets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|listed| listed["BudgetName"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: the created budget is not listed: {body}"));
    // Both read paths report the same stored budget, apart from the `CalculatedSpend`
    // the single read computes and the listing leaves out
    // (`tests/all/recorded_response_members.rs`).
    assert!(
        same(&without(&read, &["CalculatedSpend"]), &listed),
        "case {case:?}: the two read paths must report the same budget apart from the \
         tracked spend, got {read} from DescribeBudget and {listed} from DescribeBudgets"
    );
    read
}

/// The default end date really is the instant the documentation names.
#[test]
fn the_documented_default_end_date_is_the_instant_it_claims() {
    let rendered = civil(civil_secs(DEFAULT_END_CIVIL));
    assert_eq!(
        rendered, DEFAULT_END_CIVIL,
        "case default-end: expected 06/15/87 00:00 UTC, got {rendered}"
    );
}

/// RULE, pinned against a FIXED table instead of the clock: the `Start` defaulted for a
/// write at a known instant is the midnight-UTC boundary of the period the `TimeUnit`
/// names. Every instant and expectation below is written out by hand as the date it is
/// (`TimePeriod$Start` supplies the first two rows itself: a budget created on January
/// 24, 2018 starts `01/24/18 00:00 UTC` under DAILY and `01/01/18 00:00 UTC` under
/// MONTHLY), so a wrong month or quarter cannot be mirrored by the expectation.
#[test]
fn the_defaulted_start_of_a_known_instant_is_the_period_boundary() {
    // (case, the instant written, TimeUnit, the expected Start).
    let rows: &[(&str, &str, &str, &str)] = &[
        // The model's own two examples.
        (
            "DAILY, mid-day",
            "2018-01-24T13:45:00Z",
            "DAILY",
            "2018-01-24T00:00:00Z",
        ),
        (
            "MONTHLY, mid-month",
            "2018-01-24T13:45:00Z",
            "MONTHLY",
            "2018-01-01T00:00:00Z",
        ),
        // CUSTOM names no period length, so it takes the day, like DAILY.
        (
            "CUSTOM, mid-day",
            "2018-01-24T13:45:00Z",
            "CUSTOM",
            "2018-01-24T00:00:00Z",
        ),
        // January is in the first calendar quarter, which starts with the year.
        (
            "QUARTERLY, Q1",
            "2018-01-24T13:45:00Z",
            "QUARTERLY",
            "2018-01-01T00:00:00Z",
        ),
        (
            "ANNUALLY, January",
            "2018-01-24T13:45:00Z",
            "ANNUALLY",
            "2018-01-01T00:00:00Z",
        ),
        // A boundary already on the grid stays where it is.
        (
            "DAILY, at midnight",
            "2018-01-24T00:00:00Z",
            "DAILY",
            "2018-01-24T00:00:00Z",
        ),
        // August is in Q3, which starts on the first of July.
        (
            "QUARTERLY, Q3",
            "2018-08-15T00:00:00Z",
            "QUARTERLY",
            "2018-07-01T00:00:00Z",
        ),
        // The last second of a month still belongs to that month.
        (
            "MONTHLY, last second",
            "2018-02-28T23:59:59Z",
            "MONTHLY",
            "2018-02-01T00:00:00Z",
        ),
        // The last second of a year still belongs to that year.
        (
            "ANNUALLY, last second",
            "2019-12-31T23:59:59Z",
            "ANNUALLY",
            "2019-01-01T00:00:00Z",
        ),
        // A leap year's December is in Q4, which starts on the first of October.
        (
            "QUARTERLY, leap-year Q4",
            "2020-12-31T00:00:00Z",
            "QUARTERLY",
            "2020-10-01T00:00:00Z",
        ),
        // A `TimeUnit` the model does not declare yet takes the narrowest boundary.
        (
            "an unknown TimeUnit",
            "2018-01-24T13:45:00Z",
            "FORTNIGHTLY",
            "2018-01-24T00:00:00Z",
        ),
    ];
    for (case, at, time_unit, expected) in rows {
        let written = UNIX_EPOCH
            + Duration::from_secs(u64::try_from(civil_secs(at)).expect("a positive epoch"));
        let start = default_time_period_start(time_unit, written);
        assert_eq!(
            civil(start.secs()),
            *expected,
            "case {case:?}: expected the Start {expected}, got {}",
            civil(start.secs())
        );
    }
}

/// The date `TimePeriod$Start` documents for a budget written at `at` under `time_unit`,
/// as `YYYY-MM-DD`: "if you created your budget on January 24, 2018, chose `DAILY`, and
/// didn't set a start date, Amazon Web Services set your start date to `01/24/18 00:00
/// UTC`. If you chose `MONTHLY`, Amazon Web Services set your start date to `01/01/18
/// 00:00 UTC`". `CUSTOM` names no period length, so it takes the day.
///
/// Only the boundary of the period containing `at` is derived here — which month a
/// quarter starts in is pinned, instant by instant, by the fixed table above.
fn period_boundary(time_unit: &str, at: i64) -> String {
    let day = civil_day(at);
    let (year, month) = (&day[0..4], &day[5..7]);
    match time_unit {
        "MONTHLY" => format!("{year}-{month}-01"),
        "QUARTERLY" => {
            let quarter_start = match month {
                "01" | "02" | "03" => "01",
                "04" | "05" | "06" => "04",
                "07" | "08" | "09" => "07",
                _ => "10",
            };
            format!("{year}-{quarter_start}-01")
        }
        "ANNUALLY" => format!("{year}-01-01"),
        _ => day,
    }
}

/// RULE: a budget always covers a period, so a read reports a `TimePeriod` whose
/// `Start` the caller supplied or the service defaulted to the midnight-UTC boundary of
/// the period its `TimeUnit` names, and whose `End` the caller supplied or the service
/// defaulted to `06/15/87 00:00 UTC`.
#[tokio::test]
async fn a_read_reports_the_time_period_the_caller_sent_or_the_service_defaulted() {
    // Every `TimeUnit` the model enumerates — the catalog is the generated
    // `TIME_UNIT_VALUES`, so this covers entries the issue never cited (CUSTOM, DAILY,
    // ANNUALLY), not only the recorded MONTHLY one.
    for time_unit in TIME_UNIT_VALUES.iter() {
        let case = format!("no TimePeriod, {time_unit}");
        // One store per row, so rows cannot see each other's budgets: every request
        // names the one account this service serves.
        let router = router();
        let account_id = caller_account();
        // The instants either side of the write, so an assertion cannot go off because
        // the create straddled a UTC day, month, quarter or year boundary.
        let before = now_secs();
        let read = create_and_read(
            &router,
            &account_id,
            &json!({
                "BudgetName": format!("defaulted-{time_unit}"),
                "BudgetLimit": {"Amount": "300.0", "Unit": "USD"},
                "TimeUnit": time_unit,
                "BudgetType": "COST"
            }),
            &case,
        )
        .await;
        let after = now_secs();

        let period = read["TimePeriod"].clone();
        let start = epoch_whole(&period, "Start")
            .unwrap_or_else(|| panic!("case {case:?}: expected a defaulted Start, got {period}"));
        let end = epoch_whole(&period, "End")
            .unwrap_or_else(|| panic!("case {case:?}: expected a defaulted End, got {period}"));
        assert_eq!(
            start % DAY_SECS,
            0,
            "case {case:?}: expected a midnight-UTC Start, got {start}"
        );
        assert!(
            start <= after,
            "case {case:?}: expected a Start at or before now ({after}), got {start}"
        );
        let started = civil_day(start);
        let boundaries = [
            period_boundary(time_unit, before),
            period_boundary(time_unit, after),
        ];
        assert!(
            boundaries.contains(&started),
            "case {case:?}: expected the boundary of the period containing the write \
             ({boundaries:?}), got {started}"
        );
        assert_eq!(
            civil(end),
            DEFAULT_END_CIVIL,
            "case {case:?}: expected the documented default end date, got {}",
            civil(end)
        );
    }

    // A period the caller supplied is reported as sent; only the half that is missing is
    // defaulted. Rows differ along which half that is.
    let supplied: [(&str, Value, Option<i64>, Option<i64>); 3] = [
        (
            // The end date is ahead of now: a budget past its `TimePeriod$End` is deleted
            // (`tests/all/budget_end_date_expiry.rs`), so there would be nothing to read.
            "both halves supplied",
            json!({"Start": 1_704_067_200, "End": 2_556_144_000_i64}),
            Some(1_704_067_200),
            Some(2_556_144_000),
        ),
        (
            "only Start supplied",
            json!({"Start": 1_704_067_200}),
            Some(1_704_067_200),
            None,
        ),
        (
            // An end date in the far future, but still before the latest one a budget can
            // carry (`Budget$TimePeriod`: "The end date must come before 06/15/87 00:00
            // UTC") — `tests/all/budget_validation_rules.rs` covers that bound itself.
            "only End supplied",
            json!({"End": 2_524_608_000_i64}),
            None,
            Some(2_524_608_000),
        ),
    ];
    for (case, period, want_start, want_end) in supplied {
        let router = router();
        let account_id = caller_account();
        let read = create_and_read(
            &router,
            &account_id,
            &json!({
                "BudgetName": "partial-period",
                "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "TimePeriod": period
            }),
            case,
        )
        .await;
        let read_period = read["TimePeriod"].clone();
        let start = epoch_whole(&read_period, "Start")
            .unwrap_or_else(|| panic!("case {case:?}: expected a Start, got {read_period}"));
        let end = epoch_whole(&read_period, "End")
            .unwrap_or_else(|| panic!("case {case:?}: expected an End, got {read_period}"));
        match want_start {
            Some(sent) => assert_eq!(
                start, sent,
                "case {case:?}: expected the Start the caller sent, got {start}"
            ),
            None => {
                let started = civil_day(start);
                assert!(
                    start % DAY_SECS == 0 && started.ends_with("-01"),
                    "case {case:?}: expected the defaulted monthly Start, got {started}"
                );
            }
        }
        match want_end {
            Some(sent) => assert_eq!(
                end, sent,
                "case {case:?}: expected the End the caller sent, got {end}"
            ),
            None => assert_eq!(
                civil(end),
                DEFAULT_END_CIVIL,
                "case {case:?}: expected the defaulted End, got {}",
                civil(end)
            ),
        }
    }
}

/// The per-member defaults the model documents on `CostTypes` ("The default value is
/// `true`" on every `Include*` member, `false` on `UseBlended` and `UseAmortized`).
/// `pub(crate)` so the sibling suite that asserts WHICH budgets hold cost types
/// (`tests/all/budget_cost_types_by_type.rs`) spans the same member list.
pub(crate) const COST_TYPE_DEFAULTS: &[(&str, bool)] = &[
    ("IncludeTax", true),
    ("IncludeSubscription", true),
    ("IncludeRefund", true),
    ("IncludeCredit", true),
    ("IncludeUpfront", true),
    ("IncludeRecurring", true),
    ("IncludeOtherSubscription", true),
    ("IncludeSupport", true),
    ("IncludeDiscount", true),
    ("UseBlended", false),
    ("UseAmortized", false),
];

/// RULE: a `COST` budget's read reports every `CostTypes` member, each one the value the
/// caller sent or the default the model documents for it; a budget of any other type has
/// no `CostTypes` of its own ("`USAGE`, `RI_UTILIZATION`, `RI_COVERAGE`,
/// `SAVINGS_PLANS_UTILIZATION`, and `SAVINGS_PLANS_COVERAGE` budgets don't have
/// `CostTypes`"). Either way the read reports the `HealthStatus` the service stamped.
#[tokio::test]
async fn a_read_reports_the_cost_types_and_health_status_the_service_stamped() {
    // The partial map one row sends: one `Include*` member off and one rate member on,
    // so both halves of the default table are exercised against a caller's value.
    let partial = json!({"IncludeTax": false, "UseAmortized": true});

    // Every `BudgetType` the model enumerates, from the generated catalog.
    for budget_type in BUDGET_TYPE_VALUES.iter() {
        for (variant, sent) in [("no CostTypes", None), ("partial CostTypes", Some(&partial))] {
            let case = format!("{budget_type}, {variant}");
            let router = router();
            let account_id = caller_account();
            let mut budget = json!({
                "BudgetName": format!("cost-types-{budget_type}"),
                // The amount a budget of this type may carry ([`limit_amount_for`]):
                // this table is about cost types and health status, not the limit.
                "BudgetLimit": {"Amount": limit_amount_for(budget_type, "5"), "Unit": "USD"},
                "TimeUnit": "MONTHLY",
                "BudgetType": budget_type
            });
            if let Some(supplied) = sent {
                budget["CostTypes"] = supplied.clone();
            }
            let read = create_and_read(&router, &account_id, &budget, &case).await;

            let cost_types = read.get("CostTypes").cloned();
            if *budget_type == "COST" {
                let cost_types = cost_types
                    .unwrap_or_else(|| panic!("case {case:?}: expected CostTypes, got {read}"));
                for (member, default) in COST_TYPE_DEFAULTS {
                    let expected = sent
                        .and_then(|supplied| supplied.get(member).and_then(Value::as_bool))
                        .unwrap_or(*default);
                    let actual = cost_types.get(member).and_then(Value::as_bool);
                    assert_eq!(
                        actual,
                        Some(expected),
                        "case {case:?}: expected {member} = {expected}, got {actual:?}"
                    );
                }
            } else {
                // A budget type that has no cost types reads back with none: nothing is
                // defaulted for it, and a map the caller sent is not stored either.
                assert!(
                    cost_types.as_ref().is_none_or(Value::is_null),
                    "case {case:?}: expected no CostTypes, got {cost_types:?}"
                );
            }

            let health = read["HealthStatus"].clone();
            let status = health.get("Status").and_then(Value::as_str);
            // The literal, not the crate's own constant: the expectation has to be able
            // to fail if the service ever reported the other `HealthStatusValue`.
            assert_eq!(
                status,
                Some("HEALTHY"),
                "case {case:?}: expected a HEALTHY HealthStatus, got {health}"
            );
            assert!(
                HEALTH_STATUS_VALUE_VALUES.contains(&status.unwrap_or_default()),
                "case {case:?}: the reported Status must be a modelled HealthStatusValue, \
                 got {status:?}"
            );
            assert!(
                epoch_secs(&health, "LastUpdatedTime").is_some_and(|seen| seen > 0.0),
                "case {case:?}: expected the HealthStatus to carry its LastUpdatedTime, \
                 got {health}"
            );
            assert!(
                health.get("StatusReason").is_none_or(Value::is_null),
                "case {case:?}: a healthy budget has no StatusReason, got {health}"
            );
        }
    }

    // A COST budget written in the other filter/metric syntax has no cost types either:
    // "Either FilterExpression and Metrics or CostFilters and CostTypes, not both"
    // (CreateBudget), so defaulting them here would store the forbidden combination.
    let case = "COST, FilterExpression/Metrics syntax";
    let router = router();
    let read = create_and_read(
        &router,
        &caller_account(),
        &json!({
            "BudgetName": "expression-syntax",
            "BudgetLimit": {"Amount": "5", "Unit": "USD"},
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}},
            "Metrics": ["UnblendedCost"]
        }),
        case,
    )
    .await;
    let cost_types = read.get("CostTypes").cloned();
    assert!(
        cost_types.as_ref().is_none_or(Value::is_null),
        "case {case:?}: expected no CostTypes beside Metrics, got {cost_types:?}"
    );
}

/// RULE: `LastUpdatedTime` is the stored time of the last write, not the time of the
/// read — successive reads report the same value, an update restamps it, and the
/// `HealthStatus` keeps the timestamp of the status it still reports.
#[tokio::test]
async fn last_updated_time_is_the_stored_write_time_not_the_time_of_the_read() {
    // Rows differing along time unit and type: the rule is about the write path, not
    // about any one budget shape.
    for (time_unit, budget_type) in [
        ("MONTHLY", "COST"),
        ("DAILY", "USAGE"),
        ("ANNUALLY", "RI_COVERAGE"),
    ] {
        let case = format!("{time_unit}/{budget_type}");
        let router = router();
        let account_id = caller_account();
        let name = format!("stamped-{time_unit}");
        let budget = json!({
            "BudgetName": &name,
            // The amount a budget of this type may carry ([`limit_amount_for`]): this
            // table is about the stored write time, not about the limit.
            "BudgetLimit": {"Amount": limit_amount_for(budget_type, "1"), "Unit": "USD"},
            "TimeUnit": time_unit,
            "BudgetType": budget_type
        });
        let read = create_and_read(&router, &account_id, &budget, &case).await;
        let stored = epoch_secs(&read, "LastUpdatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: expected a LastUpdatedTime, got {read}"));
        let health_stamped = epoch_secs(&read["HealthStatus"], "LastUpdatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: expected a health timestamp, got {read}"));

        // The clock moves between the reads, so a value recomputed per request would
        // differ from the one stored at create.
        let describe = json!({"AccountId": &account_id, "BudgetName": &name});
        for attempt in 0..2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let (_, body) = call(&router, "DescribeBudget", &describe).await;
            let seen = epoch_secs(&body["Budget"], "LastUpdatedTime");
            assert_eq!(
                seen,
                Some(stored),
                "case {case:?}: read {attempt} must report the stored LastUpdatedTime \
                 {stored}, got {seen:?}"
            );
        }

        // An update is a write, so it restamps the budget; the health status it still
        // reports did not change, so neither did that status's own timestamp.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, body) = call(
            &router,
            "UpdateBudget",
            &json!({"AccountId": &account_id, "NewBudget": budget}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        let (_, body) = call(&router, "DescribeBudget", &describe).await;
        let updated = epoch_secs(&body["Budget"], "LastUpdatedTime");
        assert!(
            updated.is_some_and(|seen| seen > stored),
            "case {case:?}: expected the update to restamp LastUpdatedTime past {stored}, \
             got {updated:?}"
        );
        let health_after = epoch_secs(&body["Budget"]["HealthStatus"], "LastUpdatedTime");
        assert_eq!(
            health_after,
            Some(health_stamped),
            "case {case:?}: the health status did not change, so its LastUpdatedTime \
             must still be {health_stamped}, got {health_after:?}"
        );
    }
}

/// RULE: a budget created with `PlannedBudgetLimits` also reports a `BudgetLimit` — the
/// planned entry whose period has most recently started (the first entry when every
/// planned period is still in the future) — derived on the read and never written back,
/// while the planned map itself is reported as stored.
#[tokio::test]
async fn a_planned_limit_budget_reports_the_limit_in_force() {
    // Rows differ along the number of planned periods, whether they are past or future,
    // and the order they are written in. Every key is the start of a month, which is
    // what `Budget$PlannedBudgetLimits` keys a monthly plan by ("The `key` is the start
    // of the month, `UTC` in epoch seconds") and what the write path now requires.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "one period, already started",
            json!({month_start_offset(-1).to_string(): {"Amount": "50.0", "Unit": "USD"}}),
            "50.0",
        ),
        (
            "several periods, the latest started one is in force",
            json!({
                month_start_offset(-3).to_string(): {"Amount": "10", "Unit": "USD"},
                month_start_offset(-2).to_string(): {"Amount": "20", "Unit": "USD"},
                month_start_offset(-1).to_string(): {"Amount": "30", "Unit": "USD"},
                month_start_offset(1).to_string(): {"Amount": "40", "Unit": "USD"}
            }),
            "30",
        ),
        (
            "the same periods written in the opposite order",
            json!({
                month_start_offset(1).to_string(): {"Amount": "40", "Unit": "USD"},
                month_start_offset(-1).to_string(): {"Amount": "30", "Unit": "USD"},
                month_start_offset(-2).to_string(): {"Amount": "20", "Unit": "USD"},
                month_start_offset(-3).to_string(): {"Amount": "10", "Unit": "USD"}
            }),
            "30",
        ),
        (
            "the last planned limit continues once the plan runs out",
            json!({
                month_start_offset(-2).to_string(): {"Amount": "70", "Unit": "USD"},
                month_start_offset(-1).to_string(): {"Amount": "80", "Unit": "USD"}
            }),
            "80",
        ),
        (
            "every planned period is still in the future",
            json!({
                month_start_offset(1).to_string(): {"Amount": "90", "Unit": "USD"},
                month_start_offset(2).to_string(): {"Amount": "95", "Unit": "USD"}
            }),
            "90",
        ),
    ];

    for (case, planned, expected_amount) in rows {
        let (router, engine) = router_with_engine();
        let account_id = caller_account();
        let name = "planned";
        let read = create_and_read(
            &router,
            &account_id,
            &json!({
                "BudgetName": name,
                "PlannedBudgetLimits": planned.clone(),
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST"
            }),
            case,
        )
        .await;

        let limit = read["BudgetLimit"].clone();
        assert!(
            same(
                &json!({"Amount": expected_amount, "Unit": "USD"}),
                &limit
            ),
            "case {case:?}: expected the planned limit in force \
             {{Amount: {expected_amount}, Unit: USD}}, got {limit}"
        );
        assert!(
            same(&planned, &read["PlannedBudgetLimits"]),
            "case {case:?}: expected the planned map to be reported as stored {planned}, \
             got {}",
            read["PlannedBudgetLimits"]
        );

        // Derived on the read: the stored budget still carries no `BudgetLimit` of its
        // own, so a later read derives it again from the clock rather than from a value
        // this read wrote back.
        let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
        let store = state.lock();
        let stored = store
            .budgets
            .get(&(account_id.clone(), name.to_string()))
            .unwrap_or_else(|| panic!("case {case:?}: the budget must be stored"));
        assert!(
            stored.budget.budget_limit.is_none(),
            "case {case:?}: a read must not write the derived BudgetLimit back, found {:?}",
            stored.budget.budget_limit
        );
    }

    // A budget created with its own `BudgetLimit` keeps it: there is nothing to derive.
    let router = router();
    let account_id = caller_account();
    let read = create_and_read(
        &router,
        &account_id,
        &json!({
            "BudgetName": "fixed",
            "BudgetLimit": {"Amount": "123.45", "Unit": "USD"},
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST"
        }),
        "a fixed limit is reported as sent",
    )
    .await;
    assert!(
        same(
            &json!({"Amount": "123.45", "Unit": "USD"}),
            &read["BudgetLimit"]
        ),
        "case \"a fixed limit is reported as sent\": got {}",
        read["BudgetLimit"]
    );
}

/// RULE: every notification a budget was created with is reported by
/// `DescribeNotificationsForBudget`, with the members the caller sent and a
/// `NotificationState` the SERVICE decides — so the state reported for one notification
/// is the SAME whichever state the request carried for it, and is always one of the
/// modelled values.
///
/// Which state that is, per what the notification alerts on, is the table in
/// `tests/all/notification_alarm_state.rs`; what this rule pins is that the request has
/// no say in it.
#[tokio::test]
async fn a_notification_is_reported_with_the_state_the_service_computes() {
    // The catalogs are the generated `NOTIFICATION_TYPE_VALUES` /
    // `COMPARISON_OPERATOR_VALUES` / `NOTIFICATION_STATE_VALUES` constants, and the
    // thresholds are both published bounds of `NotificationThreshold`
    // (@range(min: 0, max: 15000000000000)) plus the recorded 80.
    let thresholds = [0.0_f64, 80.0, 15_000_000_000_000.0];
    let mut index = 0_u64;
    for notification_type in NOTIFICATION_TYPE_VALUES {
        for comparison in COMPARISON_OPERATOR_VALUES {
            let threshold = thresholds[(index as usize) % thresholds.len()];
            index += 1;
            // What each read reported, keyed by the state the REQUEST carried, so the
            // three can be compared against each other below.
            let mut reported_states: Vec<(String, String)> = Vec::new();
            for sent_state in ::std::iter::once(None)
                .chain(NOTIFICATION_STATE_VALUES.iter().map(|state| Some(*state)))
            {
                let case =
                    format!("{notification_type}/{comparison}/{sent_state:?}/{threshold}");
                let account_id = caller_account();
                let router = router();
                let name = "notified";
                let mut notification = json!({
                    "NotificationType": notification_type,
                    "ComparisonOperator": comparison,
                    "Threshold": threshold,
                    "ThresholdType": "PERCENTAGE"
                });
                if let Some(state) = sent_state {
                    notification["NotificationState"] = json!(state);
                }
                let (status, body) = call(
                    &router,
                    "CreateBudget",
                    &json!({
                        "AccountId": &account_id,
                        "Budget": {
                            "BudgetName": name,
                            "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                            "TimeUnit": "MONTHLY",
                            "BudgetType": "COST"
                        },
                        "NotificationsWithSubscribers": [{
                            "Notification": notification.clone(),
                            "Subscribers": [
                                {"SubscriptionType": "EMAIL", "Address": "ops@example.com"}
                            ]
                        }]
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
                );

                let (status, body) = call(
                    &router,
                    "DescribeNotificationsForBudget",
                    &json!({"AccountId": &account_id, "BudgetName": name}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DescribeNotificationsForBudget 200, \
                     got {status} {body}"
                );
                let reported = body["Notifications"].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    reported.len(),
                    1,
                    "case {case:?}: expected the created notification to be reported, got {body}"
                );
                let read = &reported[0];
                for member in ["NotificationType", "ComparisonOperator", "Threshold"] {
                    let expected = &notification[member];
                    let actual = read.get(member).unwrap_or(&Value::Null);
                    assert!(
                        same(expected, actual),
                        "case {case:?}: expected {member} = {expected}, got {actual}"
                    );
                }
                // `ThresholdType` is the default percentage here, which the service keeps
                // in the one spelling an absent member already means, so no read reports
                // it (`tests/all/recorded_response_members.rs`).
                assert!(
                    read.get("ThresholdType").is_none(),
                    "case {case:?}: expected the default ThresholdType to be left out, got \
                     {read}"
                );
                // The reported state is one the model declares, and it is the service's
                // own: the comparison across the three requests below is what pins that
                // the request had no say in it.
                let actual_state = read
                    .get("NotificationState")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a NotificationState, got {read}")
                    });
                assert!(
                    NOTIFICATION_STATE_VALUES.contains(&actual_state),
                    "case {case:?}: expected a modelled NotificationState, got \
                     {actual_state:?}"
                );
                reported_states.push((
                    sent_state.unwrap_or("absent").to_string(),
                    actual_state.to_string(),
                ));
            }
            let (first_sent, first_state) = reported_states
                .first()
                .cloned()
                .expect("every notification was read back");
            for (sent, state) in &reported_states {
                assert_eq!(
                    state, &first_state,
                    "case {notification_type}/{comparison}/{threshold}: the notification \
                     sent with NotificationState {sent:?} read back as {state:?}, while \
                     the one sent with {first_sent:?} read back as {first_state:?} — the \
                     state a read reports must not depend on the request"
                );
            }
        }
    }
}

/// RULE: the notifications of a budget are reported in the order they were created,
/// however many there are — a budget created with none reports an empty list.
#[tokio::test]
async fn every_notification_a_budget_was_created_with_is_reported_in_order() {
    // Rows differ along count: none, one, several.
    for count in [0_usize, 1, 5] {
        let case = format!("{count} notifications");
        let router = router();
        let account_id = caller_account();
        let name = "ordered";
        let supplied: Vec<Value> = (0..count)
            .map(|position| {
                json!({
                    "Notification": {
                        "NotificationType": NOTIFICATION_TYPE_VALUES[position % NOTIFICATION_TYPE_VALUES.len()],
                        "ComparisonOperator": COMPARISON_OPERATOR_VALUES[position % COMPARISON_OPERATOR_VALUES.len()],
                        // A distinct threshold per position, so the order is observable.
                        "Threshold": (position as f64) + 1.5
                    },
                    "Subscribers": [
                        {"SubscriptionType": "EMAIL", "Address": format!("o{position}@example.com")}
                    ]
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": name,
                    "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST"
                },
                "NotificationsWithSubscribers": supplied.clone()
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            "DescribeNotificationsForBudget",
            &json!({"AccountId": &account_id, "BudgetName": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNotificationsForBudget 200, got {status} {body}"
        );
        let reported = body["Notifications"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            count,
            "case {case:?}: expected {count} notifications, got {body}"
        );
        for (position, sent) in supplied.iter().enumerate() {
            let expected = sent["Notification"]["Threshold"].clone();
            let actual = reported[position].get("Threshold").cloned().unwrap_or(Value::Null);
            assert!(
                same(&expected, &actual),
                "case {case:?}: notification {position} expected Threshold {expected}, \
                 got {actual}"
            );
        }
    }
}
