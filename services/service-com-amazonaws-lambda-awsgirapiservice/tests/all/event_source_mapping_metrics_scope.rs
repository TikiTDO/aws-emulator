// SPDX-License-Identifier: CC0-1.0

//! Which `MetricsConfig` values an event source mapping may be asked for, judged against the
//! EVENT SOURCE the mapping resolves — over the wire, on `CreateEventSourceMapping` and on
//! `UpdateEventSourceMapping`.
//!
//! The RULE asserted over the tables below: `EventSourceMappingMetricsConfig$Metrics` is "the
//! metrics you want your event source mapping to produce", which is a SET of the values its
//! enum declares drawn from the ones the mapping's own event source supports — so a value
//! that source does not support leaves the selection short of being one and is refused with
//! the modelled `InvalidParameterValueException` (HTTP 400) naming the values that source
//! DOES expect, with no mapping created or changed. Two of the three values are documented
//! for the Kafka event sources alone — "`ErrorCount` (Amazon MSK and self-managed Apache
//! Kafka)" and "`KafkaMetrics` (Amazon MSK and self-managed Apache Kafka)" — so they are
//! supported only on a mapping whose event source is an Amazon MSK cluster or VPC connection
//! or a self-managed Apache Kafka cluster. `EventCount` carries no such scope and is
//! supported on every source. A selection that REPEATS a value falls short of being a set for
//! a reason of the LIST alone and carries its own fixed sentence, whatever the source; that
//! class is tabled in `event_source_mapping_metrics_duplicates`.
//!
//! The rows differ along what the rule generalises over: which event source the mapping
//! names (each vendor `EventSourceArn` enumerates, plus the self-managed cluster that has no
//! ARN), which of the enum's values the selection holds, how many it holds, in which order,
//! and whether the selection arrives on the create or on a later update.
//!
//! The metric vocabulary and its per-source scope are the model's own
//! (`smithy_query(query="shape", shape="com.amazonaws.lambda#EventSourceMappingMetricsConfig")`
//! and `…#EventSourceMappingMetric`), which is where the values and the parentheses quoted
//! above come from.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::metrics::{
    ERROR_COUNT_METRICS_SELECTOR, EVENT_COUNT_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_request, create_mapping, get_mapping, router_on, list_mappings,
    DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, MSK_SOURCE, QUEUE_SOURCE,
};

/// The modelled error a selection the event source does not support is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The whole refusal a selection its event source does not support answers: `expected` is
/// the values that source does expect, as the message lists them.
fn refusal(expected: &str) -> String {
    format!("Invalid EventSourceMapping metrics. Expected values: [{expected}]")
}

/// The `State` a mapping reports once its create has settled, which is when it is available
/// to take an update.
const STATE_ENABLED: &str = "Enabled";

/// How long a wait for the create to settle is given before the row is a failure: well above
/// the settle span the mapping's own state machine owes.
const BUDGET: Duration = Duration::from_secs(25);

/// How long between two looks while waiting for it.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// An Amazon MSK VPC connection, which is the second resource type `EventSourceArn`
/// documents for Amazon MSK: "the ARN of the VPC connection (for cross-account event source
/// mappings)".
const MSK_VPC_CONNECTION_SOURCE: &str =
    "arn:aws:kafka:us-east-1:000000000000:vpc-connection/order-cluster/0bb1c2d3-4e56-7890-abcd-ef1234567890-2";

/// An Amazon MQ broker, from the same enumeration: "Amazon MQ – The ARN of the broker".
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:order-broker:b-1111";

/// An Amazon DocumentDB cluster, from the same enumeration: "Amazon DocumentDB – The ARN of
/// the DocumentDB change stream".
const DOCDB_SOURCE: &str = "arn:aws:rds:us-east-1:000000000000:cluster:order-docdb";

/// The `SelfManagedEventSource` of a self-managed Apache Kafka cluster, which is the ONE
/// event source a mapping names instead of an `EventSourceArn`.
fn self_managed_cluster() -> Value {
    json!({
        "SelfManagedEventSource": {
            "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
        },
        "Topics": ["order-events"],
        "SelfManagedKafkaEventSourceConfig": { "ConsumerGroupId": "self-managed-group" },
    })
}

/// Lambda alone: the rule is about what the control plane ACCEPTS, so no queue, stream or
/// cluster has to exist for it.
fn emulator() -> MultiServiceRouter {
    router_on(&std::sync::Arc::new(engine::InMemoryEngine::new()))
}

