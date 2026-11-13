// SPDX-License-Identifier: CC0-1.0

//! `ExecuteScheduledReport`, end to end over the wire.
//!
//! The rule under test: an execution request on a stored scheduled report whose
//! schedule is `ENABLED` triggers one execution and refreshes the report's health,
//! where the health is DERIVED from what this service stores — healthy exactly while
//! the dashboard the report renders and every widget its `widgetIds` name still
//! resolve — a `DISABLED` report is refused with the modelled `ValidationException`, a
//! `dryRun` validates and writes nothing, and a `clientToken` makes the call idempotent
//! for the minute the model documents.
//!
//! Every assertion spans the class rather than one request: the tables below differ in
//! schedule state (absent, `ENABLED`, `DISABLED`), in widget-id count (absent, empty,
//! one), in which stored resource was removed under the report (none, the dashboard, the
//! widget) and in whether a client token is presented.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, graph_config, ok_call, router, timestamp};
use super::scheduled_report_lifecycle::{error_code, source_dashboard, EXECUTION_ROLE_ARN};

/// The `HealthStatusCode` values, source:
/// `smithy_query(query="shape",shape="HealthStatusCode")` — `HEALTHY`, `UNHEALTHY`.
pub(crate) const HEALTHY: &str = "HEALTHY";
pub(crate) const UNHEALTHY: &str = "UNHEALTHY";

/// The two `StatusReason` values this store can establish, source:
/// `smithy_query(query="shape",shape="StatusReason")` (the full list also carries
/// `DATA_SOURCE_ACCESS_DENIED`, `EXECUTION_ROLE_ASSUME_FAILED`,
/// `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS`, `DASHBOARD_ACCESS_DENIED` and
/// `INTERNAL_FAILURE`, which name failures of resources outside this service).
pub(crate) const DASHBOARD_NOT_FOUND: &str = "DASHBOARD_NOT_FOUND";
pub(crate) const WIDGET_ID_NOT_FOUND: &str = "WIDGET_ID_NOT_FOUND";

/// A well-formed `ScheduledReportArn` no report in a fresh store is keyed by.
const MISSING_REPORT_ARN: &str =
    "arn:aws:bcm-dashboards::123456789012:scheduled-report/never-created";

/// Create a scheduled report on `dashboard_arn` and return its ARN.
///
/// `schedule_state` is `None` to leave `scheduleConfig.state` out of the request
/// altogether, and `widget_ids` is `None` to leave `widgetIds` out. Shared with the
/// sibling audit suite.
pub(crate) async fn create_report(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    schedule_state: Option<&str>,
    widget_ids: Option<Vec<String>>,
) -> String {
    let mut schedule_config = json!({"scheduleExpression": "cron(0 8 1 * ? *)"});
    if let Some(state) = schedule_state {
        schedule_config["state"] = json!(state);
    }
    let mut report = json!({
        "name": name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": schedule_config,
    });
    if let Some(ids) = widget_ids {
        report["widgetIds"] = json!(ids);
    }
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({"scheduledReport": report}),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// Create a scheduled report on `dashboard_arn` whose stored `widgetIds` name a widget
/// that dashboard does NOT hold, and return its ARN.
///
/// A create can no longer be asked for that configuration directly — a submitted
/// `widgetIds` entry has to identify a widget of the dashboard the request names — so this
/// is how a client reaches it: the report is created on a dashboard of its own, naming that
/// dashboard's widget, and then re-pointed at `dashboard_arn`, which does not hold that
/// widget. `dashboard_arn`'s own widgets are untouched, so another report of the same suite
/// that names one of them keeps resolving.
pub(crate) async fn create_report_naming_a_foreign_widget(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    schedule_state: Option<&str>,
) -> String {
    let (own_dashboard, own_widget) =
        source_dashboard(router, &format!("{name} Widget Source")).await;
    let arn = create_report(
        router,
        &own_dashboard,
        name,
        schedule_state,
        Some(vec![own_widget]),
    )
    .await;
    ok_call(
        router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "dashboardArn": dashboard_arn}),
    )
    .await;
    arn
}

/// The stored record `GetScheduledReport` answers for `arn`. Shared with the sibling
/// health-view suite.
pub(crate) async fn stored_report(router: &MultiServiceRouter, arn: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .cloned()
        .unwrap_or_else(|| panic!("no scheduledReport in {fetched}"))
}

/// The `ScheduledReportSummary` `ListScheduledReports` pages over for `arn`. Shared with
/// the sibling health-view suite.
pub(crate) async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Value {
    let listed = ok_call(router, "ListScheduledReports", &json!({})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answered no list: {listed}"))
        .iter()
        .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(arn))
        .cloned()
        .unwrap_or_else(|| panic!("no summary for {arn} in {listed}"))
}

