// SPDX-License-Identifier: CC0-1.0

//! A stack that declares a change-data-capture estate: an `AWS::Kinesis::Stream` and the
//! `AWS::DynamoDB::Table` whose `KinesisStreamSpecification` replicates to it, read back
//! through the owning services' own reads.
//!
//! RULE: a stack reports `CREATE_COMPLETE` only once every resource it declares EXISTS in
//! the service that owns it, carrying the configuration its declaration asked for — so a
//! client that deploys this template and then calls `DescribeTable` on the name the stack
//! published reaches the table, with the throughput the declaration named, and the
//! destination its `KinesisStreamSpecification` declared settles `ACTIVE` on the data
//! stream the stack's own `AWS::Kinesis::Stream` created. `CREATE_COMPLETE` is a usable
//! signal that the estate is there, never a status reported over resources nothing
//! provisioned.
//!
//! The rows differ along what the rule generalises over: the capacity mode the streaming
//! table declares (`ProvisionedThroughput`, in two different unit pairs, and
//! `PAY_PER_REQUEST`), and the replication precision the declaration names — the enum's
//! two values (`MILLISECOND`, `MICROSECOND`,
//! `smithy_query(query="shape",shape="ApproximateCreationDateTimePrecision",
//! model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/\
//! com-amazonaws-dynamodb-dynamodb-20120810.json")`) and a declaration naming none. Every
//! row's stream ARN is `Fn::GetAtt` of the stack's OWN stream, so the destination is
//! resolved against a stream this deployment created rather than one seeded beside it, and
//! every row carries a second table with no streaming destination at all — the table that
//! must report none.
//!
//! The CloudFormation calls go over the awsQuery wire this crate's harness owns and the
//! DynamoDB calls as a client's SDK makes them, through ONE router carrying all three
//! services on one engine, exactly as the emulator mounts them.
//!
//! The sibling suite `dynamodb_table_settings.rs` is about WHICH properties of the type
//! DynamoDB applies through a call of its own, each read back through the owning read, over a
//! stream seeded beside the stack; this one is about the composite ESTATE — the stream the
//! stack itself declares, the `Fn::GetAtt` that binds the table to it, the capacity the
//! table declares, and the sibling table that declares no destination at all.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, dynamodb, failure_reasons};
use crate::stack_lifecycle::{
    create_stack, describe_stack, output_value, router, xml_text, ACCOUNT_ID, REGION,
};

/// The `StackStatus` a deployment of this estate settles on.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How often a row asks whether something it is waiting for has settled, and how long it
/// goes on asking.
///
/// The budget covers the spans the stack, the data stream, the table and the streaming
/// destination each owe, one after the other, so a loaded machine is slower than those
/// spans without the rule being any different; a row that never settles reports rather
/// than running on.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS: usize = 600;

/// One row: the capacity mode the streaming table declares and the precision its
/// destination replicates under.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `(ReadCapacityUnits, WriteCapacityUnits)` the declaration provisions, or `None`
    /// for a `PAY_PER_REQUEST` table.
    throughput: Option<(u64, u64)>,
    /// The `ApproximateCreationDateTimePrecision` the declaration names, or `None` where it
    /// names none.
    precision: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a provisioned table replicating to the stack's own stream",
            stack: "ddbkds-provisioned",
            throughput: Some((5, 5)),
            precision: None,
        },
        Row {
            case: "another provisioned pair, with the creation time in microseconds",
            stack: "ddbkds-provisioned-micro",
            throughput: Some((3, 11)),
            precision: Some("MICROSECOND"),
        },
        Row {
            case: "an on-demand table, with the creation time in milliseconds",
            stack: "ddbkds-on-demand",
            throughput: None,
            precision: Some("MILLISECOND"),
        },
    ]
}

/// The template one row deploys: the data stream, the table that replicates to it, a second
/// table with no destination, and the three outputs a client reads the estate through.
fn template(row: &Row) -> String {
    let mut streaming = json!({
        "TableName": streaming_table(row),
        "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}],
        "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
        "KinesisStreamSpecification": kinesis_stream_specification(row),
    });
    let members = streaming
        .as_object_mut()
        .expect("the declaration is an object");
    match row.throughput {
        Some((read, write)) => {
            members.insert(
                "ProvisionedThroughput".to_string(),
                json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write}),
            );
        }
        None => {
            members.insert("BillingMode".to_string(), json!("PAY_PER_REQUEST"));
        }
    }
    json!({
        "Resources": {
            "ChangeStream": {
                "Type": "AWS::Kinesis::Stream",
                "Properties": {"Name": stream_name(row), "ShardCount": 1},
            },
            "TableWithStream": {"Type": "AWS::DynamoDB::Table", "Properties": streaming},
            "PlainTable": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": plain_table(row),
                    "BillingMode": "PAY_PER_REQUEST",
                    "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}],
                    "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
                },
            },
        },
        "Outputs": {
            "PlainTableName": {"Value": {"Ref": "PlainTable"}},
            "StreamArn": {"Value": {"Fn::GetAtt": ["ChangeStream", "Arn"]}},
            "StreamTableName": {"Value": {"Ref": "TableWithStream"}},
        },
    })
    .to_string()
}

