// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for a budget's ACTIONS: `CreateBudgetAction`, read back through
//! `DescribeBudgetAction`, and the `Action$Status` state machine `ExecuteBudgetAction`
//! drives — all over the wire through the same `MultiServiceRouter` the production
//! router serves from.
//!
//! Each test states a RULE and asserts it over a set of inputs, not one request:
//!
//! 1. A created action is PERSISTED on the budget that owns it and read back as
//!    supplied, under a minted UUID `ActionId`, with only the member the service owns
//!    (`Status`) stamped — `STANDBY`, the armed status.
//! 2. `ExecuteBudgetAction` moves the action into the `*_IN_PROGRESS` status its
//!    `ExecutionType` names, and the clock settles that status a span later; an
//!    execution the action's current status does not allow — one its status leaves
//!    nothing to do (the approval of an action nothing has triggered among them), or one
//!    aimed at work that is still running — is refused for that state. The
//!    rule is asserted over the WHOLE cross product of the modelled `ActionStatus` and
//!    `ExecutionType` catalogs.
//! 3. The settle is owed to the CLOCK, so it happens with no further call.
//! 4. An action belongs to one budget of one account, so a request naming another
//!    budget, another account or an id no action carries locates nothing.
//! 5. `Action$ActionType` "determines the format for definition", so exactly the
//!    `Definition` member that action type names must be supplied.
//! 6. A budget holds a bounded number of actions, and every action gets its own id.
//!
//! The harness (`router`, `router_with_engine`, `call`, `is_error`, `account`, `same`)
//! is the lifecycle suite's, so every suite in this target drives the service the same
//! way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_STATUS_VALUES, ACTION_TYPE_VALUES, EXECUTION_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, ACTION_SETTLES_AFTER,
    MAX_ACTIONS_PER_BUDGET,
};

use super::budget_lifecycle::{caller_account, call, is_error, router, router_with_engine, same};
use super::recorded_response_members::as_action_subscribers;

/// A budget that satisfies every modelled constraint and documented cross-member rule,
/// so an action row varies only the action.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// Create `name` under `account_id` and fail the test if the setup call did not take.
///
/// `pub(crate)` so the sibling suites in this test target seed their budgets the same
/// way rather than copying the setup.
pub(crate) async fn seed_budget(router: &MultiServiceRouter, account_id: &str, name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget(name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
}

/// The `ExecutionRoleArn` every row uses: "the role passed for action execution and
/// reversion", satisfying `RoleArn`'s `@length(32, 618)` and its `arn:aws:iam::<12
/// digits>:role/<name>` pattern — built from the request's own account, since "roles and
/// actions must be in the same account" (`Action$ExecutionRoleArn`).
///
/// `pub(crate)` so the sibling suites in this test target name a role the same way.
pub(crate) fn execution_role_arn(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-executor")
}

/// `Definition.IamActionDefinition` — `PolicyArn` satisfies `PolicyArn`'s
/// `@length(25, 684)` and pattern; `Roles` is one of the three optional attach lists.
pub(crate) fn iam_definition() -> Value {
    json!({
        "IamActionDefinition": {
            "PolicyArn": "arn:aws:iam::111122223333:policy/DenyExpensiveLaunches",
            "Roles": ["app-runtime"]
        }
    })
}

/// `Definition.ScpActionDefinition` — `PolicyId` satisfies `^p-[0-9a-zA-Z_]{8,128}$`
/// and each `TargetId` the `(ou-…|\d{12})` alternatives of its pattern.
fn scp_definition() -> Value {
    json!({
        "ScpActionDefinition": {
            "PolicyId": "p-abcd1234ef",
            "TargetIds": ["111122223333", "ou-abcd-1234abcd"]
        }
    })
}

/// `Definition.SsmActionDefinition` — `ActionSubType` is one of the modelled
/// `ActionSubType` values, `Region` satisfies `^\w{2,4}-\w+(-\w+)?-\d$`, and each
/// `InstanceId` the `^i-(\w{8}|\w{17})$` alternative of its pattern.
fn ssm_definition() -> Value {
    json!({
        "SsmActionDefinition": {
            "ActionSubType": "STOP_EC2_INSTANCES",
            "Region": "us-east-1",
            "InstanceIds": ["i-1234567890abcdef0"]
        }
    })
}

/// The `Definition` member each `ActionType` is written with — "this field also
/// determines the format for definition" (`Action$ActionType`).
pub(crate) fn definition_for(action_type: &str) -> Value {
    match action_type {
        "APPLY_IAM_POLICY" => iam_definition(),
        "APPLY_SCP_POLICY" => scp_definition(),
        "RUN_SSM_DOCUMENTS" => ssm_definition(),
        other => panic!("the model's ActionType catalog grew a value this suite does not cover: {other}"),
    }
}

/// The `ActionStatus` a freshly created action carries, read off the action as it was
/// created (either the request body or the `Action` a read reports).
///
/// A created action is ARMED (`STANDBY`) while the spend the budget tracks has not reached
/// the threshold that triggers it, and `PENDING` once that spend has reached it AND the
/// action's execution needs the manual approval its `ApprovalModel` asks for. Three facts
/// about this emulator decide that for the rows below: nothing spends behind a budget, so
/// every budget here reports an ACTUAL spend of zero; no forecast is reported at all, so a
/// `FORECASTED` action watches a figure the budget does not state and nothing has reached
/// its threshold; and nothing performs an action's execution, so an `AUTOMATIC` action is
/// left armed. A `MANUAL` `ACTUAL` action at a threshold of zero — the `@range(min: 0)` of
/// `NotificationThreshold`, which is also `ActionThresholdValue`'s documented default
/// (`smithy_query(query="shape",shape="ActionThreshold")`) — is therefore the one created
/// action waiting for its approval. Stated for a budget that reports a limit, which every
/// budget these suites seed does.
pub(crate) fn created_status(action: &Value) -> &'static str {
    let threshold = action["ActionThreshold"]["ActionThresholdValue"]
        .as_f64()
        .unwrap_or_default();
    let watches_actual_spend = action["NotificationType"] == json!("ACTUAL");
    let needs_approval = action["ApprovalModel"] == json!("MANUAL");
    if watches_actual_spend && needs_approval && threshold <= 0.0 {
        "PENDING"
    } else {
        "STANDBY"
    }
}

