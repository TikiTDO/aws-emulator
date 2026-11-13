// SPDX-License-Identifier: CC0-1.0

//! An EMPTY collection member of a `Budget` selects nothing.
//!
//! The RULE this suite asserts: a map or list member of a `Budget` that a caller sends
//! with no entries selects nothing, so every rule that asks "did the caller supply this
//! limit, this filter syntax or this metric?" answers it the same way as for a member
//! that was not sent at all — and the budget that is stored and read back is the same
//! one either way.
//!
//! The members the rule is stated over are the collection members of `Budget`
//! (`smithy_query(query="shape", shape="Budget")`): the `PlannedBudgetLimits` and
//! `CostFilters` maps, and the `Metrics` list the model bounds at
//! `@length(min: 0, max: 1)`, so an empty list is a value it admits. A member that DOES
//! carry an entry is a selection, which the second table pins is still judged by every
//! rule stated on it.
//!
//! Each table row drives both the `CreateBudget` write path and a read back through
//! `DescribeBudget` / `DescribeBudgets`, over the same `MultiServiceRouter` the
//! production router serves from; the harness (`router`, `call`, `caller_account`,
//! `is_error`, `same`, `civil_secs`) is the one `tests/all/budget_lifecycle.rs` builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The model-derived `Metric` catalog, so the rows that send a metric span every value
/// the model names rather than the one the issue cited.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::METRIC_VALUES;

use super::budget_lifecycle::{caller_account, call, civil_secs, is_error, router, same};
use super::budget_read_projection::COST_TYPE_DEFAULTS;
use super::recorded_response_members::without;

/// The refusal both write paths answer a broken cross-member rule with.
const REFUSAL: &str = "InvalidParameterException";

/// A `Spend` of `amount` US dollars.
fn spend(amount: &str) -> Value {
    json!({"Amount": amount, "Unit": "USD"})
}

/// The collection members of a `Budget`, each with the EMPTY value a caller may send it
/// as. Taken from `smithy_query(query="shape", shape="Budget")`: `PlannedBudgetLimits`
/// and `CostFilters` are maps, `Metrics` is a list.
fn empty_collection_members() -> Vec<(&'static str, Value)> {
    vec![
        ("PlannedBudgetLimits", json!({})),
        ("CostFilters", json!({})),
        ("Metrics", json!([])),
    ]
}

/// The budgets the parity table starts from — valid budgets differing along the budget
/// type, the granularity, the member their limit comes from and the filter syntax they
/// are written in, which is what the rule generalises over.
fn base_budgets() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a MONTHLY COST budget with a fixed limit",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10")
            }),
        ),
        (
            "a MONTHLY COST budget carrying CostTypes",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "CostTypes": {"IncludeTax": false}
            }),
        ),
        (
            "a MONTHLY COST budget in the FilterExpression syntax",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}},
                "Metrics": ["UnblendedCost"]
            }),
        ),
        (
            "a QUARTERLY COST budget whose limit comes from a planned map",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "QUARTERLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    civil_secs("2024-04-01T00:00:00Z").to_string(): spend("7")
                }
            }),
        ),
        (
            "a DAILY USAGE budget with a fixed limit",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "DAILY",
                "BudgetType": "USAGE",
                "BudgetLimit": spend("3")
            }),
        ),
        (
            // A type that has no cost types and that a planned map is not available
            // for, and that needs no limit at all.
            "an ANNUALLY RI_UTILIZATION budget with no limit at all",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "RI_UTILIZATION"
            }),
        ),
        (
            // A granularity a planned map is not available for either.
            "a CUSTOM COST budget with a fixed limit and its own period",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "BudgetLimit": spend("4"),
                "TimePeriod": {
                    "Start": civil_secs("2024-01-01T00:00:00Z"),
                    "End": civil_secs("2024-12-31T00:00:00Z")
                }
            }),
        ),
        (
            "a MONTHLY COST budget scoped by a non-empty CostFilters map",
            json!({
                "BudgetName": "empty-selection",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "CostFilters": {"Service": ["Amazon Elastic Compute Cloud - Compute"]}
            }),
        ),
    ]
}

