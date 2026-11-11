// SPDX-License-Identifier: CC0-1.0

//! `PutEvents` DELIVERS a matched event to the targets of the rules that matched
//! it, end to end over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`) through the real `MultiServiceRouter`, with SQS
//! mounted on the same engine so a delivery is observed where a client observes
//! it: on the queue.
//!
//! The RULE asserted here: for every ENABLED rule on the entry's bus whose
//! `EventPattern` matches, each of its SQS queue targets receives one message
//! whose body is the WHOLE event envelope (`version`, `id`, `detail-type`,
//! `source`, `account`, `time`, `region`, `resources`, `detail`) — or, when the
//! target asks for something else, exactly what it asked for: the constant text
//! of `Input` ("Valid JSON text passed to the target. In this case, nothing from
//! the event itself is passed to the target", `Target$Input`), the part of the
//! event `InputPath` selects, or the rendering of an `InputTransformer`'s
//! template. A rule that does not match, and a rule that is `DISABLED`, deliver
//! nothing.
//!
//! Each test spans a TABLE whose rows differ along what decides the delivery:
//! the target's input configuration, how many targets a rule has, the target's
//! service, and whether the rule matches at all.
//!
//! The other target kinds a rule may carry are invoked too, each through its own
//! service's API; `tests/all/target_dispatch.rs` is the suite for that rule, and
//! this one keeps the queue case because a queue is where a delivery is easiest
//! to read back. `tests/all/input_transformation.rs` is the suite for WHAT a
//! transformed target receives, over the forms a path and a template may take.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The source and detail type the rules below match on.
const SOURCE: &str = "com.demo.widgets";
const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"A-1","amount":7}"#;

/// The emulator: EventBridge and SQS on one engine, because a rule's target is an
/// SQS queue and the delivery is asserted on the queue.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// A queue called `name`, carrying the policy a rule's target needs: "for Lambda
/// functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`), so a queue that has granted the service
/// nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: SetQueueAttributes answered {status} {body}"
    );
    (url, arn)
}

/// Every message body waiting on the queue at `url`, as raw text.
async fn drain(emulator: &Emulator, url: &str) -> Vec<String> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A rule called `name` with `pattern` and `state`, carrying `targets`.
async fn given_rule(emulator: &Emulator, name: &str, pattern: &str, state: &str, targets: Value) {
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": state }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "Targets": targets }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {body}"
    );
}

/// The pattern that matches the event every test below publishes.
fn matching_pattern() -> String {
    json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string()
}

/// Publish the one event every test below routes, and answer its `EventId`.
async fn publish(emulator: &Emulator) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {body}"))
        .to_string()
}

/// What a row expects on the target's queue.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// One message carrying the whole event envelope.
    WholeEvent,
    /// One message carrying exactly this text.
    Constant(&'static str),
}

/// The envelope members every delivered event carries, with the values this
/// publication fixes. Source: the EventBridge event-structure reference (and the
/// `PutEvents` entry members the event is built from).
fn assert_whole_envelope(case: &str, body: &str, event_id: &str) {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case}: the delivered body is not JSON ({error})"));
    let rows = [
        ("version", json!("0")),
        ("id", json!(event_id)),
        ("detail-type", json!(DETAIL_TYPE)),
        ("source", json!(SOURCE)),
        ("account", json!(engine::emulator_account_id())),
        ("region", json!(REGION)),
        ("resources", json!([])),
        (
            "detail",
            serde_json::from_str::<Value>(DETAIL).expect("the detail is JSON"),
        ),
    ];
    for (member, expected) in rows {
        assert_eq!(
            parsed[member], expected,
            "case {case}: expected the envelope's {member} to be {expected}, got {parsed}"
        );
    }
    assert!(
        parsed["time"].as_str().is_some_and(|time| !time.is_empty()),
        "case {case}: the envelope carries the event's time, got {parsed}"
    );
}

/// A matched event reaches the rule's SQS target, and WHAT it receives is decided
/// by the target's input configuration.
#[tokio::test]
async fn a_matched_event_reaches_the_rules_sqs_target() {
    let rows: [(&str, Value, Expect); 4] = [
        ("no input configuration", json!({}), Expect::WholeEvent),
        (
            "a constant Input",
            json!({ "Input": "{\"fixed\":true}" }),
            Expect::Constant("{\"fixed\":true}"),
        ),
        (
            // `InputPath` selects part of the event, and the JSON text of that
            // part is what the target receives.
            "an InputPath selecting part of the event",
            json!({ "InputPath": "$.detail.orderId" }),
            Expect::Constant("\"A-1\""),
        ),
        (
            // An `InputTransformer`'s template is rendered with what its paths
            // select; a template that is not a JSON object is text, so the
            // quotes it writes around the placeholder are its own.
            "an InputTransformer rendering its template",
            json!({
                "InputTransformer": {
                    "InputPathsMap": { "order": "$.detail.orderId" },
                    "InputTemplate": "\"<order>\"",
                },
            }),
            Expect::Constant("\"A-1\""),
        ),
    ];

    for (case, extra, expected) in rows {
        let emulator = emulator();
        let (url, arn) = given_queue(&emulator, "delivery-probe").await;
        let mut target = json!({ "Id": "t1", "Arn": arn });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            target[member] = value;
        }
        given_rule(
            &emulator,
            "delivery",
            &matching_pattern(),
            "ENABLED",
            json!([target]),
        )
        .await;

        let event_id = publish(&emulator).await;
        let bodies = drain(&emulator, &url).await;
        match expected {
            Expect::WholeEvent => {
                assert_eq!(
                    bodies.len(),
                    1,
                    "case {case}: expected exactly one delivery, got {bodies:?}"
                );
                assert_whole_envelope(case, &bodies[0], &event_id);
            }
            Expect::Constant(text) => {
                assert_eq!(
                    bodies,
                    vec![text.to_string()],
                    "case {case}: expected the text the target asked for, got {bodies:?}"
                );
            }
        }
    }
}

