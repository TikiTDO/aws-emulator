// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `BatchGetServiceLevelObjectiveBudgetReport`
//! (POST `/budget-report`).
//!
//! The rule under test: a budget report is derived from the STORED SLO and the instant the
//! report is asked for — the error budget of a period-based SLO is the share of its
//! evaluation interval that its attainment goal leaves unattained, the interval being the
//! rolling span ending at that instant or the calendar tile containing it — and an id that
//! names no stored SLO earns a per-item `Errors` entry rather than discarding the reports
//! of the other ids the request carries.
//!
//! Every rule is asserted over a TABLE spanning the class: the rows differ along the
//! interval kind, the duration unit and count, the attainment goal, the indicator the SLO
//! carries and where the requested instant falls relative to the SLO and the interval.
//!
//! Every test drives the real `restJson1` surface through the router that production
//! mounts, through the one harness (`router`, `call`, `create`, …) `slo_lifecycle` already
//! drives this service's surface with.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config, ACCOUNT, REGION};

/// The `ErrorCode` an id that names no stored SLO earns: the HTTP status of the failure
/// class, as a string, since the caller is what got the item wrong.
pub(crate) const CALLER_ERROR_CODE: &str = "400";

/// The modelled `ServiceLevelObjectiveBudgetStatus` values this emulator can report
/// (`smithy_query(shape="ServiceLevelObjectiveBudgetStatus")` -> `["OK", "WARNING",
/// "BREACHED", "INSUFFICIENT_DATA"]`): `OK` for an evaluated window with its budget intact,
/// `INSUFFICIENT_DATA` for a window nothing was evaluated in.
const STATUS_OK: &str = "OK";
/// [`STATUS_OK`], for a report over a window nothing was evaluated in.
const STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Epoch seconds of fixed instants the rows ask for a report at, each `T00:00:00Z`.
const JAN_01_2020: i64 = 1_577_836_800;
/// 2020-01-15, a mid-month calendar anchor.
const JAN_15_2020: i64 = 1_579_046_400;
/// 2020-01-31, a calendar anchor on a day-of-month February does not have.
const JAN_31_2020: i64 = 1_580_428_800;
/// 2020-02-20, inside February of a leap year (29 days).
const FEB_20_2020: i64 = 1_582_156_800;
/// 2021-01-01.
const JAN_01_2021: i64 = 1_609_459_200;
/// 2021-02-20, inside February of a common year (28 days).
const FEB_20_2021: i64 = 1_613_779_200;
/// 2021-05-10, inside the April–July quarter of 2021 (91 days).
const MAY_10_2021: i64 = 1_620_604_800;
/// 2020-09-13, an instant before any SLO this suite creates existed.
const SEP_13_2020: i64 = 1_600_000_000;
/// 2030-01-01, after every instant this suite asks for a report at.
const JAN_01_2030: i64 = 1_893_456_000;

/// Seconds in a day, the unit the expected budgets below are stated in.
const DAY_SECONDS: i64 = 86_400;

/// The ARN of the SLO `name`, in the region every request of this suite is signed for.
fn slo_arn(name: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}")
}

/// Epoch seconds now, the instant a freshly created SLO is created at.
pub(crate) fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("epoch seconds fit an i64")
}

/// One `BatchGetServiceLevelObjectiveBudgetReport` call. `Timestamp` is an epoch-seconds
/// `Timestamp` on `restJson1`.
pub(crate) async fn budget_report(
    router: &MultiServiceRouter,
    slo_ids: &[String],
    timestamp: i64,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/budget-report",
        Some(json!({ "SloIds": slo_ids, "Timestamp": timestamp })),
    )
    .await
}

/// The report of the SLO called `name` in a response, or `Value::Null` when the response
/// carries none.
fn report_of(response: &Value, name: &str) -> Value {
    response["Reports"]
        .as_array()
        .and_then(|reports| {
            reports
                .iter()
                .find(|report| report["Name"].as_str() == Some(name))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// A rolling `Goal.Interval`.
fn rolling(duration_unit: &str, duration: i64) -> Value {
    json!({ "RollingInterval": { "DurationUnit": duration_unit, "Duration": duration } })
}

/// A calendar `Goal.Interval`, anchored at an epoch-seconds `StartTime`.
fn calendar(duration_unit: &str, duration: i64, start_time: i64) -> Value {
    json!({
        "CalendarInterval": {
            "StartTime": start_time,
            "DurationUnit": duration_unit,
            "Duration": duration
        }
    })
}

/// A request-based `RequestBasedSliConfig` on an Application Signals service.
fn request_based_sli_config() -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
            "MetricType": "AVAILABILITY"
        }
    })
}

