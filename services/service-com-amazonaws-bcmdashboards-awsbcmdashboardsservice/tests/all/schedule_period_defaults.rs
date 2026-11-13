// SPDX-License-Identifier: CC0-1.0

//! The `schedulePeriod` a stored scheduled report reads back with, end to end over the
//! wire.
//!
//! The rule under test: a member whose own documentation states a service-side default is
//! resolved by the write that stores it, so a read answers the value the service chose
//! rather than nothing at all. Both bounds of `SchedulePeriod` state one — "The start time
//! of the schedule period. If not specified, defaults to the time of the create or update
//! request" and "The end time of the schedule period. If not specified, defaults to 3
//! years from the time of the create or update request"
//! (`smithy_query(query="shape",shape="SchedulePeriod")`) — and the sentence is written for
//! the create request and the update request alike, so every stored schedule carries a
//! period with both bounds filled: the ones the request named, and for the rest the
//! instants those sentences name.
//!
//! The table spans what the rule generalises over: which bounds the request names (neither,
//! only the start, only the end, both), the FORM the period takes (absent altogether, an
//! empty structure, a half-filled one, a full one), the instants named (today, days ahead,
//! well inside the three-year window) and the write path that stores it (the create and the
//! update). The recorded case, a create whose `scheduleConfig` carries no `schedulePeriod`
//! at all, is the first row.
//!
//! The request's own instant is read from the record it wrote — `createdAt` for a create,
//! `updatedAt` for an update — so a resolved bound is asserted against the instant of the
//! call that resolved it rather than against a second reading of the test's clock.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{ok_call, router, timestamp};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// One day in seconds, the step the rows below are built from.
const DAY: f64 = 86_400.0;

/// The shortest and the longest span of three calendar years, in days: three 365-day years
/// plus the leap days the window can contain (none or one). The end-time default is "3
/// years from the time of the create or update request", and a year is a calendar year, so
/// the resolved end lands inside this window wherever in the calendar the request falls.
const THREE_YEARS_MIN_DAYS: f64 = 1095.0;
const THREE_YEARS_MAX_DAYS: f64 = 1096.0;

/// How far either side of an instant two renderings of it may differ and still be the same
/// instant: the bounds travel as epoch seconds with a fractional part, and the test
/// compares one such number against another.
const INSTANT_TOLERANCE_SECONDS: f64 = 0.001;

/// The same tolerance, in days, for the SPAN between two instants: a span is the difference
/// of two large epoch seconds divided by a day, so its last bits are the float arithmetic's
/// rather than the service's — a span landing exactly on a day bound reads a few hundred
/// nanoseconds past it. The day bounds below are compared with this slack so the check is
/// about the instant the service chose and not about the representation of the number
/// carrying it.
const SPAN_TOLERANCE_DAYS: f64 = INSTANT_TOLERANCE_SECONDS / DAY;

/// Epoch seconds now, the form `SchedulePeriod`'s bounds take on the wire.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
        .trunc()
}

/// What a row expects of one bound of the stored period.
#[derive(Debug, Clone, Copy)]
enum Bound {
    /// The request named this instant, so the stored bound is it.
    AsSubmitted(f64),
    /// The request named no instant, so the stored bound is the time of the request.
    DefaultsToTheRequest,
    /// The request named no instant, so the stored bound is 3 years from the request.
    DefaultsToThreeYearsOn,
}

/// One row: the `schedulePeriod` a write submits (or none at all), and what each bound of
/// the stored period must then be.
struct PeriodCase {
    label: &'static str,
    /// `None` for a request whose `scheduleConfig` carries no `schedulePeriod` member.
    submitted: Option<Value>,
    start: Bound,
    end: Bound,
}

