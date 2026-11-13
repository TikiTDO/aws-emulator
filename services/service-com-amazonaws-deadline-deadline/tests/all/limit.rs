// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the limit slice of AWS Deadline Cloud: `CreateLimit` and
//! `GetLimit`.
//!
//! The rules under test:
//!
//! * a limit is created under its farm and `GetLimit` reports the values that create
//!   persisted, under a minted `^limit-[0-9a-f]{32}$` id, with `currentCount` at 0 while
//!   nothing holds any of its resources;
//! * `maxCount` must not be 0 — every other value of the modeled range is accepted, and 0
//!   is refused with HTTP 400 `ValidationException`, storing nothing;
//! * a limit hangs off its farm, so a create naming a farm this emulator does not hold
//!   creates nothing, and a read naming another farm does not find it;
//! * `clientToken` deduplicates a retried create.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so these tests go through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, is_resource_id, string_at, Emulator};

/// Modeled `@range` bounds of `MaxCount`, the target of `maxCount` (`range -1..2147483647`),
/// from `smithy_query(query="shape", shape="MaxCount")`.
const MIN_MAX_COUNT: i64 = -1;
const MAX_MAX_COUNT: i64 = 2_147_483_647;

/// Modeled `@length` bounds of `ResourceName`, the target of `displayName` (`len 1..100`).
const MIN_DISPLAY_NAME: usize = 1;
const MAX_DISPLAY_NAME: usize = 100;

/// Modeled `@length` bounds of `Description`, the target of `description` (`len 0..100`).
const MAX_DESCRIPTION: usize = 100;

/// Modeled `@length` maximum of `AmountRequirementName`, the target of
/// `amountRequirementName` (`len ..1024`, so the empty string satisfies it).
const MAX_AMOUNT_REQUIREMENT_NAME: usize = 1024;

/// `POST`/`GET` collection path of the limit resource, from
/// `smithy_query(query="operation", operation="CreateLimit")`.
fn limits_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits")
}

/// Single-limit path, from `smithy_query(query="operation", operation="GetLimit")`.
fn limit_path(farm_id: &str, limit_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits/{limit_id}")
}

/// A `CreateLimit` body carrying every member the model requires.
fn create_body(display_name: &str, amount_requirement_name: &str, max_count: i64) -> Value {
    json!({
        "displayName": display_name,
        "amountRequirementName": amount_requirement_name,
        "maxCount": max_count,
    })
}

/// Create a limit under `farm_id` and return its id.
///
/// `pub(crate)` so every suite in this test target makes a limit the same way.
pub(crate) async fn create_limit(emulator: &Emulator, farm_id: &str, request: Value) -> String {
    let (status, body) = emulator.post(&limits_path(farm_id), request).await;
    assert_eq!(
        status, 200,
        "CreateLimit: expected 200, got {status} with {body}"
    );
    string_at(&body, "limitId")
}

