// SPDX-License-Identifier: CC0-1.0

//! The start-to-close budget SWF enforces on a workflow execution ITSELF, end to end
//! over the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: an execution's resolved
//! `executionStartToCloseTimeout` is a DEADLINE the service itself enforces — once that
//! many seconds have passed since the run started, with no decider and no client call
//! involved, the run carries exactly one `WorkflowExecutionTimedOut` event with
//! `timeoutType=START_TO_CLOSE`, reads back `executionStatus=CLOSED` with
//! `closeStatus=TIMED_OUT`, leaves the open listings for the closed ones, and frees its
//! `workflowId` for a new run — while a budget that is NOT yet spent leaves the run
//! exactly as it was.
//!
//! The rows vary what the rule generalises over: the budget's value (the member's
//! published lower bound, a second past it, and one far beyond any wait here), where
//! that budget came from (the request or the workflow type's registered default), and
//! whether the deadline has passed when the run is read.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, merge, seeded, string_at, Emulator,
};

/// `oldestDate` is required on a visibility time filter, and this is the oldest instant
/// its documented Unix-time form can name, so the window excludes nothing.
const EPOCH_SECONDS: f64 = 0.0;

/// The workflow type every run here is started from.
const TYPE: (&str, &str) = ("demo-wf", "1.0");

/// The task list that type registers as its `defaultTaskList`.
const TASK_LIST: &str = "demo-tasks";

/// The slack added to a wait so the deadline is certainly in the past by the time the
/// run is read back. The sweep is driven by the wall clock, so the wait only has to
/// cross the deadline, never match it.
const SLACK: Duration = Duration::from_millis(400);

/// The budget a run whose `executionStartToCloseTimeout` is the member's published
/// lower bound of `0` actually gets: `MIN_OPEN_EXECUTION_SPAN`, the floor that keeps
/// the OPEN run `StartWorkflowExecution` reports observable.
const ZERO_BUDGET: Duration = Duration::from_millis(500);

/// One unattended run, varying the budget and where it came from.
struct BudgetCase {
    name: &'static str,
    /// What the workflow type registers as `defaultExecutionStartToCloseTimeout`.
    registered: &'static str,
    /// What the start request overrides it with, if anything.
    requested: Option<&'static str>,
    /// How long after the start the budget is spent.
    budget: Duration,
}

/// The workflow-type defaults, with `defaultExecutionStartToCloseTimeout` replaced.
fn defaults_with(execution_timeout: &str) -> Value {
    let mut defaults = full_defaults(TASK_LIST, "TERMINATE");
    merge(
        &mut defaults,
        json!({ "defaultExecutionStartToCloseTimeout": execution_timeout }),
    );
    defaults
}

/// The start request for `workflow_id`, overriding the execution budget when the case
/// asks for it.
fn start_body(domain: &str, workflow_id: &str, requested: Option<&str>) -> Value {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": TYPE.0, "version": TYPE.1 },
    });
    if let Some(timeout) = requested {
        merge(&mut body, json!({ "executionStartToCloseTimeout": timeout }));
    }
    body
}

/// Now, in the epoch-seconds form a visibility time filter takes.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now is after the Unix epoch")
        .as_secs_f64()
}

/// Every event of `run_id`'s history, oldest first.
async fn history(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str, run_id: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            case,
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await;
    body.pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected events in {body:?}"))
}

/// The events of `events` whose `eventType` is `event_type`.
fn of_type(events: &[Value], event_type: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.get("eventType").and_then(Value::as_str) == Some(event_type))
        .cloned()
        .collect()
}

/// `DescribeWorkflowExecution` for one run.
async fn describe(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await
}

/// The `workflowId`s one visibility listing reported.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    body.pointer("/executionInfos")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected executionInfos in {body:?}"))
        .iter()
        .map(|info| string_at(info, "/execution/workflowId"))
        .collect()
}

