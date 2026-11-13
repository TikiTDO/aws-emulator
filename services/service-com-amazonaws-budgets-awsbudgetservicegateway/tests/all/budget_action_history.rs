// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the budget ACTION slice — `CreateBudgetAction`,
//! `DescribeBudgetAction`, `UpdateBudgetAction` and `DescribeBudgetActionHistories` —
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from (the harness is the lifecycle suite's, so every suite drives the service
//! identically).
//!
//! The RULE these tables assert: a budget action is a resource of the budget that owns
//! it, identified by the `ActionId` the SERVICE mints ("a system-generated universally
//! unique identifier (UUID) for the action"); every call that writes an action appends
//! exactly one record to that action's history, carrying the event it was, the action's
//! status "at the time of the event" and the action as that event left it; and
//! `DescribeBudgetActionHistories` REPORTS those records oldest first — narrowed to the
//! `TimePeriod` the caller named, one `MaxResults`-sized page at a time — without
//! changing anything a later caller sees.
//!
//! Each table spans that class rather than one request: its rows differ along the action
//! type and the `Definition` format it determines, the approval model, the notification
//! type, the threshold (each published `@range` bound and a value between them), the
//! subscriber count (each published `@length` bound), which member an update names, the
//! page size, and where the time window falls relative to the records.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Action, ActionHistory, ActionHistoryDetails, ACTION_TYPE_VALUES, APPROVAL_MODEL_VALUES,
    NOTIFICATION_TYPE_VALUES, SUBSCRIPTION_TYPE_VALUES, THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    encode_next_token, ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    ACTION_STATUS_EXECUTION_IN_PROGRESS, ACTION_STATUS_PENDING, ACTION_STATUS_STANDBY,
    EVENT_TYPE_CREATE_ACTION, EVENT_TYPE_EXECUTE_ACTION, EVENT_TYPE_SYSTEM,
    EVENT_TYPE_UPDATE_ACTION, EXECUTION_TYPE_APPROVE, MAX_ACTIONS_PER_BUDGET,
    MAX_ACTION_HISTORY_RECORDS, MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_lifecycle::{caller_account, call, is_error, router, router_with_engine, same};
use super::recorded_response_members::as_action_subscribers;

/// An `ExecutionRoleArn` satisfying `RoleArn`'s modelled `@pattern` and `@length(32, 618)`
/// (`smithy_query(query="shape",shape="RoleArn")`), in the account the action lives in:
/// "roles and actions must be in the same account" (`Action$ExecutionRoleArn`).
fn execution_role(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-role")
}

/// A second role of the same account, for the update table's row that replaces the role.
fn other_execution_role(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-role-two")
}

/// `NotificationThreshold`'s published `@range` — "range 0..15000000000000" from
/// `smithy_query(query="shape",shape="NotificationThreshold")`.
const THRESHOLD_MIN: f64 = 0.0;
const THRESHOLD_MAX: f64 = 15_000_000_000_000.0;

/// `Subscribers`' published `@length(min: 1, max: 11)`
/// (`smithy_query(query="shape",shape="Subscribers")`).
const SUBSCRIBERS_MIN: usize = 1;
const SUBSCRIBERS_MAX: usize = 11;

/// The `Definition` that `action_type` names.
///
/// "The type of action ... also determines the format for definition"
/// (`Action$ActionType`), so each `ActionType` the model lists has exactly one
/// `Definition` member. Every value below satisfies the modelled pattern of the member
/// it sits in (`PolicyArn`, `PolicyId`, `TargetId`, `Region`, `InstanceId`), read from
/// `smithy_query(query="closure",operation="CreateBudgetAction")`.
fn definition_for(action_type: &str) -> Value {
    match action_type {
        "APPLY_IAM_POLICY" => json!({
            "IamActionDefinition": {
                "PolicyArn": "arn:aws:iam::123456789012:policy/budget-restrict",
                "Roles": ["BudgetRestrictedRole"],
            }
        }),
        "APPLY_SCP_POLICY" => json!({
            "ScpActionDefinition": {"PolicyId": "p-abcd1234", "TargetIds": ["210987654321"]}
        }),
        "RUN_SSM_DOCUMENTS" => json!({
            "SsmActionDefinition": {
                "ActionSubType": "STOP_EC2_INSTANCES",
                "Region": "us-east-1",
                "InstanceIds": ["i-1234abcd"],
            }
        }),
        other => panic!("the model lists no ActionType {other}"),
    }
}

/// A second `Definition` of the same format as [`definition_for`], so an update row can
/// replace the type-specific parameters without changing the action type.
fn other_definition_for(action_type: &str) -> Value {
    match action_type {
        "APPLY_IAM_POLICY" => json!({
            "IamActionDefinition": {
                "PolicyArn": "arn:aws:iam::123456789012:policy/budget-deny-all",
                "Users": ["budget-user"],
            }
        }),
        "APPLY_SCP_POLICY" => json!({
            "ScpActionDefinition": {"PolicyId": "p-zyxw9876", "TargetIds": ["310987654321"]}
        }),
        "RUN_SSM_DOCUMENTS" => json!({
            "SsmActionDefinition": {
                "ActionSubType": "STOP_RDS_INSTANCES",
                "Region": "eu-west-1",
                "InstanceIds": ["i-abcd1234", "i-abcd5678"],
            }
        }),
        other => panic!("the model lists no ActionType {other}"),
    }
}

/// `count` subscribers spanning every `SubscriptionType` the model lists, within the
/// documented per-type split of a subscriber set ("a notification can have one SNS
/// subscriber and up to 10 email subscribers, for a total of 11 subscribers" —
/// `Notification`): the first entries are the SNS subscribers a set may hold, the rest
/// are email. A `count` past the modelled `@length(1, 11)` total therefore overflows the
/// total rather than a per-type half.
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
                    "Address": format!("budget-ops+{index}@example.com")
                })
            }
        })
        .collect();
    Value::Array(items)
}

