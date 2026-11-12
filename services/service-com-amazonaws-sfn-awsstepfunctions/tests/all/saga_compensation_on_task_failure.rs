// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the COMPENSATION half of a saga: what a `Task` state's `Catch`
//! does once the work that state asked for FAILED, driven over all three services' own
//! wire protocols through the production `MultiServiceRouter` with Step Functions,
//! Lambda and DynamoDB mounted on ONE engine — so the work function really runs, really
//! writes its item and really raises, and the compensating function's own effect is read
//! back through DynamoDB's own API.
//!
//! RULE: a `Task` state whose work reported a failure is a FAILED state, so the
//! interpreter evaluates that state's `Catch` against the failure's error name — the
//! `errorType` of the error document a Lambda run that raised produced — and the first
//! catcher whose `ErrorEquals` names it (its own name, or either documented wildcard)
//! transfers the execution to that catcher's `Next` instead of the state's success path,
//! handing it the `{Error, Cause}` error output through the catcher's own `ResultPath`.
//! The state the catcher names then performs its own work, side effects included. With
//! no catcher naming the error, nothing of the sort happens: the execution fails under
//! the error document's own class and whatever the failed work had already written stays
//! written.
//!
//! The rows differ along what the rule generalises over: what the catcher's `ErrorEquals`
//! names (the raised class itself, the task-failure wildcard, the catch-all wildcard, and
//! a class that is not the one raised), the class the handler raised (one of its own and
//! one of the language's), the `Resource` form the two tasks name their function in (the
//! optimized integration and the function's own ARN), whether the work raised at all, and
//! the order each row is about — which is the item's key and the value the raised message
//! embeds.
//!
//! The `Resource` form also decides which FAMILY of history events the work's lifecycle is
//! recorded under ([`Resource::failed`]), so each row reads the events its own form
//! selects; the error NAME a catcher is evaluated against is the error document's own
//! class either way, which is what the rule above is about.
//!
//! The harness (`Chain`, `given_function_of`, `ran`, `sfn`, `ddb`) is the one
//! `lambda_task_control_flow` builds for these services, and the history's event types are
//! read through `lambda_function_timeout`'s own reading of them; `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::lambda_function_timeout::event_types;
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;

/// The table both functions act on: the work step reserves an order in it and the
/// compensation releases that reservation.
///
/// `pub(crate)`, not private: `tests/all.rs` is one crate root, so the sibling suite that
/// drives a saga's error output reads this table through [`given_table`] and [`item`]
/// rather than carrying a second copy of either.
pub(crate) const TABLE: &str = "saga-orders";

/// The attribute the table is keyed by. `pub(crate)` for the reason [`TABLE`] gives.
pub(crate) const KEY: &str = "pk";

/// The attribute the work step stamps a reservation with, and the value it stamps there.
/// `pub(crate)` for the reason [`TABLE`] gives.
pub(crate) const STATUS_ATTRIBUTE: &str = "status";
/// See [`STATUS_ATTRIBUTE`].
pub(crate) const RESERVED: &str = "reserved";

/// The attribute the compensation stamps its own record with: the error name it was
/// handed, which is how a row shows the error output reached the compensating state.
/// `pub(crate)` for the reason [`TABLE`] gives.
pub(crate) const ERROR_ATTRIBUTE: &str = "error";

/// How long one invocation of either function is given, as the function's own `Timeout`:
/// well past what a run needs, because what is under test is the control flow around the
/// task rather than how fast an interpreter gets through a handler. `pub(crate)` for the
/// reason [`TABLE`] gives.
pub(crate) const FUNCTION_TIMEOUT_SECONDS: u64 = 30;

/// How long a row waits for a created table to serve items — "You can perform read and
/// write operations only on an ACTIVE table" — before it is a failure.
const TABLE_PATIENCE: Duration = Duration::from_secs(20);

/// How long one look at a table's status sleeps before the next.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The `Resource` of the optimized Lambda integration, which names the service and the
/// action rather than the function; the function is then named by a `FunctionName`
/// parameter.
const OPTIMIZED_RESOURCE: &str = "arn:aws:states:::lambda:invoke";

/// The class the work handler raises when a row asks it for its own exception, and the
/// one it raises when a row asks for one of the language's own.
const CUSTOM_ERROR_TYPE: &str = "OrderNotFulfillable";
/// See [`CUSTOM_ERROR_TYPE`].
const BUILTIN_ERROR_TYPE: &str = "ValueError";

