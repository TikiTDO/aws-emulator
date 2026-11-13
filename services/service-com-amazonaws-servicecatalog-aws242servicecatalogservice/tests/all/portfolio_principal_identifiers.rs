// SPDX-License-Identifier: CC0-1.0

//! What `ListPrincipalsForPortfolio` reports for each principal associated with a
//! portfolio, over the wire, with the account's identity records served by the IAM
//! service mounted beside Service Catalog on the ONE shared engine.
//!
//! The rule under test: an association of a CONCRETE principal (`PrincipalType` `IAM`, a
//! fully defined IAM ARN) records and reports that principal's own unique id in the
//! account's identity records — never the ARN string the association request carried —
//! while an `IAM_PATTERN`, which `Principal$PrincipalARN` allows to omit the account id
//! and to carry wildcards and which therefore names no single principal, records and
//! reports the pattern as given.
//!
//! Each case is a ROW of a table spanning what the rule generalises over: the kind of
//! principal (role, user, group), its path (the root, one segment, a nested path), the
//! alphabet of its name, whether the identity records hold it, and the wildcard forms a
//! pattern takes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire transport and the response readers every suite in this crate shares.
use crate::harness::{call, create_portfolio, ACCOUNT_ID, DEFAULT_REGION};

/// The awsQuery API version an IAM request carries, from that service's model
/// (`com.amazonaws.iam#AWSIdentityManagementV20100508`, version 2010-05-08).
const IAM_API_VERSION: &str = "2010-05-08";

/// A trust document a created role carries. `CreateRole$AssumeRolePolicyDocument` is
/// required, and nothing here turns on what it says.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"servicecatalog.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// How many characters an AWS unique id carries after its four-character type prefix
/// (`AIDAJ4XQZ7X5YQ2R3TZ4A` is `AIDA` plus 17), the form `User$UserId`, `Role$RoleId` and
/// `Group$GroupId` are minted in.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// One kind of IAM principal — "the principal (user, role, or group)"
/// (`Principal$PrincipalARN`) — with everything a case needs to create one, read it back
/// and know the id it carries.
struct Kind {
    /// The resource type its ARN carries.
    resource_type: &'static str,
    /// The awsQuery `Action` that creates one.
    create: &'static str,
    /// The awsQuery `Action` that reads one back.
    read: &'static str,
    /// The request parameter its friendly name is passed under.
    name_param: &'static str,
    /// The XML element its unique id is reported in.
    id_element: &'static str,
    /// The type prefix AWS gives its unique id, from IAM's "unique identifier prefixes"
    /// table (`AIDA` a user, `AROA` a role, `AGPA` a group).
    id_prefix: &'static str,
}

/// An IAM role.
const ROLE: Kind = Kind {
    resource_type: "role",
    create: "CreateRole",
    read: "GetRole",
    name_param: "RoleName",
    id_element: "RoleId",
    id_prefix: "AROA",
};

/// An IAM user.
const USER: Kind = Kind {
    resource_type: "user",
    create: "CreateUser",
    read: "GetUser",
    name_param: "UserName",
    id_element: "UserId",
    id_prefix: "AIDA",
};

/// An IAM group.
const GROUP: Kind = Kind {
    resource_type: "group",
    create: "CreateGroup",
    read: "GetGroup",
    name_param: "GroupName",
    id_element: "GroupId",
    id_prefix: "AGPA",
};

/// One emulator carrying Service Catalog and the service that holds the account's
/// principals, both on the ONE shared engine, exactly as `core/router/src/lib.rs` mounts
/// them.
///
/// A portfolio's principals are IAM's records, so a suite that asserts what an
/// association reports for one must have that service there.
fn emulator_with_identity() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Drive one IAM request through the same router, as a client's own SDK sends it: an
/// awsQuery form body answered with XML.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut form = ::form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", IAM_API_VERSION);
    for (name, value) in params {
        form.append_pair(name, value);
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        // An unsigned request is routed by the endpoint host's first label, which is
        // that service's signing name.
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{DEFAULT_REGION}/iam/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(form.finish()))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// The text of the first `<tag>` element of an XML body.
fn xml_text(case: &str, body: &str, tag: &str) -> String {
    body.split(&format!("<{tag}>"))
        .nth(1)
        .and_then(|rest| rest.split(&format!("</{tag}>")).next())
        .unwrap_or_else(|| panic!("case {case:?}: no <{tag}> in {body}"))
        .to_string()
}

