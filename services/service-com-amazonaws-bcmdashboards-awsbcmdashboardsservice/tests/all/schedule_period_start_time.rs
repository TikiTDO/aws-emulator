// SPDX-License-Identifier: CC0-1.0

//! The earliest start time this service accepts for a submitted schedule period, end to end
//! over the wire.
//!
//! The rule under test: the `startTime` of a `schedulePeriod` a write submits may be at most
//! 5 minutes before that request, and a write naming one further back is refused with
//! `ValidationException` (HTTP 400) and persists nothing. `SchedulePeriod$startTime` is
//! documented "The start time of the schedule period. If not specified, defaults to the time
//! of the create or update request. The start time cannot be more than 5 minutes before the
//! time of the request" (`smithy_query(query="shape",shape="SchedulePeriod")`) — a sentence
//! written for both write paths, so it is applied wherever a `scheduleConfig` is submitted:
//! `CreateScheduledReport` and `UpdateScheduledReport`. What the service stores for a bound
//! the request does NOT name is the other half of that sentence, tabled in
//! `tests/all/schedule_period_defaults.rs`; the ceiling its sibling member states on the
//! period's end is tabled in `tests/all/schedule_period_ceiling.rs`.
//!
//! The rows below are TABLES spanning that class rather than one request. The refusals differ
//! in which operation submits the period, in how far before the request the start time sits
//! (a minute past the grace, an hour, a day, a year, the epoch) and in whether the period
//! also names an end time or the request other members. The acceptances differ in where
//! relative to the request the start time sits (inside the grace, a minute before, the
//! instant of the request, a day on, a year on), each one stored and read back through
//! `GetScheduledReport`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
// The oracles this crate's period suites share: how a bound is spelled on the wire, how a
// refusal names one, and the read that answers a stored report. Declared beside the rule on
// the period's other bound.
use super::schedule_period_ceiling::{instant_text, now_secs, stored_record};
use super::scheduled_report_lifecycle::{acceptable_period, source_dashboard, EXECUTION_ROLE_ARN};

/// The documented grace, in seconds: "The start time cannot be more than 5 minutes before
/// the time of the request" (`smithy_query(query="shape",shape="SchedulePeriod")`, member
/// `startTime`).
const GRACE_SECONDS: i64 = 5 * 60;

/// A margin a row sits inside (or outside) the grace by, in seconds. The emulator reads its
/// own clock when the request arrives, which is never before the instant this test computed
/// its value at, so a row has to miss the bound by more than the time the whole table takes
/// to be built and served for its side of the bound to be the one under test — generous
/// room, since only the bound itself is tight.
const MARGIN: i64 = 120;

/// The whole message a refusal of the start time `secs` submitted at the member path `path`
/// must carry: the member that broke the rule, the instant it carried, and the rule itself.
fn refusal_message(path: &str, secs: i64) -> String {
    format!(
        "{path}.schedulePeriod.startTime: '{}' is before the earliest allowed value; the \
         start time of a schedule period may be at most 5 minutes before the time of the \
         request",
        instant_text(secs)
    )
}

/// One row of the refusal table.
struct RefusalCase {
    label: String,
    operation: &'static str,
    request: Value,
    /// The whole `message` the refusal must carry.
    message: String,
}

/// The refusal rows, built against the stored `report_arn` and `dashboard_arn` so a refused
/// row can be shown to have left the stored record alone.
fn refusal_cases(report_arn: &str, dashboard_arn: &str) -> Vec<RefusalCase> {
    let now = now_secs();
    // How far before the request each row's start time sits, by absolute instant. The last
    // row is the epoch itself, which no grace from any clock admits.
    let starts: Vec<(&str, i64)> = vec![
        ("a minute past the grace", now - (GRACE_SECONDS + MARGIN)),
        ("an hour before the request", now - 3_600),
        ("a day before the request", now - 86_400),
        ("a year before the request", now - 365 * 86_400),
        ("the epoch", 0),
    ];

    let mut cases = Vec::new();
    for (index, (label, start)) in starts.iter().copied().enumerate() {
        // The update path: every other member of the request is one the handler would have
        // accepted, so the start time is what the refusal is about.
        let mut update = json!({
            "arn": report_arn,
            "scheduleConfig": {
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "schedulePeriod": {"startTime": start}
            }
        });
        if index % 2 == 0 {
            // Some rows name an end time as well, and some carry other updatable members:
            // neither changes which member broke the rule.
            update["scheduleConfig"]["schedulePeriod"]["endTime"] = json!(now + 86_400);
            update["name"] = json!("Start Renamed");
        }
        cases.push(RefusalCase {
            label: format!("UpdateScheduledReport, {label}"),
            operation: "UpdateScheduledReport",
            request: update,
            message: refusal_message("scheduleConfig", start),
        });

        let mut schedule = json!({
            "scheduleExpression": "cron(0/15 * * * ? *)",
            "schedulePeriod": {"startTime": start}
        });
        if index % 2 == 1 {
            schedule["schedulePeriod"]["endTime"] = json!(now + 30 * 86_400);
            schedule["scheduleExpressionTimeZone"] = json!("Europe/Berlin");
        }
        cases.push(RefusalCase {
            label: format!("CreateScheduledReport, {label}"),
            operation: "CreateScheduledReport",
            request: json!({
                "scheduledReport": {
                    "name": format!("Start Create-{index}"),
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": schedule
                }
            }),
            message: refusal_message("scheduledReport.scheduleConfig", start),
        });
    }
    cases
}

