// SPDX-License-Identifier: CC0-1.0

//! How IAM states a refusal of a request's INPUT, end to end over the awsQuery wire.
//!
//! The rules under test, each over a FAMILY of operations and members rather than over one
//! call:
//!
//! 1. every input refusal — a value outside a declared bound, outside a member's
//!    `@pattern`, outside an enumerated set, and a `@required` member the request did not
//!    carry — is answered HTTP 400 under ONE code, `ValidationError`. The framework tells
//!    the layer that found the fault apart (a body it could not read at all carries the
//!    other query code), AWS does not: a client branching on the code sees one code for
//!    every way its input was refused;
//! 2. a clause the model-level walk states names the member and the requirement and echoes
//!    NOTHING the request carried, and a `@pattern` requirement quotes the expression the
//!    way AWS publishes one for a member — without the pair of anchors the model writes
//!    around a whole-value expression;
//! 3. an absent member's clause states no value for a SCALAR member and the `null` the
//!    framework writes for a member the model types as a COLLECTION, which has no scalar
//!    rendering;
//! 4. the members IAM takes ITSELF are still answered with AWS's one fixed sentence for
//!    that member's whole form — one sentence whichever of the member's constraints the
//!    value broke, and with no envelope and no clause beside it. Which members those are is
//!    per member and not per published form: the character class is declared by every
//!    entity-name shape in the model, and `instanceProfileName` is recorded refused by the
//!    walk where `roleName` and `userName` are recorded refused with the sentence.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: which
//! member carries the value and which operation takes it, which constraint class the value
//! broke and HOW (an absent member, a character outside the class, a value past the
//! published length), whether the member is a scalar or a collection, and which published
//! expression the member's shape declares. No single request is the specification, and the
//! control rows show none of these refusals is a blanket refusal of the member.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// The ONE code IAM answers every refusal of a request's input under. Written out rather
/// than read from the implementation: what these suites pin is the code a client branches
/// on.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// A trust policy with a single statement, which the role fixtures need.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An identity policy with a single statement, for the rows that have to carry one.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// AWS's sentence for a value outside the character class published for an entity NAME,
/// with the member it was supplied for in place of `{member}`.
///
/// Shared with the suite that drives the same sentence over the PATH-bearing names and the
/// rename members (`tests/all/entity_name_path_prefix_refusal.rs`), so the sentence is
/// written out once for the two tables that assert it.
pub(crate) fn invalid_name(member: &str) -> String {
    format!(
        "The specified value for {member} is invalid. It must contain only alphanumeric \
         characters and/or the following: +=,.@_-"
    )
}

/// The envelope AWS states one clause in.
pub(crate) fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a member that was absent, as a SCALAR member states it: the requirement
/// alone.
fn absent_scalar(member: &str) -> String {
    one_error(&format!(
        "Value at '{member}' failed to satisfy constraint: Member must not be null"
    ))
}

/// The clause for a member that was absent, as a COLLECTION member states it: the absence
/// itself, because a list and a map have no scalar rendering.
fn absent_collection(member: &str) -> String {
    one_error(&format!(
        "Value null at '{member}' failed to satisfy constraint: Member must not be null"
    ))
}

/// The clause for a value outside its member's `@pattern`: the member, the expression as
/// AWS publishes it, and nothing of the value.
pub(crate) fn outside_pattern(member: &str, published: &str) -> String {
    one_error(&format!(
        "Value at '{member}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {published}"
    ))
}

/// One row: the call to make, and the whole message its refusal must carry.
///
/// Shared with the suite that drives the same rule for a value nested inside one of an
/// input's collections (`tests/all/nested_member_clause_wording.rs`), so an input refusal is
/// asserted exactly one way across this crate.
pub(crate) struct RefusalCase {
    pub(crate) case: String,
    pub(crate) action: &'static str,
    pub(crate) params: Vec<(String, String)>,
    pub(crate) expected: String,
    /// The value the request carried that the refusal must not echo, when the row carries
    /// one at all (an absent-member row carries none).
    pub(crate) rejected: Option<String>,
}

/// One row of an ABSENT-member table: the case, the call, and the member whose absence the
/// clause must name.
type AbsentRow = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    &'static str,
);

/// One row of the `@pattern` table: the case, the call, the member the clause names, the
/// expression AWS publishes for it, and the value the refusal must not echo.
type PatternRow = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static str,
    String,
);

/// One row of the fixed-sentence table: the case, the call, the member the sentence names,
/// and the value it must not echo.
type SentenceRow = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    &'static str,
    String,
);