/// An email subscriber; `Address` satisfies `SubscriberAddress`'s `@length(min: 1)` and
/// its "can't contain line breaks" rule.
pub(crate) fn email_subscriber(n: usize) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("ops{n}@example.com")})
}

/// The SNS subscriber: "either an SNS topic or an email".
pub(crate) fn sns_subscriber() -> Value {
    json!({
        "SubscriptionType": "SNS",
        "Address": format!("arn:aws:sns:us-east-1:{}:budget-action-alerts", caller_account())
    })
}

/// A `CreateBudgetAction` request body.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_body(
    account_id: &str,
    budget_name: &str,
    action_type: &str,
    definition: &Value,
    approval_model: &str,
    notification_type: &str,
    threshold: &Value,
    subscribers: &Value,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "NotificationType": notification_type,
        "ActionType": action_type,
        "ActionThreshold": threshold,
        "Definition": definition,
        "ExecutionRoleArn": execution_role_arn(account_id),
        "ApprovalModel": approval_model,
        "Subscribers": subscribers
    });
    if let Some(tags) = tags {
        body["ResourceTags"] = tags;
    }
    body
}

/// The simplest action a row can create, for the tests that vary something other than
/// the action's own members.
pub(crate) fn simple_create_body(account_id: &str, budget_name: &str) -> Value {
    create_body(
        account_id,
        budget_name,
        "APPLY_IAM_POLICY",
        &iam_definition(),
        "MANUAL",
        "ACTUAL",
        &json!({"ActionThresholdValue": 100.0, "ActionThresholdType": "PERCENTAGE"}),
        &json!([email_subscriber(0)]),
        None,
    )
}

/// The simplest action a row can create that is READY to be executed: an action whose
/// threshold has already been reached and whose `ApprovalModel` is `MANUAL` is created
/// waiting for that approval (`PENDING`), which is the status
/// `APPROVE_BUDGET_ACTION` is accepted from. Every budget these suites seed reports an
/// `ACTUAL` spend of zero, so a zero threshold — `NotificationThreshold`'s published
/// `@range` minimum — is the one already reached.
///
/// `pub(crate)` so the sibling suites in this test target reach an executable action the
/// same way.
pub(crate) fn pending_create_body(account_id: &str, budget_name: &str) -> Value {
    create_body(
        account_id,
        budget_name,
        "APPLY_IAM_POLICY",
        &iam_definition(),
        "MANUAL",
        "ACTUAL",
        &json!({"ActionThresholdValue": 0.0, "ActionThresholdType": "PERCENTAGE"}),
        &json!([email_subscriber(0)]),
        None,
    )
}

