// SPDX-License-Identifier: CC0-1.0

//! What a saga's `Catch` HANDS ON, and what the execution then reports: driven over all
//! three services' own wire protocols through the production `MultiServiceRouter` with
//! Step Functions, Lambda and DynamoDB mounted on ONE engine — so the reserve step
//! really writes its item through the optimized `dynamodb:putItem` integration, the
//! charge step's function really runs and really raises, and the compensating function's
//! own effect is read back through DynamoDB's own API.
//!
//! RULE: the first `Catch` catcher of a failed `Task` state whose `ErrorEquals` names the
//! failure's error name takes it over: the `{Error, Cause}` error output is placed in the
//! state's own RAW input at that catcher's `ResultPath` — the whole document when the
//! catcher declares none, since that field's default is `$` — and the execution continues
//! at the catcher's `Next`, whose work reads the error output from that very place and
//! whose own transitions then decide the outcome. So a compensation branch that ends
//! normally makes the whole execution `SUCCEEDED`, carrying the compensating work's own
//! answer as the execution's output, while a failure no catcher names ends the execution
//! `FAILED` under the error's own name with whatever the failed work had already written
//! left standing.
//!
//! The rows differ along what the rule generalises over: where the catcher places the
//! error output (a member of the document, a member the document does not carry yet two
//! levels down, and the default of `$` for a catcher declaring no `ResultPath`), what the
//! catcher's `ErrorEquals` names (either documented wildcard, the raised class itself, and
//! a class that is not the one raised), the `Resource` form the failing task names its
//! function in (the optimized integration and the function's own ARN), how the
//! compensation branch ends (on the compensating task itself, and through a `Succeed`
//! state after it), and whether the charge step raises at all.
//!
//! The harness (`Chain`, `given_function_of`, `ran`, `sfn`) is the one
//! `lambda_task_control_flow` builds for these services, and the table both handlers act
//! on, the names its items carry, the two wildcards a catcher may name and the reading of
//! a run's own error document are `saga_compensation_on_task_failure`'s;
//! `tests/all.rs` is this binary's crate root, so both are reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use crate::lambda_task_control_flow::Chain;
use crate::saga_compensation_on_task_failure::{
    document, given_table, item, ALL_ERRORS_WILDCARD, ERROR_ATTRIBUTE, FUNCTION_TIMEOUT_SECONDS,
    KEY, RESERVED, STATUS_ATTRIBUTE, TABLE, TASK_FAILED_WILDCARD, UNRELATED_ERROR,
};
use crate::state_machine_lifecycle::string;

/// The attribute the compensating handler records the MESSAGE of the run's own error
/// document under, beside the error name the suite it borrows
/// [`ERROR_ATTRIBUTE`] from records.
const MESSAGE_ATTRIBUTE: &str = "message";

/// The `Resource` of the optimized DynamoDB `PutItem` integration the reserve step is,
/// and the one of the optimized Lambda integration, which names the service and the
/// action rather than the function.
const PUT_ITEM_RESOURCE: &str = "arn:aws:states:::dynamodb:putItem";
/// See [`PUT_ITEM_RESOURCE`].
const INVOKE_RESOURCE: &str = "arn:aws:states:::lambda:invoke";

/// The class the charge handler raises, and the message it raises it with for one order.
const CHARGE_ERROR_TYPE: &str = "ChargeDeclined";

/// The states the saga is made of — written here because the rows assert which of them an
/// execution entered.
const RESERVE_STATE: &str = "Reserve";
/// See [`RESERVE_STATE`].
const CHARGE_STATE: &str = "Charge";
/// See [`RESERVE_STATE`].
const COMPENSATE_STATE: &str = "Compensate";
/// See [`RESERVE_STATE`].
const CONFIRM_STATE: &str = "Confirm";
/// See [`RESERVE_STATE`].
const ROLLED_BACK_STATE: &str = "RolledBack";

