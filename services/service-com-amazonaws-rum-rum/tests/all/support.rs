// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness this crate's suites drive the emulator through.
//!
//! Every suite in `tests/all/` dispatches real, signed restJson1 requests at a
//! `MultiServiceRouter` with the RUM service mounted, so routing, deserialization, the
//! modelled constraints, the handler and the response envelope are all exercised rather
//! than just a handler body. The pieces that are the same for every suite — the SigV4
//! credential scope, the emulator, the raw `Reply`, the dispatch helpers and the two
//! calls (`CreateAppMonitor`, `GetAppMonitor`) a suite needs to set up or read back a
//! resource — live here, so a change to the wire path is made once instead of per suite.
//!
//! The extended-metrics helpers at the end are here for the same reason: the
//! `(Destination, DestinationArn)` pair, the `PutRumMetricsDestination` that has to
//! precede any metric definition, and the paginated read that is the only way to see a
//! stored definition are the same for every suite that touches metrics.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The region every case signs for unless it is testing the region itself.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`rum`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on, and the region the ARN is derived from. Request
/// authentication is switched off below, so only the scope's shape matters.
pub(crate) fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/rum/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One emulator: a shared engine, the RUM service, and the router that dispatches to
/// it. Authentication is off so a test needs no signing key. The engine is handed back
/// as well, so a case can read the persisted record for the members no implemented
/// operation reports yet (the event buffer `PutRumEvents` appends to).
///
/// The CloudWatch Logs service is mounted beside RUM because a request that turns
/// `CwLogEnabled` on provisions the vended log group by calling it in process, exactly as
/// the assembled emulator does (`core/router`'s `build_router` registers every service).
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One HTTP answer, kept as raw bytes so a case can assert on an EMPTY body as well as
/// on JSON, and with the response headers so a case can assert on the envelope (the
/// media type the service announces) rather than only on the payload.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Bytes,
    pub(crate) headers: http::HeaderMap,
}

impl Reply {
    /// The media type this answer announces, as the client reads it.
    pub(crate) fn content_type(&self) -> Option<&str> {
        self.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
    }

    pub(crate) fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    pub(crate) fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://rum.{region}.amazonaws.com{path_and_query}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    Reply {
        status,
        body: response.into_body(),
        headers,
    }
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path_and_query, body).await
}

/// `AppMonitorId`'s modelled pattern: 36 characters,
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
/// Source: `smithy_query(query="shape", shape="AppMonitor")`, member `Id`.
///
/// Also checks the two nibbles a version-4, RFC 4122 UUID fixes — the version digit and
/// the variant digit — since that is the form the service mints.
pub(crate) fn is_modelled_app_monitor_id(id: &str) -> bool {
    let shaped = id.len() == 36
        && id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && id.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    let version = id.chars().nth(14) == Some('4');
    let variant = matches!(id.chars().nth(19), Some('8' | '9' | 'a' | 'b'));
    shaped && version && variant
}

/// The `message` of an error body, under either spelling the member may arrive as.
///
/// ONE definition for every suite: a restJson1 error body carries the modelled `message`
/// member, but an SDK and some AWS frontends spell it `Message`, so a suite that pinned
/// only one spelling would pass or fail on the casing rather than on the text. Takes the
/// parsed body rather than a `Reply`, so the suites that still own their own reply type
/// can use it too.
pub(crate) fn message(document: &Value) -> Option<&str> {
    document["message"]
        .as_str()
        .or_else(|| document["Message"].as_str())
}

