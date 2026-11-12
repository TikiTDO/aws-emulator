// SPDX-License-Identifier: CC0-1.0

//! The plan-execution lifecycle: how the clock drives a workflow's steps, how a manual
//! approval is decided, what holds a plan, and the refusals a plan's own state produces.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! case:
//!
//! 1. **Step progression.** The clock, not a read, moves an execution off `pending`, and
//!    the state it moves to is decided by the next step of the workflow the requested
//!    action runs: a manual-approval block stops the execution at `pendingManualApproval`
//!    with that step `pendingApproval`; any other block drives another service's
//!    resources, which this emulator does not run, so the step and the execution fail;
//!    a workflow with no block left to reach completes.
//! 2. **Deciding an approval.** Approving the step the execution is waiting on completes
//!    that step and lets the clock reach the next one; declining cancels the execution.
//!    Any other step — unknown, not reached, already decided — is refused, and the
//!    refusal depends on which of the two it is.
//! 3. **One execution at a time.** While an execution is in a state it can still leave,
//!    a second `StartPlanExecution` and a `DeletePlan` on that plan are both refused
//!    `IllegalStateException` (HTTP 400) and neither the plan nor the execution changes.
//! 4. **A missing plan.** Every operation that looks a plan up by the ARN in the request
//!    answers `ResourceNotFoundException` (HTTP 404) with the same message, built from
//!    that ARN.
//! 5. **A plan's Region pair.** The two Regions of a plan are two DIFFERENT Regions, so
//!    `CreatePlan` refuses a pair that names one Region twice and stores nothing.
//! 6. **The approval window.** An approval nobody decides inside the window its own
//!    execution block configures ends the execution `planExecutionTimedOut`.
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    self, APPROVAL_TIMEOUT_REASON, EXECUTION_START_SPAN, UNSUPPORTED_EXECUTION_REASON,
};
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
/// this emulator cannot perform it.
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

/// One step carrying an ARC routing control execution block: it flips a routing control
/// in Route 53 ARC, which this emulator does not run either.
fn routing_control_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ARCRoutingControl",
        "executionBlockConfiguration": {
            "arcRoutingControlConfig": {
                "timeoutMinutes": 60,
                "regionAndRoutingControls": {
                    "us-west-2": [{
                        "routingControlArn":
                            format!("arn:aws:route53-recovery-control::{ACCOUNT}:controlpanel/abc/routingcontrol/def"),
                        "state": "On",
                    }],
                },
            }
        }
    })
}

/// One step carrying a parallel execution block, which runs the steps it nests.
fn parallel_step(name: &str, nested: Vec<Value>) -> Value {
    json!({
        "name": name,
        "executionBlockType": "Parallel",
        "executionBlockConfiguration": {
            "parallelConfig": {"steps": nested}
        }
    })
}

/// A `CreatePlan` request for a plan whose `activate` workflow runs `steps`.
fn plan_request(name: &str, regions: [&str; 2], steps: Vec<Value>) -> Value {
    json!({
        "name": name,
        "regions": regions,
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [{
            "workflowTargetAction": "activate",
            "steps": steps,
        }],
    })
}

