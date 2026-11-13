// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's extended and custom metric definitions:
//! `UpdateRumMetricDefinition`, with `BatchCreateRumMetricDefinitions` creating what it
//! updates and the paginated `BatchGetRumMetricDefinitions` reading it back.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings of the read, the modelled constraints, the handler and the response envelope
//! are all exercised rather than just a handler body. An update is asserted through the
//! read-back, never through its own answer: `UpdateRumMetricDefinitionResponse` models no
//! member at all, so the PERSISTED definition is the only evidence the call did anything.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the
//! create/read-back table spans both `MetricDestination` values, one and several
//! definitions per call, and a definition carrying every optional member down to a custom
//! metric carrying only the members it must; the catalog test walks every extended metric
//! name the model pins a combination for and every dimension field path it lists; the
//! update table varies which members the replacement adds, changes and drops, and runs
//! each row against both destination kinds; the refusal table walks one refused target or
//! one refused definition per row and then shows the stored definition unchanged; the
//! namespace table walks both sides of the reserved-prefix boundary; the pagination test is
//! an invariant check over a grid of store sizes and page sizes; the `MaxResults` table
//! walks each published bound of the modelled range plus one past each, beside the
//! documented default page size; and the re-send table re-sends a batch of definitions
//! over both destination kinds, batch sizes and member combinations, beside a batch that
//! repeats and adds, a definition differing in one member, and one identity named twice in
//! a single batch.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    assert_definition_conflicts, assert_reports_no_created_definitions, call,
    canonical_metric_definition, create, emulator, message, percent_encoded, Reply, REGION,
};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// One extended metric RUM defines: its name, the `ValueKey` the model pins for it (`None`
/// where it says "`ValueKey` must be null") and the JSON object the model requires its
/// `EventPattern` to include.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Extended {
    pub(crate) name: &'static str,
    pub(crate) value_key: Option<&'static str>,
    pub(crate) event_pattern: &'static str,
}

/// Every extended metric the model pins a `Name`/`ValueKey`/`EventPattern` combination
/// for, in the order it lists them.
/// Source: `smithy_query(query="doc", shape="MetricDefinitionRequest")` — one "If `Name` is
/// …, then `ValueKey` must be … and the `EventPattern` must include …" rule per name. The
/// comparison operators are written here as the characters they are (the documentation
/// carries them HTML-escaped), and the rows whose documented literal is missing a separator
/// or carries a stray closing brace are written as the JSON object those literals describe.
pub(crate) const EXTENDED_METRICS: [Extended; 14] = [
    Extended {
        name: "PerformanceNavigationDuration",
        value_key: Some("event_details.duration"),
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"]}"#,
    },
    Extended {
        name: "PerformanceResourceDuration",
        value_key: Some("event_details.duration"),
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_resource_event"]}"#,
    },
    Extended {
        name: "NavigationSatisfiedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">",2000]}]}}"#,
    },
    Extended {
        name: "NavigationToleratedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">=",2000,"<",8000]}]}}"#,
    },
    Extended {
        name: "NavigationFrustratedTransaction",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.performance_navigation_event"],"event_details":{"duration":[{"numeric":[">=",8000]}]}}"#,
    },
    Extended {
        name: "WebVitalsCumulativeLayoutShift",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.cumulative_layout_shift_event"]}"#,
    },
    Extended {
        name: "WebVitalsFirstInputDelay",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.first_input_delay_event"]}"#,
    },
    Extended {
        name: "WebVitalsLargestContentfulPaint",
        value_key: Some("event_details.value"),
        event_pattern: r#"{"event_type":["com.amazon.rum.largest_contentful_paint_event"]}"#,
    },
    Extended {
        name: "JsErrorCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.js_error_event"]}"#,
    },
    Extended {
        name: "HttpErrorCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"]}"#,
    },
    Extended {
        name: "SessionCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.session_start_event"]}"#,
    },
    Extended {
        name: "PageViewCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.page_view_event"]}"#,
    },
    Extended {
        name: "Http4xxCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"],"event_details":{"response":{"status":[{"numeric":[">=",400,"<",500]}]}}}"#,
    },
    Extended {
        name: "Http5xxCount",
        value_key: None,
        event_pattern: r#"{"event_type":["com.amazon.rum.http_event"],"event_details":{"response":{"status":[{"numeric":[">=",500,"<=",599]}]}}}"#,
    },
];

/// Every additional dimension an extended metric may carry, as the model lists them: the
/// event field path, and the CloudWatch dimension name it maps to.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest")`, member
/// `DimensionKeys`.
pub(crate) const EXTENDED_DIMENSION_KEYS: [(&str, &str); 6] = [
    ("metadata.pageId", "PageId"),
    ("metadata.browserName", "BrowserName"),
    ("metadata.deviceType", "DeviceType"),
    ("metadata.osName", "OSName"),
    ("metadata.countryCode", "CountryCode"),
    ("event_details.fileType", "FileType"),
];

/// The members a `MetricDefinitionRequest` carries, which are also the members the
/// `MetricDefinition` a read reports carries beside its id.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest,MetricDefinition")`.
const DEFINITION_MEMBERS: [&str; 6] = [
    "Name",
    "ValueKey",
    "UnitLabel",
    "DimensionKeys",
    "EventPattern",
    "Namespace",
];

/// `MetricDefinitionId`'s modelled length: 1..255 characters, and no pattern at all, so
/// the only thing a client may rely on is that the service hands back an id in that range
/// and keeps naming the same definition by it.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionId")`.
const METRIC_DEFINITION_ID_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// The ARN of an emulated Evidently experiment, the resource an `Evidently` destination
/// names. Built with the shared `engine::emulator_arn` helper, so it is a canonical ARN
/// rather than a hand-spelled string.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The ARN of an emulated IAM role, the role RUM assumes to write to an experiment. IAM is
/// global, so the region segment is empty.
fn role_arn(role: &str) -> String {
    engine::emulator_arn("iam", "", &format!("role/{role}"))
}

/// One extended-metrics destination: the `(Destination, DestinationArn)` pair that
/// identifies it, plus the role an Evidently destination is written through.
#[derive(Debug, Clone)]
struct Destination {
    destination: String,
    destination_arn: Option<String>,
    iam_role_arn: Option<String>,
}

impl Destination {
    /// The `CloudWatch` destination: the model says it carries neither ARN.
    fn cloud_watch() -> Self {
        Self {
            destination: METRIC_DESTINATIONS[0].to_string(),
            destination_arn: None,
            iam_role_arn: None,
        }
    }

    /// An `Evidently` destination: the experiment that receives the metrics plus the role
    /// RUM writes to it through.
    fn evidently(experiment: &str) -> Self {
        Self {
            destination: METRIC_DESTINATIONS[1].to_string(),
            destination_arn: Some(experiment_arn(experiment)),
            iam_role_arn: Some(role_arn(&format!("{experiment}-writer"))),
        }
    }

