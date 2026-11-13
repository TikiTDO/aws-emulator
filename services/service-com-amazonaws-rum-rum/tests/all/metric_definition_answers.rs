// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what CloudWatch RUM's metric-definition and
//! metrics-destination calls ANSWER: which members a body carries, which namespace a read
//! reports, what a partly failing batch leaves behind, and how each refusal of the add
//! path is classified and worded.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings, the modelled constraints, the handler and the response envelope are all
//! exercised rather than just a handler body. What a write did is asserted through
//! `BatchGetRumMetricDefinitions`, the operation that reports stored definitions.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! * a batch create answers its per-item `Errors` and reports NONE of the definitions it
//!   created: `MetricDefinitions` is the empty list however many entries the call stored,
//!   so the stored definitions and the ids the call minted are seen only through the read;
//! * a definition that named no namespace is an extended metric, which RUM publishes to
//!   its own vended namespace, so a read reports `AWS/RUM` for it and the namespace the
//!   request named for a custom metric;
//! * a batch delete deletes every id the destination holds a definition under even when
//!   another id in the same call fails, and a failing item's `ErrorMessage` is the error
//!   class's own fixed text with no part of the request in it;
//! * an update that names a destination the app monitor does not have is refused as that
//!   missing DESTINATION, with the requested destination as the message's only variable
//!   part, while an update whose destination exists and whose `MetricDefinitionId` names
//!   no definition fails as the service's own add failure: the `retryable`
//!   `InternalServerException` the model declares, naming the app monitor;
//! * deleting a metrics destination is `@idempotent`: a destination that is not configured
//!   on an EXISTING app monitor is the empty 200 success, and only a missing app monitor is
//!   the modelled 404.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{custom, extended, with, EXTENDED_METRICS};
use crate::support::{
    all_metric_definitions, assert_reports_no_created_definitions, batch_create_metric_definitions,
    call, create, emulator, experiment_arn, message, metric_definition_id,
    metrics_destination_query, percent_encoded, put_metrics_destination,
    update_metric_definition_reply, EXTENDED_METRICS_NAMESPACE,
};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";
const EVIDENTLY: &str = "Evidently";

/// The `ErrorCode` a batch delete reports for an id the destination holds no definition
/// under, and the wire code of the two refusals the add path answers.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const INTERNAL_CODE: &str = "InternalServerException";

/// The whole `ErrorMessage` such an id is reported with: the error class's own fixed
/// sentence, with no part of the request interpolated into it.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// One app monitor holding one extended-metrics destination, ready to hold definitions.
async fn destination_holding_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) {
    create(
        router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(router, case, monitor, destination, destination_arn).await;
}

/// The raw body of a reply, for a case that is about which members are ON THE WIRE rather
/// than about the values a parsed body reports (restJson1 reads an absent member and an
/// explicit `null` the same way, so only the bytes tell them apart).
fn body_text(reply: &crate::support::Reply) -> String {
    String::from_utf8_lossy(&reply.body).into_owned()
}

/// `BatchDeleteRumMetricDefinitions`: `DELETE /rummetrics/{AppMonitorName}/metrics`, with
/// the ids bound to the repeated `@httpQuery("metricDefinitionIds")` parameter.
async fn batch_delete(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    ids: &[String],
) -> crate::support::Reply {
    let mut path = format!(
        "/rummetrics/{monitor}/metrics?{}",
        metrics_destination_query(destination, destination_arn)
    );
    for id in ids {
        path.push_str(&format!("&metricDefinitionIds={}", percent_encoded(id)));
    }
    call(router, "DELETE", &path, None).await
}

/// `DeleteRumMetricsDestination`: `DELETE /rummetrics/{AppMonitorName}/metricsdestination`,
/// with the pair in the query string.
async fn delete_destination(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) -> crate::support::Reply {
    call(
        router,
        "DELETE",
        &format!(
            "/rummetrics/{monitor}/metricsdestination?{}",
            metrics_destination_query(destination, destination_arn)
        ),
        None,
    )
    .await
}

/// The ids a batch delete reports under `MetricDefinitionIds`.
fn deleted_ids(case: &str, answered: &Value) -> BTreeSet<String> {
    answered["MetricDefinitionIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the delete reported no MetricDefinitionIds"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a reported id is not a string"))
                .to_string()
        })
        .collect()
}

