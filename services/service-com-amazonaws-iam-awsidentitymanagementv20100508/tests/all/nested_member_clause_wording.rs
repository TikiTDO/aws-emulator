// SPDX-License-Identifier: CC0-1.0

//! How IAM states a refusal of a value that arrived INSIDE one of an operation's
//! collections, end to end over the awsQuery wire.
//!
//! The rule under test, per constraint class, as `src/service/refusal_clauses.rs` states it:
//!
//! 1. a value outside a declared `@length` is stated with the member and the requirement and
//!    NOTHING the request carried, wherever the value arrived — that restating reads the
//!    member out of the clause's path and never asks how deep it sat;
//! 2. a value outside a `@pattern` is restated only for a member of the input ITSELF (the
//!    value dropped, the expression quoted without the pair of anchors the model writes
//!    around a whole-value one); a value reached inside one of the input's collections keeps
//!    the framework's own clause instead — the value it carried and the expression as the
//!    model writes it;
//! 3. either way the member path is the framework's, naming the position the value arrived
//!    at (`tagKeys.1.member` is one key of a list, `tags.2.member.value` one value of a list
//!    of structures), and the refusal is HTTP 400 under the one code this service answers a
//!    request value it cannot accept with.
//!
//! `tests/all/validation_error_clauses.rs` with `tests/all/range_refusal_wording.rs` owns
//! those rules for the members of an input itself, and
//! `tests/all/server_certificate_untag_input_bounds.rs` pins one key of one list; this suite
//! is the NESTED axis across the whole class, and its second test is the CONTRAST the two
//! halves of rule 2 turn on: a character-class violation stated one way where the value was
//! supplied for a member of the input itself and another where it was reached inside a
//! collection. The envelope, the code and the non-echo are asserted with the `RefusalCase`
//! table `tests/all/validation_error_clauses.rs` owns, so an input refusal is read exactly
//! one way across this crate.
//!
//! The rows differ along what the rules generalise over: which collection carries the value
//! (a list of strings, a list of structures), which of the structure's members carries it,
//! which operation takes the collection, which position in the list the value arrived at,
//! which constraint class it broke (a `@pattern`, a `@length` maximum, a `@length` minimum)
//! and which published bound or expression the member's shape declares. The bounds come
//! from the model (`smithy_query(query="shape",
//! shape="tagKeyType,tagValueType,serviceNamespaceType,thumbprintType,ActionNameType")`):
//! `tagKeyType` is length 1..128 with pattern `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`,
//! `tagValueType` length 0..256, `serviceNamespaceType` length 1..64 with pattern
//! `^[\w-]*$`, `thumbprintType` length 40..40 and `ActionNameType` length 3..128.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery code, `Message`) rather than a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, router};
use crate::validation_error_clauses::{assert_refused, one_error, outside_pattern, RefusalCase};

/// An identity policy with a single statement, for the simulation rows that must carry one.
const POLICY_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// `tagKeyType`'s `@pattern`, as the model writes it — the form a clause about a value
/// reached inside a collection quotes.
const MODELLED_TAG_KEY_CLASS: &str = r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$";

/// `serviceNamespaceType`'s `@pattern`, as the model writes it.
const MODELLED_NAMESPACE_CLASS: &str = r"^[\w-]*$";

/// The expression a clause about a member of the input ITSELF quotes for `accessKeyIdType` —
/// the model's `^[\w]+$` without the pair of anchors it writes around a whole-value one.
const PUBLISHED_ACCESS_KEY_CLASS: &str = r"[\w]+";

/// The clause for a value reached inside one of the input's collections that was outside its
/// member's `@pattern`: the value it carried, the member path it arrived at, and the model's
/// expression as written.
fn nested_outside_pattern(member: &str, value: &str, modelled: &str) -> String {
    one_error(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {modelled}"
    ))
}

/// The clause for a value past a declared `@length` maximum.
fn above_length(member: &str, max: usize) -> String {
    one_error(&format!(
        "Value at '{member}' failed to satisfy constraint: Member must have length less \
         than or equal to {max}"
    ))
}

/// The clause for a value under a declared `@length` minimum.
fn below_length(member: &str, min: usize) -> String {
    one_error(&format!(
        "Value at '{member}' failed to satisfy constraint: Member must have length greater \
         than or equal to {min}"
    ))
}

/// One row: the case, the call to make, the whole message its refusal must carry, and the
/// value the refusal must NOT echo — `None` for a row whose clause states the value, which
/// the message assertion itself then pins.
struct NestedRow {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    expected: String,
    unechoed: Option<String>,
}

