// SPDX-License-Identifier: CC0-1.0

//! `UpdateIntegrationResponse` over the restJson1 wire: the `IntegrationResponseKey`
//! grammar it holds a patch to, and the patch semantics of the members around it.
//!
//! The rule under test: an `IntegrationResponseKey` is one of exactly two things — the
//! catch-all `$default`, or a regular expression written between slashes — and, on an
//! integration whose responses are selected by the STATUS the call produced (every one that
//! is not a Lambda integration), the wrapped pattern has to be able to match a status code
//! at all. That grammar belongs to the KEY, not to the operation that writes it, so every
//! writer of an integration response is held to it: a value failing either stage is refused
//! `BadRequestException` (HTTP 400) and the stored record keeps the key it already had,
//! whether the request that carried the value was the create or this patch.
//!
//! The rest of the patch is a PATCH: a member the request carries replaces the stored one,
//! a member it omits leaves the stored value exactly as it was, and the answer is read back
//! out of the store, so `GetIntegrationResponse` reports the same record afterwards.
//!
//! The tables span what the rules generalise over: the FORM a key arrives in (the catch-all,
//! a class pattern, a quantified pattern, a literal code, a wildcard, bare text, a
//! half-wrapped value, an empty wrapping), which of the two stages refuses it, the KIND of
//! integration it is judged on (a `MOCK` integration, which answers with a status, and a
//! Lambda integration, whose responses are selected on the function's error message), and
//! which members a patch carries (one, several, a map sent empty, none of them).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::duplicate_identity::integration_response_key_in_use_message;

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};
use crate::integration_response_key_grammar::{
    integration, lambda_integration, mock_integration, websocket_api,
};

/// The key every fixture below starts life under: the catch-all, which is a key on either
/// kind of integration.
const STARTING_KEY: &str = "$default";

/// What a row expects of a patched key: stored, or refused by one of the two stages the
/// grammar has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The key passes both stages, so it is stored and read back.
    Stored,
    /// The key is neither the catch-all nor a wrapped pattern: refused by the first stage,
    /// whose sentence names the member and the value.
    Malformed,
    /// A wrapped pattern that can match no status code, on an integration whose responses
    /// are selected by one: refused by the second stage, whose sentence names the
    /// integration kind.
    NotStatusShaped,
}

/// One patch row: the case, the body it sends, and the members the record must carry
/// afterwards.
type PatchRow = (&'static str, Value, Vec<(&'static str, Value)>);

/// One integration response under `integration_id`, created with `key`, and the URI the
/// collection it belongs to is addressed by.
struct Fixture {
    emulator: MultiServiceRouter,
    api_id: String,
    integration_id: String,
    response_id: String,
}

