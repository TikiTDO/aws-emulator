// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the event fields a CUSTOM metric definition may name and the
//! pattern it may match them by, on both operations that accept a definition:
//! `BatchCreateRumMetricDefinitions`, where a definition the service will not store fails
//! on its own, and `UpdateRumMetricDefinition`, where it fails the call.
//!
//! The rule under test is the one the model states for a custom metric: a custom metric is
//! computed from the RUM events its pattern matches, so every first-level key its
//! `ValueKey`, `EventPattern` and `DimensionKeys` name has to be a field a RUM event
//! carries, an `EventPattern` that filters on a non-empty `event_details` has to carry a
//! non-empty `event_type` too, and the array a FIRST-LEVEL key of an `EventPattern` carries
//! matches that field against exactly one value — while the matchers the member's own
//! documented examples nest below such a key (a value list under `metadata`, a numeric range
//! clause under `event_details`) carry as many values as the matcher needs.
//!
//! The tables span the rule rather than one request: the acceptance table carries one
//! definition per RUM event field the model lists, beside a pattern nested several levels
//! deep, the documented multi-value matchers, a dotted `ValueKey`, dimensions the pattern
//! names and an empty `event_details` (which needs no `event_type`); the refusal table varies
//! WHICH member names the key that
//! is no event field (`ValueKey`, `EventPattern`, `DimensionKeys`, and a listed field
//! spelled in the wrong case), how the `event_details`/`event_type` pairing is broken
//! (absent, and present but empty) and how a first-level key's array is not a single value
//! (two values, and none).
//! Every refused definition is then shown to have been stored nowhere, read back through
//! `BatchGetRumMetricDefinitions`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    all_metric_definitions, assert_created_definitions, call, create, emulator,
    held_metric_definition, message, metric_definition_id, put_metrics_destination,
    sole_metric_definition, Reply,
};

/// Every field a RUM event carries, which is every first-level key a custom metric's
/// `ValueKey`, `EventPattern` and `DimensionKeys` may name.
/// Source: `smithy_query(query="doc", shape="MetricDefinitionRequest")` — "Valid key values
/// for first-level keys in the `ValueKey`, `EventPattern`, and `DimensionKeys` fields", in
/// the order the model lists them.
const RUM_EVENT_FIELDS: [&str; 16] = [
    "account_id",
    "application_Id",
    "application_version",
    "application_name",
    "batch_id",
    "event_details",
    "event_id",
    "event_interaction",
    "event_timestamp",
    "event_type",
    "event_version",
    "log_stream",
    "metadata",
    "sessionId",
    "user_details",
    "userId",
];

/// The destination every case stores its definitions under: the one that carries no ARN.
const DESTINATION: &str = "CloudWatch";

/// The error a definition the service will not store is reported with — the code of the
/// per-definition error of the batch create, and the modelled error of the update.
const VALIDATION_CODE: &str = "ValidationException";

/// A namespace of the caller's own, which is what makes each of these definitions a CUSTOM
/// metric rather than an extended one.
const CUSTOM_NAMESPACE: &str = "MyApp/Custom";

/// One app monitor with a CloudWatch extended-metrics destination — the destination a
/// definition has to be created under.
async fn seeded_monitor(router: &MultiServiceRouter, case: &str, monitor: &str) {
    create(
        router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(router, case, monitor, DESTINATION, None).await;
}

/// `BatchCreateRumMetricDefinitions` for the CloudWatch destination of `monitor`.
async fn batch_create(router: &MultiServiceRouter, monitor: &str, definitions: &[Value]) -> Reply {
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({
            "Destination": DESTINATION,
            "MetricDefinitions": Value::Array(definitions.to_vec()),
        })),
    )
    .await
}

/// `UpdateRumMetricDefinition` replacing the definition `id` of `monitor` with
/// `definition`.
async fn update(
    router: &MultiServiceRouter,
    monitor: &str,
    id: &str,
    definition: &Value,
) -> Reply {
    call(
        router,
        "PATCH",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({
            "Destination": DESTINATION,
            "MetricDefinitionId": id,
            "MetricDefinition": definition.clone(),
        })),
    )
    .await
}

/// A custom metric named `name` under a namespace of its own, carrying the members
/// `members` adds.
fn custom_metric(name: &str, members: Value) -> Value {
    let mut definition = json!({"Name": name, "Namespace": CUSTOM_NAMESPACE});
    for (member, value) in members.as_object().expect("the members are a JSON object") {
        definition[member] = value.clone();
    }
    definition
}

