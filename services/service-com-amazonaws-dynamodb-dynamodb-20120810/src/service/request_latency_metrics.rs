// SPDX-License-Identifier: CC0-1.0

//! The `AWS/DynamoDB` latency series this service publishes for each table request it
//! serves, dimensioned by the table AND by the API that was served.
//!
//! # What the series is
//!
//! Amazon DynamoDB reports how long each of its data-plane calls took as the metric
//! `SuccessfulRequestLatency`, and that family is dimensioned by the PAIR `{TableName,
//! Operation}` — the table the request addressed and the name of the API it was made with
//! (`PutItem`, `GetItem`, `Query`, …) — so a client reads latency and request COUNT per API
//! call rather than per table only. The `SampleCount` of a period is therefore how many
//! requests of that operation that table served in it, the `Average`/`Minimum`/`Maximum` are
//! the elapsed times of those requests, and the unit is milliseconds.
//!
//! The pair is the family's WHOLE identity: a series "is uniquely defined by a name, a
//! namespace, and zero or more dimensions", so this family is published under the pair and
//! under nothing else — a read of `SuccessfulRequestLatency` under `{TableName}` alone names
//! a series DynamoDB does not have and finds no datapoints, exactly as the capacity families
//! ([`crate::state::consumed_capacity_metrics`]) are published under `{TableName}` and are
//! absent under the pair.
//!
//! # Which requests contribute, and what each contributes
//!
//! A request contributes ONE observation — it was one request — per table it addressed:
//!
//! * only a SUCCESSFUL request, which is what the metric counts: a call answered with an
//!   error status contributed no successful request and so no datapoint, which is what lets a
//!   client read the success rate of an operation off this family's sample count;
//! * only a request of one of the operations the `Operation` dimension takes a value from
//!   ([`METERED_OPERATIONS`]), which is the item, batch, transactional and statement data
//!   plane. A control-plane call (a create, a describe, a listing) is not a request against a
//!   table's data, carries no `Operation` dimension value and contributes nothing;
//! * once per table, however many times the request named it: a transaction that writes two
//!   items of one table is one request of that table, so it is one sample of it.
//!
//! The elapsed time observed is the time this emulator really spent serving the request,
//! measured around the operation's own run by the seam below — not a figure chosen for it.
//!
//! # Where the datapoints go
//!
//! Into the CloudWatch emulator's own metric store, the same store `ListMetrics`,
//! `GetMetricStatistics`, `GetMetricData` and the alarm evaluation read, reached through the
//! shared engine — so one metric has one home and the publisher cannot drift from its
//! readers. Nothing here fails a request: a CloudWatch store this engine does not hold, or
//! one at capacity, drops the observation and the request is answered as it was.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use serde_json::Value;

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that reports how long a served request took.
const SUCCESSFUL_REQUEST_LATENCY: &str = "SuccessfulRequestLatency";

/// The dimension that names the table a series belongs to.
const TABLE_NAME_DIMENSION: &str = "TableName";

/// The dimension that names the API the requests of a series were made with.
const OPERATION_DIMENSION: &str = "Operation";

/// The unit an elapsed time is published in: the `StandardUnit` value for milliseconds,
/// which is what a request latency is reported in.
const MILLISECONDS_UNIT: &str = "Milliseconds";

/// Milliseconds in one second, for stating an elapsed `Duration` in the unit above.
const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// The storage resolution a standard-resolution metric is published at, in seconds — which is
/// what `PutMetricData` stores for a publisher that names no `StorageResolution`.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The lowest status of the success class, and the first status past it: "the elapsed time for
/// successful requests" is what this metric is over, and a 2xx answer is the request this
/// service served.
const SUCCESS_STATUS: u16 = 200;
const PAST_SUCCESS_STATUS: u16 = 300;

/// The value every observation this family carries stands for: one request.
const ONE_REQUEST: f64 = 1.0;

