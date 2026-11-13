// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a provider's PRINCIPAL-TAG ATTRIBUTE MAP:
//! `SetPrincipalTagAttributeMap` and `GetPrincipalTagAttributeMap`.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call: a set
//! call REPLACES the whole mapping of one `(IdentityPoolId, IdentityProviderName)` pair and
//! the get reports back exactly that mapping — the custom map it was given, with every key
//! mapped to the attribute it names whatever characters either is spelled with, or the
//! DEFAULT mapping when the call asked for the defaults instead; a pool this service does
//! not store, and a provider nobody has mapped, are the modelled
//! `ResourceNotFoundException`; and a tag key that is no tag key at all is refused with
//! HTTP 400, leaving the mapping the pair already had.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// A provider name of the form a Cognito user pool provider carries, which is what
/// `IdentityProviderName` (len 1..128) names.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcdefghi";

/// The federated providers the pools below DECLARE, which is what makes a mapping for one
/// of them a mapping this service stores: a user pool, a public login provider and an
/// OpenID Connect issuer, so the rows span the provider kinds a pool can be fronted by.
const DECLARED_PROVIDERS: [&str; 3] = [
    USER_POOL_PROVIDER,
    "graph.facebook.com",
    "accounts.google.com",
];

/// A pool declaring [`DECLARED_PROVIDERS`], and answering its minted `IdentityPoolId`.
async fn declared_pool(emulator: &Emulator, name: &str, allow_unauthenticated: bool) -> String {
    emulator
        .pool_with_providers(name, allow_unauthenticated, &DECLARED_PROVIDERS)
        .await
}

/// The `SetPrincipalTagAttributeMap` request for one mapping: each optional member is sent
/// only when the row carries it, so a row with `None` exercises the member's absence.
fn set_request(
    pool_id: &str,
    provider: &str,
    use_defaults: Option<bool>,
    principal_tags: Option<&Value>,
) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolId".to_string(), json!(pool_id));
    request.insert("IdentityProviderName".to_string(), json!(provider));
    if let Some(use_defaults) = use_defaults {
        request.insert("UseDefaults".to_string(), json!(use_defaults));
    }
    if let Some(principal_tags) = principal_tags {
        request.insert("PrincipalTags".to_string(), principal_tags.clone());
    }
    Value::Object(request)
}

/// The DEFAULT mapping a call asking for the defaults configures: the two tags a provider's
/// users are mapped by when the call names no attributes of its own — the pair
/// `SetPrincipalTagAttributeMap`'s own doc calls the "default (username and clientID)
/// attribute mappings" (`smithy_query(query="doc",shape="SetPrincipalTagAttributeMap")`) —
/// each taken from the claim an issued OpenID Connect token carries it in: the subject and
/// the audience.
fn default_mapping() -> Value {
    json!({"client": "aud", "username": "sub"})
}

/// The mapping a pair ends up configured with for one request: the defaults when the call
/// asked for them, otherwise the custom map it carried — and nothing at all when it carried
/// neither.
fn effective_mapping(use_defaults: Option<bool>, principal_tags: Option<&Value>) -> Value {
    if use_defaults.unwrap_or(false) {
        default_mapping()
    } else {
        principal_tags.cloned().unwrap_or_else(|| json!({}))
    }
}

/// A `PrincipalTags` map of `count` entries, each key spelled with letters and digits and
/// each value naming a custom user attribute.
fn tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("tag{index}"), json!(format!("custom:attr{index}"))))
            .collect(),
    )
}

/// A string of `count` copies of `character`, for the bounds of a modelled length.
fn repeated(character: char, count: usize) -> String {
    character.to_string().repeat(count)
}

