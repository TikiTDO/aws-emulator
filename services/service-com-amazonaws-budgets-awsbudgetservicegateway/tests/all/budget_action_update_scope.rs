// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for what one `UpdateBudgetAction` call WRITES, driven over the wire
//! through the same `MultiServiceRouter` the production router serves from (the harness
//! is the lifecycle suite's, so every suite drives the service identically).
//!
//! The RULE these tables assert: `UpdateBudgetAction` writes exactly the one action the
//! request names — the members the caller supplied replace the stored values, the members
//! it left out keep theirs, and every OTHER action, of that budget or of any other budget
//! of the account, is left as its own last write left it. The call reports the action as
//! it was before the write (`OldAction`) and as the write left it (`NewAction`), a read
//! reports the latter, and so re-sending an identical update is idempotent in the action
//! it leaves behind — while each call still records the event it was.
//!
//! Each table spans that class rather than one request: its rows differ along which
//! member the update names (one row per member `UpdateBudgetAction` accepts, a row naming
//! every one of them and a row naming none), the action type the stored action was
//! created with (and with it the `Definition` format that type determines), the published
//! bounds of the members that carry one (`NotificationThreshold`'s `@range`,
//! `Subscribers`' `@length`), and which of the planted actions is the one the request
//! names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_TYPE_VALUES, APPROVAL_MODEL_VALUES, NOTIFICATION_TYPE_VALUES, SUBSCRIPTION_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    EVENT_TYPE_UPDATE_ACTION, MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_action_history::read_history;
use super::budget_lifecycle::{call, caller_account, router, same};
use super::recorded_response_members::as_action_subscribers;

/// `NotificationThreshold`'s published `@range` — "range 0..15000000000000", from
/// `smithy_query(query="shape",shape="NotificationThreshold")`, which is the target of
/// `ActionThreshold$ActionThresholdValue`.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// `Subscribers`' published `@length(min: 1, max: 11)`
/// (`smithy_query(query="shape",shape="Subscribers")`).
const SUBSCRIBERS_MIN: usize = 1;
const SUBSCRIBERS_MAX: usize = 11;

/// One row per member `UpdateBudgetAction` accepts beyond the three that NAME the action
/// (`AccountId`, `BudgetName`, `ActionId`), plus a row that names every one of them at
/// once and a row that names none
/// (`smithy_query(query="operation",operation="UpdateBudgetAction")`).
const PATCH_ROWS: &[&str] = &[
    "notification-type",
    "approval-model",
    "action-threshold-at-min",
    "action-threshold-at-max",
    "subscribers-at-min",
    "subscribers-at-max",
    "execution-role",
    "definition",
    "every-member",
    "no-member",
];

/// An `ExecutionRoleArn` satisfying `RoleArn`'s modelled `@pattern` and `@length(32, 618)`
/// (`smithy_query(query="shape",shape="RoleArn")`), in the account the action lives in:
/// "roles and actions must be in the same account" (`Action$ExecutionRoleArn`). `suffix`
/// is what makes two roles of that account distinguishable.
fn execution_role(account_id: &str, suffix: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-{suffix}")
}

/// The `Definition` that `action_type` names, in one of two interchangeable variants.
///
/// "The type of action ... also determines the format for definition"
/// (`Action$ActionType`), so each `ActionType` the model lists has exactly one
/// `Definition` member and the two variants differ only in the type-specific parameters
/// inside it. Every value satisfies the modelled pattern of the member it sits in
/// (`PolicyArn`, `PolicyId`, `TargetId`, `Region`, `InstanceId`, `ActionSubType`), read
/// from `smithy_query(query="closure",operation="UpdateBudgetAction")`.
fn definition_for(action_type: &str, variant: usize) -> Value {
    match (action_type, variant % 2) {
        ("APPLY_IAM_POLICY", 0) => json!({
            "IamActionDefinition": {
                "PolicyArn": "arn:aws:iam::123456789012:policy/budget-restrict",
                "Roles": ["BudgetRestrictedRole"],
            }
        }),
        ("APPLY_IAM_POLICY", _) => json!({
            "IamActionDefinition": {
                "PolicyArn": "arn:aws:iam::123456789012:policy/budget-deny-all",
                "Users": ["budget-user"],
            }
        }),
        ("APPLY_SCP_POLICY", 0) => json!({
            "ScpActionDefinition": {"PolicyId": "p-abcd1234", "TargetIds": ["210987654321"]}
        }),
        ("APPLY_SCP_POLICY", _) => json!({
            "ScpActionDefinition": {"PolicyId": "p-zyxw9876", "TargetIds": ["310987654321"]}
        }),
        ("RUN_SSM_DOCUMENTS", 0) => json!({
            "SsmActionDefinition": {
                "ActionSubType": "STOP_EC2_INSTANCES",
                "Region": "us-east-1",
                "InstanceIds": ["i-1234abcd"],
            }
        }),
        ("RUN_SSM_DOCUMENTS", _) => json!({
            "SsmActionDefinition": {
                "ActionSubType": "STOP_RDS_INSTANCES",
                "Region": "eu-west-1",
                "InstanceIds": ["i-abcd1234", "i-abcd5678"],
            }
        }),
        (other, _) => panic!("the model lists no ActionType {other}"),
    }
}

