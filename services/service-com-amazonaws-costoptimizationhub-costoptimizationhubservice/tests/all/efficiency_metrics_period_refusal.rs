// SPDX-License-Identifier: CC0-1.0

//! The periods `ListEfficiencyMetrics` reports a series over, and the sentences
//! it refuses the others with, driven over the wire (`awsJson1_0`) through the
//! same multi-service router the emulator boots.
//!
//! The rules under test, each stated in a refusal that names the member in the
//! capitalization of the shape it targets (`TimePeriod`, not the wire spelling
//! `timePeriod`):
//!
//! * a bound is written in the form the requested granularity takes its bounds
//!   in — the member documents its dates as given "in either YYYY-MM-DD format
//!   or YYYY-MM format depending on the desired granularity";
//! * no bound names an interval later than the present one, because a series is
//!   reported over elapsed time;
//! * the end bound comes after the start bound, stated in the unit the requested
//!   granularity reports its data points in (a day for `Daily`, a month for
//!   `Monthly`);
//! * no bound reaches back before the window the granularity reports over (90
//!   days of a daily series, 3 months of a monthly one).
//!
//! Every sentence is fixed text: no submitted bound, and no other per-run value,
//! appears in it. Each table spans both granularities and the forms a period
//! takes, and every period is built from the clock rather than written as a fixed
//! date, because the window moves with the clock.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, daily_period, day_text, enrolled_router, month_text,
    monthly_period, months_after, months_before, ok, today,
};

/// The members a refusal about one bound of the period attributes itself to.
const START_MEMBER: &str = "timePeriod.start";
const END_MEMBER: &str = "timePeriod.end";

/// How the service names this input member in a message: the capitalization of
/// the shape the member targets (`smithy_query(query="shape",
/// shape="TimePeriod")`), not the wire spelling `timePeriod`.
const MEMBER_IN_MESSAGE: &str = "TimePeriod";

/// Each granularity with the unit it reports its data points in, the form it
/// takes its bounds in, and how far back it reports — the three things its
/// refusals are stated in terms of.
///
/// `GranularityType.DAILY` timestamps a point `YYYY-MM-DD` and
/// `GranularityType.MONTHLY` timestamps it `YYYY-MM`
/// (`smithy_query(query="shape", shape="GranularityType")`); the window and the
/// pattern spellings are the ones the service states in its own refusals.
struct Reported {
    granularity: &'static str,
    unit: &'static str,
    format: &'static str,
    aggregation: &'static str,
    window: usize,
    window_unit: &'static str,
}

const REPORTED: &[Reported] = &[
    Reported {
        granularity: "Daily",
        unit: "day",
        format: "yyyy-MM-dd",
        aggregation: "daily",
        window: 90,
        window_unit: "days",
    },
    Reported {
        granularity: "Monthly",
        unit: "month",
        format: "yyyy-MM",
        aggregation: "monthly",
        window: 3,
        window_unit: "months",
    },
];

/// A well-formed request for one granularity over one period.
fn request(granularity: &str, start: &str, end: &str) -> Value {
    json!({
        "granularity": granularity,
        "timePeriod": {"start": start, "end": end},
    })
}

