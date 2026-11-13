// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `DimensionKeys` ENTRIES an extended metric definition may
//! carry, on both operations that accept a definition:
//! `BatchCreateRumMetricDefinitions`, where a definition the service will not store fails
//! on its own, and `UpdateRumMetricDefinition`, where it fails the call.
//!
//! The rule under test is the one the model states for the shape: for an extended metric,
//! every entry of `DimensionKeys` is one of the `field path`/`dimension name` pairs the
//! model enumerates, so a field path it does not list and a listed path mapped to any other
//! dimension name are both refused — while a CUSTOM metric, whose dimensions it computes
//! itself, keeps naming whatever RUM event fields it likes.
//!
//! The tables span the rule rather than one request: the acceptance table carries one
//! definition per listed entry, one carrying every listed entry at once, one under each
//! namespace an extended metric may name, one carrying no dimension at all, and custom
//! metrics whose dimension names the extended list does not pin (including one keyed on a
//! listed path); the refusal table varies HOW the entry departs from the list — another
//! entry's dimension name, a name that is no RUM dimension, the right name in the wrong
//! case, a listed path in the wrong case, a listed path's first segment alone, a listed
//! path with a segment too many, a RUM event field the list does not name, a path that is
//! no RUM event field at all, and one bad entry beside a good one. Every refused definition
//! is then shown to have been stored nowhere, read back through
//! `BatchGetRumMetricDefinitions`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    dimension_keys, event_pattern, extended, extended_with_dimensions, with,
    EXTENDED_DIMENSION_KEYS, EXTENDED_METRICS,
};
use crate::support::{
    all_metric_definitions, assert_reports_no_created_definitions, call, create, emulator,
    held_metric_definition, message, metric_definition_id, put_metrics_destination,
    sole_metric_definition, Reply,
};

/// The destination every case stores its definitions under: the one that carries no ARN.
const DESTINATION: &str = "CloudWatch";

/// The error a definition the service will not store is reported with — the code of the
/// per-definition error of the batch create, and the modelled error of the update.
const VALIDATION_CODE: &str = "ValidationException";

/// The namespace RUM publishes extended metrics to, the one value an extended metric's
/// `Namespace` may carry — the shared constant, so this suite and the read-back rule name
/// one namespace.
use crate::support::EXTENDED_METRICS_NAMESPACE as EXTENDED_NAMESPACE;

/// A namespace of the caller's own, which is what makes a definition a CUSTOM metric.
const CUSTOM_NAMESPACE: &str = "MyApp/Custom";

/// The event type a custom metric's pattern below matches.
const CUSTOM_EVENT_TYPE: &str = "com.example.probe";

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
async fn update(router: &MultiServiceRouter, monitor: &str, id: &str, definition: &Value) -> Reply {
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

/// A custom metric: a name the extended catalog does not list, under a namespace of its
/// own, carrying `entries` as its `DimensionKeys` and a pattern naming each entry's field
/// path.
fn custom_dimensioned(name: &str, entries: &[(&str, &str)]) -> Value {
    let paths: Vec<&str> = entries.iter().map(|(field, _)| *field).collect();
    json!({
        "Name": name,
        "Namespace": CUSTOM_NAMESPACE,
        "EventPattern": event_pattern(CUSTOM_EVENT_TYPE, &paths),
        "DimensionKeys": dimension_keys(entries),
    })
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

/// Definitions the model declares VALID, each with the reason it is one.
fn accepted_definitions() -> Vec<(String, Value)> {
    // One extended metric per entry the model lists, each carrying that one entry — the
    // name walks the catalog so every row is a definition of its own.
    let mut rows: Vec<(String, Value)> = EXTENDED_DIMENSION_KEYS
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                format!("the listed entry {:?}: {:?}", entry.0, entry.1),
                extended_with_dimensions(EXTENDED_METRICS[index].name, &[*entry]),
            )
        })
        .collect();
    rows.extend([
        (
            "every listed entry at once".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[8].name, &EXTENDED_DIMENSION_KEYS),
        ),
        (
            // "The Namespace parameter must be omitted or set to AWS/RUM": the entry rule
            // holds under both, so a row states each.
            "a listed entry under the extended-metrics namespace".to_string(),
            with(
                extended_with_dimensions(EXTENDED_METRICS[9].name, &EXTENDED_DIMENSION_KEYS[1..3]),
                "Namespace",
                json!(EXTENDED_NAMESPACE),
            ),
        ),
        (
            // The rule bounds the entries a definition HAS; carrying none breaks nothing.
            "an extended metric carrying no dimension at all".to_string(),
            extended(EXTENDED_METRICS[10].name),
        ),
        (
            // "For extended metrics, valid values for the entries in this field are the
            // following" — a CUSTOM metric's dimensions are its own, so its names are free.
            "a custom metric whose dimension name the extended list does not pin".to_string(),
            custom_dimensioned("CheckoutStarts", &[("metadata.title", "PageTitle")]),
        ),
        (
            "a custom metric naming a listed path under a dimension of its own".to_string(),
            custom_dimensioned("CheckoutBrowsers", &[("metadata.browserName", "Browser")]),
        ),
    ]);
    rows
}

