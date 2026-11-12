// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled constraints of a metric definition's
//! `DimensionKeys` map, on both operations that accept a `MetricDefinitionRequest`:
//! `BatchCreateRumMetricDefinitions` and `UpdateRumMetricDefinition`.
//!
//! The rule under test: every constraint `DimensionKeysMap` declares on its KEY shape and on
//! its VALUE shape is applied before a definition is stored, so a request breaking one is
//! refused with HTTP 400 `ValidationException` stating the clause, and a request inside them
//! is stored and read back whole. The two halves the framework's boundary walk leaves
//! unapplied are the point: a map key is not a constrained leaf for that walk at all, and a
//! `@pattern` it cannot compile — `DimensionName`'s opens with a negative look-ahead — is
//! treated as matching.
//!
//! The constraints, cited from the model (`smithy_query(query="shape",
//! shape="DimensionKeysMap,DimensionKey,DimensionName")` against
//! `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`), are
//! spelled out below rather than read back out of the crate, so these cases pin the bounds
//! and the rendering instead of agreeing with them by construction.
//!
//! The tables span the rule rather than one request: the refusal table varies which half of
//! an entry offends (the field path, the dimension name), which requirement it breaks (a
//! length minimum, a length maximum one character past the published bound, the pattern's
//! leading guard, the pattern's non-blank run) and how many clauses one entry produces, and
//! one case sends the offending definition as the SECOND element of a batch, so the position
//! a clause reports is pinned too; the acceptance table carries each published bound exactly,
//! a dimension name that merely RESEMBLES what the pattern excludes, and a catalog entry the
//! report did not cite. An acceptance row is sent as whichever KIND of metric may carry an
//! entry of that shape at all — a custom metric where the entry is not one of the six pairs
//! the model pins for an extended metric, an extended metric where it is — since the bound
//! under test belongs to the map, declared once on the shape every `DimensionKeys` member
//! targets. Every accepted row is read back through `BatchGetRumMetricDefinitions`, and every
//! refused one is shown to have changed nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    dimension_keys, event_pattern, extended_with_dimensions, EXTENDED_DIMENSION_KEYS,
};
use crate::support::{
    all_metric_definitions, call, create, emulator, message, put_metrics_destination, Reply,
};

/// The error code a constraint refusal of a metrics call carries: the `ValidationException`
/// this service's model declares (`@httpError(400)`).
const VALIDATION_CODE: &str = "ValidationException";

/// The destination the definitions below are created under — `DimensionKeys` is the member
/// "to use only if you are sending the metric to CloudWatch".
const DESTINATION: &str = "CloudWatch";

/// The extended metric every definition below is built from: its catalog row pins a
/// `ValueKey` of null and an `EventPattern`, so each row's only interesting difference is
/// its `DimensionKeys`.
const METRIC: &str = "JsErrorCount";

/// The CUSTOM metric a row whose entry the extended list does not name is sent as: a name
/// of the caller's own, a namespace of the caller's own (which is what makes a definition a
/// custom metric) and the event type its pattern matches.
const CUSTOM_METRIC: &str = "CheckoutDimensioned";
const CUSTOM_NAMESPACE: &str = "MyApp/Checkout";
const CUSTOM_EVENT_TYPE: &str = "com.example.checkout";

/// `DimensionKey`'s modelled `@length`: the bounds the map's KEY — a dotted event-field path
/// — is declared within.
const KEY_MIN_LENGTH: usize = 1;
const KEY_MAX_LENGTH: usize = 280;

/// `DimensionName`'s modelled `@length`: the bounds the map's VALUE — the CloudWatch
/// dimension name — is declared within.
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 255;

/// `DimensionName`'s modelled `@pattern` as a refusal QUOTES it: the expression the WHOLE
/// value must match. The model anchors its START only (`^(?!:).*[^\s].*`), so the `^` is not
/// part of the character grammar a caller satisfies, and the end the model leaves open is
/// already spelled with the unconstrained run the expression ends in.
const NAME_PATTERN: &str = r"(?!:).*[^\s].*";