    /// This destination as the `PutRumMetricsDestination` body that creates it: a member
    /// the destination does not have is left out of the request entirely.
    fn put_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
        }
        if let Some(arn) = &self.iam_role_arn {
            body["IamRoleArn"] = json!(arn);
        }
        body
    }

    /// The `Destination`/`DestinationArn` members a metric-definition request names this
    /// destination by, as the JSON body of a create or an update.
    fn addressed_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
        }
        body
    }

    /// The same pair as the query string the read binds them to
    /// (`@httpQuery("destination")` / `@httpQuery("destinationArn")`).
    fn query(&self) -> String {
        let mut query = format!("destination={}", percent_encoded(&self.destination));
        if let Some(arn) = &self.destination_arn {
            query.push_str(&format!("&destinationArn={}", percent_encoded(arn)));
        }
        query
    }

    /// A fragment of an app-monitor name, so each row of a table gets its own monitor.
    /// `AppMonitorName` admits letters, digits and `-`, so the experiment's last segment
    /// is used rather than the whole ARN.
    fn slug(&self) -> String {
        match &self.destination_arn {
            Some(arn) => format!(
                "{}-{}",
                self.destination,
                arn.rsplit('/').next().unwrap_or("experiment")
            ),
            None => self.destination.clone(),
        }
    }
}

/// An event pattern that matches events of `event_type` and names each of `fields` — the
/// dotted field paths a `DimensionKeys` map keys dimensions by — so the pattern includes
/// every dimension the definition lists, as the model requires.
pub(crate) fn event_pattern(event_type: &str, fields: &[&str]) -> String {
    naming_fields(json!({"event_type": [event_type]}), fields)
}

/// `pattern`, rendered as the `EventPattern` string a request carries, with each of
/// `fields` named in it.
fn naming_fields(mut pattern: Value, fields: &[&str]) -> String {
    for field in fields {
        let mut segments: Vec<&str> = field.split('.').collect();
        let leaf = segments.pop().expect("a field path has a last segment");
        let mut cursor = &mut pattern;
        for segment in segments {
            cursor = cursor
                .as_object_mut()
                .expect("an event pattern is a JSON object")
                .entry((*segment).to_string())
                .or_insert_with(|| json!({}));
        }
        cursor[leaf] = json!(["any"]);
    }
    pattern.to_string()
}

/// A `DimensionKeys` map built from the model's catalog entries.
pub(crate) fn dimension_keys(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(field, dimension)| ((*field).to_string(), json!(dimension)))
            .collect(),
    )
}

/// The catalog row the model pins for the extended metric `name`.
fn catalog_row(name: &str) -> &'static Extended {
    EXTENDED_METRICS
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {name:?}: the model lists no extended metric by that name"))
}

/// The definition the model pins for the extended metric `name`: that name, the `ValueKey`
/// its catalog row requires and an `EventPattern` carrying the object that row requires.
pub(crate) fn extended(name: &str) -> Value {
    extended_with_dimensions(name, &[])
}

/// The same, with `dimensions` listed in `DimensionKeys` and their field paths named by the
/// `EventPattern` — which is exactly what the model allows beyond the required object: "the
/// EventPattern can also include information used by the DimensionKeys field".
pub(crate) fn extended_with_dimensions(name: &str, dimensions: &[(&str, &str)]) -> Value {
    let entry = catalog_row(name);
    let fields: Vec<&str> = dimensions.iter().map(|(field, _)| *field).collect();
    let pattern = naming_fields(
        serde_json::from_str(entry.event_pattern).expect("a catalog pattern is a JSON object"),
        &fields,
    );
    let mut definition = json!({"Name": entry.name, "EventPattern": pattern});
    if let Some(value_key) = entry.value_key {
        definition["ValueKey"] = json!(value_key);
    }
    if !dimensions.is_empty() {
        definition["DimensionKeys"] = dimension_keys(dimensions);
    }
    definition
}

/// `definition` with `member` set to `value`, so a row states one difference from the
/// definition it varies.
pub(crate) fn with(mut definition: Value, member: &str, value: Value) -> Value {
    definition[member] = value;
    definition
}

/// A custom metric: a name the extended catalog does not list, published to the namespace
/// `namespace` — which the model requires a custom metric to name.
pub(crate) fn custom(name: &str, namespace: &str) -> Value {
    json!({"Name": name, "Namespace": namespace})
}

/// Create one app monitor to hang destinations and definitions off.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
}

/// `PutRumMetricsDestination`, the call that has to have created a destination before any
/// metric definition can be created under it.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) {
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(destination.put_body()),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumMetricsDestination to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `BatchCreateRumMetricDefinitions`: `POST /rummetrics/{AppMonitorName}/metrics`.
async fn batch_create(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &Destination,
    definitions: &[Value],
) -> Reply {
    let mut body = destination.addressed_body();
    body["MetricDefinitions"] = Value::Array(definitions.to_vec());
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await
}

/// The definitions a successful `BatchCreateRumMetricDefinitions` created, in request
/// order, asserting it refused none of them.
///
/// The create's answer reports NONE of what it created
/// ([`crate::support::assert_reports_no_created_definitions`]), so the definitions are read
/// back through `BatchGetRumMetricDefinitions`, which is where their ids are learned.
async fn create_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
    definitions: &[Value],
) -> Vec<Value> {
    let reply = batch_create(router, monitor, destination, definitions).await;
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
    assert_reports_no_created_definitions(case, &answered);
    // Resolved by reading the destination back: a definition's identity is the definition
    // itself, so one requested entry resolves to exactly one stored definition.
    crate::support::held_metric_definitions(
        router,
        case,
        monitor,
        &destination.destination,
        destination.destination_arn.as_deref(),
        definitions,
    )
    .await
}

/// The definition the destination now holds for each of `definitions`, in request order —
/// the read-back that shows what a create stored, which the create's own answer reports
/// nothing about.
///
/// One definition of "the definition the destination holds for this request" for the whole
/// crate: the shared [`crate::support::held_metric_definitions`], which resolves each
/// requested definition in the canonical form the service stores one in, so a request
/// naming one definition twice resolves to that one definition twice.
async fn held_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
    definitions: &[Value],
) -> Vec<Value> {
    crate::support::held_metric_definitions(
        router,
        case,
        monitor,
        &destination.destination,
        destination.destination_arn.as_deref(),
        definitions,
    )
    .await
}

