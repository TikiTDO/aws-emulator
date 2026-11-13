// SPDX-License-Identifier: CC0-1.0

//! What a running metric stream exports when its `OutputFormat` names one of the OpenTelemetry
//! releases, end to end over the wire: CloudWatch, Amazon Data Firehose and Amazon S3 on one
//! engine, exactly as the router mounts them.
//!
//! The rule under test: the records a running stream exports are rendered in the output format
//! that stream carries — "The output format for the stream. Valid values are `json`,
//! `opentelemetry1.0`, and `opentelemetry0.7`." (`PutMetricStreamInput$OutputFormat`) — so a
//! stream naming one of the OpenTelemetry releases delivers that release's metrics message,
//! protobuf-encoded and framed with its own length, carrying the metric's identity, the period
//! it covers, and the period's statistics as a summary: the count and the sum of "the `MAX`,
//! `MIN`, `SUM`, and `SAMPLECOUNT` statistics" a stream always sends, the smallest and largest
//! observation as the quantiles at either end, and one more quantile for each additional
//! statistic the stream asks for, which these formats take as percentiles only
//! (`MetricStreamStatisticsConfiguration$AdditionalStatistics`).
//!
//! Nothing here reads CloudWatch to check the export: every assertion is made on the bytes a
//! consumer reads out of the destination bucket of the Firehose stream, decoded with message
//! declarations of its own — so what is asserted is that those bytes ARE the protocol's
//! messages, not that this emulator can read back what it wrote.
//!
//! The pipeline (CloudWatch, Firehose and S3 on one engine) and the clock-free way a case
//! reaches a past period are the ones `metric_stream_delivery` stands up for the json format;
//! only the formats and the decoding are this suite's own.

use std::time::{Duration, Instant};

use prost::Message;
use serde_json::{json, Value};

use crate::metric_stream_delivery::{
    period_of, publish_datum, stamp, Emulator, ACCOUNT, POLL_INTERVAL, REGION, ROLE_ARN,
};

/// The `MetricStreamOutputFormat` values naming the two OpenTelemetry releases, and the one
/// naming the json documents (`smithy_query(query="shape",shape="MetricStreamOutputFormat")`).
const OPEN_TELEMETRY_1_0: &str = "opentelemetry1.0";
const OPEN_TELEMETRY_0_7: &str = "opentelemetry0.7";
const JSON: &str = "json";

/// How long a created delivery stream is waited for before a case fails: far longer than that
/// service's own clock-owed `CREATING` -> `ACTIVE` span, so a loaded machine does not fail a
/// case the clock would have served. The wait is for the STATUS, polled, not for a duration.
const ACTIVATION_DEADLINE: Duration = Duration::from_secs(120);

/// How long a delivery is waited for before a case fails — far longer than the Firehose delivery
/// span, for the same reason: what is waited for is the ARRIVAL of the records, polled.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(120);

/// The status a delivery stream takes records in (`DeliveryStreamStatus`).
const ACTIVE: &str = "ACTIVE";

/// One nanosecond count of one second, the unit an OpenTelemetry data point states an instant in.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// The span one exported record covers, in the seconds `MetricStat$Period` states a period in.
const PERIOD_SECS: f64 = 60.0;

/// The resource attributes an exported record names its origin with.
const CLOUD_PROVIDER_KEY: &str = "cloud.provider";
const CLOUD_ACCOUNT_KEY: &str = "cloud.account.id";
const CLOUD_REGION_KEY: &str = "cloud.region";
const EXPORTER_ARN_KEY: &str = "aws.exporter.arn";

/// The pairs that identify which metric a data point reports.
const NAMESPACE_KEY: &str = "Namespace";
const METRIC_NAME_KEY: &str = "MetricName";
const DIMENSIONS_KEY: &str = "Dimensions";

// ---------------------------------------------------------------------------------------------
// The consumer's side of the protocol: the messages an exported record is decoded as here. They
// are declared from the OpenTelemetry metrics message names, not from this emulator's renderer,
// so a record that is not those messages fails to decode.
// ---------------------------------------------------------------------------------------------

/// `opentelemetry.proto.collector.metrics.v1.ExportMetricsServiceRequest`.
#[derive(Clone, PartialEq, Message)]
struct ExportMetricsServiceRequest {
    #[prost(message, repeated, tag = "1")]
    resource_metrics: Vec<ResourceMetrics>,
}

