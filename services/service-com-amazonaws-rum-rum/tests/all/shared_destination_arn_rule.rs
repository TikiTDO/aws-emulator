// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an optional ARN member every metrics call of this service applies,
//! driven through each of the six operations that used to carry its own copy of it:
//! `PutRumMetricsDestination`, `DeleteRumMetricsDestination`,
//! `BatchCreateRumMetricDefinitions`, `BatchDeleteRumMetricDefinitions`,
//! `BatchGetRumMetricDefinitions` and `UpdateRumMetricDefinition` (and, on the put, the
//! `IamRoleArn` it writes through).
//!
//! The rule under test: surrounding whitespace is not part of an ARN, and an ARN that is
//! absent — or empty once that whitespace is gone — is "no ARN", so two spellings of one
//! destination's `(Destination, DestinationArn)` pair address the ONE stored destination
//! whichever of those operations names it, and what the put stores is the ARN without the
//! whitespace.
//!
//! Each row of the table differs along the spelling the rule generalises over (no ARN at
//! all; an ARN as written; the same ARN with whitespace before it, after it, on both
//! sides, as a tab and a newline, and as several spaces), on a destination of its own, and
//! every row is driven through all six operations in turn, so the shared path is reached
//! from each of them. A row addresses the destination through the padded spelling and reads
//! it back through the plain one (and one row the other way round), so a reading that
//! trimmed in only one of the two places would leave the pair addressing two destinations
//! and fail the row.
//!
//! A value that is wholly empty or wholly whitespace is NOT a row here: it is outside
//! `DestinationArn`'s modelled pattern, so the member's own constraint refuses it before
//! any handler runs, which `batch_get_metric_definitions_reads.rs` already pins.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings, the modelled constraints, the handler and the response envelope are exercised
//! rather than just a handler body.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    all_metric_definitions, call, create, emulator, experiment_arn, metric_definition_id,
    metrics_destination_query, metrics_role_arn, percent_encoded,
};

/// The two values `MetricDestination` admits: a `CloudWatch` destination carries no ARN and
/// an `Evidently` one is identified by the ARN of the experiment it names.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";
const EVIDENTLY: &str = "Evidently";

/// How one request spells the ARN of the destination it addresses: what goes before and
/// after the ARN itself.
///
/// `DestinationArn` is modelled `@length(min: 0, max: 2048)` with
/// `@pattern(arn:[^:]*:[^:]*:[^:]*:[^:]*:.*)`, which Smithy matches anywhere in the value,
/// so a value carrying whitespace around an ARN reaches a handler.
/// Source: `smithy_query(query="shape", shape="DestinationArn")`.
#[derive(Debug, Clone, Copy)]
struct Padding {
    before: &'static str,
    after: &'static str,
}

impl Padding {
    /// This spelling of `arn`.
    fn spell(&self, arn: &str) -> String {
        format!("{}{arn}{}", self.before, self.after)
    }
}

/// One row: the destination kind, the spelling the request that CREATED the destination
/// used, and the spelling every later request addresses it by.
#[derive(Debug)]
struct Row {
    case: &'static str,
    destination: &'static str,
    /// The experiment this row's `Evidently` destination names; `None` for the row whose
    /// destination has no ARN at all.
    experiment: Option<&'static str>,
    /// How the put that stored the destination spelled its ARN.
    stored_as: Padding,
    /// How every later call spells the same ARN.
    addressed_as: Padding,
}

/// The ARN as written, with nothing around it.
const PLAIN: Padding = Padding {
    before: "",
    after: "",
};

/// The table: the "no ARN" destination, then one row per way whitespace can surround an
/// ARN, plus the mirror case where it is the STORED spelling that carries the whitespace.
const ROWS: [Row; 8] = [
    Row {
        case: "a destination with no ARN",
        destination: CLOUD_WATCH,
        experiment: None,
        stored_as: PLAIN,
        addressed_as: PLAIN,
    },
    Row {
        case: "an ARN as written",
        destination: EVIDENTLY,
        experiment: Some("checkout-copy"),
        stored_as: PLAIN,
        addressed_as: PLAIN,
    },
    Row {
        case: "an ARN with a leading space",
        destination: EVIDENTLY,
        experiment: Some("home-page-hero"),
        stored_as: PLAIN,
        addressed_as: Padding {
            before: " ",
            after: "",
        },
    },
    Row {
        case: "an ARN with a trailing space",
        destination: EVIDENTLY,
        experiment: Some("search-ranking"),
        stored_as: PLAIN,
        addressed_as: Padding {
            before: "",
            after: " ",
        },
    },
    Row {
        case: "an ARN with a space on both sides",
        destination: EVIDENTLY,
        experiment: Some("cart-reminder"),
        stored_as: PLAIN,
        addressed_as: Padding {
            before: " ",
            after: " ",
        },
    },
    Row {
        case: "an ARN wrapped in a tab and a newline",
        destination: EVIDENTLY,
        experiment: Some("pricing-banner"),
        stored_as: PLAIN,
        addressed_as: Padding {
            before: "\t",
            after: "\n",
        },
    },
    Row {
        case: "an ARN padded with several spaces",
        destination: EVIDENTLY,
        experiment: Some("signup-flow"),
        stored_as: PLAIN,
        addressed_as: Padding {
            before: "   ",
            after: "  ",
        },
    },
    Row {
        case: "an ARN stored with whitespace and addressed without it",
        destination: EVIDENTLY,
        experiment: Some("onboarding-tips"),
        stored_as: Padding {
            before: " ",
            after: " ",
        },
        addressed_as: PLAIN,
    },
];

