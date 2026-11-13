// SPDX-License-Identifier: CC0-1.0

//! The scope one `ExecuteScheduledReport` `clientToken` has, end to end over the wire.
//!
//! The rule under test: an idempotency token is a key for the REQUEST that carried it —
//! here the report the request names together with the token — so presenting one token
//! while sweeping over several scheduled reports executes each of them on its own
//! merits, and only a repeat of the same report-and-token pair inside the documented
//! minute is suppressed.
//!
//! Every assertion spans the class rather than one request: the table below differs in
//! how many reports share the token (two, three, five), in the token's own form (a short
//! value, the 128-character maximum, one made of the punctuation its pattern admits), in
//! each report's schedule state (absent, `ENABLED`, `DISABLED`) and in whether each
//! report's resources still resolve (so the sweep spans a triggered execution, an
//! untriggered one and a refusal).

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, ok_call, router, timestamp};
use super::scheduled_report_execution::{
    create_report, create_report_naming_a_foreign_widget, stored_report, HEALTHY, UNHEALTHY,
    WIDGET_ID_NOT_FOUND,
};
use super::scheduled_report_lifecycle::{error_code, source_dashboard};

/// Which widgets a report of the sweep names.
#[derive(Clone, Copy)]
enum Widgets {
    /// `widgetIds` left out of the create request.
    Absent,
    /// An empty `widgetIds` list.
    Empty,
    /// The one widget the source dashboard really holds.
    Real,
    /// A widget id the dashboard does not hold — reached by re-pointing the report, since
    /// a create has to name a widget of the dashboard it names
    /// (`create_report_naming_a_foreign_widget`).
    Missing,
}

/// What the execution request on one report of the sweep must answer.
#[derive(Clone, Copy, PartialEq)]
enum Expected {
    /// HTTP 200, HEALTHY, the execution triggered and persisted.
    Triggered,
    /// HTTP 200, UNHEALTHY, nothing triggered and no execution instant written.
    NotTriggered,
    /// HTTP 400 `ValidationException` — the report's schedule is DISABLED.
    Refused,
}

/// One report of a sweep.
struct ReportSpec {
    name: &'static str,
    /// `None` leaves `scheduleConfig.state` out of the create request.
    schedule_state: Option<&'static str>,
    widgets: Widgets,
    expected: Expected,
}

/// One row of the sweep table: a token, and the reports one client loops over with it.
struct SweepCase {
    label: &'static str,
    token: String,
    reports: Vec<ReportSpec>,
}

/// The 128-character maximum `ClientToken` admits (length 1..128,
/// `smithy_query(query="shape",shape="ClientToken")`).
fn max_length_token() -> String {
    let token = format!("sweep-{}", "9".repeat(122));
    assert_eq!(token.len(), 128, "the maximum-length token is 128 chars");
    token
}

fn sweep_cases() -> Vec<SweepCase> {
    vec![
        SweepCase {
            label: "two reports, short token",
            token: "sweep-1".to_string(),
            reports: vec![
                ReportSpec {
                    name: "Sweep Two First",
                    schedule_state: None,
                    widgets: Widgets::Absent,
                    expected: Expected::Triggered,
                },
                ReportSpec {
                    name: "Sweep Two Second",
                    schedule_state: Some("ENABLED"),
                    widgets: Widgets::Real,
                    expected: Expected::Triggered,
                },
            ],
        },
        SweepCase {
            label: "three reports, maximum-length token, one unhealthy",
            token: max_length_token(),
            reports: vec![
                ReportSpec {
                    name: "Sweep Three First",
                    schedule_state: Some("ENABLED"),
                    widgets: Widgets::Empty,
                    expected: Expected::Triggered,
                },
                ReportSpec {
                    name: "Sweep Three Unhealthy",
                    schedule_state: None,
                    widgets: Widgets::Missing,
                    expected: Expected::NotTriggered,
                },
                ReportSpec {
                    name: "Sweep Three Last",
                    schedule_state: None,
                    widgets: Widgets::Real,
                    expected: Expected::Triggered,
                },
            ],
        },
        SweepCase {
            // Every character here is in the printable range `ClientToken`'s pattern
            // `^[\u0021-\u007E]+$` admits.
            label: "five reports, punctuation token",
            token: "!#$%&'()*+,-./:;<=>?@[]^_`{|}~".to_string(),
            reports: (0..5)
                .map(|index| ReportSpec {
                    name: match index {
                        0 => "Sweep Five A",
                        1 => "Sweep Five B",
                        2 => "Sweep Five C",
                        3 => "Sweep Five D",
                        _ => "Sweep Five E",
                    },
                    schedule_state: if index % 2 == 0 {
                        Some("ENABLED")
                    } else {
                        None
                    },
                    widgets: if index % 2 == 0 {
                        Widgets::Absent
                    } else {
                        Widgets::Empty
                    },
                    expected: Expected::Triggered,
                })
                .collect(),
        },
        SweepCase {
            label: "a DISABLED report in the middle of the sweep",
            token: "sweep-with-a-paused-report".to_string(),
            reports: vec![
                ReportSpec {
                    name: "Sweep Paused First",
                    schedule_state: Some("ENABLED"),
                    widgets: Widgets::Real,
                    expected: Expected::Triggered,
                },
                ReportSpec {
                    name: "Sweep Paused Middle",
                    schedule_state: Some("DISABLED"),
                    widgets: Widgets::Absent,
                    expected: Expected::Refused,
                },
                ReportSpec {
                    name: "Sweep Paused Last",
                    schedule_state: None,
                    widgets: Widgets::Absent,
                    expected: Expected::Triggered,
                },
            ],
        },
    ]
}