/// Create a plan whose `activate` workflow runs `steps`, and return its ARN.
async fn plan_with_steps(emulator: &Emulator, case: &str, name: &str, steps: Vec<Value>) -> String {
    let (status, created) = emulator
        .call("CreatePlan", &plan_request(name, ["us-east-1", "us-west-2"], steps))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start an `activate` execution of `plan_arn` in `us-west-2` and return its id.
async fn start_execution(emulator: &Emulator, case: &str, plan_arn: &str) -> String {
    let (status, started) = emulator
        .call(
            "StartPlanExecution",
            &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
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
async fn get_execution(emulator: &Emulator, case: &str, plan_arn: &str, execution_id: &str) -> Value {
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

/// One row of the step-progression table.
struct Progression {
    case: &'static str,
    steps: fn() -> Vec<Value>,
    /// The `ExecutionState` the clock moves the execution to.
    state: &'static str,
    /// The `StepStatus` of each step once it has.
    step_statuses: &'static [&'static str],
    /// The `ExecutionEventType`s recorded by then, oldest first.
    events: &'static [&'static str],
}

/// RULE 1: the state the clock moves an execution to is decided by the next step of the
/// workflow it runs — never by the read that observes it.
#[tokio::test]
async fn the_clock_moves_an_execution_to_the_state_its_next_step_decides() {
    // The rows differ along what the rule generalises over: the kind of the first
    // execution block, the number of steps, and where in the list each kind falls.
    let cases = [
        Progression {
            case: "one manual approval",
            steps: || vec![approval_step("Approve", 60)],
            state: "pendingManualApproval",
            step_statuses: &["pendingApproval"],
            events: &["executionPending", "executionStarted", "stepPendingApproval"],
        },
        Progression {
            case: "two manual approvals: only the first is reached",
            steps: || vec![approval_step("Approve", 60), approval_step("Confirm", 15)],
            state: "pendingManualApproval",
            step_statuses: &["pendingApproval", "notStarted"],
            events: &["executionPending", "executionStarted", "stepPendingApproval"],
        },
        Progression {
            case: "a Lambda block this emulator cannot run",
            steps: || vec![lambda_step("Invoke")],
            state: "failed",
            step_statuses: &["failed"],
            events: &["executionPending", "executionStarted", "executionFailed"],
        },
        Progression {
            case: "an ARC routing control block this emulator cannot run",
            steps: || vec![routing_control_step("Flip")],
            state: "failed",
            step_statuses: &["failed"],
            events: &["executionPending", "executionStarted", "executionFailed"],
        },
        Progression {
            case: "a parallel block, whose nested steps this emulator cannot run",
            steps: || vec![parallel_step("Both", vec![approval_step("Approve", 60)])],
            state: "failed",
            step_statuses: &["failed"],
            events: &["executionPending", "executionStarted", "executionFailed"],
        },
        Progression {
            case: "an approval behind a block this emulator cannot run",
            steps: || vec![lambda_step("Invoke"), approval_step("Approve", 60)],
            state: "failed",
            step_statuses: &["failed", "notStarted"],
            events: &["executionPending", "executionStarted", "executionFailed"],
        },
        Progression {
            case: "a workflow that configures no execution block at all",
            steps: Vec::new,
            state: "completed",
            step_statuses: &[],
            events: &["executionPending", "executionStarted", "executionSucceeded"],
        },
    ];

    for Progression {
        case,
        steps,
        state,
        step_statuses,
        events,
    } in cases
    {
        let emulator = Emulator::new();
        let plan_arn = plan_with_steps(&emulator, case, "progression", steps()).await;
        let execution_id = start_execution(&emulator, case, &plan_arn).await;

        // The span is never zero: a caller reading straight after the start still sees
        // the state the start left behind.
        let immediately = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            immediately["executionState"],
            json!("pending"),
            "case {case:?}: straight after the start expected pending, got {}",
            immediately["executionState"]
        );

        emulator.wait_for_the_clock().await;

        let settled = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            settled["executionState"],
            json!(state),
            "case {case:?}: expected executionState {state}, got {}",
            settled["executionState"]
        );
        let got_statuses: Vec<&str> = settled["stepStates"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| step["status"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got_statuses, step_statuses,
            "case {case:?}: expected step statuses {step_statuses:?}, got {got_statuses:?}"
        );
        // A terminal execution carries the instant it ended; one still waiting does not.
        let terminal = state != "pendingManualApproval";
        assert_eq!(
            settled["endTime"].is_number(),
            terminal,
            "case {case:?}: expected endTime present {terminal}, got {}",
            settled["endTime"]
        );

        let got_events = event_types(&emulator, &plan_arn, &execution_id).await;
        assert_eq!(
            got_events, events,
            "case {case:?}: expected the timeline {events:?}, got {got_events:?}"
        );

        // A failure says WHY, rather than leaving a caller to guess.
        if state == "failed" {
            let (_, page) = emulator
                .call(
                    "ListPlanExecutionEvents",
                    &json!({"planArn": plan_arn, "executionId": execution_id}),
                )
                .await;
            let reason = page["items"]
                .as_array()
                .and_then(|items| items.iter().find(|event| event["type"] == json!("executionFailed")))
                .map(|event| event["error"].clone())
                .unwrap_or(Value::Null);
            assert_eq!(
                reason,
                json!(UNSUPPORTED_EXECUTION_REASON),
                "case {case:?}: expected the unsupported-execution reason, got {reason}"
            );
        }

        // Only an execution that has reached a state it never leaves releases its plan.
        let (_, page) = emulator.call("ListPlans", &json!({})).await;
        let active = page["plans"][0]["activePlanExecution"].clone();
        let expected_active = if terminal {
            Value::Null
        } else {
            json!(execution_id)
        };
        assert_eq!(
            active, expected_active,
            "case {case:?}: expected activePlanExecution {expected_active}, got {active}"
        );
    }
}

/// RULE 2: approving the step an execution is waiting on completes that step and lets the
/// clock reach the next one, until no step is left and the execution completes.
#[tokio::test]
async fn approving_each_step_in_turn_walks_the_workflow_to_completed() {
    // The rows differ in how many approvals the workflow chains, which is what the rule
    // generalises over: the walk is per step, not per workflow.
    for approvals in [1usize, 2, 3] {
        let case = format!("{approvals} chained approvals");
        let emulator = Emulator::new();
        let names: Vec<String> = (0..approvals).map(|index| format!("approve-{index}")).collect();
        let steps: Vec<Value> = names
            .iter()
            .map(|name| approval_step(name, 60))
            .collect();
        let plan_arn = plan_with_steps(&emulator, &case, "approvals", steps).await;
        let execution_id = start_execution(&emulator, &case, &plan_arn).await;

        for (index, name) in names.iter().enumerate() {
            emulator.wait_for_the_clock().await;
            let waiting = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
            assert_eq!(
                waiting["executionState"],
                json!("pendingManualApproval"),
                "case {case:?}: before approving {name} expected pendingManualApproval, got {}",
                waiting["executionState"]
            );
            assert_eq!(
                waiting["stepStates"][index]["status"],
                json!("pendingApproval"),
                "case {case:?}: expected step {name} pendingApproval, got {}",
                waiting["stepStates"][index]
            );

            let (status, approved) = emulator
                .call(
                    "ApprovePlanExecutionStep",
                    &json!({
                        "planArn": plan_arn,
                        "executionId": execution_id,
                        "stepName": name,
                        "approval": "approve",
                        "comment": format!("approving {name}"),
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: approving {name} expected 200, got {status} with {approved}"
            );

            // Read the decision back: the step is done and the execution is between
            // steps — the clock, not this call, starts the next one.
            let after = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
            assert_eq!(
                after["stepStates"][index]["status"],
                json!("completed"),
                "case {case:?}: expected step {name} completed, got {}",
                after["stepStates"][index]
            );
            assert!(
                after["stepStates"][index]["endTime"].is_number(),
                "case {case:?}: expected an endTime on the approved step, got {}",
                after["stepStates"][index]
            );
            assert_eq!(
                after["executionState"],
                json!("inProgress"),
                "case {case:?}: straight after approving {name} expected inProgress, got {}",
                after["executionState"]
            );
        }

        emulator.wait_for_the_clock().await;
        let done = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
        assert_eq!(
            done["executionState"],
            json!("completed"),
            "case {case:?}: expected completed once every step was approved, got {}",
            done["executionState"]
        );
        let statuses: Vec<&str> = done["stepStates"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| step["status"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            statuses,
            vec!["completed"; approvals],
            "case {case:?}: expected every step completed, got {statuses:?}"
        );

        // A completed execution releases the plan, so the plan is deletable again.
        let (status, deleted) = emulator.call("DeletePlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeletePlan after completion expected 200, got {status} with {deleted}"
        );
    }
}

/// RULE 2: declining the step an execution is waiting on cancels the execution, FAILS
/// that step — its own negative outcome, which `canceled` does not say — leaves the steps
/// behind it unreached, and releases the plan.
#[tokio::test]
async fn declining_a_step_cancels_the_execution_and_releases_the_plan() {
    let case = "decline the first of two approvals";
    let emulator = Emulator::new();
    let plan_arn = plan_with_steps(
        &emulator,
        case,
        "declined",
        vec![approval_step("Approve", 60), approval_step("Confirm", 60)],
    )
    .await;
    let execution_id = start_execution(&emulator, case, &plan_arn).await;
    emulator.wait_for_the_clock().await;

    let (status, declined) = emulator
        .call(
            "ApprovePlanExecutionStep",
            &json!({
                "planArn": plan_arn,
                "executionId": execution_id,
                "stepName": "Approve",
                "approval": "decline",
                "comment": "not now",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: declining expected 200, got {status} with {declined}"
    );

    let after = get_execution(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        after["executionState"],
        json!("canceled"),
        "case {case:?}: expected canceled, got {}",
        after["executionState"]
    );
    assert_eq!(
        after["stepStates"][0]["status"],
        json!("failed"),
        "case {case:?}: expected the declined step failed, got {}",
        after["stepStates"][0]
    );
    assert_eq!(
        after["stepStates"][1]["status"],
        json!("notStarted"),
        "case {case:?}: expected the step behind it unreached, got {}",
        after["stepStates"][1]
    );

    let events = event_types(&emulator, &plan_arn, &execution_id).await;
    assert_eq!(
        events,
        vec![
            "executionPending",
            "executionStarted",
            "stepPendingApproval",
            "executionCanceled"
        ],
        "case {case:?}: expected the recorded timeline, got {events:?}"
    );

    // A canceled execution is one the plan never returns to, so the plan is free.
    let (status, deleted) = emulator.call("DeletePlan", &json!({"arn": plan_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeletePlan after a decline expected 200, got {status} with {deleted}"
    );
}

/// RULE 2: only the step the execution is actually waiting on can be decided. A step the
/// execution does not run at all is the modelled 404; a step it runs but is not waiting
/// on is the modelled 400.
#[tokio::test]
async fn deciding_a_step_the_execution_is_not_waiting_on_is_refused() {
    let emulator = Emulator::new();
    let plan_arn = plan_with_steps(
        &emulator,
        "refusals",
        "decisions",
        vec![approval_step("Approve", 60), approval_step("Confirm", 60)],
    )
    .await;
    let execution_id = start_execution(&emulator, "refusals", &plan_arn).await;
    emulator.wait_for_the_clock().await;

    // The rows differ along what the rule generalises over: which part of the request
    // names something the execution is not waiting on.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a step name no workflow of the plan carries",
            json!({
                "planArn": plan_arn,
                "executionId": execution_id,
                "stepName": "NoSuchStep",
                "approval": "approve",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an execution the plan never started",
            json!({
                "planArn": plan_arn,
                "executionId": "us-west-2/0123456789abcdef",
                "stepName": "Approve",
                "approval": "approve",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a step the clock has not reached yet",
            json!({
                "planArn": plan_arn,
                "executionId": execution_id,
                "stepName": "Confirm",
                "approval": "approve",
            }),
            400,
            "IllegalStateException",
        ),
    ];
    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = emulator.call("ApprovePlanExecutionStep", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // And the refused decisions changed nothing: the step is still waiting.
    let waiting = get_execution(&emulator, "refusals", &plan_arn, &execution_id).await;
    assert_eq!(
        waiting["executionState"],
        json!("pendingManualApproval"),
        "case \"after the refusals\": expected pendingManualApproval, got {}",
        waiting["executionState"]
    );

    // Deciding the SAME step twice is refused the second time: it is no longer waiting.
    let approve = json!({
        "planArn": plan_arn,
        "executionId": execution_id,
        "stepName": "Approve",
        "approval": "approve",
    });
    let (status, first) = emulator.call("ApprovePlanExecutionStep", &approve).await;
    assert_eq!(
        status, 200,
        "case \"the step that is waiting\": expected 200, got {status} with {first}"
    );
    let (status, second) = emulator.call("ApprovePlanExecutionStep", &approve).await;
    assert_eq!(
        status, 400,
        "case \"the same step twice\": expected 400, got {status} with {second}"
    );
    assert_eq!(
        error_code(&second),
        "IllegalStateException",
        "case \"the same step twice\": expected IllegalStateException, got {second}"
    );
}

/// RULE 3: while an execution is in a state it can still leave, a second start and a
/// delete are both refused — and the refusal names the plan the request named and the
/// execution still running against it.
#[tokio::test]
async fn a_plan_an_execution_still_holds_refuses_a_second_start_and_a_delete() {
    // The rows are the non-terminal states an execution of this emulator can be read in,
    // which is what the rule generalises over: the hold is about the state being one the
    // execution can still leave, not about which one it is.
    let cases: [(&str, usize, bool); 3] = [
        ("pending, straight after the start", 1, false),
        ("pendingManualApproval, waiting for a decision", 1, true),
        ("inProgress, between two approved steps", 2, true),
    ];
    for (case, approvals, settled) in cases {
        let emulator = Emulator::new();
        let steps: Vec<Value> = (0..approvals)
            .map(|index| approval_step(&format!("approve-{index}"), 60))
            .collect();
        let plan_arn = plan_with_steps(&emulator, case, "holder", steps).await;
        let execution_id = start_execution(&emulator, case, &plan_arn).await;
        if settled {
            emulator.wait_for_the_clock().await;
        }
        if approvals == 2 {
            // Approving the first of two leaves the execution between steps.
            let (status, body) = emulator
                .call(
                    "ApprovePlanExecutionStep",
                    &json!({
                        "planArn": plan_arn,
                        "executionId": execution_id,
                        "stepName": "approve-0",
                        "approval": "approve",
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first approval expected 200, got {status} with {body}"
            );
        }

        let before = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        let held_state = before["executionState"].clone();

        let (status, refused_start) = emulator
            .call(
                "StartPlanExecution",
                &json!({"planArn": plan_arn, "targetRegion": "us-east-1", "action": "activate"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second start of a held plan expected 400, got {status} with \
             {refused_start}"
        );
        assert_eq!(
            error_code(&refused_start),
            "IllegalStateException",
            "case {case:?}: expected IllegalStateException, got {refused_start}"
        );
        assert_eq!(
            error_message(&refused_start),
            format!("There is already an execution ongoing for plan {plan_arn}"),
            "case {case:?}: expected the ongoing-execution refusal, got {refused_start}"
        );

        let (status, refused_delete) = emulator.call("DeletePlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a held plan expected 400, got {status} with {refused_delete}"
        );
        assert_eq!(
            error_code(&refused_delete),
            "IllegalStateException",
            "case {case:?}: expected IllegalStateException, got {refused_delete}"
        );
        // An `ExecutionId` is `<region>/<id>` on the wire, and the refusal names the
        // Region and the BARE id once each — a pair a client can feed straight back.
        let bare_execution_id = execution_id
            .split_once('/')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| execution_id.clone());
        assert_eq!(
            error_message(&refused_delete),
            format!("Active executions found for {plan_arn}: us-west-2: {bare_execution_id}"),
            "case {case:?}: expected the active-execution refusal, got {refused_delete}"
        );

        // Neither refusal touched the plan or its one execution.
        let (status, plan) = emulator.call("GetPlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plan after the refusals expected 200, got {status} with {plan}"
        );
        let after = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            after["executionState"], held_state,
            "case {case:?}: expected the execution still {held_state}, got {}",
            after["executionState"]
        );
        let (_, page) = emulator.call("ListPlans", &json!({})).await;
        assert_eq!(
            page["plans"][0]["activePlanExecution"],
            json!(execution_id),
            "case {case:?}: expected the same active execution, got {}",
            page["plans"][0]["activePlanExecution"]
        );
    }
}

/// RULE 4: every operation that looks a plan up by the ARN the request carries answers
/// the same not-found message, built from that ARN — the Region-scoped (`...InRegion`)
/// reads included, which answer it at a status of their own (see
/// [`crate::in_region_unresolvable_plan`]).
#[tokio::test]
async fn a_plan_this_emulator_holds_no_plan_for_is_one_message_built_from_the_arn() {
    let emulator = Emulator::new();
    // A plan that IS stored, so the cases are about the ARN they name rather than about
    // an empty store; and one that was stored and deleted.
    let kept = plan_with_steps(&emulator, "kept", "kept", vec![approval_step("Approve", 60)]).await;
    let deleted =
        plan_with_steps(&emulator, "deleted", "doomed", vec![approval_step("Approve", 60)]).await;
    let (status, body) = emulator.call("DeletePlan", &json!({"arn": deleted})).await;
    assert_eq!(status, 200, "DeletePlan expected 200, got {status} with {body}");

    // The rows differ along what the rule generalises over: which operation looks the
    // plan up, and why the ARN names no plan.
    let arns = [
        (
            "an ARN no plan was ever created under",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
        ),
        ("a plan that was deleted", deleted.clone()),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/kept:zzzzzz"),
        ),
    ];
    for (why, arn) in arns {
        assert_ne!(arn, kept, "case {why:?}: the case must not name the stored plan");
        // The expected status goes with the operation: the Region-agnostic reads answer
        // the status `ResourceNotFoundException`'s `@httpError` names, the Region-scoped
        // one answers the status this service answers an `arn` it cannot resolve from the
        // Region reached with.
        let requests: Vec<(&str, Value, u16)> = vec![
            ("GetPlan", json!({"arn": arn}), 404),
            ("DeletePlan", json!({"arn": arn}), 404),
            (
                "StartPlanExecution",
                json!({"planArn": arn, "targetRegion": "us-west-2", "action": "activate"}),
                404,
            ),
            ("ListRoute53HealthChecks", json!({"arn": arn}), 404),
            (
                "ListRoute53HealthChecksInRegion",
                json!({"arn": arn}),
                state::IN_REGION_NOT_FOUND_STATUS,
            ),
            (
                "GetPlanInRegion",
                json!({"arn": arn}),
                state::IN_REGION_NOT_FOUND_STATUS,
            ),
        ];
        for (operation, request, expected_status) in requests {
            let case = format!("{operation} on {why}");
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("Plan not found for ARN: {arn}"),
                "case {case:?}: expected the request-derived not-found message, got {body}"
            );
        }
    }
}

/// RULE 5: a plan's two Regions are two DIFFERENT Regions, so a pair that names one
/// Region twice is refused — with the service's own FIXED text, which quotes no Region
/// back — and nothing is stored.
#[tokio::test]
async fn create_plan_refuses_a_region_pair_that_names_one_region_twice() {
    // Regions drawn from `Region`'s own pattern `^[a-z]{2}-[a-z-]+-\d+$`
    // (`smithy_query(query="shape", shape="Region", …)`), including one with a
    // multi-word middle segment, so the rule is not tested on one Region's spelling.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-4", "us-gov-west-1"];
    for region in regions {
        let emulator = Emulator::new();
        let case = format!("{region} named twice");
        let (status, body) = emulator
            .call(
                "CreatePlan",
                &plan_request("duplicate", [region, region], vec![approval_step("Approve", 60)]),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            state::DUPLICATE_REGIONS_MESSAGE,
            "case {case:?}: expected the service's fixed duplicate-region text, got {body}"
        );

        // Nothing was stored: the store is still empty.
        let (status, page) = emulator.call("ListPlans", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPlans expected 200, got {status} with {page}"
        );
        assert_eq!(
            page["plans"],
            json!([]),
            "case {case:?}: a refused CreatePlan must store nothing, got {}",
            page["plans"]
        );

        // And the same request with two DIFFERENT Regions is accepted, so the refusal is
        // about the pair and not about the rest of the request.
        let other = if region == "us-west-2" {
            "us-east-1"
        } else {
            "us-west-2"
        };
        let (status, created) = emulator
            .call(
                "CreatePlan",
                &plan_request("distinct", [region, other], vec![approval_step("Approve", 60)]),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a distinct pair expected 200, got {status} with {created}"
        );
        assert_eq!(
            created["plan"]["regions"],
            json!([region, other]),
            "case {case:?}: expected the Regions as given, got {}",
            created["plan"]["regions"]
        );
    }
}

/// RULE 6: an approval nobody decides inside the window its own execution block
/// configures ends the execution `planExecutionTimedOut`, and releases the plan.
#[tokio::test]
async fn an_undecided_approval_times_out_when_its_own_window_closes() {
    // The rows are windows across the member's published range
    // (`ExecutionApprovalConfiguration$timeoutMinutes`, `@range(min: 1)`,
    // `@default: 60`): its minimum, its default, and a day.
    for minutes in [1u32, 60, 1_440] {
        let case = format!("a {minutes}-minute approval window");
        let emulator = Emulator::new();
        let plan_arn =
            plan_with_steps(&emulator, &case, "window", vec![approval_step("Approve", minutes)])
                .await;
        let execution_id = start_execution(&emulator, &case, &plan_arn).await;
        emulator.wait_for_the_clock().await;
        let waiting = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
        assert_eq!(
            waiting["executionState"],
            json!("pendingManualApproval"),
            "case {case:?}: expected pendingManualApproval, got {}",
            waiting["executionState"]
        );

        // Half way through the window nothing has changed: the window is the block's,
        // not a round number this emulator picked.
        let window = Duration::from_secs(u64::from(minutes) * 60);
        emulator.settle_ahead(window / 2);
        let halfway = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
        assert_eq!(
            halfway["executionState"],
            json!("pendingManualApproval"),
            "case {case:?}: half way through the window expected pendingManualApproval, got {}",
            halfway["executionState"]
        );

        // Past its end the execution has timed out.
        emulator.settle_ahead(window + EXECUTION_START_SPAN + Duration::from_secs(60));
        let timed_out = get_execution(&emulator, &case, &plan_arn, &execution_id).await;
        assert_eq!(
            timed_out["executionState"],
            json!("planExecutionTimedOut"),
            "case {case:?}: past the window expected planExecutionTimedOut, got {}",
            timed_out["executionState"]
        );
        assert_eq!(
            timed_out["stepStates"][0]["status"],
            json!("canceled"),
            "case {case:?}: expected the undecided step canceled, got {}",
            timed_out["stepStates"][0]
        );

        let (_, page) = emulator
            .call(
                "ListPlanExecutionEvents",
                &json!({"planArn": plan_arn, "executionId": execution_id}),
            )
            .await;
        let reason = page["items"]
            .as_array()
            .and_then(|items| items.iter().find(|event| event["type"] == json!("executionFailed")))
            .map(|event| event["error"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            reason,
            json!(APPROVAL_TIMEOUT_REASON),
            "case {case:?}: expected the approval-timeout reason, got {reason}"
        );

        // A timed-out execution holds the plan no longer.
        let (status, deleted) = emulator.call("DeletePlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeletePlan after the timeout expected 200, got {status} with {deleted}"
        );
    }
}