/// The `Tags.member.N.Key`/`.Value` parameters for a tag list given as literals, so a row
/// can name an offending key or value at any position with every other pair valid.
fn tag_params(leading: Vec<(String, String)>, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = leading;
    for (index, (key, value)) in pairs.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    params
}

/// The `TagKeys.member.N` parameters for a key list given as literals.
fn key_params(leading: Vec<(String, String)>, keys: &[&str]) -> Vec<(String, String)> {
    let mut params = leading;
    for (index, key) in keys.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("TagKeys.member.{position}"), key));
    }
    params
}

/// The rows: one broken requirement per request, spanning the collections, the member
/// positions, the operations and the constraint classes.
fn rows() -> Vec<NestedRow> {
    let role_arn = format!("arn:aws:iam::{}:role/a-role-not-held", account_id());
    let provider_arn = format!(
        "arn:aws:iam::{}:oidc-provider/example.com/not-held",
        account_id()
    );
    // A thumbprint is exactly 40 hex characters, so one of 39 is under the published
    // minimum and one of 41 is past the published maximum.
    let short_thumbprint = "a".repeat(39);
    let long_thumbprint = "a".repeat(41);
    let over_long_key = "k".repeat(129);
    let over_long_value = "v".repeat(257);

    vec![
        // A list of strings: one key of an untagging write, at the first position.
        NestedRow {
            case: "a role untag key outside the published alphabet",
            action: "UntagRole",
            params: key_params(vec![param("RoleName", "a-role")], &["beta?build"]),
            expected: nested_outside_pattern(
                "tagKeys.1.member",
                "beta?build",
                MODELLED_TAG_KEY_CLASS,
            ),
            unechoed: None,
        },
        // The same class on a different operation, and at a LATER position, with every
        // earlier key inside the published form.
        NestedRow {
            case: "a user untag key outside the published alphabet, second in the list",
            action: "UntagUser",
            params: key_params(
                vec![param("UserName", "a-user")],
                &["Cost Center", "ops|team"],
            ),
            expected: nested_outside_pattern(
                "tagKeys.2.member",
                "ops|team",
                MODELLED_TAG_KEY_CLASS,
            ),
            unechoed: None,
        },
        // The other constraint class on the same member, at a third position: a bound, whose
        // clause states no value however deep the value sat.
        NestedRow {
            case: "an untag key past the published length, third in the list",
            action: "UntagInstanceProfile",
            params: key_params(
                vec![param("InstanceProfileName", "a-profile")],
                &["key-00", "key-01", over_long_key.as_str()],
            ),
            expected: above_length("tagKeys.3.member", 128),
            unechoed: Some(over_long_key.clone()),
        },
        // A list of STRUCTURES: the key of one tag of a tagging write.
        NestedRow {
            case: "a role tag key outside the published alphabet",
            action: "TagRole",
            params: tag_params(
                vec![param("RoleName", "a-role")],
                &[("team~alpha", "a-value")],
            ),
            expected: nested_outside_pattern(
                "tags.1.member.key",
                "team~alpha",
                MODELLED_TAG_KEY_CLASS,
            ),
            unechoed: None,
        },
        // The OTHER member of that structure, at a later position: a value, not a key, and a
        // bound rather than an expression.
        NestedRow {
            case: "a user tag value past the published length, second in the list",
            action: "TagUser",
            params: tag_params(
                vec![param("UserName", "a-user")],
                &[
                    ("Cost Center", "a-value"),
                    ("Department", over_long_value.as_str()),
                ],
            ),
            expected: above_length("tags.2.member.value", 256),
            unechoed: Some(over_long_value.clone()),
        },
        // The key of that same structure against its own published length.
        NestedRow {
            case: "a policy tag key past the published length",
            action: "TagPolicy",
            params: tag_params(
                vec![param(
                    "PolicyArn",
                    &format!("arn:aws:iam::{}:policy/a-policy-not-held", account_id()),
                )],
                &[(over_long_key.as_str(), "a-value")],
            ),
            expected: above_length("tags.1.member.key", 128),
            unechoed: Some(over_long_key),
        },
        // A list of strings on a READ: one namespace of a service-access listing, against
        // its own published expression.
        NestedRow {
            case: "a service namespace outside the published alphabet",
            action: "ListPoliciesGrantingServiceAccess",
            params: vec![
                param("Arn", &role_arn),
                param("ServiceNamespaces.member.1", "not a namespace"),
            ],
            expected: nested_outside_pattern(
                "serviceNamespaces.1.member",
                "not a namespace",
                MODELLED_NAMESPACE_CLASS,
            ),
            unechoed: None,
        },
        // Both ends of a fixed-width bound, on a list whose member declares the same number
        // as its minimum and its maximum.
        NestedRow {
            case: "a thumbprint under the published length",
            action: "UpdateOpenIDConnectProviderThumbprint",
            params: vec![
                param("OpenIDConnectProviderArn", &provider_arn),
                param("ThumbprintList.member.1", &short_thumbprint),
            ],
            expected: below_length("thumbprintList.1.member", 40),
            unechoed: Some(short_thumbprint),
        },
        NestedRow {
            case: "a thumbprint past the published length, second in the list",
            action: "UpdateOpenIDConnectProviderThumbprint",
            params: vec![
                param("OpenIDConnectProviderArn", &provider_arn),
                param("ThumbprintList.member.1", &"b".repeat(40)),
                param("ThumbprintList.member.2", &long_thumbprint),
            ],
            expected: above_length("thumbprintList.2.member", 40),
            unechoed: Some(long_thumbprint),
        },
        // A list of strings on a simulation, under its member's published minimum. The
        // name is one no policy document in this suite mentions, so the non-echo assertion
        // is about the clause and not about a substring of the policy.
        NestedRow {
            case: "a simulated action name under the published length",
            action: "SimulateCustomPolicy",
            params: vec![
                param("PolicyInputList.member.1", POLICY_DOCUMENT),
                param("ActionNames.member.1", "qz"),
            ],
            expected: below_length("actionNames.1.member", 3),
            unechoed: Some("qz".to_string()),
        },
    ]
}