/// `UpdateRumMetricDefinition`: `PATCH /rummetrics/{AppMonitorName}/metrics`.
async fn update(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &Destination,
    metric_definition_id: &str,
    definition: &Value,
) -> Reply {
    let mut body = destination.addressed_body();
    body["MetricDefinitionId"] = json!(metric_definition_id);
    body["MetricDefinition"] = definition.clone();
    call(
        router,
        "PATCH",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await
}

/// An accepted `UpdateRumMetricDefinition`, asserting the answer the model declares: a 200
/// whose body carries no member, since `UpdateRumMetricDefinitionResponse` has none.
async fn update_ok(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
    metric_definition_id: &str,
    definition: &Value,
) {
    let reply = update(router, monitor, destination, metric_definition_id, definition).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected UpdateRumMetricDefinition to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = String::from_utf8_lossy(&reply.body).into_owned();
    assert!(
        body.is_empty() || body == "{}",
        "case {case:?}: expected the update's body to carry no member, got {body:?}"
    );
}

/// One page of `BatchGetRumMetricDefinitions`: `GET
/// /rummetrics/{AppMonitorName}/metrics?destination=…`, with `extra` carrying any
/// `maxResults`/`nextToken` the case binds.
async fn get_page(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &Destination,
    extra: &str,
) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/rummetrics/{monitor}/metrics?{}{extra}",
            destination.query()
        ),
        None,
    )
    .await
}

/// Every definition that destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions` and following its tokens to the end.
async fn all_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) -> Vec<Value> {
    let mut reported = Vec::new();
    let mut extra = String::new();
    loop {
        let reply = get_page(router, monitor, destination, &extra).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected BatchGetRumMetricDefinitions to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        reported.extend(
            page["MetricDefinitions"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a page reported no MetricDefinitions: {page}")
                })
                .clone(),
        );
        match page["NextToken"].as_str() {
            Some(token) => extra = format!("&nextToken={}", percent_encoded(token)),
            None => return reported,
        }
    }
}

/// The definitions a destination holds, by the id each is stored under.
async fn stored_by_id(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) -> BTreeMap<String, Value> {
    all_definitions(router, case, monitor, destination)
        .await
        .into_iter()
        .map(|definition| (id_of(case, &definition), definition))
        .collect()
}

/// The `MetricDefinitionId` of a reported definition.
fn id_of(case: &str, reported: &Value) -> String {
    reported["MetricDefinitionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a reported definition carries no MetricDefinitionId: {reported}")
        })
        .to_string()
}

/// Assert `reported` is exactly the definition `requested` asked for, under the id `id`:
/// every member of the form the service stores that definition in
/// ([`canonical_metric_definition`]) reported with its value, and every member it leaves
/// out absent (restJson1 draws no distinction between an absent member and a null one, so
/// an omitted member reads as `Null` either way).
fn assert_definition(case: &str, reported: &Value, requested: &Value, id: &str) {
    assert_eq!(
        reported["MetricDefinitionId"].as_str(),
        Some(id),
        "case {case:?}: expected the definition to be reported under {id:?}, got {reported}"
    );
    let canonical = canonical_metric_definition(requested);
    for member in DEFINITION_MEMBERS {
        let expected = &canonical[member];
        assert_eq!(
            &reported[member], expected,
            "case {case:?}: expected {member} to be {expected:?}, got {:?}",
            reported[member]
        );
    }
}

/// Every extended metric the model lists, each as the definition the model pins for it, so
/// the catalog is exercised in full rather than on the one name an example happens to use.
fn catalog_definitions() -> Vec<Value> {
    EXTENDED_METRICS
        .iter()
        .map(|entry| extended(entry.name))
        .collect()
}

#[tokio::test]
async fn created_definitions_are_read_back_under_the_ids_the_create_minted() {
    let (router, _engine) = emulator();

    let rows: Vec<(&str, Destination, Vec<Value>)> = vec![
        (
            "one extended metric, CloudWatch",
            Destination::cloud_watch(),
            vec![with(
                extended(EXTENDED_METRICS[0].name),
                "UnitLabel",
                json!("Milliseconds"),
            )],
        ),
        (
            "three definitions in one call, CloudWatch",
            Destination::cloud_watch(),
            vec![
                extended_with_dimensions(
                    EXTENDED_METRICS[8].name,
                    &EXTENDED_DIMENSION_KEYS[1..2],
                ),
                json!({
                    "Name": "CartCheckout",
                    "Namespace": "MyApp/Checkout",
                    "ValueKey": "event_details.total",
                    "UnitLabel": "Count",
                    "EventPattern": event_pattern("com.example.cart_checkout", &[]),
                }),
                extended(EXTENDED_METRICS[10].name),
            ],
        ),
        (
            "one custom metric, an Evidently experiment",
            Destination::evidently("alpha"),
            vec![json!({
                "Name": "ConversionRate",
                "Namespace": "MyApp/Experiments",
                "ValueKey": "event_details.converted",
                "EventPattern": event_pattern("com.example.conversion", &[]),
            })],
        ),
        (
            "two definitions, another Evidently experiment",
            Destination::evidently("beta"),
            vec![
                extended(EXTENDED_METRICS[9].name),
                extended(EXTENDED_METRICS[11].name),
            ],
        ),
        (
            "a custom metric carrying nothing but the members it must",
            Destination::cloud_watch(),
            vec![custom("BareCustom", "MyApp/Bare")],
        ),
    ];

    // Across every row: one id is minted per definition, and no id is ever handed out
    // twice, whichever monitor or destination the definition belongs to.
    let mut minted: HashSet<String> = HashSet::new();
    for (index, (case, destination, definitions)) in rows.into_iter().enumerate() {
        let monitor = format!("created-{index}-{}", destination.slug());
        create_monitor(&router, case, &monitor).await;
        put_destination(&router, case, &monitor, &destination).await;

        let created = create_definitions(&router, case, &monitor, &destination, &definitions).await;
        assert_eq!(
            created.len(),
            definitions.len(),
            "case {case:?}: expected {} created definitions, got {}",
            definitions.len(),
            created.len()
        );
        let stored = stored_by_id(&router, case, &monitor, &destination).await;
        assert_eq!(
            stored.len(),
            definitions.len(),
            "case {case:?}: expected the destination to hold {} definitions, got {}",
            definitions.len(),
            stored.len()
        );

        for (requested, reported) in definitions.iter().zip(created.iter()) {
            let id = id_of(case, reported);
            assert!(
                METRIC_DEFINITION_ID_LENGTH.contains(&id.len()),
                "case {case:?}: expected a minted id of 1..255 characters, got {id:?}"
            );
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: id {id:?} was minted twice"
            );
            assert_definition(case, reported, requested, &id);
            let read_back = stored.get(&id).unwrap_or_else(|| {
                panic!("case {case:?}: BatchGetRumMetricDefinitions reported no definition {id:?}")
            });
            assert_definition(case, read_back, requested, &id);
        }
    }
}

