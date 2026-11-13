// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateScheduledReport` call leaves inside the stored `scheduleConfig`, end to
//! end over the wire.
//!
//! The rule under test: an update applies only the properties its request names — "Only
//! the parameters included in the request are updated; all other properties remain
//! unchanged" (`smithy_query(query="doc",shape="UpdateScheduledReport")`) — and a property
//! of a submitted STRUCTURE is named only when that structure carries it, so a submitted
//! `scheduleConfig` is merged into the stored one property by property rather than
//! swapping it: `scheduleExpression`, `scheduleExpressionTimeZone`, `schedulePeriod` and
//! `state` (the four members of `ScheduleConfig`, source:
//! `smithy_query(query="shape",shape="ScheduleConfig")`) each move only when the submitted
//! config carries that member, and the request models no flag that clears any of them —
//! only `clearWidgetIds` and `clearWidgetDateRangeOverride`
//! (`smithy_query(query="operation",operation="UpdateScheduledReport")`).
//!
//! The rows below are a TABLE spanning that class rather than one request: they differ in
//! WHICH subset of the config's properties the update names (each one on its own, two at a
//! time, all four, and none at all), in the KIND of the property (two strings, a nested
//! structure, an enum), in the FORM of the values submitted (a cron and a rate expression,
//! two time zones, a period with other bounds, both schedule states) and in WHAT the report
//! was created with (a config carrying every property, and a config carrying the expression
//! alone, where there is nothing stored for an absent property to keep). Every row asserts
//! the whole stored config through `GetScheduledReport` and the three of its properties the
//! `ListScheduledReports` summary carries, so the fix is visible in both views.
//!
//! A `schedulePeriod` a row submits always carries BOTH bounds: inside a submitted period
//! an omitted bound is not "unchanged" but the default its own member documents ("defaults
//! to the time of the create or update request", "defaults to 3 years from the time of the
//! create or update request", `smithy_query(query="shape",shape="SchedulePeriod")`), so a
//! period the request carries is one whole parameter.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// Epoch seconds now, the form `SchedulePeriod`'s bounds take on the wire.
fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs() as i64
}

/// One day in seconds, the step the periods below are built from.
const DAY: i64 = 86_400;

/// A period starting `in_days` from now and running for `for_days`, well inside the
/// three-year ceiling `SchedulePeriod$endTime` documents.
fn period(in_days: i64, for_days: i64) -> Value {
    let start = epoch_now() + in_days * DAY;
    json!({"startTime": start, "endTime": start + for_days * DAY})
}

/// One row: the config the report is created with, the config the update submits, and the
/// config the stored record must read back as.
struct MergeCase {
    label: String,
    created: Value,
    submitted: Value,
    expected: Value,
}

