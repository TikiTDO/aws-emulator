// SPDX-License-Identifier: CC0-1.0

//! What the EDGE reports about the viewer traffic it served, read back through CloudWatch.
//!
//! RULE: **every answer the edge serves for a distribution is counted in the
//! `AWS/CloudFront` namespace against that distribution — dimensioned by its own
//! `DistributionId` beside `Region=Global`, which is the Region a per-distribution series is
//! reported for — as one observation of `Requests`, of the bytes carried in each direction,
//! and of each error rate: a rate carries the whole 100 Percent for a request whose own
//! status is of its class and 0 for one that is not, so the `Average` over a period is the
//! share of that period's requests answered in that class.**
//!
//! Only a suite with CloudWatch mounted beside this service can state that: the series the
//! edge publishes live in the CloudWatch store (there is one home for a metric), so what a
//! client sees is what `ListMetrics` and `GetMetricStatistics` answer. The sibling
//! `edge_data_plane` suite pins what the edge SERVES; this one pins what it REPORTS about it.
//!
//! The rows span what the rule generalises over: the status class of the answer (a 2xx and a
//! 4xx and a 5xx the origin produced, a 4xx CloudFront produced itself for a method the
//! behaviour does not allow, and a 5xx CloudFront produced for an origin it could not reach),
//! the method (one a byte counter counts and one it does not), and the distribution the
//! traffic was addressed to — two of them, so a series is shown to carry one distribution's
//! requests and not another's.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::service::edge_metrics;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::distribution_lifecycle::{call, create_distribution, past_propagation, API_VERSION, HOST};

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// viewer request under it reaches the origin, so no request is absorbed by a cache hit and
/// each row's answer is counted on its own.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// An origin this emulator SERVES: this service's own control-plane endpoint, which answers
/// `GET /2020-05-31/distribution` with a `DistributionList` and a read of an id it holds no
/// distribution for with its own 404.
const SERVED_ORIGIN: &str = HOST;

/// An origin NOTHING in this emulator serves — the host shape of a Lambda function URL — so
/// the edge reports it as an origin it could not reach, with its own 502.
const UNSERVED_ORIGIN: &str = "a0b1c2d3e4f5g6h7.lambda-url.us-east-1.on.aws";

/// CloudWatch's `X-Amz-Target` prefix and its signing name, which is what routes a call to it.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The Region every call is signed for. CloudFront reports its per-distribution metrics in
/// one Region, so the dimension that names one carries `Global` whatever Region a reader asks
/// from.
const REGION: &str = "us-east-1";

/// The period the series are read at, in seconds: the granularity CloudFront reports a
/// distribution's traffic at.
const PERIOD: i64 = 60;