/// A `PutRumEvents` body carrying two events of different types, for the monitor whose
/// id is `id`.
///
/// Shared, because more than one suite needs a well-formed batch: the lifecycle suite to
/// show the events land on the monitor the id names, and the error-body suite to reach
/// the not-found refusal for an id that names none.
pub(crate) fn rum_events_body(id: &str) -> Value {
    json!({
        "BatchId": "11111111-2222-4333-8444-555555555555",
        "AppMonitorDetails": {"name": "any", "id": id, "version": "1.0.0"},
        "UserDetails": {
            "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
        },
        "RumEvents": [
            {
                "id": "12345678-1234-4234-8234-123456789012",
                "timestamp": 1_704_067_200,
                "type": "com.amazon.rum.page_view_event",
                "metadata": "{\"version\":\"1.0.0\"}",
                "details": "{\"pageId\":\"/home\"}",
            },
            {
                "id": "22345678-1234-4234-8234-123456789012",
                "timestamp": 1_704_067_260,
                "type": "com.amazon.rum.js_error_event",
                "details": "{\"message\":\"boom\"}",
            },
        ],
    })
}

/// The `Tags` of a reported shape, with an omitted member read as the empty map it
/// stands for: restJson1 draws no distinction between an absent map member and an empty
/// one.
pub(crate) fn tag_map(reported: &Value) -> Value {
    match &reported["Tags"] {
        Value::Null => json!({}),
        tags => tags.clone(),
    }
}

/// Percent-encode one URI path segment or query value: every byte outside RFC 3986's
/// unreserved set is escaped.
///
/// A `ResourceArn` is a non-greedy `@httpLabel`, so an SDK encodes it into ONE path
/// segment — its `:` and `/` included — and a tag key travels the same way in a
/// `@httpQuery` value (so a `+` in a key is not read as a space). An Evidently experiment
/// ARN in `?destinationArn=` and an `application/x-www-form-urlencoded` component (a
/// policy document's braces and quotes) are escaped the same way. This is the one copy
/// every suite in this crate uses, including the ones that bring their own router and
/// `call`.
pub(crate) fn percent_encoded(raw: &str) -> String {
    raw.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create one app monitor, returning the `Id` the call minted.
pub(crate) async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let reply = call(router, "POST", "/appmonitor", Some(body)).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let id = reply.json(case)["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor returned no Id"))
        .to_string();
    assert!(
        is_modelled_app_monitor_id(&id),
        "case {case:?}: expected Id to match AppMonitorId's 36-character UUID pattern, got {id:?}"
    );
    id
}

/// The ARN of an emulated Evidently experiment — the resource an `Evidently`
/// extended-metrics destination names — built with the shared `engine::emulator_arn`
/// helper, so it is a canonical ARN rather than a hand-spelled string.
pub(crate) fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The ARN of an emulated IAM role — the role RUM assumes to write to an experiment. IAM
/// is global, so the region segment is empty.
pub(crate) fn metrics_role_arn(role: &str) -> String {
    engine::emulator_arn("iam", "", &format!("role/{role}"))
}

/// The `destination`/`destinationArn` query string every metrics operation addressed by a
/// `(Destination, DestinationArn)` pair binds that pair to
/// (`@httpQuery("destination")` / `@httpQuery("destinationArn")`). `Some("")` sends the
/// member with an empty value, for a case that is about the member's own constraint.
pub(crate) fn metrics_destination_query(destination: &str, destination_arn: Option<&str>) -> String {
    let mut query = format!("destination={}", percent_encoded(destination));
    if let Some(arn) = destination_arn {
        query.push_str(&format!("&destinationArn={}", percent_encoded(arn)));
    }
    query
}

/// `PutRumMetricsDestination`: the call that has to have created a destination before any
/// metric definition can be stored under it. An Evidently destination carries the
/// experiment ARN and the role RUM writes to it through; a CloudWatch one carries neither.
pub(crate) async fn put_metrics_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) {
    let mut body = json!({"Destination": destination});
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
        body["IamRoleArn"] = json!(metrics_role_arn("rum-writer"));
    }
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(body),
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

/// One page of `BatchGetRumMetricDefinitions`: `GET /rummetrics/{AppMonitorName}/metrics`
/// with `query` carrying the target and any `maxResults`/`nextToken` the case binds.
pub(crate) async fn metric_definitions_page(
    router: &MultiServiceRouter,
    monitor: &str,
    query: &str,
) -> Reply {
    call(
        router,
        "GET",
        &format!("/rummetrics/{monitor}/metrics?{query}"),
        None,
    )
    .await
}

