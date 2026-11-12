// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateQueueFleetAssociation`.
//!
//! The rules under test:
//!
//! * the call stores one association per `(queue, fleet)` pair OF ONE FARM — a queue may be
//!   associated with every fleet of its farm and a fleet with every queue of it — and each
//!   stored association is readable through `GetQueueFleetAssociation` as `ACTIVE`, carrying
//!   the two ids the call named, the principal that made the call and the instant it was
//!   made, and no update members until something updates it;
//! * the operation is `@idempotent` and the model declares no `ConflictException` for it
//!   (`smithy_query(query="operation", operation="CreateQueueFleetAssociation")`), so asking
//!   for a pair that is already associated answers the same success again and leaves the
//!   stored association — its status, its creation instant and its creator — as the first
//!   call left it;
//! * an association joins a queue and a fleet the named farm holds, so a call naming a farm,
//!   a queue or a fleet that farm does not hold stores nothing and answers a 404 naming the
//!   level that is missing;
//! * an id that does not satisfy its modelled pattern never reaches the store: the call is
//!   refused with a 400 `ValidationException` and nothing is associated.
//!
//! The harness (`Emulator`) and the fleet helper are the ones the other suites drive the
//! service with, so every call here goes through routing, deserialization, constraint
//! validation, the handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::create_fleet;

/// The principal the harness signs with: `extract_akid` reads `AKIDEXAMPLE` out of its
/// `authorization` header, and the service reports a caller as an IAM user ARN of it.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIDEXAMPLE";

/// `PUT /2023-10-12/farms/{farmId}/queue-fleet-associations` — where an association is
/// created, with `queueId` and `fleetId` in the body.
fn create_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations")
}

/// `GET|PATCH /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId}` —
/// where one association is read and updated.
fn association_path(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations/{queue_id}/{fleet_id}")
}

/// The composite id this service reports for an association: the three ids of its path,
/// since the model mints the association no id of its own.
fn association_id(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("{farm_id}, {queue_id} and {fleet_id}")
}

/// Ask for the association of `queue_id` and `fleet_id` under `farm_id`.
async fn create_association(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &create_path(farm_id),
            Some(json!({"queueId": queue_id, "fleetId": fleet_id})),
            None,
        )
        .await
}

/// Ask for the association and fail the test unless it was created.
async fn associate(emulator: &Emulator, farm_id: &str, queue_id: &str, fleet_id: &str) {
    let (status, body) = create_association(emulator, farm_id, queue_id, fleet_id).await;
    assert_eq!(
        status, 200,
        "CreateQueueFleetAssociation {queue_id}/{fleet_id}: expected 200, got {status} with \
         {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "CreateQueueFleetAssociation {queue_id}/{fleet_id}: expected an empty response, got \
         {body}"
    );
}

/// What `GetQueueFleetAssociation` reports for a pair, with its status.
async fn read_association(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
) -> (u16, Value) {
    emulator
        .get(&association_path(farm_id, queue_id, fleet_id))
        .await
}

