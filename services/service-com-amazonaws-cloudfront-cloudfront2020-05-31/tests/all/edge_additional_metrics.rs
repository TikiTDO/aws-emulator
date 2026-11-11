// SPDX-License-Identifier: CC0-1.0

//! The ADDITIONAL `AWS/CloudFront` series a monitoring subscription buys, read back through
//! CloudWatch.
//!
//! RULE: **the additional per-distribution series — the cache hit rate, the origin latency and
//! the per-status-code error rates — are published for the traffic a distribution serves ONLY
//! while that distribution's monitoring subscription carries
//! `RealtimeMetricsSubscriptionStatus` `Enabled` ("a flag that indicates whether additional
//! CloudWatch metrics are enabled for a given CloudFront distribution"); for every other
//! distribution they have no datapoint in any minute and are listed nowhere, while the default
//! series the same traffic fills are published either way.**
//!
//! Each row drives the SAME traffic through a distribution of its own and differs only in what
//! that distribution's subscription says, so the contrast is the subscription and nothing else.
//! The rows span every value the flag's enumerated type takes
//! (`REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES`), a subscription that carries no
//! configuration at all, and a distribution carrying no subscription.
//!
//! The traffic spans what each additional series is a measure OF: a read the origin answers (a
//! cache miss), the same read again (a cache hit), a read the origin answers with a 404, and a
//! method the cache behaviour does not allow, which CloudFront refuses itself with a 403 and no
//! origin request at all.

use serde_json::{json, Value};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::service::edge_metrics;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, create_distribution, past_propagation, API_VERSION, HOST};
use crate::edge_request_metrics::{
    assert_deployed, cloudwatch, dimensions, emulator, held, now_secs, view, Held,
};

/// The AWS-managed cache policy `Managed-CachingOptimized`, whose `DefaultTTL` is a day: the
/// second identical read of an object under it is answered by the edge cache, which is what a
/// hit rate is a rate of.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// An origin this emulator SERVES: this service's own control-plane endpoint, which answers
/// `GET /2020-05-31/distribution` with a `DistributionList` and a read of an id it holds no
/// distribution for with its own 404.
const SERVED_ORIGIN: &str = HOST;

/// The `DistributionConfig` every row serves through: one origin under a caching policy, and a
/// default behaviour allowing only the reading methods — so a `DELETE` is refused by CloudFront
/// itself.
fn config(caller_reference: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{SERVED_ORIGIN}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>3</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method><Method>OPTIONS</Method></Items>\
         <CachedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
         </AllowedMethods>\
         <CachePolicyId>{CACHING_OPTIMIZED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>edge additional metrics</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// Create one distribution for a row and answer its `(id, DomainName)`.
async fn created(router: &MultiServiceRouter, caller_reference: &str) -> (String, String) {
    let created = create_distribution(router, &config(caller_reference)).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference:?}: expected HTTP 201 from CreateDistribution, got {}: {}",
        created.status, created.raw
    );
    let root = created.root();
    let id = root
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = root
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    (id, domain)
}

/// `CreateMonitoringSubscription` for `id`, carrying `status` — or carrying no
/// `RealtimeMetricsSubscriptionConfig` at all when the row names none.
async fn subscribe(router: &MultiServiceRouter, case: &str, id: &str, status: Option<&str>) {
    let mut document = format!(
        "<MonitoringSubscription xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(status) = status {
        document.push_str(&format!(
            "<RealtimeMetricsSubscriptionConfig>\
             <RealtimeMetricsSubscriptionStatus>{status}</RealtimeMetricsSubscriptionStatus>\
             </RealtimeMetricsSubscriptionConfig>"
        ));
    }
    document.push_str("</MonitoringSubscription>");
    let answer = call(
        router,
        "POST",
        &format!("/{API_VERSION}/distributions/{id}/monitoring-subscription"),
        Some(&document),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateMonitoringSubscription, got {}: {}",
        answer.status, answer.raw
    );
}

/// One viewer request a row drives, and what the edge is expected to answer it with.
struct Driven {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    status: u16,
}

