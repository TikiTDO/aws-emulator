// SPDX-License-Identifier: CC0-1.0

//! How this service quotes a `@pattern` requirement when it refuses a value the member
//! does not admit.
//!
//! The requirement a refusal states is the EXPRESSION a value has to satisfy, so the
//! `^` / `$` the model wraps it in — Smithy's way of turning a find into a whole-value
//! match — are not part of what the message quotes. Every row below drives the real
//! wire path through [`super::harness`], so what refuses it is the boundary validator
//! and not a handler, and each row asserts the whole message: the framework's envelope,
//! the member path, the echoed value, and the expression WITHOUT the anchor pair.
//!
//! The rows span the members whose values reach that validator from the URI — a path
//! segment (`@httpLabel`) and a query parameter (`@httpQuery`) — over five different
//! modelled expressions, every HTTP method this service serves (GET, POST, PATCH,
//! DELETE) and several ways a value can miss its expression (a word, another vendor's
//! ARN, an ARN of another resource type, a truncated ARN, a bare id). Every expression
//! is quoted from this service's own model, anchors included
//! (`smithy_query(query="operation",operation="GetTemplate,CreateServicePrincipalName,\
//! GetServicePrincipalName,ListConnectors,ListTemplates,\
//! UpdateTemplateGroupAccessControlEntry")`), and the expression each message has to
//! state is COMPUTED from it here rather than written out, so a row states the rule
//! instead of one recorded string.

use serde_json::{json, Value};

use super::harness::{call, encode_label, router, ACCOUNT, REGION};

/// The clause opener a `@pattern` requirement is stated with. Shared with the sibling
/// suite that asserts the same rule over the ARN members of the service-principal-name
/// and connector operations, so the sentence is written here once.
pub(crate) const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// `TemplateArn`'s modelled expression, as the model declares it.
const TEMPLATE_ARN_PATTERN: &str = r"^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/template\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// `ConnectorArn`'s modelled expression.
pub(crate) const CONNECTOR_ARN_PATTERN: &str = r"^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// `DirectoryRegistrationArn`'s modelled expression.
pub(crate) const DIRECTORY_REGISTRATION_ARN_PATTERN: &str =
    r"^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}$";

/// `GroupSecurityIdentifier`'s modelled expression.
const GROUP_SID_PATTERN: &str = r"^S-[0-9]-([0-9]+-){1,14}[0-9]+$";

/// `NextToken`'s modelled expression.
const NEXT_TOKEN_PATTERN: &str =
    r"^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$";

/// A security identifier `GroupSecurityIdentifier`'s expression admits.
const WELL_FORMED_SID: &str = "S-1-5-21-1004336348-1177238915-682003330-512";

/// A pagination token `NextToken`'s expression admits (it is not one this service ever
/// issued, which is a different refusal and not this suite's).
const WELL_FORMED_TOKEN: &str = "pcad0000000000000001";

/// A template ARN `TemplateArn`'s expression admits, in the Region and account every
/// ARN this emulator mints carries.
fn well_formed_template_arn() -> String {
    format!(
        "arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/\
         11111111-2222-3333-4444-555555555555/template/66666666-7777-8888-9999-aaaaaaaaaaaa"
    )
}

/// A connector ARN `ConnectorArn`'s expression admits.
pub(crate) fn well_formed_connector_arn() -> String {
    format!(
        "arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/11111111-2222-3333-4444-555555555555"
    )
}

/// A directory-registration ARN `DirectoryRegistrationArn`'s expression admits.
pub(crate) fn well_formed_directory_registration_arn() -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:directory-registration/d-0123456789")
}

/// The body a `CreateTemplateGroupAccessControlEntry` carries, every member of it
/// acceptable, so the one clause the refusal states is the one the URI earned.
fn access_control_entry_body() -> Value {
    json!({
        "GroupSecurityIdentifier": WELL_FORMED_SID,
        "GroupDisplayName": "Domain Admins",
        "AccessRights": { "Enroll": "ALLOW", "AutoEnroll": "DENY" },
    })
}

/// One refusal: a request whose URI carries exactly one value a member's `@pattern`
/// does not admit.
struct PatternCase {
    /// Why this row is here: the operation, the member, and the way the value misses.
    case: &'static str,
    method: &'static str,
    /// The request target, with `{value}` standing where the member's value goes.
    uri: String,
    body: Option<Value>,
    /// The member path the clause names, as the validating walk spells it.
    member: &'static str,
    /// The value the clause echoes back.
    value: String,
    /// The expression the member declares, anchors included: what the message quotes is
    /// this expression WITHOUT them.
    pattern: &'static str,
    /// A value the same member's expression DOES admit, so each row also pins that the
    /// check still runs the model's pattern as written.
    accepted: String,
}