/// `opentelemetry.proto.metrics.v1.ResourceMetrics`, whose second member carries the metrics in
/// both releases (the scope of 1.0, the instrumentation library of 0.7).
#[derive(Clone, PartialEq, Message)]
struct ResourceMetrics {
    #[prost(message, optional, tag = "1")]
    resource: Option<Resource>,
    #[prost(message, repeated, tag = "2")]
    scope_metrics: Vec<ScopeMetrics>,
}

/// `opentelemetry.proto.resource.v1.Resource`.
#[derive(Clone, PartialEq, Message)]
struct Resource {
    #[prost(message, repeated, tag = "1")]
    attributes: Vec<KeyValue>,
}

/// `opentelemetry.proto.metrics.v1.ScopeMetrics`.
#[derive(Clone, PartialEq, Message)]
struct ScopeMetrics {
    #[prost(message, repeated, tag = "2")]
    metrics: Vec<Metric>,
}

/// `opentelemetry.proto.metrics.v1.Metric`.
#[derive(Clone, PartialEq, Message)]
struct Metric {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(string, tag = "3")]
    unit: String,
    #[prost(message, optional, tag = "11")]
    summary: Option<Summary>,
}

/// `opentelemetry.proto.metrics.v1.Summary`, whose data points are read as the bytes of the
/// release the stream names: the two releases differ inside the data point, and a
/// length-delimited member is the same bytes whether it is declared as a message or as that
/// message's encoding.
#[derive(Clone, PartialEq, Message)]
struct Summary {
    #[prost(bytes = "vec", repeated, tag = "1")]
    data_points: Vec<Vec<u8>>,
}

/// `opentelemetry.proto.metrics.v1.SummaryDataPoint` of the 1.0 release: attributes, and a
/// fixed-width count.
#[derive(Clone, PartialEq, Message)]
struct SummaryDataPoint {
    #[prost(fixed64, tag = "2")]
    start_time_unix_nano: u64,
    #[prost(fixed64, tag = "3")]
    time_unix_nano: u64,
    #[prost(fixed64, tag = "4")]
    count: u64,
    #[prost(double, tag = "5")]
    sum: f64,
    #[prost(message, repeated, tag = "6")]
    quantile_values: Vec<ValueAtQuantile>,
    #[prost(message, repeated, tag = "7")]
    attributes: Vec<KeyValue>,
}

/// `opentelemetry.proto.metrics.v1.DoubleSummaryDataPoint` of the 0.7 release: string labels,
/// and a variable-width count.
#[derive(Clone, PartialEq, Message)]
struct DoubleSummaryDataPoint {
    #[prost(message, repeated, tag = "1")]
    labels: Vec<StringKeyValue>,
    #[prost(fixed64, tag = "2")]
    start_time_unix_nano: u64,
    #[prost(fixed64, tag = "3")]
    time_unix_nano: u64,
    #[prost(uint64, tag = "4")]
    count: u64,
    #[prost(double, tag = "5")]
    sum: f64,
    #[prost(message, repeated, tag = "6")]
    quantile_values: Vec<ValueAtQuantile>,
}

/// `opentelemetry.proto.metrics.v1.SummaryDataPoint.ValueAtQuantile`.
#[derive(Clone, PartialEq, Message)]
struct ValueAtQuantile {
    #[prost(double, tag = "1")]
    quantile: f64,
    #[prost(double, tag = "2")]
    value: f64,
}

/// `opentelemetry.proto.common.v1.KeyValue`.
#[derive(Clone, PartialEq, Message)]
struct KeyValue {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(message, optional, tag = "2")]
    value: Option<AnyValue>,
}

/// `opentelemetry.proto.common.v1.AnyValue`, of which a record uses the string and the list of
/// pairs.
#[derive(Clone, PartialEq, Message)]
struct AnyValue {
    #[prost(string, optional, tag = "1")]
    string_value: Option<String>,
    #[prost(message, optional, tag = "6")]
    kvlist_value: Option<KeyValueList>,
}

/// `opentelemetry.proto.common.v1.KeyValueList`.
#[derive(Clone, PartialEq, Message)]
struct KeyValueList {
    #[prost(message, repeated, tag = "1")]
    values: Vec<KeyValue>,
}

