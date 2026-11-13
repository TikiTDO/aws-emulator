// SPDX-License-Identifier: CC0-1.0

//! What this service accepts as the active period of a submitted schedule, and what it
//! refuses, end to end over the wire.
//!
//! The rule under test: the `endTime` of a `schedulePeriod` a write submits may be at most
//! 3 years past that request, and a write that reaches beyond the ceiling is refused with
//! `ValidationException` (HTTP 400) and persists nothing. `SchedulePeriod$endTime` is
//! documented "The end time of the schedule period. If not specified, defaults to 3 years
//! from the time of the create or update request. The maximum allowed value is 3 years from
//! the current time. Setting an end time beyond this limit returns a
//! `ValidationException`" (`smithy_query(query="shape",shape="SchedulePeriod")`) — a
//! sentence written for both write paths, so it is applied wherever a `scheduleConfig` is
//! submitted: `CreateScheduledReport` and `UpdateScheduledReport`.
//!
//! The rows below are a TABLE spanning that class rather than one request. The refusals
//! differ in which operation submits the period, in how far past the ceiling the end time
//! reaches (a minute, a day, a year, centuries), and in whether the period also names a
//! start time or the request other members. The acceptances differ in where inside the
//! window the end time sits (the published ceiling itself, just inside it, a day away) and
//! in whether the period names an end time at all — each one stored and read back through
//! `GetScheduledReport`, where a bound the request did not name reads back as the instant
//! its own member's documented default resolves to
//! (`tests/all/schedule_period_defaults.rs` owns that rule's table).
//!
//! The ceiling is 3 CALENDAR years, which is what the test computes for itself from the
//! documented rule rather than asking the emulator: a span of 365-day years would place the
//! bound up to a day early.

use std::time::SystemTime;

use chrono::{DateTime, Months, SecondsFormat, Utc};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router, timestamp};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The documented ceiling, in calendar months: "3 years from the current time"
/// (`smithy_query(query="shape",shape="SchedulePeriod")`, member `endTime`).
const CEILING_MONTHS: u32 = 36;

/// A margin a row sits past (or short of) the ceiling by, in seconds. The emulator reads
/// its own clock when the request arrives, which is never before the instant this test
/// computed the ceiling at, so a row has to miss the bound by more than the handful of
/// milliseconds a call takes for its side of the bound to be the one under test.
const MARGIN: i64 = 60;

/// The epoch second of the published ceiling right now: [`CEILING_MONTHS`] calendar months
/// on from this instant.
fn ceiling_secs() -> i64 {
    let now = DateTime::<Utc>::from(SystemTime::now());
    now.checked_add_months(Months::new(CEILING_MONTHS))
        .unwrap_or_else(|| panic!("3 calendar years on from {now} is a date"))
        .timestamp()
}

/// How far apart two renderings of ONE instant may be for this test to call them the same
/// instant: the bounds travel as epoch seconds with a fractional part, so only the rounding
/// of that rendering is in play.
const INSTANT_TOLERANCE_SECONDS: f64 = 0.001;

/// [`CEILING_MONTHS`] calendar months on from the instant `secs` (epoch seconds) — the
/// instant `SchedulePeriod$endTime` both defaults to and is bounded by, for a write made at
/// `secs`.
fn ceiling_from(secs: f64) -> f64 {
    let whole = secs.floor();
    let instant = DateTime::<Utc>::from_timestamp(whole as i64, 0)
        .unwrap_or_else(|| panic!("{secs} is an instant"));
    let moved = instant
        .checked_add_months(Months::new(CEILING_MONTHS))
        .unwrap_or_else(|| panic!("3 calendar years on from {instant} is a date"));
    moved.timestamp() as f64 + (secs - whole)
}

/// Assert the stored period of `record`: a bound the submitted period named is stored as
/// submitted, and a bound it did not name is the default its own member documents at the
/// write — the start at the instant of the write, the end [`CEILING_MONTHS`] calendar
/// months on — measured from the instant the record itself reports in `written_at_member`
/// (`createdAt` for a create, `updatedAt` for an update). The defaults have their own table
/// in `tests/all/schedule_period_defaults.rs`; here they only have to leave every accepted
/// period readable back as two concrete instants.
fn assert_stored_period(
    label: &str,
    record: &Value,
    submitted: &Value,
    written_at_member: &str,
) {
    let written_at = timestamp(record, written_at_member, label);
    let period = record
        .get("scheduleConfig")
        .and_then(|config| config.get("schedulePeriod"))
        .unwrap_or_else(|| {
            panic!("case {label}: the stored scheduleConfig carries no schedulePeriod: {record}")
        });
    for (bound, default) in [
        ("startTime", written_at),
        ("endTime", ceiling_from(written_at)),
    ] {
        let actual = period
            .get(bound)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {label}: schedulePeriod.{bound} is absent or not an instant: {period}")
            });
        match submitted.get(bound).and_then(Value::as_f64) {
            // A bound the request named is stored as submitted, to the instant: nothing
            // recomputes it, so nothing may perturb it either.
            Some(submitted) => assert_eq!(
                actual, submitted,
                "case {label}: schedulePeriod.{bound} must be the submitted {submitted}, got \
                 {actual}"
            ),
            // A bound the request left out is the default its member documents, computed
            // here from the write's own instant — within the rounding of the epoch-seconds
            // rendering both sides travel as.
            None => assert!(
                (actual - default).abs() <= INSTANT_TOLERANCE_SECONDS,
                "case {label}: schedulePeriod.{bound} must be the default its member \
                 documents for the write at {written_at}, {default}, got {actual}"
            ),
        }
    }
}