/// The path segment a definition's dimension map is reported under: the lowerCamel spelling
/// of `MetricDefinitionRequest$DimensionKeys`.
const DIMENSION_KEYS_MEMBER: &str = "dimensionKeys";

/// The path prefix the single-definition update reports a violation inside its
/// `MetricDefinition` member under.
const UPDATE_PREFIX: &str = "metricDefinition";

/// The path prefix the batch create reports a violation inside the `position`-th element of
/// its `MetricDefinitions` list under: the member, the element's 1-BASED position and the
/// step into the collection.
fn batch_prefix(position: usize) -> String {
    format!("metricDefinitions.{position}.member")
}

/// The requirement a `@length` minimum states.
fn min_length(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a `@length` maximum states.
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a `@pattern` violation states.
fn pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {NAME_PATTERN}")
}

/// One row of the refusal table: an entry of a definition's `DimensionKeys`, and the clauses
/// the refusal states for it — in the order the boundary walk writes an entry, the KEY before
/// its VALUE, each clause carrying the value echoed and the requirement it failed.
struct Offending {
    case: &'static str,
    key: String,
    name: String,
    clauses: Vec<(String, String)>,
}

/// The whole message a definition carrying `row`'s entry is refused with, reported under
/// `prefix`: AWS's envelope over one clause per offending (value, requirement), each stated
/// at the position the entry holds in the map.
fn refusal_message(prefix: &str, row: &Offending) -> String {
    let rendered: Vec<String> = row
        .clauses
        .iter()
        .map(|(value, requirement)| {
            format!(
                "Value '{value}' at '{prefix}.{DIMENSION_KEYS_MEMBER}.{}' failed to satisfy \
                 constraint: {requirement}",
                row.key
            )
        })
        .collect();
    match rendered.len() {
        1 => format!("1 validation error detected: {}", rendered[0]),
        n => format!("{n} validation errors detected: {}", rendered.join("; ")),
    }
}

/// Entries the model's constraints reject, each with the clause(s) its refusal states.
fn offending_entries() -> Vec<Offending> {
    let long_key = "p".repeat(KEY_MAX_LENGTH + 1);
    let long_name = "D".repeat(NAME_MAX_LENGTH + 1);
    vec![
        Offending {
            case: "a field path that is the empty string",
            key: String::new(),
            name: "BrowserName".to_string(),
            clauses: vec![(String::new(), min_length(KEY_MIN_LENGTH))],
        },
        Offending {
            case: "a field path one character past DimensionKey's maximum",
            key: long_key.clone(),
            name: "PageId".to_string(),
            clauses: vec![(long_key, max_length(KEY_MAX_LENGTH))],
        },
        Offending {
            case: "a dimension name opening with the colon the pattern's guard excludes",
            key: "metadata.pageId".to_string(),
            name: ":PageId".to_string(),
            clauses: vec![(":PageId".to_string(), pattern_requirement())],
        },
        Offending {
            case: "a dimension name that is one blank character",
            key: "metadata.pageId".to_string(),
            name: " ".to_string(),
            clauses: vec![(" ".to_string(), pattern_requirement())],
        },
        Offending {
            case: "a dimension name of whitespace alone",
            key: "metadata.browserName".to_string(),
            name: " \t ".to_string(),
            clauses: vec![(" \t ".to_string(), pattern_requirement())],
        },
        Offending {
            // `.` matches no line break, so the non-blank character the pattern's `[^\s]`
            // demands has to stand on the value's first line.
            case: "a dimension name whose only non-blank character is past a line break",
            key: "metadata.browserName".to_string(),
            name: " \n Browser".to_string(),
            clauses: vec![(" \n Browser".to_string(), pattern_requirement())],
        },
        Offending {
            case: "a dimension name that is the empty string",
            key: "metadata.deviceType".to_string(),
            name: String::new(),
            clauses: vec![(String::new(), min_length(NAME_MIN_LENGTH))],
        },
        Offending {
            case: "a dimension name one character past DimensionName's maximum",
            key: "metadata.deviceType".to_string(),
            name: long_name.clone(),
            clauses: vec![(long_name, max_length(NAME_MAX_LENGTH))],
        },
        Offending {
            // Both halves of one entry offend, so one entry is TWO validation errors —
            // stated in the order the walk writes an entry.
            case: "an entry whose field path and dimension name both offend",
            key: String::new(),
            name: ":PageId".to_string(),
            clauses: vec![
                (String::new(), min_length(KEY_MIN_LENGTH)),
                (":PageId".to_string(), pattern_requirement()),
            ],
        },
    ]
}

