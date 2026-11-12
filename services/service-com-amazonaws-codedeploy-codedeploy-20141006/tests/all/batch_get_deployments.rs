// SPDX-License-Identifier: CC0-1.0

//! `BatchGetDeployments`, driven over the wire.
//!
//! RULE under test: `BatchGetDeployments` reports, for each deployment id the
//! request names, exactly the stored record that the single-id reader
//! (`GetDeployment`) reports for that id, in the order the request named them,
//! omitting the ids this store holds no deployment for; and it refuses a request
//! that carries more ids than the shared request-size quota its
//! `BatchLimitExceededException` states (100), one that carries no
//! id at all (or an empty one), and one that carries a value that is not shaped
//! like a deployment id — each with its own modelled code.
//!
//! The harness (router, `error_code`, `s3_revision`, `seeded_group`) is reused
//! from `codedeploy_lifecycle`, so every case here goes through the same
//! `X-Amz-Target` front door a client uses.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{
    deployment_id_on_free_group, error_code, s3_revision, seeded_group, Emulator, POLL_DEADLINE,
};

/// The request-size quota one `BatchGet*` request may carry, which is the one
/// `BatchLimitExceededException` states for every batch reader of this service
/// ("The maximum number of names or IDs allowed for this request (100) was
/// exceeded") and the one `crate::state::MAX_BATCH_SIZE` holds.
const MAX_BATCH_SIZE: usize = 100;

/// The `deploymentId` of each entry of a `deploymentsInfo` answer, in order.
fn reported_ids(body: &Value) -> Vec<String> {
    body["deploymentsInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|info| {
            info["deploymentId"]
                .as_str()
                .expect("every answered entry carries its deploymentId")
                .to_string()
        })
        .collect()
}

/// Create `count` deployments on one freshly seeded group, answering their ids
/// in creation order.
///
/// The group runs one deployment at a time, so each is created once the one
/// before it is terminal ([`deployment_id_on_free_group`]).
async fn seeded_deployments(emulator: &Emulator, application: &str, count: usize) -> Vec<String> {
    let group = seeded_group(emulator, application, "Server").await;
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            deployment_id_on_free_group(
                emulator,
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "revision": s3_revision("my-bucket", &format!("app-{index}.zip")),
                    "description": format!("deployment {index}"),
                }),
            )
            .await,
        );
    }
    ids
}

