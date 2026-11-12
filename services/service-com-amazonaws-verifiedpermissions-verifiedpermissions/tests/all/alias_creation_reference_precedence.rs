// SPDX-License-Identifier: CC0-1.0

//! Which refusal `CreatePolicyStoreAlias` answers for a request that is faulty in
//! TWO ways at once: its `policyStoreId` references a policy store this service
//! does not hold AND its `aliasName` is a value no alias could ever carry.
//!
//! The rule under test is the one
//! [`crate::not_found_precedes_payload_refusal`] pins for the authorization calls
//! and the identity-source writers, and
//! [`crate::list_cursor_refusal_follows_store_lookup`] for the store-scoped
//! listings: an operation resolves every resource the request REFERENCES before it
//! reads the request's own payload, so a reference that names nothing answers the
//! 404 `ResourceNotFoundException` whatever else the request got wrong. The store
//! an alias is associated with is the resource this creation references; the alias
//! name is its own payload, so the name's documented-prefix refusal comes after.
//! Against the store that DOES exist the very same name still earns its 400
//! `ValidationException` naming `/aliasName`, so what is pinned here is the
//! PRECEDENCE and not a blanket 404.
//!
//! `tests/all/alias_name_prefix.rs` pins WHAT the name refusal looks like and
//! `tests/all/alias_name_prefix_precedence.rs` that it does not depend on what
//! state holds for the name itself; this suite adds the one thing it does come
//! after.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so
//! each answer is the one a client would really receive, and what a refused
//! request did NOT write is visibly absent from the next request's answer.

use serde_json::json;
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{
    call, error_code, fault_message, field_list, ok, router, store_with_validation_off,
};

/// The prefix every alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s model documentation
/// (`smithy_query(query="shape", shape="CreatePolicyStoreAliasInput")`): "The
/// alias name must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The JSON pointer a refusal of that member names — where the member sits in the
/// request body, which is the form `ValidationExceptionField$path` carries.
const ALIAS_NAME_POINTER: &str = "/aliasName";

/// A policy store id no store in a fresh engine carries.
const UNKNOWN_STORE: &str = "PSdoesnotexist11111111";

/// The resource type this service reports for a policy store it cannot resolve.
const POLICY_STORE: &str = "POLICY_STORE";

/// One `aliasName` value a row hands the creation.
struct Name {
    /// What is being varied, for the case name.
    case: &'static str,
    /// The value itself.
    value: &'static str,
    /// Whether it carries the documented prefix, so the operation's own payload
    /// check admits it.
    well_formed: bool,
}

/// The `aliasName` values the rows span: the ways a name can miss the documented
/// prefix (absent, empty, the prefix without its separator, a look-alike prefix,
/// the prefix somewhere other than the start) and, as the control, one that
/// carries it.
const NAMES: &[Name] = &[
    Name {
        case: "no prefix at all",
        value: "photos",
        well_formed: false,
    },
    Name {
        case: "an empty name",
        value: "",
        well_formed: false,
    },
    Name {
        case: "the prefix without its separator",
        value: "policy-store-alias",
        well_formed: false,
    },
    Name {
        case: "a look-alike prefix",
        value: "policy-store-aliases/photos",
        well_formed: false,
    },
    Name {
        case: "the prefix not at the start",
        value: "x/policy-store-alias/photos",
        well_formed: false,
    },
    Name {
        case: "a well-formed name",
        value: "policy-store-alias/photos",
        well_formed: true,
    },
];

/// The value a row sends, made unique per store reference so a creation that is
/// expected to SUCCEED never collides with one another row already made.
fn alias_name_for(name: &Name, row: usize) -> String {
    if name.well_formed {
        format!("{}-{row}", name.value)
    } else {
        name.value.to_string()
    }
}

