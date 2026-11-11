// SPDX-License-Identifier: CC0-1.0

//! The ERROR CODE this service answers an input-constraint refusal with, over the restJson1
//! wire.
//!
//! The rule under test: a request whose value breaks the FORM its target shape declares — a
//! `@length` bound or a `@pattern` — is refused with the operation's own modelled client
//! error, AppSync's `BadRequestException` at HTTP 400, carrying the constraint report that
//! names the offending member and the expression it broke. `ValidationException` is no
//! operation's modelled error on this service
//! (`smithy_query(query="operation", operation="TagResource,UntagResource,
//! PutGraphqlApiEnvironmentVariables,ListDomainNames")` — each declares
//! `BadRequestException` 400 and none declares a constraint-violation shape), so a client
//! dispatching on the modelled error type never matches that code and code branching on it
//! takes the wrong branch.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over: the
//! OPERATION and its HTTP method, WHERE the offending value sits (a map key, a map value, a
//! list element, a query parameter), the CLASS of constraint broken (a minimum length, a
//! maximum length, a pattern) and the kind of shape that declares it (a map, a list, a
//! string). Each clause's expression is read from the model, named on the constant that
//! carries it.

use serde_json::{json, Map, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, router, Answer};
use crate::resource_tags::encode_label;

/// AppSync's own modelled error for a request that is not well formed, and its status:
/// `BadRequestException`, `@httpError(400)`
/// (`smithy_query(query="shape", shape="BadRequestException")`).
const BAD_REQUEST: &str = "BadRequestException";
const BAD_REQUEST_STATUS: u16 = 400;

/// `TagValue`'s declared expression, as the model states it — the set of characters a tag
/// value may hold. Source: `smithy_query(query="shape", shape="TagValue")`.
const TAG_VALUE_PATTERN: &str = r"^[\s\w+-=\.:/@]*$";

/// AppSync's fixed sentence for a tag whose VALUE carries a character tags may not hold —
/// the grammar the service states rather than the member's own clause. Source: the service's
/// one copy of it, `state::INVALID_TAG_VALUE_MESSAGE`.
const TAG_GRAMMAR: &str = state::INVALID_TAG_VALUE_MESSAGE;

/// `TagKey`'s declared expression is enforced by the tagging handlers' own sentence rather
/// than at the boundary, so no row here breaks it: this suite is about the refusals the
/// constraint layer takes.
///
/// `CertificateArn`'s declared expression. Source:
/// `smithy_query(query="shape", shape="CertificateArn")`.
const CERTIFICATE_ARN_PATTERN: &str =
    r"^arn:[a-z-]*:(acm|iam):[a-z0-9-]*:\d{12}:(certificate|server-certificate)/[0-9A-Za-z_/-]*$";

/// The modelled maximum length of `DomainName`. Source:
/// `smithy_query(query="shape", shape="DomainName")` — `@length` 1..253.
const DOMAIN_NAME_MAX_LEN: usize = 253;

/// `CreateDomainName`'s path. Source: the `http` binding of `CreateDomainName`.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// `PaginationToken`'s declared expression. Source:
/// `smithy_query(query="shape", shape="PaginationToken")`.
const PAGINATION_TOKEN_PATTERN: &str = r"^[\S]+$";

/// The modelled maximum of `EnvironmentVariableMap`, the entries one API may carry. Source:
/// `smithy_query(query="shape", shape="EnvironmentVariableMap")` — `@length` 0..50.
const ENVIRONMENT_VARIABLES_MAX: usize = 50;

/// The modelled minimum of `TagMap`, the entries one tagging write must carry. Source:
/// `smithy_query(query="shape", shape="TagMap")` — `@length` 1..50.
const TAGS_MIN: usize = 1;

/// The modelled maximum length of `EnvironmentVariableValue`. Source:
/// `smithy_query(query="shape", shape="EnvironmentVariableValue")` — `@length` 0..512.
const ENVIRONMENT_VARIABLE_VALUE_MAX: usize = 512;

/// Create one GraphQL API and answer its id and ARN.
async fn api(router: &MultiServiceRouter) -> (String, String) {
    let answer = call(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": "Api_constraint_codes",
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"fixture\": expected CreateGraphqlApi 200, got {answer:?}"
    );
    (
        answer.body["graphqlApi"]["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        answer.body["graphqlApi"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// A map of `count` well-formed environment variables.
fn environment_variables(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("VAR_{index:03}"), json!("value"));
    }
    Value::Object(map)
}

/// A domain name one character past the modelled maximum length, whose first label absorbs
/// the padding — so the only clause it breaks is the length, not the pattern.
fn overlong_domain_name() -> String {
    let tail = ".example.com";
    format!(
        "{}{tail}",
        "a".repeat(DOMAIN_NAME_MAX_LEN + 1 - tail.len())
    )
}

/// One row: a refusal the constraint layer takes, and the clause its report must carry.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause names, as the constraint walk spells it.
    member: String,
    /// The requirement the clause states, as the framework words that constraint class.
    requirement: String,
}

