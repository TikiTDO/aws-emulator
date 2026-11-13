// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateQueueLimitAssociation` and `GetQueueLimitAssociation`.
//!
//! The rules under test:
//!
//! * a queue-limit association is stored under the queue its request names and reported
//!   back by the read of the same three ids, `ACTIVE` and carrying the principal and the
//!   instant the create stored, with no update stamps before anything updates it;
//! * the create is `@idempotent`, so a repeat of the same pair answers 200 and leaves the
//!   stored association exactly as the first call made it;
//! * two limits carrying the same `amountRequirementName` are not both associated with one
//!   queue: the second create is refused with HTTP 400 `ValidationException` and stores
//!   nothing, while the same pair of limits on two different queues is accepted;
//! * an association joins a queue and a limit OF ONE FARM, so a create or a read naming an
//!   unheld farm, an unheld queue, an unheld limit or another farm's queue or limit stores
//!   nothing and answers a 404 naming the level that is missing.
//!
//! The harness (`Emulator`) and the limit helper are the ones the other suites drive the
//! service with, so these tests go through routing, deserialization, constraint validation,
//! the handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::limit::create_limit;

/// `PUT /2023-10-12/farms/{farmId}/queue-limit-associations` — where an association is
/// created, from `smithy_query(query="operation", operation="CreateQueueLimitAssociation")`.
fn create_association_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-limit-associations")
}

/// `GET /2023-10-12/farms/{farmId}/queue-limit-associations/{queueId}/{limitId}` — where
/// one association is read, from
/// `smithy_query(query="operation", operation="GetQueueLimitAssociation")`.
fn association_path(farm_id: &str, queue_id: &str, limit_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-limit-associations/{queue_id}/{limit_id}")
}

/// The composite id this service reports for a queue-limit association: the three ids of
/// its path, since the model mints the association no id of its own.
fn association_id(farm_id: &str, queue_id: &str, limit_id: &str) -> String {
    format!("{farm_id}, {queue_id} and {limit_id}")
}

/// A `CreateLimit` body for a limit declaring `amount_requirement_name`.
fn limit_body(display_name: &str, amount_requirement_name: &str) -> Value {
    json!({
        "displayName": display_name,
        "amountRequirementName": amount_requirement_name,
        "maxCount": 5,
    })
}

/// Associate `queue_id` with `limit_id` under `farm_id`.
async fn associate(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    limit_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &create_association_path(farm_id),
            Some(json!({"queueId": queue_id, "limitId": limit_id})),
            None,
        )
        .await
}

/// Associate the pair and fail the test unless the create was accepted.
async fn associate_ok(emulator: &Emulator, farm_id: &str, queue_id: &str, limit_id: &str) {
    let (status, body) = associate(emulator, farm_id, queue_id, limit_id).await;
    assert_eq!(
        status, 200,
        "CreateQueueLimitAssociation {queue_id}/{limit_id}: expected 200, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "CreateQueueLimitAssociation {queue_id}/{limit_id}: expected an empty response, got {body}"
    );
}

/// Read one association back, failing the test if it can't be read.
async fn read_association(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    limit_id: &str,
) -> Value {
    let (status, body) = emulator
        .get(&association_path(farm_id, queue_id, limit_id))
        .await;
    assert_eq!(
        status, 200,
        "GetQueueLimitAssociation {queue_id}/{limit_id}: expected 200, got {status} with {body}"
    );
    body
}

