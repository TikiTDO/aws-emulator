// SPDX-License-Identifier: CC0-1.0

//! A quantum task is EXECUTED: its program runs, its result document is written where a
//! client reads it, and the task reports what that run produced.
//!
//! The rule these suites pin: `CreateQuantumTask` on an online gate-model device runs the
//! program its `action` carries and writes the run's result document under the task's
//! `outputS3Directory` in the caller's bucket, so `GetQuantumTask` then reports the task
//! `COMPLETED` with one measurement per shot readable from Amazon S3; a request the
//! service cannot serve — an output bucket S3 holds no bucket for, an `action` that is not
//! a Braket IR document, a `shots` value outside the device's published range — is
//! refused with `ValidationException` (400) before any task is minted; and a task that
//! has reached a terminal status can no longer be cancelled (`ConflictException`, 409).
//!
//! Each suite asserts that over a TABLE spanning what the rule generalises over: the
//! circuit (its gates, its qubit count, which qubits it measures), the shot count, and
//! the member of the request that is out of range. The recorded Bell circuit at 100 shots
//! is ONE row of the first table.
//!
//! Everything goes over HTTP through the production router, with Amazon S3 — the owner of
//! the output location — mounted beside this service on the same engine, so a result
//! object exists here only because S3 answered for it.

use std::time::Duration;

use serde_json::{json, Value};

use crate::quantum_task_loop::{member, Braket, Reply};

/// The bucket the suites write their results to, named as Amazon Braket's own
/// documentation names an output bucket.
const BUCKET: &str = "amazon-braket-emulator-results";

/// The key prefix every create in these suites asks for.
const PREFIX: &str = "results";

/// The managed state-vector simulator of the seeded inventory — the device the recorded
/// run used, and the one these suites submit to.
const SV1: &str = "arn:aws:braket:::device/quantum-simulator/amazon/sv1";

/// The tensor-network simulator, whose published `shotsRange` starts at 1 rather than 0:
/// the device a shot count BELOW the range is asserted against.
const TN1: &str = "arn:aws:braket:::device/quantum-simulator/amazon/tn1";

/// The result object a finished task's results are read from, under its
/// `outputS3Directory`.
const RESULTS_OBJECT: &str = "results.json";

/// The `braketSchemaHeader` name of a gate-model result document.
const RESULT_SCHEMA: &str = "braket.task_result.gate_model_task_result";

/// An OpenQASM program action document, as a client sends it.
fn openqasm_action(source: &str) -> String {
    json!({
        "braketSchemaHeader": { "name": "braket.ir.openqasm.program", "version": "1" },
        "source": source,
    })
    .to_string()
}

/// The body of a `CreateQuantumTask` call, with every required member filled in.
fn create_body(
    client_token: &str,
    device_arn: &str,
    shots: i64,
    bucket: &str,
    action: &str,
) -> Value {
    json!({
        "clientToken": client_token,
        "deviceArn": device_arn,
        "shots": shots,
        "outputS3Bucket": bucket,
        "outputS3KeyPrefix": PREFIX,
        "action": action,
    })
}

/// Create the suite's bucket through Amazon S3's own API.
///
/// The harness signs for a Region outside `us-east-1`, which S3 serves a create for only
/// with the matching location constraint.
async fn create_bucket(s3: &::aws_sdk_s3::Client) {
    s3.create_bucket()
        .bucket(BUCKET)
        .create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(
                    crate::quantum_task_loop::REGION,
                ))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {BUCKET} must be created, got {error:?}"));
}

/// The body of one object, or `None` when the object store serves no such object.
async fn object_body(s3: &::aws_sdk_s3::Client, key: &str) -> Option<String> {
    let object = s3.get_object().bucket(BUCKET).key(key).send().await.ok()?;
    let bytes = object.body.collect().await.ok()?.into_bytes();
    String::from_utf8(bytes.to_vec()).ok()
}

