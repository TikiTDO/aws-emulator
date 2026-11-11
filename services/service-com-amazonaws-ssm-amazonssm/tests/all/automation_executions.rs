// SPDX-License-Identifier: CC0-1.0

//! Automation end to end: `StartAutomationExecution` resolving an Automation runbook and
//! persisting the execution it begins, and `GetAutomationExecution` reporting back the
//! execution — and the walk through the runbook's steps — that start persisted.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! An automation is `Pending` until the service picks it up, and a step that waits waits
//! out its own span, so the cases that look PAST either instant move the clock through the
//! service's own state ([`pick_up`], [`finish_waits`]) rather than sleeping for it: what
//! they assert is the walk, not how long a span is.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the branch value, the step action, the wait, the execution mode, the
//! parameter set, the target values, the rate, the Region — rather than the single start
//! the recorded units happened to make.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{automation, ComAmazonawsSsmAmazonssmState};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so an execution served here proves the Region came from the
/// request's SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that an execution is reachable only through the Region it
/// started in.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// A well-formed `AutomationExecutionId` (the modelled length is exactly 36) no execution
/// was ever minted under.
const UNKNOWN_EXECUTION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A well-formed `IdempotencyToken` (the modelled UUID form).
const TOKEN: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A second one, to pin that two tokens name two executions.
const OTHER_TOKEN: &str = "ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A document Systems Manager publishes that IS an Automation runbook, so a start resolves
/// it without anybody creating it (`state::PUBLISHED_DOCUMENTS`).
const PUBLISHED_RUNBOOK: &str = "AWS-ConfigureS3BucketVersioning";

/// A document Systems Manager publishes that is a COMMAND document, so it is no Automation
/// runbook at all.
const PUBLISHED_COMMAND: &str = "AWS-RunShellScript";

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case reads the store through and moves the clock with.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// Create an Automation runbook of `name` with `content`, asserting it succeeded.
async fn runbook(router: &MultiServiceRouter, name: &str, content: &Value) {
    let (status, created) = call(
        router,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": "Automation",
            "Content": content.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the runbook {name} must be created first, got {status} with {created:?}"
    );
}

/// Start an automation, asserting it succeeded, and answering the id it minted.
async fn started(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "StartAutomationExecution", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {response:?}"
    );
    let id = text(&response, "AutomationExecutionId");
    assert_eq!(
        id.chars().count(),
        36,
        "case {case:?}: expected the modelled 36-character execution id, got {id:?}"
    );
    id
}

/// Read one execution back, asserting the read succeeded.
async fn read(router: &MultiServiceRouter, case: &str, execution_id: &str) -> Value {
    let (status, response) = call(
        router,
        "GetAutomationExecution",
        &json!({"AutomationExecutionId": execution_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetAutomationExecution, got {status} with \
         {response:?}"
    );
    response["AutomationExecution"].clone()
}

/// Each step of an execution as `(name, status)`, in the runbook's own order.
fn steps(execution: &Value) -> Vec<(String, String)> {
    execution["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| (text(step, "StepName"), text(step, "StepStatus")))
        .collect()
}

/// One step of an execution, by name.
fn step(execution: &Value, name: &str) -> Value {
    execution["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|step| text(step, "StepName") == name)
        .unwrap_or(Value::Null)
}

/// How many automation executions the store holds, to pin that a refused start persisted
/// nothing.
fn stored(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .automation_executions
        .len()
}

/// An instant well in the past, which is what the clock is moved to below.
fn long_ago() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3_600)
}

/// Let the clock reach the pickup every execution of `region` is waiting for, without
/// touching the waits its steps are in.
///
/// The service owes a started automation ONE transition, a span after the start
/// (`automation::PICKUP_DELAY`); a case that asserts what the walk does moves the clock to
/// it through the store rather than sleeping for it.
fn pick_up(engine: &Arc<engine::InMemoryEngine>, region: &str) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    for ((held, _), execution) in store.automation_executions.iter_mut() {
        if held == region && execution.runnable_at.is_some() {
            execution.runnable_at = Some(long_ago());
        }
    }
}

/// Let the clock reach the end of every wait a step of `region`'s executions is in.
fn finish_waits(engine: &Arc<engine::InMemoryEngine>, region: &str) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    for ((held, _), execution) in store.automation_executions.iter_mut() {
        if held != region {
            continue;
        }
        for step in &mut execution.steps {
            if step.due_at.is_some() {
                step.due_at = Some(long_ago());
            }
        }
    }
}

/// A runbook that branches on one parameter and sleeps for no time at all on each path, so
/// the path the branch takes is the path that ran.
fn branching_runbook() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Takes one path.",
        "parameters": {
            "Path": {"type": "String", "default": "left"},
        },
        "mainSteps": [
            {
                "name": "decide",
                "action": "aws:branch",
                "inputs": {
                    "Choices": [
                        {"NextStep": "left", "Variable": "{{ Path }}", "StringEquals": "left"},
                        {"NextStep": "right", "Variable": "{{ Path }}", "StringEquals": "right"},
                    ],
                    "Default": "neither",
                },
            },
            {"name": "left", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
            {"name": "right", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
            {"name": "neither", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
        ],
    })
}

/// A runbook of one step, whose action and flow members the case picks.
fn one_step_runbook(action: &str, inputs: Value, flow: Value) -> Value {
    let mut step = json!({"name": "only", "action": action, "inputs": inputs});
    for (member, value) in flow.as_object().cloned().unwrap_or_default() {
        step[member] = value;
    }
    json!({"schemaVersion": "0.3", "mainSteps": [step]})
}

/// One row of a walk table: the start to make, the path the walk must take, the value the
/// step's references must resolve to, and the status every step of the runbook must carry
/// afterwards.
struct WalkRow {
    case: &'static str,
    body: Value,
    taken: &'static str,
    resolved: &'static str,
    steps: Vec<(&'static str, &'static str)>,
}

