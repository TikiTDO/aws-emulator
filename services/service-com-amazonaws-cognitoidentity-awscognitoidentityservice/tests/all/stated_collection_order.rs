// SPDX-License-Identifier: CC0-1.0

//! The ORDER a constraint refusal of this service states a COLLECTION in.
//!
//! One rule, over the two collections a clause of this service embeds: a collection stated
//! inside a constraint-violation message is listed in the order the TABLE holding it is
//! walked — bucket by bucket, the bucket being the low bits of each element's 31-based
//! string hash mixed with its own high half — and never sorted, nor in the order the request
//! wrote it. The two collections are:
//!
//! * the values an enumerated type admits, named by a clause about a value outside that
//!   type;
//! * the entries of a MAP the request submitted, quoted by a clause about that map member.
//!
//! The order is a function of the element TEXT and, for a map, of the entry COUNT (which
//! decides the table's width) — so it is the same on every run and for every member
//! carrying the same collection, which is why it is asserted here rather than taken from
//! one recorded response.
//!
//! Every bucket below is arrived at from the rule, not from this emulator's output: a key of
//! one ASCII character hashes to that character's own code point and one of two characters
//! to `31 * <first> + <second>`, neither of which reaches the high half of the hash, so each
//! bucket is that number masked to the table's width. The arithmetic is spelled out beside
//! each table.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::identity_pool_flow::{error_code, Emulator};

/// The error code the constraint layer ahead of every handler refuses with — the protocol's
/// own, which no operation of this service declares.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The message a refusal states, which every refusal has to carry.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// The modelled maximum of `SetPrincipalTagAttributeMap$PrincipalTags`
/// (`smithy_query(query="shape",shape="PrincipalTags")`: `length 0..50`).
const PRINCIPAL_TAGS_MAX: usize = 50;