/// `count` subscribers spanning every `SubscriptionType` the model lists, within the
/// documented per-type split of a subscriber set ("a notification can have one SNS
/// subscriber and up to 10 email subscribers, for a total of 11 subscribers" —
/// `Notification`): the first entries are the SNS subscribers a set may hold, the rest
/// are email.
fn subscribers(count: usize) -> Value {
    // The catalog this helper spans is the model's own, so a third subscription type
    // fails the suite rather than silently going untested.
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog this helper spans changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );
    let items: Vec<Value> = (0..count)
        .map(|index| {
            if index < MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION {
                json!({
                    "SubscriptionType": "SNS",
                    "Address": format!(
                        "arn:aws:sns:us-east-1:{}:budget-topic-{index}",
                        caller_account()
                    )
                })
            } else {
                json!({
                    "SubscriptionType": "EMAIL",
                    "Address": format!("ops-{index}@example.com")
                })
            }
        })
        .collect();
    Value::Array(items)
}

/// The first value of `catalog` that is not the one the stored action already carries —
/// so a row that replaces an enum member genuinely changes it.
fn other_of(catalog: &[&'static str], current: Option<&str>) -> &'static str {
    catalog
        .iter()
        .copied()
        .find(|value| Some(*value) != current)
        .unwrap_or_else(|| panic!("the catalog {catalog:?} holds no value besides {current:?}"))
}

/// One action this suite planted: where it lives and the type it was created with.
struct Planted {
    budget_name: String,
    action_id: String,
    action_type: &'static str,
}

impl Planted {
    /// How this action is named in an assertion message.
    fn label(&self) -> String {
        format!("{}/{}", self.budget_name, self.action_type)
    }
}

/// Create the budget the actions below hang off, over the wire.
async fn create_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {budget_name}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// Create one action and return the `ActionId` the service minted for it.
async fn create_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_type: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "NotificationType": NOTIFICATION_TYPE_VALUES[0],
            "ActionType": action_type,
            "ActionThreshold": {
                "ActionThresholdValue": 42.5,
                "ActionThresholdType": "PERCENTAGE",
            },
            "Definition": definition_for(action_type, 0),
            "ExecutionRoleArn": execution_role(account_id, "base"),
            "ApprovalModel": APPROVAL_MODEL_VALUES[0],
            "Subscribers": subscribers(2),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {budget_name}/{action_type}: expected CreateBudgetAction to answer 200, got \
         {status} {body}"
    );
    body.get("ActionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {budget_name}/{action_type}: expected a minted ActionId, got {body}")
        })
        .to_string()
}

/// `DescribeBudgetAction`'s `Action` for an action that exists.
async fn describe_action(
    router: &MultiServiceRouter,
    account_id: &str,
    action: &Planted,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribeBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": &action.budget_name,
            "ActionId": &action.action_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeBudgetAction to answer 200 for {}, got {status} {body}",
        action.label()
    );
    body.get("Action").cloned().unwrap_or(Value::Null)
}

/// Two budgets of the caller's account, each holding one action per modelled
/// `ActionType` — so every update below has siblings in its own budget AND in another
/// budget that the call must leave alone.
async fn plant(router: &MultiServiceRouter, account_id: &str) -> Vec<Planted> {
    let mut planted = Vec::new();
    for budget_name in ["named-budget", "bystander-budget"] {
        create_budget(router, account_id, budget_name).await;
        for action_type in ACTION_TYPE_VALUES {
            let action_id = create_action(router, account_id, budget_name, action_type).await;
            planted.push(Planted {
                budget_name: budget_name.to_string(),
                action_id,
                action_type,
            });
        }
    }
    planted
}

