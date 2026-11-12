// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the storage-profile slice of AWS Deadline Cloud:
//! `CreateStorageProfile` and `GetStorageProfile`.
//!
//! The rules under test:
//!
//! * a storage profile is created under its farm and `GetStorageProfile` reports the
//!   values that create persisted, under a minted `^sp-[0-9a-f]{32}$` id;
//! * a storage profile hangs off its farm, so a create naming a farm this emulator does
//!   not hold creates nothing, and a read naming another farm does not find it;
//! * `clientToken` deduplicates a retried create;
//! * a location list longer than the modeled maximum is refused and stores nothing.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so these tests go through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, is_resource_id, string_at, Emulator};

/// Every value of the modeled `StorageProfileOperatingSystemFamily`, from
/// `smithy_query(query="shape", shape="StorageProfileOperatingSystemFamily")`:
/// `WINDOWS`, `LINUX`, `MACOS`.
const OS_FAMILIES: [&str; 3] = ["WINDOWS", "LINUX", "MACOS"];

/// Every value of the modeled `FileSystemLocationType`, from
/// `smithy_query(query="shape", shape="FileSystemLocationType")`: `SHARED`, `LOCAL`.
const LOCATION_TYPES: [&str; 2] = ["SHARED", "LOCAL"];

/// Modeled `@length` maximum of `FileSystemLocationsList`, the target of
/// `fileSystemLocations` (`len 0..20`), from
/// `smithy_query(query="operation", operation="CreateStorageProfile")`.
const MAX_FILE_SYSTEM_LOCATIONS: usize = 20;

/// Modeled `@length` bounds of `ResourceName`, the target of `displayName` (`len 1..100`).
const MIN_DISPLAY_NAME: usize = 1;
const MAX_DISPLAY_NAME: usize = 100;

/// Modeled `@length` maximum of `FileSystemLocationName` (`len 1..64`, pattern
/// `^[0-9A-Za-z ]*$`).
const MAX_LOCATION_NAME: usize = 64;

/// Modeled `@length` maximum of `PathString`, the target of a location's `path`
/// (`len 0..1024`).
const MAX_LOCATION_PATH: usize = 1024;

fn create_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/storage-profiles")
}

fn get_path(farm_id: &str, storage_profile_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/storage-profiles/{storage_profile_id}")
}

/// One file system location: `name`, `path` and `type` are all required
/// (`smithy_query(query="shape", shape="FileSystemLocation")`).
fn location(name: &str, path: &str, kind: &str) -> Value {
    json!({"name": name, "path": path, "type": kind})
}

/// A path exactly `len` characters long, so a row can sit on the modeled bound.
fn path_of_length(len: usize) -> String {
    let prefix = "/mnt/";
    format!("{prefix}{}", "a".repeat(len - prefix.len()))
}

/// Create a storage profile under `farm_id` and return its id.
async fn create_profile(emulator: &Emulator, farm_id: &str, request: Value) -> String {
    let (status, body) = emulator.post(&create_path(farm_id), request).await;
    assert_eq!(
        status, 200,
        "CreateStorageProfile: expected 200, got {status} with {body}"
    );
    string_at(&body, "storageProfileId")
}