#[tokio::test]
async fn create_queue_limit_association_then_get_reports_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Association farm").await;
    let first_queue = emulator.queue(&farm_id, "first queue").await;
    let second_queue = emulator.queue(&farm_id, "second queue").await;
    let maya = create_limit(&emulator, &farm_id, limit_body("Maya", "amount.maya")).await;
    let nuke = create_limit(&emulator, &farm_id, limit_body("Nuke", "amount.nuke")).await;
    let houdini = create_limit(
        &emulator,
        &farm_id,
        limit_body("Houdini", &"r".repeat(1024)),
    )
    .await;

    // The rule: the read of the three ids reports the association the create stored. The
    // rows differ along how the pairs fan out — one queue with several limits, one limit on
    // several queues, and a limit whose `amountRequirementName` sits at its modeled length
    // maximum (`AmountRequirementName, len ..1024`).
    let cases: Vec<(&str, &String, &String)> = vec![
        ("the first limit of a queue", &first_queue, &maya),
        ("a second limit of the same queue", &first_queue, &nuke),
        (
            "a limit with a maximum-length amountRequirementName",
            &first_queue,
            &houdini,
        ),
        ("the same limit on another queue", &second_queue, &maya),
    ];

    for (case, queue_id, limit_id) in &cases {
        associate_ok(&emulator, &farm_id, queue_id, limit_id).await;

        let body = read_association(&emulator, &farm_id, queue_id, limit_id).await;
        assert_eq!(
            (
                string_at(&body, "queueId"),
                string_at(&body, "limitId"),
                string_at(&body, "status"),
            ),
            (
                (*queue_id).clone(),
                (*limit_id).clone(),
                "ACTIVE".to_string(),
            ),
            "case {case:?}: expected the stored pair, active, got {body}"
        );
        assert!(
            !string_at(&body, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {body}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            body.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {body}"
        );
        assert!(
            body.get("updatedAt").is_none() && body.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {body}"
        );

        // A read does not change what the next reader sees.
        let again = read_association(&emulator, &farm_id, queue_id, limit_id).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read answered differently, got {again}"
        );
    }

    // Every pair above is still there afterwards: a later create adds an association, it
    // does not replace the ones the queue already holds.
    for (case, queue_id, limit_id) in &cases {
        let body = read_association(&emulator, &farm_id, queue_id, limit_id).await;
        assert_eq!(
            string_at(&body, "limitId"),
            (*limit_id).clone(),
            "case {case:?}: expected every created association to still be held, got {body}"
        );
    }

    // A limit the queue was never associated with is the association that can't be found,
    // which is what makes the reads above assertions about stored state.
    let unassociated = create_limit(
        &emulator,
        &farm_id,
        limit_body("Unassociated", "amount.blender"),
    )
    .await;
    let (status, body) = emulator
        .get(&association_path(&farm_id, &second_queue, &unassociated))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "a limit this queue was never associated with: expected a 404, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_repeated_create_of_the_same_pair_changes_nothing() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Idempotent association farm").await;
    let queue_id = emulator.queue(&farm_id, "retried").await;
    let limit_id = create_limit(&emulator, &farm_id, limit_body("Retried", "amount.maya")).await;

    associate_ok(&emulator, &farm_id, &queue_id, &limit_id).await;
    let first = read_association(&emulator, &farm_id, &queue_id, &limit_id).await;

    // The rule: the create is `@idempotent`, so a repeat of the same pair answers 200 and
    // leaves the stored association as the first call made it — including its `createdAt`,
    // which a re-created association would have moved. The rows are the number of repeats.
    for case in ["the second create", "the third create"] {
        associate_ok(&emulator, &farm_id, &queue_id, &limit_id).await;
        let again = read_association(&emulator, &farm_id, &queue_id, &limit_id).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the stored association to be untouched, got {again}"
        );
    }
}

#[tokio::test]
async fn two_limits_with_the_same_amount_requirement_name_are_not_both_associated() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Requirement farm").await;
    let queue_id = emulator.queue(&farm_id, "one requirement name").await;
    let other_queue = emulator.queue(&farm_id, "another queue").await;

    // The rule: a step declares a limit requirement by the limit's `amountRequirementName`,
    // so one queue holds at most one association per name — and the same two limits are
    // both associable with a DIFFERENT queue. The rows differ along the name: an ordinary
    // one, the empty name (`AmountRequirementName` is `len ..1024`, so "" satisfies it) and
    // one at that length maximum.
    let names: Vec<(&str, String)> = vec![
        ("an ordinary amountRequirementName", "amount.maya".to_string()),
        ("the empty amountRequirementName", String::new()),
        (
            "an amountRequirementName at its modeled length maximum",
            "r".repeat(1024),
        ),
    ];

    for (case, name) in &names {
        let held = create_limit(&emulator, &farm_id, limit_body("Held", name)).await;
        let clashing = create_limit(&emulator, &farm_id, limit_body("Clashing", name)).await;
        associate_ok(&emulator, &farm_id, &queue_id, &held).await;

        let (status, body) = associate(&emulator, &farm_id, &queue_id, &clashing).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the clashing create to answer 400, got {status} with {body}"
        );
        assert_eq!(
            (error_code(&body), string_at(&body, "reason")),
            (
                "ValidationException".to_string(),
                "FIELD_VALIDATION_FAILED".to_string()
            ),
            "case {case:?}: expected a field ValidationException, got {body}"
        );
        assert!(
            string_at(&body, "message").contains(&held)
                && string_at(&body, "message").contains(&queue_id),
            "case {case:?}: expected the message to name the limit already associated and \
             the queue, got {body}"
        );

        // The refused create stored nothing: the clashing pair is not an association.
        let (status, read) = emulator
            .get(&association_path(&farm_id, &queue_id, &clashing))
            .await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the refused pair to be unheld, got {status} with {read}"
        );
        // And it left the association already held exactly where it was.
        assert_eq!(
            string_at(
                &read_association(&emulator, &farm_id, &queue_id, &held).await,
                "status"
            ),
            "ACTIVE",
            "case {case:?}: expected the held association to be untouched"
        );

        // The name is unique PER QUEUE, so both limits associate with another queue.
        associate_ok(&emulator, &farm_id, &other_queue, &clashing).await;
    }
}