/// One emulator carrying this service and CloudWatch on ONE engine: the edge publishes into
/// the CloudWatch store, so both have to be mounted on the same engine for a client to read
/// back what the edge reported.
pub(crate) fn emulator() -> MultiServiceRouter {
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

/// One viewer request: no AWS credential, addressed to the distribution's own domain name,
/// exactly as a browser reaches a distribution. Answers the status and the body's length.
pub(crate) async fn view(
    router: &MultiServiceRouter,
    method: &str,
    domain: &str,
    path: &str,
) -> (u16, usize) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", domain)
        .body(::bytes::Bytes::new())
        .expect("the viewer request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().len())
}

/// One CloudWatch call (`awsJson1_0`), which must answer 200.
pub(crate) async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                 {CLOUDWATCH_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(::bytes::Bytes::from(body.to_string()))
        .expect("the CloudWatch request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Wall-clock now as epoch seconds, the clock a datapoint's instant is stamped on.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The `DistributionConfig` these rows serve through: one origin, and a default behaviour
/// that allows only `GET`, `HEAD` and `OPTIONS` — so a `DELETE` is refused by CloudFront
/// itself and that refusal is a 4xx of its own.
fn config(caller_reference: &str, origin: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{origin}</DomainName>\
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
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>edge request metrics</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// Create one distribution over `origin` and answer its `(id, DomainName)`. The clock is
/// waited out once, by the caller, for every distribution it created.
async fn created(router: &MultiServiceRouter, caller_reference: &str, origin: &str) -> (String, String) {
    let created = create_distribution(router, &config(caller_reference, origin)).await;
    assert_eq!(
        created.status, 201,
        "creating the distribution over {origin} answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    (id, domain)
}

/// Assert through `GetDistribution` that the clock has deployed `id`.
pub(crate) async fn assert_deployed(router: &MultiServiceRouter, id: &str) {
    let read = call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        read.root().find_text("Status"),
        Some("Deployed"),
        "the distribution {id} did not reach Deployed: {}",
        read.raw
    );
}

/// The whole span of the series this suite reads, as a `(StartTime, EndTime)` pair around the
/// instant the traffic was served: wide enough that every datapoint falls inside it whichever
/// minute the suite happens to run in.
fn window(at: f64) -> (f64, f64) {
    (at - 2.0 * PERIOD as f64, at + 2.0 * PERIOD as f64)
}

/// What one series holds over the whole window: how many observations, their sum, and the
/// unit every one of them was published under.
#[derive(Debug, Default)]
pub(crate) struct Held {
    pub(crate) samples: f64,
    pub(crate) sum: f64,
    pub(crate) units: Vec<String>,
}

impl Held {
    /// The average over every observation in the window — the statistic a rate is read with,
    /// folded across the windows the read split the span into so the answer does not depend
    /// on which minute boundary the traffic straddled.
    pub(crate) fn average(&self) -> f64 {
        if self.samples == 0.0 {
            return f64::NAN;
        }
        self.sum / self.samples
    }
}

/// Read one of a distribution's series over the whole window.
pub(crate) async fn held(
    router: &MultiServiceRouter,
    case: &str,
    distribution_id: &str,
    metric: &str,
    at: f64,
) -> Held {
    let (start, end) = window(at);
    let answered = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": edge_metrics::NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions(distribution_id),
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD,
            "Statistics": ["SampleCount", "Sum", "Average"],
        }),
    )
    .await;
    let mut folded = Held::default();
    for datapoint in answered["Datapoints"].as_array().cloned().unwrap_or_default() {
        folded.samples += datapoint["SampleCount"].as_f64().unwrap_or_default();
        folded.sum += datapoint["Sum"].as_f64().unwrap_or_default();
        if let Some(unit) = datapoint["Unit"].as_str() {
            if !folded.units.iter().any(|held| held == unit) {
                folded.units.push(unit.to_string());
            }
        }
    }
    folded
}

/// The dimension set a distribution's own series carries, as a request names it.
pub(crate) fn dimensions(distribution_id: &str) -> Value {
    json!([
        {"Name": edge_metrics::DISTRIBUTION_ID_DIMENSION, "Value": distribution_id},
        {"Name": edge_metrics::REGION_DIMENSION, "Value": edge_metrics::GLOBAL_REGION},
    ])
}

/// One viewer request a row drives, and what the edge is expected to answer it with.
struct Driven {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    /// The status class the answer falls in, which is what the rates count.
    status: u16,
}

/// The requests driven against the distribution whose origin this emulator serves.
fn served_requests() -> Vec<Driven> {
    vec![
        Driven {
            case: "a GET the origin answers",
            method: "GET",
            path: "/2020-05-31/distribution",
            status: 200,
        },
        Driven {
            case: "a HEAD the origin answers with a status of its own",
            method: "HEAD",
            path: "/2020-05-31/distribution",
            status: 501,
        },
        Driven {
            case: "a GET for a distribution the origin does not hold",
            method: "GET",
            path: "/2020-05-31/distribution/E0NOSUCHDISTRIB",
            status: 404,
        },
        Driven {
            case: "a DELETE the cache behaviour does not allow",
            method: "DELETE",
            path: "/2020-05-31/distribution",
            status: 403,
        },
    ]
}

#[tokio::test]
async fn every_answer_the_edge_serves_is_counted_against_its_own_distribution() {
    let router = emulator();
    let (served_id, served_domain) = created(&router, "edge-metrics-served", SERVED_ORIGIN).await;
    let (unserved_id, unserved_domain) =
        created(&router, "edge-metrics-unserved", UNSERVED_ORIGIN).await;
    ::tokio::time::sleep(past_propagation()).await;
    assert_deployed(&router, &served_id).await;
    assert_deployed(&router, &unserved_id).await;

    let at = now_secs();
    let mut requests = 0.0_f64;
    let mut client_errors = 0.0_f64;
    let mut server_errors = 0.0_f64;
    let mut downloaded = 0.0_f64;
    for row in served_requests() {
        let (status, bytes) = view(&router, row.method, &served_domain, row.path).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected the edge to answer {}, got {status}",
            row.case, row.status
        );
        requests += 1.0;
        if (400..500).contains(&status) {
            client_errors += 1.0;
        }
        if (500..600).contains(&status) {
            server_errors += 1.0;
        }
        if ["GET", "HEAD", "OPTIONS"].contains(&row.method) {
            downloaded += bytes as f64;
        }
    }

    // `Requests` counts every answer, whoever produced it.
    let counted = held(&router, "the served distribution", &served_id, edge_metrics::REQUESTS_METRIC, at).await;
    assert_eq!(
        counted.sum, requests,
        "every viewer request must be counted once: expected {requests}, got {counted:?}"
    );
    assert_eq!(
        counted.units,
        vec!["None".to_string()],
        "a counter CloudFront publishes carries no unit of measure, got {counted:?}"
    );

    // The rates are the SHARE of those requests answered in each class.
    let four_xx = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::FOUR_XX_ERROR_RATE_METRIC,
        at,
    )
    .await;
    let expected_four_xx = 100.0 * client_errors / requests;
    let expected_five_xx = 100.0 * server_errors / requests;
    let expected_total = 100.0 * (client_errors + server_errors) / requests;
    assert!(
        (four_xx.average() - expected_four_xx).abs() < f64::EPSILON,
        "the 4xx rate must be the share of the window's requests answered 4xx ({client_errors} \
         of {requests} = {expected_four_xx}), got {four_xx:?}"
    );
    assert_eq!(
        four_xx.units,
        vec!["Percent".to_string()],
        "an error rate is a percentage of the period's requests, got {four_xx:?}"
    );
    let five_xx = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::FIVE_XX_ERROR_RATE_METRIC,
        at,
    )
    .await;
    assert!(
        (five_xx.average() - expected_five_xx).abs() < f64::EPSILON,
        "the 5xx rate must be the share answered 5xx ({server_errors} of {requests} = \
         {expected_five_xx}), got {five_xx:?}"
    );
    let total = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::TOTAL_ERROR_RATE_METRIC,
        at,
    )
    .await;
    assert!(
        (total.average() - expected_total).abs() < f64::EPSILON,
        "the total error rate must be the share answered 4xx or 5xx ({expected_total}), got \
         {total:?}"
    );

    // The byte counters count the methods their own definitions name: the `DELETE` row's
    // answer carried bytes and contributed none.
    let bytes_downloaded = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::BYTES_DOWNLOADED_METRIC,
        at,
    )
    .await;
    assert_eq!(
        bytes_downloaded.sum, downloaded,
        "the bytes downloaded must be the bytes the edge served for the reading methods \
         ({downloaded}), got {bytes_downloaded:?}"
    );
    let bytes_uploaded = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::BYTES_UPLOADED_METRIC,
        at,
    )
    .await;
    assert_eq!(
        bytes_uploaded.sum, 0.0,
        "no row uploaded a body, so the counter must stand at 0, got {bytes_uploaded:?}"
    );

    // The OTHER distribution: one viewer request, answered by CloudFront itself because the
    // origin cannot be reached — a 5xx, counted against that distribution and against no
    // other.
    let (status, _bytes) = view(&router, "GET", &unserved_domain, "/anything").await;
    assert_eq!(
        status, 502,
        "the unreachable origin must be reported as CloudFront reports one, got {status}"
    );
    let other_five_xx = held(
        &router,
        "the unreachable origin",
        &unserved_id,
        edge_metrics::FIVE_XX_ERROR_RATE_METRIC,
        at,
    )
    .await;
    assert!(
        (other_five_xx.average() - 100.0).abs() < f64::EPSILON,
        "the one request of this distribution was answered 5xx, so its rate must be 100, got \
         {other_five_xx:?}"
    );
    let other_counted = held(
        &router,
        "the unreachable origin",
        &unserved_id,
        edge_metrics::REQUESTS_METRIC,
        at,
    )
    .await;
    assert_eq!(
        other_counted.sum, 1.0,
        "a distribution's series holds its OWN requests and no other's, got {other_counted:?}"
    );
    let still_counted = held(
        &router,
        "the served distribution",
        &served_id,
        edge_metrics::REQUESTS_METRIC,
        at,
    )
    .await;
    assert_eq!(
        still_counted.sum, requests,
        "the other distribution's request must not be counted here, got {still_counted:?}"
    );
}

