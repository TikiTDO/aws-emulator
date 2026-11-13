// SPDX-License-Identifier: CC0-1.0

//! The `IntegrationResponseKey` grammar `CreateIntegrationResponse` holds a request to,
//! over the restJson1 wire.
//!
//! The rule under test: an `IntegrationResponseKey` is one of exactly two things — the
//! catch-all `$default`, or a regular expression written between slashes — and, on an
//! integration whose responses are selected by the STATUS the call produced (every one that
//! is not a Lambda integration), the wrapped pattern has to be able to match a status code
//! at all. A value failing either stage is refused `BadRequestException` (HTTP 400) naming
//! the member and the value it sent, and nothing is persisted; a value passing both is
//! stored and selects at invoke time.
//!
//! The tables span what the rule generalises over: the FORM a value arrives in (the
//! catch-all, a class pattern, a quantified pattern, a literal code, an alternation, a
//! wildcard, bare text, a half-wrapped value, an empty wrapping), which of the two stages
//! refuses it, and the KIND of integration it is judged on (a `MOCK` integration, which has
//! no backend but answers with a status, and a Lambda integration, whose responses are
//! selected on the function's error message instead). The recorded case that filed the
//! issue — `badkey` on a `MOCK` integration — is one row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// A Lambda function URI, which is what makes an integration one whose responses are
/// selected on the function's error message rather than on a status code.
pub(crate) const FUNCTION_URI: &str = "arn:aws:lambda:eu-west-1:000000000000:function:greeter";

/// A WebSocket api, the protocol whose integrations carry integration responses.
pub(crate) async fn websocket_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// One integration of `api_id`, from the request body the case needs.
pub(crate) async fn integration(
    emulator: &MultiServiceRouter,
    api_id: &str,
    request: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

/// The `MOCK` integration the status-selected cases are judged on: "a MOCK integration lets
/// API Gateway return a response without sending the request further to the backend", so it
/// names no endpoint and still answers with a status.
pub(crate) fn mock_integration() -> Value {
    json!({ "integrationType": "MOCK" })
}

/// The Lambda integration the error-message-selected cases are judged on: an `AWS`
/// integration of a WebSocket api whose `IntegrationUri` names a function.
pub(crate) fn lambda_integration() -> Value {
    json!({
        "integrationType": "AWS",
        "integrationUri": FUNCTION_URI,
        "integrationMethod": "POST"
    })
}

/// RULE: a key that is neither `$default` nor a slash-wrapped pattern is refused 400,
/// whatever the integration is, and nothing is stored.
#[tokio::test]
async fn a_key_that_is_neither_the_catch_all_nor_a_wrapped_pattern_is_refused() {
    // The rows span the ways a value fails to be either form: bare text, a status code
    // written without the wrapping, a half-wrapped value either side, an empty wrapping,
    // the catch-all misspelled and the catch-all escaped as a literal.
    let malformed = [
        "badkey",
        "200",
        "/200",
        "200/",
        "//",
        "default",
        "$DEFAULT",
        "\\$default",
    ];
    // Judged on BOTH kinds of integration: this stage does not depend on which.
    for (kind, request) in [("MOCK", mock_integration()), ("Lambda", lambda_integration())] {
        let emulator = emulator();
        let api_id = websocket_api(&emulator, &format!("key-grammar-{kind}")).await;
        let integration_id = integration(&emulator, &api_id, request).await;
        let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses");
        for key in malformed {
            let (status, body) = call(
                &emulator,
                "POST",
                &uri,
                Some(json!({ "integrationResponseKey": key })),
            )
            .await;
            assert_eq!(
                status, 400,
                "case ({kind:?}, {key:?}): expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case ({kind:?}, {key:?}): expected BadRequestException, got {body}"
            );
            let message = as_str(&body, "message");
            assert!(
                message.contains("IntegrationResponseKey") && message.contains(key),
                "case ({kind:?}, {key:?}): expected the refusal to name the member and the \
                 value, got {message:?}"
            );
        }

        // Nothing was stored: the integration still holds no integration response.
        let (status, listed) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected 200 from GetIntegrationResponses, got {status} {listed}"
        );
        assert_eq!(
            listed.get("items"),
            Some(&json!([])),
            "case {kind:?}: expected the refusals to have stored nothing, got {listed}"
        );
    }
}