/// `budget` with `member` set to `value`.
fn with_member(budget: &Value, member: &str, value: &Value) -> Value {
    let mut written = budget.clone();
    written[member] = value.clone();
    written
}

/// Create `budget` in a store of its own and report `(status, the budget as it reads
/// back)`. The read is `Value::Null` when nothing was stored.
async fn create_and_read(budget: &Value) -> (u16, Value) {
    let store = router();
    let account_id = caller_account();
    let name = budget["BudgetName"]
        .as_str()
        .expect("every row names its budget")
        .to_string();
    let (status, body) = call(
        &store,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": budget}),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    let read = read_back(&store, &account_id, &name).await;
    (status, read)
}

/// The budget both read paths report for `name`, having checked they agree: the single
/// read and the list read report the same stored budget.
async fn read_back(store: &MultiServiceRouter, account_id: &str, name: &str) -> Value {
    let (_, single) = call(
        store,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    let read = single["Budget"].clone();
    let (_, listed) = call(store, "DescribeBudgets", &json!({"AccountId": account_id})).await;
    let from_list = listed["Budgets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|budget| budget["BudgetName"] == json!(name))
        .unwrap_or(Value::Null);
    assert!(
        same(&without(&read, &["CalculatedSpend"]), &from_list),
        "expected both read paths to report the same budget for {name:?} apart from the \
         tracked spend, got {read} from DescribeBudget and {from_list} from DescribeBudgets"
    );
    read
}

/// `read` without the members two separate writes cannot agree on: the instants the
/// service stamps on a write — the `CreatedAt` of the budget, the `LastUpdatedTime` of
/// the budget and the one its `HealthStatus` carries — and the empty collection member
/// under test, which the budget that never carried it cannot report.
fn comparable(read: &Value, member: &str) -> Value {
    let mut stripped = read.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("CreatedAt");
        object.remove("LastUpdatedTime");
        object.remove(member);
        if let Some(health) = object.get_mut("HealthStatus").and_then(Value::as_object_mut) {
            health.remove("LastUpdatedTime");
        }
    }
    stripped
}

/// RULE: a collection member sent with no entries selects nothing, so a budget that
/// carries it empty is accepted, stored and read back exactly like the same budget
/// written without that member at all.
#[tokio::test]
async fn an_empty_collection_member_is_judged_like_an_absent_one() {
    for (base_case, base) in base_budgets() {
        let (base_status, base_read) = create_and_read(&base).await;
        assert_eq!(
            base_status, 200,
            "case {base_case:?}: the base budget must be accepted, got {base_status} \
             {base_read}"
        );
        for (member, empty) in empty_collection_members() {
            // A base that already selects entries in this member says nothing about the
            // empty value of it.
            if base.get(member).is_some() {
                continue;
            }
            let case = format!("{base_case} beside an empty {member}");
            let (status, read) = create_and_read(&with_member(&base, member, &empty)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {read}"
            );
            let (with_empty, with_none) =
                (comparable(&read, member), comparable(&base_read, member));
            assert!(
                same(&with_empty, &with_none),
                "case {case:?}: expected the stored budget to be {with_none}, got {with_empty}"
            );
        }
    }
}

/// RULE, read off the other side: a `COST` budget "has" cost types unless it is written
/// in the `FilterExpression`/`Metrics` syntax — "The types of costs that are included in
/// this `COST` budget" (`Budget$CostTypes`) — and an empty `Metrics` list writes it in no
/// syntax at all, so such a budget still reads back with every `CostTypes` member at its
/// documented default.
#[tokio::test]
async fn a_cost_budget_whose_metrics_list_is_empty_still_has_cost_types() {
    // Rows differ along what the empty list is sent beside: nothing, the other empty
    // map, and a CostTypes selection of the caller's own.
    let rows: Vec<(&str, Value, Option<bool>)> = vec![
        ("an empty Metrics list alone", json!({}), None),
        (
            "an empty Metrics list beside an empty CostFilters map",
            json!({"CostFilters": {}}),
            None,
        ),
        (
            "an empty Metrics list beside a CostTypes selection",
            json!({"CostTypes": {"IncludeTax": false, "UseAmortized": true}}),
            Some(false),
        ),
    ];
    for (case, extra, include_tax) in rows {
        let mut budget = json!({
            "BudgetName": "empty-metrics",
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": spend("10"),
            "Metrics": []
        });
        for (member, value) in extra.as_object().expect("a member map") {
            budget[member] = value.clone();
        }
        let (status, read) = create_and_read(&budget).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {read}"
        );
        let reported = read.get("CostTypes").cloned().unwrap_or(Value::Null);
        for (member, default) in COST_TYPE_DEFAULTS {
            // The one member a row may have set is expected at the value it sent; every
            // other member at the default the model documents for it.
            let expected = match (*member, include_tax) {
                ("IncludeTax", Some(sent)) => sent,
                ("UseAmortized", Some(_)) => true,
                _ => *default,
            };
            let actual = reported.get(member).and_then(Value::as_bool);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected CostTypes {member} = {expected}, got {actual:?} in \
                 {reported}"
            );
        }
    }
}

