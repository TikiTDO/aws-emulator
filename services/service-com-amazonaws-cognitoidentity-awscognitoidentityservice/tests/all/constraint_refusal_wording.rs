// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal: the message, which is as much a part of the wire
//! contract as the status and the error code — it is the only machine-readable description
//! of what was refused, and a client (or an operator's log pipeline) branches on it.
//!
//! Five rules, each asserted over a table of inputs rather than over one recorded call://!
//! * a clause about a value that broke a constraint the MODEL states QUOTES that value, and
//!   for a COLLECTION member the value is the collection itself — a list as
//!   `[<element>, …]`, a map as `{<key>=<value>, …}` — never a count of what it held. The
//!   rows span both collection kinds, both `@length` bounds (a list under its minimum and
//!   one over its maximum), the empty collection, and a request inside the bound, which is
//!   not refused at all;
//! * a constraint the collection's ELEMENTS broke is stated once at the COLLECTION member,
//!   quoting the collection and listing every requirement the element shape declares,
//!   however many elements broke it;
//! * a map KEY outside the constraints its key shape declares is the protocol's
//!   `ValidationException` (HTTP 400) — not one of the service's own modelled errors — with
//!   the map quoted and every requirement a key of that map has to satisfy listed. The rows
//!   span the two maps this service applies a key shape to and the forms a bad key takes;
//! * a refusal AWS words with a FIXED sentence carries that sentence and nothing of the
//!   request: a tag key in the namespace AWS reserves for its own tags, and a `ResourceArn`
//!   that is no ARN of this service. The rows span every operation that can answer each,
//!   and several forms of the offending value, because the sentence names none of them;
//! * a request whose developer provider is not the one its identity pool is configured with
//!   is refused with the one sentence this service states for that, whether the pool names
//!   no developer provider at all or names another one.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::{
    identity_pool_arn, ARN_VALIDATION_FAILED_MESSAGE, RESERVED_TAG_KEY_PREFIX,
    SYSTEM_TAG_MUTATION_MESSAGE, UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
};

use crate::identity_pool_flow::{error_code, Emulator, DEFAULT_REGION};

/// The error code the constraint layer ahead of every handler refuses with — the
/// protocol's own, which no operation of this service declares.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this service's handlers answer for a request whose values are
/// well-formed but cannot be served.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The message a refusal states, which every refusal has to carry.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// A well-formed `IdentityId` of `region`, numbered so a case can build as many distinct
/// ids as it needs. `IdentityId` is modelled `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`, so
/// the number is rendered as hex in the uuid-shaped half.
fn identity_id(region: &str, number: usize) -> String {
    format!("{region}:00000000-0000-4000-8000-{number:012x}")
}

/// The modelled maximum of `DeleteIdentities$IdentityIdsToDelete`
/// (`smithy_query(query="shape",shape="IdentityIdList")`: `length 1..60`), and its minimum.
const IDENTITY_IDS_MAX: usize = 60;
const IDENTITY_IDS_MIN: usize = 1;

/// The modelled maximum of `SetPrincipalTagAttributeMap$PrincipalTags`
/// (`smithy_query(query="shape",shape="PrincipalTags")`: `length 0..50`).
const PRINCIPAL_TAGS_MAX: usize = 50;