/// Where a row asks for its report.
#[derive(Debug, Clone, Copy)]
enum At {
    /// Just after the SLO was created, so the evaluated window is non-empty.
    AfterCreation,
    /// A fixed instant, independent of when the test runs.
    Fixed(i64),
}

/// One row of the budget-report table.
#[derive(Debug)]
struct Case {
    /// The SLO name, which is also the case label.
    name: &'static str,
    /// The `Goal` the SLO is created with, or `None` to send none at all — the documented
    /// defaults then apply (a rolling interval of 7 days, an attainment goal of 99).
    goal: Option<Value>,
    /// Whether the SLO carries the period-based indicator (`SliConfig`) rather than the
    /// request-based one.
    period_based: bool,
    /// The instant the report is asked for.
    at: At,
    /// The `TotalBudgetSeconds` the report must state, or `None` for a report that states
    /// none at all.
    total_budget_seconds: Option<i64>,
    /// The `BudgetStatus` the report must state.
    budget_status: &'static str,
    /// The `Attainment` the report must state, or `None` for a report that states none.
    attainment: Option<f64>,
}

/// A report states the stored SLO's error budget for the interval the requested instant
/// falls in, and `INSUFFICIENT_DATA` for a window the SLO was not evaluated over.
///
/// The rows span the class: the default goal and an explicit one, a rolling interval in
/// each duration unit and several counts, attainment goals from 95 to 100, a calendar
/// interval in days and in months (including a leap February, a mid-month anchor, an anchor
/// on a day February does not have, a multi-month tile and a tile that has not begun), both
/// indicator kinds, and an instant before the SLO existed.
#[tokio::test]
async fn budget_report_states_the_error_budget_of_the_requested_interval() {
    let cases = vec![
        // No `Goal` at all: the documented defaults are a rolling interval of 7 days and an
        // attainment goal of 99, so the budget is 1% of 7 days.
        Case {
            name: "default-goal",
            goal: None,
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(7 * DAY_SECONDS / 100),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        // 1% of a minute is 0.6 seconds, which the integer member states as the nearest
        // whole second it can come to.
        Case {
            name: "rolling-one-minute",
            goal: Some(json!({ "Interval": rolling("MINUTE", 1), "AttainmentGoal": 99.0 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(1),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        Case {
            name: "rolling-thirty-minutes",
            goal: Some(json!({ "Interval": rolling("MINUTE", 30), "AttainmentGoal": 99.0 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(18),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        Case {
            name: "rolling-twelve-hours",
            goal: Some(json!({ "Interval": rolling("HOUR", 12), "AttainmentGoal": 99.9 })),
            period_based: true,
            at: At::AfterCreation,
            // 0.1% of 12 hours is 43.2 seconds, to the nearest second 43.
            total_budget_seconds: Some(43),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        Case {
            name: "rolling-seven-days",
            goal: Some(json!({ "Interval": rolling("DAY", 7), "AttainmentGoal": 99.95 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(302),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        // The example this operation's own doc states: a 99.95% monthly SLO has an error
        // budget of about 21.9 minutes of downtime in a 30-day month.
        Case {
            name: "rolling-one-month",
            goal: Some(json!({ "Interval": rolling("MONTH", 1), "AttainmentGoal": 99.95 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(30 * DAY_SECONDS * 5 / 10_000),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        Case {
            name: "rolling-four-weeks",
            goal: Some(json!({ "Interval": rolling("DAY", 28), "AttainmentGoal": 95.0 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(28 * DAY_SECONDS * 5 / 100),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        // An attainment goal of 100% tolerates no unhealthy time at all, so the budget is
        // zero seconds — and nothing has been spent of it, so it is not breached.
        Case {
            name: "rolling-perfect-goal",
            goal: Some(json!({ "Interval": rolling("DAY", 7), "AttainmentGoal": 100.0 })),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: Some(0),
            budget_status: STATUS_OK,
            attainment: Some(100.0),
        },
        // A request-based SLO's budget is a number of REQUESTS and its attainment a
        // quotient of request counts; no request has been observed, so there is no count to
        // take a share of and the status is the one for missing attainment data.
        Case {
            name: "request-based",
            goal: Some(json!({ "Interval": rolling("DAY", 7), "AttainmentGoal": 99.0 })),
            period_based: false,
            at: At::AfterCreation,
            total_budget_seconds: None,
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // An instant before the SLO was created was never evaluated for it, so there is no
        // attainment — but the interval's total budget is still a fact about the goal.
        Case {
            name: "before-the-slo-existed",
            goal: Some(json!({ "Interval": rolling("DAY", 7), "AttainmentGoal": 99.0 })),
            period_based: true,
            at: At::Fixed(SEP_13_2020),
            total_budget_seconds: Some(7 * DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // A calendar interval of months is stepped by the calendar: the tile containing the
        // instant is the month it falls in, which is 29 days long in a leap February.
        Case {
            name: "calendar-leap-february",
            goal: Some(
                json!({ "Interval": calendar("MONTH", 1, JAN_01_2020), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::Fixed(FEB_20_2020),
            total_budget_seconds: Some(29 * DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // A mid-month anchor tiles from its own day of the month: the tile containing
        // 2021-02-20 runs 2021-02-15 to 2021-03-15, which is 28 days.
        Case {
            name: "calendar-mid-month-anchor",
            goal: Some(
                json!({ "Interval": calendar("MONTH", 1, JAN_15_2020), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::Fixed(FEB_20_2021),
            total_budget_seconds: Some(28 * DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // An anchor on the 31st: the tile ends on the last day the next month has, so the
        // one containing 2020-02-20 runs 2020-01-31 to 2020-02-29 — 29 days.
        Case {
            name: "calendar-clamped-anchor",
            goal: Some(
                json!({ "Interval": calendar("MONTH", 1, JAN_31_2020), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::Fixed(FEB_20_2020),
            total_budget_seconds: Some(29 * DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // A three-month tile: the one containing 2021-05-10 runs 2021-04-01 to 2021-07-01,
        // which is 91 days.
        Case {
            name: "calendar-quarter",
            goal: Some(
                json!({ "Interval": calendar("MONTH", 3, JAN_01_2021), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::Fixed(MAY_10_2021),
            total_budget_seconds: Some(91 * DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        Case {
            name: "calendar-one-day",
            goal: Some(
                json!({ "Interval": calendar("DAY", 1, JAN_01_2021), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::Fixed(MAY_10_2021),
            total_budget_seconds: Some(DAY_SECONDS / 100),
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
        // A calendar interval whose first tile has not begun: no interval has been
        // evaluated, so there is no budget to report at all.
        Case {
            name: "calendar-not-yet-begun",
            goal: Some(
                json!({ "Interval": calendar("MONTH", 1, JAN_01_2030), "AttainmentGoal": 99.0 }),
            ),
            period_based: true,
            at: At::AfterCreation,
            total_budget_seconds: None,
            budget_status: STATUS_INSUFFICIENT_DATA,
            attainment: None,
        },
    ];

    let router = router();
    for case in &cases {
        let mut body = json!({ "Name": case.name });
        if case.period_based {
            body["SliConfig"] = sli_config(case.name, "GET /orders", 250.0);
        } else {
            body["RequestBasedSliConfig"] = request_based_sli_config();
        }
        if let Some(goal) = case.goal.clone() {
            body["Goal"] = goal;
        }
        let (status, created) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}",
            case.name
        );

        let timestamp = match case.at {
            // A minute past the create, so the evaluated window — the part of the interval
            // that followed the create and has already elapsed — is not empty.
            At::AfterCreation => now_seconds() + 60,
            At::Fixed(instant) => instant,
        };
        let (status, response) =
            budget_report(&router, &[case.name.to_string()], timestamp).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from BatchGetServiceLevelObjectiveBudgetReport, got {status} with {response}",
            case.name
        );
        assert_eq!(
            response["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: a stored SLO earns no error entry, got {:?}",
            case.name,
            response["Errors"]
        );
        assert_eq!(
            response["Timestamp"].as_f64(),
            Some(timestamp as f64),
            "case {:?}: expected the report to be for {timestamp}, got {:?}",
            case.name,
            response["Timestamp"]
        );

        let report = report_of(&response, case.name);
        let expected_arn = slo_arn(case.name);
        assert_eq!(
            report["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.name,
            report["Arn"]
        );
        assert_eq!(
            report["BudgetStatus"].as_str(),
            Some(case.budget_status),
            "case {:?}: expected BudgetStatus {:?}, got {:?}",
            case.name,
            case.budget_status,
            report["BudgetStatus"]
        );
        assert_eq!(
            report["Attainment"].as_f64(),
            case.attainment,
            "case {:?}: expected Attainment {:?}, got {:?}",
            case.name,
            case.attainment,
            report["Attainment"]
        );
        assert_eq!(
            report["TotalBudgetSeconds"].as_i64(),
            case.total_budget_seconds,
            "case {:?}: expected TotalBudgetSeconds {:?}, got {:?}",
            case.name,
            case.total_budget_seconds,
            report["TotalBudgetSeconds"]
        );
        // Nothing has been spent of an evaluated budget, and a window nothing was evaluated
        // over has no remaining budget to state.
        let expected_remaining = case
            .total_budget_seconds
            .filter(|_| case.budget_status == STATUS_OK);
        assert_eq!(
            report["BudgetSecondsRemaining"].as_i64(),
            expected_remaining,
            "case {:?}: expected BudgetSecondsRemaining {expected_remaining:?}, got {:?}",
            case.name,
            report["BudgetSecondsRemaining"]
        );
        // The request counts of a request-based SLO are not observed, and a period-based
        // SLO has no such member at all.
        for member in ["TotalBudgetRequests", "BudgetRequestsRemaining"] {
            assert!(
                report[member].is_null(),
                "case {:?}: expected no {member}, got {:?}",
                case.name,
                report[member]
            );
        }
        // The report states the indicator and goal of the record it is OF.
        let expected_evaluation_type = if case.period_based {
            "PeriodBased"
        } else {
            "RequestBased"
        };
        assert_eq!(
            report["EvaluationType"].as_str(),
            Some(expected_evaluation_type),
            "case {:?}: expected EvaluationType {expected_evaluation_type:?}, got {:?}",
            case.name,
            report["EvaluationType"]
        );
        assert_eq!(
            report["Goal"], created["Slo"]["Goal"],
            "case {:?}: expected the stored Goal {:?}, got {:?}",
            case.name, created["Slo"]["Goal"], report["Goal"]
        );
        assert_eq!(
            report["Sli"], created["Slo"]["Sli"],
            "case {:?}: expected the stored Sli {:?}, got {:?}",
            case.name, created["Slo"]["Sli"], report["Sli"]
        );
        assert_eq!(
            report["RequestBasedSli"], created["Slo"]["RequestBasedSli"],
            "case {:?}: expected the stored RequestBasedSli {:?}, got {:?}",
            case.name, created["Slo"]["RequestBasedSli"], report["RequestBasedSli"]
        );
    }
}

/// A calendar interval whose tiles reach the present reports the budget of the tile the
/// request falls in, with its budget intact.
///
/// The length of the current calendar month depends on when the test runs, so the assertion
/// is on the BOUNDS a month admits (28 to 31 days) rather than on one month's length —
/// which is the point the fixed-instant rows above cannot make: the tiling advances from a
/// 2020 anchor to today.
#[tokio::test]
async fn calendar_interval_reaching_the_present_reports_the_current_tile() {
    let name = "calendar-current-month";
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "Name": name,
            "SliConfig": sli_config("orders", "GET /orders", 250.0),
            "Goal": { "Interval": calendar("MONTH", 1, JAN_01_2020), "AttainmentGoal": 99.0 }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}"
    );

    let (status, response) = budget_report(&router, &[name.to_string()], now_seconds() + 60).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from BatchGetServiceLevelObjectiveBudgetReport, got {status} with {response}"
    );
    let report = report_of(&response, name);
    assert_eq!(
        report["BudgetStatus"].as_str(),
        Some(STATUS_OK),
        "expected BudgetStatus {STATUS_OK:?} for the current tile, got {:?}",
        report["BudgetStatus"]
    );
    let total = report["TotalBudgetSeconds"].as_i64().unwrap_or_default();
    let shortest = 28 * DAY_SECONDS / 100;
    let longest = 31 * DAY_SECONDS / 100;
    assert!(
        (shortest..=longest).contains(&total),
        "expected a monthly budget between {shortest} and {longest} seconds, got {total} in {report}"
    );
    assert_eq!(
        report["BudgetSecondsRemaining"].as_i64(),
        Some(total),
        "expected the whole budget remaining, got {:?}",
        report["BudgetSecondsRemaining"]
    );
}

/// An id that names no stored SLO earns one `Errors` entry identifying it by the `Arn` it was
/// asked about — and by nothing else — for every id form the modelled
/// `ServiceLevelObjectiveId` admits.
///
/// The rows differ along the id's form and the identity it names: a bare name, this
/// account's ARN of the same name, an ARN of another Region and an ARN of another account —
/// the last two being well-formed ids that name no SLO this store holds.
#[tokio::test]
async fn budget_report_identifies_each_id_that_names_no_slo() {
    struct Case {
        label: &'static str,
        id: String,
        arn: String,
    }

    let cases = [
        Case {
            label: "a bare name",
            id: "no-such-slo".to_string(),
            // No record to read an ARN off, so the ARN is the one the name would carry in
            // the region the request is signed for.
            arn: slo_arn("no-such-slo"),
        },
        Case {
            label: "an ARN of this account",
            id: slo_arn("absent-by-arn"),
            arn: slo_arn("absent-by-arn"),
        },
        Case {
            label: "an ARN of another Region",
            id: "arn:aws:application-signals:us-west-2:000000000000:slo/absent-elsewhere"
                .to_string(),
            arn: "arn:aws:application-signals:us-west-2:000000000000:slo/absent-elsewhere"
                .to_string(),
        },
        Case {
            label: "an ARN of another account",
            id: "arn:aws:application-signals:eu-west-2:999999999999:slo/absent-other-account"
                .to_string(),
            arn: "arn:aws:application-signals:eu-west-2:999999999999:slo/absent-other-account"
                .to_string(),
        },
    ];

    let router = router();
    let ids: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();
    let (status, response) = budget_report(&router, &ids, now_seconds()).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 even when every id names no SLO, got {status} with {response}"
    );
    assert_eq!(
        response["Reports"].as_array().map(Vec::len),
        Some(0),
        "expected no reports, got {:?}",
        response["Reports"]
    );

    let errors = response["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        cases.len(),
        "expected one error per id, got {:?}",
        response["Errors"]
    );
    for (case, error) in cases.iter().zip(errors.iter()) {
        assert!(
            error["Name"].is_null(),
            "case {:?}: expected no Name member on an entry for an id that names nothing, \
             got {:?}",
            case.label,
            error["Name"]
        );
        assert_eq!(
            error["Arn"].as_str(),
            Some(case.arn.as_str()),
            "case {:?}: expected Arn {:?}, got {:?}",
            case.label,
            case.arn,
            error["Arn"]
        );
        assert_eq!(
            error["ErrorCode"].as_str(),
            Some(CALLER_ERROR_CODE),
            "case {:?}: expected ErrorCode {CALLER_ERROR_CODE:?}, got {:?}",
            case.label,
            error["ErrorCode"]
        );
        let message = error["ErrorMessage"].as_str().unwrap_or_default();
        assert!(
            message.contains(&case.arn),
            "case {:?}: expected the message to name the ARN {:?}, got {message:?}",
            case.label,
            case.arn
        );
    }
}

/// One request reports every stored SLO it names and errs on every id it does not, and the
/// same SLO named twice is reported once.
#[tokio::test]
async fn budget_report_resolves_each_id_independently() {
    let router = router();
    for name in ["alpha-slo", "beta-slo"] {
        let (status, created) = create(
            &router,
            json!({
                "Name": name,
                "SliConfig": sli_config(name, "GET /orders", 250.0),
                "Goal": { "Interval": rolling("DAY", 7), "AttainmentGoal": 99.0 }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 creating {name}, got {status} with {created}"
        );
    }

    // `alpha-slo` is named twice — once by name and once by ARN, the two forms the modelled
    // id admits — and both name one record. The id that names NOTHING comes first, so an
    // unresolvable id can be seen not to displace a resolvable one.
    let ids = vec![
        "arn:aws:application-signals:us-west-2:000000000000:slo/ghost-elsewhere".to_string(),
        "alpha-slo".to_string(),
        slo_arn("alpha-slo"),
        "beta-slo".to_string(),
        "ghost-slo".to_string(),
    ];
    let (status, response) = budget_report(&router, &ids, now_seconds() + 60).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from a mixed batch, got {status} with {response}"
    );

    let mut reported: Vec<String> = response["Reports"]
        .as_array()
        .map(|reports| {
            reports
                .iter()
                .map(|report| report["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    assert_eq!(
        reported,
        vec!["alpha-slo".to_string(), "beta-slo".to_string()],
        "expected one report per stored SLO named, got {:?}",
        response["Reports"]
    );

    let errors = response["Errors"].as_array().cloned().unwrap_or_default();
    // An entry names its item by the `Arn` alone, so the ids that erred are read off that
    // member rather than off a name no entry carries.
    let mut erred: Vec<String> = errors
        .iter()
        .map(|error| error["Arn"].as_str().unwrap_or_default().to_string())
        .collect();
    erred.sort();
    assert_eq!(
        erred,
        vec![
            slo_arn("ghost-slo"),
            "arn:aws:application-signals:us-west-2:000000000000:slo/ghost-elsewhere".to_string(),
        ],
        "expected exactly the ids naming no SLO to err, got {:?}",
        response["Errors"]
    );
    // One id failing leaves the reports of the others in place.
    assert_eq!(
        report_of(&response, "beta-slo")["BudgetStatus"].as_str(),
        Some(STATUS_OK),
        "expected beta-slo still reported, got {:?}",
        response["Reports"]
    );
}

/// An `SloIds` entry that is no `ServiceLevelObjectiveId` is refused with the modelled
/// HTTP 400 `ValidationException`, and every form the pattern admits is served.
///
/// `SloIds` targets a list of plain `String`, so these entries reach the handler; the rows
/// span the ways an id can fail the pattern — too short, a character the name class
/// excludes, a leading or trailing character that is not alphanumeric, too long, an ARN of
/// another partition, of another service, or of another resource type.
#[tokio::test]
async fn budget_report_refuses_an_id_that_is_no_slo_id() {
    let too_long = format!("a{}z", "b".repeat(127));
    let refused: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("one character", "a".to_string()),
        ("a character the name class excludes", "bad!name".to_string()),
        ("a colon, which no name carries", "has:colon".to_string()),
        ("a leading separator", "-leading".to_string()),
        ("a trailing separator", "trailing-".to_string()),
        ("longer than the pattern admits", too_long),
        (
            "an ARN of another partition",
            "arn:aws-cn:application-signals:eu-west-2:000000000000:slo/elsewhere".to_string(),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::slo/not-ours".to_string(),
        ),
        (
            "an ARN of another resource type",
            "arn:aws:application-signals:eu-west-2:000000000000:instrumentationConfig/x".to_string(),
        ),
        (
            "an SLO ARN with no name",
            "arn:aws:application-signals:eu-west-2:000000000000:slo/".to_string(),
        ),
    ];

    let router = router();
    for (label, id) in &refused {
        let (status, error) =
            budget_report(&router, ::std::slice::from_ref(id), now_seconds()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for the id {id:?}, got {status} with {error}"
        );
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("Value at 'sloIds' failed to satisfy constraint"),
            "case {label:?}: expected the refusal to name the member, got {message:?}"
        );
    }

    // Every form the pattern DOES admit is served, whether or not it names a stored SLO.
    let served = vec![
        ("the shortest name", "ab".to_string()),
        ("a name with every separator", "a-b.c_d e9".to_string()),
        ("the longest name", format!("a{}z", "b".repeat(126))),
        ("an ARN of this account", slo_arn("by-arn")),
        (
            "an ARN of the other partition the pattern admits",
            "arn:aws-us-gov:application-signals:us-gov-west-1:000000000000:slo/govcloud"
                .to_string(),
        ),
    ];
    for (label, id) in &served {
        let (status, response) =
            budget_report(&router, ::std::slice::from_ref(id), now_seconds()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for the id {id:?}, got {status} with {response}"
        );
    }
}

/// Reporting a budget changes nothing a later caller sees: two reports of one SLO for one
/// instant are identical, and the SLO reads back unchanged afterwards.
#[tokio::test]
async fn budget_report_is_a_read() {
    let name = "read-only-slo";
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "Name": name,
            "SliConfig": sli_config(name, "GET /orders", 250.0),
            "Goal": { "Interval": rolling("DAY", 7), "AttainmentGoal": 99.9 }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}"
    );

    let at = now_seconds() + 60;
    let (_, first) = budget_report(&router, &[name.to_string()], at).await;
    let (_, second) = budget_report(&router, &[slo_arn(name)], at).await;
    assert_eq!(
        report_of(&first, name),
        report_of(&second, name),
        "two reports of one SLO for one instant must be identical, got {first} then {second}"
    );

    let (status, fetched) = call(
        &router,
        "GET",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetServiceLevelObjective, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["Slo"], created["Slo"],
        "a budget report must leave the record exactly as it was, got {:?} after {:?}",
        fetched["Slo"], created["Slo"]
    );
}