#[tokio::test]
async fn create_limit_then_get_limit_returns_what_was_created() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Limit farm").await;

    // The rule: GetLimit reports the values CreateLimit persisted, and a limit nothing has
    // acquired reports `currentCount` 0. The rows differ along every member the create
    // accepts — `maxCount` at each published bound of its modeled range and inside it,
    // `displayName` at both length bounds, `amountRequirementName` empty, ordinary and at
    // its length maximum, and `description` absent, empty and at its length maximum.
    let cases: Vec<(&str, Value)> = vec![
        (
            "maxCount at the modeled minimum, no description member at all",
            create_body("Unrestricted licenses", "maya", MIN_MAX_COUNT),
        ),
        (
            "maxCount just above the forbidden 0",
            json!({
                "displayName": "One license",
                "amountRequirementName": "amount.nuke",
                "maxCount": 1,
                "description": "",
            }),
        ),
        (
            "maxCount at the modeled maximum, description at its length maximum",
            json!({
                "displayName": "All the licenses",
                "amountRequirementName": "amount.houdini",
                "maxCount": MAX_MAX_COUNT,
                "description": "d".repeat(MAX_DESCRIPTION),
            }),
        ),
        (
            "an empty amountRequirementName",
            create_body("Empty requirement name", "", 7),
        ),
        (
            "an amountRequirementName at its modeled length maximum",
            create_body(
                "Long requirement name",
                &"r".repeat(MAX_AMOUNT_REQUIREMENT_NAME),
                12,
            ),
        ),
        (
            "a display name at the modeled minimum length",
            create_body(&"A".repeat(MIN_DISPLAY_NAME), "amount.blender", 3),
        ),
        (
            "a display name at the modeled maximum length",
            create_body(&"Z".repeat(MAX_DISPLAY_NAME), "amount.arnold", 2_000_000),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let (status, created) = emulator.post(&limits_path(&farm_id), request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLimit to answer 200, got {status} with {created}"
        );
        let limit_id = string_at(&created, "limitId");
        assert!(
            is_resource_id(&limit_id, "limit"),
            "case {case:?}: expected a ^limit-[0-9a-f]{{32}}$ id, got {limit_id:?}"
        );
        assert!(
            !minted.contains(&limit_id),
            "case {case:?}: id {limit_id} was already minted for another limit"
        );
        minted.push(limit_id.clone());

        let (status, limit) = emulator.get(&limit_path(&farm_id, &limit_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLimit to answer 200, got {status} with {limit}"
        );
        assert_eq!(
            (
                string_at(&limit, "limitId"),
                string_at(&limit, "farmId"),
                string_at(&limit, "displayName"),
                string_at(&limit, "amountRequirementName"),
                limit.get("maxCount").and_then(Value::as_i64),
            ),
            (
                limit_id.clone(),
                farm_id.clone(),
                string_at(request, "displayName"),
                string_at(request, "amountRequirementName"),
                request.get("maxCount").and_then(Value::as_i64),
            ),
            "case {case:?}: expected the created values to be reported, got {limit}"
        );
        // `description` carries a modeled default of "", so a create that leaves it out
        // persists the empty description.
        assert_eq!(
            string_at(&limit, "description"),
            request
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            "case {case:?}: expected the created description to be reported, got {limit}"
        );
        assert_eq!(
            limit.get("currentCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: expected a fresh limit to report currentCount 0, got {limit}"
        );
        assert!(
            !string_at(&limit, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {limit}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            limit.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {limit}"
        );
        assert!(
            limit.get("updatedAt").is_none() && limit.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {limit}"
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator.get(&limit_path(&farm_id, &limit_id)).await;
        assert_eq!(
            again, limit,
            "case {case:?}: a second read answered differently, got {again}"
        );
    }
}

#[tokio::test]
async fn a_max_count_outside_what_a_limit_may_carry_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Bounded limit farm").await;

    // The rule: `maxCount` must not be 0, and it stays inside the modeled range, so each
    // of those values is refused with a 400 ValidationException and stores nothing. The
    // values AT the published bounds (-1 and 2147483647) are accepted by the table above.
    let cases: Vec<(&str, i64)> = vec![
        ("the forbidden maxCount 0", 0),
        ("one below the modeled minimum", MIN_MAX_COUNT - 1),
        ("one above the modeled maximum", MAX_MAX_COUNT + 1),
    ];
    for (case, max_count) in &cases {
        let (status, body) = emulator
            .post(
                &limits_path(&farm_id),
                create_body("Refused limit", "amount.maya", *max_count),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateLimit to answer 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert!(
            body.get("limitId").is_none(),
            "case {case:?}: a refused create must store no limit, got {body}"
        );
    }
}

#[tokio::test]
async fn a_limit_is_reachable_only_through_its_own_farm() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Owning farm").await;
    let other_farm_id = emulator.farm("Other farm").await;
    let limit_id = create_limit(
        &emulator,
        &farm_id,
        create_body("Floating licenses", "amount.maya", 5),
    )
    .await;
    let absent_farm = absent_id("farm", "00000000000000000000000000000001");
    let absent_limit = absent_id("limit", "00000000000000000000000000000002");

    // The rule: a limit hangs off its farm, so a read naming another farm, an unheld farm
    // or an unheld limit is a 404 naming the level that is missing.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a limit of another farm",
            other_farm_id,
            limit_id.clone(),
            "limit",
            limit_id.clone(),
        ),
        (
            "an unheld limit under a held farm",
            farm_id.clone(),
            absent_limit.clone(),
            "limit",
            absent_limit,
        ),
        (
            "a limit under an unheld farm",
            absent_farm.clone(),
            limit_id,
            "farm",
            absent_farm.clone(),
        ),
    ];
    for (case, farm, limit, resource_type, resource_id) in &cases {
        let (status, body) = emulator.get(&limit_path(farm, limit)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetLimit to answer 404, got {status} with {body}"
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
            &limits_path(&absent_farm),
            create_body("Orphan limit", "amount.maya", 4),
        )
        .await;
    assert_eq!(
        status, 404,
        "CreateLimit under an unheld farm: expected 404, got {status} with {body}"
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
        "CreateLimit under an unheld farm: expected the 404 to name the farm, got {body}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_creates_one_limit() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Idempotent limit farm").await;
    let other_farm_id = emulator.farm("Second idempotent limit farm").await;
    let request = create_body("Retried", "amount.maya", 6);

    // The rule: `clientToken` is an idempotency token, so a retry of the same create
    // answers with the resource the first call made. The rows differ along what changes
    // between the two calls: nothing, the token, or the farm the token is used under.
    let (_, first) = emulator
        .call(
            "POST",
            &limits_path(&farm_id),
            Some(request.clone()),
            Some("limit-token"),
        )
        .await;
    let first_id = string_at(&first, "limitId");

    let cases: Vec<(&str, String, &str, bool)> = vec![
        (
            "the same token on the same farm",
            farm_id.clone(),
            "limit-token",
            true,
        ),
        (
            "a fresh token on the same farm",
            farm_id.clone(),
            "another-limit-token",
            false,
        ),
        (
            "the same token on another farm",
            other_farm_id,
            "limit-token",
            false,
        ),
    ];
    for (case, farm, token, expect_same) in &cases {
        let (status, body) = emulator
            .call("POST", &limits_path(farm), Some(request.clone()), Some(token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let id = string_at(&body, "limitId");
        assert_eq!(
            id == first_id,
            *expect_same,
            "case {case:?}: expected same-as-first={expect_same}, got {id} against {first_id}"
        );
    }

    // Exactly one limit answers at the deduplicated id, and it is the one the first call
    // created.
    let (status, limit) = emulator.get(&limit_path(&farm_id, &first_id)).await;
    assert_eq!(
        status, 200,
        "expected the deduplicated limit to be readable, got {status} with {limit}"
    );
    assert_eq!(
        (
            string_at(&limit, "displayName"),
            limit.get("maxCount").and_then(Value::as_i64),
        ),
        (
            string_at(&request, "displayName"),
            request.get("maxCount").and_then(Value::as_i64),
        ),
        "expected the first call's values, got {limit}"
    );
}