#[tokio::test]
async fn create_storage_profile_then_get_storage_profile_returns_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Storage profile farm").await;

    // The rule: GetStorageProfile reports the values CreateStorageProfile persisted. The
    // rows differ along every member the create accepts — each operating-system family
    // the model names, the display name at both modeled length bounds, and a location
    // list that is absent, empty, singular, of both location types at the name and path
    // bounds, and at the modeled maximum element count.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no fileSystemLocations member at all",
            json!({"displayName": "Windows renders", "osFamily": OS_FAMILIES[0]}),
        ),
        (
            "an empty location list",
            json!({
                "displayName": "Linux renders",
                "osFamily": OS_FAMILIES[1],
                "fileSystemLocations": [],
            }),
        ),
        (
            "one location, at the shorter name and path bounds",
            json!({
                "displayName": "macOS renders",
                "osFamily": OS_FAMILIES[2],
                "fileSystemLocations": [location("a", "", LOCATION_TYPES[0])],
            }),
        ),
        (
            "both location types, at the longer name and path bounds",
            json!({
                "displayName": "Mixed locations",
                "osFamily": OS_FAMILIES[1],
                "fileSystemLocations": [
                    location(
                        &"n".repeat(MAX_LOCATION_NAME),
                        &path_of_length(MAX_LOCATION_PATH),
                        LOCATION_TYPES[0],
                    ),
                    location("Scratch 2", "/local/scratch", LOCATION_TYPES[1]),
                ],
            }),
        ),
        (
            "the modeled maximum number of locations",
            json!({
                "displayName": "Twenty locations",
                "osFamily": OS_FAMILIES[0],
                "fileSystemLocations": (0..MAX_FILE_SYSTEM_LOCATIONS)
                    .map(|index| location(
                        &format!("loc {index}"),
                        &format!("/mnt/share{index}"),
                        LOCATION_TYPES[index % LOCATION_TYPES.len()],
                    ))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "a display name at the modeled minimum length",
            json!({
                "displayName": "A".repeat(MIN_DISPLAY_NAME),
                "osFamily": OS_FAMILIES[2],
            }),
        ),
        (
            "a display name at the modeled maximum length",
            json!({
                "displayName": "Z".repeat(MAX_DISPLAY_NAME),
                "osFamily": OS_FAMILIES[1],
                "fileSystemLocations": [location("Assets", "/mnt/assets", LOCATION_TYPES[1])],
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let (status, created) = emulator.post(&create_path(&farm_id), request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStorageProfile to answer 200, got {status} with \
             {created}"
        );
        let storage_profile_id = string_at(&created, "storageProfileId");
        assert!(
            is_resource_id(&storage_profile_id, "sp"),
            "case {case:?}: expected a ^sp-[0-9a-f]{{32}}$ id, got {storage_profile_id:?}"
        );
        assert!(
            !minted.contains(&storage_profile_id),
            "case {case:?}: id {storage_profile_id} was already minted for another profile"
        );
        minted.push(storage_profile_id.clone());

        let (status, profile) = emulator.get(&get_path(&farm_id, &storage_profile_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStorageProfile to answer 200, got {status} with \
             {profile}"
        );
        assert_eq!(
            (
                string_at(&profile, "storageProfileId"),
                string_at(&profile, "displayName"),
                string_at(&profile, "osFamily"),
            ),
            (
                storage_profile_id.clone(),
                string_at(request, "displayName"),
                string_at(request, "osFamily"),
            ),
            "case {case:?}: expected the created identity to be reported, got {profile}"
        );
        assert_eq!(
            profile.get("fileSystemLocations"),
            request.get("fileSystemLocations"),
            "case {case:?}: expected the created locations to be reported, got {profile}"
        );
        assert!(
            !string_at(&profile, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {profile}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            profile.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {profile}"
        );
        assert!(
            profile.get("updatedAt").is_none() && profile.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {profile}"
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator.get(&get_path(&farm_id, &storage_profile_id)).await;
        assert_eq!(
            again, profile,
            "case {case:?}: a second read answered differently, got {again}"
        );
    }
}

#[tokio::test]
async fn a_location_list_past_the_modeled_maximum_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Bounded locations farm").await;

    // The rule: `fileSystemLocations` holds at most the modeled number of entries, so a
    // list one past that bound is refused and stores nothing. The row at the bound itself
    // is accepted by the table above.
    let too_many: Vec<Value> = (0..=MAX_FILE_SYSTEM_LOCATIONS)
        .map(|index| location(&format!("loc {index}"), &format!("/mnt/{index}"), "SHARED"))
        .collect();
    let case = format!("{} locations", too_many.len());
    let (status, body) = emulator
        .post(
            &create_path(&farm_id),
            json!({
                "displayName": "Too many locations",
                "osFamily": OS_FAMILIES[1],
                "fileSystemLocations": too_many,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateStorageProfile to answer 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body}"
    );
}

#[tokio::test]
async fn a_storage_profile_is_reachable_only_through_its_own_farm() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Owning farm").await;
    let other_farm_id = emulator.farm("Other farm").await;
    let storage_profile_id = create_profile(
        &emulator,
        &farm_id,
        json!({"displayName": "Shared assets", "osFamily": OS_FAMILIES[1]}),
    )
    .await;
    let absent_farm = absent_id("farm", "00000000000000000000000000000001");
    let absent_profile = absent_id("sp", "00000000000000000000000000000002");

    // The rule: a storage profile hangs off its farm, so a read naming another farm, an
    // unheld farm or an unheld profile is a 404 naming the level that is missing.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a profile of another farm",
            other_farm_id,
            storage_profile_id.clone(),
            "storage-profile",
            storage_profile_id.clone(),
        ),
        (
            "an unheld profile under a held farm",
            farm_id.clone(),
            absent_profile.clone(),
            "storage-profile",
            absent_profile,
        ),
        (
            "a profile under an unheld farm",
            absent_farm.clone(),
            storage_profile_id,
            "farm",
            absent_farm.clone(),
        ),
    ];
    for (case, farm, profile, resource_type, resource_id) in &cases {
        let (status, body) = emulator.get(&get_path(farm, profile)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetStorageProfile to answer 404, got {status} with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                (*resource_type).to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected the 404 to name the missing level, got {body}"
        );
    }

    // A create under a farm this emulator does not hold creates nothing.
    let (status, body) = emulator
        .post(
            &create_path(&absent_farm),
            json!({"displayName": "Orphan", "osFamily": OS_FAMILIES[0]}),
        )
        .await;
    assert_eq!(
        status, 404,
        "CreateStorageProfile under an unheld farm: expected 404, got {status} with {body}"
    );
    assert_eq!(
        (
            error_code(&body),
            string_at(&body, "resourceType"),
            string_at(&body, "resourceId"),
        ),
        (
            "ResourceNotFoundException".to_string(),
            "farm".to_string(),
            absent_farm,
        ),
        "CreateStorageProfile under an unheld farm: expected the 404 to name the farm, got \
         {body}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_creates_one_storage_profile() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Idempotent farm").await;
    let other_farm_id = emulator.farm("Second idempotent farm").await;
    let request = json!({"displayName": "Retried", "osFamily": OS_FAMILIES[1]});

    // The rule: `clientToken` is an idempotency token, so a retry of the same create
    // answers with the resource the first call made. The rows differ along what changes
    // between the two calls: nothing, the token, or the farm the token is used under.
    let (_, first) = emulator
        .call(
            "POST",
            &create_path(&farm_id),
            Some(request.clone()),
            Some("storage-profile-token"),
        )
        .await;
    let first_id = string_at(&first, "storageProfileId");

    let cases: Vec<(&str, String, &str, bool)> = vec![
        (
            "the same token on the same farm",
            farm_id.clone(),
            "storage-profile-token",
            true,
        ),
        (
            "a fresh token on the same farm",
            farm_id.clone(),
            "another-storage-profile-token",
            false,
        ),
        (
            "the same token on another farm",
            other_farm_id,
            "storage-profile-token",
            false,
        ),
    ];
    for (case, farm, token, expect_same) in &cases {
        let (status, body) = emulator
            .call("POST", &create_path(farm), Some(request.clone()), Some(token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let id = string_at(&body, "storageProfileId");
        assert_eq!(
            id == first_id,
            *expect_same,
            "case {case:?}: expected same-as-first={expect_same}, got {id} against {first_id}"
        );
    }

    // Exactly one profile answers at the deduplicated id, and it is the one the first
    // call created.
    let (status, profile) = emulator.get(&get_path(&farm_id, &first_id)).await;
    assert_eq!(
        status, 200,
        "expected the deduplicated profile to be readable, got {status} with {profile}"
    );
    assert_eq!(
        string_at(&profile, "displayName"),
        string_at(&request, "displayName"),
        "expected the first call's values, got {profile}"
    );
}
