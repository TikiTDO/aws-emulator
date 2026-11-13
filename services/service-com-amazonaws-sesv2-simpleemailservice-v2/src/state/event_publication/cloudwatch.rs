// SPDX-License-Identifier: CC0-1.0

//! The Amazon CloudWatch destination of an event destination: the datapoint ONE email
//! event of a send becomes, and the dimensions the destination DECLARED for it.
//!
//! A `CloudWatchDestination` is "an object that defines an Amazon CloudWatch destination
//! for email events", carrying the `DimensionConfigurations` that "define the dimensions
//! to use when you send email events to Amazon CloudWatch". So the RULE this module
//! implements, for a destination that receives an event ([`super::notification`] decides
//! which do): the event is counted as one observation of the metric named after it in the
//! [`METRIC_NAMESPACE`] namespace, dimensioned by EXACTLY the dimensions the destination
//! declared — each one resolved from the send by its own
//! `CloudWatchDimensionConfiguration$DimensionValueSource` ("the location where the Amazon
//! SES API v2 finds the value of a dimension to publish to Amazon CloudWatch"), and
//! falling back to its `DefaultDimensionValue` ("the default value of the dimension that
//! is published to Amazon CloudWatch if you don't provide the value of the dimension when
//! you send an email") where the send supplies none.
//!
//! Only a DECLARED dimension is published. A dimension a destination's configuration does
//! not name is not part of the series that destination publishes, not even carrying its
//! default — so the message tag SES applies to every send through a configuration set is a
//! published dimension only where the destination declared a dimension for it, and a
//! reader asking for a dimension combination the destination never declared finds no
//! series at all rather than one filled in with defaults.
//!
//! The datapoint is appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read,
//! through CloudWatch's own bounded append, so the metric one service publishes and the
//! reader of it cannot disagree.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The CloudWatch store an email event's datapoint lives in.
///
/// It is the CloudWatch service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so what a send publishes is what `ListMetrics` and
/// `GetMetricStatistics` report.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine, or `None` when the engine carries no
/// such store — a deployment with no CloudWatch to publish to, where a send's events
/// still reach every other destination kind.
pub fn metric_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    cloudwatch::service_state(engine)
}

/// The namespace Amazon SES publishes a configuration set's email-sending events to.
pub const METRIC_NAMESPACE: &str = "AWS/SES";

/// `DimensionValueSource`'s `MESSAGE_TAG`: "to use the message tags that you specify using
/// an `X-SES-MESSAGE-TAGS` header or a parameter to the `SendEmail` or `SendRawEmail` API"
/// (see `crate::ops::types::DIMENSION_VALUE_SOURCE_VALUES`).
const MESSAGE_TAG_SOURCE: &str = "MESSAGE_TAG";

/// `DimensionValueSource`'s `EMAIL_HEADER`: "to use your own email headers".
const EMAIL_HEADER_SOURCE: &str = "EMAIL_HEADER";

/// The unit one email event's observation carries: an event is one email counted once, and
/// `Count` is the unit CloudWatch declares for a count of things (`StandardUnit`).
const EVENT_UNIT: &str = "Count";

/// How many observations one event is: the event happened once.
const ONE_EVENT: f64 = 1.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The dimension set `destination` publishes an event of this send under: one dimension per
/// `CloudWatchDimensionConfiguration` the destination declared, in each case the value the
/// send supplies for it or the configuration's own default.
///
/// `message_tags` are the tags of the send, in the order they apply, and `headers` the
/// headers of the message; both are the send's own ([`super::notification::PublishedSend`]).
/// A dimension name declared more than once is one dimension, decided by its first
/// configuration, because a CloudWatch dimension set carries one value per name.
pub fn dimension_set(
    destination: &crate::ops::types::CloudWatchDestination,
    message_tags: &[(String, String)],
    headers: &[(String, String)],
) -> cloudwatch::DimensionSet {
    let mut dimensions = cloudwatch::DimensionSet::new();
    for configuration in &destination.dimension_configurations {
        let name = configuration.dimension_name.as_str();
        let supplied = match configuration.dimension_value_source.as_str() {
            // A message tag's name is matched exactly: `MessageTag$Name` is the name the
            // sender chose and nothing normalises its case.
            MESSAGE_TAG_SOURCE => message_tags
                .iter()
                .find(|(tag, _)| tag == name)
                .map(|(_, value)| value.clone()),
            // A header's name is matched without regard to case, the way a mail header
            // name compares (RFC 5322 field names are case-insensitive).
            EMAIL_HEADER_SOURCE => headers
                .iter()
                .find(|(header, _)| header.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone()),
            // `LINK_TAG` resolves a dimension from the tags on a LINK a recipient
            // followed, which belongs to an engagement event this emulator's sends do not
            // produce: the send supplies no value for such a dimension, so it carries the
            // default the configuration declares for exactly that case.
            _ => None,
        };
        let value = supplied
            .unwrap_or_else(|| configuration.default_dimension_value.clone());
        dimensions.entry(name.to_string()).or_insert(value);
    }
    dimensions
}

/// Count ONE event of a send: append a single observation of `metric_name` to the series
/// `dimensions` identifies, stamped at `at`.
///
/// A store at capacity takes nothing, which is logged and changes nothing about the send:
/// the message was accepted before any event of it existed.
pub fn count_event(
    store: &::engine::ServiceState<CloudWatchState>,
    metric_name: &str,
    dimensions: cloudwatch::DimensionSet,
    at: ::std::time::SystemTime,
) {
    let key = cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions,
    };
    let datum = cloudwatch::StoredDatum {
        timestamp_secs: epoch_secs(at),
        sample_count: ONE_EVENT,
        sum: ONE_EVENT,
        minimum: ONE_EVENT,
        maximum: ONE_EVENT,
        unit: Some(EVENT_UNIT.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    };
    if store.lock().append_datapoints(key, vec![datum]).is_err() {
        ::tracing::warn!(
            metric_name,
            "the CloudWatch store is at capacity; an email event's datapoint could not be \
             published"
        );
    }
}

/// `instant` as the epoch seconds a stored datapoint is stamped with — the same clock
/// CloudWatch's own publishers read ([`cloudwatch::now_secs`]).
fn epoch_secs(instant: ::std::time::SystemTime) -> f64 {
    instant
        .duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}
