// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series the EDGE publishes about the viewer requests it serves.
//!
//! CloudFront reports what its edge did with viewer traffic as the per-distribution metrics
//! of the [`NAMESPACE`] namespace, dimensioned by the distribution the traffic was addressed
//! to ([`DISTRIBUTION_ID_DIMENSION`]) beside the Region the metrics are reported for, which
//! for a distribution is always [`GLOBAL_REGION`] — CloudFront is not a regional service, so
//! its per-distribution series carry that one value. Those are the series a client watches a
//! distribution with: an alarm over [`FOUR_XX_ERROR_RATE_METRIC`] is how a client is told its
//! viewers are getting errors, so an edge that publishes nothing leaves every such alarm
//! without data and an alarm that can never fire cannot be told from one correctly staying
//! `OK`.
//!
//! EVERY answer the edge serves for a distribution is counted — the ones an origin produced,
//! the ones the edge cache held and the ones CloudFront generated itself — because each of
//! them is a viewer request CloudFront received and answered. A request that resolves to no
//! distribution of this account is counted into nothing: these series are per-distribution,
//! and there is no distribution to charge it to.
//!
//! What each series carries follows the metric's own definition:
//!
//! * [`REQUESTS_METRIC`] — "the total number of viewer requests received by CloudFront, for
//!   all HTTP methods and for both HTTP and HTTPS requests": one observation of 1 per request,
//!   so the `Sum` over a period is how many requests that period had;
//! * [`BYTES_DOWNLOADED_METRIC`] — the bytes viewers downloaded, for the methods that read an
//!   object ([`DOWNLOADING_METHODS`]): the length of the body the edge served;
//! * [`BYTES_UPLOADED_METRIC`] — the bytes uploaded to the origin, for the methods that write
//!   one ([`UPLOADING_METHODS`]): the length of the body the viewer sent;
//! * [`FOUR_XX_ERROR_RATE_METRIC`], [`FIVE_XX_ERROR_RATE_METRIC`] and
//!   [`TOTAL_ERROR_RATE_METRIC`] — the PERCENTAGE of the period's viewer requests whose answer
//!   carried a status of that class. A percentage is published per request as
//!   [`WHOLE_PERCENT`] when that request's own status is of the class and 0 when it is not, so
//!   the `Average` over any period is exactly the share of that period's requests that
//!   answered in it — which is what the metric is, and is why it is published with
//!   [`PERCENT_UNIT`].
//!
//! Beside those DEFAULT series, a distribution has a second set a client OPTS IN to: the
//! additional metrics a monitoring subscription buys
//! (`RealtimeMetricsSubscriptionConfig$RealtimeMetricsSubscriptionStatus` — "a flag that
//! indicates whether additional CloudWatch metrics are enabled for a given CloudFront
//! distribution"). They are published for exactly the same traffic, in the same namespace and
//! under the same dimensions, and ONLY while that flag is `Enabled`
//! ([`ADDITIONAL_DISTRIBUTION_METRICS`], [`AdditionalMetrics`]): a distribution carrying no
//! subscription, or one whose subscription is `Disabled`, publishes none of them — so such a
//! series has no datapoint in any minute, while the DEFAULT series the same traffic fills is
//! what tells a client the traffic was served at all. What each of them carries follows its
//! own definition:
//!
//! * [`CACHE_HIT_RATE_METRIC`] — "the percentage of all cacheable requests for which
//!   CloudFront served the content from its cache", where "HTTP POST and PUT requests, and
//!   errors, are not considered cacheable requests": one observation of [`WHOLE_PERCENT`] for
//!   a cacheable request the edge cache answered and 0 for one the origin answered, and NO
//!   observation at all for a request that is not cacheable — so the `Average` over a period
//!   is the share of that period's cacheable requests served from the cache, and a period of
//!   nothing but uploads and errors has no hit rate rather than a rate of zero;
//! * [`ORIGIN_LATENCY_METRIC`] — "the total time spent from when CloudFront receives a request
//!   to when it starts providing a response to the network (not the viewer) for requests that
//!   are served from the origin, not the CloudFront cache": the measured span of serving one
//!   such request, in [`MILLISECONDS_UNIT`], published only for a request an origin answered;
//! * [`STATUS_ERROR_RATE_METRICS`] — one series per status code CloudFront reports a rate for,
//!   each "the percentage of all viewer requests for which the response's HTTP status code is"
//!   that code: published per request exactly as the class rates above are, so the `Average`
//!   over a period is the share of its requests answered with that one status.
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same store
//! `ListMetrics`, `GetMetricStatistics` and the alarm evaluation read — reached through the
//! shared engine, so one metric has one home and the publisher and its readers cannot drift.
//! They never travel over `PutMetricData`, which refuses a publish into a namespace the
//! services own ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricDataInput$Namespace`).

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The namespace CloudFront reports its per-distribution metrics in.
pub const NAMESPACE: &str = "AWS/CloudFront";

