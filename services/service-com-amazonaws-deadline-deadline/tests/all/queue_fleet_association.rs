// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteQueueFleetAssociation`, over the status lifecycle
//! `UpdateQueueFleetAssociation` drives an association through.
//!
//! The rules under test:
//!
//! * an association is taken away only from the status in which it holds none of the
//!   queue's work on the fleet (`STOPPED`): a delete asked for in any other
//!   `QueueFleetAssociationStatus` is refused with a 409 `ConflictException` naming the
//!   status it is in, and leaves the stored association exactly as it was;
//! * a delete of a stopped association removes it, so a later `GetQueueFleetAssociation`
//!   of that pair is a 404, the fleet it held is released, and every other association of
//!   the farm is left exactly as it was;
//! * an association is reached only through its own farm and queue, so a delete naming an
//!   unheld farm, an unheld queue, another farm's queue or a pair the queue is not
//!   associated with removes nothing and answers a 404 naming the level that is missing.
//!
//! The harness (`Emulator`) and the fleet helpers are the ones the other suites drive the
//! service with, so these tests go through routing, deserialization, constraint validation,
//! the handler and serialization, exactly as the router does in production.

use std::time::Duration;

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::{create_fleet, get_fleet_path};

/// Long enough for the `STOP_SCHEDULING_AND_* -> STOPPED` move to have come due: the
/// service owes it a few seconds after the update that asks for the stop, and nothing in
/// the emulator sleeps for it.
const PAST_THE_DRAIN: Duration = Duration::from_millis(3_400);

/// `PUT /2023-10-12/farms/{farmId}/queue-fleet-associations` — where an association is
/// created.
fn create_association_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations")
}

/// `GET|PATCH|DELETE /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId}`
/// — where one association is read, updated and deleted.
fn association_path(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations/{queue_id}/{fleet_id}")
}

/// The composite id this service reports for a queue-fleet association: the three ids of
/// its path, since the model mints the association no id of its own.
///
/// `pub(crate)` so every suite in this test target names an association the same way.
pub(crate) fn association_id(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("{farm_id}, {queue_id} and {fleet_id}")
}

/// Associate `queue_id` with `fleet_id` under `farm_id`.
async fn associate(emulator: &Emulator, farm_id: &str, queue_id: &str, fleet_id: &str) {
    let (status, body) = emulator
        .call(
            "PUT",
            &create_association_path(farm_id),
            Some(json!({"queueId": queue_id, "fleetId": fleet_id})),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateQueueFleetAssociation {queue_id}/{fleet_id}: expected 200, got {status} with {body}"
    );
}

/// Ask the association of `queue_id` and `fleet_id` for `status`.
async fn ask_for_status(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
    status: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PATCH",
            &association_path(farm_id, queue_id, fleet_id),
            Some(json!({"status": status})),
            None,
        )
        .await
}

/// Ask for `status` and fail the test unless the update was accepted.
async fn stop(emulator: &Emulator, farm_id: &str, queue_id: &str, fleet_id: &str, status: &str) {
    let (got, body) = ask_for_status(emulator, farm_id, queue_id, fleet_id, status).await;
    assert_eq!(
        got, 200,
        "UpdateQueueFleetAssociation {queue_id}/{fleet_id} to {status}: expected 200, got {got} with {body}"
    );
}

/// `DELETE` the association of `queue_id` and `fleet_id` under `farm_id`.
async fn delete_association(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &association_path(farm_id, queue_id, fleet_id),
            None,
            None,
        )
        .await
}

/// The status a read of the association reports, failing the test if it can't be read.
async fn read_status(emulator: &Emulator, farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    let (got, body) = emulator
        .get(&association_path(farm_id, queue_id, fleet_id))
        .await;
    assert_eq!(
        got, 200,
        "GetQueueFleetAssociation {queue_id}/{fleet_id}: expected 200, got {got} with {body}"
    );
    string_at(&body, "status")
}

