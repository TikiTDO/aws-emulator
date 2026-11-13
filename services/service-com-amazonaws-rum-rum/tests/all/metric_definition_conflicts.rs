// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `BatchCreateRumMetricDefinitions` answers for a
//! definition the addressed destination ALREADY holds.
//!
//! The rule under test: a definition IS the metric it defines — the members a request
//! carries — and one extended-metrics destination holds at most one definition of a given
//! metric, so a requested definition that destination already holds is a resource that
//! already exists: that entry of the batch is refused with its own per-item
//! `ConflictException` naming the definition the request asked for, the stored definition
//! keeps the id and the content the call that created it wrote, and the rest of the batch
//! still succeeds. Deleting the stored definition frees the identity, after which the same
//! request creates it again.
//!
//! The per-item error carries the requested definition printed as the service prints a
//! modelled structure: the shape's name over its members in the order the model declares
//! them, each as `name=value`, with the literal `null` for a member the request left out.
//! Source: `smithy_query(query="shape", shape="MetricDefinitionRequest")` for the member
//! list and its order, and
//! `smithy_query(query="operation", operation="BatchCreateRumMetricDefinitions")` for the
//! modelled `ConflictException` the `ErrorCode` names.
//!
//! The table spans the rule rather than one request: its rows differ along WHICH members the
//! definition carries (a bare custom metric; one carrying every optional member; one
//! carrying a two-entry `DimensionKeys`, so the printed map is pinned; an extended metric,
//! whose `Namespace` is absent) and along the `MetricDestination` the definition lives
//! under. Each row then checks the rest of the rule: a batch mixing the conflict with a
//! definition the destination does not hold, that a definition differing in ONE member is
//! no conflict at all, and that the identical request succeeds once the stored definition is
//! deleted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — and reads what is stored back through `BatchGetRumMetricDefinitions`,
//! which is the operation that reports a created definition: the create's own answer carries
//! no definition at all.

use serde_json::{json, Value};

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, with, EXTENDED_DIMENSION_KEYS,
    EXTENDED_METRICS,
};
use crate::support::{
    all_metric_definitions, already_defined_message, assert_definition_conflicts,
    assert_metric_definition, assert_reports_no_created_definitions,
    batch_create_metric_definitions, call, conflicting_metric_definitions, create,
    created_metric_definitions, emulator, experiment_arn, metric_definition_id,
    metrics_destination_query, percent_encoded, put_metrics_destination, CONFLICT_CODE,
};

/// Both values `MetricDestination` admits: an `Evidently` destination is identified by the
/// ARN of the experiment that receives the metrics and a `CloudWatch` one carries none.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";
const EVIDENTLY: &str = "Evidently";

/// One row: the definition to create twice, and the destination it lives under.
struct Row {
    case: &'static str,
    definition: Value,
    destination: &'static str,
    /// The experiment an `Evidently` row's destination names; `None` for a `CloudWatch` one.
    experiment: Option<&'static str>,
}

/// The table. Each row's definition differs from the others in which members it carries, so
/// the printed structure a conflict quotes is exercised with every optional member present
/// and absent.
fn rows() -> Vec<Row> {
    let browser = EXTENDED_DIMENSION_KEYS[1];
    let country = EXTENDED_DIMENSION_KEYS[4];
    vec![
        Row {
            case: "a custom metric carrying only its name and namespace",
            definition: custom("Signups", "MyApp/Funnel"),
            destination: CLOUD_WATCH,
            experiment: None,
        },
        Row {
            case: "a custom metric carrying every optional member",
            definition: with(
                with(
                    with(
                        custom("CheckoutLatency", "MyApp/Checkout"),
                        "ValueKey",
                        json!("event_details.latency"),
                    ),
                    "UnitLabel",
                    json!("Milliseconds"),
                ),
                "EventPattern",
                json!(event_pattern("com.example.checkout", &[browser.0])),
            ),
            destination: EVIDENTLY,
            experiment: Some("checkout"),
        },
        Row {
            case: "a custom metric carrying two dimension entries",
            definition: with(
                with(
                    custom("PageViews", "MyApp/Pages"),
                    "EventPattern",
                    json!(event_pattern("com.example.page", &[browser.0, country.0])),
                ),
                "DimensionKeys",
                dimension_keys(&[browser, country]),
            ),
            destination: CLOUD_WATCH,
            experiment: None,
        },
        Row {
            case: "an extended metric, whose namespace the request leaves out",
            definition: extended(EXTENDED_METRICS[8].name),
            destination: EVIDENTLY,
            experiment: Some("errors"),
        },
    ]
}