impl Fixture {
    /// A WebSocket api carrying one integration of the body `request` describes, with one
    /// integration response on it keyed `key`.
    async fn new(name: &str, request: Value, key: &str) -> Self {
        let emulator = emulator();
        let api_id = websocket_api(&emulator, name).await;
        let integration_id = integration(&emulator, &api_id, request).await;
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
            Some(json!({ "integrationResponseKey": key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {key:?}: expected 201 from CreateIntegrationResponse, got {status} {created}"
        );
        let response_id = as_str(&created, "integrationResponseId");
        Self {
            emulator,
            api_id,
            integration_id,
            response_id,
        }
    }

    /// The URI of this integration's integration-response collection.
    fn collection(&self) -> String {
        format!(
            "/v2/apis/{}/integrations/{}/integrationresponses",
            self.api_id, self.integration_id
        )
    }

    /// The URI of the integration response the cases patch.
    fn member(&self) -> String {
        format!("{}/{}", self.collection(), self.response_id)
    }

    /// `PATCH` the integration response with `body`.
    async fn patch(&self, body: Value) -> (u16, Value) {
        call(&self.emulator, "PATCH", &self.member(), Some(body)).await
    }

    /// The integration response as `GetIntegrationResponse` reports it.
    async fn read(&self, case: &str) -> Value {
        let (status, fetched) = call(&self.emulator, "GET", &self.member(), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrationResponse, got {status} {fetched}"
        );
        fetched
    }
}

/// RULE: a patched `IntegrationResponseKey` goes through the same two-stage grammar the
/// create holds a key to, and a refused patch leaves the stored key exactly as it was.
#[tokio::test]
async fn a_patched_key_is_held_to_the_grammar_the_create_holds_a_key_to() {
    // (the key the patch carries, what the grammar says of it on a status-selected
    // integration). The rows span the accepted forms — the catch-all, a class pattern with
    // digit escapes, a quantified escape, a literal code, a character class, wildcards, an
    // alternation — the ways a value is neither form, and wrapped patterns that match no
    // status code.
    let rows: &[(&str, Verdict)] = &[
        (r"/2\d\d/", Verdict::Stored),
        (r"/4\d{2}/", Verdict::Stored),
        ("/404/", Verdict::Stored),
        (r"/[45]\d\d/", Verdict::Stored),
        ("/5../", Verdict::Stored),
        (r"/(200|404)/", Verdict::Stored),
        ("$default", Verdict::Stored),
        ("badkey", Verdict::Malformed),
        ("200", Verdict::Malformed),
        ("/200", Verdict::Malformed),
        ("200/", Verdict::Malformed),
        ("//", Verdict::Malformed),
        ("default", Verdict::Malformed),
        ("$DEFAULT", Verdict::Malformed),
        ("/notastatus/", Verdict::NotStatusShaped),
        ("/error.*/", Verdict::NotStatusShaped),
        // Three digits, but no response carries a class outside 1xx..5xx, and no status
        // code has six digits or two.
        (r"/0\d\d/", Verdict::NotStatusShaped),
        (r"/7\d\d/", Verdict::NotStatusShaped),
        (r"/\d{6}/", Verdict::NotStatusShaped),
        ("/42/", Verdict::NotStatusShaped),
        // A pattern no engine builds matches nothing, so it matches no status code.
        ("/(/", Verdict::NotStatusShaped),
    ];

    let fixture = Fixture::new("patched-key-grammar", mock_integration(), STARTING_KEY).await;
    // The key the store holds right now: a refused patch must leave it, and an accepted one
    // must replace it.
    let mut held = STARTING_KEY.to_string();
    for (key, verdict) in rows {
        let (status, body) = fixture
            .patch(json!({ "integrationResponseKey": key }))
            .await;
        match verdict {
            Verdict::Stored => {
                assert_eq!(
                    status, 200,
                    "case {key:?}: expected 200 from UpdateIntegrationResponse, got {status} \
                     {body}"
                );
                assert_eq!(
                    as_str(&body, "integrationResponseKey"),
                    *key,
                    "case {key:?}: expected the patched key to be reported back, got {body}"
                );
                held = (*key).to_string();
            }
            Verdict::Malformed | Verdict::NotStatusShaped => {
                assert_eq!(
                    status, 400,
                    "case {key:?}: expected 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "BadRequestException",
                    "case {key:?}: expected BadRequestException, got {body}"
                );
                let message = as_str(&body, "message");
                assert!(
                    message.contains("IntegrationResponseKey") && message.contains(key),
                    "case {key:?}: expected the refusal to name the member and the value, \
                     got {message:?}"
                );
                let names_the_kind = message.contains("non-Lambda Integration");
                assert_eq!(
                    names_the_kind,
                    *verdict == Verdict::NotStatusShaped,
                    "case {key:?}: expected the refusal of the {verdict:?} stage, got \
                     {message:?}"
                );
            }
        }
        // Whatever the verdict, the store holds exactly one key for this response: the
        // patched one when it was accepted, the one it already had when it was refused.
        let fetched = fixture.read(key).await;
        assert_eq!(
            as_str(&fetched, "integrationResponseKey"),
            held,
            "case {key:?}: expected the reader to report {held:?}, got {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "integrationResponseId"),
            fixture.response_id,
            "case {key:?}: expected the patch to keep the response's identity, got {fetched}"
        );
    }
}

/// RULE: the second stage is about the integration's own selection subject, so a Lambda
/// integration — whose responses are selected on the function's error message — takes any
/// wrapped pattern on a patch, while the first stage still refuses a value that is neither
/// form.
#[tokio::test]
async fn a_patched_key_matching_no_status_is_a_key_on_a_lambda_integration() {
    let fixture = Fixture::new("patched-key-lambda", lambda_integration(), STARTING_KEY).await;
    // Patterns that match no status code at all, which this integration never selects by.
    for key in ["/error.*/", "/notastatus/", r"/7\d\d/"] {
        let (status, body) = fixture
            .patch(json!({ "integrationResponseKey": key }))
            .await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected 200 on a Lambda integration, got {status} {body}"
        );
        let fetched = fixture.read(key).await;
        assert_eq!(
            as_str(&fetched, "integrationResponseKey"),
            key,
            "case {key:?}: expected the reader to report the patched key, got {fetched}"
        );
    }
    // The first stage does not depend on the integration's kind.
    for key in ["badkey", "//", "$DEFAULT"] {
        let (status, body) = fixture
            .patch(json!({ "integrationResponseKey": key }))
            .await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected 400 on a Lambda integration too, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {key:?}: expected BadRequestException, got {body}"
        );
    }
}