/// Create a report on `dashboard_arn` whose `scheduleConfig` is `config`, and return its
/// ARN.
async fn report_with_config(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
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
                "scheduleConfig": config
            }
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// The stored start time of the report `record` carries.
fn stored_start(label: &str, record: &Value) -> f64 {
    record
        .get("scheduleConfig")
        .and_then(|config| config.get("schedulePeriod"))
        .and_then(|period| period.get("startTime"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {label}: the stored period carries no startTime: {record}")
        })
}

/// Every row whose start time sits further back than the documented grace is refused with
/// HTTP 400 `ValidationException` carrying the message the broken rule states, and persists
/// nothing: the refused update leaves the whole stored record as it was, and the refused
/// creates store no report at all.
#[tokio::test]
async fn a_start_time_before_the_documented_grace_is_refused() {
    let router = router();
    let (dashboard, _widget) = source_dashboard(&router, "StartTimeSource").await;
    let report = report_with_config(
        &router,
        &dashboard,
        "Start Keeper",
        &json!({
            "scheduleExpression": "cron(0 8 1 * ? *)",
            "schedulePeriod": acceptable_period()
        }),
        "setup",
    )
    .await;
    let before = stored_record(&router, &report, "setup").await;

    for case in refusal_cases(&report, &dashboard) {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 from {}, got {status}: {body}",
            case.label, case.operation
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {:?}: expected ValidationException, got {:?}: {body}",
            case.label,
            error_code(&body)
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            message, case.message,
            "case {:?}: expected the refusal to state its rule, got {message:?}",
            case.label
        );

        let after = stored_record(&router, &report, &case.label).await;
        assert_eq!(
            after, before,
            "case {:?}: the whole stored record must be untouched, got {after} after {before}",
            case.label
        );
    }

    // The refused creates minted no ARN, so the one report of this store is the one the
    // setup created.
    let listed = ok_call(&router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    let arns: Vec<&str> = listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("no scheduledReports list in {listed}"))
        .iter()
        .filter_map(|summary| summary.get("arn").and_then(Value::as_str))
        .collect();
    assert_eq!(
        arns,
        vec![report.as_str()],
        "the refused creates must have stored no scheduled report, got {arns:?}"
    );
}

/// One row of the acceptance table.
struct AcceptedStart {
    label: &'static str,
    start: i64,
}

/// The accepted rows: inside the grace, at the request, and on into the future — the side of
/// the bound the rule admits.
fn accepted_starts() -> Vec<AcceptedStart> {
    let now = now_secs();
    vec![
        AcceptedStart {
            label: "a minute inside the grace",
            start: now - (GRACE_SECONDS - MARGIN),
        },
        AcceptedStart {
            label: "a minute before the request",
            start: now - 60,
        },
        AcceptedStart {
            label: "this instant",
            start: now,
        },
        AcceptedStart {
            label: "a day from now",
            start: now + 86_400,
        },
        AcceptedStart {
            label: "a year from now",
            start: now + 365 * 86_400,
        },
    ]
}

/// A start time inside the grace, at the request or in the future is stored and reads back
/// verbatim, on the create path and on the update path alike.
#[tokio::test]
async fn a_start_time_inside_the_documented_grace_round_trips() {
    let router = router();
    let (dashboard, _widget) = source_dashboard(&router, "StartAcceptedSource").await;

    for (index, case) in accepted_starts().into_iter().enumerate() {
        let label = case.label;
        let arn = report_with_config(
            &router,
            &dashboard,
            &format!("Start Accepted-{index}"),
            &json!({
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "schedulePeriod": {"startTime": case.start}
            }),
            label,
        )
        .await;
        let created = stored_record(&router, &arn, label).await;
        assert_eq!(
            stored_start(label, &created),
            case.start as f64,
            "case {label}: the created startTime must be the submitted {}, got {created}",
            case.start
        );

        // The same start time through the update path, onto a report created without one.
        let other = report_with_config(
            &router,
            &dashboard,
            &format!("Start Updated-{index}"),
            &json!({"scheduleExpression": "cron(0 8 1 * ? *)"}),
            label,
        )
        .await;
        ok_call(
            &router,
            "UpdateScheduledReport",
            &json!({
                "arn": other,
                "scheduleConfig": {
                    "scheduleExpression": "cron(0 12 L * ? *)",
                    "schedulePeriod": {"startTime": case.start}
                }
            }),
        )
        .await;
        let updated = stored_record(&router, &other, label).await;
        assert_eq!(
            stored_start(label, &updated),
            case.start as f64,
            "case {label}: the updated startTime must be the submitted {}, got {updated}",
            case.start
        );
    }
}
