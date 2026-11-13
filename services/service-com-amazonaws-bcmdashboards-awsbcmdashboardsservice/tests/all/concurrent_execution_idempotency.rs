// SPDX-License-Identifier: CC0-1.0

//! `ExecuteScheduledReport` under calls that are IN FLIGHT AT THE SAME TIME, end to end
//! over the wire.
//!
//! The RULE under test: an idempotency token identifies the request that carried it, so
//! the whole set of calls presenting one token for one report produces exactly ONE
//! execution — one notification on the account's default event bus and one answer every
//! caller is given — no matter how those calls overlap in time, while calls that present
//! no token, or a token each, are separate requests and each execute. Source:
//! `smithy_query(query="doc",shape="ExecuteScheduledReport")` — "If a clientToken is
//! provided, the service uses it for idempotency. Requests with the same client token
//! will not trigger a new execution within the same minute" — together with
//! `smithy_query(query="doc",shape="ExecuteScheduledReportRequest",member="clientToken")`,
//! "A unique, case-sensitive identifier that you provide to ensure the idempotency of the
//! request".
//!
//! Overlapping calls are what distinguishes this suite from the sequential retries the
//! sibling suites assert: an execution reaches the event bus with the store unlocked, so
//! only a token taken before that delivery suppresses a duplicate that is already
//! running.
//!
//! Every assertion spans the class rather than one request: the tables below differ in
//! how many callers race (two, three, five, eight), in the token's form (a short value,
//! the 128-character maximum, one made of the punctuation its pattern admits), in the
//! report's widget-id count (absent, empty, one) and in whether the report's resources
//! still resolve.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, emulator, timestamp};
use super::scheduled_report_execution::{
    create_report, create_report_naming_a_foreign_widget, stored_report, HEALTHY, UNHEALTHY,
};
use super::scheduled_report_execution_events::{
    published, Published, DEFAULT_BUS, EVENT_SOURCE, EXECUTION_DETAIL_TYPE,
};
use super::scheduled_report_lifecycle::source_dashboard;

/// How many times a racing task re-schedules itself while waiting for its peers before it
/// gives up waiting and calls anyway. A bound, so a task can never spin forever: the
/// assertions below hold whether or not the calls really overlapped, and the wait only
/// makes the overlap likely.
const MAX_RENDEZVOUS_SPINS: usize = 10_000;

/// Which widgets the report of a row names.
#[derive(Clone, Copy)]
enum Widgets {
    /// `widgetIds` left out of the create request.
    Absent,
    /// An empty `widgetIds` list.
    Empty,
    /// The one widget the source dashboard really holds.
    Real,
    /// A widget id the dashboard does not hold, so nothing resolves — reached by
    /// re-pointing the report, since a create has to name a widget of the dashboard it
    /// names (`create_report_naming_a_foreign_widget`).
    Missing,
}

/// The 128-character maximum `ClientToken` admits (length 1..128,
/// `smithy_query(query="shape",shape="ClientToken")`).
fn max_length_token() -> String {
    let token = format!("race-{}", "7".repeat(123));
    assert_eq!(token.len(), 128, "the maximum-length token is 128 chars");
    token
}

/// Issue `requests` as calls that are in flight at the same time, and answer what each
/// one was told, in the order the requests were given.
///
/// Every task reaches a rendezvous before it dispatches — it announces itself and then
/// re-schedules until all of its peers have announced themselves too — so the calls are
/// handed to the router together rather than one after another.
async fn raced_calls(
    router: &Arc<MultiServiceRouter>,
    requests: Vec<Value>,
) -> Vec<(u16, Value)> {
    let callers = requests.len();
    let ready = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::with_capacity(callers);
    for request in requests {
        let router = Arc::clone(router);
        let ready = Arc::clone(&ready);
        tasks.push(tokio::spawn(async move {
            ready.fetch_add(1, Ordering::SeqCst);
            let mut spins = 0;
            while ready.load(Ordering::SeqCst) < callers && spins < MAX_RENDEZVOUS_SPINS {
                spins += 1;
                tokio::task::yield_now().await;
            }
            call(&router, "ExecuteScheduledReport", &request).await
        }));
    }

    let mut answers = Vec::with_capacity(callers);
    for task in tasks {
        answers.push(task.await.expect("the racing call ran to completion"));
    }
    answers
}

/// One row of the shared-token table.
struct RaceCase {
    label: &'static str,
    name: &'static str,
    /// `None` leaves `scheduleConfig.state` out of the create request.
    schedule_state: Option<&'static str>,
    widgets: Widgets,
    token: String,
    callers: usize,
    /// Whether the report's resources resolve, so an execution is triggered at all.
    triggered: bool,
}