/// The charge step: it raises for the order the document names, unless that document asks
/// it to charge — so what the task reports is the run's own error document or the run's
/// own answer.
const CHARGE_HANDLER: &str = r#"
class ChargeDeclined(Exception):
    pass


def handler(event, context):
    order = event["orderId"]
    if event.get("outcome") == "charged":
        return {"orderId": order, "status": "charged"}
    raise ChargeDeclined("the charge for %s was declined" % order)
"#;

/// The compensating step: it RELEASES the reservation the reserve step wrote and records
/// which failure it compensated, out of the error output the catcher placed where its
/// task's `Payload` reads it.
const COMPENSATE_HANDLER: &str = r#"
import json

import boto3

CLIENT = boto3.client("dynamodb")


def handler(event, context):
    raised = json.loads(event["failure"]["Cause"])
    CLIENT.delete_item(TableName=event["table"], Key={"pk": {"S": event["order"]}})
    CLIENT.put_item(
        TableName=event["table"],
        Item={
            "pk": {"S": event["record"]},
            "error": {"S": event["failure"]["Error"]},
            "message": {"S": raised["errorMessage"]},
        },
    )
    return {"compensated": event["order"]}
"#;

/// The message the charge handler raises for `order`, built the way the handler builds it.
fn raised_message(order: &str) -> String {
    format!("the charge for {order} was declined")
}

/// Which form a row's charge task names its function in.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The optimized integration, with the function named by a parameter and the event
    /// built by one.
    Optimized,
    /// The function's own ARN, which names the function itself and hands it the state's
    /// whole effective input as the event.
    FunctionArn,
}

/// How a row's compensation branch ENDS, which is what decides the execution's own
/// outcome once the catcher has transferred to it.
#[derive(Debug, Clone, Copy)]
enum Branch {
    /// The compensating task is the last state of the branch.
    EndsOnTheTask,
    /// The compensating task transitions to a `Succeed` state of its own.
    ThroughASucceedState,
}

/// What one row's charge step ends as, and so what the saga does about it.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// The charge raised and the catcher named that error: the compensation ran.
    Compensated,
    /// The charge raised and no catcher named that error: nothing compensated it.
    Uncaught,
    /// The charge answered, so there is nothing to compensate.
    Charged,
}

/// One row: what the charge task's catcher names and where it places the error output,
/// the `Resource` form that task names its function in, how the compensation branch ends,
/// and the outcome.
struct Row {
    case: &'static str,
    resource: Resource,
    /// The one `ErrorEquals` name the charge state's catcher declares.
    catch_on: &'static str,
    /// The `ResultPath` the catcher declares, or `None` for a catcher declaring none —
    /// whose default is `$`, so the error output IS the document handed on.
    result_path: Option<&'static str>,
    branch: Branch,
    outcome: Outcome,
}

impl Row {
    /// The path the compensating task's `Payload` reads the error output at: the one the
    /// catcher placed it at, or the whole document for a catcher that declared none.
    fn failure_path(&self) -> &'static str {
        self.result_path.unwrap_or("$")
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the catch-all wildcard placing the error output in a member",
            resource: Resource::Optimized,
            catch_on: ALL_ERRORS_WILDCARD,
            result_path: Some("$.error"),
            branch: Branch::EndsOnTheTask,
            outcome: Outcome::Compensated,
        },
        Row {
            case: "the task-failure wildcard placing it two levels down",
            resource: Resource::Optimized,
            catch_on: TASK_FAILED_WILDCARD,
            result_path: Some("$.saga.failure"),
            branch: Branch::ThroughASucceedState,
            outcome: Outcome::Compensated,
        },
        Row {
            case: "a catcher declaring no ResultPath, whose default replaces the document",
            resource: Resource::Optimized,
            catch_on: ALL_ERRORS_WILDCARD,
            result_path: None,
            branch: Branch::EndsOnTheTask,
            outcome: Outcome::Compensated,
        },
        Row {
            case: "a catcher naming the raised class, on a task naming its function by ARN",
            resource: Resource::FunctionArn,
            catch_on: CHARGE_ERROR_TYPE,
            result_path: Some("$.error"),
            branch: Branch::ThroughASucceedState,
            outcome: Outcome::Compensated,
        },
        Row {
            case: "a catcher naming another class, which does not match",
            resource: Resource::Optimized,
            catch_on: UNRELATED_ERROR,
            result_path: Some("$.error"),
            branch: Branch::EndsOnTheTask,
            outcome: Outcome::Uncaught,
        },
        Row {
            case: "a charge step that answers, so no catcher is evaluated at all",
            resource: Resource::Optimized,
            catch_on: ALL_ERRORS_WILDCARD,
            result_path: Some("$.error"),
            branch: Branch::EndsOnTheTask,
            outcome: Outcome::Charged,
        },
    ]
}

