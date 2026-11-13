// SPDX-License-Identifier: CC0-1.0

//! `DescribeSecret$VersionIdsToStages` keys the caller's own version under the
//! `ClientRequestToken` the caller sent, WHATEVER that token's text is, and keeps
//! `AWSCURRENT` on it when later token-less writes are refused.
//!
//! THE RULE: a write that stores a secret value creates a version whose id is the
//! `ClientRequestToken` the request carries, verbatim — "this parameter specifies
//! the unique identifier for the new version [...] If you generate a raw HTTP
//! request to the Secrets Manager service endpoint, then you must generate a
//! ClientRequestToken and include it in the request."
//! (`CreateSecretRequest$ClientRequestToken`, and the same sentence on
//! `PutSecretValueRequest$ClientRequestToken` and
//! `UpdateSecretRequest$ClientRequestToken`, this service's Smithy model) — so a
//! version-creating write that names none is refused with `InvalidRequestException`
//! (HTTP 400, this service's Smithy model) and creates nothing, leaving the map
//! `DescribeSecret` reports holding exactly the versions the caller named, the last
//! of them still `AWSCURRENT`.
//!
//! The suites beside this one pin the same rule over the write (
//! [`crate::secret_client_request_token`], a table over operation, value member and
//! token LENGTH), over the reading surface
//! ([`crate::secret_refused_write_staging_report`]) and over generated write
//! sequences ([`crate::secret_version_ids_are_caller_named`]). What this suite adds
//! is the dimension none of them varies: the token's TEXT. `ClientRequestTokenType`
//! and `SecretVersionIdType` constrain only `@length` 32..64 and carry no `@pattern`
//! (this service's Smithy model), so every token inside that bound is a version id
//! the service has to echo back and key its staging map by — a client that generates
//! hyphenated UUIDs, hexadecimal digests or its own prefixed tokens must each find
//! its own version current, and must never find a version it did not name.
//!
//! Each row is one token form, run through the sequence a raw client makes: create
//! the secret naming the token, read the staging map, make the token-less writes AWS
//! refuses, and read the map again.

use serde_json::{json, Map, Value};

use crate::secret_client_request_token::{
    call, emulator, ok, refusal, staging_map, text, value_member, MISSING_TOKEN_MESSAGE,
    TOKEN_MAX_LEN, TOKEN_MIN_LEN,
};

/// The label a created version holds, and the one a read with no version and no
/// stage resolves: "Secrets Manager automatically moves the staging label
/// AWSCURRENT to the new version." (`PutSecretValue`, this service's Smithy model.)
const STAGE_CURRENT: &str = "AWSCURRENT";

/// The token-less writes made against the secret after it is created: both
/// operations that append a version, each carrying its value in both of the members
/// it can carry one in ("You must include SecretBinary or SecretString, but not
/// both.", `PutSecretValueRequest$SecretString`, this service's Smithy model).
const REFUSED_WRITES: &[(&str, &str)] = &[
    ("PutSecretValue", "SecretString"),
    ("UpdateSecret", "SecretString"),
    ("PutSecretValue", "SecretBinary"),
    ("UpdateSecret", "SecretBinary"),
];

/// The token forms the rule is checked over: `ClientRequestTokenType` is `@length`
/// 32..64 with no `@pattern` (this service's Smithy model), so the rows vary the
/// token's characters and its length across that bound — both published bounds
/// included — and nothing else.
fn token_forms() -> Vec<(&'static str, String)> {
    vec![
        (
            "a lowercase hyphenated UUID",
            "3f1a7c9e-5b2d-4e6f-8a01-9c4d2e7b6f30".to_string(),
        ),
        (
            "the same UUID in uppercase",
            "3F1A7C9E-5B2D-4E6F-8A01-9C4D2E7B6F30".to_string(),
        ),
        (
            "32 hexadecimal characters, the shortest token the model allows",
            "3f1a7c9e5b2d4e6f8a019c4d2e7b6f30".to_string(),
        ),
        (
            "64 characters, the longest token the model allows",
            "3f1a7c9e5b2d4e6f8a019c4d2e7b6f30".repeat(2),
        ),
        (
            "a token the client prefixes with a label of its own",
            "my-own-request-token-3f1a7c9e5b2d4e6f".to_string(),
        ),
        (
            "decimal digits only",
            "01234567890123456789012345678901".to_string(),
        ),
        (
            "punctuation the length bound does not exclude",
            "client.request_token+with-punctuation".to_string(),
        ),
        ("a mixed-case alphanumeric token", "A1b2".repeat(12)),
    ]
}

