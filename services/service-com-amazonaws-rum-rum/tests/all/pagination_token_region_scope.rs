// SPDX-License-Identifier: CC0-1.0

//! The REGIONAL scope of a continuation token this service issues.
//!
//! The rule under test: a `NextToken` is only honoured by the listing that issued it, and a
//! listing of this service walks the resources of the region its request was signed for —
//! a resource of this service belongs to exactly one region and its name is only unique
//! within it — so a token presented at any OTHER regional endpoint names a position in a
//! collection that endpoint's listing never walked, and is refused with the modelled
//! `ValidationException` (HTTP 400) naming it rather than read as a resume point. Honouring
//! it would drop every entry ordered before that position from the walk, reported with a
//! 200 and no refusal.
//!
//! Every case drives the real wire path against ONE emulator, so a request signed for one
//! region meets the same store as a request signed for another and the cross-region resume
//! lands on a real, independently seeded collection (a case that built an emulator per
//! region could not see it).
//!
//! The table spans what the rule generalises over:
//!
//! * WHICH reader is asked — all four paginated readers of this service, whose tokens
//!   differ in marker, in arity, in what the key resumes after (a name, a destination pair,
//!   a definition id, a count of events already returned), in whether the request addresses
//!   one app monitor or the account's collection, and in where `NextToken` is bound (the
//!   query string or the request body);
//! * WHICH pair of regions — every ordered pair of a catalog of three, so each reader's
//!   token is presented at every endpoint that did not issue it; and
//! * the entries themselves — each region is seeded with the SAME names, which is the state
//!   in which a cross-region resume lands on a different collection holding keys in common.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call_in, emulator, is_modelled_app_monitor_id, message, metrics_destination_query,
    metrics_role_arn, percent_encoded, Reply,
};

/// The regions the rule is asserted over: every reader's store is seeded in each, and each
/// endpoint's token is presented at both of the others. Any `credentialScope` region an
/// endpoint can be signed for is admissible — the service reads the region off the SigV4
/// credential scope — so the catalog spans distant geographies rather than one pair.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-2", "ap-southeast-1"];

/// How many entries each listing is seeded with per region, asked for one at a time: more
/// than one, so a first page always leaves something to resume and therefore issues a
/// token, and more than two, so a resume that skipped a page would still answer entries.
const STORE_SIZE: usize = 3;

/// The `MetricDestination` value that carries no `DestinationArn`.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — an enum of
/// `CloudWatch` and `Evidently`.
const CLOUD_WATCH: &str = "CloudWatch";

/// The other `MetricDestination` value, which names the Evidently experiment it writes to.
const EVIDENTLY: &str = "Evidently";

/// The app monitor every monitor-addressed reader addresses — the SAME name in every
/// region, so the monitor a cross-region token's key names really exists at the endpoint it
/// is replayed against.
const MONITOR: &str = "region-scope";

/// `&nextToken=<token>` for a reader that binds `NextToken` to the query string.
fn token_query(token: Option<&str>) -> String {
    match token {
        Some(token) => format!("&nextToken={}", percent_encoded(token)),
        None => String::new(),
    }
}

/// The ARN of an emulated Evidently experiment of `region` — the resource an `Evidently`
/// extended-metrics destination names.
fn experiment_arn_in(region: &str, experiment: &str) -> String {
    engine::emulator_arn("evidently", region, &format!("experiment/{experiment}"))
}