/// The table: every row breaks a declared `@length` or `@pattern`, at a different position
/// and in a different class.
fn rows(api_id: &str, api_arn: &str) -> Vec<Row> {
    let tags_path = format!("/v1/tags/{}", encode_label(api_arn));
    let environment_path = format!("/v1/apis/{api_id}/environmentVariables");
    vec![
        Row {
            case: "a tag map under its declared minimum",
            method: "POST",
            path: tags_path.clone(),
            body: Some(json!({"tags": {}})),
            member: "tags".to_string(),
            requirement: format!("Member must have length greater than or equal to {TAGS_MIN}"),
        },
        Row {
            case: "a certificate ARN outside its declared pattern",
            method: "POST",
            path: DOMAIN_NAMES_PATH.to_string(),
            body: Some(json!({
                "domainName": "certificate.example.com",
                "certificateArn": "arn:aws:s3:::held/certificate/one",
            })),
            member: "certificateArn".to_string(),
            requirement: format!(
                "Member must satisfy regular expression pattern: {CERTIFICATE_ARN_PATTERN}"
            ),
        },
        Row {
            case: "a domain name past its declared maximum length",
            method: "POST",
            path: DOMAIN_NAMES_PATH.to_string(),
            body: Some(json!({
                "domainName": overlong_domain_name(),
                "certificateArn":
                    "arn:aws:acm:us-east-1:000000000000:certificate/constraint-0001",
            })),
            member: "domainName".to_string(),
            requirement: format!(
                "Member must have length less than or equal to {DOMAIN_NAME_MAX_LEN}"
            ),
        },
        Row {
            case: "an environment-variable map past its declared maximum",
            method: "PUT",
            path: environment_path.clone(),
            body: Some(json!({
                "environmentVariables": environment_variables(ENVIRONMENT_VARIABLES_MAX + 1),
            })),
            member: "environmentVariables".to_string(),
            requirement: format!(
                "Member must have length less than or equal to {ENVIRONMENT_VARIABLES_MAX}"
            ),
        },
        Row {
            case: "an environment-variable value past its declared maximum length",
            method: "PUT",
            path: environment_path,
            body: Some(json!({
                "environmentVariables": {
                    "LONG_VALUE": "v".repeat(ENVIRONMENT_VARIABLE_VALUE_MAX + 1),
                },
            })),
            member: "environmentVariables.LONG_VALUE".to_string(),
            requirement: format!(
                "Member must have length less than or equal to {ENVIRONMENT_VARIABLE_VALUE_MAX}"
            ),
        },
        Row {
            case: "a pagination token outside its declared pattern",
            method: "GET",
            path: "/v1/domainnames?nextToken=has%20a%20space".to_string(),
            body: None,
            member: "nextToken".to_string(),
            requirement: format!(
                "Member must satisfy regular expression pattern: {PAGINATION_TOKEN_PATTERN}"
            ),
        },
    ]
}

/// RULE: a value that breaks the form its target shape declares is refused with the
/// modelled `BadRequestException` (400) and a report naming the member and the requirement.
#[tokio::test]
async fn a_value_outside_its_declared_form_is_a_bad_request() {
    let router = router();
    let (api_id, api_arn) = api(&router).await;

    for row in rows(&api_id, &api_arn) {
        let Row {
            case,
            method,
            path,
            body,
            member,
            requirement,
        } = row;
        let answer: Answer = call(&router, method, &path, body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (BAD_REQUEST_STATUS, BAD_REQUEST),
            "case {case:?}: expected {BAD_REQUEST_STATUS} {BAD_REQUEST}, got {answer:?}"
        );
        assert!(
            answer.message.starts_with("1 validation error detected: "),
            "case {case:?}: expected one clause for the one offending member, got {answer:?}"
        );
        assert!(
            answer
                .message
                .contains(&format!("at '{member}' failed to satisfy constraint: {requirement}")),
            "case {case:?}: expected the clause to name {member:?} and its requirement \
             {requirement:?}, got {answer:?}"
        );
    }
}

/// RULE: the refused request changed nothing — the refusal is taken at the boundary, before
/// any handler runs, so the resources the rows addressed are as they were.
#[tokio::test]
async fn a_refused_request_writes_nothing() {
    let router = router();
    let (api_id, api_arn) = api(&router).await;

    for row in rows(&api_id, &api_arn) {
        let answer = call(&router, row.method, &row.path, row.body).await;
        assert_eq!(
            answer.status, BAD_REQUEST_STATUS,
            "case {:?}: expected the row to be refused, got {answer:?}",
            row.case
        );
    }

    let tags = call(
        &router,
        "GET",
        &format!("/v1/tags/{}", encode_label(&api_arn)),
        None,
    )
    .await;
    assert_eq!(
        tags.body["tags"],
        json!({}),
        "case \"a refused tagging write\": expected no tags on the API, got {tags:?}"
    );
    let environment = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/environmentVariables"),
        None,
    )
    .await;
    assert_eq!(
        environment.body["environmentVariables"],
        json!({}),
        "case \"a refused environment write\": expected no variables on the API, got \
         {environment:?}"
    );
}