/// Assert a `healthStatus` member: its code, its reasons (`None` for "the member must be
/// absent"), and that its `lastRefreshedAt` is a plausible instant.
fn assert_health(health: &Value, label: &str, code: &str, reasons: Option<&[&str]>) {
    assert_eq!(
        health.get("statusCode").and_then(Value::as_str),
        Some(code),
        "case {label}: expected healthStatus.statusCode {code:?}, got {health}"
    );
    match reasons {
        Some(expected) => assert_eq!(
            health.get("statusReasons"),
            Some(&json!(expected)),
            "case {label}: expected statusReasons {expected:?}, got {health}"
        ),
        None => assert!(
            health.get("statusReasons").is_none(),
            "case {label}: a healthy report carries no statusReasons, got {health}"
        ),
    }
    let refreshed = timestamp(health, "lastRefreshedAt", label);
    assert!(
        refreshed > 1_700_000_000.0,
        "case {label}: lastRefreshedAt {refreshed} is not a plausible epoch-seconds instant"
    );
}

/// Assert the `healthStatus` of a record nothing has refreshed: HEALTHY, with neither a
/// refresh instant nor reasons — the health a report the service accepted carries before
/// any execution has refreshed it.
fn assert_unrefreshed_health(report: &Value, label: &str) {
    assert_eq!(
        report.get("healthStatus"),
        Some(&json!({"statusCode": HEALTHY})),
        "case {label}: an unrefreshed report reports HEALTHY and nothing else: {report}"
    );
}

/// One row of the trigger table.
struct TriggerCase {
    label: &'static str,
    name: &'static str,
    /// `None` leaves `scheduleConfig.state` out of the create request.
    schedule_state: Option<&'static str>,
    /// `None` leaves `widgetIds` out; `Some(true)` names the dashboard's real widget.
    names_widget: Option<bool>,
    /// Whether the execution request presents a `clientToken`.
    client_token: Option<&'static str>,
}

/// An execution request on an ENABLED report triggers one execution, answers HEALTHY,
/// and persists both the execution instant and the refreshed health on the record —
/// over a table differing in schedule state, widget-id count and client token.
#[tokio::test]
async fn executing_an_enabled_report_triggers_it_and_persists_the_execution() {
    let cases = [
        TriggerCase {
            label: "state omitted, no widgetIds, no token",
            name: "Default State Report",
            schedule_state: None,
            names_widget: None,
            client_token: None,
        },
        TriggerCase {
            label: "state ENABLED explicitly, empty widgetIds, no token",
            name: "Explicit Enabled",
            schedule_state: Some("ENABLED"),
            names_widget: Some(false),
            client_token: None,
        },
        TriggerCase {
            label: "state ENABLED, one real widget id, with token",
            name: "One Widget Report",
            schedule_state: Some("ENABLED"),
            names_widget: Some(true),
            client_token: Some("exec-token-0001"),
        },
    ];

    for case in cases {
        let label = case.label;
        // A fresh store per row, so one row's execution cannot be read as another's.
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "ExecSource").await;
        let widget_ids = case.names_widget.map(|names| {
            if names {
                vec![widget_id.clone()]
            } else {
                Vec::new()
            }
        });
        let arn = create_report(
            &router,
            &dashboard_arn,
            case.name,
            case.schedule_state,
            widget_ids,
        )
        .await;

        let before = stored_report(&router, &arn).await;
        assert!(
            before.get("lastExecutionAt").is_none(),
            "case {label}: a report that never ran has no lastExecutionAt: {before}"
        );
        assert_unrefreshed_health(&before, label);
        let created_at = timestamp(&before, "createdAt", label);
        let updated_at = timestamp(&before, "updatedAt", label);

        let mut request = json!({"arn": arn});
        if let Some(token) = case.client_token {
            request["clientToken"] = json!(token);
        }
        let answered = ok_call(&router, "ExecuteScheduledReport", &request).await;
        assert_eq!(
            answered.get("executionTriggered").and_then(Value::as_bool),
            Some(true),
            "case {label}: an enabled report whose resources resolve is triggered, got {answered}"
        );
        let health = answered
            .get("healthStatus")
            .unwrap_or_else(|| panic!("case {label}: no healthStatus in {answered}"));
        assert_health(health, label, HEALTHY, None);

        // The execution is persisted: the record carries the execution instant and the
        // refreshed health, and the configuration — `updatedAt` included — is untouched.
        let after = stored_report(&router, &arn).await;
        let last_execution_at = timestamp(&after, "lastExecutionAt", label);
        assert!(
            last_execution_at >= created_at,
            "case {label}: lastExecutionAt {last_execution_at} precedes createdAt {created_at}: {after}"
        );
        assert_health(
            after
                .get("healthStatus")
                .unwrap_or_else(|| panic!("case {label}: no stored healthStatus in {after}")),
            label,
            HEALTHY,
            None,
        );
        assert_eq!(
            timestamp(&after, "updatedAt", label),
            updated_at,
            "case {label}: an execution does not update the configuration: {after}"
        );
        assert_eq!(
            timestamp(&after, "createdAt", label),
            created_at,
            "case {label}: an execution does not move createdAt: {after}"
        );

        // The listing projects the same refreshed health.
        let summary = listed_summary(&router, &arn).await;
        assert_eq!(
            summary["healthStatus"]["statusCode"].as_str(),
            Some(HEALTHY),
            "case {label}: the listing reports the refreshed health: {summary}"
        );

        // Reading the record twice answers the same thing: the reads changed nothing.
        assert_eq!(
            stored_report(&router, &arn).await,
            after,
            "case {label}: reading the record twice must answer the same record"
        );
    }
}