#[tokio::test]
async fn delete_queue_fleet_association_refuses_an_association_that_has_not_stopped() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("still working").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "the fleet").await;

    // The rule: the delete is refused while the association still holds the queue's work on
    // the fleet. The rows are every value of `QueueFleetAssociationStatus`
    // (`smithy_query(query="shape", shape="QueueFleetAssociationStatus")`) other than the
    // one that holds none of it: the association scheduling, and each way of draining it,
    // deleted while that drain is still underway.
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("ACTIVE", None),
        (
            "STOP_SCHEDULING_AND_COMPLETE_TASKS",
            Some("STOP_SCHEDULING_AND_COMPLETE_TASKS"),
        ),
        (
            "STOP_SCHEDULING_AND_CANCEL_TASKS",
            Some("STOP_SCHEDULING_AND_CANCEL_TASKS"),
        ),
    ];

    for (case, asked_for) in cases {
        let queue_id = emulator.queue(&farm_id, case).await;
        associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
        if let Some(status) = asked_for {
            stop(&emulator, &farm_id, &queue_id, &fleet_id, status).await;
        }

        let (got, body) = delete_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
        assert_eq!(
            got, 409,
            "case {case:?}: expected DeleteQueueFleetAssociation to answer 409, got {got} with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "message"),
                string_at(&body, "reason"),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId")
            ),
            (
                "ConflictException".to_string(),
                format!("QueueFleetAssociation is in {case} status and cannot be deleted."),
                "STATUS_CONFLICT".to_string(),
                "queue-fleet-association".to_string(),
                association_id(&farm_id, &queue_id, &fleet_id)
            ),
            "case {case:?}: expected the 409 to name the association and the status it is in, got {body}"
        );

        // The refusal removed nothing and moved nothing: the association is still there, in
        // the status it was refused in.
        assert_eq!(
            read_status(&emulator, &farm_id, &queue_id, &fleet_id).await,
            case,
            "case {case:?}: expected the refused association to be left as it was"
        );
    }
}

#[tokio::test]
async fn delete_queue_fleet_association_removes_a_stopped_association() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("stopping").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "the fleet").await;

    // The rule: an association that has stopped is removed. The rows differ along the route
    // it took to `STOPPED` — which `UpdateQueueFleetAssociationStatus` stop was asked for
    // (`smithy_query(query="shape", shape="UpdateQueueFleetAssociationStatus")`), whether
    // the same stop was asked for twice, and whether the association was put back to
    // `ACTIVE` before being stopped again.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "stopped completing its tasks",
            vec!["STOP_SCHEDULING_AND_COMPLETE_TASKS"],
        ),
        (
            "stopped cancelling its tasks",
            vec!["STOP_SCHEDULING_AND_CANCEL_TASKS"],
        ),
        (
            "the same stop asked for twice",
            vec![
                "STOP_SCHEDULING_AND_CANCEL_TASKS",
                "STOP_SCHEDULING_AND_CANCEL_TASKS",
            ],
        ),
        (
            "reactivated, then stopped again",
            vec![
                "STOP_SCHEDULING_AND_COMPLETE_TASKS",
                "ACTIVE",
                "STOP_SCHEDULING_AND_CANCEL_TASKS",
            ],
        ),
    ];

    let mut queue_ids = Vec::new();
    for (case, asked_for) in &cases {
        let queue_id = emulator.queue(&farm_id, case).await;
        associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
        for status in asked_for {
            stop(&emulator, &farm_id, &queue_id, &fleet_id, status).await;
        }
        queue_ids.push(queue_id);
    }
    // One association nothing in this test deletes, so the delete is shown to remove the
    // one pair its path names.
    let bystander = emulator.queue(&farm_id, "untouched").await;
    associate(&emulator, &farm_id, &bystander, &fleet_id).await;

    // Every drain above is owed by the clock, so waiting once covers all of them.
    tokio::time::sleep(PAST_THE_DRAIN).await;

    for ((case, _), queue_id) in cases.iter().zip(queue_ids.iter()) {
        assert_eq!(
            read_status(&emulator, &farm_id, queue_id, &fleet_id).await,
            "STOPPED",
            "case {case:?}: expected the drain to have come due"
        );
        // A drain is the queue's work being stopped on the fleet, so an association that has
        // already stopped has none left to stop: asking it to stop again starts no second
        // drain, and the delete that follows is still allowed.
        stop(
            &emulator,
            &farm_id,
            queue_id,
            &fleet_id,
            "STOP_SCHEDULING_AND_COMPLETE_TASKS",
        )
        .await;
        assert_eq!(
            read_status(&emulator, &farm_id, queue_id, &fleet_id).await,
            "STOPPED",
            "case {case:?}: expected a stopped association to stay stopped"
        );

        let (got, body) = delete_association(&emulator, &farm_id, queue_id, &fleet_id).await;
        assert_eq!(
            got, 200,
            "case {case:?}: expected DeleteQueueFleetAssociation to answer 200, got {got} with {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
            "case {case:?}: expected an empty DeleteQueueFleetAssociation response, got {body}"
        );

        // The association is gone: reading it back is the 404 naming it.
        let (got, read) = emulator
            .get(&association_path(&farm_id, queue_id, &fleet_id))
            .await;
        let missing = association_id(&farm_id, queue_id, &fleet_id);
        assert_eq!(
            (
                got,
                error_code(&read),
                string_at(&read, "message"),
                string_at(&read, "resourceType"),
                string_at(&read, "resourceId")
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                format!(
                    "Resource of type queue-fleet-association with id {missing} does not exist."
                ),
                "queue-fleet-association".to_string(),
                missing.clone()
            ),
            "case {case:?}: expected the deleted association to be gone, got {got} with {read}"
        );

        // The association nothing deleted is still there, still scheduling: the delete took
        // only the pair its path named.
        assert_eq!(
            read_status(&emulator, &farm_id, &bystander, &fleet_id).await,
            "ACTIVE",
            "case {case:?}: expected another queue's association to be untouched"
        );
    }
}