/// Poll `GetQuantumTask` until the task reaches a terminal status, and answer what it
/// then reports.
///
/// The terminal transition is owed by the clock, so the poll waits it out rather than
/// sleeping for a fixed span; a task that never gets there fails the suite instead of
/// hanging it.
async fn poll_until_terminal(braket: &Braket, arn: &str) -> Value {
    for _ in 0..100 {
        let reply = braket.get_task(arn).await;
        assert_eq!(
            reply.status, 200,
            "case poll {arn}: expected 200, got {} {}",
            reply.status, reply.body
        );
        if matches!(
            member(&reply.body, "status"),
            "COMPLETED" | "FAILED" | "CANCELLED"
        ) {
            return reply.body;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("case poll {arn}: the task never reached a terminal status");
}

/// What the measurements of a run must satisfy, which is what each row of the table
/// below differs along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// Every shot reports exactly this row.
    Exactly(&'static [u8]),
    /// Every shot reports a row whose bits all agree, and over the run both the all-zero
    /// and the all-one outcome appear — the entangled case.
    Correlated,
}

/// A run writes one measurement per shot, over the whole class of gate-model circuits:
/// a circuit whose outcome is certain reports it on every shot, and an entangled one
/// reports only the outcomes its state allows.
#[tokio::test]
async fn a_quantum_task_runs_its_program_and_writes_its_measurements() {
    let (braket, s3) = Braket::with_s3();
    create_bucket(&s3).await;

    // (case, source, shots, measured qubits, what the shots must satisfy)
    let cases: Vec<(&str, &str, i64, Vec<u64>, Expected)> = vec![
        (
            // The recorded run: a two-qubit Bell circuit at 100 shots.
            "a Bell circuit at 100 shots",
            "OPENQASM 3.0; bit[2] b; qubit[2] q; h q[0]; cnot q[0], q[1]; \
             b[0] = measure q[0]; b[1] = measure q[1];",
            100,
            vec![0, 1],
            Expected::Correlated,
        ),
        (
            "a one-qubit circuit at one shot",
            "OPENQASM 3.0; qubit[1] q; x q[0];",
            1,
            vec![0],
            Expected::Exactly(&[1]),
        ),
        (
            "a three-qubit GHZ circuit",
            "OPENQASM 3.0; qubit[3] q; h q[0]; cnot q[0], q[1]; cnot q[1], q[2];",
            64,
            vec![0, 1, 2],
            Expected::Correlated,
        ),
        (
            "a circuit of rotations only",
            "OPENQASM 3.0; qubit[2] q; rx(pi) q[0]; ry(2 * pi) q[1];",
            5,
            vec![0, 1],
            Expected::Exactly(&[1, 0]),
        ),
        (
            "a circuit that measures one of its qubits",
            "OPENQASM 3.0; bit b; qubit[2] q; x q[1]; b = measure q[1];",
            7,
            vec![1],
            Expected::Exactly(&[1]),
        ),
    ];

    for (index, (case, source, shots, measured_qubits, expected)) in cases.iter().enumerate() {
        let created = braket
            .create_task(create_body(
                &format!("exec-token-{index}"),
                SV1,
                *shots,
                BUCKET,
                &openqasm_action(source),
            ))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let task = poll_until_terminal(&braket, &arn).await;
        assert_eq!(
            member(&task, "status"),
            "COMPLETED",
            "case {case}: expected COMPLETED, got {task}"
        );
        assert!(
            task.get("failureReason").is_none(),
            "case {case}: a completed task carries no failureReason, got {task}"
        );
        assert_eq!(
            task.get("numSuccessfulShots").and_then(Value::as_i64),
            Some(*shots),
            "case {case}: expected {shots} successful shots, got {task}"
        );

        // The results live where the task says they do, in the caller's own bucket.
        let directory = member(&task, "outputS3Directory").to_string();
        assert_eq!(
            member(&task, "outputS3Bucket"),
            BUCKET,
            "case {case}: expected the bucket the create named, got {task}"
        );
        let key = format!("{directory}/{RESULTS_OBJECT}");
        let body = object_body(&s3, &key)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected a result object at {key}"));
        let document: Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("case {case}: the result object is JSON: {error}"));
        assert_eq!(
            document["braketSchemaHeader"]["name"].as_str(),
            Some(RESULT_SCHEMA),
            "case {case}: expected the gate-model result schema, got {document}"
        );
        assert_eq!(
            document["measuredQubits"],
            json!(measured_qubits),
            "case {case}: expected the measured qubits {measured_qubits:?}, got {}",
            document["measuredQubits"]
        );
        assert_eq!(
            document["taskMetadata"]["id"].as_str(),
            Some(arn.as_str()),
            "case {case}: expected the task's own ARN in its metadata, got {}",
            document["taskMetadata"]
        );
        assert_eq!(
            document["taskMetadata"]["deviceId"].as_str(),
            Some(SV1),
            "case {case}: expected the device that ran it, got {}",
            document["taskMetadata"]
        );
        let rows: Vec<Vec<u64>> = document["measurements"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: measurements is a list, got {document}"))
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap_or_else(|| panic!("case {case}: a measurement row is a list"))
                    .iter()
                    .map(|bit| {
                        bit.as_u64()
                            .unwrap_or_else(|| panic!("case {case}: a measured bit is a number"))
                    })
                    .collect()
            })
            .collect();
        assert_eq!(
            rows.len(),
            usize::try_from(*shots).expect("a shot count fits a usize"),
            "case {case}: expected one row per shot, got {} rows",
            rows.len()
        );
        for row in &rows {
            assert_eq!(
                row.len(),
                measured_qubits.len(),
                "case {case}: expected one bit per measured qubit, got {row:?}"
            );
            assert!(
                row.iter().all(|bit| *bit <= 1),
                "case {case}: expected measured bits, got {row:?}"
            );
        }
        match expected {
            Expected::Exactly(wanted) => {
                let wanted: Vec<u64> = wanted.iter().map(|bit| u64::from(*bit)).collect();
                for row in &rows {
                    assert_eq!(
                        row, &wanted,
                        "case {case}: expected every shot to report {wanted:?}, got {row:?}"
                    );
                }
            }
            Expected::Correlated => {
                let mut zeros = 0;
                let mut ones = 0;
                for row in &rows {
                    assert!(
                        row.iter().all(|bit| *bit == row[0]),
                        "case {case}: expected correlated bits, got {row:?}"
                    );
                    if row[0] == 0 {
                        zeros += 1;
                    } else {
                        ones += 1;
                    }
                }
                assert!(
                    zeros > 0 && ones > 0,
                    "case {case}: expected both outcomes over {shots} shots, got {zeros} \
                     all-zero and {ones} all-one"
                );
            }
        }
    }
}

