// SPDX-License-Identifier: CC0-1.0

//! What a REPORT of an execution hands over: the call that tells a client the execution
//! has ended has already published the event saying so.
//!
//! The RULE under test: **a transition owes the same writes whichever call materialised
//! it.** An execution's terminal status falls due after the call that started it has
//! answered, so it is written by whichever call settles the store first — and that is as
//! often one of the operations that REPORT an execution (`DescribeExecution`,
//! `ListExecutions`, `GetExecutionHistory`, each of which settles before it reads) as one
//! that moves it. A report that materialises a transition therefore hands over everything
//! that transition owes — the execution status change for the account's default
//! EventBridge bus, and what the execution publishes about itself — rather than leaving
//! the publication to the one background clock, because a client polling a describe until
//! it reports the end is the only caller there is.
//!
//! Why this needs a suite of its own rather than a row in
//! `execution_status_change_events.rs`: that suite reads its events off the rule's queue
//! while the clock is running, so an event it receives may have been published by either.
//! Here the clock is STARVED — the runtime the start spawned it on is dropped before the
//! transition comes due, which cancels it — so an event that arrives after the report was
//! handed over by the report itself. Nothing but the one report call runs between the
//! start and the read.
//!
//! The rows span what the rule generalises over: WHICH report materialises the
//! transition — the three operations of this service that report an execution's own
//! progress — each run against its own emulator, and each asserted to have published
//! BOTH the status the start wrote and the terminal status its own settle materialised.

use serde_json::{json, Value};

use crate::execution_status_change_events::{
    given_execution, given_machine, given_rule, read_once, Emulator, EVENT_SOURCE,
    EXECUTION_STATUS_CHANGE,
};
use crate::state_machine_lifecycle::PAST_THE_EXECUTION_DELAY;

/// A definition that succeeds immediately with a static result, so the only transitions
/// the run has are the start's `RUNNING` and the terminal `SUCCEEDED` its settle writes.
const SUCCEEDS: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// The statuses the run moves through, in the order it enters them.
const RUNNING: &str = "RUNNING";
const SUCCEEDED: &str = "SUCCEEDED";

/// One row: the report whose settle must materialise — and hand over — the terminal
/// status, as the operation's name and the request that makes it from the execution's ARN
/// and its state machine's.
struct Report {
    case: &'static str,
    operation: &'static str,
    request: fn(&str, &str) -> Value,
}

/// Every operation of this service that reports an execution's own progress.
fn reports() -> Vec<Report> {
    vec![
        Report {
            case: "DescribeExecution reports the execution",
            operation: "DescribeExecution",
            request: |execution, _machine| json!({ "executionArn": execution }),
        },
        Report {
            case: "ListExecutions lists the state machine's executions",
            operation: "ListExecutions",
            request: |_execution, machine| json!({ "stateMachineArn": machine }),
        },
        Report {
            case: "GetExecutionHistory reads the execution's history",
            operation: "GetExecutionHistory",
            request: |execution, _machine| json!({ "executionArn": execution }),
        },
    ]
}

/// One runtime that runs `work` to completion and is then DROPPED, which cancels every
/// task spawned on it — the in-process clock this service starts included.
fn on_a_runtime_that_is_then_dropped<F: std::future::Future>(work: F) -> F::Output {
    let runtime = ::tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build a current-thread runtime");
    runtime.block_on(work)
}