/// Create one action and answer its minted `ActionId`, failing the test if the setup
/// call did not take.
pub(crate) async fn seed_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> String {
    seed_action_from(router, &simple_create_body(account_id, budget_name), budget_name).await
}

/// As [`seed_action`], for an action created in the one status an approval is accepted
/// from ([`pending_create_body`]).
pub(crate) async fn seed_pending_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
) -> String {
    let action_id =
        seed_action_from(router, &pending_create_body(account_id, budget_name), budget_name).await;
    let status = read_status(router, account_id, budget_name, &action_id).await;
    assert_eq!(
        status, "PENDING",
        "setup: expected the created action to be waiting for its approval, got {status:?}"
    );
    action_id
}

/// Create one action from `create` and answer the `ActionId` the service minted for it.
async fn seed_action_from(
    router: &MultiServiceRouter,
    create: &Value,
    budget_name: &str,
) -> String {
    let (status, body) = call(router, "CreateBudgetAction", create).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction({budget_name}) to answer 200, got {status} {body}"
    );
    body["ActionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Whether `value` has the shape `ActionId`'s modelled pattern requires
/// (`smithy_query(query="shape",shape="ActionId")`:
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`).
pub(crate) fn is_action_id(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == GROUPS.len()
        && groups
            .iter()
            .zip(GROUPS)
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// The `Action` one `DescribeBudgetAction` call reports, or `Value::Null` when the read
/// was refused.
pub(crate) async fn describe_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id
        }),
    )
    .await
}

/// The `Action$Status` a read reports for `action_id`.
pub(crate) async fn read_status(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> String {
    let (status, body) = describe_action(router, account_id, budget_name, action_id).await;
    assert_eq!(
        status, 200,
        "expected DescribeBudgetAction({action_id}) to answer 200, got {status} {body}"
    );
    body["Action"]["Status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One `ExecuteBudgetAction` call.
pub(crate) async fn execute_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    execution_type: &str,
) -> (u16, Value) {
    call(
        router,
        "ExecuteBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id,
            "ExecutionType": execution_type
        }),
    )
    .await
}

/// Put the stored action into `status` with nothing owed, so the state machine can be
/// asserted from statuses no request in this emulator reaches (the `*_FAILURE` ones,
/// which nothing here mints because nothing here fails an action's work).
pub(crate) fn seed_status(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    status: &str,
) {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let mut store = state.lock();
    let stored = store
        .budgets
        .get_mut(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget");
    let action = stored
        .actions
        .iter_mut()
        .find(|held| held.action.action_id == action_id)
        .expect("the seeded action");
    action.action.status = status.to_string();
    action.pending = None;
}

/// The instant the clock owes the stored action's settle, or `None` when nothing is
/// owed.
fn settles_at(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) -> Option<SystemTime> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget")
        .actions
        .iter()
        .find(|held| held.action.action_id == action_id)
        .expect("the seeded action")
        .pending
        .as_ref()
        .map(|pending| pending.settles_at)
}

/// Bring the owed settle forward to NOW, so a test asserts the settled status without
/// waiting out the span (the span itself is asserted in
/// [`the_clock_settles_an_execution_with_no_further_call`]). The instant stays AFTER the
/// execution that owed it, which is what the clock would have done.
pub(crate) fn make_settle_due(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
) {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let mut store = state.lock();
    let stored = store
        .budgets
        .get_mut(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget");
    let action = stored
        .actions
        .iter_mut()
        .find(|held| held.action.action_id == action_id)
        .expect("the seeded action");
    let pending = action
        .pending
        .as_mut()
        .expect("an execution that is still in flight");
    pending.settles_at = SystemTime::now();
}

/// How many actions the budget holds.
fn action_count(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> usize {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the seeded budget")
        .actions
        .len()
}

/// One row of the persistence table.
struct Row {
    case: &'static str,
    action_type: &'static str,
    definition: Value,
    approval_model: &'static str,
    notification_type: &'static str,
    threshold: Value,
    subscribers: Value,
    tags: Option<Value>,
}

/// The persistence table. Rows differ along what rule 1 generalises over: the action
/// type and the `Definition` member it is written with, the approval model, the
/// notification type, the threshold value (both published `@range` bounds and a
/// fractional value) and its type, the number and types of subscribers (up to the
/// modelled `@length` maximum of 11), and whether `ResourceTags` were supplied.
///
/// Bounds and value sets come from the model
/// (`smithy_query(query="shape",shape="Action,NotificationThreshold,Subscribers")`):
/// `NotificationThreshold` is `@range(min: 0, max: 15000000000000)` and `Subscribers`
/// is `@length(min: 1, max: 11)`.
fn persistence_rows() -> Vec<Row> {
    vec![
        Row {
            case: "IAM policy action, automatic approval, percentage threshold",
            action_type: "APPLY_IAM_POLICY",
            definition: iam_definition(),
            approval_model: "AUTOMATIC",
            notification_type: "ACTUAL",
            threshold: json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
            subscribers: json!([email_subscriber(0)]),
            tags: None,
        },
        Row {
            case: "SCP action, manual approval, threshold at the published minimum, 11 subscribers",
            action_type: "APPLY_SCP_POLICY",
            definition: scp_definition(),
            approval_model: "MANUAL",
            notification_type: "FORECASTED",
            threshold: json!({"ActionThresholdValue": 0.0, "ActionThresholdType": "ABSOLUTE_VALUE"}),
            subscribers: Value::Array(
                std::iter::once(sns_subscriber())
                    .chain((0..10).map(email_subscriber))
                    .collect(),
            ),
            tags: None,
        },
        Row {
            case: "SSM action, threshold at the published maximum, one SNS subscriber",
            action_type: "RUN_SSM_DOCUMENTS",
            definition: ssm_definition(),
            approval_model: "MANUAL",
            notification_type: "ACTUAL",
            threshold: json!({
                "ActionThresholdValue": 15_000_000_000_000.0_f64,
                "ActionThresholdType": "ABSOLUTE_VALUE"
            }),
            subscribers: json!([sns_subscriber()]),
            tags: None,
        },
        Row {
            case: "IAM policy action attaching roles, groups and users, with tags",
            action_type: "APPLY_IAM_POLICY",
            definition: json!({
                "IamActionDefinition": {
                    "PolicyArn": "arn:aws:iam::aws:policy/AWSDenyAll",
                    "Roles": ["app-runtime", "batch-runner"],
                    "Groups": ["engineering"],
                    "Users": ["build-bot"]
                }
            }),
            approval_model: "AUTOMATIC",
            notification_type: "FORECASTED",
            threshold: json!({"ActionThresholdValue": 0.5, "ActionThresholdType": "PERCENTAGE"}),
            subscribers: json!([email_subscriber(0), email_subscriber(1)]),
            tags: Some(json!([{"Key": "team", "Value": "platform"}])),
        },
    ]
}

/// RULE 1: a created action is persisted on the budget that owns it and read back as
/// supplied, under a minted UUID `ActionId`, with `Status` — the one member the service
/// owns — stamped `STANDBY`.
#[tokio::test]
async fn a_created_action_round_trips_as_supplied() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "action-round-trip";
    seed_budget(&router, &account_id, budget_name).await;

    let mut minted: Vec<String> = Vec::new();
    for row in persistence_rows() {
        let case = row.case;
        let request = create_body(
            &account_id,
            budget_name,
            row.action_type,
            &row.definition,
            row.approval_model,
            row.notification_type,
            &row.threshold,
            &row.subscribers,
            row.tags.clone(),
        );
        let (status, created) = call(&router, "CreateBudgetAction", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {created}"
        );
        assert_eq!(
            created["AccountId"].as_str(),
            Some(account_id.as_str()),
            "case {case:?}: expected the response to echo the account id, got {created}"
        );
        assert_eq!(
            created["BudgetName"].as_str(),
            Some(budget_name),
            "case {case:?}: expected the response to echo the budget name, got {created}"
        );
        let action_id = created["ActionId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_action_id(&action_id),
            "case {case:?}: expected a UUID ActionId, got {action_id:?}"
        );
        assert!(
            !minted.contains(&action_id),
            "case {case:?}: expected a fresh ActionId, got the already-minted {action_id:?}"
        );
        minted.push(action_id.clone());

        let (status, body) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudgetAction to answer 200, got {status} {body}"
        );
        let action = &body["Action"];
        assert_eq!(
            action["ActionId"].as_str(),
            Some(action_id.as_str()),
            "case {case:?}: expected the read to report the minted id, got {action}"
        );
        assert_eq!(
            action["BudgetName"].as_str(),
            Some(budget_name),
            "case {case:?}: expected the action to name its budget, got {action}"
        );
        assert_eq!(
            action["Status"].as_str(),
            Some("STANDBY"),
            "case {case:?}: expected a created action to be STANDBY, got {action}"
        );
        for member in [
            "NotificationType",
            "ActionType",
            "ActionThreshold",
            "Definition",
            "ExecutionRoleArn",
            "ApprovalModel",
        ] {
            assert!(
                same(&action[member], &request[member]),
                "case {case:?}: expected {member} to read back as supplied ({}), got {}",
                request[member],
                action[member]
            );
        }
        // `Subscribers` reads back as supplied too, each subscriber carrying the one
        // member of it the SERVICE owns — the verification state of its address, which
        // `tests/all/recorded_response_members.rs` states the rule for.
        let expected_subscribers = as_action_subscribers(&request["Subscribers"]);
        assert!(
            same(&action["Subscribers"], &expected_subscribers),
            "case {case:?}: expected Subscribers to read back as {expected_subscribers}, got {}",
            action["Subscribers"]
        );

        // A read reports; it does not change what the next caller sees.
        let (_, again) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert!(
            same(&again, &body),
            "case {case:?}: expected a second read to report the same action, got {again} after {body}"
        );
    }
}

/// The `*_IN_PROGRESS` statuses of the modelled `ActionStatus` catalog: work this
/// service has started and not settled yet, which is the locked resource
/// `ResourceLockedException` names.
///
/// `pub(crate)` so the sibling suite that asserts the refusal's WORDING reads the same
/// matrix rather than copying it.
pub(crate) const IN_PROGRESS_STATUSES: [&str; 3] = [
    "EXECUTION_IN_PROGRESS",
    "REVERSE_IN_PROGRESS",
    "RESET_IN_PROGRESS",
];

/// The transitions the rule allows, written out as data: `(status the action is in,
/// execution type, the `*_IN_PROGRESS` status it takes, the status the clock settles it
/// to)`.
///
/// `APPROVE_BUDGET_ACTION` gives the approval a triggered action is waiting for;
/// `REVERSE_…` takes an applied one back off; `RESET_…` re-arms one that has already run
/// (and `ActionStatus` has no `RESET_SUCCESS`, so a completed reset leaves the action
/// armed in `STANDBY`); `RETRY_…` runs the work that failed again, whichever operation
/// that was. Every other pair of the catalogs has nothing to do — an armed `STANDBY`
/// action has no triggered execution to approve — and is refused.
///
/// `pub(crate)` so the sibling suite that asserts the refusal's WORDING reads the same
/// matrix rather than copying it.
pub(crate) const ALLOWED_EXECUTIONS: &[(&str, &str, &str, &str)] = &[
    (
        "PENDING",
        "APPROVE_BUDGET_ACTION",
        "EXECUTION_IN_PROGRESS",
        "EXECUTION_SUCCESS",
    ),
    (
        "EXECUTION_SUCCESS",
        "REVERSE_BUDGET_ACTION",
        "REVERSE_IN_PROGRESS",
        "REVERSE_SUCCESS",
    ),
    (
        "REVERSE_FAILURE",
        "REVERSE_BUDGET_ACTION",
        "REVERSE_IN_PROGRESS",
        "REVERSE_SUCCESS",
    ),
    (
        "EXECUTION_SUCCESS",
        "RESET_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
    (
        "EXECUTION_FAILURE",
        "RESET_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
    (
        "REVERSE_SUCCESS",
        "RESET_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
    (
        "REVERSE_FAILURE",
        "RESET_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
    (
        "RESET_FAILURE",
        "RESET_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
    (
        "EXECUTION_FAILURE",
        "RETRY_BUDGET_ACTION",
        "EXECUTION_IN_PROGRESS",
        "EXECUTION_SUCCESS",
    ),
    (
        "REVERSE_FAILURE",
        "RETRY_BUDGET_ACTION",
        "REVERSE_IN_PROGRESS",
        "REVERSE_SUCCESS",
    ),
    (
        "RESET_FAILURE",
        "RETRY_BUDGET_ACTION",
        "RESET_IN_PROGRESS",
        "STANDBY",
    ),
];

/// RULE 2, over the WHOLE cross product of the modelled `ActionStatus` and
/// `ExecutionType` catalogs: an execution the action's status leaves work for takes it
/// into the matching `*_IN_PROGRESS` status and settles there; every other pair is
/// refused for the state the action is in — an action already mid-flight and a settled
/// status that leaves this execution type nothing to do alike. A refusal changes
/// nothing.
#[tokio::test]
async fn an_execution_moves_the_action_only_where_its_status_leaves_work() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "action-state-machine";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for status in ACTION_STATUS_VALUES {
        for execution_type in EXECUTION_TYPE_VALUES {
            let case = format!("{status} + {execution_type}");
            seed_status(&engine, &account_id, budget_name, &action_id, status);

            let (http, body) = execute_action(
                &router,
                &account_id,
                budget_name,
                &action_id,
                execution_type,
            )
            .await;

            let allowed = ALLOWED_EXECUTIONS
                .iter()
                .find(|(from, execution, _, _)| from == status && execution == execution_type);

            let Some((_, _, in_progress, settled)) = allowed else {
                assert_eq!(
                    http, 400,
                    "case {case:?}: expected an execution with nothing to do to be refused with 400, got {http} {body}"
                );
                assert!(
                    is_error(&body, "ResourceLockedException"),
                    "case {case:?}: expected ResourceLockedException, got {body}"
                );
                let read = read_status(&router, &account_id, budget_name, &action_id).await;
                assert_eq!(
                    read, *status,
                    "case {case:?}: expected the refusal to leave the status alone, got {read:?}"
                );
                continue;
            };

            assert_eq!(
                http, 200,
                "case {case:?}: expected the execution to be accepted with 200, got {http} {body}"
            );
            for (member, expected) in [
                ("AccountId", account_id.as_str()),
                ("BudgetName", budget_name),
                ("ActionId", action_id.as_str()),
                ("ExecutionType", execution_type),
            ] {
                assert_eq!(
                    body[member].as_str(),
                    Some(expected),
                    "case {case:?}: expected the response to echo {member} as {expected:?}, got {body}"
                );
            }

            let read = read_status(&router, &account_id, budget_name, &action_id).await;
            assert_eq!(
                read, *in_progress,
                "case {case:?}: expected the action to be {in_progress} while the work runs, got {read:?}"
            );
            assert!(
                settles_at(&engine, &account_id, budget_name, &action_id).is_some(),
                "case {case:?}: expected a settle to be owed while the action is {in_progress}"
            );

            make_settle_due(&engine, &account_id, budget_name, &action_id);
            let read = read_status(&router, &account_id, budget_name, &action_id).await;
            assert_eq!(
                read, *settled,
                "case {case:?}: expected the settled status to be {settled}, got {read:?}"
            );
            assert!(
                settles_at(&engine, &account_id, budget_name, &action_id).is_none(),
                "case {case:?}: expected nothing further to be owed once the action settled"
            );
        }
    }
}

/// RULE 3: the settle is owed to the CLOCK — it happens a span after the execution with
/// no further call, and until then the action is mid-flight and locked against a second
/// execution. The span is the one the emulator publishes, and it is NOT zero: a caller
/// acting straight after the call still sees the in-progress status.
#[tokio::test]
async fn the_clock_settles_an_execution_with_no_further_call() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "action-settles-on-the-clock";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_pending_action(&router, &account_id, budget_name).await;

    // The window a transition this emulator owes falls due in.
    let floor = Duration::from_millis(250);
    let ceiling = Duration::from_millis(750);
    assert!(
        ACTION_SETTLES_AFTER >= floor && ACTION_SETTLES_AFTER <= ceiling,
        "expected the published settle span to be between {floor:?} and {ceiling:?}, got {ACTION_SETTLES_AFTER:?}"
    );

    let before = SystemTime::now();
    let (http, body) = execute_action(
        &router,
        &account_id,
        budget_name,
        &action_id,
        "APPROVE_BUDGET_ACTION",
    )
    .await;
    assert_eq!(
        http, 200,
        "expected the approval to be accepted with 200, got {http} {body}"
    );

    let due = settles_at(&engine, &account_id, budget_name, &action_id)
        .expect("a settle is owed while the execution runs");
    assert!(
        due >= before + floor,
        "expected the settle to fall due at least {floor:?} after the call, got {:?}",
        due.duration_since(before)
    );

    let read = read_status(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        read, "EXECUTION_IN_PROGRESS",
        "expected the caller reading straight after the call to see the work in flight, got {read:?}"
    );
    let (http, body) = execute_action(
        &router,
        &account_id,
        budget_name,
        &action_id,
        "APPROVE_BUDGET_ACTION",
    )
    .await;
    assert_eq!(
        http, 400,
        "expected a second execution over in-flight work to be refused with 400, got {http} {body}"
    );
    assert!(
        is_error(&body, "ResourceLockedException"),
        "expected ResourceLockedException while the work is in flight, got {body}"
    );

    // No further call makes the move: the clock does.
    tokio::time::sleep(ACTION_SETTLES_AFTER + Duration::from_millis(250)).await;
    let read = read_status(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        read, "EXECUTION_SUCCESS",
        "expected the clock to settle the execution with no further call, got {read:?}"
    );

    // And the settled action is the one the next execution decides from.
    let (http, body) = execute_action(
        &router,
        &account_id,
        budget_name,
        &action_id,
        "REVERSE_BUDGET_ACTION",
    )
    .await;
    assert_eq!(
        http, 200,
        "expected an applied action to be reversible, got {http} {body}"
    );
    let read = read_status(&router, &account_id, budget_name, &action_id).await;
    assert_eq!(
        read, "REVERSE_IN_PROGRESS",
        "expected the reversal to be in flight, got {read:?}"
    );
}

/// RULE 4: an action belongs to one budget of the account, so a request naming another
/// budget, or an id no action carries, locates nothing — for the read and for the
/// execution alike.
#[tokio::test]
async fn an_action_is_scoped_to_the_budget_that_owns_it() {
    let router = router();
    let owner = caller_account();
    let budget_name = "action-scoping";
    let sibling = "action-scoping-sibling";
    seed_budget(&router, &owner, budget_name).await;
    seed_budget(&router, &owner, sibling).await;
    let action_id = seed_action(&router, &owner, budget_name).await;

    // A well-formed `ActionId` no action carries: the modelled pattern is satisfied, so
    // the request reaches the handler.
    let absent_id = "00000000-0000-4000-8000-000000000000";
    assert!(
        is_action_id(absent_id),
        "the absent id must satisfy the modelled ActionId pattern"
    );

    let uppercased = budget_name.to_uppercase();
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a budget that does not exist",
            owner.as_str(),
            "no-such-budget",
            action_id.as_str(),
        ),
        (
            "another budget of the same account",
            owner.as_str(),
            sibling,
            action_id.as_str(),
        ),
        (
            "the owning budget's name in another case",
            owner.as_str(),
            uppercased.as_str(),
            action_id.as_str(),
        ),
        (
            "an id no action carries",
            owner.as_str(),
            budget_name,
            absent_id,
        ),
    ];

    for (case, account_id, budget, action) in rows {
        let (http, body) = describe_action(&router, account_id, budget, action).await;
        assert_eq!(
            http, 400,
            "case {case:?}: expected DescribeBudgetAction to be refused with 400, got {http} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException from the read, got {body}"
        );

        let (http, body) =
            execute_action(&router, account_id, budget, action, "APPROVE_BUDGET_ACTION").await;
        assert_eq!(
            http, 400,
            "case {case:?}: expected ExecuteBudgetAction to be refused with 400, got {http} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException from the execution, got {body}"
        );
    }

    // The owner's own action is still exactly where it was, in the status it was
    // created in.
    let read = read_status(&router, &owner, budget_name, &action_id).await;
    assert_eq!(
        read, "STANDBY",
        "expected the refusals to leave the owner's action alone, got {read:?}"
    );
}

/// RULE 5: "`ActionType` … also determines the format for definition", so exactly the
/// `Definition` member the action type names must be supplied — asserted over the whole
/// modelled `ActionType` catalog crossed with every combination of supplied members
/// this suite can build.
#[tokio::test]
async fn an_action_definition_must_match_its_action_type() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "action-definition-rule";
    seed_budget(&router, &account_id, budget_name).await;

    let supplied: Vec<(&str, Value)> = vec![
        ("the IAM member alone", iam_definition()),
        ("the SCP member alone", scp_definition()),
        ("the SSM member alone", ssm_definition()),
        ("no member at all", json!({})),
        (
            "two members at once",
            json!({
                "IamActionDefinition": iam_definition()["IamActionDefinition"],
                "ScpActionDefinition": scp_definition()["ScpActionDefinition"]
            }),
        ),
    ];

    let mut accepted = 0usize;
    for action_type in ACTION_TYPE_VALUES {
        for (shape, definition) in &supplied {
            let case = format!("{action_type} with {shape}");
            let matches = same(definition, &definition_for(action_type));
            let (http, body) = call(
                &router,
                "CreateBudgetAction",
                &create_body(
                    &account_id,
                    budget_name,
                    action_type,
                    definition,
                    "MANUAL",
                    "ACTUAL",
                    &json!({"ActionThresholdValue": 1.0, "ActionThresholdType": "PERCENTAGE"}),
                    &json!([email_subscriber(0)]),
                    None,
                ),
            )
            .await;

            if matches {
                accepted += 1;
                assert_eq!(
                    http, 200,
                    "case {case:?}: expected the matching definition to be accepted, got {http} {body}"
                );
            } else {
                assert_eq!(
                    http, 400,
                    "case {case:?}: expected a mismatched definition to be refused with 400, got {http} {body}"
                );
                assert!(
                    is_error(&body, "InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
            }
            assert_eq!(
                action_count(&engine, &account_id, budget_name),
                accepted,
                "case {case:?}: expected only the accepted creates to have persisted an action"
            );
        }
    }
    assert_eq!(
        accepted,
        ACTION_TYPE_VALUES.len(),
        "expected exactly one definition shape per action type to be accepted"
    );
}

/// RULE 6: a budget holds at most [`MAX_ACTIONS_PER_BUDGET`] actions, and every action
/// it holds has its own minted id.
#[tokio::test]
async fn a_budget_holds_a_bounded_number_of_actions_each_with_its_own_id() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "action-quota";
    let sibling = "action-quota-sibling";
    seed_budget(&router, &account_id, budget_name).await;
    seed_budget(&router, &account_id, sibling).await;

    let mut minted: Vec<String> = Vec::new();
    for index in 0..MAX_ACTIONS_PER_BUDGET {
        let (http, body) = call(
            &router,
            "CreateBudgetAction",
            &simple_create_body(&account_id, budget_name),
        )
        .await;
        assert_eq!(
            http, 200,
            "action {index}: expected a create below the ceiling to answer 200, got {http} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_action_id(&action_id) && !minted.contains(&action_id),
            "action {index}: expected a fresh UUID ActionId, got {action_id:?} after {minted:?}"
        );
        minted.push(action_id);
    }

    let (http, body) = call(
        &router,
        "CreateBudgetAction",
        &simple_create_body(&account_id, budget_name),
    )
    .await;
    assert_eq!(
        http, 400,
        "expected the create past the ceiling to be refused with 400, got {http} {body}"
    );
    assert!(
        is_error(&body, "ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException past the ceiling, got {body}"
    );
    assert_eq!(
        action_count(&engine, &account_id, budget_name),
        MAX_ACTIONS_PER_BUDGET,
        "expected the refused create to have persisted nothing"
    );

    // The ceiling is per budget, so a sibling budget of the same account still takes
    // actions, and every one of them is readable under the budget that owns it.
    let (http, body) = call(
        &router,
        "CreateBudgetAction",
        &simple_create_body(&account_id, sibling),
    )
    .await;
    assert_eq!(
        http, 200,
        "expected a sibling budget to take its own actions, got {http} {body}"
    );
    let sibling_action = body["ActionId"].as_str().unwrap_or_default().to_string();
    assert!(
        !minted.contains(&sibling_action),
        "expected the sibling's action to carry its own id, got {sibling_action:?}"
    );
    for action_id in &minted {
        let read = read_status(&router, &account_id, budget_name, action_id).await;
        assert_eq!(
            read, "STANDBY",
            "expected every action of the budget to be readable and armed, got {read:?} for {action_id}"
        );
    }
}

/// A create naming a budget the account does not hold persists nothing: an action is a
/// resource OF its budget.
#[tokio::test]
async fn an_action_cannot_be_created_on_a_budget_that_does_not_exist() {
    let router = router();
    let account_id = caller_account();

    let (http, body) = call(
        &router,
        "CreateBudgetAction",
        &simple_create_body(&account_id, "no-such-budget"),
    )
    .await;
    assert_eq!(
        http, 400,
        "expected a create on an absent budget to be refused with 400, got {http} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "expected NotFoundException, got {body}"
    );
}
