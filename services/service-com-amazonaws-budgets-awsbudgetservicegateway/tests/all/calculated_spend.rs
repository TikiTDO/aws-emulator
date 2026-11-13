// SPDX-License-Identifier: CC0-1.0

//! What a budget READ reports for `Budget$CalculatedSpend` — "the actual and forecasted
//! cost or usage that the budget tracks", a figure the SERVICE computes and
//! `CalculatedSpend$ActualSpend` makes `@required`, so the single-budget read carries it
//! for every budget that names a unit of measurement.
//!
//! Every test states a RULE and asserts it over a table; the request the issue recorded
//! is one row. The harness (`router`, `call`, `caller_account`, `same`) is the one
//! `tests/all/budget_lifecycle.rs` already builds, and the planned-period keys come from
//! the calendar walk in `tests/all/budget_read_projection.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived catalogs the tables below span, so they cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ATTRIBUTED_SPEND_AMOUNT, PERCENTAGE_LIMIT_BUDGET_TYPES, PERCENTAGE_LIMIT_UNIT,
};

use super::budget_lifecycle::{caller_account, call, router, same};
use super::budget_read_projection::month_start_offset;

/// Create one budget, which must be accepted.
async fn create(router: &MultiServiceRouter, case: &str, account_id: &str, budget: &Value) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// The `Budget` `DescribeBudget` reports for `name`.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
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
    body["Budget"].clone()
}

