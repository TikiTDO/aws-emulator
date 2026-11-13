// SPDX-License-Identifier: CC0-1.0

//! The schedule STATE an `UpdateScheduledReport` call leaves behind, end to end over the
//! wire.
//!
//! The rule under test: an update applies only the properties its request names — "Only
//! the parameters included in the request are updated; all other properties remain
//! unchanged" (`smithy_query(query="doc",shape="UpdateScheduledReport")`) — and the
//! ENABLED default is documented on the CREATE member alone
//! (`ScheduledReportInput$scheduleConfig`: "If the schedule state is not specified, it
//! defaults to ENABLED", `smithy_query(query="shape",shape="ScheduledReportInput")`), while
//! `UpdateScheduledReportRequest$scheduleConfig` documents no default and the request
//! models no flag that clears a schedule state. So an update whose `scheduleConfig` names
//! no `state` keeps the state the account last decided — a caller changing a cron
//! expression cannot resume a report it paused — and only a `state` the request names
//! moves it.
//!
//! The rows below are a TABLE spanning that class rather than one request: they differ in
//! the state the report was CREATED with (`DISABLED`, `ENABLED`, and a create that names
//! none), in WHAT the update submits (a cron expression, a rate expression, a time zone, a
//! schedule period, an empty config, no `scheduleConfig` at all) and in whether the update
//! NAMES a state (none, `ENABLED`, `DISABLED`). Every row asserts the three places the
//! state is observable: `GetScheduledReport`, the `ListScheduledReports` summary, and
//! whether `ExecuteScheduledReport` — which refuses a report that is not ENABLED — runs it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// A `SchedulePeriod` a row can submit: two epoch-second instants, the form the
/// `schedulePeriod` member takes on the wire.
const PERIOD_START: i64 = 1_767_225_600;
const PERIOD_END: i64 = 1_798_761_600;

/// One row: how the report is created, what the update submits, and the state the stored
/// record must read back as.
struct StateCase {
    label: &'static str,
    /// The `state` the create names inside its `scheduleConfig`; `None` for a create that
    /// names none, which the create member's documented default stores as `ENABLED`.
    created_state: Option<&'static str>,
    /// The `scheduleConfig` the update submits, or `None` for an update that names no
    /// schedule at all — it renames the report instead.
    submitted: Option<Value>,
    expected_state: &'static str,
}

fn cases() -> Vec<StateCase> {
    vec![
        StateCase {
            label: "a paused report keeps DISABLED when the update only re-crons it",
            created_state: Some("DISABLED"),
            submitted: Some(json!({"scheduleExpression": "cron(0 9 1 * ? *)"})),
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a paused report keeps DISABLED across a rate expression and a time zone",
            created_state: Some("DISABLED"),
            submitted: Some(json!({
                "scheduleExpression": "rate(7 days)",
                "scheduleExpressionTimeZone": "Europe/Berlin"
            })),
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a paused report keeps DISABLED when the update only sets a period",
            created_state: Some("DISABLED"),
            submitted: Some(json!({
                "schedulePeriod": {"startTime": PERIOD_START, "endTime": PERIOD_END}
            })),
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a paused report keeps DISABLED when the update submits an empty config",
            created_state: Some("DISABLED"),
            submitted: Some(json!({})),
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a paused report keeps DISABLED when the update names no schedule at all",
            created_state: Some("DISABLED"),
            submitted: None,
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a paused report is resumed by an update that names ENABLED",
            created_state: Some("DISABLED"),
            submitted: Some(json!({
                "scheduleExpression": "cron(0 9 1 * ? *)",
                "state": "ENABLED"
            })),
            expected_state: "ENABLED",
        },
        StateCase {
            label: "an enabled report keeps ENABLED when the update names no state",
            created_state: Some("ENABLED"),
            submitted: Some(json!({"scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)"})),
            expected_state: "ENABLED",
        },
        StateCase {
            label: "an enabled report is paused by an update that names DISABLED",
            created_state: Some("ENABLED"),
            submitted: Some(json!({
                "scheduleExpression": "cron(0 6 1 1,4,7,10 ? *)",
                "state": "DISABLED"
            })),
            expected_state: "DISABLED",
        },
        StateCase {
            label: "a report created without a state keeps the created default",
            created_state: None,
            submitted: Some(json!({"scheduleExpression": "rate(1 day)"})),
            expected_state: "ENABLED",
        },
        StateCase {
            label: "a report created without a state is paused by an update that names DISABLED",
            created_state: None,
            submitted: Some(json!({"state": "DISABLED"})),
            expected_state: "DISABLED",
        },
    ]
}