/// Create one principal of `kind` through IAM's own wire and answer the ARN it minted.
async fn create_principal(
    router: &MultiServiceRouter,
    case: &str,
    kind: &Kind,
    name: &str,
    path: &str,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![(kind.name_param, name), ("Path", path)];
    if kind.resource_type == ROLE.resource_type {
        params.push(("AssumeRolePolicyDocument", TRUST_POLICY));
    }
    let (status, body) = iam(router, kind.create, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must create the principal, got {status} with {body}",
        kind.create
    );
    xml_text(case, &body, "Arn")
}

/// The unique id the identity records REPORT for the principal `name` of `kind` — read
/// back through IAM's own read, so the expectation is that service's answer and not a
/// value this suite made up.
async fn held_identifier(
    router: &MultiServiceRouter,
    case: &str,
    kind: &Kind,
    name: &str,
) -> String {
    let (status, body) = iam(router, kind.read, &[(kind.name_param, name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must report the principal, got {status} with {body}",
        kind.read
    );
    xml_text(case, &body, kind.id_element)
}

/// Associate `principal_arn` with `portfolio` under `principal_type`.
async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    portfolio: &str,
    principal_arn: &str,
    principal_type: &str,
) {
    let (status, body) = call(
        router,
        "AssociatePrincipalWithPortfolio",
        &json!({
            "PortfolioId": portfolio,
            "PrincipalARN": principal_arn,
            "PrincipalType": principal_type,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {principal_arn} must be associable as {principal_type}, got {status} \
         with {body}"
    );
}

/// The portfolio's associations as `(PrincipalARN, PrincipalType)`, in the order the
/// service reports them.
async fn listed_principals(router: &MultiServiceRouter, portfolio: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": portfolio}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListPrincipalsForPortfolio({portfolio}): expected 200, got {status} with {body}"
    );
    body.pointer("/Principals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListPrincipalsForPortfolio({portfolio}): no Principals in {body}"))
        .iter()
        .map(|principal| {
            (
                principal["PrincipalARN"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no PrincipalARN in {principal}"))
                    .to_string(),
                principal["PrincipalType"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no PrincipalType in {principal}"))
                    .to_string(),
            )
        })
        .collect()
}

/// Whether `value` is an AWS unique id of `kind`: that kind's type prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] characters of the unique-id alphabet (uppercase letters and
/// digits).
fn is_unique_id(kind: &Kind, value: &str) -> bool {
    value.strip_prefix(kind.id_prefix).is_some_and(|suffix| {
        suffix.chars().count() == UNIQUE_ID_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    })
}

/// RULE: an association of a concrete `IAM` principal reports the unique id the account's
/// identity records hold for that principal, for every kind of principal and every path.
#[tokio::test]
async fn a_concrete_principal_reports_the_identifier_the_identity_records_hold() {
    let router = emulator_with_identity();
    let portfolio = create_portfolio(&router, "identified principals", "acme", &[], "ident").await;

    // Rows differ along the kind of principal, the path it sits under (the root, one
    // segment, a nested path) and the alphabet of its name — the whole `^[\w+=,.@-]+$`
    // a principal name is drawn from (`smithy_query query="operation"
    // operation="CreateRole"`).
    let rows: &[(&str, &Kind, &str, &str)] = &[
        ("a role at the account root", &ROLE, "sc-admin", "/"),
        (
            "a role under a one-segment path",
            &ROLE,
            "sc-launcher",
            "/service-role/",
        ),
        (
            "a role under a nested path",
            &ROLE,
            "sc-provisioner",
            "/eng/tools/",
        ),
        (
            "a role whose name spans the name alphabet",
            &ROLE,
            "SC_Role.1+a=b,c@d-e",
            "/",
        ),
        ("a user at the account root", &USER, "sc-builder", "/"),
        ("a user under a path", &USER, "sc-developer", "/eng/"),
        ("a group at the account root", &GROUP, "sc-builders", "/"),
        ("a group under a path", &GROUP, "sc-operators", "/teams/"),
    ];

    // Create each principal, associate it, and remember what the identity records report
    // for it beside the ARN the association request carried.
    let mut expected: Vec<(&str, &Kind, String, String)> = Vec::new();
    for (case, kind, name, path) in rows {
        let arn = create_principal(&router, case, kind, name, path).await;
        let identifier = held_identifier(&router, case, kind, name).await;
        assert!(
            is_unique_id(kind, &identifier),
            "case {case:?}: the identity records must report a {} unique id, got {identifier:?}",
            kind.id_prefix
        );
        associate(&router, case, &portfolio, &arn, "IAM").await;
        expected.push((case, kind, arn, identifier));
    }

    // The associations are reported in the order they were made, so each row is checked
    // against its own principal.
    let listed = listed_principals(&router, &portfolio).await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "every association must be listed once, got {listed:?}"
    );
    for ((case, kind, arn, identifier), (reported_arn, reported_type)) in
        expected.iter().zip(listed.iter())
    {
        assert_eq!(
            reported_arn, identifier,
            "case {case:?}: expected the {} identifier the identity records hold, got \
             {reported_arn:?}",
            kind.resource_type
        );
        assert_ne!(
            reported_arn, arn,
            "case {case:?}: the association's request ARN must not be what is reported"
        );
        assert_eq!(
            reported_type, "IAM",
            "case {case:?}: expected the stored type, got {reported_type:?}"
        );
    }

    // And the identifier is a property of the association, not of the call: a second read
    // reports the same values.
    let again = listed_principals(&router, &portfolio).await;
    assert_eq!(
        again, listed,
        "case \"a second read\": a read must not change what a later caller sees"
    );
}

/// RULE: a principal that names no single principal — an `IAM_PATTERN`, which may omit the
/// account id and may carry wildcards — records and reports the pattern it was given,
/// whichever form the pattern takes.
#[tokio::test]
async fn a_pattern_association_reports_the_pattern_it_was_given() {
    let router = emulator_with_identity();
    let portfolio = create_portfolio(&router, "pattern principals", "acme", &[], "pattern").await;

    // A principal the identity records DO hold, so the fully defined row is not merely
    // one they cannot resolve.
    let held_arn = create_principal(&router, "a held role", &ROLE, "sc-exact", "/").await;

    // Rows differ along the pattern's form: account-less and fully defined, exact and
    // wildcarded, with `*` and with `?`, and over each resource type a pattern may name
    // (`AssociatePrincipalWithPortfolio$PrincipalARN`).
    let rows: Vec<(&str, String)> = vec![
        (
            "an account-less wildcard role pattern",
            "arn:aws:iam:::role/Team*".to_string(),
        ),
        (
            "an account-less pattern under a path",
            "arn:aws:iam:::role/service-role/Launcher*".to_string(),
        ),
        (
            "a single-character wildcard over a user",
            "arn:aws:iam:::user/Dev?".to_string(),
        ),
        (
            "an account-less exact group pattern",
            "arn:aws:iam:::group/Builders".to_string(),
        ),
        ("a fully defined ARN as a pattern", held_arn.clone()),
    ];

    for (case, pattern) in &rows {
        associate(&router, case, &portfolio, pattern, "IAM_PATTERN").await;
    }

    let listed = listed_principals(&router, &portfolio).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "every pattern must be listed once, got {listed:?}"
    );
    for ((case, pattern), (reported_arn, reported_type)) in rows.iter().zip(listed.iter()) {
        assert_eq!(
            reported_arn, pattern,
            "case {case:?}: a pattern names no single principal, so it is reported as given, \
             got {reported_arn:?}"
        );
        assert_eq!(
            reported_type, "IAM_PATTERN",
            "case {case:?}: expected the stored type, got {reported_type:?}"
        );
    }
}

/// RULE: the identifier belongs to the ASSOCIATION and the identity records are the
/// authority on it — so an ARN re-associated under the other principal type is reported
/// under what that type calls for, and a concrete principal the records hold no record of
/// is reported as given until they do hold one.
#[tokio::test]
async fn the_recorded_identifier_follows_the_type_and_the_identity_records() {
    let router = emulator_with_identity();
    let portfolio = create_portfolio(&router, "re-associated", "acme", &[], "reassoc").await;

    let arn = create_principal(&router, "a held role", &ROLE, "sc-switcher", "/").await;
    let identifier = held_identifier(&router, "a held role", &ROLE, "sc-switcher").await;

    // (case, the type the ARN is associated under, what the association reports)
    let rows: &[(&str, &str, &str)] = &[
        ("first associated as a concrete principal", "IAM", &identifier),
        ("re-associated as a pattern", "IAM_PATTERN", &arn),
        ("re-associated as a concrete principal", "IAM", &identifier),
    ];
    for (case, principal_type, reported) in rows {
        associate(&router, case, &portfolio, &arn, principal_type).await;
        let listed = listed_principals(&router, &portfolio).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: one ARN is one association, got {listed:?}"
        );
        assert_eq!(
            listed[0],
            ((*reported).to_string(), (*principal_type).to_string()),
            "case {case:?}: expected {reported:?} as {principal_type}, got {:?}",
            listed[0]
        );
    }

    // A principal's identifier is the identity records' to mint, so a concrete ARN they
    // hold no record of — one provisioned out of band — is recorded as given: this
    // service never invents an id, which would be an id no `iam:GetRole` would report.
    // The rows differ along the kind of principal and its path.
    let unheld: &[(&str, &Kind, &str, &str)] = &[
        ("an unheld role", &ROLE, "sc-out-of-band", "/"),
        (
            "an unheld role under a path",
            &ROLE,
            "sc-out-of-band-pathed",
            "/service-role/",
        ),
        ("an unheld user", &USER, "sc-out-of-band-user", "/"),
        ("an unheld group", &GROUP, "sc-out-of-band-group", "/teams/"),
    ];
    let other = create_portfolio(&router, "out of band", "acme", &[], "outofband").await;
    for (case, kind, name, path) in unheld {
        let arn = format!(
            "arn:aws:iam::{ACCOUNT_ID}:{}{path}{name}",
            kind.resource_type
        );
        associate(&router, case, &other, &arn, "IAM").await;
    }
    let listed = listed_principals(&router, &other).await;
    assert_eq!(
        listed.len(),
        unheld.len(),
        "every association must be listed once, got {listed:?}"
    );
    for ((case, kind, name, path), (reported_arn, _)) in unheld.iter().zip(listed.iter()) {
        let arn = format!(
            "arn:aws:iam::{ACCOUNT_ID}:{}{path}{name}",
            kind.resource_type
        );
        assert_eq!(
            reported_arn, &arn,
            "case {case:?}: the records hold no such principal, so the value is recorded as \
             given, got {reported_arn:?}"
        );
    }

    // And once the identity records DO hold one of them, re-associating it reports the id
    // those records mint for it: the resolution reads their state, not this suite's.
    let (case, kind, name, path) = unheld[0];
    let created = create_principal(&router, case, kind, name, path).await;
    let minted = held_identifier(&router, case, kind, name).await;
    assert!(
        is_unique_id(kind, &minted),
        "case {case:?}: expected the records to mint a {} unique id, got {minted:?}",
        kind.id_prefix
    );
    associate(&router, case, &other, &created, "IAM").await;
    let listed = listed_principals(&router, &other).await;
    assert_eq!(
        listed[0],
        (minted.clone(), "IAM".to_string()),
        "case {case:?}: expected {minted:?} once the records hold the principal, got {:?}",
        listed[0]
    );
    assert_eq!(
        listed.len(),
        unheld.len(),
        "case {case:?}: re-associating one ARN must not add an association, got {listed:?}"
    );
}
