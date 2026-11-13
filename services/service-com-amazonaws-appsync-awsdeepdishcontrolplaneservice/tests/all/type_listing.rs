// SPDX-License-Identifier: CC0-1.0

//! The type listings of a GraphQL API, end to end over HTTP: `ListTypes` and
//! `ListTypesByAssociation`.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. a listing answers the types the addressed API holds, each through the same
//!    projection a single type read answers with, in the API's own order and cut into the
//!    page the request's `maxResults` and `nextToken` select — so walking the cursor from
//!    the first page visits every type exactly once, whatever the page size, and an
//!    update replaces a type's definition without moving its place;
//! 2. a `nextToken` that resumes nothing is a 400 `BadRequestException`, an `apiId` this
//!    account does not hold a 404 `NotFoundException`, and one that cannot name an API at
//!    all a 400;
//! 3. `ListTypesByAssociation` answers the types the SOURCE API of the named association
//!    holds, under the same paging rule, and refuses a pair that names no association of
//!    the addressed Merged API with the refusal the shared lookup rule states (a 404 for
//!    an `associationId` the Merged API does not hold and for a Merged API nothing holds,
//!    a 400 for an identifier that can name neither).
//!
//! The rows span every kind of type a GraphQL SDL definition declares, both
//! `TypeDefinitionFormat` values, every page size from one up to past the number of types
//! held (and the unspecified one), and a listing of an API that holds no type at all.
//!
//! The harness (`router`, `call`, `create_api`, `create_type`, `get_type`, the type
//! catalog `rows()` and the projection assertion) is the `graphql_types` suite's, and the
//! association pair is the `source_api_association` suite's, reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::graphql_api_lifecycle::router;
use super::graphql_types::{
    assert_projects, call, create_api, create_type, get_type, rendered_sdl, rows, Answer, FORMATS,
};
use super::source_api_association::{associate_path, create_pair};

/// The text a `BadRequestException` carries for a `nextToken` that resumes nothing.
/// Pinned here rather than read from the service, so a drift in the wording fails this
/// suite.
const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token given.";

/// The text a `NotFoundException` carries for a pair that names no source-API
/// association of the addressed Merged API.
const ASSOCIATION_NOT_FOUND: &str = "No source API association was found";

/// The text a `NotFoundException` carries for an API this account does not hold.
const API_NOT_FOUND: &str = "API not found.";