/// One row of the action table.
struct Row {
    case: String,
    action_type: &'static str,
    notification_type: &'static str,
    approval_model: &'static str,
    threshold_value: f64,
    threshold_type: &'static str,
    subscriber_count: usize,
}

impl Row {
    /// The `ActionThreshold` this row creates its action with.
    fn threshold(&self) -> Value {
        json!({
            "ActionThresholdValue": self.threshold_value,
            "ActionThresholdType": self.threshold_type,
        })
    }

    /// The `CreateBudgetAction` request body for this row.
    fn create_request(&self, account_id: &str, budget_name: &str) -> Value {
        json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "NotificationType": self.notification_type,
            "ActionType": self.action_type,
            "ActionThreshold": self.threshold(),
            "Definition": definition_for(self.action_type),
            "ExecutionRoleArn": execution_role(account_id),
            "ApprovalModel": self.approval_model,
            "Subscribers": subscribers(self.subscriber_count),
        })
    }

    /// The `ActionStatus` a created action of this row carries: ARMED (`STANDBY`) while
    /// the spend the budget tracks has not reached the threshold that triggers the action,
    /// `PENDING` once it has and the action's execution needs the manual approval its
    /// `ApprovalModel` asks for. Nothing spends behind a budget here, no forecast is
    /// reported for one and nothing performs an action's execution, so the `MANUAL` rows
    /// that watch the ACTUAL spend at the published minimum threshold ([`THRESHOLD_MIN`])
    /// are the ones a created action waits in `PENDING` for.
    fn created_status(&self) -> &'static str {
        if self.notification_type == "ACTUAL"
            && self.approval_model == "MANUAL"
            && self.threshold_value <= THRESHOLD_MIN
        {
            ACTION_STATUS_PENDING
        } else {
            ACTION_STATUS_STANDBY
        }
    }

    /// The `Action` a read must report for this row once created: every member the
    /// caller supplied, plus the two the SERVICE owns (the minted `ActionId` and the
    /// `Status` a new action starts in) and the verification state it records for each
    /// subscriber's address (`tests/all/recorded_response_members.rs`).
    fn expected_action(&self, account_id: &str, budget_name: &str, action_id: &str) -> Value {
        json!({
            "ActionId": action_id,
            "BudgetName": budget_name,
            "NotificationType": self.notification_type,
            "ActionType": self.action_type,
            "ActionThreshold": self.threshold(),
            "Definition": definition_for(self.action_type),
            "ExecutionRoleArn": execution_role(account_id),
            "ApprovalModel": self.approval_model,
            "Status": self.created_status(),
            "Subscribers": as_action_subscribers(&subscribers(self.subscriber_count)),
        })
    }
}

/// The action table: the full cross product of the model's `ActionType` and
/// `ApprovalModel` catalogs, with the notification type, the threshold (each published
/// `@range` bound of `NotificationThreshold` and a value between them), the threshold
/// type and the subscriber count (each published `@length` bound of `Subscribers`)
/// varying across the rows — so a table that matches on one of them cannot be matching
/// on another by accident.
fn action_catalog() -> Vec<Row> {
    let thresholds = [THRESHOLD_MIN, 42.5, THRESHOLD_MAX];
    let subscriber_counts = [SUBSCRIBERS_MIN, 4, SUBSCRIBERS_MAX];
    let mut rows = Vec::new();
    for (type_index, action_type) in ACTION_TYPE_VALUES.iter().enumerate() {
        for (model_index, approval_model) in APPROVAL_MODEL_VALUES.iter().enumerate() {
            let position = type_index * APPROVAL_MODEL_VALUES.len() + model_index;
            rows.push(Row {
                case: format!("{action_type}/{approval_model}/row-{position}"),
                action_type,
                notification_type: NOTIFICATION_TYPE_VALUES
                    [position % NOTIFICATION_TYPE_VALUES.len()],
                approval_model,
                threshold_value: thresholds[position % thresholds.len()],
                threshold_type: THRESHOLD_TYPE_VALUES[position % THRESHOLD_TYPE_VALUES.len()],
                subscriber_count: subscriber_counts[position % subscriber_counts.len()],
            });
        }
    }
    rows
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
    row: &Row,
) -> String {
    let (status, body) = call(
        router,
        "CreateBudgetAction",
        &row.create_request(account_id, budget_name),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {}: expected CreateBudgetAction to answer 200, got {status} {body}",
        row.case
    );
    assert_eq!(
        body.get("AccountId").and_then(Value::as_str),
        Some(account_id),
        "case {}: expected the response to echo the account, got {body}",
        row.case
    );
    assert_eq!(
        body.get("BudgetName").and_then(Value::as_str),
        Some(budget_name),
        "case {}: expected the response to echo the budget name, got {body}",
        row.case
    );
    let action_id = body
        .get("ActionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_uuid(&action_id),
        "case {}: expected a minted UUID ActionId, got {action_id:?}",
        row.case
    );
    action_id
}

