// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that the 400-class refusals of this service are TEMPLATES over
//! the values the request carried, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rule under test: a `ValidationException` raised because of a particular value
//! the request carried states the service's fixed sentence for that condition and
//! reproduces that value — the one that identifies the resource, or every entry of
//! the list, in the order the request listed them — rather than a summary of it, so a
//! caller that sends many such requests reads WHICH value was refused out of the
//! refusal itself. The error code is shared by every validation rule of this API, so
//! the message is the only part of the refusal that says what was wrong.
//!
//! The two operations this is asserted on, and what each reproduces:
//!
//! 1. `UpdateMapRun` on a Map Run that has already finished: `Cannot update completed
//!    MapRun ` and then the `mapRunArn` the request addressed. The rows are Map Runs
//!    of ONE state machine and ONE `Map` label, so their ARNs differ only in the run
//!    identifier at the end: a message built from anything but the request's own
//!    member cannot satisfy all of them.
//! 2. `CreateStateMachineAlias` and `UpdateStateMachineAlias` on a routing
//!    configuration whose weights do not add up: `Sum of routing configuration
//!    weights must equal 100. Received: [` then the weights as received and `]`. The
//!    rows span the number of entries (one and the modelled maximum of two), the
//!    ORDER of the same two weights, how far the sum misses the total (under, over,
//!    nothing at all) and both published bounds of `VersionWeight` (`@range` 0..100)
//!    plus one past each — and every assertion parses the bracketed list back out of
//!    the message, so a refusal that summed the entries, sorted them or dropped one
//!    fails it even though its fixed text is right.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`) is the one
//! `state_machine_lifecycle` builds, `failure` is `error_messages`' own, the
//! Distributed Map processor and the `DescribeMapRun` read are `map_run_updates`', the
//! published versions come from `state_machine_versions`' `machine_with_versions` and
//! the routing entries from `state_machine_aliases`' `route`; `tests/all.rs` is this
//! binary's crate root, so each is reached through `crate::`.

use serde_json::{json, Value};

use crate::error_messages::failure;
use crate::map_run_updates::{describe, passing_processor};
use crate::state_machine_aliases::route;
use crate::state_machine_lifecycle::{
    create_machine, start_execution, string, Emulator, PAST_THE_EXECUTION_DELAY,
};
use crate::state_machine_versions::machine_with_versions;

/// The fixed text of the refusal a configuration change asked of a finished Map Run
/// is answered with, written out here rather than taken from the implementation.
const COMPLETED_MAP_RUN_PREFIX: &str = "Cannot update completed MapRun ";

/// The fixed text before the weights a refused routing configuration submitted, with
/// the closing bracket in [`listed_weights`] — again written out here rather than
/// read from the implementation.
const RECEIVED_WEIGHTS_PREFIX: &str =
    "Sum of routing configuration weights must equal 100. Received: [";

/// The `MaxConcurrency` every `Map` state below declares, so what a refused update
/// did not write stays visible in the record.
const DECLARED_CONCURRENCY: i64 = 4;

/// The published bounds of `VersionWeight` (`smithy_query(query="shape",
/// shape="VersionWeight")`: `@range` 0..100).
const MIN_WEIGHT: i64 = 0;
const MAX_WEIGHT: i64 = 100;

/// The identifier a refusal of a finished Map Run names, carved back out of
/// `message`: the text after the fixed sentence, which must be the whole of the rest
/// of it.
fn named_map_run(message: &str) -> Option<&str> {
    message.strip_prefix(COMPLETED_MAP_RUN_PREFIX)
}

/// The weights a refused routing configuration is told it submitted, parsed back out
/// of the bracketed segment of `message`.
///
/// Parsing rather than comparing whole strings is what makes this assert the
/// TEMPLATE: a refusal that rendered the sum, sorted the entries, dropped one or
/// stated no entry at all does not parse back to the list the request sent, whatever
/// the separator between the entries looks like.
fn listed_weights(message: &str) -> Option<Vec<i64>> {
    let listed = message
        .strip_prefix(RECEIVED_WEIGHTS_PREFIX)?
        .strip_suffix(']')?;
    listed
        .split(',')
        .map(|entry| entry.trim().parse::<i64>().ok())
        .collect()
}

/// A state machine whose one Distributed `Map` state is labelled `label` and whose
/// every item succeeds, so each execution of it leaves one Map Run that the clock
/// finishes.
fn distributed_definition(label: &str) -> String {
    json!({
        "StartAt": "M",
        "States": {
            "M": {
                "Type": "Map",
                "Label": label,
                "Items": [1, 2],
                "ItemProcessor": passing_processor(),
                "MaxConcurrency": DECLARED_CONCURRENCY,
                "End": true,
            },
        },
    })
    .to_string()
}