/// One batch of definitions a create case sends, with the destination it sends them to.
struct CreateCase {
    case: &'static str,
    destination: &'static str,
    /// The experiment an `Evidently` destination names; a `CloudWatch` one has none.
    experiment: Option<&'static str>,
    definitions: Vec<Value>,
}

/// The create table: batches of one, two, three and five definitions, over extended
/// metrics (which name no namespace), custom metrics (which name one of their own) and
/// mixtures of both, under either `MetricDestination`.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "one extended metric",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![extended(EXTENDED_METRICS[0].name)],
        },
        CreateCase {
            case: "one custom metric",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![custom("Checkouts", "MyApp/Funnel")],
        },
        CreateCase {
            case: "two definitions, one of each kind",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![
                extended(EXTENDED_METRICS[1].name),
                custom("Signups", "MyApp/Funnel"),
            ],
        },
        CreateCase {
            case: "three custom metrics under an Evidently destination",
            destination: EVIDENTLY,
            experiment: Some("create-answers"),
            definitions: vec![
                custom("MetricA", "BatchAnswers"),
                custom("MetricB", "BatchAnswers"),
                custom("MetricC", "BatchAnswers"),
            ],
        },
        CreateCase {
            case: "five definitions, extended and custom mixed",
            destination: CLOUD_WATCH,
            experiment: None,
            definitions: vec![
                extended(EXTENDED_METRICS[2].name),
                custom("Orders", "MyApp/Orders"),
                extended(EXTENDED_METRICS[3].name),
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

#[tokio::test]
async fn a_batch_create_reports_its_errors_and_none_of_what_it_created() {
    let (router, _engine) = emulator();

    for (index, row) in create_cases().into_iter().enumerate() {
        let case = row.case;
        let monitor = format!("create-answer-{index}");
        let arn = row.experiment.map(experiment_arn);
        destination_holding_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            arn.as_deref(),
        )
        .await;

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
            body_text(&reply)
        );
        // The body carries the `@required` per-item `Errors` as the EMPTY list a call that
        // refused nothing reports, and `MetricDefinitions` as the EMPTY list whatever the
        // call created — both present rather than left out, which is asserted on the BYTES
        // because restJson1 reads an absent member and an empty list differently on the
        // wire while a parsed body does not tell them apart.
        let text = body_text(&reply);
        assert!(
            text.contains(r#""Errors":[]"#),
            "case {case:?}: expected the empty Errors list on the wire, got {text:?}"
        );
        assert!(
            text.contains(r#""MetricDefinitions":[]"#),
            "case {case:?}: expected the empty MetricDefinitions list on the wire, got {text:?}"
        );
        assert_reports_no_created_definitions(case, &reply.json(case));

        // And every definition it was asked for IS stored, each under an id of its own:
        // what the create does not report is reported by the read.
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
        for requested in &row.definitions {
            let name = requested["Name"].as_str().unwrap_or_default();
            assert!(
                held.values().any(|reported| reported["Name"] == json!(name)),
                "case {case:?}: no definition named {name:?} was stored, got {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_partly_refused_batch_create_answers_only_the_items_that_failed() {
    let (router, _engine) = emulator();
    let case = "a mixed batch";
    let monitor = "create-answer-mixed";
    destination_holding_definitions(&router, case, monitor, CLOUD_WATCH, None).await;

    // A definition the service will not store beside two it will, so the answer has an
    // `Errors` list with something in it — and reports none of the two it created.
    let good = [
        custom("Accepted", "MyApp/Mixed"),
        extended(EXTENDED_METRICS[4].name),
    ];
    let bad = custom("Refused", "AWS/Billing");
    let reply = batch_create_metric_definitions(
        &router,
        monitor,
        CLOUD_WATCH,
        None,
        &[good[0].clone(), bad.clone(), good[1].clone()],
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        body_text(&reply)
    );
    let answered = reply.json(case);
    // The two definitions the service stored are reported nowhere in the answer.
    assert_reports_no_created_definitions(case, &answered);
    let errors = answered["Errors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no Errors: {answered}"));
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: expected one per-item error, got {errors:?}"
    );
    assert_eq!(
        errors[0]["MetricDefinition"], bad,
        "case {case:?}: expected the error to carry the definition that caused it, got {:?}",
        errors[0]
    );

    let held = all_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, None).await;
    assert_eq!(
        held.len(),
        good.len(),
        "case {case:?}: expected only the accepted definitions to be stored, got {held:?}"
    );
}

#[tokio::test]
async fn a_definition_that_names_no_namespace_is_read_back_under_the_vended_namespace() {
    let (router, _engine) = emulator();

    // Every extended metric the model pins a combination for, sent BOTH ways the model
    // admits ("The Namespace parameter must be omitted or set to AWS/RUM"), beside custom
    // metrics, whose own namespace is reported as it was named. One monitor per row, so
    // each row's destination holds exactly the definition it created.
    let mut rows: Vec<(String, Value, String)> = Vec::new();
    for entry in EXTENDED_METRICS {
        rows.push((
            format!("{} with no Namespace", entry.name),
            extended(entry.name),
            EXTENDED_METRICS_NAMESPACE.to_string(),
        ));
        rows.push((
            format!("{} naming the vended namespace", entry.name),
            with(
                extended(entry.name),
                "Namespace",
                json!(EXTENDED_METRICS_NAMESPACE),
            ),
            EXTENDED_METRICS_NAMESPACE.to_string(),
        ));
    }
    for namespace in ["MyApp/Checkout", "a", "aws/rum", "AWSCustom"] {
        rows.push((
            format!("a custom metric under {namespace}"),
            custom("Conversions", namespace),
            namespace.to_string(),
        ));
    }

    for (index, (case, definition, expected)) in rows.iter().enumerate() {
        let case = case.as_str();
        let monitor = format!("namespace-read-{index}");
        destination_holding_definitions(&router, case, &monitor, CLOUD_WATCH, None).await;
        let reply = batch_create_metric_definitions(
            &router,
            &monitor,
            CLOUD_WATCH,
            None,
            std::slice::from_ref(definition),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the create to answer 200, got {} {:?}",
            reply.status,
            body_text(&reply)
        );

        let held = all_metric_definitions(&router, case, &monitor, CLOUD_WATCH, None, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected one stored definition, got {held:?}"
        );
        let (_, reported) = held.iter().next().expect("one stored definition");
        assert_eq!(
            reported["Namespace"],
            json!(expected),
            "case {case:?}: expected Namespace {expected:?}, got {:?}",
            reported["Namespace"]
        );
    }

    // The two kinds side by side under ONE destination: the extended metric's namespace is
    // the vended one and the custom metric's is its own, so the member still tells them
    // apart.
    let case = "an extended metric beside a custom one";
    let monitor = "namespace-read-siblings";
    destination_holding_definitions(&router, case, monitor, CLOUD_WATCH, None).await;
    let siblings = [
        extended(EXTENDED_METRICS[5].name),
        custom("CustomClickCount", "MyApp/Clicks"),
    ];
    let reply =
        batch_create_metric_definitions(&router, monitor, CLOUD_WATCH, None, &siblings).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected the create to answer 200, got {} {:?}",
        reply.status,
        body_text(&reply)
    );
    let held = all_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, None).await;
    for (name, expected) in [
        (EXTENDED_METRICS[5].name, EXTENDED_METRICS_NAMESPACE),
        ("CustomClickCount", "MyApp/Clicks"),
    ] {
        let reported = held
            .values()
            .find(|reported| reported["Name"] == json!(name))
            .unwrap_or_else(|| panic!("case {case:?}: no definition named {name:?}: {held:?}"));
        assert_eq!(
            reported["Namespace"],
            json!(expected),
            "case {case:?}: expected {name} to report Namespace {expected:?}, got {:?}",
            reported["Namespace"]
        );
    }
}

