// SPDX-License-Identifier: CC0-1.0

//! The members of a `Budget` the SERVICE owns rather than the caller: the spend and
//! auto-adjust figures it derives from the cost it tracks (never taken from a request),
//! and the `BudgetLimit` a utilization or coverage budget defaults to.
//!
//! A derived member a request carried is discarded, and what a read reports for it is
//! whatever the service itself has: nothing for the auto-adjust figures, and for
//! `CalculatedSpend` the spend it computes for the budget (see [`computed_spend`]).
//!
//! Both write paths are covered: `CreateBudget`, an `UpdateBudget` that resends the
//! budget unchanged, and an `UpdateBudget` that REWRITES the budget's caller-owned parts
//! while supplying the service-owned ones — "you can change every part of a budget except
//! for the `budgetName` and the `calculatedSpend`" (`UpdateBudget`).
//!
//! Every test states a RULE and asserts it over a table; the requests the issues recorded
//! are rows of those tables. The harness (`router`, `call`, `account`, `carries`, …) is
//! the one `tests/all/budget_lifecycle.rs` already builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived catalog the tables below span, so they cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ATTRIBUTED_SPEND_AMOUNT, PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_BUDGET_TYPES,
    PERCENTAGE_LIMIT_UNIT,
};

use super::budget_lifecycle::{caller_account, call, carries, is_error, router, same};

/// The `BudgetType` members `Budget$BudgetLimit` requires a limit of ("`BudgetLimit` is
/// required for cost or usage budgets"), which is therefore the complement of the
/// utilization/coverage types that default to `100`.
///
/// Deliberately a SECOND, independent copy of the service's own private constant of the
/// same name (`src/state.rs`), not a stale duplicate of it: the pin below is only worth
/// anything if the expected set is spelled out here from the documentation rather than
/// read back out of the code under test.
const LIMIT_REQUIRED_BUDGET_TYPES: [&str; 2] = ["COST", "USAGE"];

/// Drive one write operation (`CreateBudget` with `Budget`, `UpdateBudget` with
/// `NewBudget`) for `account_id` and return the `Budget` the read paths report. The
/// write must be accepted, and BOTH read paths must report the same budget — the issue
/// behind this suite is observable through `DescribeBudget` and `DescribeBudgets` alike.
async fn write_and_read(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    account_id: &str,
    budget: &Value,
) -> Value {
    let request = match operation {
        "CreateBudget" => json!({"AccountId": account_id, "Budget": budget.clone()}),
        _ => json!({"AccountId": account_id, "NewBudget": budget.clone()}),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body}"
    );

    let name = budget["BudgetName"]
        .as_str()
        .expect("every row names its budget");
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
    );
    let read = body["Budget"].clone();

    let (status, body) = call(router, "DescribeBudgets", &json!({"AccountId": account_id})).await;
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
        .unwrap_or_else(|| panic!("case {case:?}: {name} is not listed after {operation}: {body}"));
    // Both read paths report the same stored budget, apart from the `CalculatedSpend` the
    // single read computes and the listing leaves out
    // (`tests/all/recorded_response_members.rs`).
    assert!(
        same(&without(&read, &["CalculatedSpend"]), &listed),
        "case {case:?}: the two read paths must report the same budget after {operation} \
         apart from the tracked spend, got {read} from DescribeBudget and {listed} from \
         DescribeBudgets"
    );
    // Every row of this suite is also a row of the spend rule: the budget reads back with
    // the spend the SERVICE computes for it, whether or not the request carried one.
    assert_computed_spend(case, &read);
    read
}

/// The `CalculatedSpend` the service computes for the budget a read reported: the spend
/// attributed to that budget (nothing spends in this emulator, so
/// `ATTRIBUTED_SPEND_AMOUNT`) measured in the unit of the limit the read carries, and no
/// forecast — there is no usage history to predict one from. `Value::Null` for a budget
/// that reports no limit at all, whose record names no unit of measurement.
fn computed_spend(read: &Value) -> Value {
    match read["BudgetLimit"]["Unit"].as_str() {
        Some(unit) => json!({"ActualSpend": {"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": unit}}),
        None => Value::Null,
    }
}