/// `opentelemetry.proto.common.v1.StringKeyValue`.
#[derive(Clone, PartialEq, Message)]
struct StringKeyValue {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

/// Every message the bytes of one delivered object carry, read one length-delimited message
/// after another until the bytes are spent.
fn messages_of(case: &str, key: &str, bytes: &[u8]) -> Vec<ExportMetricsServiceRequest> {
    let mut cursor: &[u8] = bytes;
    let mut messages = Vec::new();
    while !cursor.is_empty() {
        let message = ExportMetricsServiceRequest::decode_length_delimited(&mut cursor)
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: object {key} must be length-delimited OpenTelemetry metrics \
                     messages, got {error} over {} byte(s)",
                    bytes.len()
                )
            });
        messages.push(message);
    }
    messages
}

/// Every attribute of a list as `(key, string value)` pairs, in the order the list holds them.
fn string_pairs(attributes: &[KeyValue]) -> Vec<(String, String)> {
    attributes
        .iter()
        .map(|attribute| {
            (
                attribute.key.clone(),
                attribute
                    .value
                    .clone()
                    .and_then(|value| value.string_value)
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The value of one attribute of a list, by key.
fn attribute(attributes: &[KeyValue], key: &str) -> Option<String> {
    string_pairs(attributes)
        .into_iter()
        .find(|(held, _)| held == key)
        .map(|(_, value)| value)
}

/// The only data point of a decoded record, with the metric it belongs to and the resource it
/// was collected from.
struct Exported {
    resource: Vec<KeyValue>,
    metric: Metric,
    point: Point,
}

/// One record's data point, read through the declaration of the release it is framed in: the
/// members every release carries, plus the identifying pairs in whichever form that release
/// carries them.
#[derive(Debug, Default)]
struct Point {
    /// The string labels of the 0.7 release, empty for a record of the 1.0 one.
    labels: Vec<(String, String)>,
    /// The attributes of the 1.0 release, empty for a record of the 0.7 one.
    attributes: Vec<KeyValue>,
    start_time_unix_nano: u64,
    time_unix_nano: u64,
    count: u64,
    sum: f64,
    quantile_values: Vec<ValueAtQuantile>,
}

/// Read one record's single resource, metric and data point, decoding the data point as the
/// release `output_format` names declares it.
fn exported(case: &str, output_format: &str, message: &ExportMetricsServiceRequest) -> Exported {
    let resource_metrics = message
        .resource_metrics
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: a record must carry one resource, got {message:?}"));
    let scope = resource_metrics
        .scope_metrics
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: a record must carry one scope, got {message:?}"));
    let metric = scope
        .metrics
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: a record must carry one metric, got {message:?}"));
    let encoded = metric
        .summary
        .as_ref()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a streamed metric must be summarised, got {metric:?}")
        })
        .data_points
        .first()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a summary must carry one data point, got {metric:?}")
        });
    let point = if output_format == OPEN_TELEMETRY_1_0 {
        let decoded = SummaryDataPoint::decode(encoded.as_slice()).unwrap_or_else(|error| {
            panic!("case {case:?}: the data point must be the 1.0 release's: {error}")
        });
        Point {
            attributes: decoded.attributes,
            start_time_unix_nano: decoded.start_time_unix_nano,
            time_unix_nano: decoded.time_unix_nano,
            count: decoded.count,
            sum: decoded.sum,
            quantile_values: decoded.quantile_values,
            ..Point::default()
        }
    } else {
        let decoded = DoubleSummaryDataPoint::decode(encoded.as_slice()).unwrap_or_else(|error| {
            panic!("case {case:?}: the data point must be the 0.7 release's: {error}")
        });
        Point {
            labels: decoded
                .labels
                .iter()
                .map(|label| (label.key.clone(), label.value.clone()))
                .collect(),
            start_time_unix_nano: decoded.start_time_unix_nano,
            time_unix_nano: decoded.time_unix_nano,
            count: decoded.count,
            sum: decoded.sum,
            quantile_values: decoded.quantile_values,
            ..Point::default()
        }
    };
    Exported {
        resource: resource_metrics
            .resource
            .clone()
            .unwrap_or_default()
            .attributes,
        metric: metric.clone(),
        point,
    }
}

/// The Firehose delivery stream `delivery_stream`, writing into `bucket`, and the metric stream
/// `configuration` describes — created, with the delivery stream waited past its `CREATING`
/// status, which is when it takes records.
async fn given_delivery_stream(emulator: &Emulator, bucket: &str, delivery_stream: &str) {
    emulator.create_bucket(bucket).await;
    emulator
        .firehose(
            "CreateDeliveryStream",
            &json!({
                "DeliveryStreamName": delivery_stream,
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": format!("arn:aws:s3:::{bucket}"),
                    // The shortest wait `IntervalInSeconds` admits, so a case asserting that the
                    // exported records reach the bucket does not wait out a buffering interval
                    // it is not about.
                    "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                },
            }),
        )
        .await;
}