/// The dimension naming the distribution a datapoint is about.
pub const DISTRIBUTION_ID_DIMENSION: &str = "DistributionId";

/// The dimension naming the Region a per-distribution series is reported for.
pub const REGION_DIMENSION: &str = "Region";

/// The one value [`REGION_DIMENSION`] takes for a distribution's own series: CloudFront
/// serves a distribution from every edge location rather than from a Region, so what its
/// per-distribution metrics are reported for is the whole of it.
pub const GLOBAL_REGION: &str = "Global";

/// How many viewer requests the edge received.
pub const REQUESTS_METRIC: &str = "Requests";

/// How many bytes viewers downloaded through the edge.
pub const BYTES_DOWNLOADED_METRIC: &str = "BytesDownloaded";

/// How many bytes viewers uploaded to the origin through the edge.
pub const BYTES_UPLOADED_METRIC: &str = "BytesUploaded";

/// The share of the period's viewer requests answered with a 4xx or a 5xx status.
pub const TOTAL_ERROR_RATE_METRIC: &str = "TotalErrorRate";

/// The share of the period's viewer requests answered with a 4xx status.
pub const FOUR_XX_ERROR_RATE_METRIC: &str = "4xxErrorRate";

/// The share of the period's viewer requests answered with a 5xx status.
pub const FIVE_XX_ERROR_RATE_METRIC: &str = "5xxErrorRate";

/// Every DEFAULT per-distribution series the edge publishes for a distribution that serves
/// traffic — the ones every distribution has, whether or not it is subscribed to the
/// additional metrics ([`ADDITIONAL_DISTRIBUTION_METRICS`]).
///
/// All of them are published for every request, so a distribution with traffic HAS each of
/// these series and a reader of one is not told a period had no requests because that
/// period's requests happened to carry no bytes or no errors.
pub const DISTRIBUTION_METRICS: &[&str] = &[
    REQUESTS_METRIC,
    BYTES_DOWNLOADED_METRIC,
    BYTES_UPLOADED_METRIC,
    TOTAL_ERROR_RATE_METRIC,
    FOUR_XX_ERROR_RATE_METRIC,
    FIVE_XX_ERROR_RATE_METRIC,
];

/// The share of the period's CACHEABLE viewer requests the edge cache answered.
///
/// One of the ADDITIONAL metrics: published only while the distribution's monitoring
/// subscription enables them ([`AdditionalMetrics`]).
pub const CACHE_HIT_RATE_METRIC: &str = "CacheHitRate";

/// How long the edge took to answer a viewer request an ORIGIN answered.
///
/// One of the ADDITIONAL metrics ([`AdditionalMetrics`]).
pub const ORIGIN_LATENCY_METRIC: &str = "OriginLatency";

/// The per-STATUS-CODE error rates CloudFront reports, each beside the status whose share of
/// the period's viewer requests it carries.
///
/// One series per code, all of them ADDITIONAL metrics ([`AdditionalMetrics`]): the codes
/// CloudFront publishes a rate of its own for, as the "Additional metrics" of its
/// distribution metrics name them (`401ErrorRate`, `403ErrorRate`, `404ErrorRate`,
/// `502ErrorRate`, `503ErrorRate`, `504ErrorRate`). A status outside this list is reported by
/// the CLASS rates alone, which every distribution publishes.
pub const STATUS_ERROR_RATE_METRICS: [(u16, &str); 6] = [
    (401, "401ErrorRate"),
    (403, "403ErrorRate"),
    (404, "404ErrorRate"),
    (502, "502ErrorRate"),
    (503, "503ErrorRate"),
    (504, "504ErrorRate"),
];

