// SPDX-License-Identifier: CC0-1.0

//! WHICH `AWS/DynamoDB` series each part of a request's consumed capacity is published into.
//!
//! The rule under test: the units a request consumes are counted into the series whose
//! dimension set matches the `ConsumedCapacity` line that itemised them — the table's own line
//! under `{TableName}` alone, and each global secondary index's line under
//! `{TableName, GlobalSecondaryIndexName}` — so a client can tell base-table consumption from
//! index consumption, and the table's series is never the folded total. A local secondary
//! index draws on the table's own provisioned throughput and has no dimension of its own, so
//! its line is counted with the table's; and no series is published under an index name with
//! no table beside it.
//!
//! The catalog spans what the rule generalises over: both halves of the throughput, a write
//! that reaches the index and one that does not, the item API and the batch API, a read of the
//! table and a read served by each index kind, and both index kinds on one table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{describe_when_active, emulator_with_cloudwatch, ok, ok_cloudwatch};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts read capacity units.
const CONSUMED_READ: &str = "ConsumedReadCapacityUnits";

/// The metric that counts write capacity units.
const CONSUMED_WRITE: &str = "ConsumedWriteCapacityUnits";

/// The global secondary index every row of the catalog is measured against.
const GLOBAL_INDEX: &str = "by-group";

/// The local secondary index beside it: an index kind that has no dimension of its own.
const LOCAL_INDEX: &str = "by-score";

/// How close two capacity figures must be to be the same figure. The units are halves and
/// whole numbers, so any difference the rule is about is far wider than this.
const TOLERANCE: f64 = 1e-9;

/// One request, and what each of the table's series must have gained once it was served.
struct Row {
    case: &'static str,
    /// What must be stored before the request under test; measured before, so these writes'
    /// own consumption is not part of the deltas below.
    setup: Vec<(&'static str, Value)>,
    operation: &'static str,
    body: Value,
    /// Whether the request spends READ capacity; a write spends the other half.
    reads: bool,
    /// The units the `{TableName}` series must gain: the table's own line, plus a local
    /// secondary index's line, which comes out of the table's throughput.
    base: f64,
    /// The units the `{TableName, GlobalSecondaryIndexName}` series must gain.
    index: f64,
}

/// The catalog. Every item here is far under a capacity unit's worth of bytes, so a write
/// weighs one unit per copy it lands (the table's and each index's) and an eventually
/// consistent read weighs half a unit.
fn rows() -> Vec<Row> {
    let projected = |sort: &str| {
        json!({"pk": {"S": "p"}, "sk": {"S": sort}, "gk": {"S": "g"}, "score": {"N": "1"}})
    };
    vec![
        Row {
            case: "a write carrying both index keys",
            setup: Vec::new(),
            operation: "PutItem",
            body: json!({"TableName": "", "Item": projected("w1")}),
            reads: false,
            // The table's own unit plus the local index's, which the table pays for.
            base: 2.0,
            index: 1.0,
        },
        Row {
            case: "a write carrying the global index key only",
            setup: Vec::new(),
            operation: "PutItem",
            body: json!({
                "TableName": "",
                "Item": {"pk": {"S": "p"}, "sk": {"S": "w2"}, "gk": {"S": "g"}},
            }),
            reads: false,
            base: 1.0,
            index: 1.0,
        },
        Row {
            case: "a write carrying the local index key only",
            setup: Vec::new(),
            operation: "PutItem",
            body: json!({
                "TableName": "",
                "Item": {"pk": {"S": "p"}, "sk": {"S": "w3"}, "score": {"N": "7"}},
            }),
            reads: false,
            base: 2.0,
            // The item carries no global index key, so that index holds no entry for it.
            index: 0.0,
        },
        Row {
            case: "a write carrying neither index key",
            setup: Vec::new(),
            operation: "PutItem",
            body: json!({
                "TableName": "",
                "Item": {"pk": {"S": "p"}, "sk": {"S": "w4"}},
            }),
            reads: false,
            base: 1.0,
            index: 0.0,
        },
        Row {
            case: "an update that adds the global index key",
            setup: vec![(
                "PutItem",
                json!({"TableName": "", "Item": {"pk": {"S": "p"}, "sk": {"S": "u1"}}}),
            )],
            operation: "UpdateItem",
            body: json!({
                "TableName": "",
                "Key": {"pk": {"S": "p"}, "sk": {"S": "u1"}},
                "UpdateExpression": "SET gk = :g",
                "ExpressionAttributeValues": {":g": {"S": "g"}},
            }),
            reads: false,
            base: 1.0,
            index: 1.0,
        },
        Row {
            case: "a delete of an item the global index holds",
            setup: vec![(
                "PutItem",
                json!({
                    "TableName": "",
                    "Item": {"pk": {"S": "p"}, "sk": {"S": "d1"}, "gk": {"S": "g"}},
                }),
            )],
            operation: "DeleteItem",
            body: json!({"TableName": "", "Key": {"pk": {"S": "p"}, "sk": {"S": "d1"}}}),
            reads: false,
            base: 1.0,
            index: 1.0,
        },
        Row {
            case: "two writes through the batch API, one of them sparse",
            setup: Vec::new(),
            operation: "BatchWriteItem",
            // The batch API addresses its tables inside `RequestItems` rather than through a
            // `TableName` member, so this row's body is built where the table is known.
            body: Value::Null,
            reads: false,
            base: 2.0,
            index: 1.0,
        },
        Row {
            case: "a read of the table through GetItem",
            setup: vec![("PutItem", json!({"TableName": "", "Item": projected("r1")}))],
            operation: "GetItem",
            body: json!({"TableName": "", "Key": {"pk": {"S": "p"}, "sk": {"S": "r1"}}}),
            reads: true,
            base: 0.5,
            index: 0.0,
        },
        Row {
            case: "a read of the table through Query",
            setup: vec![("PutItem", json!({"TableName": "", "Item": projected("r2")}))],
            operation: "Query",
            body: json!({
                "TableName": "",
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "p"}},
            }),
            reads: true,
            base: 0.5,
            index: 0.0,
        },
        Row {
            case: "a Query served by the global secondary index",
            setup: vec![("PutItem", json!({"TableName": "", "Item": projected("r3")}))],
            operation: "Query",
            body: json!({
                "TableName": "",
                "IndexName": GLOBAL_INDEX,
                "KeyConditionExpression": "gk = :g",
                "ExpressionAttributeValues": {":g": {"S": "g"}},
            }),
            reads: true,
            // The read did not go to the table, which is why its own line bills nothing.
            base: 0.0,
            index: 0.5,
        },
        Row {
            case: "a Scan served by the global secondary index",
            setup: vec![("PutItem", json!({"TableName": "", "Item": projected("r4")}))],
            operation: "Scan",
            body: json!({"TableName": "", "IndexName": GLOBAL_INDEX}),
            reads: true,
            base: 0.0,
            index: 0.5,
        },
        Row {
            case: "a Query served by the local secondary index",
            setup: vec![("PutItem", json!({"TableName": "", "Item": projected("r5")}))],
            operation: "Query",
            body: json!({
                "TableName": "",
                "IndexName": LOCAL_INDEX,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "p"}},
            }),
            reads: true,
            // A local index has no series of its own: it spends the table's throughput.
            base: 0.5,
            index: 0.0,
        },
    ]
}