#[tokio::test]
async fn create_queue_fleet_association_stores_every_pair_it_is_asked_for() {
    // The rule: a queue of a farm may be associated with every fleet of that farm, and a
    // fleet with every queue of it, each pair stored on its own. The rows differ along the
    // COUNT of queues and fleets the farm holds, so the association is shown to be keyed by
    // the pair rather than by either side alone.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("one queue on one fleet", 1, 1),
        ("one queue on three fleets", 1, 3),
        ("three queues on one fleet", 3, 1),
        ("two queues on two fleets", 2, 2),
    ];

    for (case, queue_count, fleet_count) in cases {
        let emulator = Emulator::new();
        let farm_id = emulator.farm(case).await;
        let mut queue_ids = Vec::new();
        for index in 0..queue_count {
            queue_ids.push(emulator.queue(&farm_id, &format!("queue {index}")).await);
        }
        let mut fleet_ids = Vec::new();
        for index in 0..fleet_count {
            fleet_ids.push(create_fleet(&emulator, &farm_id, &format!("fleet {index}")).await);
        }
        // A fleet of the same farm this test associates with nothing, so a stored
        // association is shown to cover only the pairs that were asked for.
        let unassociated_fleet = create_fleet(&emulator, &farm_id, "associated with nothing").await;

        for queue_id in &queue_ids {
            for fleet_id in &fleet_ids {
                associate(&emulator, &farm_id, queue_id, fleet_id).await;
            }
        }

        for queue_id in &queue_ids {
            for fleet_id in &fleet_ids {
                let (status, association) =
                    read_association(&emulator, &farm_id, queue_id, fleet_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetQueueFleetAssociation {queue_id}/{fleet_id} to \
                     answer 200, got {status} with {association}"
                );
                assert_eq!(
                    (
                        string_at(&association, "queueId"),
                        string_at(&association, "fleetId"),
                        // `QueueFleetAssociationStatus` is `ACTIVE | STOPPED |
                        // STOP_SCHEDULING_AND_CANCEL_TASKS |
                        // STOP_SCHEDULING_AND_COMPLETE_TASKS`
                        // (`smithy_query(query="shape", shape="QueueFleetAssociationStatus")`):
                        // a fresh association schedules the queue's work onto the fleet.
                        string_at(&association, "status"),
                        string_at(&association, "createdBy"),
                    ),
                    (
                        queue_id.clone(),
                        fleet_id.clone(),
                        "ACTIVE".to_string(),
                        CALLER.to_string(),
                    ),
                    "case {case:?}: expected the stored pair {queue_id}/{fleet_id} reported as \
                     created and scheduling, got {association}"
                );
                assert!(
                    // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never
                    // the protocol-default epoch number (`date_time_wire_form` owns that
                    // rule).
                    association.get("createdAt").and_then(Value::as_str).is_some(),
                    "case {case:?}: expected createdAt as an RFC 3339 string for \
                     {queue_id}/{fleet_id}, got {association}"
                );
                assert!(
                    association.get("updatedAt").is_none()
                        && association.get("updatedBy").is_none(),
                    "case {case:?}: expected no update members for {queue_id}/{fleet_id} before \
                     an update, got {association}"
                );
            }

            // The fleet nothing associated is not associated: the create stored only the
            // pairs it was asked for.
            let (status, body) =
                read_association(&emulator, &farm_id, queue_id, &unassociated_fleet).await;
            let missing = association_id(&farm_id, queue_id, &unassociated_fleet);
            assert_eq!(
                (
                    status,
                    error_code(&body),
                    string_at(&body, "message"),
                    string_at(&body, "resourceType"),
                    string_at(&body, "resourceId"),
                ),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    format!(
                        "Resource of type queue-fleet-association with id {missing} does not \
                         exist."
                    ),
                    "queue-fleet-association".to_string(),
                    missing.clone(),
                ),
                "case {case:?}: expected {queue_id} not to be associated with a fleet nothing \
                 asked for, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_create_queue_fleet_association_leaves_the_stored_association_alone() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotent").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "the fleet").await;

    // The rule: the operation is `@idempotent` and models no conflict, so a repeat answers
    // the same success and the stored association stays as the first call made it. The rows
    // differ along HOW MANY times the pair is asked for, and along whether the association
    // was updated in between — a stop asked for through
    // `UpdateQueueFleetAssociation` is not undone by asking for the association again.
    let cases: Vec<(&str, usize, Option<&str>)> = vec![
        ("asked for twice", 2, None),
        ("asked for five times", 5, None),
        (
            "repeated after a complete-tasks stop",
            2,
            Some("STOP_SCHEDULING_AND_COMPLETE_TASKS"),
        ),
        (
            "repeated after a cancel-tasks stop",
            3,
            Some("STOP_SCHEDULING_AND_CANCEL_TASKS"),
        ),
    ];

    for (case, attempts, stop) in cases {
        let queue_id = emulator.queue(&farm_id, case).await;
        associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
        if let Some(status) = stop {
            let (got, body) = emulator
                .call(
                    "PATCH",
                    &association_path(&farm_id, &queue_id, &fleet_id),
                    Some(json!({"status": status})),
                    None,
                )
                .await;
            assert_eq!(
                got, 200,
                "case {case:?}: expected the stop to {status} to answer 200, got {got} with \
                 {body}"
            );
        }
        let (_, first) = read_association(&emulator, &farm_id, &queue_id, &fleet_id).await;

        for attempt in 2..=attempts {
            associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
            let (got, again) = read_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
            assert_eq!(
                got, 200,
                "case {case:?}: expected the association to still be readable after attempt \
                 {attempt}, got {got} with {again}"
            );
            // Compared member by member rather than whole-body, because the
            // `STOP_SCHEDULING_AND_* -> STOPPED` drain the stop started is owed by the clock
            // and may come due between two reads; what the repeat must not do is re-create
            // the association or put it back to scheduling.
            assert_eq!(
                (
                    string_at(&again, "queueId"),
                    string_at(&again, "fleetId"),
                    again.get("createdAt").cloned(),
                    string_at(&again, "createdBy"),
                    again.get("updatedBy").cloned(),
                ),
                (
                    string_at(&first, "queueId"),
                    string_at(&first, "fleetId"),
                    first.get("createdAt").cloned(),
                    string_at(&first, "createdBy"),
                    first.get("updatedBy").cloned(),
                ),
                "case {case:?}: attempt {attempt} re-created the association: {first} then \
                 {again}"
            );
            let status_now = string_at(&again, "status");
            let expected: &[&str] = match stop {
                None => &["ACTIVE"],
                Some(asked_for) => &[asked_for, "STOPPED"],
            };
            assert!(
                expected.contains(&status_now.as_str()),
                "case {case:?}: attempt {attempt} expected the status to stay one of \
                 {expected:?}, got {status_now:?}"
            );
        }
    }
}