/// The table.
fn cases() -> Vec<MergeCase> {
    // Both periods are built ONCE, so a row's expectation names the very instants its
    // request submitted rather than a second reading of the clock.
    let created_period = period(0, 365);
    let other_period = period(7, 30);
    let full = json!({
        "scheduleExpression": "cron(0 8 1 * ? *)",
        "scheduleExpressionTimeZone": "America/New_York",
        "schedulePeriod": created_period,
        "state": "DISABLED"
    });
    // A config carrying one property: an absent property of the submitted config has
    // nothing stored to keep, so it stays absent.
    let sparse = json!({"scheduleExpression": "cron(0 8 1 * ? *)"});

    let mut cases = Vec::new();

    // One property at a time, over a config that carries all four.
    for (label, submitted, changed) in [
        (
            "the expression alone",
            json!({"scheduleExpression": "cron(0 9 1 * ? *)"}),
            json!({"scheduleExpression": "cron(0 9 1 * ? *)"}),
        ),
        (
            "the time zone alone",
            json!({"scheduleExpressionTimeZone": "Europe/Berlin"}),
            json!({"scheduleExpressionTimeZone": "Europe/Berlin"}),
        ),
        (
            "the period alone",
            json!({"schedulePeriod": other_period}),
            json!({"schedulePeriod": other_period}),
        ),
        (
            "the state alone",
            json!({"state": "ENABLED"}),
            json!({"state": "ENABLED"}),
        ),
        (
            "the expression and the time zone",
            json!({
                "scheduleExpression": "rate(7 days)",
                "scheduleExpressionTimeZone": "UTC"
            }),
            json!({
                "scheduleExpression": "rate(7 days)",
                "scheduleExpressionTimeZone": "UTC"
            }),
        ),
        (
            "the expression and the state",
            json!({"scheduleExpression": "rate(1 day)", "state": "ENABLED"}),
            json!({"scheduleExpression": "rate(1 day)", "state": "ENABLED"}),
        ),
        (
            "the period and the state",
            json!({"schedulePeriod": other_period, "state": "ENABLED"}),
            json!({"schedulePeriod": other_period, "state": "ENABLED"}),
        ),
        (
            "no property at all",
            json!({}),
            // Nothing is named, so the stored config stands whole — and the state the
            // account last decided is not re-defaulted to ENABLED.
            json!({}),
        ),
        (
            "every property at once",
            json!({
                "scheduleExpression": "cron(0 7 ? * MON *)",
                "scheduleExpressionTimeZone": "Asia/Tokyo",
                "schedulePeriod": other_period,
                "state": "ENABLED"
            }),
            json!({
                "scheduleExpression": "cron(0 7 ? * MON *)",
                "scheduleExpressionTimeZone": "Asia/Tokyo",
                "schedulePeriod": other_period,
                "state": "ENABLED"
            }),
        ),
    ] {
        cases.push(MergeCase {
            label: format!("a config carrying every property, updated with {label}"),
            created: full.clone(),
            submitted,
            expected: overlay(&full, &changed),
        });
    }

    // The same subsets over a config that carries the expression alone: an absent
    // property has nothing to keep, so it reads back absent — except `state`, which
    // every stored config carries once the create resolved its documented default.
    for (label, submitted, changed) in [
        (
            "the expression alone",
            json!({"scheduleExpression": "rate(2 days)"}),
            json!({"scheduleExpression": "rate(2 days)"}),
        ),
        (
            "the time zone alone",
            json!({"scheduleExpressionTimeZone": "UTC"}),
            json!({"scheduleExpressionTimeZone": "UTC"}),
        ),
        (
            "the period alone",
            json!({"schedulePeriod": other_period}),
            json!({"schedulePeriod": other_period}),
        ),
        ("no property at all", json!({}), json!({})),
    ] {
        cases.push(MergeCase {
            label: format!("a config carrying the expression alone, updated with {label}"),
            created: sparse.clone(),
            // A create that names no state stores the ENABLED default the create member
            // documents, and no row here names one.
            expected: overlay(&overlay(&sparse, &json!({"state": "ENABLED"})), &changed),
            submitted,
        });
    }

    cases
}

/// `base` with every property of `changed` written over it — the merge the rule calls for,
/// computed here so a row states only what its update names.
fn overlay(base: &Value, changed: &Value) -> Value {
    let mut merged = base.clone();
    let members = merged
        .as_object_mut()
        .expect("a scheduleConfig row is a JSON object");
    for (member, value) in changed
        .as_object()
        .expect("a scheduleConfig row is a JSON object")
    {
        members.insert(member.clone(), value.clone());
    }
    merged
}

/// Create a report on `dashboard_arn` whose schedule is `config`, and return its ARN.
async fn report_with_config(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    widget_id: &str,
    name: &str,
    config: &Value,
    label: &str,
) -> String {
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": name,
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": config,
                "widgetIds": [widget_id]
            }
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// The `scheduleConfig` of the stored report `arn`, as `GetScheduledReport` answers it.
async fn stored_config(router: &MultiServiceRouter, arn: &str, label: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .and_then(|report| report.get("scheduleConfig"))
        .unwrap_or_else(|| panic!("case {label}: no scheduleConfig in {fetched}"))
        .clone()
}

/// The `ListScheduledReports` summary of `arn`.
async fn listed_summary(router: &MultiServiceRouter, arn: &str, label: &str) -> Value {
    let page = ok_call(router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    page["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: no scheduledReports list in {page}"))
        .iter()
        .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {label}: {arn} is not listed in {page}"))
        .clone()
}

