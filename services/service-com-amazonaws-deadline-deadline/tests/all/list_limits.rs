// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLimits`.
//!
//! The rules under test:
//!
//! * `ListLimits` reports the limits of the farm in its path and nothing else, each
//!   summary carrying exactly what the farm's stored limit carries (the same values
//!   `GetLimit` reports), with `currentCount` at 0 while nothing holds any of its
//!   resources;
//! * paging hands back every limit exactly once in at most `maxResults` per page and
//!   stops handing back a token at the end; an unknown token is a 400
//!   `ValidationException`;
//! * a limit hangs off its farm, so listing under a farm this emulator does not hold is a
//!   404 `ResourceNotFoundException` naming the farm, and a farm with no limits lists
//!   none;
//! * listing is a read: a second call answers the same, and `GetLimit` after it is
//!   unchanged.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so these tests go through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, is_resource_id, string_at, Emulator};

/// Modeled `@range` bounds of `MaxResults`, the target of `maxResults`
/// (`range 1..100`, default 100), from
/// `smithy_query(query="operation", operation="ListLimits")`.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 100;

/// Collection path of the limit resource, from
/// `smithy_query(query="operation", operation="ListLimits")`:
/// `GET /2023-10-12/farms/{farmId}/limits`.
fn limits_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits")
}

/// Single-limit path, from `smithy_query(query="operation", operation="GetLimit")`.
fn limit_path(farm_id: &str, limit_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/limits/{limit_id}")
}

/// Create a limit under `farm_id` from `request` and return its id.
async fn create_limit(emulator: &Emulator, farm_id: &str, request: Value) -> String {
    let (status, body) = emulator.post(&limits_path(farm_id), request).await;
    assert_eq!(
        status, 200,
        "CreateLimit: expected 200, got {status} with {body}"
    );
    string_at(&body, "limitId")
}

