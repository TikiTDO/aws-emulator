// SPDX-License-Identifier: CC0-1.0

//! `UpdatePlanExecution`: applying one `UpdatePlanExecutionAction` to a plan execution.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! case:
//!
//! 1. **An in-progress execution is what this operation updates.** While the execution
//!    is still running every action is admitted — `pause` stops it, `resume` puts a paused
//!    one back where it stood, a mode switch re-points the rest of its workflow, and an
//!    action asking for what the execution has already reached changes nothing. Once it
//!    has reached a state it never leaves there is no in-progress execution to update, so
//!    every action is refused `IllegalStateException` (HTTP 400) naming that state, and
//!    nothing about the execution changes.
//! 2. **A pause stops the clock, and a resume serves what it still owed.** While an
//!    execution is paused no transition the clock would have made due reaches it, however
//!    far the clock runs on; the resume puts it back where it stood and the remainder of
//!    the transition it was waiting for is measured from that call.
//! 3. **A mode switch re-points the rest of the workflow.** The execution reports the new
//!    mode, and so does every step the clock has NOT reached; a step already reached keeps
//!    the mode it ran in.
//! 4. **A plan or an execution this emulator does not hold.** Either is
//!    `ResourceNotFoundException` (HTTP 404), with the message built from the ARN or the
//!    id the REQUEST carried.
//! 5. **An execution's event timeline is bounded.** Updating one execution over and over
//!    cannot grow this process without limit: the timeline stops at its ceiling, dropping
//!    its oldest event.
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{self, EXECUTION_START_SPAN};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// How long a case waits for a transition the clock owes: the span itself plus enough
/// slack that a loaded test machine does not read the state before it is due.
const SETTLE_WAIT: Duration = Duration::from_millis(250);

/// This one service on a fresh shared engine, plus the engine itself — so a case about
/// the clock can ask the store to apply what it would owe at a later instant instead of
/// sleeping for an hour.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite exercises operation behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Call one operation and return its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("ArcRegionSwitch.{operation}"))
            // The router keys on the signing name in the SigV4 credential scope.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTEST/20260101/us-east-1/arc-region-switch/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Wait for the transition the clock owes now, the way a client would.
    async fn wait_for_the_clock(&self) {
        tokio::time::sleep(EXECUTION_START_SPAN + SETTLE_WAIT).await;
    }

    /// Apply the transitions the clock will owe `ahead` from now.
    ///
    /// The same `settle` every handler runs first, at a later instant: a case about a
    /// window measured in minutes cannot sleep for it, and sleeping would test the test
    /// machine rather than the rule.
    fn settle_ahead(&self, ahead: Duration) {
        state::service_state(&*self.engine)
            .expect("the shared engine holds this service's store")
            .lock()
            .settle(SystemTime::now() + ahead);
    }
}

/// The wire error code of an error response: the `__type` member's shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// One step carrying a manual-approval execution block — the one block type whose work is
/// inside this service — with the window its configuration gives the approval.
///
/// Members from `smithy_query(query="shape", shape="ExecutionApprovalConfiguration",
/// model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `approvalRole` is required and `timeoutMinutes` is `@range(min: 1)` with
/// `@default: 60`.
fn approval_step(name: &str, timeout_minutes: u32) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ManualApproval",
        "executionBlockConfiguration": {
            "executionApprovalConfig": {
                "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                "timeoutMinutes": timeout_minutes,
            }
        }
    })
}

/// One step carrying a Lambda execution block: it invokes another service's function, so
/// this emulator cannot perform it and the execution fails when the clock reaches it.
fn lambda_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "CustomActionLambda",
        "executionBlockConfiguration": {
            "customActionLambdaConfig": {
                "timeoutMinutes": 60,
                "retryIntervalMinutes": 1.0,
                "regionToRun": "activatingRegion",
                "lambdas": [{"arn": format!("arn:aws:lambda:us-west-2:{ACCOUNT}:function:switch")}],
            }
        }
    })
}