#[tokio::test]
async fn every_extended_metric_name_and_dimension_the_model_lists_round_trips() {
    let (router, _engine) = emulator();
    let destination = Destination::cloud_watch();
    let monitor = "catalog";
    create_monitor(&router, "catalog", monitor).await;
    put_destination(&router, "catalog", monitor, &destination).await;

    // Every name in the catalog, plus one definition carrying EVERY dimension field path
    // the model lists, with an event pattern that includes each of them.
    let mut requested = catalog_definitions();
    requested.push(extended_with_dimensions(
        EXTENDED_METRICS[0].name,
        &EXTENDED_DIMENSION_KEYS,
    ));

    let created = create_definitions(&router, "catalog", monitor, &destination, &requested).await;
    let stored = stored_by_id(&router, "catalog", monitor, &destination).await;
    assert_eq!(
        stored.len(),
        requested.len(),
        "case {:?}: expected {} stored definitions, got {}",
        "catalog",
        requested.len(),
        stored.len()
    );
    for (wanted, reported) in requested.iter().zip(created.iter()) {
        let case = wanted["Name"].as_str().unwrap_or("unnamed");
        let id = id_of(case, reported);
        let read_back = stored
            .get(&id)
            .unwrap_or_else(|| panic!("case {case:?}: no definition {id:?} was stored"));
        assert_definition(case, read_back, wanted, &id);
    }
}

#[tokio::test]
async fn an_update_replaces_the_whole_definition_under_the_id_it_names() {
    let (router, _engine) = emulator();

    // Each row is one original definition and the replacement applied to it: the rows
    // differ in which members the replacement changes, adds, drops and how many
    // dimensions it lists.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the metric name alone",
            extended(EXTENDED_METRICS[9].name),
            extended(EXTENDED_METRICS[8].name),
        ),
        (
            "a unit label added",
            extended(EXTENDED_METRICS[0].name),
            with(
                extended(EXTENDED_METRICS[0].name),
                "UnitLabel",
                json!("Milliseconds"),
            ),
        ),
        (
            "every droppable member dropped",
            with(
                extended_with_dimensions(
                    EXTENDED_METRICS[1].name,
                    &EXTENDED_DIMENSION_KEYS[0..1],
                ),
                "UnitLabel",
                json!("Milliseconds"),
            ),
            extended(EXTENDED_METRICS[1].name),
        ),
        (
            "one dimension becomes three",
            extended_with_dimensions(EXTENDED_METRICS[2].name, &EXTENDED_DIMENSION_KEYS[0..1]),
            extended_with_dimensions(EXTENDED_METRICS[2].name, &EXTENDED_DIMENSION_KEYS[0..3]),
        ),
        (
            "an extended metric becomes a custom one",
            extended(EXTENDED_METRICS[10].name),
            json!({
                "Name": "CheckoutLatency",
                "Namespace": "MyApp/Checkout",
                "ValueKey": "event_details.latency",
                "UnitLabel": "Milliseconds",
                "EventPattern": event_pattern("com.example.checkout", &[]),
            }),
        ),
        (
            "the same name, a different value key",
            json!({"Name": "CartSize", "Namespace": "MyApp/Cart", "ValueKey": "event_details.items"}),
            json!({"Name": "CartSize", "Namespace": "MyApp/Cart", "ValueKey": "event_details.units"}),
        ),
    ];

    // The definition created beside the one each row updates: it is never named by the
    // update, so it must come out of every row untouched.
    let untouched = with(
        extended(EXTENDED_METRICS[7].name),
        "UnitLabel",
        json!("Milliseconds"),
    );

    for (index, (case, original, replacement)) in rows.iter().enumerate() {
        for destination in [Destination::cloud_watch(), Destination::evidently("gamma")] {
            let monitor = format!("updated-{index}-{}", destination.slug());
            create_monitor(&router, case, &monitor).await;
            put_destination(&router, case, &monitor, &destination).await;
            let created = create_definitions(
                &router,
                case,
                &monitor,
                &destination,
                &[original.clone(), untouched.clone()],
            )
            .await;
            let id = id_of(case, &created[0]);
            let sibling = id_of(case, &created[1]);

            update_ok(&router, case, &monitor, &destination, &id, replacement).await;

            let stored = stored_by_id(&router, case, &monitor, &destination).await;
            assert_eq!(
                stored.len(),
                2,
                "case {case:?}: an update must replace a definition, not add one: {stored:?}"
            );
            let updated = stored
                .get(&id)
                .unwrap_or_else(|| panic!("case {case:?}: definition {id:?} is gone"));
            assert_definition(case, updated, replacement, &id);
            let beside = stored
                .get(&sibling)
                .unwrap_or_else(|| panic!("case {case:?}: definition {sibling:?} is gone"));
            assert_definition(case, beside, &untouched, &sibling);

            // `@idempotent`: applying the same update again leaves exactly the same state.
            update_ok(&router, case, &monitor, &destination, &id, replacement).await;
            let again = stored_by_id(&router, case, &monitor, &destination).await;
            assert_eq!(
                again, stored,
                "case {case:?}: repeating the update changed the stored definitions"
            );
        }
    }
}

