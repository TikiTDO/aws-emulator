// SPDX-License-Identifier: CC0-1.0

//! The records a running metric stream exports in one of the OpenTelemetry output formats.
//!
//! "The output format for the stream. Valid values are `json`, `opentelemetry1.0`, and
//! `opentelemetry0.7`." (`PutMetricStreamInput$OutputFormat`) — so the format a stream carries
//! decides the FORM of the records [`super`] pushes into the delivery stream, and the two
//! OpenTelemetry values ask for the OpenTelemetry protocol's own metrics message rather than for
//! the json documents [`super::record`] renders. This module renders that message.
//!
//! What one record is:
//!
//! * an `ExportMetricsServiceRequest` of the OpenTelemetry metrics service, carrying one
//!   resource, one scope and one metric — the metric this record reports, for one period;
//! * the metric's measurements as a `Summary`, the OpenTelemetry data type whose data point
//!   carries exactly what a streamed metric reports: a `count`, a `sum` and quantile values.
//!   "a metric stream always sends the `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT` statistics for
//!   each metric that is streamed" (`PutMetricStream`), so the count and the sum are those two
//!   statistics and the minimum and the maximum are the quantiles at either end of the
//!   distribution ([`MIN_QUANTILE`], [`MAX_QUANTILE`]);
//! * each additional statistic as one more quantile value, which is why the member these
//!   formats accept carries percentiles only: "If the `OutputFormat` for the stream is
//!   `opentelemetry1.0` or `opentelemetry0.7`, the only valid values are `p??` percentile
//!   statistics such as `p90`, `p99` and so on."
//!   (`MetricStreamStatisticsConfiguration$AdditionalStatistics`) — a percentile IS a quantile,
//!   in percent;
//! * serialized as protobuf and framed with its own length, so a consumer reading the bytes a
//!   delivery wrote reads one message after another however many of them one buffer was flushed
//!   with ([`record`]).
//!
//! The messages are declared here, as the subset of the OpenTelemetry protocol one exported
//! record needs, and the maintained protobuf codec encodes them. The two format values are two
//! RELEASES of that protocol, and they differ inside the data point: the release the
//! `opentelemetry0.7` value names carries a data point's identifying pairs as string labels and
//! the count of its population as a variable-width integer, while the one `opentelemetry1.0`
//! names carries the pairs as attributes, whose value may itself be a list of pairs, and the
//! count as a fixed-width one ([`SummaryDataPoint`], [`DoubleSummaryDataPoint`]). Every message
//! that ENCLOSES the data point is identical in both, so those are declared once.

use ::prost::Message;

use crate::state::{BasicStatistics, DimensionSet, MetricKey};

/// The `MetricStreamOutputFormat` value naming the 1.0 release of the OpenTelemetry protocol.
pub const OPEN_TELEMETRY_1_0: &str = "opentelemetry1.0";

/// The `MetricStreamOutputFormat` value naming its 0.7 release.
pub const OPEN_TELEMETRY_0_7: &str = "opentelemetry0.7";

/// The quantile the smallest observation of a period sits at, and the one the largest sits at:
/// the two ends of the distribution the period's datapoints make up.
const MIN_QUANTILE: f64 = 0.0;
const MAX_QUANTILE: f64 = 1.0;

/// How many percent one whole quantile is, which is what turns a `pNN` percentile statistic into
/// the quantile a `Summary` reports it at.
const PERCENT_PER_QUANTILE: f64 = 100.0;

/// How many nanoseconds one second is: the unit every instant of an OpenTelemetry data point is
/// stated in.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// The resource attributes every exported record carries: which cloud the metric came from, the
/// account and Region it was published in, and the metric stream that exported it.
const CLOUD_PROVIDER_KEY: &str = "cloud.provider";
const CLOUD_PROVIDER: &str = "aws";
const CLOUD_ACCOUNT_KEY: &str = "cloud.account.id";
const CLOUD_REGION_KEY: &str = "cloud.region";
const EXPORTER_ARN_KEY: &str = "aws.exporter.arn";