fn cases() -> Vec<PeriodCase> {
    let now = epoch_now();
    vec![
        PeriodCase {
            label: "no schedulePeriod at all",
            submitted: None,
            start: Bound::DefaultsToTheRequest,
            end: Bound::DefaultsToThreeYearsOn,
        },
        PeriodCase {
            label: "an empty schedulePeriod, naming neither bound",
            submitted: Some(json!({})),
            start: Bound::DefaultsToTheRequest,
            end: Bound::DefaultsToThreeYearsOn,
        },
        PeriodCase {
            label: "a start time only, today",
            submitted: Some(json!({"startTime": now})),
            start: Bound::AsSubmitted(now),
            end: Bound::DefaultsToThreeYearsOn,
        },
        PeriodCase {
            label: "a start time only, a week ahead",
            submitted: Some(json!({"startTime": now + 7.0 * DAY})),
            start: Bound::AsSubmitted(now + 7.0 * DAY),
            end: Bound::DefaultsToThreeYearsOn,
        },
        PeriodCase {
            label: "an end time only, a day ahead",
            submitted: Some(json!({"endTime": now + DAY})),
            start: Bound::DefaultsToTheRequest,
            end: Bound::AsSubmitted(now + DAY),
        },
        PeriodCase {
            label: "an end time only, well inside the three-year window",
            submitted: Some(json!({"endTime": now + 1000.0 * DAY})),
            start: Bound::DefaultsToTheRequest,
            end: Bound::AsSubmitted(now + 1000.0 * DAY),
        },
        PeriodCase {
            label: "both bounds named",
            submitted: Some(json!({"startTime": now + DAY, "endTime": now + 30.0 * DAY})),
            start: Bound::AsSubmitted(now + DAY),
            end: Bound::AsSubmitted(now + 30.0 * DAY),
        },
    ]
}

/// The `scheduleConfig` a row submits: the expression every row shares, plus the period
/// the row names, if any.
fn schedule_config(case: &PeriodCase, expression: &str) -> Value {
    let mut config = json!({"scheduleExpression": expression});
    if let Some(period) = &case.submitted {
        config["schedulePeriod"] = period.clone();
    }
    config
}

/// The stored record of `arn`, as `GetScheduledReport` answers it.
async fn stored_report(router: &MultiServiceRouter, arn: &str, label: &str) -> Value {
    let fetched = ok_call(router, "GetScheduledReport", &json!({"arn": arn})).await;
    fetched
        .get("scheduledReport")
        .unwrap_or_else(|| panic!("case {label}: no scheduledReport in {fetched}"))
        .clone()
}

/// Assert the stored period of `record`: both bounds present, and each one the instant the
/// row says — the submitted value, or the default its member documents, measured from
/// `requested_at`, the instant of the call that wrote the record.
fn assert_period(label: &str, record: &Value, case: &PeriodCase, requested_at: f64) {
    let period = record
        .get("scheduleConfig")
        .and_then(|config| config.get("schedulePeriod"))
        .unwrap_or_else(|| {
            panic!("case {label}: the stored scheduleConfig carries no schedulePeriod: {record}")
        })
        .clone();

    for (member, expected) in [("startTime", case.start), ("endTime", case.end)] {
        let actual = period
            .get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {label}: schedulePeriod.{member} is absent or not an instant: {period}")
            });
        match expected {
            Bound::AsSubmitted(submitted) => assert!(
                (actual - submitted).abs() <= INSTANT_TOLERANCE_SECONDS,
                "case {label}: schedulePeriod.{member} must be the submitted {submitted}, got {actual}"
            ),
            Bound::DefaultsToTheRequest => assert!(
                (actual - requested_at).abs() <= INSTANT_TOLERANCE_SECONDS,
                "case {label}: schedulePeriod.{member} must default to the instant of the \
                 request, {requested_at}, got {actual}"
            ),
            Bound::DefaultsToThreeYearsOn => {
                let days = (actual - requested_at) / DAY;
                let window = (THREE_YEARS_MIN_DAYS - SPAN_TOLERANCE_DAYS)
                    ..=(THREE_YEARS_MAX_DAYS + SPAN_TOLERANCE_DAYS);
                assert!(
                    window.contains(&days),
                    "case {label}: schedulePeriod.{member} must default to 3 years from the \
                     request ({THREE_YEARS_MIN_DAYS}..={THREE_YEARS_MAX_DAYS} days on from \
                     {requested_at}), got {actual}, which is {days} days on"
                );
            }
        }
    }
}

