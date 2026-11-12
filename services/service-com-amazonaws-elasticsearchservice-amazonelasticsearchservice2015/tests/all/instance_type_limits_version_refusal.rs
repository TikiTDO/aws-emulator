// SPDX-License-Identifier: CC0-1.0

//! How the legacy `2015-01-01` instance-type paths word a refusal of a member whose
//! `@pattern` is an alternation of individually anchored branches, driven as real
//! restJson1 requests through the router.
//!
//! The rule under test: a `@pattern` requirement is quoted as the expression the service
//! advertises for THAT member — the model's text without the whole-value `^…$` pair the
//! model wrapped it in, and the model's text UNCHANGED where what that pair encloses
//! carries anchors of its own, because the two ends of such a text are the first and last
//! branch's anchors and dropping them would state a requirement of no branch. The
//! rendering is per member and per expression, not a blanket anchor policy, and it does
//! not depend on where the value was bound (a URI label, a query parameter, a body
//! member) or on which operation carried it.
//!
//! The consequence this suite pins: the expression a caller lifts out of the message
//! decides exactly the values the service decides. The last test compiles the quoted
//! expression and requires it to agree with the service's own accept/refuse answer on
//! every row, so a client that pre-validates engine versions locally cannot accept a
//! value the API refuses.
//!
//! Every modelled expression below is the model's own, read with
//!
//! ```text
//! smithy_query(query="shape",shape="ElasticsearchVersionString,DomainName",model_path="services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json")
//! ```
//!
//! and the version catalog's rows are the three branches that expression declares, each
//! probed inside and outside the digit runs it admits.

use serde_json::{json, Value};

use crate::harness::{call_in, router};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The `@pattern` of `ElasticsearchVersionString`: three branches, each anchored on its
/// own, so the model's text is already a whole-value expression and is published as
/// declared.
const VERSION_PATTERN: &str =
    r"^[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}$";

/// The `@pattern` of `DomainName`: one expression the model wrapped in the whole-value
/// pair, so the pair is dropped and the expression inside it is published.
const DOMAIN_NAME_PATTERN: &str = r"^[a-z][a-z0-9\-]+$";

/// [`DOMAIN_NAME_PATTERN`] as this API publishes it.
const DOMAIN_NAME_PUBLISHED: &str = r"[a-z][a-z0-9\-]+";

/// The whole `ValidationException` message refusing one member's value on a `@pattern`:
/// the aggregate envelope, the echoed value, the member path in lowerCamel and the
/// expression the member advertises.
fn pattern_refusal(value: &str, member_path: &str, expression: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {expression}"
    )
}

/// One row: a request carrying a value its member's `@pattern` does not admit, and the
/// whole message the API has to answer with.
struct Case {
    /// What the row is about, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause states, as a clause spells one.
    member_path: &'static str,
    /// The value the request carried, which the clause echoes.
    value: &'static str,
    /// The `@pattern` the model declares for that member.
    modelled: &'static str,
    /// The expression the clause has to quote.
    published: &'static str,
}