#[tokio::test]
async fn the_published_series_are_the_per_distribution_ones_dimensioned_globally() {
    let router = emulator();
    let (id, domain) = created(&router, "edge-metrics-listed", SERVED_ORIGIN).await;
    ::tokio::time::sleep(past_propagation()).await;
    assert_deployed(&router, &id).await;

    // Before any viewer request there is nothing to report: the series exist because traffic
    // was served, not because a distribution was created.
    let before = cloudwatch(
        &router,
        "before any viewer request",
        "ListMetrics",
        &json!({"Namespace": edge_metrics::NAMESPACE}),
    )
    .await;
    assert_eq!(
        before["Metrics"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "a distribution nothing has viewed reports no series, got {before}"
    );

    let (status, _bytes) = view(&router, "GET", &domain, "/2020-05-31/distribution").await;
    assert_eq!(status, 200, "the origin must answer the warm-up request");

    let listed = cloudwatch(
        &router,
        "after one viewer request",
        "ListMetrics",
        &json!({"Namespace": edge_metrics::NAMESPACE}),
    )
    .await;
    let metrics = listed["Metrics"].as_array().cloned().unwrap_or_default();
    let mut names: Vec<String> = metrics
        .iter()
        .filter_map(|metric| metric["MetricName"].as_str().map(str::to_string))
        .collect();
    names.sort();
    let mut expected: Vec<String> = edge_metrics::DISTRIBUTION_METRICS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "one served request must create every per-distribution series, got {listed}"
    );
    for metric in &metrics {
        assert_eq!(
            metric["Dimensions"],
            dimensions(&id),
            "every series must carry the distribution and the Region it is reported for, got \
             {metric}"
        );
    }
}