/// Wait until `delivery_stream` reports the status it takes records in, polling for the STATUS
/// rather than for a span of time.
async fn until_active(emulator: &Emulator, delivery_stream: &str) {
    let deadline = Instant::now() + ACTIVATION_DEADLINE;
    loop {
        let described = emulator
            .firehose(
                "DescribeDeliveryStream",
                &json!({"DeliveryStreamName": delivery_stream}),
            )
            .await;
        let status = described
            .get("DeliveryStreamDescription")
            .and_then(|description| description.get("DeliveryStreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == ACTIVE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "delivery stream {delivery_stream} must reach {ACTIVE} before it takes records, last \
             status {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `PutMetricStream` request for a stream named `name` in `output_format`, streaming only
/// `namespace` and asking for `statistics` on `metric_name` where a row names any.
fn metric_stream_request(
    name: &str,
    delivery_stream: &str,
    output_format: &str,
    namespace: &str,
    metric_name: &str,
    statistics: Option<&[&str]>,
) -> Value {
    let mut request = json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/{delivery_stream}"),
        "RoleArn": ROLE_ARN,
        "OutputFormat": output_format,
        "IncludeFilters": [{"Namespace": namespace}],
    });
    if let Some(statistics) = statistics {
        request["StatisticsConfigurations"] = json!([{
            "IncludeMetrics": [{"Namespace": namespace, "MetricName": metric_name}],
            "AdditionalStatistics": statistics,
        }]);
    }
    request
}

/// Every record delivered into `bucket`, in key order, once at least `want` of them have
/// arrived — or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
async fn delivered_records(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    want: usize,
) -> Vec<ExportMetricsServiceRequest> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let mut records: Vec<ExportMetricsServiceRequest> = Vec::new();
        for key in emulator.keys(bucket).await {
            let bytes = emulator.object(bucket, &key).await;
            records.extend(messages_of(case, &key, &bytes));
        }
        if records.len() >= want {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {want} exported record(s) in {bucket}, got {}",
            records.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One format case: the stream's configuration, the datum published, and what the record it
/// exports must carry.
struct FormatRow {
    case: &'static str,
    /// The `OutputFormat` the stream is created with.
    output_format: &'static str,
    /// The namespace the row publishes into, which its stream streams and no other row's does.
    namespace: &'static str,
    metric_name: &'static str,
    unit: &'static str,
    /// The metric's dimensions, which are part of its identity.
    dimensions: Vec<(&'static str, &'static str)>,
    /// The members carrying the row's observations (`Value`, `Values`, `StatisticValues`).
    observations: Value,
    /// The additional statistics the stream asks for, where the row asks for any.
    statistics: Option<Vec<&'static str>>,
    /// The count and sum of the period, and its smallest and largest observation.
    count: u64,
    sum: f64,
    minimum: f64,
    maximum: f64,
    /// The quantiles the record must report, ascending.
    quantiles: Vec<f64>,
}

/// The format cases.
///
/// The rows differ along what the rule generalises over: which release the stream names, how the
/// observations were published (one value, a list of values, a statistic set), how many
/// dimensions the metric carries, which unit it is measured in, and how many additional
/// percentiles the stream asks for — including one the period cannot define, which is reported
/// as nothing rather than invented.
fn format_rows() -> Vec<FormatRow> {
    vec![
        FormatRow {
            case: "the 1.0 release, one value, no dimension, no additional statistic",
            output_format: OPEN_TELEMETRY_1_0,
            namespace: "Custom/OtelPlain",
            metric_name: "StreamedMetric",
            unit: "None",
            dimensions: vec![],
            observations: json!({"Value": 777.0}),
            statistics: None,
            count: 1,
            sum: 777.0,
            minimum: 777.0,
            maximum: 777.0,
            quantiles: vec![0.0, 1.0],
        },
        FormatRow {
            case: "the 1.0 release, a distribution, one dimension, p99",
            output_format: OPEN_TELEMETRY_1_0,
            namespace: "Custom/OtelPercentile",
            metric_name: "Latency",
            unit: "Milliseconds",
            dimensions: vec![("Service", "checkout")],
            observations: json!({"Values": (1..=10).map(f64::from).collect::<Vec<f64>>()}),
            statistics: Some(vec!["p99"]),
            count: 10,
            sum: 55.0,
            minimum: 1.0,
            maximum: 10.0,
            quantiles: vec![0.0, 0.99, 1.0],
        },
        FormatRow {
            case: "the 0.7 release, a distribution, two dimensions, p50 and p99",
            output_format: OPEN_TELEMETRY_0_7,
            namespace: "Custom/OtelEarlier",
            metric_name: "Latency",
            unit: "Count",
            dimensions: vec![("Service", "checkout"), ("Stage", "prod")],
            observations: json!({"Values": (1..=10).map(f64::from).collect::<Vec<f64>>()}),
            statistics: Some(vec!["p50", "p99"]),
            count: 10,
            sum: 55.0,
            minimum: 1.0,
            maximum: 10.0,
            quantiles: vec![0.0, 0.5, 0.99, 1.0],
        },
        FormatRow {
            case: "the 0.7 release, a statistic set whose percentile cannot be computed",
            output_format: OPEN_TELEMETRY_0_7,
            namespace: "Custom/OtelStatisticSet",
            metric_name: "PayloadSize",
            unit: "Bytes",
            dimensions: vec![],
            observations: json!({
                "StatisticValues": {
                    "SampleCount": 4.0,
                    "Sum": 100.0,
                    "Minimum": 10.0,
                    "Maximum": 40.0,
                },
            }),
            statistics: Some(vec!["p99"]),
            count: 4,
            sum: 100.0,
            minimum: 10.0,
            maximum: 40.0,
            // The individual values were never published, so the asked percentile is no value
            // this store holds: only the ends of the distribution are reported.
            quantiles: vec![0.0, 1.0],
        },
    ]
}

/// The `MetricDatum` a row publishes: its identity, its unit, its observations and a stamp in
/// the period before the one the clock is in.
fn datum_of(row: &FormatRow) -> Value {
    let mut datum = json!({
        "MetricName": row.metric_name,
        "Unit": row.unit,
        "Timestamp": stamp(1),
    });
    if !row.dimensions.is_empty() {
        datum["Dimensions"] = row
            .dimensions
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value}))
            .collect();
    }
    for (member, value) in row
        .observations
        .as_object()
        .expect("a row's observations are the members carrying them")
    {
        datum[member] = value.clone();
    }
    datum
}

/// RULE: a running stream whose `OutputFormat` names an OpenTelemetry release exports each
/// metric update as that release's metrics message — the metric's identity and unit, the period
/// it covers, and the period's statistics as a summary with one quantile per additional
/// percentile the stream asks for.
#[tokio::test]
async fn an_open_telemetry_stream_exports_each_period_as_a_summary() {
    let emulator = Emulator::new();
    let rows = format_rows();
    // Every row's pipeline is created first and waited for together, so the suite waits once for
    // the status a delivery stream takes records in rather than once per row.
    for index in 0..rows.len() {
        given_delivery_stream(&emulator, &bucket_of(index), &delivery_stream_of(index)).await;
    }
    for (index, row) in rows.iter().enumerate() {
        until_active(&emulator, &delivery_stream_of(index)).await;
        emulator
            .cloudwatch(
                "PutMetricStream",
                &metric_stream_request(
                    &stream_of(index),
                    &delivery_stream_of(index),
                    row.output_format,
                    row.namespace,
                    row.metric_name,
                    row.statistics.as_deref(),
                ),
            )
            .await;
    }

    for (index, row) in rows.iter().enumerate() {
        // The period the row publishes into, read once: a period boundary crossed while the
        // pipeline delivers moves the clock on, but not the stamp the record already carries.
        let period = period_of(1);
        publish_datum(&emulator, row.namespace, datum_of(row)).await;
        let records = delivered_records(&emulator, row.case, &bucket_of(index), 1).await;
        assert_eq!(
            records.len(),
            1,
            "case {:?}: one metric update of one period must export one record, got {records:?}",
            row.case
        );
        let read = exported(row.case, row.output_format, &records[0]);

        // WHERE the record came from.
        for (key, expected) in [
            (CLOUD_PROVIDER_KEY, "aws".to_string()),
            (CLOUD_ACCOUNT_KEY, ACCOUNT.to_string()),
            (CLOUD_REGION_KEY, REGION.to_string()),
            (
                EXPORTER_ARN_KEY,
                format!(
                    "arn:aws:cloudwatch:{REGION}:{ACCOUNT}:metric-stream/{}",
                    stream_of(index)
                ),
            ),
        ] {
            assert_eq!(
                attribute(&read.resource, key),
                Some(expected.clone()),
                "case {:?}: expected the resource attribute {key} to be {expected:?}, got {:?}",
                row.case,
                read.resource
            );
        }

        // WHICH metric it reports, and in which unit.
        assert_eq!(
            (read.metric.name.as_str(), read.metric.unit.as_str()),
            (row.metric_name, row.unit),
            "case {:?}: expected the metric's name and unit, got {:?}",
            row.case,
            read.metric
        );
        assert_identity(row, &read.point);

        // WHICH SPAN it covers.
        let start = (period as u64) * NANOS_PER_SECOND;
        assert_eq!(
            (read.point.start_time_unix_nano, read.point.time_unix_nano),
            (start, start + (PERIOD_SECS as u64) * NANOS_PER_SECOND),
            "case {:?}: expected the data point to cover the period it was published in, got \
             {:?}",
            row.case,
            read.point
        );

        // The statistics OF that span.
        assert_eq!(
            (read.point.count, read.point.sum),
            (row.count, row.sum),
            "case {:?}: expected the period's count and sum, got {:?}",
            row.case,
            read.point
        );
        assert_quantiles(row, &read.point);
    }
}

/// The destination bucket, the delivery stream and the metric stream of the row at `index`.
fn bucket_of(index: usize) -> String {
    format!("otel-sink-{index}")
}
fn delivery_stream_of(index: usize) -> String {
    format!("otel-delivery-{index}")
}
fn stream_of(index: usize) -> String {
    format!("otel-stream-{index}")
}

/// The identifying pairs a data point carries, which is one of the two things the releases
/// differ in: attributes whose `Dimensions` value is a list of pairs in 1.0, one string label
/// per dimension in 0.7.
fn assert_identity(row: &FormatRow, point: &Point) {
    let dimensions: Vec<(String, String)> = row
        .dimensions
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    if row.output_format == OPEN_TELEMETRY_1_0 {
        assert!(
            point.labels.is_empty(),
            "case {:?}: the 1.0 release carries no string label, got {:?}",
            row.case,
            point.labels
        );
        for (key, expected) in [
            (NAMESPACE_KEY, row.namespace.to_string()),
            (METRIC_NAME_KEY, row.metric_name.to_string()),
        ] {
            assert_eq!(
                attribute(&point.attributes, key),
                Some(expected.clone()),
                "case {:?}: expected the attribute {key} to be {expected:?}, got {:?}",
                row.case,
                point.attributes
            );
        }
        let carried = point
            .attributes
            .iter()
            .find(|attribute| attribute.key == DIMENSIONS_KEY)
            .and_then(|attribute| attribute.value.clone())
            .and_then(|value| value.kvlist_value)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the 1.0 release carries the dimension set as a list of pairs, \
                     got {:?}",
                    row.case, point.attributes
                )
            });
        assert_eq!(
            string_pairs(&carried.values),
            dimensions,
            "case {:?}: expected the metric's dimensions, got {:?}",
            row.case,
            carried.values
        );
    } else {
        assert!(
            point.attributes.is_empty(),
            "case {:?}: the 0.7 release carries no attribute, got {:?}",
            row.case,
            point.attributes
        );
        let mut expected = vec![
            (NAMESPACE_KEY.to_string(), row.namespace.to_string()),
            (METRIC_NAME_KEY.to_string(), row.metric_name.to_string()),
        ];
        expected.extend(dimensions);
        assert_eq!(
            point.labels, expected,
            "case {:?}: expected the metric's identity and dimensions as string labels, got {:?}",
            row.case, point.labels
        );
    }
}