/// Assert `row` is refused 400 under the one input-refusal code, with exactly the message it
/// states and with nothing of the value it carried.
pub(crate) async fn assert_refused(router: &MultiServiceRouter, row: RefusalCase) {
    let RefusalCase {
        case,
        action,
        params,
        expected,
        rejected,
    } = row;
    let (status, body) = call(router, action, &borrowed(&params)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    let answered = error_code(&body);
    assert_eq!(
        answered, VALUE_REFUSAL_CODE,
        "case {case:?}: expected the code {VALUE_REFUSAL_CODE:?}, got {answered:?}: {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    if let Some(rejected) = rejected {
        assert!(
            !body.contains(&rejected),
            "case {case:?}: the refusal must not echo {rejected:?}, got {body}"
        );
    }
}

/// RULE 1 + 3a: a `@required` SCALAR member the request did not carry is refused 400
/// `ValidationError` — the same code a bad VALUE is refused under, not the code the
/// framework names a body it could not read with — and its clause states the member and the
/// requirement and no value at all.
#[tokio::test]
async fn an_absent_required_scalar_member_is_refused_under_the_one_input_code() {
    let router = router();
    let policy_arn = format!("arn:aws:iam::{}:policy/a-policy-not-held", account_id());
    // Rows: eight members across eight operations — a read, two deletes, two writes, a
    // create, a membership write and an update — each carrying every other required member
    // so the absent one is the only fault, and each targeting a different shape.
    let rows: Vec<AbsentRow> = vec![
        (
            "a role policy read with no policy name",
            "GetRolePolicy",
            vec![param("RoleName", "a-role")],
            "policyName",
        ),
        (
            "a role policy delete with no policy name",
            "DeleteRolePolicy",
            vec![param("RoleName", "a-role")],
            "policyName",
        ),
        (
            "a user policy read with no policy name",
            "GetUserPolicy",
            vec![param("UserName", "a-user")],
            "policyName",
        ),
        (
            "a role policy write with no document",
            "PutRolePolicy",
            vec![param("RoleName", "a-role"), param("PolicyName", "a-policy")],
            "policyDocument",
        ),
        (
            "a role create with no name",
            "CreateRole",
            vec![param("AssumeRolePolicyDocument", TRUST_POLICY)],
            "roleName",
        ),
        (
            "a group membership write with no group",
            "AddUserToGroup",
            vec![param("UserName", "a-user")],
            "groupName",
        ),
        (
            "a default-version write with no version",
            "SetDefaultPolicyVersion",
            vec![param("PolicyArn", &policy_arn)],
            "versionId",
        ),
        (
            "an access-key update with no status",
            "UpdateAccessKey",
            vec![
                param("UserName", "a-user"),
                param("AccessKeyId", "AKIAIOSFODNN7EXAMPLE"),
            ],
            "status",
        ),
    ];
    for (case, action, params, member) in rows {
        assert_refused(
            &router,
            RefusalCase {
                case: case.to_string(),
                action,
                params,
                expected: absent_scalar(member),
                rejected: None,
            },
        )
        .await;
    }
}

/// RULE 3b: a `@required` member the model types as a COLLECTION states its absence — the
/// `null` a list or a map has in place of a scalar rendering — under the same code.
#[tokio::test]
async fn an_absent_required_collection_member_states_its_absence() {
    let router = router();
    let role_arn = format!("arn:aws:iam::{}:role/a-role-not-held", account_id());
    let provider_arn = format!(
        "arn:aws:iam::{}:oidc-provider/example.com/not-held",
        account_id()
    );
    // Rows: every distinct required collection member this model declares — a list of
    // structures, three lists of strings, a list of thumbprints and the one required map —
    // across the operations that take them.
    let rows: Vec<AbsentRow> = vec![
        (
            "a principal simulation with no actions",
            "SimulatePrincipalPolicy",
            vec![param("PolicySourceArn", &role_arn)],
            "actionNames",
        ),
        (
            "a custom simulation with no actions",
            "SimulateCustomPolicy",
            vec![param("PolicyInputList.member.1", POLICY_DOCUMENT)],
            "actionNames",
        ),
        (
            "a custom simulation with no policies",
            "SimulateCustomPolicy",
            vec![param("ActionNames.member.1", "s3:GetObject")],
            "policyInputList",
        ),
        (
            "a context-key read with no policies",
            "GetContextKeysForCustomPolicy",
            vec![],
            "policyInputList",
        ),
        (
            "a role tagging write with no tags",
            "TagRole",
            vec![param("RoleName", "a-role")],
            "tags",
        ),
        (
            "a user tagging write with no tags",
            "TagUser",
            vec![param("UserName", "a-user")],
            "tags",
        ),
        (
            "a role untagging write with no keys",
            "UntagRole",
            vec![param("RoleName", "a-role")],
            "tagKeys",
        ),
        (
            "a service-access listing with no namespaces",
            "ListPoliciesGrantingServiceAccess",
            vec![param("Arn", &role_arn)],
            "serviceNamespaces",
        ),
        (
            "a thumbprint update with no thumbprints",
            "UpdateOpenIDConnectProviderThumbprint",
            vec![param("OpenIDConnectProviderArn", &provider_arn)],
            "thumbprintList",
        ),
        (
            "an account-property write with no properties",
            "PutAccountProperties",
            vec![],
            "properties",
        ),
    ];
    for (case, action, params, member) in rows {
        assert_refused(
            &router,
            RefusalCase {
                case: case.to_string(),
                action,
                params,
                expected: absent_collection(member),
                rejected: None,
            },
        )
        .await;
    }
}

/// RULE 2: a value outside the `@pattern` its member's shape declares — a pattern IAM
/// states no sentence of its own for — is refused with the walk's clause naming the member
/// and the expression AWS publishes for it, with the whole-value anchors dropped and
/// nothing of the value echoed.
#[tokio::test]
async fn a_value_outside_its_pattern_states_the_expression_without_its_anchors() {
    let router = router();
    // The published expressions are the models' own `@pattern`s with the whole-value pair
    // dropped: `instanceProfileNameType` is `^[\w+=,.@-]+$`, `accessKeyIdType` `^[\w]+$`,
    // `serialNumberType` `^[\w+=/:,.@-]+$` and `markerType` `^[\u0020-\u00FF]+$`
    // (`smithy_query(query="shape",
    // shape="instanceProfileNameType,accessKeyIdType,serialNumberType,markerType")`).
    let name_class = r"[\w+=,.@-]+";
    let profile_arn = format!(
        "arn:aws:iam::{}:instance-profile/a-profile",
        account_id()
    );
    // Rows: one member across five operations that take it — a read, a create, a delete, a
    // binding write and a tag listing — and three more members on three other operations,
    // with the value outside the published class a different way in each (a colon-bearing
    // ARN, a space, a slash, a colon, a character above the published range).
    let rows: Vec<PatternRow> = vec![
        (
            "an instance profile read addressed by ARN",
            "GetInstanceProfile",
            vec![param("InstanceProfileName", &profile_arn)],
            "instanceProfileName",
            name_class,
            profile_arn.clone(),
        ),
        (
            "an instance profile create with a space",
            "CreateInstanceProfile",
            vec![param("InstanceProfileName", "has space")],
            "instanceProfileName",
            name_class,
            "has space".to_string(),
        ),
        (
            "an instance profile delete with a slash",
            "DeleteInstanceProfile",
            vec![param("InstanceProfileName", "has/slash")],
            "instanceProfileName",
            name_class,
            "has/slash".to_string(),
        ),
        (
            "a role binding with a colon in the profile name",
            "AddRoleToInstanceProfile",
            vec![
                param("InstanceProfileName", "has:colon"),
                param("RoleName", "a-role"),
            ],
            "instanceProfileName",
            name_class,
            "has:colon".to_string(),
        ),
        (
            "an instance profile tag listing with a space",
            "ListInstanceProfileTags",
            vec![param("InstanceProfileName", "has space")],
            "instanceProfileName",
            name_class,
            "has space".to_string(),
        ),
        (
            "an access key id carrying slashes",
            "UpdateAccessKey",
            vec![
                param("UserName", "a-user"),
                param("AccessKeyId", "AKIA/NOT/VALID/ID"),
                param("Status", "Active"),
            ],
            "accessKeyId",
            r"[\w]+",
            "AKIA/NOT/VALID/ID".to_string(),
        ),
        (
            "an MFA serial number carrying a space",
            "DeactivateMFADevice",
            vec![
                param("UserName", "a-user"),
                param("SerialNumber", "has space 123"),
            ],
            "serialNumber",
            r"[\w+=/:,.@-]+",
            "has space 123".to_string(),
        ),
        (
            "a pagination marker above the published range",
            "ListRoles",
            vec![param("Marker", "page\u{20AC}")],
            "marker",
            r"[\u0020-\u00FF]+",
            "page\u{20AC}".to_string(),
        ),
    ];
    for (case, action, params, member, published, rejected) in rows {
        assert_refused(
            &router,
            RefusalCase {
                case: case.to_string(),
                action,
                params,
                expected: outside_pattern(member, published),
                rejected: Some(rejected),
            },
        )
        .await;
    }
}

/// RULE 2 (control): the clause states no anchor of the model's own expression, so a client
/// matching on the published form reads back what AWS publishes.
#[tokio::test]
async fn the_stated_expression_carries_no_whole_value_anchors() {
    let router = router();
    let (status, body) = call(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", "has space")],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status}: {body}");
    let message = element(&body, "Message");
    let published = r"Member must satisfy regular expression pattern: [\w+=,.@-]+";
    assert!(
        message.ends_with(published),
        "expected the clause to close with {published:?}, got {message:?}"
    );
    assert!(
        !message.contains(r"^[\w") && !message.contains(r"]+$"),
        "expected no whole-value anchors in the stated expression, got {message:?}"
    );
}

/// RULE 4: the members IAM takes ITSELF are answered with AWS's one fixed sentence for the
/// member — on every operation that carries the member, and whichever of the member's
/// constraints the value broke, so a name that is BOTH outside the class and past the
/// published length is one sentence and not two clauses.
#[tokio::test]
async fn a_self_taken_name_is_refused_with_awss_fixed_sentence() {
    let router = router();
    let role_arn = format!("arn:aws:iam::{}:role/a-role-not-held", account_id());
    let user_arn = format!("arn:aws:iam::{}:user/eng/a-user-not-held", account_id());
    // Rows: `roleName` on a read and two listings and `userName` on the read, each supplied
    // an ARN or a path-prefixed name — values outside the character class, and the ARN rows
    // also past `roleNameType`'s published maximum of 64 characters, which the one sentence
    // covers as well.
    let rows: Vec<SentenceRow> = vec![
        (
            "a role policy read addressed by role ARN",
            "GetRolePolicy",
            vec![
                param("RoleName", &role_arn),
                param("PolicyName", "a-policy"),
            ],
            "roleName",
            role_arn.clone(),
        ),
        (
            "a role policy listing addressed by a path-prefixed name",
            "ListRolePolicies",
            vec![param("RoleName", "/eng/a-role")],
            "roleName",
            "/eng/a-role".to_string(),
        ),
        (
            "an attached-policy listing addressed by role ARN",
            "ListAttachedRolePolicies",
            vec![param("RoleName", &role_arn)],
            "roleName",
            role_arn.clone(),
        ),
        (
            "a user read addressed by user ARN",
            "GetUser",
            vec![param("UserName", &user_arn)],
            "userName",
            user_arn.clone(),
        ),
    ];
    for (case, action, params, member, rejected) in rows {
        assert_refused(
            &router,
            RefusalCase {
                case: case.to_string(),
                action,
                params,
                expected: invalid_name(member),
                rejected: Some(rejected),
            },
        )
        .await;
    }

    // The sentence is the WHOLE answer: no envelope and no clause beside it, however many
    // of the member's constraints the value broke.
    let (_, body) = call(
        &router,
        "GetRolePolicy",
        &[("RoleName", role_arn.as_str()), ("PolicyName", "a-policy")],
    )
    .await;
    let message = element(&body, "Message");
    assert!(
        !message.contains("validation error") && !message.contains("constraint"),
        "expected the fixed sentence alone, got {message:?}"
    );
}

/// RULE 1 + 2 + 4 (control): each of these refusals is about the INPUT, so a request whose
/// members are all inside their published forms reaches the handler — a profile the account
/// holds is created and read back, and a role policy the account does not hold is reported
/// absent rather than refused.
#[tokio::test]
async fn a_request_inside_every_published_form_reaches_the_handler() {
    let router = router();
    // Rows: names at the edges of the published class — the punctuation the class admits,
    // and a name at `instanceProfileNameType`'s published maximum length of 128.
    for name in [
        "a-profile".to_string(),
        "Profile+Name=1,2.3@x_-".to_string(),
        "a".repeat(128),
    ] {
        let (status, body) = call(
            &router,
            "CreateInstanceProfile",
            &[("InstanceProfileName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status}: {body}"
        );
        // Round trip: the create is read back through the implemented read.
        let (status, body) = call(
            &router,
            "GetInstanceProfile",
            &[("InstanceProfileName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the profile to be read back, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "InstanceProfileName"),
            name,
            "case {name:?}: expected the stored name to be reported, got {body}"
        );
    }

    // A request carrying every required member, each inside its form, is answered by the
    // handler: the role policy the account does not hold is reported absent (404), not
    // refused (400).
    let (status, body) = call(
        &router,
        "GetRolePolicy",
        &[("RoleName", "a-role"), ("PolicyName", "a-policy")],
    )
    .await;
    assert_eq!(
        status, 404,
        "expected the absent role policy to be reported, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected the modelled absence code, got {body}"
    );
}