/// Every mapping shape the model admits, as `(case, UseDefaults, PrincipalTags)`. The rows
/// differ in whether each optional member is sent at all, in the flag's value, in how many
/// tags are mapped — up to the 50 the modelled `PrincipalTags` (`length 0..50`) admits —
/// and in what the keys and the values are spelled with: the punctuation an AWS tag key
/// admits (`_ . : / = + - @`, from AWS's tagging rules), a key and a value at the modelled
/// maximum length (`PrincipalTagID` len 1..128, `PrincipalTagValue` len 1..256, from
/// `smithy_query(query="shape",shape="PrincipalTags")`), unicode, and a row sending the
/// flag and custom tags together.
fn mappings() -> Vec<(&'static str, Option<bool>, Option<Value>)> {
    vec![
        ("neither member sent", None, None),
        ("the default mappings alone", Some(true), None),
        ("the flag sent as false", Some(false), None),
        ("one custom tag", None, Some(tags(1))),
        ("ten custom tags", None, Some(tags(10))),
        (
            "the modelled maximum of fifty custom tags",
            Some(false),
            Some(tags(50)),
        ),
        (
            "the default mappings and custom tags together",
            Some(true),
            Some(json!({"department": "custom:department"})),
        ),
        (
            "keys spelled with every admitted punctuation character",
            None,
            Some(json!({
                "tag_key": "custom:underscore",
                "tag.key": "custom:dot",
                "tag:key": "custom:colon",
                "tag/key": "custom:slash",
                "tag=key": "custom:equals",
                "tag+key": "custom:plus",
                "tag-key": "custom:dash",
                "tag@key": "custom:at",
                "tag key": "custom:space",
                "Tag1": "custom:digit",
            })),
        ),
        (
            "values carrying special characters and unicode",
            None,
            Some(json!({
                "special": "custom:a b/c=d+e-f@g.h:i",
                "unicode": "custom:café — ☕ 团队",
                "quoted": "custom:\"quoted\" back\\slash",
                "clientID": "aud",
                "username": "sub",
            })),
        ),
        (
            "unicode keys and single-character names",
            None,
            Some(json!({"équipe": "custom:équipe", "团队": "x", "a": "b"})),
        ),
        (
            "a key and a value at the modelled maximum length",
            None,
            Some(json!({repeated('k', 128): repeated('v', 256)})),
        ),
    ]
}

/// RULE: a set call stores the mapping it is given for one `(IdentityPoolId,
/// IdentityProviderName)` pair, answers with exactly what it stored, and the get reports
/// that same mapping back — the flag as sent (a call that does not send it does not use the
/// defaults) and every key mapped to the attribute it was given.
#[tokio::test]
async fn the_mapping_a_set_stores_is_the_mapping_a_get_reports() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-round-trip", true).await;

    for (case, use_defaults, principal_tags) in mappings() {
        let answered = emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, USER_POOL_PROVIDER, use_defaults, principal_tags.as_ref()),
            )
            .await;
        // What a call that does not send a member means: no default mappings, and no tag
        // mapped. Asking for the defaults configures them INSTEAD of a custom map.
        let expected_defaults = use_defaults.unwrap_or(false);
        let expected_tags = effective_mapping(use_defaults, principal_tags.as_ref());

        for (operation, got) in [
            ("SetPrincipalTagAttributeMap", answered),
            (
                "GetPrincipalTagAttributeMap",
                emulator
                    .ok(
                        "GetPrincipalTagAttributeMap",
                        &json!({
                            "IdentityPoolId": &pool_id,
                            "IdentityProviderName": USER_POOL_PROVIDER,
                        }),
                    )
                    .await,
            ),
        ] {
            assert_eq!(
                got["IdentityPoolId"].as_str(),
                Some(pool_id.as_str()),
                "case {case:?}: {operation} names the pool it was asked about, got {got}"
            );
            assert_eq!(
                got["IdentityProviderName"].as_str(),
                Some(USER_POOL_PROVIDER),
                "case {case:?}: {operation} names the provider it was asked about, got {got}"
            );
            assert_eq!(
                got["UseDefaults"],
                json!(expected_defaults),
                "case {case:?}: {operation} expected UseDefaults {expected_defaults}, got {got}"
            );
            assert_eq!(
                got["PrincipalTags"], expected_tags,
                "case {case:?}: {operation} expected the stored PrincipalTags {expected_tags}, \
                 got {got}"
            );
        }
    }
}