/// RULE: the report that materialises an execution's terminal status publishes that
/// status's event itself, so a client that only polls a report sees it.
#[test]
fn the_report_that_materialises_a_terminal_status_publishes_it() {
    for row in reports() {
        let case = row.case;
        let emulator = Emulator::new();

        // Phase 1: the rule's queue, the state machine and the start — and NOTHING else.
        // The runtime is dropped at the end of the phase, which cancels the clock the
        // start spawned, so no background task can publish what comes due next.
        let (url, machine, execution) = on_a_runtime_that_is_then_dropped(async {
            let url = given_rule(&emulator, "settled-by-a-report", &json!({ "source": [EVENT_SOURCE] })).await;
            let machine = given_machine(&emulator, "settled-by-a-report", SUCCEEDS, "STANDARD").await;
            let execution = given_execution(&emulator, &machine, "polled", "{}").await;
            // What the start itself published: the status it wrote, and nothing of the
            // outcome that is not due yet.
            let started = read_once(&emulator, &url, case).await;
            let statuses: Vec<&str> = started
                .iter()
                .map(|event| event["detail"]["status"].as_str().unwrap_or("<none>"))
                .collect();
            assert_eq!(
                statuses,
                vec![RUNNING],
                "case {case:?}: the start publishes the status it wrote and no outcome that \
                 is not due, got {started:?}"
            );
            (url, machine, execution)
        });

        // The outcome comes due while nothing at all is running: the clock was cancelled
        // with its runtime, so the transition is owed to whichever call settles next.
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        // Phase 2: ONE report, then ONE read of the queue. Whatever is in the queue was
        // published by the report.
        let (reported, delivered) = on_a_runtime_that_is_then_dropped(async {
            let reported = emulator
                .sfn(row.operation, &(row.request)(&execution, &machine))
                .await;
            let delivered = read_once(&emulator, &url, case).await;
            (reported, delivered)
        });

        // The report itself reports the terminal status, so the event must report it too:
        // a client told the execution has ended by this call must find the event saying so.
        let described = status_in(&reported, &execution);
        assert_eq!(
            described.as_deref(),
            Some(SUCCEEDED),
            "case {case:?}: expected the report to show the execution terminal, got {reported}"
        );

        let about: Vec<&Value> = delivered
            .iter()
            .filter(|event| event["detail"]["executionArn"].as_str() == Some(execution.as_str()))
            .collect();
        let statuses: Vec<&str> = about
            .iter()
            .map(|event| event["detail"]["status"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            statuses,
            vec![SUCCEEDED],
            "case {case:?}: the report that materialised the terminal status must have \
             published it, got {delivered:?}"
        );
        let event = about[0];
        assert_eq!(
            event["source"].as_str(),
            Some(EVENT_SOURCE),
            "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
        );
        assert_eq!(
            event["detail-type"].as_str(),
            Some(EXECUTION_STATUS_CHANGE),
            "case {case:?}: expected detail-type {EXECUTION_STATUS_CHANGE}, got {event}"
        );
        assert_eq!(
            event["resources"],
            json!([&execution]),
            "case {case:?}: expected the execution in resources, got {event}"
        );
        assert_eq!(
            event["detail"]["stateMachineArn"].as_str(),
            Some(machine.as_str()),
            "case {case:?}: expected the state machine in the detail, got {event}"
        );
        assert!(
            event["detail"]["stopDate"].is_i64(),
            "case {case:?}: a terminal execution reports when it stopped, got {event}"
        );
    }
}

/// The status `reported` gives for `execution_arn`, whichever shape the report has: a
/// describe answers the execution's members at the top level, a list answers a page of
/// items, and a history answers the events the run wrote — whose LAST one names the
/// outcome, so the status it reports is read off the event type.
fn status_in(reported: &Value, execution_arn: &str) -> Option<String> {
    if let Some(status) = reported["status"].as_str() {
        return Some(status.to_string());
    }
    if let Some(executions) = reported["executions"].as_array() {
        return executions
            .iter()
            .find(|item| item["executionArn"].as_str() == Some(execution_arn))
            .and_then(|item| item["status"].as_str())
            .map(str::to_string);
    }
    // `ExecutionSucceeded` is the event an execution that succeeded ends on
    // (`smithy_query(query="shape",shape="HistoryEventType")`), so a history whose last
    // event is that one reports the same outcome a describe does.
    reported["events"]
        .as_array()
        .and_then(|events| events.last())
        .and_then(|event| event["type"].as_str())
        .map(|kind| match kind {
            "ExecutionSucceeded" => SUCCEEDED.to_string(),
            other => other.to_string(),
        })
}

/// A report of an execution leaves nothing of its own in the queue: with the clock
/// starved, a SECOND report publishes nothing, because the status it reads has not moved
/// since the first one published it.
#[test]
fn a_second_report_publishes_nothing_because_nothing_moved() {
    let case = "a second report";
    let emulator = Emulator::new();
    let (url, execution) = on_a_runtime_that_is_then_dropped(async {
        let url = given_rule(&emulator, "settled-twice", &json!({ "source": [EVENT_SOURCE] })).await;
        let machine = given_machine(&emulator, "settled-twice", SUCCEEDS, "STANDARD").await;
        let execution = given_execution(&emulator, &machine, "polled-twice", "{}").await;
        (url, execution)
    });
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let delivered = on_a_runtime_that_is_then_dropped(async {
        let request = json!({ "executionArn": &execution });
        emulator.sfn("DescribeExecution", &request).await;
        let first = read_once(&emulator, &url, case).await;
        emulator.sfn("DescribeExecution", &request).await;
        let second = read_once(&emulator, &url, case).await;
        (first, second)
    });
    let (first, second) = delivered;
    let statuses: Vec<&str> = first
        .iter()
        .filter(|event| event["detail"]["executionArn"].as_str() == Some(execution.as_str()))
        .map(|event| event["detail"]["status"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        statuses,
        vec![RUNNING, SUCCEEDED],
        "case {case:?}: the first report publishes every status not published yet, got \
         {first:?}"
    );
    assert!(
        second.is_empty(),
        "case {case:?}: a status that has not moved owes no event, got {second:?}"
    );
}
