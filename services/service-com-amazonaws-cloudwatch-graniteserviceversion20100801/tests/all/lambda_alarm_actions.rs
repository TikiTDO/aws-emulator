// SPDX-License-Identifier: CC0-1.0

//! An alarm action that names a LAMBDA FUNCTION invokes that function with the transition, end
//! to end over the wire through the real `MultiServiceRouter` with CloudWatch, AWS Lambda and
//! CloudWatch Logs on one engine — so what a client sees is what is asserted: the line the
//! running handler wrote into the function's own log group, and the `Action` item the alarm's
//! history records.
//!
//! The RULE asserted here: "Lambda actions: Invoke the latest version of a Lambda function:
//! arn:aws:lambda:region:account-id:function:function-name […]"
//! (`PutMetricAlarmInput$AlarmActions`) — when an alarm transitions into a state whose action
//! list names a function, CloudWatch invokes that function with the state change (the alarm's
//! ARN, account and Region beside `alarmData`, which carries the alarm's name and the state it
//! moved to and out of), and the FUNCTION's own resource-based policy decides the invocation: a
//! policy that grants this service `lambda:InvokeFunction` admits it and the alarm's history
//! records the action as executed, while a function that grants it nothing is not invoked at
//! all and the history records the authorization failure instead.
//!
//! The rows differ along what the rule generalises over: the FORM the function's policy takes
//! (no policy at all, an unscoped grant, a grant scoped by `aws:SourceArn` to the firing alarm,
//! a grant scoped to another alarm, a grant of another action) and which action LIST the
//! function is named in — so the transition that fires it is into ALARM for one row and into OK
//! for another.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{rpc, Emulator, REGION};

/// The IAM action an invocation performs, which is what a grant for this service has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The principal AWS has a client grant a Lambda alarm action to.
const ALARM_ACTION_PRINCIPAL: &str = "lambda.alarms.cloudwatch.amazonaws.com";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The role the created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";

/// What the handler prints, which is the line a client reading the function's log group looks
/// for.
const PRINTED_PREFIX: &str = "ALARMED:";

/// How long this suite waits for work another service's clock owes — a created function
/// becoming `Active`, the asynchronous invocation, and the handler's own log delivery — before
/// giving up on it. Generous on purpose: nothing here is asserted about how fast the host runs.
const WORK_WAIT: Duration = Duration::from_secs(60);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// Past the span a created function spends in `Pending` before it is `Active`: waited out by
/// LOOKING at the function's own `State` until that service reports it ready, so the row never
/// depends on how long the host took to get there.
///
/// "During this time, you can't invoke or modify the function" (`CreateFunction`), so an
/// invocation withheld after this is the policy's refusal and not the lifecycle's.
async fn until_active(emulator: &Emulator, case: &str, function: &str) {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, configuration) = lambda(
            emulator,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        if configuration["State"] == json!("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the function {function:?} never became Active, last read {last}");
}

/// The handler the rows deploy: it prints one line naming the alarm and the state it moved to,
/// read out of the document the invocation handed it.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    data = event.get("alarmData") or {}
    state = (data.get("state") or {}).get("value") or "none"
    print("ALARMED:" + str(data.get("alarmName")) + ":" + str(state))
    return {"seen": data.get("alarmName")}
"#;