/// The definitions a page reports, with an omitted member read as the empty list it
/// stands for: restJson1 draws no distinction between an absent list member and an empty
/// one.
pub(crate) fn reported_metric_definitions(page: &Value) -> Vec<Value> {
    page["MetricDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `MetricDefinitionId` of a reported definition.
pub(crate) fn metric_definition_id(case: &str, reported: &Value) -> String {
    reported["MetricDefinitionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {reported} carries no MetricDefinitionId"))
        .to_string()
}

/// Every definition one destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions` and following its tokens to the end, keyed by id —
/// asserting on the way that no page reports a definition twice.
pub(crate) async fn all_metric_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    page_size: Option<usize>,
) -> BTreeMap<String, Value> {
    let mut collected: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = metrics_destination_query(destination, destination_arn);
        if let Some(size) = page_size {
            query.push_str(&format!("&maxResults={size}"));
        }
        if let Some(issued) = &token {
            query.push_str(&format!("&nextToken={}", percent_encoded(issued)));
        }
        let reply = metric_definitions_page(router, monitor, &query).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected a page to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        for reported in reported_metric_definitions(&page) {
            let id = metric_definition_id(case, &reported);
            assert!(
                collected.insert(id.clone(), reported).is_none(),
                "case {case:?}: definition {id:?} was reported twice"
            );
        }
        match page["NextToken"].as_str() {
            Some(issued) => token = Some(issued.to_string()),
            None => break,
        }
    }
    collected
}

/// `BatchCreateRumMetricDefinitions`: `POST /rummetrics/{AppMonitorName}/metrics`, naming
/// one destination and carrying `definitions`.
pub(crate) async fn batch_create_metric_definitions(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
) -> Reply {
    let mut body = json!({
        "Destination": destination,
        "MetricDefinitions": Value::Array(definitions.to_vec()),
    });
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
    }
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await
}

/// Assert an accepted batch create ECHOES what it created: `MetricDefinitions` carries ONE
/// definition per requested entry, in request order, each the canonical form of the request
/// it was created from ([`canonical_metric_definition`]) under an id the call minted, and no
/// two of them sharing an id. Returns those ids in request order — the handles
/// `UpdateRumMetricDefinition` and `BatchDeleteRumMetricDefinitions` address the new
/// definitions by.
pub(crate) fn assert_created_definitions(
    case: &str,
    answered: &Value,
    requested: &[Value],
) -> Vec<String> {
    let created = answered["MetricDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        created.len(),
        requested.len(),
        "case {case:?}: expected the create to report {} definition(s), got {created:?}",
        requested.len()
    );
    let mut ids: Vec<String> = Vec::new();
    for (asked, reported) in requested.iter().zip(created.iter()) {
        let id = metric_definition_id(case, reported);
        assert!(
            (1..=255).contains(&id.chars().count()),
            "case {case:?}: expected a MetricDefinitionId within MetricDefinitionId's \
             1..255 length, got {id:?}"
        );
        assert!(
            !ids.contains(&id),
            "case {case:?}: expected every created definition to carry its own id, got \
             {id:?} twice in {created:?}"
        );
        assert_metric_definition(case, reported, asked, &id);
        ids.push(id);
    }
    ids
}

/// The definitions an accepted `BatchCreateRumMetricDefinitions` created, in REQUEST
/// order, asserting it refused none of them: a 200 whose `Errors` is the empty list the
/// `@required` member carries when nothing was refused.
///
/// The create's own answer ECHOES what it created ([`assert_created_definitions`]), so each
/// requested definition is resolved to the stored one through the id the create reported for
/// it rather than by matching content — which is what the echo is FOR: two definitions the
/// service tells apart can read back identically, so content alone does not say which of
/// them a call created. The stored definition is then asserted to be the one that was asked
/// for, in the canonical form the service stores it in ([`canonical_metric_definition`]).
pub(crate) async fn created_metric_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
) -> Vec<Value> {
    let reply =
        batch_create_metric_definitions(router, monitor, destination, destination_arn, definitions)
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
    let echoed = assert_created_definitions(case, &answered, definitions);
    stored_under_echoed_ids(
        router,
        case,
        monitor,
        destination,
        destination_arn,
        definitions,
        &echoed,
    )
    .await
}