/// RULE: a collection member that DOES carry an entry is a selection, so every rule
/// stated on it still judges it — the mutual exclusion of `BudgetLimit` and
/// `PlannedBudgetLimits`, the mutual exclusion of the two filter syntaxes, the budget
/// types and granularities a planned map is available for, and the cost types a budget
/// written in the expression syntax does not have.
#[tokio::test]
async fn a_collection_member_carrying_an_entry_is_a_selection_the_rules_judge() {
    let month_start = civil_secs("2024-02-01T00:00:00Z");
    let refused: Vec<(String, Value)> = vec![
        (
            "a fixed BudgetLimit beside a planned map that plans a period".to_string(),
            json!({
                "BudgetName": "selected",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "PlannedBudgetLimits": {month_start.to_string(): spend("10")}
            }),
        ),
        (
            "a planned map that plans a period on a granularity it is not available for"
                .to_string(),
            json!({
                "BudgetName": "selected",
                "TimeUnit": "ANNUALLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {month_start.to_string(): spend("10")}
            }),
        ),
        (
            "a CostFilters map that filters something beside a FilterExpression".to_string(),
            json!({
                "BudgetName": "selected",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "CostFilters": {"Service": ["Amazon Elastic Compute Cloud - Compute"]},
                "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}}
            }),
        ),
    ];
    // One row per member of the model's own `Metric` catalog: a list carrying any of
    // them selects a metric, so it cannot be used with the CostTypes syntax.
    let refused = refused.into_iter().chain(METRIC_VALUES.iter().map(|metric| {
        (
            format!("a Metrics list selecting {metric} beside CostTypes"),
            json!({
                "BudgetName": "selected",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("10"),
                "CostTypes": {"IncludeTax": false},
                "Metrics": [metric]
            }),
        )
    }));
    for (case, budget) in refused {
        let (status, body) = create_and_read(&budget).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateBudget to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, REFUSAL),
            "case {case:?}: expected {REFUSAL} from CreateBudget, got {body}"
        );
    }

    // And the accepted half: a metric selected on its own writes the budget in the
    // expression syntax, so that budget has no cost types.
    for metric in METRIC_VALUES {
        let case = format!("a COST budget selecting the metric {metric}");
        let (status, read) = create_and_read(&json!({
            "BudgetName": "selected-metric",
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": spend("10"),
            "Metrics": [metric]
        }))
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {read}"
        );
        let reported = read.get("CostTypes").cloned().unwrap_or(Value::Null);
        assert!(
            reported.is_null(),
            "case {case:?}: a budget in the expression syntax has no CostTypes, got {reported}"
        );
        let metrics = read.get("Metrics").cloned().unwrap_or(Value::Null);
        assert!(
            same(&metrics, &json!([metric])),
            "case {case:?}: expected the selected metric to read back, got {metrics}"
        );
    }
}