/// Definitions the model declares INVALID, each with the reason it is one and the strings
/// the refusal has to name.
fn refused_definitions() -> Vec<(String, Value, Vec<String>)> {
    vec![
        (
            // The instance this rule was filed on: a listed path under a name that is no
            // RUM dimension at all.
            "a listed path mapped to a name the model does not list".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[8].name,
                &[("metadata.browserName", "NotADimension")],
            ),
            vec!["metadata.browserName".to_string(), "BrowserName".to_string()],
        ),
        (
            "a listed path mapped to another entry's dimension name".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[8].name,
                &[("metadata.browserName", "PageId")],
            ),
            vec!["metadata.browserName".to_string(), "BrowserName".to_string()],
        ),
        (
            // A dimension name is matched as written, so the right name in another case
            // names another dimension.
            "a listed path mapped to its dimension name in the wrong case".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[9].name, &[("metadata.osName", "OsName")]),
            vec!["metadata.osName".to_string(), "OSName".to_string()],
        ),
        (
            "the details-keyed entry mapped to another entry's name".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[11].name,
                &[("event_details.fileType", "OSName")],
            ),
            vec!["event_details.fileType".to_string(), "FileType".to_string()],
        ),
        (
            // A field path is matched as written too.
            "a listed path spelled in the wrong case".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[10].name, &[("metadata.pageid", "PageId")]),
            vec!["metadata.pageid".to_string()],
        ),
        (
            "the first segment of a listed path alone".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[12].name, &[("metadata", "BrowserName")]),
            vec!["metadata".to_string()],
        ),
        (
            "a listed path with a segment too many".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[13].name,
                &[("metadata.pageId.suffix", "PageId")],
            ),
            vec!["metadata.pageId.suffix".to_string()],
        ),
        (
            // The path reads a field a RUM event has, which the extended list still does
            // not name.
            "a RUM event field the extended list does not name".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[0].name, &[("metadata.title", "PageTitle")]),
            vec!["metadata.title".to_string()],
        ),
        (
            // The instance this rule was filed on: a path that is no RUM event field.
            "a path that is no RUM event field at all".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[1].name,
                &[("not_a_rum_field.x", "Whatever")],
            ),
            vec!["not_a_rum_field.x".to_string()],
        ),
        (
            // Every entry is judged, so one bad entry is refused beside good ones.
            "one entry outside the list beside two listed ones".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[2].name,
                &[
                    EXTENDED_DIMENSION_KEYS[0],
                    ("metadata.referrer", "Referrer"),
                    EXTENDED_DIMENSION_KEYS[4],
                ],
            ),
            vec!["metadata.referrer".to_string()],
        ),
    ]
}

#[tokio::test]
async fn every_listed_dimension_entry_is_created_and_read_back() {
    let (router, _engine) = emulator();
    let monitor = "dimension-entries-accepted";
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
    // The create reports none of the definitions it created.
    assert_reports_no_created_definitions("accepted", &answered);

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
    let update_monitor = "dimension-entries-updated";
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
async fn no_dimension_entry_outside_the_list_is_stored_by_either_operation() {
    let (router, _engine) = emulator();
    let monitor = "dimension-entries-refused";
    seeded_monitor(&router, "refused", monitor).await;

    // One definition the model declares valid beside the invalid ones, so the batch shows
    // what the model asks for: the bad definitions fail and the valid one still succeeds.
    let valid = extended_with_dimensions(EXTENDED_METRICS[3].name, &EXTENDED_DIMENSION_KEYS[0..2]);
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
    // the entry that caused it.
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
        for wanted in named {
            assert!(
                error["ErrorMessage"]
                    .as_str()
                    .is_some_and(|text| text.contains(wanted)),
                "case {case:?}: expected the error message to name {wanted:?}, got {error}"
            );
        }
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
        for wanted in named {
            assert!(
                message(&body).is_some_and(|text| text.contains(wanted)),
                "case {case:?}: expected the refusal to name {wanted:?}, got {body}"
            );
        }
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