/// The `Budget` `DescribeBudgets` lists for `name` — the other read path, which reports
/// the same stored budget without the computed spend.
async fn describe_in_list(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
    let (status, body) = call(router, "DescribeBudgets", &json!({"AccountId": account_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgets to answer 200, got {status} {body}"
    );
    body["Budgets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|listed| listed["BudgetName"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: {name} is not listed: {body}"))
}

/// The `CalculatedSpend` a read must report for a budget measured in `unit`: the spend
/// this emulator has attributed to the budget, and no forecast — there is no usage history
/// to predict one from, and `CalculatedSpend$ForecastedSpend` is optional.
fn expected_spend(unit: &str) -> Value {
    json!({"ActualSpend": {"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": unit}})
}

/// Assert the budget `read` reports the spend of a budget measured in `unit`, or no
/// `CalculatedSpend` at all when the budget names no unit (`unit` is `None`).
fn assert_spend(case: &str, read: &Value, unit: Option<&str>) {
    let actual = &read["CalculatedSpend"];
    match unit {
        Some(unit) => {
            let expected = expected_spend(unit);
            assert!(
                same(&expected, actual),
                "case {case:?}: expected the computed spend {expected}, got {actual} in {read}"
            );
            // The spend is measured in the same unit as the limit the read reports:
            // `Spend$Unit` is one unit of measurement for the budget's limit, its actual
            // spend and its forecast alike.
            let limit_unit = &read["BudgetLimit"]["Unit"];
            assert_eq!(
                limit_unit,
                &json!(unit),
                "case {case:?}: expected the read to report its limit in {unit}, got \
                 {limit_unit} in {read}"
            );
        }
        None => assert!(
            actual.is_null(),
            "case {case:?}: a budget that reports no limit names no unit of measurement, \
             so it must report no CalculatedSpend, got {actual} in {read}"
        ),
    }
}

/// One row of the spend table: the `Budget` to create, and the `Spend$Unit` its spend must
/// be measured in — `None` for a budget whose read reports no limit at all.
struct Row {
    case: &'static str,
    budget: Value,
    unit: Option<&'static str>,
}

/// The table. Rows differ along what the rule generalises over: WHERE the unit the budget
/// tracks comes from (a fixed `BudgetLimit`, the planned entry in force, the documented
/// utilization/coverage default, or nowhere), WHICH unit it is (a currency, a usage unit, a
/// percentage), the budget type and the granularity.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The budget the issue recorded (step 1).
            case: "cost budget with a fixed limit in USD",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }),
            unit: Some("USD"),
        },
        Row {
            case: "cost budget whose limit is in another currency",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "12.75", "Unit": "EUR"}
            }),
            unit: Some("EUR"),
        },
        Row {
            // "A `Spend` for `3 GB` of S3 usage has ... a `Unit` of `GB`" (`Spend`).
            case: "usage budget measured in GB, decimal limit",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "3.5", "Unit": "GB"}
            }),
            unit: Some("GB"),
        },
        Row {
            case: "usage budget measured in hours, limit under one",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": ".25", "Unit": "Hrs"}
            }),
            unit: Some("Hrs"),
        },
        Row {
            // "RI and Savings Plans utilization or coverage budgets default to `100`"
            // (`Budget$BudgetLimit`), so the unit the read reports is the default's.
            case: "RI utilization budget taking the documented default limit",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "MONTHLY",
                "BudgetType": "RI_UTILIZATION"
            }),
            unit: Some(PERCENTAGE_LIMIT_UNIT),
        },
        Row {
            case: "savings plans coverage budget with its own limit unit",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "CUSTOM",
                "BudgetType": "SAVINGS_PLANS_COVERAGE",
                "BudgetLimit": {"Amount": "100", "Unit": "PERCENT"}
            }),
            unit: Some("PERCENT"),
        },
        Row {
            // The read derives the `BudgetLimit` from the planned entry in force, so the
            // spend is measured in THAT entry's unit, not in another period's.
            case: "planned-limit budget whose entry in force is the latest started one",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(-2).to_string(): {"Amount": "10", "Unit": "EUR"},
                    month_start_offset(-1).to_string(): {"Amount": "20", "Unit": "USD"}
                }
            }),
            unit: Some("USD"),
        },
        Row {
            case: "planned-limit budget whose periods all start in the future",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "MONTHLY",
                "BudgetType": "USAGE",
                "PlannedBudgetLimits": {
                    month_start_offset(1).to_string(): {"Amount": "5", "Unit": "GB"},
                    month_start_offset(2).to_string(): {"Amount": "6", "Unit": "TB"}
                }
            }),
            unit: Some("GB"),
        },
        Row {
            // An auto-adjusting budget's amount is the average of cost data this emulator
            // does not hold, so its read reports no limit — and nothing in that record
            // names a unit for a spend.
            case: "auto-adjusting cost budget that reports no limit",
            budget: json!({
                "BudgetName": "spend-read",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 3}
                }
            }),
            unit: None,
        },
    ]
}

/// RULE: a read of a budget reports the `CalculatedSpend` the SERVICE computes for it —
/// the cost or usage attributed to that budget, measured in the unit the budget tracks
/// (the unit of the limit the same read reports) — and reading it again reports the same
/// figure. The account-wide listing reports the same budget without that member, the rule
/// `tests/all/recorded_response_members.rs` states.
#[tokio::test]
async fn a_read_reports_the_computed_spend_in_the_unit_the_budget_tracks() {
    // One store per row, so rows cannot see each other's budgets: every request names
    // the one account this service serves.
    for row in rows() {
        let router = router();
        let account_id = caller_account();
        let name = row.budget["BudgetName"]
            .as_str()
            .expect("every row names its budget")
            .to_string();
        create(&router, row.case, &account_id, &row.budget).await;

        let read = describe(&router, row.case, &account_id, &name).await;
        assert_spend(row.case, &read, row.unit);

        let listed = describe_in_list(&router, row.case, &account_id, &name).await;
        // The listing leaves the computed spend out, which
        // `tests/all/recorded_response_members.rs` states the rule for; what it reports
        // for the budget is otherwise the same read.
        assert!(
            listed.get("CalculatedSpend").is_none(),
            "case {:?}: expected the listing to leave the computed spend out, got {listed}",
            row.case
        );

        // A read reports a figure, it does not establish one: the next read answers the
        // same.
        let again = describe(&router, row.case, &account_id, &name).await;
        let (first, second) = (&read["CalculatedSpend"], &again["CalculatedSpend"]);
        assert!(
            same(first, second),
            "case {:?}: expected a second read to report the same spend {first}, got \
             {second}",
            row.case
        );
    }
}