/// RULE: a set call REPLACES the pair's whole mapping — the tags and the flag a previous
/// call stored are gone rather than merged into.
#[tokio::test]
async fn a_set_replaces_the_whole_mapping() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-replace", true).await;

    // Each row is applied on top of the one before it, so every row is also a case of
    // replacing a mapping that is already stored.
    for (case, use_defaults, principal_tags) in mappings() {
        emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, "graph.facebook.com", use_defaults, principal_tags.as_ref()),
            )
            .await;
        let got = emulator
            .ok(
                "GetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": "graph.facebook.com",
                }),
            )
            .await;
        assert_eq!(
            got["PrincipalTags"],
            effective_mapping(use_defaults, principal_tags.as_ref()),
            "case {case:?}: only the last mapping is reported, got {got}"
        );
        assert_eq!(
            got["UseDefaults"],
            json!(use_defaults.unwrap_or(false)),
            "case {case:?}: only the last flag is reported, got {got}"
        );
    }
}

/// RULE: the mapping belongs to one `(IdentityPoolId, IdentityProviderName)` pair — setting
/// one provider's leaves every other provider's, and every other pool's, where it was, and
/// deleting a pool takes its providers' mappings with it.
#[tokio::test]
async fn a_mapping_belongs_to_one_pool_and_provider() {
    let emulator = Emulator::new();
    let pools = [
        declared_pool(&emulator, "principal-tags-first", true).await,
        declared_pool(&emulator, "principal-tags-second", false).await,
    ];
    let providers = DECLARED_PROVIDERS;

    for (pool_index, pool_id) in pools.iter().enumerate() {
        for (provider_index, provider) in providers.iter().enumerate() {
            emulator
                .ok(
                    "SetPrincipalTagAttributeMap",
                    &set_request(
                        pool_id,
                        provider,
                        Some(provider_index == 1),
                        Some(&json!({
                            format!("tag{pool_index}{provider_index}"):
                                format!("custom:attr{pool_index}{provider_index}"),
                        })),
                    ),
                )
                .await;
        }
    }
    for (pool_index, pool_id) in pools.iter().enumerate() {
        for (provider_index, provider) in providers.iter().enumerate() {
            let got = emulator
                .ok(
                    "GetPrincipalTagAttributeMap",
                    &json!({"IdentityPoolId": pool_id, "IdentityProviderName": provider}),
                )
                .await;
            let custom = json!({
                format!("tag{pool_index}{provider_index}"):
                    format!("custom:attr{pool_index}{provider_index}"),
            });
            assert_eq!(
                got["PrincipalTags"],
                effective_mapping(Some(provider_index == 1), Some(&custom)),
                "case pool {pool_index} provider {provider:?}: each pair keeps its own \
                 mapping, got {got}"
            );
            assert_eq!(
                got["UseDefaults"],
                json!(provider_index == 1),
                "case pool {pool_index} provider {provider:?}: each pair keeps its own flag, \
                 got {got}"
            );
        }
    }

    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &pools[0]}))
        .await;
    for provider in providers {
        let (status, body) = emulator
            .call(
                "GetPrincipalTagAttributeMap",
                &json!({"IdentityPoolId": &pools[0], "IdentityProviderName": provider}),
            )
            .await;
        assert_eq!(
            status, 400,
            "provider {provider:?}: a deleted pool's mappings are gone with it, got {status} \
             with {body}"
        );
    }
    let got = emulator
        .ok(
            "GetPrincipalTagAttributeMap",
            &json!({"IdentityPoolId": &pools[1], "IdentityProviderName": providers[0]}),
        )
        .await;
    assert_eq!(
        got["PrincipalTags"],
        json!({"tag10": "custom:attr10"}),
        "deleting one pool leaves another pool's mappings alone, got {got}"
    );
}

