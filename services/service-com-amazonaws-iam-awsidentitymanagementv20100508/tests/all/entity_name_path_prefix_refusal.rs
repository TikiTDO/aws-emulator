// SPDX-License-Identifier: CC0-1.0

//! How IAM refuses an entity NAME that carries a path where a plain name belongs, end to end
//! over the awsQuery wire.
//!
//! RULE under test: every input member this model targets at an entity-name shape publishes
//! the one character class `^[\w+=,.@-]+$` (`smithy_query(query="shape",
//! shape="roleNameType,userNameType,groupNameType,policyNameType,serverCertificateNameType")`),
//! which admits no `/`, so a value carrying a path — the form an entity's separate `Path`
//! member takes — is outside it. Such a value is refused HTTP 400 under the one code IAM
//! names an input refusal with, stated as AWS's single fixed sentence for that class: `The
//! specified value for <member> is invalid. It must contain only alphanumeric characters
//! and/or the following: +=,.@_-`. The sentence names the MEMBER the value was supplied for
//! and nothing else — no part of the request is interpolated, so the rejected value is not
//! echoed, and the model-level constraint walk's envelope and its raw expression are not
//! stated at all. The member's LENGTH is a separate requirement, so a name inside the class
//! but past the published maximum keeps that walk's own clause, and a name outside BOTH is
//! the sentence alone.
//!
//! What this suite adds to the crate's other refusal-wording tables
//! (`tests/all/validation_error_clauses.rs` owns the split between the sentence and the
//! walk's clause, `tests/all/net_new_refusal_wording.rs` the sentence over the creates,
//! `tests/all/range_refusal_wording.rs` the bound clauses) is the PATH-bearing name itself,
//! over the single-member role reads and writes and over the rename members no other suite
//! drives — the members whose name is not the entity's own (`newUserName`, `newGroupName`,
//! `newServerCertificateName`), which the rule reaches because the sentence is keyed by the
//! published class and names whichever member carried the value.
//!
//! The rows differ along what the rule generalises over: which operation takes the value (a
//! read, a delete, two updates, a tag listing, a policy write, a rename), which member
//! carries it and which name shape that member targets, and HOW the value leaves the class
//! (a service-prefixed name, a leading slash, a nested path, a trailing slash, a space, an
//! asterisk, a whole ARN). No single request is the specification; the control suites show
//! the sentence is not a blanket refusal of the member.
//!
//! An input refusal is asserted through the crate's shared harness
//! (`tests/all/validation_error_clauses.rs`'s `RefusalCase` + `assert_refused`), which also
//! owns the sentence this suite expects, so the status, the wire code, the whole message and
//! the no-echo rule are all checked exactly one way across this crate. Every assertion drives
//! a `MultiServiceRouter` holding this service over one shared `InMemoryEngine`, exactly as
//! the production router does. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares.

use crate::role_lifecycle::param;
use crate::user_lifecycle::{account_id, call, element, error_code, router};
use crate::validation_error_clauses::{assert_refused, invalid_name, RefusalCase};

