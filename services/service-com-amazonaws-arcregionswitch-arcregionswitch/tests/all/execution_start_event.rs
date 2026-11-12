// SPDX-License-Identifier: CC0-1.0

//! The START event of a plan execution's timeline.
//!
//! THE RULE UNDER TEST: `ListPlanExecutionEvents` is "a detailed timeline" of an
//! execution, and `ExecutionEventType` models `executionStarted` — the event that says the
//! execution is being driven. An execution records it exactly ONCE, when it leaves
//! `pending`: after the `executionPending` event it was created with, before the first
//! event of any step it reaches and before the event that ends it, each event carrying its
//! own timestamp and chained to the one before it. So a client reconstructing the timeline
//! always has a start event to anchor it, and the span from start to finish is readable
//! from the stream.
//!
//! The table spans what the rule generalises over: what the workflow's first step is (a
//! manual approval the clock stops at, a block this emulator cannot perform, no step at
//! all), how many steps there are, and how the execution ends (approved to completion,
//! failed, completed with nothing to do).

use serde_json::{json, Value};

use crate::switch_support::{
    approval_step, events_of, lambda_step, plan_with_steps, poll_until_state, start_execution,
    Emulator,
};

/// The event type an execution is created with.
const PENDING: &str = "executionPending";

/// The event type under test.
const STARTED: &str = "executionStarted";

/// One row of the timeline table.
struct Row {
    case: &'static str,
    steps: fn() -> Vec<Value>,
    /// Whether the case approves the step the clock stops at.
    approve: bool,
    /// The states the execution has reached by the time the timeline is read.
    terminal: &'static [&'static str],
    /// The whole timeline, oldest first.
    events: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one manual approval, approved to completion",
        steps: || vec![approval_step("Approve")],
        approve: true,
        terminal: &["completed"],
        events: &[
            PENDING,
            STARTED,
            "stepPendingApproval",
            "stepSucceeded",
            "executionSucceeded",
        ],
    },
    Row {
        case: "two manual approvals, the first one approved",
        steps: || vec![approval_step("Approve"), approval_step("Confirm")],
        approve: true,
        terminal: &["pendingManualApproval"],
        events: &[
            PENDING,
            STARTED,
            "stepPendingApproval",
            "stepSucceeded",
            "stepPendingApproval",
        ],
    },
    Row {
        case: "one manual approval nobody decides yet",
        steps: || vec![approval_step("Approve")],
        approve: false,
        terminal: &["pendingManualApproval"],
        events: &[PENDING, STARTED, "stepPendingApproval"],
    },
    Row {
        case: "a block this emulator cannot perform",
        steps: || vec![lambda_step("Invoke")],
        approve: false,
        terminal: &["failed"],
        events: &[PENDING, STARTED, "executionFailed"],
    },
    Row {
        case: "a workflow that configures no execution block at all",
        steps: Vec::new,
        approve: false,
        terminal: &["completed"],
        events: &[PENDING, STARTED, "executionSucceeded"],
    },
];

/// The `type` of each event, in the order reported.
fn types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn an_execution_records_one_start_event_before_the_work_it_drives() {
    for Row {
        case,
        steps,
        approve,
        terminal,
        events,
    } in ROWS
    {
        let emulator = Emulator::new();
        let plan_arn = plan_with_steps(&emulator, case, "timeline", steps()).await;

        // Straight after the start the execution is `pending` and the timeline carries
        // only the event it was created with: the START event is the clock's, not this
        // call's.
        let execution_id = start_execution(&emulator, case, &plan_arn, "us-west-2").await;
        let opening = events_of(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            types(&opening),
            vec![PENDING],
            "case {case:?}: straight after the start expected only the pending event, got {:?}",
            types(&opening)
        );

        // Wait for the CONDITION — the state the clock owes — rather than for a duration.
        poll_until_state(
            &emulator,
            case,
            &plan_arn,
            &execution_id,
            &["pendingManualApproval", "failed", "completed"],
        )
        .await;
        if *approve {
            let (status, decided) = emulator
                .call(
                    "ApprovePlanExecutionStep",
                    &json!({
                        "planArn": plan_arn,
                        "executionId": execution_id,
                        "stepName": "Approve",
                        "approval": "approve",
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the approval expected 200, got {status} with {decided}"
            );
            poll_until_state(&emulator, case, &plan_arn, &execution_id, terminal).await;
        } else {
            poll_until_state(&emulator, case, &plan_arn, &execution_id, terminal).await;
        }

        let timeline = events_of(&emulator, case, &plan_arn, &execution_id).await;
        let reported = types(&timeline);
        assert_eq!(
            reported, *events,
            "case {case:?}: expected the timeline {events:?}, got {reported:?}"
        );

        // Exactly one start event, in second place: after the pending event the execution
        // was created with, and before everything the clock then drove.
        let starts = reported.iter().filter(|kind| **kind == STARTED).count();
        assert_eq!(
            starts, 1,
            "case {case:?}: expected exactly one {STARTED} event, got {starts} in {reported:?}"
        );
        let started_at = reported
            .iter()
            .position(|kind| *kind == STARTED)
            .unwrap_or_default();
        assert_eq!(
            started_at, 1,
            "case {case:?}: expected the start event right after {PENDING}, got {reported:?}"
        );

        // Every event carries its own timestamp, the stream is in non-decreasing time
        // order, and each event names the one before it — so the start event dates the
        // beginning of the span a client measures.
        let mut previous_id: Option<&str> = None;
        let mut previous_time = f64::MIN;
        for (position, event) in timeline.iter().enumerate() {
            let stamp = event["timestamp"].as_f64();
            let stamp = stamp.unwrap_or_else(|| {
                panic!("case {case:?}: event {position} carries no timestamp: {event}")
            });
            assert!(
                stamp >= previous_time,
                "case {case:?}: event {position} is stamped {stamp}, before the one before \
                 it ({previous_time}): {event}"
            );
            previous_time = stamp;
            assert_eq!(
                event["previousEventId"].as_str(),
                previous_id,
                "case {case:?}: event {position} should name the event before it, got {event}"
            );
            previous_id = event["eventId"].as_str();
        }
        let start_stamp = timeline[started_at]["timestamp"]
            .as_f64()
            .unwrap_or_default();
        let last_stamp = timeline
            .last()
            .and_then(|event| event["timestamp"].as_f64())
            .unwrap_or_default();
        assert!(
            start_stamp <= last_stamp,
            "case {case:?}: the start event ({start_stamp}) must not follow the last event \
             ({last_stamp})"
        );
    }
}