/// An `EventPattern`, as the JSON string a request carries it as.
fn pattern(value: Value) -> Value {
    json!(value.to_string())
}

/// Assert `reported` carries every member `requested` asked for, with the value it asked
/// for.
fn assert_reported(case: &str, reported: &Value, requested: &Value) {
    for (member, expected) in requested.as_object().expect("a definition is a JSON object") {
        assert_eq!(
            &reported[member], expected,
            "case {case:?}: expected {member} to be {expected:?}, got {:?}",
            reported[member]
        );
    }
}

/// Custom metrics the model declares VALID, each with the reason it is one.
fn accepted_definitions() -> Vec<(String, Value)> {
    // One definition per field the model lists, each matching that field against a single
    // value — beside the `event_type` every pattern filtering on details needs.
    let mut rows: Vec<(String, Value)> = RUM_EVENT_FIELDS
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let mut matchers = json!({"event_type": ["com.example.probe"]});
            matchers[*field] = json!(["probe"]);
            (
                format!("a pattern naming the event field {field}"),
                custom_metric(
                    &format!("FieldNamed{index}"),
                    json!({"EventPattern": pattern(matchers)}),
                ),
            )
        })
        .collect();
    rows.extend([
        (
            // "The values that you specify for ValueKey … must be fields in RUM events, so
            // all first-level keys in these fields must be one of the keys in the list": a
            // dotted path reads the field its FIRST segment names.
            "a dotted ValueKey under a listed field".to_string(),
            custom_metric(
                "DottedValueKey",
                json!({
                    "ValueKey": "event_details.latency",
                    "UnitLabel": "Milliseconds",
                    "EventPattern": pattern(json!({"event_type": ["com.example.checkout"]})),
                }),
            ),
        ),
        (
            "a ValueKey naming a listed field with no path at all".to_string(),
            custom_metric(
                "BareValueKey",
                json!({"ValueKey": "event_interaction"}),
            ),
        ),
        (
            // "In EventPattern, any JSON array must contain only one value" — satisfied at
            // every depth, including an array nested inside another array's element.
            "a pattern nested several levels deep, every array a single value".to_string(),
            custom_metric(
                "DeeplyNested",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.example.http"],
                        "event_details": {"response": {"status": [{"prefix": ["4"]}]}},
                    })),
                }),
            ),
        ),
        (
            // "All dimensions listed in the DimensionKeys field must be present in the
            // value of EventPattern", and its first-level key is a listed field too.
            "dimensions keyed on listed fields and named by the pattern".to_string(),
            custom_metric(
                "Dimensioned",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.example.page_view"],
                        "metadata": {"deviceType": ["phone"]},
                        "user_details": {"userId": ["u-1"]},
                    })),
                    "DimensionKeys": {
                        "metadata.deviceType": "DeviceType",
                        "user_details.userId": "UserId",
                    },
                }),
            ),
        ),
        (
            // "For every non-empty event_details, there must be a non-empty event_type": an
            // event_details that filters on nothing asks for no event_type.
            "a pattern whose event_details is empty, with no event_type".to_string(),
            custom_metric(
                "EmptyDetails",
                json!({"EventPattern": pattern(json!({"event_details": {}}))}),
            ),
        ),
        (
            // The model's own example patterns for this member: the matchers NESTED below a
            // first-level key carry as many values as the matcher needs — a value list under
            // `metadata`, and a numeric range clause under `event_details`.
            // Source: `smithy_query(query="doc", shape="MetricDefinitionRequest",
            // member="EventPattern")`.
            "a pattern matching a metadata field against several values".to_string(),
            custom_metric(
                "MetadataValueList",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.amazon.rum.performance_navigation_event"],
                        "metadata": {"browserName": ["Chrome", "Firefox"]},
                    })),
                }),
            ),
        ),
        (
            "a pattern thresholding on a nested numeric range".to_string(),
            custom_metric(
                "NumericRange",
                json!({
                    "ValueKey": "event_details.duration",
                    "EventPattern": pattern(json!({
                        "event_type": ["com.amazon.rum.performance_navigation_event"],
                        "event_details": {"duration": [{"numeric": [">=", 2000, "<", 8000]}]},
                    })),
                }),
            ),
        ),
        (
            // The boundary of the single-value rule, inferred rather than recorded: the rule
            // binds the array a first-level key carries DIRECTLY, so an array below one is
            // the matcher's own shape whatever it holds — an empty nested matcher list is a
            // case neither the documented examples nor a recording settles, and it is
            // accepted because the rule does not reach it.
            "a pattern whose nested array carries no value at all".to_string(),
            custom_metric(
                "EmptyNestedList",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.example.page_view"],
                        "metadata": {"browserName": []},
                    })),
                }),
            ),
        ),
        (
            // A nested numeric range whose bounds carry several decimals: the pattern is a
            // JSON string on the wire, so what a read reports is the text the request sent,
            // decimals and all.
            "a pattern thresholding on fractional bounds".to_string(),
            custom_metric(
                "FractionalRange",
                json!({
                    "ValueKey": "event_details.duration",
                    "EventPattern": pattern(json!({
                        "event_type": ["com.amazon.rum.performance_navigation_event"],
                        "event_details": {
                            "duration": [{"numeric": [">=", 1234.567891, "<", 987654.321098]}],
                        },
                    })),
                }),
            ),
        ),
        (
            "a pattern carrying no array at all".to_string(),
            custom_metric(
                "ScalarMatchers",
                json!({"EventPattern": pattern(json!({"event_version": "1.0.0"}))}),
            ),
        ),
    ]);
    rows
}