/// The `limits` array of a successful `ListLimits` answer.
fn limits_of(case: &str, body: &Value) -> Vec<Value> {
    body["limits"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a limits list in {body}"))
        .clone()
}

/// Every member a `LimitSummary` carries, from
/// `smithy_query(query="shape", shape="LimitSummary")`, as a comparable value. `updatedAt`
/// and `updatedBy` are the only optional ones, so an absent member is `Value::Null` here.
fn summary_members(item: &Value) -> Vec<(&'static str, Value)> {
    [
        "farmId",
        "limitId",
        "currentCount",
        "createdAt",
        "createdBy",
        "updatedAt",
        "updatedBy",
        "displayName",
        "amountRequirementName",
        "maxCount",
    ]
    .into_iter()
    .map(|member| (member, item.get(member).cloned().unwrap_or(Value::Null)))
    .collect()
}

/// Walk every page of `ListLimits` for `farm_id` at `max_results` and return the ids in
/// the order they were handed back.
async fn page_all(emulator: &Emulator, case: &str, farm_id: &str, max_results: i64) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
            None => format!("?maxResults={max_results}"),
        };
        let (status, body) = emulator
            .get(&format!("{}{query}", limits_path(farm_id)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListLimits to answer 200, got {status} with {body}"
        );
        let page = limits_of(case, &body);
        assert!(
            page.len() as i64 <= max_results,
            "case {case:?}: expected at most {max_results} limits on a page, got {}",
            page.len()
        );
        for item in &page {
            seen.push(string_at(item, "limitId"));
        }
        pages += 1;
        assert!(
            pages <= seen.len() + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn list_limits_reports_every_stored_member_of_each_limit() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Listing farm").await;

    // The rule: each summary carries what the farm's stored limit carries. The rows differ
    // along the members a create accepts — `maxCount` at the published bounds of its
    // modeled range and inside it, `amountRequirementName` empty and ordinary, and
    // `description` (which a summary does not carry) present and absent.
    let cases: Vec<(&str, Value)> = vec![
        (
            "maxCount at the modeled minimum, no description",
            json!({
                "displayName": "Unrestricted licenses",
                "amountRequirementName": "amount.maya",
                "maxCount": -1,
            }),
        ),
        (
            "maxCount just above the forbidden 0, empty requirement name",
            json!({
                "displayName": "One license",
                "amountRequirementName": "",
                "maxCount": 1,
                "description": "a single seat",
            }),
        ),
        (
            "maxCount at the modeled maximum",
            json!({
                "displayName": "All the licenses",
                "amountRequirementName": "amount.houdini",
                "maxCount": 2_147_483_647_i64,
            }),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, request) in &cases {
        created.push((case, create_limit(&emulator, &farm_id, request.clone()).await));
    }

    let (status, listed) = emulator.get(&limits_path(&farm_id)).await;
    assert_eq!(
        status, 200,
        "expected ListLimits to answer 200, got {status} with {listed}"
    );
    let items = limits_of("every stored limit", &listed);
    assert_eq!(
        items.len(),
        created.len(),
        "expected every created limit on the default page, got {listed}"
    );
    assert!(
        listed.get("nextToken").is_none(),
        "expected no nextToken once the list is exhausted, got {listed}"
    );

    for ((case, limit_id), request) in created.iter().zip(cases.iter().map(|(_, body)| body)) {
        let item = items
            .iter()
            .find(|item| string_at(item, "limitId") == *limit_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {limit_id} in {listed}"));
        assert!(
            is_resource_id(limit_id, "limit"),
            "case {case:?}: expected a ^limit-[0-9a-f]{{32}}$ id, got {limit_id:?}"
        );
        assert_eq!(
            (
                string_at(item, "farmId"),
                string_at(item, "displayName"),
                string_at(item, "amountRequirementName"),
                item.get("maxCount").and_then(Value::as_i64),
                item.get("currentCount").and_then(Value::as_i64),
            ),
            (
                farm_id.clone(),
                string_at(request, "displayName"),
                string_at(request, "amountRequirementName"),
                request.get("maxCount").and_then(Value::as_i64),
                Some(0),
            ),
            "case {case:?}: expected the stored values on the summary, got {item}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            item.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {item}"
        );
        assert!(
            !string_at(item, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {item}"
        );
        assert!(
            item.get("updatedAt").is_none() && item.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {item}"
        );
        // A summary is the stored limit, so every member it shares with `GetLimit` agrees.
        let (status, got) = emulator.get(&limit_path(&farm_id, limit_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetLimit to answer 200, got {status} with {got}"
        );
        assert_eq!(
            summary_members(item),
            summary_members(&got),
            "case {case:?}: expected the summary to agree with GetLimit, got {item} against {got}"
        );
    }

    // A read does not change what the next reader sees.
    let (_, again) = emulator.get(&limits_path(&farm_id)).await;
    assert_eq!(
        again, listed,
        "a second ListLimits answered differently, got {again}"
    );
}

#[tokio::test]
async fn list_limits_pages_over_every_limit_exactly_once() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Paging farm").await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        created.push(
            create_limit(
                &emulator,
                &farm_id,
                json!({
                    "displayName": format!("page-{index}"),
                    "amountRequirementName": format!("amount.page{index}"),
                    "maxCount": index + 1,
                }),
            )
            .await,
        );
    }
    let mut want = created.clone();
    want.sort();

    // The rule: paging returns every limit exactly once, in at most `maxResults` per page,
    // and stops handing back a token at the end. The rows are page sizes that divide the
    // set evenly, unevenly and exceed it, plus both published bounds of `maxResults`.
    for max_results in [MIN_MAX_RESULTS, 2, 3, 7, 50, MAX_MAX_RESULTS] {
        let case = format!("maxResults {max_results}");
        let seen = page_all(&emulator, &case, &farm_id, max_results).await;
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, want,
            "case {case:?}: expected every limit exactly once, got {seen:?}"
        );
        // Paging order is stable, so the same walk at the same page size repeats itself.
        let again = page_all(&emulator, &case, &farm_id, max_results).await;
        assert_eq!(
            again, seen,
            "case {case:?}: expected a stable paging order, got {again:?} after {seen:?}"
        );
    }

    // Every page size walks the set in one order: the order the default page reports.
    let (_, listed) = emulator.get(&limits_path(&farm_id)).await;
    let default_order: Vec<String> = limits_of("default page", &listed)
        .iter()
        .map(|item| string_at(item, "limitId"))
        .collect();
    for max_results in [MIN_MAX_RESULTS, 3, MAX_MAX_RESULTS] {
        let case = format!("order at maxResults {max_results}");
        let seen = page_all(&emulator, &case, &farm_id, max_results).await;
        assert_eq!(
            seen, default_order,
            "case {case:?}: expected the same order as the unpaged list {default_order:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modeled_range_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("Bounded paging farm").await;
    create_limit(
        &emulator,
        &farm_id,
        json!({
            "displayName": "Bounded",
            "amountRequirementName": "amount.maya",
            "maxCount": 5,
        }),
    )
    .await;

    // The rule: `maxResults` stays inside its modeled range, so a value outside it is a
    // 400 ValidationException rather than a page. The values AT the published bounds are
    // walked by the paging table above.
    let cases: Vec<(&str, i64)> = vec![
        ("one below the modeled minimum", MIN_MAX_RESULTS - 1),
        ("one above the modeled maximum", MAX_MAX_RESULTS + 1),
    ];
    for (case, max_results) in &cases {
        let (status, body) = emulator
            .get(&format!(
                "{}?maxResults={max_results}",
                limits_path(&farm_id)
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ListLimits to answer 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }

    // An unknown token is a client error, not an empty page.
    let unknown = absent_id("limit", "00000000000000000000000000000009");
    let (status, body) = emulator
        .get(&format!("{}?nextToken={unknown}", limits_path(&farm_id)))
        .await;
    assert_eq!(
        status, 400,
        "unknown nextToken: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "unknown nextToken: expected a ValidationException, got {body}"
    );
}

#[tokio::test]
async fn list_limits_reports_only_the_limits_of_its_own_farm() {
    let emulator = Emulator::new();
    let left = emulator.farm("Left farm").await;
    let right = emulator.farm("Right farm").await;
    let empty = emulator.farm("Empty farm").await;

    let mut left_limits = vec![
        create_limit(
            &emulator,
            &left,
            json!({"displayName": "left-one", "amountRequirementName": "amount.a", "maxCount": 1}),
        )
        .await,
        create_limit(
            &emulator,
            &left,
            json!({"displayName": "left-two", "amountRequirementName": "amount.b", "maxCount": 2}),
        )
        .await,
    ];
    left_limits.sort();
    let right_limits = vec![
        create_limit(
            &emulator,
            &right,
            json!({"displayName": "right-one", "amountRequirementName": "amount.c", "maxCount": 3}),
        )
        .await,
    ];

    // The rule: a limit hangs off its farm, so a list reports that farm's limits and no
    // other farm's — a farm holding none lists none.
    let cases: Vec<(&str, &String, Vec<String>)> = vec![
        ("the farm holding two limits", &left, left_limits),
        ("the farm holding one limit", &right, right_limits),
        ("a farm holding no limits", &empty, Vec::new()),
    ];
    for (case, farm_id, expected) in &cases {
        let (status, body) = emulator.get(&limits_path(farm_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListLimits to answer 200, got {status} with {body}"
        );
        let mut listed: Vec<String> = limits_of(case, &body)
            .iter()
            .map(|item| string_at(item, "limitId"))
            .collect();
        listed.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            listed, want,
            "case {case:?}: expected exactly this farm's limits {want:?}, got {listed:?}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: expected no nextToken on a complete page, got {body}"
        );
    }

    // A list under a farm this emulator does not hold names the farm in a 404.
    let absent_farm = absent_id("farm", "00000000000000000000000000000001");
    let (status, body) = emulator.get(&limits_path(&absent_farm)).await;
    assert_eq!(
        status, 404,
        "an unheld farm: expected ListLimits to answer 404, got {status} with {body}"
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
        "an unheld farm: expected the 404 to name the farm, got {body}"
    );
}
