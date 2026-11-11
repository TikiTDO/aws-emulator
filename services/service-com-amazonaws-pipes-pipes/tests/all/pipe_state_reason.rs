// SPDX-License-Identifier: CC0-1.0

//! What a pipe reports as its `StateReason`, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — reusing the harness in `tests/all/pipe_lifecycle.rs`
//! rather than copying it, so what is tested is the HTTP behaviour of the lifecycle calls
//! read back through `DescribePipe`/`ListPipes` and not a Rust helper.
//!
//! RULE: a pipe's `StateReason` says HOW it came to be in the state it is in, so a state a
//! LIFECYCLE CALL drove it into — a creation into the `DesiredState` it asked for, a start,
//! a stop, an update — is reported with the fixed user-initiated reason token, from the
//! moment the call lands and for as long as the pipe rests there, by every read that
//! carries the member.
//!
//! The tables span the class that rule generalises over: which lifecycle call drove the
//! state, which state it drove the pipe into, whether the move is still in flight or has
//! settled, and the shape of the read (`DescribePipe`, an unpaged listing, a filtered one,
//! and each page of a paged one).

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::USER_INITIATED_STATE_REASON;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{
    call, create, describe, list, minimal_body, router, Reply, PAST_SETTLE, REGION,
};

const SOURCE: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET: &str = "arn:aws:sqs:us-west-2:000000000000:results";
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";

/// A `CreatePipe` body, optionally asking for a `DesiredState`.
fn create_body(desired_state: Option<&str>) -> Value {
    let mut body = minimal_body(SOURCE, TARGET);
    if let Some(state) = desired_state {
        body.as_object_mut()
            .expect("the create body is an object")
            .insert("DesiredState".to_string(), json!(state));
    }
    body
}

/// One lifecycle call on an existing pipe, named by the operation the case drives.
async fn drive(router: &MultiServiceRouter, name: &str, operation: &str) -> Reply {
    match operation {
        "StartPipe" => {
            call(
                router,
                "POST",
                &format!("/v1/pipes/{name}/start"),
                None,
                REGION,
            )
            .await
        }
        "StopPipe" => {
            call(
                router,
                "POST",
                &format!("/v1/pipes/{name}/stop"),
                None,
                REGION,
            )
            .await
        }
        "UpdatePipe" => {
            call(
                router,
                "PUT",
                &format!("/v1/pipes/{name}"),
                Some(json!({ "RoleArn": ROLE })),
                REGION,
            )
            .await
        }
        other => panic!("case {name:?}: no lifecycle call named {other:?}"),
    }
}