/// Assert one execution answer against what the report's own state entitles it to, and
/// return the execution instant the call persisted (`None` when none was written).
async fn assert_outcome(
    router: &smithy_server_runtime::MultiServiceRouter,
    case_label: &str,
    report_name: &str,
    arn: &str,
    token: &str,
    expected: Expected,
) -> Option<f64> {
    let label = format!("{case_label} / {report_name}");
    let request = json!({"arn": arn, "clientToken": token});
    let (status, body) = call(router, "ExecuteScheduledReport", &request).await;

    if expected == Expected::Refused {
        assert_eq!(
            status, 400,
            "case {label}: a DISABLED report is refused with 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label}: expected ValidationException, got {body}"
        );
        let stored = stored_report(router, arn).await;
        assert!(
            stored.get("lastExecutionAt").is_none(),
            "case {label}: a refused execution writes nothing: {stored}"
        );
        return None;
    }

    assert_eq!(
        status, 200,
        "case {label}: a shared client token must not refuse this report, got {status}: {body}"
    );
    let triggered = expected == Expected::Triggered;
    assert_eq!(
        body.get("executionTriggered").and_then(Value::as_bool),
        Some(triggered),
        "case {label}: expected executionTriggered {triggered}, got {body}"
    );
    let expected_code = if triggered { HEALTHY } else { UNHEALTHY };
    assert_eq!(
        body["healthStatus"]["statusCode"].as_str(),
        Some(expected_code),
        "case {label}: expected healthStatus.statusCode {expected_code:?}, got {body}"
    );
    if !triggered {
        assert_eq!(
            body["healthStatus"]["statusReasons"],
            json!([WIDGET_ID_NOT_FOUND]),
            "case {label}: the unhealthy report names the widget it cannot resolve: {body}"
        );
    }

    let stored = stored_report(router, arn).await;
    assert_eq!(
        stored.get("lastExecutionAt").is_some(),
        triggered,
        "case {label}: lastExecutionAt is written only by a triggered execution: {stored}"
    );
    assert_eq!(
        stored["healthStatus"]["statusCode"].as_str(),
        Some(expected_code),
        "case {label}: the record keeps the health this execution refreshed: {stored}"
    );
    triggered.then(|| timestamp(&stored, "lastExecutionAt", &label))
}