#[tokio::test]
async fn an_update_the_service_cannot_apply_is_refused_and_changes_nothing() {
    let (router, _engine) = emulator();
    let monitor = "update-refusals";
    create_monitor(&router, "setup", monitor).await;
    let cloud_watch = Destination::cloud_watch();
    let evidently = Destination::evidently("delta");
    put_destination(&router, "setup", monitor, &cloud_watch).await;
    put_destination(&router, "setup", monitor, &evidently).await;

    let original = with(
        extended(EXTENDED_METRICS[10].name),
        "UnitLabel",
        json!("Count"),
    );
    let created =
        create_definitions(&router, "setup", monitor, &cloud_watch, std::slice::from_ref(&original))
            .await;
    let id = id_of("setup", &created[0]);
    let valid = extended(EXTENDED_METRICS[9].name);

    // Each row refuses for ONE reason: an app monitor or a destination that names no
    // resource (404 `ResourceNotFoundException`), a `MetricDefinitionId` the destination
    // holds nothing under, which is the add failing on a service that has the destination
    // but no definition to replace (500 `InternalServerException`, modelled `retryable`),
    // or a definition the service will not store / a destination pairing the model forbids
    // (400). `absent` is the app monitor the row addresses.
    let rows: Vec<(&str, &str, Destination, String, Value, u16, &str)> = vec![
        (
            "an app monitor that does not exist",
            "no-such-monitor",
            cloud_watch.clone(),
            id.clone(),
            valid.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a destination this monitor does not have",
            monitor,
            Destination::evidently("never-put"),
            id.clone(),
            valid.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a definition id nothing was created under",
            monitor,
            cloud_watch.clone(),
            "00000000-0000-4000-8000-000000000000".to_string(),
            valid.clone(),
            500,
            "InternalServerException",
        ),
        (
            "an id that belongs to another destination of the same monitor",
            monitor,
            evidently.clone(),
            id.clone(),
            valid.clone(),
            500,
            "InternalServerException",
        ),
        (
            "CloudWatch with a DestinationArn",
            monitor,
            Destination {
                destination: METRIC_DESTINATIONS[0].to_string(),
                destination_arn: Some(experiment_arn("delta")),
                iam_role_arn: None,
            },
            id.clone(),
            valid.clone(),
            400,
            "ValidationException",
        ),
        (
            "Evidently with no DestinationArn",
            monitor,
            Destination {
                destination: METRIC_DESTINATIONS[1].to_string(),
                destination_arn: None,
                iam_role_arn: None,
            },
            id.clone(),
            valid.clone(),
            400,
            "ValidationException",
        ),
        (
            "a namespace in AWS's reserved space",
            monitor,
            cloud_watch.clone(),
            id.clone(),
            json!({"Name": "Billed", "Namespace": "AWS/Billing"}),
            400,
            "ValidationException",
        ),
        (
            "an EventPattern that is not JSON",
            monitor,
            cloud_watch.clone(),
            id.clone(),
            json!({"Name": "Broken", "Namespace": "MyApp/Broken", "EventPattern": "{not json"}),
            400,
            "ValidationException",
        ),
        (
            "an EventPattern that is JSON but not an object",
            monitor,
            cloud_watch.clone(),
            id.clone(),
            json!({
                "Name": "Broken",
                "Namespace": "MyApp/Broken",
                "EventPattern": "[\"com.example.event\"]",
            }),
            400,
            "ValidationException",
        ),
        (
            "a DimensionKeys field the EventPattern leaves out",
            monitor,
            cloud_watch.clone(),
            id.clone(),
            with(
                extended_with_dimensions(
                    EXTENDED_METRICS[8].name,
                    &EXTENDED_DIMENSION_KEYS[1..2],
                ),
                "EventPattern",
                json!(event_pattern("com.amazon.rum.js_error_event", &["metadata.pageId"])),
            ),
            400,
            "ValidationException",
        ),
        (
            "DimensionKeys with no EventPattern at all",
            monitor,
            cloud_watch.clone(),
            id.clone(),
            with(
                custom("Dimensioned", "MyApp/Dimensioned"),
                "DimensionKeys",
                dimension_keys(&EXTENDED_DIMENSION_KEYS[0..1]),
            ),
            400,
            "ValidationException",
        ),
    ];

    for (case, addressed, destination, metric_definition_id, definition, status, code) in rows {
        let reply = update(
            &router,
            addressed,
            &destination,
            &metric_definition_id,
            &definition,
        )
        .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            code,
            "case {case:?}: expected the error code {code}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        assert!(
            message(&body).is_some_and(|text| !text.is_empty()),
            "case {case:?}: the refusal carries no message: {body}"
        );
    }

    // Not one of those refusals touched the stored definition, and none of them created
    // anything under the destination they named.
    let stored = stored_by_id(&router, "aftermath", monitor, &cloud_watch).await;
    assert_eq!(
        stored.len(),
        1,
        "case {:?}: expected the CloudWatch destination to still hold one definition, got {stored:?}",
        "aftermath"
    );
    assert_definition(
        "aftermath",
        stored
            .get(&id)
            .unwrap_or_else(|| panic!("definition {id:?} is gone")),
        &original,
        &id,
    );
    assert_eq!(
        stored_by_id(&router, "aftermath", monitor, &evidently)
            .await
            .len(),
        0,
        "case {:?}: a refused update created a definition under the Evidently destination",
        "aftermath"
    );
}

