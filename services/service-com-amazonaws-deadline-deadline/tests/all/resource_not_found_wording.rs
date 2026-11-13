// SPDX-License-Identifier: CC0-1.0

//! How this service words a resource it does not hold.
//!
//! RULE: a `ResourceNotFoundException` names the missing resource by the kind the API PATH
//! spells — the kebab-case URI segment, singular (`/storage-profiles/` names a
//! `storage-profile`) — in `resourceType`, carries the id the request named in `resourceId`,
//! and words `message` as `Resource of type <resourceType> with id <resourceId> does not
//! exist.`, trailing period included. The model documents `resourceType` only as "the type of
//! the resource that couldn't be found" and enumerates no values
//! (`smithy_query(query="shape", shape="ResourceNotFoundException")`: `httpError` 404, with
//! `message`, `resourceId` and `resourceType` all `@required`), so the path is what spells
//! the token: a client switching on it, or matching the sentence, must see the same text it
//! sees against AWS.
//!
//! The table below is one row per resource kind this service refuses on an implemented read
//! path, so the rule is asserted over the class rather than over the one kind a divergence
//! was observed on; the composite queue-fleet association is a row too, since it is named by
//! the three ids of its path rather than by one of its own.
//!
//! The harness (`Emulator`) and the fleet helper are the ones the other suites drive the
//! service with, so every call goes through routing, deserialization, constraint validation,
//! the handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::create_fleet;
use crate::queue_fleet_association::association_id;

/// The sentence this service words a missing resource with, as the responses recorded from
/// AWS show it.
///
/// `pub(crate)` so every suite in this test target expects the one sentence from the one
/// place rather than spelling it out again.
pub(crate) fn not_found_message(resource_type: &str, resource_id: &str) -> String {
    format!("Resource of type {resource_type} with id {resource_id} does not exist.")
}

/// One row: the call that names nothing, and the kind and id its 404 must report.
struct NotFound {
    case: &'static str,
    path: String,
    resource_type: &'static str,
    resource_id: String,
}

/// The table: one row per resource kind an implemented read path of this service refuses.
///
/// Every id is well formed for its modelled `^<prefix>-[0-9a-f]{32}$` label pattern, so the
/// boundary validator passes the request and the handler is what decides the outcome.
async fn rows(emulator: &Emulator) -> Vec<NotFound> {
    let farm_id = emulator.farm("Refusing farm").await;
    let queue_id = emulator.queue(&farm_id, "Refusing queue").await;
    let fleet_id = create_fleet(emulator, &farm_id, "Refusing fleet").await;

    let absent_farm = absent_id("farm", &format!("{:032x}", 0x01));
    let absent_queue = absent_id("queue", &format!("{:032x}", 0x02));
    let absent_fleet = absent_id("fleet", &format!("{:032x}", 0x03));
    let absent_job = absent_id("job", &format!("{:032x}", 0x04));
    let absent_worker = absent_id("worker", &format!("{:032x}", 0x05));
    let absent_budget = absent_id("budget", &format!("{:032x}", 0x06));
    let absent_limit = absent_id("limit", &format!("{:032x}", 0x07));
    let absent_profile = absent_id("sp", &format!("{:032x}", 0x08));

    vec![
        NotFound {
            case: "a farm, at /farms/",
            path: format!("/2023-10-12/farms/{absent_farm}"),
            resource_type: "farm",
            resource_id: absent_farm.clone(),
        },
        NotFound {
            case: "a queue, at /queues/",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{absent_queue}"),
            resource_type: "queue",
            resource_id: absent_queue,
        },
        NotFound {
            case: "a fleet, at /fleets/",
            path: format!("/2023-10-12/farms/{farm_id}/fleets/{absent_fleet}"),
            resource_type: "fleet",
            resource_id: absent_fleet,
        },
        NotFound {
            case: "a job, at /jobs/",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{absent_job}"),
            resource_type: "job",
            resource_id: absent_job,
        },
        NotFound {
            case: "a worker, at /workers/",
            path: format!(
                "/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{absent_worker}"
            ),
            resource_type: "worker",
            resource_id: absent_worker,
        },
        NotFound {
            case: "a budget, at /budgets/",
            path: format!("/2023-10-12/farms/{farm_id}/budgets/{absent_budget}"),
            resource_type: "budget",
            resource_id: absent_budget,
        },
        NotFound {
            case: "a limit, at /limits/",
            path: format!("/2023-10-12/farms/{farm_id}/limits/{absent_limit}"),
            resource_type: "limit",
            resource_id: absent_limit,
        },
        NotFound {
            // The kind whose path segment is two words: the token is kebab-case, never the
            // camelCase of the `storageProfileId` member.
            case: "a storage profile, at /storage-profiles/",
            path: format!("/2023-10-12/farms/{farm_id}/storage-profiles/{absent_profile}"),
            resource_type: "storage-profile",
            resource_id: absent_profile,
        },
        NotFound {
            // The composite kind: the model mints it no id, so it is named by the three ids
            // of its path, and its segment is three words.
            case: "a queue-fleet association, at /queue-fleet-associations/",
            path: format!(
                "/2023-10-12/farms/{farm_id}/queue-fleet-associations/{queue_id}/{fleet_id}"
            ),
            resource_type: "queue-fleet-association",
            resource_id: association_id(&farm_id, &queue_id, &fleet_id),
        },
        NotFound {
            // The parent level is refused by its own kind, not by the kind of the resource
            // the path ends in.
            case: "the farm of a storage-profile read",
            path: format!("/2023-10-12/farms/{absent_farm}/storage-profiles/{absent_profile_2}",
                absent_profile_2 = absent_id("sp", &format!("{:032x}", 0x09))),
            resource_type: "farm",
            resource_id: absent_farm,
        },
    ]
}