/// `CreateAppMonitor` signed for `region`, returning the minted `Id`.
async fn create_monitor(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> String {
    let reply = call_in(
        router,
        region,
        "POST",
        "/appmonitor",
        Some(json!({"Name": name, "Domain": "example.com"})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor signed for {region} to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let id = reply.json(case)["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor returned no Id"))
        .to_string();
    assert!(
        is_modelled_app_monitor_id(&id),
        "case {case:?}: expected a modelled AppMonitorId, got {id:?}"
    );
    id
}

/// `PutRumMetricsDestination` signed for `region`.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) {
    let mut body = json!({"Destination": destination});
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
        body["IamRoleArn"] = json!(metrics_role_arn("rum-writer"));
    }
    let reply = call_in(
        router,
        region,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumMetricsDestination signed for {region} to answer 200, got \
         {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `BatchCreateRumMetricDefinitions` signed for `region`, creating one custom definition per
/// name under the monitor's CloudWatch destination.
async fn create_definitions(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    monitor: &str,
    names: &BTreeSet<String>,
) {
    let definitions: Vec<Value> = names
        .iter()
        .map(|name| json!({"Name": name, "Namespace": "MyApp/RegionScope"}))
        .collect();
    let reply = call_in(
        router,
        region,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({
            "Destination": CLOUD_WATCH,
            "MetricDefinitions": Value::Array(definitions),
        })),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions signed for {region} to answer \
         200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.json(case)["Errors"],
        json!([]),
        "case {case:?}: expected no per-definition errors, got {:?}",
        reply.json(case)["Errors"]
    );
}

/// One `RumEvent` id, in the modelled 36-character UUID shape
/// (`^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`, per
/// `smithy_query(query="shape", shape="RumEvent")`), distinct per region and per event so a
/// walk's entries name the region that collected them.
fn event_id(region_index: usize, event_index: usize) -> String {
    format!("{region_index:08x}-1234-4234-8234-{event_index:012x}")
}

/// `PutRumEvents` signed for `region`, delivering one event per id to the monitor `id`
/// names.
async fn put_events(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    region_index: usize,
    monitor_id: &str,
    event_ids: &BTreeSet<String>,
) {
    let events: Vec<Value> = event_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            json!({
                "id": id,
                "timestamp": 1_704_067_200_i64 + index as i64,
                "type": "com.amazon.rum.page_view_event",
                "details": json!({"pageId": format!("/{region}/{index}")}).to_string(),
            })
        })
        .collect();
    let body = json!({
        // One batch per region, under an id of its own, so no two regions' deliveries look
        // like the same batch.
        "BatchId": format!("{region_index:08x}-2222-4333-8444-555555555555"),
        "AppMonitorDetails": {"name": MONITOR, "id": monitor_id, "version": "1.0.0"},
        "UserDetails": {
            "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
        },
        "RumEvents": Value::Array(events),
    });
    let reply = call_in(
        router,
        region,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumEvents signed for {region} to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// One paginated reader of this service.
#[derive(Clone, Copy, Debug)]
enum Reader {
    /// `ListAppMonitors`: the account's collection in the signed region, token in the query
    /// string.
    AppMonitors,
    /// `ListRumMetricsDestinations`: one monitor's destinations, token in the query string.
    MetricsDestinations,
    /// `BatchGetRumMetricDefinitions`: one destination's definitions, token in the query
    /// string.
    MetricDefinitions,
    /// `GetAppMonitorData`: one monitor's collected events, token in the request BODY.
    AppMonitorData,
}

/// Every paginated reader of this service.
fn readers() -> [Reader; 4] {
    [
        Reader::AppMonitors,
        Reader::MetricsDestinations,
        Reader::MetricDefinitions,
        Reader::AppMonitorData,
    ]
}

impl Reader {
    /// The operation, named in every assertion.
    fn operation(self) -> &'static str {
        match self {
            Reader::AppMonitors => "ListAppMonitors",
            Reader::MetricsDestinations => "ListRumMetricsDestinations",
            Reader::MetricDefinitions => "BatchGetRumMetricDefinitions",
            Reader::AppMonitorData => "GetAppMonitorData",
        }
    }

    /// Its HTTP method.
    fn method(self) -> &'static str {
        match self {
            Reader::MetricsDestinations | Reader::MetricDefinitions => "GET",
            Reader::AppMonitors | Reader::AppMonitorData => "POST",
        }
    }

    /// Its URI for a page of one entry, carrying the token where the operation binds
    /// `NextToken` to the query string.
    fn uri(self, token: Option<&str>) -> String {
        match self {
            Reader::AppMonitors => format!("/appmonitors?maxResults=1{}", token_query(token)),
            Reader::MetricsDestinations => format!(
                "/rummetrics/{MONITOR}/metricsdestination?maxResults=1{}",
                token_query(token)
            ),
            Reader::MetricDefinitions => format!(
                "/rummetrics/{MONITOR}/metrics?{}&maxResults=1{}",
                metrics_destination_query(CLOUD_WATCH, None),
                token_query(token)
            ),
            Reader::AppMonitorData => format!("/appmonitor/{MONITOR}/data"),
        }
    }

    /// Its body, carrying the token where the operation binds `NextToken` to the body.
    fn body(self, token: Option<&str>) -> Option<Value> {
        match self {
            Reader::AppMonitorData => {
                let mut body = json!({"TimeRange": {"After": 0}, "MaxResults": 1});
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                Some(body)
            }
            _ => None,
        }
    }

    /// The entries one page reports, each as the value that identifies it within its own
    /// region's collection.
    fn entries(self, case: &str, page: &Value) -> Vec<String> {
        let member = match self {
            Reader::AppMonitors => "AppMonitorSummaries",
            Reader::MetricsDestinations => "Destinations",
            Reader::MetricDefinitions => "MetricDefinitions",
            Reader::AppMonitorData => "Events",
        };
        let reported = page[member].as_array().cloned().unwrap_or_default();
        reported
            .iter()
            .map(|entry| match self {
                Reader::AppMonitors | Reader::MetricDefinitions => entry["Name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: {entry} carries no Name"))
                    .to_string(),
                Reader::MetricsDestinations => format!(
                    "{}|{}",
                    entry["Destination"].as_str().unwrap_or_else(|| panic!(
                        "case {case:?}: {entry} carries no Destination"
                    )),
                    entry["DestinationArn"].as_str().unwrap_or_default()
                ),
                Reader::AppMonitorData => {
                    let text = entry
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: {entry} is not an event string"));
                    let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                        panic!("case {case:?}: expected an event document, got {text:?} ({error})")
                    });
                    document["event_id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: {text} carries no event_id"))
                        .to_string()
                }
            })
            .collect()
    }

    /// Seed this reader's store in `region` and report the entries that region's walk must
    /// then report.
    async fn seed(
        self,
        router: &MultiServiceRouter,
        region: &str,
        region_index: usize,
    ) -> BTreeSet<String> {
        let case = format!("seed {} in {region}", self.operation());
        match self {
            Reader::AppMonitors => {
                // The SAME names in every region, so a listing that resumed another
                // region's token would be skipping names it must report rather than merely
                // reporting fewer rows.
                let mut names = BTreeSet::new();
                for index in 0..STORE_SIZE {
                    let name = format!("{MONITOR}-{index}");
                    create_monitor(router, &case, region, &name).await;
                    names.insert(name);
                }
                names
            }
            Reader::MetricsDestinations => {
                create_monitor(router, &case, region, MONITOR).await;
                // The CloudWatch destination, which carries no ARN, plus one Evidently
                // destination per experiment of the region.
                let mut entries = BTreeSet::new();
                put_destination(router, &case, region, MONITOR, CLOUD_WATCH, None).await;
                entries.insert(format!("{CLOUD_WATCH}|"));
                for index in 1..STORE_SIZE {
                    let arn = experiment_arn_in(region, &format!("{MONITOR}-{index}"));
                    put_destination(router, &case, region, MONITOR, EVIDENTLY, Some(&arn)).await;
                    entries.insert(format!("{EVIDENTLY}|{arn}"));
                }
                entries
            }
            Reader::MetricDefinitions => {
                create_monitor(router, &case, region, MONITOR).await;
                put_destination(router, &case, region, MONITOR, CLOUD_WATCH, None).await;
                let names: BTreeSet<String> = (0..STORE_SIZE)
                    .map(|index| format!("RegionScope{index}"))
                    .collect();
                create_definitions(router, &case, region, MONITOR, &names).await;
                names
            }
            Reader::AppMonitorData => {
                let id = create_monitor(router, &case, region, MONITOR).await;
                let event_ids: BTreeSet<String> = (0..STORE_SIZE)
                    .map(|index| event_id(region_index, index))
                    .collect();
                put_events(router, &case, region, region_index, &id, &event_ids).await;
                event_ids
            }
        }
    }

    /// Ask this reader for a page of one entry at `region`'s endpoint, resuming after
    /// `token` when the case carries one.
    async fn page(
        self,
        router: &MultiServiceRouter,
        region: &str,
        token: Option<&str>,
    ) -> Reply {
        call_in(
            router,
            region,
            self.method(),
            &self.uri(token),
            self.body(token),
        )
        .await
    }

    /// The `NextToken` this reader's FIRST page at `region`'s endpoint issues over that
    /// region's seeded store.
    async fn issued_token(self, router: &MultiServiceRouter, region: &str) -> String {
        let case = format!("{} issues at {region}", self.operation());
        let reply = self.page(router, region, None).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the first page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(&case);
        page["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a NextToken while {} entries remain, got {page}",
                    STORE_SIZE - 1
                )
            })
            .to_string()
    }

    /// Everything this reader reports at `region`'s endpoint, a page of one at a time,
    /// following its own tokens to the end.
    async fn walk(self, router: &MultiServiceRouter, case: &str, region: &str) -> Vec<String> {
        let mut reported = Vec::new();
        let mut token: Option<String> = None;
        let mut seen_tokens = BTreeSet::new();
        loop {
            let reply = self.page(router, region, token.as_deref()).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected a page to answer 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let page = reply.json(case);
            reported.extend(self.entries(case, &page));
            match page["NextToken"].as_str() {
                Some(issued) => {
                    assert!(
                        seen_tokens.insert(issued.to_string()),
                        "case {case:?}: token {issued:?} was issued twice, so paging does not \
                         advance"
                    );
                    assert!(
                        reported.len() <= STORE_SIZE,
                        "case {case:?}: paging reported more than the {STORE_SIZE} seeded \
                         entries: {reported:?}"
                    );
                    token = Some(issued.to_string());
                }
                None => break,
            }
        }
        reported
    }
}