/// Create a report on `dashboard_arn` whose schedule carries `state` (or no state at all),
/// and return its ARN.
async fn report_in_state(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    widget_id: &str,
    name: &str,
    state: Option<&str>,
    label: &str,
) -> String {
    let mut schedule = json!({
        "scheduleExpression": "cron(0 8 1 * ? *)",
        "scheduleExpressionTimeZone": "UTC"
    });
    if let Some(state) = state {
        schedule["state"] = json!(state);
    }
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": name,
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": schedule,
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
async fn stored_schedule(router: &MultiServiceRouter, arn: &str, label: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .and_then(|report| report.get("scheduleConfig"))
        .unwrap_or_else(|| panic!("case {label}: no scheduleConfig in {fetched}"))
        .clone()
}

/// The `state` the `ListScheduledReports` summary of `arn` reports.
async fn listed_state(router: &MultiServiceRouter, arn: &str, label: &str) -> String {
    let page = ok_call(router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    let summaries = page["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: no scheduledReports list in {page}"));
    let summary = summaries
        .iter()
        .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {label}: {arn} is not listed in {page}"));
    summary
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: the summary carries no state: {summary}"))
        .to_string()
}

/// Every row: the update moves the schedule state only when it names one, the state it
/// leaves behind is what both reads report, and a property the submitted config does not
/// name keeps the stored value.
#[tokio::test]
async fn an_update_moves_the_schedule_state_only_when_it_names_one() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "ScheduleStateSource").await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let arn = report_in_state(
            &router,
            &dashboard,
            &widget,
            &format!("State Report-{index}"),
            case.created_state,
            label,
        )
        .await;

        let mut request = json!({"arn": arn});
        match &case.submitted {
            Some(schedule) => request["scheduleConfig"] = schedule.clone(),
            // A row that names no schedule still has to be an update of something, so
            // the state it leaves behind is the state nothing in the request named.
            None => request["name"] = json!(format!("State Renamed-{index}")),
        }
        // The schedule the create left, read before the update: a row that submits no
        // period keeps it.
        let before = stored_schedule(&router, &arn, label).await;
        ok_call(&router, "UpdateScheduledReport", &request).await;

        let schedule = stored_schedule(&router, &arn, label).await;
        assert_eq!(
            schedule.get("state").and_then(Value::as_str),
            Some(case.expected_state),
            "case {label}: expected scheduleConfig.state {:?}, got {schedule}",
            case.expected_state
        );
        assert_eq!(
            listed_state(&router, &arn, label).await,
            case.expected_state,
            "case {label}: the listing must report the same state as the detail view",
        );

        // A property of the submitted config is named only when that config carries it:
        // the properties it names replace what was stored, and the ones it leaves out keep
        // the stored value (the create above names a time zone, so there is one to keep).
        if let Some(submitted) = &case.submitted {
            for (member, created) in [
                ("scheduleExpression", json!("cron(0 8 1 * ? *)")),
                ("scheduleExpressionTimeZone", json!("UTC")),
            ] {
                let expected = submitted.get(member).unwrap_or(&created);
                assert_eq!(
                    schedule.get(member),
                    Some(expected),
                    "case {label}: expected scheduleConfig.{member} {expected:?}, got {schedule}"
                );
            }
            // The create names no period, so it stored the period the two bounds of
            // `SchedulePeriod` document defaults for; a row that submits one replaces that
            // period whole, and a row that submits none keeps it.
            for member in ["startTime", "endTime"] {
                let expected = submitted
                    .get("schedulePeriod")
                    .and_then(|period| period.get(member))
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        before
                            .get("schedulePeriod")
                            .and_then(|period| period.get(member))
                            .and_then(Value::as_f64)
                    });
                assert_eq!(
                    schedule
                        .get("schedulePeriod")
                        .and_then(|period| period.get(member))
                        .and_then(Value::as_f64),
                    expected,
                    "case {label}: expected schedulePeriod.{member} {expected:?}, got {schedule}"
                );
            }
        }

        // The consequence the state has: an execution is refused for a report that is not
        // ENABLED, and runs for one that is.
        let (status, body) = call(&router, "ExecuteScheduledReport", &json!({"arn": arn})).await;
        if case.expected_state == "ENABLED" {
            assert_eq!(
                status, 200,
                "case {label}: an ENABLED report executes, got {status}: {body}"
            );
            assert_eq!(
                body.get("executionTriggered").and_then(Value::as_bool),
                Some(true),
                "case {label}: expected executionTriggered true, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label}: a report that is not ENABLED is refused, got {status}: {body}"
            );
            let code = error_code(&body);
            assert!(
                code.ends_with("ValidationException"),
                "case {label}: expected ValidationException, got {code:?}: {body}"
            );
            let message = body.get("message").and_then(Value::as_str).unwrap_or("");
            assert!(
                message.contains("ENABLED") && message.contains(arn.as_str()),
                "case {label}: expected a message naming the required state and the report, \
                 got {body}"
            );
        }
    }
}

/// A paused report survives a sequence of updates that never name a state: each one
/// changes its own parameter and none of them resumes the schedule.
#[tokio::test]
async fn successive_stateless_updates_never_resume_a_paused_report() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "PausedSequenceSource").await;
    let label = "successive stateless updates";
    let arn = report_in_state(
        &router,
        &dashboard,
        &widget,
        "Paused Report",
        Some("DISABLED"),
        label,
    )
    .await;

    for request in [
        json!({"arn": arn, "name": "Paused Renamed"}),
        json!({"arn": arn, "description": "Still paused."}),
        json!({"arn": arn, "scheduleConfig": {"scheduleExpression": "cron(0 10 1 * ? *)"}}),
        json!({"arn": arn, "clearWidgetIds": true}),
    ] {
        ok_call(&router, "UpdateScheduledReport", &request).await;
        let schedule = stored_schedule(&router, &arn, label).await;
        assert_eq!(
            schedule.get("state").and_then(Value::as_str),
            Some("DISABLED"),
            "case {label}: {request} must leave the schedule paused, got {schedule}"
        );
    }
}
