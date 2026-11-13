// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch latency series this service publishes for each table request it serves.
//!
//! The rule under test: every successfully served DynamoDB data-plane request publishes one
//! `AWS/DynamoDB` `SuccessfulRequestLatency` observation per table it addressed, under the
//! dimension PAIR `{TableName, Operation}` with the name of the API that was served — so the
//! `SampleCount` of a period is how many requests of that operation that table served in it
//! and the `Average`/`Minimum`/`Maximum` are their elapsed times in milliseconds. The pair is
//! the whole identity of the series: nothing of this family is published under `{TableName}`
//! alone, and an operation that was not called has no series at all.
//!
//! The catalog spans what the rule generalises over — which API the request was made with
//! (the item, batched, transactional and statement surfaces), how many requests were made,
//! and how many tables one request addresses — rather than one operation against one table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{call, describe_when_active, emulator_with_cloudwatch, ok, ok_cloudwatch};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that reports how long a served request took.
const METRIC: &str = "SuccessfulRequestLatency";

/// The unit an elapsed request time is reported in.
const MILLISECONDS: &str = "Milliseconds";

/// The partition key value the seeded item carries, which the reads address.
const SEEDED_KEY: &str = "seed";

/// One row of the catalog: an operation, the requests this suite makes of it, and an
/// operation it does NOT make, whose series must therefore stay empty.
struct LatencyRow {
    case: &'static str,
    /// The API served, which is the `Operation` dimension value its samples are booked under.
    operation: &'static str,
    /// The request bodies sent, one per call — several, where the rule is about the count.
    bodies: fn(&str) -> Vec<Value>,
    /// An operation this row never calls: no request of it was served, so the table has no
    /// series for it.
    never_called: &'static str,
}

/// The catalog: the item API's reads and writes, the batched surfaces, the transactional
/// surfaces and the statement surface, over request counts of one, two and three.
const LATENCY_ROWS: &[LatencyRow] = &[
    LatencyRow {
        case: "three writes through PutItem",
        operation: "PutItem",
        bodies: |table| {
            (0..3)
                .map(|index| json!({"TableName": table, "Item": {"pk": {"S": format!("w{index}")}}}))
                .collect()
        },
        never_called: "DeleteItem",
    },
    LatencyRow {
        case: "two reads through GetItem",
        operation: "GetItem",
        bodies: |table| {
            (0..2)
                .map(|_| json!({"TableName": table, "Key": {"pk": {"S": SEEDED_KEY}}}))
                .collect()
        },
        never_called: "Scan",
    },
    LatencyRow {
        case: "one write through UpdateItem",
        operation: "UpdateItem",
        bodies: |table| {
            vec![json!({
                "TableName": table,
                "Key": {"pk": {"S": SEEDED_KEY}},
                "UpdateExpression": "SET n = :n",
                "ExpressionAttributeValues": {":n": {"N": "1"}},
            })]
        },
        never_called: "Query",
    },
    LatencyRow {
        case: "one write through DeleteItem",
        operation: "DeleteItem",
        bodies: |table| vec![json!({"TableName": table, "Key": {"pk": {"S": SEEDED_KEY}}})],
        never_called: "GetItem",
    },
    LatencyRow {
        case: "two reads through Query",
        operation: "Query",
        bodies: |table| {
            (0..2)
                .map(|_| {
                    json!({
                        "TableName": table,
                        "KeyConditionExpression": "pk = :pk",
                        "ExpressionAttributeValues": {":pk": {"S": SEEDED_KEY}},
                    })
                })
                .collect()
        },
        never_called: "Scan",
    },
    LatencyRow {
        case: "one read through Scan",
        operation: "Scan",
        bodies: |table| vec![json!({"TableName": table})],
        never_called: "Query",
    },
    LatencyRow {
        case: "two writes through BatchWriteItem",
        operation: "BatchWriteItem",
        bodies: |table| {
            (0..2)
                .map(|index| {
                    json!({
                        "RequestItems": {
                            table: [{
                                "PutRequest": {
                                    "Item": {"pk": {"S": format!("b{index}")}},
                                },
                            }],
                        },
                    })
                })
                .collect()
        },
        never_called: "BatchGetItem",
    },
    LatencyRow {
        case: "one read through BatchGetItem",
        operation: "BatchGetItem",
        bodies: |table| {
            vec![json!({
                "RequestItems": {table: {"Keys": [{"pk": {"S": SEEDED_KEY}}]}},
            })]
        },
        never_called: "BatchWriteItem",
    },
    LatencyRow {
        case: "one write through TransactWriteItems",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{
                    "Put": {"TableName": table, "Item": {"pk": {"S": "t0"}}},
                }],
            })]
        },
        never_called: "TransactGetItems",
    },
    LatencyRow {
        case: "one read through TransactGetItems",
        operation: "TransactGetItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{
                    "Get": {"TableName": table, "Key": {"pk": {"S": SEEDED_KEY}}},
                }],
            })]
        },
        never_called: "TransactWriteItems",
    },
    LatencyRow {
        case: "two statements through ExecuteStatement, one of them parameterised",
        operation: "ExecuteStatement",
        bodies: |table| {
            vec![
                json!({"Statement": format!("SELECT * FROM {table} WHERE pk = '{SEEDED_KEY}'")}),
                json!({
                    "Statement": format!("SELECT * FROM {table} WHERE pk = ?"),
                    "Parameters": [{"S": SEEDED_KEY}],
                }),
            ]
        },
        never_called: "ExecuteTransaction",
    },
    LatencyRow {
        case: "two statements through BatchExecuteStatement",
        operation: "BatchExecuteStatement",
        bodies: |table| {
            vec![json!({
                "Statements": [
                    {"Statement": format!("SELECT * FROM {table} WHERE pk = '{SEEDED_KEY}'")},
                    {
                        "Statement": format!("SELECT * FROM {table} WHERE pk = ?"),
                        "Parameters": [{"S": SEEDED_KEY}],
                    },
                ],
            })]
        },
        never_called: "ExecuteStatement",
    },
];