/// A program the simulator cannot run ends its task in the modelled failure status
/// carrying the reason — never in `COMPLETED`, and never by taking the emulator down.
///
/// The rows span the ways a program can be unrunnable: text past the length the service
/// reads, nesting past the constants it folds (in brackets and spelled without them), a
/// state larger than the simulator holds, a statement form it does not run, and a gate it
/// has no matrix for.
#[tokio::test]
async fn a_program_the_simulator_cannot_run_fails_its_task_with_the_reason() {
    let (braket, s3) = Braket::with_s3();
    create_bucket(&s3).await;

    // (case, source, a fragment of the reason the task must carry)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "program text past the length the service reads",
            format!("OPENQASM 3.0; qubit[1] q;{}", " ".repeat(16_384)),
            "more than the",
        ),
        (
            "a prefix-operator chain nested without brackets",
            format!("OPENQASM 3.0; qubit[1] q; rx({}1) q[0];", "-".repeat(512)),
            "not a constant the emulator could evaluate",
        ),
        (
            "a binary-operator chain nested without brackets",
            format!("OPENQASM 3.0; qubit[1] q; rx(1{}) q[0];", "+1".repeat(512)),
            "not a constant the emulator could evaluate",
        ),
        (
            "brackets nested deeper than the constants it folds",
            format!(
                "OPENQASM 3.0; qubit[1] q; rx({}1{}) q[0];",
                "(".repeat(256),
                ")".repeat(256)
            ),
            "not a constant the emulator could evaluate",
        ),
        (
            "a state larger than the simulator holds",
            "OPENQASM 3.0; qubit[40] q; h q[0];".to_string(),
            "qubits this emulator's simulator holds",
        ),
        (
            "a statement form the simulator does not run",
            "OPENQASM 3.0; qubit[1] q; for int i in [0:1] { h q[0]; }".to_string(),
            "does not run the OpenQASM statement",
        ),
        (
            "a gate the simulator has no matrix for",
            "OPENQASM 3.0; qubit[1] q; frobnicate q[0];".to_string(),
            "does not apply the gate",
        ),
    ];

    for (index, (case, source, fragment)) in cases.iter().enumerate() {
        let created = braket
            .create_task(create_body(
                &format!("unrunnable-token-{index}"),
                SV1,
                4,
                BUCKET,
                &openqasm_action(source),
            ))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: the create is accepted, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("quantumTaskArn");
        let task = poll_until_terminal(&braket, &arn).await;
        assert_eq!(
            member(&task, "status"),
            "FAILED",
            "case {case}: expected the modelled failure status, got {task}"
        );
        assert!(
            member(&task, "failureReason").contains(fragment),
            "case {case}: expected a reason carrying {fragment:?}, got {:?}",
            member(&task, "failureReason")
        );
        assert!(
            task.get("numSuccessfulShots").is_none(),
            "case {case}: a task that produced no result reports no successful shots, \
             got {task}"
        );
    }

    // The emulator is still serving: the control request after the table completes.
    let after = braket
        .create_task(create_body(
            "unrunnable-control",
            SV1,
            4,
            BUCKET,
            &openqasm_action("OPENQASM 3.0; qubit[1] q; x q[0];"),
        ))
        .await;
    assert_eq!(
        after.status, 201,
        "case the control after the table: expected 201, got {} {}",
        after.status, after.body
    );
    let task = poll_until_terminal(&braket, &after.str_member("quantumTaskArn")).await;
    assert_eq!(
        member(&task, "status"),
        "COMPLETED",
        "case the control after the table: expected COMPLETED, got {task}"
    );
}