/// One partial-delete row: which of the five seeded definitions the request names, and at
/// which positions of the id list it also names an id nothing is stored under.
struct PartialDelete {
    case: &'static str,
    named: &'static [usize],
    /// Positions, in the request's id list, the unknown ids are inserted at.
    unknown_at: &'static [usize],
}

#[tokio::test]
async fn a_partial_batch_delete_removes_every_id_it_holds_a_definition_under() {
    // The rule generalises over WHERE in the batch the failing items sit and how many
    // there are, so the table walks the unknown ids from the front of the list to the
    // back, and from one of them to more than the valid ids.
    let rows: [PartialDelete; 6] = [
        PartialDelete {
            case: "an unknown id after the valid ones",
            named: &[0],
            unknown_at: &[1],
        },
        PartialDelete {
            case: "an unknown id before the valid ones",
            named: &[1, 2],
            unknown_at: &[0],
        },
        PartialDelete {
            case: "an unknown id between the valid ones",
            named: &[0, 3],
            unknown_at: &[1],
        },
        PartialDelete {
            case: "unknown ids at both ends",
            named: &[2],
            unknown_at: &[0, 2],
        },
        PartialDelete {
            case: "more unknown ids than valid ones",
            named: &[4],
            unknown_at: &[0, 1, 3],
        },
        PartialDelete {
            case: "every named id valid",
            named: &[0, 1, 2, 3, 4],
            unknown_at: &[],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let case = row.case;
        let monitor = format!("partial-delete-{index}");
        destination_holding_definitions(&router, case, &monitor, CLOUD_WATCH, None).await;

        // Five definitions, each a custom metric with a name of its own, so a survivor is
        // recognisable by name whatever id it was minted under.
        let seeded: Vec<Value> = (0..5)
            .map(|at| custom(&format!("Metric{at}"), "PartialDelete"))
            .collect();
        let reply =
            batch_create_metric_definitions(&router, &monitor, CLOUD_WATCH, None, &seeded).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: seeding the definitions must succeed, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
        let held = all_metric_definitions(&router, case, &monitor, CLOUD_WATCH, None, None).await;
        let ids: Vec<String> = (0..5)
            .map(|at| {
                let name = json!(format!("Metric{at}"));
                held.values()
                    .find(|reported| reported["Name"] == name)
                    .map(|reported| metric_definition_id(case, reported))
                    .unwrap_or_else(|| panic!("case {case:?}: {name} was not stored: {held:?}"))
            })
            .collect();

        // The id list the request carries: the named definitions' ids with the unknown
        // ids inserted at the row's positions.
        let mut requested: Vec<String> = row.named.iter().map(|at| ids[*at].clone()).collect();
        let unknown: Vec<String> = (0..row.unknown_at.len())
            .map(|n| format!("00000000-0000-4000-8000-00000000000{n}"))
            .collect();
        for (at, id) in row.unknown_at.iter().zip(unknown.iter()) {
            requested.insert((*at).min(requested.len()), id.clone());
        }

        let reply = batch_delete(&router, &monitor, CLOUD_WATCH, None, &requested).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
        let answered = reply.json(case);

        // Every valid id is reported deleted even though another item of the same call
        // failed…
        let expected_deleted: BTreeSet<String> =
            row.named.iter().map(|at| ids[*at].clone()).collect();
        assert_eq!(
            deleted_ids(case, &answered),
            expected_deleted,
            "case {case:?}: expected the valid ids to be reported deleted, got {answered}"
        );

        // …each failing id is reported on its own, with the error class's fixed text…
        let errors = answered["Errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the delete reported no Errors: {answered}"))
            .clone();
        assert_eq!(
            errors.len(),
            unknown.len(),
            "case {case:?}: expected {} per-id error(s), got {errors:?}",
            unknown.len()
        );
        for failed in &unknown {
            let reported = errors
                .iter()
                .find(|entry| entry["MetricDefinitionId"] == json!(failed))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no error reported for {failed:?}: {errors:?}")
                });
            assert_eq!(
                reported["ErrorCode"],
                json!(NOT_FOUND_CODE),
                "case {case:?}: expected ErrorCode {NOT_FOUND_CODE:?}, got {reported}"
            );
            assert_eq!(
                reported["ErrorMessage"],
                json!(NOT_FOUND_MESSAGE),
                "case {case:?}: expected ErrorMessage {NOT_FOUND_MESSAGE:?}, got {reported}"
            );
        }

        // …and the state agrees with the answer: the destination holds exactly the
        // definitions the call did not name.
        let survivors: BTreeSet<String> =
            all_metric_definitions(&router, case, &monitor, CLOUD_WATCH, None, None)
                .await
                .values()
                .map(|reported| reported["Name"].as_str().unwrap_or_default().to_string())
                .collect();
        let expected_survivors: BTreeSet<String> = (0..5)
            .filter(|at| !row.named.contains(at))
            .map(|at| format!("Metric{at}"))
            .collect();
        assert_eq!(
            survivors, expected_survivors,
            "case {case:?}: expected the destination to hold only the definitions the \
             request did not name"
        );
    }
}

