// SPDX-License-Identifier: CC0-1.0

//! An EventBridge rule's Lambda target RUNS, and what the run does is observed where a client
//! observes it — in Parameter Store — end to end over the wire: EventBridge, Lambda, AWS
//! Systems Manager and Amazon SQS on ONE engine, and a `python3.x` function whose code does
//! `import boto3` and writes what the invocation handed it.
//!
//! The RULE pinned here: a matched rule hands the event to its Lambda target only where the
//! FUNCTION's own resource-based policy allows `events.amazonaws.com` `lambda:InvokeFunction`
//! on it for the INVOKING rule ("for Lambda and Amazon SNS resources, EventBridge relies on
//! resource-based policies", `PutTargets`); where it does, the handler really runs and the
//! invoking rule's identity travels with the invocation, so the `aws.events.rule-arn` the
//! target's `InputTransformer` names is the ARN of the rule that matched — and the account
//! that ARN's own account field carries is the account the rule belongs to; where it does not,
//! nothing is invoked and the event goes to the queue the target's `DeadLetterConfig` names
//! carrying the permission condition.
//!
//! Both halves are asserted at the function's OWN side effect: the handler writes what it was
//! handed to a parameter, and the suite reads that parameter through Systems Manager's own
//! `GetParameter`. That is what tells a client the difference between a correct permission
//! refusal and a bus that delivers nothing — the permitted arm writes the parameter, the
//! refused arm writes none and dead-letters instead.
//!
//! The rows differ along what the rule generalises over: whether the function carries a
//! statement at all, and — where it does — whether the grant is scoped by `aws:SourceArn` and
//! whether that scope names the rule that invokes it.
//! `core/router/tests/all/events_lambda_target_resource_policy.rs` is the suite for the whole
//! table of policy shapes a handover is decided by; what this one adds is the handler actually
//! RUNNING and the invoking rule's ARN reaching it.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and what the rule
/// matches on.
const SOURCE: &str = "com.demo.rule-provenance";
const DETAIL_TYPE: &str = "Rule Provenance Probe";

/// The principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The IAM action an invocation of a function performs, which is what a grant for this service
/// has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// Past the span a created function spends in `Pending` before it is `Active`, so a delivery
/// that arrives after it is one the function's lifecycle does not refuse.
const PAST_THE_FUNCTION_TRANSITION: Duration =
    ACTIVATION_DELAY.saturating_add(Duration::from_millis(300));

/// How long this suite waits for work the clock owes — the asynchronous delivery of the
/// invocation and the handler's own write — before giving up on it.
const WORK_WAIT: Duration = Duration::from_secs(30);

/// How long it watches a place nothing must reach, after the dead letter has shown the
/// handover was already decided.
const QUIET_WAIT: Duration = Duration::from_secs(3);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// EventBridge, Lambda, AWS Systems Manager and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own SDK call is dispatched back into
/// this same router, so the parameter it writes is the parameter this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Systems Manager call (`awsJson1_1`, `AmazonSSM.<Operation>`), as its status and parsed
/// body — a read of a name no parameter answers to is that service's refusal, so the status is
/// answered rather than asserted here.
async fn ssm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "ssm", "AmazonSSM", "1.1", operation, body).await
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The handler every row deploys: it writes what the invocation handed it into Parameter Store,
/// under the parameter name the event itself names, and extracts the ACCOUNT out of the
/// invoking rule's ARN the way the ARN format states it — the account id is the fifth
/// colon-separated field of `arn:<partition>:<service>:<region>:<account>:<resource>`.
const RECORDING_SOURCE: &str = r#"
import json

import boto3


def lambda_handler(event, context):
    rule_arn = event["rule_arn"]
    recorded = {
        "rule_arn": rule_arn,
        "account": rule_arn.split(":")[4],
        "rule_name": event["rule_name"],
        "marker": event["marker"],
    }
    boto3.client("ssm").put_parameter(
        Name=event["parameter"],
        Value=json.dumps(recorded),
        Type="String",
        Overwrite=True,
    )
    return recorded
"#;

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
            .write_all(RECORDING_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
}