/// Whether `value` has the shape `ActionId`'s modelled `@pattern` requires:
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| group.len() == *width)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `DescribeBudgetAction`'s `Action` for an action that exists.
async fn describe_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribeBudgetAction",
        &json!({"AccountId": account_id, "BudgetName": budget_name, "ActionId": action_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeBudgetAction to answer 200, got {status} {body}"
    );
    body.get("Action").cloned().unwrap_or(Value::Null)
}

/// Every history record of one action, walking `NextToken` until the service stops
/// handing one back. `max_results` is the page size the walk asks for.
///
/// `pub(crate)` so the sibling suites in this test target read a history the same way
/// rather than copying the pagination walk.
pub(crate) async fn read_history(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    max_results: Option<usize>,
    case: &str,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("AccountId".to_string(), json!(account_id));
        request.insert("BudgetName".to_string(), json!(budget_name));
        request.insert("ActionId".to_string(), json!(action_id));
        if let Some(max) = max_results {
            request.insert("MaxResults".to_string(), json!(max));
        }
        if let Some(carried) = &token {
            request.insert("NextToken".to_string(), json!(carried));
        }
        let (status, body) = call(
            router,
            "DescribeBudgetActionHistories",
            &Value::Object(request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected DescribeBudgetActionHistories to answer 200, got \
             {status} {body}"
        );
        let page = body
            .get("ActionHistories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected ActionHistories, got {body}"));
        if let Some(max) = max_results {
            assert!(
                page.len() <= max,
                "case {case}: expected at most {max} records a page, got {}",
                page.len()
            );
        }
        collected.extend(page);
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            collected.len() <= 1_000,
            "case {case}: the pagination walk did not terminate"
        );
    }
    collected
}

/// `left` with every member of `patch` written over it — what an update leaves behind.
fn merged(left: &Value, patch: &Value) -> Value {
    let mut merged = left.as_object().cloned().unwrap_or_default();
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        merged.insert(member, value);
    }
    Value::Object(merged)
}

/// RULE row 1: a created action is reported back exactly as it was supplied, with the
/// service-owned `ActionId` and `Status`, and its history holds exactly the one
/// `CREATE_ACTION` record that call wrote.
#[tokio::test]
async fn creating_an_action_records_one_create_action_event() {
    let router = router();
    let account_id = caller_account();
    for row in action_catalog() {
        let budget_name = format!("budget-{}", row.case.replace('/', "-"));
        create_budget(&router, &account_id, &budget_name).await;
        let action_id = create_action(&router, &account_id, &budget_name, &row).await;

        let expected = row.expected_action(&account_id, &budget_name, &action_id);
        let read_back = describe_action(&router, &account_id, &budget_name, &action_id, &row.case)
            .await;
        assert!(
            same(&read_back, &expected),
            "case {}: expected the action {expected}, got {read_back}",
            row.case
        );

        let records = read_history(
            &router,
            &account_id,
            &budget_name,
            &action_id,
            None,
            &row.case,
        )
        .await;
        assert_eq!(
            records.len(),
            1,
            "case {}: expected one history record, got {records:?}",
            row.case
        );
        let record = &records[0];
        assert_eq!(
            record.get("EventType").and_then(Value::as_str),
            Some(EVENT_TYPE_CREATE_ACTION),
            "case {}: expected a CREATE_ACTION record, got {record}",
            row.case
        );
        assert_eq!(
            record.get("Status").and_then(Value::as_str),
            Some(row.created_status()),
            "case {}: expected the status at the time of the event, got {record}",
            row.case
        );
        assert!(
            record.get("Timestamp").and_then(Value::as_f64).is_some(),
            "case {}: expected a Timestamp on the record, got {record}",
            row.case
        );
        let details = record
            .get("ActionHistoryDetails")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            details
                .get("Message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {}: expected a Message on the record details, got {details}",
            row.case
        );
        let recorded_action = details.get("Action").cloned().unwrap_or(Value::Null);
        assert!(
            same(&recorded_action, &expected),
            "case {}: expected the recorded action {expected}, got {recorded_action}",
            row.case
        );
    }
}

