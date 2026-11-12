// SPDX-License-Identifier: CC0-1.0

//! How Braket WORDS the refusal a quantum task no record is held for earns, over HTTP.
//!
//! The rule these suites pin: a door that resolves a quantum task by ARN answers an ARN
//! that is well formed but names no stored task in the service's FIXED sentence for that
//! class — the whole message is constant, interpolating nothing from the request (not the
//! ARN, not the task id, not the account or the region) — so the refusal reads identically
//! whichever task was asked for, however long the value was and wherever the request was
//! signed, and no caller-supplied input of unbounded length reaches the error body.
//!
//! Each suite asserts that over a TABLE spanning the class: the rows differ along the
//! ARN's partition, region, account id and resource-id form (including an id far longer
//! than the one the service mints), along the door that refused, and along the region the
//! request was signed for. The recorded call behind the rule — a `uuid4` task id in the
//! form this emulator mints — is one row.
//!
//! Everything goes through the production router, reusing the harness in
//! `quantum_task_loop.rs` and the absent-ARN table in `arn_syntax.rs`, so what is pinned
//! is what a Braket client sees.

use crate::arn_syntax::ABSENT_QUANTUM_TASK_ARNS;
use crate::quantum_task_loop::{create_body, Braket, Reply};

/// The sentence both quantum-task doors refuse a never-issued task ARN with: fixed text,
/// nothing of the request in it, and no trailing period.
const QUANTUM_TASK_NOT_FOUND: &str = "Quantum task not found";

/// The regions a row is signed for: a fixed sentence is the same sentence wherever the
/// request was signed, which only a table over more than one region can show.
const REGIONS: &[&str] = &["us-west-2", "eu-west-1", "ap-northeast-1"];

/// Well-formed quantum-task ARNs naming no stored task, beyond the forms
/// `arn_syntax.rs` already tables. `(case, value)`.
const MORE_ABSENT_QUANTUM_TASK_ARNS: &[(&str, &str)] = &[
    // The recorded request this rule was filed for: a fresh uuid4 id in another region.
    (
        "a uuid4 id in the form a client generates",
        "arn:aws:braket:us-west-2:000000000000:quantum-task/50f1f85f-4329-4981-8688-cd23b549beae",
    ),
    (
        "an account id that is not this emulator's",
        "arn:aws:braket:us-east-1:123456789012:quantum-task/t-other-account",
    ),
    (
        "an id carrying colons",
        "arn:aws:braket:ap-northeast-1:000000000000:quantum-task/a:b:c",
    ),
];

/// A well-formed quantum-task ARN whose id is far longer than the one the service mints,
/// still inside the modelled `@length 0..256` bound on `QuantumTaskArn`
/// (`smithy_query(query="operation",operation="GetQuantumTask")`): the sentence must not
/// grow with it.
fn long_id_arn() -> String {
    let prefix = "arn:aws:braket:us-east-1:000000000000:quantum-task/";
    let id = "z".repeat(256 - prefix.len());
    let arn = format!("{prefix}{id}");
    assert!(
        arn.len() <= 256,
        "case the long-id row: the ARN must stay inside the modelled 256-character \
         bound, got {}",
        arn.len()
    );
    arn
}

/// Every absent-task row this file asserts over: `(case, value)`.
fn absent_rows() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = ABSENT_QUANTUM_TASK_ARNS
        .iter()
        .chain(MORE_ABSENT_QUANTUM_TASK_ARNS.iter())
        .map(|(case, value)| ((*case).to_string(), (*value).to_string()))
        .collect();
    rows.push((
        "a resource id filling the modelled length bound".to_string(),
        long_id_arn(),
    ));
    rows
}

/// Assert `reply` is the 404 `ResourceNotFoundException` carrying exactly the fixed
/// sentence, with no part of `arn` interpolated into it.
fn assert_fixed_not_found(reply: &Reply, case: &str, arn: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404 for a well-formed ARN naming no task, got {} ({})",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected the modelled ResourceNotFoundException, got {:?} ({})",
        reply.error,
        reply.body
    );
    assert_eq!(
        reply.message(),
        QUANTUM_TASK_NOT_FOUND,
        "case {case:?}: expected the fixed sentence {QUANTUM_TASK_NOT_FOUND:?}, got {:?}",
        reply.message()
    );
    // The sentence interpolates nothing, so no field of the ARN the request carried — the
    // whole ARN, its resource id, its account id or its region — can appear in it. Fields
    // of one or two characters are skipped: a letter of an English sentence is not
    // evidence of interpolation.
    let mut absent: Vec<&str> = vec![arn];
    let fields: Vec<&str> = arn.split(':').collect();
    absent.extend(fields[3..].iter().filter(|field| field.len() > 2));
    if let Some(id) = arn.rsplit('/').next().filter(|id| id.len() > 2) {
        absent.push(id);
    }
    for leaked in absent {
        assert!(
            !reply.message().contains(leaked),
            "case {case:?}: the sentence interpolates nothing, so it must not carry \
             {leaked:?}, got {:?}",
            reply.message()
        );
    }
}