#[tokio::test]
async fn every_served_request_books_one_latency_sample_under_its_own_operation() {
    for row in LATENCY_ROWS {
        let case = row.case;
        let router = emulator_with_cloudwatch();
        let table = "latency";
        create_table(&router, table).await;
        seed_item(&router, table).await;

        let before = samples(&router, table, row.operation).await;
        let bodies = (row.bodies)(table);
        let mut calls = 0.0_f64;
        for body in bodies {
            calls += 1.0;
            let (status, answered) = call(&router, row.operation, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {} {body} to answer 200, got {status} {answered:?}",
                row.operation
            );
        }

        let points = datapoints(&router, table, row.operation).await;
        let after = sample_count(&points);
        assert!(
            (after - before - calls).abs() < f64::EPSILON,
            "case {case:?}: expected the {} series of {table:?} to gain {calls} samples from \
             {before}, got {after} over {points:?}",
            row.operation
        );
        for point in &points {
            let unit = point["Unit"].as_str().unwrap_or_default();
            assert_eq!(
                unit, MILLISECONDS,
                "case {case:?}: expected a latency datapoint in {MILLISECONDS}, got {unit:?} in \
                 {point:?}"
            );
            for statistic in ["Average", "Minimum", "Maximum"] {
                let value = point[statistic].as_f64();
                assert!(
                    value.is_some_and(|value| value > 0.0 && value.is_finite()),
                    "case {case:?}: expected a positive {statistic} in milliseconds, got \
                     {value:?} in {point:?}"
                );
            }
        }

        // The operation this row never made a request of has no series: the family is booked
        // under the operation that was served and under no other.
        let absent = datapoints(&router, table, row.never_called).await;
        assert!(
            absent.is_empty(),
            "case {case:?}: expected no {} series on {table:?}, got {absent:?}",
            row.never_called
        );
        // And nothing of the family is published under the table dimension ALONE: the pair is
        // the whole identity of the series.
        let table_only = read_series(
            &router,
            json!([{"Name": "TableName", "Value": table}]),
        )
        .await;
        assert!(
            table_only.is_empty(),
            "case {case:?}: expected no {METRIC} series under TableName alone, got {table_only:?}"
        );
    }
}

