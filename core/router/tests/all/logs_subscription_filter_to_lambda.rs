// SPDX-License-Identifier: CC0-1.0

//! A CloudWatch Logs subscription filter whose destination is a same-account AWS Lambda
//! function hands the function every ingested log event the filter selected, and what the
//! function printed is readable in the function's own log group.
//!
//! RULE under test: **an event a `PutLogEvents` batch stores in a log group is handed to the
//! destination of every subscription filter of that group whose `filterPattern` selects it,
//! and for a `destinationArn` naming a Lambda function the hand-over is an invocation of that
//! function with the `awslogs` document — whose `data` is the base64 of the gzip-framed
//! CloudWatch Logs subscription envelope, carrying the owning account, the log group, the log
//! stream, the selecting filter's name, the `DATA_MESSAGE` message type and exactly the
//! selected events — so the run really happens and the lines it writes land in that function's
//! log group.**
//!
//! `PutSubscriptionFilter$destinationArn` names that destination kind itself ("A Lambda
//! function belonging to the same account as the subscription filter, for same-account
//! delivery") and states the framing every destination is handed ("When log events are sent to
//! the receiving service, they are Base64 encoded and compressed with the GZIP format"); the
//! function's output goes to its own group because "By default, Lambda functions send logs to
//! a default log group named `/aws/lambda/<function name>`" (`LoggingConfig$LogGroup`).
//!
//! A filter whose destination is never invoked is indistinguishable from a log group carrying
//! no filter at all, which is why the deliverable here is the EFFECT: the destination function
//! observes the events. The handler does the decoding the framing calls for and prints the
//! envelope DOCUMENT verbatim, one line; every assertion about the envelope's members is made
//! here, against the document read back out of the function's log group, so what is checked is
//! what the function was really handed. Nothing is asserted about the recorded instance's
//! names or markers — every expected value is built from what the row wrote.
//!
//! The rows differ along what the rule generalises over:
//!
//! * how many events ONE batch carries, so a single-event batch and a multi-event batch are
//!   both handed over (the envelope carries the batch's selected events, not just the first);
//! * whether the filter's `filterPattern` selects the WHOLE batch or PART of it, so the
//!   envelope's event list is exactly the selected events and the rejected ones are observably
//!   not in it;
//! * how the topology was WIRED — a group, a filter and an invoke permission a client put
//!   through each service's own API, and the same three DECLARED by a CloudFormation stack,
//!   which is how an infrastructure-as-code client builds this pipeline.
//!
//! The destination function itself always comes from that one stack, because a template is what
//! archives inline source into a deployment package this emulator can run, and the stack-wired
//! row's group, permission and filter are declared beside it — one deployment, the shape the
//! recorded topology has.
//!
//! Four crates have to agree for the rule to hold — CloudWatch Logs holds the group, runs the
//! pattern and makes the hand-over, AWS Lambda runs the function and delivers its log, IAM
//! holds the execution role that log is written under, and CloudFormation declares one row's
//! topology — so the suite lives here rather than in any one of them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode, outputs_of};

/// The Region every request below is signed for, and so the Region every ARN here names — the
/// same Region the shared stack-status helper signs for.
const REGION: &str = "us-east-1";

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The destination function every row's filter points at, and the execution role it runs as.
const FUNCTION: &str = "logs-subscription-destination";
const EXECUTION_ROLE: &str = "logs-subscription-destination-role";

/// The log group AWS Lambda delivers an invocation's log to, for a function whose
/// `LoggingConfig` names none: "a default log group named `/aws/lambda/<function name>`"
/// (`LoggingConfig$LogGroup`).
const LAMBDA_LOG_GROUP_PREFIX: &str = "/aws/lambda/";

/// The `messageType` a subscription envelope carrying ingested log events reports.
const DATA_MESSAGE: &str = "DATA_MESSAGE";

/// What the handler prefixes the envelope document with, so the line it printed can be told
/// apart from the runtime's own framing in the same log group.
const ENVELOPE_PREFIX: &str = "ENVELOPE ";

/// The fixed framing AWS Lambda writes for every run it finishes, which is what says the log
/// of a run really was delivered beside what the handler printed.
const REPORT_PREFIX: &str = "REPORT RequestId: ";

/// How long one invocation of the handler below is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of something another service owes, and how many polls to
/// make. The budget is far longer than any span these services choose, so a loaded host is
/// slower without the rule being any different; nothing here asserts on elapsed time.
const POLL: Duration = Duration::from_millis(100);
const POLLS: usize = 600;