impl PatternCase {
    /// The request target carrying the offending value, percent-encoded the way an SDK
    /// encodes a non-greedy label or a query value.
    fn target(&self) -> String {
        self.uri.replace("{value}", &encode_label(&self.value))
    }

    /// The same target carrying the value the member's expression admits.
    fn accepted_target(&self) -> String {
        self.uri.replace("{value}", &encode_label(&self.accepted))
    }
}

/// The rows: eight `templateArn` refusals — one per operation that takes the member in
/// its URI — and six more over this service's other URI-bound patterned members, so the
/// rule is asserted over five expressions rather than one.
fn pattern_cases() -> Vec<PatternCase> {
    let template_arn = well_formed_template_arn();
    let connector_arn = well_formed_connector_arn();
    let directory_registration_arn = well_formed_directory_registration_arn();
    // The three URIs whose offending value is NOT the first label: the other labels
    // carry values their own expressions admit, so exactly one clause is reported.
    let one_entry_of_template = format!(
        "/templates/{}/accessControlEntries/{{value}}",
        encode_label(&template_arn)
    );
    let entry_of_each_template = format!(
        "/templates/{{value}}/accessControlEntries/{}",
        encode_label(WELL_FORMED_SID)
    );
    let one_name_of_registration = format!(
        "/directoryRegistrations/{}/servicePrincipalNames/{{value}}",
        encode_label(&directory_registration_arn)
    );
    let name_of_each_registration = format!(
        "/directoryRegistrations/{{value}}/servicePrincipalNames/{}",
        encode_label(&connector_arn)
    );
    vec![
        PatternCase {
            case: "GetTemplate, a word for a template ARN",
            method: "GET",
            uri: "/templates/{value}".to_string(),
            body: None,
            member: "templateArn",
            value: "not-a-valid-arn".to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "CreateTemplateGroupAccessControlEntry, a word for a template ARN",
            method: "POST",
            uri: "/templates/{value}/accessControlEntries".to_string(),
            body: Some(access_control_entry_body()),
            member: "templateArn",
            value: "not-a-valid-arn".to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "GetTemplateGroupAccessControlEntry, a connector ARN for a template ARN",
            method: "GET",
            uri: entry_of_each_template.clone(),
            body: None,
            member: "templateArn",
            value: connector_arn.clone(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "ListTemplateGroupAccessControlEntries, a bare template id",
            method: "GET",
            uri: "/templates/{value}/accessControlEntries".to_string(),
            body: None,
            member: "templateArn",
            value: "66666666-7777-8888-9999-aaaaaaaaaaaa".to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "UpdateTemplateGroupAccessControlEntry, a word for a template ARN",
            method: "PATCH",
            uri: entry_of_each_template.clone(),
            body: Some(json!({ "GroupDisplayName": "Domain Admins" })),
            member: "templateArn",
            value: "not-a-valid-arn".to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "DeleteTemplateGroupAccessControlEntry, another vendor's ARN",
            method: "DELETE",
            uri: entry_of_each_template,
            body: None,
            member: "templateArn",
            value: "arn:aws:acm-pca:us-west-2:111122223333:certificate-authority/\
                    11111111-2222-3333-4444-555555555555"
                .to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "UpdateTemplate, a truncated template ARN",
            method: "PATCH",
            uri: "/templates/{value}".to_string(),
            body: Some(json!({ "ReenrollAllCertificateHolders": true })),
            member: "templateArn",
            value: "arn:aws:pca-connector-ad:us-west-2:111122223333:connector/\
                    11111111-2222-3333-4444-555555555555/template/"
                .to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "DeleteTemplate, a word for a template ARN",
            method: "DELETE",
            uri: "/templates/{value}".to_string(),
            body: None,
            member: "templateArn",
            value: "not-a-valid-arn".to_string(),
            pattern: TEMPLATE_ARN_PATTERN,
            accepted: template_arn.clone(),
        },
        PatternCase {
            case: "UpdateTemplateGroupAccessControlEntry, a word for a security identifier",
            method: "PATCH",
            uri: one_entry_of_template.clone(),
            body: Some(json!({ "GroupDisplayName": "Domain Admins" })),
            member: "groupSecurityIdentifier",
            value: "not-a-valid-sid".to_string(),
            pattern: GROUP_SID_PATTERN,
            accepted: WELL_FORMED_SID.to_string(),
        },
        PatternCase {
            case: "DeleteTemplateGroupAccessControlEntry, a UUID for a security identifier",
            method: "DELETE",
            uri: one_entry_of_template,
            body: None,
            member: "groupSecurityIdentifier",
            value: "11111111-2222-3333-4444-555555555555".to_string(),
            pattern: GROUP_SID_PATTERN,
            accepted: WELL_FORMED_SID.to_string(),
        },
        PatternCase {
            case: "CreateServicePrincipalName, a connector ARN for a directory registration",
            method: "POST",
            uri: name_of_each_registration,
            body: Some(json!({})),
            member: "directoryRegistrationArn",
            value: connector_arn.clone(),
            pattern: DIRECTORY_REGISTRATION_ARN_PATTERN,
            accepted: directory_registration_arn,
        },
        PatternCase {
            case: "GetServicePrincipalName, a directory id for a connector ARN",
            method: "GET",
            uri: one_name_of_registration,
            body: None,
            member: "connectorArn",
            value: "d-0123456789".to_string(),
            pattern: CONNECTOR_ARN_PATTERN,
            accepted: connector_arn.clone(),
        },
        PatternCase {
            case: "ListConnectors, a sentence for a pagination token",
            method: "GET",
            uri: "/connectors?NextToken={value}".to_string(),
            body: None,
            member: "nextToken",
            value: "this-is-not-a-real-pagination-token".to_string(),
            pattern: NEXT_TOKEN_PATTERN,
            accepted: WELL_FORMED_TOKEN.to_string(),
        },
        PatternCase {
            case: "ListTemplates, a template ARN for a connector ARN",
            method: "GET",
            uri: "/templates?ConnectorArn={value}".to_string(),
            body: None,
            member: "connectorArn",
            value: template_arn,
            pattern: CONNECTOR_ARN_PATTERN,
            accepted: connector_arn,
        },
    ]
}

/// The expression a refusal quotes for `pattern`: the modelled text without the
/// whole-value anchor pair the model wraps it in. Written here as the inverse of that
/// wrapping, so no row carries a hand-copied expression of its own.
pub(crate) fn advertised(pattern: &str) -> &str {
    pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or(pattern)
}

#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_without_the_models_whole_value_anchors() {
    let router = router();
    for row in pattern_cases() {
        let (status, code, parsed) =
            call(&router, row.method, &row.target(), row.body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {:?}: a value the member's pattern does not admit is refused with 400, \
             got {status} / {code:?} / {parsed}",
            row.case
        );
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?} / {parsed}",
            row.case
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             {PATTERN_REQUIREMENT}{}",
            row.value,
            row.member,
            advertised(row.pattern)
        );
        assert_eq!(
            parsed["message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected message {expected:?}, got {parsed}",
            row.case
        );
    }
}