/// The `KinesisStreamSpecification` one row declares: the ARN of the stack's OWN stream,
/// plus the precision where the row names one.
fn kinesis_stream_specification(row: &Row) -> Value {
    let mut specification = json!({"StreamArn": {"Fn::GetAtt": ["ChangeStream", "Arn"]}});
    if let Some(precision) = row.precision {
        specification
            .as_object_mut()
            .expect("the specification is an object")
            .insert(
                "ApproximateCreationDateTimePrecision".to_string(),
                json!(precision),
            );
    }
    specification
}

/// The names one row's three resources are declared under.
fn stream_name(row: &Row) -> String {
    format!("{}-changes", row.stack)
}
fn streaming_table(row: &Row) -> String {
    format!("{}-streaming", row.stack)
}
fn plain_table(row: &Row) -> String {
    format!("{}-plain", row.stack)
}

/// Poll `stack` until it reports `wanted`, answering the last status read and the answer it
/// came from.
///
/// A stack that settles on a DIFFERENT terminal status is answered straight away rather than
/// polled to the end of the budget, so a row whose deployment failed reports the reason it
/// failed for instead of waiting out a status that will never change. That short circuit is
/// why this is not the sibling suite's poll helper
/// (`dynamodb_table_settings::wait_for_stack_status`): that one waits for a status a stack
/// reaches AFTER an earlier terminal one (`ROLLBACK_COMPLETE` after `CREATE_FAILED`,
/// `UPDATE_COMPLETE` after `CREATE_COMPLETE`), where stopping at the first terminal status
/// would answer the wrong one. Here the wanted status is the first terminal status of a
/// create, so anything else terminal means the deployment is over and failed.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLLS {
        let (status, body) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        last = (reported, body);
        if last.0 == wanted || settled_elsewhere(&last.0, wanted) {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}