#[tokio::test]
async fn create_queue_fleet_association_refuses_a_level_the_farm_does_not_hold() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let queue_id = emulator.queue(&farm_id, "owned queue").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "owned fleet").await;
    let other_queue_id = emulator.queue(&other_farm_id, "another farm's queue").await;
    let other_fleet_id = create_fleet(&emulator, &other_farm_id, "another farm's fleet").await;

    let absent_farm = absent_id("farm", "00000000000000000000000000000041");
    let absent_queue = absent_id("queue", "00000000000000000000000000000042");
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000043");

    // The rule: an association joins a queue and a fleet the named farm holds, so whichever
    // of the three the farm does not hold is the level the 404 names. The rows differ along
    // which level that is, and whether it is absent everywhere or merely held by another
    // farm.
    let cases: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "an unheld farm",
            absent_farm.clone(),
            queue_id.clone(),
            fleet_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld queue",
            farm_id.clone(),
            absent_queue.clone(),
            fleet_id.clone(),
            "queue",
            absent_queue.clone(),
        ),
        (
            "an unheld fleet",
            farm_id.clone(),
            queue_id.clone(),
            absent_fleet.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
        (
            "another farm's queue",
            farm_id.clone(),
            other_queue_id.clone(),
            fleet_id.clone(),
            "queue",
            other_queue_id.clone(),
        ),
        (
            "another farm's fleet",
            farm_id.clone(),
            queue_id.clone(),
            other_fleet_id.clone(),
            "fleet",
            other_fleet_id.clone(),
        ),
        (
            "this farm's queue under another farm",
            other_farm_id.clone(),
            queue_id.clone(),
            other_fleet_id.clone(),
            "queue",
            queue_id.clone(),
        ),
    ];

    for (case, farm, queue, fleet, resource_type, resource_id) in cases {
        let (status, body) = create_association(&emulator, &farm, &queue, &fleet).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected CreateQueueFleetAssociation to answer 404, got {status} \
             with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "message"),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                format!("Resource of type {resource_type} with id {resource_id} does not exist."),
                resource_type.to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected the 404 to name the level that is missing, got {body}"
        );

        // The refusal stored nothing: reading the pair it named back is a 404 too, for
        // whichever level the create refused on.
        let (read, read_body) = read_association(&emulator, &farm, &queue, &fleet).await;
        assert_eq!(
            (read, error_code(&read_body)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the refused pair to be unknown to a read, got {read} with \
             {read_body}"
        );
    }

    // And a pair of one farm's own queue and fleet is still accepted after those refusals.
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
    let (status, association) = read_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        (status, string_at(&association, "status")),
        (200, "ACTIVE".to_string()),
        "after the refusals: expected the farm's own pair to associate, got {status} with \
         {association}"
    );
}

#[tokio::test]
async fn create_queue_fleet_association_refuses_an_id_that_does_not_match_its_pattern() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("validated").await;
    let queue_id = emulator.queue(&farm_id, "validated queue").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "validated fleet").await;

    // The rule: each id carries a modelled pattern — `^farm-[0-9a-f]{32}$`,
    // `^queue-[0-9a-f]{32}$` and `^fleet-[0-9a-f]{32}$`
    // (`smithy_query(query="operation", operation="CreateQueueFleetAssociation")`) — so a
    // value that does not satisfy it is refused before any association is stored. The rows
    // differ along which id is malformed and how: the wrong prefix, the wrong length, a
    // non-hex character, and an empty value.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a queueId with a fleet's prefix",
            farm_id.clone(),
            json!({"queueId": fleet_id.clone(), "fleetId": fleet_id.clone()}),
        ),
        (
            "a queueId one character short",
            farm_id.clone(),
            json!({"queueId": "queue-0000000000000000000000000000000", "fleetId": fleet_id.clone()}),
        ),
        (
            "a fleetId with a non-hex character",
            farm_id.clone(),
            json!({"queueId": queue_id.clone(), "fleetId": "fleet-g000000000000000000000000000000f"}),
        ),
        (
            "an empty fleetId",
            farm_id.clone(),
            json!({"queueId": queue_id.clone(), "fleetId": ""}),
        ),
        (
            "a malformed farmId in the path",
            "farm-not-a-farm".to_string(),
            json!({"queueId": queue_id.clone(), "fleetId": fleet_id.clone()}),
        ),
    ];

    for (case, farm, body) in cases {
        let (status, answer) = emulator
            .call("PUT", &create_path(&farm), Some(body), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer}"
        );
    }

    // None of those refusals associated anything, and the well-formed pair still does.
    let (status, body) = read_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "after the refusals: expected no association to have been stored, got {status} with \
         {body}"
    );
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
    let (status, association) = read_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        (status, string_at(&association, "status")),
        (200, "ACTIVE".to_string()),
        "after the refusals: expected the well-formed pair to associate, got {status} with \
         {association}"
    );
}