/// How many polls the LOCAL waits make: the stack reporting itself complete and the function
/// reporting itself invocable are both answered out of this emulator's own store, so they are
/// given a budget many times the span those services choose rather than the whole one above.
const LOCAL_POLLS: usize = 200;

/// The trust policy the destination function's execution role carries: Lambda assumes it to
/// run the function.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"#,
    r#"{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// What that role may do: the three CloudWatch Logs calls an invocation's log delivery makes,
/// which is what the AWS-managed basic execution policy grants a function.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#
);

/// The handler every hand-over runs: it undoes the two layers the subscription payload is
/// framed in — base64, then gzip — and prints the envelope document it found, so the document
/// the function was handed is readable in the function's own log group and every claim about
/// it is checked by the suite rather than by the handler.
const HANDLER_SOURCE: &str = r#"
import binascii
import gzip


def handler(event, context):
    frame = binascii.a2b_base64(event["awslogs"]["data"])
    print("ENVELOPE " + gzip.decompress(frame).decode("utf-8"))
    return {"framedBytes": len(frame)}
"#;

/// The logical ids the ONE stack below declares, which the suite reads its published values
/// and its provisioned filter back by.
const DESTINATION_ID: &str = "Destination";
const GROUP_ID: &str = "Group";
const PERMISSION_ID: &str = "InvokePermission";
const FILTER_ID: &str = "Filter";

/// How a row's log group, subscription filter and invoke permission came to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WiredBy {
    /// Each one put through its own service's API, the way a client wires it by hand.
    ClientCalls,
    /// All three declared by a CloudFormation stack, the way an infrastructure-as-code client
    /// deploys the pipeline.
    Stack,
}

/// One event a row's batch carries: its message, and whether the row's `filterPattern` selects
/// it — which is whether it must be in the envelope the function was handed.
struct Written {
    message: &'static str,
    selected: bool,
}

/// One row: a log group of its own, the pattern its filter carries, how the three resources
/// were wired, and the batch it writes.
struct Row {
    case: &'static str,
    /// Names this row's log group, log stream and filter, so no two rows share one.
    name: &'static str,
    /// The filter's own `filterPattern`. The empty pattern selects every event.
    pattern: &'static str,
    wired_by: WiredBy,
    batch: &'static [Written],
}

const ROWS: &[Row] = &[
    Row {
        case: "a client-wired filter matching everything, with a single-event batch",
        name: "single",
        pattern: "",
        wired_by: WiredBy::ClientCalls,
        batch: &[Written {
            message: "single-batch only event",
            selected: true,
        }],
    },
    Row {
        case: "a client-wired filter whose term pattern selects part of a four-event batch",
        name: "selective",
        pattern: "SHIPPED",
        wired_by: WiredBy::ClientCalls,
        batch: &[
            Written {
                message: "SHIPPED selective first event",
                selected: true,
            },
            Written {
                message: "withheld selective second event",
                selected: false,
            },
            Written {
                message: "SHIPPED selective third event",
                selected: true,
            },
            Written {
                message: "withheld selective fourth event",
                selected: false,
            },
        ],
    },
    Row {
        case: "a stack-declared group, filter and invoke permission, with a three-event batch",
        name: "deployed",
        pattern: "",
        wired_by: WiredBy::Stack,
        batch: &[
            Written {
                message: "deployed first event",
                selected: true,
            },
            Written {
                message: "deployed second event",
                selected: true,
            },
            Written {
                message: "deployed third event",
                selected: true,
            },
        ],
    },
];

impl Row {
    /// This row's log group.
    fn group(&self) -> String {
        format!("/aws/emulator/subscription-to-lambda/{}", self.name)
    }

    /// The log stream this row's batch is written to.
    fn stream(&self) -> String {
        format!("{}-stream", self.name)
    }

    /// This row's subscription filter.
    fn filter(&self) -> String {
        format!("{}-to-lambda", self.name)
    }

    /// The statement id the invoke permission for this row's group is added under.
    fn statement_id(&self) -> String {
        format!("{}-logs-invoke", self.name)
    }