#[tokio::test]
async fn each_part_of_a_request_lands_in_the_series_that_itemised_it() {
    for row in rows() {
        let case = row.case;
        let router = emulator_with_cloudwatch();
        let table = "itemised";
        create_table(&router, case, table).await;
        for (operation, body) in &row.setup {
            ok(&router, operation, addressed(body, table)).await;
        }

        let before = Reading::take(&router, table).await;
        let request = match row.operation {
            // The batch API addresses its tables inside `RequestItems`, so its body is built
            // here rather than carried as one table-named member. The two puts are one
            // projected item and one sparse one, so the base and index series must part.
            "BatchWriteItem" => json!({
                "RequestItems": {
                    table: [
                        {"PutRequest": {"Item": {
                            "pk": {"S": "p"}, "sk": {"S": "b1"}, "gk": {"S": "g"},
                        }}},
                        {"PutRequest": {"Item": {"pk": {"S": "p"}, "sk": {"S": "b2"}}}},
                    ],
                },
            }),
            _ => addressed(&row.body, table),
        };
        ok(&router, row.operation, request).await;
        let after = Reading::take(&router, table).await;

        let (expected_base, expected_index) = (row.base, row.index);
        let base = after.base(row.reads) - before.base(row.reads);
        assert!(
            (base - expected_base).abs() < TOLERANCE,
            "case {case:?}: expected the {{TableName}} series of {} to gain \
             {expected_base}, got {base:?}",
            metric(row.reads)
        );
        let index = after.index(row.reads) - before.index(row.reads);
        assert!(
            (index - expected_index).abs() < TOLERANCE,
            "case {case:?}: expected the {{TableName, GlobalSecondaryIndexName}} series of \
             {} to gain {expected_index}, got {index:?}",
            metric(row.reads)
        );
        // And the base series is the base-table portion, never the folded total the two add
        // up to: that is what makes the split readable at all.
        if expected_index > 0.0 {
            let folded = expected_base + expected_index;
            assert!(
                (base - folded).abs() > TOLERANCE,
                "case {case:?}: the {{TableName}} series of {} gained the folded total \
                 {folded}, so the index portion cannot be told from the table's",
                metric(row.reads)
            );
        }
        // The other half of the throughput moved nowhere: a read spends no write capacity and
        // a write spends no read capacity, on either series.
        let other_base = after.base(!row.reads) - before.base(!row.reads);
        let other_index = after.index(!row.reads) - before.index(!row.reads);
        assert!(
            other_base.abs() < TOLERANCE && other_index.abs() < TOLERANCE,
            "case {case:?}: expected the request to spend no {}, which moved by \
             {other_base:?} on the table's series and {other_index:?} on the index's",
            metric(!row.reads)
        );
        // Neither dimension set DynamoDB does not publish ever holds a datapoint: an index
        // name with no table beside it, and the LOCAL index's name under the global index's
        // dimension.
        for (described, dimensions) in [
            (
                "the global index name alone",
                json!([{"Name": "GlobalSecondaryIndexName", "Value": GLOBAL_INDEX}]),
            ),
            (
                "the local index name under the global index dimension",
                json!([
                    {"Name": "TableName", "Value": table},
                    {"Name": "GlobalSecondaryIndexName", "Value": LOCAL_INDEX},
                ]),
            ),
        ] {
            for metric_name in [CONSUMED_READ, CONSUMED_WRITE] {
                let (points, sum) = series(&router, metric_name, dimensions.clone()).await;
                assert!(
                    points == 0 && sum.abs() < TOLERANCE,
                    "case {case:?}: expected {metric_name} dimensioned by {described} to \
                     report no datapoints, got {points} summing to {sum:?}"
                );
            }
        }
    }
}