/// The health an execution answers is derived from the store, not from the request: a
/// report whose dashboard was deleted, or whose `widgetIds` name a widget the dashboard
/// no longer holds, answers UNHEALTHY with exactly that reason and triggers nothing.
#[tokio::test]
async fn health_is_derived_from_the_resources_the_report_still_resolves() {
    // (label, what to remove under the report, expected code, expected reasons)
    enum Removal {
        Nothing,
        Dashboard,
        TheNamedWidget,
    }
    let cases: [(&str, Removal, &str, Option<&[&str]>); 3] = [
        ("every resource still resolves", Removal::Nothing, HEALTHY, None),
        (
            "the dashboard was deleted",
            Removal::Dashboard,
            UNHEALTHY,
            Some(&[DASHBOARD_NOT_FOUND]),
        ),
        (
            "the named widget was replaced",
            Removal::TheNamedWidget,
            UNHEALTHY,
            Some(&[WIDGET_ID_NOT_FOUND]),
        ),
    ];

    for (label, removal, expected_code, expected_reasons) in cases {
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "HealthSource").await;
        let arn = create_report(
            &router,
            &dashboard_arn,
            "Health Report",
            None,
            Some(vec![widget_id.clone()]),
        )
        .await;

        match removal {
            Removal::Nothing => {}
            Removal::Dashboard => {
                ok_call(&router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
            }
            Removal::TheNamedWidget => {
                // UpdateDashboard's `widgets` "Replaces all existing widgets", so the
                // widget the report names is gone and the replacement has a new id.
                ok_call(
                    &router,
                    "UpdateDashboard",
                    &json!({
                        "arn": dashboard_arn,
                        "name": "HealthSource",
                        "widgets": [{
                            "title": "Replacement",
                            "configs": [graph_config("BlendedCost", "BAR", "DAILY")]
                        }]
                    }),
                )
                .await;
            }
        }

        let answered = ok_call(&router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
        let triggered = expected_code == HEALTHY;
        assert_eq!(
            answered.get("executionTriggered").and_then(Value::as_bool),
            Some(triggered),
            "case {label}: expected executionTriggered {triggered}, got {answered}"
        );
        assert_health(
            answered
                .get("healthStatus")
                .unwrap_or_else(|| panic!("case {label}: no healthStatus in {answered}")),
            label,
            expected_code,
            expected_reasons,
        );

        // The refreshed health is persisted either way, and an execution that was NOT
        // triggered leaves no execution instant behind.
        let stored = stored_report(&router, &arn).await;
        assert_eq!(
            stored["healthStatus"]["statusCode"].as_str(),
            Some(expected_code),
            "case {label}: the record keeps the refreshed health: {stored}"
        );
        assert_eq!(
            stored.get("lastExecutionAt").is_some(),
            triggered,
            "case {label}: lastExecutionAt is written only by a triggered execution: {stored}"
        );
    }
}

/// A report whose schedule is DISABLED is refused with the modelled ValidationException
/// (400) and nothing is written — whatever else the request carries (a client token, a
/// dry run) and whichever way the state was set.
#[tokio::test]
async fn executing_a_disabled_report_is_refused_and_writes_nothing() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "DisabledSource").await;
    let plain = create_report(
        &router,
        &dashboard_arn,
        "Paused Report",
        Some("DISABLED"),
        None,
    )
    .await;
    let with_widget = create_report(
        &router,
        &dashboard_arn,
        "Paused With Widget",
        Some("DISABLED"),
        Some(vec![widget_id]),
    )
    .await;

    let cases = [
        ("plain request", json!({"arn": plain})),
        (
            "with a client token",
            json!({"arn": plain, "clientToken": "disabled-token-0001"}),
        ),
        ("dry run", json!({"arn": plain, "dryRun": true})),
        ("dry run explicitly false", json!({"arn": plain, "dryRun": false})),
        ("a different disabled report", json!({"arn": with_widget})),
    ];

    for (label, request) in cases {
        let (status, body) = call(&router, "ExecuteScheduledReport", &request).await;
        assert_eq!(
            status, 400,
            "case {label}: a DISABLED report is refused with 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label}: expected ValidationException, got {body}"
        );
        let arn = request["arn"].as_str().expect("the request names an arn");
        let stored = stored_report(&router, arn).await;
        assert!(
            stored.get("lastExecutionAt").is_none(),
            "case {label}: a refused execution writes nothing: {stored}"
        );
        // Nothing refreshed the health, so it still reads as it did at create time.
        assert_unrefreshed_health(&stored, label);
    }
}