/// RULE row 2: every write against an action appends exactly one `UPDATE_ACTION` record
/// carrying the action as that write left it, and leaves the records already there
/// alone — whichever member the update names.
#[tokio::test]
async fn each_update_appends_one_update_action_event() {
    let router = router();
    let account_id = caller_account();
    let row = &action_catalog()[0];
    let budget_name = "update-history".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let action_id = create_action(&router, &account_id, &budget_name, row).await;
    let created = row.expected_action(&account_id, &budget_name, &action_id);

    // One row per member `UpdateBudgetAction` accepts; `ActionId`, `BudgetName`,
    // `ActionType` and the service-owned `Status` are not among them.
    let patches = vec![
        ("NotificationType", json!({"NotificationType": "FORECASTED"})),
        (
            "ActionThreshold",
            json!({"ActionThreshold": {
                "ActionThresholdValue": 73.25,
                "ActionThresholdType": "ABSOLUTE_VALUE",
            }}),
        ),
        ("ApprovalModel", json!({"ApprovalModel": "MANUAL"})),
        ("Subscribers", json!({"Subscribers": subscribers(3)})),
        (
            "ExecutionRoleArn",
            json!({"ExecutionRoleArn": other_execution_role(&account_id)}),
        ),
        (
            "Definition",
            json!({"Definition": other_definition_for(row.action_type)}),
        ),
        // A call that names no member at all still records the event it was.
        ("no-member", json!({})),
    ];

    let mut current = created.clone();
    let mut expected_records = 1;
    for (case, patch) in patches {
        let mut request = json!({
            "AccountId": &account_id,
            "BudgetName": &budget_name,
            "ActionId": &action_id,
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

        let old_action = body.get("OldAction").cloned().unwrap_or(Value::Null);
        assert!(
            same(&old_action, &current),
            "case {case}: expected OldAction {current}, got {old_action}"
        );
        let mut expected_new = merged(&current, &patch);
        // A replacement subscriber set reads back with the member of each subscriber the
        // SERVICE owns stamped on it, exactly as the create path establishes one.
        if patch.get("Subscribers").is_some() {
            expected_new["Subscribers"] = as_action_subscribers(&patch["Subscribers"]);
        }
        let new_action = body.get("NewAction").cloned().unwrap_or(Value::Null);
        assert!(
            same(&new_action, &expected_new),
            "case {case}: expected NewAction {expected_new}, got {new_action}"
        );

        let read_back =
            describe_action(&router, &account_id, &budget_name, &action_id, case).await;
        assert!(
            same(&read_back, &expected_new),
            "case {case}: expected the stored action {expected_new}, got {read_back}"
        );

        expected_records += 1;
        let records =
            read_history(&router, &account_id, &budget_name, &action_id, None, case).await;
        assert_eq!(
            records.len(),
            expected_records,
            "case {case}: expected {expected_records} history records, got {}",
            records.len()
        );
        let latest = records.last().expect("the history is not empty");
        assert_eq!(
            latest.get("EventType").and_then(Value::as_str),
            Some(EVENT_TYPE_UPDATE_ACTION),
            "case {case}: expected the latest record to be an UPDATE_ACTION, got {latest}"
        );
        let recorded = latest
            .get("ActionHistoryDetails")
            .and_then(|details| details.get("Action"))
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            same(&recorded, &expected_new),
            "case {case}: expected the record to carry the updated action {expected_new}, got \
             {recorded}"
        );

        let first = &records[0];
        assert_eq!(
            first.get("EventType").and_then(Value::as_str),
            Some(EVENT_TYPE_CREATE_ACTION),
            "case {case}: expected the first record to stay a CREATE_ACTION, got {first}"
        );
        let first_action = first
            .get("ActionHistoryDetails")
            .and_then(|details| details.get("Action"))
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            same(&first_action, &created),
            "case {case}: expected the first record to keep the action as created {created}, \
             got {first_action}"
        );

        let timestamps: Vec<f64> = records
            .iter()
            .map(|record| record.get("Timestamp").and_then(Value::as_f64).unwrap_or(-1.0))
            .collect();
        assert!(
            timestamps.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case}: expected the history oldest first, got timestamps {timestamps:?}"
        );

        current = expected_new;
    }
}