/// The anchors go from the RENDERING: whatever the member, a stated requirement is
/// never the model's text and never carries an anchor at either end.
#[tokio::test]
async fn no_pattern_refusal_states_the_modelled_text_or_either_anchor() {
    let router = router();
    for row in pattern_cases() {
        let (_, _, parsed) = call(&router, row.method, &row.target(), row.body.as_ref()).await;
        let message = parsed["message"].as_str().unwrap_or_default().to_string();
        let quoted = message
            .rsplit(PATTERN_REQUIREMENT)
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            !quoted.is_empty() && !quoted.starts_with('^') && !quoted.ends_with('$'),
            "case {:?}: the quoted expression must carry neither anchor, got {message:?}",
            row.case
        );
        assert_ne!(
            quoted, row.pattern,
            "case {:?}: the message must not quote the modelled text verbatim, got {message:?}",
            row.case
        );
    }
}

/// Only the wording moved: the CHECK still runs the model's pattern as written, so a
/// value the expression admits is never refused for that pattern.
#[tokio::test]
async fn a_value_the_pattern_admits_is_never_refused_for_it() {
    let router = router();
    for row in pattern_cases() {
        let (status, code, parsed) =
            call(&router, row.method, &row.accepted_target(), row.body.as_ref()).await;
        let message = parsed["message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.contains(PATTERN_REQUIREMENT),
            "case {:?}: a value the pattern admits must not earn a pattern refusal, \
             got {status} / {code:?} / {message:?}",
            row.case
        );
    }
}