/// A trust policy with a single statement, for the rows and controls that must carry one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An identity policy with a single statement, for the row that must carry one.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// One row before it is turned into a shared [`RefusalCase`]: the call to make, the member
/// the sentence must name, and the other members the request has to carry for that name to be
/// the only fault.
struct NameCase {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    value: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

/// The member's spelling on the awsQuery wire (UpperCamel), from the spelling a refusal names
/// it by (lowerCamel): the two are the same word, so a row states it once.
fn wire_member(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The rows: a path-bearing or otherwise out-of-class name on each operation and member the
/// rule reaches.
fn name_cases() -> &'static [NameCase] {
    &[
        // `roleName` on the READ the recorded refusal was taken from: the service-linked
        // form, where the path prefix carries no leading slash.
        NameCase {
            case: "a role read addressed by a service-prefixed name",
            action: "GetRole",
            member: "roleName",
            value: "service-role/AWSServiceRoleForSomeService",
            extra: &[],
        },
        // The same member on the same read, outside the class the other two ways a path
        // leaves it: an opening delimiter, and a nested path closed by one.
        NameCase {
            case: "a role read addressed by a name opening with the path delimiter",
            action: "GetRole",
            member: "roleName",
            value: "/a-role",
            extra: &[],
        },
        NameCase {
            case: "a role read addressed by a nested path and name",
            action: "GetRole",
            member: "roleName",
            value: "/eng/tools/a-role",
            extra: &[],
        },
        // A DELETE, so the rule is not about reporting operations.
        NameCase {
            case: "a role delete addressed by a name closing with the path delimiter",
            action: "DeleteRole",
            member: "roleName",
            value: "a-role/",
            extra: &[],
        },
        // An UPDATE, with the value outside the class by a character a path never carries,
        // so the rule is about the class and not about the delimiter.
        NameCase {
            case: "a role update addressed by a name carrying a space",
            action: "UpdateRole",
            member: "roleName",
            value: "a role",
            extra: &[],
        },
        NameCase {
            case: "a role update addressed by a name carrying an asterisk",
            action: "UpdateRole",
            member: "roleName",
            value: "a-role*",
            extra: &[],
        },
        // A tag listing, addressed by a whole role ARN — the value a client reaches for when
        // a name is wanted, which carries both delimiters and a colon.
        NameCase {
            case: "a role tag listing addressed by the role's ARN",
            action: "ListRoleTags",
            member: "roleName",
            value: "arn:aws:iam::123456789012:role/eng/a-role",
            extra: &[],
        },
        // `groupName`, a different name shape with a different published length, on its read.
        NameCase {
            case: "a group read addressed by a path-prefixed name",
            action: "GetGroup",
            member: "groupName",
            value: "/eng/a-group",
            extra: &[],
        },
        // `policyName` beside members that are all inside their forms, so the sentence
        // answers one member of a request carrying three.
        NameCase {
            case: "an inline policy write naming a path-prefixed policy",
            action: "PutRolePolicy",
            member: "policyName",
            value: "eng/a-policy",
            extra: &[("RoleName", "a-role"), ("PolicyDocument", POLICY_DOCUMENT)],
        },
        // The trust-policy write, whose own document member is inside its form.
        NameCase {
            case: "a trust policy write addressed by a path-prefixed role name",
            action: "UpdateAssumeRolePolicy",
            member: "roleName",
            value: "/eng/a-role",
            extra: &[("PolicyDocument", TRUST_POLICY)],
        },
        // The RENAME members: the member the sentence names is not the entity's own name,
        // and the entity's name on the same request is inside its form.
        NameCase {
            case: "a user rename to a path-prefixed name",
            action: "UpdateUser",
            member: "newUserName",
            value: "/eng/renamed-user",
            extra: &[("UserName", "a-user")],
        },
        NameCase {
            case: "a group rename to a path-prefixed name",
            action: "UpdateGroup",
            member: "newGroupName",
            value: "eng/renamed-group",
            extra: &[("GroupName", "a-group")],
        },
        NameCase {
            case: "a server certificate rename to a path-prefixed name",
            action: "UpdateServerCertificate",
            member: "newServerCertificateName",
            value: "/certs/renamed",
            extra: &[("ServerCertificateName", "a-certificate")],
        },
    ]
}

/// RULE: a name carrying a path, or any other character outside the published entity-name
/// class, is refused 400 under the one input-refusal code with AWS's single sentence for that
/// class — naming the member and nothing of the value.
///
/// The message is compared WHOLE, so the same assertion is what rules out the model-level
/// walk's wording: a refusal carrying its envelope, one of its per-constraint clauses or the
/// raw expression a client would otherwise surface to a user is not equal to the sentence.
#[tokio::test]
async fn a_name_outside_the_published_class_is_refused_with_awss_sentence() {
    let router = router();
    for row in name_cases() {
        // The member under test is supplied last, so a row's `extra` members are all inside
        // their published forms and the name is the only fault.
        let mut params: Vec<(String, String)> = row
            .extra
            .iter()
            .map(|(name, supplied)| param(name, supplied))
            .collect();
        params.push(param(&wire_member(row.member), row.value));
        assert_refused(
            &router,
            RefusalCase {
                case: row.case.to_string(),
                action: row.action,
                params,
                expected: invalid_name(row.member),
                rejected: Some(row.value.to_string()),
            },
        )
        .await;
    }
}

/// RULE: the member's LENGTH is a separate requirement, so a name INSIDE the published class
/// and past the published maximum keeps the model-level walk's own clause, and a name outside
/// both is answered with the sentence alone — the class is taken first and states the whole
/// form it describes.
#[tokio::test]
async fn the_published_length_is_a_separate_requirement() {
    let router = router();
    // `roleNameType` is length 1..64 (`smithy_query(query="shape", shape="roleNameType")`),
    // so one character past it, spelled from the class's own alphabet.
    let too_long = "r".repeat(65);
    assert_refused(
        &router,
        RefusalCase {
            case: "a role name one character past its published maximum".to_string(),
            action: "GetRole",
            params: vec![param("RoleName", &too_long)],
            expected: crate::validation_error_clauses::one_error(
                "Value at 'roleName' failed to satisfy constraint: Member must have length \
                 less than or equal to 64",
            ),
            rejected: Some(too_long.clone()),
        },
    )
    .await;

    // Outside the class AND past the maximum: one sentence, because the class's sentence
    // states that member's whole form.
    let long_and_pathed = format!("/eng/{too_long}");
    assert_refused(
        &router,
        RefusalCase {
            case: "a role name outside the class and past its maximum".to_string(),
            action: "GetRole",
            params: vec![param("RoleName", &long_and_pathed)],
            expected: invalid_name("roleName"),
            rejected: Some(long_and_pathed.clone()),
        },
    )
    .await;

    // At the published maximum, inside the class: the refusal is about the form, so this
    // reaches the handler and is reported absent rather than refused.
    let at_maximum = "r".repeat(64);
    let (status, body) = call(&router, "GetRole", &[("RoleName", at_maximum.as_str())]).await;
    assert_eq!(
        status, 404,
        "a role name at the published maximum reaches the handler, got {status}: {body}"
    );
    let answered = error_code(&body);
    assert_eq!(
        answered, "NoSuchEntity",
        "expected the modelled absence code, got {answered:?}: {body}"
    );
}

/// RULE (control): the sentence is about the FORM of the name, so a name inside the published
/// class reaches the handler — the role's PATH is the member a path belongs in, and a role
/// created under a nested path is read back by its plain name.
#[tokio::test]
async fn a_name_inside_the_published_class_is_served_and_the_path_carries_the_path() {
    let router = router();
    // Rows: a plain name, every punctuation character the class admits, and a name at
    // `roleNameType`'s published maximum length of 64 — each created under a PATH carrying
    // the delimiters the name may not.
    for (case, path, name) in [
        ("a plain name at the account root", "/", "a-role".to_string()),
        (
            "the punctuation the class admits, under a service path",
            "/service-role/",
            "Role+Name=1,2.3@x_-".to_string(),
        ),
        (
            "a name at the published maximum, under a nested path",
            "/eng/tools/",
            "r".repeat(64),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", name.as_str()),
                ("Path", path),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the role to be created, got {status}: {body}"
        );
        // Round trip through the read the issue was filed against: the name is reported as
        // supplied, and the path is reported from the member that carries it.
        let (status, body) = call(&router, "GetRole", &[("RoleName", name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the role to be read back, got {status}: {body}"
        );
        let reported = element(&body, "RoleName");
        assert_eq!(
            reported, name,
            "case {case:?}: expected the stored name to be reported, got {reported:?}"
        );
        let reported_path = element(&body, "Path");
        assert_eq!(
            reported_path, path,
            "case {case:?}: expected the stored path to be reported, got {reported_path:?}"
        );
        // The ARN the create minted carries the path and the name as supplied, so the two
        // members are not interchangeable: the name alone addresses the role.
        let arn = element(&body, "Arn");
        let expected_arn = format!("arn:aws:iam::{}:role{path}{name}", account_id());
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {arn:?}"
        );
    }

    // A well-formed name the account holds no role under is reported absent, not refused:
    // the form check never stands in for the entity lookup.
    let (status, body) = call(&router, "GetRole", &[("RoleName", "a-role-not-held")]).await;
    assert_eq!(
        status, 404,
        "expected the absent role to be reported, got {status}: {body}"
    );
    let answered = error_code(&body);
    assert_eq!(
        answered, "NoSuchEntity",
        "expected the modelled absence code, got {answered:?}: {body}"
    );
}