/// The pairs that identify WHICH metric a data point reports: the two halves of a metric's name
/// ("a metric is uniquely defined by a name, a namespace, and zero or more dimensions") and its
/// dimension set.
const NAMESPACE_KEY: &str = "Namespace";
const METRIC_NAME_KEY: &str = "MetricName";
const DIMENSIONS_KEY: &str = "Dimensions";

/// Which release of the OpenTelemetry protocol a stream's records are framed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTelemetryVersion {
    /// The release [`OPEN_TELEMETRY_1_0`] names.
    V1_0,
    /// The release [`OPEN_TELEMETRY_0_7`] names.
    V0_7,
}

impl OpenTelemetryVersion {
    /// The release an `OutputFormat` value names, or `None` for a format that is not one of the
    /// OpenTelemetry values (`json`, whose records [`super::record`] renders).
    #[must_use]
    pub fn named(format: &str) -> Option<Self> {
        match format {
            OPEN_TELEMETRY_1_0 => Some(Self::V1_0),
            OPEN_TELEMETRY_0_7 => Some(Self::V0_7),
            _ => None,
        }
    }
}

/// One period of one metric, as one exported record reports it.
#[derive(Debug, Clone)]
pub struct ExportedPeriod<'a> {
    /// Which release the record is framed in.
    pub version: OpenTelemetryVersion,
    /// The ARN of the metric stream exporting the record.
    pub exporter_arn: &'a str,
    /// The Region the metric was published in.
    pub region: &'a str,
    /// The metric the record reports.
    pub key: &'a MetricKey,
    /// The instant the period opens at, as epoch seconds.
    pub start_secs: f64,
    /// The instant it closes at, as epoch seconds.
    pub end_secs: f64,
    /// The unit the reported datapoints were published under.
    pub unit: &'a str,
    /// The statistics every streamed metric carries, folded over the period's datapoints.
    pub statistics: BasicStatistics,
    /// The additional statistics the stream asks for, as `(percentile, value)` pairs with the
    /// percentile in percent, as a `pNN` statistic names it.
    pub percentiles: &'a [(f64, f64)],
}

/// ONE exported record: the period's `ExportMetricsServiceRequest`, protobuf-encoded and
/// prefixed with its own length, so the records of a flushed buffer are read back one after
/// another from the bytes the delivery wrote.
#[must_use]
pub fn record(period: &ExportedPeriod<'_>) -> Vec<u8> {
    request(period).encode_length_delimited_to_vec()
}

/// The request one record carries: one resource, one scope, one metric.
fn request(period: &ExportedPeriod<'_>) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: resource_attributes(period),
            }),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![metric(period)],
            }],
        }],
    }
}

/// Where the record came from: the cloud, the account and Region of the publish, and the metric
/// stream that exported it.
fn resource_attributes(period: &ExportedPeriod<'_>) -> Vec<KeyValue> {
    [
        (CLOUD_PROVIDER_KEY, CLOUD_PROVIDER.to_string()),
        (
            CLOUD_ACCOUNT_KEY,
            ::emulator_server::emulator_account_id().to_string(),
        ),
        (CLOUD_REGION_KEY, period.region.to_string()),
        (EXPORTER_ARN_KEY, period.exporter_arn.to_string()),
    ]
    .into_iter()
    .map(|(key, value)| string_attribute(key, value))
    .collect()
}

/// The metric itself: its name, the unit its measurements are expressed in, and the summary of
/// the period.
fn metric(period: &ExportedPeriod<'_>) -> Metric {
    Metric {
        name: period.key.metric_name.clone(),
        unit: period.unit.to_string(),
        summary: Some(Summary {
            data_points: vec![data_point(period)],
        }),
    }
}

