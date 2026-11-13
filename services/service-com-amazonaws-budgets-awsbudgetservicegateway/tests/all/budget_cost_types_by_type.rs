// SPDX-License-Identifier: CC0-1.0

//! Which budgets hold a cost-types selection at all, driven end to end over the wire
//! through the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE these tables assert: `CostTypes` is a member of a `COST` budget only — "the
//! types of costs that are included in this `COST` budget ... `USAGE`, `RI_UTILIZATION`,
//! `RI_COVERAGE`, `SAVINGS_PLANS_UTILIZATION`, and `SAVINGS_PLANS_COVERAGE` budgets do
//! not have `CostTypes`" (`Budget$CostTypes`). So a budget whose type has cost types
//! reads back with every member of them, each at the value the caller sent or the default
//! the model documents for it, and a budget whose type has none reads back with NO
//! `CostTypes` whatever the write carried for it.
//!
//! Both write paths are covered (`CreateBudget` and the `UpdateBudget` that replaces the
//! same budget), and every read path that reports a budget's cost types: `DescribeBudget`,
//! `DescribeBudgets` and the `BudgetPerformanceHistory` header. The table's rows differ
//! along the budget type (every member of the model's own catalog) and along the map the
//! write carries — absent, empty, a partial selection, and every member set — which is
//! what the rule generalises over.
//!
//! The harness (`router`, `call`, `caller_account`, `same`) is the one
//! `tests/all/budget_lifecycle.rs` builds, and the member list is the one
//! `tests/all/budget_read_projection.rs` writes from the model's documentation.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived catalog the table spans, so it cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::BUDGET_TYPE_VALUES;

use super::budget_lifecycle::{call, caller_account, limit_amount_for, router, same};
use super::budget_read_projection::COST_TYPE_DEFAULTS;
use super::recorded_response_members::without;

/// The one `BudgetType` whose budget HAS cost types.
///
/// Deliberately spelled out here from `Budget$CostTypes` ("the types of costs that are
/// included in this `COST` budget") rather than read out of the service's own private
/// constant: an expectation taken from the code under test cannot fail with it.
const COST_TYPES_BUDGET_TYPE: &str = "COST";

/// The budget every row writes, under `name` and of `budget_type`. `MONTHLY`, so the same
/// budget is also readable through `DescribeBudgetPerformanceHistory` (a history is
/// retained for daily, monthly and quarterly budgets).
fn budget(name: &str, budget_type: &str) -> Value {
    json!({
        "BudgetName": name,
        // The amount a budget of this type may carry ([`limit_amount_for`]): this table
        // is about which budgets hold cost types, not about the limit.
        "BudgetLimit": {"Amount": limit_amount_for(budget_type, "5"), "Unit": "USD"},
        "TimeUnit": "MONTHLY",
        "BudgetType": budget_type
    })
}

/// The `CostTypes` maps the table writes, differing along the SIZE and the VALUES of the
/// selection: none at all, an empty selection, a partial one that sets one `Include*`
/// member and one rate member, and one that sets every member to the opposite of its
/// documented default.
fn sent_variants() -> Vec<(&'static str, Option<Value>)> {
    let inverted: Map<String, Value> = COST_TYPE_DEFAULTS
        .iter()
        .map(|(member, default)| ((*member).to_string(), json!(!*default)))
        .collect();
    vec![
        ("no CostTypes", None),
        ("an empty CostTypes", Some(json!({}))),
        (
            "a partial CostTypes",
            Some(json!({"IncludeTax": false, "UseAmortized": true})),
        ),
        ("every member inverted", Some(Value::Object(inverted))),
    ]
}

/// Drive one write operation (`CreateBudget` with `Budget`, `UpdateBudget` with
/// `NewBudget`) and fail the test if the service refuses it: the rule is about what a
/// budget HOLDS, so every row has to be written before it can be read.
async fn write(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    account_id: &str,
    budget: &Value,
) {
    let request = match operation {
        "CreateBudget" => json!({"AccountId": account_id, "Budget": budget.clone()}),
        "UpdateBudget" => json!({"AccountId": account_id, "NewBudget": budget.clone()}),
        other => panic!("case {case:?}: this suite drives no write operation {other}"),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body}"
    );
}