/// RULE: a patch replaces the members it carries and leaves the ones it omits, and the
/// answer is the record the store now holds.
#[tokio::test]
async fn a_patch_replaces_what_it_carries_and_leaves_what_it_omits() {
    let fixture = Fixture::new("patched-members", mock_integration(), STARTING_KEY).await;
    // Fill every optional member first, so a later patch has something to leave alone.
    let (status, body) = fixture
        .patch(json!({
            "integrationResponseKey": r"/2\d\d/",
            "contentHandlingStrategy": "CONVERT_TO_TEXT",
            "responseParameters": { "method.response.header.X-Origin": "'mock'" },
            "responseTemplates": { "application/json": "{\"ok\":true}" },
            "templateSelectionExpression": "$request.body.action"
        }))
        .await;
    assert_eq!(
        status, 200,
        "case \"every member\": expected 200, got {status} {body}"
    );

    // (case, the patch, the members the record must carry afterwards)
    let rows: Vec<PatchRow> = vec![
        (
            "one scalar member",
            json!({ "contentHandlingStrategy": "CONVERT_TO_BINARY" }),
            vec![
                ("contentHandlingStrategy", json!("CONVERT_TO_BINARY")),
                ("integrationResponseKey", json!(r"/2\d\d/")),
                (
                    "responseTemplates",
                    json!({ "application/json": "{\"ok\":true}" }),
                ),
            ],
        ),
        (
            "the key alone",
            json!({ "integrationResponseKey": "/404/" }),
            vec![
                ("integrationResponseKey", json!("/404/")),
                ("contentHandlingStrategy", json!("CONVERT_TO_BINARY")),
                (
                    "responseParameters",
                    json!({ "method.response.header.X-Origin": "'mock'" }),
                ),
                ("templateSelectionExpression", json!("$request.body.action")),
            ],
        ),
        (
            "two map members at once",
            json!({
                "responseParameters": { "method.response.header.X-Served": "'patched'" },
                "responseTemplates": { "text/plain": "ok" }
            }),
            vec![
                (
                    "responseParameters",
                    json!({ "method.response.header.X-Served": "'patched'" }),
                ),
                ("responseTemplates", json!({ "text/plain": "ok" })),
                ("integrationResponseKey", json!("/404/")),
            ],
        ),
        (
            "a map sent empty, which clears it",
            json!({ "responseTemplates": {} }),
            vec![
                ("responseTemplates", json!({})),
                (
                    "responseParameters",
                    json!({ "method.response.header.X-Served": "'patched'" }),
                ),
            ],
        ),
        (
            "no member at all, which changes nothing",
            json!({}),
            vec![
                ("integrationResponseKey", json!("/404/")),
                ("contentHandlingStrategy", json!("CONVERT_TO_BINARY")),
                ("responseTemplates", json!({})),
                ("templateSelectionExpression", json!("$request.body.action")),
            ],
        ),
    ];

    for (case, patch, expected) in &rows {
        let (status, answered) = fixture.patch(patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answered}"
        );
        // The answer is the stored record, so the reader reports the same members.
        let fetched = fixture.read(case).await;
        for (member, value) in expected {
            assert_eq!(
                answered.get(*member),
                Some(value),
                "case {case:?}: expected the answer's {member} to be {value}, got {answered}"
            );
            assert_eq!(
                fetched.get(*member),
                Some(value),
                "case {case:?}: expected the reader's {member} to be {value}, got {fetched}"
            );
        }
        assert_eq!(
            as_str(&fetched, "integrationResponseId"),
            fixture.response_id,
            "case {case:?}: expected the response's identity to survive, got {fetched}"
        );
    }
}