/// The operations the `Operation` dimension takes a value from: the DynamoDB data plane —
/// the item API, the batched and transactional APIs, and the three statement surfaces.
/// A request of any other operation addresses a table's CONFIGURATION rather than its data
/// and is no part of this family.
///
/// Sorted, so membership is a binary search over a constant.
const METERED_OPERATIONS: &[&str] = &[
    "BatchExecuteStatement",
    "BatchGetItem",
    "BatchWriteItem",
    "DeleteItem",
    "ExecuteStatement",
    "ExecuteTransaction",
    "GetItem",
    "PutItem",
    "Query",
    "Scan",
    "TransactGetItems",
    "TransactWriteItems",
    "UpdateItem",
];

/// The member a request that addresses ONE table names it in.
const TABLE_NAME_MEMBER: &str = "TableName";

/// The member the batched surfaces carry their per-table work in, KEYED by the table name
/// (`BatchGetItem$RequestItems`, `BatchWriteItem$RequestItems`).
const REQUEST_ITEMS_MEMBER: &str = "RequestItems";

/// The member the transactional item surfaces carry their elements in, each element naming
/// its own table on the action it states (`TransactGetItems$TransactItems`,
/// `TransactWriteItems$TransactItems`).
const TRANSACT_ITEMS_MEMBER: &str = "TransactItems";

/// The member one PartiQL statement's text arrives in.
const STATEMENT_MEMBER: &str = "Statement";

/// The members a request carrying SEVERAL statements lists them in
/// (`BatchExecuteStatement$Statements`, `ExecuteTransaction$TransactStatements`), each
/// element carrying a [`STATEMENT_MEMBER`] of its own.
const STATEMENT_LIST_MEMBERS: [&str; 2] = ["Statements", "TransactStatements"];

/// The member the values a statement's `?` markers stand for arrive in.
const PARAMETERS_MEMBER: &str = "Parameters";

/// The value the `?` markers of a statement are bound to while its TABLE is read back out of
/// it ([`statement_table`]).
///
/// A statement names its table as an identifier, never as a marker, so what the markers are
/// bound to cannot change which table the statement addresses; they are bound at all only
/// because the one statement parser of this service binds every marker before it answers, and
/// nothing of the bound value is read.
const PLACEHOLDER_PARAMETER: &str = "0";

/// Publish the latency of one request this service served into the series of each table it
/// addressed, under the operation it was served as.
///
/// Call it with NO store lock held: it takes CloudWatch's store to append the observations.
/// It is called from the one seam every request of this service passes through
/// ([`super::OperationScopedClauseForm`]), with the answer already produced, so what a
/// request took is published as part of serving it.
pub fn publish_served_request<Eng: Send + Sync + 'static>(
    engine: &Eng,
    operation: &str,
    req: &RtRequest,
    parts: &::http::request::Parts,
    response: &RtResponse,
    took: Duration,
) {
    if METERED_OPERATIONS.binary_search(&operation).is_err() {
        return;
    }
    if !(SUCCESS_STATUS..PAST_SUCCESS_STATUS).contains(&response.status().as_u16()) {
        return;
    }
    let Some(body) = req.body().bytes() else {
        return;
    };
    let Ok(document) = ::serde_json::from_slice::<Value>(body) else {
        return;
    };
    // A table name is unique within one region, and the region is the one this request's
    // endpoint names — the same resolution every handler of this service reads a table by.
    let region = crate::state::request_region(parts);
    let tables = addressed_tables(&document, &region);
    if tables.is_empty() {
        return;
    }
    publish(
        engine,
        operation,
        &tables,
        elapsed_milliseconds(took),
        SystemTime::now(),
    );
}