/// A start is answered `Pending` with nothing run, the service picks it up a span later,
/// and the walk then takes the ONE path the branch's own values choose.
#[tokio::test]
async fn an_automation_is_pending_until_it_is_picked_up_and_then_walks_one_path() {
    let (router, engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;

    // One row per branch value: each of the two choices, a value only the default matches,
    // and the value the runbook's own default parameter carries when the start names none.
    let rows = vec![
        WalkRow {
            case: "the first choice",
            body: json!({"DocumentName": "Branching", "Parameters": {"Path": ["left"]}}),
            taken: "left",
            resolved: "left",
            steps: vec![
                ("decide", "Success"),
                ("left", "Success"),
                ("right", "Pending"),
                ("neither", "Pending"),
            ],
        },
        WalkRow {
            case: "the second choice",
            body: json!({"DocumentName": "Branching", "Parameters": {"Path": ["right"]}}),
            taken: "right",
            resolved: "right",
            steps: vec![
                ("decide", "Success"),
                ("left", "Pending"),
                ("right", "Success"),
                ("neither", "Pending"),
            ],
        },
        WalkRow {
            case: "no choice, so the default",
            body: json!({"DocumentName": "Branching", "Parameters": {"Path": ["elsewhere"]}}),
            taken: "neither",
            resolved: "elsewhere",
            steps: vec![
                ("decide", "Success"),
                ("left", "Pending"),
                ("right", "Pending"),
                ("neither", "Success"),
            ],
        },
        WalkRow {
            case: "the parameter the runbook defaults",
            body: json!({"DocumentName": "Branching"}),
            taken: "left",
            resolved: "left",
            steps: vec![
                ("decide", "Success"),
                ("left", "Success"),
                ("right", "Pending"),
                ("neither", "Pending"),
            ],
        },
    ];

    for WalkRow {
        case,
        body,
        taken,
        resolved,
        steps: expected,
    } in rows
    {
        let id = started(&router, case, &body).await;

        // Answered, and nothing has run: the execution is `Pending`, every step with it.
        let pending = read(&router, case, &id).await;
        assert_eq!(
            text(&pending, "AutomationExecutionStatus"),
            "Pending",
            "case {case:?}: expected a started automation to be Pending, got {pending}"
        );
        assert!(
            steps(&pending)
                .iter()
                .all(|(_, status)| status == "Pending"),
            "case {case:?}: expected no step of a Pending automation to have run, got {pending}"
        );
        assert!(
            pending["StepExecutions"][0]["ExecutionStartTime"].is_null()
                && pending["ExecutionEndTime"].is_null(),
            "case {case:?}: expected no execution window for work nothing has run, got {pending}"
        );

        pick_up(&engine, REGION);
        let walked = read(&router, case, &id).await;
        assert_eq!(
            text(&walked, "AutomationExecutionStatus"),
            "Success",
            "case {case:?}: expected the walk to succeed, got {walked}"
        );
        let actual = steps(&walked);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(name, status)| (name.to_string(), status.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected only the {taken:?} path to have run, got {walked}"
        );
        // The step the branch decided reports the values it decided FROM, resolved: the
        // choices carry the parameter's value where the runbook wrote its reference.
        let decided = step(&walked, "decide");
        let choices = decided["Inputs"]["Choices"]
            .as_str()
            .unwrap_or("<no resolved Choices>")
            .to_string();
        assert!(
            choices.contains(resolved) && !choices.contains("{{"),
            "case {case:?}: expected the branch's resolved inputs to carry {resolved:?} in \
             place of the reference, got {decided}"
        );
        assert!(
            !decided["ExecutionStartTime"].is_null() && !decided["ExecutionEndTime"].is_null(),
            "case {case:?}: expected the step that ran to report its window, got {decided}"
        );
        assert!(
            !walked["ExecutionEndTime"].is_null(),
            "case {case:?}: expected an ended execution to report when it ended, got {walked}"
        );
    }
}

/// The members a start supplies, defaults and resolves are the members the read reports.
#[tokio::test]
async fn an_execution_reports_what_the_start_supplied_and_what_it_resolved() {
    let (router, engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;

    let rows: Vec<(&str, Value, &str, &str, &str)> = vec![
        (
            "the defaults",
            json!({"DocumentName": "Branching"}),
            "Auto",
            automation::DEFAULT_MAX_CONCURRENCY,
            automation::DEFAULT_MAX_ERRORS,
        ),
        (
            "members the start names",
            json!({
                "DocumentName": "Branching",
                "DocumentVersion": "$LATEST",
                "Mode": "Auto",
                "MaxConcurrency": "2",
                "MaxErrors": "50%",
            }),
            "Auto",
            "2",
            "50%",
        ),
    ];

    for (case, body, mode, concurrency, errors) in rows {
        let id = started(&router, case, &body).await;
        let execution = read(&router, case, &id).await;
        assert_eq!(
            (
                text(&execution, "AutomationExecutionId"),
                text(&execution, "DocumentName"),
                // The version the start RESOLVED, not the selector it named.
                text(&execution, "DocumentVersion"),
                text(&execution, "Mode"),
                text(&execution, "MaxConcurrency"),
                text(&execution, "MaxErrors"),
                text(&execution, "ExecutedBy"),
            ),
            (
                id.clone(),
                "Branching".to_string(),
                "1".to_string(),
                mode.to_string(),
                concurrency.to_string(),
                errors.to_string(),
                service_common::caller_principal::ambient_user_arn(),
            ),
            "case {case:?}: expected the read to report what the start resolved, got {execution}"
        );
        assert!(
            !execution["ExecutionStartTime"].is_null(),
            "case {case:?}: expected the instant the execution started, got {execution}"
        );
        // "The list of execution outputs as defined in the Automation runbook": nothing
        // has produced one, so none is reported.
        assert!(
            execution["Outputs"].is_null() && execution["FailureMessage"].is_null(),
            "case {case:?}: expected no outputs and no failure for a Pending execution, got \
             {execution}"
        );
        let _ = &engine;
    }

    // The parameters a read reports are the ones the START supplied, and a start that
    // supplied none reports none.
    let id = started(
        &router,
        "supplied parameters",
        &json!({"DocumentName": "Branching", "Parameters": {"Path": ["right"]}}),
    )
    .await;
    let execution = read(&router, "supplied parameters", &id).await;
    assert_eq!(
        execution["Parameters"],
        json!({"Path": ["right"]}),
        "case supplied parameters: expected the start's own parameters, got {execution}"
    );
}

/// A step whose action runs where this emulator does not reach runs NOTHING: the step is
/// reported failed, naming the action, and the execution fails with it.
#[tokio::test]
async fn a_step_whose_action_this_emulator_does_not_run_fails_the_execution() {
    let (router, engine) = service();

    // One row per action of a different kind: a script runtime, another service's API, a
    // managed node's agent and a function invocation.
    let rows = [
        (
            "a script runtime",
            "aws:executeScript",
            json!({"Runtime": "python3.11", "Handler": "main", "Script": "def main(e, c): return {}"}),
        ),
        (
            "another service's API",
            "aws:executeAwsApi",
            json!({"Service": "ssm", "Api": "GetParameter", "Name": "/a"}),
        ),
        (
            "a managed node's agent",
            "aws:runCommand",
            json!({"DocumentName": "AWS-RunShellScript", "InstanceIds": ["i-0abc1234"]}),
        ),
        (
            "a function invocation",
            "aws:invokeLambdaFunction",
            json!({"FunctionName": "handler"}),
        ),
    ];

    for (case, action, inputs) in rows {
        let name = format!("Unrunnable-{}", action.replace(':', "-"));
        runbook(
            &router,
            &name,
            &one_step_runbook(action, inputs, json!({})),
        )
        .await;
        let id = started(&router, case, &json!({"DocumentName": name})).await;
        pick_up(&engine, REGION);
        let execution = read(&router, case, &id).await;

        assert_eq!(
            text(&execution, "AutomationExecutionStatus"),
            "Failed",
            "case {case:?}: expected an automation whose step ran nothing to fail, got \
             {execution}"
        );
        let only = step(&execution, "only");
        assert_eq!(
            text(&only, "StepStatus"),
            "Failed",
            "case {case:?}: expected the step to be reported failed, got {only}"
        );
        for (where_it_is, message) in [
            ("the step", text(&only, "FailureMessage")),
            ("the execution", text(&execution, "FailureMessage")),
        ] {
            assert!(
                message.contains(action),
                "case {case:?}: expected {where_it_is} to say which action ran nothing, got \
                 {message:?}"
            );
        }
        // Nothing is reported for work that did not happen: no output, no response code.
        assert!(
            only["Outputs"].is_null() && only["ResponseCode"].is_null(),
            "case {case:?}: expected no output for a step that ran nothing, got {only}"
        );
        // The step's inputs WERE resolved — that happens before the action — so they are
        // reported.
        assert!(
            !only["Inputs"].is_null(),
            "case {case:?}: expected the resolved inputs of the step, got {only}"
        );
    }

    // A published Automation runbook resolves without anybody creating it, and its steps
    // are read the same way.
    let id = started(
        &router,
        "a published runbook",
        &json!({"DocumentName": PUBLISHED_RUNBOOK, "Parameters": {"BucketName": ["logs"]}}),
    )
    .await;
    pick_up(&engine, REGION);
    let execution = read(&router, "a published runbook", &id).await;
    assert_eq!(
        (
            text(&execution, "DocumentName"),
            text(&execution, "AutomationExecutionStatus")
        ),
        (PUBLISHED_RUNBOOK.to_string(), "Failed".to_string()),
        "case a published runbook: expected the published runbook to be walked, got {execution}"
    );
}

/// One row of the failure table: the flow members the failing step declares, the status
/// the execution must end with, and the status every step must carry afterwards.
struct FlowRow {
    case: &'static str,
    flow: Value,
    status: &'static str,
    steps: Vec<(&'static str, &'static str)>,
}

/// What a failed step does to the walk is what its own `onFailure` and `isCritical` say.
#[tokio::test]
async fn a_failed_steps_on_failure_and_criticality_decide_the_walk() {
    let (router, engine) = service();

    // One row per combination the step's own flow members admit: the default (abort), a
    // failure the walk continues past and the automation still fails for, a failure the
    // runbook declares non-critical, and a failure that sends the walk to a named step.
    let rows = vec![
        FlowRow {
            case: "the default, which aborts",
            flow: json!({}),
            status: "Failed",
            steps: vec![("first", "Failed"), ("second", "Pending")],
        },
        FlowRow {
            case: "a critical failure the walk continues past",
            flow: json!({"onFailure": "Continue"}),
            status: "Failed",
            steps: vec![("first", "Failed"), ("second", "Success")],
        },
        FlowRow {
            case: "a failure the runbook declares non-critical",
            flow: json!({"onFailure": "Continue", "isCritical": false}),
            status: "Success",
            steps: vec![("first", "Failed"), ("second", "Success")],
        },
        FlowRow {
            case: "a failure that names the step to go to",
            flow: json!({"onFailure": "step:second"}),
            status: "Failed",
            steps: vec![("first", "Failed"), ("second", "Success")],
        },
    ];

    for FlowRow {
        case,
        flow,
        status: expected_status,
        steps: expected_steps,
    } in rows
    {
        let name = format!(
            "OnFailure-{}",
            case.chars()
                .map(|letter| if letter.is_ascii_alphanumeric() { letter } else { '-' })
                .collect::<String>()
        );
        let mut first = json!({
            "name": "first",
            "action": "aws:executeScript",
            "inputs": {"Script": "nothing runs this"},
        });
        for (member, value) in flow.as_object().cloned().unwrap_or_default() {
            first[member] = value;
        }
        let content = json!({
            "schemaVersion": "0.3",
            "mainSteps": [
                first,
                {"name": "second", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}},
            ],
        });
        runbook(&router, &name, &content).await;
        let id = started(&router, case, &json!({"DocumentName": name})).await;
        pick_up(&engine, REGION);
        let execution = read(&router, case, &id).await;

        assert_eq!(
            text(&execution, "AutomationExecutionStatus"),
            expected_status,
            "case {case:?}: expected the execution to end {expected_status}, got {execution}"
        );
        let actual = steps(&execution);
        let expected: Vec<(String, String)> = expected_steps
            .into_iter()
            .map(|(step, status)| (step.to_string(), status.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the walk the step's flow members lay out, got {execution}"
        );
    }
}

/// An `aws:sleep` step holds the execution `InProgress` until its own span has passed, and
/// a span it cannot read fails the step rather than being waited out for a guess.
#[tokio::test]
async fn a_sleep_step_waits_out_its_own_span() {
    let (router, engine) = service();

    let at = ::aws_smithy_types::DateTime::from(SystemTime::now() + Duration::from_secs(1_800))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders");
    // One row per form of the wait: a span of no length, two spans of different lengths,
    // an instant, and the values that name no wait at all.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a span of no length",
            json!({"Duration": "PT0S"}),
            "Success",
            "Success",
        ),
        (
            "a span of seconds",
            json!({"Duration": "PT30S"}),
            "InProgress",
            "Success",
        ),
        (
            "a span of minutes and seconds",
            json!({"Duration": "PT2M30S"}),
            "InProgress",
            "Success",
        ),
        (
            "an instant",
            json!({"Timestamp": at.clone()}),
            "InProgress",
            "Success",
        ),
        (
            "a span that is no ISO 8601 duration",
            json!({"Duration": "30 seconds"}),
            "Failed",
            "Failed",
        ),
        (
            "neither a span nor an instant",
            json!({"Comment": "nothing to wait for"}),
            "Failed",
            "Failed",
        ),
    ];

    for (case, inputs, straight_away, once_the_wait_is_over) in rows {
        let name = format!("Sleeping-{}", case.replace(' ', "-"));
        runbook(
            &router,
            &name,
            &one_step_runbook("aws:sleep", inputs, json!({})),
        )
        .await;
        let id = started(&router, case, &json!({"DocumentName": name})).await;
        pick_up(&engine, REGION);

        let waiting = read(&router, case, &id).await;
        assert_eq!(
            text(&waiting, "AutomationExecutionStatus"),
            straight_away,
            "case {case:?}: expected the execution to be {straight_away} while the wait is on, \
             got {waiting}"
        );
        if straight_away == "InProgress" {
            assert_eq!(
                (
                    text(&waiting, "CurrentStepName"),
                    text(&waiting, "CurrentAction"),
                    text(&step(&waiting, "only"), "StepStatus"),
                ),
                (
                    "only".to_string(),
                    "aws:sleep".to_string(),
                    "InProgress".to_string()
                ),
                "case {case:?}: expected the step that is waiting to be the current one, got \
                 {waiting}"
            );
            assert!(
                step(&waiting, "only")["ExecutionEndTime"].is_null(),
                "case {case:?}: expected no end time for a step still waiting, got {waiting}"
            );
        }

        finish_waits(&engine, REGION);
        let done = read(&router, case, &id).await;
        assert_eq!(
            text(&done, "AutomationExecutionStatus"),
            once_the_wait_is_over,
            "case {case:?}: expected the execution to be {once_the_wait_is_over} once the wait \
             is over, got {done}"
        );
    }
}