/// The two wildcards the Amazon States Language documents for a catcher: "States.ALL — A
/// wildcard that matches any known error name", and "States.TaskFailed […] matches any
/// known error name except States.Timeout".
pub(crate) const TASK_FAILED_WILDCARD: &str = "States.TaskFailed";
/// See [`TASK_FAILED_WILDCARD`].
pub(crate) const ALL_ERRORS_WILDCARD: &str = "States.ALL";

/// An error name no run below reports, so a catcher naming it matches nothing.
/// `pub(crate)` for the reason [`TABLE`] gives.
pub(crate) const UNRELATED_ERROR: &str = "InventoryUnavailable";

/// The `Error` and `Cause` the saga's own terminal `Fail` state carries, which is what a
/// client reads off an execution the compensation ran in.
const ROLLED_BACK_ERROR: &str = "SagaRolledBack";
/// See [`ROLLED_BACK_ERROR`].
const ROLLED_BACK_CAUSE: &str = "the reservation was released after the work step failed";

/// Where the catcher places the error output in the document it hands on, which is where
/// the compensating handler reads the failure it is compensating.
const FAILURE_MEMBER: &str = "failure";

/// The state the work is done in, the state the catcher routes to, and the two terminal
/// states — written here because the rows assert which of them an execution entered.
const WORK_STATE: &str = "Reserve";
/// See [`WORK_STATE`].
const COMPENSATE_STATE: &str = "Compensate";
/// See [`WORK_STATE`].
const CONFIRM_STATE: &str = "Confirm";
/// See [`WORK_STATE`].
const ROLLED_BACK_STATE: &str = "RolledBack";

/// The work step: it RESERVES the order in the table and then, when the input asks it to,
/// raises — so every row that fails has already written the item its compensation has to
/// release. Which class it raises and what the message says are decided by the input, so
/// the error document is the function's own and its input's.
const WORK_HANDLER: &str = r#"
import boto3

CLIENT = boto3.client("dynamodb")


class OrderNotFulfillable(Exception):
    pass


def handler(event, context):
    order = event["orderId"]
    CLIENT.put_item(
        TableName=event["table"],
        Item={"pk": {"S": order}, "status": {"S": "reserved"}},
    )
    kind = event.get("kind")
    if kind == "custom":
        raise OrderNotFulfillable("order %s cannot be fulfilled" % order)
    if kind == "builtin":
        raise ValueError("order %s is not a fulfillable quantity" % order)
    return {"orderId": order, "status": "reserved"}
"#;

/// The compensating step: it releases the reservation the work step wrote and records
/// WHICH failure it compensated, out of the error output the catcher handed it.
const COMPENSATE_HANDLER: &str = r#"
import boto3

CLIENT = boto3.client("dynamodb")


def handler(event, context):
    order = event["orderId"]
    table = event["table"]
    CLIENT.delete_item(TableName=table, Key={"pk": {"S": order}})
    CLIENT.put_item(
        TableName=table,
        Item={
            "pk": {"S": "compensated-" + order},
            "error": {"S": event["failure"]["Error"]},
        },
    )
    return {"compensated": order}
"#;

/// The key the compensation records its own item under, for the order it released.
fn compensated_key(order: &str) -> String {
    format!("compensated-{order}")
}

/// The message the work handler raises for `order`, built the way the handler builds it.
fn raised_message(error_type: &str, order: &str) -> String {
    if error_type == CUSTOM_ERROR_TYPE {
        format!("order {order} cannot be fulfilled")
    } else {
        format!("order {order} is not a fulfillable quantity")
    }
}

/// Which form a row's `Task` states name their function in.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The optimized integration, with the function named by a parameter.
    Optimized,
    /// The function's own ARN, which names the function itself.
    FunctionArn,
}

impl Resource {
    /// The history event the work a state in THIS form asked for writes when it FAILED,
    /// and the detail member that event carries the failure under.
    ///
    /// `HistoryEventType` names a `Task` state's lifecycle twice over, and which family
    /// records one state's work is decided by what its `Resource` names: a `Resource`
    /// that is the ARN of a Lambda FUNCTION makes the unit of work one invocation of
    /// that function, recorded under the `LambdaFunction*` events, while a `Resource`
    /// naming a service INTEGRATION has one API call as its unit of work, recorded under
    /// the `Task*` events. The error NAME a catcher is evaluated against is the same
    /// either way — it is the error document's own class — which is what this suite's
    /// rule is about, so the rows read the family their form selects rather than one
    /// family's events for both.
    fn failed(self) -> (&'static str, &'static str) {
        match self {
            Self::Optimized => ("TaskFailed", "taskFailedEventDetails"),
            Self::FunctionArn => (
                "LambdaFunctionFailed",
                "lambdaFunctionFailedEventDetails",
            ),
        }
    }