/// RULE: a clause about a COLLECTION member quotes the collection the request carried.
#[tokio::test]
async fn a_refusal_about_a_collection_quotes_the_collection_the_request_sent() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .pool_with_providers(
            "constraint-refusal-collections",
            true,
            // The provider the principal-tag rows below write a mapping for: a mapping is
            // stored only for a provider the pool declares.
            &["graph.facebook.com"],
        )
        .await;

    // One row over the DeleteIdentities list: a batch past the modelled maximum is refused
    // quoting every id submitted, in the order the request carried them.
    let over_maximum: Vec<String> = (0..=IDENTITY_IDS_MAX)
        .map(|number| identity_id(DEFAULT_REGION, number))
        .collect();
    let quoted_list = format!("[{}]", over_maximum.join(", "));
    let (status, body) = emulator
        .call(
            "DeleteIdentities",
            &json!({ "IdentityIdsToDelete": over_maximum }),
        )
        .await;
    let case = "a batch one id past the modelled maximum";
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
            "1 validation error detected: Value '{quoted_list}' at 'identityIdsToDelete' failed \
             to satisfy constraint: Member must have length less than or equal to \
             {IDENTITY_IDS_MAX}"
        ),
        "case {case:?}: expected the submitted list quoted, got {body}"
    );

    // And one row at the other bound: an EMPTY list is under the modelled minimum, and the
    // value quoted is the empty list rather than a count of nothing.
    let empty: Vec<String> = Vec::new();
    let (status, body) = emulator
        .call("DeleteIdentities", &json!({ "IdentityIdsToDelete": empty }))
        .await;
    let case = "an empty batch, under the modelled minimum";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        message(&body, case),
        format!(
            "1 validation error detected: Value '[]' at 'identityIdsToDelete' failed to satisfy \
             constraint: Member must have length greater than or equal to {IDENTITY_IDS_MIN}"
        ),
        "case {case:?}: expected the empty list quoted, got {body}"
    );

    // A list INSIDE the bounds is not refused for its length at all: the rule is about the
    // clause a broken bound states, not about every list.
    let (status, body) = emulator
        .call(
            "DeleteIdentities",
            &json!({ "IdentityIdsToDelete": [identity_id(DEFAULT_REGION, 1)] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a batch inside the bounds\": expected HTTP 200, got {status} with {body}"
    );

    // The same rule over a MAP member: a principal-tag map one entry past its modelled
    // maximum quotes the map, in the order a map of this service is stated in.
    let over_tags: Vec<(String, String)> = (0..=PRINCIPAL_TAGS_MAX)
        .map(|number| (format!("tag{number:03}"), format!("claim{number:03}")))
        .collect();
    let quoted_map = {
        let mut entries: Vec<String> = over_tags
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        entries.sort_unstable();
        format!("{{{}}}", entries.join(", "))
    };
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "PrincipalTags": Value::Object(
                    over_tags
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect(),
                ),
            }),
        )
        .await;
    let case = "a principal-tag map one entry past the modelled maximum";
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
            "1 validation error detected: Value '{quoted_map}' at 'principalTags' failed to \
             satisfy constraint: Member must have length less than or equal to \
             {PRINCIPAL_TAGS_MAX}"
        ),
        "case {case:?}: expected the submitted map quoted, got {body}"
    );

    // And a map INSIDE the bound is served, so the clause above is about the broken bound
    // and not about every map.
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "PrincipalTags": { "department": "custom:department" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a principal-tag map inside the bound\": expected HTTP 200, got {status} with \
         {body}"
    );
}

/// An IAM role ARN of this emulator's account, so a role configuration is refused for the
/// thing a case is about rather than for its role.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// RULE: a constraint broken by a collection's ELEMENTS is stated once at the collection
/// member — quoting the whole collection the request sent, and listing every requirement
/// the element shape declares — rather than once per offending element.
#[tokio::test]
async fn a_constraint_an_element_broke_is_stated_at_the_collection() {
    let emulator = Emulator::new();

    // The requirements `IdentityId` declares, in the order a clause states them
    // (`smithy_query(query="shape",shape="IdentityIdList")`: member `length 1..55`,
    // `pattern ^[\w-]+:[0-9a-f-]+$`, quoted without the anchors that make the match total).
    let identity_id_requirements = "[Member must have length less than or equal to 55, Member \
                                    must have length greater than or equal to 1, Member must \
                                    satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+]";

    // The rows differ along how many elements the batch carries and how many of them are
    // outside the element shape — which is what the rule generalises over: the clause names
    // the member once however many elements broke it.
    let well_formed = identity_id(DEFAULT_REGION, 7);
    let rows: Vec<(&str, Vec<String>)> = vec![
        (
            "one element outside the element shape",
            vec!["not-a-well-formed-identity-id".to_string()],
        ),
        (
            "a well-formed id beside a malformed one",
            vec![well_formed.clone(), "nope".to_string()],
        ),
        (
            "two malformed ids",
            vec!["nope".to_string(), "also nope".to_string()],
        ),
        (
            "an element that is empty, so it breaks the length too",
            vec![String::new()],
        ),
    ];

    for (case, ids) in rows {
        let quoted = format!("[{}]", ids.join(", "));
        let (status, body) = emulator
            .call("DeleteIdentities", &json!({ "IdentityIdsToDelete": ids }))
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
                "1 validation error detected: Value '{quoted}' at 'identityIdsToDelete' failed \
                 to satisfy constraint: Member must satisfy constraint: \
                 {identity_id_requirements}"
            ),
            "case {case:?}: expected one clause at the collection, got {body}"
        );
    }
}

