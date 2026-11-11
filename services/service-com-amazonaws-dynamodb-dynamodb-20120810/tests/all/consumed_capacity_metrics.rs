// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series this service publishes the throughput each request consumed into.
//!
//! The rule under test: every request served against a table counts the capacity units it
//! consumed into the `AWS/DynamoDB` metric for that half of the throughput —
//! `ConsumedReadCapacityUnits` for a read and `ConsumedWriteCapacityUnits` for a write —
//! dimensioned by the table it was consumed on, so a `Sum` over a period reports the units
//! that period's requests spent. Those are the series an alarm on a table's utilisation is
//! evaluated against, which is what makes a provisioned table's auto scaling see the load.
//!
//! The catalog spans what the rule generalises over: which half of the throughput an
//! operation spends, the item API and the batch API, and the capacity mode the table is in —
//! not one operation against one table.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{call, describe_when_active, ok, REGION};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts read capacity units.
const CONSUMED_READ: &str = "ConsumedReadCapacityUnits";

/// The metric that counts write capacity units.
const CONSUMED_WRITE: &str = "ConsumedWriteCapacityUnits";

/// One request, and the series its consumption lands in.
struct ChargeRow {
    case: &'static str,
    /// The table's billing mode, which decides whether its throughput is provisioned at all —
    /// the units a request consumes are counted either way.
    billing_mode: &'static str,
    /// The operation the request makes, and its body apart from the table name.
    operation: &'static str,
    /// Whether the request spends READ capacity; a write spends the other half.
    reads: bool,
}

/// The catalog: a write and a read through the item API, the same through the batch API, and
/// both capacity modes.
const CHARGE_ROWS: &[ChargeRow] = &[
    ChargeRow {
        case: "a provisioned table's write, through PutItem",
        billing_mode: "PROVISIONED",
        operation: "PutItem",
        reads: false,
    },
    ChargeRow {
        case: "a provisioned table's read, through GetItem",
        billing_mode: "PROVISIONED",
        operation: "GetItem",
        reads: true,
    },
    ChargeRow {
        case: "a provisioned table's read, through Query",
        billing_mode: "PROVISIONED",
        operation: "Query",
        reads: true,
    },
    ChargeRow {
        case: "an on-demand table's write, through PutItem",
        billing_mode: "PAY_PER_REQUEST",
        operation: "PutItem",
        reads: false,
    },
    ChargeRow {
        case: "an on-demand table's read, through Scan",
        billing_mode: "PAY_PER_REQUEST",
        operation: "Scan",
        reads: true,
    },
];

#[tokio::test]
async fn every_request_counts_the_throughput_it_consumed_into_the_table_metric() {
    for row in CHARGE_ROWS {
        let case = row.case;
        let router = emulator_with_cloudwatch();
        let table = "charged";
        create_table(&router, table, row.billing_mode).await;
        // The item the read rows read back, written before the window the assertion is about.
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": {"pk": {"S": "a"}}}),
        )
        .await;
        let before = series_sum(&router, table, metric(row.reads)).await;
        // The other half of the throughput, read BEFORE the request under test: a read spends
        // no write capacity and a write spends no read capacity, so this sum must not move.
        let other_before = series_sum(&router, table, metric(!row.reads)).await;

        let body = match row.operation {
            "PutItem" => json!({"TableName": table, "Item": {"pk": {"S": "b"}}}),
            "GetItem" => json!({"TableName": table, "Key": {"pk": {"S": "a"}}}),
            "Query" => json!({
                "TableName": table,
                "KeyConditionExpression": "pk = :pk",
                "ExpressionAttributeValues": {":pk": {"S": "a"}},
            }),
            "Scan" => json!({"TableName": table}),
            other => panic!("case {case:?}: this suite makes no {other} request"),
        };
        let (status, answered) = call(&router, row.operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {} to answer 200, got {status} {answered:?}",
            row.operation
        );

        let after = series_sum(&router, table, metric(row.reads)).await;
        assert!(
            after > before,
            "case {case:?}: expected the request to count its {} into the table's series, \
             which stayed at {before} (now {after})",
            metric(row.reads)
        );
        // And only that half of the throughput: a read spends no write capacity, and the
        // other way round.
        let other = series_sum(&router, table, metric(!row.reads)).await;
        assert!(
            (other - other_before).abs() < f64::EPSILON,
            "case {case:?}: expected the request to spend no {}, which moved from \
             {other_before} to {other}",
            metric(!row.reads)
        );
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

/// One emulator holding this service AND CloudWatch, over ONE shared engine — the series a
/// table's consumption is published into live in that service's store, so a suite that
/// asserts them reads them where CloudWatch holds them.
fn emulator_with_cloudwatch() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Create one table in `billing_mode` with a single-attribute key.
async fn create_table(router: &MultiServiceRouter, name: &str, billing_mode: &str) {
    let mut request = json!({
        "TableName": name,
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "BillingMode": billing_mode,
    });
    if billing_mode == "PROVISIONED" {
        request["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": 100, "WriteCapacityUnits": 100});
    }
    ok(router, "CreateTable", request).await;
    // A table is served once it is ACTIVE: the waiter loop a client runs after a create.
    describe_when_active(router, name, name).await;
}

/// The `Sum` CloudWatch reports for one of a table's throughput series, over a window wide
/// enough to hold everything this suite published.
async fn series_sum(router: &MultiServiceRouter, table: &str, metric_name: &str) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let body = json!({
        "Namespace": NAMESPACE,
        "MetricName": metric_name,
        "Dimensions": [{"Name": "TableName", "Value": table}],
        "StartTime": now - 3600.0,
        "EndTime": now + 3600.0,
        "Period": 60,
        "Statistics": ["Sum"],
    });
    let (status, answered) = cloudwatch(router, "GetMetricStatistics", body).await;
    assert_eq!(
        status, 200,
        "case \"the published series\": expected GetMetricStatistics to answer 200, got \
         {status} {answered:?}"
    );
    answered["Datapoints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Send one CloudWatch operation, signed for that service.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    super::emulator::dispatch_as(
        router,
        REGION,
        "monitoring",
        &format!("GraniteServiceVersion20100801.{operation}"),
        body,
    )
    .await
}