    /// The event that work writes when it ANSWERED, in the same family.
    fn succeeded(self) -> &'static str {
        match self {
            Self::Optimized => "TaskSucceeded",
            Self::FunctionArn => "LambdaFunctionSucceeded",
        }
    }
}

/// One `Task` state invoking `function_arn` in `resource`'s form, carrying `transition` —
/// the `Catch` and `Next` members the row's state declares.
fn task(resource: Resource, function_arn: &str, transition: Value) -> Value {
    let mut state = match resource {
        Resource::Optimized => json!({
            "Type": "Task",
            "Resource": OPTIMIZED_RESOURCE,
            "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
        }),
        Resource::FunctionArn => json!({ "Type": "Task", "Resource": function_arn }),
    };
    let members = state.as_object_mut().expect("a state is an object");
    for (member, value) in transition.as_object().expect("a transition is an object") {
        members.insert(member.clone(), value.clone());
    }
    state
}

/// The saga a row runs: the work task, whose one catcher names `catch_on`, routing to the
/// compensating task and from there to a `Fail` state of its own; and, on the work task's
/// success path, a `Succeed` state.
fn definition(resource: Resource, work: &str, compensate: &str, catch_on: &str) -> Value {
    json!({
        "StartAt": WORK_STATE,
        "States": {
            WORK_STATE: task(
                resource,
                work,
                json!({
                    "Catch": [{
                        "ErrorEquals": [catch_on],
                        "ResultPath": format!("$.{FAILURE_MEMBER}"),
                        "Next": COMPENSATE_STATE,
                    }],
                    "Next": CONFIRM_STATE,
                }),
            ),
            COMPENSATE_STATE: task(resource, compensate, json!({ "Next": ROLLED_BACK_STATE })),
            CONFIRM_STATE: { "Type": "Succeed" },
            ROLLED_BACK_STATE: {
                "Type": "Fail",
                "Error": ROLLED_BACK_ERROR,
                "Cause": ROLLED_BACK_CAUSE,
            },
        },
    })
}

/// What one row's work step ends as, and so what the saga does about it.
enum Outcome {
    /// The work raised and the catcher named that error: the compensation ran.
    Compensated { error_type: &'static str },
    /// The work raised and no catcher named that error: nothing compensated it.
    Uncompensated { error_type: &'static str },
    /// The work returned, so there is nothing to compensate.
    Reserved,
}

/// One row: the `Resource` form, what the work handler is asked to end with, the order
/// the row is about, what its catcher names, and the outcome.
struct Row {
    case: &'static str,
    resource: Resource,
    /// The `kind` the input carries, which selects what the work handler ends with.
    kind: &'static str,
    /// The order the row is about: the item's key, and the value the message embeds.
    order: &'static str,
    /// The one `ErrorEquals` name the work state's catcher declares.
    catch_on: &'static str,
    outcome: Outcome,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a catcher naming the class the handler raised",
            resource: Resource::Optimized,
            kind: "custom",
            order: "order-1",
            catch_on: CUSTOM_ERROR_TYPE,
            outcome: Outcome::Compensated {
                error_type: CUSTOM_ERROR_TYPE,
            },
        },
        Row {
            case: "a catcher naming the task-failure wildcard",
            resource: Resource::Optimized,
            kind: "custom",
            order: "order-2",
            catch_on: TASK_FAILED_WILDCARD,
            outcome: Outcome::Compensated {
                error_type: CUSTOM_ERROR_TYPE,
            },
        },
        Row {
            case: "another error class under the catch-all wildcard",
            resource: Resource::Optimized,
            kind: "builtin",
            order: "order-3",
            catch_on: ALL_ERRORS_WILDCARD,
            outcome: Outcome::Compensated {
                error_type: BUILTIN_ERROR_TYPE,
            },
        },
        Row {
            case: "the same saga whose tasks name the function by its own ARN",
            resource: Resource::FunctionArn,
            kind: "custom",
            order: "order-4",
            catch_on: TASK_FAILED_WILDCARD,
            outcome: Outcome::Compensated {
                error_type: CUSTOM_ERROR_TYPE,
            },
        },
        Row {
            case: "the language's own class named by a catcher of its own",
            resource: Resource::FunctionArn,
            kind: "builtin",
            order: "order-5",
            catch_on: BUILTIN_ERROR_TYPE,
            outcome: Outcome::Compensated {
                error_type: BUILTIN_ERROR_TYPE,
            },
        },
        Row {
            case: "a catcher naming another class, which does not match",
            resource: Resource::Optimized,
            kind: "custom",
            order: "order-6",
            catch_on: UNRELATED_ERROR,
            outcome: Outcome::Uncompensated {
                error_type: CUSTOM_ERROR_TYPE,
            },
        },
        Row {
            case: "a work step that returns, so there is nothing to compensate",
            resource: Resource::Optimized,
            kind: "return",
            order: "order-7",
            catch_on: TASK_FAILED_WILDCARD,
            outcome: Outcome::Reserved,
        },
        Row {
            case: "the same, with the tasks naming the function by its own ARN",
            resource: Resource::FunctionArn,
            kind: "return",
            order: "order-8",
            catch_on: TASK_FAILED_WILDCARD,
            outcome: Outcome::Reserved,
        },
    ]
}

