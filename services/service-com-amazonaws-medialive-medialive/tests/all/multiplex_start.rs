// SPDX-License-Identifier: CC0-1.0

//! `StartMultiplex`: running a stored multiplex, end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case (a start of a multiplex holding no
//! program, which the real service accepts) is ONE row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{create_body, created, describe_multiplex, settings, MULTIPLEXES};
use crate::multiplex_programs::{create_program, program_settings};
use crate::wire::{call, error_code, error_message, medialive};

/// How many availability zones every multiplex here is created in — the two
/// `CreateMultiplexRequest$AvailabilityZones` documents ("You must specify
/// exactly two"), which is also the number of pipelines a RUNNING multiplex
/// reports as healthy, one per zone. Shared with the multiplex STOP suite, which
/// asserts the same count on the multiplex it stops.
pub(crate) const ZONES: i64 = 2;

/// `StartMultiplex`'s binding (`smithy_query(query="operation",
/// operation="StartMultiplex")`: `POST /prod/multiplexes/{MultiplexId}/start`,
/// success 202). Shared with the multiplex STOP suite, which starts a multiplex
/// to have one running to stop, and with the multiplex DELETION suite, which
/// drives a multiplex into the states a delete refuses.
pub(crate) async fn start_multiplex(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "POST", &format!("{MULTIPLEXES}/{id}/start"), None).await
}

/// A multiplex created under `name` with `multiplex_settings`, as its minted id.
pub(crate) async fn multiplex_with_settings(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    multiplex_settings: Value,
) -> String {
    let multiplex = created(
        router,
        case,
        create_body(name, &format!("token-{name}"), multiplex_settings),
    )
    .await;
    multiplex["id"].as_str().unwrap_or_default().to_string()
}

/// Create `count` programs in `multiplex_id`, each with its own name and its own
/// `ProgramNumber`, asserting every create succeeded.
pub(crate) async fn add_programs(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    count: i64,
) {
    for index in 0..count {
        let name = format!("program-{index}");
        let (status, program) = create_program(
            router,
            multiplex_id,
            &name,
            &format!("token-{multiplex_id}-{index}"),
            program_settings(index + 1),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram {name}: expected 201, got {status}: {program}"
        );
    }
}

/// Rule: `StartMultiplex` runs a stored multiplex that is stopped — it answers
/// 202 with that multiplex now `STARTING`, with no pipeline healthy yet, and the
/// clock settles it to `RUNNING`, where one pipeline per availability zone is
/// healthy; nothing else about the stored multiplex moves, and the reader
/// reports the same transition.
///
/// How many programs the multiplex holds is NOT a precondition: the recorded
/// `StartMultiplex` call runs a multiplex holding none, so the rows differ along
/// the program count, from none upwards, and along the `MultiplexSettings`
/// values — a start reads the multiplex's state, not the shape of what it
/// carries.
#[tokio::test]
async fn a_stopped_multiplex_starts_whatever_it_holds() {
    // (case, multiplex name, program count, multiplex settings)
    let rows: [(&str, &str, i64, Value); 5] = [
        ("no programs at all", "empty", 0, settings(2_000_000, 1)),
        ("one program", "single", 1, settings(1_000_000, 0)),
        ("two programs", "pair", 2, settings(20_000_000, 42)),
        ("three programs", "trio", 3, settings(100_000_000, 65_535)),
        (
            "every optional setting",
            "buffered",
            1,
            json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 7,
                "maximumVideoBufferDelayMilliseconds": 1_000,
                "transportStreamReservedBitrate": 100_000,
            }),
        ),
    ];

    for (case, name, programs, multiplex_settings) in rows {
        let router = medialive();
        let id = multiplex_with_settings(&router, case, name, multiplex_settings).await;
        add_programs(&router, case, &id, programs).await;
        // A multiplex is started from the stopped state, so the create's own
        // transition is let come due first.
        wait_for_the_clock();
        let (status, idle) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {idle}"
        );
        assert_eq!(
            idle["state"],
            json!("IDLE"),
            "case {case:?}: the test did not reach IDLE, it reached {:?}",
            idle["state"]
        );
        assert_eq!(
            idle["programCount"],
            json!(programs),
            "case {case:?}: expected the programs the multiplex holds, got {:?}",
            idle["programCount"]
        );

        let (status, starting) = start_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StartMultiplex: expected 202, got {status}: {starting}"
        );
        assert_eq!(
            starting["state"],
            json!("STARTING"),
            "case {case:?}: expected StartMultiplex to answer STARTING, got {:?}",
            starting["state"]
        );
        assert_eq!(
            starting["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipeline healthy while STARTING, got {:?}",
            starting["pipelinesRunningCount"]
        );
        // The start moved the state and nothing else: every other member is the
        // stored multiplex's, read back from the store rather than from this
        // request.
        let mut expected = idle.clone();
        expected["state"] = json!("STARTING");
        assert_eq!(
            starting, expected,
            "case {case:?}: expected only the state to have moved, got {starting}"
        );

        // The transition the clock owes: `STARTING` settles to `RUNNING`, where
        // one pipeline per availability zone is healthy.
        wait_for_the_clock();
        let (status, running) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the clock: expected 200, got {status}: {running}"
        );
        assert_eq!(
            running["state"],
            json!("RUNNING"),
            "case {case:?}: expected a started multiplex to settle to RUNNING, got {:?}",
            running["state"]
        );
        assert_eq!(
            running["pipelinesRunningCount"],
            json!(ZONES),
            "case {case:?}: expected one healthy pipeline per availability zone, got {:?}",
            running["pipelinesRunningCount"]
        );
        let mut expected = idle.clone();
        expected["state"] = json!("RUNNING");
        expected["pipelinesRunningCount"] = json!(ZONES);
        assert_eq!(
            running, expected,
            "case {case:?}: expected only the state and the pipeline count to have moved, got {running}"
        );
    }
}