/// The budget both read paths report, having checked they report the SAME budget: the
/// selection a budget holds is observable through `DescribeBudget` and `DescribeBudgets`
/// alike.
async fn read_back(
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
        .unwrap_or_else(|| panic!("case {case:?}: {name} is not listed: {body}"));
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

/// The cost types the performance-history header reports for `name`
/// (`BudgetPerformanceHistory$CostTypes` is the budget's own selection), or `Value::Null`
/// when it reports none.
async fn history_cost_types(
    router: &MultiServiceRouter,
    case: &str,
    account_id: &str,
    name: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribeBudgetPerformanceHistory",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgetPerformanceHistory to answer 200, \
         got {status} {body}"
    );
    body["BudgetPerformanceHistory"]
        .get("CostTypes")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert `reported` is the selection a budget of `budget_type` HAS, given the map the
/// last write carried: every member at the value sent or its documented default for the
/// type that has cost types, and nothing at all for any other type.
fn assert_cost_types(case: &str, budget_type: &str, sent: Option<&Value>, reported: &Value) {
    if budget_type != COST_TYPES_BUDGET_TYPE {
        assert!(
            reported.is_null(),
            "case {case:?}: a {budget_type} budget has no CostTypes, got {reported}"
        );
        return;
    }
    for (member, default) in COST_TYPE_DEFAULTS {
        let expected = sent
            .and_then(|map| map.get(member).and_then(Value::as_bool))
            .unwrap_or(*default);
        let actual = reported.get(member).and_then(Value::as_bool);
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: expected {member} = {expected}, got {actual:?} in {reported}"
        );
    }
}

/// RULE: a budget holds a `CostTypes` selection only when its type has one — a `COST`
/// budget reads back with every member of it, at the value sent or its documented
/// default, and a budget of any other type reads back with none whatever the write
/// carried.
#[tokio::test]
async fn a_budget_holds_cost_types_only_when_its_type_has_them() {
    let variants = sent_variants();
    for budget_type in BUDGET_TYPE_VALUES.iter() {
        for (index, (variant, sent)) in variants.iter().enumerate() {
            // The next variant in the list, so the update leg carries a DIFFERENT map
            // from the create leg — including the pair that drops a stored selection and
            // the pair that supplies one the create did not.
            let (next_variant, updated) = &variants[(index + 1) % variants.len()];
            let router = router();
            let account_id = caller_account();
            let name = "cost-types-by-type";

            for (operation, variant, sent) in [
                ("CreateBudget", variant, sent),
                ("UpdateBudget", next_variant, updated),
            ] {
                let case = format!("{budget_type}, {variant}, via {operation}");
                let mut written = budget(name, budget_type);
                if let Some(map) = sent {
                    written["CostTypes"] = map.clone();
                }
                write(&router, &case, operation, &account_id, &written).await;

                let read = read_back(&router, &case, &account_id, name).await;
                let reported = read.get("CostTypes").cloned().unwrap_or(Value::Null);
                assert_cost_types(&case, budget_type, sent.as_ref(), &reported);

                // The history reports the budget's own selection, so it reports exactly
                // what the budget holds.
                let from_history = history_cost_types(&router, &case, &account_id, name).await;
                assert_cost_types(&case, budget_type, sent.as_ref(), &from_history);
                assert!(
                    same(&reported, &from_history),
                    "case {case:?}: every read path reports the same selection, got \
                     {reported} from DescribeBudget and {from_history} from the history"
                );
            }
        }
    }
}

/// RULE: the other half of the same rule — a `COST` budget written in the
/// `FilterExpression`/`Metrics` syntax holds no cost types either, since "only one set of
/// filter and metric selections can be present in the syntax at one time. Either
/// `FilterExpression` and `Metrics` or `CostFilters` and `CostTypes`, not both or a
/// different combination" (`CreateBudget`).
#[tokio::test]
async fn a_cost_budget_in_the_expression_syntax_holds_no_cost_types() {
    let router = router();
    let account_id = caller_account();
    // Rows differing along which half of the expression syntax the budget carries.
    for (case, scoping) in [
        (
            "FilterExpression and Metrics",
            json!({
                "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}},
                "Metrics": ["UnblendedCost"]
            }),
        ),
        ("Metrics alone", json!({"Metrics": ["AmortizedCost"]})),
        (
            "FilterExpression alone",
            json!({"FilterExpression": {"Not": {"Tags": {"Key": "team", "Values": ["net"]}}}}),
        ),
    ] {
        let name = format!("expression-{case}");
        let mut written = budget(&name, COST_TYPES_BUDGET_TYPE);
        for (member, value) in scoping.as_object().expect("an object of members") {
            written[member] = value.clone();
        }
        write(&router, case, "CreateBudget", &account_id, &written).await;

        let read = read_back(&router, case, &account_id, &name).await;
        let reported = read.get("CostTypes").cloned().unwrap_or(Value::Null);
        assert!(
            reported.is_null(),
            "case {case:?}: a budget in the expression syntax has no CostTypes, got {reported}"
        );
    }
}
