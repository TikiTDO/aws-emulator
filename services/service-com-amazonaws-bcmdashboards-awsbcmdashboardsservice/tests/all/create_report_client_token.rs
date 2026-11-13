// SPDX-License-Identifier: CC0-1.0

//! What a `CreateScheduledReport` `clientToken` is, end to end over the wire: the create
//! key that makes one token answer for at most one scheduled report.
//!
//! The rule under test: the `@idempotencyToken` a create is presented with keys THAT
//! create, so a later call presenting it is answered from it rather than served a second
//! report (tabled over the shape of that later call in
//! `tests/all/create_report_token_spent.rs`) — and the claim a token holds lives exactly
//! as long as the report it created: once this service no longer stores that report there
//! is nothing left for the claim to name, so the token is free again and the same request
//! under it creates a report of its own.
//!
//! This suite owns the create the sibling suites present that token to, so there is one
//! description of the request those rows start from.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::service_state;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{ok_call, router};
use super::scheduled_report_lifecycle::{acceptable_period, source_dashboard, EXECUTION_ROLE_ARN};

/// The token the first create presents and every later call below presents again. Every
/// character is in the printable range `ClientToken`'s pattern `^[\u{21}-\u{7e}]+$`
/// admits (length 1..128, `smithy_query(query="shape",shape="ClientToken")`).
pub(crate) const TOKEN: &str = "sdk-generated-token-4f2c";

/// The create that spends the token: every optional member of the request populated, so
/// a sibling row can change exactly one of them.
pub(crate) fn first_request(dashboard_arn: &str, widget_id: &str) -> Value {
    json!({
        "clientToken": TOKEN,
        "scheduledReport": {
            "name": "Idempotent Rollup",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "scheduleExpressionTimeZone": "Europe/Berlin",
                // Both bounds are built relative to this instant, because each is held to
                // a rule measured from the request that submits it
                // (`tests/all/schedule_period_defaults.rs`).
                "schedulePeriod": acceptable_period()
            },
            "description": "Monthly spend rollup.",
            "widgetIds": [widget_id],
            "widgetDateRangeOverride": {
                "startTime": {"type": "RELATIVE", "value": "-P1M"},
                "endTime": {"type": "RELATIVE", "value": "P0D"}
            }
        },
        "resourceTags": [
            {"key": "team", "value": "finops"},
            {"key": "env", "value": "prod"}
        ]
    })
}

/// The ARNs of the scheduled reports this account stores.
pub(crate) async fn stored_arns(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok_call(router, "ListScheduledReports", &json!({})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answers a list: {listed}"))
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary carries an arn: {summary}"))
                .to_string()
        })
        .collect()
}

/// A row's own store, the create that spent the token on it, and what that create left:
/// the ARN it minted, the record a read answers for it, and a second dashboard for the
/// row that points its reuse elsewhere.
pub(crate) struct Served {
    pub(crate) router: MultiServiceRouter,
    pub(crate) request: Value,
    pub(crate) arn: String,
    pub(crate) stored: Value,
    pub(crate) other_dashboard: String,
}

/// Serve the create that spends the token, on a fresh store of its own.
pub(crate) async fn serve_first(label: &str) -> Served {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "ClientTokenSource").await;
    let (other_dashboard, _) = source_dashboard(&router, "ClientTokenOther").await;

    let request = first_request(&dashboard_arn, &widget_id);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the first create returns an arn: {created}"))
        .to_string();
    let stored = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
    Served {
        router,
        request,
        arn,
        stored,
        other_dashboard,
    }
}

/// A token whose report this service no longer stores holds a claim on nothing, so the
/// same request under it creates a report again and claims the token for that one — a
/// create is never refused in the name of an ARN that resolves to nothing, and the claims
/// do not outlive the reports they name. The removal here is made in the store the router
/// serves, so what is under test is the CREATE path's own sweep rather than the one
/// `DeleteScheduledReport` runs (that one is pinned by
/// `tests/all/delete_scheduled_report.rs`).
#[tokio::test]
async fn a_token_whose_report_is_gone_creates_again_instead_of_refusing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let (dashboard_arn, widget_id) = source_dashboard(&router, "GoneReportSource").await;
    let request = first_request(&dashboard_arn, &widget_id);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"].as_str().expect("an arn").to_string();

    // The report is removed from the store, as a delete would remove it.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        let mut state = state.lock();
        state.scheduled_reports.remove(&arn);
    }

    let again = ok_call(&router, "CreateScheduledReport", &request).await;
    let fresh_arn = again["arn"].as_str().expect("an arn").to_string();
    assert_ne!(
        fresh_arn, arn,
        "the create must mint a new report rather than be refused for the removed one: {again}"
    );
    assert_eq!(
        stored_arns(&router).await,
        vec![fresh_arn.clone()],
        "the store holds the report this create made"
    );
    assert_eq!(
        ok_call(&router, "GetScheduledReport", &json!({"arn": fresh_arn})).await
            ["scheduledReport"]["arn"]
            .as_str(),
        Some(fresh_arn.as_str()),
        "the report the second create made reads back under its own arn"
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the dashboard store")
            .lock()
            .scheduled_report_client_tokens
            .len(),
        1,
        "one claim per stored report, never one per token ever presented"
    );
}