#[tokio::test]
async fn the_version_the_caller_named_stays_current_whatever_the_token_looks_like() {
    for (row, (case, token)) in token_forms().into_iter().enumerate() {
        let length = token.chars().count();
        assert!(
            (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&length),
            "case {case:?}: the row's own token must be inside the modelled bound \
             {TOKEN_MIN_LEN}..={TOKEN_MAX_LEN}, got {length} for {token:?}"
        );

        let router = emulator();
        // "Name must be a string of ASCII letters, numbers and the characters /_+=.@-"
        // (`CreateSecretRequest$Name`, this service's Smithy model), so the secret is
        // named after the row rather than after the sentence or the token.
        let name = format!("token-forms/row-{row}");
        let created = ok(
            &router,
            "CreateSecret",
            &json!({
                "Name": name.clone(),
                "SecretString": "the-caller-s-value",
                "ClientRequestToken": token.clone(),
            }),
            case,
        )
        .await;
        assert_eq!(
            text(&created, "VersionId", case),
            token,
            "case {case:?}: expected the create to stake the token it named verbatim, got {created}"
        );

        // Before any refused write: the map holds exactly that version, current.
        let expected = Map::from_iter([(token.clone(), json!([STAGE_CURRENT]))]);
        let staged = staging_map(&router, &name, case).await;
        assert_eq!(
            staged, expected,
            "case {case:?}: expected the created version keyed by the caller's token, got \
             {staged:?}"
        );

        // Every token-less write is refused and creates nothing, so no version the
        // caller did not name can take AWSCURRENT from the one it did.
        for (index, (operation, member)) in REFUSED_WRITES.iter().enumerate() {
            let step = format!("{case}, step {index}: {operation} carrying {member}");
            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name.clone()));
            body.insert(
                (*member).to_string(),
                value_member(member, &format!("a-value-{index}")),
            );
            let (status, parsed) = call(&router, operation, &Value::Object(body)).await;
            assert_eq!(
                status, 400,
                "case {step:?}: expected 400 for a write naming no ClientRequestToken, got \
                 {status} with body {parsed}"
            );
            let (code, message) = refusal(&parsed);
            assert_eq!(
                code, "InvalidRequestException",
                "case {step:?}: expected InvalidRequestException, got {parsed}"
            );
            assert_eq!(
                message, MISSING_TOKEN_MESSAGE,
                "case {step:?}: expected the missing-token sentence, got {message:?}"
            );
        }

        // The map a client reads back after those refusals: one version, the one it
        // named, still AWSCURRENT.
        let staged = staging_map(&router, &name, case).await;
        assert_eq!(
            staged, expected,
            "case {case:?}: expected the caller's version alone, still {STAGE_CURRENT}, got \
             {staged:?}"
        );

        // And the read that names neither a version nor a stage resolves it, so the
        // value a refused write would have stored is nowhere.
        let read = ok(&router, "GetSecretValue", &json!({"SecretId": name}), case).await;
        assert_eq!(
            text(&read, "VersionId", case),
            token,
            "case {case:?}: expected the caller's own version to be current, got {read}"
        );
        assert_eq!(
            text(&read, "SecretString", case),
            "the-caller-s-value",
            "case {case:?}: expected the caller's own value back, got {read}"
        );
    }
}
