// SPDX-License-Identifier: CC0-1.0

//! A `MetricsConfig` that names one metric TWICE, over the wire, on
//! `CreateEventSourceMapping` and on `UpdateEventSourceMapping`.
//!
//! The RULE asserted over the tables below: `EventSourceMappingMetricsConfig$Metrics` is "the
//! metrics you want your event source mapping to produce", which is a SET of the values its
//! enum declares — which is why the member's own length stops at the number of values there
//! are (`@length(min: 0, max: 3)` over a three-value enum) — so a selection that repeats a
//! value asks for one metric twice and is not a set at all. That fault is a property of the
//! LIST on its own: nothing about the mapping's event source, and nothing about which value
//! repeats, enters it, so it is refused with the modelled `InvalidParameterValueException`
//! (HTTP 400) carrying ONE fixed sentence, whatever the source and whichever value repeats,
//! and the mapping's own stored selection is left exactly as it was. A selection whose values
//! are all distinct is judged instead against what that source supports, which is the
//! separate table in `event_source_mapping_metrics_scope`.
//!
//! The rows differ along what the rule generalises over: which of the enum's values repeats,
//! how many times it appears (twice, three times), where the repetition sits in the list
//! (adjacent, or either side of another value), how long the list is, which event source the
//! mapping names (a queue, each shard source, an Amazon MSK cluster, and the self-managed
//! Apache Kafka cluster that carries no ARN — including the sources that support ALL three
//! values and the ones that support only one), and whether the selection arrives on the
//! create or on a later update.
//!
//! The metric vocabulary, its length bound and its per-source scope are the model's own
//! (`smithy_query(query="shape", shape="com.amazonaws.lambda#EventSourceMappingMetricsConfig")`
//! and `…#EventSourceMappingMetric`), which is where the values and the bound quoted above
//! come from.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::metrics::{
    DUPLICATE_METRICS_REFUSAL, ERROR_COUNT_METRICS_SELECTOR, EVENT_COUNT_METRICS_SELECTOR,
    KAFKA_METRICS_SELECTOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, list_mappings, router_on,
    DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, MSK_SOURCE, QUEUE_SOURCE,
};

/// The modelled error a repeated metric is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The `State` a mapping reports once its create has settled, which is when it is available
/// to take an update.
const STATE_ENABLED: &str = "Enabled";

/// How long a wait for the create to settle is given before the row is a failure: well above
/// the settle span the mapping's own state machine owes.
const BUDGET: Duration = Duration::from_secs(25);

/// How long between two looks while waiting for it.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

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

/// One row of the create table: the event source the mapping names and the `Metrics`
/// selection that repeats a value.
struct DuplicateRow {
    case: &'static str,
    /// `EventSourceArn`, or `None` for the mapping that names the self-managed cluster.
    event_source: Option<&'static str>,
    /// The members a mapping on this source needs beside its `MetricsConfig`.
    extras: Value,
    /// `MetricsConfig.Metrics`, in the order the request states them.
    metrics: Vec<&'static str>,
}

/// The catalog of repetitions, over the whole `EventSourceMappingMetric` enum and over event
/// sources that support one value and all three alike.
fn duplicate_rows() -> Vec<DuplicateRow> {
    vec![
        DuplicateRow {
            case: "a queue asked for the event counts twice",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR, EVENT_COUNT_METRICS_SELECTOR],
        },
        DuplicateRow {
            case: "a queue asked for the event counts three times",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![
                EVENT_COUNT_METRICS_SELECTOR,
                EVENT_COUNT_METRICS_SELECTOR,
                EVENT_COUNT_METRICS_SELECTOR,
            ],
        },
        DuplicateRow {
            case: "a queue asked for a Kafka-scoped value twice",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![KAFKA_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
        },
        DuplicateRow {
            case: "a queue asked for the event counts either side of another value",
            event_source: Some(QUEUE_SOURCE),
            extras: Value::Null,
            metrics: vec![
                EVENT_COUNT_METRICS_SELECTOR,
                ERROR_COUNT_METRICS_SELECTOR,
                EVENT_COUNT_METRICS_SELECTOR,
            ],
        },
        DuplicateRow {
            case: "a DynamoDB stream asked for the event counts twice",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR, EVENT_COUNT_METRICS_SELECTOR],
        },
        DuplicateRow {
            case: "a Kinesis data stream asked for the event counts twice",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "LATEST" }),
            metrics: vec![EVENT_COUNT_METRICS_SELECTOR, EVENT_COUNT_METRICS_SELECTOR],
        },
        DuplicateRow {
            case: "an Amazon MSK cluster asked for the Kafka consumer metrics twice",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            metrics: vec![KAFKA_METRICS_SELECTOR, KAFKA_METRICS_SELECTOR],
        },
        DuplicateRow {
            case: "an Amazon MSK cluster asked for the error count twice beside another value",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            metrics: vec![
                ERROR_COUNT_METRICS_SELECTOR,
                KAFKA_METRICS_SELECTOR,
                ERROR_COUNT_METRICS_SELECTOR,
            ],
        },
        DuplicateRow {
            case: "a self-managed Apache Kafka cluster asked for the error count twice",
            event_source: None,
            extras: self_managed_cluster(),
            metrics: vec![ERROR_COUNT_METRICS_SELECTOR, ERROR_COUNT_METRICS_SELECTOR],
        },
    ]
}