/// The members one row of [`PATCH_ROWS`] names, each a value the stored action does not
/// already carry — so a row that is applied and a row that is ignored cannot look alike.
fn patch_for(row: &str, account_id: &str, action_type: &str, current: &Value) -> Value {
    let member = |name: &str| current.get(name).and_then(Value::as_str);
    let notification_type =
        json!({"NotificationType": other_of(NOTIFICATION_TYPE_VALUES, member("NotificationType"))});
    let approval_model =
        json!({"ApprovalModel": other_of(APPROVAL_MODEL_VALUES, member("ApprovalModel"))});
    let threshold = |value: f64, kind: &str| {
        json!({"ActionThreshold": {"ActionThresholdValue": value, "ActionThresholdType": kind}})
    };
    let role = {
        let base = execution_role(account_id, "base");
        let replacement = if member("ExecutionRoleArn") == Some(base.as_str()) {
            execution_role(account_id, "replacement")
        } else {
            base
        };
        json!({"ExecutionRoleArn": replacement})
    };
    let definition = {
        let first = definition_for(action_type, 0);
        let replacement = if current.get("Definition") == Some(&first) {
            definition_for(action_type, 1)
        } else {
            first
        };
        json!({"Definition": replacement})
    };
    match row {
        "notification-type" => notification_type,
        "approval-model" => approval_model,
        "action-threshold-at-min" => threshold(THRESHOLD_MIN, "PERCENTAGE"),
        "action-threshold-at-max" => threshold(THRESHOLD_MAX, "ABSOLUTE_VALUE"),
        "subscribers-at-min" => json!({"Subscribers": subscribers(SUBSCRIBERS_MIN)}),
        "subscribers-at-max" => json!({"Subscribers": subscribers(SUBSCRIBERS_MAX)}),
        "execution-role" => role,
        "definition" => definition,
        "every-member" => {
            let mut every = serde_json::Map::new();
            for part in [
                notification_type,
                approval_model,
                threshold(7.5, "ABSOLUTE_VALUE"),
                json!({"Subscribers": subscribers(4)}),
                role,
                definition,
            ] {
                for (name, value) in part.as_object().cloned().unwrap_or_default() {
                    every.insert(name, value);
                }
            }
            Value::Object(every)
        }
        "no-member" => json!({}),
        other => panic!("no patch is defined for row {other}"),
    }
}

/// `left` with every member of `patch` written over it — what an update leaves behind.
///
/// A replacement `Subscribers` set is left behind with the one member of a subscriber the
/// SERVICE owns stamped on it — the verification state of its address, whose rule
/// `tests/all/recorded_response_members.rs` states.
fn merged(left: &Value, patch: &Value) -> Value {
    let mut merged = left.as_object().cloned().unwrap_or_default();
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        let written = if member == "Subscribers" {
            as_action_subscribers(&value)
        } else {
            value
        };
        merged.insert(member, written);
    }
    Value::Object(merged)
}

