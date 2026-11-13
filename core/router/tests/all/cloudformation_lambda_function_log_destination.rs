// SPDX-License-Identifier: CC0-1.0

//! The AWS Lambda function a deployed stack declares, and WHERE the records of its runs land.
//!
//! RULE under test: **a stack that reports `CREATE_COMPLETE` for a declared
//! `AWS::Lambda::Function` has really created that function in AWS Lambda under the physical
//! id the stack publishes — so `GetFunction` on that id resolves and AWS Lambda reports the
//! function `Active` — and every invocation's records are delivered to the ONE group that
//! function logs to: the group its `LoggingConfig.LogGroup` names, and otherwise the default
//! `/aws/lambda/<function name>`; a destination group no one created yet is created by the
//! first invocation that writes to it, and no second group is created for a function whose
//! `LoggingConfig` points its log somewhere else.**
//!
//! `LoggingConfig$LogGroup` is "the name of the Amazon CloudWatch log group the function
//! sends logs to", and states the default: "By default, Lambda functions send logs to a
//! default log group named `/aws/lambda/<function name>`"
//! (`smithy_query(query="doc",shape="LoggingConfig",member="LogGroup")`). `Ref` on an
//! `AWS::Lambda::Function` "returns the function name", which is the id the calls below are
//! made with.
//!
//! A stack that reports a resource complete and a function that does not exist is the same
//! defect as a deploy that failed silently: the status stops being a readiness signal, and
//! every later call that takes the function as a handle (`GetFunction`, the
//! `FunctionActiveV2` wait, `Invoke`) answers `ResourceNotFoundException` for it. A function
//! whose log reaches no group is the same for a client waiting on the group it declared: it
//! waits for a delivery that never comes.
//!
//! The rows differ along what the rule generalises over rather than restating one template:
//!
//! * where the function's log DESTINATION comes from — none declared, so the default group
//!   for the function's own name; a group the SAME stack declares, reached by `Ref`; and a
//!   group named literally that nothing declares at all;
//! * where the function's NAME comes from — a template that states `FunctionName`, and one
//!   that states none, so CloudFormation generates it and the row can only learn it from the
//!   stack's own published id;
//! * whether the destination group EXISTED before the first run (the declared one) or had to
//!   be created by it (the other two).
//!
//! No expected value here is a literal from a recording: each row's function id is read out
//! of its own stack's published `Ref`, every expected group name is computed from what the
//! row declared, and the marker each run logs is built by the row.
//!
//! Four crates have to agree for the rule to hold — CloudFormation provisions the function
//! and the role, IAM answers for the execution role the log is written under, AWS Lambda runs
//! the function and delivers its log, Amazon CloudWatch Logs holds the group and its
//! events — so the suite lives here rather than in any one of them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of, physical_ids_of};

/// The Region every request below is signed for — the one the shared stack-status helper
/// polls in — and so the Region every ARN here names.
const REGION: &str = "us-east-1";

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `State` AWS Lambda reports a function it takes calls on in.
const ACTIVE: &str = "Active";

/// The prefix of the default log group of a function whose `LoggingConfig` names none: "a
/// default log group named `/aws/lambda/<function name>`" (`LoggingConfig$LogGroup`).
const DEFAULT_GROUP_PREFIX: &str = "/aws/lambda/";

/// The fixed framing AWS Lambda writes for every run it finishes, which is what says the log
/// of a run really was delivered beside what the handler printed.
const REPORT_PREFIX: &str = "REPORT RequestId: ";

/// How long one invocation of the handler below is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of something another service owes, and how many polls to
/// make. The budget is far longer than any span these services choose, so a loaded host is
/// slower without the rule being any different; nothing here asserts on elapsed time.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// How many records ONE run leaves in its group at least: the line the handler printed, and
/// the runtime's own closing frame.
const RECORDS_PER_RUN: usize = 2;

/// The logical ids the templates below declare.
const FUNCTION_ID: &str = "Logger";
const ROLE_ID: &str = "ExecutionRole";
const GROUP_ID: &str = "Group";

/// The handler every row deploys: it prints the marker the event carries and answers it, so
/// what one run left in a group can be told apart from any other run's records.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    print("logged %s" % event["marker"])
    return {"logged": event["marker"]}
"#;