/// The `mapRunArn` of the one Map Run the named execution of `machine` started.
async fn map_run_of(emulator: &Emulator, machine: &str, execution_name: &str) -> String {
    let execution = start_execution(emulator, machine, execution_name, r#"{"go":true}"#).await;
    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let map_runs = listed
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        map_runs.len(),
        1,
        "case {execution_name:?}: one Distributed Map state started one Map Run, got {listed:?}"
    );
    string(&map_runs[0], "mapRunArn")
}

// ---------------------------------------------------------------------------
// Rule, part 1: the Map Run the request addressed
// ---------------------------------------------------------------------------

/// A configuration change asked of a Map Run that has already finished is refused
/// with the fixed sentence for that condition followed by exactly the `mapRunArn` the
/// request carried — one of several finished Map Runs whose ARNs differ only in the
/// run identifier, so the named one can only have come from the request.
#[tokio::test]
async fn the_completed_map_run_refusal_reproduces_the_requested_arn() {
    let emulator = Emulator::new();
    let machine = create_machine(
        &emulator,
        "request-value-map-run",
        &distributed_definition("FanOut.v2"),
    )
    .await;

    // (the execution that starts the Map Run, the members the refused update names
    // beside `mapRunArn`)
    let rows: Vec<(&str, Value)> = vec![
        ("run-a", json!({ "maxConcurrency": 1 })),
        ("run-b", json!({ "toleratedFailureCount": 7 })),
        (
            "run-c",
            json!({
                "maxConcurrency": 0,
                "toleratedFailureCount": 0,
                "toleratedFailurePercentage": 100,
            }),
        ),
    ];

    // Every Map Run is started before the wait, so they all become due together.
    let mut addressed: Vec<(&str, Value, String)> = Vec::new();
    for (execution_name, update) in rows {
        let arn = map_run_of(&emulator, &machine, execution_name).await;
        addressed.push((execution_name, update, arn));
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The rows share their state machine and their `Map` label, so the ARNs differ
    // only in the run identifier the service minted for each.
    for (case, _, arn) in &addressed {
        assert_eq!(
            addressed.iter().filter(|(_, _, other)| other == arn).count(),
            1,
            "case {case:?}: each row addresses a Map Run of its own, got {arn:?} twice"
        );
    }

    for (case, update, arn) in &addressed {
        let described = describe(&emulator, arn).await;
        assert_eq!(
            string(&described, "status"),
            "SUCCEEDED",
            "case {case:?}: expected the Map Run to have finished, got {described:?}"
        );

        let mut body = update.clone();
        body["mapRunArn"] = json!(arn);
        let (status, code, message) = failure(&emulator, case, "UpdateMapRun", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            named_map_run(&message),
            Some(arn.as_str()),
            "case {case:?}: expected the fixed text {COMPLETED_MAP_RUN_PREFIX:?} and then the \
             requested {arn:?}, got {message:?}"
        );

        // The refusal wrote nothing: the setting the `Map` state declared is still
        // the one the record reports.
        let after = describe(&emulator, arn).await;
        assert_eq!(
            after.get("maxConcurrency").and_then(Value::as_i64),
            Some(DECLARED_CONCURRENCY),
            "case {case:?}: a refused update left the record alone, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule, part 2: the weights the request submitted
// ---------------------------------------------------------------------------

/// A routing configuration whose weights do not add up is refused with the fixed
/// sentence for that rule and the weights as RECEIVED — every entry, in request
/// order — on both operations that take one, and nothing is stored.
#[tokio::test]
async fn the_routing_weight_refusal_reproduces_every_submitted_weight() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "request-value-weights", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");

    // The alias the update rows address, created at the total so it exists.
    let live = string(
        &emulator
            .ok(
                "CreateStateMachineAlias",
                &json!({ "name": "LIVE", "routingConfiguration": [route(&first, MAX_WEIGHT)] }),
            )
            .await,
        "stateMachineAliasArn",
    );

    // The name the create rows use, whose alias must not exist afterwards.
    let refused_name = "CANARY";
    let refused_arn = format!("{machine}:{refused_name}");

    // (case, the weights of the entries the request names, in request order)
    let rows: Vec<(&str, Vec<i64>)> = vec![
        ("one entry under the total", vec![42]),
        ("one entry at the bottom of the range", vec![MIN_WEIGHT]),
        ("one entry one short of the total", vec![MAX_WEIGHT - 1]),
        ("two entries under the total", vec![40, 50]),
        // The same two weights the other way round: a refusal that sorted or
        // summed them could not tell this row from the one above it.
        ("those two entries in the other order", vec![50, 40]),
        ("two entries past the total", vec![MAX_WEIGHT, 1]),
        ("two entries at the bottom of the range", vec![0, 0]),
    ];

    for (case, weights) in &rows {
        let routing: Vec<Value> = weights
            .iter()
            .zip([&first, &second])
            .map(|(weight, version)| route(version, *weight))
            .collect();
        assert_eq!(
            routing.len(),
            weights.len(),
            "case {case:?}: every weight names a version of its own"
        );

        for (operation, body) in [
            (
                "CreateStateMachineAlias",
                json!({ "name": refused_name, "routingConfiguration": &routing }),
            ),
            (
                "UpdateStateMachineAlias",
                json!({ "stateMachineAliasArn": &live, "routingConfiguration": &routing }),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {code} with \
                 message {message:?}"
            );
            assert_eq!(
                listed_weights(&message).as_deref(),
                Some(weights.as_slice()),
                "case {case:?}: expected the fixed text {RECEIVED_WEIGHTS_PREFIX:?} and then the \
                 submitted weights {weights:?}, got {message:?}"
            );
        }

        // Neither refusal stored anything: the create left no alias of that name,
        // and the update left the one it addressed at the configuration it had.
        let (status, parsed) = emulator
            .call(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": &refused_arn }),
            )
            .await;
        assert_ne!(
            status, 200,
            "case {case:?}: a refused create stored no alias, got {parsed:?}"
        );
        let described = emulator
            .ok(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": &live }),
            )
            .await;
        assert_eq!(
            reported_routing(&described),
            vec![(first.clone(), MAX_WEIGHT)],
            "case {case:?}: a refused update left the stored configuration, got {described:?}"
        );
    }
}

/// The routing configuration a record reports, as `(versionArn, weight)` pairs in the
/// order it lists them.
fn reported_routing(described: &Value) -> Vec<(String, i64)> {
    described
        .get("routingConfiguration")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a routingConfiguration in {described:?}"))
        .iter()
        .map(|entry| {
            (
                string(entry, "stateMachineVersionArn"),
                entry
                    .get("weight")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected a weight in {entry:?}")),
            )
        })
        .collect()
}

/// The controls on the rule above: a weight PAST the modelled range is the constraint
/// violation for that member and not the sum rule, and a configuration whose weights
/// do add up is applied and read back entry by entry.
#[tokio::test]
async fn a_weight_outside_the_range_and_one_that_adds_up_are_answered_differently() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "weight-range-control", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");

    // One past each published bound of `VersionWeight`: neither is a configuration
    // whose weights are summed at all, so neither is answered with the sum sentence.
    for (case, weight) in [
        ("one past the top of the range", MAX_WEIGHT + 1),
        ("one past the bottom of the range", MIN_WEIGHT - 1),
    ] {
        let (status, code, message) = failure(
            &emulator,
            case,
            "CreateStateMachineAlias",
            &json!({ "name": "OUTSIDE", "routingConfiguration": [route(&first, weight)] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            listed_weights(&message),
            None,
            "case {case:?}: a weight outside the modelled range is that constraint violation, \
             not the sum rule, got {message:?}"
        );
    }

    // The configurations that DO add up are applied, over both entry counts.
    for (case, name, routing, expected) in [
        (
            "one entry at the total",
            "ADDS-UP-ONE",
            vec![route(&first, MAX_WEIGHT)],
            vec![(first.clone(), MAX_WEIGHT)],
        ),
        (
            "two entries adding up to it",
            "ADDS-UP-TWO",
            vec![route(&first, 60), route(&second, 40)],
            vec![(first.clone(), 60), (second.clone(), 40)],
        ),
        (
            "two entries one of which takes nothing",
            "ADDS-UP-ZERO",
            vec![route(&second, MAX_WEIGHT), route(&first, MIN_WEIGHT)],
            vec![(second.clone(), MAX_WEIGHT), (first.clone(), MIN_WEIGHT)],
        ),
    ] {
        let alias = string(
            &emulator
                .ok(
                    "CreateStateMachineAlias",
                    &json!({ "name": name, "routingConfiguration": &routing }),
                )
                .await,
            "stateMachineAliasArn",
        );
        let described = emulator
            .ok(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": &alias }),
            )
            .await;
        assert_eq!(
            reported_routing(&described),
            expected,
            "case {case:?}: expected the configuration as it was given, got {described:?}"
        );
    }
}