/// The rows of the version rule: the same shape bound as a URI label on each of the two
/// legacy instance-type routes and as a body member on two writers, carrying values that
/// fall outside the alternation in each of the ways it can be missed — too long a digit
/// run before the separator, a tail that would satisfy a branch if the branch were not
/// anchored at its start, a head that would satisfy one if it were not anchored at its
/// end, a branch prefix with its minor part missing, and a case-shifted prefix.
fn version_cases() -> Vec<Case> {
    vec![
        Case {
            case: "DescribeElasticsearchInstanceTypeLimits' ElasticsearchVersion label",
            method: "GET",
            path: "/2015-01-01/es/instanceTypeLimits/99.99/m3.medium.elasticsearch".to_string(),
            body: None,
            member_path: "elasticsearchVersion",
            value: "99.99",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
        Case {
            // A value whose TAIL satisfies the first branch: admitted by a de-anchored
            // reading of the expression, refused by the trait.
            case: "the limits label with a prefixed major.minor",
            method: "GET",
            path: "/2015-01-01/es/instanceTypeLimits/v7.10/m3.medium.elasticsearch".to_string(),
            body: None,
            member_path: "elasticsearchVersion",
            value: "v7.10",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
        Case {
            // A value whose HEAD satisfies the last branch but does not terminate there.
            case: "the limits label with a third version part",
            method: "GET",
            path: "/2015-01-01/es/instanceTypeLimits/OS_2.3.1/m3.medium.elasticsearch".to_string(),
            body: None,
            member_path: "elasticsearchVersion",
            value: "OS_2.3.1",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
        Case {
            // The same shape as a label on the other legacy route.
            case: "ListElasticsearchInstanceTypes' ElasticsearchVersion label",
            method: "GET",
            path: "/2015-01-01/es/instanceTypes/OpenSearch_2".to_string(),
            body: None,
            member_path: "elasticsearchVersion",
            value: "OpenSearch_2",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
        Case {
            // The same shape as a top-level BODY member, under another member name.
            case: "UpgradeElasticsearchDomain's TargetVersion member",
            method: "POST",
            path: "/2015-01-01/es/upgradeDomain".to_string(),
            body: Some(json!({"DomainName": "upgrade-probe", "TargetVersion": "7.10.1"})),
            member_path: "targetVersion",
            value: "7.10.1",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
        Case {
            case: "CreateElasticsearchDomain's ElasticsearchVersion member",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({"DomainName": "create-probe", "ElasticsearchVersion": "os_2.3"})),
            member_path: "elasticsearchVersion",
            value: "os_2.3",
            modelled: VERSION_PATTERN,
            published: VERSION_PATTERN,
        },
    ]
}

/// The rows that pin the rendering as PER MEMBER rather than a blanket anchor policy: the
/// member whose expression the model wrapped is published without the pair, including on
/// the very operations whose version member is published with its anchors.
fn wrapped_expression_cases() -> Vec<Case> {
    vec![
        Case {
            case: "DescribeElasticsearchInstanceTypeLimits' DomainName query parameter",
            method: "GET",
            path: "/2015-01-01/es/instanceTypeLimits/OS_2.3/m3.medium.elasticsearch\
                   ?domainName=Bad_Domain"
                .to_string(),
            body: None,
            member_path: "domainName",
            value: "Bad_Domain",
            modelled: DOMAIN_NAME_PATTERN,
            published: DOMAIN_NAME_PUBLISHED,
        },
        Case {
            case: "ListElasticsearchInstanceTypes' DomainName query parameter",
            method: "GET",
            path: "/2015-01-01/es/instanceTypes/OS_2.3?domainName=Bad_Domain".to_string(),
            body: None,
            member_path: "domainName",
            value: "Bad_Domain",
            modelled: DOMAIN_NAME_PATTERN,
            published: DOMAIN_NAME_PUBLISHED,
        },
        Case {
            case: "GetCompatibleElasticsearchVersions' DomainName query parameter",
            method: "GET",
            path: "/2015-01-01/es/compatibleVersions?domainName=BAD".to_string(),
            body: None,
            member_path: "domainName",
            value: "BAD",
            modelled: DOMAIN_NAME_PATTERN,
            published: DOMAIN_NAME_PUBLISHED,
        },
    ]
}

/// Drive one row and assert the WHOLE message, so nothing of the template or of the
/// quoted expression is left unpinned.
async fn assert_refused_as_stated(cases: Vec<Case>) {
    let router = router();

    for Case {
        case,
        method,
        path,
        body,
        member_path,
        value,
        modelled,
        published,
    } in cases
    {
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside the pattern must be refused with 400, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        let message = answer.text(case, "/message");
        let expected = pattern_refusal(value, member_path, published);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?} (the model declares {modelled:?}), got \
             {message:?}"
        );
    }
}

/// RULE: a `@pattern` whose branches are each anchored is quoted as the model declares
/// it, whatever member carries the shape and wherever the value was bound.
#[tokio::test]
async fn a_branch_anchored_pattern_is_quoted_as_the_model_declares_it() {
    assert_refused_as_stated(version_cases()).await;
}

/// RULE: the same rendering drops the whole-value pair for a member whose expression
/// carries no anchor of its own — so the two renderings live side by side on one
/// operation, keyed by the expression rather than by a blanket policy.
#[tokio::test]
async fn a_model_wrapped_pattern_is_quoted_without_its_pair() {
    assert_refused_as_stated(wrapped_expression_cases()).await;
}

/// The version catalog: the three branches of `ElasticsearchVersionString`'s `@pattern`,
/// each probed inside the digit runs it admits and just outside them, beside the values
/// that miss a branch only because the branch is anchored. `true` means the trait admits
/// the value.
///
/// Source: the `@pattern` of `ElasticsearchVersionString` (the `smithy_query` in the
/// module docs above); nothing here is a value this API mints.
const VERSION_CATALOG: [(&str, &str, bool); 16] = [
    ("the first branch, one minor digit", "7.1", true),
    ("the first branch, two minor digits", "7.10", true),
    ("the first branch, lowest major", "1.5", true),
    ("a two-digit major, past the first branch", "99.99", false),
    ("three minor digits, past the first branch", "7.100", false),
    ("a third version part", "7.10.1", false),
    ("a prefixed major.minor", "v7.10", false),
    ("the OpenSearch branch", "OpenSearch_2.3", true),
    (
        "the OpenSearch branch, two-digit parts",
        "OpenSearch_11.11",
        true,
    ),
    ("the OpenSearch branch with no minor", "OpenSearch_2", false),
    (
        "the OpenSearch branch, three major digits",
        "OpenSearch_111.1",
        false,
    ),
    ("the OS branch", "OS_2.3", true),
    ("the OS branch, two-digit parts", "OS_11.11", true),
    ("the OS branch with a third part", "OS_2.3.1", false),
    ("a case-shifted OS branch", "os_2.3", false),
    ("a trailing separator", "7.", false),
];

/// RULE: the expression the refusal quotes decides exactly the values the service
/// decides — a caller that compiles it out of the message and pre-validates locally
/// accepts what the API accepts and refuses what the API refuses.
///
/// Driven through `CreateElasticsearchDomain`, the writer that carries the shape in its
/// body, so an admitted version reaches the handler and answers 200 rather than the 501 a
/// not-yet-written reader would answer.
#[tokio::test]
async fn the_quoted_expression_decides_the_values_the_service_decides() {
    let router = router();
    let quoted = ::regex::Regex::new(VERSION_PATTERN)
        .expect("the quoted expression must compile for a caller that lifts it out");

    for (index, (case, version, admitted)) in VERSION_CATALOG.into_iter().enumerate() {
        let name = format!("version-probe-{index}");
        let answer = call_in(
            &router,
            REGION,
            "POST",
            "/2015-01-01/es/domain",
            Some(&json!({"DomainName": name, "ElasticsearchVersion": version})),
        )
        .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({version:?}): the pattern admits this version, got {} {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?} ({version:?}): the pattern does not admit this version, got {} {}",
                answer.status, answer.body
            );
            let message = answer.text(case, "/message");
            let expected = pattern_refusal(version, "elasticsearchVersion", VERSION_PATTERN);
            assert_eq!(
                message, expected,
                "case {case:?} ({version:?}): expected {expected:?}, got {message:?}"
            );
        }
        // The quoted expression, applied by a caller the way Smithy applies a `@pattern`
        // (a find), has to decide the value the way the service just decided it.
        let matched = quoted.is_match(version);
        assert_eq!(
            matched, admitted,
            "case {case:?} ({version:?}): the quoted expression {VERSION_PATTERN:?} must decide \
             this version the way the service did ({admitted}), got {matched}"
        );
    }
}