#[tokio::test]
async fn a_missing_resource_is_named_by_the_kind_its_path_spells() {
    let emulator = Emulator::new();

    for row in rows(&emulator).await {
        let (status, body) = emulator.get(&row.path).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404, got {status} with {body}",
            row.case
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
                string_at(&body, "message"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                row.resource_type.to_string(),
                row.resource_id.clone(),
                not_found_message(row.resource_type, &row.resource_id),
            ),
            "case {:?}: expected the 404 to name the missing kind and id, got {body}",
            row.case
        );
        // Every token is the path's own spelling: lower-case, kebab-case where the segment
        // is more than one word, and never a camelCase member name.
        assert!(
            row.resource_type
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-'),
            "case {:?}: a resourceType is the lower-case path token, got {:?}",
            row.case,
            row.resource_type
        );
    }
}

/// The same wording answers a read of a resource that WAS held and is now deleted: the
/// refusal follows the state, so a deleted storage profile is as missing as one that never
/// existed.
#[tokio::test]
async fn a_deleted_resource_is_refused_with_the_same_wording() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Deleting farm").await;

    // The rows differ along what the profile held, none of which changes the refusal.
    let cases: [(&str, Value); 3] = [
        (
            "no location list",
            json!({"displayName": "Gone one", "osFamily": "LINUX"}),
        ),
        (
            "an empty location list",
            json!({
                "displayName": "Gone two",
                "osFamily": "WINDOWS",
                "fileSystemLocations": [],
            }),
        ),
        (
            "one location",
            json!({
                "displayName": "Gone three",
                "osFamily": "MACOS",
                "fileSystemLocations": [{"name": "Assets", "path": "/mnt/assets", "type": "SHARED"}],
            }),
        ),
    ];

    for (case, request) in &cases {
        let (status, body) = emulator
            .post(
                &format!("/2023-10-12/farms/{farm_id}/storage-profiles"),
                request.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStorageProfile to answer 200, got {status} with \
             {body}"
        );
        let storage_profile_id = string_at(&body, "storageProfileId");
        let path = format!("/2023-10-12/farms/{farm_id}/storage-profiles/{storage_profile_id}");

        let (status, body) = emulator.call("DELETE", &path, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteStorageProfile to answer 200, got {status} with \
             {body}"
        );

        let (status, read) = emulator.get(&path).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted profile to be gone, got {status} with {read}"
        );
        assert_eq!(
            (
                error_code(&read),
                string_at(&read, "resourceType"),
                string_at(&read, "resourceId"),
                string_at(&read, "message"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                "storage-profile".to_string(),
                storage_profile_id.clone(),
                not_found_message("storage-profile", &storage_profile_id),
            ),
            "case {case:?}: expected the deleted profile's 404 to be worded as any other, \
             got {read}"
        );
    }
}