/// RULE: one key per integration, so a patch cannot move a response onto a key a sibling
/// already holds — while re-sending its own key is no conflict with itself.
#[tokio::test]
async fn a_patch_onto_a_siblings_key_is_refused() {
    let fixture = Fixture::new("patched-key-collision", mock_integration(), STARTING_KEY).await;
    let (status, sibling) = call(
        &fixture.emulator,
        "POST",
        &fixture.collection(),
        Some(json!({ "integrationResponseKey": "/404/" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for the sibling response, got {status} {sibling}"
    );

    let (status, body) = fixture
        .patch(json!({ "integrationResponseKey": "/404/" }))
        .await;
    assert_eq!(
        status, 400,
        "case \"a sibling's key\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case \"a sibling's key\": expected BadRequestException, got {body}"
    );
    assert_eq!(
        as_str(&body, "message"),
        integration_response_key_in_use_message("/404/", &fixture.integration_id),
        "case \"a sibling's key\": expected the create's own sentence, got {body}"
    );
    let fetched = fixture.read("a sibling's key").await;
    assert_eq!(
        as_str(&fetched, "integrationResponseKey"),
        STARTING_KEY,
        "case \"a sibling's key\": expected the refused patch to have changed nothing, got \
         {fetched}"
    );

    // Its own key is not a collision with itself.
    let (status, body) = fixture
        .patch(json!({ "integrationResponseKey": STARTING_KEY }))
        .await;
    assert_eq!(
        status, 200,
        "case \"its own key\": expected 200, got {status} {body}"
    );
}

/// RULE: a patch resolves the resources its path names before it reads its body, and a
/// member it cannot store is refused with nothing written.
#[tokio::test]
async fn a_patch_is_refused_about_what_its_path_or_its_body_cannot_name() {
    let fixture = Fixture::new("patched-addressing", mock_integration(), STARTING_KEY).await;
    let patch = json!({ "integrationResponseKey": "/500/" });

    // (case, the URI, the status it is refused with). The rows walk the path from the api
    // down to the response itself, each naming something no record holds.
    let rows: &[(&str, String, u16)] = &[
        (
            "an api no record holds",
            format!(
                "/v2/apis/nosuchapi00/integrations/{}/integrationresponses/{}",
                fixture.integration_id, fixture.response_id
            ),
            404,
        ),
        (
            "an integration no record holds",
            format!(
                "/v2/apis/{}/integrations/nosuchintg0/integrationresponses/{}",
                fixture.api_id, fixture.response_id
            ),
            404,
        ),
        (
            "an integration response no record holds",
            format!("{}/nosuchresp0", fixture.collection()),
            404,
        ),
    ];
    for (case, uri, expected) in rows {
        let (status, body) = call(&fixture.emulator, "PATCH", uri, Some(patch.clone())).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // A template selection expression may name only the variable namespaces the service
    // evaluates, so one naming anything else is refused and stores nothing.
    let (status, body) = fixture
        .patch(json!({ "templateSelectionExpression": "$nosuchnamespace.value" }))
        .await;
    assert_eq!(
        status, 400,
        "case \"an unknown selection variable\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case \"an unknown selection variable\": expected BadRequestException, got {body}"
    );
    let fetched = fixture.read("an unknown selection variable").await;
    assert!(
        carries_nothing(&fetched, "templateSelectionExpression"),
        "case \"an unknown selection variable\": expected the refused patch to have stored \
         nothing, got {fetched}"
    );
    assert_eq!(
        as_str(&fetched, "integrationResponseKey"),
        STARTING_KEY,
        "case \"an unknown selection variable\": expected the stored key to be untouched, \
         got {fetched}"
    );
}