/// The quantiles of the period: exactly the ones the row expects, with the two ends of the
/// distribution at the row's smallest and largest observation, every value inside those ends,
/// and the values rising with the quantile — which is what makes a reported quantile a reading
/// of the distribution rather than a number.
fn assert_quantiles(row: &FormatRow, point: &Point) {
    let carried: Vec<f64> = point
        .quantile_values
        .iter()
        .map(|value| value.quantile)
        .collect();
    assert_eq!(
        carried, row.quantiles,
        "case {:?}: expected the quantiles {:?}, got {:?}",
        row.case, row.quantiles, point.quantile_values
    );
    let value_at = |quantile: f64| {
        point
            .quantile_values
            .iter()
            .find(|value| value.quantile == quantile)
            .map(|value| value.value)
    };
    assert_eq!(
        (value_at(0.0), value_at(1.0)),
        (Some(row.minimum), Some(row.maximum)),
        "case {:?}: expected the ends of the distribution to be its smallest and largest \
         observation, got {:?}",
        row.case,
        point.quantile_values
    );
    for value in &point.quantile_values {
        assert!(
            value.value >= row.minimum && value.value <= row.maximum,
            "case {:?}: the value at quantile {} must lie inside the distribution [{}, {}], got \
             {}",
            row.case,
            value.quantile,
            row.minimum,
            row.maximum,
            value.value
        );
    }
    assert!(
        point
            .quantile_values
            .windows(2)
            .all(|pair| pair[0].value <= pair[1].value),
        "case {:?}: the values must rise with the quantile, got {:?}",
        row.case,
        point.quantile_values
    );
}

