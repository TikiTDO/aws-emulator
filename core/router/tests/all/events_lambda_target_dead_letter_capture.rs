// SPDX-License-Identifier: CC0-1.0

//! What an EventBridge rule target's DEAD-LETTER queue receives when the rule cannot hand a
//! matched event to its Lambda function target — end to end over the real
//! `MultiServiceRouter` with EventBridge, Lambda and Amazon SQS mounted on one engine.
//!
//! The rule pinned here: a delivery EventBridge could not make is written to the queue the
//! target's `DeadLetterConfig` names, carrying as its BODY exactly the bytes that target
//! would have been handed — the whole event envelope, or the reshaping the target asked for
//! (`Target$Input`, `Target$InputPath`, `Target$InputTransformer`) — plus the custom message
//! attributes that name the failed delivery: `RULE_ARN` and `TARGET_ARN` for which handover
//! it was, and `ERROR_CODE`/`ERROR_MESSAGE` for why it failed. A target the delivery
//! SUCCEEDS for dead-letters nothing, and an undeliverable target that names no dead-letter
//! queue writes nowhere.
//!
//! The rows differ along what the rule generalises over: what the target asks to be
//! delivered (so the dead letter's body is the delivery's, not a fixed rendering of the
//! event), WHY the handover could not be made, and whether the target carries a
//! `DeadLetterConfig` at all. The failures span the two classes a function target has:
//! EventBridge withholding the invocation because no document admits it — a function
//! carrying no resource-based policy, and a function that is not there at all, which is the
//! same absence ("for Lambda functions … EventBridge relies on resource-based policies",
//! `PutTargets`) — and the function's own service REFUSING the invocation. The `ERROR_CODE`
//! is EventBridge's own classification of the failure in both classes, never the target
//! service's error code: the permission condition where no document admits the invocation,
//! and `UNKNOWN` where the function's service answered a refusal that classification does
//! not place — whose own code and sentence the `ERROR_MESSAGE` beside it carries in full.
//!
//! What is NOT dead-lettered here is as much of the rule: EventBridge invokes a function
//! target asynchronously ("Event – Invoke the function asynchronously",
//! `Invoke$InvocationType`), so a function that ACCEPTS the queued event is a delivery
//! EventBridge made, whatever the run then comes to. The granted-and-invocable row pins that
//! boundary — its deployment package holds no handler for the run to execute, so the
//! accepted event comes to nothing, and the target's dead-letter queue still holds nothing:
//! a failed RUN belongs to Lambda's own asynchronous retry and to the function's
//! `DestinationConfig.OnFailure` record, not to the queue this target names. That is why no
//! row here dead-letters a handler that raised: the delivery that reaches the dead-letter
//! queue is one EventBridge could not HAND OVER, which is what the row below publishing
//! inside the function's creation transition ("During this time, you can't invoke or modify
//! the function", `CreateFunction`) exercises.
//!
//! `core/router/tests/all/events_lambda_target_resource_policy.rs` is the suite for WHICH
//! policies admit the invocation; this one is the suite for what the dead-lettered message
//! itself carries.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and what the
/// rules match on.
const SOURCE: &str = "lambda.target.deadletter";
const DETAIL_TYPE: &str = "Dead Letter Probe";

/// The principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The IAM action an invocation of a function performs, which is what a grant for this
/// service has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A deployment package as `Code.ZipFile` carries it: an empty archive, which holds no
/// handler for a run to execute. No row of this suite asserts anything a handler computed —
/// what the granted row asserts is that the dead-letter queue stays empty even so, because
/// the asynchronous invocation EventBridge made was ACCEPTED.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// The condition EventBridge names on a dead letter for a delivery it WITHHELD because no
/// document admits the invocation, spelled here rather than imported from the service so a
/// wrong value in the service would fail this suite.
const NO_PERMISSIONS: &str = "NO_PERMISSIONS";

/// The condition it names for a refusal its classification does not place: the target's
/// service answered something that is neither a refusal of the identity nor a resource that
/// is not there, so the condition says only that, and the answer itself is what the
/// `ERROR_MESSAGE` beside it carries. Spelled here for the same reason.
const UNKNOWN: &str = "UNKNOWN";