/// Every row, on the CREATE path: the create stores a period with both bounds filled, and
/// `GetScheduledReport` answers it — the bounds the request named verbatim, and the ones it
/// left unset resolved to the instants their members document, measured from the create.
#[tokio::test]
async fn a_create_stores_the_schedule_period_its_bounds_document() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "PeriodDefaultSource").await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Period Default-{index}"),
                    "dashboardArn": dashboard,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": schedule_config(&case, "cron(0 8 1 * ? *)"),
                    "widgetIds": [widget]
                }
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the create returns an arn: {created}"))
            .to_string();

        let record = stored_report(&router, &arn, label).await;
        let created_at = timestamp(&record, "createdAt", label);
        assert_period(label, &record, &case, created_at);

        // A read answers the stored value, so reading again answers the same period: the
        // defaults were resolved by the write, not by the reader.
        let again = stored_report(&router, &arn, label).await;
        assert_eq!(
            again, record,
            "case {label}: reading the report twice must answer the same record, got {again}"
        );
    }
}

/// Every row, on the UPDATE path: a submitted period is one whole parameter, so it replaces
/// the stored one with its own bounds resolved — the ones the update named verbatim, and the
/// ones it left unset measured from the update, not from the create.
#[tokio::test]
async fn an_update_resolves_the_bounds_of_the_period_it_submits() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "PeriodUpdateSource").await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        // The report starts with a period naming both bounds, so a row's update is visible
        // as a replacement of a period that was already there.
        let before = epoch_now();
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Period Updated-{index}"),
                    "dashboardArn": dashboard,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {
                        "scheduleExpression": "cron(0 8 1 * ? *)",
                        "schedulePeriod": {
                            "startTime": before + 2.0 * DAY,
                            "endTime": before + 60.0 * DAY
                        }
                    },
                    "widgetIds": [widget]
                }
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the create returns an arn: {created}"))
            .to_string();
        let created_period = stored_report(&router, &arn, label).await["scheduleConfig"]
            ["schedulePeriod"]
            .clone();

        let mut update = json!({"arn": arn});
        update["scheduleConfig"] = schedule_config(&case, "cron(0 12 L * ? *)");
        ok_call(&router, "UpdateScheduledReport", &update).await;

        let record = stored_report(&router, &arn, label).await;
        match &case.submitted {
            // A submitted config that names no period leaves the stored one alone: there
            // is nothing for this write to resolve, so the create's period stands.
            None => assert_eq!(
                record["scheduleConfig"]["schedulePeriod"], created_period,
                "case {label}: an update naming no period keeps the stored one, got {record}"
            ),
            Some(_) => {
                assert_period(label, &record, &case, timestamp(&record, "updatedAt", label))
            }
        }
    }
}

/// The period a write resolves is one the service's own rule admits: the end time it
/// defaults to is the ceiling `SchedulePeriod$endTime` states, which is the maximum ALLOWED
/// value — so a create that names no period is served, and the report it stores can then be
/// updated without the stored period being refused back at the caller.
#[tokio::test]
async fn a_resolved_period_is_a_period_the_service_admits() {
    let router = router();
    let (dashboard, widget) = source_dashboard(&router, "PeriodAdmittedSource").await;
    let label = "a create with no period, then an update of another member";

    let created = ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Period Admitted",
                "dashboardArn": dashboard,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                "widgetIds": [widget]
            }
        }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the create returns an arn: {created}"))
        .to_string();
    let stored = stored_report(&router, &arn, label).await;
    let period = stored["scheduleConfig"]["schedulePeriod"].clone();

    ok_call(
        &router,
        "UpdateScheduledReport",
        &json!({"arn": arn, "name": "Period Admitted Again"}),
    )
    .await;
    let after = stored_report(&router, &arn, label).await;
    assert_eq!(
        after["scheduleConfig"]["schedulePeriod"], period,
        "case {label}: an update that names no period keeps the resolved one, got {after}"
    );
}