/// This instant, as an epoch second — the form a `GenericTimeStamp` takes on the
/// `awsJson1_0` wire. Shared with the sibling suite that tables the rule on the period's
/// OTHER bound (`tests/all/schedule_period_start_time.rs`), so one spelling serves both.
pub(crate) fn now_secs() -> i64 {
    DateTime::<Utc>::from(SystemTime::now()).timestamp()
}

/// How a refusal names the instant `secs`: the RFC 3339 instant it spells. An independent
/// oracle — written out here rather than imported from the service, and shared with the
/// sibling suite, so the two cannot drift on how an instant is spelled.
pub(crate) fn instant_text(secs: i64) -> String {
    DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| panic!("{secs} is an instant"))
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The whole message a refusal of the end time `secs` submitted at the member path `path`
/// must carry: the member that broke the rule, the instant it carried, and the rule itself.
fn refusal_message(path: &str, secs: i64) -> String {
    format!(
        "{path}.schedulePeriod.endTime: '{}' is beyond the maximum allowed value; the end \
         time of a schedule period may be at most 3 years from the time of the request",
        instant_text(secs)
    )
}

/// A `schedulePeriod`: an end time, and a start time only where the row names one.
fn period(start: Option<i64>, end: Option<i64>) -> Value {
    let mut period = json!({});
    if let Some(start) = start {
        period["startTime"] = json!(start);
    }
    if let Some(end) = end {
        period["endTime"] = json!(end);
    }
    period
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
    let ceiling = ceiling_secs();
    // How far past the ceiling each row reaches, by absolute end time. The last two are
    // values no window from any clock admits: the year 2286 and the last second of 9999.
    let ends: Vec<(&str, i64)> = vec![
        ("a minute past the ceiling", ceiling + MARGIN),
        ("a day past the ceiling", ceiling + 86_400),
        ("a year past the ceiling", ceiling + 365 * 86_400),
        ("centuries past the ceiling", 9_999_999_999),
        ("the last second of year 9999", 253_402_300_799),
    ];

    let mut cases = Vec::new();
    for (index, (label, end)) in ends.iter().copied().enumerate() {
        // The update path: every other member of the request is one the handler would have
        // accepted, so the period is what the refusal is about.
        let mut update = json!({
            "arn": report_arn,
            "scheduleConfig": {
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "schedulePeriod": period(None, Some(end))
            }
        });
        if index % 2 == 0 {
            // Some rows name a start time as well, and some carry other updatable members:
            // neither changes which member broke the rule.
            update["scheduleConfig"]["schedulePeriod"]["startTime"] = json!(now_secs());
            update["name"] = json!("Period Renamed");
        }
        cases.push(RefusalCase {
            label: format!("UpdateScheduledReport, {label}"),
            operation: "UpdateScheduledReport",
            request: update,
            message: refusal_message("scheduleConfig", end),
        });

        let mut schedule = json!({
            "scheduleExpression": "rate(7 days)",
            "schedulePeriod": period(None, Some(end))
        });
        if index % 2 == 1 {
            schedule["schedulePeriod"]["startTime"] = json!(now_secs());
            schedule["scheduleExpressionTimeZone"] = json!("Europe/Berlin");
        }
        cases.push(RefusalCase {
            label: format!("CreateScheduledReport, {label}"),
            operation: "CreateScheduledReport",
            request: json!({
                "scheduledReport": {
                    "name": format!("Period Create-{index}"),
                    "dashboardArn": dashboard_arn,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": schedule
                }
            }),
            message: refusal_message("scheduledReport.scheduleConfig", end),
        });
    }
    cases
}

/// Create a report on `dashboard_arn` whose schedule carries `schedule_period` (where the
/// row names one), and return its ARN.
async fn report_with_period(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    schedule_period: Option<Value>,
    label: &str,
) -> String {
    let mut schedule = json!({"scheduleExpression": "cron(0 8 1 * ? *)"});
    if let Some(schedule_period) = schedule_period {
        schedule["schedulePeriod"] = schedule_period;
    }
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": name,
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": schedule
            }
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// The stored record of the report `arn`, as `GetScheduledReport` answers it. Shared with
/// the sibling suite that tables the rule on the period's OTHER bound
/// (`tests/all/schedule_period_start_time.rs`).
pub(crate) async fn stored_record(
    router: &MultiServiceRouter,
    arn: &str,
    label: &str,
) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .unwrap_or_else(|| panic!("case {label}: no scheduledReport in {fetched}"))
        .clone()
}