/// The traffic every row drives, in order: the second row repeats the first row's read, so the
/// edge cache answers it.
fn driven() -> [Driven; 4] {
    [
        Driven {
            case: "a read the origin answers",
            method: "GET",
            path: "/2020-05-31/distribution",
            status: 200,
        },
        Driven {
            case: "the same read again, which the edge cache answers",
            method: "GET",
            path: "/2020-05-31/distribution",
            status: 200,
        },
        Driven {
            case: "a read of an object the origin does not hold",
            method: "GET",
            path: "/2020-05-31/distribution/E0NOSUCHDISTRIB",
            status: 404,
        },
        Driven {
            case: "a method the cache behaviour does not allow",
            method: "DELETE",
            path: "/2020-05-31/distribution",
            status: 403,
        },
    ]
}

/// How many of [`driven`]'s requests the whole table drives, which the default series count.
const DRIVEN_REQUESTS: f64 = 4.0;

/// How many of them are CACHEABLE requests the hit rate is a rate of: the two reads answered
/// 200 — the 404 and the 403 are errors, which "are not considered cacheable requests".
const CACHEABLE_REQUESTS: f64 = 2.0;

/// The hit rate those cacheable requests have: one of the two was answered by the edge cache.
const EXPECTED_HIT_RATE: f64 = 50.0;

/// How many of them an ORIGIN answered, which is what the origin latency is published for: the
/// first read and the 404. The repeat read came from the cache and the 403 from CloudFront
/// itself.
const ORIGIN_ANSWERED_REQUESTS: f64 = 2.0;

/// One row of the gating table: what a distribution's monitoring subscription says.
struct Gate {
    case: &'static str,
    /// The `RealtimeMetricsSubscriptionStatus` the row subscribes with, `Some(None)` for a
    /// subscription carrying no configuration at all, and `None` for a distribution that is
    /// never subscribed.
    subscription: Option<Option<&'static str>>,
    /// Whether the additional series are published for this row.
    published: bool,
}

/// The gating table: every value the flag's enumerated type takes, a subscription carrying no
/// configuration, and no subscription at all.
fn gates() -> Vec<Gate> {
    let mut rows: Vec<Gate> = REALTIME_METRICS_SUBSCRIPTION_STATUS_VALUES
        .iter()
        .map(|status| Gate {
            case: status,
            subscription: Some(Some(status)),
            published: *status == "Enabled",
        })
        .collect();
    rows.push(Gate {
        case: "a subscription carrying no configuration",
        subscription: Some(None),
        published: false,
    });
    rows.push(Gate {
        case: "no subscription at all",
        subscription: None,
        published: false,
    });
    rows
}

