// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ResetAuthorizersCache`.
//!
//! "Resets all authorizer cache entries on a stage. Supported only for HTTP APIs."
//! (`ResetAuthorizersCache`), `DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers`,
//! modelled `code: 204` but answered `200 OK` on the wire
//! (`crate::state::FLUSHED_CACHE_OK_STATUS`), with `NotFoundException` (404) and
//! `TooManyRequestsException` (429) as its only modelled errors. So the rules these cases
//! assert:
//!
//! * a stage of an api this service holds is answered that `200`, with no body, and
//!   the call is idempotent — the second reset of a stage answers exactly what the first
//!   did;
//! * the two path labels address a stage OF an api, so a label that resolves to nothing is
//!   refused with the modelled `NotFoundException` (404) and not with an unmodelled code: an
//!   `ApiId` the account does not hold, a `StageName` that api has no stage under, and a
//!   stage name that belongs to ANOTHER api of the same account;
//! * a reset is not a change to the stage's configuration: the stage reads back exactly as
//!   it was configured, with the same `LastUpdatedDate`, so a client that flushes the cache
//!   does not see its stage move.
//!
//! The rows span what the rules generalise over: the form of the stage name (`$default`, a
//! plain name, one with a hyphen and an underscore, one at the published 128-character
//! bound), which of the two labels resolves to nothing, and how many times the call is made.
//! Every api here is an HTTP api, the protocol type the operation documents itself for
//! ("Supported only for HTTP APIs"); what AWS answers for a WEBSOCKET api's stage is not
//! recorded anywhere this suite could assert from, and this operation declares no error for
//! it, so nothing is pinned about that case.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The status API Gateway answers a flush of a stage's authorizer cache with, which is not
/// the `code: 204` the model declares for it.
const FLUSHED_STATUS: u16 = 200;

/// The stage that serves an api's root (`crate::state::DEFAULT_STAGE_NAME`).
const DEFAULT_STAGE: &str = "$default";

/// The upper bound of `StringWithLengthBetween1And128`, the shape `CreateStage$StageName`
/// targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// Create one HTTP api — the protocol type this operation documents itself for — and answer
/// its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateApi must answer 201, got {status} with {created:?}"
    );
    as_str(&created, "apiId")
}

/// Create one stage of `api_id` and answer the stage document the service stored.
async fn create_stage(emulator: &MultiServiceRouter, api_id: &str, stage_name: &str) -> Value {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStage must answer 201 for {stage_name:?}, got {status} with {created:?}"
    );
    created
}

/// The URI `ResetAuthorizersCache` is bound to for one stage.
fn cache_uri(api_id: &str, stage_name: &str) -> String {
    format!("/v2/apis/{api_id}/stages/{stage_name}/cache/authorizers")
}

/// One `ResetAuthorizersCache` call.
async fn reset(emulator: &MultiServiceRouter, api_id: &str, stage_name: &str) -> (u16, Value) {
    call(emulator, "DELETE", &cache_uri(api_id, stage_name), None).await
}

#[tokio::test]
async fn resetting_the_cache_of_a_stage_answers_the_flush_status_and_is_idempotent() {
    let emulator = emulator();

    // (case, the stage name)
    let rows: Vec<(String, String)> = vec![
        (
            "the stage that serves an HTTP api's root".to_string(),
            DEFAULT_STAGE.to_string(),
        ),
        ("a named stage".to_string(), "prod".to_string()),
        (
            "a stage name carrying a hyphen and an underscore".to_string(),
            "pre-prod_2".to_string(),
        ),
        (
            format!("a stage name at the published {MAX_STAGE_NAME_LEN}-character bound"),
            "s".repeat(MAX_STAGE_NAME_LEN),
        ),
    ];

    for (index, (case, stage_name)) in rows.into_iter().enumerate() {
        let api_id = create_api(&emulator, &format!("reset-{index}")).await;
        let created = create_stage(&emulator, &api_id, &stage_name).await;

        // Twice: the second call observes the state the first left, so the answer pins that
        // a reset is idempotent rather than consuming anything.
        for attempt in 1..=2 {
            let (status, body) = reset(&emulator, &api_id, &stage_name).await;
            assert_eq!(
                status, FLUSHED_STATUS,
                "case {case:?} attempt {attempt}: expected {FLUSHED_STATUS}, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                body,
                Value::Null,
                "case {case:?} attempt {attempt}: expected no body at a {FLUSHED_STATUS}, got \
                 {body:?}"
            );
        }

        // The stage is untouched: a cache flush changes no configuration a later reader
        // sees, including the instant the stage was last updated at.
        let (status, read_back) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} with {read_back:?}"
        );
        assert_eq!(
            read_back, created,
            "case {case:?}: expected the stage to read back as it was configured, got \
             {read_back:?}"
        );
    }
}

#[tokio::test]
async fn resetting_the_cache_of_a_stage_that_does_not_exist_is_the_modelled_not_found() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "holds-prod").await;
    create_stage(&emulator, &api_id, "prod").await;
    // A second api, whose own stage name must not resolve under the first.
    let other = create_api(&emulator, "holds-staging").await;
    create_stage(&emulator, &other, "staging").await;

    // (case, the api id addressed, the stage name addressed)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an api id the account holds nothing under",
            "nosuchapi00".to_string(),
            "prod".to_string(),
        ),
        (
            "an api id of the right length that was never minted",
            "zzzzzzzzzz".to_string(),
            DEFAULT_STAGE.to_string(),
        ),
        (
            "a stage name this api has no stage under",
            api_id.clone(),
            "nosuchstage".to_string(),
        ),
        (
            "the stage that serves the api's root, which this api has not created",
            api_id.clone(),
            DEFAULT_STAGE.to_string(),
        ),
        (
            "a stage name that belongs to another api of the same account",
            api_id.clone(),
            "staging".to_string(),
        ),
    ];

    for (case, addressed_api, stage_name) in rows {
        let (status, body) = reset(&emulator, &addressed_api, &stage_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );
        assert!(
            !as_str(&body, "message").is_empty(),
            "case {case:?}: expected the refusal to say what resolved to nothing, got {body:?}"
        );
    }

    // The stage that DOES exist is still reset, so the refusals above are about the labels
    // those rows named and not about this operation in general.
    let (status, body) = reset(&emulator, &api_id, "prod").await;
    assert_eq!(
        status, FLUSHED_STATUS,
        "the stage the account holds must still be reset, got {status} with {body:?}"
    );
}