/// RULE row 2, the other write path: an execution is recorded as the action's
/// `EXECUTE_ACTION` event, carrying the status that call moved the action to — and the
/// records already there are left as the events that wrote them left them.
#[tokio::test]
async fn executing_an_action_records_one_execute_action_event() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "execution-history".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    // An action WAITING for its manual approval, which is the one status an
    // `APPROVE_BUDGET_ACTION` is accepted from: the published minimum threshold is
    // already reached by the zero ACTUAL spend every budget here reports.
    let row = &Row {
        case: "approve".to_string(),
        action_type: ACTION_TYPE_VALUES[0],
        notification_type: "ACTUAL",
        approval_model: "MANUAL",
        threshold_value: THRESHOLD_MIN,
        threshold_type: THRESHOLD_TYPE_VALUES[0],
        subscriber_count: SUBSCRIBERS_MIN,
    };
    assert_eq!(
        row.created_status(),
        ACTION_STATUS_PENDING,
        "case approve: expected the row to create an action waiting for its approval"
    );
    let action_id = create_action(&router, &account_id, &budget_name, row).await;

    let (status, body) = call(
        &router,
        "ExecuteBudgetAction",
        &json!({
            "AccountId": &account_id,
            "BudgetName": &budget_name,
            "ActionId": &action_id,
            "ExecutionType": EXECUTION_TYPE_APPROVE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case approve: expected ExecuteBudgetAction to answer 200, got {status} {body}"
    );

    let records = read_history(
        &router,
        &account_id,
        &budget_name,
        &action_id,
        None,
        "approve",
    )
    .await;
    // The create and the execute event. The settle the clock owes may already have
    // fallen due by the time this read ran, and that settle records an event of its own,
    // so the execute record is named by its POSITION rather than assumed to be the
    // newest; the settle's own record is asserted in `budget_action_settle_history`.
    assert!(
        records.len() >= 2,
        "case approve: expected the create and the execute event, got {records:?}"
    );
    let executed = &records[1];
    assert_eq!(
        executed.get("EventType").and_then(Value::as_str),
        Some(EVENT_TYPE_EXECUTE_ACTION),
        "case approve: expected an EXECUTE_ACTION record, got {executed}"
    );
    assert_eq!(
        executed.get("Status").and_then(Value::as_str),
        Some(ACTION_STATUS_EXECUTION_IN_PROGRESS),
        "case approve: expected the status at the time of the event, got {executed}"
    );
    let recorded_status = executed
        .get("ActionHistoryDetails")
        .and_then(|details| details.get("Action"))
        .and_then(|action| action.get("Status"))
        .and_then(Value::as_str);
    assert_eq!(
        recorded_status,
        Some(ACTION_STATUS_EXECUTION_IN_PROGRESS),
        "case approve: expected the record to carry the action as the event left it, got \
         {executed}"
    );
    for later in &records[2..] {
        assert_eq!(
            later.get("EventType").and_then(Value::as_str),
            Some(EVENT_TYPE_SYSTEM),
            "case approve: only the settle this service makes on its own may follow the \
             execute event, got {later}"
        );
    }
    assert_eq!(
        records[0].get("Status").and_then(Value::as_str),
        Some(row.created_status()),
        "case approve: expected the create record to keep the status the create left, got {}",
        records[0]
    );
}

/// How many records the seeded histories below hold, and how far apart they sit.
const SEEDED_RECORDS: usize = 5;
/// `2026-01-01T00:00:00Z` in epoch seconds — the first seeded record's timestamp.
const SEEDED_START: i64 = 1_767_225_600;
/// One day in seconds: the gap between two seeded records.
const SEEDED_GAP: i64 = 86_400;

/// Replace the history of the single action on `budget_name` with
/// [`SEEDED_RECORDS`] records at known, spread-out instants, so a table over time
/// windows and page sizes is deterministic rather than depending on how fast the
/// requests that wrote them ran. Returns the action id.
fn seed_history(engine: &engine::InMemoryEngine, account_id: &str, budget_name: &str) -> String {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let mut store = state.lock();
    let stored = store
        .budgets
        .get_mut(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget exists");
    let held = stored
        .actions
        .first_mut()
        .expect("the seeded budget holds one action");
    let action: Action = held.action.clone();
    held.history = (0..SEEDED_RECORDS)
        .map(|index| {
            let offset = SEEDED_GAP * i64::try_from(index).expect("the index fits");
            ActionHistory {
                timestamp: aws_smithy_types::DateTime::from_secs(SEEDED_START + offset),
                status: action.status.clone(),
                event_type: if index == 0 {
                    EVENT_TYPE_CREATE_ACTION.to_string()
                } else {
                    EVENT_TYPE_UPDATE_ACTION.to_string()
                },
                action_history_details: ActionHistoryDetails {
                    message: format!("seeded record {index}"),
                    action: action.clone(),
                },
            }
        })
        .collect();
    action.action_id
}

/// The messages of `records`, which is what identifies a seeded record.
fn messages(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record
                .get("ActionHistoryDetails")
                .and_then(|details| details.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every seeded record's message, oldest first.
fn all_messages() -> Vec<String> {
    (0..SEEDED_RECORDS)
        .map(|index| format!("seeded record {index}"))
        .collect()
}

/// Build a router holding one budget with one action whose history is the seeded one.
async fn seeded() -> (MultiServiceRouter, String, String, String) {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "seeded-history".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];
    create_action(&router, &account_id, &budget_name, row).await;
    let action_id = seed_history(&engine, &account_id, &budget_name);
    (router, account_id, budget_name, action_id)
}

/// RULE row 3: the read reports every record of the action, oldest first, in pages of at
/// most `MaxResults` — and a walk over any page size sees exactly the same records in
/// exactly the same order as a walk over any other.
#[tokio::test]
async fn a_history_read_pages_through_every_record_in_order() {
    let (router, account_id, budget_name, action_id) = seeded().await;
    let expected = all_messages();

    // Every page size from one record a page to past the whole history, plus the
    // modelled `MaxResults` `@range` bound (100) and no `MaxResults` at all.
    let page_sizes: Vec<Option<usize>> = vec![
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(SEEDED_RECORDS),
        Some(SEEDED_RECORDS + 1),
        Some(100),
    ];
    for max_results in page_sizes {
        let case = format!("MaxResults={max_results:?}");
        let records = read_history(
            &router,
            &account_id,
            &budget_name,
            &action_id,
            max_results,
            &case,
        )
        .await;
        assert_eq!(
            messages(&records),
            expected,
            "case {case}: expected the whole history oldest first, got {:?}",
            messages(&records)
        );
    }
}

/// RULE row 3, the other half: a page size the history outruns hands back a `NextToken`,
/// and the last page does not.
#[tokio::test]
async fn only_a_page_with_records_after_it_carries_a_next_token() {
    let (router, account_id, budget_name, action_id) = seeded().await;
    for max_results in [1_usize, 2, 4, SEEDED_RECORDS, SEEDED_RECORDS + 2] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = call(
            &router,
            "DescribeBudgetActionHistories",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
        let carried = body.get("NextToken").and_then(Value::as_str).is_some();
        assert_eq!(
            carried,
            max_results < SEEDED_RECORDS,
            "case {case}: expected a NextToken only while records remain, got {body}"
        );
    }
}

/// RULE row 4: a `NextToken` this service did not mint for this list is refused with the
/// modelled `InvalidNextTokenException`, never reported as an empty or a wrong page.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_mint_is_refused() {
    let (router, account_id, budget_name, action_id) = seeded().await;
    let tokens = vec![
        ("not-hex", "not-a-token".to_string()),
        ("empty", String::new()),
        ("hex-but-foreign", hex::encode("other:offset:1")),
        // Minted by this service, but for an offset this list never had a page at.
        (
            "offset-past-the-end",
            encode_next_token(SEEDED_RECORDS + 10),
        ),
        ("offset-at-the-end", encode_next_token(SEEDED_RECORDS)),
    ];
    for (case, token) in tokens {
        let (status, body) = call(
            &router,
            "DescribeBudgetActionHistories",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refusal to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidNextTokenException"),
            "case {case}: expected InvalidNextTokenException, got {body}"
        );
    }
}

/// RULE row 5: the read reports exactly the records the `TimePeriod` covers — a bound
/// the caller left out does not restrict that side, and the bounds themselves are
/// covered.
#[tokio::test]
async fn a_history_read_reports_only_the_records_the_time_period_covers() {
    let (router, account_id, budget_name, action_id) = seeded().await;
    let at = |index: i64| SEEDED_START + SEEDED_GAP * index;
    let message = |index: usize| format!("seeded record {index}");

    // Each row: the window, and the records it covers.
    let windows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no-period", json!({}), all_messages()),
        (
            "the whole span, bounds included",
            json!({"Start": at(0), "End": at(4)}),
            all_messages(),
        ),
        (
            "inside the span",
            json!({"Start": at(1), "End": at(3)}),
            vec![message(1), message(2), message(3)],
        ),
        (
            "start only",
            json!({"Start": at(3)}),
            vec![message(3), message(4)],
        ),
        (
            "end only",
            json!({"End": at(1)}),
            vec![message(0), message(1)],
        ),
        (
            "one record wide",
            json!({"Start": at(2), "End": at(2) + 1}),
            vec![message(2)],
        ),
        (
            "entirely after the history",
            json!({"Start": at(5), "End": at(9)}),
            Vec::new(),
        ),
        (
            "entirely before the history",
            json!({"Start": at(-9), "End": at(-1)}),
            Vec::new(),
        ),
        (
            "a bound between two records",
            json!({"Start": at(1) + 1, "End": at(3) - 1}),
            vec![message(2)],
        ),
    ];

    for (case, period, expected) in windows {
        let mut request = json!({
            "AccountId": &account_id,
            "BudgetName": &budget_name,
            "ActionId": &action_id,
        });
        if period.as_object().is_some_and(|members| !members.is_empty()) {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert("TimePeriod".to_string(), period);
        }
        let (status, body) = call(&router, "DescribeBudgetActionHistories", &request).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
        let records = body
            .get("ActionHistories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            messages(&records),
            expected,
            "case {case}: expected the records {expected:?}, got {:?}",
            messages(&records)
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case}: the whole filtered page fits, so no NextToken is owed, got {body}"
        );
    }
}