/// RULE: the form a record takes is the stream's own `OutputFormat` — two streams fed by the
/// same publish deliver the same metric update in the form each of them names, and a stream
/// naming `json` keeps delivering json documents.
#[tokio::test]
async fn the_output_format_decides_the_form_each_stream_delivers() {
    let namespace = "Custom/OtelForms";
    let formats: [&str; 3] = [JSON, OPEN_TELEMETRY_0_7, OPEN_TELEMETRY_1_0];
    let emulator = Emulator::new();
    for format in formats {
        given_delivery_stream(&emulator, &form_bucket(format), &form_delivery_stream(format)).await;
    }
    for format in formats {
        until_active(&emulator, &form_delivery_stream(format)).await;
        emulator
            .cloudwatch(
                "PutMetricStream",
                &metric_stream_request(
                    &form_stream(format),
                    &form_delivery_stream(format),
                    format,
                    namespace,
                    "Shared",
                    None,
                ),
            )
            .await;
    }
    // ONE publish, selected by all three streams.
    publish_datum(
        &emulator,
        namespace,
        json!({
            "MetricName": "Shared",
            "Value": 42.0,
            "Unit": "Count",
            "Timestamp": stamp(1),
        }),
    )
    .await;

    for format in formats {
        let case = format!("the delivered form of {format}");
        let bucket = form_bucket(format);
        if format == JSON {
            let text = delivered_text(&emulator, &case, &bucket).await;
            let document: Value = serde_json::from_str(text.lines().next().unwrap_or_default())
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: a json record must be a JSON document: {error}")
                });
            assert_eq!(
                document.get("metric_name").and_then(Value::as_str),
                Some("Shared"),
                "case {case:?}: expected the published metric in a json record, got {document}"
            );
            continue;
        }
        let records = delivered_records(&emulator, &case, &bucket, 1).await;
        let read = exported(&case, format, &records[0]);
        assert_eq!(
            read.metric.name.as_str(),
            "Shared",
            "case {case:?}: expected the published metric, got {:?}",
            read.metric
        );
        assert_eq!(
            read.point.sum, 42.0,
            "case {case:?}: expected the published value, got {:?}",
            read.point
        );
    }
}

/// The text of the first object delivered into `bucket`, once one has arrived.
async fn delivered_text(emulator: &Emulator, case: &str, bucket: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        if let Some(key) = emulator.keys(bucket).await.first().cloned() {
            let bytes = emulator.object(bucket, &key).await;
            return String::from_utf8(bytes).unwrap_or_else(|error| {
                panic!("case {case:?}: a json record must be text: {error}")
            });
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object in {bucket}, got none"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The destination bucket, delivery stream and metric stream of one format in the form case.
fn form_bucket(format: &str) -> String {
    format!("form-sink-{}", slug(format))
}
fn form_delivery_stream(format: &str) -> String {
    format!("form-delivery-{}", slug(format))
}
fn form_stream(format: &str) -> String {
    format!("form-stream-{}", slug(format))
}

/// A format value as a name a bucket and a stream may carry: the dot of a release number is not
/// one of the characters a metric stream's name admits ("Valid characters are A-Z, a-z, 0-9,
/// \"-\" and \"_\"", `PutMetricStreamInput$Name`).
fn slug(format: &str) -> String {
    format.replace('.', "-")
}