/// The error AWS Lambda answers an invocation of a function that is not invocable yet, which
/// is the sentence such a delivery's `ERROR_MESSAGE` carries — its wire code is in that
/// message too, where the whole of the service's answer is reported.
const RESOURCE_CONFLICT: &str = "ResourceConflictException";

/// A well-formed execution role ARN for the created functions.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(200);

/// EventBridge, Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
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
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
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

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// An SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
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

/// A dead-letter queue whose OWN policy admits the dead-letter write, so what reaches it is
/// decided by the function target's document rather than by the queue's ("for … Amazon SQS
/// queues, EventBridge relies on resource-based policies"). Answers its url and the ARN a
/// target names it by, read back from the queue rather than built here.
async fn given_dead_letter_queue(
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

/// Every message waiting on the queue at `url`, with its attributes.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answer = sqs(
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
    answer["Messages"].as_array().cloned().unwrap_or_default()
}

/// The `detail` every event this suite publishes carries.
fn detail() -> Value {
    json!({ "orderId": "A-1", "amount": 7 })
}

/// What the dead-lettered body has to be: the bytes the TARGET would have received.
#[derive(Debug, Clone)]
enum Delivered {
    /// The whole event envelope, as a target asking for no reshaping receives it.
    Envelope,
    /// Exactly this JSON document, as the target's own input configuration renders it.
    Document(Value),
}

/// What the dead-lettered message has to carry, for a row that dead-letters one.
#[derive(Debug, Clone)]
struct Expected {
    /// The bytes the TARGET would have received, which are the dead letter's body.
    delivered: Delivered,
    /// Why the handover could not be made, which decides its `ERROR_CODE` and
    /// `ERROR_MESSAGE`.
    failure: Failure,
}

/// Why EventBridge could not hand the event over, in the two classes a function target has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// No document admits this service the invocation, so EventBridge WITHHELD it and never
    /// reached the function's API: a function carrying no resource-based policy, and one
    /// that is not there at all, are the same absence.
    NoPolicyAdmitsIt,
    /// The function's own service refused the invocation, because the function is still
    /// inside the creation transition in which "you can't invoke or modify the function"
    /// (`CreateFunction`).
    TheFunctionRefusedIt,
}

impl Failure {
    /// The `ERROR_CODE` the dead letter reports, which is EventBridge's OWN classification
    /// of why the delivery failed and never the target service's error code: the permission
    /// condition for an invocation no document admits, and the unplaced one for a refusal
    /// the function's own service answered.
    fn code(self) -> &'static str {
        match self {
            Self::NoPolicyAdmitsIt => NO_PERMISSIONS,
            Self::TheFunctionRefusedIt => UNKNOWN,
        }
    }

    /// A sentence the `ERROR_MESSAGE` for the function `arn` has to carry: the one AWS
    /// Lambda answers a caller its resource-based policy does not permit, naming the
    /// principal, the action and the resource; and, for a refusal the function's own service
    /// answered, that service's own code and sentence, which the message reports in full
    /// beside the condition.
    fn reported_sentence(self, arn: &str) -> String {
        match self {
            Self::NoPolicyAdmitsIt => format!(
                "User: {EVENTS_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on \
                 resource: {arn} because no resource-based policy allows the \
                 {INVOKE_ACTION} action"
            ),
            Self::TheFunctionRefusedIt => format!(
                "{RESOURCE_CONFLICT}: The operation cannot be performed at this time. The \
                 function is currently in the following state: Pending"
            ),
        }
    }
}