/// Wait for every id to reach a clock-owed terminal status, so a later read is
/// repeatable and can be compared field by field.
///
/// Which outcome a deployment reaches is the store's business (a deployment whose
/// group resolves no targets ends `Failed`, one that has targets `Succeeded`), so
/// this waits for ANY of the statuses `DeploymentStatus` models as outcomes
/// rather than for one of them.
async fn await_terminal(emulator: &Emulator, ids: &[String]) {
    // `smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
    // InProgress | Baking | Succeeded | Failed | Stopped | Ready — the last three
    // of those are the outcomes.
    const TERMINAL: [&str; 3] = ["Succeeded", "Failed", "Stopped"];
    let deadline = Instant::now() + POLL_DEADLINE;
    for id in ids {
        loop {
            let polled = emulator
                .ok("GetDeployment", json!({"deploymentId": id}))
                .await;
            let status = polled["deploymentInfo"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if TERMINAL.contains(&status.as_str()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case waiting for {id}: expected one of {TERMINAL:?} within {POLL_DEADLINE:?}, \
                 got {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// RULE: the batch reader answers one entry per named id, in request order,
/// omitting the ids the store holds no deployment for.
#[tokio::test]
async fn a_batch_answers_one_entry_per_named_id_in_request_order() {
    let emulator = Emulator::new();
    let ids = seeded_deployments(&emulator, "batched", 3).await;
    await_terminal(&emulator, &ids).await;
    // A well-formed id of a deployment nobody created: the store holds nothing
    // for it, and this operation models no does-not-exist error.
    let unknown = "d-ZZZZZZZZZ".to_string();

    // The rows differ along COUNT, ORDER, repetition and the presence of ids the
    // store does not hold — what the rule generalises over.
    let cases: &[(&str, Vec<String>, Vec<String>)] = &[
        ("one id", vec![ids[0].clone()], vec![ids[0].clone()]),
        ("every id, creation order", ids.clone(), ids.clone()),
        (
            "every id, reversed",
            ids.iter().rev().cloned().collect(),
            ids.iter().rev().cloned().collect(),
        ),
        (
            "the same id twice",
            vec![ids[1].clone(), ids[1].clone()],
            vec![ids[1].clone(), ids[1].clone()],
        ),
        (
            "an unknown id between two known ones",
            vec![ids[0].clone(), unknown.clone(), ids[2].clone()],
            vec![ids[0].clone(), ids[2].clone()],
        ),
        ("only an unknown id", vec![unknown.clone()], Vec::new()),
        (
            // Sizes the quota accepts: the batch the operation's own prose names
            // (25), one past it, and the quota itself.
            "twenty-five ids",
            (0..25)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
            (0..25)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
        ),
        (
            "twenty-six ids",
            (0..26)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
            (0..26)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
        ),
        (
            "the quota itself",
            (0..MAX_BATCH_SIZE)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
            (0..MAX_BATCH_SIZE)
                .map(|index| ids[index % ids.len()].clone())
                .collect(),
        ),
    ];

    for (case, requested, expected) in cases {
        let answered = emulator
            .ok(
                "BatchGetDeployments",
                json!({"deploymentIds": requested.clone()}),
            )
            .await;
        let reported = reported_ids(&answered);
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: a batch entry is the SAME stored record the single-id reader answers —
/// every member, including the creation timestamp with the precision it was
/// stored at.
#[tokio::test]
async fn a_batch_entry_equals_what_the_single_id_reader_answers() {
    let emulator = Emulator::new();
    let ids = seeded_deployments(&emulator, "compared", 3).await;
    await_terminal(&emulator, &ids).await;

    let answered = emulator
        .ok("BatchGetDeployments", json!({"deploymentIds": ids.clone()}))
        .await;
    let entries = answered["deploymentsInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        ids.len(),
        "case every created id: expected {} entries, got {answered:?}",
        ids.len()
    );

    for (id, entry) in ids.iter().zip(entries.iter()) {
        let single = emulator
            .ok("GetDeployment", json!({"deploymentId": id}))
            .await;
        assert_eq!(
            entry, &single["deploymentInfo"],
            "case {id}: expected the stored record the single-id reader answers ({:?}), got \
             {entry:?}",
            single["deploymentInfo"]
        );
        // The creation instant keeps the sub-second precision it was stored
        // with: a batch read must not round or re-stamp it.
        let create_time = entry["createTime"].as_f64();
        assert_eq!(
            create_time,
            single["deploymentInfo"]["createTime"].as_f64(),
            "case {id}: expected the stored createTime, got {create_time:?}"
        );
        assert!(
            create_time.is_some_and(|value| value > 0.0),
            "case {id}: expected a createTime instant, got {create_time:?}"
        );
    }
}

/// RULE: a read never changes what a later caller sees — the same batch answers
/// the same records however many times it is called.
#[tokio::test]
async fn a_batch_read_is_repeatable() {
    let emulator = Emulator::new();
    let ids = seeded_deployments(&emulator, "repeated", 2).await;
    await_terminal(&emulator, &ids).await;

    let first = emulator
        .ok("BatchGetDeployments", json!({"deploymentIds": ids.clone()}))
        .await;
    let again = emulator
        .ok("BatchGetDeployments", json!({"deploymentIds": ids.clone()}))
        .await;
    assert_eq!(
        again, first,
        "case reading twice: expected a repeatable read, got {again:?}"
    );
}

/// RULE: a request carrying more ids than the operation documents, no id at all,
/// an empty id, or a value that is not shaped like a deployment id is refused
/// with 400 and that failure's own modelled code — and the request-wide batch
/// bound is answered before any individual id is examined.
#[tokio::test]
async fn a_bad_batch_is_refused_with_its_own_code() {
    let emulator = Emulator::new();
    let ids = seeded_deployments(&emulator, "refused", 1).await;
    let known = ids[0].clone();
    // One past the quota, and the far larger batch a client that ignores it
    // sends.
    let one_too_many: Vec<String> = (0..=MAX_BATCH_SIZE).map(|_| known.clone()).collect();
    let far_too_many: Vec<String> = (0..500).map(|_| known.clone()).collect();
    // Too many ids AND every one of them malformed: the batch bound wins.
    let too_many_malformed: Vec<String> = (0..500).map(|index| format!("bogus{index}")).collect();

    let cases: &[(&str, Vec<String>, &str)] = &[
        (
            "one past the quota",
            one_too_many,
            "BatchLimitExceededException",
        ),
        (
            "far past the quota",
            far_too_many,
            "BatchLimitExceededException",
        ),
        (
            "too many ids, none of them well formed",
            too_many_malformed,
            "BatchLimitExceededException",
        ),
        ("no ids at all", Vec::new(), "DeploymentIdRequiredException"),
        (
            "an empty id beside a known one",
            vec![known.clone(), String::new()],
            "DeploymentIdRequiredException",
        ),
        (
            "an id of only whitespace",
            vec!["   ".to_string()],
            "DeploymentIdRequiredException",
        ),
        (
            "a value that is not an id",
            vec!["not-an-id".to_string()],
            "InvalidDeploymentIdException",
        ),
        (
            "an ARN instead of an id",
            vec!["arn:aws:codedeploy:us-west-2:123456789012:deploymentgroup:app/dg".to_string()],
            "InvalidDeploymentIdException",
        ),
        (
            "an id missing the d- prefix",
            vec!["ABCDEFGHI".to_string()],
            "InvalidDeploymentIdException",
        ),
        (
            "a punctuated id suffix",
            vec![known.clone(), "d-ABC_DEF12".to_string()],
            "InvalidDeploymentIdException",
        ),
    ];

    for (case, requested, expected) in cases {
        let (status, body) = emulator
            .call(
                "BatchGetDeployments",
                json!({"deploymentIds": requested.clone()}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {status} {body:?}"
        );
    }

    // A refused batch persisted nothing and hid nothing: the known deployment
    // still reads back.
    let answered = emulator
        .ok("BatchGetDeployments", json!({"deploymentIds": [known.clone()]}))
        .await;
    assert_eq!(
        reported_ids(&answered),
        vec![known.clone()],
        "case after the refusals: expected {known} to still read back, got {answered:?}"
    );
}