#[tokio::test]
async fn an_association_is_reached_only_through_its_own_farm_queue_and_limit() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Owning farm").await;
    let other_farm_id = emulator.farm("Stranger farm").await;
    let queue_id = emulator.queue(&farm_id, "associated").await;
    let other_farm_queue = emulator.queue(&other_farm_id, "elsewhere").await;
    let limit_id = create_limit(&emulator, &farm_id, limit_body("Owned", "amount.maya")).await;
    let other_farm_limit = create_limit(
        &emulator,
        &other_farm_id,
        limit_body("Elsewhere", "amount.maya"),
    )
    .await;
    associate_ok(&emulator, &farm_id, &queue_id, &limit_id).await;

    let absent_farm = absent_id("farm", "0000000000000000000000000000004a");
    let absent_queue = absent_id("queue", "0000000000000000000000000000004b");
    let absent_limit = absent_id("limit", "0000000000000000000000000000004c");

    // The rule: an association joins a queue and a limit of ONE farm, so a request naming
    // an unheld farm, an unheld queue, an unheld limit, another farm's queue or another
    // farm's limit answers a 404 naming the level that is missing. The rows differ along
    // which level that is.
    let cases: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "an unheld farm",
            absent_farm.clone(),
            queue_id.clone(),
            limit_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld queue",
            farm_id.clone(),
            absent_queue.clone(),
            limit_id.clone(),
            "queue",
            absent_queue.clone(),
        ),
        (
            "an unheld limit",
            farm_id.clone(),
            queue_id.clone(),
            absent_limit.clone(),
            "limit",
            absent_limit.clone(),
        ),
        (
            "a queue of another farm",
            farm_id.clone(),
            other_farm_queue.clone(),
            limit_id.clone(),
            "queue",
            other_farm_queue.clone(),
        ),
        (
            "a limit of another farm",
            farm_id.clone(),
            queue_id.clone(),
            other_farm_limit.clone(),
            "limit",
            other_farm_limit.clone(),
        ),
    ];

    for (case, farm, queue, limit, resource_type, resource_id) in &cases {
        let (status, body) = associate(&emulator, farm, queue, limit).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the create to answer 404, got {status} with {body}"
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
                (*resource_type).to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected the create's 404 to name what is missing, got {body}"
        );

        let (status, body) = emulator.get(&association_path(farm, queue, limit)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the read to answer 404, got {status} with {body}"
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
                (*resource_type).to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected the read's 404 to name what is missing, got {body}"
        );
    }

    // A held pair of one farm is not an association of the other: the read through the
    // other farm's own queue and limit is the 404 naming the association itself.
    let (status, body) = emulator
        .get(&association_path(
            &other_farm_id,
            &other_farm_queue,
            &other_farm_limit,
        ))
        .await;
    let missing = association_id(&other_farm_id, &other_farm_queue, &other_farm_limit);
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
                "Resource of type queue-limit-association with id {missing} does not exist."
            ),
            "queue-limit-association".to_string(),
            missing.clone(),
        ),
        "the other farm's own unassociated pair: expected the 404 to name the association, \
         got {status} with {body}"
    );

    // None of those refusals disturbed the one association this test did create.
    let body = read_association(&emulator, &farm_id, &queue_id, &limit_id).await;
    assert_eq!(
        string_at(&body, "status"),
        "ACTIVE",
        "after the refusals: expected the held association to be untouched, got {body}"
    );
}