/// The period's data point, encoded: which metric it reports, the span it covers and the
/// statistics over it.
///
/// The data point is where the two releases differ, so each is declared and encoded on its own
/// ([`SummaryDataPoint`], [`DoubleSummaryDataPoint`]) and the result is embedded in the summary
/// that holds it ([`Summary::data_points`]).
fn data_point(period: &ExportedPeriod<'_>) -> Vec<u8> {
    let start_time_unix_nano = unix_nanos(period.start_secs);
    let time_unix_nano = unix_nanos(period.end_secs);
    let count = observation_count(period.statistics.sample_count);
    let sum = reported(period.statistics.sum);
    let quantile_values = quantile_values(period);
    match period.version {
        OpenTelemetryVersion::V1_0 => SummaryDataPoint {
            start_time_unix_nano,
            time_unix_nano,
            count,
            sum,
            quantile_values,
            attributes: data_point_attributes(period.key),
        }
        .encode_to_vec(),
        OpenTelemetryVersion::V0_7 => DoubleSummaryDataPoint {
            labels: data_point_labels(period.key),
            start_time_unix_nano,
            time_unix_nano,
            count,
            sum,
            quantile_values,
        }
        .encode_to_vec(),
    }
}

/// The identifying pairs as the 1.0 release carries them: the namespace, the metric name, and
/// the dimension set as a list of pairs of its own, which is a shape an attribute's value admits
/// and a string label does not.
fn data_point_attributes(key: &MetricKey) -> Vec<KeyValue> {
    vec![
        string_attribute(NAMESPACE_KEY, key.namespace.clone()),
        string_attribute(METRIC_NAME_KEY, key.metric_name.clone()),
        KeyValue {
            key: DIMENSIONS_KEY.to_string(),
            value: Some(AnyValue {
                string_value: None,
                kvlist_value: Some(KeyValueList {
                    values: dimension_attributes(&key.dimensions),
                }),
            }),
        },
    ]
}

/// One dimension of the set, as one attribute of the list the `Dimensions` attribute carries.
fn dimension_attributes(dimensions: &DimensionSet) -> Vec<KeyValue> {
    dimensions
        .iter()
        .map(|(name, value)| string_attribute(name, value.clone()))
        .collect()
}

/// The identifying pairs as the 0.7 release carries them: a label's value is a string, so the
/// dimension set is carried as one label per dimension beside the namespace and the metric name.
fn data_point_labels(key: &MetricKey) -> Vec<StringKeyValue> {
    let mut labels = vec![
        StringKeyValue {
            key: NAMESPACE_KEY.to_string(),
            value: key.namespace.clone(),
        },
        StringKeyValue {
            key: METRIC_NAME_KEY.to_string(),
            value: key.metric_name.clone(),
        },
    ];
    for (name, value) in &key.dimensions {
        labels.push(StringKeyValue {
            key: name.clone(),
            value: value.clone(),
        });
    }
    labels
}

/// The quantiles of the period, ascending: the ends of its distribution, and one for each
/// additional percentile statistic the stream asks for.
///
/// A statistic the period does not define is not reported as anything — a percentile over
/// datapoints whose individual values were never published is a value this store cannot compute
/// rather than one it may invent — and neither is a quantile outside the distribution.
fn quantile_values(period: &ExportedPeriod<'_>) -> Vec<ValueAtQuantile> {
    let mut values: Vec<ValueAtQuantile> = Vec::new();
    for (quantile, value) in [
        (MIN_QUANTILE, period.statistics.minimum),
        (MAX_QUANTILE, period.statistics.maximum),
    ] {
        if value.is_finite() {
            values.push(ValueAtQuantile { quantile, value });
        }
    }
    for (percentile, value) in period.percentiles {
        let quantile = percentile / PERCENT_PER_QUANTILE;
        if value.is_finite() && (MIN_QUANTILE..=MAX_QUANTILE).contains(&quantile) {
            values.push(ValueAtQuantile {
                quantile,
                value: *value,
            });
        }
    }
    values.sort_by(|left, right| left.quantile.total_cmp(&right.quantile));
    // One value per quantile: a stream asking for the percentile at either end of the
    // distribution asks for a quantile the data point already reports, and a quantile reported
    // twice is not a reading a consumer can index by.
    values.dedup_by(|later, held| later.quantile.total_cmp(&held.quantile).is_eq());
    values
}

/// One attribute whose value is a string.
fn string_attribute(key: &str, value: String) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            string_value: Some(value),
            kvlist_value: None,
        }),
    }
}