/// Whether `reported` is a terminal `StackStatus` other than `wanted`: one the stack will not
/// leave on its own, so there is nothing left to wait for.
fn settled_elsewhere(reported: &str, wanted: &str) -> bool {
    reported != wanted && (reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED"))
}

/// The `KinesisDataStreamDestinations` DynamoDB reports for `table`, waited for until one of
/// them reports `wanted` — the destination's registration is asynchronous in that service,
/// so the status it settles on is owed from the instant its own span falls due.
async fn wait_for_destinations(
    router: &MultiServiceRouter,
    table: &str,
    wanted: &str,
) -> Vec<Value> {
    let mut last = Vec::new();
    for _ in 0..POLLS {
        let (status, answered) =
            dynamodb(router, "DescribeKinesisStreamingDestination", &json!({"TableName": table}))
                .await;
        assert_eq!(
            status, 200,
            "DescribeKinesisStreamingDestination {table:?}: expected HTTP 200, got {status} \
             with body {answered}"
        );
        last = answered
            .get("KinesisDataStreamDestinations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if last
            .iter()
            .any(|destination| destination.get("DestinationStatus") == Some(&json!(wanted)))
        {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}

/// Every row: the stack completes, both tables exist in DynamoDB under the names it
/// published, the streaming one carries the throughput its declaration named, and its
/// destination settles `ACTIVE` on the ARN of the stream the stack created.
#[tokio::test]
async fn a_stack_declared_streaming_table_exists_with_its_destination_active() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let (status, answer) = create_stack(&router, row.stack, &template(&row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {answer}"
        );

        let (reported, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            reported, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {reported:?} \
             because {:?} in {described}",
            failure_reasons(&router, row.stack).await
        );

        // The outputs a client reads the estate through: the two table names and the stream
        // ARN, each carrying a value.
        let streaming_name = output_value(&described, "StreamTableName");
        assert_eq!(
            streaming_name.as_deref(),
            Some(streaming_table(&row).as_str()),
            "case {case:?}: expected Ref of the streaming table to answer its declared name, \
             got {streaming_name:?}"
        );
        let plain_name = output_value(&described, "PlainTableName");
        assert_eq!(
            plain_name.as_deref(),
            Some(plain_table(&row).as_str()),
            "case {case:?}: expected Ref of the plain table to answer its declared name, got \
             {plain_name:?}"
        );
        let stream_arn = output_value(&described, "StreamArn").unwrap_or_default();
        assert_eq!(
            stream_arn,
            format!(
                "arn:aws:kinesis:{REGION}:{ACCOUNT_ID}:stream/{}",
                stream_name(&row)
            ),
            "case {case:?}: expected the stack's StreamArn output to be the data stream's own \
             ARN, got {stream_arn:?}"
        );

        // Both declared tables EXIST in DynamoDB, under the names the stack published: the
        // whole point of a complete stack.
        for table in [streaming_table(&row), plain_table(&row)] {
            let held = describe_table(&router, &table).await;
            assert!(
                held.is_some(),
                "case {case:?}: expected DynamoDB to hold the table {table:?} the stack \
                 reports created, got no table at all because {:?}",
                failure_reasons(&router, row.stack).await
            );
        }

        // The streaming table carries the capacity its declaration asked for.
        let table = describe_table(&router, &streaming_table(&row))
            .await
            .expect("the table the stack created");
        match row.throughput {
            Some((read, write)) => {
                let provisioned = table.get("ProvisionedThroughput").cloned();
                let reported = provisioned.as_ref().map(|figures| {
                    (
                        figures.get("ReadCapacityUnits").cloned(),
                        figures.get("WriteCapacityUnits").cloned(),
                    )
                });
                assert_eq!(
                    reported,
                    Some((Some(json!(read)), Some(json!(write)))),
                    "case {case:?}: expected the declared {read}/{write} provisioned \
                     throughput, got {provisioned:?}"
                );
            }
            None => {
                let mode = table
                    .get("BillingModeSummary")
                    .and_then(|summary| summary.get("BillingMode"))
                    .cloned();
                assert_eq!(
                    mode,
                    Some(json!("PAY_PER_REQUEST")),
                    "case {case:?}: expected the declared on-demand capacity mode, got {mode:?}"
                );
            }
        }

        // And the destination its `KinesisStreamSpecification` declared: ONE record, on the
        // stack's own stream, settled ACTIVE, under the precision the declaration named.
        let destinations =
            wait_for_destinations(&router, &streaming_table(&row), "ACTIVE").await;
        assert_eq!(
            destinations.len(),
            1,
            "case {case:?}: expected the declaration to materialise exactly one streaming \
             destination, got {destinations:?}"
        );
        let destination = &destinations[0];
        assert_eq!(
            destination.get("StreamArn"),
            Some(&json!(stream_arn)),
            "case {case:?}: expected the destination to replicate to the stack's own stream, \
             got {destination:?}"
        );
        assert_eq!(
            destination.get("DestinationStatus"),
            Some(&json!("ACTIVE")),
            "case {case:?}: expected the destination to settle ACTIVE, got {destination:?}"
        );
        // The precision the DECLARATION named is the one in force. For a declaration that
        // names NONE the member is not pinned to one spelling: AWS documents MILLISECOND as
        // the default while the model puts no `@default` on it
        // (`smithy_query(query="shape",shape="KinesisDataStreamDestination",
        // model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/\
        // com-amazonaws-dynamodb-dynamodb-20120810.json")`), so either the defaulted value or
        // no member at all is faithful; what the row asserts is that the deployment added no
        // precision of its own. The UNIT a delivered record's creation time is stated in IS
        // pinned, in the owning crate's `kinesis_destination_record_delivery`.
        let precision = destination
            .get("ApproximateCreationDateTimePrecision")
            .and_then(Value::as_str);
        match row.precision {
            Some(declared) => assert_eq!(
                precision,
                Some(declared),
                "case {case:?}: expected the declared precision {declared:?} in force, got \
                 {precision:?}"
            ),
            None => assert!(
                precision.is_none() || precision == Some("MILLISECOND"),
                "case {case:?}: expected the default precision or no precision member for a \
                 declaration that named none, got {precision:?}"
            ),
        }

        // The table the template declared NO destination for reports none, so the estate is
        // the one the template describes rather than a destination attached to every table.
        let (status, answered) = dynamodb(
            &router,
            "DescribeKinesisStreamingDestination",
            &json!({"TableName": plain_table(&row)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the plain table's destinations, got \
             {status} with body {answered}"
        );
        let plain_destinations = answered
            .get("KinesisDataStreamDestinations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            plain_destinations.is_empty(),
            "case {case:?}: expected no streaming destination on the table that declared \
             none, got {plain_destinations:?}"
        );
    }
}
