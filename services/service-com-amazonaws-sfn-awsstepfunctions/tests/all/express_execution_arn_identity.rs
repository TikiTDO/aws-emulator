// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY an asynchronous start gives a run, driven over
//! this service's own wire protocol (`awsJson1_0`, `POST /` with `X-Amz-Target:
//! AWSStepFunctions.<Operation>`) through the production `MultiServiceRouter`, so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. the ARN `StartExecution` mints is the WORKFLOW TYPE's own form, read off the
//!    state machine the start named: a run of an EXPRESS workflow is named by the
//!    `express` resource type and carries a trailing invocation id, because "for
//!    EXPRESS workflows, execution names can be reused immediately"
//!    (`StartExecution$name`) and it is that id which tells two runs of one name
//!    apart, while a run of a STANDARD workflow is named by the `execution` resource
//!    type and its name alone, which is what makes the start "idempotent for STANDARD
//!    workflows" (`StartExecution`);
//! 2. an execution ARN whose RESOURCE TYPE an operation does not accept is refused
//!    before any lookup, with the one diagnosis this service words for a resource type
//!    that is not valid where the ARN was handed in — HTTP 400 `InvalidArn` carrying
//!    `Invalid Arn: 'Resource type not valid in this context: <type>'`, naming the type
//!    the request's ARN carried and nothing else, whichever operation read it.
//!
//! The rows span what each rule generalises over: the workflow type of the state
//! machine (EXPRESS, STANDARD, and a create that names no type at all), whether the
//! start supplies an execution name or asks the service to mint one, the length and
//! character set of that name, a name started twice, which surface minted the ARN
//! (asynchronous and synchronous), an express ARN of a state machine nothing answers
//! to, an express ARN carrying no invocation id, and every operation of this service
//! that reads an execution ARN.
//!
//! The harness (`Emulator`, `create_machine`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds; `tests/all.rs` is this binary's crate root, so it
//! is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EXECUTION_RESOURCE, EXPRESS_EXECUTION_RESOURCE, STATE_MACHINE_TYPE_EXPRESS,
};

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// The modelled refusal class an ARN this service does not serve answers, and its
/// status.
const INVALID_ARN: &str = "InvalidArn";
const CLIENT_REFUSAL_STATUS: u16 = 400;

/// The one diagnosis a resource type that is not valid where the ARN was handed in is
/// refused with.
fn resource_type_refusal(kind: &str) -> String {
    format!("Invalid Arn: 'Resource type not valid in this context: {kind}'")
}

/// Every operation of this service whose input carries an `executionArn`
/// (`smithy_query(query="digest")`: `DescribeExecution`, `DescribeStateMachineForExecution`,
/// `GetExecutionHistory`, `ListMapRuns`, `RedriveExecution`, `StopExecution`).
const EXECUTION_ARN_READERS: &[&str] = &[
    "DescribeExecution",
    "DescribeStateMachineForExecution",
    "GetExecutionHistory",
    "ListMapRuns",
    "RedriveExecution",
    "StopExecution",
];

/// Whether `value` is rendered as a universally unique identifier: the 8-4-4-4-12
/// groups of lowercase hexadecimal that `StartExecution$name`'s own documentation
/// names ("Step Functions automatically generates a universally unique identifier
/// (UUID) as the execution name").
fn is_uuid(value: &str) -> bool {
    /// The characters of each group of the 8-4-4-4-12 form.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// The six colon-delimited fields of `arn`; the last carries the resource's own
/// colons.
fn arn_fields(arn: &str) -> Vec<&str> {
    arn.splitn(6, ':').collect()
}

/// The colon-delimited fields of the RESOURCE portion of `arn`, which a six-field ARN
/// must carry.
fn resource_fields(case: &str, arn: &str) -> Vec<String> {
    let fields = arn_fields(arn);
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected an ARN of six fields, got {arn:?}"
    );
    assert_eq!(
        &fields[..5],
        ["arn", "aws", "states", REGION, ACCOUNT],
        "case {case:?}: expected this account's and region's ARN, got {arn:?}"
    );
    fields[5].split(':').map(str::to_string).collect()
}