/// Send one `ListTypes` for `api_id`, in `format`, with the paging the row selects.
async fn list_types(
    router: &MultiServiceRouter,
    api_id: &str,
    format: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> Answer {
    let mut query = format!("format={format}");
    if let Some(max_results) = max_results {
        query.push_str(&format!("&maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push_str(&format!("&nextToken={next_token}"));
    }
    call(router, "GET", &format!("/v1/apis/{api_id}/types?{query}"), None).await
}

/// Send one `ListTypesByAssociation` for the pair, in `format`, with the paging the row
/// selects.
async fn list_types_by_association(
    router: &MultiServiceRouter,
    merged_api_identifier: &str,
    association_id: &str,
    format: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> Answer {
    let mut query = format!("format={format}");
    if let Some(max_results) = max_results {
        query.push_str(&format!("&maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push_str(&format!("&nextToken={next_token}"));
    }
    call(
        router,
        "GET",
        &format!(
            "/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}/types?{query}"
        ),
        None,
    )
    .await
}

/// The `message` of an error body, or the empty string when there is none.
fn message(answer: &Answer) -> &str {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The types one answer carries.
fn listed(answer: &Answer) -> Vec<Value> {
    answer.body["types"].as_array().cloned().unwrap_or_default()
}

/// Walk the whole listing from the first page with `max_results` per page, and return
/// every type reported, in the order the pages reported them.
async fn walk(
    router: &MultiServiceRouter,
    api_id: &str,
    format: &str,
    max_results: Option<usize>,
    case: &str,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per type held, plus the page that closes the walk: a cursor that would
    // need more than that is not resuming.
    for page in 0..=rows().len() + 1 {
        let answer = list_types(router, api_id, format, max_results, next_token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case} page {page}: expected ListTypes 200, got {answer:?}"
        );
        let items = listed(&answer);
        if let Some(size) = max_results {
            assert!(
                items.len() <= size,
                "case {case} page {page}: expected at most {size} types, got {}",
                items.len()
            );
        }
        collected.extend(items);
        match answer.body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {case}: the cursor never closed the walk after {collected:?}");
}

/// A listing answers every type the API holds, through the same projection a single read
/// answers with, in the API's own order and whatever the page size — and the cursor
/// visits each type exactly once.
#[tokio::test]
async fn a_listing_answers_every_type_the_api_holds_across_page_sizes() {
    let router = router();
    let api_id = create_api(&router, "type-listing-pages").await;
    let held = rows();

    for row in &held {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?}: expected 200, got {created:?}",
            row.name
        );
    }

    // The order is the API's own, which is the order the types were created in.
    let expected_names: Vec<&str> = held.iter().map(|row| row.name).collect();

    // Every page size from one up to past the number of types held, and the unspecified
    // one (`maxResults` defaults to 0, i.e. "unspecified").
    let sizes: Vec<Option<usize>> = (1..=held.len() + 2)
        .map(Some)
        .chain(std::iter::once(None))
        .collect();
    for format in FORMATS {
        for size in &sizes {
            let case = format!("{format}, page size {size:?}");
            let collected = walk(&router, &api_id, format, *size, &case).await;
            let names: Vec<&str> = collected
                .iter()
                .map(|listed| listed["name"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                names, expected_names,
                "case {case}: expected the API's types in order, got {names:?}"
            );
            for (row, reported) in held.iter().zip(collected.iter()) {
                assert_projects(row, &api_id, format, reported, &case);
                // The listing and the single read answer the SAME body for one type.
                let read = get_type(&router, &api_id, row.name, format).await;
                assert_eq!(
                    &read.body["type"], reported,
                    "case {case}: expected the listing to answer what GetType answers for \
                     {:?}, got {reported:?}",
                    row.name
                );
            }
        }
    }
}

/// An update replaces a type's definition in the listing without moving its place: the
/// order a caller pages over is the API's own, not an order the last write chose.
#[tokio::test]
async fn an_update_replaces_a_listed_definition_without_moving_it() {
    let router = router();
    let api_id = create_api(&router, "type-listing-after-update").await;
    let held = rows();

    for row in &held {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?}: expected 200, got {created:?}",
            row.name
        );
    }

    // Every held name in turn, so the updated type's position varies over the listing.
    for row in &held {
        let case = format!("after updating {:?}", row.name);
        let replacement = format!("\"\"\"Replaced.\"\"\"\nscalar {}\n", row.name);
        let updated = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}", row.name),
            Some(json!({"definition": replacement, "format": state::SDL_FORMAT})),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateType 200, got {updated:?}"
        );

        let collected = walk(&router, &api_id, state::SDL_FORMAT, Some(2), &case).await;
        let names: Vec<&str> = collected
            .iter()
            .map(|listed| listed["name"].as_str().unwrap_or_default())
            .collect();
        let expected_names: Vec<&str> = held.iter().map(|held| held.name).collect();
        assert_eq!(
            names, expected_names,
            "case {case}: expected the listing order to be unchanged, got {names:?}"
        );
        let reported = collected
            .iter()
            .find(|listed| listed["name"].as_str() == Some(row.name))
            .unwrap_or_else(|| panic!("case {case}: expected {:?} in the listing", row.name));
        assert_eq!(
            reported["definition"],
            json!(rendered_sdl(&replacement)),
            "case {case}: expected the listing to answer the new definition, got {reported:?}"
        );
    }
}

/// An API that holds no type lists none, and a request that resumes nothing or addresses
/// no API is refused.
#[tokio::test]
async fn a_listing_of_nothing_is_empty_and_an_unresumable_one_is_refused() {
    let router = router();
    let api_id = create_api(&router, "type-listing-refusals").await;

    for format in FORMATS {
        let answer = list_types(&router, &api_id, format, None, None).await;
        assert_eq!(
            answer.status, 200,
            "case an API holding no type ({format}): expected 200, got {answer:?}"
        );
        assert!(
            listed(&answer).is_empty(),
            "case an API holding no type ({format}): expected no types, got {answer:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case an API holding no type ({format}): expected no cursor, got {answer:?}"
        );
    }

    let created = create_type(
        &router,
        &api_id,
        rows()[0].definition,
        state::SDL_FORMAT,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create one type: expected 200, got {created:?}"
    );

    // A cursor is the name the next page starts at, so one that names no type of this API
    // resumes nothing.
    for token in ["NeverCreated", "zzzz", rows()[1].name] {
        let refused = list_types(&router, &api_id, state::SDL_FORMAT, None, Some(token)).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "BadRequestException"),
            "case a nextToken of {token:?}: expected a 400 BadRequestException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            INVALID_PAGINATION_TOKEN,
            "case a nextToken of {token:?}: expected the unresumable-cursor text, got {refused:?}"
        );
    }

    for (case, api, expected_status, expected_error) in [
        (
            "an apiId no API has",
            "z".repeat(state::API_ID_LEN),
            404,
            "NotFoundException",
        ),
        (
            "an apiId too long to be one",
            "z".repeat(state::API_ID_LEN + 1),
            400,
            "BadRequestException",
        ),
    ] {
        let refused = list_types(&router, &api, state::SDL_FORMAT, None, None).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (expected_status, expected_error),
            "case listing under {case}: expected {expected_status} {expected_error}, got {refused:?}"
        );
    }
}