/// RULE row 1: an update replaces the members it names on the action it names, keeps the
/// members it does not name, reports the action either side of that write, and leaves
/// every other action — sibling of the same budget, or action of another budget — and
/// every other action's history exactly as they were.
#[tokio::test]
async fn an_update_writes_only_the_action_it_names() {
    let router = router();
    let account_id = caller_account();
    let planted = plant(&router, &account_id).await;

    // Every planted action as a read reports it, and how many history records it holds,
    // carried forward across the rows so each row asserts against the state its
    // predecessor left.
    let mut stored: Vec<Value> = Vec::new();
    let mut records: Vec<usize> = Vec::new();
    for action in &planted {
        stored.push(describe_action(&router, &account_id, action, "planted").await);
        records.push(
            read_history(
                &router,
                &account_id,
                &action.budget_name,
                &action.action_id,
                None,
                "planted",
            )
            .await
            .len(),
        );
    }

    for target in 0..planted.len() {
        for row in PATCH_ROWS {
            let case = format!("{}/{row}", planted[target].label());
            let patch = patch_for(row, &account_id, planted[target].action_type, &stored[target]);

            let mut request = json!({
                "AccountId": &account_id,
                "BudgetName": &planted[target].budget_name,
                "ActionId": &planted[target].action_id,
            });
            let request_members = request.as_object_mut().expect("the request is an object");
            for (member, value) in patch.as_object().cloned().unwrap_or_default() {
                request_members.insert(member, value);
            }

            let (status, body) = call(&router, "UpdateBudgetAction", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected UpdateBudgetAction to answer 200, got {status} {body}"
            );
            assert_eq!(
                body.get("AccountId").and_then(Value::as_str),
                Some(account_id.as_str()),
                "case {case}: expected the response to echo the account, got {body}"
            );
            assert_eq!(
                body.get("BudgetName").and_then(Value::as_str),
                Some(planted[target].budget_name.as_str()),
                "case {case}: expected the response to echo the budget name, got {body}"
            );

            let old_action = body.get("OldAction").cloned().unwrap_or(Value::Null);
            assert!(
                same(&old_action, &stored[target]),
                "case {case}: expected OldAction {}, got {old_action}",
                stored[target]
            );
            let expected = merged(&stored[target], &patch);
            let new_action = body.get("NewAction").cloned().unwrap_or(Value::Null);
            assert!(
                same(&new_action, &expected),
                "case {case}: expected NewAction {expected}, got {new_action}"
            );

            for (index, action) in planted.iter().enumerate() {
                let read_back = describe_action(&router, &account_id, action, &case).await;
                let wanted = if index == target {
                    &expected
                } else {
                    &stored[index]
                };
                assert!(
                    same(&read_back, wanted),
                    "case {case}: expected {} to read back as {wanted}, got {read_back}",
                    action.label()
                );

                let history = read_history(
                    &router,
                    &account_id,
                    &action.budget_name,
                    &action.action_id,
                    None,
                    &case,
                )
                .await;
                let expected_records = records[index] + usize::from(index == target);
                assert_eq!(
                    history.len(),
                    expected_records,
                    "case {case}: expected {} to hold {expected_records} history records, got \
                     {}",
                    action.label(),
                    history.len()
                );
                if index == target {
                    let latest = history.last().expect("the history is not empty");
                    assert_eq!(
                        latest.get("EventType").and_then(Value::as_str),
                        Some(EVENT_TYPE_UPDATE_ACTION),
                        "case {case}: expected the newest record to be an UPDATE_ACTION, got \
                         {latest}"
                    );
                    let recorded = latest
                        .get("ActionHistoryDetails")
                        .and_then(|details| details.get("Action"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    assert!(
                        same(&recorded, &expected),
                        "case {case}: expected the record to carry the action the update left \
                         {expected}, got {recorded}"
                    );
                }
                records[index] = expected_records;
            }
            stored[target] = expected;
        }
    }
}

/// RULE row 2: the write an update performs is a replacement of the members it names, so
/// sending the SAME update twice leaves the same action behind — the second call reports
/// an `OldAction` equal to its `NewAction` — while each call still records the event it
/// was.
#[tokio::test]
async fn re_sending_the_same_update_leaves_the_same_action() {
    let router = router();
    let account_id = caller_account();
    let planted = plant(&router, &account_id).await;

    for action in &planted {
        let before = describe_action(&router, &account_id, action, "planted").await;
        let planted_records = read_history(
            &router,
            &account_id,
            &action.budget_name,
            &action.action_id,
            None,
            "planted",
        )
        .await
        .len();
        let patch = patch_for("every-member", &account_id, action.action_type, &before);
        let expected = merged(&before, &patch);

        let mut request = json!({
            "AccountId": &account_id,
            "BudgetName": &action.budget_name,
            "ActionId": &action.action_id,
        });
        let request_members = request.as_object_mut().expect("the request is an object");
        for (member, value) in patch.as_object().cloned().unwrap_or_default() {
            request_members.insert(member, value);
        }

        for attempt in 1..=2 {
            let case = format!("{}/attempt-{attempt}", action.label());
            let (status, body) = call(&router, "UpdateBudgetAction", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected UpdateBudgetAction to answer 200, got {status} {body}"
            );
            let new_action = body.get("NewAction").cloned().unwrap_or(Value::Null);
            assert!(
                same(&new_action, &expected),
                "case {case}: expected NewAction {expected}, got {new_action}"
            );
            if attempt == 2 {
                let old_action = body.get("OldAction").cloned().unwrap_or(Value::Null);
                assert!(
                    same(&old_action, &new_action),
                    "case {case}: expected a repeated update to find the action it already \
                     wrote, got OldAction {old_action} against NewAction {new_action}"
                );
            }
        }

        let case = format!("{}/settled", action.label());
        let read_back = describe_action(&router, &account_id, action, &case).await;
        assert!(
            same(&read_back, &expected),
            "case {case}: expected the stored action {expected}, got {read_back}"
        );

        let history = read_history(
            &router,
            &account_id,
            &action.budget_name,
            &action.action_id,
            None,
            &case,
        )
        .await;
        assert_eq!(
            history.len(),
            planted_records + 2,
            "case {case}: expected both updates to be recorded beside the {planted_records} \
             record(s) the action was planted with, got {}",
            history.len()
        );
        for record in &history[planted_records..] {
            assert_eq!(
                record.get("EventType").and_then(Value::as_str),
                Some(EVENT_TYPE_UPDATE_ACTION),
                "case {case}: expected each recorded update to be an UPDATE_ACTION, got {record}"
            );
        }
    }
}