/// The map the rows below submit: one entry per key, all carrying `value`.
fn tags(keys: &[&str], value: &str) -> Value {
    Value::Object(
        keys.iter()
            .map(|key| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// RULE: a clause quoting a MAP member states the entries bucket by bucket of the table
/// holding them — not sorted by key — with two keys of one bucket in the order they were
/// submitted.
#[tokio::test]
async fn a_clause_about_a_map_states_its_entries_in_the_order_its_table_is_walked() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .pool_with_providers("stated-order-tags", true, &["graph.facebook.com"])
        .await;

    // 51 keys — one past the modelled maximum, so the map's own `@length` is what refuses
    // the call — of two characters each, whose buckets in the 128-bucket table that many
    // entries are held in are `(31 * <letter> + <digit>) & 127` xor `1680 & 127 = 16`
    // (`1680` being the high half of every such hash):
    //
    //   a0..a9 -> 111..120    b0..b9 ->  14..23    c0..c9 ->  45..54
    //   d0..d9 ->  76..85     f0..f9 ->  10..19    g0     ->  41
    //
    // so 'b' and 'f' share the six buckets 14..19, where the two keys of one bucket are
    // stated in key order.
    let keys: Vec<String> = ["a", "b", "c", "d", "f"]
        .iter()
        .flat_map(|letter| (0..10).map(move |digit| format!("{letter}{digit}")))
        .chain(std::iter::once("g0".to_string()))
        .collect();
    let submitted: Vec<&str> = keys.iter().map(String::as_str).collect();
    assert_eq!(
        submitted.len(),
        PRINCIPAL_TAGS_MAX + 1,
        "this case needs one entry past the modelled maximum, got {}",
        submitted.len()
    );
    let stated = [
        "f0", "f1", "f2", "f3", "b0", "f4", "b1", "f5", "b2", "f6", "b3", "f7", "b4", "f8", "b5",
        "f9", "b6", "b7", "b8", "b9", "g0", "c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8",
        "c9", "d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9", "a0", "a1", "a2", "a3",
        "a4", "a5", "a6", "a7", "a8", "a9",
    ];

    // The rows differ along the VALUE the entries carry: the order follows the keys and the
    // entry count, so it does not move when the values do.
    for (case, value) in [
        ("fifty-one tags", "sub"),
        ("the same keys carrying another value", "custom:department"),
    ] {
        let quoted = format!(
            "{{{}}}",
            stated
                .iter()
                .map(|key| format!("{key}={value}"))
                .collect::<Vec<String>>()
                .join(", ")
        );
        let (status, body) = emulator
            .call(
                "SetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": "graph.facebook.com",
                    "PrincipalTags": tags(&submitted, value),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {body}"
        );
        assert_eq!(
            message(&body, case),
            format!(
                "1 validation error detected: Value '{quoted}' at 'principalTags' failed to \
                 satisfy constraint: Member must have length less than or equal to \
                 {PRINCIPAL_TAGS_MAX}"
            ),
            "case {case:?}: expected the map stated bucket by bucket, got {body}"
        );
    }
}

/// An IAM role ARN of this emulator's account, so a role mapping is refused for the thing a
/// case is about rather than for its role.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// RULE: a clause about a value outside an enumerated type names that type's admissible
/// values bucket by bucket of the table holding them — not sorted, and not in the order the
/// model declares them.
#[tokio::test]
async fn a_clause_about_an_enumerated_type_states_its_values_in_the_order_its_table_is_walked() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("stated-order-enums", true).await;
    let provider = "graph.facebook.com";
    let mapped = role_arn("Cognito_statedOrderMapped_Role");

    // The three enumerated types a `SetIdentityPoolRoles` request carries, each with the
    // bucket its values fall in. A value's bucket is its 31-based hash reduced to the
    // 32-bucket table of admissible values, which for these values is the alternating sum of
    // their UTF-16 code units (31 ≡ -1, so every other unit is subtracted), taken from the
    // last unit:
    //
    //   MappingRuleMatchType          Equals 31, Contains 31, StartsWith 23, NotEqual 1
    //   RoleMappingType               Token 25, Rules 23
    //   AmbiguousRoleResolutionType   AuthenticatedRole 5, Deny 12
    //
    // `Equals` and `Contains` share bucket 31, where the chain is read from the head each
    // value was pushed onto, so the later-declared `Contains` is stated first.
    //
    // (case, the member path the clause names, the rejected value, the request, the value
    // set as stated)
    let rows: Vec<(&str, &str, &str, Value, &str)> = vec![
        (
            "a rule's MatchType outside MappingRuleMatchType",
            "roleMappings.graph.facebook.com.member.rulesConfiguration.rules.1.member.matchType",
            "Matches",
            json!({
                provider: {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "isAdmin",
                        "MatchType": "Matches",
                        "Value": "yes",
                        "RoleARN": &mapped,
                    }]},
                },
            }),
            "NotEqual, StartsWith, Contains, Equals",
        ),
        (
            "a mapping's Type outside RoleMappingType",
            "roleMappings.graph.facebook.com.member.type",
            "RulesEngine",
            json!({
                provider: {"Type": "RulesEngine", "AmbiguousRoleResolution": "Deny"},
            }),
            "Rules, Token",
        ),
        (
            "a mapping's AmbiguousRoleResolution outside AmbiguousRoleResolutionType",
            "roleMappings.graph.facebook.com.member.ambiguousRoleResolution",
            "Allow",
            json!({
                provider: {"Type": "Token", "AmbiguousRoleResolution": "Allow"},
            }),
            "AuthenticatedRole, Deny",
        ),
    ];

    for (case, path, rejected, role_mappings, set) in rows {
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"authenticated": role_arn("Cognito_statedOrderAuth_Role")},
                    "RoleMappings": role_mappings,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {body}"
        );
        assert_eq!(
            message(&body, case),
            format!(
                "1 validation error detected: Value '{rejected}' at '{path}' failed to satisfy \
                 constraint: Member must satisfy enum value set: [{set}]"
            ),
            "case {case:?}: expected the value set stated bucket by bucket, got {body}"
        );
    }

    // A value INSIDE each of those types is served, so the refusals above are about the
    // value being outside the type and not about the request's shape.
    let (status, body) = emulator
        .call(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": role_arn("Cognito_statedOrderAuth_Role")},
                "RoleMappings": {
                    provider: {
                        "Type": "Rules",
                        "AmbiguousRoleResolution": "AuthenticatedRole",
                        "RulesConfiguration": {"Rules": [{
                            "Claim": "isAdmin",
                            "MatchType": "Equals",
                            "Value": "yes",
                            "RoleARN": &mapped,
                        }]},
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"every value inside its type\": expected HTTP 200, got {status} with {body}"
    );
}

/// RULE: a role mapping that states no `AmbiguousRoleResolution` is refused with the
/// sentence AWS words that condition with, over the `Type` that requires it and nothing else
/// of the request.
#[tokio::test]
async fn a_mapping_with_no_ambiguous_role_resolution_names_the_type_that_requires_it() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("stated-order-ambiguous", true).await;
    let mapped = role_arn("Cognito_statedOrderMapped_Role");

    // The rows span both values of `RoleMappingType` — the model's
    // `RoleMapping$AmbiguousRoleResolution` states "If you specify Token or Rules as the
    // Type, AmbiguousRoleResolution is required" — and, within `Rules`, the provider the
    // mapping is keyed by and how many mappings the call carries, which the sentence names
    // nothing of.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a Token mapping",
            "Token",
            json!({"graph.facebook.com": {"Type": "Token"}}),
        ),
        (
            "a Rules mapping",
            "Rules",
            json!({
                "graph.facebook.com": {
                    "Type": "Rules",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "isAdmin",
                        "MatchType": "Equals",
                        "Value": "yes",
                        "RoleARN": &mapped,
                    }]},
                },
            }),
        ),
        (
            "a Rules mapping keyed by a user pool, beside a complete one",
            "Rules",
            json!({
                "accounts.google.com": {
                    "Type": "Token",
                    "AmbiguousRoleResolution": "Deny",
                },
                "cognito-idp.us-east-1.amazonaws.com/us-east-1_stated1:statedclient": {
                    "Type": "Rules",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "isAdmin",
                        "MatchType": "Equals",
                        "Value": "yes",
                        "RoleARN": &mapped,
                    }]},
                },
            }),
        ),
    ];

    for (case, role_mapping_type, role_mappings) in rows {
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"authenticated": role_arn("Cognito_statedOrderAuth_Role")},
                    "RoleMappings": role_mappings,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        assert_eq!(
            message(&body, case),
            format!("AmbiguousRoleResolution must be specified for {role_mapping_type}"),
            "case {case:?}: expected the sentence over the Type, got {body}"
        );
    }
}