/// Every ADDITIONAL per-distribution series, in the order a request observes them: the series
/// a distribution publishes only while its monitoring subscription enables them.
///
/// [`CACHE_HIT_RATE_METRIC`] and [`ORIGIN_LATENCY_METRIC`] are each observed only by the
/// requests their own definitions count, so one request need not fill every one of these — but
/// no request fills ANY of them for a distribution that is not subscribed.
pub const ADDITIONAL_DISTRIBUTION_METRICS: &[&str] = &[
    CACHE_HIT_RATE_METRIC,
    ORIGIN_LATENCY_METRIC,
    STATUS_ERROR_RATE_METRICS[0].1,
    STATUS_ERROR_RATE_METRICS[1].1,
    STATUS_ERROR_RATE_METRICS[2].1,
    STATUS_ERROR_RATE_METRICS[3].1,
    STATUS_ERROR_RATE_METRICS[4].1,
    STATUS_ERROR_RATE_METRICS[5].1,
];

/// The unit an error-rate series carries: it is a percentage of the period's requests, which
/// is the `StandardUnit` value `Percent`.
const PERCENT_UNIT: &str = "Percent";

/// The unit a latency series carries, as the `StandardUnit` enumerated type spells it.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The percentage ONE request contributes to the rate of the class its own status falls in:
/// that request was wholly of that class.
const WHOLE_PERCENT: f64 = 100.0;