/// How many observations the period stands for, as the whole count a data point carries: a
/// fractional sample count is the count of the observations a statistic set stood for, rounded
/// to the observations it reports, and a count no whole number can hold is reported as the
/// largest one that can.
fn observation_count(sample_count: f64) -> u64 {
    if sample_count.is_finite() && sample_count > 0.0 {
        sample_count.round() as u64
    } else {
        0
    }
}

/// A statistic as the data point reports it: a fold that is no measurement at all (no
/// observation in the period) is reported as zero, since the member is not an optional one and a
/// non-finite double is a number no consumer could read as a measurement. A period with no
/// observation owes no record, so nothing reaches this.
fn reported(statistic: f64) -> f64 {
    if statistic.is_finite() {
        statistic
    } else {
        0.0
    }
}

/// An epoch instant in the nanoseconds an OpenTelemetry data point states it in.
///
/// Taken from the WHOLE seconds of the instant, which is what a period boundary is: the
/// nanosecond count of a present-day instant is past the integers a double represents exactly,
/// so multiplying the double itself would move the boundary.
fn unix_nanos(seconds: f64) -> u64 {
    let whole = seconds.trunc();
    if whole <= 0.0 {
        return 0;
    }
    (whole as u64).saturating_mul(NANOS_PER_SECOND)
}

/// `opentelemetry.proto.collector.metrics.v1.ExportMetricsServiceRequest`: what one record is.
#[derive(Clone, PartialEq, Message)]
struct ExportMetricsServiceRequest {
    #[prost(message, repeated, tag = "1")]
    resource_metrics: Vec<ResourceMetrics>,
}

/// `opentelemetry.proto.metrics.v1.ResourceMetrics`: the metrics of one resource.
///
/// Its second member is the scope of the 1.0 release and the instrumentation library of the 0.7
/// one; both carry their metrics at the same field number, and an exported record names neither
/// a scope nor a library, so one declaration serves both releases.
#[derive(Clone, PartialEq, Message)]
struct ResourceMetrics {
    #[prost(message, optional, tag = "1")]
    resource: Option<Resource>,
    #[prost(message, repeated, tag = "2")]
    scope_metrics: Vec<ScopeMetrics>,
}

/// `opentelemetry.proto.resource.v1.Resource`: what the metrics were collected from.
#[derive(Clone, PartialEq, Message)]
struct Resource {
    #[prost(message, repeated, tag = "1")]
    attributes: Vec<KeyValue>,
}

/// `opentelemetry.proto.metrics.v1.ScopeMetrics`: the metrics of one scope.
#[derive(Clone, PartialEq, Message)]
struct ScopeMetrics {
    #[prost(message, repeated, tag = "2")]
    metrics: Vec<Metric>,
}

/// `opentelemetry.proto.metrics.v1.Metric`: one metric, whose data is a summary.
#[derive(Clone, PartialEq, Message)]
struct Metric {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(string, tag = "3")]
    unit: String,
    #[prost(message, optional, tag = "11")]
    summary: Option<Summary>,
}

/// `opentelemetry.proto.metrics.v1.Summary`: the summary data points of one metric.
///
/// A data point is held as the bytes its own release encodes to ([`data_point`]), because the
/// two releases differ INSIDE the data point: a length-delimited member carries the same bytes
/// on the wire whether it is declared as a message or as that message's encoding, so the
/// enclosing messages — identical in both releases — are declared once here.
#[derive(Clone, PartialEq, Message)]
struct Summary {
    #[prost(bytes = "vec", repeated, tag = "1")]
    data_points: Vec<Vec<u8>>,
}

/// `opentelemetry.proto.metrics.v1.SummaryDataPoint` as the 1.0 release declares it: the
/// identifying pairs are attributes, whose value may itself be a list of pairs, and the count of
/// the population is a fixed-width integer.
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

/// `opentelemetry.proto.metrics.v1.DoubleSummaryDataPoint` as the 0.7 release declares it: the
/// identifying pairs are string labels at the field number the 1.0 release has since retired,
/// and the count of the population is a variable-width integer.
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