/// A definition of ANOTHER metric the destination does not hold: the same name under another
/// unit label, which is one member's difference and so a metric of its own.
fn differing(definition: &Value) -> Value {
    with(definition.clone(), "UnitLabel", json!("Count"))
}

#[tokio::test]
async fn a_definition_a_destination_already_holds_is_reported_as_that_entrys_own_conflict() {
    for (index, row) in rows().into_iter().enumerate() {
        let (router, _engine) = emulator();
        let case = row.case;
        let monitor = format!("conflict-{index}");
        let arn = row.experiment.map(experiment_arn);
        create(
            &router,
            case,
            json!({"Name": monitor, "Domain": "example.com"}),
        )
        .await;
        put_metrics_destination(&router, case, &monitor, row.destination, arn.as_deref()).await;

        // The definition the destination holds, and the id the create minted for it.
        let created = created_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            std::slice::from_ref(&row.definition),
        )
        .await;
        let id = metric_definition_id(case, &created[0]);

        // The IDENTICAL request again: one per-item conflict carrying the requested
        // definition and the message the printed structure spells, and no definition in
        // `MetricDefinitions`.
        let reply = batch_create_metric_definitions(
            &router,
            &monitor,
            row.destination,
            arn.as_deref(),
            std::slice::from_ref(&row.definition),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the repeated create to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let answered = reply.json(case);
        assert_definition_conflicts(case, &answered, std::slice::from_ref(&row.definition));
        // The only entry was refused, and the create reports nothing it created anyway.
        assert_reports_no_created_definitions(case, &answered);
        // The message states the definition the request carried, member by member, with
        // `null` for each one it left out.
        let stated = answered["Errors"][0]["ErrorMessage"]
            .as_str()
            .unwrap_or_default();
        assert!(
            stated.starts_with("Metric definition already exists: MetricDefinitionRequest(name="),
            "case {case:?}: expected the printed structure, got {stated:?}"
        );
        for member in ["ValueKey", "UnitLabel", "DimensionKeys", "EventPattern", "Namespace"] {
            if row.definition.get(member).is_none() {
                let printed = format!("{}=null", lower_camel(member));
                assert!(
                    stated.contains(&printed),
                    "case {case:?}: expected the absent {member} to print as {printed:?}, got \
                     {stated:?}"
                );
            }
        }

        // The pre-existing definition is untouched: the destination holds exactly it, under
        // the id the first call minted and with the content that call wrote.
        let held =
            all_metric_definitions(&router, case, &monitor, row.destination, arn.as_deref(), None)
                .await;
        assert_eq!(
            held.keys().cloned().collect::<Vec<String>>(),
            vec![id.clone()],
            "case {case:?}: the refused create changed what the destination holds: {held:?}"
        );
        assert_metric_definition(case, &held[&id], &row.definition, &id);

        // A batch mixing the conflict with a definition the destination does NOT hold: the
        // valid one is created and only the repeated one is refused.
        let other = differing(&row.definition);
        let reply = batch_create_metric_definitions(
            &router,
            &monitor,
            row.destination,
            arn.as_deref(),
            &[row.definition.clone(), other.clone()],
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the mixed batch to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let mixed = reply.json(case);
        assert_definition_conflicts(case, &mixed, std::slice::from_ref(&row.definition));
        // The entry the destination does NOT hold is created, which the SAME answer reports
        // nothing about: it is seen by reading the destination back, under the id the create
        // minted for it.
        assert_reports_no_created_definitions(case, &mixed);
        let held =
            all_metric_definitions(&router, case, &monitor, row.destination, arn.as_deref(), None)
                .await;
        assert_eq!(
            held.len(),
            2,
            "case {case:?}: expected the definition differing in one member to be created \
             beside the stored one, got {held:?}"
        );
        let other_id = held
            .iter()
            .find(|(held_id, _)| *held_id != &id)
            .map(|(held_id, _)| held_id.clone())
            .unwrap_or_else(|| panic!("case {case:?}: the addition was not stored: {held:?}"));
        assert_metric_definition(case, &held[&other_id], &other, &other_id);

        // Deleting the stored definition frees the identity: the IDENTICAL request now
        // creates it again, under an id of its own.
        let deleted = call(
            &router,
            "DELETE",
            &format!(
                "/rummetrics/{monitor}/metrics?{}&metricDefinitionIds={}",
                metrics_destination_query(row.destination, arn.as_deref()),
                percent_encoded(&id)
            ),
            None,
        )
        .await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected the delete to answer 200, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        let recreated = created_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            std::slice::from_ref(&row.definition),
        )
        .await;
        let fresh_id = metric_definition_id(case, &recreated[0]);
        assert_ne!(
            fresh_id, id,
            "case {case:?}: expected the re-created definition to carry a freshly minted id"
        );
        assert_metric_definition(case, &recreated[0], &row.definition, &fresh_id);

        // And it conflicts again, now under the id this create minted.
        let again = conflicting_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            std::slice::from_ref(&row.definition),
        )
        .await;
        assert_metric_definition(case, &again[0], &row.definition, &fresh_id);
    }
}

