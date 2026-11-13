// SPDX-License-Identifier: CC0-1.0

//! `ListScheduledReports`, end to end over the wire.
//!
//! The rule under test: `ListScheduledReports` pages over the scheduled reports this
//! account stores, in ARN order, answering at most `maxResults` summaries per page
//! (the model's declared default, 50, when the member is absent), each one the
//! projection of the stored record that `GetScheduledReport` reads back, with
//! `nextToken` present exactly when a report remains after the page and resuming the
//! walk where it left off. It is a read: it changes nothing a later caller sees.
//!
//! Each assertion spans the class rather than one recorded request: the table below
//! differs in schedule expression (four cron forms and none at all), in schedule state
//! (absent, `ENABLED`, `DISABLED`), in whether a time zone is present and in widget-id
//! count (absent, empty, one), and the paging is walked at every page size from 1 to
//! past the stored count, plus the defaulted one.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, ok_call, router};
// The `HealthStatusCode` a report whose health nothing has refreshed reports, declared
// once for the crate's suites beside the other health constants.
use super::scheduled_report_execution::HEALTHY;
use super::scheduled_report_lifecycle::{
    acceptable_period, error_code, source_dashboard, EXECUTION_ROLE_ARN,
};

/// One row of the listing table: the `scheduleConfig` a report is created with, and
/// the summary members that configuration must be projected onto.
struct ReportRow {
    label: &'static str,
    name: &'static str,
    /// The `scheduleConfig` as submitted.
    schedule_config: Value,
    /// The `scheduleExpression` the summary must carry — the empty string where the
    /// stored schedule has none, since the member is `@required` in the summary shape
    /// and a `GenericString` of length 0..1024.
    expected_expression: &'static str,
    /// The `state` the summary must carry — `ENABLED` where the request omitted it.
    expected_state: &'static str,
    expected_time_zone: Option<&'static str>,
    /// `None` when the request omits `widgetIds`, otherwise the list it submits.
    widget_ids: Option<Vec<String>>,
}

/// The table. `widget_id` is a real widget of the source dashboard, so the row that
/// names a widget names an existing one.
fn rows(widget_id: &str) -> Vec<ReportRow> {
    vec![
        ReportRow {
            label: "cron expression, state omitted, no widgets",
            name: "Alpha Rollup",
            schedule_config: json!({"scheduleExpression": "cron(0 8 1 * ? *)"}),
            expected_expression: "cron(0 8 1 * ? *)",
            expected_state: "ENABLED",
            expected_time_zone: None,
            widget_ids: None,
        },
        ReportRow {
            label: "stepped cron expression, DISABLED, time zone, empty widget list",
            name: "Bravo Rollup",
            schedule_config: json!({
                "scheduleExpression": "cron(0/15 * * * ? *)",
                "scheduleExpressionTimeZone": "Europe/Berlin",
                "state": "DISABLED"
            }),
            expected_expression: "cron(0/15 * * * ? *)",
            expected_state: "DISABLED",
            expected_time_zone: Some("Europe/Berlin"),
            widget_ids: Some(vec![]),
        },
        ReportRow {
            label: "explicit ENABLED, one widget id, UTC",
            name: "Charlie Rollup",
            schedule_config: json!({
                "scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)",
                "scheduleExpressionTimeZone": "UTC",
                "state": "ENABLED"
            }),
            expected_expression: "cron(0 6 1 1,4,7,10 ? *)",
            expected_state: "ENABLED",
            expected_time_zone: Some("UTC"),
            widget_ids: Some(vec![widget_id.to_string()]),
        },
        ReportRow {
            label: "schedule with no expression at all",
            name: "Delta Rollup",
            // Every `ScheduleConfig` member is optional in the model, so a schedule
            // can be stored without an expression.
            schedule_config: json!({}),
            expected_expression: "",
            expected_state: "ENABLED",
            expected_time_zone: None,
            widget_ids: None,
        },
        ReportRow {
            label: "last-day-of-month cron expression, schedule period",
            name: "Echo Rollup",
            schedule_config: json!({
                "scheduleExpression": "cron(0 12 L * ? *)",
                // Both bounds relative to this instant, because each is held to a rule
                // measured from the request that submits it
                // (`tests/all/schedule_period_defaults.rs`).
                "schedulePeriod": acceptable_period()
            }),
            expected_expression: "cron(0 12 L * ? *)",
            expected_state: "ENABLED",
            expected_time_zone: None,
            widget_ids: Some(vec![]),
        },
    ]
}