/// A state machine of `machine_type`, as its ARN; `None` leaves the member out, which
/// the model defaults to STANDARD.
async fn create_typed(emulator: &Emulator, name: &str, machine_type: Option<&str>) -> String {
    let mut body = json!({ "name": name, "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN });
    if let Some(machine_type) = machine_type {
        body["type"] = json!(machine_type);
    }
    let created = emulator.ok("CreateStateMachine", &body).await;
    string(&created, "stateMachineArn")
}

/// One row of the start table.
struct StartCase {
    case: &'static str,
    /// What the create names as the workflow type, or `None` for a create that names
    /// none.
    machine_type: Option<&'static str>,
    /// The execution name the start supplies, or `None` to have one minted.
    name: Option<String>,
    /// The resource type a run of this workflow type is named by.
    resource_type: &'static str,
    /// Whether the ARN carries a trailing invocation id beside the execution name.
    carries_invocation_id: bool,
}

/// The table: the workflow type crossed with how the run is named.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "a STANDARD run under a supplied name",
            machine_type: Some("STANDARD"),
            name: Some("run-1".to_string()),
            resource_type: EXECUTION_RESOURCE,
            carries_invocation_id: false,
        },
        StartCase {
            case: "a run of a state machine created with no type, which is STANDARD",
            machine_type: None,
            name: Some("run-1".to_string()),
            resource_type: EXECUTION_RESOURCE,
            carries_invocation_id: false,
        },
        StartCase {
            case: "a STANDARD run whose name the service mints",
            machine_type: Some("STANDARD"),
            name: None,
            resource_type: EXECUTION_RESOURCE,
            carries_invocation_id: false,
        },
        StartCase {
            case: "an EXPRESS run under a supplied name",
            machine_type: Some(STATE_MACHINE_TYPE_EXPRESS),
            name: Some("run-1".to_string()),
            resource_type: EXPRESS_EXECUTION_RESOURCE,
            carries_invocation_id: true,
        },
        StartCase {
            case: "an EXPRESS run whose name the service mints",
            machine_type: Some(STATE_MACHINE_TYPE_EXPRESS),
            name: None,
            resource_type: EXPRESS_EXECUTION_RESOURCE,
            carries_invocation_id: true,
        },
        StartCase {
            // `StartExecution$name` is a `Name` of 1..80 characters, so the shortest
            // and the longest name the model admits are rows of their own.
            case: "an EXPRESS run under a one-character name",
            machine_type: Some(STATE_MACHINE_TYPE_EXPRESS),
            name: Some("x".to_string()),
            resource_type: EXPRESS_EXECUTION_RESOURCE,
            carries_invocation_id: true,
        },
        StartCase {
            case: "a STANDARD run under a name at the modelled maximum",
            machine_type: Some("STANDARD"),
            name: Some("n".repeat(80)),
            resource_type: EXECUTION_RESOURCE,
            carries_invocation_id: false,
        },
        StartCase {
            case: "an EXPRESS run under a name of mixed punctuation",
            machine_type: Some(STATE_MACHINE_TYPE_EXPRESS),
            name: Some("Run_2.three-4".to_string()),
            resource_type: EXPRESS_EXECUTION_RESOURCE,
            carries_invocation_id: true,
        },
    ]
}