#[tokio::test]
async fn the_reserved_namespace_prefix_is_the_only_namespace_a_definition_cannot_use() {
    let (router, _engine) = emulator();
    let destination = Destination::cloud_watch();

    // Both sides of the boundary the model draws for a CUSTOM metric's namespace: it
    // cannot start with `AWS/`, and the extended-metrics namespace itself is closed to it
    // ("You can use the AWS/RUM namespace only for extended metrics"). The prefix is
    // matched as written, so a namespace that merely resembles it is accepted.
    //
    // The two refusals differ in CHANNEL, because they are two different rules: a
    // definition addressed to the extended-metrics namespace defines one of RUM's own
    // metrics, so a `Name` outside that catalog makes the whole REQUEST invalid (HTTP 400,
    // nothing stored), while any other namespace under the reserved prefix is a namespace
    // this one definition may not be published to, which fails that definition alone inside
    // a 200.
    //
    // (case, namespace, created, refused for the whole request)
    let rows: [(&str, &str, bool, bool); 7] = [
        (
            "the extended-metrics namespace itself",
            "AWS/RUM",
            false,
            true,
        ),
        ("another AWS namespace", "AWS/Billing", false, false),
        ("a longer name under the prefix", "AWS/RUMExtended", false, false),
        ("the prefix with nothing after it", "AWS/", false, false),
        ("the prefix without its separator", "AWSCustom", true, false),
        ("the prefix in another case", "aws/rum", true, false),
        ("a namespace of one's own", "MyApp/Checkout", true, false),
    ];

    for (index, (case, namespace, accepted, whole_request)) in rows.iter().enumerate() {
        let monitor = format!("namespace-{index}");
        create_monitor(&router, case, &monitor).await;
        put_destination(&router, case, &monitor, &destination).await;
        let definition = custom("NamespacedMetric", namespace);

        // The same rule governs the create, where one bad definition fails on its own,
        // and the update, where it fails the call.
        let create_reply =
            batch_create(&router, &monitor, &destination, std::slice::from_ref(&definition)).await;
        let create_status = if *whole_request { 400 } else { 200 };
        assert_eq!(
            create_reply.status,
            create_status,
            "case {case:?}: expected the batch create to answer {create_status}, got {} {:?}",
            create_reply.status,
            String::from_utf8_lossy(&create_reply.body)
        );
        let answered = create_reply.json(case);
        let refused = answered["Errors"].as_array().map(Vec::len).unwrap_or_default();
        // What the create stored is read back from the destination it was asked to store it
        // under — a fresh one per row, so every definition it holds is one this row created.
        let created = all_definitions(&router, case, &monitor, &destination)
            .await
            .len();
        let expected_counts = match (*accepted, *whole_request) {
            (true, _) => (1, 0),
            // A whole-request refusal carries no per-definition error list at all.
            (false, true) => (0, 0),
            (false, false) => (0, 1),
        };
        assert_eq!(
            (created, refused),
            expected_counts,
            "case {case:?}: expected namespace {namespace:?} to be {}, got {answered}",
            if *accepted { "created" } else { "refused" }
        );

        // And the update is judged the same way, against a definition created under a
        // namespace of its own — on a monitor of its own, so the definition the create
        // above may have stored is not a sibling this update would duplicate: a
        // destination holds at most one definition of a given metric, and an update that
        // would store a second one is a conflict rather than a validity judgement
        // (`metric_definition_identity.rs`).
        let update_monitor = format!("namespace-{index}-update");
        create_monitor(&router, case, &update_monitor).await;
        put_destination(&router, case, &update_monitor, &destination).await;
        let seeded = create_definitions(
            &router,
            case,
            &update_monitor,
            &destination,
            &[custom("Seeded", "MyApp/Seeded")],
        )
        .await;
        let id = id_of(case, &seeded[0]);
        let reply = update(&router, &update_monitor, &destination, &id, &definition).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected the update of namespace {namespace:?} to answer \
             {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    // The extended-metrics namespace is closed to a custom metric and open to an extended
    // one: the SAME namespace, accepted or refused by the kind of metric naming it.
    let monitor = "namespace-extended";
    create_monitor(&router, "the extended-metrics namespace", monitor).await;
    put_destination(&router, "the extended-metrics namespace", monitor, &destination).await;
    for (case, definition) in [
        (
            "an extended metric naming AWS/RUM",
            with(
                extended(EXTENDED_METRICS[8].name),
                "Namespace",
                json!("AWS/RUM"),
            ),
        ),
        (
            "an extended metric naming no namespace",
            extended(EXTENDED_METRICS[8].name),
        ),
    ] {
        let created = create_definitions(
            &router,
            case,
            monitor,
            &destination,
            std::slice::from_ref(&definition),
        )
        .await;
        let id = id_of(case, &created[0]);
        assert_definition(case, &created[0], &definition, &id);
    }
}

#[tokio::test]
async fn a_batch_create_refuses_only_the_definitions_it_cannot_store() {
    let (router, _engine) = emulator();
    let destination = Destination::cloud_watch();
    let monitor = "partial-batch";
    create_monitor(&router, "partial", monitor).await;
    put_destination(&router, "partial", monitor, &destination).await;

    let good_one = with(
        extended(EXTENDED_METRICS[10].name),
        "UnitLabel",
        json!("Count"),
    );
    let good_two = custom("Conversions", "MyApp/Funnel");
    let bad_namespace = custom("Reserved", "AWS/Billing");
    let bad_pattern = with(custom("Unparsed", "MyApp/Unparsed"), "EventPattern", json!("{"));

    let reply = batch_create(
        &router,
        monitor,
        &destination,
        &[
            good_one.clone(),
            bad_namespace.clone(),
            good_two.clone(),
            bad_pattern.clone(),
        ],
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected a mixed batch to answer 200, got {} {:?}",
        "mixed batch",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json("mixed batch");

    // The valid definitions still succeed, and the answer reports NONE of them — the
    // refused ones are reported in `Errors` and the stored ones nowhere.
    assert_reports_no_created_definitions("mixed batch", &answered);
    let created = held_definitions(
        &router,
        "mixed batch",
        monitor,
        &destination,
        &[good_one.clone(), good_two.clone()],
    )
    .await;
    assert_eq!(
        created.len(),
        2,
        "case {:?}: expected the two valid definitions to be created, got {created:?}",
        "mixed batch"
    );
    for (wanted, reported) in [&good_one, &good_two].iter().zip(created.iter()) {
        let id = id_of("mixed batch", reported);
        assert_definition("mixed batch", reported, wanted, &id);
    }

    // And each refused one is reported on its own, with the definition that caused it.
    let refused = answered["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        refused.len(),
        2,
        "case {:?}: expected two per-definition errors, got {answered}",
        "mixed batch"
    );
    for (wanted, error) in [&bad_namespace, &bad_pattern].iter().zip(refused.iter()) {
        assert_eq!(
            &error["MetricDefinition"], *wanted,
            "case {:?}: expected the error to carry the definition that caused it, got {error}",
            "mixed batch"
        );
        assert!(
            error["ErrorCode"].as_str().is_some_and(|code| !code.is_empty()),
            "case {:?}: the error carries no ErrorCode: {error}",
            "mixed batch"
        );
        assert!(
            error["ErrorMessage"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {:?}: the error carries no ErrorMessage: {error}",
            "mixed batch"
        );
    }

    // Only the accepted definitions are persisted.
    let stored = stored_by_id(&router, "mixed batch", monitor, &destination).await;
    assert_eq!(
        stored.len(),
        2,
        "case {:?}: expected two stored definitions, got {stored:?}",
        "mixed batch"
    );

    // A batch longer than the documented maximum is refused whole, and persists nothing.
    let oversized: Vec<Value> = (0..=200)
        .map(|index| custom(&format!("Bulk{index}"), "MyApp/Bulk"))
        .collect();
    let refusal = batch_create(&router, monitor, &destination, &oversized).await;
    assert_eq!(
        refusal.status,
        400,
        "case {:?}: expected a 201-definition batch to be refused, got {} {:?}",
        "oversized batch",
        refusal.status,
        String::from_utf8_lossy(&refusal.body)
    );
    assert_eq!(
        refusal.error_code("oversized batch"),
        "ValidationException",
        "case {:?}: expected a ValidationException, got {:?}",
        "oversized batch",
        String::from_utf8_lossy(&refusal.body)
    );
    assert_eq!(
        stored_by_id(&router, "oversized batch", monitor, &destination)
            .await
            .len(),
        2,
        "case {:?}: the refused batch persisted definitions",
        "oversized batch"
    );
}

#[tokio::test]
async fn every_page_of_a_destinations_definitions_covers_each_one_exactly_once() {
    let (router, _engine) = emulator();
    let destination = Destination::cloud_watch();

    for store_size in [0_usize, 1, 5, 7] {
        for page_size in [1_usize, 2, 3, 100] {
            let case = format!("{store_size} definitions in pages of {page_size}");
            let monitor = format!("paged-{store_size}-{page_size}");
            create_monitor(&router, &case, &monitor).await;
            put_destination(&router, &case, &monitor, &destination).await;
            let requested: Vec<Value> = (0..store_size)
                .map(|index| custom(&format!("Paged{index}"), "MyApp/Paged"))
                .collect();
            let minted: BTreeSet<String> = if store_size == 0 {
                BTreeSet::new()
            } else {
                create_definitions(&router, &case, &monitor, &destination, &requested)
                    .await
                    .iter()
                    .map(|definition| id_of(&case, definition))
                    .collect()
            };

            let mut seen: Vec<String> = Vec::new();
            let mut extra = format!("&maxResults={page_size}");
            loop {
                let reply = get_page(&router, &monitor, &destination, &extra).await;
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?}: expected a page to answer 200, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                let page = reply.json(&case);
                let definitions = page["MetricDefinitions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    definitions.len() <= page_size,
                    "case {case:?}: a page returned {} definitions, past the {page_size} asked for",
                    definitions.len()
                );
                seen.extend(definitions.iter().map(|d| id_of(&case, d)));
                match page["NextToken"].as_str() {
                    Some(token) => {
                        assert_eq!(
                            definitions.len(),
                            page_size,
                            "case {case:?}: a page that carries a token returned {} of the \
                             {page_size} it could",
                            definitions.len()
                        );
                        extra = format!(
                            "&maxResults={page_size}&nextToken={}",
                            percent_encoded(token)
                        );
                    }
                    None => break,
                }
            }

            assert_eq!(
                seen.len(),
                minted.len(),
                "case {case:?}: paging returned {} ids for {} definitions",
                seen.len(),
                minted.len()
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<String>>(),
                minted,
                "case {case:?}: paging did not cover every definition exactly once"
            );
        }
    }
}

#[tokio::test]
async fn max_results_walks_the_modelled_range_and_defaults_to_fifty() {
    let (router, _engine) = emulator();
    let destination = Destination::cloud_watch();
    let monitor = "max-results";
    create_monitor(&router, "max-results", monitor).await;
    put_destination(&router, "max-results", monitor, &destination).await;
    // One more definition than the documented default page, so the default is observable.
    let requested: Vec<Value> = (0..51)
        .map(|index| custom(&format!("Bounded{index}"), "MyApp/Bounded"))
        .collect();
    create_definitions(&router, "max-results", monitor, &destination, &requested).await;

    // `MaxResults` is modelled `range 1..100`: each published bound is accepted and one
    // past each is refused.
    for (case, max_results, accepted) in [
        ("the minimum of the range", 1_i64, true),
        ("below the minimum", 0, false),
        ("the maximum of the range", 100, true),
        ("past the maximum", 101, false),
    ] {
        let reply = get_page(
            &router,
            monitor,
            &destination,
            &format!("&maxResults={max_results}"),
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected maxResults={max_results} to be accepted, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let page = reply.json(case);
            let returned = page["MetricDefinitions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            let expected = usize::try_from(max_results).unwrap_or_default().min(51);
            assert_eq!(
                returned, expected,
                "case {case:?}: expected {expected} definitions, got {returned}"
            );
        } else {
            assert_eq!(
                reply.status,
                400,
                "case {case:?}: expected maxResults={max_results} to be refused, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }
    }

    // No `MaxResults` at all pages at the documented default of 50.
    let reply = get_page(&router, monitor, &destination, "").await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the default page to answer 200, got {}",
        "default page size",
        reply.status
    );
    let page = reply.json("default page size");
    assert_eq!(
        page["MetricDefinitions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        50,
        "case {:?}: expected the documented default of 50 definitions, got {page}",
        "default page size"
    );
    assert!(
        page["NextToken"].as_str().is_some(),
        "case {:?}: expected a token while a definition is left, got {page}",
        "default page size"
    );
}

#[tokio::test]
async fn definitions_belong_to_the_destination_they_were_created_under() {
    let (router, _engine) = emulator();
    let monitor = "scoped";
    create_monitor(&router, "scoped", monitor).await;
    let cloud_watch = Destination::cloud_watch();
    let evidently = Destination::evidently("epsilon");
    put_destination(&router, "scoped", monitor, &cloud_watch).await;
    put_destination(&router, "scoped", monitor, &evidently).await;

    // The same definition under both destinations: each is its own metric, with its own
    // id, and neither listing reports the other's.
    let shared = with(
        extended(EXTENDED_METRICS[10].name),
        "UnitLabel",
        json!("Count"),
    );
    let in_cloud_watch = create_definitions(
        &router,
        "scoped",
        monitor,
        &cloud_watch,
        std::slice::from_ref(&shared),
    )
    .await;
    let in_evidently = create_definitions(
        &router,
        "scoped",
        monitor,
        &evidently,
        std::slice::from_ref(&shared),
    )
    .await;
    let cloud_watch_id = id_of("scoped", &in_cloud_watch[0]);
    let evidently_id = id_of("scoped", &in_evidently[0]);
    assert_ne!(
        cloud_watch_id, evidently_id,
        "case {:?}: the two destinations' definitions share the id {cloud_watch_id:?}",
        "scoped"
    );

    // Updating one leaves the other exactly as it was.
    let replacement = with(
        extended(EXTENDED_METRICS[9].name),
        "UnitLabel",
        json!("Count"),
    );
    update_ok(
        &router,
        "scoped",
        monitor,
        &cloud_watch,
        &cloud_watch_id,
        &replacement,
    )
    .await;
    let stored_cloud_watch = stored_by_id(&router, "scoped", monitor, &cloud_watch).await;
    let stored_evidently = stored_by_id(&router, "scoped", monitor, &evidently).await;
    assert_definition(
        "scoped",
        stored_cloud_watch
            .get(&cloud_watch_id)
            .unwrap_or_else(|| panic!("definition {cloud_watch_id:?} is gone")),
        &replacement,
        &cloud_watch_id,
    );
    assert_definition(
        "scoped",
        stored_evidently
            .get(&evidently_id)
            .unwrap_or_else(|| panic!("definition {evidently_id:?} is gone")),
        &shared,
        &evidently_id,
    );

    // A continuation token one destination's listing issued is not a resume point in
    // another's.
    let first = get_page(&router, monitor, &cloud_watch, "&maxResults=1").await;
    let issued = first.json("scoped")["NextToken"].as_str().map(str::to_string);
    if let Some(token) = issued {
        let reply = get_page(
            &router,
            monitor,
            &evidently,
            &format!("&maxResults=1&nextToken={}", percent_encoded(&token)),
        )
        .await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected another destination's token to be refused, got {} {:?}",
            "scoped",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    // Deleting the destination takes its definitions with it: the resource they belonged
    // to is gone, so the read answers not-found rather than an empty list, while the other
    // destination still reports its own.
    let deleted = call(
        &router,
        "DELETE",
        &format!(
            "/rummetrics/{monitor}/metricsdestination?{}",
            cloud_watch.query()
        ),
        None,
    )
    .await;
    assert_eq!(
        deleted.status,
        200,
        "case {:?}: expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
        "scoped",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    let gone = get_page(&router, monitor, &cloud_watch, "").await;
    assert_eq!(
        gone.status,
        404,
        "case {:?}: expected the deleted destination's definitions to be gone, got {} {:?}",
        "scoped",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );
    assert_eq!(
        stored_by_id(&router, "scoped", monitor, &evidently)
            .await
            .len(),
        1,
        "case {:?}: deleting one destination dropped another's definitions",
        "scoped"
    );

    // And a definition of a destination that is gone can no longer be updated.
    let orphaned = update(
        &router,
        monitor,
        &cloud_watch,
        &cloud_watch_id,
        &replacement,
    )
    .await;
    assert_eq!(
        orphaned.status,
        404,
        "case {:?}: expected the update to answer 404 for a deleted destination, got {} {:?}",
        "scoped",
        orphaned.status,
        String::from_utf8_lossy(&orphaned.body)
    );
}

#[tokio::test]
async fn a_repeated_batch_create_reports_a_conflict_and_stores_no_second_copy() {
    let (router, _engine) = emulator();

    // One row per batch to create and then re-send. The conflict is a rule about the
    // IDENTITY a create is keyed by, not about one request, so the rows differ along what
    // that identity is read off: which destination the definitions live under, how many
    // definitions one call carries, and which members a definition uses — from an extended
    // metric carrying dimensions down to a custom metric carrying only what it must.
    let rows: [(&str, Destination, Vec<Value>); 4] = [
        (
            "one extended definition",
            Destination::cloud_watch(),
            vec![extended(EXTENDED_METRICS[0].name)],
        ),
        (
            "three definitions, every optional member between them",
            Destination::cloud_watch(),
            vec![
                extended_with_dimensions(EXTENDED_METRICS[5].name, &EXTENDED_DIMENSION_KEYS[..2]),
                with(custom("Checkouts", "MyApp/Funnel"), "UnitLabel", json!("Count")),
                custom("Signups", "MyApp/Funnel"),
            ],
        ),
        (
            "an Evidently destination",
            Destination::evidently("checkout-ab"),
            vec![
                extended(EXTENDED_METRICS[12].name),
                custom("Abandonments", "MyApp/Checkout"),
            ],
        ),
        (
            "a custom metric defined off an event pattern and its dimensions",
            Destination::evidently("signup-ab"),
            vec![with(
                with(
                    custom("Conversions", "MyApp/Signup"),
                    "EventPattern",
                    json!(event_pattern(
                        "com.example.signup",
                        &["metadata.browserName"]
                    )),
                ),
                "DimensionKeys",
                dimension_keys(&EXTENDED_DIMENSION_KEYS[1..2]),
            )],
        ),
    ];

    for (index, (case, destination, definitions)) in rows.into_iter().enumerate() {
        let monitor = format!("repeat-{index}-{}", destination.slug());
        create_monitor(&router, case, &monitor).await;
        put_destination(&router, case, &monitor, &destination).await;

        // What the first delivery of the batch created.
        let first = create_definitions(&router, case, &monitor, &destination, &definitions).await;
        let ids: Vec<String> = first.iter().map(|one| id_of(case, one)).collect();
        let stored = stored_by_id(&router, case, &monitor, &destination).await;
        assert_eq!(
            stored.len(),
            definitions.len(),
            "case {case:?}: expected one stored definition per requested one, got {stored:?}"
        );

        // The SAME request again: every definition the destination already holds is a
        // definition that already exists, so each is reported as its own per-item
        // `ConflictException` and the stored state is exactly the one the first delivery
        // left.
        let repeated = batch_create(&router, &monitor, &destination, &definitions).await;
        assert_eq!(
            repeated.status,
            200,
            "case {case:?}: expected the repeat to answer 200, got {} {:?}",
            repeated.status,
            String::from_utf8_lossy(&repeated.body)
        );
        let answered_repeat = repeated.json(case);
        assert_definition_conflicts(case, &answered_repeat, &definitions);
        // Every entry was refused, and the repeat reports no definition at all — as no
        // create does.
        assert_reports_no_created_definitions(case, &answered_repeat);
        let after_repeat = stored_by_id(&router, case, &monitor, &destination).await;
        assert_eq!(
            after_repeat, stored,
            "case {case:?}: repeating the batch changed the stored definitions, got \
             {after_repeat:?}"
        );

        // A batch that repeats those definitions and adds ONE the destination does not
        // hold: only the addition is created, the repeated ones are the conflicts, and the
        // stored definitions keep their ids.
        let added = custom(&format!("Added{index}"), "MyApp/Added");
        let mut with_added = definitions.clone();
        with_added.push(added.clone());
        let reply = batch_create(&router, &monitor, &destination, &with_added).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the mixed batch to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let answered_mixed = reply.json(case);
        assert_definition_conflicts(case, &answered_mixed, &definitions);
        // Only the addition was created, which the answer reports nothing about.
        assert_reports_no_created_definitions(case, &answered_mixed);
        let after_addition = stored_by_id(&router, case, &monitor, &destination).await;
        assert_eq!(
            after_addition.len(),
            definitions.len() + 1,
            "case {case:?}: expected only the added definition to be stored beside the \
             existing ones, got {after_addition:?}"
        );
        for (requested, id) in definitions.iter().zip(&ids) {
            assert_definition(
                case,
                after_addition
                    .get(id)
                    .unwrap_or_else(|| panic!("case {case:?}: {id:?} is gone: {after_addition:?}")),
                requested,
                id,
            );
        }
        let added_id = added_definition_id(case, &after_addition, &ids);
        assert_definition(
            case,
            &after_addition[&added_id],
            &added,
            &added_id,
        );

        // A definition that differs from a stored one in ONE member defines another metric,
        // so it IS created — the identity is the definition, not the name it carries.
        let changed = with(definitions[0].clone(), "UnitLabel", json!("Milliseconds"));
        let answered = create_definitions(
            &router,
            case,
            &monitor,
            &destination,
            std::slice::from_ref(&changed),
        )
        .await;
        let changed_id = id_of(case, &answered[0]);
        assert!(
            !ids.contains(&changed_id) && changed_id != added_id,
            "case {case:?}: expected a changed definition to be created under an id of its \
             own, got {changed_id:?}"
        );
        assert_definition(case, &answered[0], &changed, &changed_id);

        // The same identity named twice in ONE batch: the entries are processed in request
        // order, so the first creates the definition and the second finds it already
        // defined, which is the same conflict.
        let twice = custom(&format!("Twice{index}"), "MyApp/Added");
        let reply = batch_create(
            &router,
            &monitor,
            &destination,
            &[twice.clone(), twice.clone()],
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the batch naming one definition twice to answer 200, \
             got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let answered_twice = reply.json(case);
        assert_definition_conflicts(case, &answered_twice, std::slice::from_ref(&twice));
        // The FIRST of the two entries created the definition and the second raised the one
        // conflict; what the first entry stored is reported nowhere in the answer.
        assert_reports_no_created_definitions(case, &answered_twice);
        let finally = stored_by_id(&router, case, &monitor, &destination).await;
        assert_eq!(
            finally.len(),
            definitions.len() + 3,
            "case {case:?}: expected the three definitions this row added beside the \
             repeated ones, got {finally:?}"
        );
    }
}

/// The ONE id of `held` that is not among `existing` — the definition a mixed batch added.
fn added_definition_id(
    case: &str,
    held: &BTreeMap<String, Value>,
    existing: &[String],
) -> String {
    let mut fresh = held
        .keys()
        .filter(|id| !existing.contains(id))
        .cloned()
        .collect::<Vec<String>>();
    assert_eq!(
        fresh.len(),
        1,
        "case {case:?}: expected exactly one definition beside the stored ones, got {fresh:?}"
    );
    fresh.remove(0)
}