/// An execution waiting for a caller's signal is reported `Waiting`, and nothing of it has
/// run past the step that waits.
#[tokio::test]
async fn an_execution_waiting_for_a_signal_is_reported_waiting() {
    let (router, engine) = service();
    let content = json!({
        "schemaVersion": "0.3",
        "mainSteps": [
            {"name": "hold", "action": "aws:pause", "inputs": {}},
            {"name": "after", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}},
        ],
    });
    runbook(&router, "Pausing", &content).await;

    // One row per way an execution comes to wait: the action that pauses it, and the mode
    // in which every step waits to be started.
    let rows = [
        (
            "a pause step",
            json!({"DocumentName": "Pausing"}),
            vec![("hold", "Waiting"), ("after", "Pending")],
        ),
        (
            "an interactive execution",
            json!({"DocumentName": "Branching", "Mode": "Interactive"}),
            vec![
                ("decide", "Pending"),
                ("left", "Pending"),
                ("right", "Pending"),
                ("neither", "Pending"),
            ],
        ),
    ];
    runbook(&router, "Branching", &branching_runbook()).await;

    for (case, body, expected_steps) in rows {
        let id = started(&router, case, &body).await;
        pick_up(&engine, REGION);
        let execution = read(&router, case, &id).await;
        assert_eq!(
            text(&execution, "AutomationExecutionStatus"),
            "Waiting",
            "case {case:?}: expected the execution to wait for the caller, got {execution}"
        );
        let actual = steps(&execution);
        let expected: Vec<(String, String)> = expected_steps
            .into_iter()
            .map(|(step, status)| (step.to_string(), status.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected nothing past the waiting step to have run, got {execution}"
        );
        assert!(
            execution["ExecutionEndTime"].is_null(),
            "case {case:?}: expected no end time for an execution that has not ended, got \
             {execution}"
        );
    }
}