/// One client token looped over several reports executes every one of them: the token
/// scopes to the request that carried it, so no report in the sweep is refused because
/// another one used the token first, and each answers its own health.
#[tokio::test]
async fn one_token_swept_over_several_reports_executes_each_of_them() {
    for case in sweep_cases() {
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "SweepSource").await;

        // Create every report of the sweep up front, as a client enumerating its reports
        // before looping over them would.
        let mut arns: Vec<String> = Vec::new();
        for report in &case.reports {
            let arn = match report.widgets {
                Widgets::Missing => {
                    create_report_naming_a_foreign_widget(
                        &router,
                        &dashboard_arn,
                        report.name,
                        report.schedule_state,
                    )
                    .await
                }
                widgets => {
                    let widget_ids = match widgets {
                        Widgets::Absent => None,
                        Widgets::Empty => Some(Vec::new()),
                        _ => Some(vec![widget_id.clone()]),
                    };
                    create_report(
                        &router,
                        &dashboard_arn,
                        report.name,
                        report.schedule_state,
                        widget_ids,
                    )
                    .await
                }
            };
            arns.push(arn);
        }

        // The sweep itself: one token, every report.
        let mut first_pass: Vec<Option<f64>> = Vec::new();
        for (report, arn) in case.reports.iter().zip(&arns) {
            first_pass.push(
                assert_outcome(
                    &router,
                    case.label,
                    report.name,
                    arn,
                    &case.token,
                    report.expected,
                )
                .await,
            );
        }

        // A second sweep under the same token is a retry of each of those requests: each
        // answers its own first outcome and triggers no second execution, so every
        // execution instant stays where the first pass put it.
        for ((report, arn), executed_at) in case.reports.iter().zip(&arns).zip(&first_pass) {
            let label = format!("{} / {} retry", case.label, report.name);
            let again = assert_outcome(
                &router,
                case.label,
                report.name,
                arn,
                &case.token,
                report.expected,
            )
            .await;
            assert_eq!(
                again, *executed_at,
                "case {label}: a retry of the same report under the same token must not trigger a second execution"
            );
        }

        // A fresh token is a fresh request for each report, so each executes again.
        for (report, arn) in case.reports.iter().zip(&arns) {
            if report.expected == Expected::Refused {
                continue;
            }
            let label = format!("{} / {} fresh token", case.label, report.name);
            let answered = ok_call(
                &router,
                "ExecuteScheduledReport",
                &json!({"arn": arn, "clientToken": format!("fresh-{}", report.name.len())}),
            )
            .await;
            assert_eq!(
                answered.get("executionTriggered").and_then(Value::as_bool),
                Some(report.expected == Expected::Triggered),
                "case {label}: a fresh token executes the report on its own merits, got {answered}"
            );
        }
    }
}

/// The idempotency key is the pair, so two reports swept under one token keep their own
/// outcomes: executing the second one does not replay the first one's answer, and the
/// first one's suppressed retry does not report the second one's health.
#[tokio::test]
async fn each_report_keeps_its_own_outcome_under_a_shared_token() {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "SharedTokenSource").await;
    let healthy = create_report(&router, &dashboard_arn, "Shared Healthy", None, None).await;
    let unhealthy = create_report_naming_a_foreign_widget(
        &router,
        &dashboard_arn,
        "Shared Unhealthy",
        None,
    )
    .await;
    let with_widget = create_report(
        &router,
        &dashboard_arn,
        "Shared With Widget",
        Some("ENABLED"),
        Some(vec![widget_id]),
    )
    .await;

    let token = "one-token-per-sweep";
    let mut answers: Vec<(String, Value)> = Vec::new();
    for (label, arn) in [
        ("healthy", &healthy),
        ("unhealthy", &unhealthy),
        ("with widget", &with_widget),
    ] {
        let answered = ok_call(
            &router,
            "ExecuteScheduledReport",
            &json!({"arn": arn, "clientToken": token}),
        )
        .await;
        assert_eq!(
            answered.get("executionTriggered").and_then(Value::as_bool),
            Some(label != "unhealthy"),
            "case {label}: the report decides on its own state, got {answered}"
        );
        answers.push((label.to_string(), answered));
    }

    // Each report's retry replays ITS OWN outcome, not the outcome of whichever report
    // presented the token first.
    for ((label, first), arn) in answers
        .iter()
        .zip([&healthy, &unhealthy, &with_widget].into_iter())
    {
        let retried = ok_call(
            &router,
            "ExecuteScheduledReport",
            &json!({"arn": arn, "clientToken": token}),
        )
        .await;
        assert_eq!(
            &retried, first,
            "case {label}: the retry must replay this report's own first outcome"
        );
    }

    // And the unhealthy report of the sweep is the only one carrying an unhealthy code.
    let unhealthy_stored = stored_report(&router, &unhealthy).await;
    assert_eq!(
        unhealthy_stored["healthStatus"]["statusCode"].as_str(),
        Some(UNHEALTHY),
        "the report whose widget does not resolve keeps its own health: {unhealthy_stored}"
    );
    for (label, arn) in [("healthy", &healthy), ("with widget", &with_widget)] {
        let stored = stored_report(&router, arn).await;
        assert_eq!(
            stored["healthStatus"]["statusCode"].as_str(),
            Some(HEALTHY),
            "case {label}: a report that resolves is healthy whatever its sweep mates are: {stored}"
        );
    }
}