/// A request the service cannot serve is refused with `ValidationException` (400) and the
/// message of the member that failed, and NO task is minted for it.
#[tokio::test]
async fn a_create_the_service_cannot_serve_is_refused_before_a_task_is_minted() {
    let (braket, s3) = Braket::with_s3();
    create_bucket(&s3).await;
    let bell = openqasm_action(
        "OPENQASM 3.0; bit[2] b; qubit[2] q; h q[0]; cnot q[0], q[1]; \
         b[0] = measure q[0]; b[1] = measure q[1];",
    );

    // (case, device, shots, bucket, action, the message the refusal carries)
    let cases: Vec<(&str, &str, i64, &str, String, String)> = vec![
        (
            // The recorded request: a syntactically valid bucket nobody created.
            "an output bucket Amazon S3 holds no bucket for",
            SV1,
            100,
            "amazon-braket-never-created",
            bell.clone(),
            "Caller doesn't have access to bucket or it doesn't exist. Learn more on our \
             developer guide: \
             https://docs.aws.amazon.com/braket/latest/developerguide/braket-slr.html"
                .to_string(),
        ),
        (
            "an action that is not a Braket IR document",
            SV1,
            100,
            BUCKET,
            json!({ "foo": "bar" }).to_string(),
            "Provided action is not valid. Please refer to the github repo \
             amazon-braket-schemas-python for examples"
                .to_string(),
        ),
        (
            "an action document carrying no schema header",
            SV1,
            100,
            BUCKET,
            json!({ "source": "OPENQASM 3.0; qubit[1] q;" }).to_string(),
            "Provided action is not valid. Please refer to the github repo \
             amazon-braket-schemas-python for examples"
                .to_string(),
        ),
        (
            "a shot count above the device's published maximum",
            SV1,
            100_001,
            BUCKET,
            bell.clone(),
            format!("Shot count should be between 0 and 100000 on {SV1}, but found 100001."),
        ),
        (
            "a shot count below the device's published minimum",
            TN1,
            0,
            BUCKET,
            bell.clone(),
            format!("Shot count should be between 1 and 1000 on {TN1}, but found 0."),
        ),
    ];

    for (index, (case, device, shots, bucket, action, expected)) in cases.iter().enumerate() {
        let reply = braket
            .create_task(create_body(
                &format!("refused-token-{index}"),
                device,
                *shots,
                bucket,
                action,
            ))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            reply.error
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the message {expected:?}, got {:?}",
            reply.message()
        );
        assert!(
            reply.body.get("quantumTaskArn").is_none(),
            "case {case}: a refused create mints no task, got {}",
            reply.body
        );
    }

    // The positive control: the same request with every member in range is accepted.
    let accepted = braket
        .create_task(create_body("accepted-token", SV1, 10, BUCKET, &bell))
        .await;
    assert_eq!(
        accepted.status, 201,
        "case the control request: expected 201, got {} {}",
        accepted.status, accepted.body
    );
}

