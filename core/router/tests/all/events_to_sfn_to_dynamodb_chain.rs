// SPDX-License-Identifier: CC0-1.0

//! An event published on a custom bus reaches a DynamoDB item: the rule's state
//! machine target is started, its `Task` state's DynamoDB integration is performed,
//! and the item that task wrote is readable through DynamoDB's own API.
//!
//! The rule under test: **every handover of an event-driven chain is really made, so
//! the chain's FAR-END effect is there afterwards** — a matched event is delivered to
//! the rule's state-machine target as `states:StartExecution`, and the execution's
//! `Task` state performs the API call its `Resource` names with the document its
//! `Parameters` built from the event, which is what makes the item appear. Three
//! crates have to agree for that to hold (EventBridge delivers, Step Functions runs
//! the task, DynamoDB serves the write), so the suite lives here rather than in any
//! of them.
//!
//! The rows differ along what the rule generalises over: which member of the event
//! envelope the item's key is taken from, the integration family the `Resource` is
//! written in (the optimized `arn:aws:states:::dynamodb:putItem` and the AWS SDK
//! `arn:aws:states:::aws-sdk:dynamodb:putItem` form), and whether the target hands
//! the state machine the whole envelope or its own constant `Input`.
//!
//! Every expected value is computed here from what the row published, never read
//! back out of the answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// Long enough that a created table has become `ACTIVE`: "You can perform read and
/// write operations only on an ACTIVE table", and a created one is `CREATING` for one
/// DynamoDB status transition. Read from that service's own constant rather than
/// written down, so a change to the span cannot leave this wait short.
const PAST_THE_TABLE_TRANSITION: Duration =
    service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
        .saturating_add(Duration::from_millis(400));

/// The three services this chain runs through, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callbacks EventBridge and Step Functions make
/// sign their requests either way (the credential scope is what routes them).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is
/// what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one operation of an `awsJson` service and return `(status, body)`.
async fn call(
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
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one EventBridge operation (awsJson1_1, `AWSEvents.<Operation>`), requiring a
/// 200.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

/// Call one Step Functions operation (awsJson1_0, `AWSStepFunctions.<Operation>`),
/// requiring a 200.
async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(
        router,
        "states",
        "AWSStepFunctions",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

/// Call one DynamoDB operation (awsJson1_0, `DynamoDB_20120810.<Operation>`),
/// requiring a 200.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(
        router,
        "dynamodb",
        "DynamoDB_20120810",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

/// One row of the chain table: the event published, the integration the state
/// machine's task is written as, and the key the item is expected under.
struct ChainCase {
    case: &'static str,
    /// The `Resource` of the state machine's single `Task` state.
    resource: &'static str,
    /// The path the item's key is taken from, inside the document the execution is
    /// started with — the event envelope, or the target's own `Input` when it sets
    /// one.
    key_path: &'static str,
    /// The `detail` of the event published.
    detail: Value,
    /// The target's own `Input`: "valid JSON text passed to the target. In this
    /// case, nothing from the event itself is passed to the target", so the
    /// execution is started with THIS document instead of the envelope. `None`
    /// leaves the target to be handed the whole envelope.
    target_input: Option<&'static str>,
    /// The key the item must be stored under.
    expected_key: &'static str,
}

/// A published event that matches a rule reaches the item its target's state
/// machine writes.
#[tokio::test]
async fn an_event_reaches_the_item_the_targets_state_machine_writes() {
    let cases = [
        ChainCase {
            case: "the optimized DynamoDB integration, keyed by the event's detail",
            resource: "arn:aws:states:::dynamodb:putItem",
            key_path: "$.detail.order_id",
            detail: json!({ "order_id": "o-1" }),
            target_input: None,
            expected_key: "o-1",
        },
        ChainCase {
            case: "the AWS SDK integration of the same operation",
            resource: "arn:aws:states:::aws-sdk:dynamodb:putItem",
            key_path: "$.detail.order_id",
            detail: json!({ "order_id": "o-2" }),
            target_input: None,
            expected_key: "o-2",
        },
        ChainCase {
            case: "a key taken from a nested member of the detail",
            resource: "arn:aws:states:::dynamodb:putItem",
            key_path: "$.detail.order.id",
            detail: json!({ "order": { "id": "o-3" } }),
            target_input: None,
            expected_key: "o-3",
        },
        ChainCase {
            case: "a key taken from the envelope's own member rather than the detail",
            resource: "arn:aws:states:::dynamodb:putItem",
            key_path: "$.detail-type",
            detail: json!({ "order_id": "o-4" }),
            target_input: None,
            expected_key: "OrderPlaced",
        },
        ChainCase {
            case: "a target with its own constant Input, which the execution is \
                   started with instead of the envelope",
            resource: "arn:aws:states:::dynamodb:putItem",
            key_path: "$.order_id",
            detail: json!({ "order_id": "never" }),
            target_input: Some(r#"{"order_id":"o-5"}"#),
            expected_key: "o-5",
        },
    ];

    let router = build_router();
    dynamodb(
        &router,
        "CreateTable",
        &json!({
            "TableName": "orders",
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST"
        }),
    )
    .await;
    events(&router, "CreateEventBus", &json!({ "Name": "chain-bus" })).await;
    std::thread::sleep(PAST_THE_TABLE_TRANSITION);

    for (index, case) in cases.iter().enumerate() {
        // A state machine whose only state is the integration this row names,
        // writing the key the row's path selects out of the event it is started
        // with.
        let definition = json!({
            "StartAt": "Write",
            "States": {
                "Write": {
                    "Type": "Task",
                    "Resource": case.resource,
                    "Parameters": {
                        "TableName": "orders",
                        "Item": {
                            "pk": { "S.$": case.key_path },
                            "status": { "S": "received" }
                        }
                    },
                    "End": true
                }
            }
        });
        let created = sfn(
            &router,
            "CreateStateMachine",
            &json!({
                "name": format!("chain-{index}"),
                "definition": definition.to_string(),
                "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/StepFunctionsRole")
            }),
        )
        .await;
        let machine = created
            .get("stateMachineArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: expected a state machine ARN", case.case))
            .to_string();

        // A rule on the custom bus matching this row's event, with that state
        // machine as its target.
        let rule = format!("chain-rule-{index}");
        events(
            &router,
            "PutRule",
            &json!({
                "Name": rule,
                "EventBusName": "chain-bus",
                "EventPattern": json!({
                    "source": ["sim2real.test"],
                    "detail-type": ["OrderPlaced"]
                })
                .to_string()
            }),
        )
        .await;
        let mut target = json!({
            "Id": "to-state-machine",
            "Arn": machine,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/EventsInvokeStates")
        });
        if let Some(constant) = case.target_input {
            target["Input"] = Value::String(constant.to_string());
        }
        let targeted = events(
            &router,
            "PutTargets",
            &json!({
                "Rule": rule,
                "EventBusName": "chain-bus",
                "Targets": [target]
            }),
        )
        .await;
        assert_eq!(
            targeted.get("FailedEntryCount").and_then(Value::as_i64),
            Some(0),
            "case {:?}: the target must be accepted, got {targeted:?}",
            case.case
        );

        // The event itself, which is what starts the chain.
        let published = events(
            &router,
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": "chain-bus",
                    "Source": "sim2real.test",
                    "DetailType": "OrderPlaced",
                    "Detail": case.detail.to_string()
                }]
            }),
        )
        .await;
        assert_eq!(
            published.get("FailedEntryCount").and_then(Value::as_i64),
            Some(0),
            "case {:?}: the event must be accepted, got {published:?}",
            case.case
        );

        // The far end: the item the chain's last handover wrote.
        let read = dynamodb(
            &router,
            "GetItem",
            &json!({
                "TableName": "orders",
                "Key": { "pk": { "S": case.expected_key } }
            }),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            Some(&json!({
                "pk": { "S": case.expected_key },
                "status": { "S": "received" }
            })),
            "case {:?}: the chain's own effect must be readable through GetItem, got {read:?}",
            case.case
        );
    }
}