/// RULE row 5, the refusal: "The start date must come before the end date"
/// (`TimePeriod`), so a window whose bounds do not hold together is an invalid input
/// value — not an empty history.
#[tokio::test]
async fn a_time_period_whose_bounds_do_not_hold_together_is_refused() {
    let (router, account_id, budget_name, action_id) = seeded().await;
    let at = |index: i64| SEEDED_START + SEEDED_GAP * index;
    let windows = vec![
        ("end before start", json!({"Start": at(3), "End": at(1)})),
        ("start equal to end", json!({"Start": at(2), "End": at(2)})),
    ];
    for (case, period) in windows {
        let (status, body) = call(
            &router,
            "DescribeBudgetActionHistories",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "TimePeriod": period,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refusal to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case}: expected InvalidParameterException, got {body}"
        );
    }
}

/// RULE row 6: an action is reached through the budget that owns it, so a budget or an
/// action that does not exist is refused with the modelled `NotFoundException` — on
/// every operation of the slice, and whichever part of the identity is wrong.
#[tokio::test]
async fn an_action_that_does_not_exist_is_refused() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "owned-budget".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];
    let action_id = create_action(&router, &account_id, &budget_name, row).await;
    // Well-formed per `ActionId`'s `@pattern`, and minted for nothing.
    let unknown_action = "00000000-0000-4000-8000-000000000000";

    let identities: Vec<(&str, String, String, String)> = vec![
        (
            "unknown action",
            account_id.clone(),
            budget_name.clone(),
            unknown_action.to_string(),
        ),
        (
            "unknown budget",
            account_id.clone(),
            "no-such-budget".to_string(),
            action_id.clone(),
        ),
        (
            "the budget's name in another case",
            account_id.clone(),
            budget_name.to_uppercase(),
            action_id.clone(),
        ),
    ];
    for (case, account_id, budget_name, action_id) in identities {
        for operation in ["DescribeBudgetActionHistories", "DescribeBudgetAction"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &budget_name,
                    "ActionId": &action_id,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}/{operation}: expected the refusal to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case}/{operation}: expected NotFoundException, got {body}"
            );
        }
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &budget_name,
                "ActionId": &action_id,
                "ApprovalModel": "MANUAL",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}/UpdateBudgetAction: expected the refusal to answer 400, got \
             {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case}/UpdateBudgetAction: expected NotFoundException, got {body}"
        );
    }

    // And creating an action needs the budget to exist first.
    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &row.create_request(&account_id, "no-such-budget"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case create-on-unknown-budget: expected the refusal to answer 400, got {status} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "case create-on-unknown-budget: expected NotFoundException, got {body}"
    );
}