/// RULE: a mapping operation names a pool this service stores, and a read names a provider
/// some call has mapped, or it is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn a_mapping_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-missing", true).await;
    // A pool whose id is well formed but was never created, and one whose mapping was set
    // and whose pool was then deleted, are the same case: nothing is stored.
    let deleted_pool_id = declared_pool(&emulator, "principal-tags-deleted", true).await;
    emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &set_request(&deleted_pool_id, USER_POOL_PROVIDER, Some(true), None),
        )
        .await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": &deleted_pool_id}),
        )
        .await;

    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for absent_pool_id in [ABSENT_POOL_ID.to_string(), deleted_pool_id] {
        cases.push((
            format!("setting a mapping on pool {absent_pool_id}"),
            "SetPrincipalTagAttributeMap",
            set_request(&absent_pool_id, USER_POOL_PROVIDER, Some(true), None),
        ));
        cases.push((
            format!("reading a mapping on pool {absent_pool_id}"),
            "GetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": &absent_pool_id,
                "IdentityProviderName": USER_POOL_PROVIDER,
            }),
        ));
    }
    // A stored pool whose provider nobody has mapped: the pool exists, the mapping does
    // not. Including a provider that is a prefix of one that IS mapped, so the lookup is
    // of the whole name.
    emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &set_request(&pool_id, USER_POOL_PROVIDER, None, Some(&tags(1))),
        )
        .await;
    for unmapped in [
        "graph.facebook.com",
        "cognito-idp.us-east-1.amazonaws.com",
        "COGNITO-IDP.us-east-1.amazonaws.com/us-east-1_abcdefghi",
    ] {
        cases.push((
            format!("reading the unmapped provider {unmapped:?}"),
            "GetPrincipalTagAttributeMap",
            json!({"IdentityPoolId": &pool_id, "IdentityProviderName": unmapped}),
        ));
    }

    for (case, operation, request) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: a `PrincipalTags` key that is no tag key at all — one spelled with characters a
/// tag key does not admit — and a key or value outside the modelled bounds, are refused with
/// HTTP 400, and the pair keeps the mapping it already had. A key in the reserved `aws:`
/// namespace is NOT one of them: that namespace is reserved for the tags AWS assigns to a
/// RESOURCE, while a principal tag is a session tag of the federated principal, so such a
/// mapping is stored and read back.
#[tokio::test]
async fn a_key_that_is_not_a_tag_key_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-refusals", true).await;

    // The mapping every refusal below must leave untouched.
    let stored = json!({"department": "custom:department"});
    emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &set_request(&pool_id, USER_POOL_PROVIDER, Some(false), Some(&stored)),
        )
        .await;

    let refusals: Vec<(&str, Value)> = vec![
        // Characters outside the set AWS's tagging rules publish for a tag key.
        ("an exclamation mark", json!({"tag!": "custom:a"})),
        ("a hash", json!({"tag#1": "custom:a"})),
        ("a percent sign", json!({"100%": "custom:a"})),
        ("a comma", json!({"a,b": "custom:a"})),
        ("a backslash", json!({"a\\b": "custom:a"})),
        ("a quotation mark", json!({"a\"b": "custom:a"})),
        ("a newline", json!({"a\nb": "custom:a"})),
        ("a brace", json!({"{a}": "custom:a"})),
        // The modelled bounds of the key and the value shapes, one past each.
        ("an empty key", json!({"": "custom:a"})),
        (
            "a key one character past the modelled maximum",
            json!({repeated('k', 129): "custom:a"}),
        ),
        (
            "a value one character past the modelled maximum",
            json!({"department": repeated('v', 257)}),
        ),
        ("an empty value", json!({"department": ""})),
    ];

    for (case, principal_tags) in refusals {
        let (status, body) = emulator
            .call(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, USER_POOL_PROVIDER, None, Some(&principal_tags)),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );

        let got = emulator
            .ok(
                "GetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": USER_POOL_PROVIDER,
                }),
            )
            .await;
        assert_eq!(
            got["PrincipalTags"], stored,
            "case {case:?}: a refused call leaves the stored PrincipalTags alone, got {got}"
        );
        assert_eq!(
            got["UseDefaults"],
            json!(false),
            "case {case:?}: a refused call leaves the stored UseDefaults alone, got {got}"
        );
    }

    // A key in the namespace AWS reserves for the tags it assigns to a resource IS a
    // principal tag: the rows span the spellings that namespace is written in, and each is
    // stored and read back.
    for (case, principal_tags) in [
        ("the reserved prefix", json!({"aws:department": "custom:a"})),
        ("the reserved prefix, upper case", json!({"AWS:Tag": "custom:a"})),
        (
            "the reserved prefix beside a key outside it",
            json!({"aws:subject": "sub", "department": "custom:department"}),
        ),
    ] {
        let answered = emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, USER_POOL_PROVIDER, None, Some(&principal_tags)),
            )
            .await;
        assert_eq!(
            answered["PrincipalTags"], principal_tags,
            "case {case:?}: expected the mapping to be echoed, got {answered}"
        );
        let got = emulator
            .ok(
                "GetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": USER_POOL_PROVIDER,
                }),
            )
            .await;
        assert_eq!(
            got["PrincipalTags"], principal_tags,
            "case {case:?}: expected the mapping to be read back, got {got}"
        );
    }
}