/// What the rule does with one row: accept the selection, or refuse it naming the values the
/// event source does expect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The selection is one this event source supports.
    Accepted,
    /// The selection is not, and these are the values that source expects.
    Refused(&'static str),
}

/// One row of the scope table: the event source the mapping names, the `Metrics` selection,
/// and what the rule does with that pair.
struct ScopeRow {
    case: &'static str,
    /// `EventSourceArn`, or `None` for the mapping that names the self-managed cluster.
    event_source: Option<&'static str>,
    /// The members a mapping on this source needs beside its `MetricsConfig`.
    extras: Value,
    /// `MetricsConfig.Metrics`, in the order the request states them.
    metrics: Vec<&'static str>,
    /// What the rule does with this source and this selection.
    verdict: Verdict,
}

/// The catalog: every vendor `EventSourceArn` enumerates (Amazon SQS, Amazon Kinesis, Amazon
/// DynamoDB Streams, Amazon MSK as a cluster and as a VPC connection, Amazon MQ, Amazon
/// DocumentDB) and the self-managed Apache Kafka cluster that carries no ARN, each against
/// selections drawn from the whole `EventSourceMappingMetric` enum — one value, two values,
/// all three, and the empty list.
fn scope_rows() -> Vec<ScopeRow> {
    vec![
        ScopeRow {
            case: "a queue asked for the event counts",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "a queue asked for nothing at all",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: Vec::new(),
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "a queue asked for the Kafka consumer metrics",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a queue asked for the processing error count",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a queue asked for the event counts beside a Kafka metric",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a queue asked for both Kafka metrics, the error count first",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a Kinesis data stream asked for the processing error count",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "LATEST" }),
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a Kinesis data stream asked for the event counts",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "LATEST" }),
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "a DynamoDB stream asked for the Kafka consumer metrics",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            metrics: vec![KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "an Amazon MQ broker asked for the processing error count",
            event_source: Some(MQ_SOURCE),
            extras: json!({ "Queues": ["order-queue"] }),
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "a DocumentDB change stream asked for the Kafka consumer metrics",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({ "DocumentDBEventSourceConfig": { "DatabaseName": "orders" } }),
            metrics: vec![KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        ScopeRow {
            case: "an Amazon MSK cluster asked for the Kafka consumer metrics",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            metrics: vec![KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "an Amazon MSK cluster asked for all three metrics",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            metrics: vec![
                EVENT_COUNT_METRICS_SELECTOR,
                ERROR_COUNT_METRICS_SELECTOR,
                KAFKA_METRICS_SELECTOR,
            ],
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "an Amazon MSK VPC connection asked for the processing error count",
            event_source: Some(MSK_VPC_CONNECTION_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
        ScopeRow {
            case: "a self-managed Apache Kafka cluster asked for both Kafka metrics",
            event_source: None,
            extras: self_managed_cluster(),
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
    ]
}

/// The `CreateEventSourceMapping` body of one row, for the function `function`.
fn request_of(row: &ScopeRow, function: &str) -> Value {
    let mut extras = row.extras.clone();
    if !extras.is_object() {
        extras = json!({});
    }
    extras["MetricsConfig"] = json!({ "Metrics": row.metrics });
    create_request(function, row.event_source, &extras)
}

/// RULE: a `MetricsConfig` is a SET of metrics the mapping's own event source supports — a
/// selection naming a value the model scopes to the Kafka event sources, on a mapping whose
/// source is not one of those, is refused with the modelled
/// `InvalidParameterValueException` naming the values that source expects, and no mapping is
/// created.
#[tokio::test]
async fn a_metrics_config_is_judged_against_the_resolved_event_source() {
    let router = emulator();
    for (index, row) in scope_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("metrics-scope-{index}");
        create_function(&router, &function).await;
        let answer = create_mapping(&router, &request_of(row, &function)).await;

        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();
        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    answer.status,
                    202,
                    "case {case:?}: this selection is one the event source can be asked for, \
                     so the create must answer 202, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.json().get("MetricsConfig"),
                    Some(&json!({ "Metrics": row.metrics })),
                    "case {case:?}: the accepted selection must be reported back, got {}",
                    answer.text()
                );
                assert_eq!(
                    held, 1,
                    "case {case:?}: the accepted mapping must be held, got {held} mapping(s) \
                     in {}",
                    listed.text()
                );
            }
            Verdict::Refused(expected) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: this selection is not one the event source supports, so \
                     the create must answer 400, got {} {}",
                    answer.status,
                    answer.text()
                );
                let code = answer.error_code();
                assert!(
                    code.contains(INVALID_PARAMETER),
                    "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
                );
                let message = answer.string("message").unwrap_or_default();
                assert_eq!(
                    message,
                    refusal(expected),
                    "case {case:?}: the refusal must name the values this event source \
                     expects, got {message:?}"
                );
                assert_eq!(
                    held, 0,
                    "case {case:?}: a refused create must store no mapping, got {held} \
                     mapping(s) in {}",
                    listed.text()
                );
            }
        }
    }
}

