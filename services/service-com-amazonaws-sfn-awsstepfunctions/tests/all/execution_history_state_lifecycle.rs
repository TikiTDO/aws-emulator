// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the events a COMPOUND state writes BETWEEN the pair it was
//! entered and left with, driven over this service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! `tests/all/execution_history.rs` asserts the events of one simple state and how
//! they are paged, and `tests/all/execution_history_outcomes.rs` the event an
//! execution ENDS on. The rules here are the ones about the work a state does while it
//! is inside it:
//!
//! 1. every attempt at a state whose work is a state machine of its own writes that
//!    state type's own `<Type>StateStarted` and the event of the outcome the attempt
//!    reached (`<Type>StateSucceeded` or `<Type>StateFailed`), between the state's
//!    `<Type>StateEntered` and `<Type>StateExited` — so a retried state is started
//!    once per attempt and each attempt's outcome is pairable with its start;
//! 2. each unit of a state's CONCURRENT work writes its own start and outcome: one
//!    `MapIterationStarted` and one `MapIterationSucceeded`/`MapIterationFailed` per
//!    item, carrying the state's name and "the index of the array belonging to the Map
//!    state iteration", and `MapStateStarted` carries "the size of the array for Map
//!    state iterations";
//! 3. a `Task` state's own work is handed over, started and finished on the state that
//!    named it: `TaskScheduled` (with the kind of resource, the action, the region and
//!    the document the attempt built), `TaskStarted`, and exactly one `TaskSucceeded`,
//!    `TaskFailed` or `TaskTimedOut` carrying what the work answered or failed with —
//!    whatever the execution then does with that failure;
//! 4. a unit of concurrent work that fails a state that tolerates no failure STOPS the
//!    rest of that state's work at the instant it failed: a state a surviving branch or
//!    iteration would only have entered after that instant is never entered, the work
//!    that was in flight records that state type's own aborted event, and the state
//!    fails at that instant rather than when the stopped work would have finished.
//!
//! The harness is the one `state_machine_lifecycle` builds, the whole-history reader is
//! `execution_history`'s, and the Step Functions + DynamoDB chain a task whose
//! integration really answers needs is `task_integrations`'; `tests/all.rs` is this
//! binary's crate root, so each is reached through `crate::<suite>`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};
use crate::task_integrations::Chain;

/// The `type` of each event, in the order they were answered.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a type in {event:?}"))
                .to_string()
        })
        .collect()
}

/// Where every event of `event_type` sits in the history, in the order it was
/// answered.
fn positions(events: &[Value], event_type: &str) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.get("type").and_then(Value::as_str) == Some(event_type))
        .map(|(index, _)| index)
        .collect()
}

/// The detail member `member` of every event of `event_type`, in order.
fn details(events: &[Value], event_type: &str, member: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .map(|event| {
            event
                .get(member)
                .cloned()
                .unwrap_or_else(|| panic!("expected a {member} in {event:?}"))
        })
        .collect()
}

/// The name of every state the execution ENTERED, in order.
fn entered(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event.get("stateEnteredEventDetails"))
        .filter_map(|detail| detail.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One row of the lifecycle table: a definition, and how many of each event type its
/// history must carry.
struct Lifecycle {
    case: &'static str,
    definition: Value,
    input: Value,
    /// How much further out the definition's own `Wait` states and `Retry` intervals
    /// push the outcome's due instant.
    owed: Duration,
    /// `(event type, how many of them)`, which is exact: an event type the rule says
    /// nothing happened for is listed as `0`.
    counts: &'static [(&'static str, usize)],
}

/// The history of one execution of `definition`, once the clock has made its outcome
/// due.
async fn ran(
    emulator: &Emulator,
    name: &str,
    definition: &Value,
    input: &Value,
    owed: Duration,
) -> (Vec<Value>, Value) {
    let machine = create_machine(emulator, name, &definition.to_string()).await;
    let execution = start_execution(emulator, &machine, "only", &input.to_string()).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY.saturating_add(owed));
    let described = describe_execution(emulator, &execution).await;
    (history(emulator, &execution, json!({})).await, described)
}

/// A `Parallel` state whose branches are `branches`, carrying `extra` members.
fn parallel(branches: Value, extra: Value) -> Value {
    let mut state = json!({ "Type": "Parallel", "Branches": branches, "End": true });
    for (member, value) in extra.as_object().expect("the extras are an object") {
        state[member.as_str()] = value.clone();
    }
    json!({ "StartAt": "Fan", "States": { "Fan": state } })
}