/// RULE: a `PrincipalTags` map past the modelled maximum of fifty entries is refused with
/// HTTP 400 rather than stored.
#[tokio::test]
async fn a_map_past_the_modelled_maximum_is_refused() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-too-many", true).await;

    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &set_request(&pool_id, USER_POOL_PROVIDER, None, Some(&tags(51))),
        )
        .await;
    assert_eq!(
        status, 400,
        "fifty-one tags: expected HTTP 400, got {status} with {body}"
    );
    let (status, body) = emulator
        .call(
            "GetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": USER_POOL_PROVIDER,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "fifty-one tags: the refused mapping is not stored, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "fifty-one tags: the refused mapping is not stored, got {body}"
    );
}

/// RULE: asking for the default mappings configures them INSTEAD of a custom map — the
/// mapping the pair ends up with is the default one, whatever custom map the same call, or
/// a previous one, carried; and turning the flag off again leaves the call's own custom map.
#[tokio::test]
async fn the_defaults_replace_a_custom_mapping_rather_than_merging_with_it() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-defaults", true).await;

    // Each row is a custom map that must NOT survive the call asking for the defaults: one
    // mapping a tag the defaults also carry, one mapping a tag they do not, one mapping a
    // default tag key to another attribute, and none at all.
    let conflicting: Vec<(&str, Option<Value>)> = vec![
        ("no custom map at all", None),
        ("an empty custom map", Some(json!({}))),
        (
            "a tag the defaults do not carry",
            Some(json!({"department": "sub"})),
        ),
        (
            "a default tag key mapped to another attribute",
            Some(json!({"username": "custom:login", "clientID": "custom:app"})),
        ),
        (
            "several custom tags",
            Some(json!({"a": "sub", "b": "aud", "c": "custom:c"})),
        ),
    ];

    for (case, principal_tags) in conflicting {
        // Start from a custom mapping, so the defaults also have something to replace.
        emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(
                    &pool_id,
                    USER_POOL_PROVIDER,
                    None,
                    Some(&json!({"previous": "custom:previous"})),
                ),
            )
            .await;

        let answered = emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(
                    &pool_id,
                    USER_POOL_PROVIDER,
                    Some(true),
                    principal_tags.as_ref(),
                ),
            )
            .await;
        let got = emulator
            .ok(
                "GetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": USER_POOL_PROVIDER,
                }),
            )
            .await;
        for (operation, reported) in [
            ("SetPrincipalTagAttributeMap", &answered),
            ("GetPrincipalTagAttributeMap", &got),
        ] {
            assert_eq!(
                reported["UseDefaults"],
                json!(true),
                "case {case:?}: {operation} reports the flag it was given, got {reported}"
            );
            assert_eq!(
                reported["PrincipalTags"],
                default_mapping(),
                "case {case:?}: {operation} expected the default mapping {}, got {reported}",
                default_mapping()
            );
        }

        // Turning the flag off again is the custom map's call once more: the defaults do
        // not linger.
        let custom = json!({"tenant": "custom:tenant"});
        let off = emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, USER_POOL_PROVIDER, Some(false), Some(&custom)),
            )
            .await;
        assert_eq!(
            off["PrincipalTags"], custom,
            "case {case:?}: the flag off leaves the call's own custom map, got {off}"
        );
        assert_eq!(
            off["UseDefaults"],
            json!(false),
            "case {case:?}: the flag off is reported as off, got {off}"
        );
    }
}