/// `dryRun` validates the configuration and answers its health without triggering an
/// execution or writing anything — on a report whose resources resolve and on one whose
/// dashboard is gone alike, and repeatably.
#[tokio::test]
async fn a_dry_run_validates_without_triggering_or_writing() {
    for (label, delete_dashboard, expected_code, expected_reasons) in [
        ("resources resolve", false, HEALTHY, None),
        (
            "dashboard deleted",
            true,
            UNHEALTHY,
            Some(&[DASHBOARD_NOT_FOUND][..]),
        ),
    ] {
        let router = router();
        let (dashboard_arn, _) = source_dashboard(&router, "DryRunSource").await;
        let arn = create_report(&router, &dashboard_arn, "Dry Run Report", None, None).await;
        if delete_dashboard {
            ok_call(&router, "DeleteDashboard", &json!({"arn": dashboard_arn})).await;
        }
        let before = stored_report(&router, &arn).await;

        // Twice, with and without a client token: a dry run is a validation, so neither
        // call triggers anything and neither consumes the token.
        for request in [
            json!({"arn": arn, "dryRun": true}),
            json!({"arn": arn, "dryRun": true, "clientToken": "dry-run-token-0001"}),
        ] {
            let answered = ok_call(&router, "ExecuteScheduledReport", &request).await;
            assert_eq!(
                answered.get("executionTriggered").and_then(Value::as_bool),
                Some(false),
                "case {label}: a dry run triggers nothing, got {answered}"
            );
            assert_health(
                answered
                    .get("healthStatus")
                    .unwrap_or_else(|| panic!("case {label}: no healthStatus in {answered}")),
                label,
                expected_code,
                expected_reasons,
            );
            assert_eq!(
                stored_report(&router, &arn).await,
                before,
                "case {label}: a dry run must leave the record exactly as it was"
            );
        }

        // The token a dry run presented was never consumed, so a real execution under it
        // still runs.
        let answered = ok_call(
            &router,
            "ExecuteScheduledReport",
            &json!({"arn": arn, "clientToken": "dry-run-token-0001"}),
        )
        .await;
        assert_eq!(
            answered.get("executionTriggered").and_then(Value::as_bool),
            Some(expected_code == HEALTHY),
            "case {label}: the execution after the dry run decides on its own, got {answered}"
        );
    }
}