/// RULE: a map KEY outside the constraints its key shape declares is the protocol's
/// `ValidationException`, quoting the map and listing what a key of it must satisfy.
#[tokio::test]
async fn a_map_key_outside_its_shape_is_refused_with_the_constraints_a_key_must_satisfy() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .pool_with_providers("constraint-refusal-map-keys", true, &["graph.facebook.com"])
        .await;
    let authenticated = role_arn("Cognito_constraintRefusalAuth_Role");

    // `RolesMap`'s key shape is `RoleType` (`length 1..128`,
    // `pattern ^(un)?authenticated$`), so the clause lists both bounds and the expression.
    // The rows differ along the FORM of the offending key — a word the pattern does not
    // admit, a key that only contains an admitted one, one differing in case, and the empty
    // string, which breaks the minimum length as well — and along how many entries the map
    // carries, which is what the quoted value spans.
    let roles_requirements = "[Member must have length less than or equal to 128, Member must \
                              have length greater than or equal to 1, Member must satisfy \
                              regular expression pattern: (un)?authenticated]";
    let rows: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "a key the pattern does not admit",
            vec![("invalid-key", authenticated.clone())],
        ),
        (
            "a key containing an admitted one",
            vec![("preauthenticated", authenticated.clone())],
        ),
        (
            "a key differing only in case",
            vec![("Authenticated", authenticated.clone())],
        ),
        ("an empty key", vec![("", authenticated.clone())]),
        (
            "one admitted key beside a bad one",
            vec![
                ("authenticated", authenticated.clone()),
                ("sysadmin", role_arn("Cognito_constraintRefusalOther_Role")),
            ],
        ),
    ];

    for (case, entries) in rows {
        let quoted = {
            let mut rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            rendered.sort_unstable();
            format!("{{{}}}", rendered.join(", "))
        };
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": Value::Object(
                        entries
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
                            .collect(),
                    ),
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
                "1 validation error detected: Value '{quoted}' at 'roles' failed to satisfy \
                 constraint: Map keys must satisfy constraint: {roles_requirements}"
            ),
            "case {case:?}: expected the map quoted with the key's requirements, got {body}"
        );
    }

    // The same rule on the other map this service applies a key shape to: `PrincipalTags`,
    // whose key shape `PrincipalTagID` declares `length 1..128` and NO pattern, so the
    // clause lists the two bounds and nothing else.
    let too_long = "t".repeat(129);
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "PrincipalTags": { &too_long: "sub" },
            }),
        )
        .await;
    let case = "a principal-tag key one character past its maximum";
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
            "1 validation error detected: Value '{{{too_long}=sub}}' at 'principalTags' failed \
             to satisfy constraint: Map keys must satisfy constraint: [Member must have length \
             less than or equal to 128, Member must have length greater than or equal to 1]"
        ),
        "case {case:?}: expected the map quoted with the key's requirements, got {body}"
    );

    // A key ON the bound is admitted, so the refusal above is about the bound being broken.
    let at_bound = "t".repeat(128);
    let (status, body) = emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "PrincipalTags": { &at_bound: "sub" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a principal-tag key at its maximum\": expected HTTP 200, got {status} with {body}"
    );
}

