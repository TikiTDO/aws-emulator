// SPDX-License-Identifier: CC0-1.0

//! `BatchDeleteRumMetricDefinitions`, end to end.
//!
//! The rule under test: the call removes, from ONE extended-metrics destination of one app
//! monitor, the definitions whose ids the request names — "Removes the specified metrics
//! from being sent to an extended metrics destination" — and answers per id: "If some
//! metric definition IDs specified in a BatchDeleteRumMetricDefinitions operations are not
//! valid, those metric definitions fail and return errors, but all valid metric definition
//! IDs in the same operation are still deleted." So an id the destination holds a
//! definition under is deleted and reported under `MetricDefinitionIds` ("The IDs of the
//! metric definitions that were deleted"), an id it holds nothing under is reported as its
//! own entry of `Errors` and changes nothing, and a request that names no place to delete
//! from at all — a monitor or a destination that is not there, a forbidden
//! `Destination`/`DestinationArn` pairing, a batch past the documented maximum of 200 — is
//! refused WHOLE and leaves every definition stored.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the three
//! `@httpQuery` bindings (`destination`, `destinationArn` and the repeated
//! `metricDefinitionIds`), the handler and the response envelope are exercised rather than
//! just a handler body. What a delete did is asserted through `BatchGetRumMetricDefinitions`
//! reading the same map back, never out of the store.
//!
//! The tables span what the rule generalises over: both `MetricDestination` values, how
//! many ids one request names (none, one, several, every one the destination holds), the
//! order it names them in, whether an id is one the destination holds, and a repeated id;
//! the refusal table walks each half of the target that can be missing plus both forbidden
//! pairings and each side of the documented batch bound.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::authorization::{identity_policy, resource_policy, role_arn, Emulator, ACCOUNT_KEY};
use crate::support::{call, create, emulator, message, percent_encoded, Reply, REGION};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// The IAM action this operation is authorized against: the service's own action prefix
/// plus the operation name, as every other operation of this service is.
const ACTION: &str = "rum:BatchDeleteRumMetricDefinitions";

/// Another action of the same service, for the principal that is allowed something else.
const OTHER_ACTION: &str = "rum:BatchGetRumMetricDefinitions";

/// The most metric definition ids one call may name.
/// Source: the model's `BatchDeleteRumMetricDefinitions` documentation — "The maximum
/// number of metric definitions that you can specify in one
/// BatchDeleteRumMetricDefinitions operation is 200."
const MAX_IDS_PER_BATCH: usize = 200;

/// The `ErrorCode` an id the destination holds no definition under is reported with, and
/// the status and wire code a refused call answers.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const VALIDATION_CODE: &str = "ValidationException";
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The whole `ErrorMessage` such an id is reported with: the error class's own fixed
/// sentence, with no part of the request in it — the item is already named by the entry's
/// own `MetricDefinitionId`.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// How many definitions each destination of a row's monitor is seeded with.
const SEEDED: usize = 5;

/// The ARN of an emulated Evidently experiment — the resource an `Evidently` destination
/// names — built with the shared `engine::emulator_arn` helper.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// One extended-metrics destination: the `(Destination, DestinationArn)` pair that
/// identifies it, plus the role an Evidently destination is written through.
#[derive(Debug, Clone)]
struct Target {
    destination: String,
    destination_arn: Option<String>,
}

impl Target {
    /// The `CloudWatch` destination: the model says it carries no `DestinationArn`.
    fn cloud_watch() -> Self {
        Self {
            destination: METRIC_DESTINATIONS[0].to_string(),
            destination_arn: None,
        }
    }

    /// An `Evidently` destination, identified by the experiment that receives the metrics.
    fn evidently(experiment: &str) -> Self {
        Self {
            destination: METRIC_DESTINATIONS[1].to_string(),
            destination_arn: Some(experiment_arn(experiment)),
        }
    }

