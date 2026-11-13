// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `BatchCreateRumMetricDefinitions` ANSWERS about the
//! definitions it stored: nothing. Its `MetricDefinitions` member is the EMPTY list however
//! many definitions the call created, so the answer reports only the per-definition
//! `Errors`, and the `MetricDefinitionId` a caller addresses a new definition by is learned
//! by reading the destination back with `BatchGetRumMetricDefinitions`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings, the modelled constraints, the handler and the response envelope are all
//! exercised rather than just a handler body.
//!
//! The rule is asserted over a TABLE rather than one request: the rows differ in how many
//! definitions one call carries (one, two, five), which `MetricDestination` they are created
//! under, and which optional members a definition uses — from an extended metric that names
//! no namespace through a custom metric carrying every optional member it may. A further
//! case drives a batch only PART of which was stored, and another one a batch NONE of which
//! was stored, since the answer is the same empty list either way. The last case shows the
//! create still did the work: the ids the read-back reports address the new definitions
//! through `UpdateRumMetricDefinition` and `BatchDeleteRumMetricDefinitions`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, with, EXTENDED_DIMENSION_KEYS,
    EXTENDED_METRICS,
};
use crate::support::{
    all_metric_definitions, assert_metric_definition, assert_reports_no_created_definitions,
    batch_create_metric_definitions, call, create, emulator, experiment_arn, metric_definition_ids,
    metric_definitions_page, metrics_destination_query, percent_encoded, put_metrics_destination,
    reported_metric_definitions, update_metric_definition, CONFLICT_CODE,
};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";
const EVIDENTLY: &str = "Evidently";

/// One row: the batch to create, and the destination it is created under.
struct Row {
    case: &'static str,
    destination: &'static str,
    /// The experiment an `Evidently` destination names; a `CloudWatch` one has none.
    experiment: Option<&'static str>,
    definitions: Vec<Value>,
}

/// The table. The rows differ along what the rule generalises over: the batch's SIZE, the
/// destination kind, and which of a definition's optional members carry a value.
fn rows() -> Vec<Row> {
    let browser = EXTENDED_DIMENSION_KEYS[1];
    let country = EXTENDED_DIMENSION_KEYS[4];
    vec![
        Row {
            case: "one extended metric, no namespace of its own",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![extended(EXTENDED_METRICS[0].name)],
        },
        Row {
            case: "one custom metric carrying every optional member",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![with(
                with(
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
                    json!(event_pattern(
                        "com.example.checkout",
                        &[browser.0, country.0]
                    )),
                ),
                "DimensionKeys",
                dimension_keys(&[browser, country]),
            )],
        },
        Row {
            case: "two definitions, one of each kind",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![
                extended(EXTENDED_METRICS[1].name),
                custom("Signups", "MyApp/Funnel"),
            ],
        },
        Row {
            case: "two definitions under an Evidently destination",
            destination: EVIDENTLY,
            experiment: Some("answer-ab"),
            definitions: vec![
                extended(EXTENDED_METRICS[12].name),
                custom("Abandonments", "MyApp/Checkout"),
            ],
        },
        Row {
            case: "five definitions, extended and custom mixed",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![
                extended(EXTENDED_METRICS[2].name),
                custom("Orders", "MyApp/Orders"),
                with(
                    with(
                        custom("Dimensioned", "MyApp/Orders"),
                        "EventPattern",
                        json!(event_pattern("com.example.order", &[country.0])),
                    ),
                    "DimensionKeys",
                    dimension_keys(&[country]),
                ),
                custom("Refunds", "MyApp/Orders"),
                with(
                    custom("Latency", "MyApp/Orders"),
                    "UnitLabel",
                    json!("Milliseconds"),
                ),
            ],
        },
    ]
}