/// Associate `source` with `merged` and return the association's id.
async fn associate(router: &MultiServiceRouter, source_id: &str, merged_id: &str) -> String {
    let answer = call(
        router,
        "POST",
        &associate_path(source_id),
        Some(json!({"mergedApiIdentifier": merged_id})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case associate {source_id} with {merged_id}: expected 200, got {answer:?}"
    );
    answer.body["sourceApiAssociation"]["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A listing by association answers the SOURCE API's types, under the same paging rule.
#[tokio::test]
async fn a_listing_by_association_answers_the_source_apis_types() {
    let router = router();
    let (source, merged) = create_pair(&router, "types-by-association").await;
    let association_id = associate(&router, &source.id, &merged.id).await;

    let held = rows();
    for row in &held {
        let created = create_type(&router, &source.id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?} on the source API: expected 200, got {created:?}",
            row.name
        );
    }
    // A type only the MERGED API holds, so a listing that answered the Merged API's own
    // types rather than the source's would report it.
    let only_merged = "scalar OnlyOnTheMergedApi\n";
    let created = create_type(&router, &merged.id, only_merged, state::SDL_FORMAT).await;
    assert_eq!(
        created.status, 200,
        "case create the Merged API's own type: expected 200, got {created:?}"
    );

    let expected_names: Vec<&str> = held.iter().map(|row| row.name).collect();
    for format in FORMATS {
        for size in [Some(1), Some(3), None] {
            let case = format!("{format}, page size {size:?}");
            // Walk the cursor to the end, so the page size cannot hide a type.
            let mut collected: Vec<Value> = Vec::new();
            let mut next_token: Option<String> = None;
            for page in 0..=held.len() + 1 {
                let answer = list_types_by_association(
                    &router,
                    &merged.id,
                    &association_id,
                    format,
                    size,
                    next_token.as_deref(),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case} page {page}: expected ListTypesByAssociation 200, got {answer:?}"
                );
                collected.extend(listed(&answer));
                match answer.body["nextToken"].as_str() {
                    Some(token) => next_token = Some(token.to_string()),
                    None => break,
                }
            }
            let names: Vec<&str> = collected
                .iter()
                .map(|listed| listed["name"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                names, expected_names,
                "case {case}: expected the source API's types in order, got {names:?}"
            );
            // Each type is the source API's own, named under the source API's path.
            for (row, reported) in held.iter().zip(collected.iter()) {
                assert_projects(row, &source.id, format, reported, &case);
            }
        }
    }
}

/// A pair that names no association of the addressed Merged API lists nothing and says
/// which refusal it is.
#[tokio::test]
async fn a_listing_by_an_association_nothing_holds_is_refused() {
    let router = router();
    let (source, merged) = create_pair(&router, "types-by-association-refusals").await;
    let other = create_api(&router, "types-by-association-other-merged").await;
    let association_id = associate(&router, &source.id, &merged.id).await;

    // An `associationId` of the shape the service mints that it never minted, and one it
    // did mint — but under another Merged API, which does not hold it.
    let unheld: String = "z".repeat(association_id.chars().count());
    for (case, merged_identifier, id, expected_status, expected_error, expected_message) in [
        (
            "an associationId the Merged API does not hold",
            merged.id.clone(),
            unheld.clone(),
            404,
            "NotFoundException",
            ASSOCIATION_NOT_FOUND,
        ),
        (
            "an API that does not hold this association",
            other.clone(),
            association_id.clone(),
            404,
            "NotFoundException",
            ASSOCIATION_NOT_FOUND,
        ),
        (
            "a Merged API nothing holds",
            "z".repeat(state::API_ID_LEN),
            association_id.clone(),
            404,
            "NotFoundException",
            API_NOT_FOUND,
        ),
        (
            "an identifier too long to name an API",
            "z".repeat(state::API_ID_LEN + 1),
            association_id.clone(),
            400,
            "BadRequestException",
            "",
        ),
        (
            "an associationId of no form the service mints",
            merged.id.clone(),
            "not-an-association-id!".to_string(),
            400,
            "BadRequestException",
            "",
        ),
    ] {
        let refused = list_types_by_association(
            &router,
            &merged_identifier,
            &id,
            state::SDL_FORMAT,
            None,
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (expected_status, expected_error),
            "case {case}: expected {expected_status} {expected_error}, got {refused:?}"
        );
        if !expected_message.is_empty() {
            assert_eq!(
                message(&refused),
                expected_message,
                "case {case}: expected the refusal text {expected_message:?}, got {refused:?}"
            );
        }
        assert!(
            refused.body["types"].is_null(),
            "case {case}: expected no types on a refusal, got {refused:?}"
        );
    }
}