/// The `CreateEventSourceMapping` body of one row, for the function `function`.
fn request_of(row: &DuplicateRow, function: &str) -> Value {
    let mut extras = row.extras.clone();
    if !extras.is_object() {
        extras = json!({});
    }
    extras["MetricsConfig"] = json!({ "Metrics": row.metrics });
    create_request(function, row.event_source, &extras)
}

/// Assert `answer` is the modelled 400 a repeated metric answers, carrying the ONE fixed
/// sentence that refusal states.
fn assert_duplicate_refusal(case: &str, status: u16, code: &str, message: &str, text: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a selection that repeats a metric must answer 400, got {status} {text}"
    );
    assert!(
        code.contains(INVALID_PARAMETER),
        "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
    );
    assert_eq!(
        message, DUPLICATE_METRICS_REFUSAL,
        "case {case:?}: the refusal must state that the configuration repeats a metric, got \
         {message:?}"
    );
}

/// RULE: a `MetricsConfig` whose `Metrics` repeats a value is not the SET the member takes, so
/// `CreateEventSourceMapping` refuses it with the modelled `InvalidParameterValueException`
/// and its one fixed sentence — whatever the event source and whichever value repeats — and
/// creates no mapping.
#[tokio::test]
async fn a_create_refuses_a_repeated_metric_on_its_own_terms() {
    let router = emulator();
    for (index, row) in duplicate_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("metrics-duplicate-{index}");
        create_function(&router, &function).await;
        let answer = create_mapping(&router, &request_of(row, &function)).await;
        assert_duplicate_refusal(
            case,
            answer.status,
            &answer.error_code(),
            &answer.string("message").unwrap_or_default(),
            &answer.text(),
        );

        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();
        assert_eq!(
            held,
            0,
            "case {case:?}: a refused create must store no mapping, got {held} mapping(s) in {}",
            listed.text()
        );
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

/// RULE: the same reading holds on an update — a `Metrics` that repeats a value is refused
/// with that same fixed sentence and leaves the stored selection exactly as it was, while a
/// selection of distinct values the source supports is accepted, echoed back and read back.
#[tokio::test]
async fn an_update_refuses_a_repeated_metric_and_changes_nothing() {
    let router = emulator();
    // Only the rows whose source a queue-independent update can be driven on: each row is
    // created with the SAME stored selection, so what the update left behind is read against
    // one known value.
    for (index, row) in duplicate_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("metrics-duplicate-update-{index}");
        create_function(&router, &function).await;
        let mut extras = row.extras.clone();
        if !extras.is_object() {
            extras = json!({});
        }
        extras["MetricsConfig"] = json!({ "Metrics": [EVENT_COUNT_METRICS_SELECTOR] });
        let created =
            create_mapping(&router, &create_request(&function, row.event_source, &extras)).await;
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
        assert_duplicate_refusal(
            case,
            updated.status,
            &updated.error_code(),
            &updated.string("message").unwrap_or_default(),
            &updated.text(),
        );
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.json().get("MetricsConfig"),
            Some(&json!({ "Metrics": [EVENT_COUNT_METRICS_SELECTOR] })),
            "case {case:?}: a refused update must leave the stored selection alone, got {}",
            reading.text()
        );

        // The positive control on the same mapping: the distinct selection every source
        // supports is accepted, echoed back and read back.
        let accepted = call(
            &router,
            "PUT",
            &format!("{MAPPINGS_URI}/{uuid}"),
            &json!({ "MetricsConfig": { "Metrics": [EVENT_COUNT_METRICS_SELECTOR] } }),
        )
        .await;
        assert_eq!(
            accepted.status,
            202,
            "case {case:?}: a selection of distinct supported values must answer 202, got {} {}",
            accepted.status,
            accepted.text()
        );
        assert_eq!(
            accepted.json().get("MetricsConfig"),
            Some(&json!({ "Metrics": [EVENT_COUNT_METRICS_SELECTOR] })),
            "case {case:?}: the accepted selection must be reported back, got {}",
            accepted.text()
        );
        let read_back = get_mapping(&router, &uuid).await;
        assert_eq!(
            read_back.json().get("MetricsConfig"),
            Some(&json!({ "Metrics": [EVENT_COUNT_METRICS_SELECTOR] })),
            "case {case:?}: the accepted selection must be read back, got {}",
            read_back.text()
        );
    }
}