/// `GetQuantumTask` answers every never-issued task ARN with the one fixed sentence, in
/// every region a request can be signed for.
#[tokio::test]
async fn get_quantum_task_answers_one_fixed_not_found_sentence() {
    let braket = Braket::new();

    for (case, arn) in absent_rows() {
        for region in REGIONS {
            let reply = braket
                .call_in_region(
                    region,
                    "GET",
                    &format!("/quantum-task/{}", crate::quantum_task_loop::encode(&arn)),
                    None,
                )
                .await;
            assert_fixed_not_found(&reply, &format!("GetQuantumTask {case} in {region}"), &arn);
        }
    }
}

/// `CancelQuantumTask`, the other door that resolves a quantum task by ARN, answers the
/// SAME class in the SAME fixed sentence — over the same table, so the two cannot drift.
#[tokio::test]
async fn cancel_quantum_task_answers_the_same_fixed_not_found_sentence() {
    let braket = Braket::new();

    let mut index = 0;
    for (case, arn) in absent_rows() {
        for region in REGIONS {
            index += 1;
            let reply = braket
                .call_in_region(
                    region,
                    "PUT",
                    &format!(
                        "/quantum-task/{}/cancel",
                        crate::quantum_task_loop::encode(&arn)
                    ),
                    Some(serde_json::json!({
                        "clientToken": format!("tok-absent-cancel-{index}"),
                    })),
                )
                .await;
            assert_fixed_not_found(
                &reply,
                &format!("CancelQuantumTask {case} in {region}"),
                &arn,
            );
        }
    }
}

/// The fixed sentence is stable and is not the sentence either door answers a MALFORMED
/// label with: a caller branching on the message can still tell a missing task from a
/// value that is not a task ARN, and a repeated call reads the same text.
#[tokio::test]
async fn the_not_found_sentence_is_stable_and_distinct_from_the_malformed_refusals() {
    let braket = Braket::new();
    let arn = "arn:aws:braket:eu-west-1:000000000000:quantum-task/00000000-0000-4000-8000-000000000001";

    for attempt in 1..=3 {
        let read = braket.get_task(arn).await;
        assert_fixed_not_found(&read, &format!("GetQuantumTask attempt {attempt}"), arn);
        let cancel = braket
            .cancel_task(arn, &format!("tok-absent-stable-{attempt}"))
            .await;
        assert_fixed_not_found(&cancel, &format!("CancelQuantumTask attempt {attempt}"), arn);
    }

    // The malformed-label class stays its own refusal, worded per operation.
    let malformed = "not-a-valid-quantum-task-arn";
    let read = braket.get_task(malformed).await;
    assert_eq!(
        read.status, 400,
        "case a malformed label at GetQuantumTask: expected HTTP 400, got {} ({})",
        read.status, read.body
    );
    assert_ne!(
        read.message(),
        QUANTUM_TASK_NOT_FOUND,
        "case a malformed label at GetQuantumTask: a value that is not an ARN is invalid \
         input, not a missing task, got the not-found sentence"
    );
    let cancel = braket.cancel_task(malformed, "tok-absent-malformed").await;
    assert_eq!(
        cancel.status, 400,
        "case a malformed label at CancelQuantumTask: expected HTTP 400, got {} ({})",
        cancel.status, cancel.body
    );
    assert_ne!(
        cancel.message(),
        QUANTUM_TASK_NOT_FOUND,
        "case a malformed label at CancelQuantumTask: a value that is not an ARN is \
         invalid input, not a missing task, got the not-found sentence"
    );
}

/// The refusal is taken from the STORE, not from the shape of the value: a task the
/// service minted reads back and cancels, and the same ARN with one character of its id
/// changed — just as well formed — is the fixed refusal.
#[tokio::test]
async fn a_stored_task_still_answers_and_only_its_absent_neighbour_is_refused() {
    let braket = Braket::new();
    let device_arn = braket.an_online_device().await;

    let created = braket
        .create_task(create_body("tok-absent-roundtrip", &device_arn, 1))
        .await;
    assert_eq!(
        created.status, 201,
        "case a created task: expected HTTP 201, got {} ({})",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");

    let read = braket.get_task(&arn).await;
    assert_eq!(
        read.status, 200,
        "case the created task {arn}: expected HTTP 200 from GetQuantumTask, got {} ({})",
        read.status, read.body
    );
    assert_eq!(
        read.str_member("quantumTaskArn"),
        arn,
        "case the created task {arn}: expected the ARN to read back, got {}",
        read.body
    );

    // One character of the minted id changed: the same grammar, no stored task.
    let last = arn.chars().next_back().expect("a minted ARN is not empty");
    let neighbour = format!(
        "{}{}",
        &arn[..arn.len() - last.len_utf8()],
        if last == '0' { '1' } else { '0' }
    );
    assert_ne!(
        neighbour, arn,
        "case the neighbour of {arn}: the row must name a different task"
    );
    let miss = braket.get_task(&neighbour).await;
    assert_fixed_not_found(
        &miss,
        &format!("GetQuantumTask on the absent neighbour of {arn}"),
        &neighbour,
    );

    // And the stored task is still cancellable: the not-found branch was not taken for it.
    let cancelled = braket.cancel_task(&arn, "tok-absent-roundtrip-cancel").await;
    assert_eq!(
        cancelled.status, 200,
        "case cancelling the created task {arn}: expected HTTP 200, got {} ({})",
        cancelled.status, cancelled.body
    );
}