/// RULE: a value INSIDE the declared form is served, so the rows above are refused for the
/// constraint they break rather than for the operation they name.
///
/// Each row sits at the published bound of the constraint its refused counterpart crosses.
#[tokio::test]
async fn a_value_at_the_declared_bound_is_served() {
    let router = router();
    let (api_id, api_arn) = api(&router).await;
    let tags_path = format!("/v1/tags/{}", encode_label(&api_arn));
    let environment_path = format!("/v1/apis/{api_id}/environmentVariables");

    // (case, method, path, body, the status the call is served with)
    let served: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "a tag map at its declared minimum",
            "POST",
            tags_path.clone(),
            Some(json!({"tags": {"stage": "beta"}})),
            204,
        ),
        (
            "an environment-variable map at its declared maximum",
            "PUT",
            environment_path.clone(),
            Some(json!({
                "environmentVariables": environment_variables(ENVIRONMENT_VARIABLES_MAX),
            })),
            200,
        ),
        (
            "an environment-variable value at its declared maximum length",
            "PUT",
            environment_path,
            Some(json!({
                "environmentVariables": {
                    "LONG_VALUE": "v".repeat(ENVIRONMENT_VARIABLE_VALUE_MAX),
                },
            })),
            200,
        ),
        (
            "a listing that names no resume token at all",
            "GET",
            "/v1/domainnames".to_string(),
            None,
            200,
        ),
    ];
    for (case, method, path, body, expected) in served {
        let answer = call(&router, method, &path, body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected the call to be served with {expected}, got {answer:?}"
        );
    }
}

/// RULE: a tag whose VALUE carries a character outside the set tags may hold is refused
/// with the service's `BadRequestException` (400) stating the TAG GRAMMAR — the permitted
/// characters, and nothing of the request — rather than with the member's own constraint
/// clause; a value drawn only from that set round-trips.
///
/// The refused rows differ along WHICH character outside the set the value carries (an
/// emoji, a Greek letter, CJK text, and three punctuation marks the expression does not
/// admit) and along where in the value it sits; the served rows span the whole permitted
/// set, taken from `TagValue`'s declared expression.
#[tokio::test]
async fn a_tag_value_outside_the_permitted_characters_states_the_tag_grammar() {
    let router = router();
    let (_api_id, api_arn) = api(&router).await;
    let tags_path = format!("/v1/tags/{}", encode_label(&api_arn));

    // (case, the tag key, the tag value) — the key is inside `TagKey`'s own set in every
    // row, so what the request breaks is the VALUE's.
    //
    // Every value below carries a character the declared expression admits NOWHERE: its
    // `+-=` run is a RANGE (`+` through `=`, so the digits, `,`, `;`, `<` and `>` are inside
    // the set), and `\w`'s own scope is the framework's business rather than this rule's, so
    // the rows are built from punctuation outside both.
    let refused: &[(&str, &str, &str)] = &[
        ("an emoji", "stage", "val-🎉-beta"),
        ("an exclamation mark at the end", "stage", "beta!"),
        ("a hash at the start", "stage", "#beta"),
        ("brackets inside an otherwise valid value", "stage", "beta[1]"),
        ("parentheses", "Unicode_Tag", "beta(1)"),
        ("an asterisk", "stage", "beta*"),
        ("a percent sign", "stage", "100%"),
        ("an ampersand", "stage", "orders&more"),
    ];
    for (case, key, value) in refused {
        let answer = call(
            &router,
            "POST",
            &tags_path,
            Some(json!({"tags": {*key: *value}})),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (BAD_REQUEST_STATUS, BAD_REQUEST),
            "case {case:?}: expected {BAD_REQUEST_STATUS} {BAD_REQUEST}, got {answer:?}"
        );
        assert_eq!(
            answer.message, TAG_GRAMMAR,
            "case {case:?}: expected the tag grammar sentence, got {answer:?}"
        );
        assert!(
            !answer.message.contains(value) && !answer.message.contains(TAG_VALUE_PATTERN),
            "case {case:?}: expected the sentence to state the permitted characters alone, \
             got {answer:?}"
        );
    }

    // Every character the declared expression admits, in one value and then split across
    // several tags: the refusal above is about the characters, not about tagging.
    let permitted = "Letters09 _.:/=+-@";
    let answer = call(
        &router,
        "POST",
        &tags_path,
        Some(json!({"tags": {"stage": permitted, "team": "orders", "empty": ""}})),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case \"every permitted character\": expected the write to be served, got {answer:?}"
    );
    let listed = call(&router, "GET", &tags_path, None).await;
    assert_eq!(
        listed.body["tags"],
        json!({"stage": permitted, "team": "orders", "empty": ""}),
        "case \"every permitted character\": expected the tags to round-trip, got {listed:?}"
    );
}