/// The percentage one request contributes to the rate of a class its status is not in.
const NO_PERCENT: f64 = 0.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`) — the
/// granularity CloudFront reports a distribution's traffic at.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The methods whose answer's bytes are bytes a viewer DOWNLOADED, as
/// [`BYTES_DOWNLOADED_METRIC`] counts them ("the total number of bytes downloaded by viewers
/// for GET, HEAD, and OPTIONS requests").
const DOWNLOADING_METHODS: [&str; 3] = ["GET", "HEAD", "OPTIONS"];

/// The methods whose request body is bytes a viewer UPLOADED, as [`BYTES_UPLOADED_METRIC`]
/// counts them ("the total number of bytes that viewers uploaded to your origin […] using
/// POST and PUT requests").
const UPLOADING_METHODS: [&str; 2] = ["POST", "PUT"];

/// The methods whose requests are NOT cacheable requests, and so take no part in
/// [`CACHE_HIT_RATE_METRIC`]: "HTTP POST and PUT requests, and errors, are not considered
/// cacheable requests".
const UNCACHEABLE_METHODS: [&str; 2] = ["POST", "PUT"];

/// The lowest status of the 4xx class, and of the 5xx class, and the first status past both.
const CLIENT_ERROR_STATUS: u16 = 400;
const SERVER_ERROR_STATUS: u16 = 500;
const PAST_SERVER_ERROR_STATUS: u16 = 600;

/// WHO produced the answer one viewer request was served with, which is what the cache and
/// origin series read it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedFrom {
    /// The edge cache held the answer, so no origin request was made for this viewer.
    EdgeCache,
    /// An origin produced the answer for this very request.
    Origin,
    /// CloudFront produced the answer itself, reaching neither its cache nor an origin.
    CloudFront,
}

/// Whether a distribution's ADDITIONAL metrics are published for the traffic it serves.
///
/// The one reading of the subscription flag, so the publisher and anything else asking what a
/// distribution reports cannot disagree: the additional series exist for a distribution whose
/// monitoring subscription carries
/// `RealtimeMetricsSubscriptionConfig$RealtimeMetricsSubscriptionStatus` `Enabled` — "a flag
/// that indicates whether additional CloudWatch metrics are enabled for a given CloudFront
/// distribution" — and for no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdditionalMetrics {
    /// A subscription enables them, so they are published beside the default series.
    Enabled,
    /// No subscription enables them, so none of them is published at all.
    Withheld,
}

/// The one `RealtimeMetricsSubscriptionStatus` value that enables the additional metrics, as
/// the enumerated type spells it (`REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES`).
const REALTIME_METRICS_ENABLED: &str = "Enabled";

impl AdditionalMetrics {
    /// What the monitoring subscription a distribution carries — `None` for a distribution
    /// carrying none — says about its additional metrics.
    pub fn of_subscription(subscription: Option<&crate::ops::types::MonitoringSubscription>) -> Self {
        let enabled = subscription
            .and_then(|subscription| subscription.realtime_metrics_subscription_config.as_ref())
            .is_some_and(|config| {
                config.realtime_metrics_subscription_status == REALTIME_METRICS_ENABLED
            });
        if enabled {
            Self::Enabled
        } else {
            Self::Withheld
        }
    }
}

/// ONE viewer request the edge answered, as these series read it.
#[derive(Debug)]
pub struct ServedRequest<'a> {
    /// The request's method, upper-cased, which decides which byte counter it feeds.
    pub method: &'a str,
    /// The status the edge answered with, whoever produced the answer.
    pub status: u16,
    /// How many bytes of body the edge served.
    pub response_bytes: usize,
    /// How many bytes of body the viewer sent.
    pub request_bytes: usize,
    /// Who produced the answer, which is what the hit rate reads.
    pub served_from: ServedFrom,
    /// How long the edge took to answer, for an answer an ORIGIN produced: the span
    /// [`ORIGIN_LATENCY_METRIC`] is the measure of. `None` for a request no origin answered,
    /// which that series does not count.
    pub origin_latency: Option<Duration>,
}

/// One observation a served request contributes to one series: the metric, the value, and the
/// unit the value is published under (`None` for a measurement CloudFront publishes without
/// one).
type Observation = (&'static str, f64, Option<&'static str>);

impl ServedRequest<'_> {
    /// What this one request contributes to each of the distribution's DEFAULT series: the
    /// metric, the value, and the unit the value is published under.
    fn observations(&self) -> [Observation; 6] {
        let four_xx = (CLIENT_ERROR_STATUS..SERVER_ERROR_STATUS).contains(&self.status);
        let five_xx = (SERVER_ERROR_STATUS..PAST_SERVER_ERROR_STATUS).contains(&self.status);
        [
            // A counter CloudFront publishes with no unit of measure, which CloudWatch
            // spells by leaving the unit out of the datum.
            (REQUESTS_METRIC, 1.0, None),
            (
                BYTES_DOWNLOADED_METRIC,
                bytes_of(DOWNLOADING_METHODS.contains(&self.method), self.response_bytes),
                None,
            ),
            (
                BYTES_UPLOADED_METRIC,
                bytes_of(UPLOADING_METHODS.contains(&self.method), self.request_bytes),
                None,
            ),
            (
                TOTAL_ERROR_RATE_METRIC,
                percent_of(four_xx || five_xx),
                Some(PERCENT_UNIT),
            ),
            (
                FOUR_XX_ERROR_RATE_METRIC,
                percent_of(four_xx),
                Some(PERCENT_UNIT),
            ),
            (
                FIVE_XX_ERROR_RATE_METRIC,
                percent_of(five_xx),
                Some(PERCENT_UNIT),
            ),
        ]
    }

    /// What this one request contributes to each ADDITIONAL series — the ones a monitoring
    /// subscription buys — in the order [`ADDITIONAL_DISTRIBUTION_METRICS`] lists them.
    ///
    /// A series this request takes no part in is absent from the list rather than observed as
    /// zero: a request that is not a cacheable request has no hit rate of its own, and a
    /// request no origin answered has no origin latency, so neither is folded into the
    /// period's average as if it were one.
    fn additional_observations(&self) -> Vec<Observation> {
        let mut observations: Vec<Observation> = Vec::new();
        if let Some(rate) = self.cache_hit_rate() {
            observations.push((CACHE_HIT_RATE_METRIC, rate, Some(PERCENT_UNIT)));
        }
        if let Some(latency) = self.origin_latency {
            observations.push((
                ORIGIN_LATENCY_METRIC,
                milliseconds_of(latency),
                Some(MILLISECONDS_UNIT),
            ));
        }
        for (status, metric) in STATUS_ERROR_RATE_METRICS {
            observations.push((metric, percent_of(self.status == status), Some(PERCENT_UNIT)));
        }
        observations
    }

    /// The percentage this request contributes to [`CACHE_HIT_RATE_METRIC`], or `None` when it
    /// is not one of the requests that metric is a percentage OF.
    ///
    /// "The percentage of all cacheable requests for which CloudFront served the content from
    /// its cache. HTTP POST and PUT requests, and errors, are not considered cacheable
    /// requests": an upload and an answer of an error class take no part in the rate, and
    /// neither does an answer CloudFront produced itself, which came from no cache and no
    /// origin. Everything left is a cacheable request, and it is wholly a hit when the edge
    /// cache answered it and wholly a miss when the origin did.
    fn cache_hit_rate(&self) -> Option<f64> {
        if UNCACHEABLE_METHODS.contains(&self.method) || self.status >= CLIENT_ERROR_STATUS {
            return None;
        }
        match self.served_from {
            ServedFrom::EdgeCache => Some(WHOLE_PERCENT),
            ServedFrom::Origin => Some(NO_PERCENT),
            ServedFrom::CloudFront => None,
        }
    }
}

/// `latency` in the unit [`ORIGIN_LATENCY_METRIC`] is published under.
fn milliseconds_of(latency: Duration) -> f64 {
    latency.as_secs_f64() * MILLISECONDS_PER_SECOND
}

/// How many milliseconds one second is, which is what a measured span is published as.
const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// The percentage one request contributes to a rate whose class its status either is or is
/// not in.
fn percent_of(in_class: bool) -> f64 {
    if in_class {
        WHOLE_PERCENT
    } else {
        NO_PERCENT
    }
}

/// `bytes` when this request's method is one the counter counts, and 0 when it is not: a
/// request of another method contributed nothing to it, which is what a counter's own 0 says.
fn bytes_of(counted: bool, bytes: usize) -> f64 {
    if counted {
        // A body longer than f64's exact integers cannot arise: the edge buffers both bodies
        // in memory.
        bytes as f64
    } else {
        0.0
    }
}

/// Count one answer the edge served into the series of the distribution it served it for.
///
/// The DEFAULT series are published for every distribution; the ADDITIONAL ones only where
/// `additional` says this distribution's monitoring subscription enables them, so a
/// distribution that is not subscribed to them never gets one of their datapoints.
///
/// Nothing is published when the CloudWatch store is not mounted beside this service: a
/// datapoint has one home, and this path never makes a second one.
pub fn count_viewer_request<Eng: Send + Sync + 'static>(
    engine: &Eng,
    distribution_id: &str,
    at: SystemTime,
    served: &ServedRequest<'_>,
    additional: AdditionalMetrics,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let dimensions: cloudwatch::state::DimensionSet = [
        (
            DISTRIBUTION_ID_DIMENSION.to_string(),
            distribution_id.to_string(),
        ),
        (REGION_DIMENSION.to_string(), GLOBAL_REGION.to_string()),
    ]
    .into_iter()
    .collect();
    let timestamp_secs = epoch_secs(at);
    let published: Vec<Observation> = match additional {
        AdditionalMetrics::Enabled => served
            .observations()
            .into_iter()
            .chain(served.additional_observations())
            .collect(),
        AdditionalMetrics::Withheld => served.observations().to_vec(),
    };
    let mut store = state.lock();
    for (metric_name, value, unit) in published {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: dimensions.clone(),
        };
        let datum = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit its
            // own readers window by.
            timestamp_secs,
            // One observation: this request's own contribution to the series.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: unit.map(str::to_string),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already served,
            // so nothing is failed for it; the dropped observation is reported where a
            // developer can see it.
            ::tracing::warn!(
                distribution = %distribution_id,
                metric = %metric_name,
                "the CloudWatch metric store is at capacity, so this viewer request was not \
                 counted"
            );
        }
    }
}

/// `at` as the epoch seconds a stored datapoint carries.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

#[cfg(test)]
mod tests {
    //! What one served answer contributes to each series, which is the rule a reader folding
    //! a period of them depends on. The published series themselves are pinned end to end
    //! over the wire in `tests/all/edge_request_metrics.rs`.

    use super::*;

    /// The value `metric` takes in `served`'s observations.
    fn value_of(served: &ServedRequest<'_>, metric: &str) -> f64 {
        served
            .observations()
            .into_iter()
            .find(|(name, _, _)| *name == metric)
            .map(|(_, value, _)| value)
            .unwrap_or_else(|| panic!("{metric} is one of the series the edge publishes"))
    }

    #[test]
    fn an_error_rate_is_the_whole_of_a_request_of_its_own_class() {
        // Rows span the status classes the rates split viewer requests into, including the
        // boundaries of each class.
        let cases: [(u16, f64, f64, f64); 8] = [
            (200, 0.0, 0.0, 0.0),
            (304, 0.0, 0.0, 0.0),
            (399, 0.0, 0.0, 0.0),
            (400, 100.0, 0.0, 100.0),
            (404, 100.0, 0.0, 100.0),
            (499, 100.0, 0.0, 100.0),
            (500, 0.0, 100.0, 100.0),
            (502, 0.0, 100.0, 100.0),
        ];
        for (status, four_xx, five_xx, total) in cases {
            let served = ServedRequest {
                method: "GET",
                status,
                response_bytes: 0,
                request_bytes: 0,
                served_from: ServedFrom::Origin,
                origin_latency: None,
            };
            assert_eq!(
                value_of(&served, FOUR_XX_ERROR_RATE_METRIC),
                four_xx,
                "case {status}: expected a 4xx rate of {four_xx}"
            );
            assert_eq!(
                value_of(&served, FIVE_XX_ERROR_RATE_METRIC),
                five_xx,
                "case {status}: expected a 5xx rate of {five_xx}"
            );
            assert_eq!(
                value_of(&served, TOTAL_ERROR_RATE_METRIC),
                total,
                "case {status}: expected a total error rate of {total}"
            );
            assert_eq!(
                value_of(&served, REQUESTS_METRIC),
                1.0,
                "case {status}: every answer is one viewer request"
            );
        }
    }

    #[test]
    fn the_byte_counters_count_the_methods_their_own_definitions_name() {
        // Rows span the methods a cache behaviour can allow, each one carrying bytes in both
        // directions so what is counted is the method's own counter and not the bytes.
        let cases: [(&str, f64, f64); 7] = [
            ("GET", 11.0, 0.0),
            ("HEAD", 11.0, 0.0),
            ("OPTIONS", 11.0, 0.0),
            ("POST", 0.0, 7.0),
            ("PUT", 0.0, 7.0),
            ("PATCH", 0.0, 0.0),
            ("DELETE", 0.0, 0.0),
        ];
        for (method, downloaded, uploaded) in cases {
            let served = ServedRequest {
                method,
                status: 200,
                response_bytes: 11,
                request_bytes: 7,
                served_from: ServedFrom::Origin,
                origin_latency: None,
            };
            assert_eq!(
                value_of(&served, BYTES_DOWNLOADED_METRIC),
                downloaded,
                "case {method}: expected {downloaded} bytes downloaded"
            );
            assert_eq!(
                value_of(&served, BYTES_UPLOADED_METRIC),
                uploaded,
                "case {method}: expected {uploaded} bytes uploaded"
            );
        }
    }

    #[test]
    fn every_published_series_is_observed_by_every_request() {
        let served = ServedRequest {
            method: "GET",
            status: 404,
            response_bytes: 3,
            request_bytes: 0,
            served_from: ServedFrom::Origin,
            origin_latency: None,
        };
        let observed: Vec<&str> = served
            .observations()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(
            observed, DISTRIBUTION_METRICS,
            "every series the edge publishes is observed by every request it serves"
        );
    }

    /// One request of `method` answered `status` by `served_from`, with a measured span for an
    /// answer an origin produced.
    fn served_of(method: &'static str, status: u16, served_from: ServedFrom) -> ServedRequest<'static> {
        ServedRequest {
            method,
            status,
            response_bytes: 5,
            request_bytes: 0,
            served_from,
            origin_latency: matches!(served_from, ServedFrom::Origin)
                .then(|| Duration::from_millis(7)),
        }
    }

    /// The value `metric` takes in `served`'s ADDITIONAL observations, or `None` when this
    /// request contributes nothing to that series.
    fn additional_value_of(served: &ServedRequest<'_>, metric: &str) -> Option<f64> {
        served
            .additional_observations()
            .into_iter()
            .find(|(name, _, _)| *name == metric)
            .map(|(_, value, _)| value)
    }

    #[test]
    fn the_hit_rate_counts_only_the_cacheable_requests_a_cache_or_an_origin_answered() {
        // Rows span who answered the request, the methods the metric's definition excludes,
        // and an answer of an error class — the three things that decide whether a request is
        // a cacheable request at all.
        let cases: [(&str, &'static str, u16, ServedFrom, Option<f64>); 7] = [
            ("a cached read", "GET", 200, ServedFrom::EdgeCache, Some(100.0)),
            ("a read the origin answered", "GET", 200, ServedFrom::Origin, Some(0.0)),
            ("a cached conditional read", "HEAD", 304, ServedFrom::EdgeCache, Some(100.0)),
            ("an upload", "POST", 200, ServedFrom::Origin, None),
            ("a replaced object", "PUT", 200, ServedFrom::Origin, None),
            ("an error the origin answered", "GET", 404, ServedFrom::Origin, None),
            (
                "an answer CloudFront produced itself",
                "GET",
                403,
                ServedFrom::CloudFront,
                None,
            ),
        ];
        for (case, method, status, served_from, expected) in cases {
            let served = served_of(method, status, served_from);
            let actual = additional_value_of(&served, CACHE_HIT_RATE_METRIC);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected a hit rate of {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_origin_latency_is_published_only_for_an_answer_an_origin_produced() {
        let from_origin = ServedRequest {
            method: "GET",
            status: 200,
            response_bytes: 5,
            request_bytes: 0,
            served_from: ServedFrom::Origin,
            origin_latency: Some(Duration::from_millis(12)),
        };
        assert_eq!(
            additional_value_of(&from_origin, ORIGIN_LATENCY_METRIC),
            Some(12.0),
            "case {:?}: the measured span is published in milliseconds",
            "an answer the origin produced"
        );
        for (case, served_from) in [
            ("an answer the edge cache held", ServedFrom::EdgeCache),
            ("an answer CloudFront produced itself", ServedFrom::CloudFront),
        ] {
            let served = served_of("GET", 200, served_from);
            let actual = additional_value_of(&served, ORIGIN_LATENCY_METRIC);
            assert_eq!(
                actual, None,
                "case {case:?}: no origin answered it, so it has no origin latency, got \
                 {actual:?}"
            );
        }
    }

    #[test]
    fn a_per_status_rate_is_the_whole_of_a_request_answered_with_its_own_status() {
        // Rows span each status CloudFront publishes a rate of its own for, plus a status it
        // publishes none for (which the class rates still carry).
        let statuses: [u16; 7] = [401, 403, 404, 502, 503, 504, 410];
        for status in statuses {
            let served = served_of("GET", status, ServedFrom::Origin);
            for (code, metric) in STATUS_ERROR_RATE_METRICS {
                let expected = percent_of(code == status);
                let actual = additional_value_of(&served, metric);
                assert_eq!(
                    actual,
                    Some(expected),
                    "case {status}: expected {metric} to carry {expected}, got {actual:?}"
                );
            }
        }
    }

    #[test]
    fn the_additional_series_are_the_ones_a_subscription_buys() {
        let served = served_of("GET", 200, ServedFrom::Origin);
        let observed: Vec<&str> = served
            .additional_observations()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(
            observed, ADDITIONAL_DISTRIBUTION_METRICS,
            "a cacheable request the origin answered observes every additional series"
        );
        for metric in ADDITIONAL_DISTRIBUTION_METRICS {
            assert!(
                !DISTRIBUTION_METRICS.contains(metric),
                "case {metric:?}: an additional series is not one of the default series"
            );
        }
    }

    #[test]
    fn only_an_enabled_subscription_enables_the_additional_metrics() {
        use crate::ops::types::{MonitoringSubscription, RealtimeMetricsSubscriptionConfig};

        /// One subscription carrying `status`.
        fn subscription(status: &str) -> MonitoringSubscription {
            MonitoringSubscription {
                realtime_metrics_subscription_config: Some(RealtimeMetricsSubscriptionConfig {
                    realtime_metrics_subscription_status: status.to_string(),
                }),
            }
        }

        // Rows span every value the flag's enumerated type takes
        // (`REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES`), a subscription carrying no
        // configuration at all, and a distribution carrying no subscription.
        let enabled = subscription("Enabled");
        let disabled = subscription("Disabled");
        let empty = MonitoringSubscription {
            realtime_metrics_subscription_config: None,
        };
        let cases: [(&str, Option<&MonitoringSubscription>, AdditionalMetrics); 4] = [
            ("an Enabled subscription", Some(&enabled), AdditionalMetrics::Enabled),
            ("a Disabled subscription", Some(&disabled), AdditionalMetrics::Withheld),
            (
                "a subscription carrying no configuration",
                Some(&empty),
                AdditionalMetrics::Withheld,
            ),
            ("no subscription at all", None, AdditionalMetrics::Withheld),
        ];
        for (case, subscription, expected) in cases {
            let actual = AdditionalMetrics::of_subscription(subscription);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