/// The table both handlers act on, waited out until it serves items. `pub(crate)` for the
/// reason [`TABLE`] gives.
pub(crate) async fn given_table(chain: &Chain, case: &str) {
    chain
        .ddb(
            case,
            "CreateTable",
            &json!({
                "TableName": TABLE,
                "KeySchema": [{ "AttributeName": KEY, "KeyType": "HASH" }],
                "AttributeDefinitions": [{ "AttributeName": KEY, "AttributeType": "S" }],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    let deadline = Instant::now() + TABLE_PATIENCE;
    loop {
        let described = chain
            .ddb(case, "DescribeTable", &json!({ "TableName": TABLE }))
            .await;
        if described.pointer("/Table/TableStatus") == Some(&json!("ACTIVE")) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the table must serve items within {TABLE_PATIENCE:?}, last saw {described}"
        );
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// The item stored under `key`, or `None` when the table holds none. `pub(crate)` for the
/// reason [`TABLE`] gives.
pub(crate) async fn item(chain: &Chain, case: &str, key: &str) -> Option<Value> {
    let read = chain
        .ddb(
            case,
            "GetItem",
            &json!({ "TableName": TABLE, "Key": { KEY: { "S": key } } }),
        )
        .await;
    read.get("Item").cloned()
}

/// How many of `events` are an entry into the state called `name`.
fn entries(events: &[Value], name: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.ends_with("StateEntered"))
                && event
                    .pointer("/stateEnteredEventDetails/name")
                    .and_then(Value::as_str)
                    == Some(name)
        })
        .count()
}

/// How many of `events` are of `kind`.
fn count(events: &[Value], kind: &str) -> usize {
    event_types(events)
        .iter()
        .filter(|written| *written == kind)
        .count()
}

/// The `error` and `cause` of the one event of `kind` a run wrote, under `member`.
fn only_failure(events: &[Value], kind: &str, member: &str, case: &str) -> (String, String) {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(kind))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {kind}, got {:?}",
        event_types(events)
    );
    let detail = found[0]
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} on {}", found[0]));
    let read = |name: &str| {
        detail
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {name} on {detail}"))
            .to_string()
    };
    (read("error"), read("cause"))
}