/// RULE row 7: "the type of action ... also determines the format for definition"
/// (`Action$ActionType`) — the one set of type-specific parameters that matches the
/// action type is required, and any other is refused with the modelled
/// `InvalidParameterException`.
#[tokio::test]
async fn a_definition_must_match_the_action_type() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "definition-rules".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let template = &action_catalog()[0];

    for action_type in ACTION_TYPE_VALUES {
        // Every definition format the model has, plus the empty one.
        let mut definitions: Vec<(String, Value)> = ACTION_TYPE_VALUES
            .iter()
            .map(|owner| ((*owner).to_string(), definition_for(owner)))
            .collect();
        definitions.push(("empty".to_string(), json!({})));
        for (owner, definition) in definitions {
            let case = format!("{action_type} with the {owner} definition");
            let mut request = template.create_request(&account_id, &budget_name);
            let members = request.as_object_mut().expect("the request is an object");
            members.insert("ActionType".to_string(), json!(action_type));
            members.insert("Definition".to_string(), definition);
            let (status, body) = call(&router, "CreateBudgetAction", &request).await;
            if &owner == action_type {
                assert_eq!(
                    status, 200,
                    "case {case}: the definition matches the action type, so expected 200, got \
                     {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the refusal to answer 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case}: expected InvalidParameterException, got {body}"
                );
            }
        }
    }
}

/// RULE row 7, through an update: a replacement `Definition` is checked against the
/// action type the action was created with, which no update can change.
#[tokio::test]
async fn an_update_cannot_replace_a_definition_with_another_format() {
    let router = router();
    let account_id = caller_account();
    let catalog = action_catalog();

    for action_type in ACTION_TYPE_VALUES {
        let row = catalog
            .iter()
            .find(|row| row.action_type == *action_type)
            .expect("the catalog spans every ActionType");
        let budget_name = format!("definition-updates-{action_type}");
        create_budget(&router, &account_id, &budget_name).await;
        let action_id = create_action(&router, &account_id, &budget_name, row).await;
        for owner in ACTION_TYPE_VALUES {
            let case = format!("{} updated with the {owner} definition", row.action_type);
            let (status, body) = call(
                &router,
                "UpdateBudgetAction",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &budget_name,
                    "ActionId": &action_id,
                    "Definition": definition_for(owner),
                }),
            )
            .await;
            if *owner == row.action_type {
                assert_eq!(
                    status, 200,
                    "case {case}: the definition keeps the action's format, so expected 200, \
                     got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the refusal to answer 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case}: expected InvalidParameterException, got {body}"
                );
            }
        }
    }
}

/// RULE row 8: the store is bounded — a budget cannot hold more actions than its service
/// quota, and the request past it is refused with the modelled
/// `ServiceQuotaExceededException` rather than growing the process.
#[tokio::test]
async fn a_budget_cannot_hold_more_actions_than_its_quota() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "action-quota".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];

    let mut minted = Vec::new();
    for index in 0..MAX_ACTIONS_PER_BUDGET {
        let action_id = create_action(&router, &account_id, &budget_name, row).await;
        assert!(
            !minted.contains(&action_id),
            "case action-{index}: expected a fresh ActionId, got the already minted \
             {action_id}"
        );
        minted.push(action_id);
    }

    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &row.create_request(&account_id, &budget_name),
    )
    .await;
    assert_eq!(
        status, 400,
        "case at-quota: expected the refusal to answer 400, got {status} {body}"
    );
    assert!(
        is_error(&body, "ServiceQuotaExceededException"),
        "case at-quota: expected ServiceQuotaExceededException, got {body}"
    );
}

/// RULE row 9: reading an action, or its history, changes nothing a later caller sees —
/// the same read twice over answers the same thing, and the write path is the only thing
/// that appends a record.
#[tokio::test]
async fn reading_an_action_history_changes_nothing() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "repeated-reads".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];
    let action_id = create_action(&router, &account_id, &budget_name, row).await;

    let first = read_history(&router, &account_id, &budget_name, &action_id, None, "first").await;
    let action_first =
        describe_action(&router, &account_id, &budget_name, &action_id, "first").await;
    for round in 0..3 {
        let case = format!("round-{round}");
        let again =
            read_history(&router, &account_id, &budget_name, &action_id, Some(1), &case).await;
        assert_eq!(
            again.len(),
            first.len(),
            "case {case}: expected the history to stay {} records long, got {}",
            first.len(),
            again.len()
        );
        assert!(
            same(&Value::Array(again.clone()), &Value::Array(first.clone())),
            "case {case}: expected the same history {first:?}, got {again:?}"
        );
        let action_again =
            describe_action(&router, &account_id, &budget_name, &action_id, &case).await;
        assert!(
            same(&action_again, &action_first),
            "case {case}: expected the same action {action_first}, got {action_again}"
        );
    }
}