/// What both of a table's series stood at, for both halves of the throughput.
struct Reading {
    base_read: f64,
    base_write: f64,
    index_read: f64,
    index_write: f64,
}

impl Reading {
    /// Read all four series of `table` as they stand now.
    async fn take(router: &MultiServiceRouter, table: &str) -> Self {
        let base = json!([{"Name": "TableName", "Value": table}]);
        let indexed = json!([
            {"Name": "TableName", "Value": table},
            {"Name": "GlobalSecondaryIndexName", "Value": GLOBAL_INDEX},
        ]);
        Self {
            base_read: series(router, CONSUMED_READ, base.clone()).await.1,
            base_write: series(router, CONSUMED_WRITE, base).await.1,
            index_read: series(router, CONSUMED_READ, indexed.clone()).await.1,
            index_write: series(router, CONSUMED_WRITE, indexed).await.1,
        }
    }

    /// The `{TableName}` series of one half of the throughput.
    fn base(&self, reads: bool) -> f64 {
        if reads {
            self.base_read
        } else {
            self.base_write
        }
    }

    /// The `{TableName, GlobalSecondaryIndexName}` series of one half of the throughput.
    fn index(&self, reads: bool) -> f64 {
        if reads {
            self.index_read
        } else {
            self.index_write
        }
    }
}

/// The metric one half of the throughput is counted under.
fn metric(reads: bool) -> &'static str {
    if reads {
        CONSUMED_READ
    } else {
        CONSUMED_WRITE
    }
}

/// `body` with its `TableName` filled in — the catalog carries the shape of a request, and the
/// suite decides the table it is addressed at.
fn addressed(body: &Value, table: &str) -> Value {
    let mut body = body.clone();
    body["TableName"] = json!(table);
    body
}

/// Create one on-demand table with a global AND a local secondary index, both projecting
/// everything, and wait for it to be served.
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gk", "AttributeType": "S"},
                {"AttributeName": "score", "AttributeType": "N"},
            ],
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": GLOBAL_INDEX,
                "KeySchema": [{"AttributeName": "gk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "LocalSecondaryIndexes": [{
                "IndexName": LOCAL_INDEX,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "score", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(router, case, name).await;
}

/// How many datapoints one series holds over a window wide enough for everything this suite
/// published, and what they sum to.
async fn series(
    router: &MultiServiceRouter,
    metric_name: &str,
    dimensions: Value,
) -> (usize, f64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = ok_cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric_name,
            "Dimensions": dimensions,
            "StartTime": now - 3600.0,
            "EndTime": now + 3600.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let points: Vec<&Value> = answered["Datapoints"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let sum = points.iter().filter_map(|point| point["Sum"].as_f64()).sum();
    (points.len(), sum)
}