/// A `CreateScheduledReport` request for `row`.
fn create_request(row: &ReportRow, dashboard_arn: &str) -> Value {
    let mut report = json!({
        "name": row.name,
        "dashboardArn": dashboard_arn,
        "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
        "scheduleConfig": row.schedule_config.clone(),
    });
    if let Some(widget_ids) = &row.widget_ids {
        report["widgetIds"] = json!(widget_ids);
    }
    json!({"scheduledReport": report})
}

/// A summary is the projection of the stored record: every member it carries comes
/// from the record `GetScheduledReport` answers, never from the listing request.
fn assert_summary_projects_the_record(item: &Value, record: &Value, row: &ReportRow, label: &str) {
    for member in ["arn", "name", "dashboardArn"] {
        assert_eq!(
            item.get(member),
            record.get(member),
            "case {label}: summary member {member} must equal the stored record's, got {item} vs {record}"
        );
    }

    let stored_config = record
        .get("scheduleConfig")
        .unwrap_or_else(|| panic!("case {label}: the stored record has no scheduleConfig: {record}"));
    assert_eq!(
        item.get("scheduleExpression").and_then(Value::as_str),
        Some(row.expected_expression),
        "case {label}: expected scheduleExpression {:?}, got {item}",
        row.expected_expression
    );
    assert_eq!(
        item.get("scheduleExpression").and_then(Value::as_str),
        Some(
            stored_config
                .get("scheduleExpression")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        "case {label}: scheduleExpression must come from the stored scheduleConfig, got {item} vs {stored_config}"
    );
    assert_eq!(
        item.get("state").and_then(Value::as_str),
        Some(row.expected_state),
        "case {label}: expected state {:?}, got {item}",
        row.expected_state
    );
    assert_eq!(
        item.get("state"),
        stored_config.get("state"),
        "case {label}: state must come from the stored scheduleConfig, got {item} vs {stored_config}"
    );
    assert_eq!(
        item.get("scheduleExpressionTimeZone").and_then(Value::as_str),
        row.expected_time_zone,
        "case {label}: expected scheduleExpressionTimeZone {:?}, got {item}",
        row.expected_time_zone
    );
    assert_eq!(
        item.get("scheduleExpressionTimeZone"),
        stored_config.get("scheduleExpressionTimeZone"),
        "case {label}: the time zone must come from the stored scheduleConfig, got {item} vs {stored_config}"
    );

    let expected_widget_ids = row.widget_ids.as_ref().map(|ids| json!(ids));
    assert_eq!(
        item.get("widgetIds"),
        expected_widget_ids.as_ref(),
        "case {label}: expected widgetIds {expected_widget_ids:?}, got {item}"
    );
    assert_eq!(
        item.get("widgetIds"),
        record.get("widgetIds"),
        "case {label}: widgetIds must equal the stored record's, got {item} vs {record}"
    );

    // `healthStatus` is `@required` in the summary shape, and nothing has executed or
    // refreshed these reports: the create validated them, so they summarise as HEALTHY
    // with no refresh time and no reasons (the model documents reasons as present only
    // when the status is UNHEALTHY).
    let health = item
        .get("healthStatus")
        .unwrap_or_else(|| panic!("case {label}: healthStatus is required in a summary: {item}"));
    assert_eq!(
        health.get("statusCode").and_then(Value::as_str),
        Some(HEALTHY),
        "case {label}: expected healthStatus.statusCode {HEALTHY}, got {health}"
    );
    assert!(
        health.get("statusReasons").is_none(),
        "case {label}: a healthy report carries no statusReasons, got {health}"
    );
    assert!(
        health.get("lastRefreshedAt").is_none(),
        "case {label}: nothing refreshed this report's health, got {health}"
    );
}

/// ListScheduledReports pages for real: every page holds at most `maxResults`
/// summaries projected from the stored records, `nextToken` appears exactly when
/// reports remain, and resuming from it walks the whole set exactly once — for every
/// page size, and for the default page size when `maxResults` is absent.
#[tokio::test]
async fn list_scheduled_reports_pages_over_the_stored_reports() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "ListingSource").await;

    // An account with no scheduled reports: the required member is an empty list and
    // there is no cursor.
    let empty = ok_call(&router, "ListScheduledReports", &json!({})).await;
    assert_eq!(
        empty["scheduledReports"].as_array().map(Vec::len),
        Some(0),
        "case empty store: expected an empty scheduledReports list, got {empty}"
    );
    assert!(
        empty.get("nextToken").is_none(),
        "case empty store: an exhausted listing carries no nextToken: {empty}"
    );

    let table = rows(&widget_id);
    let mut row_of_arn: BTreeMap<String, usize> = BTreeMap::new();
    let mut expected: Vec<String> = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &create_request(row, &dashboard_arn),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no arn in {created}", row.label))
            .to_string();
        row_of_arn.insert(arn.clone(), index);
        expected.push(arn);
    }
    // The store is keyed by ARN, so the listing order is ARN order.
    expected.sort();

    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok_call(&router, "ListScheduledReports", &request).await;
            pages += 1;
            let items = page["scheduledReports"]
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size}: no scheduledReports: {page}"))
                .clone();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page returned {} items: {page}",
                items.len()
            );
            for item in &items {
                let arn = item["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case page size {page_size}: no arn in {item}"))
                    .to_string();
                let index = *row_of_arn
                    .get(&arn)
                    .unwrap_or_else(|| panic!("case page size {page_size}: {arn} was never created"));
                let row = &table[index];
                let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
                assert_summary_projects_the_record(
                    item,
                    &fetched["scheduledReport"],
                    row,
                    &format!("page size {page_size}, {}", row.label),
                );
                collected.push(arn);
            }
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= table.len(),
                "case page size {page_size}: pagination did not terminate"
            );
        }

        assert_eq!(
            collected, expected,
            "case page size {page_size}: paging must walk every report exactly once, in order"
        );
        let want_pages = table.len().div_ceil(page_size);
        assert_eq!(
            pages, want_pages,
            "case page size {page_size}: expected {want_pages} pages, walked {pages}"
        );
    }

    // `maxResults` absent: the modelled default (50) covers all five, so no cursor.
    let defaulted = ok_call(&router, "ListScheduledReports", &json!({})).await;
    assert_eq!(
        defaulted["scheduledReports"]
            .as_array()
            .expect("scheduledReports")
            .len(),
        table.len(),
        "case default page size: expected every report on one page: {defaulted}"
    );
    assert!(
        defaulted.get("nextToken").is_none(),
        "case default page size: the last page carries no nextToken: {defaulted}"
    );

    // A read: listing twice answers the same thing, and the stored records are
    // untouched by having been listed.
    assert_eq!(
        ok_call(&router, "ListScheduledReports", &json!({})).await,
        defaulted,
        "listing twice must answer the same page"
    );
    for arn in &expected {
        let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["scheduledReport"]["arn"].as_str(),
            Some(arn.as_str()),
            "the listing must not have changed the stored record: {fetched}"
        );
    }
}