/// A start that names no runnable runbook, no declared parameter or no resolvable target is
/// refused with the modelled error for it, and persists nothing.
#[tokio::test]
async fn a_start_that_resolves_nothing_is_refused_and_persists_nothing() {
    let (router, engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;
    let content = json!({
        "schemaVersion": "0.3",
        "parameters": {"Node": {"type": "String"}},
        "mainSteps": [{"name": "only", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}}],
    });
    runbook(&router, "Requiring", &content).await;
    let held = stored(&engine);

    // One row per way a start resolves nothing: the runbook, its version, its parameters
    // and its targets.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a name no document carries",
            json!({"DocumentName": "No-Such-Runbook"}),
            "AutomationDefinitionNotFoundException",
            "No-Such-Runbook",
        ),
        (
            "a document that is no Automation runbook",
            json!({"DocumentName": PUBLISHED_COMMAND}),
            "AutomationDefinitionNotFoundException",
            PUBLISHED_COMMAND,
        ),
        (
            "a version the runbook does not hold",
            json!({"DocumentName": "Branching", "DocumentVersion": "7"}),
            "AutomationDefinitionVersionNotFoundException",
            "7",
        ),
        (
            "a parameter the runbook does not declare",
            json!({"DocumentName": "Branching", "Parameters": {"Route": ["left"]}}),
            "InvalidAutomationExecutionParametersException",
            "Route",
        ),
        (
            "a required parameter the start leaves out",
            json!({"DocumentName": "Requiring"}),
            "InvalidAutomationExecutionParametersException",
            "Node",
        ),
        (
            "targets with no target parameter to bind them to",
            json!({
                "DocumentName": "Requiring",
                "Targets": [{"Key": "ParameterValues", "Values": ["a"]}],
            }),
            "InvalidAutomationExecutionParametersException",
            "TargetParameterName",
        ),
        (
            "a target parameter with no targets",
            json!({"DocumentName": "Requiring", "TargetParameterName": "Node"}),
            "InvalidAutomationExecutionParametersException",
            "Targets",
        ),
        (
            "a target parameter the runbook does not declare",
            json!({
                "DocumentName": "Requiring",
                "TargetParameterName": "Absent",
                "Targets": [{"Key": "ParameterValues", "Values": ["a"]}],
            }),
            "InvalidAutomationExecutionParametersException",
            "Absent",
        ),
        (
            "a target key an automation does not resolve by",
            json!({
                "DocumentName": "Requiring",
                "TargetParameterName": "Node",
                "Targets": [{"Key": "colour", "Values": ["blue"]}],
            }),
            "InvalidTarget",
            "colour",
        ),
        (
            "a target that resolves to no resource",
            json!({
                "DocumentName": "Requiring",
                "TargetParameterName": "Node",
                "Targets": [{"Key": "tag:env", "Values": ["dev"]}],
            }),
            "InvalidTarget",
            "Targets",
        ),
    ];

    for (case, body, expected_code, says) in rows {
        let (status, response) = call(&router, "StartAutomationExecution", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} with {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(says),
            "case {case:?}: expected the refusal to name {says:?}, got {message:?}"
        );
        assert_eq!(
            stored(&engine),
            held,
            "case {case:?}: expected a refused start to persist nothing"
        );
    }

    // A read of an id nothing minted is the modelled miss, and so is a read through
    // another Region's endpoint of an execution this one started.
    let id = started(&router, "region isolation", &json!({"DocumentName": "Branching"})).await;
    for (case, region, execution_id) in [
        ("an id nothing minted", REGION, UNKNOWN_EXECUTION_ID),
        ("another Region's endpoint", OTHER_REGION, id.as_str()),
    ] {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetAutomationExecution",
            &json!({"AutomationExecutionId": execution_id}),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "AutomationExecutionNotFoundException"),
            "case {case:?}: expected the modelled miss, got {status} with {response}"
        );
        assert!(
            error_message(&response).contains(execution_id),
            "case {case:?}: expected the refusal to name the id asked for, got {response}"
        );
    }
}