/// Which rules and targets a delivery reaches: every target of every matching
/// rule, and nothing of a rule that does not match or is not enabled.
#[tokio::test]
async fn delivery_follows_the_rules_that_match_and_no_others() {
    // Each row: a rule, its pattern, its state, and how many messages each of its
    // targets must hold afterwards.
    let rows: [(&str, String, &str, usize); 4] = [
        ("matching-enabled", matching_pattern(), "ENABLED", 1),
        (
            "matching-disabled",
            matching_pattern(),
            // "EventBridge does not match any events against the rule" — so a
            // disabled rule's target receives nothing.
            "DISABLED",
            0,
        ),
        (
            "non-matching-source",
            json!({ "source": ["com.other.app"] }).to_string(),
            "ENABLED",
            0,
        ),
        (
            "non-matching-detail",
            json!({ "source": [SOURCE], "detail": { "amount": [{ "numeric": [">", 1000] }] } })
                .to_string(),
            "ENABLED",
            0,
        ),
    ];

    let emulator = emulator();
    let mut queues: Vec<(&str, String, usize)> = Vec::new();
    for (rule, pattern, state, expected) in &rows {
        // Two targets on the matching rule, one on each of the others: a matching
        // rule delivers to EVERY target it carries.
        let (first_url, first_arn) = given_queue(&emulator, &format!("{rule}-a")).await;
        let (second_url, second_arn) = given_queue(&emulator, &format!("{rule}-b")).await;
        given_rule(
            &emulator,
            rule,
            pattern,
            state,
            json!([
                { "Id": "t1", "Arn": first_arn },
                { "Id": "t2", "Arn": second_arn },
            ]),
        )
        .await;
        queues.push((rule, first_url, *expected));
        queues.push((rule, second_url, *expected));
    }

    let _ = publish(&emulator).await;

    for (rule, url, expected) in queues {
        let bodies = drain(&emulator, &url).await;
        assert_eq!(
            bodies.len(),
            expected,
            "case {rule}: expected {expected} delivery/deliveries on {url}, got {bodies:?}"
        );
    }
}

/// Every target of a matching rule is RECORDED as a target the event reached —
/// the routing log says where the event went — whatever service it belongs to,
/// and nothing is invented for one whose own service is not mounted here: no
/// other queue receives the event in its place.
#[tokio::test]
async fn a_target_that_is_not_a_queue_is_recorded_and_not_invented_for() {
    // Rows differ along the target's service: a function, a topic, a state
    // machine, and a log group — whose services this router does not mount, so
    // their invocations reach nothing — plus an SQS queue as the positive
    // control.
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, "mixed-probe").await;
    let account = engine::emulator_account_id();
    let others = [
        format!("arn:aws:lambda:{REGION}:{account}:function:handler"),
        format!("arn:aws:sns:{REGION}:{account}:topic"),
        format!("arn:aws:states:{REGION}:{account}:stateMachine:flow"),
        format!("arn:aws:logs:{REGION}:{account}:log-group:/demo:*"),
    ];
    let mut targets: Vec<Value> = vec![json!({ "Id": "queue", "Arn": queue_arn.clone() })];
    for (index, arn) in others.iter().enumerate() {
        let mut target = json!({ "Id": format!("other-{index}"), "Arn": arn });
        // A target EventBridge invokes AS the role it names has to name one, so the
        // state machine among these rows carries a `RoleArn`; the function, the
        // topic and the log group are invoked under their own resource-based
        // policies and need none.
        if arn.contains(":states:") {
            target["RoleArn"] = json!(format!("arn:aws:iam::{account}:role/events-invoke"));
        }
        targets.push(target);
    }
    given_rule(
        &emulator,
        "mixed",
        &matching_pattern(),
        "ENABLED",
        Value::Array(targets),
    )
    .await;

    let event_id = publish(&emulator).await;

    let bodies = drain(&emulator, &url).await;
    assert_eq!(
        bodies.len(),
        1,
        "the queue target receives the event exactly once, got {bodies:?}"
    );
    assert_whole_envelope("mixed targets", &bodies[0], &event_id);

    let recorded: Vec<String> = emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .iter()
        .flat_map(|event| event.target_arns.clone())
        .collect();
    let mut expected = vec![queue_arn];
    expected.extend(others.iter().cloned());
    assert_eq!(
        recorded, expected,
        "every target of the matching rule is recorded in the routing log, got {recorded:?}"
    );
}