/// One request, several tables: each table it addressed gains ONE sample, because each of
/// them served one request — and a request that names one table twice is still one request of
/// it.
#[tokio::test]
async fn one_request_books_one_sample_per_table_it_addressed() {
    let router = emulator_with_cloudwatch();
    for table in ["left", "right"] {
        create_table(&router, table).await;
    }

    // A batched write over BOTH tables.
    ok(
        &router,
        "BatchWriteItem",
        json!({
            "RequestItems": {
                "left": [{"PutRequest": {"Item": {"pk": {"S": "a"}}}}],
                "right": [{"PutRequest": {"Item": {"pk": {"S": "a"}}}}],
            },
        }),
    )
    .await;
    for table in ["left", "right"] {
        let booked = samples(&router, table, "BatchWriteItem").await;
        assert!(
            (booked - 1.0).abs() < f64::EPSILON,
            "case \"a batched write over two tables\": expected {table:?} to book one \
             BatchWriteItem sample, got {booked}"
        );
    }

    // A transaction that writes TWO items of ONE table: one request, so one sample.
    ok(
        &router,
        "TransactWriteItems",
        json!({
            "TransactItems": [
                {"Put": {"TableName": "left", "Item": {"pk": {"S": "x"}}}},
                {"Put": {"TableName": "left", "Item": {"pk": {"S": "y"}}}},
            ],
        }),
    )
    .await;
    let booked = samples(&router, "left", "TransactWriteItems").await;
    assert!(
        (booked - 1.0).abs() < f64::EPSILON,
        "case \"a transaction writing two items of one table\": expected one \
         TransactWriteItems sample, got {booked}"
    );
    let untouched = datapoints(&router, "right", "TransactWriteItems").await;
    assert!(
        untouched.is_empty(),
        "case \"a transaction writing two items of one table\": expected no series on the \
         table it did not address, got {untouched:?}"
    );
}

/// What is NOT a successful table request books nothing: a refused request, and a
/// control-plane call, which carries no operation dimension value of this family at all.
#[tokio::test]
async fn only_successful_data_plane_requests_are_counted() {
    let router = emulator_with_cloudwatch();
    let table = "refusals";
    create_table(&router, table).await;
    seed_item(&router, table).await;
    let before = samples(&router, table, "PutItem").await;

    // A write whose condition the stored item does not satisfy is refused, so it served no
    // successful request.
    let (status, refused) = call(
        &router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {"pk": {"S": SEEDED_KEY}},
            "ConditionExpression": "attribute_not_exists(pk)",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a refused conditional write\": expected it refused, got {status} {refused:?}"
    );
    let after = samples(&router, table, "PutItem").await;
    assert!(
        (after - before).abs() < f64::EPSILON,
        "case \"a refused conditional write\": expected the PutItem sample count to stay at \
         {before}, got {after}"
    );

    // A request against a table that is not there is refused too, and mints no series.
    let (status, missing) = call(
        &router,
        "PutItem",
        json!({"TableName": "absent", "Item": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a write to a table that is not there\": expected it refused, got {status} \
         {missing:?}"
    );
    let minted = datapoints(&router, "absent", "PutItem").await;
    assert!(
        minted.is_empty(),
        "case \"a write to a table that is not there\": expected no series for it, got \
         {minted:?}"
    );

    // The control plane is not a request on a table's data: it has no series of this family.
    ok(&router, "DescribeTable", json!({"TableName": table})).await;
    for operation in ["DescribeTable", "CreateTable"] {
        let control = datapoints(&router, table, operation).await;
        assert!(
            control.is_empty(),
            "case \"a control-plane call\": expected no {operation} latency series, got \
             {control:?}"
        );
    }
}

/// Create one on-demand table with a single-attribute key, ready to serve.
async fn create_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    // A table is served once it is ACTIVE: the waiter loop a client runs after a create.
    describe_when_active(router, name, name).await;
}

/// Write the item the reads of a row address. It is written BEFORE the sample counts a row
/// asserts are read, so what it books is outside the delta under test.
async fn seed_item(router: &MultiServiceRouter, table: &str) {
    ok(
        router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": SEEDED_KEY}, "n": {"N": "0"}}}),
    )
    .await;
}

/// How many samples the `{TableName, Operation}` series holds.
async fn samples(router: &MultiServiceRouter, table: &str, operation: &str) -> f64 {
    sample_count(&datapoints(router, table, operation).await)
}

/// The sum of the sample counts of `points`: a window may span more than one period, and the
/// requests of all of them are the requests that were served.
fn sample_count(points: &[Value]) -> f64 {
    points
        .iter()
        .filter_map(|point| point["SampleCount"].as_f64())
        .sum()
}

/// Every datapoint of the `{TableName, Operation}` series.
async fn datapoints(router: &MultiServiceRouter, table: &str, operation: &str) -> Vec<Value> {
    read_series(
        router,
        json!([
            {"Name": "TableName", "Value": table},
            {"Name": "Operation", "Value": operation},
        ]),
    )
    .await
}

/// Every datapoint CloudWatch reports for the latency metric under `dimensions`, over a
/// window wide enough to hold everything this suite published.
async fn read_series(router: &MultiServiceRouter, dimensions: Value) -> Vec<Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = ok_cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": dimensions,
            "StartTime": now - 3600.0,
            "EndTime": now + 3600.0,
            "Period": 60,
            "Statistics": ["SampleCount", "Average", "Minimum", "Maximum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