/// `opentelemetry.proto.metrics.v1.SummaryDataPoint.ValueAtQuantile`: the value the distribution
/// takes at one quantile.
#[derive(Clone, PartialEq, Message)]
struct ValueAtQuantile {
    #[prost(double, tag = "1")]
    quantile: f64,
    #[prost(double, tag = "2")]
    value: f64,
}

/// `opentelemetry.proto.common.v1.KeyValue`: one attribute.
#[derive(Clone, PartialEq, Message)]
struct KeyValue {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(message, optional, tag = "2")]
    value: Option<AnyValue>,
}

/// `opentelemetry.proto.common.v1.AnyValue`: an attribute's value, of which an exported record
/// uses the two forms it carries — a string, and a list of pairs for the dimension set.
///
/// The declaration carries them as two members at the field numbers the protocol's own union
/// gives them: a message setting exactly one of them is the same bytes as that union, which is
/// what a consumer decodes.
#[derive(Clone, PartialEq, Message)]
struct AnyValue {
    #[prost(string, optional, tag = "1")]
    string_value: Option<String>,
    #[prost(message, optional, tag = "6")]
    kvlist_value: Option<KeyValueList>,
}

/// `opentelemetry.proto.common.v1.KeyValueList`: a list of attributes, as one attribute's value.
#[derive(Clone, PartialEq, Message)]
struct KeyValueList {
    #[prost(message, repeated, tag = "1")]
    values: Vec<KeyValue>,
}