/// The default page size is the one the model declares for `maxResults` (50), not the
/// one `ListDashboards` declares (20): with 51 reports stored, a request that names no
/// `maxResults` answers 50 of them plus a cursor, and the cursor answers the last one.
#[tokio::test]
async fn the_defaulted_page_size_is_the_modelled_default() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "DefaultPageSource").await;

    // Source: `maxResults` of `ListScheduledReportsRequest`, default 50
    // (`smithy_query(query="operation",operation="ListScheduledReports")`).
    const MODELLED_DEFAULT: usize = 50;
    let stored = MODELLED_DEFAULT + 1;
    for index in 0..stored {
        ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Bulk Report {index:03}"),
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
        )
        .await;
    }

    let first = ok_call(&router, "ListScheduledReports", &json!({})).await;
    let items = first["scheduledReports"]
        .as_array()
        .expect("scheduledReports");
    assert_eq!(
        items.len(),
        MODELLED_DEFAULT,
        "case defaulted: expected {MODELLED_DEFAULT} summaries on the first page, got {}",
        items.len()
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case defaulted: a report remains, so a cursor is owed: {first}"))
        .to_string();

    // Asking for the default explicitly is the same request.
    let explicit = ok_call(
        &router,
        "ListScheduledReports",
        &json!({"maxResults": MODELLED_DEFAULT}),
    )
    .await;
    assert_eq!(
        explicit, first,
        "case explicit {MODELLED_DEFAULT}: naming the default must answer the defaulted page"
    );

    let second = ok_call(&router, "ListScheduledReports", &json!({"nextToken": token})).await;
    assert_eq!(
        second["scheduledReports"]
            .as_array()
            .expect("scheduledReports")
            .len(),
        stored - MODELLED_DEFAULT,
        "case resumed: expected the remaining reports, got {second}"
    );
    assert!(
        second.get("nextToken").is_none(),
        "case resumed: the last page carries no nextToken: {second}"
    );
}