/// The `StateReason` each listed pipe reported, keyed by the pipe's name, in the order the
/// listing returned them. `None` for an entry that omitted the member altogether, which is
/// the omission under test.
///
/// `pub(crate)` because `tests/all/pipe_listing_state_reason.rs` reads the entries of its
/// listings the same way, so there is ONE reader of the `Pipes[]` shape.
pub(crate) fn listed_reasons(reply: &Reply) -> Vec<(String, Option<String>)> {
    reply
        .member("Pipes")
        .as_array()
        .map(|pipes| {
            pipes
                .iter()
                .map(|pipe| {
                    (
                        pipe.get("Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        pipe.get("StateReason")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the lifecycle table: the call that drove the pipe's state, and the states
/// that call passes the pipe through.
struct Row {
    /// Also the pipe's name, so every row is independent.
    case: &'static str,
    /// The `DesiredState` the pipe is created with, or `None` for a create that names none.
    desired_state: Option<&'static str>,
    /// The lifecycle call that drives the state under test, or `None` for the row the
    /// creation itself drives.
    operation: Option<&'static str>,
    /// The transient `PipeState` that call reports while its move is in flight.
    transient: &'static str,
    /// The `PipeState` the pipe settles in.
    settled: &'static str,
}

/// RULE: whichever lifecycle call drove the pipe's current state, `DescribePipe` reports
/// the user-initiated reason for it — while the move is still in flight and once it has
/// settled.
///
/// The rows differ along the call that drove the state, the state it drove the pipe into,
/// and the state the pipe was in before it.
#[tokio::test]
async fn describe_pipe_reports_the_user_initiated_reason_for_every_caller_driven_state() {
    let rows = vec![
        Row {
            case: "created-default",
            desired_state: None,
            operation: None,
            transient: "CREATING",
            settled: "RUNNING",
        },
        Row {
            case: "created-running",
            desired_state: Some("RUNNING"),
            operation: None,
            transient: "CREATING",
            settled: "RUNNING",
        },
        Row {
            case: "created-stopped",
            desired_state: Some("STOPPED"),
            operation: None,
            transient: "CREATING",
            settled: "STOPPED",
        },
        Row {
            case: "started-from-stopped",
            desired_state: Some("STOPPED"),
            operation: Some("StartPipe"),
            transient: "STARTING",
            settled: "RUNNING",
        },
        Row {
            case: "stopped-from-running",
            desired_state: Some("RUNNING"),
            operation: Some("StopPipe"),
            transient: "STOPPING",
            settled: "STOPPED",
        },
        Row {
            case: "updated-while-running",
            desired_state: Some("RUNNING"),
            operation: Some("UpdatePipe"),
            transient: "UPDATING",
            settled: "RUNNING",
        },
    ];

    for row in rows {
        let (case, transient, settled) = (row.case, row.transient, row.settled);
        let router = router();
        let created = create(&router, case, create_body(row.desired_state)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        // A case that drives a LATER call lets the creation settle first, so the state
        // under test is the one that call drove and not the creation's.
        let driving = match row.operation {
            None => created,
            Some(operation) => {
                tokio::time::sleep(PAST_SETTLE).await;
                let reply = drive(&router, case, operation).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {operation} must succeed, got {reply:?}"
                );
                reply
            }
        };
        assert_eq!(
            driving.string("CurrentState"),
            transient,
            "case {case:?}: expected the call to report the pipe {transient}, got {driving:?}"
        );

        // While the move is still in flight: the pipe is in a transient state its caller
        // asked for, so the reason is already the user-initiated one.
        let described = describe(&router, case).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribePipe must find the pipe, got {described:?}"
        );
        let in_flight = described.member("StateReason").as_str().map(str::to_string);
        assert_eq!(
            in_flight.as_deref(),
            Some(USER_INITIATED_STATE_REASON),
            "case {case:?}: expected StateReason {USER_INITIATED_STATE_REASON} while the \
             move is in flight, got {in_flight:?}"
        );

        // And once the move has landed, the reason still says how the pipe got there.
        tokio::time::sleep(PAST_SETTLE).await;
        let described = describe(&router, case).await;
        assert_eq!(
            described.string("CurrentState"),
            settled,
            "case {case:?}: expected the pipe to settle in {settled}, got {described:?}"
        );
        let rested = described.member("StateReason").as_str().map(str::to_string);
        assert_eq!(
            rested.as_deref(),
            Some(USER_INITIATED_STATE_REASON),
            "case {case:?}: expected StateReason {USER_INITIATED_STATE_REASON} for a pipe \
             resting in the state its caller asked for, got {rested:?}"
        );
    }
}

/// RULE: as above, read through `ListPipes`: every listed pipe carries the reason for the
/// state it is in, whichever listing shape returned it.
///
/// The rows differ along the shape of the read — unpaged, filtered by name prefix, filtered
/// by desired state, and each page of a paged listing — over a set of pipes whose states
/// were driven by different lifecycle calls.
#[tokio::test]
async fn every_listed_pipe_reports_the_reason_for_the_state_it_is_in() {
    let router = router();
    // Three pipes whose resting states were driven differently: one left as created, one
    // stopped by a later call, one started by a later call.
    let pipes: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("list-a-created", None, None),
        ("list-b-stopped", Some("RUNNING"), Some("StopPipe")),
        ("list-c-started", Some("STOPPED"), Some("StartPipe")),
    ];
    for (name, desired_state, _) in &pipes {
        let created = create(&router, name, create_body(*desired_state)).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreatePipe must succeed, got {created:?}"
        );
    }
    tokio::time::sleep(PAST_SETTLE).await;
    for (name, _, operation) in &pipes {
        if let Some(operation) = operation {
            let reply = drive(&router, name, operation).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: {operation} must succeed, got {reply:?}"
            );
        }
    }
    tokio::time::sleep(PAST_SETTLE).await;

    // (case, the query, the names the listing is expected to return)
    let listings: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "unpaged",
            String::new(),
            vec!["list-a-created", "list-b-stopped", "list-c-started"],
        ),
        (
            "name-prefix",
            "?NamePrefix=list-b".to_string(),
            vec!["list-b-stopped"],
        ),
        (
            "desired-state-filter",
            "?DesiredState=STOPPED".to_string(),
            vec!["list-b-stopped"],
        ),
        (
            "current-state-filter",
            "?CurrentState=RUNNING".to_string(),
            vec!["list-a-created", "list-c-started"],
        ),
    ];
    for (case, query, expected_names) in listings {
        let listed = list(&router, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListPipes must succeed, got {listed:?}"
        );
        let reasons = listed_reasons(&listed);
        let names: Vec<&str> = reasons.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the listing to return {expected_names:?}, got {names:?}"
        );
        for (name, reason) in &reasons {
            assert_eq!(
                reason.as_deref(),
                Some(USER_INITIATED_STATE_REASON),
                "case {case:?}: expected pipe {name:?} to carry StateReason \
                 {USER_INITIATED_STATE_REASON}, got {reason:?}"
            );
        }
    }

    // A paged listing: the member is on every entry of the first page and of the
    // continuation the `NextToken` resumes.
    let first = list(&router, "?Limit=2").await;
    let first_reasons = listed_reasons(&first);
    assert_eq!(
        first_reasons.len(),
        2,
        "case \"page-1\": expected two entries on a Limit=2 page, got {first:?}"
    );
    let token = first.string("NextToken");
    assert!(
        !token.is_empty(),
        "case \"page-1\": expected a NextToken for the remaining pipe, got {first:?}"
    );
    let second = list(&router, &format!("?Limit=2&NextToken={token}")).await;
    let second_reasons = listed_reasons(&second);
    assert_eq!(
        second_reasons.len(),
        1,
        "case \"page-2\": expected the one remaining pipe, got {second:?}"
    );
    for (page, reasons) in [("page-1", first_reasons), ("page-2", second_reasons)] {
        for (name, reason) in reasons {
            assert_eq!(
                reason.as_deref(),
                Some(USER_INITIATED_STATE_REASON),
                "case {page:?}: expected pipe {name:?} to carry StateReason \
                 {USER_INITIATED_STATE_REASON}, got {reason:?}"
            );
        }
    }
}
