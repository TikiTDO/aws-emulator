// SPDX-License-Identifier: CC0-1.0

//! A Lambda-target handover is decided against the FUNCTION's resource-based policy AS IT
//! STANDS at that delivery, so a grant a client adds or removes BETWEEN publications governs
//! every matching event that follows it and none that came before.
//!
//! The RULE pinned here: "for Lambda functions, Amazon SNS topics and Amazon SQS queues,
//! EventBridge relies on resource-based policies" (`PutTargets`), and the document in force is
//! the one the function carries when the event is routed — not one read once when the target
//! was written. So each delivery of one unchanged rule and one unchanged target is decided on
//! its own: while the function's policy allows the service principal `lambda:InvokeFunction`
//! for the invoking rule, each matching event invokes the function once and is counted on the
//! rule's `Invocations`; while it does not, nothing is invoked, the envelope goes to the
//! target's dead-letter queue carrying Lambda's own authorization refusal, and the attempt is
//! counted on the rule's `FailedInvocations` instead.
//!
//! That is what tells a client "correctly refused for lack of permission" from "nothing is
//! ever invoked": the two states are reached by one `AddPermission`/`RemovePermission` apart,
//! on one rule, one target and one event pattern, and each leg's three observables — the
//! function's own queued invocations, the dead-letter queue, and the rule's AWS/Events
//! counters — move only for the leg whose policy admits or refuses it.
//!
//! The rows differ along what the rule generalises over: the ORDER the legs come in (refused
//! then granted, granted then revoked, alternating), the FORM of the grant in force
//! (unscoped, scoped by `aws:SourceArn` to the invoking rule, scoped to another rule of the
//! bus), and HOW MANY matching events each leg publishes, so a count is a count rather than a
//! flag.
//!
//! `lambda_target_handler_records_the_invoking_rule` is the suite where a permitted target's
//! handler really RUNS; what this one adds is that the decision is taken per delivery, and
//! the accounting of each leg.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_EVENT_BUS_NAME, EVENT_BUS_NAME_DIMENSION, FAILED_INVOCATIONS_METRIC,
    INVOCATIONS_METRIC, METRIC_NAMESPACE, NO_PERMISSIONS, RULE_NAME_DIMENSION, SERVICE_PRINCIPAL,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and what the rule
/// matches on.
const SOURCE: &str = "com.demo.grant-timing";
const DETAIL_TYPE: &str = "Grant Timing Probe";

/// The IAM action an invocation of a function performs, which is what a grant for this service
/// has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The id of the one statement a row's grant is written under, so the next leg can take it
/// away again by name.
const STATEMENT_ID: &str = "events-invoke";

/// A deployment package as `Code.ZipFile` carries it. What it holds does not matter here: only
/// that Lambda accepts the function and records the invocation EventBridge makes.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// Past the span a created function spends in `Pending` before it is `Active`, so a delivery
/// that arrives after it is one the function's lifecycle does not refuse.
const PAST_THE_FUNCTION_TRANSITION: Duration =
    ACTIVATION_DELAY.saturating_add(Duration::from_millis(300));