/// RULE: a `PrincipalTags` key outside the bounds the model states for `PrincipalTagID` is
/// refused with HTTP 400 `ValidationException` — the code AWS answers for a modelled
/// constraint — and the refusal is built from the request: it names the member, the value
/// the request gave it, and the constraint that value failed, however many of the map's
/// keys are outside the bound.
#[tokio::test]
async fn a_key_outside_the_modelled_bounds_is_refused_naming_the_member_and_the_constraint() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-messages", true).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "one key one character past the maximum of 128",
            json!({repeated('k', 129): "custom:a"}),
        ),
        (
            "two keys past the maximum",
            json!({repeated('a', 129): "custom:a", repeated('b', 200): "custom:b"}),
        ),
        ("a key below the minimum of 1", json!({"": "custom:a"})),
        (
            "one key inside the bound and one outside",
            json!({
                "department": "custom:department",
                repeated('c', 129): "custom:c",
            }),
        ),
        (
            "a key past the maximum counted in characters, not bytes",
            json!({repeated('é', 129): "custom:accented"}),
        ),
    ];

    for (case, principal_tags) in cases {
        let (status, body) = emulator
            .call(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, USER_POOL_PROVIDER, None, Some(&principal_tags)),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries a message: {body}"));
        assert!(
            message.contains("at 'principalTags' failed to satisfy constraint"),
            "case {case:?}: expected the refusal to name the member, got {message:?}"
        );
        assert!(
            message.contains("Member must have length less than or equal to 128")
                && message.contains("Member must have length greater than or equal to 1"),
            "case {case:?}: expected the refusal to name the constraint the value failed, got \
             {message:?}"
        );
        // The value the request gave the member, as the refusal names it: every entry of
        // the map, whether or not that entry is the offending one.
        for (tag_key, attribute) in principal_tags
            .as_object()
            .expect("the row is a PrincipalTags map")
        {
            let rendered = format!(
                "{tag_key}={}",
                attribute.as_str().expect("the attribute is a string")
            );
            assert!(
                message.contains(&rendered),
                "case {case:?}: expected the refusal to name the value given, missing \
                 {rendered:?} in {message:?}"
            );
        }
    }
}

/// RULE: a pool holds principal-tag attribute maps for at most as many providers as this
/// emulator keeps — a mapping for a provider past that ceiling is refused with HTTP 400,
/// while REPLACING the mapping of a provider the pool already holds one for always works,
/// however many times it is called.
#[tokio::test]
async fn a_pool_holds_a_bounded_number_of_provider_mappings() {
    let emulator = Emulator::new();
    // The ceiling this emulator keeps per pool, from
    // `crate::state::MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL`.
    let ceiling = 50;
    // A mapping is stored only for a provider the pool DECLARES, so the pool declares one
    // more provider than the ceiling admits mappings for: what refuses the last call is the
    // ceiling and not the pool's configuration.
    let declared: Vec<String> = (0..=ceiling)
        .map(|index| format!("provider-{index}.example.com"))
        .collect();
    let names: Vec<&str> = declared.iter().map(String::as_str).collect();
    let pool_id = emulator
        .pool_with_providers("principal-tags-ceiling", true, &names)
        .await;

    for name in &names[..ceiling] {
        emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, name, None, Some(&tags(1))),
            )
            .await;
    }

    // One provider past the ceiling is refused, and nothing is stored for it.
    let past = names[ceiling];
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &set_request(&pool_id, past, None, Some(&tags(1))),
        )
        .await;
    assert_eq!(
        status, 400,
        "a provider past the ceiling: expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterException"),
        "a provider past the ceiling: expected InvalidParameterException, got {body}"
    );
    let (status, body) = emulator
        .call(
            "GetPrincipalTagAttributeMap",
            &json!({"IdentityPoolId": &pool_id, "IdentityProviderName": past}),
        )
        .await;
    assert_eq!(
        status, 400,
        "a provider past the ceiling: the refused mapping is not stored, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "a provider past the ceiling: the refused mapping is not stored, got {body}"
    );

    // Replacing a mapping the pool already holds grows nothing, so it is never refused.
    for round in 0..3 {
        let replacement = json!({format!("round{round}"): format!("custom:round{round}")});
        let answered = emulator
            .ok(
                "SetPrincipalTagAttributeMap",
                &set_request(&pool_id, names[0], None, Some(&replacement)),
            )
            .await;
        assert_eq!(
            answered["PrincipalTags"], replacement,
            "round {round}: replacing a stored mapping works at the ceiling, got {answered}"
        );
    }

    // The ceiling is per POOL: another pool starts with none of its own.
    let other_pool_id = emulator
        .pool_with_providers("principal-tags-ceiling-other", true, &[past])
        .await;
    let answered = emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &set_request(&other_pool_id, past, None, Some(&tags(1))),
        )
        .await;
    assert_eq!(
        answered["PrincipalTags"],
        tags(1),
        "the ceiling is per pool, got {answered}"
    );
}