/// RULE: on an integration whose responses are selected by the status of the call, a wrapped
/// pattern has to be able to match a status code; on a Lambda integration, selected on the
/// function's error message, any wrapped pattern is a key.
#[tokio::test]
async fn a_pattern_matching_no_status_code_is_refused_only_off_a_lambda_integration() {
    // (the key, whether it can match a status code). The rows span the pattern forms a
    // status key takes — a class with digit escapes, a quantified escape, a literal code, a
    // character class, wildcards, an alternation — and patterns that match no code at all.
    let rows: &[(&str, bool)] = &[
        (r"/2\d\d/", true),
        (r"/4\d{2}/", true),
        ("/200/", true),
        (r"/[45]\d\d/", true),
        ("/5../", true),
        (r"/(200|404)/", true),
        ("/notastatus/", false),
        ("/error.*/", false),
        // Three digits, but no response carries a class outside 1xx..5xx.
        (r"/0\d\d/", false),
        (r"/7\d\d/", false),
        (r"/\d{6}/", false),
        ("/42/", false),
        // A pattern no engine builds matches nothing, so it matches no status code.
        ("/(/", false),
    ];

    let emulator = emulator();
    let api_id = websocket_api(&emulator, "status-key-api").await;
    let mock = integration(&emulator, &api_id, mock_integration()).await;
    let lambda = integration(&emulator, &api_id, lambda_integration()).await;

    for (key, status_shaped) in rows {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{mock}/integrationresponses"),
            Some(json!({ "integrationResponseKey": key })),
        )
        .await;
        if *status_shaped {
            assert_eq!(
                status, 201,
                "case {key:?}: expected 201 on a status-selected integration, got {status} \
                 {body}"
            );
            assert_eq!(
                as_str(&body, "integrationResponseKey"),
                *key,
                "case {key:?}: expected the stored key to be reported back, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {key:?}: expected 400 on a status-selected integration, got {status} \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {key:?}: expected BadRequestException, got {body}"
            );
            let message = as_str(&body, "message");
            assert!(
                message.contains("non-Lambda Integration") && message.contains(key),
                "case {key:?}: expected the refusal to name the integration kind and the \
                 value, got {message:?}"
            );
        }
        // The SAME key on a Lambda integration is a key: that integration's responses are
        // selected on the function's error message, which any pattern can match.
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{lambda}/integrationresponses"),
            Some(json!({ "integrationResponseKey": key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {key:?}: expected 201 on a Lambda integration, got {status} {body}"
        );
    }
}

/// RULE: a key that passes both stages is persisted and read back byte for byte, and the
/// catch-all is accepted on either kind of integration.
#[tokio::test]
async fn an_accepted_key_round_trips_through_the_integration_response_reader() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "key-round-trip").await;
    let mock = integration(&emulator, &api_id, mock_integration()).await;
    let lambda = integration(&emulator, &api_id, lambda_integration()).await;

    // Each accepted form, read back through the single-response reader.
    for (integration_id, key) in [
        (&mock, "$default"),
        (&mock, r"/2\d\d/"),
        (&mock, "/404/"),
        (&lambda, "$default"),
        (&lambda, "/error.*/"),
    ] {
        let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses");
        let (status, created) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({ "integrationResponseKey": key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {key:?}: expected 201, got {status} {created}"
        );
        let response_id = as_str(&created, "integrationResponseId");
        let (status, fetched) = call(&emulator, "GET", &format!("{uri}/{response_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected 200 from GetIntegrationResponse, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "integrationResponseKey"),
            key,
            "case {key:?}: expected the reader to report the stored key, got {fetched}"
        );
    }

    // One key per integration: the second response under the same key would never be
    // selected, so the request is refused rather than stored.
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{mock}/integrationresponses"),
        Some(json!({ "integrationResponseKey": "$default" })),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected 400 for a key the integration already holds, got {status} {body}"
    );

    // The collection reader reports the keys each integration holds, and only its own.
    for (integration_id, expected) in [
        (&mock, vec!["$default", r"/2\d\d/", "/404/"]),
        (&lambda, vec!["$default", "/error.*/"]),
    ] {
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {integration_id:?}: expected 200 from GetIntegrationResponses, got \
             {status} {listed}"
        );
        let mut keys: Vec<String> = listed
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| as_str(item, "integrationResponseKey"))
                    .collect()
            })
            .unwrap_or_default();
        keys.sort();
        let mut expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        expected.sort();
        assert_eq!(
            keys, expected,
            "case {integration_id:?}: expected the collection reader to report this \
             integration's own keys, got {listed}"
        );
    }
}