/// A start carrying an idempotency token names ONE execution: the same token and the same
/// members answer the execution it already started, and the same token with other members
/// is the modelled mismatch.
#[tokio::test]
async fn an_idempotency_token_names_one_execution() {
    let (router, engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;

    let start = |token: &str, path: &str| {
        json!({
            "DocumentName": "Branching",
            "ClientToken": token,
            "Parameters": {"Path": [path]},
        })
    };
    let first = started(&router, "the first start", &start(TOKEN, "left")).await;

    // One row per retry: the same call again, the same call with the token spelled in
    // another case ("is case insensitive"), and a second token.
    for (case, body, expected) in [
        ("the same start retried", start(TOKEN, "left"), Some(&first)),
        (
            "the same token in another case",
            start(&TOKEN.to_uppercase(), "left"),
            Some(&first),
        ),
        ("another token", start(OTHER_TOKEN, "left"), None),
    ] {
        let id = started(&router, case, &body).await;
        match expected {
            Some(expected) => assert_eq!(
                &id, expected,
                "case {case:?}: expected the execution the token already started"
            ),
            None => assert_ne!(
                &id, &first,
                "case {case:?}: expected a second token to start its own execution"
            ),
        }
    }
    assert_eq!(
        stored(&engine),
        2,
        "expected one execution per token, however many retries each token made"
    );

    // "the parameters don't match the original call to the API with the same idempotency
    // token": one row per member a retry can differ in.
    for (case, body) in [
        ("other parameters", start(TOKEN, "right")),
        (
            "another runbook",
            json!({"DocumentName": PUBLISHED_RUNBOOK, "ClientToken": TOKEN,
                   "Parameters": {"BucketName": ["logs"]}}),
        ),
        (
            "another rate",
            json!({"DocumentName": "Branching", "ClientToken": TOKEN, "MaxErrors": "4"}),
        ),
        (
            "another target map",
            json!({"DocumentName": "Branching", "ClientToken": TOKEN,
                   "Parameters": {"Path": ["left"]},
                   "TargetMaps": [{"Path": ["left"]}]}),
        ),
        (
            "another target location",
            json!({"DocumentName": "Branching", "ClientToken": TOKEN,
                   "Parameters": {"Path": ["left"]},
                   "TargetLocations": [{"Accounts": ["000000000000"], "Regions": [REGION]}]}),
        ),
    ] {
        let (status, response) = call(&router, "StartAutomationExecution", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "IdempotentParameterMismatch"),
            "case {case:?}: expected the modelled mismatch, got {status} with {response}"
        );
    }
    assert_eq!(
        stored(&engine),
        2,
        "expected a refused retry to persist nothing"
    );
}