/// Entries inside every constraint the model declares, each as the definition that carries
/// it and the reason it is one.
///
/// A row whose entry the extended list does not name is sent as a CUSTOM metric, since for
/// an extended metric the entries themselves are pinned to the six pairs the model lists:
/// the bound under test is the map's, declared once on `DimensionKeysMap`, so it is stated
/// on whichever kind of metric may carry an entry of that shape at all. `DimensionKey`'s
/// MINIMUM of one character is reachable by no valid definition — the shortest field path
/// either kind may name is a RUM event field — so the boundary left to state there is the
/// empty key one character below it, which the refusal table above carries.
fn admitted_entries() -> Vec<(String, Value)> {
    vec![
        (
            "a dimension name of one character, at DimensionName's minimum".to_string(),
            custom_definition(&[("userId".to_string(), "D".to_string())]),
        ),
        (
            "a field path exactly at DimensionKey's maximum".to_string(),
            custom_definition(&[(longest_field_path(), "PageId".to_string())]),
        ),
        (
            "a dimension name exactly at DimensionName's maximum".to_string(),
            custom_definition(&[(
                "metadata.pageId".to_string(),
                "D".repeat(NAME_MAX_LENGTH),
            )]),
        ),
        (
            // The pattern's guard is anchored at the start of the value, so a colon
            // anywhere else is just a character.
            "a dimension name carrying a colon past its first character".to_string(),
            custom_definition(&[("metadata.pageId".to_string(), "Page:Id".to_string())]),
        ),
        (
            // Blank is what the pattern forbids, not leading whitespace.
            "a dimension name opening with whitespace around a non-blank run".to_string(),
            custom_definition(&[(
                "metadata.browserName".to_string(),
                " Browser ".to_string(),
            )]),
        ),
        (
            "a catalog entry the model lists".to_string(),
            definition(&[(
                EXTENDED_DIMENSION_KEYS[3].0.to_string(),
                EXTENDED_DIMENSION_KEYS[3].1.to_string(),
            )]),
        ),
        (
            "every dimension the model lists for an extended metric".to_string(),
            definition(
                &EXTENDED_DIMENSION_KEYS
                    .iter()
                    .map(|(field, dimension)| ((*field).to_string(), (*dimension).to_string()))
                    .collect::<Vec<(String, String)>>(),
            ),
        ),
    ]
}

/// A field path of exactly `DimensionKey`'s maximum length that a definition may name: a
/// RUM event field, then a leaf filling the rest of the bound.
fn longest_field_path() -> String {
    let field = "metadata.";
    format!("{field}{}", "p".repeat(KEY_MAX_LENGTH - field.len()))
}

/// The definition each row is sent as: the extended metric's own pinned combination, with
/// `entries` listed in `DimensionKeys` and their field paths named by the `EventPattern` —
/// so nothing but the entry under test can be what a refusal is about.
fn definition(entries: &[(String, String)]) -> Value {
    extended_with_dimensions(METRIC, &pairs(entries))
}