/// Every table one request addressed, each once, in the order the request names them.
///
/// The four FORMS a request of this data plane names a table in, rather than a case per
/// operation: the request-level member, the batched surfaces' per-table map keys, the
/// transactional surfaces' per-element member, and the statement surfaces' statement text. A
/// name spelled as the table's ARN resolves to the table's own name, because the series is
/// the TABLE's and a table is one resource however a request addressed it.
fn addressed_tables(document: &Value, region: &str) -> Vec<String> {
    let mut tables: Vec<String> = Vec::new();
    let mut addressed = |name: &str| {
        // A 2xx answer means the name was one this service resolved, so a name this
        // resolution refuses cannot arise; it is declined rather than guessed at.
        if let Ok(resolved) = crate::state::resolve_table_name_in(name, region) {
            if !tables.contains(&resolved) {
                tables.push(resolved);
            }
        }
    };
    if let Some(name) = document.get(TABLE_NAME_MEMBER).and_then(Value::as_str) {
        addressed(name);
    }
    for name in document
        .get(REQUEST_ITEMS_MEMBER)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(::serde_json::Map::keys)
    {
        addressed(name);
    }
    for element in document
        .get(TRANSACT_ITEMS_MEMBER)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // One element states ONE action (a get, a put, an update, a delete, a condition
        // check), and the action names the table it is against.
        for action in element
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, action)| action)
        {
            if let Some(name) = action.get(TABLE_NAME_MEMBER).and_then(Value::as_str) {
                addressed(name);
            }
        }
    }
    for (statement, parameters) in carried_statements(document) {
        if let Some(name) = statement_table(statement, parameters) {
            addressed(&name);
        }
    }
    tables
}

/// Every PartiQL statement one request carries, with how many values its `?` markers stand
/// for: the single statement of the statement surface, and each element of the two surfaces
/// that carry a list of them.
fn carried_statements(document: &Value) -> Vec<(&str, usize)> {
    let mut statements: Vec<(&str, usize)> = Vec::new();
    if let Some(text) = document.get(STATEMENT_MEMBER).and_then(Value::as_str) {
        statements.push((text, parameter_count(document)));
    }
    for member in STATEMENT_LIST_MEMBERS {
        for element in document
            .get(member)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(text) = element.get(STATEMENT_MEMBER).and_then(Value::as_str) {
                statements.push((text, parameter_count(element)));
            }
        }
    }
    statements
}

/// How many values the `?` markers of the statement `carrier` holds stand for.
///
/// Bounded by the most markers a statement can carry at all, which is the parser's own token
/// bound: a request that sent MORE values than its statement has markers for was refused
/// rather than served ("the parameters for the PartiQL statement" must go together with it),
/// so the bound cannot change how a statement this service really served reads.
fn parameter_count(carrier: &Value) -> usize {
    carrier
        .get(PARAMETERS_MEMBER)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
        .min(crate::state::MAX_STATEMENT_TOKENS)
}

/// The table one statement addresses, read by the SAME parser that served it
/// ([`crate::state::parse_statement`]) so the table a statement was run against and the table
/// its latency is booked under cannot differ.
///
/// `None` for text that parser does not accept, which a served statement is not.
fn statement_table(statement: &str, parameters: usize) -> Option<String> {
    let bound = vec![
        crate::ops::types::AttributeValue::S(PLACEHOLDER_PARAMETER.to_string());
        parameters
    ];
    crate::state::parse_statement(statement, &bound)
        .ok()
        .map(|parsed| parsed.table().to_string())
}

/// `took` as the milliseconds this family's datapoints carry.
fn elapsed_milliseconds(took: Duration) -> f64 {
    took.as_secs_f64() * MILLISECONDS_PER_SECOND
}

/// Append one observation of `milliseconds` to each table's series, under `operation`.
fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    operation: &str,
    tables: &[String],
    milliseconds: f64,
    at: SystemTime,
) {
    let Some(metrics) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let timestamp_secs = at
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let mut store = metrics.lock();
    for table in tables {
        let dimensions: cloudwatch::state::DimensionSet = [
            (TABLE_NAME_DIMENSION.to_string(), table.clone()),
            (OPERATION_DIMENSION.to_string(), operation.to_string()),
        ]
        .into_iter()
        .collect();
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: SUCCESSFUL_REQUEST_LATENCY.to_string(),
            dimensions,
        };
        let datum = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit its
            // own readers window by.
            timestamp_secs,
            // One observation: this request, which took what it took.
            sample_count: ONE_REQUEST,
            sum: milliseconds,
            minimum: milliseconds,
            maximum: milliseconds,
            unit: Some(MILLISECONDS_UNIT.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already served,
            // so nothing is failed for it; the dropped observation is reported where a
            // developer can see it.
            ::tracing::warn!(
                table = %table,
                operation = %operation,
                "the CloudWatch metric store is at capacity, so the latency of this request \
                 was not published"
            );
        }
    }
}