/// The chain only fires for an event the rule MATCHES: an event of another source
/// on the same bus writes nothing, so the item that appears above is the delivery's
/// own effect rather than something the publish does for every event.
#[tokio::test]
async fn an_event_no_rule_matches_reaches_nothing() {
    let router = build_router();
    dynamodb(
        &router,
        "CreateTable",
        &json!({
            "TableName": "orders",
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST"
        }),
    )
    .await;
    events(&router, "CreateEventBus", &json!({ "Name": "chain-bus" })).await;
    std::thread::sleep(PAST_THE_TABLE_TRANSITION);

    let definition = json!({
        "StartAt": "Write",
        "States": {
            "Write": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Parameters": {
                    "TableName": "orders",
                    "Item": { "pk": { "S.$": "$.detail.order_id" } }
                },
                "End": true
            }
        }
    });
    let created = sfn(
        &router,
        "CreateStateMachine",
        &json!({
            "name": "unmatched",
            "definition": definition.to_string(),
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/StepFunctionsRole")
        }),
    )
    .await;
    let machine = created
        .get("stateMachineArn")
        .and_then(Value::as_str)
        .expect("a state machine ARN")
        .to_string();
    events(
        &router,
        "PutRule",
        &json!({
            "Name": "unmatched-rule",
            "EventBusName": "chain-bus",
            "EventPattern": json!({ "source": ["sim2real.test"] }).to_string()
        }),
    )
    .await;
    events(
        &router,
        "PutTargets",
        &json!({
            "Rule": "unmatched-rule",
            "EventBusName": "chain-bus",
            // EventBridge starts a state machine AS the role the target names, so
            // `PutTargets` requires one ("for EC2 instances, Kinesis Data Streams,
            // Step Functions state machines and API Gateway APIs, EventBridge
            // relies on IAM roles that you specify in the RoleARN argument in
            // PutTargets"), as the matched-rule rows above also name.
            "Targets": [{
                "Id": "to-state-machine",
                "Arn": machine,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/EventsInvokeStates"),
            }]
        }),
    )
    .await;

    events(
        &router,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": "chain-bus",
                "Source": "other.source",
                "DetailType": "OrderPlaced",
                "Detail": json!({ "order_id": "never" }).to_string()
            }]
        }),
    )
    .await;

    let read = dynamodb(
        &router,
        "GetItem",
        &json!({ "TableName": "orders", "Key": { "pk": { "S": "never" } } }),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        None,
        "case \"an event no rule matches\": nothing may be written for it, got {read:?}"
    );
}