/// The definition the destination holds under each of `echoed` — the ids a create reported
/// for `definitions` — asserting each is the definition that entry of the request asked for.
///
/// The ONE read-back a create is checked through, so every suite resolves "the definition
/// this entry created" the same way: by the id the create handed back.
pub(crate) async fn stored_under_echoed_ids(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
    echoed: &[String],
) -> Vec<Value> {
    let held = all_metric_definitions(router, case, monitor, destination, destination_arn, None)
        .await;
    definitions
        .iter()
        .zip(echoed.iter())
        .map(|(requested, id)| {
            let reported = held.get(id).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the create reported {id:?}, which the destination holds no \
                     definition under: {held:?}"
                )
            });
            assert_metric_definition(case, reported, requested, id);
            reported.clone()
        })
        .collect()
}

/// The definition the destination now holds for each of `definitions`, in request order —
/// the read-back that shows what the create stored, beside the answer it echoed.
pub(crate) async fn held_metric_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
) -> Vec<Value> {
    let held = all_metric_definitions(router, case, monitor, destination, destination_arn, None)
        .await
        .into_values()
        .collect::<Vec<Value>>();
    definitions
        .iter()
        .map(|requested| matching_metric_definition(case, &held, requested).clone())
        .collect()
}

/// The one of `held` that IS `requested`: the definition whose every member equals the
/// canonical form the service stores that request in
/// ([`canonical_metric_definition`]) — the `MetricDefinitionId` aside, which is the handle
/// the service minted and so is what the caller is looking for.
///
/// ONE definition of "the stored definition this request names" for every suite, so no two
/// of them resolve a requested definition differently.
pub(crate) fn matching_metric_definition<'a>(
    case: &str,
    held: &'a [Value],
    requested: &Value,
) -> &'a Value {
    let wanted = canonical_metric_definition(requested);
    held.iter()
        .find(|reported| {
            METRIC_DEFINITION_MEMBERS
                .iter()
                .all(|member| reported[member] == wanted[member])
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no definition is stored for {wanted}, got {held:?}")
        })
}

/// The same, for a suite that holds a destination's definitions as the id -> definition map
/// [`all_metric_definitions`] reports.
pub(crate) fn held_metric_definition(
    case: &str,
    held: &BTreeMap<String, Value>,
    requested: &Value,
) -> Value {
    let held: Vec<Value> = held.values().cloned().collect();
    matching_metric_definition(case, &held, requested).clone()
}

/// The form the service stores a requested `MetricDefinitionRequest` in, which is the form
/// every read reports it in:
///
/// * an `EventPattern` is the pattern left once the whitespace around it is gone, and one
///   that declares nothing at all is an ABSENT member;
/// * a `DimensionKeys` map with no entry is an absent member;
/// * an omitted `Namespace` is the namespace the metric is published to: a definition that
///   names none is an extended metric, and extended metrics go to RUM's own
///   [`EXTENDED_METRICS_NAMESPACE`].
///
/// A member the definition does not carry is spelled as JSON `null`, which is how
/// restJson1 reports an absent member back.
pub(crate) fn canonical_metric_definition(requested: &Value) -> Value {
    let mut canonical = json!({});
    for member in METRIC_DEFINITION_MEMBERS {
        canonical[member] = requested.get(member).cloned().unwrap_or(Value::Null);
    }
    if let Some(pattern) = canonical["EventPattern"].as_str() {
        let declared = pattern.trim();
        canonical["EventPattern"] = if declared.is_empty() {
            Value::Null
        } else {
            json!(declared)
        };
    }
    if canonical["DimensionKeys"]
        .as_object()
        .is_some_and(|keys| keys.is_empty())
    {
        canonical["DimensionKeys"] = Value::Null;
    }
    if canonical["Namespace"].is_null() {
        canonical["Namespace"] = json!(EXTENDED_METRICS_NAMESPACE);
    }
    canonical
}