/// A rate-controlled start runs ONE execution per resolved target value, each with the
/// target parameter bound to its own value, and the parent's status is their aggregate.
#[tokio::test]
async fn a_rate_controlled_start_runs_one_execution_per_target_value() {
    let (router, engine) = service();
    // The runbook branches on the target parameter, so a value can be made to fail.
    let content = json!({
        "schemaVersion": "0.3",
        "parameters": {"Node": {"type": "String"}},
        "mainSteps": [
            {
                "name": "decide",
                "action": "aws:branch",
                "inputs": {
                    "Choices": [
                        {"NextStep": "unrunnable", "Variable": "{{ Node }}", "StringEquals": "fails"},
                    ],
                    "Default": "wait",
                },
            },
            {"name": "wait", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
            {
                "name": "unrunnable",
                "action": "aws:executeScript",
                "inputs": {"Script": "nothing runs this"},
                "isEnd": true,
            },
        ],
    });
    runbook(&router, "PerTarget", &content).await;

    // One row per shape of the fan-out: one value, several at once, several one at a time,
    // and a set in which one value fails more often than `MaxErrors` allows.
    let rows: Vec<(&str, Value, Vec<&str>, &str)> = vec![
        (
            "one target value",
            json!({
                "DocumentName": "PerTarget",
                "TargetParameterName": "Node",
                "Targets": [{"Key": "ParameterValues", "Values": ["one"]}],
            }),
            vec!["one"],
            "Success",
        ),
        (
            "three values at once",
            json!({
                "DocumentName": "PerTarget",
                "TargetParameterName": "Node",
                "MaxConcurrency": "3",
                "Targets": [{"Key": "ParameterValues", "Values": ["a", "b", "c"]}],
            }),
            vec!["a", "b", "c"],
            "Success",
        ),
        (
            "three values one at a time",
            json!({
                "DocumentName": "PerTarget",
                "TargetParameterName": "Node",
                "MaxConcurrency": "1",
                "Targets": [{"Key": "ParameterValues", "Values": ["d", "e", "f"]}],
            }),
            vec!["d", "e", "f"],
            "Success",
        ),
        (
            "a value that fails",
            json!({
                "DocumentName": "PerTarget",
                "TargetParameterName": "Node",
                "MaxConcurrency": "1",
                "MaxErrors": "0",
                "Targets": [{"Key": "ParameterValues", "Values": ["fails", "never-reached"]}],
            }),
            vec!["fails", "never-reached"],
            "Failed",
        ),
    ];

    for (case, body, values, expected_status) in rows {
        let parent_id = started(&router, case, &body).await;
        pick_up(&engine, REGION);
        // `MaxConcurrency` releases the children in batches, so the walk is read until it
        // settles — each read materialises what the clock owes at that instant.
        let mut parent = read(&router, case, &parent_id).await;
        for _ in 0..values.len() {
            if text(&parent, "AutomationExecutionStatus") != "InProgress" {
                break;
            }
            parent = read(&router, case, &parent_id).await;
        }

        assert_eq!(
            text(&parent, "AutomationExecutionStatus"),
            expected_status,
            "case {case:?}: expected the parent's status to be its children's aggregate, got \
             {parent}"
        );
        assert_eq!(
            parent["ResolvedTargets"]["ParameterValues"],
            json!(values),
            "case {case:?}: expected the values the targets resolved to, got {parent}"
        );
        assert!(
            parent["StepExecutions"].is_null(),
            "case {case:?}: expected a rate-controlled parent to run no step of its own, got \
             {parent}"
        );

        // The children: one per value, each naming its parent and bound to its own value.
        let children: Vec<(String, String, String)> = {
            let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
            let store = state.lock();
            store
                .automation_executions
                .values()
                .filter(|execution| {
                    execution.parent_execution_id.as_deref() == Some(parent_id.as_str())
                })
                .map(|execution| {
                    (
                        execution.execution_id.clone(),
                        execution.target.clone().unwrap_or_default(),
                        execution.status.clone(),
                    )
                })
                .collect()
        };
        let mut targeted: Vec<String> = children.iter().map(|(_, target, _)| target.clone()).collect();
        targeted.sort();
        let mut expected: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        expected.sort();
        assert_eq!(
            targeted, expected,
            "case {case:?}: expected one execution per resolved target value, got {children:?}"
        );
        for (child_id, target, status) in &children {
            let child = read(&router, case, child_id).await;
            assert_eq!(
                (
                    text(&child, "ParentAutomationExecutionId"),
                    text(&child, "Target"),
                    text(&child, "AutomationExecutionStatus"),
                ),
                (parent_id.clone(), target.clone(), status.clone()),
                "case {case:?}: expected the child to report its parent, its target and its \
                 own status, got {child}"
            );
            // The child's walk resolved the target parameter to ITS value, which is the
            // path the branch took for it.
            let taken = if target == "fails" { "unrunnable" } else { "wait" };
            let expected_step = if target == "fails" { "Failed" } else { "Success" };
            if status == "Cancelled" {
                assert!(
                    steps(&child)
                        .iter()
                        .all(|(_, step_status)| step_status == "Pending"),
                    "case {case:?}: expected a cancelled child to have run nothing, got {child}"
                );
                continue;
            }
            assert_eq!(
                text(&step(&child, taken), "StepStatus"),
                expected_step,
                "case {case:?}: expected the child bound to {target:?} to take the {taken:?} \
                 path, got {child}"
            );
        }
        // "the system stops running the automation on additional targets": once more
        // children have failed than `MaxErrors` allows, the rest are stopped rather than
        // left waiting.
        if expected_status == "Failed" {
            assert!(
                children
                    .iter()
                    .any(|(_, _, status)| status == "Cancelled"),
                "case {case:?}: expected the targets past MaxErrors to be cancelled, got \
                 {children:?}"
            );
        }
    }
}

/// The tags a start carries are the tags the tag plane reports for that execution's id.
#[tokio::test]
async fn the_tags_a_start_carries_are_the_tags_the_execution_holds() {
    let (router, _engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;

    let id = started(
        &router,
        "a tagged start",
        &json!({
            "DocumentName": "Branching",
            "Tags": [{"Key": "env", "Value": "dev"}, {"Key": "owner", "Value": "team"}],
        }),
    )
    .await;
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceType": "Automation", "ResourceId": id}),
    )
    .await;
    assert_eq!(
        (status, listed["TagList"].clone()),
        (
            200,
            json!([{"Key": "env", "Value": "dev"}, {"Key": "owner", "Value": "team"}])
        ),
        "case a tagged start: expected the tag plane to report the start's own tags, got \
         {listed}"
    );

    // "you can specify a maximum of 5 tags" when starting an automation, and the tag plane
    // holds an automation to that same maximum: one row per side of it. The start above
    // already carried two, so the row that stays within it adds the rest.
    let within = (0..automation::MAX_TAGS_PER_AUTOMATION - 2)
        .map(|index| json!({"Key": format!("key{index}"), "Value": "v"}))
        .collect::<Vec<Value>>();
    let (status, response) = call(
        &router,
        "AddTagsToResource",
        &json!({"ResourceType": "Automation", "ResourceId": id, "Tags": within}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the tags an automation holds: expected the maximum to be accepted, got {status} \
         with {response}"
    );
    let past = (0..=automation::MAX_TAGS_PER_AUTOMATION)
        .map(|index| json!({"Key": format!("more{index}"), "Value": "v"}))
        .collect::<Vec<Value>>();
    let (status, response) = call(
        &router,
        "AddTagsToResource",
        &json!({"ResourceType": "Automation", "ResourceId": id, "Tags": past}),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "TooManyTagsError"),
        "case more tags than an automation holds: expected the tag plane to refuse them, got \
         {status} with {response}"
    );

    let too_many: Vec<Value> = (0..=automation::MAX_TAGS_PER_AUTOMATION)
        .map(|index| json!({"Key": format!("key{index}"), "Value": "v"}))
        .collect();
    let (status, response) = call(
        &router,
        "StartAutomationExecution",
        &json!({"DocumentName": "Branching", "Tags": too_many}),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case more tags than an automation holds: expected the refusal, got {status} with \
         {response}"
    );
}

/// Resolving a step's references expands the runbook's own content into the values the
/// execution holds, and an execution that expands more than its budget allows fails the
/// step that ran over it rather than holding what it produced.
#[tokio::test]
async fn resolving_more_than_the_input_budget_allows_fails_the_step() {
    let (router, engine) = service();
    // Each reference below resolves to the whole of one parameter's values, which the
    // modelled `AutomationParameterValue` admits 512 bytes of, 50 to a list — so a handful
    // of references expand past the budget one execution holds
    // (`runbook::MAX_RESOLVED_INPUT_BYTES`).
    let value = "x".repeat(512);
    let values: Vec<String> = (0..50).map(|_| value.clone()).collect();
    let mut inputs = (0..8)
        .map(|index| (format!("Held{index}"), Value::String("{{ Big }}".to_string())))
        .collect::<serde_json::Map<String, Value>>();
    inputs.insert("Duration".to_string(), Value::String("PT0S".to_string()));
    let content = json!({
        "schemaVersion": "0.3",
        "parameters": {"Big": {"type": "StringList"}},
        "mainSteps": [{"name": "only", "action": "aws:sleep", "inputs": inputs}],
    });
    runbook(&router, "Expanding", &content).await;

    let case = "references that expand past the budget";
    let id = started(
        &router,
        case,
        &json!({"DocumentName": "Expanding", "Parameters": {"Big": values}}),
    )
    .await;
    pick_up(&engine, REGION);
    let execution = read(&router, case, &id).await;
    assert_eq!(
        text(&execution, "AutomationExecutionStatus"),
        "Failed",
        "case {case:?}: expected the execution to fail, got {execution}"
    );
    let message = text(&step(&execution, "only"), "FailureMessage");
    assert!(
        message.contains(&automation::runbook::MAX_RESOLVED_INPUT_BYTES.to_string()),
        "case {case:?}: expected the refusal to name the budget, got {message:?}"
    );

    // The same runbook with a short value resolves well within the budget and runs.
    let case = "references that stay within the budget";
    let id = started(
        &router,
        case,
        &json!({"DocumentName": "Expanding", "Parameters": {"Big": ["one"]}}),
    )
    .await;
    pick_up(&engine, REGION);
    let execution = read(&router, case, &id).await;
    assert_eq!(
        text(&execution, "AutomationExecutionStatus"),
        "Success",
        "case {case:?}: expected the walk to succeed, got {execution}"
    );
}

/// A runbook whose steps this service cannot walk is refused at the start rather than
/// persisted as an execution nothing could run.
#[tokio::test]
async fn a_runbook_with_no_walkable_steps_is_refused_at_the_start() {
    let (router, engine) = service();

    // One row per way the content declares no walk: no steps at all, and flow members
    // naming a step the runbook does not declare.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a runbook with no mainSteps",
            json!({"schemaVersion": "0.3", "description": "Nothing to run."}),
            "mainSteps",
        ),
        (
            "a runbook whose nextStep names no step",
            json!({
                "schemaVersion": "0.3",
                "mainSteps": [{
                    "name": "only",
                    "action": "aws:sleep",
                    "inputs": {"Duration": "PT0S"},
                    "nextStep": "elsewhere",
                }],
            }),
            "elsewhere",
        ),
        (
            "a branch whose choice names no step",
            json!({
                "schemaVersion": "0.3",
                "mainSteps": [{
                    "name": "only",
                    "action": "aws:branch",
                    "inputs": {"Choices": [{"NextStep": "nowhere", "Variable": "a", "StringEquals": "a"}]},
                }],
            }),
            "nowhere",
        ),
    ];

    for (case, content, says) in rows {
        let name = format!("Unwalkable-{}", case.len());
        runbook(&router, &name, &content).await;
        let (status, response) = call(
            &router,
            "StartAutomationExecution",
            &json!({"DocumentName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the start to be refused, got {status} with {response}"
        );
        assert!(
            error_message(&response).contains(says),
            "case {case:?}: expected the refusal to name {says:?}, got {response}"
        );
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: expected a refused start to persist nothing"
        );
    }
}