#[tokio::test]
async fn an_unattended_run_is_closed_by_the_service_when_its_budget_is_spent() {
    let cases = [
        BudgetCase {
            name: "the member's published lower bound, on the request",
            registered: "300",
            requested: Some("0"),
            budget: ZERO_BUDGET,
        },
        BudgetCase {
            name: "one second, on the request",
            registered: "300",
            requested: Some("1"),
            budget: Duration::from_secs(1),
        },
        BudgetCase {
            name: "one second, from the workflow type's registered default",
            registered: "1",
            requested: None,
            budget: Duration::from_secs(1),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("timeout-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            TYPE,
            defaults_with(case.registered),
        )
        .await;

        let listed_before = now_seconds();
        let started = emulator
            .ok(
                case.name,
                "StartWorkflowExecution",
                &start_body(&domain, "wf-1", case.requested),
            )
            .await;
        let run_id = string_at(&started, "/runId");

        // Nothing polls and nothing responds: the only thing that acts on this run from
        // here on is the clock.
        tokio::time::sleep(case.budget + SLACK).await;

        let described = describe(&emulator, case.name, &domain, "wf-1", &run_id).await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                string_at(&described, "/executionInfo/closeStatus"),
            ),
            ("CLOSED".to_string(), "TIMED_OUT".to_string()),
            "case {:?}: a run whose start-to-close budget is spent must read back \
             CLOSED/TIMED_OUT, got {described:?}",
            case.name
        );
        let start_timestamp = described
            .pointer("/executionInfo/startTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {:?}: no startTimestamp", case.name));
        let close_timestamp = described
            .pointer("/executionInfo/closeTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {:?}: no closeTimestamp", case.name));
        let elapsed = close_timestamp - start_timestamp;
        // `closeTimestamp` IS the deadline, so the gap between the two instants is the
        // budget itself — in BOTH directions, which `>=` alone did not pin. Both instants
        // travel as epoch seconds in a JSON number, whose resolution around the present
        // epoch is a fraction of a microsecond, so the gap is compared to a tolerance far
        // below the budgets above rather than exactly.
        const INSTANT_TOLERANCE: f64 = 1e-3;
        assert!(
            (elapsed - case.budget.as_secs_f64()).abs() < INSTANT_TOLERANCE,
            "case {:?}: the close instant must be the deadline, i.e. {:?} after the start, \
             got {elapsed} seconds",
            case.name,
            case.budget
        );
        // The run was closed by the clock with its first decision task still scheduled
        // and never answered, so that one task is still outstanding at the instant the
        // run closed and stays counted ("The count of decision tasks whose status is
        // OPEN"); nothing else of the run was ever in flight.
        assert_eq!(
            i64_at(&described, "/openCounts/openDecisionTasks"),
            1,
            "case {:?}: the decision task the run was closed without answering is still \
             counted, got {described:?}",
            case.name
        );
        for counter in [
            "openActivityTasks",
            "openTimers",
            "openChildWorkflowExecutions",
            "openLambdaFunctions",
        ] {
            assert_eq!(
                i64_at(&described, &format!("/openCounts/{counter}")),
                0,
                "case {:?}: this run never had any {counter}, got {described:?}",
                case.name
            );
        }

        let events = history(&emulator, case.name, &domain, "wf-1", &run_id).await;
        let timed_out = of_type(&events, "WorkflowExecutionTimedOut");
        assert_eq!(
            timed_out.len(),
            1,
            "case {:?}: exactly one WorkflowExecutionTimedOut event must be recorded, \
             got {events:?}",
            case.name
        );
        assert_eq!(
            string_at(
                &timed_out[0],
                "/workflowExecutionTimedOutEventAttributes/timeoutType"
            ),
            "START_TO_CLOSE",
            "case {:?}: the only timeoutType an execution-wide timeout carries is \
             START_TO_CLOSE, got {:?}",
            case.name,
            timed_out[0]
        );
        assert_eq!(
            string_at(
                &timed_out[0],
                "/workflowExecutionTimedOutEventAttributes/childPolicy"
            ),
            "TERMINATE",
            "case {:?}: the event carries the run's own resolved childPolicy, got {:?}",
            case.name,
            timed_out[0]
        );
        assert_eq!(
            events.last().and_then(|event| event.get("eventType")),
            Some(&json!("WorkflowExecutionTimedOut")),
            "case {:?}: the timeout is the last thing that happened to the run, \
             got {events:?}",
            case.name
        );

        // Reading again must not record the close a second time: the sweep materialises
        // what the clock already owed, and it owed it once.
        let reread = history(&emulator, case.name, &domain, "wf-1", &run_id).await;
        assert_eq!(
            reread.len(),
            events.len(),
            "case {:?}: re-reading a timed-out run must not append another event, \
             got {reread:?} after {events:?}",
            case.name
        );

        let open = emulator
            .ok(
                case.name,
                "ListOpenWorkflowExecutions",
                &json!({
                    "domain": domain,
                    "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
                }),
            )
            .await;
        assert_eq!(
            listed_ids(case.name, &open),
            Vec::<String>::new(),
            "case {:?}: a timed-out run has left the open listing, got {open:?}",
            case.name
        );
        let closed = emulator
            .ok(
                case.name,
                "ListClosedWorkflowExecutions",
                &json!({
                    "domain": domain,
                    "closeTimeFilter": { "oldestDate": listed_before },
                    "closeStatusFilter": { "status": "TIMED_OUT" },
                }),
            )
            .await;
        assert_eq!(
            listed_ids(case.name, &closed),
            vec!["wf-1".to_string()],
            "case {:?}: a timed-out run is listed among the closed ones under \
             closeStatus TIMED_OUT, got {closed:?}",
            case.name
        );
        let counted = emulator
            .ok(
                case.name,
                "CountOpenWorkflowExecutions",
                &json!({
                    "domain": domain,
                    "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
                }),
            )
            .await;
        assert_eq!(
            i64_at(&counted, "/count"),
            0,
            "case {:?}: a timed-out run is no longer counted open, got {counted:?}",
            case.name
        );
        let pending = emulator
            .ok(
                case.name,
                "CountPendingDecisionTasks",
                &json!({ "domain": domain, "taskList": { "name": TASK_LIST } }),
            )
            .await;
        assert_eq!(
            i64_at(&pending, "/count"),
            0,
            "case {:?}: the decision task of a timed-out run is gone from its task \
             list, got {pending:?}",
            case.name
        );

        // The quota the closed run held is released: the same workflowId starts again.
        let restarted = emulator
            .ok(
                case.name,
                "StartWorkflowExecution",
                &start_body(&domain, "wf-1", Some("300")),
            )
            .await;
        assert_ne!(
            string_at(&restarted, "/runId"),
            run_id,
            "case {:?}: the restart must mint a new runId, got {restarted:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_budget_that_is_not_spent_yet_leaves_the_run_open() {
    // Budgets far longer than the wait below, from each place a budget can come from.
    let cases = [
        BudgetCase {
            name: "a one-second budget, read well before it expires",
            registered: "300",
            requested: Some("1"),
            budget: Duration::from_secs(1),
        },
        BudgetCase {
            name: "a five-minute registered default",
            registered: "300",
            requested: None,
            budget: Duration::from_secs(300),
        },
        BudgetCase {
            name: "the member's one-year maximum",
            registered: "300",
            requested: Some("31536000"),
            budget: Duration::from_secs(31_536_000),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("not-due-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            TYPE,
            defaults_with(case.registered),
        )
        .await;
        let started = emulator
            .ok(
                case.name,
                "StartWorkflowExecution",
                &start_body(&domain, "wf-1", case.requested),
            )
            .await;
        let run_id = string_at(&started, "/runId");

        // A tenth of the shortest budget here: enough wall clock to prove the sweep is
        // not closing runs eagerly, and nowhere near any deadline.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let described = describe(&emulator, case.name, &domain, "wf-1", &run_id).await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {:?}: a run whose budget is not spent yet stays OPEN, got {described:?}",
            case.name
        );
        assert_eq!(
            described.pointer("/executionInfo/closeStatus"),
            None,
            "case {:?}: an OPEN run has no closeStatus, got {described:?}",
            case.name
        );
        assert_eq!(
            i64_at(&described, "/openCounts/openDecisionTasks"),
            1,
            "case {:?}: the run's first decision task is still open, got {described:?}",
            case.name
        );

        let events = history(&emulator, case.name, &domain, "wf-1", &run_id).await;
        assert_eq!(
            of_type(&events, "WorkflowExecutionTimedOut"),
            Vec::<Value>::new(),
            "case {:?}: no timeout event may be recorded before the deadline, \
             got {events:?}",
            case.name
        );
        let open = emulator
            .ok(
                case.name,
                "ListOpenWorkflowExecutions",
                &json!({
                    "domain": domain,
                    "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
                }),
            )
            .await;
        assert_eq!(
            listed_ids(case.name, &open),
            vec!["wf-1".to_string()],
            "case {:?}: the run is still reported open, got {open:?}",
            case.name
        );

        // And its workflowId is still taken, with the fault whose message AWS leaves
        // empty.
        let (status, refused) = emulator
            .call(
                "StartWorkflowExecution",
                &start_body(&domain, "wf-1", case.requested),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "WorkflowExecutionAlreadyStartedFault"),
            "case {:?}: a second start of an open workflowId is refused, got {refused:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_already_started_fault_carries_an_empty_message_and_the_open_run() {
    // The fault's message is empty and its `workflowExecution` IDENTIFIES the run that
    // holds the id: one row per way the colliding start differs from the first, none of
    // which changes either.
    let variants: &[(&str, Option<&str>)] = &[
        ("the identical request", None),
        ("a different execution budget", Some("600")),
        ("the lower-bound budget", Some("0")),
    ];

    let emulator = Emulator::new();
    let domain = "already-started";
    seeded(&emulator, "already started", domain, TYPE, defaults_with("300")).await;
    let started = emulator
        .ok(
            "already started",
            "StartWorkflowExecution",
            &start_body(domain, "wf-1", None),
        )
        .await;
    let run_id = string_at(&started, "/runId");

    for (case, requested) in variants {
        let (status, refused) = emulator
            .call("StartWorkflowExecution", &start_body(domain, "wf-1", *requested))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "WorkflowExecutionAlreadyStartedFault"),
            "case {case:?}: a start colliding with an open run is 400 \
             WorkflowExecutionAlreadyStartedFault, got {refused:?}"
        );
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, "",
            "case {case:?}: the fault's message is empty, got {message:?}"
        );
        assert_eq!(
            refused.get("workflowExecution"),
            Some(&json!({ "workflowId": "wf-1", "runId": run_id })),
            "case {case:?}: the fault names the open run that holds the workflowId, \
             got {refused:?}"
        );
    }

    // Exactly one open run is left behind, and it is the first one.
    let open = emulator
        .ok(
            "already started",
            "ListOpenWorkflowExecutions",
            &json!({
                "domain": domain,
                "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
            }),
        )
        .await;
    assert_eq!(
        listed_ids("already started", &open),
        vec!["wf-1".to_string()],
        "a refused duplicate start leaves exactly one open run, got {open:?}"
    );
    assert_eq!(
        string_at(
            &describe(&emulator, "already started", domain, "wf-1", &run_id).await,
            "/executionInfo/execution/runId"
        ),
        run_id,
        "the run left behind is the one the first start minted"
    );
}