/// The namespace RUM publishes EXTENDED metrics to — the namespace a definition that names
/// none is read back under.
/// Source: `smithy_query(query="doc", shape="MetricDefinitionRequest")` — "The `Namespace`
/// parameter must be omitted or set to `AWS/RUM`".
pub(crate) const EXTENDED_METRICS_NAMESPACE: &str = "AWS/RUM";

/// The `ErrorCode` the batch create reports for a definition the addressed destination
/// already holds.
/// Source: `smithy_query(query="operation",
/// operation="BatchCreateRumMetricDefinitions")` — the modelled `ConflictException`, "This
/// operation attempted to create a resource that already exists".
pub(crate) const CONFLICT_CODE: &str = "ConflictException";

/// The `ErrorMessage` that per-definition conflict carries: the definition the request asked
/// for, printed as the service prints a modelled structure — the shape's name over its
/// members in the order the model declares them, each as `name=value`, with `null` for a
/// member the request left out and a map as its entries in key order.
///
/// Spelled out here rather than read back out of the crate, so these cases pin the wording
/// instead of agreeing with it by construction.
pub(crate) fn already_defined_message(requested: &Value) -> String {
    let printed = |member: &str| match requested.get(member).and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    };
    let dimension_keys = match requested.get("DimensionKeys").and_then(Value::as_object) {
        Some(keys) => {
            // In KEY order, which is the order the service prints a map in — and the one
            // order one request's map can be printed in twice, since nothing carries the
            // order the request spelled its entries in.
            let mut entries: Vec<String> = keys
                .iter()
                .map(|(field, dimension)| {
                    format!("{field}={}", dimension.as_str().unwrap_or_default())
                })
                .collect();
            entries.sort();
            format!("{{{}}}", entries.join(", "))
        }
        None => "null".to_string(),
    };
    format!(
        "Metric definition already exists: MetricDefinitionRequest(name={}, valueKey={}, \
         unitLabel={}, dimensionKeys={dimension_keys}, eventPattern={}, namespace={})",
        printed("Name"),
        printed("ValueKey"),
        printed("UnitLabel"),
        printed("EventPattern"),
        printed("Namespace"),
    )
}

/// Assert the `Errors` of an answered batch create report exactly one conflict per
/// definition of `conflicting`, in the order the request named them: the modelled error's
/// code, the definition that caused it and the message above.
///
/// What the same answer ECHOES in `MetricDefinitions` is asserted on its own
/// ([`assert_created_definitions`]), because a batch can carry both: the entries the
/// destination already holds are refused while the rest are still created.
pub(crate) fn assert_definition_conflicts(case: &str, answered: &Value, conflicting: &[Value]) {
    let errors = answered["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        conflicting.len(),
        "case {case:?}: expected {} per-definition conflict(s), got {errors:?}",
        conflicting.len()
    );
    for (requested, error) in conflicting.iter().zip(errors.iter()) {
        assert_eq!(
            &error["MetricDefinition"], requested,
            "case {case:?}: expected the conflict to carry the definition that caused it, \
             got {error}"
        );
        assert_eq!(
            error["ErrorCode"].as_str(),
            Some(CONFLICT_CODE),
            "case {case:?}: expected the error code {CONFLICT_CODE}, got {error}"
        );
        let expected = already_defined_message(requested);
        assert_eq!(
            error["ErrorMessage"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {error}"
        );
    }
}

/// A `BatchCreateRumMetricDefinitions` whose every definition the addressed destination
/// ALREADY holds: asserts the 200 and one per-item conflict per definition
/// ([`assert_definition_conflicts`]), and reports the definition the destination holds for
/// each of them — the stored definitions the refused call left alone.
pub(crate) async fn conflicting_metric_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
) -> Vec<Value> {
    let reply =
        batch_create_metric_definitions(router, monitor, destination, destination_arn, definitions)
            .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_definition_conflicts(case, &answered, definitions);
    // Every entry was refused, so the call created nothing and echoes nothing.
    assert_created_definitions(case, &answered, &[]);
    held_metric_definitions(router, case, monitor, destination, destination_arn, definitions).await
}