/// The saga a row runs: the optimized `PutItem` reserve step, the charge task with the
/// row's one catcher, the compensating task reading the error output where that catcher
/// placed it, and a `Succeed` state on each path.
fn definition(row: &Row, charge: &str, compensate: &str, record: &str) -> Value {
    let mut catcher = json!({ "ErrorEquals": [row.catch_on], "Next": COMPENSATE_STATE });
    if let Some(path) = row.result_path {
        catcher["ResultPath"] = json!(path);
    }
    let mut charge_state = match row.resource {
        Resource::Optimized => json!({
            "Type": "Task",
            "Resource": INVOKE_RESOURCE,
            "Parameters": { "FunctionName": charge, "Payload.$": "$" },
        }),
        Resource::FunctionArn => json!({ "Type": "Task", "Resource": charge }),
    };
    let members = charge_state
        .as_object_mut()
        .expect("a state is an object");
    members.insert("Catch".to_string(), json!([catcher]));
    members.insert("Next".to_string(), json!(CONFIRM_STATE));

    // The order the compensation releases is the one the execution was started for, which
    // the error output does not carry: it is read from the execution's own input through
    // the context object, so the row whose catcher replaced the whole document reaches it
    // too.
    let mut compensate_state = json!({
        "Type": "Task",
        "Resource": INVOKE_RESOURCE,
        "Parameters": {
            "FunctionName": compensate,
            "Payload": {
                "table": TABLE,
                "record": record,
                "order.$": "$$.Execution.Input.orderId",
                "failure.$": row.failure_path(),
            },
        },
    });
    let mut states = json!({
        RESERVE_STATE: {
            "Type": "Task",
            "Resource": PUT_ITEM_RESOURCE,
            "Parameters": {
                "TableName": TABLE,
                "Item": { KEY: { "S.$": "$.orderId" }, STATUS_ATTRIBUTE: { "S": RESERVED } },
            },
            "ResultPath": "$.reserved",
            "Next": CHARGE_STATE,
        },
        CHARGE_STATE: charge_state,
        CONFIRM_STATE: { "Type": "Succeed" },
    });
    let written = states.as_object_mut().expect("the states are an object");
    match row.branch {
        Branch::EndsOnTheTask => {
            compensate_state["End"] = json!(true);
        }
        Branch::ThroughASucceedState => {
            compensate_state["Next"] = json!(ROLLED_BACK_STATE);
            written.insert(ROLLED_BACK_STATE.to_string(), json!({ "Type": "Succeed" }));
        }
    }
    written.insert(COMPENSATE_STATE.to_string(), compensate_state);
    json!({ "StartAt": RESERVE_STATE, "States": states })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_catcher_hands_the_error_output_to_the_compensating_state() {
    let chain = Chain::with_dynamodb();
    given_table(&chain, "the saga's table").await;
    let charge = chain
        .given_function_of("charge-order", CHARGE_HANDLER, Some(FUNCTION_TIMEOUT_SECONDS))
        .await;
    let compensate = chain
        .given_function_of(
            "release-order",
            COMPENSATE_HANDLER,
            Some(FUNCTION_TIMEOUT_SECONDS),
        )
        .await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let order = format!("order-{index}");
        let record = format!("compensated-{index}");
        let charged = matches!(row.outcome, Outcome::Charged);
        let described = chain
            .ran(
                case,
                &format!("saga-catch-{index}"),
                &definition(&row, &charge, &compensate, &record),
                &json!({
                    "orderId": order,
                    "outcome": if charged { "charged" } else { "decline" },
                }),
            )
            .await;
        let reserved = item(&chain, case, &order).await;
        let recorded = item(&chain, case, &record).await;
        let status = described.get("status").and_then(Value::as_str);

        match row.outcome {
            Outcome::Compensated => {
                // What a client reads off the execution: the compensation branch ran to
                // its own end, so the execution SUCCEEDED and its output is the
                // compensating work's own answer.
                assert_eq!(
                    (
                        status,
                        described.get("error").and_then(Value::as_str),
                        described.get("cause").and_then(Value::as_str),
                    ),
                    (Some("SUCCEEDED"), None, None),
                    "case {case:?}: expected the compensated saga to succeed, got {described}"
                );
                let output: Value = document(&string(&described, "output"), case);
                assert_eq!(
                    output.get("Payload"),
                    Some(&json!({ "compensated": order })),
                    "case {case:?}: expected the compensating run's own answer, got {output}"
                );

                // The compensation really ran, and it ran on the error output the catcher
                // placed at the path the row declared: the reservation is released and the
                // record carries the failure's own name and the run's own message.
                assert_eq!(
                    reserved, None,
                    "case {case:?}: expected the reservation to have been released, got \
                     {reserved:?}"
                );
                assert_eq!(
                    recorded,
                    Some(json!({
                        KEY: { "S": record },
                        ERROR_ATTRIBUTE: { "S": CHARGE_ERROR_TYPE },
                        MESSAGE_ATTRIBUTE: { "S": raised_message(&order) },
                    })),
                    "case {case:?}: expected the error output at {:?} to reach the \
                     compensation, got {recorded:?}",
                    row.failure_path()
                );
            }
            Outcome::Uncaught => {
                // Nothing names this error, so the execution fails under the error
                // document's own class and carries that document as its cause...
                assert_eq!(
                    (status, described.get("error").and_then(Value::as_str)),
                    (Some("FAILED"), Some(CHARGE_ERROR_TYPE)),
                    "case {case:?}: expected the failure to stand under \
                     {CHARGE_ERROR_TYPE:?}, got {described}"
                );
                let raised = document(&string(&described, "cause"), case);
                assert_eq!(
                    (
                        raised.get("errorType").and_then(Value::as_str),
                        raised.get("errorMessage").and_then(Value::as_str),
                    ),
                    (
                        Some(CHARGE_ERROR_TYPE),
                        Some(raised_message(&order).as_str())
                    ),
                    "case {case:?}: expected the run's own error document, got {raised}"
                );
                // ...and what the reserve step had already written stays written, with no
                // compensation record beside it.
                assert_eq!(
                    (reserved, recorded),
                    (
                        Some(json!({
                            KEY: { "S": order },
                            STATUS_ATTRIBUTE: { "S": RESERVED },
                        })),
                        None
                    ),
                    "case {case:?}: expected the uncompensated reservation to stand alone"
                );
            }
            Outcome::Charged => {
                // The charge answered, so the catcher is never evaluated: the success path
                // is the one taken and the reservation stands.
                assert_eq!(
                    status,
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the execution to succeed, got {described}"
                );
                assert_eq!(
                    (reserved, recorded),
                    (
                        Some(json!({
                            KEY: { "S": order },
                            STATUS_ATTRIBUTE: { "S": RESERVED },
                        })),
                        None
                    ),
                    "case {case:?}: expected the reservation and no compensation at all"
                );
            }
        }
    }
}