/// RULE: a refusal AWS words with a FIXED sentence carries that sentence alone — nothing of
/// the request's own values.
#[tokio::test]
async fn a_fixed_sentence_refusal_states_nothing_of_the_request() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("constraint-refusal-fixed-text", true).await;
    let arn = identity_pool_arn(&pool_id);

    // Mutating a tag in the namespace AWS reserves for its own tags, over the operations
    // that can do it (assigning one, removing one) and the forms a reserved key takes — the
    // prefix is a NAMESPACE, so its casing and what follows it do not change the refusal.
    let reserved_keys = [
        ("the prefix with a name after it", "aws:cloudformation-stack"),
        ("the prefix with nothing after it", RESERVED_TAG_KEY_PREFIX),
        ("the prefix upper-cased", "AWS:Reserved"),
        ("the prefix mixed-cased", "Aws:reserved"),
    ];
    for (case, reserved) in reserved_keys {
        let (status, body) = emulator
            .call(
                "TagResource",
                &json!({ "ResourceArn": &arn, "Tags": { reserved: "nope" } }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on TagResource: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER,
            "case {case:?} on TagResource: expected {INVALID_PARAMETER}, got {body}"
        );
        assert_eq!(
            message(&body, case),
            SYSTEM_TAG_MUTATION_MESSAGE,
            "case {case:?} on TagResource: expected the fixed sentence, got {body}"
        );

        let (status, body) = emulator
            .call(
                "UntagResource",
                &json!({ "ResourceArn": &arn, "TagKeys": [reserved] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on UntagResource: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            message(&body, case),
            SYSTEM_TAG_MUTATION_MESSAGE,
            "case {case:?} on UntagResource: expected the fixed sentence, got {body}"
        );
    }

    // And a `ResourceArn` that is no ARN of a resource of this service, over every
    // operation that takes one and the forms such a value takes: not an ARN at all, an ARN
    // of another service, an ARN of this service naming another resource type, and an ARN
    // of a pool in the wrong partition. The sentence names none of them.
    let malformed = [
        (
            "not an ARN at all",
            "not-a-valid-identity-pool-arn-at-all".to_string(),
        ),
        (
            "an ARN of another service",
            format!(
                "arn:aws:sns:{DEFAULT_REGION}:{}:a-topic-name",
                engine::emulator_account_id()
            ),
        ),
        (
            "an ARN of another resource type of this service",
            format!(
                "arn:aws:cognito-identity:{DEFAULT_REGION}:{}:identity/{pool_id}",
                engine::emulator_account_id()
            ),
        ),
        (
            "an identity-pool ARN with an empty id",
            format!(
                "arn:aws:cognito-identity:{DEFAULT_REGION}:{}:identitypool/",
                engine::emulator_account_id()
            ),
        ),
    ];
    for (case, resource_arn) in malformed {
        let calls: Vec<(&str, Value)> = vec![
            (
                "TagResource",
                json!({ "ResourceArn": &resource_arn, "Tags": { "Environment": "Test" } }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": &resource_arn, "TagKeys": ["Environment"] }),
            ),
            (
                "ListTagsForResource",
                json!({ "ResourceArn": &resource_arn }),
            ),
        ];
        for (operation, request) in calls {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER,
                "case {case:?} on {operation}: expected {INVALID_PARAMETER}, got {body}"
            );
            assert_eq!(
                message(&body, case),
                ARN_VALIDATION_FAILED_MESSAGE,
                "case {case:?} on {operation}: expected the fixed sentence, got {body}"
            );
        }
    }
}

/// RULE: a developer-authenticated call whose developer provider is not the one its
/// identity pool is configured with is refused with the one sentence this service states
/// for that — whichever way the request fails to name the pool's own provider.
#[tokio::test]
async fn a_developer_provider_a_pool_does_not_support_is_refused_with_one_sentence() {
    let emulator = Emulator::new();
    // One pool with no developer provider at all, and one configured with its own.
    let without = emulator.pool("constraint-refusal-no-developer", true).await;
    let with = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "constraint-refusal-developer",
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": "login.example.app",
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    // The rows differ along WHY the pool does not support the provider the call names: the
    // pool names none, the call names another developer domain, and the call names a public
    // provider instead. A call carrying NO login at all names no provider to support, which
    // is the other missing-input sentence (`developer_identity_resolution`).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a pool that names no developer provider",
            without.as_str(),
            json!({ "login.example.app": "a-backend-user" }),
        ),
        (
            "a login naming another developer domain",
            with.as_str(),
            json!({ "login.other.app": "a-backend-user" }),
        ),
        (
            "a login naming a public provider",
            with.as_str(),
            json!({ "graph.facebook.com": "a-facebook-access-token" }),
        ),
    ];

    for (case, pool_id, logins) in rows {
        let (status, body) = emulator
            .call(
                "GetOpenIdTokenForDeveloperIdentity",
                &json!({ "IdentityPoolId": pool_id, "Logins": logins }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {body}"
        );
        assert_eq!(
            message(&body, case),
            UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
            "case {case:?}: expected the fixed sentence, got {body}"
        );
    }

    // The pool's OWN developer provider is served, so the refusals above are about the
    // provider and not about the operation.
    let served = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": &with,
                "Logins": { "login.example.app": "a-backend-user" },
            }),
        )
        .await;
    assert!(
        served
            .get("IdentityId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "case \"the pool's own developer provider\": expected a minted IdentityId, got {served}"
    );
}
