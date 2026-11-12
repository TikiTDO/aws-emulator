// SPDX-License-Identifier: CC0-1.0

//! The ONE page-size rule every `MaxResultsInteger`-paged listing of this service applies,
//! driven through each of the three operations that used to carry its own copy of it:
//! `ListAppMonitors`, `ListRumMetricsDestinations` and `BatchGetRumMetricDefinitions`.
//!
//! The rule under test: a listing paged by `MaxResults` reports at most as many entries as
//! the request asked for, and a request that names no `MaxResults` is paged at the size
//! that member's documentation gives as its default — the same size whichever of those
//! operations is asked, and a token whenever the page left something unreported.
//!
//! Each row of the table differs along the value the rule generalises over (the requested
//! page size, absent included, each published bound of the modelled range among them), and
//! each row is run against every one of the three listings, so the shared path is reached
//! from all three. The store behind each listing is larger than the documented default, so
//! a row cannot satisfy its assertion by returning everything there is.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    all_metric_definitions, call, create, emulator, experiment_arn, metrics_destination_query,
    put_metrics_destination,
};

/// The page every one of these listings returns when the request names no `MaxResults`.
/// From the model: each of `ListAppMonitorsRequest$MaxResults`,
/// `ListRumMetricsDestinationsRequest$MaxResults` and
/// `BatchGetRumMetricDefinitionsRequest$MaxResults` documents "The maximum number of
/// results to return in one operation. The default is 50. The maximum that you can specify
/// is 100."
/// Source: `smithy_query(query="shape", shape="ListAppMonitorsRequest,
/// ListRumMetricsDestinationsRequest,BatchGetRumMetricDefinitionsRequest")`.
const DOCUMENTED_DEFAULT: usize = 50;

/// The largest page the same member admits: all three target `MaxResultsInteger`, modelled
/// `@range(min: 1, max: 100)`.
/// Source: `smithy_query(query="shape", shape="MaxResultsInteger")`.
const MODELLED_MAXIMUM: usize = 100;

/// How many entries each listing is seeded with: more than the documented default, so a
/// default-sized page is a strict prefix of the store, and under the lowest per-parent
/// ceiling involved (a monitor holds at most 100 metrics destinations).
const STORE_SIZE: usize = 60;

/// The `MetricDestination` a definition is created under here.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";

/// The other `MetricDestination` value, which carries the ARN of the Evidently experiment
/// it names — the member that makes a monitor's destinations distinct from one another.
const EVIDENTLY: &str = "Evidently";

/// One paginated listing, with what it takes to ask it for a page.
#[derive(Debug)]
enum Listing {
    /// `ListAppMonitors`: `POST /appmonitors`, over the account's monitors.
    AppMonitors,
    /// `ListRumMetricsDestinations`: `GET /rummetrics/{AppMonitorName}/metricsdestination`,
    /// over one monitor's destinations.
    MetricsDestinations { monitor: String },
    /// `BatchGetRumMetricDefinitions`: `GET /rummetrics/{AppMonitorName}/metrics`, over one
    /// destination's definitions.
    MetricDefinitions { monitor: String },
}

/// What one page of a listing reported: how many entries, and whether it handed back a
/// continuation token.
#[derive(Debug)]
struct Page {
    entries: usize,
    has_token: bool,
}

impl Listing {
    /// The operation this listing is, for a case label.
    fn operation(&self) -> &'static str {
        match self {
            Listing::AppMonitors => "ListAppMonitors",
            Listing::MetricsDestinations { .. } => "ListRumMetricsDestinations",
            Listing::MetricDefinitions { .. } => "BatchGetRumMetricDefinitions",
        }
    }

    /// Ask this listing for its first page, with `max_results` bound to `?maxResults=` when
    /// the row names one.
    async fn first_page(
        &self,
        router: &MultiServiceRouter,
        case: &str,
        max_results: Option<usize>,
    ) -> Page {
        let (method, path, items) = match self {
            Listing::AppMonitors => ("POST", "/appmonitors".to_string(), "AppMonitorSummaries"),
            Listing::MetricsDestinations { monitor } => (
                "GET",
                format!("/rummetrics/{monitor}/metricsdestination"),
                "Destinations",
            ),
            Listing::MetricDefinitions { monitor } => (
                "GET",
                format!("/rummetrics/{monitor}/metrics"),
                "MetricDefinitions",
            ),
        };
        let mut parameters: Vec<String> = Vec::new();
        if matches!(self, Listing::MetricDefinitions { .. }) {
            parameters.push(metrics_destination_query(CLOUD_WATCH, None));
        }
        if let Some(size) = max_results {
            parameters.push(format!("maxResults={size}"));
        }
        let query = match parameters.is_empty() {
            true => String::new(),
            false => format!("?{}", parameters.join("&")),
        };

        let reply = call(router, method, &format!("{path}{query}"), None).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected {} to answer 200, got {} {:?}",
            self.operation(),
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        let entries = body[items].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: {} reported no {items}: {body}", self.operation())
        });
        Page {
            entries: entries.len(),
            has_token: body["NextToken"].as_str().is_some(),
        }
    }
}