/// One row: what the target asks to be delivered, why the handover fails, and what the
/// dead-letter queue must hold afterwards.
struct Row {
    case: &'static str,
    /// The members this row adds to the target beside its `Id` and `Arn`.
    input: Value,
    /// Whether the function's resource-based policy admits this service the invocation.
    granted: bool,
    /// Whether the function is deleted again after the target is written, so the delivery
    /// finds no function and no policy.
    delete_function: bool,
    /// Whether the event is published past the function's creation transition, so the
    /// function is invocable when the handover is made.
    past_the_transition: bool,
    /// Whether the target names a dead-letter queue at all.
    dead_letter_config: bool,
    /// What the dead-letter queue must hold: nothing, or the one message this describes.
    expected: Option<Expected>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a target taking the event as published dead-letters the whole envelope",
            input: json!({}),
            granted: false,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Envelope,
                failure: Failure::NoPolicyAdmitsIt,
            }),
        },
        Row {
            case: "a target carrying Input dead-letters that constant text",
            input: json!({ "Input": "{\"fixed\":\"text\"}" }),
            granted: false,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Document(json!({ "fixed": "text" })),
                failure: Failure::NoPolicyAdmitsIt,
            }),
        },
        Row {
            case: "a target carrying InputPath dead-letters the part it selects",
            input: json!({ "InputPath": "$.detail.orderId" }),
            granted: false,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Document(json!("A-1")),
                failure: Failure::NoPolicyAdmitsIt,
            }),
        },
        Row {
            case: "a target carrying an InputTransformer dead-letters its rendering",
            input: json!({
                "InputTransformer": {
                    "InputPathsMap": { "amount": "$.detail.amount" },
                    "InputTemplate": "{\"n\": <amount>}",
                },
            }),
            granted: false,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Document(json!({ "n": 7 })),
                failure: Failure::NoPolicyAdmitsIt,
            }),
        },
        Row {
            case: "a target whose function is no longer there dead-letters the envelope",
            input: json!({}),
            granted: true,
            delete_function: true,
            past_the_transition: true,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Envelope,
                failure: Failure::NoPolicyAdmitsIt,
            }),
        },
        Row {
            // The grant is in force here, so what stops the handover is the FUNCTION, and
            // the dead letter reports the code that service answered rather than a
            // permission condition.
            case: "a target whose granted function is not invocable yet dead-letters under \
                   the code that function answered",
            input: json!({}),
            granted: true,
            delete_function: false,
            past_the_transition: false,
            dead_letter_config: true,
            expected: Some(Expected {
                delivered: Delivered::Envelope,
                failure: Failure::TheFunctionRefusedIt,
            }),
        },
        Row {
            case: "a target whose policy admits the invocation dead-letters nothing, even \
                   though the accepted event's run has no handler to execute",
            input: json!({}),
            granted: true,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: true,
            expected: None,
        },
        Row {
            case: "an undeliverable target naming no dead-letter queue writes nowhere",
            input: json!({}),
            granted: false,
            delete_function: false,
            past_the_transition: true,
            dead_letter_config: false,
            expected: None,
        },
    ]
}