/// The error document `cause` carries, as the run produced it. `pub(crate)` for the reason
/// [`TABLE`] gives.
pub(crate) fn document(cause: &str, case: &str) -> Value {
    serde_json::from_str(cause).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the cause to be the error document, got {error} for {cause:?}")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_tasks_catch_runs_the_compensating_state() {
    let chain = Chain::with_dynamodb();
    given_table(&chain, "the saga's table").await;
    let work = chain
        .given_function_of("reserve-order", WORK_HANDLER, Some(FUNCTION_TIMEOUT_SECONDS))
        .await;
    let compensate = chain
        .given_function_of(
            "release-order",
            COMPENSATE_HANDLER,
            Some(FUNCTION_TIMEOUT_SECONDS),
        )
        .await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            resource,
            kind,
            order,
            catch_on,
            outcome,
        } = row;
        let described = chain
            .ran(
                case,
                &format!("saga-{index}"),
                &definition(resource, &work, &compensate, catch_on),
                &json!({ "orderId": order, "table": TABLE, "kind": kind }),
            )
            .await;
        let answered = chain
            .sfn(
                case,
                "GetExecutionHistory",
                &json!({ "executionArn": string(&described, "executionArn"), "maxResults": 200 }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let reserved = item(&chain, case, order).await;
        let recorded = item(&chain, case, &compensated_key(order)).await;

        match outcome {
            Outcome::Compensated { error_type } => {
                // What a client reads off the execution: the saga's own terminal state,
                // reached through the catcher rather than the work task's success path.
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                        described.get("cause").and_then(Value::as_str),
                    ),
                    (
                        Some("FAILED"),
                        Some(ROLLED_BACK_ERROR),
                        Some(ROLLED_BACK_CAUSE)
                    ),
                    "case {case:?}: expected the saga to end in its own rollback state, got \
                     {described}"
                );

                // The compensation really ran: the reservation the work step wrote is
                // gone, and the record the compensating handler wrote names the failure
                // it was handed.
                assert_eq!(
                    reserved, None,
                    "case {case:?}: expected the reservation to have been released, got \
                     {reserved:?}"
                );
                assert_eq!(
                    recorded,
                    Some(json!({
                        KEY: { "S": compensated_key(order) },
                        ERROR_ATTRIBUTE: { "S": error_type },
                    })),
                    "case {case:?}: expected the compensation to record {error_type:?}, got \
                     {recorded:?}"
                );

                // And the history: the work task failed under the run's own class, the
                // catcher's state was entered exactly once, and the one task that
                // succeeded is the compensating one.
                let (failed, member) = resource.failed();
                let (error, cause) = only_failure(&events, failed, member, case);
                assert_eq!(
                    error, error_type,
                    "case {case:?}: expected the task to fail under {error_type:?}, got {error:?}"
                );
                let raised = document(&cause, case);
                assert_eq!(
                    (
                        raised.get("errorType").and_then(Value::as_str),
                        raised.get("errorMessage").and_then(Value::as_str),
                    ),
                    (
                        Some(error_type),
                        Some(raised_message(error_type, order).as_str())
                    ),
                    "case {case:?}: expected the run's own error document, got {raised}"
                );
                assert_eq!(
                    (
                        entries(&events, WORK_STATE),
                        entries(&events, COMPENSATE_STATE),
                        entries(&events, CONFIRM_STATE),
                        entries(&events, ROLLED_BACK_STATE),
                        count(&events, resource.succeeded()),
                    ),
                    (1, 1, 0, 1, 1),
                    "case {case:?}: expected the catcher's state to run and the success path \
                     not to, got {:?}",
                    event_types(&events)
                );
            }
            Outcome::Uncompensated { error_type } => {
                // Nothing names this error, so the execution fails under the document's
                // own class...
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("FAILED"),
                    "case {case:?}: expected the execution to fail, got {described}"
                );
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(error_type),
                    "case {case:?}: expected the failure to name {error_type:?}, got {described}"
                );
                let raised = document(&string(&described, "cause"), case);
                assert_eq!(
                    (
                        raised.get("errorType").and_then(Value::as_str),
                        raised.get("errorMessage").and_then(Value::as_str),
                    ),
                    (
                        Some(error_type),
                        Some(raised_message(error_type, order).as_str())
                    ),
                    "case {case:?}: expected the run's own error document, got {raised}"
                );
                // ...and what the failed work had already written stays written, with no
                // compensation record beside it.
                assert_eq!(
                    reserved,
                    Some(json!({
                        KEY: { "S": order },
                        STATUS_ATTRIBUTE: { "S": RESERVED },
                    })),
                    "case {case:?}: expected the uncompensated reservation to stand, got \
                     {reserved:?}"
                );
                assert_eq!(
                    (recorded, entries(&events, COMPENSATE_STATE)),
                    (None, 0),
                    "case {case:?}: expected no compensation at all, got {:?}",
                    event_types(&events)
                );
                assert_eq!(
                    count(&events, resource.succeeded()),
                    0,
                    "case {case:?}: expected no task to have succeeded, got {:?}",
                    event_types(&events)
                );
            }
            Outcome::Reserved => {
                // The work returned, so the success path is the one taken and the
                // catcher's state is never entered.
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the execution to succeed, got {described}"
                );
                assert_eq!(
                    reserved,
                    Some(json!({
                        KEY: { "S": order },
                        STATUS_ATTRIBUTE: { "S": RESERVED },
                    })),
                    "case {case:?}: expected the reservation the work step wrote, got \
                     {reserved:?}"
                );
                assert_eq!(
                    (
                        recorded,
                        entries(&events, COMPENSATE_STATE),
                        entries(&events, CONFIRM_STATE),
                        count(&events, resource.succeeded()),
                        count(&events, resource.failed().0),
                    ),
                    (None, 0, 1, 1, 0),
                    "case {case:?}: expected the success path and nothing else, got {:?}",
                    event_types(&events)
                );
            }
        }
    }
}