/// Assert the budget a read reported carries exactly the service-computed
/// `CalculatedSpend` ([`computed_spend`]) — never a figure a request supplied.
fn assert_computed_spend(case: &str, read: &Value) {
    let expected = computed_spend(read);
    let actual = member(read, &["CalculatedSpend"]);
    assert_eq!(
        actual, &expected,
        "case {case:?}: expected the service-computed CalculatedSpend {expected}, got \
         {actual} in {read}"
    );
}

/// The value at `path` (a trail of member names) in `budget`, or `Value::Null`.
fn member<'a>(budget: &'a Value, path: &[&str]) -> &'a Value {
    let mut here = budget;
    for segment in path {
        match here.get(segment) {
            Some(next) => here = next,
            None => return &Value::Null,
        }
    }
    here
}

/// Assert that `path` is absent from (or null in) the budget a read reported.
fn assert_absent(case: &str, budget: &Value, path: &[&str]) {
    let actual = member(budget, path);
    assert!(
        actual.is_null(),
        "case {case:?}: expected the service to report no {}, got {actual} in {budget}",
        path.join(".")
    );
}

/// One row of the service-derived table: the case, the `Budget` to write, and the member
/// paths the read must report NOTHING for, because the service has no value of its own
/// for them (the spend it computes is checked for every row by [`assert_computed_spend`]).
struct Derived {
    case: &'static str,
    budget: Value,
    dropped: Vec<Vec<&'static str>>,
    /// Member paths the caller DOES own, which must survive the write unchanged.
    kept: Value,
}

/// The table. Rows differ along what the rule generalises over: WHICH service-derived
/// member the request carries, its value (zero, small, the top of its modelled range),
/// the budget type and the time-unit granularity, and whether the budget is a fixed-limit,
/// planned-limit or auto-adjusting one.
fn derived_rows() -> Vec<Derived> {
    vec![
        Derived {
            // The request the issue recorded (step 1).
            case: "cost budget supplying an actual spend",
            budget: json!({
                "BudgetName": "echoed-spend",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "CalculatedSpend": {"ActualSpend": {"Amount": "777", "Unit": "USD"}}
            }),
            dropped: vec![],
            kept: json!({"BudgetLimit": {"Amount": "100", "Unit": "USD"}}),
        },
        Derived {
            case: "usage budget supplying an actual and a forecasted spend",
            budget: json!({
                "BudgetName": "echoed-forecast",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"},
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "0", "Unit": "GB"},
                    "ForecastedSpend": {"Amount": "12.25", "Unit": "GB"}
                }
            }),
            dropped: vec![],
            kept: json!({"BudgetLimit": {"Amount": "3.5", "Unit": "GB"}}),
        },
        Derived {
            case: "planned-limit budget supplying a spend",
            budget: json!({
                "BudgetName": "planned-with-spend",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}},
                "CalculatedSpend": {"ActualSpend": {"Amount": "9999999", "Unit": "USD"}}
            }),
            dropped: vec![],
            kept: json!({"PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}}}),
        },
        Derived {
            case: "RI coverage budget supplying a spend in percent",
            budget: json!({
                "BudgetName": "ri-coverage-with-spend",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "RI_COVERAGE",
                "CalculatedSpend": {"ActualSpend": {"Amount": "42", "Unit": "PERCENTAGE"}}
            }),
            dropped: vec![],
            kept: json!({"BudgetType": "RI_COVERAGE"}),
        },
        Derived {
            // The request the issue recorded (step 3).
            case: "historical auto-adjust supplying both derived auto-adjust members",
            budget: json!({
                "BudgetName": "auto-adjust-echo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {
                        "BudgetAdjustmentPeriod": 3,
                        "LookBackAvailablePeriods": 42
                    },
                    "LastAutoAdjustTime": 0
                }
            }),
            dropped: vec![
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
                vec!["AutoAdjustData", "HistoricalOptions", "LookBackAvailablePeriods"],
            ],
            kept: json!({
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 3}
                }
            }),
        },
        Derived {
            case: "daily historical auto-adjust at the top of the look-back range",
            budget: json!({
                "BudgetName": "auto-adjust-max",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {
                        "BudgetAdjustmentPeriod": 60,
                        "LookBackAvailablePeriods": 60
                    },
                    "LastAutoAdjustTime": 1_767_225_600
                }
            }),
            dropped: vec![
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
                vec!["AutoAdjustData", "HistoricalOptions", "LookBackAvailablePeriods"],
            ],
            kept: json!({
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 60}
                }
            }),
        },
        Derived {
            case: "forecast auto-adjust supplying an adjust time, plus a spend",
            budget: json!({
                "BudgetName": "forecast-auto-adjust",
                "TimeUnit": "CUSTOM",
                "BudgetType": "SAVINGS_PLANS_UTILIZATION",
                "AutoAdjustData": {
                    "AutoAdjustType": "FORECAST",
                    "LastAutoAdjustTime": 1_704_067_200
                },
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "1", "Unit": "PERCENTAGE"},
                    "ForecastedSpend": {"Amount": "2", "Unit": "PERCENTAGE"}
                }
            }),
            dropped: vec![vec!["AutoAdjustData", "LastAutoAdjustTime"]],
            kept: json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
        },
    ]
}