/// The definition a row whose entry the extended list does not name is sent as: a CUSTOM
/// metric, under a name and a namespace of its own, with `entries` listed in
/// `DimensionKeys` and their field paths named by the `EventPattern`. A custom metric's
/// dimensions are its own, so the rules left on an entry are the modelled constraints under
/// test and the first-level key naming a RUM event field.
fn custom_definition(entries: &[(String, String)]) -> Value {
    let paths: Vec<&str> = entries.iter().map(|(field, _)| field.as_str()).collect();
    json!({
        "Name": CUSTOM_METRIC,
        "Namespace": CUSTOM_NAMESPACE,
        "EventPattern": event_pattern(CUSTOM_EVENT_TYPE, &paths),
        "DimensionKeys": dimension_keys(&pairs(entries)),
    })
}

/// `entries` as the borrowed pairs the shared definition builders take.
fn pairs(entries: &[(String, String)]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|(field, dimension)| (field.as_str(), dimension.as_str()))
        .collect()
}

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

/// `BatchCreateRumMetricDefinitions` with `definitions`, in that order.
async fn batch_create(
    router: &MultiServiceRouter,
    monitor: &str,
    definitions: &[Value],
) -> Reply {
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

/// `UpdateRumMetricDefinition` replacing the definition `id` with `definition`.
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

/// Every definition the destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions`.
async fn stored(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
) -> BTreeMap<String, Value> {
    all_metric_definitions(router, case, monitor, DESTINATION, None, None).await
}

/// Assert `reply` is the refusal `expected`, with the modelled status and code.
fn assert_refused(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        VALIDATION_CODE,
        "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the message {expected:?}, got {stated:?}"
    );
}