#[tokio::test]
async fn the_conflict_is_keyed_on_the_definition_rather_than_on_its_name() {
    let (router, _engine) = emulator();
    let case = "one name, two patterns";
    let monitor = "conflict-by-content";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(&router, case, monitor, CLOUD_WATCH, None).await;

    // One metric NAME, each definition matching a different event: the name alone is not the
    // identity, so both are created and coexist.
    let first = with(
        custom("Conversions", "MyApp/Funnel"),
        "EventPattern",
        json!(event_pattern("com.example.checkout", &[])),
    );
    let second = with(
        custom("Conversions", "MyApp/Funnel"),
        "EventPattern",
        json!(event_pattern("com.example.signup", &[])),
    );
    let created = created_metric_definitions(
        &router,
        case,
        monitor,
        CLOUD_WATCH,
        None,
        &[first.clone(), second.clone()],
    )
    .await;
    let first_id = metric_definition_id(case, &created[0]);
    let second_id = metric_definition_id(case, &created[1]);
    assert_ne!(
        first_id, second_id,
        "case {case:?}: expected the two patterns to be two definitions"
    );

    // Each of them, re-sent, is its OWN conflict: the message quotes the definition that was
    // asked for, which is what tells the two apart.
    for requested in [&first, &second] {
        let reply =
            batch_create_metric_definitions(&router, monitor, CLOUD_WATCH, None, std::slice::from_ref(requested))
                .await;
        let answered = reply.json(case);
        assert_eq!(
            answered["Errors"][0]["ErrorCode"].as_str(),
            Some(CONFLICT_CODE),
            "case {case:?}: expected a conflict for {requested}, got {answered}"
        );
        let expected = already_defined_message(requested);
        assert_eq!(
            answered["Errors"][0]["ErrorMessage"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {answered}"
        );
    }

    // And both definitions are still there, each under the id its create minted.
    let held = all_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, None).await;
    assert_eq!(
        held.len(),
        2,
        "case {case:?}: expected both definitions to be held, got {held:?}"
    );
    assert_metric_definition(case, &held[&first_id], &first, &first_id);
    assert_metric_definition(case, &held[&second_id], &second, &second_id);
}

/// The lowerCamel spelling a printed structure names a member by: the model's member name
/// with its first character lowercased.
fn lower_camel(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}