/// The `message` of a refused call, with the status, the error class and the
/// member the refusal names checked first, so a wording assertion never reports
/// on another refusal.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    member: &str,
) -> String {
    let (status, payload) = call(router, "ListEfficiencyMetrics", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_stated_rule_refusal(&payload, case, member);
    assert!(
        payload["efficiencyMetricsByGroup"].is_null(),
        "case {case:?}: expected a refusal rather than metrics, got {payload}"
    );
    payload["message"].as_str().unwrap_or_default().to_string()
}

/// A period inside the reported window, at one granularity: the bounds a case
/// starts from before replacing the one it is about.
fn inside_window(granularity: &str) -> (String, String) {
    if granularity == "Daily" {
        daily_period(3, 2)
    } else {
        monthly_period(2, 1)
    }
}

/// A period whose end bound does not come after its start bound is refused with
/// the rule stated in the unit the requested granularity reports in.
#[tokio::test]
async fn an_empty_period_is_refused_in_the_unit_the_granularity_reports_in() {
    let router = enrolled_router().await;

    for reported in REPORTED {
        // (case, the bounds): each a period inside the reported window whose end
        // does not come after its start.
        let (start, end) = inside_window(reported.granularity);
        let cases: Vec<(&str, String, String)> = vec![
            ("both bounds the same interval", start.clone(), start.clone()),
            ("the period running backwards", end.clone(), start.clone()),
            (
                "the period running backwards over the whole window",
                if reported.granularity == "Daily" {
                    day_text(today())
                } else {
                    month_text(today())
                },
                if reported.granularity == "Daily" {
                    day_text(today() - 89)
                } else {
                    month_text(months_before(today(), 2))
                },
            ),
        ];

        for (period, start, end) in cases {
            let case = format!("{}, {period}", reported.granularity);
            let message = refusal_message(
                &router,
                &case,
                &request(reported.granularity, &start, &end),
                END_MEMBER,
            )
            .await;
            assert_eq!(
                message,
                format!(
                    "Invalid {MEMBER_IN_MESSAGE} specified. End {} must be after start {}.",
                    reported.unit, reported.unit
                ),
                "case {case:?}: expected the period rule stated in {}s, got {message:?}",
                reported.unit
            );
            // Fixed text: neither submitted bound is interpolated into it.
            for bound in [&start, &end] {
                assert!(
                    !message.contains(bound.as_str()),
                    "case {case:?}: expected no submitted bound in the refusal, \
                     got {message:?} carrying {bound}"
                );
            }
        }
    }
}

/// A bound that is not in the form the requested granularity takes its bounds in
/// is refused with the rule stating that granularity's pattern — the other
/// granularity's form included, because the form depends on the granularity
/// asked for.
#[tokio::test]
async fn a_bound_in_another_form_than_the_granularity_takes_is_refused() {
    let router = enrolled_router().await;

    for reported in REPORTED {
        let (start, end) = inside_window(reported.granularity);
        // Every form a bound can take that this granularity does not: the other
        // granularity's form, and text that is no date at all.
        let others: Vec<(String, String)> = vec![
            (
                "the other granularity's form".to_string(),
                if reported.granularity == "Daily" {
                    month_text(today())
                } else {
                    day_text(today())
                },
            ),
            ("no date at all".to_string(), "not-a-date".to_string()),
            ("a month the year has not".to_string(), "2026-13".to_string()),
            (
                "a day the month has not".to_string(),
                "2026-02-30".to_string(),
            ),
            ("a year on its own".to_string(), "2026".to_string()),
            ("the empty value".to_string(), String::new()),
            ("another date notation".to_string(), "03/01/2026".to_string()),
            (
                "a whole instant".to_string(),
                "2026-03-01T00:00:00Z".to_string(),
            ),
        ];

        for (form, value) in &others {
            // On either bound, so neither is read in a form the other sets.
            for (bound, member) in [("start", START_MEMBER), ("end", END_MEMBER)] {
                let case = format!("{}, {bound} as {form}", reported.granularity);
                let mut body = request(reported.granularity, &start, &end);
                body["timePeriod"][bound] = json!(value);
                let message = refusal_message(&router, &case, &body, member).await;
                assert_eq!(
                    message,
                    format!(
                        "Invalid {MEMBER_IN_MESSAGE} specified. Start and End must have format \
                         '{}' for {} granularity.",
                        reported.format, reported.aggregation
                    ),
                    "case {case:?}: expected the form rule for {} granularity, got {message:?}",
                    reported.aggregation
                );
            }
        }
    }
}

/// A bound naming an interval later than the present one is refused: a series is
/// reported over elapsed time, so nothing has been spent over an interval that
/// has not happened.
#[tokio::test]
async fn a_period_reaching_past_the_present_is_refused() {
    let router = enrolled_router().await;

    for reported in REPORTED {
        // (case, start, end, the bound the refusal names): each a period one
        // bound of which names an interval that has not elapsed. A future bound
        // is two intervals ahead, not one, because the clock is read here and
        // again in the handler: a bound one interval ahead would be the present
        // one if the UTC day rolled over between the two.
        let cases: Vec<(&str, String, String, &str)> = if reported.granularity == "Daily" {
            vec![
                (
                    "the end two days ahead",
                    day_text(today() - 1),
                    day_text(today() + 2),
                    END_MEMBER,
                ),
                (
                    "the whole period ahead",
                    day_text(today() + 2),
                    day_text(today() + 4),
                    START_MEMBER,
                ),
                (
                    "the end a year ahead",
                    day_text(today() - 1),
                    day_text(today() + 365),
                    END_MEMBER,
                ),
            ]
        } else {
            vec![
                (
                    "the end two months ahead",
                    month_text(months_before(today(), 1)),
                    month_text(months_after(today(), 2)),
                    END_MEMBER,
                ),
                (
                    "the whole period ahead",
                    month_text(months_after(today(), 2)),
                    month_text(months_after(today(), 4)),
                    START_MEMBER,
                ),
            ]
        };

        for (period, start, end, member) in cases {
            let case = format!("{}, {period}", reported.granularity);
            let message = refusal_message(
                &router,
                &case,
                &request(reported.granularity, &start, &end),
                member,
            )
            .await;
            assert_eq!(
                message,
                format!("Invalid {MEMBER_IN_MESSAGE} specified. Time period cannot be in the future."),
                "case {case:?}: expected the future rule, got {message:?}"
            );
        }
    }
}

/// A bound reaching back before the window the granularity reports over is
/// refused with that window stated in the granularity's own intervals — on the
/// first interval outside it, and on periods reaching much further back.
#[tokio::test]
async fn a_period_reaching_before_the_reported_window_is_refused() {
    let router = enrolled_router().await;

    for reported in REPORTED {
        // (case, start, end): the first bound outside the window, and two
        // periods well before it.
        let cases: Vec<(&str, String, String)> = if reported.granularity == "Daily" {
            vec![
                (
                    "one day before the window",
                    day_text(today() - 91),
                    day_text(today()),
                ),
                (
                    "a period a year back",
                    day_text(today() - 365),
                    day_text(today() - 364),
                ),
            ]
        } else {
            vec![
                (
                    "one month before the window",
                    month_text(months_before(today(), 4)),
                    month_text(today()),
                ),
                (
                    "a period a year back",
                    month_text(months_before(today(), 12)),
                    month_text(months_before(today(), 11)),
                ),
            ]
        };

        for (period, start, end) in cases {
            let case = format!("{}, {period}", reported.granularity);
            let message = refusal_message(
                &router,
                &case,
                &request(reported.granularity, &start, &end),
                START_MEMBER,
            )
            .await;
            assert_eq!(
                message,
                format!(
                    "Invalid {MEMBER_IN_MESSAGE} specified. {} granularity supports up to {} {}.",
                    reported.granularity, reported.window, reported.window_unit
                ),
                "case {case:?}: expected the window rule, got {message:?}"
            );
        }
    }
}

/// The complement: every period inside the window, at either granularity, is
/// reported rather than refused — the whole window, its last interval, and the
/// interval at its far edge.
#[tokio::test]
async fn every_period_inside_the_reported_window_is_served() {
    let router = enrolled_router().await;

    // (case, granularity, start, end)
    //
    // A served case sits one interval inside the far edge of the window: the
    // window is read from the clock twice — once here and once in the handler —
    // so a case ON the edge would flip if the UTC day rolled over between them.
    let mut served: Vec<(String, &str, String, String)> = Vec::new();
    for (case, (start, end)) in [
        ("nearly the whole window", daily_period(89, 89)),
        ("the last day", daily_period(1, 1)),
        ("the day at the far edge", daily_period(89, 1)),
        ("a fortnight", daily_period(20, 14)),
    ] {
        served.push((case.to_string(), "Daily", start, end));
    }
    for (case, (start, end)) in [
        ("nearly the whole window", monthly_period(2, 2)),
        ("the last whole month", monthly_period(1, 1)),
        ("the month at the far edge", monthly_period(2, 1)),
    ] {
        served.push((case.to_string(), "Monthly", start, end));
    }

    for (period, granularity, start, end) in &served {
        let case = format!("{granularity}, {period}");
        let answered = ok(
            &router,
            "ListEfficiencyMetrics",
            &request(granularity, start, end),
        )
        .await;
        let groups = answered["efficiencyMetricsByGroup"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected groups in {answered}"));
        assert!(
            !groups.is_empty(),
            "case {case:?}: expected the period to be reported, got {answered}"
        );
        let points = groups[0]["metricsByTime"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a series in {answered}"))
            .len();
        assert!(
            points > 0 && points <= reported_window(granularity),
            "case {case:?}: expected between 1 and {} points, got {points}",
            reported_window(granularity)
        );
    }
}

/// How many data points one series carries at most, which is the window stated
/// in that granularity's own intervals.
fn reported_window(granularity: &str) -> usize {
    REPORTED
        .iter()
        .find(|reported| reported.granularity == granularity)
        .map(|reported| reported.window)
        .unwrap_or_else(|| panic!("case {granularity}: expected a reported granularity"))
}
