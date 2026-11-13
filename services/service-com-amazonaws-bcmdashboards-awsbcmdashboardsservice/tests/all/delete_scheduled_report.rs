// SPDX-License-Identifier: CC0-1.0

//! `DeleteScheduledReport`, end to end over the wire.
//!
//! The rule under test: a delete whose operation DECLARES `ResourceNotFoundException`
//! removes the stored resource the requested ARN is the key of and answers HTTP 200 with
//! its required output member echoing exactly that ARN, while an ARN that keys no stored
//! resource is REFUSED with this service's not-found answer — HTTP 400, the wire error code
//! `ResourceNotFoundException` and a `resourceId` member echoing the requested ARN (see
//! `tests/all/resource_not_found.rs`) — and changes nothing a later caller can see. That
//! declaration is what tells this delete from `DeleteDashboard`, which declares no
//! not-found error at all and therefore answers an absent ARN with an idempotent 200
//! (source: `smithy_query(query="operation",operation="DeleteScheduledReport")` versus
//! `...operation="DeleteDashboard"`). An ARN outside `ScheduledReportArn`'s modelled
//! constraints is refused at the boundary with the modelled `ValidationException` (400) in
//! either case: a missing resource and a malformed request are different failures.
//!
//! The rows below are a TABLE spanning that class rather than one request: they differ in
//! the deleted report's CONFIGURATION (which optional members it carries — description,
//! widget ids, tags — and its schedule state, kept and defaulted), in how many reports are
//! stored BESIDE it, in the NUMBER of deletes of the same ARN (one, two, three), and — for
//! the refusals — in the ARN's FORM (the partition suffix, the account id, the wildcard
//! resource id the pattern admits, a one-character id, the 32-hex id form this emulator
//! mints), in its LENGTH (the published 2048-character maximum, and one past it) and in the
//! resource's HISTORY (never created, versus created and then deleted).
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::service_state;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::resource_not_found::assert_not_found;
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The fixed part of every `ScheduledReportArn` this suite writes by hand: `arn:aws` + the
/// (here empty) partition suffix, the service, the empty region segment, the 12-digit
/// account id and the resource type. Source: `ScheduledReportArn`'s modelled pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$`, length
/// 20..2048, as reported by
/// `smithy_query(query="operation",operation="DeleteScheduledReport")`.
const ARN_PREFIX: &str = "arn:aws:bcm-dashboards::000000000000:scheduled-report/";

/// An ARN of exactly `len` characters satisfying that pattern. Padding is `a`, which the
/// `[-a-z0-9]+` resource segment admits.
fn arn_of_length(len: usize) -> String {
    assert!(
        len > ARN_PREFIX.len(),
        "an arn of {len} characters cannot carry the {}-character prefix and a resource id",
        ARN_PREFIX.len()
    );
    format!("{ARN_PREFIX}{}", "a".repeat(len - ARN_PREFIX.len()))
}

// This service's not-found answer is asserted by the suite that tables it over every
// operation answering it: `super::resource_not_found::assert_not_found` checks the status,
// the wire code, the requested ARN echoed in `resourceId` and the message, so the rows
// below and that table cannot drift apart on what the answer looks like.

/// The ARNs `ListScheduledReports` reports, in listing order.
async fn listed_report_arns(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok_call(router, "ListScheduledReports", &json!({})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answers a list: {listed}"))
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<no arn>").to_string())
        .collect()
}

/// One stored report, how many reports stand beside it, and how many times its delete is
/// repeated after the one that removed it.
struct StoredCase {
    label: &'static str,
    description: Option<&'static str>,
    /// `scheduleConfig.state` as submitted — `None` leaves it to the service's default.
    state: Option<&'static str>,
    /// Whether the report names a widget of the dashboard it reports on.
    names_widget: bool,
    tags: Option<Value>,
    siblings: usize,
    /// Deletes of the same ARN after the first one.
    repeats: usize,
}

fn stored_cases() -> Vec<StoredCase> {
    vec![
        StoredCase {
            label: "required members only, the only report in the store, deleted once",
            description: None,
            state: None,
            names_widget: false,
            tags: None,
            siblings: 0,
            repeats: 0,
        },
        StoredCase {
            label: "a tagged report with a description, two reports beside it, deleted twice",
            description: Some("Monthly spend rollup."),
            state: None,
            names_widget: false,
            tags: Some(json!([{"key": "team", "value": "finops"}, {"key": "env", "value": "prod"}])),
            siblings: 2,
            repeats: 1,
        },
        StoredCase {
            // `ScheduleState` values, source: the `ScheduleState` enum in the model.
            label: "a DISABLED report naming a widget, one report beside it, deleted three times",
            description: Some("One widget only."),
            state: Some("DISABLED"),
            names_widget: true,
            tags: Some(json!([{"key": "owner", "value": "billing"}])),
            siblings: 1,
            repeats: 2,
        },
        StoredCase {
            label: "an ENABLED report spelled out, four reports beside it, deleted once",
            description: None,
            state: Some("ENABLED"),
            names_widget: true,
            tags: None,
            siblings: 4,
            repeats: 0,
        },
    ]
}

/// A `CreateScheduledReport` request for `case`, under `name`.
fn create_request(
    case: &StoredCase,
    name: &str,
    dashboard_arn: &str,
    widget_id: &str,
) -> Value {
    let mut schedule = json!({"scheduleExpression": "cron(0 8 1 * ? *)"});
    if let Some(state) = case.state {
        schedule["state"] = json!(state);
    }
    let mut report = json!({
        "name": name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": schedule,
    });
    if let Some(description) = case.description {
        report["description"] = json!(description);
    }
    if case.names_widget {
        report["widgetIds"] = json!([widget_id]);
    }
    let mut request = json!({"scheduledReport": report});
    if let Some(tags) = &case.tags {
        request["resourceTags"] = tags.clone();
    }
    request
}

/// A plain report standing beside the doomed one.
fn sibling_request(name: &str, dashboard_arn: &str) -> Value {
    json!({
        "scheduledReport": {
            "name": name,
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 12 L * ? *)"}
        }
    })
}