/// Every row reaching past the ceiling is refused with HTTP 400 `ValidationException`,
/// carrying the message the broken rule states.
#[tokio::test]
async fn an_end_time_past_the_published_ceiling_is_refused() {
    let router = router();
    let (dashboard, _widget) = source_dashboard(&router, "PeriodCeilingSource").await;
    let report = report_with_period(&router, &dashboard, "Period Report", None, "setup").await;

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
    }
}

/// A refused write persists nothing: the refused update leaves the whole stored record as
/// it was, and the refused creates store no report at all.
#[tokio::test]
async fn a_refused_schedule_period_changes_no_state() {
    let router = router();
    let (dashboard, _widget) = source_dashboard(&router, "PeriodUnchangedSource").await;
    let inside = ceiling_secs() - MARGIN;
    let report = report_with_period(
        &router,
        &dashboard,
        "Period Keeper",
        Some(period(Some(now_secs()), Some(inside))),
        "setup",
    )
    .await;
    let before = ok_call(&router, "GetScheduledReport", &json!({"arn": report})).await;

    for case in refusal_cases(&report, &dashboard) {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the row must be refused for this test to mean anything, got \
             {status}: {body}",
            case.label
        );
        let after = ok_call(&router, "GetScheduledReport", &json!({"arn": report})).await;
        assert_eq!(
            after, before,
            "case {:?}: the whole stored record must be untouched, got {after} after \
             {before}",
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

/// One row of the acceptance table: a period inside the window, built from this instant.
struct AcceptedPeriod {
    label: &'static str,
    schedule_period: Value,
}

fn accepted_periods() -> Vec<AcceptedPeriod> {
    let ceiling = ceiling_secs();
    let now = now_secs();
    vec![
        AcceptedPeriod {
            label: "the published ceiling itself, which is the maximum ALLOWED value",
            schedule_period: period(Some(now), Some(ceiling)),
        },
        AcceptedPeriod {
            label: "a minute inside the ceiling",
            schedule_period: period(None, Some(ceiling - MARGIN)),
        },
        AcceptedPeriod {
            label: "a day from now",
            schedule_period: period(Some(now), Some(now + 86_400)),
        },
        AcceptedPeriod {
            // The start time sits inside the grace its own member states ("not more than 5
            // minutes before the time of the request"), and no documented rule refuses an
            // end time that has already passed.
            label: "a period that has already ended, which no documented rule refuses",
            schedule_period: period(Some(now - 240), Some(now - 120)),
        },
        AcceptedPeriod {
            label: "a period that names a start time and no end time",
            schedule_period: period(Some(now), None),
        },
        AcceptedPeriod {
            label: "an empty period, which names neither bound",
            schedule_period: period(None, None),
        },
    ]
}

/// A period inside the window is stored and reads back verbatim, on the create path and on
/// the update path alike — with every bound the request did not name materialised to the
/// instant its own member's documented default resolves to at that write.
#[tokio::test]
async fn an_end_time_within_the_ceiling_round_trips() {
    let router = router();
    let (dashboard, _widget) = source_dashboard(&router, "PeriodAcceptedSource").await;

    for (index, case) in accepted_periods().into_iter().enumerate() {
        let label = case.label;
        let arn = report_with_period(
            &router,
            &dashboard,
            &format!("Period Accepted-{index}"),
            Some(case.schedule_period.clone()),
            label,
        )
        .await;
        let created = stored_record(&router, &arn, label).await;
        assert_stored_period(label, &created, &case.schedule_period, "createdAt");
        assert_eq!(
            created["scheduleConfig"]
                .get("scheduleExpression")
                .and_then(Value::as_str),
            Some("cron(0 8 1 * ? *)"),
            "case {label}: the accepted create must have stored its schedule, got {created}"
        );

        // The same period through the update path, onto a report created without one.
        let other = report_with_period(
            &router,
            &dashboard,
            &format!("Period Updated-{index}"),
            None,
            label,
        )
        .await;
        ok_call(
            &router,
            "UpdateScheduledReport",
            &json!({
                "arn": other,
                "scheduleConfig": {
                    "scheduleExpression": "rate(1 day)",
                    "schedulePeriod": case.schedule_period
                }
            }),
        )
        .await;
        let updated = stored_record(&router, &other, label).await;
        assert_stored_period(label, &updated, &case.schedule_period, "updatedAt");
        assert_eq!(
            updated["scheduleConfig"]
                .get("scheduleExpression")
                .and_then(Value::as_str),
            Some("rate(1 day)"),
            "case {label}: the accepted update must have stored its schedule, got {updated}"
        );
    }
}