/// RULE: a request that is invalid is refused for being invalid before the resource it
/// names is looked up — a malformed `PrincipalTags` entry is HTTP 400 whether or not the
/// pool it names is stored, the way the protocol's own boundary walk refuses a modelled
/// constraint before any handler runs.
#[tokio::test]
async fn an_invalid_request_is_refused_before_the_pool_is_looked_up() {
    let emulator = Emulator::new();
    let stored_pool_id = declared_pool(&emulator, "principal-tags-ordering", true).await;

    for (case, principal_tags) in [
        ("a key past the modelled maximum", json!({repeated('k', 129): "custom:a"})),
        ("an empty key", json!({"": "custom:a"})),
        ("a key spelled with a character a tag key does not admit", json!({"tag#1": "custom:a"})),
        ("a map past the modelled maximum", tags(51)),
    ] {
        for (pool_case, pool_id) in [
            ("a stored pool", stored_pool_id.as_str()),
            ("a pool that is not stored", ABSENT_POOL_ID),
        ] {
            let (status, body) = emulator
                .call(
                    "SetPrincipalTagAttributeMap",
                    &set_request(pool_id, USER_POOL_PROVIDER, None, Some(&principal_tags)),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {pool_case}: expected HTTP 400 for the invalid request, got \
                 {status} with {body}"
            );
            assert!(
                !error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?} on {pool_case}: expected the request's own invalidity to be \
                 reported rather than the pool's absence, got {body}"
            );
        }
    }

    // A well-formed request naming a pool that is not stored reports the pool's absence
    // instead: it is the request's validity, not the operation, that decides which refusal
    // comes first.
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &set_request(ABSENT_POOL_ID, USER_POOL_PROVIDER, None, Some(&tags(1))),
        )
        .await;
    assert_eq!(
        status, 400,
        "a well-formed request on a pool that is not stored: expected HTTP 400, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "a well-formed request on a pool that is not stored: expected the pool's absence to be \
         reported, got {body}"
    );
}

/// RULE: reading a mapping changes nothing a later caller sees — the same read answers the
/// same mapping however many times it is called.
#[tokio::test]
async fn reading_a_mapping_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = declared_pool(&emulator, "principal-tags-reads", true).await;
    let principal_tags = json!({"tenant": "custom:tenant", "clientID": "aud"});
    emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &set_request(
                &pool_id,
                USER_POOL_PROVIDER,
                Some(false),
                Some(&principal_tags),
            ),
        )
        .await;

    let request = json!({
        "IdentityPoolId": &pool_id,
        "IdentityProviderName": USER_POOL_PROVIDER,
    });
    let first = emulator.ok("GetPrincipalTagAttributeMap", &request).await;
    for read in 1..4 {
        let again = emulator.ok("GetPrincipalTagAttributeMap", &request).await;
        assert_eq!(
            again, first,
            "read {read}: expected the same mapping {first}, got {again}"
        );
        assert_eq!(
            again["PrincipalTags"], principal_tags,
            "read {read}: expected the stored PrincipalTags {principal_tags}, got {again}"
        );
    }
}