/// An inline `Map` state over the items its input carries, each iteration running
/// `processor`, carrying `extra` members.
fn inline_map(processor: Value, extra: Value) -> Value {
    let mut state = json!({ "Type": "Map", "ItemProcessor": processor, "End": true });
    for (member, value) in extra.as_object().expect("the extras are an object") {
        state[member.as_str()] = value.clone();
    }
    json!({ "StartAt": "Each", "States": { "Each": state } })
}

/// One row of the `Task` table: the state, the work its `Resource` names, and how the
/// work turned out.
struct Work {
    case: &'static str,
    state: Value,
    /// "The service name of the resource in a task state" and "the action of the
    /// resource called by a task state", as the state's events must report them.
    resource_type: &'static str,
    resource: &'static str,
    /// Whether the work was handed over at all: a `Resource` this emulator makes no
    /// call for fails the task before there is anything to schedule or start.
    handed_over: bool,
    /// The event the work's own outcome is reported through, and the error name on it
    /// for an outcome that is a failure.
    outcome: &'static str,
    error: Option<&'static str>,
}

/// A branch or processor of one `Pass` state.
fn passing(name: &'static str) -> Value {
    json!({ "StartAt": name, "States": { name: { "Type": "Pass", "End": true } } })
}

/// A branch or processor of one `Fail` state.
fn failing(error: &'static str) -> Value {
    json!({
        "StartAt": "Boom",
        "States": { "Boom": { "Type": "Fail", "Error": error, "Cause": "on purpose" } }
    })
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: a compound state's own lifecycle, and its iterations'
// ---------------------------------------------------------------------------

/// Every attempt at a `Parallel` or a `Map` state writes its own start and the event of
/// the outcome it reached, and every iteration of a `Map` writes its own pair.
///
/// The rows differ along what the rule generalises over: the state TYPE (`Parallel` and
/// inline `Map`), the OUTCOME (every unit succeeded, one of them failed), the number of
/// ATTEMPTS a `Retry` makes (one, two, three), the COUNT of concurrent units (none,
/// one, three), how many of them run at once (`MaxConcurrency` unset and `1`), and
/// NESTING (a `Map` inside a `Parallel` branch).
#[tokio::test]
async fn every_attempt_at_a_compound_state_records_its_own_start_and_outcome() {
    let rows: Vec<Lifecycle> = vec![
        Lifecycle {
            case: "a Parallel whose two branches both succeed",
            definition: parallel(json!([passing("Left"), passing("Right")]), json!({})),
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateEntered", 1),
                ("ParallelStateStarted", 1),
                ("ParallelStateSucceeded", 1),
                ("ParallelStateFailed", 0),
                ("ParallelStateExited", 1),
            ],
        },
        Lifecycle {
            case: "a Parallel whose branch fails, which no catcher takes over",
            definition: parallel(json!([failing("BranchBroke")]), json!({})),
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateEntered", 1),
                ("ParallelStateStarted", 1),
                ("ParallelStateSucceeded", 0),
                ("ParallelStateFailed", 1),
                ("ParallelStateExited", 0),
                ("ExecutionFailed", 1),
            ],
        },
        Lifecycle {
            case: "a Parallel retried twice and then caught: one start per attempt",
            definition: {
                let mut definition = parallel(
                    json!([failing("BranchBroke")]),
                    json!({
                        "Retry": [{
                            "ErrorEquals": ["BranchBroke"],
                            "MaxAttempts": 2,
                            "IntervalSeconds": 0,
                        }],
                        "Catch": [{ "ErrorEquals": ["BranchBroke"], "Next": "Report" }],
                        "Next": "Report",
                    }),
                );
                definition["States"]["Fan"]
                    .as_object_mut()
                    .expect("the state is an object")
                    .remove("End");
                definition["States"]["Report"] = json!({ "Type": "Pass", "End": true });
                definition
            },
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateEntered", 1),
                ("ParallelStateStarted", 3),
                ("ParallelStateFailed", 3),
                ("ParallelStateSucceeded", 0),
                ("ParallelStateExited", 1),
                ("ExecutionSucceeded", 1),
            ],
        },
        Lifecycle {
            case: "a Parallel retried once: one start per attempt",
            definition: parallel(
                json!([failing("BranchBroke")]),
                json!({
                    "Retry": [{
                        "ErrorEquals": ["States.ALL"],
                        "MaxAttempts": 1,
                        "IntervalSeconds": 0,
                    }],
                }),
            ),
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateStarted", 2),
                ("ParallelStateFailed", 2),
                ("ExecutionFailed", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map over three items",
            definition: inline_map(passing("Item"), json!({})),
            input: json!(["a", "b", "c"]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateEntered", 1),
                ("MapStateStarted", 1),
                ("MapIterationStarted", 3),
                ("MapIterationSucceeded", 3),
                ("MapIterationFailed", 0),
                ("MapStateSucceeded", 1),
                ("MapStateFailed", 0),
                ("MapStateExited", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map over one item",
            definition: inline_map(passing("Item"), json!({})),
            input: json!(["only"]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateStarted", 1),
                ("MapIterationStarted", 1),
                ("MapIterationSucceeded", 1),
                ("MapStateSucceeded", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map over no items at all",
            definition: inline_map(passing("Item"), json!({})),
            input: json!([]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateStarted", 1),
                ("MapIterationStarted", 0),
                ("MapIterationSucceeded", 0),
                ("MapStateSucceeded", 1),
                ("MapStateExited", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map with MaxConcurrency 1, which serialises its iterations",
            definition: inline_map(passing("Item"), json!({ "MaxConcurrency": 1 })),
            input: json!([1, 2, 3]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateStarted", 1),
                ("MapIterationStarted", 3),
                ("MapIterationSucceeded", 3),
                ("MapStateSucceeded", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map whose iterations fail",
            definition: inline_map(failing("ItemBroke"), json!({})),
            input: json!(["a", "b"]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateStarted", 1),
                ("MapIterationStarted", 2),
                ("MapIterationSucceeded", 0),
                ("MapIterationFailed", 2),
                ("MapStateSucceeded", 0),
                ("MapStateFailed", 1),
            ],
        },
        Lifecycle {
            case: "a Map nested inside a Parallel branch, which records both lifecycles",
            definition: parallel(
                json!([{
                    "StartAt": "Each",
                    "States": {
                        "Each": {
                            "Type": "Map",
                            "ItemsPath": "$.items",
                            "ItemProcessor": passing("Item"),
                            "End": true,
                        }
                    }
                }]),
                json!({}),
            ),
            input: json!({ "items": ["a", "b"] }),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateStarted", 1),
                ("ParallelStateSucceeded", 1),
                ("MapStateStarted", 1),
                ("MapIterationStarted", 2),
                ("MapIterationSucceeded", 2),
                ("MapStateSucceeded", 1),
            ],
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let (events, _) = ran(
            &emulator,
            &format!("lifecycle-{index}"),
            &row.definition,
            &row.input,
            row.owed,
        )
        .await;
        let present = types(&events);
        for (event_type, expected) in row.counts {
            assert_eq!(
                positions(&events, event_type).len(),
                *expected,
                "case {:?}: expected {expected} {event_type} events, got {present:?}",
                row.case
            );
        }
        // Every lifecycle event of a state sits inside the pair the state was entered
        // and left with.
        for state_type in ["Parallel", "Map"] {
            let Some(first_entered) = positions(&events, &format!("{state_type}StateEntered"))
                .first()
                .copied()
            else {
                continue;
            };
            let last = positions(&events, &format!("{state_type}StateExited"))
                .last()
                .copied()
                .unwrap_or(events.len());
            for suffix in ["Started", "Succeeded", "Failed"] {
                for position in positions(&events, &format!("{state_type}State{suffix}")) {
                    assert!(
                        position > first_entered && position < last,
                        "case {:?}: expected the {state_type}State{suffix} at {position} between \
                         the state's own {first_entered} and {last}, got {present:?}",
                        row.case
                    );
                }
            }
        }
    }
}

/// A `Map` state's own events carry the size of the array it is iterating, and each
/// iteration's events the state's name and the index of the item it was handed.
///
/// The rows differ in the COUNT of items, so the indices the iterations report are the
/// positions of a different array each time; the state's name is a member of the
/// definition rather than of this emulator, so it is read back from the events.
#[tokio::test]
async fn a_maps_events_carry_the_array_size_and_each_iterations_index() {
    let emulator = Emulator::new();
    for (index, items) in [json!([]), json!(["a"]), json!([1, 2, 3, 4])]
        .into_iter()
        .enumerate()
    {
        let count = items.as_array().expect("the items are an array").len();
        let case = format!("an inline Map over {count} items");
        let (events, _) = ran(
            &emulator,
            &format!("map-indices-{index}"),
            &inline_map(passing("Item"), json!({})),
            &items,
            Duration::ZERO,
        )
        .await;

        // "The size of the array for Map state iterations."
        assert_eq!(
            details(&events, "MapStateStarted", "mapStateStartedEventDetails")
                .iter()
                .map(|detail| detail.get("length").and_then(Value::as_i64))
                .collect::<Vec<Option<i64>>>(),
            vec![Some(i64::try_from(count).expect("the count fits"))],
            "case {case:?}: expected one MapStateStarted carrying the array's size, got {events:?}"
        );

        // One iteration per item, each naming its parent Map state and the index of
        // the item it was handed.
        for event_type in ["MapIterationStarted", "MapIterationSucceeded"] {
            let member = if event_type == "MapIterationStarted" {
                "mapIterationStartedEventDetails"
            } else {
                "mapIterationSucceededEventDetails"
            };
            let reported = details(&events, event_type, member);
            assert_eq!(
                reported
                    .iter()
                    .map(|detail| (
                        detail.get("name").and_then(Value::as_str).map(str::to_string),
                        detail.get("index").and_then(Value::as_i64),
                    ))
                    .collect::<Vec<(Option<String>, Option<i64>)>>(),
                (0..count)
                    .map(|position| (
                        Some("Each".to_string()),
                        Some(i64::try_from(position).expect("the index fits")),
                    ))
                    .collect::<Vec<(Option<String>, Option<i64>)>>(),
                "case {case:?}: expected one {event_type} per item, each naming the state and its \
                 index, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a Task state's own work
// ---------------------------------------------------------------------------

/// A `Task` state's work is handed over, started and finished on the state that named
/// it: every event names the kind of resource and the action, the handover carries the
/// document that attempt built, and the outcome carries what the work answered or
/// failed with.
///
/// The rows differ along what the rule generalises over: the integration FAMILY (the
/// optimized `arn:aws:states:::<service>:<action>` form and the AWS SDK one) and the
/// OUTCOME (the call answered, the service refused it, and a `Resource` this emulator
/// makes no call for at all, which is a failure of the task before its work was ever
/// handed over).
#[tokio::test]
async fn a_task_states_events_name_its_work_and_carry_its_outcome() {
    let chain = Chain::new();
    chain.create_table("lifecycle").await;

    let rows: Vec<Work> = vec![
        Work {
            case: "the optimized putItem integration, which answers",
            state: json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Parameters": { "TableName": "lifecycle", "Item": { "pk": { "S": "k-1" } } },
                "End": true,
            }),
            resource_type: "dynamodb",
            resource: "putItem",
            handed_over: true,
            outcome: "TaskSucceeded",
            error: None,
        },
        Work {
            case: "the aws-sdk family of the same action",
            state: json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:dynamodb:putItem",
                "Parameters": { "TableName": "lifecycle", "Item": { "pk": { "S": "k-2" } } },
                "End": true,
            }),
            resource_type: "aws-sdk:dynamodb",
            resource: "putItem",
            handed_over: true,
            outcome: "TaskSucceeded",
            error: None,
        },
        Work {
            case: "a call the service refused",
            state: json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Parameters": { "TableName": "absent", "Item": { "pk": { "S": "k-3" } } },
                "End": true,
            }),
            resource_type: "dynamodb",
            resource: "putItem",
            handed_over: true,
            outcome: "TaskFailed",
            error: Some("DynamoDB.ResourceNotFoundException"),
        },
        Work {
            case: "a Resource this emulator makes no call for, so nothing was handed over",
            state: json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::batch:submitJob",
                "Parameters": { "JobName": "j", "JobQueue": "q" },
                "End": true,
            }),
            resource_type: "batch",
            resource: "submitJob",
            handed_over: false,
            outcome: "TaskFailed",
            error: Some("States.Runtime"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let Work {
            case,
            state: work,
            resource_type,
            resource,
            handed_over,
            outcome,
            error,
        } = row;
        let handovers = usize::from(*handed_over);
        let definition = json!({ "StartAt": "Work", "States": { "Work": work } });
        let described = chain
            .run(&format!("task-events-{index}"), &definition, &json!({}))
            .await;
        let execution = described
            .get("executionArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an executionArn in {described:?}"));
        let answered = chain
            .sfn("GetExecutionHistory", &json!({ "executionArn": execution }))
            .await;
        let events: Vec<Value> = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered:?}"))
            .clone();
        let present = types(&events);

        // One handover, one start and exactly one outcome, in that order, inside the
        // pair the state was entered with. Work that was never handed over has no
        // handover and no start: only the failure of the task.
        for (event_type, expected) in [
            ("TaskScheduled", handovers),
            ("TaskStarted", handovers),
            (*outcome, 1),
            ("TaskTimedOut", 0),
        ] {
            assert_eq!(
                positions(&events, event_type).len(),
                expected,
                "case {case:?}: expected {expected} {event_type} events, got {present:?}"
            );
        }
        let expected_order: Vec<&str> = if *handed_over {
            vec!["TaskStateEntered", "TaskScheduled", "TaskStarted", outcome]
        } else {
            vec!["TaskStateEntered", outcome]
        };
        let order: Vec<usize> = expected_order
            .iter()
            .map(|event_type| {
                *positions(&events, event_type).first().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a {event_type} in {present:?}")
                })
            })
            .collect();
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: expected the task's events in order, got {present:?}"
        );

        // Every one of them names the work: "the service name of the resource in a
        // task state" and "the action of the resource called by a task state".
        let named: Vec<(&str, &str)> = vec![
            (
                *outcome,
                if *outcome == "TaskSucceeded" {
                    "taskSucceededEventDetails"
                } else {
                    "taskFailedEventDetails"
                },
            ),
            ("TaskScheduled", "taskScheduledEventDetails"),
            ("TaskStarted", "taskStartedEventDetails"),
        ];
        for (event_type, member) in named.into_iter().take(1 + 2 * handovers) {
            let reported = details(&events, event_type, member);
            assert_eq!(
                reported
                    .iter()
                    .map(|detail| (
                        detail
                            .get("resourceType")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        detail
                            .get("resource")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    ))
                    .collect::<Vec<(Option<String>, Option<String>)>>(),
                vec![(
                    Some((*resource_type).to_string()),
                    Some((*resource).to_string())
                )],
                "case {case:?}: expected {event_type} to name the work, got {reported:?}"
            );
        }

        // The handover carries the document the attempt built, in the region the
        // execution's own ARN names.
        for detail in details(&events, "TaskScheduled", "taskScheduledEventDetails") {
            assert_eq!(
                detail.get("parameters").and_then(Value::as_str).map(|text| {
                    serde_json::from_str::<Value>(text).expect("the parameters are JSON")
                }),
                work.get("Parameters").cloned(),
                "case {case:?}: expected the document the attempt built, got {detail:?}"
            );
            assert!(
                detail
                    .get("region")
                    .and_then(Value::as_str)
                    .is_some_and(|region| execution.contains(region)),
                "case {case:?}: expected the region of the scheduled task, got {detail:?}"
            );
        }

        // The outcome carries what the work answered, or the error and cause it failed
        // with.
        let member = if *outcome == "TaskSucceeded" {
            "taskSucceededEventDetails"
        } else {
            "taskFailedEventDetails"
        };
        let detail = details(&events, outcome, member).remove(0);
        match error {
            None => assert!(
                detail
                    .get("output")
                    .and_then(Value::as_str)
                    .is_some_and(|output| serde_json::from_str::<Value>(output).is_ok()),
                "case {case:?}: expected the document the call answered, got {detail:?}"
            ),
            Some(expected) => {
                assert_eq!(
                    detail.get("error").and_then(Value::as_str),
                    Some(*expected),
                    "case {case:?}: expected the error the work failed with, got {detail:?}"
                );
                assert!(
                    detail
                        .get("cause")
                        .and_then(Value::as_str)
                        .is_some_and(|cause| !cause.is_empty()),
                    "case {case:?}: expected a cause saying what happened, got {detail:?}"
                );
            }
        }
    }
}

