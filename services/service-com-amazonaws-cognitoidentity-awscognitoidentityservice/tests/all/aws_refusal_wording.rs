// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusals whose message AWS fixes, over the operations that answer
//! them.
//!
//! RULE: a refusal carries the message AWS words for its CONDITION, with only the parts
//! AWS takes from the request in it — never a sentence this emulator authors and never a
//! value read out of stored state:
//!
//! * an identity this service holds no record of is `Identity '<IdentityId>' not found.`,
//!   whichever operation addressed it, naming the id the REQUEST carried and nothing of the
//!   pool it was looked for in (an identity of another pool is a record this service DOES
//!   hold, so it is an ownership refusal instead — `developer_identity_resolution`);
//! * a developer-identity mapping the caller may not resolve — an identity of another pool,
//!   an identifier the pool has registered against no identity — is
//!   `You do not have access to this identity`, one fixed sentence naming neither;
//! * a `RoleMappings` entry of type `Rules` carrying no `RulesConfiguration` is
//!   `RuleConfiguration is required for Rules`, naming neither the provider the mapping is
//!   keyed by nor the pool;
//! * a tag write that would take a pool past its published ceiling is
//!   `The resultant tag set must not have more than <ceiling> user tags.`, naming the
//!   ceiling and nothing of the pool.
//!
//! Each case asserts the rule over a TABLE spanning the class, its rows differing along
//! what the rule generalises over: which operation answers the refusal, which `IdentityId`
//! it was asked about, how the mapping is incomplete, and how a tag set crosses the
//! ceiling.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID, DEFAULT_REGION};

/// The developer provider every pool below is created with, so the developer-identity
/// operations have one to name.
const DEVELOPER_PROVIDER: &str = "login.wording.developer";

/// The published identity-pool tag ceiling, from the model's own `TagResource` doc: "An
/// identity pool can have as many as 50 tags".
const TAG_CEILING: usize = 50;

/// The message an error response carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// A pool carrying a developer provider, answering its minted id.
async fn pool(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string()
}

/// A well-formed `IdentityId` of `region` that no call ever minted.
fn absent_identity_id(number: u8) -> String {
    format!("{DEFAULT_REGION}:0000000{number}-0000-0000-0000-000000000000")
}