/// A runbook of one `aws:sleep` step named `step`, so which VERSION of a runbook an
/// execution walked is visible in the walk itself.
fn versioned_runbook(step: &str) -> Value {
    json!({
        "schemaVersion": "0.3",
        "mainSteps": [
            {"name": step, "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
        ],
    })
}

/// Update the runbook `name` with `content`, asserting it succeeded, and answering the
/// version number that update minted.
async fn updated(router: &MultiServiceRouter, name: &str, content: &Value) -> String {
    let (status, response) = call(
        router,
        "UpdateDocument",
        &json!({"Name": name, "Content": content.to_string()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "updating the runbook {name} must succeed, got {status} with {response:?}"
    );
    text(&response["DocumentDescription"], "DocumentVersion")
}

/// Switch the default version of the runbook `name` to `version`, asserting it succeeded.
async fn default_version(router: &MultiServiceRouter, name: &str, version: &str) {
    let (status, response) = call(
        router,
        "UpdateDocumentDefaultVersion",
        &json!({"Name": name, "DocumentVersion": version}),
    )
    .await;
    assert_eq!(
        status, 200,
        "switching {name} to default version {version} must succeed, got {status} with \
         {response:?}"
    );
}

/// One row of the version table: the `DocumentVersion` a start names, the version number
/// the execution must report, and the step only that version of the runbook declares.
struct VersionRow {
    case: &'static str,
    requested: Option<&'static str>,
    reported: &'static str,
    walks: &'static str,
}

/// A start runs the version of the runbook its `DocumentVersion` names — the three
/// spellings the modelled selector admits (`$LATEST` the newest, `$DEFAULT` the document's
/// default version, a number that numbered version), defaulting to `$DEFAULT` when the
/// start names none — reports the version number it resolved, and walks THAT version's own
/// content, whatever a later update does to the document.
///
/// The selector's spellings are the modelled `DocumentVersion` pattern
/// (`^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$`, `StartAutomationExecutionRequest`), so each
/// row below is one of them rather than the one spelling a recorded start happened to use.
#[tokio::test]
async fn a_start_runs_the_runbook_version_it_names() {
    let (router, engine) = service();
    let name = "Versioned";
    runbook(&router, name, &versioned_runbook("sleepV1")).await;
    for (expected, step) in [("2", "sleepV2"), ("3", "sleepV3")] {
        let minted = updated(&router, name, &versioned_runbook(step)).await;
        assert_eq!(
            minted, expected,
            "case {step:?}: expected the update to mint version {expected}, got {minted:?}"
        );
    }
    // The default version is the one a switch names, not the newest one: that is what
    // tells `$DEFAULT` and `$LATEST` apart below.
    default_version(&router, name, "2").await;

    let rows = [
        VersionRow {
            case: "no version, so the document's default",
            requested: None,
            reported: "2",
            walks: "sleepV2",
        },
        VersionRow {
            case: "the default version by selector",
            requested: Some("$DEFAULT"),
            reported: "2",
            walks: "sleepV2",
        },
        VersionRow {
            case: "the newest version by selector",
            requested: Some("$LATEST"),
            reported: "3",
            walks: "sleepV3",
        },
        VersionRow {
            case: "the first version by number",
            requested: Some("1"),
            reported: "1",
            walks: "sleepV1",
        },
        VersionRow {
            case: "the newest version by number",
            requested: Some("3"),
            reported: "3",
            walks: "sleepV3",
        },
    ];

    for VersionRow {
        case,
        requested,
        reported,
        walks,
    } in rows
    {
        let mut body = json!({"DocumentName": name});
        if let Some(requested) = requested {
            body["DocumentVersion"] = json!(requested);
        }
        let id = started(&router, case, &body).await;
        let pending = read(&router, case, &id).await;
        assert_eq!(
            text(&pending, "DocumentVersion"),
            reported,
            "case {case:?}: expected the execution to report version {reported}, got {pending}"
        );

        pick_up(&engine, REGION);
        let walked = read(&router, case, &id).await;
        assert_eq!(
            text(&walked, "AutomationExecutionStatus"),
            "Success",
            "case {case:?}: expected the walk of version {reported} to succeed, got {walked}"
        );
        assert_eq!(
            steps(&walked),
            vec![(walks.to_string(), "Success".to_string())],
            "case {case:?}: expected the steps version {reported} declares, got {walked}"
        );
        assert_eq!(
            text(&walked, "DocumentVersion"),
            reported,
            "case {case:?}: expected the resolved version to stay reported, got {walked}"
        );
    }

    // A number the document holds no version for names none of its versions: the modelled
    // miss, naming the version asked for, and nothing started. The newest version here is
    // 3, so the rows are the number one past it and one far past it.
    let held = stored(&engine);
    for (case, requested) in [
        ("one past the newest version", "4"),
        ("far past the newest version", "99"),
    ] {
        let (status, refused) = call(
            &router,
            "StartAutomationExecution",
            &json!({"DocumentName": name, "DocumentVersion": requested}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "AutomationDefinitionVersionNotFoundException"),
            "case {case:?}: expected the modelled version miss, got {status} with {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Document {name} version {requested} does not exist."),
            "case {case:?}: expected the refusal to name the runbook and the version asked \
             for, got {refused}"
        );
        assert_eq!(
            stored(&engine),
            held,
            "case {case:?}: expected a refused start to persist nothing"
        );
    }

    // The execution walks the version it RESOLVED at the start, so a version minted after
    // it started is not the content it runs.
    let case = "a version minted after the start";
    let id = started(
        &router,
        case,
        &json!({"DocumentName": name, "DocumentVersion": "$LATEST"}),
    )
    .await;
    let minted = updated(&router, name, &versioned_runbook("sleepV4")).await;
    assert_eq!(
        minted, "4",
        "case {case:?}: expected the update to mint version 4, got {minted:?}"
    );
    pick_up(&engine, REGION);
    let walked = read(&router, case, &id).await;
    assert_eq!(
        text(&walked, "DocumentVersion"),
        "3",
        "case {case:?}: expected the execution to stay on the version it started on, got \
         {walked}"
    );
    assert_eq!(
        steps(&walked),
        vec![("sleepV3".to_string(), "Success".to_string())],
        "case {case:?}: expected the steps of the version it started on, got {walked}"
    );
}