/// The metric definition each row creates: a custom metric, so it carries the `Namespace`
/// the model requires of one ("The namespace that you use can't start with AWS/").
fn definition(namespace: &str, unit: &str) -> Value {
    json!({"Name": "CheckoutsCompleted", "Namespace": namespace, "UnitLabel": unit})
}

/// The `(Destination, DestinationArn)` pair as a request BODY carries it, for the three
/// operations that take it in one.
fn addressed_body(destination: &str, destination_arn: Option<&str>) -> Value {
    let mut body = json!({"Destination": destination});
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
    }
    body
}

/// `PutRumMetricsDestination`: `POST /rummetrics/{AppMonitorName}/metricsdestination`,
/// carrying the pair and — for an Evidently destination — the role RUM writes through.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    iam_role_arn: Option<&str>,
) {
    let mut body = addressed_body(destination, destination_arn);
    if let Some(role) = iam_role_arn {
        body["IamRoleArn"] = json!(role);
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

/// Every destination `ListRumMetricsDestinations` reports for one app monitor.
async fn listed_destinations(router: &MultiServiceRouter, case: &str, monitor: &str) -> Vec<Value> {
    let reply = call(
        router,
        "GET",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["Destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `BatchCreateRumMetricDefinitions`: `POST /rummetrics/{AppMonitorName}/metrics`, under the
/// destination the pair addresses. Returns the id the call minted.
async fn create_definition(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definition: Value,
) -> String {
    let mut body = addressed_body(destination, destination_arn);
    body["MetricDefinitions"] = json!([definition]);
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected BatchCreateRumMetricDefinitions to reach the destination \
         this pair addresses and answer 200, got {} {:?}",
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
    let echoed = answered["MetricDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        echoed.len(),
        1,
        "case {case:?}: expected the create to echo the definition it created, got {answered}"
    );
    let echoed_id = metric_definition_id(case, &echoed[0]);
    // The id the create echoed is read back through `BatchGetRumMetricDefinitions` — under
    // the same spelling of the pair, which is what makes this a check of the ARN rule rather
    // than of the create's own answer.
    let created = all_metric_definitions(router, case, monitor, destination, destination_arn, None)
        .await
        .into_values()
        .collect::<Vec<Value>>();
    assert_eq!(
        created.len(),
        1,
        "case {case:?}: expected one created definition, got {created:?}"
    );
    let held_id = metric_definition_id(case, &created[0]);
    assert_eq!(
        held_id, echoed_id,
        "case {case:?}: expected the stored definition to be held under the id the create \
         echoed ({echoed_id:?}), got {held_id:?}"
    );
    held_id
}

#[tokio::test]
async fn every_metrics_operation_reads_a_destination_arn_the_same_way() {
    for (index, row) in ROWS.iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("shared-arn-{index:02}");
        let case = row.case;
        create(
            &router,
            case,
            json!({"Name": monitor, "Domain": "example.com"}),
        )
        .await;

        // The ARN this row's destination is identified by, in the two spellings the row
        // uses: the one the put stores it under and the one every later call addresses it
        // by. A `CloudWatch` destination has no ARN in either.
        let arn = row.experiment.map(experiment_arn);
        let stored_arn = arn.as_ref().map(|arn| row.stored_as.spell(arn));
        let addressed_arn = arn.as_ref().map(|arn| row.addressed_as.spell(arn));
        let plain_arn = arn.clone();
        // An Evidently destination is written through an IAM role, and the role the put
        // stores is read the same way as the ARN. The second put below replaces it.
        let first_role = arn.as_ref().map(|_| metrics_role_arn("rum-writer"));
        let second_role = arn
            .as_ref()
            .map(|_| row.addressed_as.spell(&metrics_role_arn("rum-rewriter")));

        // The destination this row addresses, created once under its stored spelling.
        put_destination(
            &router,
            case,
            &monitor,
            row.destination,
            stored_arn.as_deref(),
            first_role.as_deref(),
        )
        .await;

        // 1. The batch create reaches the destination the put stored, under the addressed
        //    spelling of the same ARN.
        let definition_id = create_definition(
            &router,
            case,
            &monitor,
            row.destination,
            addressed_arn.as_deref(),
            definition("SharedArn/Rule", "Count"),
        )
        .await;

        // 2. The batch read reports it under BOTH spellings — the one that created it and
        //    the plain one — which is what "one destination" means.
        for (spelling, label) in [(&addressed_arn, "addressed"), (&plain_arn, "plain")] {
            let held = all_metric_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                spelling.as_deref(),
                None,
            )
            .await;
            assert_eq!(
                held.keys().cloned().collect::<Vec<_>>(),
                vec![definition_id.clone()],
                "case {case:?}: the {label} spelling must report the one definition stored \
                 under this destination, got {held:?}"
            );
        }

        // 3. The update addresses the same destination and replaces the definition there.
        let update_body = {
            let mut body = addressed_body(row.destination, addressed_arn.as_deref());
            body["MetricDefinitionId"] = json!(definition_id);
            body["MetricDefinition"] = definition("SharedArn/Updated", "Milliseconds");
            body
        };
        let updated = call(
            &router,
            "PATCH",
            &format!("/rummetrics/{monitor}/metrics"),
            Some(update_body),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected UpdateRumMetricDefinition to answer 200, got {} {:?}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        let held = all_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            plain_arn.as_deref(),
            None,
        )
        .await;
        let reported = held.get(&definition_id).unwrap_or_else(|| {
            panic!("case {case:?}: the updated definition is no longer reported: {held:?}")
        });
        assert_eq!(
            reported["Namespace"].as_str(),
            Some("SharedArn/Updated"),
            "case {case:?}: expected the update to reach the definition the plain spelling \
             reports, got {reported}"
        );

        // 4. A second put under the addressed spelling UPDATES the one destination rather
        //    than adding a second one, and stores both ARNs without the whitespace the
        //    request spelled them with.
        put_destination(
            &router,
            case,
            &monitor,
            row.destination,
            addressed_arn.as_deref(),
            second_role.as_deref(),
        )
        .await;
        let listed = listed_destinations(&router, case, &monitor).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: two spellings of one ARN must address one destination, got \
             {listed:?}"
        );
        // `MetricDestinationSummary` reports a destination that has no ARN or no role with
        // those members as the empty string.
        assert_eq!(
            listed[0]["DestinationArn"].as_str(),
            Some(plain_arn.as_deref().unwrap_or_default()),
            "case {case:?}: expected the stored DestinationArn to carry no surrounding \
             whitespace, got {}",
            listed[0]
        );
        assert_eq!(
            listed[0]["IamRoleArn"].as_str(),
            Some(
                second_role
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
            ),
            "case {case:?}: expected the stored IamRoleArn to carry no surrounding \
             whitespace, got {}",
            listed[0]
        );

        // 5. The batch delete addresses the same destination and removes the definition the
        //    create stored there.
        let deleted = call(
            &router,
            "DELETE",
            &format!(
                "/rummetrics/{monitor}/metrics?{}&metricDefinitionIds={}",
                metrics_destination_query(row.destination, addressed_arn.as_deref()),
                percent_encoded(&definition_id)
            ),
            None,
        )
        .await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected BatchDeleteRumMetricDefinitions to answer 200, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        let answered = deleted.json(case);
        assert_eq!(
            answered["Errors"],
            json!([]),
            "case {case:?}: expected the delete to refuse nothing, got {:?}",
            answered["Errors"]
        );
        let held = all_metric_definitions(
            &router,
            case,
            &monitor,
            row.destination,
            plain_arn.as_deref(),
            None,
        )
        .await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected the delete to reach the definition the plain spelling \
             reports, got {held:?}"
        );

        // 6. The delete of the destination itself addresses the one the put created, so the
        //    monitor is left holding none.
        let removed = call(
            &router,
            "DELETE",
            &format!(
                "/rummetrics/{monitor}/metricsdestination?{}",
                metrics_destination_query(row.destination, addressed_arn.as_deref())
            ),
            None,
        )
        .await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        let listed = listed_destinations(&router, case, &monitor).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the delete to remove the destination the put stored, \
             got {listed:?}"
        );
    }
}