/// Every series `ListMetrics` reports for `distribution_id` in the `AWS/CloudFront` namespace.
async fn listed(router: &MultiServiceRouter, case: &str, distribution_id: &str) -> Vec<String> {
    let answered = cloudwatch(
        router,
        case,
        "ListMetrics",
        &json!({"Namespace": edge_metrics::NAMESPACE}),
    )
    .await;
    let wanted = dimensions(distribution_id);
    let mut names: Vec<String> = answered["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|metric: &&Value| metric["Dimensions"] == wanted)
        .filter_map(|metric| metric["MetricName"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

#[tokio::test]
async fn the_additional_series_are_published_only_while_a_subscription_enables_them() {
    let router = emulator();
    let rows = gates();

    // Every row's distribution is created and deployed first, so the clock is waited out once
    // for all of them.
    let mut subjects: Vec<(String, String)> = Vec::new();
    for index in 0..rows.len() {
        subjects.push(created(&router, &format!("edge-additional-{index}")).await);
    }
    ::tokio::time::sleep(past_propagation()).await;

    for (row, (id, domain)) in rows.iter().zip(subjects.iter()) {
        let case = row.case;
        assert_deployed(&router, id).await;
        if let Some(status) = row.subscription {
            subscribe(&router, case, id, status).await;
        }

        let at = now_secs();
        for request in driven() {
            let (status, _bytes) = view(&router, request.method, domain, request.path).await;
            assert_eq!(
                status, request.status,
                "case {case:?} ({}): expected the edge to answer {}, got {status}",
                request.case, request.status
            );
        }

        // The DEFAULT series are published for every row, subscribed or not: they are what
        // tells a client the traffic was served at all.
        let counted = held(&router, case, id, edge_metrics::REQUESTS_METRIC, at).await;
        assert_eq!(
            counted.sum, DRIVEN_REQUESTS,
            "case {case:?}: every viewer request is counted whatever the subscription says, got \
             {counted:?}"
        );
        let four_xx = held(
            &router,
            case,
            id,
            edge_metrics::FOUR_XX_ERROR_RATE_METRIC,
            at,
        )
        .await;
        let expected_four_xx = 100.0 * 2.0 / DRIVEN_REQUESTS;
        assert!(
            (four_xx.average() - expected_four_xx).abs() < f64::EPSILON,
            "case {case:?}: expected a 4xx rate of {expected_four_xx}, got {four_xx:?}"
        );

        // The ADDITIONAL series: each one is either the measure of this traffic or has no
        // datapoint at all, and which it is depends only on the subscription.
        let hit_rate = held(&router, case, id, edge_metrics::CACHE_HIT_RATE_METRIC, at).await;
        let latency = held(&router, case, id, edge_metrics::ORIGIN_LATENCY_METRIC, at).await;
        if row.published {
            assert_eq!(
                hit_rate.samples, CACHEABLE_REQUESTS,
                "case {case:?}: the hit rate is a rate of the cacheable requests alone, got \
                 {hit_rate:?}"
            );
            assert!(
                (hit_rate.average() - EXPECTED_HIT_RATE).abs() < f64::EPSILON,
                "case {case:?}: expected a hit rate of {EXPECTED_HIT_RATE}, got {hit_rate:?}"
            );
            assert_eq!(
                hit_rate.units,
                vec!["Percent".to_string()],
                "case {case:?}: a hit rate is a percentage, got {hit_rate:?}"
            );
            assert_eq!(
                latency.samples, ORIGIN_ANSWERED_REQUESTS,
                "case {case:?}: the origin latency is published for the requests an origin \
                 answered, got {latency:?}"
            );
            assert_eq!(
                latency.units,
                vec![edge_metrics::MILLISECONDS_UNIT.to_string()],
                "case {case:?}: an origin latency is a span of time, got {latency:?}"
            );
            assert!(
                latency.sum >= 0.0 && latency.sum.is_finite(),
                "case {case:?}: the published latency must be a measured span, got {latency:?}"
            );
        } else {
            assert_no_datapoint(case, "CacheHitRate", &hit_rate);
            assert_no_datapoint(case, "OriginLatency", &latency);
        }

        // One series per status code CloudFront reports a rate for: each is the share of the
        // traffic answered with that one status while the subscription enables them, and has
        // nothing at all otherwise.
        for (code, metric) in edge_metrics::STATUS_ERROR_RATE_METRICS {
            let rate = held(&router, case, id, metric, at).await;
            if row.published {
                let answered_with = driven()
                    .iter()
                    .filter(|request| request.status == code)
                    .count() as f64;
                let expected = 100.0 * answered_with / DRIVEN_REQUESTS;
                assert_eq!(
                    rate.samples, DRIVEN_REQUESTS,
                    "case {case:?} ({metric}): every request observes a per-status rate, got \
                     {rate:?}"
                );
                assert!(
                    (rate.average() - expected).abs() < f64::EPSILON,
                    "case {case:?} ({metric}): expected a rate of {expected} for the \
                     {answered_with} of {DRIVEN_REQUESTS} requests answered {code}, got {rate:?}"
                );
            } else {
                assert_no_datapoint(case, metric, &rate);
            }
        }

        // And what a client LISTS agrees with what it can read: the additional series are
        // listed for a subscribed distribution and for no other.
        let names = listed(&router, case, id).await;
        for metric in edge_metrics::ADDITIONAL_DISTRIBUTION_METRICS {
            let found = names.iter().any(|name| name == metric);
            assert_eq!(
                found, row.published,
                "case {case:?} ({metric}): expected listed = {}, got {names:?}",
                row.published
            );
        }
        for metric in edge_metrics::DISTRIBUTION_METRICS {
            assert!(
                names.iter().any(|name| name == metric),
                "case {case:?} ({metric}): a default series is listed whatever the subscription \
                 says, got {names:?}"
            );
        }
    }
}

/// Assert that `held` holds nothing: a series a distribution is not subscribed to has no
/// datapoint in any minute.
fn assert_no_datapoint(case: &str, metric: &str, held: &Held) {
    assert_eq!(
        held.samples, 0.0,
        "case {case:?} ({metric}): this series is withheld without a subscription, got {held:?}"
    );
    assert_eq!(
        held.sum, 0.0,
        "case {case:?} ({metric}): this series is withheld without a subscription, got {held:?}"
    );
}
