// SPDX-License-Identifier: CC0-1.0

//! When a stack reports an `AWS::DynamoDB::Table` CREATED, measured against what Amazon
//! DynamoDB says about the table at that instant.
//!
//! RULE under test: **CloudFormation holds a declared resource's creation open until the
//! service that owns it reports the resource out of its own creation window — the window in
//! which the resource takes none of the calls its declaration published it for — so a client
//! that reads the stack as complete finds a resource it can really use. For an
//! `AWS::DynamoDB::Table` that window is `TableStatus` `CREATING`: "you can perform read and
//! write operations only on an ACTIVE table", and a table still in it answers an item call
//! as if the table were not there.** It is the same case `AWS::EC2::Instance` states for the
//! `pending` of a launched instance.
//!
//! So the assertions are made at the FIRST instant the stack reports `CREATE_COMPLETE`,
//! found by polling: DynamoDB must report the table `ACTIVE` then, and an item written and
//! read back through the table's own wire must round-trip with no further wait. A stack that
//! completed while the table was still coming up would fail both.
//!
//! The rows span what the rule generalises over: the capacity mode, whether the key schema
//! carries a sort key, whether the declaration enables a stream, whether the template names
//! the table or leaves CloudFormation to name it, and whether the declaration asks for a
//! setting DynamoDB applies through a call of its own (`TimeToLiveSpecification`).
//!
//! Both services are driven through one router — CloudFormation over the awsQuery wire and
//! DynamoDB as a client's SDK calls it (the caller [`super::dynamodb_table_resources`] owns)
//! — so what is asserted is what a client that deploys a table by template and then uses it
//! observes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, dynamodb};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, output_value, router, xml_text,
};

/// How long a case waits for a deployment to settle, and how often it looks.
///
/// The interval is short so the stack is read as complete within a moment of the instant it
/// becomes complete — which is the instant this suite is about — and the deadline is far
/// longer than any span this deployment owes, so a loaded host cannot turn the wait into a
/// failure.
const DEPLOY_DEADLINE: Duration = Duration::from_secs(120);
const DEPLOY_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The status DynamoDB reports for a table that takes item calls, and the one it reports
/// while the table is still coming up ("You can perform read and write operations only on an
/// ACTIVE table" — the `CreateTable` doc).
const ACTIVE: &str = "ACTIVE";
const CREATING: &str = "CREATING";

/// One row: a declared table, and what makes it a different declaration.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `AWS::DynamoDB::Table` properties the template declares.
    properties: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an on-demand table CloudFormation names itself",
            stack: "ddb-window-on-demand",
            properties: json!({
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            }),
        },
        Row {
            case: "a provisioned table with a sort key, named by the template",
            stack: "ddb-window-provisioned",
            properties: json!({
                "TableName": "window-provisioned",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "sk", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": "sk", "AttributeType": "N"},
                ],
            }),
        },
        Row {
            case: "a table whose declaration enables a stream",
            stack: "ddb-window-streamed",
            properties: json!({
                "TableName": "window-streamed",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
            }),
        },
        Row {
            case: "a table whose declaration asks for a setting of its own",
            stack: "ddb-window-ttl",
            properties: json!({
                "TableName": "window-ttl",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "TimeToLiveSpecification": {"Enabled": true, "AttributeName": "expires"},
            }),
        },
    ]
}

/// The template one row deploys: the table, and the output a client reads its name through.
fn template(row: &Row) -> String {
    json!({
        "Resources": {"Table": {"Type": "AWS::DynamoDB::Table",
                                "Properties": row.properties.clone()}},
        "Outputs": {"TableName": {"Value": {"Ref": "Table"}}},
    })
    .to_string()
}

/// Every resource failure reason the stack recorded: what a case whose stack did not
/// complete needs in order to say WHY.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    crate::stack_lifecycle::xml_blocks(&body, "member")
        .into_iter()
        .filter_map(|event| xml_text(&event, "ResourceStatusReason"))
        .collect()
}

/// The `DescribeStacks` body at the first poll that reports the stack out of
/// `CREATE_IN_PROGRESS`.
///
/// The wait is on the CONDITION — the stack reaching a terminal status — and gives up only
/// past [`DEPLOY_DEADLINE`].
async fn described_once_settled(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let deadline = Instant::now() + DEPLOY_DEADLINE;
    loop {
        let (status, described) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stack to be readable, got {status} {described}"
        );
        if xml_text(&described, "StackStatus").as_deref() != Some("CREATE_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the create to settle, last read {described}"
        );
        tokio::time::sleep(DEPLOY_POLL_INTERVAL).await;
    }
}

/// The key one item of a row's table is written under: the partition key every row declares,
/// plus the sort key the rows that declare one need.
fn item_key(properties: &Value) -> Value {
    let sorted = properties["KeySchema"]
        .as_array()
        .is_some_and(|keys| keys.len() == 2);
    if sorted {
        return json!({"pk": {"S": "window"}, "sk": {"N": "1"}});
    }
    json!({"pk": {"S": "window"}})
}

#[tokio::test]
async fn a_stack_reports_a_table_created_only_once_dynamodb_serves_calls_on_it() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(&router, row.stack, &template(&row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );

        let described = described_once_settled(case, &router, row.stack).await;
        let stack_status = xml_text(&described, "StackStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {stack_status:?} because {:?}",
            failure_reasons(&router, row.stack).await
        );

        // At the instant the stack reports the resource created, DynamoDB reports the table
        // out of the window in which it serves no item call.
        let name = output_value(&described, "TableName").unwrap_or_default();
        let table = describe_table(&router, &name).await.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected DynamoDB to hold the table {name:?} the stack \
                 reports created, got no table at all"
            )
        });
        let reported = table.get("TableStatus").and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(ACTIVE),
            "case {case:?}: expected DynamoDB to report the table {ACTIVE} when the stack \
             reports it created, got {reported:?} (which {CREATING} is the window this rule \
             is about)"
        );

        // ...and the calls the stack published the table for go through with no further
        // wait: an item written is an item read back.
        let key = item_key(&row.properties);
        let mut item = key.clone();
        item["note"] = json!({"S": case});
        let (status, written) =
            dynamodb(&router, "PutItem", &json!({"TableName": &name, "Item": item})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the item write to be served the instant the stack \
             completed, got {status} {written}"
        );
        let (status, read) = dynamodb(
            &router,
            "GetItem",
            &json!({"TableName": &name, "Key": key, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the item read to be served, got {status} {read}"
        );
        assert_eq!(
            read["Item"]["note"],
            json!({"S": case}),
            "case {case:?}: expected the item just written, got {read}"
        );
    }
}