fn race_cases() -> Vec<RaceCase> {
    vec![
        RaceCase {
            label: "two callers, state omitted, no widgetIds",
            name: "Raced Pair",
            schedule_state: None,
            widgets: Widgets::Absent,
            token: "raced-token-0001".to_string(),
            callers: 2,
            triggered: true,
        },
        RaceCase {
            label: "three callers, one real widget, maximum-length token",
            name: "Raced Trio",
            schedule_state: Some("ENABLED"),
            widgets: Widgets::Real,
            token: max_length_token(),
            callers: 3,
            triggered: true,
        },
        RaceCase {
            // Every character here is in the printable range `ClientToken`'s pattern
            // `^[\u{21}-\u{7E}]+$` admits.
            label: "five callers, empty widgetIds, punctuation token",
            name: "Raced Five",
            schedule_state: Some("ENABLED"),
            widgets: Widgets::Empty,
            token: "!#$%&'()*+,-./:;<=>?@[]^_`{|}~".to_string(),
            callers: 5,
            triggered: true,
        },
        RaceCase {
            label: "eight callers, state omitted, no widgetIds",
            name: "Raced Eight",
            schedule_state: None,
            widgets: Widgets::Absent,
            token: "raced-token-0008".to_string(),
            callers: 8,
            triggered: true,
        },
        RaceCase {
            label: "two callers, a widget id that does not resolve",
            name: "Raced Unresolved",
            schedule_state: None,
            widgets: Widgets::Missing,
            token: "raced-unresolved-0001".to_string(),
            callers: 2,
            triggered: false,
        },
    ]
}

/// The report a row executes: created on `dashboard_arn`, carrying the widget ids the row
/// asks for.
async fn report_of(
    router: &MultiServiceRouter,
    dashboard_arn: &str,
    name: &str,
    schedule_state: Option<&str>,
    widgets: Widgets,
    real: &str,
) -> String {
    let widget_ids = match widgets {
        Widgets::Absent => None,
        Widgets::Empty => Some(Vec::new()),
        Widgets::Real => Some(vec![real.to_string()]),
        Widgets::Missing => {
            return create_report_naming_a_foreign_widget(
                router,
                dashboard_arn,
                name,
                schedule_state,
            )
            .await
        }
    };
    create_report(router, dashboard_arn, name, schedule_state, widget_ids).await
}