/// The account every emulated resource of this suite belongs to, as the engine holds it —
/// which is the account the rule's ARN carries and therefore the one the handler extracts.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Create the function the rule targets and answer the ARN Lambda assigned it.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/service-role/lambda-exec", account()),
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 20,
            "Code": { "ZipFile": package() },
        }),
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
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": "events-invoke",
        "Action": INVOKE_ACTION,
        "Principal": EVENTS_PRINCIPAL,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answered) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answered})"
    );
}

/// The queue the target dead-letters to, carrying the policy this service's own send needs;
/// answers its URL and its ARN.
async fn dead_letter_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    (url, arn)
}

/// What Systems Manager reports for `name`: the parsed document the handler wrote, or `None`
/// while no parameter answers to that name.
async fn recorded(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, answered) = ssm(router, "GetParameter", &json!({ "Name": name })).await;
    if status != 200 {
        return None;
    }
    let value = answered["Parameter"]["Value"].as_str()?;
    serde_json::from_str(value).ok()
}

/// Wait until the parameter `name` is there, which is the handler having run and written it.
async fn until_recorded(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        if let Some(document) = recorded(router, name).await {
            return Some(document);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    recorded(router, name).await
}

/// Every message waiting on the queue at `url`, with its attributes, taken off it.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    answered["Messages"].as_array().cloned().unwrap_or_default()
}

/// Wait until something is on the queue at `url`, and answer what was taken off it.
async fn until_dead_lettered(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let held = drain(router, case, url).await;
        if !held.is_empty() {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    Vec::new()
}

/// What one row grants on the function before the event is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No `AddPermission` at all, so the function carries no policy.
    None,
    /// The service principal allowed the invocation, unscoped.
    Unscoped,
    /// The same grant, scoped by `aws:SourceArn` to the rule that invokes the function.
    ThisRule,
    /// The same grant, scoped by `aws:SourceArn` to another rule of the same bus.
    AnotherRule,
}

/// One row: what the function's policy says, and whether the handler therefore runs.
struct Row {
    case: &'static str,
    grant: Grant,
    /// Whether EventBridge invokes the function, so the handler writes its parameter.
    invoked: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the grant is scoped by SourceArn to the rule that invokes the function",
            grant: Grant::ThisRule,
            invoked: true,
        },
        Row {
            case: "the grant allows this service the invocation unscoped",
            grant: Grant::Unscoped,
            invoked: true,
        },
        Row {
            case: "the function carries no resource-based policy at all",
            grant: Grant::None,
            invoked: false,
        },
        Row {
            case: "the grant is scoped by SourceArn to another rule of the same bus",
            grant: Grant::AnotherRule,
            invoked: false,
        },
    ]
}