/// Create ONE metric definition under a destination and report the `MetricDefinitionId`
/// the service minted for it.
///
/// The subject a case needs when it drives `UpdateRumMetricDefinition` through a TABLE of
/// definitions: a destination holds at most one definition of a given metric, so a subject
/// sharing its destination with the definitions a case is about would be updated into a copy
/// of a sibling and refused as a conflict. Seeding it alone keeps the update's answer about
/// the definition the row carries.
pub(crate) async fn sole_metric_definition(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definition: &Value,
) -> String {
    let created = created_metric_definitions(
        router,
        case,
        monitor,
        destination,
        destination_arn,
        std::slice::from_ref(definition),
    )
    .await;
    assert_eq!(
        created.len(),
        1,
        "case {case:?}: expected the seed to create one definition, got {created:?}"
    );
    metric_definition_id(case, &created[0])
}

/// `UpdateRumMetricDefinition`: `PATCH /rummetrics/{AppMonitorName}/metrics`, as the RAW
/// answer — for a case that is about a refusal rather than an accepted replacement.
pub(crate) async fn update_metric_definition_reply(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    metric_definition_id: &str,
    definition: &Value,
) -> Reply {
    let mut body = json!({
        "Destination": destination,
        "MetricDefinitionId": metric_definition_id,
        "MetricDefinition": definition.clone(),
    });
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
    }
    call(
        router,
        "PATCH",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await
}

/// `UpdateRumMetricDefinition`, asserting the answer the model declares — a 200 whose body
/// carries no member, since `UpdateRumMetricDefinitionResponse` models none.
pub(crate) async fn update_metric_definition(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    metric_definition_id: &str,
    definition: &Value,
) {
    let reply = update_metric_definition_reply(
        router,
        monitor,
        destination,
        destination_arn,
        metric_definition_id,
        definition,
    )
    .await;
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

/// The members a `MetricDefinitionRequest` carries, which are also the members the
/// `MetricDefinition` a read reports carries beside its id — so a case can assert on the
/// WHOLE definition rather than on the member it varies.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest,MetricDefinition")`.
pub(crate) const METRIC_DEFINITION_MEMBERS: [&str; 6] = [
    "Name",
    "ValueKey",
    "UnitLabel",
    "DimensionKeys",
    "EventPattern",
    "Namespace",
];

/// Assert `reported` is the definition `expected` spells, under the id `id`: every member
/// of the form the service stores that definition in ([`canonical_metric_definition`])
/// reported with its value, and every member it leaves out absent (restJson1 draws no
/// distinction between an absent member and a null one, so an omitted member reads as
/// `Null` either way).
pub(crate) fn assert_metric_definition(case: &str, reported: &Value, expected: &Value, id: &str) {
    assert_eq!(
        reported["MetricDefinitionId"].as_str(),
        Some(id),
        "case {case:?}: expected the definition to be reported under {id:?}, got {reported}"
    );
    let canonical = canonical_metric_definition(expected);
    for member in METRIC_DEFINITION_MEMBERS {
        let wanted = &canonical[member];
        assert_eq!(
            &reported[member], wanted,
            "case {case:?}: expected {member} to be {wanted:?}, got {:?}",
            reported[member]
        );
    }
}

/// `definition` with `member` left out entirely, for a case whose rows differ in which
/// member a definition carries at all.
pub(crate) fn metric_definition_without(definition: &Value, member: &str) -> Value {
    let mut stripped = definition.clone();
    stripped
        .as_object_mut()
        .expect("a metric definition is a JSON object")
        .remove(member);
    stripped
}

/// The `AppMonitor` structure `GetAppMonitor` reports for `name`.
pub(crate) async fn get_app_monitor(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let reply = call(router, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"].clone()
}