/// Several calls presenting ONE client token for one report, all in flight at the same
/// time, produce exactly one execution: one notification on the account's default event
/// bus, one execution instant on the record, and the one outcome every caller is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_calls_under_one_token_execute_the_report_once() {
    for case in race_cases() {
        let label = case.label;
        // A fresh emulator per row, so one row's delivery cannot be read as another's.
        let (router, engine) = emulator();
        let router = Arc::new(router);
        let (dashboard_arn, widget_id) = source_dashboard(&router, "RaceSource").await;
        let arn = report_of(
            &router,
            &dashboard_arn,
            case.name,
            case.schedule_state,
            case.widgets,
            &widget_id,
        )
        .await;

        let request = json!({"arn": arn, "clientToken": case.token});
        let answers = raced_calls(&router, vec![request; case.callers]).await;

        let expected_code = if case.triggered { HEALTHY } else { UNHEALTHY };
        for (index, (status, body)) in answers.iter().enumerate() {
            assert_eq!(
                *status, 200,
                "case {label}: caller {index} expected 200, got {status}: {body}"
            );
            assert_eq!(
                body.get("executionTriggered").and_then(Value::as_bool),
                Some(case.triggered),
                "case {label}: caller {index} expected executionTriggered {}, got {body}",
                case.triggered
            );
            assert_eq!(
                body.pointer("/healthStatus/statusCode").and_then(Value::as_str),
                Some(expected_code),
                "case {label}: caller {index} expected healthStatus.statusCode \
                 {expected_code:?}, got {body}"
            );
        }

        // One request, one outcome: every caller is told what the single execution
        // established, down to the instant its health was refreshed — which is what a
        // caller whose own call was suppressed is entitled to.
        let (_, first) = answers
            .first()
            .unwrap_or_else(|| panic!("case {label}: no answer at all"));
        for (index, (_, body)) in answers.iter().enumerate() {
            assert_eq!(
                body, first,
                "case {label}: caller {index} was told a different outcome, got {body} \
                 against {first}"
            );
        }

        // The bus a client reads the execution on: one notification for the one
        // execution, and none at all when nothing was triggered.
        let delivered: Vec<Published> = published(&engine);
        let expected_notifications = usize::from(case.triggered);
        assert_eq!(
            delivered.len(),
            expected_notifications,
            "case {label}: expected {expected_notifications} notification(s) for one \
             idempotent request, got {delivered:?}"
        );

        let stored = stored_report(&router, &arn).await;
        assert_eq!(
            stored.get("lastExecutionAt").is_some(),
            case.triggered,
            "case {label}: lastExecutionAt is written only by a triggered execution: \
             {stored}"
        );

        if let Some(event) = delivered.first() {
            assert_eq!(
                event.bus, DEFAULT_BUS,
                "case {label}: expected the notification on the {DEFAULT_BUS} bus, got \
                 {event:?}"
            );
            assert_eq!(
                event.source, EVENT_SOURCE,
                "case {label}: expected source {EVENT_SOURCE}, got {event:?}"
            );
            assert_eq!(
                event.detail_type, EXECUTION_DETAIL_TYPE,
                "case {label}: expected detail-type {EXECUTION_DETAIL_TYPE}, got {event:?}"
            );
            assert_eq!(
                event.resources,
                vec![arn.clone(), dashboard_arn.clone()],
                "case {label}: the notification names the report and its dashboard, got \
                 {event:?}"
            );
            // The one notification is the one execution the record reports.
            let last_execution_at = timestamp(&stored, "lastExecutionAt", label);
            assert!(
                (event.time - last_execution_at).abs() < 1.0,
                "case {label}: the notification's time {} is not the execution instant \
                 {last_execution_at}",
                event.time
            );
        }

        // And a call arriving after the race, under the same token, is still the same
        // request: it replays that one outcome and adds no notification.
        let (status, retried) = call(
            &router,
            "ExecuteScheduledReport",
            &json!({"arn": arn, "clientToken": case.token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: the retry after the race expected 200, got {status}: {retried}"
        );
        assert_eq!(
            &retried, first,
            "case {label}: the retry after the race replays the one outcome, got {retried}"
        );
        assert_eq!(
            published(&engine).len(),
            expected_notifications,
            "case {label}: the retry after the race delivers nothing new, got {:?}",
            published(&engine)
        );
    }
}

/// How the racing callers of a row present their `clientToken`.
#[derive(Clone, Copy)]
enum Tokens {
    /// Each caller presents a token of its own.
    OnePerCaller,
    /// No caller presents a token at all.
    None,
}

/// Racing calls that are NOT the same request each execute the report: a token suppresses
/// a repeat of the request that carried it, so callers presenting a token each — or none
/// at all — are separate executions and each delivers its own notification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_calls_that_share_no_token_each_execute() {
    // (label, how many callers race, how they present a token, the report's widgets)
    let cases: [(&str, usize, Tokens, Widgets); 4] = [
        ("two callers, a token each", 2, Tokens::OnePerCaller, Widgets::Absent),
        (
            "three callers, a token each, one real widget",
            3,
            Tokens::OnePerCaller,
            Widgets::Real,
        ),
        ("two callers, no token", 2, Tokens::None, Widgets::Empty),
        ("four callers, no token", 4, Tokens::None, Widgets::Absent),
    ];

    for (label, callers, tokens, widgets) in cases {
        let (router, engine) = emulator();
        let router = Arc::new(router);
        let (dashboard_arn, widget_id) = source_dashboard(&router, "UnsharedSource").await;
        let arn = report_of(
            &router,
            &dashboard_arn,
            "Unshared Token Report",
            None,
            widgets,
            &widget_id,
        )
        .await;

        let requests: Vec<Value> = (0..callers)
            .map(|caller| match tokens {
                Tokens::OnePerCaller => {
                    json!({"arn": arn, "clientToken": format!("unshared-{caller}")})
                }
                Tokens::None => json!({"arn": arn}),
            })
            .collect();
        let answers = raced_calls(&router, requests).await;

        for (index, (status, body)) in answers.iter().enumerate() {
            assert_eq!(
                *status, 200,
                "case {label}: caller {index} expected 200, got {status}: {body}"
            );
            assert_eq!(
                body.get("executionTriggered").and_then(Value::as_bool),
                Some(true),
                "case {label}: caller {index} is its own request and executes, got {body}"
            );
            assert_eq!(
                body.pointer("/healthStatus/statusCode").and_then(Value::as_str),
                Some(HEALTHY),
                "case {label}: caller {index} expected {HEALTHY}, got {body}"
            );
        }

        let delivered = published(&engine);
        assert_eq!(
            delivered.len(),
            callers,
            "case {label}: expected {callers} notification(s), one per request, got \
             {delivered:?}"
        );
        for event in &delivered {
            assert_eq!(
                event.resources,
                vec![arn.clone(), dashboard_arn.clone()],
                "case {label}: every notification names the report and its dashboard, got \
                 {event:?}"
            );
        }
    }
}