/// RULE: the function's own policy decides whether the handler runs at all, and a handler that
/// runs is handed the ARN of the rule that invoked it, from which the rule's account follows.
#[tokio::test]
async fn a_permitted_lambda_target_runs_and_records_the_invoking_rules_arn() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router();
        let function_name = format!("rule-provenance-{index}");
        let rule_name = format!("rule-provenance-rule-{index}");
        let other_rule_name = format!("rule-provenance-other-{index}");
        let parameter = format!("/emulator/rule-provenance/{index}");
        let marker = format!("marker-{index}");

        let (dead_letter_url, dead_letter_arn) =
            dead_letter_queue(&router, case, &format!("rule-provenance-dlq-{index}")).await;
        let function_arn = create_function(&router, case, &function_name).await;

        let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
        let rule = events(
            &router,
            case,
            "PutRule",
            &json!({ "Name": rule_name, "EventPattern": pattern, "State": "ENABLED" }),
        )
        .await;
        let rule_arn = rule["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
            .to_string();
        // A second rule of the same bus, so the grant one row scopes to another rule names a
        // rule this service really holds. It is disabled, so it matches nothing itself.
        let other = events(
            &router,
            case,
            "PutRule",
            &json!({ "Name": other_rule_name, "EventPattern": pattern, "State": "DISABLED" }),
        )
        .await;
        let other_rule_arn = other["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {other}"))
            .to_string();

        match row.grant {
            Grant::None => {}
            Grant::Unscoped => add_permission(&router, case, &function_name, None).await,
            Grant::ThisRule => {
                add_permission(&router, case, &function_name, Some(&rule_arn)).await;
            }
            Grant::AnotherRule => {
                add_permission(&router, case, &function_name, Some(&other_rule_arn)).await;
            }
        }

        // The target takes the event through an `InputTransformer`, so what the function is
        // handed carries the INVOKING rule's own identity — `aws.events.rule-arn` and
        // `aws.events.rule-name` are the placeholders EventBridge fills from the delivery —
        // beside the two values the event itself names.
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{
                    "Id": "fn",
                    "Arn": function_arn,
                    "RetryPolicy": { "MaximumRetryAttempts": 0 },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                    "InputTransformer": {
                        "InputPathsMap": {
                            "parameter": "$.detail.parameter",
                            "marker": "$.detail.marker",
                        },
                        "InputTemplate": "{\"rule_arn\": <aws.events.rule-arn>, \
                                           \"rule_name\": <aws.events.rule-name>, \
                                           \"parameter\": <parameter>, \"marker\": <marker>}",
                    },
                }],
            }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target to be accepted, got {wired}"
        );

        // "During this time, you can't invoke or modify the function" (`CreateFunction`): the
        // created function leaves `Pending` by the clock, so a handover withheld after this
        // span is the policy's refusal and not the lifecycle's.
        tokio::time::sleep(PAST_THE_FUNCTION_TRANSITION).await;

        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "parameter": parameter, "marker": marker }).to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the entry to be accepted, got {published}"
        );

        if row.invoked {
            let document = until_recorded(&router, &parameter).await.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the handler to run and write the parameter \
                     {parameter:?}, but Parameter Store holds none"
                )
            });
            // The invoking rule's own ARN reached the running handler, and the account it
            // carries is the account the rule belongs to.
            assert_eq!(
                document["rule_arn"],
                json!(rule_arn),
                "case {case:?}: expected the invoking rule's ARN {rule_arn:?}, got {document}"
            );
            assert_eq!(
                document["account"],
                json!(account()),
                "case {case:?}: expected the account of {rule_arn:?}, got {document}"
            );
            assert_eq!(
                document["rule_name"],
                json!(rule_name),
                "case {case:?}: expected the invoking rule's name {rule_name:?}, got {document}"
            );
            // The event's own detail travelled with it, so what ran is this publication's
            // delivery and not some other.
            assert_eq!(
                document["marker"],
                json!(marker),
                "case {case:?}: expected the published marker {marker:?}, got {document}"
            );
            // A delivery that was made dead-letters nothing. The read above has already
            // waited out the asynchronous delivery, so a message owed to this queue would be
            // on it by now.
            let dead_letters = drain(&router, case, &dead_letter_url).await;
            assert!(
                dead_letters.is_empty(),
                "case {case:?}: expected the permitted target to dead-letter nothing, got \
                 {dead_letters:?}"
            );
            continue;
        }

        // The refused arm: the event is dead-lettered carrying the permission condition, which
        // is what tells a client the invocation was REFUSED rather than never attempted.
        let dead_letters = until_dead_lettered(&router, case, &dead_letter_url).await;
        assert_eq!(
            dead_letters.len(),
            1,
            "case {case:?}: expected the refused handover to be dead-lettered once, got \
             {dead_letters:?}"
        );
        let reported = dead_letters[0]["MessageAttributes"]["ERROR_MESSAGE"]["StringValue"]
            .as_str()
            .unwrap_or_default();
        let expected_opening = format!(
            "User: {EVENTS_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on \
             resource: {function_arn} because no resource-based policy allows the \
             {INVOKE_ACTION} action"
        );
        assert!(
            reported.starts_with(&expected_opening),
            "case {case:?}: expected the dead letter to carry the refusal opening \
             {expected_opening:?}, got {reported:?}"
        );
        // And nothing ran: no parameter was written, however long the delivery is given.
        tokio::time::sleep(QUIET_WAIT).await;
        let held = recorded(&router, &parameter).await;
        assert!(
            held.is_none(),
            "case {case:?}: expected no function to run, so Parameter Store holds no \
             {parameter:?}, got {held:?}"
        );
    }
}