/// What may be cancelled depends on the status the task is in: a task still waiting is
/// accepted for cancellation, and one that has reached a terminal status is refused with
/// `ConflictException` (409) naming that status and the device that ran it.
#[tokio::test]
async fn a_task_in_a_terminal_status_can_no_longer_be_cancelled() {
    let (braket, s3) = Braket::with_s3();
    create_bucket(&s3).await;
    let action = openqasm_action("OPENQASM 3.0; qubit[2] q; h q[0]; cnot q[0], q[1];");

    // A task that has just been created is still cancellable: the positive control.
    let pending = braket
        .create_task(create_body("cancel-pending", SV1, 10, BUCKET, &action))
        .await;
    let pending_arn = pending.str_member("quantumTaskArn");
    let cancelled = braket.cancel_task(&pending_arn, "cancel-token-a").await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel a pending task: expected 200, got {} {}",
        cancelled.status, cancelled.body
    );
    assert!(
        matches!(
            member(&cancelled.body, "cancellationStatus"),
            "CANCELLING" | "CANCELLED"
        ),
        "case cancel a pending task: expected a cancellation status, got {}",
        cancelled.body
    );

    // One that ran to completion is not: there is nothing left to cancel.
    let completed = braket
        .create_task(create_body("cancel-completed", SV1, 10, BUCKET, &action))
        .await;
    let completed_arn = completed.str_member("quantumTaskArn");
    let task = poll_until_terminal(&braket, &completed_arn).await;
    assert_eq!(
        member(&task, "status"),
        "COMPLETED",
        "case setup: the task must have completed, got {task}"
    );
    let conflict = braket.cancel_task(&completed_arn, "cancel-token-b").await;
    assert_eq!(
        conflict.status, 409,
        "case cancel a completed task: expected 409, got {} {}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error.as_deref(),
        Some("ConflictException"),
        "case cancel a completed task: expected ConflictException, got {:?}",
        conflict.error
    );
    let expected = format!(
        "Amazon Braket cannot cancel a quantum task in the COMPLETED status for device ARN \
         {SV1}."
    );
    assert_eq!(
        conflict.message(),
        expected,
        "case cancel a completed task: expected {expected:?}, got {:?}",
        conflict.message()
    );
    // And the refusal changed nothing: the task is still the one that completed.
    let after = braket.get_task(&completed_arn).await;
    assert_eq!(
        member(&after.body, "status"),
        "COMPLETED",
        "case after the refused cancel: expected the status to stand, got {}",
        after.body
    );
}

/// A well-formed ARN no task is held for is reported as a missing resource, with the
/// message Amazon Braket words that class with — on every quantum-task door that takes
/// one.
#[tokio::test]
async fn a_well_formed_arn_no_task_is_held_for_is_a_missing_resource() {
    let (braket, _s3) = Braket::with_s3();
    let ghosts = [
        "arn:aws:braket:eu-west-1:000000000000:quantum-task/11111111-1111-4111-8111-111111111111",
        "arn:aws:braket:::quantum-task/never-created",
    ];
    for ghost in ghosts {
        let read: Reply = braket.get_task(ghost).await;
        let cancel: Reply = braket.cancel_task(ghost, "ghost-token").await;
        for (operation, reply) in [("GetQuantumTask", read), ("CancelQuantumTask", cancel)] {
            assert_eq!(
                reply.status, 404,
                "case {operation} on {ghost}: expected 404, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation} on {ghost}: expected ResourceNotFoundException, got {:?}",
                reply.error
            );
            assert_eq!(
                reply.message(),
                "Quantum task not found",
                "case {operation} on {ghost}: expected the service's own sentence, got {:?}",
                reply.message()
            );
        }
    }
}