/// Create a plan whose `activate` workflow runs `steps`, and return its ARN.
async fn plan_with_steps(emulator: &Emulator, case: &str, name: &str, steps: Vec<Value>) -> String {
    let request = json!({
        "name": name,
        "regions": ["us-east-1", "us-west-2"],
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [{
            "workflowTargetAction": "activate",
            "steps": steps,
        }],
    });
    let (status, created) = emulator.call("CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start an `activate` execution of `plan_arn` in `us-west-2`, in `mode`, and return its
/// id. `mode` is `StartPlanExecutionRequest$mode`, whose modelled default is `graceful`.
async fn start_execution(emulator: &Emulator, case: &str, plan_arn: &str, mode: &str) -> String {
    let (status, started) = emulator
        .call(
            "StartPlanExecution",
            &json!({
                "planArn": plan_arn,
                "targetRegion": "us-west-2",
                "action": "activate",
                "mode": mode,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPlanExecution expected 200, got {status} with {started}"
    );
    started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read one execution back through `GetPlanExecution`.
async fn get_execution(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
) -> Value {
    let (status, got) = emulator
        .call(
            "GetPlanExecution",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPlanExecution expected 200, got {status} with {got}"
    );
    got
}

/// The `executionState` `GetPlanExecution` reports for one execution.
async fn execution_state(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
) -> String {
    get_execution(emulator, case, plan_arn, execution_id).await["executionState"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Call `UpdatePlanExecution` with `action`, and return its status and body.
async fn update(
    emulator: &Emulator,
    plan_arn: &str,
    execution_id: &str,
    action: &str,
    comment: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "planArn": plan_arn,
        "executionId": execution_id,
        "action": action,
    });
    if let Some(comment) = comment {
        request["comment"] = json!(comment);
    }
    emulator.call("UpdatePlanExecution", &request).await
}

/// The `type` of every event `ListPlanExecutionEvents` reports, oldest first.
async fn event_types(emulator: &Emulator, plan_arn: &str, execution_id: &str) -> Vec<String> {
    let (status, page) = emulator
        .call(
            "ListPlanExecutionEvents",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListPlanExecutionEvents expected 200, got {status} with {page}"
    );
    page["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|event| event["type"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `stepMode` of every step `GetPlanExecution` reports, in workflow order.
fn step_modes(execution: &Value) -> Vec<String> {
    execution["stepStates"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| step["stepMode"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Where a case's execution stands when the update reaches it.
#[derive(Clone, Copy)]
enum Standing {
    /// Straight after the start: the clock has not reached the first step.
    Pending,
    /// The clock stopped at a manual-approval step.
    AwaitingApproval,
    /// The clock reached a step this emulator cannot perform, so the execution failed.
    Failed,
    /// The workflow had no step left to reach, so the execution completed.
    Completed,
    /// A `pause` stopped it before the clock reached its first step.
    Paused,
}

impl Standing {
    /// The workflow an execution standing here has to run.
    fn steps(self) -> Vec<Value> {
        match self {
            Standing::Pending | Standing::Paused => vec![approval_step("Approve", 60)],
            Standing::AwaitingApproval => vec![approval_step("Approve", 60)],
            Standing::Failed => vec![lambda_step("Invoke")],
            Standing::Completed => Vec::new(),
        }
    }
}

/// Put a fresh plan's execution into `standing`, in `mode`, and return the plan ARN and
/// the execution id.
async fn standing(
    emulator: &Emulator,
    case: &str,
    standing: Standing,
    mode: &str,
) -> (String, String) {
    let plan_arn = plan_with_steps(emulator, case, "updates", standing.steps()).await;
    let execution_id = start_execution(emulator, case, &plan_arn, mode).await;
    match standing {
        Standing::Pending => {}
        Standing::AwaitingApproval | Standing::Failed | Standing::Completed => {
            emulator.wait_for_the_clock().await;
        }
        Standing::Paused => {
            let (status, body) = update(emulator, &plan_arn, &execution_id, "pause", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the pause that sets the case up expected 200, got {status} \
                 with {body}"
            );
        }
    }
    (plan_arn, execution_id)
}

/// One row of the precondition table.
struct Precondition {
    case: &'static str,
    standing: Standing,
    /// The `ExecutionMode` the execution is started in.
    mode: &'static str,
    /// The `ExecutionState` it stands in when the update reaches it.
    state: &'static str,
    /// The `UpdatePlanExecutionAction` the request carries.
    action: &'static str,
    /// 200 while the execution is still running, 400 once it has ended.
    status: u16,
    /// The `ExecutionState` the execution reports afterwards — unchanged whenever the
    /// action asks for what it has already reached, and on a refusal.
    state_after: &'static str,
    /// The `ExecutionMode` it reports afterwards — only a mode switch moves it.
    mode_after: &'static str,
}

/// RULE 1: an in-progress execution is what this operation updates — every action is
/// admitted while the execution is still running, and refused once it has ended.
#[tokio::test]
async fn an_action_is_admitted_while_the_execution_is_still_running() {
    // The rows differ along what the rule generalises over: the action, the state the
    // execution stands in (pending, waiting for an approval, paused, and both terminal
    // outcomes) and the mode it runs in.
    let cases = [
        Precondition {
            case: "pause an execution the clock has not moved yet",
            standing: Standing::Pending,
            mode: "graceful",
            state: "pending",
            action: "pause",
            status: 200,
            state_after: "pausedByOperator",
            mode_after: "graceful",
        },
        Precondition {
            case: "pause an execution waiting for an approval",
            standing: Standing::AwaitingApproval,
            mode: "graceful",
            state: "pendingManualApproval",
            action: "pause",
            status: 200,
            state_after: "pausedByOperator",
            mode_after: "graceful",
        },
        Precondition {
            case: "pause an already paused execution",
            standing: Standing::Paused,
            mode: "graceful",
            state: "pausedByOperator",
            action: "pause",
            status: 200,
            state_after: "pausedByOperator",
            mode_after: "graceful",
        },
        Precondition {
            case: "pause a failed execution",
            standing: Standing::Failed,
            mode: "ungraceful",
            state: "failed",
            action: "pause",
            status: 400,
            state_after: "failed",
            mode_after: "ungraceful",
        },
        Precondition {
            case: "pause a completed execution",
            standing: Standing::Completed,
            mode: "graceful",
            state: "completed",
            action: "pause",
            status: 400,
            state_after: "completed",
            mode_after: "graceful",
        },
        Precondition {
            case: "resume a paused execution",
            standing: Standing::Paused,
            mode: "graceful",
            state: "pausedByOperator",
            action: "resume",
            status: 200,
            state_after: "pending",
            mode_after: "graceful",
        },
        Precondition {
            case: "resume an execution the clock has not moved yet",
            standing: Standing::Pending,
            mode: "graceful",
            state: "pending",
            action: "resume",
            status: 200,
            state_after: "pending",
            mode_after: "graceful",
        },
        Precondition {
            case: "resume an execution waiting for an approval",
            standing: Standing::AwaitingApproval,
            mode: "graceful",
            state: "pendingManualApproval",
            action: "resume",
            status: 200,
            state_after: "pendingManualApproval",
            mode_after: "graceful",
        },
        Precondition {
            case: "resume a failed execution",
            standing: Standing::Failed,
            mode: "graceful",
            state: "failed",
            action: "resume",
            status: 400,
            state_after: "failed",
            mode_after: "graceful",
        },
        Precondition {
            case: "resume a completed execution",
            standing: Standing::Completed,
            mode: "ungraceful",
            state: "completed",
            action: "resume",
            status: 400,
            state_after: "completed",
            mode_after: "ungraceful",
        },
        Precondition {
            case: "switch a graceful execution to ungraceful",
            standing: Standing::Pending,
            mode: "graceful",
            state: "pending",
            action: "switchToUngraceful",
            status: 200,
            state_after: "pending",
            mode_after: "ungraceful",
        },
        Precondition {
            case: "switch an ungraceful execution to graceful",
            standing: Standing::Pending,
            mode: "ungraceful",
            state: "pending",
            action: "switchToGraceful",
            status: 200,
            state_after: "pending",
            mode_after: "graceful",
        },
        Precondition {
            case: "switch a paused execution, which the clock can still move",
            standing: Standing::Paused,
            mode: "graceful",
            state: "pausedByOperator",
            action: "switchToUngraceful",
            status: 200,
            state_after: "pausedByOperator",
            mode_after: "ungraceful",
        },
        Precondition {
            case: "switch an execution to the mode it already runs",
            standing: Standing::Pending,
            mode: "graceful",
            state: "pending",
            action: "switchToGraceful",
            status: 200,
            state_after: "pending",
            mode_after: "graceful",
        },
        Precondition {
            case: "switch a completed execution",
            standing: Standing::Completed,
            mode: "graceful",
            state: "completed",
            action: "switchToUngraceful",
            status: 400,
            state_after: "completed",
            mode_after: "graceful",
        },
        Precondition {
            case: "switch a failed execution",
            standing: Standing::Failed,
            mode: "ungraceful",
            state: "failed",
            action: "switchToGraceful",
            status: 400,
            state_after: "failed",
            mode_after: "ungraceful",
        },
    ];

    for Precondition {
        case,
        standing: where_it_stands,
        mode,
        state,
        action,
        status: expected_status,
        state_after,
        mode_after,
    } in cases
    {
        let emulator = Emulator::new();
        let (plan_arn, execution_id) = standing(&emulator, case, where_it_stands, mode).await;

        let before = execution_state(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            before, state,
            "case {case:?}: the case stands an execution in {state}, got {before:?}"
        );

        let (status, body) = update(
            &emulator,
            &plan_arn,
            &execution_id,
            action,
            Some("an operator says so"),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {action} expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, "IllegalStateException",
                "case {case:?}: expected IllegalStateException, got {code:?} with {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&execution_id) && message.contains(&plan_arn),
                "case {case:?}: the refusal should name the execution and its plan, got \
                 {message:?}"
            );
            assert!(
                message.contains(state),
                "case {case:?}: the refusal should name the state {state} it was refused \
                 from, got {message:?}"
            );
        } else {
            assert!(
                body.get("__type").is_none(),
                "case {case:?}: {action} should answer the empty response, got {body}"
            );
        }

        let after = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            after["executionState"],
            json!(state_after),
            "case {case:?}: after {action} expected {state_after}, got {}",
            after["executionState"]
        );
        assert_eq!(
            after["mode"],
            json!(mode_after),
            "case {case:?}: after {action} expected mode {mode_after}, got {}",
            after["mode"]
        );
    }
}

/// RULE 2: a pause stops the clock on an execution, however far the clock runs on, and a
/// resume puts it back where it stood with the remainder of what the clock owed it served
/// from that call.
#[tokio::test]
async fn a_pause_stops_the_clock_and_a_resume_serves_what_it_still_owed() {
    let case = "pause and resume around a one-minute approval window";
    // One minute, so the window the pause suspends is far shorter than the hour the
    // clock is run forward by below: whether it closes can only be the pause's doing.
    const WINDOW: Duration = Duration::from_secs(60);
    let emulator = Emulator::new();
    let plan_arn =
        plan_with_steps(&emulator, case, "pause-resume", vec![approval_step("Approve", 1)]).await;
    let execution_id = start_execution(&emulator, case, &plan_arn, "graceful").await;

    // Nothing is waiting on a resume yet, so this one is admitted and changes nothing —
    // the event timeline asserted at the end of this case is what proves it.
    let (status, body) = update(&emulator, &plan_arn, &execution_id, "resume", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a resume with nothing to resume expected 200, got {status} with {body}"
    );

    // Paused before the clock reached the first step: the step it was about to reach is
    // not reached while the pause holds.
    let (status, body) = update(
        &emulator,
        &plan_arn,
        &execution_id,
        "pause",
        Some("holding the switch"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: pause expected 200, got {status} with {body}"
    );
    let paused = get_execution(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        paused["executionState"],
        json!("pausedByOperator"),
        "case {case:?}: expected pausedByOperator, got {}",
        paused["executionState"]
    );
    assert_eq!(
        paused["comment"],
        json!("holding the switch"),
        "case {case:?}: the pause's comment should be what the execution reports, got {}",
        paused["comment"]
    );

    emulator.wait_for_the_clock().await;
    emulator.settle_ahead(Duration::from_secs(3_600));
    let still = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        still, "pausedByOperator",
        "case {case:?}: an hour of clock should reach a paused execution with nothing, got \
         {still:?}"
    );

    // The resume puts it back in the state the pause interrupted, and the clock owes it
    // the step it was about to reach.
    let (status, body) = update(&emulator, &plan_arn, &execution_id, "resume", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: resume expected 200, got {status} with {body}"
    );
    let resumed = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        resumed, "pending",
        "case {case:?}: the resume should restore the state the pause interrupted, got \
         {resumed:?}"
    );
    emulator.wait_for_the_clock().await;
    let reached = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        reached, "pendingManualApproval",
        "case {case:?}: after the resume the clock should reach the approval step, got \
         {reached:?}"
    );

    // Paused again, this time with the approval window running: the window does not close
    // while the pause holds, and the remainder is served from the resume.
    let (status, body) = update(&emulator, &plan_arn, &execution_id, "pause", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the second pause expected 200, got {status} with {body}"
    );
    emulator.settle_ahead(WINDOW * 4);
    let held = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        held, "pausedByOperator",
        "case {case:?}: a paused approval window should not close, got {held:?}"
    );

    let (status, body) = update(&emulator, &plan_arn, &execution_id, "resume", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the second resume expected 200, got {status} with {body}"
    );
    let waiting = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        waiting, "pendingManualApproval",
        "case {case:?}: the resume should restore the approval wait, got {waiting:?}"
    );
    emulator.settle_ahead(WINDOW * 2);
    let timed_out = execution_state(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        timed_out, "planExecutionTimedOut",
        "case {case:?}: the window resumes running from the resume, so it closes undecided, \
         got {timed_out:?}"
    );

    let events = event_types(&emulator, &plan_arn, &execution_id).await;
    assert_eq!(
        events,
        vec![
            "executionPending",
            "executionPaused",
            "executionStarted",
            "stepPendingApproval",
            "executionPaused",
            "executionStarted",
            "executionFailed",
        ],
        "case {case:?}: every pause and resume should be on the timeline, got {events:?}"
    );
}

/// RULE 3: a mode switch re-points the execution and every step the clock has not reached;
/// a step already reached keeps the mode it ran in.
#[tokio::test]
async fn a_mode_switch_re_points_the_steps_the_clock_has_not_reached() {
    let case = "switch twice around the step the clock stops at";
    let emulator = Emulator::new();
    let plan_arn = plan_with_steps(
        &emulator,
        case,
        "mode-switch",
        vec![approval_step("Approve", 60), approval_step("Confirm", 60)],
    )
    .await;
    let execution_id = start_execution(&emulator, case, &plan_arn, "graceful").await;

    let (status, body) = update(
        &emulator,
        &plan_arn,
        &execution_id,
        "switchToUngraceful",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: switchToUngraceful expected 200, got {status} with {body}"
    );
    let switched = get_execution(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        switched["mode"],
        json!("ungraceful"),
        "case {case:?}: expected mode ungraceful, got {}",
        switched["mode"]
    );
    let modes = step_modes(&switched);
    assert_eq!(
        modes,
        vec!["ungraceful", "ungraceful"],
        "case {case:?}: no step is reached yet, so every one takes the new mode, got \
         {modes:?}"
    );

    // The clock reaches the first step, which runs in the mode the switch left behind.
    emulator.wait_for_the_clock().await;
    let (status, body) = update(&emulator, &plan_arn, &execution_id, "switchToGraceful", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: switchToGraceful expected 200, got {status} with {body}"
    );
    let back = get_execution(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        back["mode"],
        json!("graceful"),
        "case {case:?}: expected mode graceful, got {}",
        back["mode"]
    );
    let modes = step_modes(&back);
    assert_eq!(
        modes,
        vec!["ungraceful", "graceful"],
        "case {case:?}: the step the clock reached keeps its mode, the one it has not \
         takes the new one, got {modes:?}"
    );

    let events = event_types(&emulator, &plan_arn, &execution_id).await;
    assert_eq!(
        events,
        vec![
            "executionPending",
            "executionBehaviorChangedToUngraceful",
            "executionStarted",
            "stepPendingApproval",
            "executionBehaviorChangedToGraceful",
        ],
        "case {case:?}: each switch should be on the timeline, got {events:?}"
    );
}

/// RULE 5: an execution's event timeline has a ceiling, so a caller that keeps updating
/// one execution cannot grow this process's memory without limit; at the ceiling the
/// oldest event is dropped and the oldest one kept is unchained.
#[tokio::test]
async fn an_executions_event_timeline_has_a_ceiling() {
    let case = "pause and resume past the event ceiling";
    let emulator = Emulator::new();
    let plan_arn =
        plan_with_steps(&emulator, case, "event-ceiling", vec![approval_step("Approve", 60)]).await;
    let execution_id = start_execution(&emulator, case, &plan_arn, "graceful").await;

    // Each cycle appends one paused and one started event, so this drives the timeline
    // past its ceiling without the execution ever ending.
    let cycles = state::MAX_EVENTS_PER_EXECUTION / 2 + 5;
    for cycle in 0..cycles {
        for action in ["pause", "resume"] {
            let (status, body) = update(&emulator, &plan_arn, &execution_id, action, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {action} in cycle {cycle} expected 200, got {status} with {body}"
            );
        }
    }

    let kept = state::service_state(&*emulator.engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .executions
        .get(&(plan_arn.clone(), execution_id.clone()))
        .map(|execution| execution.events.len())
        .unwrap_or_default();
    assert_eq!(
        kept,
        state::MAX_EVENTS_PER_EXECUTION,
        "case {case:?}: the timeline should stop at its ceiling of {}, got {kept}",
        state::MAX_EVENTS_PER_EXECUTION
    );

    let (status, page) = emulator
        .call(
            "ListPlanExecutionEvents",
            &json!({"planArn": plan_arn, "executionId": execution_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlanExecutionEvents expected 200, got {status} with {page}"
    );
    let oldest = page["items"][0].clone();
    assert!(
        oldest.get("previousEventId").is_none(),
        "case {case:?}: the oldest event kept should chain to nothing, got {oldest}"
    );
    assert_ne!(
        oldest["type"],
        json!("executionPending"),
        "case {case:?}: the event the execution opened with should have been dropped, got \
         {oldest}"
    );
}

/// One row of the not-found table.
struct Missing {
    case: &'static str,
    /// Whether the plan ARN the request carries names the plan this case created.
    known_plan: bool,
    /// Whether the execution id it carries names the execution this case started.
    known_execution: bool,
    action: &'static str,
}

/// RULE 4: a plan or an execution this emulator does not hold is
/// `ResourceNotFoundException` (HTTP 404), built from what the REQUEST carried.
#[tokio::test]
async fn a_plan_or_an_execution_this_emulator_does_not_hold_is_not_found() {
    // The rows differ along which of the two is missing and along the action asked for,
    // since the lookup precedes every one of them.
    let cases = [
        Missing {
            case: "pause against a plan ARN that names no plan",
            known_plan: false,
            known_execution: true,
            action: "pause",
        },
        Missing {
            case: "resume against a plan ARN that names no plan",
            known_plan: false,
            known_execution: false,
            action: "resume",
        },
        Missing {
            case: "switchToUngraceful against a plan ARN that names no plan",
            known_plan: false,
            known_execution: true,
            action: "switchToUngraceful",
        },
        Missing {
            case: "pause against an execution id the plan never started",
            known_plan: true,
            known_execution: false,
            action: "pause",
        },
        Missing {
            case: "resume against an execution id the plan never started",
            known_plan: true,
            known_execution: false,
            action: "resume",
        },
        Missing {
            case: "switchToGraceful against an execution id the plan never started",
            known_plan: true,
            known_execution: false,
            action: "switchToGraceful",
        },
    ];

    for Missing {
        case,
        known_plan,
        known_execution,
        action,
    } in cases
    {
        let emulator = Emulator::new();
        let plan_arn =
            plan_with_steps(&emulator, case, "absent", vec![approval_step("Approve", 60)]).await;
        let execution_id = start_execution(&emulator, case, &plan_arn, "graceful").await;

        // An ARN no plan is stored under, and an id no execution was started under: both
        // satisfy their modelled patterns, so the lookup is what refuses them.
        let absent_plan = format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa");
        let absent_execution = "us-west-2/0123456789abcdef".to_string();
        let asked_plan = if known_plan {
            plan_arn.clone()
        } else {
            absent_plan.clone()
        };
        let asked_execution = if known_execution {
            execution_id.clone()
        } else {
            absent_execution.clone()
        };

        let (status, body) = update(&emulator, &asked_plan, &asked_execution, action, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} with {body}"
        );
        let message = error_message(&body);
        let expected = if known_plan {
            format!("No execution {asked_execution} found for plan {asked_plan}")
        } else {
            format!("Plan not found for ARN: {asked_plan}")
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The refusal left the execution the plan really holds alone.
        let after = execution_state(&emulator, case, &plan_arn, &execution_id).await;
        assert!(
            after == "pending" || after == "pendingManualApproval",
            "case {case:?}: the refused update should change nothing, got {after:?}"
        );
    }
}
