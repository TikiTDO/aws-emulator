// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteStorageProfile`.
//!
//! The rules under test:
//!
//! * a delete removes the profile from its farm, so the read that follows it answers the
//!   modeled 404 and a repeat of the delete is still an empty success — the operation is
//!   `@idempotent` and models no not-found error, so a profile this emulator no longer
//!   holds (and a farm it never held) is nothing left to remove rather than a refusal;
//! * a queue's `allowedStorageProfileIds` is a live reference to a profile of the same
//!   farm, so while one holds it the delete is refused with the only client-input error
//!   the operation models (HTTP 400 `ValidationException`) and the profile stays readable;
//!   once nothing references it the same delete succeeds.
//!
//! The harness (`Emulator`) is the one every other suite in this target drives the service
//! with, so these tests go through routing, deserialization, constraint validation, the
//! handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};

/// Every value of the modeled `StorageProfileOperatingSystemFamily`, from
/// `smithy_query(query="shape", shape="StorageProfileOperatingSystemFamily")`:
/// `WINDOWS`, `LINUX`, `MACOS`.
const OS_FAMILIES: [&str; 3] = ["WINDOWS", "LINUX", "MACOS"];

fn profile_path(farm_id: &str, storage_profile_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/storage-profiles/{storage_profile_id}")
}

/// Create a storage profile under `farm_id` and return its id.
async fn create_profile(emulator: &Emulator, farm_id: &str, request: Value) -> String {
    let (status, body) = emulator
        .post(&format!("/2023-10-12/farms/{farm_id}/storage-profiles"), request)
        .await;
    assert_eq!(
        status, 200,
        "CreateStorageProfile: expected 200, got {status} with {body}"
    );
    string_at(&body, "storageProfileId")
}

/// Create a queue under `farm_id` whose `allowedStorageProfileIds` is `allowed` (omitted
/// entirely when `allowed` is `None`) and return its id.
async fn create_queue(
    emulator: &Emulator,
    farm_id: &str,
    display_name: &str,
    allowed: Option<&[&str]>,
) -> String {
    let mut request = json!({"displayName": display_name});
    if let Some(allowed) = allowed {
        request["allowedStorageProfileIds"] = json!(allowed);
    }
    let (status, body) = emulator
        .post(&format!("/2023-10-12/farms/{farm_id}/queues"), request)
        .await;
    assert_eq!(
        status, 200,
        "CreateQueue {display_name}: expected 200, got {status} with {body}"
    );
    string_at(&body, "queueId")
}

async fn delete_profile(
    emulator: &Emulator,
    farm_id: &str,
    storage_profile_id: &str,
) -> (u16, Value) {
    emulator
        .call("DELETE", &profile_path(farm_id, storage_profile_id), None, None)
        .await
}

#[tokio::test]
async fn deleting_a_storage_profile_removes_it_and_repeats_as_an_empty_success() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Deleting farm").await;

    // The rule: the delete removes the profile from its farm. The rows differ along what
    // the profile holds — each operating-system family the model names, and a location
    // list that is absent, empty or filled — none of which changes the outcome.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no location list",
            json!({"displayName": "Windows profile", "osFamily": OS_FAMILIES[0]}),
        ),
        (
            "an empty location list",
            json!({
                "displayName": "Linux profile",
                "osFamily": OS_FAMILIES[1],
                "fileSystemLocations": [],
            }),
        ),
        (
            "two locations",
            json!({
                "displayName": "macOS profile",
                "osFamily": OS_FAMILIES[2],
                "fileSystemLocations": [
                    {"name": "Assets", "path": "/mnt/assets", "type": "SHARED"},
                    {"name": "Scratch", "path": "/local/scratch", "type": "LOCAL"},
                ],
            }),
        ),
    ];

    for (case, request) in &cases {
        let storage_profile_id = create_profile(&emulator, &farm_id, request.clone()).await;
        let (status, body) = delete_profile(&emulator, &farm_id, &storage_profile_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteStorageProfile to answer 200, got {status} with \
             {body}"
        );

        let (status, read) = emulator
            .get(&profile_path(&farm_id, &storage_profile_id))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted profile to be gone, got {status} with {read}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException for the deleted profile, \
             got {read}"
        );

        // `@idempotent`, and the model declares no not-found error: a repeat of the delete
        // has nothing left to remove and answers the same empty success.
        let (status, body) = delete_profile(&emulator, &farm_id, &storage_profile_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated delete to answer 200, got {status} with \
             {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_storage_profile_this_emulator_does_not_hold_removes_nothing() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Idempotent delete farm").await;
    let kept = create_profile(
        &emulator,
        &farm_id,
        json!({"displayName": "Kept", "osFamily": OS_FAMILIES[1]}),
    )
    .await;
    // 32 hex characters, as the modeled `^farm-[0-9a-f]{32}$` / `^sp-[0-9a-f]{32}$` label
    // patterns require, so the request passes the boundary validator.
    let absent_farm = absent_id("farm", &format!("{:032x}", 0x11));
    let absent_profile = absent_id("sp", &format!("{:032x}", 0x12));

    // The rule: with no not-found error modeled, a well-formed id naming nothing is an
    // empty success. The rows differ along which level is missing — the profile, the farm,
    // or both — and none of them touches the profile the farm does hold.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an unheld profile under a held farm", &farm_id, &absent_profile),
        ("a held profile under an unheld farm", &absent_farm, &kept),
        ("both levels unheld", &absent_farm, &absent_profile),
    ];
    for (case, farm, profile) in &cases {
        let (status, body) = delete_profile(&emulator, farm, profile).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteStorageProfile to answer 200, got {status} with \
             {body}"
        );
        let (status, read) = emulator.get(&profile_path(&farm_id, &kept)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the held profile to be untouched, got {status} with \
             {read}"
        );
    }
}