/// Create one report and answer its ARN.
async fn create_report(router: &MultiServiceRouter, request: &Value) -> String {
    ok_call(router, "CreateScheduledReport", request).await["arn"]
        .as_str()
        .expect("CreateScheduledReport answers an arn")
        .to_string()
}

/// A delete of a report that IS stored removes it and echoes its ARN at 200, every repeat of
/// that delete is the not-found refusal, and the reports beside it — and the dashboard it
/// reported on — are left alone.
#[tokio::test]
async fn deleting_a_stored_report_removes_it_and_echoes_its_arn() {
    for case in stored_cases() {
        let label = case.label;
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "DeleteSource").await;

        let doomed = create_report(
            &router,
            &create_request(&case, "Doomed Report", &dashboard_arn, &widget_id),
        )
        .await;
        let mut survivors = Vec::new();
        for index in 0..case.siblings {
            survivors.push(
                create_report(&router, &sibling_request(&format!("Survivor {index}"), &dashboard_arn))
                    .await,
            );
        }

        let (status, body) = call(&router, "DeleteScheduledReport", &json!({"arn": doomed})).await;
        assert_eq!(
            status, 200,
            "case {label}: deleting a stored report must answer 200, got {status}: {body}"
        );
        assert_eq!(
            body["arn"].as_str(),
            Some(doomed.as_str()),
            "case {label}: the answer must echo the requested arn, got {body}"
        );

        // The post-condition: the ARN resolves to nothing, carries no tags any more, and is
        // not listed.
        let (status, body) = call(&router, "GetScheduledReport", &json!({"arn": doomed})).await;
        assert_not_found(
            &format!("{label} / the deleted report is read back"),
            &doomed,
            status,
            &body,
        );
        let (status, body) =
            call(&router, "ListTagsForResource", &json!({"resourceArn": doomed})).await;
        assert_not_found(
            &format!("{label} / the deleted report's tags are read back"),
            &doomed,
            status,
            &body,
        );

        let mut listed = listed_report_arns(&router).await;
        let mut expected = survivors.clone();
        listed.sort();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {label}: the delete must remove exactly the one report, got {listed:?}"
        );

        // Every repeat of the same delete is the refusal, not a second success: this
        // operation declares the not-found error.
        for attempt in 1..=case.repeats {
            let (status, body) =
                call(&router, "DeleteScheduledReport", &json!({"arn": doomed})).await;
            assert_not_found(
                &format!("{label} / repeat {attempt} of the delete"),
                &doomed,
                status,
                &body,
            );
        }

        for survivor in &survivors {
            let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": survivor})).await;
            assert_eq!(
                fetched["scheduledReport"]["arn"].as_str(),
                Some(survivor.as_str()),
                "case {label}: a report beside the deleted one must still resolve, got {fetched}"
            );
        }
        // Deleting a report never touches the dashboard it reported on.
        let dashboard = ok_call(&router, "GetDashboard", &json!({"arn": dashboard_arn})).await;
        assert_eq!(
            dashboard["arn"].as_str(),
            Some(dashboard_arn.as_str()),
            "case {label}: the source dashboard must still resolve, got {dashboard}"
        );
    }
}