    /// This destination as the `PutRumMetricsDestination` body that creates it.
    fn put_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
            body["IamRoleArn"] = json!(engine::emulator_arn("iam", "", "role/rum-writer"));
        }
        body
    }

    /// The pair as the JSON body a create names its target by.
    fn addressed_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
        }
        body
    }

    /// The same pair as the query string the delete and the read bind it to
    /// (`@httpQuery("destination")` / `@httpQuery("destinationArn")`).
    fn query(&self) -> String {
        destination_query(&self.destination, self.destination_arn.as_deref())
    }

    /// A fragment of an app-monitor name, so each row of a table gets its own monitor.
    fn slug(&self) -> String {
        self.destination.to_lowercase()
    }
}

/// The `destination`/`destinationArn` query string, for a case that names a pair no
/// destination has.
fn destination_query(destination: &str, destination_arn: Option<&str>) -> String {
    let mut query = format!("destination={}", percent_encoded(destination));
    if let Some(arn) = destination_arn {
        query.push_str(&format!("&destinationArn={}", percent_encoded(arn)));
    }
    query
}

/// One custom metric definition: a name the extended catalog does not list, published to a
/// namespace of its own, which is what the model requires of a custom metric ("The
/// namespace that you use can't start with AWS/").
fn definition(index: usize) -> Value {
    json!({"Name": format!("OrdersPlaced{index}"), "Namespace": "batch-delete"})
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

/// `PutRumMetricsDestination`, the call that must have created a destination before any
/// definition can be created under it.
async fn put_destination(router: &MultiServiceRouter, case: &str, monitor: &str, target: &Target) {
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(target.put_body()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: seeding the destination must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `BatchCreateRumMetricDefinitions`, returning the ids it minted in request order.
///
/// The create's answer echoes one definition per entry it created; the ids returned here are
/// read back through `BatchGetRumMetricDefinitions` and matched to the definitions the
/// request named by the `Name` each carries, which is unique across the seeded rows.
async fn create_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    target: &Target,
    count: usize,
) -> Vec<String> {
    let definitions: Vec<Value> = (0..count).map(definition).collect();
    let mut body = target.addressed_body();
    body["MetricDefinitions"] = Value::Array(definitions);
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: seeding the definitions must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {case:?}: expected the seed to refuse no definition, got {:?}",
        answered["Errors"]
    );
    assert_eq!(
        answered["MetricDefinitions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        count,
        "case {case:?}: expected the create to echo {count} definition(s), got {answered}"
    );
    let held = stored_definitions(router, case, monitor, &target.query()).await;
    assert_eq!(
        held.len(),
        count,
        "case {case:?}: expected {count} definitions to be created, got {held:?}"
    );
    (0..count)
        .map(|index| {
            let wanted = definition(index);
            held.iter()
                .find(|reported| reported["Name"] == wanted["Name"])
                .map(|reported| {
                    reported["MetricDefinitionId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a stored definition carries no id")
                        })
                        .to_string()
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no definition was created for {wanted}: {held:?}")
                })
        })
        .collect()
}

/// `BatchDeleteRumMetricDefinitions`: `DELETE /rummetrics/{AppMonitorName}/metrics`, with
/// the ids bound to the repeated `@httpQuery("metricDefinitionIds")` parameter.
async fn batch_delete(
    router: &MultiServiceRouter,
    monitor: &str,
    query: &str,
    ids: &[String],
) -> Reply {
    let mut path = format!("/rummetrics/{monitor}/metrics?{query}");
    for id in ids {
        path.push_str(&format!("&metricDefinitionIds={}", percent_encoded(id)));
    }
    call(router, "DELETE", &path, None).await
}

/// The ids of every definition a destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions` and following its tokens to the end.
async fn stored_ids(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
) -> BTreeSet<String> {
    stored_definitions(router, case, monitor, query)
        .await
        .into_iter()
        .map(|reported| {
            reported["MetricDefinitionId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every definition a destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions` and following its tokens to the end.
async fn stored_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
) -> Vec<Value> {
    let mut held = Vec::new();
    let mut extra = String::new();
    loop {
        let reply = call(
            router,
            "GET",
            &format!("/rummetrics/{monitor}/metrics?{query}{extra}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected BatchGetRumMetricDefinitions to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        held.extend(
            page["MetricDefinitions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a page reported no MetricDefinitions"))
                .clone(),
        );
        match page["NextToken"].as_str() {
            Some(token) => extra = format!("&nextToken={}", percent_encoded(token)),
            None => return held,
        }
    }
}

/// The `MetricDefinitionIds` a reply reports, as a set.
fn reported_ids(answered: &Value, case: &str) -> BTreeSet<String> {
    answered["MetricDefinitionIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the delete reported no MetricDefinitionIds: {answered}")
        })
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a reported id is not a string"))
                .to_string()
        })
        .collect()
}