/// `opentelemetry.proto.common.v1.StringKeyValue`: one label of the 0.7 release, whose value is
/// a string.
#[derive(Clone, PartialEq, Message)]
struct StringKeyValue {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The period every row below renders, with `version` and `percentiles` as the row asks.
    fn period<'a>(
        version: OpenTelemetryVersion,
        key: &'a MetricKey,
        percentiles: &'a [(f64, f64)],
    ) -> ExportedPeriod<'a> {
        ExportedPeriod {
            version,
            exporter_arn: "arn:aws:cloudwatch:us-east-1:000000000000:metric-stream/exports",
            region: "us-east-1",
            key,
            start_secs: 1_700_000_040.0,
            end_secs: 1_700_000_100.0,
            unit: "Milliseconds",
            statistics: BasicStatistics {
                sample_count: 4.0,
                sum: 100.0,
                minimum: 10.0,
                maximum: 40.0,
            },
            percentiles,
        }
    }

    /// A metric key with one dimension.
    fn key() -> MetricKey {
        MetricKey {
            namespace: "Custom/Export".to_string(),
            metric_name: "Latency".to_string(),
            dimensions: [("Service".to_string(), "checkout".to_string())]
                .into_iter()
                .collect(),
        }
    }

    /// The one message a record carries, decoded from the record's own framing.
    fn decoded(bytes: &[u8]) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest::decode_length_delimited(bytes)
            .expect("a record must decode as the message it is framed as")
    }

    /// The encoded data point of a decoded record.
    fn only_data_point(request: &ExportMetricsServiceRequest) -> Vec<u8> {
        let resource = request
            .resource_metrics
            .first()
            .expect("one resource per record");
        let scope = resource
            .scope_metrics
            .first()
            .expect("one scope per record");
        let metric = scope.metrics.first().expect("one metric per record");
        metric
            .summary
            .as_ref()
            .expect("a streamed metric is summarised")
            .data_points
            .first()
            .expect("one data point per record")
            .clone()
    }

    /// The data point of a record of the 1.0 release.
    fn latest_point(bytes: &[u8]) -> SummaryDataPoint {
        SummaryDataPoint::decode(only_data_point(&decoded(bytes)).as_slice())
            .expect("the data point of the 1.0 release")
    }

    /// The data point of a record of the 0.7 release.
    fn earlier_point(bytes: &[u8]) -> DoubleSummaryDataPoint {
        DoubleSummaryDataPoint::decode(only_data_point(&decoded(bytes)).as_slice())
            .expect("the data point of the 0.7 release")
    }

    /// The span, count, sum and quantiles a data point of either release reports, read through
    /// each release's own declaration.
    fn measurements(
        version: OpenTelemetryVersion,
        bytes: &[u8],
    ) -> (u64, u64, u64, f64, Vec<(f64, f64)>) {
        let quantiles = |values: &[ValueAtQuantile]| {
            values
                .iter()
                .map(|value| (value.quantile, value.value))
                .collect()
        };
        match version {
            OpenTelemetryVersion::V1_0 => {
                let point = latest_point(bytes);
                (
                    point.start_time_unix_nano,
                    point.time_unix_nano,
                    point.count,
                    point.sum,
                    quantiles(&point.quantile_values),
                )
            }
            OpenTelemetryVersion::V0_7 => {
                let point = earlier_point(bytes);
                (
                    point.start_time_unix_nano,
                    point.time_unix_nano,
                    point.count,
                    point.sum,
                    quantiles(&point.quantile_values),
                )
            }
        }
    }

    /// RULE: a record is one length-delimited message reporting the period's count, sum and the
    /// quantiles of its distribution, whichever release it is framed in.
    #[test]
    fn a_record_decodes_as_the_period_it_reports() {
        for version in [OpenTelemetryVersion::V1_0, OpenTelemetryVersion::V0_7] {
            let key = key();
            let percentiles = [(99.0, 40.0)];
            let bytes = record(&period(version, &key, &percentiles));
            let (start, end, count, sum, quantiles) = measurements(version, &bytes);
            assert_eq!(
                (count, sum),
                (4, 100.0),
                "case {version:?}: expected the period's count and sum, got {count} and {sum}"
            );
            assert_eq!(
                (start, end),
                (
                    1_700_000_040 * NANOS_PER_SECOND,
                    1_700_000_100 * NANOS_PER_SECOND
                ),
                "case {version:?}: expected the instants the period opens and closes at, got \
                 {start} and {end}"
            );
            assert_eq!(
                quantiles,
                vec![(0.0, 10.0), (0.99, 40.0), (1.0, 40.0)],
                "case {version:?}: expected the ends of the distribution and the asked \
                 percentile, got {quantiles:?}"
            );
        }
    }

    /// RULE: one value per quantile — a stream asking for the percentile at either end of the
    /// distribution asks for a quantile the data point already reports.
    #[test]
    fn a_quantile_is_reported_once() {
        let key = key();
        // p0 and p100 are the ends of the distribution, and p99 is one of its own.
        let percentiles = [(0.0, 10.0), (100.0, 40.0), (99.0, 40.0)];
        let bytes = record(&period(OpenTelemetryVersion::V1_0, &key, &percentiles));
        let point = latest_point(&bytes);
        let quantiles: Vec<f64> = point
            .quantile_values
            .iter()
            .map(|value| value.quantile)
            .collect();
        assert_eq!(
            quantiles,
            vec![0.0, 0.99, 1.0],
            "each quantile must be reported once, got {:?}",
            point.quantile_values
        );
    }

    /// RULE: the release decides how the identifying pairs are carried — attributes, whose value
    /// may be a list of pairs, in 1.0; one string label per pair in 0.7.
    #[test]
    fn the_release_decides_how_the_identifying_pairs_are_carried() {
        let key = key();
        let latest = latest_point(&record(&period(OpenTelemetryVersion::V1_0, &key, &[])));
        let attribute_keys: Vec<String> = latest
            .attributes
            .iter()
            .map(|attribute| attribute.key.clone())
            .collect();
        assert_eq!(
            attribute_keys,
            vec![
                NAMESPACE_KEY.to_string(),
                METRIC_NAME_KEY.to_string(),
                DIMENSIONS_KEY.to_string(),
            ],
            "the 1.0 release carries the metric's identity as attributes, got \
             {attribute_keys:?}"
        );
        let dimensions = latest
            .attributes
            .iter()
            .find(|attribute| attribute.key == DIMENSIONS_KEY)
            .and_then(|attribute| attribute.value.clone())
            .and_then(|value| value.kvlist_value)
            .expect("the dimension set is carried as a list of pairs");
        assert_eq!(
            dimensions
                .values
                .iter()
                .map(|pair| (
                    pair.key.clone(),
                    pair.value
                        .clone()
                        .and_then(|value| value.string_value)
                        .unwrap_or_default()
                ))
                .collect::<Vec<(String, String)>>(),
            vec![("Service".to_string(), "checkout".to_string())],
            "the dimension list carries the metric's dimensions, got {dimensions:?}"
        );
        let earlier = earlier_point(&record(&period(OpenTelemetryVersion::V0_7, &key, &[])));
        assert_eq!(
            earlier
                .labels
                .iter()
                .map(|label| (label.key.clone(), label.value.clone()))
                .collect::<Vec<(String, String)>>(),
            vec![
                (NAMESPACE_KEY.to_string(), "Custom/Export".to_string()),
                (METRIC_NAME_KEY.to_string(), "Latency".to_string()),
                ("Service".to_string(), "checkout".to_string()),
            ],
            "the 0.7 release carries the metric's identity and dimensions as string labels, got \
             {:?}",
            earlier.labels
        );
    }

    /// RULE: a record of the 1.0 release is that release's own message — so it decodes with the
    /// protocol's PUBLISHED message definitions, not only with the declarations above, and every
    /// value it reports survives that decode.
    ///
    /// The definitions come from the OpenTelemetry protocol's own crate (a development
    /// dependency), so a record whose field numbers, wire types or nesting differ from the
    /// published protocol fails here instead of being read back by the same declarations that
    /// wrote it.
    #[test]
    fn a_record_of_the_latest_release_decodes_with_the_published_definitions() {
        use ::opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest as Published;
        use ::opentelemetry_proto::tonic::metrics::v1::metric::Data;

        let key = key();
        let percentiles = [(99.0, 40.0)];
        let bytes = record(&period(OpenTelemetryVersion::V1_0, &key, &percentiles));
        let request = Published::decode_length_delimited(bytes.as_slice())
            .expect("a 1.0 record must decode as the published message");
        let resource = request
            .resource_metrics
            .first()
            .expect("one resource per record");
        let attributes: Vec<String> = resource
            .resource
            .as_ref()
            .expect("the resource the metric was collected from")
            .attributes
            .iter()
            .map(|attribute| attribute.key.clone())
            .collect();
        assert!(
            attributes.contains(&EXPORTER_ARN_KEY.to_string())
                && attributes.contains(&CLOUD_ACCOUNT_KEY.to_string()),
            "the published decode must carry the exporter and the account, got {attributes:?}"
        );
        let metric = resource
            .scope_metrics
            .first()
            .expect("one scope per record")
            .metrics
            .first()
            .expect("one metric per record");
        assert_eq!(
            (metric.name.as_str(), metric.unit.as_str()),
            ("Latency", "Milliseconds"),
            "the published decode must carry the metric's name and unit, got {metric:?}"
        );
        let Some(Data::Summary(summary)) = metric.data.as_ref() else {
            panic!("a streamed metric must be a summary, got {:?}", metric.data);
        };
        let point = summary
            .data_points
            .first()
            .expect("one data point per record");
        assert_eq!(
            (point.count, point.sum),
            (4, 100.0),
            "the published decode must carry the period's count and sum, got {point:?}"
        );
        assert_eq!(
            point
                .quantile_values
                .iter()
                .map(|value| (value.quantile, value.value))
                .collect::<Vec<(f64, f64)>>(),
            vec![(0.0, 10.0), (0.99, 40.0), (1.0, 40.0)],
            "the published decode must carry the quantiles, got {point:?}"
        );
    }

    /// RULE: only the OpenTelemetry output format values name a release; `json` does not.
    #[test]
    fn only_the_open_telemetry_formats_name_a_release() {
        let cases: [(&str, Option<OpenTelemetryVersion>); 4] = [
            (OPEN_TELEMETRY_1_0, Some(OpenTelemetryVersion::V1_0)),
            (OPEN_TELEMETRY_0_7, Some(OpenTelemetryVersion::V0_7)),
            ("json", None),
            ("OpenTelemetry1.0", None),
        ];
        for (format, expected) in cases {
            let named = OpenTelemetryVersion::named(format);
            assert_eq!(
                named, expected,
                "case {format:?}: expected {expected:?}, got {named:?}"
            );
        }
    }
}
