// SPDX-License-Identifier: CC0-1.0

//! The serverless pipeline a client wires most often, end to end over the wire: an event is
//! published onto a bus, an enabled rule matches it, the rule's Lambda target RUNS, and the
//! item its handler wrote is read back through Amazon DynamoDB's own `GetItem`.
//!
//! The RULE pinned here: every entry a publication accepts is matched against the ENABLED
//! rules of the bus it names, and each target of a matching rule whose own resource-based
//! policy admits the invocation is invoked ONCE PER MATCHING ENTRY with the event ENVELOPE —
//! `source` and `detail-type` exactly as submitted and `detail` as the submitted JSON
//! document, every value keeping its JSON type — so whatever the target's own code does with
//! that envelope is observable afterwards through the API of the service it did it in; an
//! entry no enabled rule matches, and an entry whose rule is disabled, invoke nothing, so
//! nothing of them is observable anywhere.
//!
//! Five crates have to agree for that to hold (this service matches and hands over, Lambda
//! runs the handler and supplies it an SDK, IAM holds the execution role's grants, the token
//! service vends the session the handler's call is signed as, DynamoDB serves the table), so
//! the suite mounts them on ONE engine. Every delivered observation is one a client makes: the
//! item under the key the detail named, carrying the values the publication carried.
//!
//! The rows differ along what the rule generalises over: HOW MANY entries of one request match
//! (one, several), the SHAPE of the detail the handler is handed (flat strings, a nested
//! sub-object with a number, a fraction, a boolean, a null and an array), whether the entry's
//! `source` is one the rule's pattern names, and whether the rule is enabled at all. The
//! recorded case is the first row.
//!
//! `lambda_target_handler_records_the_invoking_rule` is the suite for an invocation whose
//! target reshapes the event with an `InputTransformer`; what this one adds is the UNTOUCHED
//! envelope reaching a handler that reads `detail` itself, and the handler's own write landing
//! in another service's store.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region the table, the function and
/// the rule belong to.
const REGION: &str = "us-west-2";

/// The `Source` the rule's pattern names, and a `Source` it does not.
const SOURCE: &str = "com.demo.pipeline";
const OTHER_SOURCE: &str = "com.demo.elsewhere";

/// The `DetailType` every entry of this suite carries.
const DETAIL_TYPE: &str = "sim2real-pipeline";

/// The principal EventBridge invokes a resource-policy-governed target as, and the action such
/// an invocation performs.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The attribute the table is keyed on, which is also the member of the detail the handler
/// keys its item by.
const KEY_ATTRIBUTE: &str = "id";

/// How long each observation of work the clock owes — the table coming up, the function
/// becoming `Active`, the asynchronous delivery of an invocation, the handler's own write —
/// waits before giving up, and how long between two looks. The deadline is generous: what is
/// under test is that the work happens at all, never how fast this host gets to it.
const BUDGET: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long one invocation is given, as the function's own `Timeout`: well past what the
/// handler needs on any host.
const FUNCTION_TIMEOUT: u64 = 60;

/// The handler every row deploys. It reads the envelope EventBridge handed it — `detail`,
/// `source` and `detail-type` — and writes one item keyed by the detail's own `id`, carrying
/// the detail's `value`, the envelope's two routing members, and the whole detail as JSON text
/// so what the handler was handed is readable through DynamoDB afterwards.
const WRITER_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    detail = event["detail"]
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE_NAME"],
        Item={
            "id": {"S": detail["id"]},
            "value": {"S": detail["value"]},
            "source": {"S": event["source"]},
            "detail_type": {"S": event["detail-type"]},
            "detail": {"S": json.dumps(detail, sort_keys=True)},
        },
    )
    return {"written": detail["id"]}
"#;