/// The ARN a start mints is the workflow type's own form: the resource type and the
/// fields after it follow from the state machine's type, never from how the request
/// was written.
#[tokio::test]
async fn a_start_mints_the_arn_form_of_the_workflow_type_it_ran() {
    let emulator = Emulator::new();

    for (index, row) in start_cases().into_iter().enumerate() {
        let StartCase {
            case,
            machine_type,
            name,
            resource_type,
            carries_invocation_id,
        } = row;
        let machine_name = format!("arn-form-{index}");
        let machine = create_typed(&emulator, &machine_name, machine_type).await;

        let mut body = json!({ "stateMachineArn": machine.clone() });
        if let Some(name) = &name {
            body["name"] = json!(name);
        }
        let started = emulator.ok("StartExecution", &body).await;
        assert!(
            started.get("startDate").is_some(),
            "case {case:?}: a start reports the date it began, got {started:?}"
        );
        let arn = string(&started, "executionArn");

        let fields = resource_fields(case, &arn);
        let expected_width = if carries_invocation_id { 4 } else { 3 };
        assert_eq!(
            fields.len(),
            expected_width,
            "case {case:?}: expected {expected_width} resource fields, got {arn:?}"
        );
        assert_eq!(
            fields[0], resource_type,
            "case {case:?}: expected the {resource_type} resource type, got {arn:?}"
        );
        assert_eq!(
            fields[1], machine_name,
            "case {case:?}: the ARN names the state machine the start ran, got {arn:?}"
        );
        match &name {
            Some(name) => assert_eq!(
                &fields[2], name,
                "case {case:?}: the ARN names the execution name the start supplied, got {arn:?}"
            ),
            None => assert!(
                is_uuid(&fields[2]),
                "case {case:?}: a name the service mints is a UUID, got {:?} in {arn:?}",
                fields[2]
            ),
        }
        if carries_invocation_id {
            assert!(
                is_uuid(&fields[3]),
                "case {case:?}: the invocation id is a UUID, got {:?} in {arn:?}",
                fields[3]
            );
        }

        // The ARN is not a label on nothing: the store the service answers from holds
        // the run under exactly it, belonging to the state machine that was started.
        let store = emulator.store();
        let store = store.lock();
        let record = store
            .executions
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case:?}: expected the store to hold {arn}"));
        assert_eq!(
            record.state_machine_arn, machine,
            "case {case:?}: the record belongs to the state machine it ran"
        );
        assert_eq!(
            record.name, fields[2],
            "case {case:?}: the stored name is the one the ARN carries"
        );
    }
}

/// The consequence of rule 1 on a name started twice: a STANDARD start is idempotent
/// under one name, so it answers the SAME ARN, while an EXPRESS start is another run
/// that its own invocation id tells apart.
#[tokio::test]
async fn a_name_started_twice_follows_the_workflow_type_of_its_state_machine() {
    let emulator = Emulator::new();

    // (case, the workflow type, whether the second start answers the first's ARN)
    let rows: &[(&str, &str, bool)] = &[
        ("a STANDARD state machine", "STANDARD", true),
        (
            "an EXPRESS state machine",
            STATE_MACHINE_TYPE_EXPRESS,
            false,
        ),
    ];

    for (case, machine_type, same_arn) in rows {
        let machine_name = format!("twice-{}", machine_type.to_lowercase());
        let machine = create_typed(&emulator, &machine_name, Some(machine_type)).await;
        let body = json!({ "stateMachineArn": machine.clone(), "name": "taken", "input": "{}" });

        let first = string(&emulator.ok("StartExecution", &body).await, "executionArn");
        let second = string(&emulator.ok("StartExecution", &body).await, "executionArn");

        if *same_arn {
            assert_eq!(
                first, second,
                "case {case:?}: a repeated start of one name and input answers the same run"
            );
            continue;
        }
        assert_ne!(
            first, second,
            "case {case:?}: a reused name is a run of its own, got {first:?} twice"
        );
        let (first_fields, second_fields) = (
            resource_fields(case, &first),
            resource_fields(case, &second),
        );
        assert_eq!(
            first_fields[..3],
            second_fields[..3],
            "case {case:?}: both runs are named by the same state machine and name, got \
             {first:?} and {second:?}"
        );
        assert_ne!(
            first_fields[3], second_fields[3],
            "case {case:?}: the invocation id is what tells the two runs apart, got {first:?} \
             and {second:?}"
        );
    }
}