/// RULE: a value reached inside one of an input's collections is refused under the one input
/// code with the clause its constraint class states — a bound's clause naming the member and
/// the requirement alone, an expression's clause naming the value it carried and the model's
/// expression — with the member path naming the position the value arrived at.
#[tokio::test]
async fn a_nested_value_is_refused_with_the_clause_its_class_states() {
    let router = router();
    for NestedRow {
        case,
        action,
        params,
        expected,
        unechoed,
    } in rows()
    {
        assert_refused(
            &router,
            RefusalCase {
                case: case.to_string(),
                action,
                params,
                expected,
                rejected: unechoed,
            },
        )
        .await;
    }
}

/// RULE (the contrast): the restating of a `@pattern` clause is for a member of the input
/// ITSELF, so a character-class violation is stated two ways — without the value and without
/// the model's whole-value anchors where the value was supplied for a member of the input,
/// and as the framework states it where the value was reached inside a collection.
#[tokio::test]
async fn the_restating_separates_an_inputs_own_member_from_a_nested_value() {
    let router = router();

    // Reached INSIDE a collection: the value it carried, and the model's expression as
    // written.
    let nested = key_params(vec![param("RoleName", "a-role")], &["beta?build"]);
    let (status, body) = call(&router, "UntagRole", &borrowed(&nested)).await;
    assert_eq!(
        status, 400,
        "a key outside the published alphabet must be refused, got {status}: {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        nested_outside_pattern("tagKeys.1.member", "beta?build", MODELLED_TAG_KEY_CLASS),
        "expected a nested clause to name the value at its position and quote the model's \
         expression, got {message:?}"
    );

    // Supplied for a member of the input ITSELF: no value, and the expression without the
    // pair of anchors the model writes around it. `accessKeyIdType` is `^[\w]+$`
    // (`smithy_query(query="shape", shape="accessKeyIdType")`) with length 16..128, so an id
    // of that length carrying a slash is outside the expression alone. The clause itself is
    // the one `tests/all/validation_error_clauses.rs` composes, so it is spelled in one
    // place.
    let (status, body) = call(
        &router,
        "UpdateAccessKey",
        &[
            ("UserName", "a-user"),
            ("AccessKeyId", "AKIA/NOT/VALID/ID"),
            ("Status", "Active"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "an access key id outside its pattern must be refused, got {status}: {body}"
    );
    let message = element(&body, "Message");
    let expected = outside_pattern("accessKeyId", PUBLISHED_ACCESS_KEY_CLASS);
    assert_eq!(
        message, expected,
        "expected the input's own member to be stated without its value and without the \
         anchors, got {message:?}"
    );
    assert!(
        !body.contains("AKIA/NOT/VALID/ID"),
        "expected the clause for an input's own member to echo nothing of the value, got \
         {body}"
    );
}