/// Custom metrics the model declares INVALID, each with the reason it is one and the field
/// path or member name the refusal has to name.
fn refused_definitions() -> Vec<(String, Value, String)> {
    vec![
        (
            // The instance this rule was filed on.
            "a ValueKey whose first-level key is no RUM event field".to_string(),
            custom_metric(
                "CheckoutLatency",
                json!({"ValueKey": "not_a_rum_field.latency"}),
            ),
            "not_a_rum_field".to_string(),
        ),
        (
            // A key is matched as written, so a listed field spelled in another case names
            // no field at all.
            "a ValueKey naming a listed field in the wrong case".to_string(),
            custom_metric(
                "CasedValueKey",
                json!({"ValueKey": "application_id.version"}),
            ),
            "application_id".to_string(),
        ),
        (
            "an EventPattern whose first-level key is no RUM event field".to_string(),
            custom_metric(
                "PageUrl",
                json!({"EventPattern": pattern(json!({"page_url": ["/checkout"]}))}),
            ),
            "page_url".to_string(),
        ),
        (
            // The dimension IS named by the pattern, so the only rule left to break is the
            // one about which keys name an event field.
            "a DimensionKeys key whose first-level key is no RUM event field".to_string(),
            custom_metric(
                "BrowserDimension",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.example.page_view"],
                        "browser": {"name": ["Chrome"]},
                    })),
                    "DimensionKeys": {"browser.name": "BrowserName"},
                }),
            ),
            "browser".to_string(),
        ),
        (
            // "If EventPattern contains an event_details field, it must also contain an
            // event_type."
            "an EventPattern filtering on event_details with no event_type".to_string(),
            custom_metric(
                "CartDepth",
                json!({"EventPattern": pattern(json!({"event_details": {"latency": [1]}}))}),
            ),
            "event_type".to_string(),
        ),
        (
            // "For every non-empty event_details, there must be a non-empty event_type."
            "an EventPattern whose event_type filters on nothing".to_string(),
            custom_metric(
                "EmptyType",
                json!({
                    "EventPattern": pattern(json!({
                        "event_details": {"latency": [1]},
                        "event_type": {},
                    })),
                }),
            ),
            "event_type".to_string(),
        ),
        (
            // "In EventPattern, any JSON array must contain only one value" — the array a
            // FIRST-LEVEL key carries, which is where that rule and the documented example
            // patterns agree (the examples nest multi-value matchers below one).
            "an EventPattern whose first-level array carries two values".to_string(),
            custom_metric(
                "CartDepth2",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": ["com.example.a", "com.example.b"],
                    })),
                }),
            ),
            "event_type".to_string(),
        ),
        (
            "an EventPattern whose first-level array carries no value at all".to_string(),
            custom_metric(
                "NoType",
                json!({
                    "EventPattern": pattern(json!({
                        "event_type": [],
                        "metadata": {"browserName": ["Chrome"]},
                    })),
                }),
            ),
            "event_type".to_string(),
        ),
    ]
}