    /// The messages this row's `filterPattern` selects, in the order they were written —
    /// exactly what the envelope handed to the function must carry.
    fn selected(&self) -> Vec<String> {
        self.batch
            .iter()
            .filter(|written| written.selected)
            .map(|written| written.message.to_string())
            .collect()
    }
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

/// The emulator with the services this flow spans, on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
///
/// Request authentication is off, as in every other suite here: each call carries a credential
/// scope so the router can name the target service, and SigV4 itself is not under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
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

/// One CloudWatch Logs call that must succeed.
async fn logs_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answered = logs(router, operation, body).await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
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

/// One `awsQuery` call, which is how both IAM and CloudFormation are addressed.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        signing_name,
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        Bytes::from(body),
    )
    .await
}

/// One IAM action that must succeed, answering the body it was served.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let answered = query(router, "iam", "2010-05-08", action, members).await;
    assert_eq!(
        answered.status,
        200,
        "setup: IAM {action} must succeed, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.text()
}

/// One CloudFormation action that must succeed, answering the body it was served.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let answered = query(router, "cloudformation", "2010-05-15", action, members).await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.text()
}

/// The ARN of one log group, which is the resource an invoke permission is scoped to with
/// `SourceArn`.
fn group_arn(account: &str, group: &str) -> String {
    format!("arn:aws:logs:{REGION}:{account}:log-group:{group}:*")
}

/// Grant `logs.amazonaws.com` the invocation of the destination function for ONE source log
/// group, which is the grant a Lambda-destination subscription filter is authorized by.
async fn add_permission(router: &MultiServiceRouter, case: &str, row: &Row, account: &str) {
    let answered = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": row.statement_id(),
            "Action": "lambda:InvokeFunction",
            "Principal": "logs.amazonaws.com",
            "SourceArn": group_arn(account, &row.group()),
            "SourceAccount": account,
        })),
    )
    .await;
    assert_eq!(
        answered.status,
        201,
        "case {case:?}: expected HTTP 201 from AddPermission on {FUNCTION}, got {} with {:?}",
        answered.status,
        answered.text()
    );
}

/// The ONE stack this suite deploys, which is the recorded topology: the destination function
/// (a template is what archives inline source into a deployment package this emulator can
/// run), and — for the row that is wired this way — the log group, the invoke permission for it
/// and the subscription filter that depends on that permission, exactly as an
/// infrastructure-as-code client declares the pipeline. The permission and the filter reach the
/// function through `Fn::GetAtt`, so each is created after the function the stack itself made.
///
/// The two client-wired rows take the function ARN the stack publishes and put their own group,
/// permission and filter through each service's own API.
fn topology_template(row: &Row, role_arn: &str, account: &str) -> String {
    let function_arn = json!({ "Fn::GetAtt": [DESTINATION_ID, "Arn"] });
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            DESTINATION_ID: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": { "ZipFile": HANDLER_SOURCE },
                },
            },
            GROUP_ID: {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": row.group(),
                    "RetentionInDays": 1,
                },
            },
            PERMISSION_ID: {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": function_arn,
                    "Action": "lambda:InvokeFunction",
                    "Principal": "logs.amazonaws.com",
                    "SourceArn": group_arn(account, &row.group()),
                    "SourceAccount": account,
                },
            },
            FILTER_ID: {
                "Type": "AWS::Logs::SubscriptionFilter",
                "DependsOn": [GROUP_ID, PERMISSION_ID],
                "Properties": {
                    "LogGroupName": row.group(),
                    "FilterName": row.filter(),
                    "FilterPattern": row.pattern,
                    "DestinationArn": function_arn,
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": function_arn },
        },
    })
    .to_string()
}

