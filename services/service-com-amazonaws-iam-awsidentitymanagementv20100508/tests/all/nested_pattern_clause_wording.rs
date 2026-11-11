// SPDX-License-Identifier: CC0-1.0

//! How a `@pattern` clause is worded depending on WHERE the refused value was reached, end
//! to end over the awsQuery wire.
//!
//! RULE under test: a `@pattern` clause about a value reached INSIDE one of an operation's
//! input collections keeps the framework's own clause — the value the request carried, and
//! the expression exactly as the model declares it — while a clause about a member of the
//! input ITSELF states neither: no value, and the expression as AWS publishes one for a
//! member, without the pair of anchors the model writes around a whole-value expression.
//! So the position a value arrived at, not the operation and not the member's name, decides
//! which of the two clauses answers it. Both are HTTP 400 under the one code this service
//! names an input refusal with.
//!
//! `tests/all/validation_error_clauses.rs` owns the restated form over the input's OWN
//! members (and `tests/all/range_refusal_wording.rs` the bound clauses), and
//! `tests/all/server_certificate_untag_input_bounds.rs` owns one operation's whole input
//! surface. What this suite pins is the SPLIT itself: the same published form, broken at a
//! member of the input and inside one of its collections, worded the two ways — so a
//! restatement cannot spread from the input's own members to the values nested under them.
//!
//! The table's rows differ along what the rule generalises over: which collection carries
//! the value (a list of strings, a list of structures), which member of a nested structure
//! it is (a tag's key, a tag's value), which position in the list it arrived at, which
//! published expression its shape declares, and which operation takes it. Every published
//! expression a row names comes from the model (`smithy_query(query="shape",
//! shape="tagKeyType,tagValueType,serviceNamespaceType,instanceProfileNameType,markerType")`).
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`). The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The ONE code this service answers every refusal of a request's input under, written out
/// rather than read from the implementation: what this suite pins is the code a client
/// branches on.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// `tagKeyType`'s `@pattern`, as the model declares it — anchored around the whole value.
const TAG_KEY_PATTERN_AS_MODELLED: &str = r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$";

/// `tagValueType`'s `@pattern`, as the model declares it. The same class as a key's, over
/// zero or more characters, because a stored tag value may be empty.
const TAG_VALUE_PATTERN_AS_MODELLED: &str = r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";

/// `serviceNamespaceType`'s `@pattern`, as the model declares it.
const SERVICE_NAMESPACE_PATTERN_AS_MODELLED: &str = r"^[\w-]*$";

/// `instanceProfileNameType`'s `@pattern` as AWS publishes it for a member: the model's
/// `^[\w+=,.@-]+$` without the whole-value anchor pair.
const ENTITY_NAME_PATTERN_AS_PUBLISHED: &str = r"[\w+=,.@-]+";

/// `markerType`'s `@pattern` as AWS publishes it for a member: the model's
/// `^[\u0020-\u00FF]+$` without the whole-value anchor pair.
const MARKER_PATTERN_AS_PUBLISHED: &str = r"[\u0020-\u00FF]+";

/// The requirement half of a `@pattern` clause, quoting `expression`.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The whole message for a value reached INSIDE one of the input's collections: the
/// framework's own clause, carrying the value and the model's expression as written.
fn clause_keeping_the_value(member: &str, value: &str, expression: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {}",
        pattern_requirement(expression)
    )
}

/// The whole message for a member of the input ITSELF: the member and the published
/// expression, and nothing the request carried.
fn clause_dropping_the_value(member: &str, published: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: {}",
        pattern_requirement(published)
    )
}

/// One row: the call to make, the whole message its refusal must carry, the value the
/// request carried, and whether the clause states that value.
struct ClauseCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    expected: String,
    rejected: String,
    /// The value was reached INSIDE one of the input's collections, so the clause keeps it.
    reached_inside_a_collection: bool,
}

/// The `TagKeys.member.N` parameters of an awsQuery tag-key list, after `leading`.
fn with_key_params(leading: Vec<(String, String)>, keys: &[&str]) -> Vec<(String, String)> {
    crate::role_tag_removal::with_key_params(
        leading,
        &keys.iter().map(|key| (*key).to_string()).collect::<Vec<_>>(),
    )
}

/// The `Tags.member.N.Key` / `.Value` parameters of an awsQuery tag list, after `leading`.
fn with_tag_params(
    leading: Vec<(String, String)>,
    list: &[(&str, &str)],
) -> Vec<(String, String)> {
    crate::role_tags::with_tag_params(leading, &crate::role_tags::tags(list))
}

/// An ARN inside `arnType`'s published length, for the rows that have to carry one.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/a-role-not-held", account_id())
}

/// The rows: the same two clauses over every nested form this model declares a `@pattern`
/// on, and over two members of an operation's own input.
fn clause_cases() -> Vec<ClauseCase> {
    vec![
        // A string list: one key of `tagKeys`, at the first position.
        ClauseCase {
            case: "a tag key outside its alphabet, first in the key list",
            action: "UntagUser",
            params: with_key_params(vec![param("UserName", "a-user")], &["no*stars"]),
            expected: clause_keeping_the_value(
                "tagKeys.1.member",
                "no*stars",
                TAG_KEY_PATTERN_AS_MODELLED,
            ),
            rejected: "no*stars".to_string(),
            reached_inside_a_collection: true,
        },
        // The same list on another operation, with the offending key further down it: the
        // clause names the position it arrived at.
        ClauseCase {
            case: "a tag key outside its alphabet, second in the key list",
            action: "UntagRole",
            params: with_key_params(
                vec![param("RoleName", "a-role")],
                &["team", "no%percent"],
            ),
            expected: clause_keeping_the_value(
                "tagKeys.2.member",
                "no%percent",
                TAG_KEY_PATTERN_AS_MODELLED,
            ),
            rejected: "no%percent".to_string(),
            reached_inside_a_collection: true,
        },
        ClauseCase {
            case: "a tag key outside its alphabet, third in the key list",
            action: "UntagInstanceProfile",
            params: with_key_params(
                vec![param("InstanceProfileName", "a-profile")],
                &["team", "env", "no#hashes"],
            ),
            expected: clause_keeping_the_value(
                "tagKeys.3.member",
                "no#hashes",
                TAG_KEY_PATTERN_AS_MODELLED,
            ),
            rejected: "no#hashes".to_string(),
            reached_inside_a_collection: true,
        },
        // A list of STRUCTURES: the key of a tag the request nested under `tags`.
        ClauseCase {
            case: "a nested tag's key outside its alphabet",
            action: "TagRole",
            params: with_tag_params(
                vec![param("RoleName", "a-role")],
                &[("no*stars", "storage")],
            ),
            expected: clause_keeping_the_value(
                "tags.1.member.key",
                "no*stars",
                TAG_KEY_PATTERN_AS_MODELLED,
            ),
            rejected: "no*stars".to_string(),
            reached_inside_a_collection: true,
        },
        // The OTHER member of that structure, which publishes a different expression, at
        // the second position.
        ClauseCase {
            case: "a nested tag's value outside its alphabet",
            action: "TagUser",
            params: with_tag_params(
                vec![param("UserName", "a-user")],
                &[("team", "storage"), ("env", "no%percent")],
            ),
            expected: clause_keeping_the_value(
                "tags.2.member.value",
                "no%percent",
                TAG_VALUE_PATTERN_AS_MODELLED,
            ),
            rejected: "no%percent".to_string(),
            reached_inside_a_collection: true,
        },
        // A string list of another shape entirely, so the rule is not about tagging.
        ClauseCase {
            case: "a service namespace outside its alphabet",
            action: "ListPoliciesGrantingServiceAccess",
            params: vec![
                param("Arn", &role_arn()),
                param("ServiceNamespaces.member.1", "s3 storage"),
            ],
            expected: clause_keeping_the_value(
                "serviceNamespaces.1.member",
                "s3 storage",
                SERVICE_NAMESPACE_PATTERN_AS_MODELLED,
            ),
            rejected: "s3 storage".to_string(),
            reached_inside_a_collection: true,
        },
        // A member of the input ITSELF: the value is dropped and the expression published
        // without its anchors.
        ClauseCase {
            case: "an instance profile name outside its alphabet",
            action: "GetInstanceProfile",
            params: vec![param("InstanceProfileName", "has space")],
            expected: clause_dropping_the_value(
                "instanceProfileName",
                ENTITY_NAME_PATTERN_AS_PUBLISHED,
            ),
            rejected: "has space".to_string(),
            reached_inside_a_collection: false,
        },
        // Another member of an input, on an operation whose OTHER members are inside their
        // forms, so one request can be refused either way depending only on which member
        // carried the bad value.
        ClauseCase {
            case: "a pagination marker outside its published range",
            action: "ListPoliciesGrantingServiceAccess",
            params: vec![
                param("Arn", &role_arn()),
                param("ServiceNamespaces.member.1", "s3"),
                param("Marker", "page\u{20AC}"),
            ],
            expected: clause_dropping_the_value("marker", MARKER_PATTERN_AS_PUBLISHED),
            rejected: "page\u{20AC}".to_string(),
            reached_inside_a_collection: false,
        },
    ]
}

/// RULE: the position the value arrived at decides the clause — a value reached inside one
/// of the input's collections keeps the framework's clause, with the value and the model's
/// expression as written, and a member of the input itself states neither.
#[tokio::test]
async fn where_the_value_was_reached_decides_how_its_pattern_clause_is_worded() {
    let router = router();
    for ClauseCase {
        case,
        action,
        params,
        expected,
        rejected,
        reached_inside_a_collection,
    } in clause_cases()
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a value outside its published pattern, \
             got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, VALUE_REFUSAL_CODE,
            "case {case:?}: expected the code {VALUE_REFUSAL_CODE:?}, got {code:?}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        if reached_inside_a_collection {
            assert!(
                message.contains(&rejected),
                "case {case:?}: a value reached inside a collection is stated, so the \
                 clause must carry {rejected:?}, got {message:?}"
            );
            assert!(
                message.contains('^') && message.contains('$'),
                "case {case:?}: the expression is stated as the model writes it, anchors \
                 included, got {message:?}"
            );
        } else {
            assert!(
                !body.contains(&rejected),
                "case {case:?}: a member of the input itself states nothing of its value, \
                 so the refusal must not carry {rejected:?}, got {body}"
            );
            assert!(
                !message.contains('^') && !message.contains('$'),
                "case {case:?}: the published expression carries no whole-value anchors, \
                 got {message:?}"
            );
        }
    }
}

/// RULE (control): neither clause is a blanket refusal of the nested member, so a value
/// INSIDE its published alphabet is served — a tag whose key and value span the published
/// class is stored by the write and removed by the untagging write, read back through
/// `GetUser`.
#[tokio::test]
async fn a_nested_value_inside_its_published_alphabet_is_served() {
    let router = router();
    // The published class's own characters: letters, a separator, digits and `_.:/=+-@`,
    // and the empty value `tagValueType`'s `*` admits.
    let spanning_key = "Cost Center_1.2:3/4=5+6-7@8";
    let spanning_value = "Team Alpha_1.2:3/4=5+6-7@8";
    let (status, body) = call(
        &router,
        "CreateUser",
        &[
            ("UserName", "nested-forms"),
            ("Tags.member.1.Key", spanning_key),
            ("Tags.member.1.Value", spanning_value),
            ("Tags.member.2.Key", "empty-value"),
            ("Tags.member.2.Value", ""),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a tag spanning the published class must be stored, got {status}: {body}"
    );

    let (status, body) = call(&router, "GetUser", &[("UserName", "nested-forms")]).await;
    assert_eq!(
        status, 200,
        "expected the created user to be read back, got {status}: {body}"
    );
    assert_eq!(
        elements(&body, "Key"),
        vec![spanning_key.to_string(), "empty-value".to_string()],
        "expected both stored keys to be reported, got {body}"
    );

    // The same key, named by an untagging write, removes exactly that tag.
    let params = with_key_params(
        vec![param("UserName", "nested-forms")],
        &[spanning_key, "empty-value"],
    );
    let (status, body) = call(&router, "UntagUser", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "a key spanning the published class must be served, got {status}: {body}"
    );
    let (status, body) = call(&router, "GetUser", &[("UserName", "nested-forms")]).await;
    assert_eq!(
        status, 200,
        "expected the user to be read back after the removal, got {status}: {body}"
    );
    assert!(
        elements(&body, "Key").is_empty(),
        "expected every named tag to have been removed, got {body}"
    );
}

/// RULE (control): the clause is about the INPUT, so a request whose nested values are all
/// inside their published forms reaches the handler — a role the account does not hold is
/// reported absent rather than refused.
#[tokio::test]
async fn a_request_whose_nested_values_are_inside_their_forms_reaches_the_handler() {
    let router = router();
    let params = with_tag_params(
        vec![param("RoleName", "a-role-not-held")],
        &[("team", "storage"), ("Cost Center", "")],
    );
    let (status, body) = call(&router, "TagRole", &borrowed(&params)).await;
    assert_eq!(
        status, 404,
        "expected the absent role to be reported, got {status}: {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "NoSuchEntity",
        "expected the modelled absence code, got {code:?}: {body}"
    );
}