/// A token one regional endpoint's page issued is a resume point for THAT endpoint's
/// listing and for no other: presented at any other region's endpoint it is refused as a
/// token that listing never issued, for every paginated reader of this service and every
/// ordered pair of regions.
#[tokio::test]
async fn a_token_is_only_honoured_at_the_regional_endpoint_that_issued_it() {
    for reader in readers() {
        // One emulator per reader, because `ListAppMonitors` reports the account's whole
        // collection: the monitors another reader's store needs would otherwise be part of
        // this one's.
        let (router, _engine) = emulator();
        for (index, region) in REGIONS.into_iter().enumerate() {
            reader.seed(&router, region, index).await;
        }

        for issuing in REGIONS {
            let issued = reader.issued_token(&router, issuing).await;

            // The control row: at the endpoint that issued it, the token resumes the
            // listing — so the refusals below are about the region and not about the token.
            let control = format!("{} resumes at {issuing}", reader.operation());
            let resumed = reader.page(&router, issuing, Some(&issued)).await;
            assert_eq!(
                resumed.status,
                200,
                "case {control:?}: expected the issued token to resume the listing, got {} {:?}",
                resumed.status,
                String::from_utf8_lossy(&resumed.body)
            );

            for other in REGIONS.into_iter().filter(|region| *region != issuing) {
                let case = format!(
                    "{}: the token {issued} issued at {issuing}, presented at {other}",
                    reader.operation()
                );
                let refused = reader.page(&router, other, Some(&issued)).await;
                assert_eq!(
                    refused.status,
                    400,
                    "case {case:?}: expected HTTP 400 for a token this endpoint's listing never \
                     issued, got {} {:?}",
                    refused.status,
                    String::from_utf8_lossy(&refused.body)
                );
                let code = refused.error_code(&case);
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {code:?}"
                );
                let body = refused.json(&case);
                let stated = message(&body).unwrap_or_else(|| {
                    panic!("case {case:?}: expected the refusal to state a message, got {body}")
                });
                assert!(
                    stated.contains(&issued),
                    "case {case:?}: expected the refusal to name the token {issued:?}, got \
                     {stated:?}"
                );
            }
        }
    }
}

/// And the walk each regional endpoint does over its own store reports exactly the entries
/// that region holds — every one of them, once: the refusal above costs no entry, and no
/// other region's entries are reachable through it.
#[tokio::test]
async fn each_regional_endpoint_pages_over_exactly_its_own_entries() {
    for reader in readers() {
        let (router, _engine) = emulator();
        let mut seeded: Vec<(&str, BTreeSet<String>)> = Vec::new();
        for (index, region) in REGIONS.into_iter().enumerate() {
            seeded.push((region, reader.seed(&router, region, index).await));
        }

        for (region, expected) in &seeded {
            let case = format!("{} walks {region}", reader.operation());
            let reported = reader.walk(&router, &case, region).await;
            let unique: BTreeSet<String> = reported.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                reported.len(),
                "case {case:?}: paging reported an entry twice: {reported:?}"
            );
            assert_eq!(
                &unique, expected,
                "case {case:?}: expected exactly the entries seeded in {region}, got {unique:?}"
            );
        }
    }
}