/// Two renderings of one `schedulePeriod` compare equal: the bounds are timestamps, so a
/// stored bound comes back as a JSON number that need not be spelled as the row spelled it.
fn comparable(config: &Value) -> Value {
    let mut config = config.clone();
    if let Some(period) = config
        .get_mut("schedulePeriod")
        .and_then(Value::as_object_mut)
    {
        for bound in ["startTime", "endTime"] {
            if let Some(value) = period.get(bound).and_then(Value::as_f64) {
                period.insert(bound.to_string(), json!(value));
            }
        }
    }
    config
}

/// Every row: the update writes the properties its config names over the stored config and
/// leaves every property it does not name exactly as stored, in both views of the record.
#[tokio::test]
async fn an_update_merges_the_submitted_schedule_config_into_the_stored_one() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ScheduleMergeSource").await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label.as_str();
        let arn = report_with_config(
            &router,
            &dashboard,
            &widget,
            &format!("Merge Report-{index}"),
            &case.created,
            label,
        )
        .await;

        // The config the create left, read before the update: a row whose update names no
        // period keeps this one.
        let created_config = stored_config(&router, &arn, label).await;

        ok_call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "scheduleConfig": case.submitted}),
        )
        .await;

        let stored = stored_config(&router, &arn, label).await;
        // No row submits a `schedulePeriod` it does not also expect, and a submitted
        // config that names none leaves the stored period alone — the period the create
        // resolved from the defaults its two bounds document. So a row whose expectation
        // names no period inherits the one the create stored: what those bounds resolve to
        // is the rule `tests/all/schedule_period_defaults.rs` pins, and this suite is
        // about which properties an update moves.
        let mut expected = case.expected.clone();
        if expected.get("schedulePeriod").is_none() {
            if let Some(created_period) = created_config.get("schedulePeriod") {
                expected
                    .as_object_mut()
                    .expect("a scheduleConfig row is a JSON object")
                    .insert("schedulePeriod".to_string(), created_period.clone());
            }
        }
        assert_eq!(
            comparable(&stored),
            comparable(&expected),
            "case {label}: expected the stored scheduleConfig to read back as {expected}, got {stored}"
        );

        // The listing projects three of those properties, so it has to agree with the
        // detail view member for member.
        let summary = listed_summary(&router, &arn, label).await;
        assert_eq!(
            summary.get("scheduleExpression"),
            stored.get("scheduleExpression"),
            "case {label}: the summary's scheduleExpression must match the stored one, \
             got {summary}"
        );
        assert_eq!(
            summary.get("scheduleExpressionTimeZone"),
            stored.get("scheduleExpressionTimeZone"),
            "case {label}: the summary's scheduleExpressionTimeZone must match the stored \
             one, got {summary}"
        );
        assert_eq!(
            summary.get("state"),
            stored.get("state"),
            "case {label}: the summary's state must match the stored one, got {summary}"
        );
    }
}

/// Successive updates accumulate: each one writes its own property and keeps what the
/// previous ones wrote, so the config at the end is every property any call named.
#[tokio::test]
async fn successive_partial_updates_accumulate_into_one_schedule_config() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ScheduleAccumulateSource").await;
    let label = "successive partial updates";
    let submitted_period = period(7, 30);
    let arn = report_with_config(
        &router,
        &dashboard,
        &widget,
        "Accumulated Report",
        &json!({
            "scheduleExpression": "cron(0 8 1 * ? *)",
            "scheduleExpressionTimeZone": "America/New_York",
            "state": "DISABLED"
        }),
        label,
    )
    .await;

    for submitted in [
        json!({"scheduleExpression": "cron(0 9 1 * ? *)"}),
        json!({"schedulePeriod": submitted_period}),
        json!({"scheduleExpressionTimeZone": "Europe/Berlin"}),
    ] {
        ok_call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": arn, "scheduleConfig": submitted}),
        )
        .await;
    }

    let stored = stored_config(&router, &arn, label).await;
    let expected = json!({
        "scheduleExpression": "cron(0 9 1 * ? *)",
        "scheduleExpressionTimeZone": "Europe/Berlin",
        "schedulePeriod": submitted_period,
        "state": "DISABLED"
    });
    assert_eq!(
        comparable(&stored),
        comparable(&expected),
        "case {label}: expected the accumulated scheduleConfig {expected}, got {stored}"
    );
}