#[tokio::test]
async fn an_offending_dimension_entry_is_refused_by_the_batch_create_and_stores_nothing() {
    let (router, _engine) = emulator();
    let monitor = "dimension-constraints-create";
    seeded_monitor(&router, "batch create", monitor).await;

    for row in offending_entries() {
        let requested = definition(&[(row.key.clone(), row.name.clone())]);
        let reply = batch_create(&router, monitor, std::slice::from_ref(&requested)).await;
        assert_refused(row.case, &reply, &refusal_message(&batch_prefix(1), &row));
        let held = stored(&router, row.case, monitor).await;
        assert!(
            held.is_empty(),
            "case {:?}: expected a refused batch create to store nothing, got {held:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_offending_entry_is_reported_at_the_position_its_definition_holds() {
    let (router, _engine) = emulator();
    let monitor = "dimension-constraints-position";
    let case = "an offending entry in the second definition of a batch";
    seeded_monitor(&router, case, monitor).await;

    let row = Offending {
        case,
        key: "metadata.pageId".to_string(),
        name: ":PageId".to_string(),
        clauses: vec![(":PageId".to_string(), pattern_requirement())],
    };
    let admitted = definition(&[(
        EXTENDED_DIMENSION_KEYS[1].0.to_string(),
        EXTENDED_DIMENSION_KEYS[1].1.to_string(),
    )]);
    let offending = definition(&[(row.key.clone(), row.name.clone())]);
    let reply = batch_create(&router, monitor, &[admitted, offending]).await;
    // The clause names the position the offending definition holds, not the first.
    assert_refused(case, &reply, &refusal_message(&batch_prefix(2), &row));
    // A modelled constraint is the REQUEST's to satisfy, so the definition that was inside
    // every constraint is not created either.
    let held = stored(&router, case, monitor).await;
    assert!(
        held.is_empty(),
        "case {case:?}: expected the whole call to be refused and nothing stored, got {held:?}"
    );
}

#[tokio::test]
async fn an_offending_dimension_entry_is_refused_by_the_update_and_changes_nothing() {
    let (router, _engine) = emulator();
    let monitor = "dimension-constraints-update";
    seeded_monitor(&router, "update", monitor).await;

    // One stored definition for the update to address, with a dimension the model lists.
    let baseline = definition(&[(
        EXTENDED_DIMENSION_KEYS[0].0.to_string(),
        EXTENDED_DIMENSION_KEYS[0].1.to_string(),
    )]);
    let reply = batch_create(&router, monitor, std::slice::from_ref(&baseline)).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the baseline create to answer 200, got {} {:?}",
        "update",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let held = stored(&router, "update", monitor).await;
    let (id, created) = held
        .iter()
        .next()
        .map(|(id, reported)| (id.clone(), reported.clone()))
        .unwrap_or_else(|| panic!("case {:?}: the baseline create stored nothing", "update"));

    for row in offending_entries() {
        let requested = definition(&[(row.key.clone(), row.name.clone())]);
        let reply = update(&router, monitor, &id, &requested).await;
        assert_refused(row.case, &reply, &refusal_message(UPDATE_PREFIX, &row));
        let held = stored(&router, row.case, monitor).await;
        assert_eq!(
            held.get(&id),
            Some(&created),
            "case {:?}: expected a refused update to leave the stored definition unchanged, \
             got {held:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_dimension_entry_inside_the_modelled_constraints_is_stored_and_read_back() {
    let (router, _engine) = emulator();
    let monitor = "dimension-constraints-admitted";
    seeded_monitor(&router, "admitted", monitor).await;

    for (case, requested) in admitted_entries() {
        let reply = batch_create(&router, monitor, std::slice::from_ref(&requested)).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the batch create to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let answered = reply.json(&case);
        assert_eq!(
            answered["Errors"],
            json!([]),
            "case {case:?}: expected the definition to be accepted, got {:?}",
            answered["Errors"]
        );
        let created = answered["MetricDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected the create to echo the definition it created, got \
             {answered:?}"
        );
        assert_eq!(
            created[0]["DimensionKeys"], requested["DimensionKeys"],
            "case {case:?}: expected the echoed DimensionKeys to be {:?}, got {:?}",
            requested["DimensionKeys"], created[0]["DimensionKeys"]
        );
        let echoed_id = created[0]["MetricDefinitionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the echo carries no id: {answered:?}"))
            .to_string();

        // Read back through the implemented reader: the map is persisted whole, key for key,
        // under the id the create echoed.
        let held = stored(&router, &case, monitor).await;
        let (id, reported) = held
            .iter()
            .find(|(_, held)| held["DimensionKeys"] == requested["DimensionKeys"])
            .unwrap_or_else(|| panic!("case {case:?}: the definition was not stored: {held:?}"));
        let id = id.clone();
        assert_eq!(
            id, echoed_id,
            "case {case:?}: expected the stored definition to be held under the id the create \
             echoed ({echoed_id:?}), got {id:?}"
        );
        assert_eq!(
            reported["DimensionKeys"], requested["DimensionKeys"],
            "case {case:?}: expected the stored DimensionKeys to be {:?}, got {:?}",
            requested["DimensionKeys"], reported["DimensionKeys"]
        );

        // And the same entry through the update, which judges a definition the same way.
        // The replacement is the definition that was created, carrying a `UnitLabel` the
        // create did not set — so the read-back shows the update replaced it.
        let mut replacement = requested.clone();
        replacement["UnitLabel"] = json!("Count");
        let reply = update(&router, monitor, &id, &replacement).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the update to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let held = stored(&router, &case, monitor).await;
        let reported = held
            .get(&id)
            .unwrap_or_else(|| panic!("case {case:?}: no definition stored under {id:?}"));
        assert_eq!(
            reported["UnitLabel"],
            json!("Count"),
            "case {case:?}: expected the update to have replaced the definition, got {reported:?}"
        );
        assert_eq!(
            reported["DimensionKeys"], requested["DimensionKeys"],
            "case {case:?}: expected the updated DimensionKeys to be {:?}, got {:?}",
            requested["DimensionKeys"], reported["DimensionKeys"]
        );
    }
}