#[tokio::test]
async fn every_custom_metric_naming_only_rum_event_fields_is_created_and_read_back() {
    let (router, _engine) = emulator();
    let monitor = "custom-fields-accepted";
    seeded_monitor(&router, "accepted", monitor).await;

    let rows = accepted_definitions();
    let requested: Vec<Value> = rows
        .iter()
        .map(|(_, definition)| definition.clone())
        .collect();
    let reply = batch_create(&router, monitor, &requested).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the batch create to answer 200, got {} {:?}",
        "accepted",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json("accepted");
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {:?}: expected every definition to be accepted, got {:?}",
        "accepted",
        answered["Errors"]
    );
    // The create echoes one definition per entry it created, in request order.
    assert_created_definitions("accepted", &answered, &requested);

    // Each is read back as the definition that was asked for.
    let held = all_metric_definitions(&router, "accepted", monitor, DESTINATION, None, None).await;
    assert_eq!(
        held.len(),
        rows.len(),
        "case {:?}: expected the destination to hold {} definitions, got {held:?}",
        "accepted",
        rows.len()
    );
    for (case, requested) in rows.iter() {
        assert_reported(
            case,
            &held_metric_definition(case, &held, requested),
            requested,
        );
    }

    // The single-definition update accepts each of them too, judging a definition the same
    // way the create judges one. It runs against a monitor of its own holding exactly ONE
    // definition, which every row replaces in turn: a destination holds at most one
    // definition of a given metric, so an update that copied a definition a SIBLING id of
    // the same destination already holds is answered as a conflict rather than as a
    // judgement of the definition's validity (`metric_definition_identity.rs`).
    let update_monitor = "custom-fields-updated";
    seeded_monitor(&router, "accepted", update_monitor).await;
    let subject = sole_metric_definition(
        &router,
        "accepted",
        update_monitor,
        DESTINATION,
        None,
        &rows[0].1,
    )
    .await;
    for (case, definition) in &rows {
        let reply = update(&router, update_monitor, &subject, definition).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the update to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn no_custom_metric_naming_a_field_no_rum_event_has_is_stored_by_either_operation() {
    let (router, _engine) = emulator();
    let monitor = "custom-fields-refused";
    seeded_monitor(&router, "refused", monitor).await;

    // One definition the model declares valid beside the invalid ones, so the batch shows
    // what the model asks for: the bad definitions fail and the valid one still succeeds.
    let valid = custom_metric(
        "Conversions",
        json!({
            "ValueKey": "event_details.converted",
            "EventPattern": pattern(json!({"event_type": ["com.example.conversion"]})),
        }),
    );
    let rows = refused_definitions();
    let mut requested = vec![valid.clone()];
    requested.extend(rows.iter().map(|(_, definition, _)| definition.clone()));

    let reply = batch_create(&router, monitor, &requested).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the batch create to answer 200, got {} {:?}",
        "refused",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json("refused");
    // What the partly refused batch stored is read back from the destination.
    let created: Vec<Value> =
        all_metric_definitions(&router, "refused", monitor, DESTINATION, None, None)
            .await
            .into_values()
            .collect();
    assert_eq!(
        created.len(),
        1,
        "case {:?}: expected only the valid definition to be created, got {created:?}",
        "refused"
    );
    assert_reported("refused", &created[0], &valid);

    // One error per refused definition, in the order the request named them, each naming
    // the key or field path that caused it.
    let errors = answered["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        rows.len(),
        "case {:?}: expected {} per-definition errors, got {errors:?}",
        "refused",
        rows.len()
    );
    for ((case, definition, named), error) in rows.iter().zip(errors.iter()) {
        assert_eq!(
            &error["MetricDefinition"], definition,
            "case {case:?}: expected the error to carry the definition that caused it, got \
             {error}"
        );
        assert_eq!(
            error["ErrorCode"].as_str(),
            Some(VALIDATION_CODE),
            "case {case:?}: expected the error code {VALIDATION_CODE}, got {error}"
        );
        assert!(
            error["ErrorMessage"]
                .as_str()
                .is_some_and(|text| text.contains(named)),
            "case {case:?}: expected the error message to name {named:?}, got {error}"
        );
    }

    // And the single-definition update refuses each of them outright, with the modelled
    // error, leaving the definition it named as it was.
    let subject = metric_definition_id("refused", &created[0]);
    for (case, definition, named) in &rows {
        let reply = update(&router, monitor, &subject, definition).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected the update to answer 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            VALIDATION_CODE,
            "case {case:?}: expected a {VALIDATION_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        assert!(
            message(&body).is_some_and(|text| text.contains(named)),
            "case {case:?}: expected the refusal to name {named:?}, got {body}"
        );
    }

    // Nothing either operation refused was stored: the destination holds the one valid
    // definition, exactly as it was created.
    let held = all_metric_definitions(&router, "refused", monitor, DESTINATION, None, None).await;
    assert_eq!(
        held.len(),
        1,
        "case {:?}: expected the destination to hold only the valid definition, got {held:?}",
        "refused"
    );
    let (_, only) = held
        .iter()
        .next()
        .expect("the destination holds the valid definition");
    assert_reported("refused", only, &valid);
}