/// One app monitor with the destination `row` names, ready to hold definitions.
async fn seeded(router: &MultiServiceRouter, row: &Row, monitor: &str) -> Option<String> {
    create(
        router,
        row.case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    let arn = row.experiment.map(experiment_arn);
    put_metrics_destination(router, row.case, monitor, row.destination, arn.as_deref()).await;
    arn
}

#[tokio::test]
async fn a_batch_create_reports_none_of_the_definitions_it_created() {
    for (index, row) in rows().into_iter().enumerate() {
        let (router, _engine) = emulator();
        let case = row.case;
        let monitor = format!("answer-{index}");
        let arn = seeded(&router, &row, &monitor).await;

        let reply = batch_create_metric_definitions(
            &router,
            &monitor,
            row.destination,
            arn.as_deref(),
            &row.definitions,
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

        // The answer reports no definition at all, however many the call stored.
        assert_reports_no_created_definitions(case, &answered);

        // And the call DID store them: the destination holds one definition per requested
        // entry, each the definition that was asked for under an id of its own.
        let held = all_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            None,
        )
        .await;
        assert_eq!(
            held.len(),
            row.definitions.len(),
            "case {case:?}: expected {} stored definition(s), got {held:?}",
            row.definitions.len()
        );
        let ids = metric_definition_ids(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            &row.definitions,
        )
        .await;
        for (requested, id) in row.definitions.iter().zip(ids.iter()) {
            let reported = held.get(id).unwrap_or_else(|| {
                panic!("case {case:?}: the read reported {id:?}, which holds nothing: {held:?}")
            });
            assert_metric_definition(case, reported, requested, id);
        }
    }
}

#[tokio::test]
async fn a_batch_create_reports_no_definition_whether_it_stored_part_of_the_batch_or_none_of_it() {
    let (router, _engine) = emulator();
    let case = "part of the batch is refused";
    let monitor = "answer-partial";
    let row = Row {
        case,
        destination: CLOUD_WATCH,
        experiment: None,
        definitions: Vec::new(),
    };
    seeded(&router, &row, monitor).await;

    // One definition the destination already holds (a conflict), one the service will not
    // store at all (a reserved namespace), and two it stores.
    let repeated = custom("Repeated", "MyApp/Partial");
    assert_reports_no_created_definitions(
        case,
        &batch_create_metric_definitions(
            &router,
            monitor,
            CLOUD_WATCH,
            None,
            std::slice::from_ref(&repeated),
        )
        .await
        .json(case),
    );
    let held_repeated = metric_definition_ids(
        &router,
        case,
        monitor,
        CLOUD_WATCH,
        None,
        std::slice::from_ref(&repeated),
    )
    .await;
    let refused = custom("Reserved", "AWS/Billing");
    let stored = [
        custom("StoredOne", "MyApp/Partial"),
        extended(EXTENDED_METRICS[4].name),
    ];

    let reply = batch_create_metric_definitions(
        &router,
        monitor,
        CLOUD_WATCH,
        None,
        &[
            repeated.clone(),
            stored[0].clone(),
            refused.clone(),
            stored[1].clone(),
        ],
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected the mixed batch to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);

    // The two entries it stored are reported nowhere in the answer — only the conflict and
    // the refusal are, in `Errors`.
    assert_reports_no_created_definitions(case, &answered);
    let errors = answered["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        2,
        "case {case:?}: expected two per-definition errors, got {answered}"
    );
    assert_eq!(
        errors[0]["ErrorCode"].as_str(),
        Some(CONFLICT_CODE),
        "case {case:?}: expected the resubmission to be the conflict, got {:?}",
        errors[0]
    );
    assert_eq!(
        &errors[1]["MetricDefinition"], &refused,
        "case {case:?}: expected the second error to carry the refused definition, got {:?}",
        errors[1]
    );

    // The destination holds the original and the two additions, and nothing for the refused
    // definition.
    let added = metric_definition_ids(&router, case, monitor, CLOUD_WATCH, None, &stored).await;
    let held = all_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, None).await;
    let mut expected: Vec<String> = added.clone();
    expected.push(held_repeated[0].clone());
    expected.sort();
    assert_eq!(
        held.keys().cloned().collect::<Vec<String>>(),
        expected,
        "case {case:?}: expected the destination to hold exactly the stored definitions, got \
         {held:?}"
    );

    // A batch NONE of which was stored answers the same way: every entry is a conflict, and
    // the answer still reports the empty list rather than the definitions the destination
    // holds.
    let none_stored = batch_create_metric_definitions(
        &router,
        monitor,
        CLOUD_WATCH,
        None,
        &[repeated.clone(), stored[0].clone()],
    )
    .await
    .json(case);
    assert_reports_no_created_definitions(case, &none_stored);
    assert_eq!(
        none_stored["Errors"]
            .as_array()
            .map(|errors| errors.len())
            .unwrap_or_default(),
        2,
        "case {case:?}: expected both entries to be refused, got {none_stored}"
    );
}

#[tokio::test]
async fn the_read_back_id_addresses_the_definition_the_create_stored() {
    for (index, row) in rows().into_iter().enumerate() {
        let (router, _engine) = emulator();
        let case = row.case;
        let monitor = format!("answer-handle-{index}");
        let arn = seeded(&router, &row, &monitor).await;

        let answered = batch_create_metric_definitions(
            &router,
            &monitor,
            row.destination,
            arn.as_deref(),
            &row.definitions,
        )
        .await
        .json(case);
        assert_reports_no_created_definitions(case, &answered);
        let ids = metric_definition_ids(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            &row.definitions,
        )
        .await;

        // The FIRST id the read reported: the update replaces the definition it names, which
        // the read-back under that same id shows.
        let replacement = with(
            row.definitions[0].clone(),
            "UnitLabel",
            json!("Microseconds"),
        );
        update_metric_definition(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            &ids[0],
            &replacement,
        )
        .await;
        let held = all_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
            None,
        )
        .await;
        let reported = held.get(&ids[0]).unwrap_or_else(|| {
            panic!("case {case:?}: nothing is held under {:?}", ids[0])
        });
        assert_metric_definition(case, reported, &replacement, &ids[0]);

        // And the same id deletes it: the delete reports it as deleted and the destination
        // holds only the definitions the batch's other entries created.
        let deleted = call(
            &router,
            "DELETE",
            &format!(
                "/rummetrics/{monitor}/metrics?{}&metricDefinitionIds={}",
                metrics_destination_query(row.destination, arn.as_deref()),
                percent_encoded(&ids[0])
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
        assert_eq!(
            deleted.json(case)["MetricDefinitionIds"],
            json!([ids[0]]),
            "case {case:?}: expected the delete to report the id it deleted, got {:?}",
            deleted.json(case)
        );
        let page = metric_definitions_page(
            &router,
            &monitor,
            &metrics_destination_query(row.destination, arn.as_deref()),
        )
        .await;
        let remaining: Vec<String> = reported_metric_definitions(&page.json(case))
            .iter()
            .map(|reported| reported["MetricDefinitionId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            remaining,
            ids[1..].to_vec(),
            "case {case:?}: expected only the batch's other definitions to remain, got \
             {remaining:?}"
        );
    }
}