/// A `clientToken` is an idempotency key for the documented minute: a retry presenting
/// it for the same report answers the first call's outcome and triggers no second
/// execution, a fresh token (or none at all) executes again, and the same token
/// presented for a different report is a different request, which executes that report.
/// (The token's scope is covered over a table in `execution_token_scope.rs`.)
#[tokio::test]
async fn a_client_token_suppresses_a_second_execution_within_the_window() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "TokenExecSource").await;
    let first_arn = create_report(&router, &dashboard_arn, "Token Report", None, None).await;
    let other_arn = create_report(&router, &dashboard_arn, "Other Report", None, None).await;

    let token = "execute-retry-token-9f";
    let request = json!({"arn": first_arn, "clientToken": token});
    let first = ok_call(&router, "ExecuteScheduledReport", &request).await;
    assert_eq!(
        first.get("executionTriggered").and_then(Value::as_bool),
        Some(true),
        "the first call under a fresh token triggers the execution: {first}"
    );
    let executed = stored_report(&router, &first_arn).await;
    let first_execution_at = timestamp(&executed, "lastExecutionAt", "first execution");

    // Two retries under the same token: each answers the first call's outcome verbatim
    // and leaves the execution instant where the first call put it.
    for attempt in 0..2 {
        let retried = ok_call(&router, "ExecuteScheduledReport", &request).await;
        assert_eq!(
            retried, first,
            "case retry {attempt}: a retry under the same token answers the first outcome"
        );
        let after = stored_report(&router, &first_arn).await;
        assert_eq!(
            timestamp(&after, "lastExecutionAt", "retry"),
            first_execution_at,
            "case retry {attempt}: a retry must not trigger a second execution: {after}"
        );
    }

    // The same token for a DIFFERENT report is a different request, not a retry: it
    // triggers that report's execution and answers that report's health.
    let (status, body) = call(
        &router,
        "ExecuteScheduledReport",
        &json!({"arn": other_arn, "clientToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token another report used is no reason to refuse this one, got {status}: {body}"
    );
    assert_eq!(
        body.get("executionTriggered").and_then(Value::as_bool),
        Some(true),
        "the other report under the shared token is triggered: {body}"
    );
    assert_health(
        body.get("healthStatus")
            .unwrap_or_else(|| panic!("no healthStatus in {body}")),
        "shared token, other report",
        HEALTHY,
        None,
    );
    let executed_other = stored_report(&router, &other_arn).await;
    assert!(
        executed_other.get("lastExecutionAt").is_some(),
        "the call under the shared token executed the other report: {executed_other}"
    );

    // A fresh token, and no token at all, each execute again.
    for (label, request) in [
        (
            "fresh token",
            json!({"arn": first_arn, "clientToken": "another-execute-token-0a"}),
        ),
        ("no token", json!({"arn": first_arn})),
    ] {
        let answered = ok_call(&router, "ExecuteScheduledReport", &request).await;
        assert_eq!(
            answered.get("executionTriggered").and_then(Value::as_bool),
            Some(true),
            "case {label}: expected a fresh execution, got {answered}"
        );
        let after = stored_report(&router, &first_arn).await;
        let again = timestamp(&after, "lastExecutionAt", label);
        assert!(
            again >= first_execution_at,
            "case {label}: lastExecutionAt {again} went backwards from {first_execution_at}: {after}"
        );
    }

    // The other report can still be executed under its own token, and its retry under
    // the shared token replays its own outcome rather than triggering again.
    let answered = ok_call(
        &router,
        "ExecuteScheduledReport",
        &json!({"arn": other_arn, "clientToken": "other-report-token-0b"}),
    )
    .await;
    assert_eq!(
        answered.get("executionTriggered").and_then(Value::as_bool),
        Some(true),
        "the other report executes under its own token: {answered}"
    );
    let other_execution_at = timestamp(
        &stored_report(&router, &other_arn).await,
        "lastExecutionAt",
        "other report",
    );
    ok_call(
        &router,
        "ExecuteScheduledReport",
        &json!({"arn": other_arn, "clientToken": token}),
    )
    .await;
    assert_eq!(
        timestamp(
            &stored_report(&router, &other_arn).await,
            "lastExecutionAt",
            "other report retry"
        ),
        other_execution_at,
        "the other report's retry under the shared token triggers no second execution"
    );
}

/// An ARN no scheduled report is stored under is refused with the service's not-found
/// answer — the `ResourceNotFoundException` code at HTTP 400 with the requested ARN in
/// `resourceId` — on a populated store, for a report that was never created and for a
/// well-formed ARN in another partition, and whatever else the request carries.
#[tokio::test]
async fn executing_a_report_that_was_never_created_is_not_found() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "NotFoundExecSource").await;
    create_report(&router, &dashboard_arn, "The Only One", None, None).await;

    for (label, request) in [
        ("never created", json!({"arn": MISSING_REPORT_ARN})),
        (
            "never created, dry run",
            json!({"arn": MISSING_REPORT_ARN, "dryRun": true}),
        ),
        (
            "never created, with token",
            json!({"arn": MISSING_REPORT_ARN, "clientToken": "not-found-token-0001"}),
        ),
        (
            "another partition",
            json!({"arn": "arn:aws-cn:bcm-dashboards::000000000000:scheduled-report/ffffffffffffffffffffffffffffffff"}),
        ),
    ] {
        let (status, body) = call(&router, "ExecuteScheduledReport", &request).await;
        assert_eq!(
            status, 400,
            "case {label}: an unknown report arn answers 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body.get("resourceId").and_then(Value::as_str),
            request["arn"].as_str(),
            "case {label}: the refusal carries the requested arn in resourceId: {body}"
        );
    }
}
