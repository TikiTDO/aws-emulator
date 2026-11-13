// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the state machine ARN a SYNCHRONOUS Express run reports, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `StartSyncExecutionOutput$stateMachineArn` is "the Amazon
//! Resource Name (ARN) that identifies the state machine" the run was started through —
//! the QUALIFIED reference the request named, so the alias that routed the run or the
//! version that served it, and the unqualified ARN only for a run started with one. The
//! output declares no member for the alias or the version
//! (`smithy_query(query="operation",operation="StartSyncExecution")`), so resolving the
//! reference down to the state machine would lose the routing of the very run it
//! reports.
//!
//! The rows differ along the FORM of the ARN the run was started with — an alias, a
//! version, the state machine itself — and, for an alias, along the routing
//! configuration it carries, which is what decides the version it draws.
//!
//! The harness (`Emulator`, `string`) is the one `state_machine_lifecycle` builds;
//! `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{string, Emulator, ROLE_ARN};

/// A definition an EXPRESS workflow can run to a terminal status inside the one call.
const DONE: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"ran":1},"End":true}}}"#;

/// A second such definition, so an update really changes what the state machine holds
/// and the version published from it is a new one.
const DONE_AGAIN: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"ran":2},"End":true}}}"#;

/// Create an EXPRESS state machine, answering its ARN.
async fn express_machine(emulator: &Emulator, name: &str, definition: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// Publish the state machine's current configuration as its next version, answering that
/// version's ARN.
async fn publish(emulator: &Emulator, machine: &str) -> String {
    let published = emulator
        .ok(
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": machine }),
        )
        .await;
    string(&published, "stateMachineVersionArn")
}

/// Create an alias over `routing`, answering its ARN.
async fn create_alias(emulator: &Emulator, name: &str, routing: Value) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({ "name": name, "routingConfiguration": routing }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

/// A synchronous Express run reports the ARN it was STARTED with, qualified exactly as
/// the request named it.
#[tokio::test]
async fn a_sync_run_reports_the_qualified_arn_it_was_started_with() {
    let emulator = Emulator::new();
    let machine = express_machine(&emulator, "sync-qualified", DONE).await;
    let first = publish(&emulator, &machine).await;
    emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine, "definition": DONE_AGAIN }),
        )
        .await;
    let second = publish(&emulator, &machine).await;

    // One alias routing all of its traffic to the first version, one routing all of it
    // to the second, and one naming both with the whole weight on the second — so the
    // reference a run reports is not the version it happens to draw.
    let to_first = create_alias(
        &emulator,
        "LIVE",
        json!([{ "stateMachineVersionArn": first, "weight": 100 }]),
    )
    .await;
    let to_second = create_alias(
        &emulator,
        "NEXT",
        json!([{ "stateMachineVersionArn": second, "weight": 100 }]),
    )
    .await;
    let staged = create_alias(
        &emulator,
        "staged",
        json!([
            { "stateMachineVersionArn": first, "weight": 0 },
            { "stateMachineVersionArn": second, "weight": 100 },
        ]),
    )
    .await;

    // (case, the ARN the run is started with, the ARN it must report)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "an alias routing every run to the first version",
            &to_first,
            &to_first,
        ),
        (
            "an alias routing every run to the second version",
            &to_second,
            &to_second,
        ),
        (
            "an alias whose configuration names two versions",
            &staged,
            &staged,
        ),
        ("a version ARN, which names the version it runs", &first, &first),
        (
            "the unqualified ARN, which is associated with no version",
            &machine,
            &machine,
        ),
    ];
    for (index, (case, started_with, expected)) in rows.into_iter().enumerate() {
        let answered = emulator
            .ok(
                "StartSyncExecution",
                &json!({
                    "stateMachineArn": started_with,
                    "name": format!("sync-{index}"),
                    "input": "{}",
                }),
            )
            .await;
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the run to succeed, got {answered:?}"
        );
        let reported = string(&answered, "stateMachineArn");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the run to report {expected:?}, got {reported:?}"
        );
        // The execution ARN is the run's OWN — a synchronous Express run is reached by
        // an ARN of its own resource type — so the qualified ARN above is about the
        // state machine reference and nothing else.
        let execution = string(&answered, "executionArn");
        assert!(
            execution.contains(":express:"),
            "case {case:?}: expected a synchronous Express execution ARN, got {execution:?}"
        );
    }
}