/// A delete of a well-formed ARN that no stored report answers to is refused with the
/// service's not-found answer — for every ARN form, and for a report that was created and
/// then deleted as well as one that never existed — and it changes nothing.
#[tokio::test]
async fn deleting_an_absent_report_is_refused_and_changes_nothing() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "AbsentSource").await;
    // A live report beside every miss, so a refusal that touched the store would show.
    let live = create_report(&router, &sibling_request("Live Report", &dashboard_arn)).await;
    // And one that existed and was removed: the same refusal as one that never existed.
    let deleted = create_report(&router, &sibling_request("Deleted Report", &dashboard_arn)).await;
    ok_call(&router, "DeleteScheduledReport", &json!({"arn": deleted})).await;

    let mut cases: Vec<(String, String)> = [
        ("an id never created", "aws", "000000000000", "never-created"),
        (
            "the wildcard resource id the pattern admits",
            "aws",
            "000000000000",
            "*",
        ),
        (
            "a partition other than aws",
            "aws-cn",
            "000000000000",
            "elsewhere",
        ),
        (
            "an account id nothing was minted under",
            "aws",
            "999999999999",
            "never-created",
        ),
        (
            "a 32-hex id of the form this emulator mints, never minted",
            "aws",
            "000000000000",
            "ffffffffffffffffffffffffffffffff",
        ),
        ("a one-character id", "aws", "000000000000", "a"),
    ]
    .iter()
    .map(|(label, partition, account, id)| {
        (
            (*label).to_string(),
            format!("arn:{partition}:bcm-dashboards::{account}:scheduled-report/{id}"),
        )
    })
    .collect();
    // The published maximum of `ScheduledReportArn`. Its 20-character minimum is
    // unreachable: the shortest string the pattern admits is longer than that.
    cases.push((
        "an arn at the 2048-character maximum".to_string(),
        arn_of_length(2048),
    ));
    cases.push((
        "a report created and then deleted".to_string(),
        deleted.clone(),
    ));

    for (label, arn) in &cases {
        let (status, body) = call(&router, "DeleteScheduledReport", &json!({"arn": arn})).await;
        assert_not_found(label, arn, status, &body);
    }

    // None of those refusals touched the store.
    assert_eq!(
        listed_report_arns(&router).await,
        vec![live.clone()],
        "the refused deletes must leave the one live report stored"
    );
    let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": live})).await;
    assert_eq!(
        fetched["scheduledReport"]["name"].as_str(),
        Some("Live Report"),
        "the live report must read back unchanged, got {fetched}"
    );
}

/// A missing resource and a malformed request are different failures: an `arn` outside
/// `ScheduledReportArn`'s pattern or its 2048-character maximum is refused with the modelled
/// `ValidationException` (400) and names no resource.
#[tokio::test]
async fn a_malformed_arn_is_refused_with_validation() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "MalformedSource").await;
    let live = create_report(&router, &sibling_request("Untouched Report", &dashboard_arn)).await;

    let malformed: Vec<(&str, String)> = vec![
        ("not an arn at all", "not-an-arn".to_string()),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:scheduled-report/abc".to_string(),
        ),
        (
            "a region where the model allows none",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:scheduled-report/abc".to_string(),
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:bcm-dashboards::12345:scheduled-report/abc".to_string(),
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:scheduled-report/ABC".to_string(),
        ),
        (
            "the other resource type this service mints",
            "arn:aws:bcm-dashboards::000000000000:dashboard/abc".to_string(),
        ),
        ("an empty arn", String::new()),
        (
            "one character past the 2048-character maximum",
            arn_of_length(2049),
        ),
    ];

    for (label, arn) in &malformed {
        let (status, body) = call(&router, "DeleteScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a malformed arn must be refused, got {status}: {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ValidationException"),
            "case {label:?}: expected ValidationException, got {code:?}: {body}"
        );
        assert_eq!(
            body.get("resourceId"),
            None,
            "case {label:?}: a boundary refusal names no resource, got {body}"
        );
    }

    assert_eq!(
        listed_report_arns(&router).await,
        vec![live],
        "the refused requests must leave the stored report alone"
    );
}

/// The delete takes the create claim of the report it removed with it, so a
/// `CreateScheduledReport` under that token creates a report again rather than being
/// refused in the name of an ARN that resolves to nothing.
#[tokio::test]
async fn deleting_a_report_drops_the_create_record_that_minted_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let (dashboard_arn, _) = source_dashboard(&router, "TokenDeleteSource").await;
    let request = json!({
        "clientToken": "delete-then-retry-7c",
        "scheduledReport": {
            "name": "Recreated Report",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
        }
    });
    let first = create_report(&router, &request).await;

    ok_call(&router, "DeleteScheduledReport", &json!({"arn": first})).await;
    assert!(
        service_state(&*engine)
            .expect("the dashboard store")
            .lock()
            .scheduled_report_client_tokens
            .is_empty(),
        "case the deleted report: the delete must drop the create claim naming it"
    );

    let again = create_report(&router, &request).await;
    assert_ne!(
        again, first,
        "case the deleted report: the second create must mint a new report rather than be refused for the removed one"
    );
    let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": again})).await;
    assert_eq!(
        fetched["scheduledReport"]["arn"].as_str(),
        Some(again.as_str()),
        "case the deleted report: the re-created report must read back under its own arn, got {fetched}"
    );
}