/// RULE: `CreatePolicyStoreAlias` resolves the policy store its `policyStoreId`
/// REFERENCES before it reads its own `aliasName`, so a reference that names no
/// store answers HTTP 404 `ResourceNotFoundException` reporting that reference —
/// whatever the alias name was — while the same name against the store that does
/// exist earns its HTTP 400 `ValidationException` naming `/aliasName`, and a
/// well-formed name there is created.
///
/// The rows differ along WHAT the `policyStoreId` names (an id no store carries, a
/// store that was deleted, an alias of a store that does exist — a value this
/// ID-only member never resolves — and the store that does exist) crossed with the
/// [`NAMES`] the `aliasName` carries. Every refusal is followed by a read, so a
/// refused request that had quietly created an alias would fail here.
#[tokio::test]
async fn an_unresolvable_policy_store_outranks_the_alias_name_refusal() {
    let router = router();
    let live_store = store_with_validation_off(&router).await;

    // A store that existed and no longer does: its id is as unresolvable as one
    // that was never minted.
    let deleted_store = store_with_validation_off(&router).await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": deleted_store }),
    )
    .await;

    // A live alias of the live store: `CreatePolicyStoreAliasInput$policyStoreId`
    // is documented "The associated policy store must be specified using its ID.
    // The alias name cannot be used", so this value names no store either.
    let live_alias = format!("{ALIAS_PREFIX}live");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": live_alias, "policyStoreId": live_store }),
    )
    .await;

    // (case, the `policyStoreId` sent, whether it resolves to a store)
    let references: [(&str, String, bool); 4] = [
        ("an id no store carries", UNKNOWN_STORE.to_string(), false),
        ("a store that was deleted", deleted_store, false),
        (
            "an alias of a store that does exist, which this member never resolves",
            live_alias,
            false,
        ),
        ("the store that does exist", live_store.clone(), true),
    ];

    for (row, (reference_case, reference, resolves)) in references.iter().enumerate() {
        // For an unresolvable reference the whole answer must be the same for
        // every name, so each row's answer is collected and compared.
        let mut answers: Vec<(String, (u16, String, String, String))> = Vec::new();

        for name in NAMES {
            let alias_name = alias_name_for(name, row);
            let case = format!("{reference_case}, {}", name.case);
            let (status, body) = call(
                &router,
                "CreatePolicyStoreAlias",
                &json!({ "aliasName": alias_name, "policyStoreId": reference }),
            )
            .await;

            if !*resolves {
                assert_eq!(
                    status, 404,
                    "case {case:?}: an unresolvable policyStoreId must answer HTTP 404, got \
                     {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["resourceType"].as_str(),
                    Some(POLICY_STORE),
                    "case {case:?}: expected resourceType {POLICY_STORE}, got {body}"
                );
                assert_eq!(
                    body["resourceId"].as_str(),
                    Some(reference.as_str()),
                    "case {case:?}: expected resourceId {reference}, got {body}"
                );
                answers.push((
                    case.clone(),
                    (
                        status,
                        error_code(&body),
                        body["resourceType"].as_str().unwrap_or_default().to_string(),
                        body["resourceId"].as_str().unwrap_or_default().to_string(),
                    ),
                ));
            } else if name.well_formed {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a well-formed creation against a live store must succeed, \
                     got {status} {body}"
                );
                assert_eq!(
                    body["aliasName"].as_str(),
                    Some(alias_name.as_str()),
                    "case {case:?}: expected the created alias to be reported, got {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a malformed aliasName against a live store must answer HTTP \
                     400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    fault_message(&body),
                    INVALID_INPUT_MESSAGE,
                    "case {case:?}: expected the fixed summary message, got {body}"
                );
                let fields = field_list(&body);
                assert_eq!(
                    fields.len(),
                    1,
                    "case {case:?}: expected exactly one offending member, got {fields:?}"
                );
                assert_eq!(
                    fields[0].0, ALIAS_NAME_POINTER,
                    "case {case:?}: expected the refusal to name {ALIAS_NAME_POINTER}, got \
                     {fields:?}"
                );
            }

            // A refused creation persisted nothing: the well-formed name it named
            // is still absent. (A malformed name is refused by the read too, so
            // only the well-formed rows can be read back.)
            if name.well_formed && !*resolves {
                let (read_status, read_body) = call(
                    &router,
                    "GetPolicyStoreAlias",
                    &json!({ "aliasName": alias_name }),
                )
                .await;
                assert_eq!(
                    (read_status, error_code(&read_body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: a refused creation must have created no alias, got \
                     {read_status} {read_body}"
                );
            }
        }

        if let Some((first_case, first)) = answers.first() {
            for (case, answer) in &answers[1..] {
                assert_eq!(
                    answer, first,
                    "case {case:?}: the answer for an unresolvable store must not depend on the \
                     alias name — {first_case:?} answered {first:?}, this answered {answer:?}"
                );
            }
        }
    }

    // The live alias the setup made, and the well-formed names the live store
    // really accepted, are the only aliases that exist: a refused row added none.
    let listed = ok(&router, "ListPolicyStoreAliases", &json!({})).await;
    let names: Vec<&str> = listed["policyStoreAliases"]
        .as_array()
        .map(|aliases| {
            aliases
                .iter()
                .filter_map(|alias| alias["aliasName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    let expected = vec![
        format!("{ALIAS_PREFIX}live"),
        format!("{ALIAS_PREFIX}photos-3"),
    ];
    assert_eq!(
        names,
        expected.iter().map(String::as_str).collect::<Vec<&str>>(),
        "case \"only the accepted creations exist\": expected {expected:?}, got {listed}"
    );
}