/// RULE: a member whose value the service derives from the cost and usage it tracks is
/// not caller-settable — the write path discards whatever the request carried for it and a
/// read reports only what the service itself derived: the spend it computes for the budget
/// ([`computed_spend`]), and nothing for the auto-adjust figures, which this emulator
/// (holding no cost history to average) never produces. The caller-owned members of the
/// same shapes survive.
#[tokio::test]
async fn a_write_discards_the_members_the_service_derives_from_tracked_cost() {
    // One store per row, so rows cannot see each other's budgets: every request names
    // the one account this service serves.
    for row in derived_rows() {
        let router = router();
        let account_id = caller_account();
        let read =
            write_and_read(&router, row.case, "CreateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &read, path);
        }
        assert!(
            carries(&row.kept, &read),
            "case {:?}: expected the read to carry the caller's {}, got {read}",
            row.case,
            row.kept
        );
    }
}

/// RULE: the same holds on the UPDATE write path — `UpdateBudget` documents that "you can
/// change every part of a budget except for the `budgetName` and the `calculatedSpend`",
/// so a budget reads back with the same service-owned members however many times a caller
/// resends values for them.
#[tokio::test]
async fn an_update_discards_them_too() {
    for row in derived_rows() {
        let router = router();
        let account_id = caller_account();
        let created =
            write_and_read(&router, row.case, "CreateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &created, path);
        }
        // The same budget sent again as the update, so the only thing under test is
        // whether the update honours the rule the create does.
        let updated =
            write_and_read(&router, row.case, "UpdateBudget", &account_id, &row.budget).await;
        for path in &row.dropped {
            assert_absent(row.case, &updated, path);
        }
        assert!(
            carries(&row.kept, &updated),
            "case {:?}: expected the update to keep the caller's {}, got {updated}",
            row.case,
            row.kept
        );
    }
}

/// One row of the MODIFYING-update table: a budget created carrying none of the
/// service-derived members, and the `NewBudget` an `UpdateBudget` then sends — which
/// changes caller-owned parts of the budget AND supplies the service-derived members.
struct Modified {
    case: &'static str,
    /// The `Budget` the row is created with.
    created: Value,
    /// The `NewBudget` the update sends: the same `BudgetName`, changed caller-owned
    /// parts, plus the service-derived members a caller cannot set.
    new_budget: Value,
    /// The caller-owned members the update must land, none of which the created budget
    /// already carried.
    changed: Value,
    /// The member paths the read must report nothing for, whatever the request carried.
    dropped: Vec<Vec<&'static str>>,
    /// Caller-owned member paths the update's `NewBudget` omits, which must therefore be
    /// gone from the read: dropping the service-derived members does not turn the update
    /// into a merge of the old budget with the new one.
    absent_after: Vec<Vec<&'static str>>,
}