/// Every operation that reads an execution ARN refuses the `express` resource type the
/// same way, whichever surface minted the ARN and whether or not anything answers to
/// it — the ARN is judged before the lookup — while a STANDARD execution ARN reaches
/// that lookup.
#[tokio::test]
async fn an_express_resource_type_is_refused_by_every_operation_that_reads_an_execution_arn() {
    let emulator = Emulator::new();
    let express = create_typed(&emulator, "express-reads", Some(STATE_MACHINE_TYPE_EXPRESS)).await;
    let asynchronous = string(
        &emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": express.clone(), "name": "async-run" }),
            )
            .await,
        "executionArn",
    );
    let synchronous = string(
        &emulator
            .ok(
                "StartSyncExecution",
                &json!({ "stateMachineArn": express.clone(), "name": "sync-run" }),
            )
            .await,
        "executionArn",
    );
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // Rows differ along WHERE the express ARN came from and whether this account holds
    // anything under it: the resource type decides the refusal on its own.
    let forms: Vec<(String, String)> = vec![
        (
            "the ARN an asynchronous start minted".to_string(),
            asynchronous.clone(),
        ),
        (
            "the ARN a synchronous start minted".to_string(),
            synchronous,
        ),
        (
            "an express ARN of a state machine this account does not hold".to_string(),
            format!("{prefix}:{EXPRESS_EXECUTION_RESOURCE}:never-created:run:0e6e0d0e-0000-4000-8000-00000000abcd"),
        ),
        (
            "an express ARN carrying no invocation id".to_string(),
            format!("{prefix}:{EXPRESS_EXECUTION_RESOURCE}:express-reads:async-run"),
        ),
        (
            "an express ARN naming no run at all".to_string(),
            format!("{prefix}:{EXPRESS_EXECUTION_RESOURCE}"),
        ),
    ];

    let expected = resource_type_refusal(EXPRESS_EXECUTION_RESOURCE);
    for (form, arn) in &forms {
        for operation in EXECUTION_ARN_READERS {
            let case = format!("{form} on {operation}");
            let (status, answered) = emulator
                .call(operation, &json!({ "executionArn": arn }))
                .await;
            let code = error_code(&answered);
            let message = answered
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                (status, code.as_str(), message),
                (CLIENT_REFUSAL_STATUS, INVALID_ARN, expected.as_str()),
                "case {case:?}: expected the resource-type ARN refusal, got {status} {code} \
                 with {answered:?}"
            );
            assert!(
                !message.contains(operation) && !message.contains(arn.as_str()),
                "case {case:?}: the diagnosis names the resource type alone, got {message:?}"
            );
        }
    }

    // The control: a STANDARD execution ARN is RESOLVED by every one of those
    // operations, so the refusal is about the ARN's resource type and not about the
    // operations — and the run reads back under the ARN its start answered.
    let standard = create_typed(&emulator, "standard-reads", Some("STANDARD")).await;
    let execution = string(
        &emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": standard, "name": "slow-run" }),
            )
            .await,
        "executionArn",
    );
    for operation in EXECUTION_ARN_READERS {
        let (status, answered) = emulator
            .call(operation, &json!({ "executionArn": execution.clone() }))
            .await;
        let code = error_code(&answered);
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            code != INVALID_ARN && !message.contains("Resource type not valid in this context"),
            "case {operation:?}: a STANDARD execution ARN must reach the lookup, got {status} \
             {code} with {answered:?}"
        );
    }
    let described = emulator
        .ok("DescribeExecution", &json!({ "executionArn": execution }))
        .await;
    assert_eq!(
        (
            string(&described, "executionArn"),
            string(&described, "name")
        ),
        (execution.clone(), "slow-run".to_string()),
        "the STANDARD run reads back under the ARN its start answered, got {described:?}"
    );
}

/// The `execution` resource type is what the operations above DO accept, so a
/// STANDARD-form ARN naming a run nothing holds is each operation's own
/// resource-absent error rather than the ARN refusal.
#[tokio::test]
async fn a_standard_execution_arn_nothing_answers_to_is_not_an_arn_refusal() {
    let emulator = Emulator::new();
    create_machine(&emulator, "absent-run", HELLO_DEFINITION).await;
    let absent =
        format!("arn:aws:states:{REGION}:{ACCOUNT}:{EXECUTION_RESOURCE}:absent-run:never-started");

    for operation in EXECUTION_ARN_READERS {
        let (status, answered) = emulator
            .call(operation, &json!({ "executionArn": absent.clone() }))
            .await;
        let code = error_code(&answered);
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_REFUSAL_STATUS, "ExecutionDoesNotExist"),
            "case {operation:?}: an accepted resource type is looked up, got {status} {code} \
             with {answered:?}"
        );
    }
}