/// RULE: the same holds for every budget type and granularity the model declares — the
/// spend is reported for whatever a budget tracks, in the unit of that budget's limit.
///
/// Spanned over the generated `BUDGET_TYPE_VALUES` × `TIME_UNIT_VALUES` catalogs, so the
/// budget types and granularities the issue did not cite are exercised too. A cost or
/// usage budget needs a limit of its own ("`BudgetLimit` is required for cost or usage
/// budgets"); a utilization or coverage budget is created without one and takes the
/// documented default.
#[tokio::test]
async fn every_modelled_budget_type_and_granularity_reports_the_spend() {
    for budget_type in BUDGET_TYPE_VALUES.iter() {
        // A unit per budget type, so the assertion cannot pass on a single unit the
        // service might report for everything.
        let (limit, unit) = if PERCENTAGE_LIMIT_BUDGET_TYPES.contains(budget_type) {
            (Value::Null, PERCENTAGE_LIMIT_UNIT)
        } else if *budget_type == "USAGE" {
            (json!({"Amount": "7", "Unit": "GB"}), "GB")
        } else {
            (json!({"Amount": "100", "Unit": "USD"}), "USD")
        };
        for time_unit in TIME_UNIT_VALUES.iter() {
            let case = format!("{budget_type} / {time_unit}");
            let router = router();
            let account_id = caller_account();
            let mut budget = json!({
                "BudgetName": "catalog-spend",
                "TimeUnit": time_unit,
                "BudgetType": budget_type
            });
            if !limit.is_null() {
                budget["BudgetLimit"] = limit.clone();
            }
            create(&router, &case, &account_id, &budget).await;
            let read = describe(&router, &case, &account_id, "catalog-spend").await;
            assert_spend(&case, &read, Some(unit));
        }
    }
}

/// RULE: the spend follows the budget — an update that changes the unit the budget tracks
/// changes the unit its spend is reported in, and one that leaves the budget with no limit
/// leaves it with no spend to report. The budget is read back through `DescribeBudget`
/// after each write.
#[tokio::test]
async fn the_spend_follows_the_unit_the_budget_tracks_across_updates() {
    let router = router();
    let account_id = caller_account();
    let name = "updated-spend";
    let created = json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    });
    create(&router, "created in USD", &account_id, &created).await;
    let read = describe(&router, "created in USD", &account_id, name).await;
    assert_spend("created in USD", &read, Some("USD"));

    // Each step is the `NewBudget` an `UpdateBudget` sends and the unit the spend must
    // then be reported in: another currency, a usage unit, the planned entry in force, and
    // an auto-adjusting budget that reports no limit at all.
    let steps: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "updated to another currency",
            json!({
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "GBP"}
            }),
            Some("GBP"),
        ),
        (
            "updated to a usage budget measured in GB",
            json!({
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "7", "Unit": "GB"}
            }),
            Some("GB"),
        ),
        (
            "updated to a planned-limit budget",
            json!({
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(-1).to_string(): {"Amount": "20", "Unit": "JPY"}
                }
            }),
            Some("JPY"),
        ),
        (
            "updated to an auto-adjusting budget",
            json!({
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "AutoAdjustData": {"AutoAdjustType": "FORECAST"}
            }),
            None,
        ),
    ];

    for (case, new_budget, unit) in steps {
        let (status, body) = call(
            &router,
            "UpdateBudget",
            &json!({"AccountId": &account_id, "NewBudget": new_budget}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        let read = describe(&router, case, &account_id, name).await;
        assert_spend(case, &read, unit);
    }
}