#[tokio::test]
async fn a_storage_profile_a_queue_still_allows_is_not_deleted() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Referencing farm").await;
    let other_farm_id = emulator.farm("Unrelated farm").await;

    // Five profiles, one per row of the table below, so each row's delete is independent.
    let mut profiles: Vec<String> = Vec::new();
    for index in 0..5 {
        profiles.push(
            create_profile(
                &emulator,
                &farm_id,
                json!({
                    "displayName": format!("Profile {index}"),
                    "osFamily": OS_FAMILIES[index % OS_FAMILIES.len()],
                }),
            )
            .await,
        );
    }
    let unrelated = create_profile(
        &emulator,
        &farm_id,
        json!({"displayName": "Never referenced", "osFamily": OS_FAMILIES[0]}),
    )
    .await;

    // The rule: a queue of the same farm that lists the profile in
    // `allowedStorageProfileIds` blocks the delete; anything else does not. The rows differ
    // along how the reference is held — alone, among several ids, by two queues at once —
    // and along the ways a reference does NOT reach the profile: a queue that allows other
    // ids, a queue with no list at all, and a queue of another farm that allows this id.
    let sole = create_queue(&emulator, &farm_id, "Sole reference", Some(&[&profiles[0]])).await;
    let among = create_queue(
        &emulator,
        &farm_id,
        "One of several",
        Some(&[
            &unrelated,
            &profiles[1],
            &absent_id("sp", &format!("{:032x}", 0x1a)),
        ]),
    )
    .await;
    let first_of_two = create_queue(&emulator, &farm_id, "First holder", Some(&[&profiles[2]])).await;
    let second_of_two =
        create_queue(&emulator, &farm_id, "Second holder", Some(&[&profiles[2]])).await;
    create_queue(&emulator, &farm_id, "Other ids only", Some(&[&unrelated])).await;
    create_queue(&emulator, &farm_id, "No list at all", None).await;
    create_queue(
        &emulator,
        &other_farm_id,
        "Another farm's queue",
        Some(&[&profiles[4]]),
    )
    .await;

    // `expected_queue` is the queue the refusal must name: the lowest id of the queues
    // holding the reference, so the answer does not depend on map order. `None` means the
    // delete is expected to succeed.
    let cases: Vec<(&str, &str, Option<String>)> = vec![
        (
            "one queue allows exactly this profile",
            &profiles[0],
            Some(sole),
        ),
        (
            "a queue allows this profile among several ids",
            &profiles[1],
            Some(among),
        ),
        (
            "two queues allow this profile",
            &profiles[2],
            Some(first_of_two.min(second_of_two)),
        ),
        (
            "no queue of this farm allows this profile",
            &profiles[3],
            None,
        ),
        (
            "only a queue of ANOTHER farm allows this profile",
            &profiles[4],
            None,
        ),
    ];

    for (case, storage_profile_id, expected_queue) in &cases {
        let (status, body) = delete_profile(&emulator, &farm_id, storage_profile_id).await;
        match expected_queue {
            Some(queue_id) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected DeleteStorageProfile to answer 400, got {status} \
                     with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body}"
                );
                assert_eq!(
                    string_at(&body, "message"),
                    format!(
                        "The storage profile {storage_profile_id} can't be deleted because it \
                         is associated with the queue {queue_id}."
                    ),
                    "case {case:?}: expected the refusal to name the profile and the queue \
                     holding it, got {body}"
                );
                // The refused delete removed nothing.
                let (status, read) = emulator
                    .get(&profile_path(&farm_id, storage_profile_id))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the refused profile to stay readable, got \
                     {status} with {read}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DeleteStorageProfile to answer 200, got {status} \
                     with {body}"
                );
                let (status, read) = emulator
                    .get(&profile_path(&farm_id, storage_profile_id))
                    .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the deleted profile to be gone, got {status} \
                     with {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_storage_profile_is_deletable_once_nothing_references_it() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Unblocking farm").await;
    let storage_profile_id = create_profile(
        &emulator,
        &farm_id,
        json!({"displayName": "Referenced then freed", "osFamily": OS_FAMILIES[1]}),
    )
    .await;
    let queue_id = create_queue(
        &emulator,
        &farm_id,
        "Holder",
        Some(&[&storage_profile_id]),
    )
    .await;

    // The rule: the refusal follows the state, not the id — the same request that was
    // refused while a queue held the reference succeeds once that queue is gone.
    let (status, body) = delete_profile(&emulator, &farm_id, &storage_profile_id).await;
    assert_eq!(
        status, 400,
        "while the queue holds the reference: expected 400, got {status} with {body}"
    );

    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteQueue: expected 200, got {status} with {body}"
    );

    let (status, body) = delete_profile(&emulator, &farm_id, &storage_profile_id).await;
    assert_eq!(
        status, 200,
        "once nothing references it: expected 200, got {status} with {body}"
    );
    let (status, read) = emulator
        .get(&profile_path(&farm_id, &storage_profile_id))
        .await;
    assert_eq!(
        status, 404,
        "expected the freed profile to be gone after the delete, got {status} with {read}"
    );
}