#[tokio::test]
async fn an_update_naming_a_destination_the_monitor_does_not_have_names_that_destination() {
    let (router, _engine) = emulator();
    let case = "a destination that was never put";
    let monitor = "update-no-destination";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;

    // Each row names a destination this monitor has no record of: either kind on a monitor
    // with none at all, and one kind while the OTHER is configured, so the refusal is about
    // the destination the request named rather than about the monitor.
    let experiment = experiment_arn("never-put");
    let rows: [(&str, &str, Option<&str>); 3] = [
        ("CloudWatch, never put", CLOUD_WATCH, None),
        ("Evidently, never put", EVIDENTLY, Some(experiment.as_str())),
        ("CloudWatch while Evidently is configured", CLOUD_WATCH, None),
    ];
    for (index, (row, destination, destination_arn)) in rows.iter().enumerate() {
        if index == 2 {
            // Configure the other kind, so the monitor does have A destination.
            put_metrics_destination(
                &router,
                row,
                monitor,
                EVIDENTLY,
                Some(&experiment_arn("configured")),
            )
            .await;
        }
        let reply = update_metric_definition_reply(
            &router,
            monitor,
            destination,
            *destination_arn,
            "00000000-0000-4000-8000-000000000000",
            &custom("Checkouts", "MyApp/Funnel"),
        )
        .await;
        assert_eq!(
            reply.status,
            404,
            "case {row:?}: expected HTTP 404, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
        assert_eq!(
            reply.error_code(row),
            NOT_FOUND_CODE,
            "case {row:?}: expected {NOT_FOUND_CODE}, got {:?}",
            body_text(&reply)
        );
        // The requested `Destination` is the only variable part of the message: the app
        // monitor exists, so naming it would report the wrong missing resource.
        let expected = format!("Error adding metrics - Destination {destination} not found.");
        let document = reply.json(row);
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {row:?}: expected the message {expected:?}, got {:?}",
            message(&document)
        );
    }
}