/// Wait until the mapping `uuid` names reports `Enabled`, which is when it is available to
/// take an update.
async fn wait_for_enabled(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        if reading.string("State").as_deref() == Some(STATE_ENABLED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping never reported {STATE_ENABLED}: {}",
            reading.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the update table: the selection the update states, and what the rule does with
/// it for a mapping on an Amazon SQS queue.
struct UpdateRow {
    case: &'static str,
    metrics: Vec<&'static str>,
    verdict: Verdict,
}

/// The catalog of selections an update of a QUEUE mapping can state, over the same enum: the
/// one value that source supports, an emptied selection, and each Kafka-scoped value, alone
/// and beside the event counts.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "the event counts",
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Accepted,
        },
        UpdateRow {
            case: "an emptied selection",
            metrics: Vec::new(),
            verdict: Verdict::Accepted,
        },
        UpdateRow {
            case: "the Kafka consumer metrics",
            metrics: vec![KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        UpdateRow {
            case: "the processing error count",
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
        UpdateRow {
            case: "the event counts beside the Kafka consumer metrics",
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
            verdict: Verdict::Refused(EVENT_COUNT_METRICS_SELECTOR),
        },
    ]
}

/// RULE: the same reading holds on an update — a selection naming a value the mapping's own
/// event source does not support is refused with the modelled
/// `InvalidParameterValueException` and leaves the stored selection exactly as it was.
#[tokio::test]
async fn an_update_cannot_select_a_metric_outside_the_mapped_source_scope() {
    let router = emulator();
    for (index, row) in update_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("metrics-scope-update-{index}");
        create_function(&router, &function).await;
        // Every row starts from the SAME stored selection, so what the update left behind is
        // read against one known value.
        let created = create_mapping(
            &router,
            &create_request(
                &function,
                Some(QUEUE_SOURCE),
                &json!({ "MetricsConfig": { "Metrics": [EVENT_COUNT_METRICS_SELECTOR] } }),
            ),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: the mapping to update must be created, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        wait_for_enabled(&router, case, &uuid).await;

        let updated = call(
            &router,
            "PUT",
            &format!("{MAPPINGS_URI}/{uuid}"),
            &json!({ "MetricsConfig": { "Metrics": row.metrics } }),
        )
        .await;
        let reading = get_mapping(&router, &uuid).await;
        let stored = reading.json().get("MetricsConfig").cloned();
        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    updated.status,
                    202,
                    "case {case:?}: this selection is one a queue supports, so the update \
                     must answer 202, got {} {}",
                    updated.status,
                    updated.text()
                );
                assert_eq!(
                    stored,
                    Some(json!({ "Metrics": row.metrics })),
                    "case {case:?}: the accepted selection must be read back, got {}",
                    reading.text()
                );
            }
            Verdict::Refused(expected) => {
                assert_eq!(
                    updated.status,
                    400,
                    "case {case:?}: this selection is not one a queue supports, so the update \
                     must answer 400, got {} {}",
                    updated.status,
                    updated.text()
                );
                let code = updated.error_code();
                assert!(
                    code.contains(INVALID_PARAMETER),
                    "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
                );
                let message = updated.string("message").unwrap_or_default();
                assert_eq!(
                    message,
                    refusal(expected),
                    "case {case:?}: the refusal must name the values a queue expects, got \
                     {message:?}"
                );
                assert_eq!(
                    stored,
                    Some(json!({ "Metrics": [EVENT_COUNT_METRICS_SELECTOR] })),
                    "case {case:?}: a refused update must leave the stored selection alone, \
                     got {}",
                    reading.text()
                );
            }
        }
    }
}