/// The modifying-update table. Rows differ along WHAT the update changes (the limit's
/// amount, its unit, the granularity, the budget type, the filters, the kind of limit)
/// and along WHICH service-derived member the same request carries, so the rule is
/// asserted over updates that really rewrite the budget rather than resend it.
fn modified_rows() -> Vec<Modified> {
    vec![
        Modified {
            // The request the issue recorded (step 2), with the limit changed too so the
            // update is a real modification and not a resend.
            case: "cost budget raising its limit while supplying an actual spend",
            created: json!({
                "BudgetName": "spend-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }),
            new_budget: json!({
                "BudgetName": "spend-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "250", "Unit": "USD"},
                "CalculatedSpend": {"ActualSpend": {"Amount": "999999", "Unit": "USD"}}
            }),
            changed: json!({"BudgetLimit": {"Amount": "250", "Unit": "USD"}}),
            dropped: vec![],
            absent_after: vec![],
        },
        Modified {
            case: "usage budget changing granularity and limit, supplying both spends",
            created: json!({
                "BudgetName": "usage-demo",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"}
            }),
            new_budget: json!({
                "BudgetName": "usage-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "7", "Unit": "GB"},
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "4", "Unit": "GB"},
                    "ForecastedSpend": {"Amount": "6.5", "Unit": "GB"}
                }
            }),
            changed: json!({
                "TimeUnit": "MONTHLY",
                "BudgetLimit": {"Amount": "7", "Unit": "GB"}
            }),
            dropped: vec![],
            absent_after: vec![],
        },
        Modified {
            // A fixed limit replaced by a planned one: the read derives the `BudgetLimit`
            // in force from `PlannedBudgetLimits`, so the caller's own fixed limit is gone
            // and the derived one is reported instead.
            case: "cost budget swapping its fixed limit for a planned one, with a spend",
            created: json!({
                "BudgetName": "planned-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }),
            new_budget: json!({
                "BudgetName": "planned-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}},
                "CalculatedSpend": {"ActualSpend": {"Amount": "5", "Unit": "USD"}}
            }),
            changed: json!({
                "PlannedBudgetLimits": {"1704067200": {"Amount": "10", "Unit": "USD"}},
                "BudgetLimit": {"Amount": "10", "Unit": "USD"}
            }),
            dropped: vec![],
            absent_after: vec![],
        },
        Modified {
            case: "cost budget becoming auto-adjusting, supplying every derived member",
            created: json!({
                "BudgetName": "auto-adjust-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }),
            new_budget: json!({
                "BudgetName": "auto-adjust-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {
                        "BudgetAdjustmentPeriod": 6,
                        "LookBackAvailablePeriods": 6
                    },
                    "LastAutoAdjustTime": 1_704_067_200
                },
                "CalculatedSpend": {"ActualSpend": {"Amount": "12", "Unit": "USD"}}
            }),
            changed: json!({
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 6}
                }
            }),
            dropped: vec![
                vec!["AutoAdjustData", "LastAutoAdjustTime"],
                vec!["AutoAdjustData", "HistoricalOptions", "LookBackAvailablePeriods"],
            ],
            // The fixed limit the created budget carried is not merged into the
            // auto-adjusting budget the update describes.
            absent_after: vec![vec!["BudgetLimit"]],
        },
        Modified {
            case: "auto-adjusting budget switching from historical to forecast adjustment",
            created: json!({
                "BudgetName": "forecast-demo",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 30}
                }
            }),
            new_budget: json!({
                "BudgetName": "forecast-demo",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "AutoAdjustData": {
                    "AutoAdjustType": "FORECAST",
                    "LastAutoAdjustTime": 1_767_225_600
                }
            }),
            changed: json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
            dropped: vec![vec!["AutoAdjustData", "LastAutoAdjustTime"]],
            // `HistoricalOptions` are "only valid for a HISTORICAL AutoAdjustType", so a
            // forecast-adjusting budget reports none.
            absent_after: vec![vec!["AutoAdjustData", "HistoricalOptions"]],
        },
        Modified {
            case: "coverage budget changing its type and granularity, with a spend in percent",
            created: json!({
                "BudgetName": "coverage-demo",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "RI_COVERAGE"
            }),
            new_budget: json!({
                "BudgetName": "coverage-demo",
                "TimeUnit": "MONTHLY",
                "BudgetType": "SAVINGS_PLANS_COVERAGE",
                "CalculatedSpend": {
                    "ActualSpend": {"Amount": "80", "Unit": "PERCENTAGE"},
                    "ForecastedSpend": {"Amount": "90", "Unit": "PERCENTAGE"}
                }
            }),
            changed: json!({
                "TimeUnit": "MONTHLY",
                "BudgetType": "SAVINGS_PLANS_COVERAGE"
            }),
            dropped: vec![],
            absent_after: vec![],
        },
        Modified {
            case: "cost budget dropping its cost filters and changing its limit, with a spend",
            created: json!({
                "BudgetName": "filters-demo",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "CostFilters": {"Service": ["Amazon Elastic Compute Cloud - Compute"]}
            }),
            new_budget: json!({
                "BudgetName": "filters-demo",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "500", "Unit": "USD"},
                "CalculatedSpend": {"ActualSpend": {"Amount": "0", "Unit": "USD"}}
            }),
            changed: json!({"BudgetLimit": {"Amount": "500", "Unit": "USD"}}),
            dropped: vec![],
            absent_after: vec![vec!["CostFilters"]],
        },
    ]
}