/// Every row: what the target's dead-letter queue holds after one matching `PutEvents`.
#[tokio::test]
async fn an_undeliverable_function_target_dead_letters_what_it_would_have_received() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router();
        let function_name = format!("dead-letter-probe-{index}");
        let rule_name = format!("dead-letter-probe-rule-{index}");
        let (dead_letter_url, dead_letter_arn) =
            given_dead_letter_queue(&router, case, &format!("dead-letter-probe-dlq-{index}"))
                .await;
        // The account every emulated resource here belongs to, taken from the queue the
        // emulator just created rather than written in by hand.
        let account = dead_letter_arn
            .split(':')
            .nth(4)
            .unwrap_or_default()
            .to_string();

        let (status, function) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            json!({
                "FunctionName": function_name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} \
             ({function})"
        );
        let function_arn = function["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {function}"))
            .to_string();

        if row.granted {
            let (status, granted) = lambda(
                &router,
                "POST",
                &format!("/2015-03-31/functions/{function_name}/policy"),
                json!({
                    "StatementId": "events-invoke",
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} \
                 ({granted})"
            );
        }

        let rule = events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule_name,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let rule_arn = rule["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
            .to_string();

        let mut target = json!({ "Id": "fn", "Arn": function_arn });
        for (member, value) in row.input.as_object().into_iter().flatten() {
            target[member] = value.clone();
        }
        if row.dead_letter_config {
            target["DeadLetterConfig"] = json!({ "Arn": dead_letter_arn });
        }
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({ "Rule": rule_name, "Targets": [target] }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target to be accepted, got {wired}"
        );

        if row.delete_function {
            let (status, answer) = lambda(
                &router,
                "DELETE",
                &format!("/2015-03-31/functions/{function_name}"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected HTTP 204 from Lambda DeleteFunction, got {status} \
                 ({answer})"
            );
        }

        // A created function leaves `Pending` by the clock. A row that wants the function
        // INVOCABLE waits past that span, so its refusal is the policy's rather than the
        // lifecycle's; the row that wants the lifecycle's publishes at once.
        if row.past_the_transition {
            tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
        } else if !row.delete_function {
            // The state the row's expectation rests on, read back immediately before the
            // event is published: a row that means to catch the function inside its
            // creation transition says so here, so losing that race reports the state it
            // found rather than an attribute that does not match.
            let (status, configuration) = lambda(
                &router,
                "GET",
                &format!("/2015-03-31/functions/{function_name}/configuration"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from Lambda GetFunctionConfiguration, got \
                 {status} ({configuration})"
            );
            assert_eq!(
                configuration["State"],
                json!("Pending"),
                "case {case:?}: this row publishes inside the function's creation \
                 transition, so the function must still be Pending, got {configuration}"
            );
        }

        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": detail().to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the event to be accepted, got {published}"
        );

        let dead_letters = drain(&router, case, &dead_letter_url).await;
        let expected_count = usize::from(row.expected.is_some());
        assert_eq!(
            dead_letters.len(),
            expected_count,
            "case {case:?}: expected {expected_count} dead letter(s), got {dead_letters:?}"
        );
        let Some(expected) = row.expected.as_ref() else {
            continue;
        };

        let message = &dead_letters[0];
        let body = message["Body"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a message Body, got {message}"));
        let carried: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: the dead-lettered body is not JSON ({error}): {body:?}")
        });
        match &expected.delivered {
            Delivered::Document(document) => assert_eq!(
                &carried, document,
                "case {case:?}: expected the dead-lettered body {document}, got {carried}"
            ),
            Delivered::Envelope => {
                for (member, value) in [
                    ("source", json!(SOURCE)),
                    ("detail-type", json!(DETAIL_TYPE)),
                    ("detail", detail()),
                    ("region", json!(REGION)),
                    ("account", json!(account)),
                ] {
                    assert_eq!(
                        carried[member], value,
                        "case {case:?}: expected the dead-lettered envelope's {member} to be \
                         {value}, got {carried}"
                    );
                }
                assert!(
                    carried["id"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {case:?}: expected the dead-lettered envelope to carry the event's \
                     id, got {carried}"
                );
            }
        }

        let attributes = &message["MessageAttributes"];
        assert_eq!(
            attributes["RULE_ARN"]["StringValue"],
            json!(rule_arn),
            "case {case:?}: expected the dead letter's RULE_ARN to name {rule_arn}, got \
             {message}"
        );
        assert_eq!(
            attributes["TARGET_ARN"]["StringValue"],
            json!(function_arn),
            "case {case:?}: expected the dead letter's TARGET_ARN to name {function_arn}, got \
             {message}"
        );
        let expected_code = expected.failure.code();
        assert_eq!(
            attributes["ERROR_CODE"]["StringValue"],
            json!(expected_code),
            "case {case:?}: expected the dead letter's ERROR_CODE to be {expected_code}, got \
             {message}"
        );
        // The sentence the refusal is reported in. A withheld invocation carries the one AWS
        // Lambda answers a caller its resource-based policy does not permit (the principal,
        // the action and the function); a refusal the function itself answered carries that
        // service's own sentence, inside the error context the SDK renders around it.
        let expected_sentence = expected.failure.reported_sentence(&function_arn);
        let reported = attributes["ERROR_MESSAGE"]["StringValue"]
            .as_str()
            .unwrap_or_default();
        assert!(
            reported.contains(&expected_sentence),
            "case {case:?}: expected the dead letter's ERROR_MESSAGE to carry \
             {expected_sentence:?}, got {reported:?}"
        );
    }
}