/// Rule: a start reads the multiplex its `MultiplexId` names — a `MultiplexId`
/// no multiplex of the account has is a 404 `NotFoundException` and starts
/// nothing, and starting one multiplex leaves every other multiplex of the
/// account where it was.
#[tokio::test]
async fn a_start_touches_only_the_multiplex_it_names() {
    let router = medialive();
    let started = multiplex_with_settings(&router, "neighbours", "started", settings(2_000_000, 1)).await;
    let untouched =
        multiplex_with_settings(&router, "neighbours", "untouched", settings(3_000_000, 2)).await;
    add_programs(&router, "neighbours", &untouched, 1).await;
    wait_for_the_clock();

    for (case, id) in [
        ("never minted", "9999999"),
        ("not a number at all", "no-such-multiplex"),
    ] {
        let (status, body) = start_multiplex(&router, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Multiplex {id} not found"),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    let (status, body) = start_multiplex(&router, &started).await;
    assert_eq!(
        status, 202,
        "StartMultiplex: expected 202, got {status}: {body}"
    );
    let (status, neighbour) = describe_multiplex(&router, &untouched).await;
    assert_eq!(
        status, 200,
        "DescribeMultiplex: expected 200, got {status}: {neighbour}"
    );
    assert_eq!(
        neighbour["state"],
        json!("IDLE"),
        "expected the multiplex the start did not name to stay stopped, got {:?}",
        neighbour["state"]
    );
    assert_eq!(
        neighbour["pipelinesRunningCount"],
        json!(0),
        "expected no pipeline healthy on the multiplex the start did not name, got {:?}",
        neighbour["pipelinesRunningCount"]
    );
}

/// Rule: a multiplex runs from the stopped state only, so a start sent while it
/// is in any other state is refused with a 409 `ConflictException` carrying this
/// service's wording for that class — which names the multiplex and the state it
/// is in — and moves nothing: a start is never a no-op.
///
/// The rows span every state this slice can put a multiplex in besides the
/// stopped one. The states an accepted delete puts it in are refused the same
/// way, on the recorded evidence `crate::multiplex_start_after_delete` pins.
#[tokio::test]
async fn a_multiplex_starts_only_from_the_stopped_state() {
    // (case, the state the multiplex is in when the start is sent)
    let conflicts: [(&str, &str); 3] = [
        ("while still creating", "CREATING"),
        ("while already starting", "STARTING"),
        ("while already running", "RUNNING"),
    ];

    for (case, state) in conflicts {
        let router = medialive();
        let id = multiplex_with_settings(&router, case, "stateful", settings(2_000_000, 1)).await;
        // A program can be created in a multiplex that is still coming up, so
        // every row carries one: the state is what the row varies.
        add_programs(&router, case, &id, 1).await;
        if state != "CREATING" {
            wait_for_the_clock();
            let (status, started) = start_multiplex(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
            );
        }
        if state == "RUNNING" {
            wait_for_the_clock();
        }

        let (status, observed) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {observed}"
        );
        assert_eq!(
            observed["state"],
            json!(state),
            "case {case:?}: the test did not reach {state}, it reached {:?}",
            observed["state"]
        );

        let (status, body) = start_multiplex(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected starting a {state} multiplex to conflict, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Multiplex {id} cannot be started while in {state} state"),
            "case {case:?}: expected the refusal to name the multiplex and the state it is in, got {body}"
        );

        // The refused start moved nothing.
        let (status, after) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the refusal: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after["state"], observed["state"],
            "case {case:?}: expected the refused start to leave the state alone, got {after}"
        );
    }
}