#[tokio::test]
async fn delete_queue_fleet_association_refuses_a_pair_it_does_not_hold() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let queue_id = emulator.queue(&farm_id, "associated").await;
    let unassociated = emulator.queue(&farm_id, "unassociated").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "owned").await;
    let other_fleet_id = create_fleet(&emulator, &farm_id, "unassociated fleet").await;
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
    stop(
        &emulator,
        &farm_id,
        &queue_id,
        &fleet_id,
        "STOP_SCHEDULING_AND_CANCEL_TASKS",
    )
    .await;

    let absent_farm = absent_id("farm", "00000000000000000000000000000031");
    let absent_queue = absent_id("queue", "00000000000000000000000000000032");
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000033");

    // The rule: an association hangs off its own farm and queue and joins them to a fleet
    // of that farm, so a delete naming another farm, an unheld farm, an unheld queue, an
    // unheld fleet or a held fleet that queue is not associated with removes nothing and
    // answers a 404 naming the level that is missing. The rows differ along which level that
    // is.
    let cases: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "a queue of another farm",
            other_farm_id.clone(),
            queue_id.clone(),
            fleet_id.clone(),
            "queue",
            queue_id.clone(),
        ),
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
            "a fleet this queue was never associated with",
            farm_id.clone(),
            queue_id.clone(),
            other_fleet_id.clone(),
            "queue-fleet-association",
            association_id(&farm_id, &queue_id, &other_fleet_id),
        ),
        (
            "a queue that is associated with nothing",
            farm_id.clone(),
            unassociated.clone(),
            fleet_id.clone(),
            "queue-fleet-association",
            association_id(&farm_id, &unassociated, &fleet_id),
        ),
    ];

    for (case, farm, queue, fleet, resource_type, resource_id) in cases {
        let (got, body) = delete_association(&emulator, &farm, &queue, &fleet).await;
        assert_eq!(
            got, 404,
            "case {case:?}: expected DeleteQueueFleetAssociation to answer 404, got {got} with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "message"),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId")
            ),
            (
                "ResourceNotFoundException".to_string(),
                format!(
                    "Resource of type {resource_type} with id {resource_id} does not exist."
                ),
                resource_type.to_string(),
                resource_id.clone()
            ),
            "case {case:?}: expected the 404 to name what is missing, got {body}"
        );
    }

    // None of those refusals removed anything: the association is still readable through
    // its own farm and queue, and is deletable once its drain has come due.
    tokio::time::sleep(PAST_THE_DRAIN).await;
    assert_eq!(
        read_status(&emulator, &farm_id, &queue_id, &fleet_id).await,
        "STOPPED",
        "after the refusals: expected the association to still be held and to have stopped"
    );
    let (got, body) = delete_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        got, 200,
        "after the refusals: expected the stopped association to be deletable, got {got} with {body}"
    );
    // And the delete does not come back a second time: the pair is now one nothing holds.
    let (got, body) = delete_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        (got, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "the second delete: expected a 404 for the pair already deleted, got {got} with {body}"
    );
}

#[tokio::test]
async fn deleting_the_association_releases_the_fleet() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("releasing").await;
    let queue_id = emulator.queue(&farm_id, "holding the fleet").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "held").await;
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;

    // While the association is stored, the fleet it names is in use by the queue. Another
    // operation's read of the store is what reports that, which is what makes the removal
    // below a real one rather than a success this handler only claims.
    let (got, body) = emulator
        .call("DELETE", &get_fleet_path(&farm_id, &fleet_id), None, None)
        .await;
    assert_eq!(
        (got, error_code(&body)),
        (409, "ConflictException".to_string()),
        "before the association is deleted: expected DeleteFleet to be refused, got {got} with {body}"
    );

    stop(
        &emulator,
        &farm_id,
        &queue_id,
        &fleet_id,
        "STOP_SCHEDULING_AND_COMPLETE_TASKS",
    )
    .await;
    tokio::time::sleep(PAST_THE_DRAIN).await;
    let (got, body) = delete_association(&emulator, &farm_id, &queue_id, &fleet_id).await;
    assert_eq!(
        got, 200,
        "expected the stopped association to be deleted, got {got} with {body}"
    );

    let (got, body) = emulator
        .call("DELETE", &get_fleet_path(&farm_id, &fleet_id), None, None)
        .await;
    assert_eq!(
        got, 200,
        "after the association is deleted: expected DeleteFleet to answer 200, got {got} with {body}"
    );
}