/// Remove `path` from `budget`, so the same `NewBudget` can be resent without the
/// service-derived members it carried.
fn without(budget: &Value, path: &[&str]) -> Value {
    let mut stripped = budget.clone();
    let mut here = &mut stripped;
    let (last, parents) = path.split_last().expect("a member path is never empty");
    for segment in parents {
        match here.get_mut(segment) {
            Some(next) => here = next,
            None => return stripped,
        }
    }
    if let Some(object) = here.as_object_mut() {
        object.remove(*last);
    }
    stripped
}

/// `budget` without the `LastUpdatedTime` every write stamps, so two reads taken at
/// different instants are compared on everything else.
fn without_write_instant(budget: &Value) -> Value {
    without(budget, &["LastUpdatedTime"])
}

/// RULE: a member whose value the service derives from the cost and usage it tracks is
/// not caller-settable on the UPDATE path either — "you can change every part of a budget
/// except for the `budgetName` and the `calculatedSpend`" (`UpdateBudget`), and "you
/// can't set your own `LookBackAvailablePeriods`" — so an update applies every change the
/// caller IS allowed to make while discarding whatever the same request carried for the
/// derived members, and the result is the same budget the update would have stored had it
/// carried none of them.
#[tokio::test]
async fn a_modifying_update_applies_the_change_and_still_discards_the_derived_members() {
    for row in modified_rows() {
        let router = router();
        let account_id = caller_account();
        let created =
            write_and_read(&router, row.case, "CreateBudget", &account_id, &row.created).await;
        // The row's change is a real change: nothing asserted below is already true of
        // the budget before the update runs.
        assert!(
            !carries(&row.changed, &created),
            "case {:?}: the created budget already carries {}, so the update changes \
             nothing: {created}",
            row.case,
            row.changed
        );

        let updated = write_and_read(
            &router,
            row.case,
            "UpdateBudget",
            &account_id,
            &row.new_budget,
        )
        .await;
        for path in &row.dropped {
            assert_absent(row.case, &updated, path);
        }
        for path in &row.absent_after {
            assert_absent(row.case, &updated, path);
        }
        assert!(
            carries(&row.changed, &updated),
            "case {:?}: expected the update to apply {}, got {updated}",
            row.case,
            row.changed
        );

        // The derived members the request carried changed NOTHING: the same update sent
        // again without them reports the same budget, down to every other member.
        let mut clean = row.new_budget.clone();
        for path in &row.dropped {
            clean = without(&clean, path);
        }
        // `CalculatedSpend` is discarded on the write path like the rest of them — the
        // read reports the service-computed spend either way — so the resend drops it too.
        clean = without(&clean, &["CalculatedSpend"]);
        let resent = write_and_read(&router, row.case, "UpdateBudget", &account_id, &clean).await;
        let (expected, actual) = (
            without_write_instant(&updated),
            without_write_instant(&resent),
        );
        assert!(
            same(&expected, &actual),
            "case {:?}: an update carrying the derived members must store what the same \
             update without them stores, expected {expected}, got {actual}",
            row.case
        );
    }
}