/// A `Retry` on a `Task` state hands the work over again for each attempt, so the
/// state's events count the attempts it really made.
#[tokio::test]
async fn a_retried_task_records_one_handover_per_attempt() {
    let chain = Chain::new();
    for (index, attempts) in [0_u64, 1, 2].into_iter().enumerate() {
        let case = format!("a Task retried {attempts} times");
        let mut work = json!({
            "Type": "Task",
            "Resource": "arn:aws:states:::dynamodb:putItem",
            "Parameters": { "TableName": "absent", "Item": { "pk": { "S": "k" } } },
            "End": true,
        });
        if attempts > 0 {
            work["Retry"] = json!([{
                "ErrorEquals": ["States.ALL"],
                "MaxAttempts": attempts,
                "IntervalSeconds": 0,
            }]);
        }
        let described = chain
            .run(
                &format!("task-retries-{index}"),
                &json!({ "StartAt": "Work", "States": { "Work": work } }),
                &json!({}),
            )
            .await;
        let execution = described
            .get("executionArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an executionArn in {described:?}"));
        let answered = chain
            .sfn("GetExecutionHistory", &json!({ "executionArn": execution }))
            .await;
        let events: Vec<Value> = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected events in {answered:?}"))
            .clone();
        let expected = usize::try_from(attempts).expect("the count fits") + 1;
        for event_type in ["TaskScheduled", "TaskStarted", "TaskFailed"] {
            assert_eq!(
                positions(&events, event_type).len(),
                expected,
                "case {case:?}: expected {expected} {event_type} events, got {:?}",
                types(&events)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the failure of one unit stops the rest of the state's work
// ---------------------------------------------------------------------------

/// A branch or iteration that fails a state tolerating no failure stops the rest of
/// that state's work at the instant it failed: a state the surviving work would only
/// have entered after that instant is never entered, what was in flight records its own
/// aborted event, and the state fails at that instant.
///
/// The rows differ along what the rule generalises over: which unit fails (the one
/// declared first and the one declared last), the kind of concurrent work (a `Parallel`
/// branch and a `Map` iteration), and the state the surviving work was parked in. The
/// surviving work is parked in a `Wait` far longer than this test runs, so an execution
/// that reported its outcome at all is one whose state did not wait for it.
#[tokio::test]
async fn a_failing_unit_stops_the_rest_of_the_states_work() {
    // The branch that survives: parked in a long Wait, and then a state that must
    // never be entered.
    let slow = json!({
        "StartAt": "LongWait",
        "States": {
            "LongWait": { "Type": "Wait", "Seconds": 600, "Next": "SlowDone" },
            "SlowDone": { "Type": "Pass", "End": true }
        }
    });
    let rows: Vec<Lifecycle> = vec![
        Lifecycle {
            case: "a Parallel whose LAST branch fails while the first is still waiting",
            definition: parallel(json!([slow.clone(), failing("Boom")]), json!({})),
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateFailed", 1),
                ("WaitStateAborted", 1),
                ("WaitStateExited", 0),
                ("ExecutionFailed", 1),
            ],
        },
        Lifecycle {
            case: "a Parallel whose FIRST branch fails while a later one is still waiting",
            definition: parallel(json!([failing("Boom"), slow.clone()]), json!({})),
            input: json!({}),
            owed: Duration::ZERO,
            counts: &[
                ("ParallelStateFailed", 1),
                ("WaitStateAborted", 1),
                ("WaitStateExited", 0),
                ("ExecutionFailed", 1),
            ],
        },
        Lifecycle {
            case: "an inline Map whose last iteration fails while an earlier one waits",
            definition: inline_map(
                json!({
                    "StartAt": "Check",
                    "States": {
                        "Check": {
                            "Type": "Choice",
                            "Choices": [{ "Variable": "$", "StringEquals": "boom", "Next": "Boom" }],
                            "Default": "LongWait"
                        },
                        "LongWait": { "Type": "Wait", "Seconds": 600, "Next": "SlowDone" },
                        "SlowDone": { "Type": "Pass", "End": true },
                        "Boom": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" }
                    }
                }),
                json!({}),
            ),
            input: json!(["wait", "boom"]),
            owed: Duration::ZERO,
            counts: &[
                ("MapStateFailed", 1),
                ("MapIterationFailed", 1),
                ("MapIterationAborted", 1),
                ("MapIterationSucceeded", 0),
                ("WaitStateAborted", 1),
                ("ExecutionFailed", 1),
            ],
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let (events, described) = ran(
            &emulator,
            &format!("abort-{index}"),
            &row.definition,
            &row.input,
            row.owed,
        )
        .await;
        let present = types(&events);
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {:?}: expected the state to fail at the instant its work did rather than \
             waiting for the work it stopped, got {described:?}",
            row.case
        );
        for (event_type, expected) in row.counts {
            assert_eq!(
                positions(&events, event_type).len(),
                *expected,
                "case {:?}: expected {expected} {event_type} events, got {present:?}",
                row.case
            );
        }
        let names = entered(&events);
        assert!(
            names.iter().any(|name| name == "LongWait"),
            "case {:?}: expected the surviving work to have been entered, got {names:?}",
            row.case
        );
        assert!(
            !names.iter().any(|name| name == "SlowDone"),
            "case {:?}: expected the state after the Wait never to be entered, got {names:?}",
            row.case
        );
    }
}