/// One row of the delete table: which of the destination's definitions the request names,
/// in that order, and how many ids nothing is stored under it also names.
struct DeleteCase {
    case: &'static str,
    /// Indices into the seeded definitions, in the order the request names them.
    named: &'static [usize],
    /// How many ids no definition is stored under the request also carries.
    unknown: usize,
}

#[tokio::test]
async fn batch_delete_removes_the_named_definitions_and_reports_the_ids_it_holds_none_for() {
    let rows: [DeleteCase; 7] = [
        DeleteCase {
            case: "no id at all",
            named: &[],
            unknown: 0,
        },
        DeleteCase {
            case: "one of several",
            named: &[2],
            unknown: 0,
        },
        DeleteCase {
            case: "several, in a different order from the create",
            named: &[3, 0, 1],
            unknown: 0,
        },
        DeleteCase {
            case: "every definition the destination holds",
            named: &[0, 1, 2, 3, 4],
            unknown: 0,
        },
        DeleteCase {
            case: "stored ids beside ids nothing is stored under",
            named: &[1, 4],
            unknown: 2,
        },
        DeleteCase {
            case: "only ids nothing is stored under",
            named: &[],
            unknown: 2,
        },
        DeleteCase {
            case: "the same stored id named twice",
            named: &[2, 2],
            unknown: 0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        for target in [Target::cloud_watch(), Target::evidently("deleted")] {
            let case = row.case;
            // A fresh emulator per row, so no row inherits another's state, and a monitor
            // holding BOTH destination kinds, so a delete under one is shown to leave the
            // other's definitions alone.
            let (router, _engine) = emulator();
            let monitor = format!("del-{index}-{}", target.slug());
            create_monitor(&router, case, &monitor).await;
            let other = match target.destination_arn {
                Some(_) => Target::cloud_watch(),
                None => Target::evidently("untouched"),
            };
            put_destination(&router, case, &monitor, &target).await;
            put_destination(&router, case, &monitor, &other).await;
            let seeded = create_definitions(&router, case, &monitor, &target, SEEDED).await;
            let beside = create_definitions(&router, case, &monitor, &other, SEEDED).await;

            // The request's id list: the named definitions, in the row's order, then the
            // ids nothing is stored under.
            let mut requested: Vec<String> =
                row.named.iter().map(|at| seeded[*at].clone()).collect();
            let unknown: Vec<String> = (0..row.unknown)
                .map(|n| format!("no-such-definition-{index}-{n}"))
                .collect();
            requested.extend(unknown.iter().cloned());

            let reply = batch_delete(&router, &monitor, &target.query(), &requested).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let answered = reply.json(case);

            // Exactly the named definitions are reported deleted — one entry per DISTINCT
            // id, since a repeated id names the one definition it identifies.
            let expected_deleted: BTreeSet<String> =
                row.named.iter().map(|at| seeded[*at].clone()).collect();
            assert_eq!(
                reported_ids(&answered, case),
                expected_deleted,
                "case {case:?}: expected the ids of the deleted definitions, got {answered}"
            );
            assert_eq!(
                answered["MetricDefinitionIds"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                expected_deleted.len(),
                "case {case:?}: expected each deleted id reported once, got {answered}"
            );

            // One error per id the destination holds no definition under, and none for an
            // id it does: `Errors` is `@required`, so a call that failed on nothing
            // reports the empty list.
            let errors = answered["Errors"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the delete reported no Errors"))
                .clone();
            assert_eq!(
                errors.len(),
                unknown.len(),
                "case {case:?}: expected {} per-id errors, got {errors:?}",
                unknown.len()
            );
            for failed in &unknown {
                let reported = errors
                    .iter()
                    .find(|entry| entry["MetricDefinitionId"] == json!(failed))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no error reported for the id {failed:?}: {errors:?}")
                    });
                assert_eq!(
                    reported["ErrorCode"],
                    json!(NOT_FOUND_CODE),
                    "case {case:?}: expected ErrorCode {NOT_FOUND_CODE}, got {reported}"
                );
                let text = reported["ErrorMessage"].as_str().unwrap_or_default();
                assert_eq!(
                    text, NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the per-id message {NOT_FOUND_MESSAGE:?}, got \
                     {text:?}"
                );
            }

            // The destination now holds exactly the definitions the call did not name,
            // read back through the operation that reports them.
            let remaining: BTreeSet<String> = seeded
                .iter()
                .enumerate()
                .filter(|(at, _)| !row.named.contains(at))
                .map(|(_, id)| id.clone())
                .collect();
            assert_eq!(
                stored_ids(&router, case, &monitor, &target.query()).await,
                remaining,
                "case {case:?}: expected the destination to hold only the definitions the \
                 request did not name"
            );
            // The monitor's OTHER destination is untouched: a definition belongs to the
            // destination it was created under.
            assert_eq!(
                stored_ids(&router, case, &monitor, &other.query()).await,
                beside.iter().cloned().collect::<BTreeSet<String>>(),
                "case {case:?}: expected the other destination's definitions to be untouched"
            );

            // The delete is complete: repeating the very same request removes nothing more
            // and now reports every id it named as an id nothing is stored under.
            let again = batch_delete(&router, &monitor, &target.query(), &requested).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected the repeat to answer 200, got {} {:?}",
                again.status,
                String::from_utf8_lossy(&again.body)
            );
            let repeated = again.json(case);
            assert_eq!(
                reported_ids(&repeated, case),
                BTreeSet::new(),
                "case {case:?}: expected the repeat to delete nothing, got {repeated}"
            );
            let repeated_errors: BTreeSet<String> = repeated["Errors"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the repeat reported no Errors"))
                .iter()
                .map(|entry| {
                    entry["MetricDefinitionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let expected_failed: BTreeSet<String> = expected_deleted
                .iter()
                .chain(unknown.iter())
                .cloned()
                .collect();
            assert_eq!(
                repeated_errors, expected_failed,
                "case {case:?}: expected the repeat to fail on every id it named"
            );
            assert_eq!(
                stored_ids(&router, case, &monitor, &target.query()).await,
                remaining,
                "case {case:?}: expected the repeat to leave the destination as it was"
            );
        }
    }
}

/// One row of the target table: the `(app monitor, destination query, id count)` a request
/// names, and the answer the service owes it.
struct TargetCase {
    case: &'static str,
    /// The app monitor the request addresses.
    monitor: Option<&'static str>,
    /// The `destination`/`destinationArn` query the request carries.
    query: String,
    /// How many ids the request names, when the row is about the batch's size rather than
    /// its target.
    id_count: Option<usize>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn batch_delete_naming_no_stored_target_is_refused_whole_and_deletes_nothing() {
    let (router, _engine) = emulator();

    // One monitor holding a CloudWatch destination with definitions, and one holding no
    // destination at all — so a row can miss on the monitor, on the destination kind, or
    // on the experiment an Evidently destination names.
    let stocked = "deltarget";
    let bare = "deltarget-bare";
    create_monitor(&router, "setup", stocked).await;
    create_monitor(&router, "setup", bare).await;
    let target = Target::cloud_watch();
    put_destination(&router, "setup", stocked, &target).await;
    let seeded: BTreeSet<String> = create_definitions(&router, "setup", stocked, &target, SEEDED)
        .await
        .into_iter()
        .collect();
    let experiment = experiment_arn("deltarget");

    let rows: Vec<TargetCase> = vec![
        TargetCase {
            case: "an app monitor nothing created",
            monitor: Some("deltarget-absent"),
            query: destination_query("CloudWatch", None),
            id_count: None,
            status: 404,
            code: NOT_FOUND_CODE,
        },
        TargetCase {
            case: "a CloudWatch destination this monitor never had",
            monitor: Some(bare),
            query: destination_query("CloudWatch", None),
            id_count: None,
            status: 404,
            code: NOT_FOUND_CODE,
        },
        TargetCase {
            case: "an Evidently destination this monitor never had",
            monitor: Some(stocked),
            query: destination_query("Evidently", Some(&experiment)),
            id_count: None,
            status: 404,
            code: NOT_FOUND_CODE,
        },
        TargetCase {
            case: "CloudWatch with a DestinationArn",
            monitor: Some(stocked),
            query: destination_query("CloudWatch", Some(&experiment)),
            id_count: None,
            status: 400,
            code: VALIDATION_CODE,
        },
        TargetCase {
            case: "Evidently with no DestinationArn",
            monitor: Some(stocked),
            query: destination_query("Evidently", None),
            id_count: None,
            status: 400,
            code: VALIDATION_CODE,
        },
        TargetCase {
            case: "one id past the documented maximum per batch",
            monitor: Some(stocked),
            query: destination_query("CloudWatch", None),
            id_count: Some(MAX_IDS_PER_BATCH + 1),
            status: 400,
            code: VALIDATION_CODE,
        },
    ];

    for row in &rows {
        let case = row.case;
        let monitor = row.monitor.unwrap_or(stocked);
        // Either the row's own oversized batch, or one id the stocked destination really
        // holds — so a refusal can only come from the target the request names.
        let ids: Vec<String> = match row.id_count {
            Some(count) => (0..count).map(|n| format!("filler-{n}")).collect(),
            None => seeded.iter().take(1).cloned().collect(),
        };
        let reply = batch_delete(&router, monitor, &row.query, &ids).await;
        assert_eq!(
            reply.status, row.status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.status,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            row.code,
            "case {case:?}: expected {}, got {:?}",
            row.code,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        // The refusal says what it could not do, so the message is built from the request
        // rather than being a bare status.
        assert!(
            !message(&body).unwrap_or_default().is_empty(),
            "case {case:?}: expected the refusal to carry a message, got {body}"
        );
        // Nothing of a refused call is reported as deleted or as a per-id error.
        assert!(
            body["MetricDefinitionIds"].is_null() && body["Errors"].is_null(),
            "case {case:?}: a refused call must report no per-id answer, got {body}"
        );
        assert_eq!(
            stored_ids(&router, case, stocked, &target.query()).await,
            seeded,
            "case {case:?}: expected every seeded definition to still be stored"
        );
    }

    // The batch bound's accepted side: a request naming exactly the documented maximum is
    // answered per id rather than refused whole, which is what makes the row above a
    // refusal of the SIZE and not of the request's shape.
    let case = "exactly the documented maximum per batch";
    let mut at_bound: Vec<String> = seeded.iter().cloned().collect();
    at_bound.extend((seeded.len()..MAX_IDS_PER_BATCH).map(|n| format!("filler-{n}")));
    let reply = batch_delete(&router, stocked, &target.query(), &at_bound).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        reported_ids(&answered, case),
        seeded,
        "case {case:?}: expected the stored definitions to be deleted, got {answered}"
    );
    assert_eq!(
        answered["Errors"].as_array().map(Vec::len).unwrap_or(0),
        MAX_IDS_PER_BATCH - seeded.len(),
        "case {case:?}: expected one error per id nothing is stored under, got {answered}"
    );
    assert_eq!(
        stored_ids(&router, case, stocked, &target.query()).await,
        BTreeSet::new(),
        "case {case:?}: expected the destination to hold no definition afterwards"
    );
}

/// One row of the grant table: where the grant allowing the operation comes from.
struct GrantCase {
    case: &'static str,
    /// The inline identity policy the calling role holds, when it holds one.
    identity: Option<String>,
    /// The resource policy on the app monitor, when it carries one. `{role}` stands for the
    /// calling role's ARN.
    resource: Option<String>,
    allowed: bool,
    /// For a refused row, the clause the refusal ends in: the policy KIND that took the
    /// decision. RECORDED for the row that holds no policy at all; the explicit-deny forms
    /// are AWS's wording for the kind of policy that carries the deny.
    reason: &'static str,
}

/// The clause a refusal states when no statement of the caller's identity-based policies
/// allowed the action.
const NO_IDENTITY_POLICY: &str =
    "because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action";

/// The clause it states when the RESOURCE's own policy denied the action outright.
const RESOURCE_POLICY_DENY: &str = "with an explicit deny in a resource-based policy";

#[tokio::test]
async fn deleting_metric_definitions_is_authorized_and_a_refusal_deletes_nothing() {
    let rows: Vec<GrantCase> = vec![
        GrantCase {
            case: "a role allowed another action of this service",
            identity: Some(identity_policy(&crate::authorization::statement(
                "Allow",
                OTHER_ACTION,
                "*",
            ))),
            resource: None,
            allowed: false,
            reason: NO_IDENTITY_POLICY,
        },
        GrantCase {
            case: "a role with no policy at all",
            identity: None,
            resource: None,
            allowed: false,
            reason: NO_IDENTITY_POLICY,
        },
        GrantCase {
            case: "an identity policy allowing exactly this action",
            identity: Some(identity_policy(&crate::authorization::statement(
                "Allow", ACTION, "*",
            ))),
            resource: None,
            allowed: true,
            reason: "",
        },
        GrantCase {
            case: "no identity policy, a resource policy naming the caller",
            identity: None,
            resource: Some(resource_policy("Allow", "{role}", ACTION, None)),
            allowed: true,
            reason: "",
        },
        GrantCase {
            case: "an identity policy that allows and a resource policy that denies",
            identity: Some(identity_policy(&crate::authorization::statement(
                "Allow", "rum:*", "*",
            ))),
            resource: Some(resource_policy("Deny", "*", ACTION, None)),
            allowed: false,
            reason: RESOURCE_POLICY_DENY,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so no row inherits another's grant or state.
        let authorized = Emulator::new();
        let monitor = format!("del-auth-{index}");
        authorized.create_monitor(&monitor).await;
        let seed = authorized
            .rum(
                ACCOUNT_KEY,
                "POST",
                &format!("/rummetrics/{monitor}/metricsdestination"),
                Some(json!({"Destination": "CloudWatch"})),
            )
            .await;
        assert_eq!(
            seed.status, 200,
            "case {case:?}: seeding the destination must succeed, got {} {:?}",
            seed.status,
            String::from_utf8_lossy(&seed.body)
        );
        let created = authorized
            .rum(
                ACCOUNT_KEY,
                "POST",
                &format!("/rummetrics/{monitor}/metrics"),
                Some(json!({
                    "Destination": "CloudWatch",
                    "MetricDefinitions": [definition(0)],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding the definition must succeed, got {} {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let echoed_id = created.json(case)["MetricDefinitions"][0]["MetricDefinitionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the create to echo the definition it created, got {:?}",
                    String::from_utf8_lossy(&created.body)
                )
            })
            .to_string();
        // The same definition is read back through `BatchGetRumMetricDefinitions`, with the
        // account's own credential.
        let seeded = authorized
            .rum(
                ACCOUNT_KEY,
                "GET",
                &format!(
                    "/rummetrics/{monitor}/metrics?{}",
                    destination_query("CloudWatch", None)
                ),
                None,
            )
            .await;
        let id = seeded.json(case)["MetricDefinitions"][0]["MetricDefinitionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the seed minted no MetricDefinitionId"))
            .to_string();
        assert_eq!(
            id, echoed_id,
            "case {case:?}: expected the stored definition to be held under the id the create \
             echoed ({echoed_id:?}), got {id:?}"
        );

        let role = format!("metric-deleter-{index}");
        let key = authorized
            .assumed_role_key(&role, row.identity.as_deref())
            .await;
        if let Some(document) = row.resource.as_deref() {
            let document = document.replace("{role}", &role_arn(&role));
            authorized.put_resource_policy(&monitor, &document).await;
        }

        let reply = authorized
            .rum(
                &key,
                "DELETE",
                &format!(
                    "/rummetrics/{monitor}/metrics?{}&metricDefinitionIds={}",
                    destination_query("CloudWatch", None),
                    percent_encoded(&id)
                ),
                None,
            )
            .await;
        if row.allowed {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status, 403,
                "case {case:?}: expected HTTP 403, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            // The refusal is worded about the decision that was taken: the principal the
            // credential speaks for, the action it asked for, the app monitor it asked to
            // act on, and the policy kind that withheld the grant.
            let expected = format!(
                "User: arn:aws:sts::{}:assumed-role/{role}/probe is not authorized to \
                 perform: {ACTION} on resource: {} {}",
                crate::authorization::account(),
                crate::authorization::monitor_arn(&monitor),
                row.reason
            );
            assert_eq!(
                reply.message(case),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                reply.message(case)
            );
        }

        // What the destination holds afterwards, read back with the account's own
        // credential: nothing after an allowed delete, and the definition still there
        // after a refused one.
        let read = authorized
            .rum(
                ACCOUNT_KEY,
                "GET",
                &format!(
                    "/rummetrics/{monitor}/metrics?{}",
                    destination_query("CloudWatch", None)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the read-back to answer 200, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let held = read.json(case)["MetricDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected = usize::from(!row.allowed);
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected the destination to hold {expected} definition(s), got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn an_iam_user_holding_no_policy_is_refused_and_the_definition_survives() {
    let case = "an IAM user with no identity-based policy";
    let emulator = Emulator::new();
    let monitor = "del-auth-user";
    emulator.create_monitor(monitor).await;
    let seeded = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/rummetrics/{monitor}/metricsdestination"),
            Some(json!({"Destination": "CloudWatch"})),
        )
        .await;
    assert_eq!(
        seeded.status, 200,
        "case {case:?}: seeding the destination must succeed, got {} {:?}",
        seeded.status,
        String::from_utf8_lossy(&seeded.body)
    );
    let created = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/rummetrics/{monitor}/metrics"),
            Some(json!({
                "Destination": "CloudWatch",
                "MetricDefinitions": [definition(0)],
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: seeding the definition must succeed, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let page = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!(
                "/rummetrics/{monitor}/metrics?{}",
                destination_query("CloudWatch", None)
            ),
            None,
        )
        .await;
    let id = page.json(case)["MetricDefinitions"][0]["MetricDefinitionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the seed minted no MetricDefinitionId"))
        .to_string();

    // An IAM user created through IAM's own calls, holding NO policy at all, signing with
    // the long-term key IAM minted for it: a principal of another kind than the sessions
    // the table above drives, judged by the same rule.
    let user = "metric-deleter-user";
    let key = emulator.iam_user_key(user, None).await;
    let reply = emulator
        .rum(
            &key,
            "DELETE",
            &format!(
                "/rummetrics/{monitor}/metrics?{}&metricDefinitionIds={}",
                destination_query("CloudWatch", None),
                percent_encoded(&id)
            ),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected HTTP 403, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        ACCESS_DENIED_CODE,
        "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let expected = format!(
        "User: arn:aws:iam::{}:user/{user} is not authorized to perform: {ACTION} on \
         resource: {} {NO_IDENTITY_POLICY}",
        crate::authorization::account(),
        crate::authorization::monitor_arn(monitor)
    );
    assert_eq!(
        reply.message(case),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        reply.message(case)
    );

    // And the definition the unauthorized caller asked to delete is still there.
    let read = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!(
                "/rummetrics/{monitor}/metrics?{}",
                destination_query("CloudWatch", None)
            ),
            None,
        )
        .await;
    assert_eq!(
        read.json(case)["MetricDefinitions"][0]["MetricDefinitionId"].as_str(),
        Some(id.as_str()),
        "case {case:?}: a refused delete removed the definition: {:?}",
        String::from_utf8_lossy(&read.body)
    );
}