/// Deploy that stack and answer the destination function's ARN once Lambda reports the function
/// invocable.
async fn deploy_topology(
    router: &MultiServiceRouter,
    row: &Row,
    role_arn: &str,
    account: &str,
) -> String {
    let stack = "logs-subscription-to-lambda";
    let template = topology_template(row, role_arn, account);
    cloudformation(
        router,
        row.case,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let arn = outputs_of(&described)
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack published no FunctionArn, got {described}"));

    // Nothing is delivered to a function that cannot yet be invoked: "During this time, you
    // can't invoke or modify the function" (`CreateFunction`), so a hand-over withheld there
    // would be the lifecycle's refusal rather than the rule under test.
    for _ in 0..LOCAL_POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if reported.json().get("State").and_then(Value::as_str) == Some("Active") {
            return arn;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {FUNCTION} Active");
}

/// Every envelope document the destination function printed, as the suite reads them back out
/// of the function's own log group. A group nothing has written to yet does not exist, which
/// that service answers as a refusal rather than as an empty list, so such an answer reads as
/// nothing printed; a line that is not an envelope (the runtime's own framing) is kept apart.
async fn printed(router: &MultiServiceRouter) -> (Vec<Value>, Vec<String>) {
    let answered = logs(
        router,
        "FilterLogEvents",
        json!({ "logGroupName": format!("{LAMBDA_LOG_GROUP_PREFIX}{FUNCTION}") }),
    )
    .await;
    if answered.status != 200 {
        return (Vec::new(), Vec::new());
    }
    let mut envelopes = Vec::new();
    let mut other = Vec::new();
    for message in answered
        .json()
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event.get("message").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
    {
        match message.strip_prefix(ENVELOPE_PREFIX) {
            Some(document) => match serde_json::from_str::<Value>(document.trim()) {
                Ok(envelope) => envelopes.push(envelope),
                Err(error) => panic!(
                    "the handler printed a line that is not the envelope document: {error} in \
                     {document:?}"
                ),
            },
            None => other.push(message),
        }
    }
    (envelopes, other)
}

/// Poll the destination function's log group until it holds at least `wanted` envelope
/// documents, and answer the last reading taken. The hand-over is asynchronous (the function's
/// answer is nothing the ingesting request reports), so this waits for the CONDITION rather
/// than for a span.
async fn printed_at_least(
    router: &MultiServiceRouter,
    wanted: usize,
) -> (Vec<Value>, Vec<String>) {
    let mut read = printed(router).await;
    for _ in 0..POLLS {
        if read.0.len() >= wanted {
            return read;
        }
        tokio::time::sleep(POLL).await;
        read = printed(router).await;
    }
    read
}

/// The string at `member` of `envelope`, or a failure naming the row.
fn text_member(case: &str, envelope: &Value, member: &str) -> String {
    envelope
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the envelope carries no {member:?}, got {envelope}")
        })
        .to_string()
}