#[tokio::test]
async fn an_update_of_an_id_the_destination_holds_nothing_under_is_the_servers_own_failure() {
    let (router, _engine) = emulator();
    let case = "setup";
    let monitor = "update-absent-definition";
    let experiment = experiment_arn("update-absent");
    destination_holding_definitions(&router, case, monitor, CLOUD_WATCH, None).await;
    put_metrics_destination(&router, case, monitor, EVIDENTLY, Some(&experiment)).await;

    // One definition under EACH destination, so a row can address a stored id through the
    // wrong destination as well as an id nothing was ever created under.
    let stored = custom("Checkouts", "MyApp/Funnel");
    for (destination, arn) in [(CLOUD_WATCH, None), (EVIDENTLY, Some(experiment.as_str()))] {
        let reply = batch_create_metric_definitions(
            &router,
            monitor,
            destination,
            arn,
            std::slice::from_ref(&stored),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: seeding {destination} must succeed, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
    }
    let cloud_watch_held =
        all_metric_definitions(&router, case, monitor, CLOUD_WATCH, None, None).await;
    let cloud_watch_id = cloud_watch_held
        .keys()
        .next()
        .expect("the CloudWatch destination holds one definition")
        .clone();

    // The rule is about the id naming no definition UNDER THE DESTINATION the request
    // names, whatever the id looks like and whatever the replacement defines.
    let replacement = with(custom("Checkouts", "MyApp/Funnel"), "UnitLabel", json!("Count"));
    let rows: Vec<(&str, &str, Option<&str>, String, Value)> = vec![
        (
            "a UUID-shaped id nothing was created under",
            CLOUD_WATCH,
            None,
            "00000000-0000-4000-8000-000000000000".to_string(),
            replacement.clone(),
        ),
        (
            "an id that is not UUID-shaped at all",
            CLOUD_WATCH,
            None,
            "x".to_string(),
            replacement.clone(),
        ),
        (
            "the longest id the model admits",
            CLOUD_WATCH,
            None,
            "z".repeat(255),
            custom("Signups", "MyApp/Funnel"),
        ),
        (
            "an id stored under the monitor's OTHER destination",
            CLOUD_WATCH,
            None,
            "unreachable".to_string(),
            replacement.clone(),
        ),
        (
            "an absent id under the Evidently destination",
            EVIDENTLY,
            Some(experiment.as_str()),
            cloud_watch_id.clone(),
            replacement.clone(),
        ),
    ];

    // The message's fixed text, with the app monitor from the request path as its only
    // variable part.
    let expected = format!("Error adding metric definitions for app monitor {monitor} to destination.");
    for (row, destination, destination_arn, id, definition) in rows {
        let reply = update_metric_definition_reply(
            &router,
            monitor,
            destination,
            destination_arn,
            &id,
            &definition,
        )
        .await;
        assert_eq!(
            reply.status,
            500,
            "case {row:?}: expected HTTP 500, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
        assert_eq!(
            reply.error_code(row),
            INTERNAL_CODE,
            "case {row:?}: expected {INTERNAL_CODE}, got {:?}",
            body_text(&reply)
        );
        let document = reply.json(row);
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {row:?}: expected the message {expected:?}, got {:?}",
            message(&document)
        );
    }

    // None of those refusals wrote anything: each destination still holds the one
    // definition it was seeded with, unchanged.
    for (destination, arn) in [(CLOUD_WATCH, None), (EVIDENTLY, Some(experiment.as_str()))] {
        let held = all_metric_definitions(&router, case, monitor, destination, arn, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: a refused update changed what {destination} holds: {held:?}"
        );
        let (_, reported) = held.iter().next().expect("one stored definition");
        assert_eq!(
            reported["UnitLabel"],
            Value::Null,
            "case {case:?}: a refused update replaced the stored definition: {reported}"
        );
    }
}

#[tokio::test]
async fn deleting_a_metrics_destination_that_is_not_configured_is_a_no_op_success() {
    let (router, _engine) = emulator();
    let case = "setup";
    let monitor = "destination-idempotent";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;

    // The rule holds for either `MetricDestination`, before anything was ever put and
    // after a put was undone: delete, put, delete, delete — every delete a 200 with an
    // empty body.
    let experiment = experiment_arn("idempotent");
    let rows: [(&str, &str, Option<&str>); 2] = [
        ("CloudWatch", CLOUD_WATCH, None),
        ("Evidently", EVIDENTLY, Some(experiment.as_str())),
    ];
    for (row, destination, destination_arn) in rows {
        for attempt in ["never configured", "after the put", "repeated"] {
            if attempt == "after the put" {
                put_metrics_destination(&router, row, monitor, destination, destination_arn).await;
            }
            let reply = delete_destination(&router, monitor, destination, destination_arn).await;
            assert_eq!(
                reply.status,
                200,
                "case {row:?} ({attempt}): expected HTTP 200, got {} {:?}",
                reply.status,
                body_text(&reply)
            );
            assert_eq!(
                reply.json(row),
                json!({}),
                "case {row:?} ({attempt}): expected an empty body, got {:?}",
                body_text(&reply)
            );
        }
    }

    // What it is NOT: an app monitor that is not stored is still the modelled 404, so a
    // caller can tell "no such monitor" from "that destination is already gone".
    for (row, destination, destination_arn) in rows {
        let reply =
            delete_destination(&router, "no-such-monitor", destination, destination_arn).await;
        assert_eq!(
            reply.status,
            404,
            "case {row:?}: expected HTTP 404 for an app monitor that does not exist, got {} {:?}",
            reply.status,
            body_text(&reply)
        );
        assert_eq!(
            reply.error_code(row),
            NOT_FOUND_CODE,
            "case {row:?}: expected {NOT_FOUND_CODE}, got {:?}",
            body_text(&reply)
        );
    }
}