/// RULE: "RI and Savings Plans utilization or coverage budgets default to `100`"
/// (`Budget$BudgetLimit`) — a utilization or coverage budget written without a
/// `BudgetLimit` is stored with that limit and reads it back; a cost or usage budget has
/// no such default (its limit is required), and a limit the caller sent is never
/// replaced.
///
/// Spanned over the generated `BUDGET_TYPE_VALUES` × `TIME_UNIT_VALUES` catalogs, so
/// every budget type the model declares is exercised — including the three the issue did
/// not cite.
#[tokio::test]
async fn a_utilization_or_coverage_budget_defaults_to_the_documented_limit() {
    let expected_default = json!({
        "Amount": PERCENTAGE_LIMIT_AMOUNT,
        "Unit": PERCENTAGE_LIMIT_UNIT
    });

    for budget_type in BUDGET_TYPE_VALUES.iter() {
        let defaults = PERCENTAGE_LIMIT_BUDGET_TYPES.contains(budget_type);
        for time_unit in TIME_UNIT_VALUES.iter() {
            let case = format!("{budget_type} / {time_unit} created without a BudgetLimit");
            let router = router();
            let account_id = caller_account();
            let budget = json!({
                "BudgetName": "no-limit",
                "TimeUnit": time_unit,
                "BudgetType": budget_type
            });
            let (status, body) = call(
                &router,
                "CreateBudget",
                &json!({"AccountId": &account_id, "Budget": budget}),
            )
            .await;

            if !defaults {
                // "BudgetLimit is required for cost or usage budgets" — no default is
                // invented for the types the model requires one of.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a budget that requires a limit to be refused, \
                     got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );
            let (status, body) = call(
                &router,
                "DescribeBudget",
                &json!({"AccountId": &account_id, "BudgetName": "no-limit"}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeBudget to answer 200, got {status} {body}"
            );
            let actual = &body["Budget"]["BudgetLimit"];
            assert!(
                carries(&expected_default, actual),
                "case {case:?}: expected the documented default limit {expected_default}, \
                 got {actual}"
            );
        }
    }
}

/// RULE (the other half): a `BudgetLimit` the caller DID send is stored as sent, whatever
/// the budget type — the default fills an absent limit, it does not overwrite one.
#[tokio::test]
async fn a_supplied_budget_limit_is_never_replaced_by_the_default() {
    // Amounts spanning the `NumericValue` pattern `^([0-9]*\.)?[0-9]+$`: an integer, a
    // decimal, and one with no leading digit, each under a different `Spend$Unit`.
    let cost_or_usage_limits = [
        json!({"Amount": "1", "Unit": "USD"}),
        json!({"Amount": "99.5", "Unit": "GB"}),
        json!({"Amount": ".25", "Unit": "PERCENT"}),
    ];
    // A utilization or coverage budget's limit has only one valid AMOUNT ("RI and Savings
    // Plans utilization or coverage budgets default to `100`. This is the only valid
    // value", `Budget$BudgetLimit`), so those rows vary the unit — which that sentence
    // does not constrain and `UnitValue`'s `.*` pattern accepts — rather than pinning
    // acceptance of an amount the documentation calls invalid.
    let percentage_limits = [
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": PERCENTAGE_LIMIT_UNIT}),
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": "PERCENT"}),
    ];
    for budget_type in BUDGET_TYPE_VALUES.iter() {
        let limits: &[Value] = if PERCENTAGE_LIMIT_BUDGET_TYPES.contains(budget_type) {
            &percentage_limits
        } else {
            &cost_or_usage_limits
        };
        for limit in limits.iter() {
            let case = format!("{budget_type} created with the limit {limit}");
            let router = router();
            let account_id = caller_account();
            let read = write_and_read(
                &router,
                &case,
                "CreateBudget",
                &account_id,
                &json!({
                    "BudgetName": "own-limit",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": budget_type,
                    "BudgetLimit": limit
                }),
            )
            .await;
            let actual = &read["BudgetLimit"];
            assert!(
                carries(limit, actual),
                "case {case:?}: expected the caller's limit {limit}, got {actual}"
            );
        }
    }
}

/// The service's own catalog of utilization/coverage budget types must BE the complement
/// of the types `Budget$BudgetLimit` requires a limit of, within the model's generated
/// `BUDGET_TYPE_VALUES` — so the default above cannot be applied to a type the model adds
/// later, and cannot miss one it already declares.
#[test]
fn the_percentage_limit_types_are_the_modelled_types_that_need_no_limit() {
    let mut expected: Vec<&str> = BUDGET_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| !LIMIT_REQUIRED_BUDGET_TYPES.contains(value))
        .collect();
    expected.sort_unstable();
    let mut actual: Vec<&str> = PERCENTAGE_LIMIT_BUDGET_TYPES.to_vec();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "case percentage-limit-catalog: expected the BudgetType members that are not \
         {LIMIT_REQUIRED_BUDGET_TYPES:?}, got {actual:?}"
    );
}