/// `maxResults` outside its modelled 1..100 range, and a `nextToken` this service
/// never minted for a scheduled-report listing — including a cursor minted by the
/// dashboard listing — are both refused with `ValidationException` (400).
#[tokio::test]
async fn list_scheduled_reports_refuses_a_bad_page_request() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "BadPageSource").await;
    // A second dashboard, so the dashboard listing has a cursor to mint.
    ok_call(
        &router,
        "CreateDashboard",
        &json!({"name": "Second Board", "widgets": []}),
    )
    .await;
    for name in ["First Report", "Second Report"] {
        ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": name,
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
                }
            }),
        )
        .await;
    }

    // Source: `maxResults` range 1..100 — each published bound, and one past it.
    let max_results_cases: &[(&str, i64, bool)] = &[
        ("at min", 1, true),
        ("at max", 100, true),
        ("below min", 0, false),
        ("above max", 101, false),
        ("negative", -1, false),
    ];
    for (label, max_results, accepted) in max_results_cases {
        let (status, body) = call(
            &router,
            "ListScheduledReports",
            &json!({"maxResults": max_results}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case maxResults {label}: {max_results} is inside 1..100, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults {label}: {max_results} is outside 1..100, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case maxResults {label}: expected ValidationException, got {}",
                error_code(&body)
            );
        }
    }

    // A cursor the DASHBOARD listing minted names a dashboard, not a report, so it is
    // not a resume point for this listing.
    let dashboard_page = ok_call(&router, "ListDashboards", &json!({"maxResults": 1})).await;
    let dashboard_cursor = dashboard_page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("two dashboards are stored, so the first page owes a cursor")
        .to_string();

    for token in [
        "not-a-token",
        "v1:",
        "v1:zzzz",
        "v1:6e6f",
        "deadbeef",
        "{\"page\": 2}",
        dashboard_cursor.as_str(),
    ] {
        let (status, body) =
            call(&router, "ListScheduledReports", &json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: an uninterpretable nextToken must be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case token {token:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }

    // And the mirror image: a cursor THIS listing minted is not a resume point for the
    // dashboard listing.
    let report_page = ok_call(&router, "ListScheduledReports", &json!({"maxResults": 1})).await;
    let report_cursor = report_page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("two reports are stored, so the first page owes a cursor")
        .to_string();
    let (status, body) = call(
        &router,
        "ListDashboards",
        &json!({"nextToken": report_cursor}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case report cursor on ListDashboards: expected a refusal, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case report cursor on ListDashboards: expected ValidationException, got {}",
        error_code(&body)
    );
}

/// The listing answers the stored records as they are now: a report created after a
/// listing shows up in the next one, and every summary it answers still matches the
/// record `GetScheduledReport` reads back.
#[tokio::test]
async fn a_report_created_after_a_listing_is_listed_next_time() {
    let router = router();
    let (dashboard_arn, _) = source_dashboard(&router, "FreshSource").await;

    let before = ok_call(&router, "ListScheduledReports", &json!({})).await;
    assert_eq!(
        before["scheduledReports"].as_array().map(Vec::len),
        Some(0),
        "case before: a fresh account stores no reports: {before}"
    );

    let mut created_arns: Vec<String> = Vec::new();
    for (index, name) in ["First Added", "Second Added", "Third Added"]
        .into_iter()
        .enumerate()
    {
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": name,
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 12 L * ? *)"}
                }
            }),
        )
        .await;
        created_arns.push(created["arn"].as_str().expect("an arn").to_string());
        created_arns.sort();

        let listed = ok_call(&router, "ListScheduledReports", &json!({})).await;
        let listed_arns: Vec<String> = listed["scheduledReports"]
            .as_array()
            .expect("scheduledReports")
            .iter()
            .map(|item| item["arn"].as_str().unwrap_or("<none>").to_string())
            .collect();
        assert_eq!(
            listed_arns, created_arns,
            "case after create {index}: the listing must hold exactly the stored reports: {listed}"
        );
        assert_eq!(
            listed["scheduledReports"][0]["name"].as_str(),
            ok_call(
                &router,
                "GetScheduledReport",
                &json!({"arn": created_arns[0]})
            )
            .await["scheduledReport"]["name"]
                .as_str(),
            "case after create {index}: the summary's name is the stored record's: {listed}"
        );
    }
}