/// EventBridge, Lambda, DynamoDB, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own SDK call is dispatched back into
/// this same router, so the table it writes is the table this suite reads.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id from. The
    // handler's OWN call is signed by the runtime with the credential it was given.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
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

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`), as its status and parsed
/// body — a read of an absent item is a served 200 carrying no `Item`, so the status is
/// answered rather than asserted here.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "dynamodb", "DynamoDB_20120810", "1.0", operation, body).await
}

/// One awsQuery call to IAM, as the account's own credential, which must be served.
async fn iam(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, String)]) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM {action}, got {status} with {answered}"
    );
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
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

/// The account every emulated resource of this suite belongs to, as the engine holds it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the table `name`, which is the resource the execution role's statement names.
fn table_arn(name: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{}:table/{name}", account())
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
            .write_all(WRITER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
}

/// Create the table the handler writes, keyed on [`KEY_ATTRIBUTE`], and wait until DynamoDB
/// reports it `ACTIVE` — "you can perform read and write operations only on an ACTIVE table".
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTable({name:?}), got {status} with {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, described) = dynamodb(router, "DescribeTable", &json!({"TableName": name})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: DynamoDB never reported {name:?} ACTIVE, last read {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create the execution role the function runs as: trusted by Lambda, and carrying the one
/// grant the handler's own call needs — `dynamodb:PutItem` on the table it writes. Answers the
/// role's ARN.
async fn execution_role(router: &MultiServiceRouter, case: &str, name: &str, table: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    iam(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "dynamodb:PutItem",
            "Resource": table_arn(table),
        }],
    })
    .to_string();
    iam(
        router,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "write-the-item".to_string()),
            ("PolicyDocument", document),
        ],
    )
    .await;
    format!("arn:aws:iam::{}:role/{name}", account())
}

/// Create the function the rule targets, wait until Lambda reports it `Active` — "during this
/// time, you can't invoke or modify the function" (`CreateFunction`) — and answer its ARN.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    table: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { "TABLE_NAME": table } },
            "Code": { "ZipFile": package() },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &Value::Null,
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Lambda never reported {name:?} Active, last read {configuration}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Grant this service the invocation on `function`, scoped by `aws:SourceArn` to the rule that
/// invokes it: "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
/// relies on resource-based policies" (`PutTargets`).
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    rule_arn: &str,
) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        &json!({
            "StatementId": "AllowEventBridgeInvoke",
            "Action": INVOKE_ACTION,
            "Principal": EVENTS_PRINCIPAL,
            "SourceArn": rule_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {status} with {granted}"
    );
}

/// What Lambda holds for every asynchronous invocation of `function`.
fn held_invocations(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
) -> Vec<AsyncInvocationOutcome> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| entry.outcome.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until `function` holds exactly `expected` invocations and none of them is still waiting
/// on its delivery attempt. Every handover is made before `PutEvents` answers, so a count that
/// stays below or above `expected` is a routing decision this suite can assert on.
async fn settled_invocations(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    expected: usize,
) -> Vec<AsyncInvocationOutcome> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let held = held_invocations(engine, function);
        let settled = held.iter().all(|outcome| {
            !matches!(
                outcome,
                AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
            )
        });
        if held.len() == expected && settled {
            return held;
        }
        if Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The item `table` holds under `id`, read strongly consistently, or `None` while it holds
/// none.
async fn item(router: &MultiServiceRouter, table: &str, id: &str) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { KEY_ATTRIBUTE: { "S": id } },
            "ConsistentRead": true,
        }),
    )
    .await;
    if status != 200 {
        return None;
    }
    answered.get("Item").cloned()
}

/// Wait until `table` holds an item under `id`, which is the handler having run and written it.
async fn until_written(router: &MultiServiceRouter, table: &str, id: &str) -> Option<Value> {
    let deadline = Instant::now() + BUDGET;
    loop {
        if let Some(held) = item(router, table, id).await {
            return Some(held);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One entry a row publishes: the `Source` it carries, and the `Detail` document it carries —
/// which always names the `id` the handler keys its item by and the `value` it writes.
struct Entry {
    source: &'static str,
    detail: Value,
}

/// One row: the state of the rule, the entries one `PutEvents` request carries, which of their
/// ids must therefore be in the table, and which must not.
struct Row {
    case: &'static str,
    rule_state: &'static str,
    entries: Vec<Entry>,
    delivered: Vec<&'static str>,
    absent: Vec<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one matching entry writes the item its detail names",
            rule_state: "ENABLED",
            entries: vec![Entry {
                source: SOURCE,
                detail: json!({ "id": "item-one", "value": "pipeline-value-42" }),
            }],
            delivered: vec!["item-one"],
            absent: vec![],
        },
        Row {
            case: "three matching entries of one request each run the handler once",
            rule_state: "ENABLED",
            entries: vec![
                Entry {
                    source: SOURCE,
                    detail: json!({ "id": "item-a", "value": "first" }),
                },
                Entry {
                    source: SOURCE,
                    detail: json!({ "id": "item-b", "value": "second" }),
                },
                Entry {
                    source: SOURCE,
                    detail: json!({ "id": "item-c", "value": "third" }),
                },
            ],
            delivered: vec!["item-a", "item-b", "item-c"],
            absent: vec![],
        },
        Row {
            case: "a nested detail reaches the handler with every value's JSON type kept",
            rule_state: "ENABLED",
            entries: vec![Entry {
                source: SOURCE,
                detail: json!({
                    "id": "item-nested",
                    "value": "nested-value",
                    "payload": {
                        "count": 7,
                        "ratio": 1.5,
                        "enabled": true,
                        "missing": null,
                        "labels": ["alpha", "beta"],
                    },
                }),
            }],
            delivered: vec!["item-nested"],
            absent: vec![],
        },
        Row {
            case: "an entry whose source the rule's pattern does not name runs nothing",
            rule_state: "ENABLED",
            entries: vec![
                Entry {
                    source: OTHER_SOURCE,
                    detail: json!({ "id": "item-unmatched", "value": "unmatched" }),
                },
                Entry {
                    source: SOURCE,
                    detail: json!({ "id": "item-matched", "value": "matched" }),
                },
            ],
            delivered: vec!["item-matched"],
            absent: vec!["item-unmatched"],
        },
        Row {
            case: "a disabled rule matches nothing, so a matching entry runs nothing",
            rule_state: "DISABLED",
            entries: vec![Entry {
                source: SOURCE,
                detail: json!({ "id": "item-disabled", "value": "disabled" }),
            }],
            delivered: vec![],
            absent: vec!["item-disabled"],
        },
    ]
}

/// RULE: each entry a publication accepts is matched against the bus's ENABLED rules, and each
/// target of a matching rule its own policy admits is invoked once per matching entry with the
/// submitted envelope — so the handler's own write is in the table afterwards, and an entry no
/// enabled rule matches writes nothing.
#[tokio::test]
async fn a_matched_rules_lambda_target_writes_the_published_detail_into_its_table() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();
        let table = format!("pipeline-landings-{index}");
        let role_name = format!("pipeline-writer-role-{index}");
        let function_name = format!("pipeline-writer-{index}");
        let rule_name = format!("pipeline-rule-{index}");

        create_table(&router, case, &table).await;
        let role_arn = execution_role(&router, case, &role_name, &table).await;
        let function_arn =
            create_function(&router, case, &function_name, &role_arn, &table).await;

        let rule = events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule_name,
                "EventPattern": json!({ "source": [SOURCE] }).to_string(),
                "State": row.rule_state,
            }),
        )
        .await;
        let rule_arn = rule["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
            .to_string();
        add_permission(&router, case, &function_name, &rule_arn).await;
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{ "Id": "1", "Arn": function_arn }],
            }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target to be accepted, got {wired}"
        );

        let entries: Vec<Value> = row
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "Source": entry.source,
                    "DetailType": DETAIL_TYPE,
                    "Detail": entry.detail.to_string(),
                })
            })
            .collect();
        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({ "Entries": entries }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected every entry to be accepted, got {published}"
        );
        let results = published["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            row.entries.len(),
            "case {case:?}: expected one result entry per published entry, got {published}"
        );
        for result in &results {
            assert!(
                result["EventId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected an accepted entry to carry an EventId, got {result}"
            );
        }

        // Every handover is made before `PutEvents` answers, so once the function holds exactly
        // the invocations the matching entries owe it and each has been attempted, the routing
        // of this publication is wholly decided and its writes are done.
        let invocations = settled_invocations(&engine, &function_name, row.delivered.len()).await;
        assert_eq!(
            invocations.len(),
            row.delivered.len(),
            "case {case:?}: expected {} invocation(s) of {function_name:?}, got {invocations:?}",
            row.delivered.len()
        );
        for outcome in &invocations {
            assert_eq!(
                outcome,
                &AsyncInvocationOutcome::Delivered {
                    function_error: None
                },
                "case {case:?}: expected every delivery to run the handler without error, got \
                 {invocations:?}"
            );
        }

        for id in &row.delivered {
            let detail = row
                .entries
                .iter()
                .find(|entry| entry.detail["id"] == json!(id))
                .map(|entry| entry.detail.clone())
                .unwrap_or_else(|| panic!("case {case:?}: no published entry names {id:?}"));
            let held = until_written(&router, &table, id)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the handler to write the item {id:?} into \
                         {table:?}, but DynamoDB holds none"
                    )
                });
            assert_eq!(
                held["value"]["S"],
                detail["value"],
                "case {case:?}: expected the published value for {id:?}, got {held}"
            );
            // The envelope's own routing members reached the handler as submitted.
            assert_eq!(
                held["source"]["S"],
                json!(
                    row.entries
                        .iter()
                        .find(|entry| entry.detail["id"] == json!(id))
                        .map(|entry| entry.source)
                        .unwrap_or_default()
                ),
                "case {case:?}: expected the published source for {id:?}, got {held}"
            );
            assert_eq!(
                held["detail_type"]["S"],
                json!(DETAIL_TYPE),
                "case {case:?}: expected the published detail-type for {id:?}, got {held}"
            );
            // And the detail arrived as the document it was published as, every value keeping
            // its JSON type rather than being stringified or coerced.
            let handed: Value = held["detail"]["S"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            assert_eq!(
                handed, detail,
                "case {case:?}: expected the handler to be handed the published detail for \
                 {id:?}, got {handed}"
            );
        }

        for id in &row.absent {
            let held = item(&router, &table, id).await;
            assert!(
                held.is_none(),
                "case {case:?}: expected no invocation for {id:?}, so {table:?} holds no item \
                 under it, got {held:?}"
            );
        }
    }
}