/// The trust policy the execution role carries: AWS Lambda assumes it to run the function.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// What that role may do: the three CloudWatch Logs calls an invocation's log delivery makes,
/// which is what the AWS-managed basic execution policy grants a function. A role short of
/// them leaves nothing for a client to read, which is that rule rather than this one.
fn logs_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
            "Resource": "*"
        }]
    })
}

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine: the function the stack creates, the role it
/// runs as and the group its run writes to are all held by the services mounted there.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: each call carries a credential scope so the
    // router can name the target service, and the signature itself is not under test here.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "cloudformation",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        Bytes::from(body),
    )
    .await
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        "lambda",
        method,
        path,
        &[("content-type", "application/json")],
        Bytes::from(payload),
    )
    .await
}

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> Answered {
    dispatch(
        router,
        "logs",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// Every group `DescribeLogGroups` resolves for the prefix `name` whose own `logGroupName` is
/// exactly `name`: what a client holding that name can find by it.
async fn groups_named(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<Value> {
    let answered = logs(
        router,
        "DescribeLogGroups",
        json!({ "logGroupNamePrefix": name }),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLogGroups for {name:?}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered
        .json()
        .get("logGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter(|group| group.get("logGroupName").and_then(Value::as_str) == Some(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Every message CloudWatch Logs holds in `group` right now. A group nothing has written to
/// yet does not exist, which that service answers as a refusal rather than as an empty list,
/// so such an answer reads here as no records.
async fn messages_in(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let answered = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
    if answered.status != 200 {
        return Vec::new();
    }
    answered
        .json()
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event.get("message").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The messages `group` holds, polled until it holds at least `at_least` of them or the polls
/// run out. The delivery of a run's log is not part of the invocation's answer, so this waits
/// for the CONDITION rather than for a span.
async fn messages_at_least(
    router: &MultiServiceRouter,
    group: &str,
    at_least: usize,
) -> Vec<String> {
    let mut held = messages_in(router, group).await;
    for _ in 0..POLLS {
        if held.len() >= at_least {
            return held;
        }
        tokio::time::sleep(POLL).await;
        held = messages_in(router, group).await;
    }
    held
}

/// Poll `GetFunctionConfiguration` until AWS Lambda first reports the function `Active`, and
/// answer the configuration it reported then. The span is that service's own, so nothing here
/// sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let path = format!("/2015-03-31/functions/{name}/configuration");
    let mut last = Value::Null;
    for _ in 0..POLLS {
        let reported = lambda(router, "GET", &path, None).await;
        last = reported.json();
        if last.get("State").and_then(Value::as_str) == Some(ACTIVE) {
            return last;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("case {case:?}: AWS Lambda never reported {name} {ACTIVE}, last read {last}");
}

/// Where a row's function sends the records of its runs, and how its template says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// The template declares no `LoggingConfig` at all, so the function logs to the default
    /// group for its own name.
    Default,
    /// The SAME stack declares an `AWS::Logs::LogGroup` under this name, and the function's
    /// `LoggingConfig` reaches it by `Ref`.
    Declared(&'static str),
    /// The function's `LoggingConfig` names this group literally, and nothing declares it.
    Named(&'static str),
}

/// One row: where the function's name comes from, and where its log goes.
struct Row {
    case: &'static str,
    /// The `FunctionName` the template states, or `None` where it states none and
    /// CloudFormation generates it.
    function_name: Option<&'static str>,
    destination: Destination,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named function declaring no logging configuration at all",
        function_name: Some("cfn-log-destination-default"),
        destination: Destination::Default,
    },
    Row {
        case: "a function CloudFormation names, declaring no logging configuration",
        function_name: None,
        destination: Destination::Default,
    },
    Row {
        case: "a named function logging into a group the same stack declares",
        function_name: Some("cfn-log-destination-declared"),
        destination: Destination::Declared("/cfn/declared-destination"),
    },
    Row {
        case: "a function CloudFormation names, logging into a group nothing declares",
        function_name: None,
        destination: Destination::Named("/cfn/undeclared-destination"),
    },
];

impl Row {
    /// The group this row's function logs to, given the function id the stack published.
    fn expected_group(&self, function: &str) -> String {
        match self.destination {
            Destination::Default => format!("{DEFAULT_GROUP_PREFIX}{function}"),
            Destination::Declared(name) | Destination::Named(name) => name.to_string(),
        }
    }

    /// Whether that group already exists before anything has been invoked, which is so only
    /// for the group the stack itself declares.
    fn exists_before_the_first_run(&self) -> bool {
        matches!(self.destination, Destination::Declared(_))
    }

    /// The template this row deploys: the execution role, the group it declares (if any), and
    /// the function pointed at its destination.
    fn template(&self) -> String {
        let mut function_properties = serde_json::Map::new();
        if let Some(name) = self.function_name {
            function_properties.insert("FunctionName".to_string(), json!(name));
        }
        function_properties.insert(
            "Role".to_string(),
            json!({ "Fn::GetAtt": [ROLE_ID, "Arn"] }),
        );
        function_properties.insert("Runtime".to_string(), json!("python3.12"));
        function_properties.insert("Handler".to_string(), json!("index.handler"));
        function_properties.insert("Timeout".to_string(), json!(FUNCTION_TIMEOUT));
        function_properties.insert("Code".to_string(), json!({ "ZipFile": HANDLER_SOURCE }));
        match self.destination {
            Destination::Default => {}
            Destination::Declared(_) => {
                function_properties.insert(
                    "LoggingConfig".to_string(),
                    json!({ "LogGroup": { "Ref": GROUP_ID } }),
                );
            }
            Destination::Named(name) => {
                function_properties
                    .insert("LoggingConfig".to_string(), json!({ "LogGroup": name }));
            }
        }

        let mut resources = serde_json::Map::new();
        resources.insert(
            ROLE_ID.to_string(),
            json!({
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": trust_policy(),
                    "Policies": [{
                        "PolicyName": "basic-execution",
                        "PolicyDocument": logs_policy()
                    }]
                }
            }),
        );
        if let Destination::Declared(name) = self.destination {
            resources.insert(
                GROUP_ID.to_string(),
                json!({
                    "Type": "AWS::Logs::LogGroup",
                    "Properties": { "LogGroupName": name }
                }),
            );
        }
        resources.insert(
            FUNCTION_ID.to_string(),
            json!({
                "Type": "AWS::Lambda::Function",
                "Properties": Value::Object(function_properties)
            }),
        );

        json!({
            "AWSTemplateFormatVersion": "2010-09-09",
            "Resources": Value::Object(resources),
            "Outputs": {
                "FunctionName": { "Value": { "Ref": FUNCTION_ID } }
            }
        })
        .to_string()
    }
}

/// Invoke the function `name` with a marker of its own, and answer what the handler returned.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, marker: &str) {
    let invoked = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        Some(json!({ "marker": marker })),
    )
    .await;
    assert_eq!(
        invoked.status, 200,
        "case {case:?}: expected HTTP 200 from Invoke of {name:?}, got {} with {:?}",
        invoked.status,
        invoked.text()
    );
    assert_eq!(
        invoked.json().get("logged").and_then(Value::as_str),
        Some(marker),
        "case {case:?}: expected the handler's own result for {marker:?}, got {:?}",
        invoked.text()
    );
}

#[tokio::test]
async fn a_stack_declared_function_exists_and_its_runs_are_logged_to_its_own_group() {
    // ONE emulator for every row: each row deploys a stack of its own, so the function and the
    // group a row reads are still that row's alone.
    let router = emulator();
    let account = engine::emulator_account_id();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // 1. The stack: the execution role, the declared destination group where the row has
        // one, and the function.
        let stack = format!("lambda-log-destination-{index}");
        let body = row.template();
        let created = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &body),
                // The template declares an IAM role, which is acknowledged with
                // CAPABILITY_IAM.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
            created.status,
            created.text()
        );
        let (reached, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // 2. What the stack published for the function: "Ref returns the function name", which
        // is the id every call below is made with, and the resource's own physical id.
        let function = outputs_of(&described)
            .get("FunctionName")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack published no function name in {described}")
            });
        let resources =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        let physical = physical_ids_of(&resources.text());
        let function_id = physical.get(FUNCTION_ID).cloned().unwrap_or_else(|| {
            panic!("case {case:?}: no physical id for {FUNCTION_ID} in {physical:?}")
        });
        assert_eq!(
            function, function_id,
            "case {case:?}: expected the published name to be the function's physical id"
        );
        if let Some(declared) = row.function_name {
            assert_eq!(
                function, declared,
                "case {case:?}: expected the published name to be the declared one"
            );
        }

        // 3. AWS Lambda really holds that function: `GetFunction` on the id the completed
        // stack published resolves it, rather than answering for a function nothing created.
        let read = lambda(&router, "GET", &format!("/2015-03-31/functions/{function}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetFunction for {function:?}, got {} with {:?}",
            read.status,
            read.text()
        );
        let configuration = read.json().get("Configuration").cloned().unwrap_or_default();
        assert_eq!(
            configuration.get("FunctionName").and_then(Value::as_str),
            Some(function.as_str()),
            "case {case:?}: expected GetFunction to report the published name, got {configuration}"
        );
        let expected_arn = format!("arn:aws:lambda:{REGION}:{account}:function:{function}");
        assert_eq!(
            configuration.get("FunctionArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the function's ARN to be {expected_arn:?}, got \
             {configuration}"
        );

        // 4. Where its log goes, as AWS Lambda itself reports it once the function is out of
        // the lifecycle window that service holds every new function in.
        let expected_group = row.expected_group(&function);
        let settled = wait_until_active(&router, case, &function).await;
        assert_eq!(
            settled
                .get("LoggingConfig")
                .and_then(|logging| logging.get("LogGroup"))
                .and_then(Value::as_str),
            Some(expected_group.as_str()),
            "case {case:?}: expected AWS Lambda to report {expected_group:?} as the function's \
             log group, got {settled}"
        );

        // 5. Before anything has run: the group the stack DECLARED is already there, and a
        // destination nothing created does not exist yet — a run's own first write is what
        // creates it.
        let resolved = groups_named(&router, case, &expected_group).await;
        let expected_count = usize::from(row.exists_before_the_first_run());
        assert_eq!(
            resolved.len(),
            expected_count,
            "case {case:?}: expected DescribeLogGroups for {expected_group:?} to resolve \
             {expected_count} group(s) before the first run, got {resolved:?}"
        );

        // 6. One run, and its records in exactly that group.
        let marker = format!("marker-{index}-first");
        invoke(&router, case, &function, &marker).await;
        let printed = format!("logged {marker}");
        let held = messages_at_least(&router, &expected_group, RECORDS_PER_RUN).await;
        assert!(
            held.iter().any(|message| message.contains(&printed)),
            "case {case:?}: expected the group {expected_group:?} to hold the printed line \
             {printed:?}, got {held:?}"
        );
        assert!(
            held.iter().any(|message| message.contains(REPORT_PREFIX)),
            "case {case:?}: expected the group {expected_group:?} to hold a {REPORT_PREFIX:?} \
             record for the run, got {held:?}"
        );

        // 7. Exactly ONE group under that name — the run's delivery created the destination
        // where there was none, and re-opened it where there was one rather than adding a
        // second.
        let resolved = groups_named(&router, case, &expected_group).await;
        assert_eq!(
            resolved.len(),
            1,
            "case {case:?}: expected DescribeLogGroups for {expected_group:?} to resolve exactly \
             1 group after the run, got {resolved:?}"
        );

        // 8. ... and nowhere else: a function whose `LoggingConfig` points its log at another
        // group leaves the default group for its own name uncreated.
        if row.destination != Destination::Default {
            let default_group = format!("{DEFAULT_GROUP_PREFIX}{function}");
            let elsewhere = groups_named(&router, case, &default_group).await;
            assert!(
                elsewhere.is_empty(),
                "case {case:?}: expected no group {default_group:?} for a function logging to \
                 {expected_group:?}, got {elsewhere:?}"
            );
        }

        // 9. A second run is logged to the SAME group, beside the first run's records rather
        // than into a group of its own.
        let second = format!("marker-{index}-second");
        invoke(&router, case, &function, &second).await;
        let printed_again = format!("logged {second}");
        let held = messages_at_least(&router, &expected_group, RECORDS_PER_RUN * 2).await;
        for expected in [&printed, &printed_again] {
            assert!(
                held.iter().any(|message| message.contains(expected)),
                "case {case:?}: expected the group {expected_group:?} to hold {expected:?} after \
                 two runs, got {held:?}"
            );
        }
        let resolved = groups_named(&router, case, &expected_group).await;
        assert_eq!(
            resolved.len(),
            1,
            "case {case:?}: expected DescribeLogGroups for {expected_group:?} to resolve exactly \
             1 group after two runs, got {resolved:?}"
        );
    }
}
