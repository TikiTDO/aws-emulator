// SPDX-License-Identifier: CC0-1.0

//! Each character class this service enforces is the class it publishes.
//!
//! The rule under test: a character class is stated ONCE — by the constant a refusal
//! quotes — and that same expression is what decides acceptance, so the set of
//! characters the service admits in a value is exactly the set its published class
//! names, matched over the WHOLE value.
//!
//! The two classes, and the operation each is reached through here:
//!
//! * `RESOURCE_ARN_CHARACTER_CLASS` — the modelled `ResourceArn` `@pattern`
//!   (`smithy_query(query="shape",shape="ResourceArn",
//!   model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")`
//!   states `^[a-zA-Z0-9:*/-]+$`), reached by `ListTagsForResource` through the
//!   judgement of the `resourceArn` it carries;
//! * `TAG_CHARACTER_CLASS` — the modelled `TagKey` / `TagValue` `@pattern`
//!   (`smithy_query(query="shape",shape="TagKey,TagValue")` states
//!   `^[a-zA-Z0-9/_+=.:@-]+$`), reached by `PutDeploymentParameter` through the key
//!   side of the constrained `TagsMap` its `tags` member targets.
//!
//! Both tables are driven over ONE catalog of characters and each row's expectation
//! is COMPUTED by applying the published class with the `regex` engine, rather than
//! written out beside it: an expression that admitted a different set from the one it
//! advertises would fail here whichever side moved. The catalog spans the characters
//! each class names, the characters one class names and the other excludes, and
//! characters outside both.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    RESOURCE_ARN_CHARACTER_CLASS, RESOURCE_ARN_MAX_LEN, TAG_CHARACTER_CLASS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, tags_of, CATALOG,
};

/// The agreement and product the records below are written under.
const AGREEMENT: &str = "agmt-classes";
const PRODUCT: &str = "prod-classes";

/// The characters both tables are driven over.
///
/// Source: the two classes this crate publishes, each the modelled `@pattern` of its
/// member (the module docs name the `smithy_query` call for each). The rows are every
/// character `RESOURCE_ARN_CHARACTER_CLASS` names, every character
/// `TAG_CHARACTER_CLASS` names — the two differ, so a row named by one is excluded by
/// the other (`*` for ARNs; `_`, `+`, `=`, `.`, `@` for tag keys) — and punctuation,
/// whitespace and letters outside ASCII that neither names.
const CANDIDATE_CHARACTERS: &[(&str, char)] = &[
    ("a lowercase letter", 'q'),
    ("an uppercase letter", 'Q'),
    ("a digit", '7'),
    ("a colon", ':'),
    ("an asterisk", '*'),
    ("a slash", '/'),
    ("a hyphen", '-'),
    ("an underscore", '_'),
    ("a plus sign", '+'),
    ("an equals sign", '='),
    ("a full stop", '.'),
    ("an at sign", '@'),
    ("a space", ' '),
    ("a number sign", '#'),
    ("a comma", ','),
    ("a percent sign", '%'),
    ("an opening parenthesis", '('),
    ("a backslash", '\\'),
    ("a tilde", '~'),
    ("a quotation mark", '"'),
    ("a letter outside ASCII", 'é'),
    ("a character outside the Latin scripts", '所'),
];

/// Whether `candidate` is, in whole, inside the published class `character_class`.
///
/// The published class is the `@pattern` expression without the anchors that make a
/// Smithy pattern a total match, so the total match is restored here — the same
/// reading the refusal's own sentence states when it says a value must be N
/// characters OF the class.
fn admits_whole(character_class: &str, candidate: &str) -> bool {
    Regex::new(&format!(r"\A(?:{character_class})\z"))
        .expect("a published character class compiles")
        .is_match(candidate)
}

/// One emulator holding one deployment parameter, and the ARN the service minted for
/// it — the ARN every resource-ARN row below is derived from.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "ClassBaseline", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

#[tokio::test]
async fn the_resource_arn_class_that_is_quoted_is_the_class_that_decides() {
    let (router, live) = emulator_with_one_parameter().await;

    for (case, character) in CANDIDATE_CHARACTERS {
        // A resource ARN of this vendor whose resource field carries the candidate
        // character: inside the class it names no stored record (the store's
        // not-found), outside it the API cannot address it at all (a bad request).
        let candidate = format!("{live}{character}");
        let inside = admits_whole(RESOURCE_ARN_CHARACTER_CLASS, &candidate);
        let (status, answered) = list_tags(&router, &candidate).await;

        if inside {
            assert_eq!(
                status, 404,
                "case {case:?}: a character the published class names must reach the store, \
                 got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {case:?}: expected the store's not-found, got {answered:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a character outside the published class must be refused, \
                 got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "BadRequestException",
                "case {case:?}: expected the front-end's bad request, got {answered:?}"
            );
            let reported = message(&answered);
            assert!(
                reported.contains(RESOURCE_ARN_CHARACTER_CLASS)
                    && reported.contains(&RESOURCE_ARN_MAX_LEN.to_string()),
                "case {case:?}: the refusal states the class that decided it, got {reported:?}"
            );
        }
    }

    // The record itself still reads back: nothing above wrote or removed anything.
    let (status, listed) = list_tags(&router, &live).await;
    assert_eq!(
        status, 200,
        "the stored record must still read back, got {status} {listed:?}"
    );
    assert!(
        tags_of(&listed).is_empty(),
        "the baseline record carries no tags, got {listed:?}"
    );
}

#[tokio::test]
async fn the_tag_key_class_that_is_quoted_is_the_class_that_decides() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);

    for (index, (case, character)) in CANDIDATE_CHARACTERS.iter().enumerate() {
        // A tag key carrying the candidate character, through the member whose target
        // map states the class. Each row writes its own record, so an accepted key is
        // read back on its own and a refused one leaves nothing behind.
        let key = format!("k{character}k");
        let inside = admits_whole(TAG_CHARACTER_CLASS, &key);
        let name = format!("ClassRow{index}");
        let (status, answered) = put(
            &router,
            CATALOG,
            PRODUCT,
            &json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": name, "secretString": "s" },
                "tags": { key.clone(): "v" },
            }),
        )
        .await;

        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: a key of characters the published class names must be stored, \
                 got {status} {answered:?}"
            );
            let expected: BTreeMap<String, String> =
                [(key.clone(), "v".to_string())].into_iter().collect();
            assert_eq!(
                tags_of(&answered),
                expected,
                "case {case:?}: the write must report the stored tag, got {answered:?}"
            );
            // Round trip: the same set comes back through the reader.
            let arn = answered
                .get("resourceArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the record must read back, got {status} {listed:?}"
            );
            assert_eq!(
                tags_of(&listed),
                expected,
                "case {case:?}: the reader must report the stored tag, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a key outside the published class must be refused, \
                 got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected the modelled input error, got {answered:?}"
            );
            assert_eq!(
                field_name(&answered),
                "tags",
                "case {case:?}: the refusal names the member that carried the key, \
                 got {answered:?}"
            );
            let reported = message(&answered);
            assert!(
                reported.contains(TAG_CHARACTER_CLASS),
                "case {case:?}: the refusal states the class that decided it, got {reported:?}"
            );
        }
    }
}