#[tokio::test]
async fn a_subscription_filters_lambda_destination_is_handed_the_events_it_selected() {
    let router = emulator();
    let account = engine::emulator_account_id();

    // The execution role the destination function runs as, and under which its own log is
    // written: a role short of the logs grants leaves nothing for a client to read, which
    // would be that rule rather than this one.
    let created = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", EXECUTION_ROLE),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    let role_arn = element(&created, "Arn");
    iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", EXECUTION_ROLE),
            ("PolicyName", "basic-execution"),
            ("PolicyDocument", ROLE_POLICY),
        ],
    )
    .await;

    // The ONE stack: the destination function, and the group, permission and filter of the row
    // whose topology is DECLARED rather than put through each service's own API.
    let stack_row = ROWS
        .iter()
        .find(|row| row.wired_by == WiredBy::Stack)
        .expect("the table carries a stack-wired row");
    let function_arn = deploy_topology(&router, stack_row, &role_arn, account).await;

    // Every row's log group, invoke permission and subscription filter — wired the way the row
    // says — and then the row's own log stream, which is what its batch is written to.
    for row in ROWS {
        let case = row.case;
        match row.wired_by {
            WiredBy::ClientCalls => {
                logs_ok(
                    &router,
                    case,
                    "CreateLogGroup",
                    json!({ "logGroupName": row.group() }),
                )
                .await;
                add_permission(&router, case, row, account).await;
                logs_ok(
                    &router,
                    case,
                    "PutSubscriptionFilter",
                    json!({
                        "logGroupName": row.group(),
                        "filterName": row.filter(),
                        "filterPattern": row.pattern,
                        // A Lambda destination carries no `roleArn`: "a Lambda function is
                        // invoked directly", so the function's own policy authorizes it.
                        "destinationArn": function_arn,
                    }),
                )
                .await;
            }
            WiredBy::Stack => {
                // The stack above already declared this row's group, permission and filter.
                // What the stack DECLARED has to be what the owning service HOLDS, so the
                // filter is read back through that service: a stack that reported itself
                // complete without putting it would deliver nothing for a reason that is not
                // the rule under test.
                let described_filters = logs_ok(
                    &router,
                    case,
                    "DescribeSubscriptionFilters",
                    json!({ "logGroupName": row.group() }),
                )
                .await;
                let named: Vec<String> = described_filters
                    .get("subscriptionFilters")
                    .and_then(Value::as_array)
                    .map(|found| {
                        found
                            .iter()
                            .filter_map(|filter| filter.get("filterName").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    named,
                    vec![row.filter()],
                    "case {case:?}: expected the stack to put exactly the filter it declared, \
                     got {named:?}"
                );
                let destination = described_filters["subscriptionFilters"][0]["destinationArn"]
                    .as_str()
                    .unwrap_or_default();
                assert_eq!(
                    destination, function_arn,
                    "case {case:?}: expected the declared filter to name the function the same \
                     stack created, got {described_filters}"
                );
            }
        }
        logs_ok(
            &router,
            case,
            "CreateLogStream",
            json!({ "logGroupName": row.group(), "logStreamName": row.stream() }),
        )
        .await;
    }

    // One batch per row, every event stamped with the same instant inside the ingestion
    // window, so what the function is handed is decided by the filter and not by admission.
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64");
    for row in ROWS {
        let events: Vec<Value> = row
            .batch
            .iter()
            .map(|written| json!({ "timestamp": now_ms, "message": written.message }))
            .collect();
        logs_ok(
            &router,
            row.case,
            "PutLogEvents",
            json!({
                "logGroupName": row.group(),
                "logStreamName": row.stream(),
                "logEvents": events,
            }),
        )
        .await;
    }

    // Each row's batch is small enough to travel as ONE envelope, so one row owes one printed
    // document; the wait is for all of them to be there.
    let (envelopes, other_lines) = printed_at_least(&router, ROWS.len()).await;
    assert_eq!(
        envelopes.len(),
        ROWS.len(),
        "expected the destination function to have printed one envelope per subscribed log \
         group ({} of them), got {envelopes:?} beside {other_lines:?}",
        ROWS.len()
    );

    let mut ids_across_rows: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        let group = row.group();
        let matching: Vec<&Value> = envelopes
            .iter()
            .filter(|envelope| envelope.get("logGroup").and_then(Value::as_str) == Some(&group))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one envelope naming the log group {group:?}, got \
             {envelopes:?}"
        );
        let envelope = matching[0];

        // Who the envelope says handed it over, and from where: the owning account, the group,
        // the stream the batch was written to and the filter that selected the events.
        assert_eq!(
            text_member(case, envelope, "owner"),
            account,
            "case {case:?}: the envelope's owner is the account holding the log group, got \
             {envelope}"
        );
        assert_eq!(
            text_member(case, envelope, "logStream"),
            row.stream(),
            "case {case:?}: the envelope names the stream the batch was written to, got \
             {envelope}"
        );
        assert_eq!(
            text_member(case, envelope, "messageType"),
            DATA_MESSAGE,
            "case {case:?}: an envelope carrying ingested log events is a {DATA_MESSAGE}, got \
             {envelope}"
        );
        let selecting: Vec<String> = envelope
            .get("subscriptionFilters")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            selecting,
            vec![row.filter()],
            "case {case:?}: the envelope names exactly the filter that selected the events, got \
             {envelope}"
        );

        // ... and WHICH events: exactly the ones the row's pattern selects, in the order they
        // were written. An event the pattern rejects is absent from this list, which is the
        // same batch's hand-over, so its absence is the pattern withholding it rather than a
        // delivery still in flight.
        let carried: Vec<String> = envelope
            .get("logEvents")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event.get("message").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            carried,
            row.selected(),
            "case {case:?}: expected the envelope to carry exactly the events the pattern {:?} \
             selects, got {envelope}",
            row.pattern
        );

        // Each delivered event is identified, and no two events anywhere share an id: the
        // envelope's `id` is the event's own handle rather than a repeat of its message.
        let events = envelope
            .get("logEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for event in &events {
            let id = text_member(case, event, "id");
            assert!(
                !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()),
                "case {case:?}: a delivered event's id is the service's own opaque decimal \
                 handle, got {id:?}"
            );
            assert!(
                ids_across_rows.insert(id.clone()),
                "case {case:?}: the id {id:?} was already carried for another event, got \
                 {envelope}"
            );
            assert_eq!(
                event.get("timestamp").and_then(Value::as_i64),
                Some(now_ms),
                "case {case:?}: a delivered event keeps the timestamp it was written with, got \
                 {event}"
            );
        }
    }

    // The runs that printed those documents really finished, which is the other half of a
    // hand-over being observable: AWS Lambda delivers the framing of every invocation it
    // completes into the same group.
    let reports = other_lines
        .iter()
        .filter(|message| message.contains(REPORT_PREFIX))
        .count();
    assert!(
        reports >= ROWS.len(),
        "expected one {REPORT_PREFIX:?} record per hand-over ({} of them) in the destination \
         function's log group, got {reports} among {other_lines:?}",
        ROWS.len()
    );
}