/// RULE row 11: an action's history is bounded — a write past the ceiling costs the
/// OLDEST record its place rather than growing the store, so a read always reports the
/// most recent history.
#[tokio::test]
async fn an_action_history_drops_its_oldest_record_at_the_ceiling() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "bounded-history".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];
    let action_id = create_action(&router, &account_id, &budget_name, row).await;

    // Seed the action right at its ceiling, a second apart, rather than driving a
    // thousand writes.
    {
        let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
        let mut store = state.lock();
        let stored = store
            .budgets
            .get_mut(&(account_id.clone(), budget_name.clone()))
            .expect("the seeded budget exists");
        let held = stored
            .actions
            .first_mut()
            .expect("the seeded budget holds one action");
        let action: Action = held.action.clone();
        held.history = (0..MAX_ACTION_HISTORY_RECORDS)
            .map(|index| ActionHistory {
                timestamp: aws_smithy_types::DateTime::from_secs(
                    SEEDED_START + i64::try_from(index).expect("the index fits"),
                ),
                status: action.status.clone(),
                event_type: EVENT_TYPE_CREATE_ACTION.to_string(),
                action_history_details: ActionHistoryDetails {
                    message: format!("seeded record {index}"),
                    action: action.clone(),
                },
            })
            .collect();
    }

    let (status, body) = call(
        &router,
        "UpdateBudgetAction",
        &json!({
            "AccountId": &account_id,
            "BudgetName": &budget_name,
            "ActionId": &action_id,
            "ApprovalModel": "MANUAL",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at-the-ceiling: expected UpdateBudgetAction to answer 200, got {status} {body}"
    );

    let records = read_history(
        &router,
        &account_id,
        &budget_name,
        &action_id,
        None,
        "at-the-ceiling",
    )
    .await;
    assert_eq!(
        records.len(),
        MAX_ACTION_HISTORY_RECORDS,
        "case at-the-ceiling: expected the history to stay {MAX_ACTION_HISTORY_RECORDS} \
         records long, got {}",
        records.len()
    );
    let reported = messages(&records);
    assert_eq!(
        reported.first().map(String::as_str),
        Some("seeded record 1"),
        "case at-the-ceiling: expected the oldest record to have been dropped, got {:?}",
        reported.first()
    );
    let latest = records.last().expect("the history is not empty");
    assert_eq!(
        latest.get("EventType").and_then(Value::as_str),
        Some(EVENT_TYPE_UPDATE_ACTION),
        "case at-the-ceiling: expected the newest record to be the update just made, got \
         {latest}"
    );
}

/// RULE row 10: a value outside a MODELLED constraint is refused at request validation,
/// before the handler runs — the published bounds of `ActionThreshold` and `Subscribers`
/// hold, and one step past either is a 400 `ValidationException`.
#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_refused() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "modelled-bounds".to_string();
    create_budget(&router, &account_id, &budget_name).await;
    let row = &action_catalog()[0];

    let outside: Vec<(&str, &str, Value)> = vec![
        (
            "threshold below the range",
            "ActionThreshold",
            json!({"ActionThresholdValue": THRESHOLD_MIN - 1.0, "ActionThresholdType": "PERCENTAGE"}),
        ),
        (
            "threshold above the range",
            "ActionThreshold",
            json!({"ActionThresholdValue": THRESHOLD_MAX + 1.0, "ActionThresholdType": "PERCENTAGE"}),
        ),
        (
            "no subscribers",
            "Subscribers",
            subscribers(SUBSCRIBERS_MIN - 1),
        ),
        (
            "too many subscribers",
            "Subscribers",
            subscribers(SUBSCRIBERS_MAX + 1),
        ),
        (
            "an action type the model does not list",
            "ActionType",
            json!("DELETE_EVERYTHING"),
        ),
    ];
    for (case, member, value) in outside {
        let mut request = row.create_request(&account_id, &budget_name);
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert(member.to_string(), value);
        let (status, body) = call(&router, "CreateBudgetAction", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refusal to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // Each published bound itself is accepted.
    let inside: Vec<(&str, &str, Value)> = vec![
        (
            "threshold at the bottom of the range",
            "ActionThreshold",
            json!({"ActionThresholdValue": THRESHOLD_MIN, "ActionThresholdType": "PERCENTAGE"}),
        ),
        (
            "threshold at the top of the range",
            "ActionThreshold",
            json!({"ActionThresholdValue": THRESHOLD_MAX, "ActionThresholdType": "PERCENTAGE"}),
        ),
        (
            "the fewest subscribers",
            "Subscribers",
            subscribers(SUBSCRIBERS_MIN),
        ),
        (
            "the most subscribers",
            "Subscribers",
            subscribers(SUBSCRIBERS_MAX),
        ),
    ];
    for (case, member, value) in inside {
        let mut request = row.create_request(&account_id, &budget_name);
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert(member.to_string(), value);
        let (status, body) = call(&router, "CreateBudgetAction", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: the value is inside its modelled bound, so expected 200, got \
             {status} {body}"
        );
    }
}