/// CloudWatch, AWS Lambda and CloudWatch Logs over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the invocation CloudWatch makes reaches Lambda's own
/// handler, which runs the function, and the lines it printed are delivered into the log group
/// this suite reads.
fn emulator_with_lambda() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router, engine }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One AWS Lambda call over its `restJson1` wire, answering its status and parsed body.
async fn lambda(emulator: &Emulator, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = response.into_body();
    let parsed: Value = serde_json::from_slice(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// One CloudWatch Logs call over its `awsJson1_1` wire, answering its status and parsed body.
async fn logs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization("logs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = response.into_body();
    let parsed: Value = serde_json::from_slice(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// One CloudWatch call that must answer 200.
async fn monitoring(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        "monitoring",
        crate::harness::TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// A metric alarm named `name` whose OK and ALARM action lists are the ones given, dimensioned
/// by its own name so one row's alarm is never another row's.
async fn put_metric_alarm(
    emulator: &Emulator,
    name: &str,
    ok_actions: &[&str],
    alarm_actions: &[&str],
) {
    monitoring(
        emulator,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/AlarmActions",
            "MetricName": "Backlog",
            "Dimensions": [{"Name": "QueueName", "Value": name}],
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            // The alarm keeps the state a row forces: "ignore — the current alarm state is
            // maintained" (`PutMetricAlarmInput$TreatMissingData`).
            "TreatMissingData": "ignore",
            "OKActions": ok_actions,
            "AlarmActions": alarm_actions,
        }),
    )
    .await;
}

/// A .zip deployment package holding the handler above as `lambda_function.py`, written in
/// memory the way a caller uploads one and base64-encoded as `FunctionCode$ZipFile` carries it.
fn package() -> String {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
}

/// Create the function an alarm action names, and answer the ARN Lambda assigned it.
async fn create_function(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 20,
            "Code": {"ZipFile": package()},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// Add one statement to the function's resource-based policy through `AddPermission`, which is
/// how a client grants this service the invocation.
async fn add_permission(
    emulator: &Emulator,
    case: &str,
    function: &str,
    action: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": "cloudwatch-alarm-action",
        "Action": action,
        "Principal": ALARM_ACTION_PRINCIPAL,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answered) = lambda(
        emulator,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answered})"
    );
}

/// The lines `FilterLogEvents` reports for the function's own log group — "By default, Lambda
/// functions send logs to a default log group named `/aws/lambda/<function name>`"
/// (`LoggingConfig$LogGroup`).
async fn logged(emulator: &Emulator, function: &str) -> Vec<String> {
    let (status, body) = logs(
        emulator,
        "FilterLogEvents",
        &json!({"logGroupName": format!("/aws/lambda/{function}")}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until the function's log group holds the line the handler prints, and answer every line
/// it holds by then. The wait is for the CONDITION rather than for a span.
async fn until_logged(emulator: &Emulator, function: &str) -> Vec<String> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let lines = logged(emulator, function).await;
        if lines.iter().any(|line| line.contains(PRINTED_PREFIX)) {
            return lines;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    logged(emulator, function).await
}

/// The `Action` history items the alarm `name` holds, as `DescribeAlarmHistory` reports them.
async fn action_items(emulator: &Emulator, name: &str) -> Vec<Value> {
    let body = monitoring(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": "Action",
        }),
    )
    .await;
    body["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Wait until the alarm `name` holds an `Action` item, which is the execution having been
/// accounted for either way.
async fn until_recorded(emulator: &Emulator, name: &str) -> Vec<Value> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let items = action_items(emulator, name).await;
        if !items.is_empty() {
            return items;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    action_items(emulator, name).await
}

/// What one row grants on the function before the alarm moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No `AddPermission` at all, so the function carries no policy.
    None,
    /// The invocation allowed this service, unscoped.
    Unscoped,
    /// The same grant, scoped by `aws:SourceArn` to the alarm that fires the action.
    ThisAlarm,
    /// The same grant, scoped by `aws:SourceArn` to another alarm of the same account.
    AnotherAlarm,
    /// A grant of ANOTHER action, which is no grant of the invocation.
    AnotherAction,
}

/// One row: what the function's policy says, which action list names it, and whether the
/// handler therefore runs.
struct Row {
    case: &'static str,
    grant: Grant,
    /// The state the alarm is forced into, whose action list names the function.
    state: &'static str,
    /// Whether CloudWatch invokes the function, so the handler prints its line.
    invoked: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an unscoped grant, fired by the transition into ALARM",
            grant: Grant::Unscoped,
            state: "ALARM",
            invoked: true,
        },
        Row {
            case: "a grant scoped by SourceArn to the alarm that fires it",
            grant: Grant::ThisAlarm,
            state: "ALARM",
            invoked: true,
        },
        Row {
            case: "an unscoped grant, fired by the transition into OK",
            grant: Grant::Unscoped,
            state: "OK",
            invoked: true,
        },
        Row {
            case: "a function carrying no resource-based policy at all",
            grant: Grant::None,
            state: "ALARM",
            invoked: false,
        },
        Row {
            case: "a grant scoped by SourceArn to another alarm",
            grant: Grant::AnotherAlarm,
            state: "ALARM",
            invoked: false,
        },
        Row {
            case: "a grant of another action than the invocation",
            grant: Grant::AnotherAction,
            state: "ALARM",
            invoked: false,
        },
    ]
}

/// RULE: an alarm action naming a function invokes that function with the transition, and the
/// function's own resource-based policy is what decides whether the invocation happens at all.
#[tokio::test]
async fn a_lambda_alarm_action_invokes_the_function_its_policy_admits() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator_with_lambda();
        let function = format!("alarm-action-fn-{index}");
        let alarm = format!("alarm-action-{index}");
        let other_alarm = format!("alarm-action-other-{index}");

        let function_arn = create_function(&emulator, case, &function).await;
        // The alarm names the function in the action list of the state this row drives it into,
        // so the transition the row makes is the one that fires it.
        let named: &[&str] = &[function_arn.as_str()];
        let none: &[&str] = &[];
        let (ok_actions, alarm_actions) = if row.state == "OK" {
            (named, none)
        } else {
            (none, named)
        };
        put_metric_alarm(&emulator, &alarm, ok_actions, alarm_actions).await;
        // Another alarm of the same account, so a grant one row scopes to another alarm names
        // an alarm this account really holds.
        put_metric_alarm(&emulator, &other_alarm, none, none).await;
        let described = monitoring(
            &emulator,
            "DescribeAlarms",
            &json!({"AlarmNames": [&alarm, &other_alarm], "AlarmTypes": ["MetricAlarm"]}),
        )
        .await;
        let arn_of = |name: &str| {
            described["MetricAlarms"]
                .as_array()
                .and_then(|alarms| {
                    alarms
                        .iter()
                        .find(|held| held["AlarmName"] == json!(name))
                        .and_then(|held| held["AlarmArn"].as_str())
                })
                .unwrap_or_else(|| panic!("case {case:?}: no AlarmArn for {name:?}: {described}"))
                .to_string()
        };
        let alarm_arn = arn_of(&alarm);
        let other_alarm_arn = arn_of(&other_alarm);

        match row.grant {
            Grant::None => {}
            Grant::Unscoped => {
                add_permission(&emulator, case, &function, INVOKE_ACTION, None).await;
            }
            Grant::ThisAlarm => {
                add_permission(
                    &emulator,
                    case,
                    &function,
                    INVOKE_ACTION,
                    Some(&alarm_arn),
                )
                .await;
            }
            Grant::AnotherAlarm => {
                add_permission(
                    &emulator,
                    case,
                    &function,
                    INVOKE_ACTION,
                    Some(&other_alarm_arn),
                )
                .await;
            }
            Grant::AnotherAction => {
                add_permission(
                    &emulator,
                    case,
                    &function,
                    "lambda:GetFunction",
                    None,
                )
                .await;
            }
        }

        // The created function leaves `Pending` by the clock; the row looks until that service
        // reports it `Active` rather than waiting out a span.
        until_active(&emulator, case, &function).await;

        monitoring(
            &emulator,
            "SetAlarmState",
            &json!({
                "AlarmName": alarm,
                "StateValue": row.state,
                "StateReason": "driven by the test",
            }),
        )
        .await;

        let items = until_recorded(&emulator, &alarm).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected the alarm to account for exactly one action execution, \
             got {items:?}"
        );
        let summary = items[0]["HistorySummary"].as_str().unwrap_or_default();
        let data: Value = serde_json::from_str(items[0]["HistoryData"].as_str().unwrap_or("null"))
            .unwrap_or(Value::Null);

        if row.invoked {
            let lines = until_logged(&emulator, &function).await;
            let expected = format!("{PRINTED_PREFIX}{alarm}:{}", row.state);
            assert!(
                lines.iter().any(|line| line.contains(&expected)),
                "case {case:?}: expected the handler to run and print {expected:?} into the \
                 function's log group, got {lines:?}"
            );
            assert_eq!(
                summary,
                format!("Successfully executed action {function_arn}"),
                "case {case:?}: expected the action to be recorded as executed, got \
                 {summary:?} with {data}"
            );
            assert_eq!(
                data["actionState"],
                json!("Succeeded"),
                "case {case:?}: expected the recorded action to have succeeded, got {data}"
            );
            // What the function was handed is the transition itself, so the record of the
            // execution names the alarm and the state it moved to.
            let published: Value =
                serde_json::from_str(data["publishedMessage"].as_str().unwrap_or("null"))
                    .unwrap_or(Value::Null);
            for (member, pointer, wanted) in [
                ("the alarm's ARN", "/alarmArn", json!(alarm_arn)),
                ("the alarm's name", "/alarmData/alarmName", json!(alarm)),
                (
                    "the state it moved to",
                    "/alarmData/state/value",
                    json!(row.state),
                ),
            ] {
                let found = published.pointer(pointer).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    found, wanted,
                    "case {case:?}: expected {member} to be {wanted} at {pointer} of the \
                     payload the function was handed, got {found} in {published}"
                );
            }
            continue;
        }

        // The refused arm: nothing was invoked, and the alarm's history says why.
        assert_eq!(
            summary,
            format!("Failed to execute action {function_arn}"),
            "case {case:?}: expected the action to be recorded as failed, got {summary:?} with \
             {data}"
        );
        let reported = data["error"].as_str().unwrap_or_default();
        let expected = format!(
            "User: {ALARM_ACTION_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on \
             resource: {function_arn} because no resource-based policy allows the \
             {INVOKE_ACTION} action"
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the recorded error to be {expected:?}, got {reported:?}"
        );
        assert!(
            data["publishedMessage"].is_null(),
            "case {case:?}: expected a refused action to report nothing published, got {data}"
        );
        // And nothing ran: the function's log group holds no line of this handler. The record
        // read above is written as the handover is decided, so a run owed here would already
        // have been started — and a function that was never invoked has no log group at all.
        let lines = logged(&emulator, &function).await;
        assert!(
            !lines.iter().any(|line| line.contains(PRINTED_PREFIX)),
            "case {case:?}: expected no run of the function, got {lines:?}"
        );
    }
}