/// How long a leg waits for what it expects on the dead-letter queue before giving up on it.
const DEAD_LETTER_WAIT: Duration = Duration::from_secs(10);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// EventBridge, Lambda, Amazon SQS and CloudWatch over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the function the rule targets, the queue it
/// dead-letters to and the counters the rule publishes all live beside this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
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
    emulator: &Emulator,
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
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A CloudWatch call, which is where this service's own counters are read back from.
async fn cloudwatch(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(emulator: &Emulator, method: &str, uri: &str, body: Value) -> (u16, Value) {
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
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The account every emulated resource of this suite belongs to, as the engine holds it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Create the function the rule targets and answer the ARN Lambda assigned it.
async fn create_function(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/service-role/lambda-exec", account()),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Code": { "ZipFile": ZIP_FILE },
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

/// Grant this service the invocation on `function` through `AddPermission`, optionally scoped
/// by `aws:SourceArn` — which is how a client repairs a refused delivery.
async fn add_permission(
    emulator: &Emulator,
    case: &str,
    function: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": SERVICE_PRINCIPAL,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answered) = lambda(
        emulator,
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

/// Take that statement away again through `RemovePermission`, which is how a client revokes it.
async fn remove_permission(emulator: &Emulator, case: &str, function: &str) {
    let (status, answered) = lambda(
        emulator,
        "DELETE",
        &format!("/2015-03-31/functions/{function}/policy/{STATEMENT_ID}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected HTTP 204 from Lambda RemovePermission, got {status} \
         ({answered})"
    );
}

/// The queue the target dead-letters to, carrying the policy this service's own send needs;
/// answers its URL and its ARN.
async fn dead_letter_queue(emulator: &Emulator, case: &str, name: &str) -> (String, String) {
    let created = sqs(emulator, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
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
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        emulator,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    (url, arn)
}

/// A rule called `name` on the default bus, matching every event this suite publishes;
/// answers the ARN EventBridge assigned it.
async fn put_rule(emulator: &Emulator, case: &str, name: &str, state: &str) -> String {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let rule = events(
        emulator,
        case,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": state }),
    )
    .await;
    rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
        .to_string()
}

/// How many asynchronous invocations Lambda holds for `function` — what the `Event` invocation
/// EventBridge makes for a function target persists, read from the service that OWNS it.
fn queued_invocations(emulator: &Emulator, function: &str) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map_or(0, |record| record.async_invocations.len())
}

/// Every message waiting on the queue at `url`, taken OFF it, so what a later leg reads is
/// that leg's own dead letters.
async fn drain(emulator: &Emulator, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    let held = answered["Messages"].as_array().cloned().unwrap_or_default();
    for message in &held {
        let receipt = message["ReceiptHandle"].as_str().unwrap_or_default();
        sqs(
            emulator,
            case,
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
        )
        .await;
    }
    held
}

/// Wait until `wanted` messages are on the queue at `url`, and answer what was taken off it.
/// A leg expecting none still waits, so a dead letter that should not be there has time to
/// show up.
async fn dead_letters(emulator: &Emulator, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let looks = DEAD_LETTER_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut taken: Vec<Value> = Vec::new();
    for _ in 0..looks {
        taken.extend(drain(emulator, case, url).await);
        if taken.len() >= wanted {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    if wanted == 0 {
        // Nothing was owed: give a message that should not exist the same span to arrive as
        // one that should, then report whatever did.
        tokio::time::sleep(LOOK_INTERVAL * 4).await;
        taken.extend(drain(emulator, case, url).await);
    }
    taken
}

/// The instant this read is taken, as CloudWatch's own timestamps are.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The `Sum` of `metric` for `rule` on the default bus, over a window around now, in
/// EventBridge's own namespace.
async fn metric_sum(emulator: &Emulator, case: &str, metric: &str, rule: &str) -> f64 {
    let now = epoch_seconds();
    let stats = cloudwatch(
        emulator,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                { "Name": EVENT_BUS_NAME_DIMENSION, "Value": DEFAULT_EVENT_BUS_NAME },
                { "Name": RULE_NAME_DIMENSION, "Value": rule },
            ],
            "StartTime": now - 300.0,
            "EndTime": now + 300.0,
            "Period": 600,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    stats["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>()
        })
        .unwrap_or_default()
}

/// Publish one matching event carrying `nonce`, and assert it was accepted.
async fn publish(emulator: &Emulator, case: &str, nonce: &str) {
    let published = events(
        emulator,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": nonce }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the entry to be accepted, got {published}"
    );
}

/// What the function's resource-based policy says during one leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// No statement at all, so the function has granted this service nothing.
    None,
    /// `lambda:InvokeFunction` allowed to this service principal, unscoped.
    Unscoped,
    /// The same grant, scoped by `aws:SourceArn` to the rule that invokes the function.
    ScopedToThisRule,
    /// The same grant, scoped by `aws:SourceArn` to another rule of the same bus.
    ScopedToAnotherRule,
}

/// One leg of a row: the policy a client has left on the function, how many matching events
/// are then published, and whether each of them invokes the function.
struct Leg {
    policy: Policy,
    events: usize,
    invoked: bool,
}

/// One row: a sequence of legs over ONE rule, ONE target and ONE function.
struct Row {
    case: &'static str,
    legs: Vec<Leg>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded sequence: the ungranted publication is refused, and the grant a
            // client then adds admits the next matching event.
            case: "an unscoped grant added after a refused publication admits the next event",
            legs: vec![
                Leg {
                    policy: Policy::None,
                    events: 1,
                    invoked: false,
                },
                Leg {
                    policy: Policy::Unscoped,
                    events: 1,
                    invoked: true,
                },
            ],
        },
        Row {
            case: "a grant scoped to the invoking rule, added after two refused publications",
            legs: vec![
                Leg {
                    policy: Policy::None,
                    events: 2,
                    invoked: false,
                },
                Leg {
                    policy: Policy::ScopedToThisRule,
                    events: 1,
                    invoked: true,
                },
            ],
        },
        Row {
            case: "the grant revoked again stops admitting the publications that follow",
            legs: vec![
                Leg {
                    policy: Policy::Unscoped,
                    events: 1,
                    invoked: true,
                },
                Leg {
                    policy: Policy::None,
                    events: 2,
                    invoked: false,
                },
            ],
        },
        Row {
            case: "a grant scoped to another rule is replaced by one scoped to this rule",
            legs: vec![
                Leg {
                    policy: Policy::ScopedToAnotherRule,
                    events: 1,
                    invoked: false,
                },
                Leg {
                    policy: Policy::ScopedToThisRule,
                    events: 2,
                    invoked: true,
                },
            ],
        },
        Row {
            case: "the grant comes and goes, so each publication is decided on its own",
            legs: vec![
                Leg {
                    policy: Policy::None,
                    events: 1,
                    invoked: false,
                },
                Leg {
                    policy: Policy::Unscoped,
                    events: 1,
                    invoked: true,
                },
                Leg {
                    policy: Policy::None,
                    events: 1,
                    invoked: false,
                },
                Leg {
                    policy: Policy::ScopedToThisRule,
                    events: 1,
                    invoked: true,
                },
            ],
        },
    ]
}

/// Leave `policy` on the function: the statement a previous leg wrote is taken away first, so
/// what the next publication is decided against is this leg's document and nothing else.
async fn leave_policy(
    emulator: &Emulator,
    case: &str,
    function: &str,
    policy: Policy,
    rule_arn: &str,
    other_rule_arn: &str,
    held: &mut bool,
) {
    if *held {
        remove_permission(emulator, case, function).await;
        *held = false;
    }
    let source_arn = match policy {
        Policy::None => return,
        Policy::Unscoped => None,
        Policy::ScopedToThisRule => Some(rule_arn),
        Policy::ScopedToAnotherRule => Some(other_rule_arn),
    };
    add_permission(emulator, case, function, source_arn).await;
    *held = true;
}

/// RULE: each delivery to a Lambda target is decided against the policy the function carries
/// at that moment, and its outcome is counted on the rule's own AWS/Events counters.
#[tokio::test]
async fn each_delivery_is_decided_against_the_functions_policy_as_it_then_stands() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let function_name = format!("grant-timing-{index}");
        let rule_name = format!("grant-timing-rule-{index}");
        let other_rule_name = format!("grant-timing-other-{index}");

        let (dead_letter_url, dead_letter_arn) =
            dead_letter_queue(&emulator, case, &format!("grant-timing-dlq-{index}")).await;
        let function_arn = create_function(&emulator, case, &function_name).await;
        let rule_arn = put_rule(&emulator, case, &rule_name, "ENABLED").await;
        // A second rule of the same bus, so a grant scoped to another rule names a rule this
        // service really holds. It is disabled, so it matches nothing itself.
        let other_rule_arn = put_rule(&emulator, case, &other_rule_name, "DISABLED").await;

        let wired = events(
            &emulator,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{
                    "Id": "fn",
                    "Arn": function_arn,
                    "RetryPolicy": { "MaximumRetryAttempts": 0 },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
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

        let mut held = false;
        let mut invoked_total = 0usize;
        let mut failed_total = 0usize;
        for (leg_index, leg) in row.legs.iter().enumerate() {
            let leg_case = format!("{case} (leg {leg_index}, policy {:?})", leg.policy);
            let leg_case = leg_case.as_str();
            leave_policy(
                &emulator,
                leg_case,
                &function_name,
                leg.policy,
                &rule_arn,
                &other_rule_arn,
                &mut held,
            )
            .await;

            for round in 0..leg.events {
                publish(&emulator, leg_case, &format!("{leg_index}-{round}")).await;
            }
            if leg.invoked {
                invoked_total += leg.events;
            } else {
                failed_total += leg.events;
            }

            // What the FUNCTION holds: one queued invocation per admitted delivery and none
            // for a refused one, so a refusal never reached the function's API.
            let queued = queued_invocations(&emulator, &function_name);
            assert_eq!(
                queued, invoked_total,
                "case {leg_case:?}: expected the function to hold {invoked_total} queued \
                 invocation(s) by now, got {queued}"
            );

            // What reached the DEAD-LETTER QUEUE: one envelope per refused delivery, carrying
            // the permission condition and Lambda's own authorization sentence, and nothing at
            // all for a leg whose deliveries were made.
            let wanted = if leg.invoked { 0 } else { leg.events };
            let letters = dead_letters(&emulator, leg_case, &dead_letter_url, wanted).await;
            assert_eq!(
                letters.len(),
                wanted,
                "case {leg_case:?}: expected {wanted} dead letter(s) for this leg, got \
                 {letters:?}"
            );
            let expected_opening = format!(
                "User: {SERVICE_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on \
                 resource: {function_arn} because no resource-based policy allows the \
                 {INVOKE_ACTION} action"
            );
            for letter in &letters {
                let attributes = &letter["MessageAttributes"];
                assert_eq!(
                    attributes["ERROR_CODE"]["StringValue"],
                    json!(NO_PERMISSIONS),
                    "case {leg_case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS}, \
                     got {letter}"
                );
                let reported = attributes["ERROR_MESSAGE"]["StringValue"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    reported.starts_with(&expected_opening),
                    "case {leg_case:?}: expected the dead letter to carry the refusal opening \
                     {expected_opening:?}, got {reported:?}"
                );
            }

            // And the ACCOUNTING: every delivery of the rule is counted once, on the counter
            // its own outcome decides.
            let counted = [
                (INVOCATIONS_METRIC, invoked_total),
                (FAILED_INVOCATIONS_METRIC, failed_total),
            ];
            for (metric, want) in counted {
                let expected = f64::from(u32::try_from(want).unwrap_or(u32::MAX));
                let got = metric_sum(&emulator, leg_case, metric, &rule_name).await;
                assert_eq!(
                    got, expected,
                    "case {leg_case:?}: expected {metric} for rule {rule_name} to sum to \
                     {expected}, got {got}"
                );
            }
        }
    }
}