#[tokio::test]
async fn an_identity_this_service_does_not_hold_is_reported_as_not_found() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "wording-identities").await;

    // (case, the identity the request names, the operations that hold no such identity)
    //
    // An identity no call ever minted is held by none of them.
    let identities: Vec<(&str, String, &[&str])> = vec![
        (
            "an id no call ever minted",
            absent_identity_id(1),
            &[
                "DescribeIdentity",
                "GetOpenIdTokenForDeveloperIdentity",
                "GetOpenIdToken",
                "GetCredentialsForIdentity",
            ],
        ),
        (
            "a second id no call ever minted",
            absent_identity_id(2),
            &[
                "DescribeIdentity",
                "GetOpenIdTokenForDeveloperIdentity",
                "GetOpenIdToken",
                "GetCredentialsForIdentity",
            ],
        ),
    ];

    for (case, identity_id, operations) in &identities {
        // The request each operation addresses that identity with.
        let requests: Vec<(&str, Value)> = vec![
            (
                "DescribeIdentity",
                json!({"IdentityId": identity_id}),
            ),
            (
                "GetOpenIdTokenForDeveloperIdentity",
                json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityId": identity_id,
                    "Logins": {DEVELOPER_PROVIDER: "wording-developer-user"},
                }),
            ),
            (
                "GetOpenIdToken",
                json!({"IdentityId": identity_id}),
            ),
            (
                "GetCredentialsForIdentity",
                json!({"IdentityId": identity_id}),
            ),
        ];
        for (operation, request) in requests {
            if !operations.contains(&operation) {
                continue;
            }
            let row = format!("{operation} on {case}");
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {row:?}: expected (400, ResourceNotFoundException), got ({status}, {body})"
            );
            assert_eq!(
                message(&row, &body),
                format!("Identity '{identity_id}' not found."),
                "case {row:?}: expected AWS's wording, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_developer_identity_the_caller_may_not_resolve_is_one_fixed_sentence() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "wording-developer").await;
    let other_pool_id = pool(&emulator, "wording-developer-other").await;
    // An identity of the OTHER pool, registered under that pool's developer provider.
    let foreign_identity = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": &other_pool_id,
                "Logins": {DEVELOPER_PROVIDER: "wording-foreign-user"},
            }),
        )
        .await["IdentityId"]
        .as_str()
        .expect("the exchange answers an IdentityId")
        .to_string();
    // An identity of THIS pool that no developer user identifier is registered under.
    let unregistered_identity = emulator.identity(&pool_id, None).await;

    // (case, the LookupDeveloperIdentity request)
    let rows: Vec<(&str, Value)> = vec![
        (
            "an identity of another pool",
            json!({"IdentityPoolId": &pool_id, "IdentityId": &foreign_identity}),
        ),
        (
            "an identity this pool has registered no developer user for",
            json!({"IdentityPoolId": &pool_id, "IdentityId": &unregistered_identity}),
        ),
        (
            "a developer user identifier this pool has not registered",
            json!({
                "IdentityPoolId": &pool_id,
                "DeveloperUserIdentifier": "wording-unknown-user",
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = emulator.call("LookupDeveloperIdentity", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "NotAuthorizedException"),
            "case {case:?}: expected (400, NotAuthorizedException), got ({status}, {body})"
        );
        assert_eq!(
            message(case, &body),
            "You do not have access to this identity",
            "case {case:?}: expected AWS's fixed sentence, got {body}"
        );
    }
}

#[tokio::test]
async fn a_rules_mapping_with_no_rules_configuration_is_one_fixed_sentence() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "wording-role-mappings").await;
    let role_arn = format!(
        "arn:aws:iam::{}:role/wording-authenticated",
        engine::emulator_account_id()
    );

    // The rows differ along the provider the incomplete mapping is keyed by, along which
    // ambiguous resolution it states, and along how many mappings the call carries — the
    // sentence names none of that.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one mapping keyed by a public provider",
            json!({
                "graph.facebook.com": {"Type": "Rules", "AmbiguousRoleResolution": "Deny"},
            }),
        ),
        (
            "one mapping keyed by a user pool and its app client",
            json!({
                "cognito-idp.us-east-1.amazonaws.com/us-east-1_wording1:wordingclient": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "AuthenticatedRole",
                },
            }),
        ),
        (
            "an incomplete mapping beside a complete one",
            json!({
                "accounts.google.com": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "isAdmin",
                        "MatchType": "Equals",
                        "Value": "yes",
                        "RoleARN": &role_arn,
                    }]},
                },
                "www.amazon.com": {"Type": "Rules", "AmbiguousRoleResolution": "Deny"},
            }),
        ),
    ];

    for (case, role_mappings) in rows {
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"authenticated": &role_arn},
                    "RoleMappings": role_mappings,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected (400, InvalidParameterException), got ({status}, {body})"
        );
        assert_eq!(
            message(case, &body),
            "RuleConfiguration is required for Rules",
            "case {case:?}: expected AWS's fixed sentence, got {body}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_past_the_ceiling_names_the_ceiling_and_nothing_of_the_pool() {
    let emulator = Emulator::new();
    let expected =
        format!("The resultant tag set must not have more than {TAG_CEILING} user tags.");

    // (case, the tags the pool is created with, the tags the call assigns) — the rows differ
    // along how the resulting set crosses the ceiling: a single oversized call, a call that
    // crosses it by one, and one that crosses it by many.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("one call one past the ceiling", 0, TAG_CEILING + 1),
        ("one call far past the ceiling", 0, TAG_CEILING + 11),
        ("a full pool and one more key", TAG_CEILING, 1),
        ("a nearly full pool and two more", TAG_CEILING - 1, 2),
    ];

    for (case, held, assigned) in rows {
        let pool_id = emulator
            .ok(
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": format!("wording-tags-{held}-{assigned}"),
                    "AllowUnauthenticatedIdentities": true,
                    "IdentityPoolTags": numbered("held", held),
                }),
            )
            .await["IdentityPoolId"]
            .as_str()
            .expect("CreateIdentityPool answers an IdentityPoolId")
            .to_string();
        let (status, body) = emulator
            .call(
                "TagResource",
                &json!({
                    "ResourceArn": format!(
                        "arn:aws:cognito-identity:{DEFAULT_REGION}:{}:identitypool/{pool_id}",
                        engine::emulator_account_id()
                    ),
                    "Tags": numbered("added", assigned),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected (400, InvalidParameterException), got ({status}, {body})"
        );
        assert_eq!(
            message(case, &body),
            expected,
            "case {case:?}: expected AWS's wording, got {body}"
        );
        assert!(
            !message(case, &body).contains(&pool_id),
            "case {case:?}: the message names nothing of the pool, got {body}"
        );
    }
}

/// RULE, the other side of the ceiling: a tag write that leaves the pool AT the ceiling is
/// served, so the refusal above is about the ceiling being crossed.
#[tokio::test]
async fn a_tag_set_on_the_ceiling_is_served() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "wording-tags-on-the-ceiling",
                "AllowUnauthenticatedIdentities": true,
                "IdentityPoolTags": numbered("held", TAG_CEILING - 1),
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string();
    let (status, body) = emulator
        .call(
            "TagResource",
            &json!({
                "ResourceArn": format!(
                    "arn:aws:cognito-identity:{DEFAULT_REGION}:{}:identitypool/{pool_id}",
                    engine::emulator_account_id()
                ),
                "Tags": numbered("added", 1),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a set landing on the ceiling\": expected HTTP 200, got {status} with {body}"
    );
}

/// `count` tags, each key prefixed with `prefix`.
fn numbered(prefix: &str, count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("{prefix}{index:03}"), json!("tag-value"));
    }
    Value::Object(tags)
}

/// RULE, applied to the pool a refusal is about: a refusal naming an identity names the id
/// the REQUEST carried, so a request naming a pool that is not stored still reports the
/// pool rather than the identity — which is what keeps the two conditions apart.
#[tokio::test]
async fn a_pool_that_is_not_stored_is_reported_as_the_missing_resource() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call(
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityId": absent_identity_id(3),
                "Logins": {DEVELOPER_PROVIDER: "wording-developer-user"},
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case \"a pool nothing created\": expected (400, ResourceNotFoundException), got \
         ({status}, {body})"
    );
    assert_eq!(
        message("a pool nothing created", &body),
        format!("IdentityPool '{ABSENT_POOL_ID}' not found."),
        "case \"a pool nothing created\": the pool is the missing resource, got {body}"
    );
}
