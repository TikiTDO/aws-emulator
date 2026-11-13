// SPDX-License-Identifier: CC0-1.0

//! The `ShowFilterExpression` projection, driven through BOTH reads that model the flag
//! (`DescribeBudget` and `DescribeBudgets`) so the one shared projection
//! (`crate::state::with_shown_filter_expression`) is what answers either call.
//!
//! The suite states a RULE and asserts it over a table of filter-expression forms; the
//! harness (`router`, `call`, `caller_account`, `same`) is the one
//! `tests/all/budget_lifecycle.rs` already builds.

use serde_json::{json, Value};

use super::budget_lifecycle::{call, caller_account, router, same};
use super::recorded_response_members::without;

/// One row of the table: a budget name and the `FilterExpression` it is created with
/// (`None` for a budget that carries none at all).
struct Row {
    case: &'static str,
    budget_name: &'static str,
    filter_expression: Option<Value>,
}

/// The forms an `Expression` takes, one row each: the three leaf filters
/// (`Dimensions`, `Tags`, `CostCategories`), the two list combinators (`And`, `Or`), the
/// negation (`Not`, nested over a combinator) and a budget with no expression at all.
/// The member list and the `Dimension` / `MatchOption` entries below are the model's
/// (`smithy_query(query="shape", shape="Expression,Dimension,MatchOption")`), and none of
/// them is the `REGION`/`And` pair the issue cited.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "dimensions-leaf",
            budget_name: "by-service",
            filter_expression: Some(json!({
                "Dimensions": {
                    "Key": "SERVICE",
                    "Values": ["Amazon Elastic Compute Cloud - Compute"],
                    "MatchOptions": ["EQUALS"]
                }
            })),
        },
        Row {
            case: "tags-leaf",
            budget_name: "by-tag",
            filter_expression: Some(json!({
                "Tags": {"Key": "cost-center", "Values": ["4711"], "MatchOptions": ["CONTAINS"]}
            })),
        },
        Row {
            case: "cost-categories-leaf",
            budget_name: "by-cost-category",
            filter_expression: Some(json!({
                "CostCategories": {"Key": "environment", "Values": ["prod", "staging"]}
            })),
        },
        Row {
            case: "and-of-three",
            budget_name: "by-and",
            filter_expression: Some(json!({
                "And": [
                    {"Dimensions": {"Key": "LINKED_ACCOUNT", "Values": ["000000000001"]}},
                    {"Dimensions": {"Key": "USAGE_TYPE", "Values": ["BoxUsage:t3.micro"]}},
                    {"Tags": {"Key": "team", "Values": ["platform"]}}
                ]
            })),
        },
        Row {
            case: "or-of-two",
            budget_name: "by-or",
            filter_expression: Some(json!({
                "Or": [
                    {"Dimensions": {"Key": "RECORD_TYPE", "Values": ["Usage"]}},
                    {"CostCategories": {"Key": "team", "Values": ["data"]}}
                ]
            })),
        },
        Row {
            case: "not-over-or",
            budget_name: "by-not",
            filter_expression: Some(json!({
                "Not": {
                    "Or": [
                        {"Dimensions": {
                            "Key": "PURCHASE_TYPE",
                            "Values": ["Spot"],
                            "MatchOptions": ["CASE_INSENSITIVE"]
                        }},
                        {"Tags": {"Key": "scratch", "Values": ["yes"], "MatchOptions": ["ABSENT"]}}
                    ]
                }
            })),
        },
        Row {
            case: "no-expression",
            budget_name: "unfiltered",
            filter_expression: None,
        },
    ]
}

/// The `Budget` payload a row is created with. A budget that names a `FilterExpression`
/// also names `Metrics`, the syntax that member belongs to.
fn budget_payload(row: &Row) -> Value {
    let mut budget = json!({
        "BudgetName": row.budget_name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "17.5", "Unit": "USD"}
    });
    if let Some(expression) = &row.filter_expression {
        budget["FilterExpression"] = expression.clone();
        budget["Metrics"] = json!(["NetUnblendedCost"]);
    }
    budget
}