/// Create one app monitor.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
}

/// An emulator holding `STORE_SIZE` app monitors, and the listing over them.
///
/// Each listing gets an emulator of its OWN, because `ListAppMonitors` reports the
/// account's whole collection: a monitor another listing's store needs would otherwise be
/// part of this one's.
async fn seeded_app_monitors() -> (MultiServiceRouter, Listing) {
    let (router, _engine) = emulator();
    for index in 0..STORE_SIZE {
        create_monitor(&router, "seed monitors", &format!("shared-page-{index:03}")).await;
    }
    (router, Listing::AppMonitors)
}

/// An emulator holding one monitor with `STORE_SIZE` metrics destinations — the CloudWatch
/// one plus an Evidently one per distinct experiment ARN — and the listing over them.
async fn seeded_metrics_destinations() -> (MultiServiceRouter, Listing) {
    let (router, _engine) = emulator();
    let monitor = "shared-page-destinations".to_string();
    create_monitor(&router, "seed destinations", &monitor).await;
    put_metrics_destination(&router, "seed destinations", &monitor, CLOUD_WATCH, None).await;
    for index in 1..STORE_SIZE {
        let arn = experiment_arn(&format!("shared-page-{index:03}"));
        put_metrics_destination(
            &router,
            "seed destinations",
            &monitor,
            EVIDENTLY,
            Some(&arn),
        )
        .await;
    }
    (router, Listing::MetricsDestinations { monitor })
}

/// An emulator holding one monitor whose CloudWatch destination holds `STORE_SIZE` custom
/// metric definitions, and the listing over them.
async fn seeded_metric_definitions() -> (MultiServiceRouter, Listing) {
    let (router, _engine) = emulator();
    let monitor = "shared-page-definitions".to_string();
    let case = "seed definitions";
    create_monitor(&router, case, &monitor).await;
    put_metrics_destination(&router, case, &monitor, CLOUD_WATCH, None).await;

    let definitions: Vec<Value> = (0..STORE_SIZE)
        .map(|index| json!({"Name": format!("SharedPage{index:03}"), "Namespace": "MyApp/Paging"}))
        .collect();
    let reply = call(
        &router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({"Destination": CLOUD_WATCH, "MetricDefinitions": definitions})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {case:?}: expected no per-definition errors, got {:?}",
        answered["Errors"]
    );
    // The create echoes one definition per entry it stored; what it stored is then counted
    // through `BatchGetRumMetricDefinitions` at the modelled maximum page size.
    assert_eq!(
        answered["MetricDefinitions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        STORE_SIZE,
        "case {case:?}: expected the create to echo {STORE_SIZE} definitions, got {answered}"
    );
    let created = all_metric_definitions(&router, case, &monitor, CLOUD_WATCH, None, Some(100))
        .await
        .len();
    assert_eq!(
        created, STORE_SIZE,
        "case {case:?}: expected {STORE_SIZE} definitions to be created, got {created}"
    );
    (router, Listing::MetricDefinitions { monitor })
}

#[tokio::test]
async fn every_max_results_paged_listing_applies_the_same_page_size_rule() {
    // One store per listing, each larger than the documented default.
    let listings = [
        seeded_app_monitors().await,
        seeded_metrics_destinations().await,
        seeded_metric_definitions().await,
    ];

    // The table: an absent `MaxResults`, the modelled minimum, a value between, both sides
    // of the documented default, the default itself, and the modelled maximum — which here
    // exceeds the store, so its page is the whole of it.
    for requested in [
        None,
        Some(1_usize),
        Some(7),
        Some(DOCUMENTED_DEFAULT - 1),
        Some(DOCUMENTED_DEFAULT),
        Some(DOCUMENTED_DEFAULT + 1),
        Some(MODELLED_MAXIMUM),
    ] {
        let expected = requested.unwrap_or(DOCUMENTED_DEFAULT).min(STORE_SIZE);
        for (router, listing) in &listings {
            let case = format!("{} with maxResults={requested:?}", listing.operation());
            let page = listing.first_page(router, &case, requested).await;
            assert_eq!(
                page.entries, expected,
                "case {case:?}: expected a page of {expected} entries out of {STORE_SIZE} \
                 stored, got {}",
                page.entries
            );
            assert_eq!(
                page.has_token,
                expected < STORE_SIZE,
                "case {case:?}: a page of {} out of {STORE_SIZE} stored entries must carry \
                 a NextToken exactly when something is left, got has_token={}",
                page.entries,
                page.has_token
            );
        }
    }
}