/// The budget a read reports for `budget_name`, through `operation`, with
/// `ShowFilterExpression` set to `show` (left out of the request when `None`).
async fn read_budget(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    budget_name: &str,
    show: Option<bool>,
    case: &str,
) -> Value {
    let account_id = caller_account();
    let mut request = json!({"AccountId": account_id});
    if operation == "DescribeBudget" {
        request["BudgetName"] = json!(budget_name);
    }
    if let Some(flag) = show {
        request["ShowFilterExpression"] = json!(flag);
    }
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?} {operation} show={show:?}: expected 200, got {status} {body}"
    );
    if operation == "DescribeBudget" {
        return body["Budget"].clone();
    }
    let listing = body["Budgets"].as_array().cloned().unwrap_or_default();
    listing
        .iter()
        .find(|budget| budget["BudgetName"] == json!(budget_name))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} {operation} show={show:?}: expected the listing to carry \
                 {budget_name:?}, got {body}"
            )
        })
}

/// RULE: a budget's `FilterExpression` is reported only when the caller asks for it with
/// `ShowFilterExpression`; the flag governs that member alone, and governs it the same
/// way in every read that models it — whatever form the stored expression takes.
#[tokio::test]
async fn the_filter_expression_flag_governs_one_member_in_every_read() {
    let router = router();
    let account_id = caller_account();
    let rows = rows();

    for row in &rows {
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({"AccountId": &account_id, "Budget": budget_payload(row)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateBudget to answer 200, got {status} {body}",
            row.case
        );
    }

    for row in &rows {
        for operation in ["DescribeBudget", "DescribeBudgets"] {
            let shown =
                read_budget(&router, operation, row.budget_name, Some(true), row.case).await;

            // Asked for: the stored expression, exactly as it was written.
            match &row.filter_expression {
                Some(expression) => {
                    let seen = shown.get("FilterExpression").cloned().unwrap_or(Value::Null);
                    assert!(
                        same(expression, &seen),
                        "case {:?} {operation} show=true: expected the stored FilterExpression \
                         {expression}, got {seen}",
                        row.case
                    );
                }
                None => assert!(
                    shown.get("FilterExpression").is_none(),
                    "case {:?} {operation} show=true: a budget that carries no FilterExpression \
                     must report none, got {shown}",
                    row.case
                ),
            }

            for show in [None, Some(false)] {
                let withheld =
                    read_budget(&router, operation, row.budget_name, show, row.case).await;
                assert!(
                    withheld.get("FilterExpression").is_none(),
                    "case {:?} {operation} show={show:?}: expected no FilterExpression, \
                     got {withheld}",
                    row.case
                );

                // The flag governs that member and nothing else: the two responses are the
                // same budget once the member it governs is taken out of both.
                let mut expected = shown.clone();
                if let Some(object) = expected.as_object_mut() {
                    object.remove("FilterExpression");
                }
                assert!(
                    same(&expected, &withheld),
                    "case {:?} {operation} show={show:?}: expected the budget reported with \
                     show=true minus FilterExpression ({expected}), got {withheld}",
                    row.case
                );
            }
        }
    }
}

/// RULE: both reads that model `ShowFilterExpression` project one stored budget the same
/// way, so for any setting of the flag the single read and the listing report the same
/// budget — apart from the computed spend the listing leaves out
/// (`tests/all/recorded_response_members.rs`).
#[tokio::test]
async fn both_reads_report_the_same_projection_for_every_flag_setting() {
    let router = router();
    let account_id = caller_account();
    let rows = rows();

    for row in &rows {
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({"AccountId": &account_id, "Budget": budget_payload(row)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateBudget to answer 200, got {status} {body}",
            row.case
        );
    }

    for row in &rows {
        for show in [None, Some(false), Some(true)] {
            let single =
                read_budget(&router, "DescribeBudget", row.budget_name, show, row.case).await;
            let listed =
                read_budget(&router, "DescribeBudgets", row.budget_name, show, row.case).await;
            assert!(
                same(&without(&single, &["CalculatedSpend"]), &listed),
                "case {:?} show={show:?}: expected DescribeBudgets to report the budget \
                 DescribeBudget does ({single}), got {listed}",
                row.case
            );
        }
    }
}
