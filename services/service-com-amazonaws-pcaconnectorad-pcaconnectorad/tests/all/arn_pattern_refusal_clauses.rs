// SPDX-License-Identifier: CC0-1.0

//! The `@pattern` requirement a refusal states for this service's two ARN members, over
//! the connector and service-principal-name operations that take them in the URI — and
//! over a request that breaks BOTH of them at once.
//!
//! The rule is the sibling suite's ([`super::pattern_refusal_messages`]): a stated
//! `@pattern` requirement is the EXPRESSION a value has to satisfy, so the `^` / `$` the
//! model wraps it in — Smithy's way of turning a find into a whole-value match — are not
//! part of what the message quotes. This suite pins it where a request breaks the
//! requirement on MORE than one member, which is where the message's shape is widest: the
//! clause count and its agreeing count word, the `; ` that joins the clauses, and the
//! order the clauses come in (the order the operation's input declares the members).
//!
//! Every row drives the real wire path through [`super::harness`], so what refuses it is
//! the boundary validator and not a handler, and each row asserts the WHOLE message. The
//! rows span both ARN members, a path segment (`@httpLabel`) and a query parameter
//! (`@httpQuery`), every HTTP method these operations serve (GET, POST, DELETE), one and
//! two broken members in a single request, and several ways a value can miss its
//! expression (a word, another vendor's ARN, an ARN of the other resource type, a
//! truncated ARN, a bare resource id). Both expressions, the member bindings and the
//! member order are this service's own model
//! (`smithy_query(query="operation",operation="GetConnector,DeleteConnector,\
//! CreateServicePrincipalName,GetServicePrincipalName,ListServicePrincipalNames,\
//! DeleteServicePrincipalName,ListTemplates")`), and the expression each clause has to
//! state is COMPUTED from the modelled text rather than written out, so no row carries a
//! recorded string of its own.

use serde_json::{json, Value};

use super::harness::{call, encode_label, router};
use super::pattern_refusal_messages::{
    advertised, well_formed_connector_arn, well_formed_directory_registration_arn,
    CONNECTOR_ARN_PATTERN, DIRECTORY_REGISTRATION_ARN_PATTERN, PATTERN_REQUIREMENT,
};

/// One clause of a refusal: the member the walk names, the value it echoes back, and the
/// expression that member declares (anchors included — what the clause states is this
/// expression WITHOUT them).
struct Clause {
    member: &'static str,
    value: String,
    pattern: &'static str,
}

impl Clause {
    fn connector(value: &str) -> Self {
        Self {
            member: "connectorArn",
            value: value.to_string(),
            pattern: CONNECTOR_ARN_PATTERN,
        }
    }

    fn directory_registration(value: &str) -> Self {
        Self {
            member: "directoryRegistrationArn",
            value: value.to_string(),
            pattern: DIRECTORY_REGISTRATION_ARN_PATTERN,
        }
    }

    /// How this clause is stated: the member path, the echoed value and the expression.
    fn stated(&self) -> String {
        format!(
            "Value '{}' at '{}' failed to satisfy constraint: {PATTERN_REQUIREMENT}{}",
            self.value,
            self.member,
            advertised(self.pattern)
        )
    }
}

/// One refusal: a request whose URI carries one or two values a member's `@pattern` does
/// not admit.
struct ArnCase {
    /// Why this row is here: the operation, the members, and the way each value misses.
    case: &'static str,
    method: &'static str,
    /// The request target, already encoded, carrying the offending value(s).
    target: String,
    body: Option<Value>,
    /// The clauses the message states, in the order it states them.
    clauses: Vec<Clause>,
    /// The same request with every ARN one its own expression admits, so each row also
    /// pins that the check still runs the model's pattern as written.
    accepted_target: String,
}

impl ArnCase {
    /// The whole message this refusal has to state: the count, the count word agreeing
    /// with it, and the clauses joined by `; `.
    fn expected_message(&self) -> String {
        let count = self.clauses.len();
        let word = if count == 1 { "error" } else { "errors" };
        let clauses: Vec<String> = self.clauses.iter().map(Clause::stated).collect();
        format!(
            "{count} validation {word} detected: {}",
            clauses.join("; ")
        )
    }
}

/// A value no expression of this service admits, and the plainest way to miss one.
const A_WORD: &str = "not-a-valid-arn";

/// Another vendor's ARN: well formed, and not this service's resource.
const ANOTHER_VENDORS_ARN: &str = "arn:aws:acm-pca:us-west-2:111122223333:certificate-authority/\
                                   11111111-2222-3333-4444-555555555555";

/// A connector ARN with its resource id cut off.
const TRUNCATED_CONNECTOR_ARN: &str = "arn:aws:pca-connector-ad:us-west-2:111122223333:connector/";

/// A bare directory id, the resource id of a directory registration without its ARN.
const BARE_DIRECTORY_ID: &str = "d-0123456789";

/// A bare connector id, the resource id of a connector without its ARN.
const BARE_CONNECTOR_ID: &str = "11111111-2222-3333-4444-555555555555";

/// `/connectors/{ConnectorArn}` — `GetConnector` and `DeleteConnector`.
fn connector_target(value: &str) -> String {
    format!("/connectors/{}", encode_label(value))
}

/// `/directoryRegistrations/{DirectoryRegistrationArn}/servicePrincipalNames/{ConnectorArn}`
/// — `CreateServicePrincipalName`, `GetServicePrincipalName`, `DeleteServicePrincipalName`.
fn service_principal_name_target(registration: &str, connector: &str) -> String {
    format!(
        "/directoryRegistrations/{}/servicePrincipalNames/{}",
        encode_label(registration),
        encode_label(connector)
    )
}

/// `/directoryRegistrations/{DirectoryRegistrationArn}/servicePrincipalNames` —
/// `ListServicePrincipalNames`.
fn service_principal_names_target(registration: &str) -> String {
    format!(
        "/directoryRegistrations/{}/servicePrincipalNames",
        encode_label(registration)
    )
}

/// `/templates?ConnectorArn=…` — `ListTemplates`, whose connector ARN is a query
/// parameter rather than a path segment.
fn templates_target(connector: &str) -> String {
    format!("/templates?ConnectorArn={}", encode_label(connector))
}

/// The rows: the seven operations that take one or both ARN members in their URI, with
/// one broken member, the other broken member, and both broken at once.
fn arn_cases() -> Vec<ArnCase> {
    let connector = well_formed_connector_arn();
    let registration = well_formed_directory_registration_arn();
    let accepted_name = service_principal_name_target(&registration, &connector);
    vec![
        ArnCase {
            case: "GetConnector, a word for a connector ARN",
            method: "GET",
            target: connector_target(A_WORD),
            body: None,
            clauses: vec![Clause::connector(A_WORD)],
            accepted_target: connector_target(&connector),
        },
        ArnCase {
            case: "GetConnector, another vendor's ARN for a connector ARN",
            method: "GET",
            target: connector_target(ANOTHER_VENDORS_ARN),
            body: None,
            clauses: vec![Clause::connector(ANOTHER_VENDORS_ARN)],
            accepted_target: connector_target(&connector),
        },
        ArnCase {
            case: "DeleteConnector, a word for a connector ARN",
            method: "DELETE",
            target: connector_target(A_WORD),
            body: None,
            clauses: vec![Clause::connector(A_WORD)],
            accepted_target: connector_target(&connector),
        },
        ArnCase {
            case: "DeleteConnector, a bare connector id",
            method: "DELETE",
            target: connector_target(BARE_CONNECTOR_ID),
            body: None,
            clauses: vec![Clause::connector(BARE_CONNECTOR_ID)],
            accepted_target: connector_target(&connector),
        },
        ArnCase {
            case: "ListServicePrincipalNames, a word for a directory-registration ARN",
            method: "GET",
            target: service_principal_names_target(A_WORD),
            body: None,
            clauses: vec![Clause::directory_registration(A_WORD)],
            accepted_target: service_principal_names_target(&registration),
        },
        ArnCase {
            case: "ListServicePrincipalNames, a connector ARN for a directory-registration ARN",
            method: "GET",
            target: service_principal_names_target(&connector),
            body: None,
            clauses: vec![Clause::directory_registration(&connector)],
            accepted_target: service_principal_names_target(&registration),
        },
        ArnCase {
            case: "ListTemplates, a word for the connector ARN query parameter",
            method: "GET",
            target: templates_target(A_WORD),
            body: None,
            clauses: vec![Clause::connector(A_WORD)],
            accepted_target: templates_target(&connector),
        },
        ArnCase {
            case: "ListTemplates, a truncated connector ARN",
            method: "GET",
            target: templates_target(TRUNCATED_CONNECTOR_ARN),
            body: None,
            clauses: vec![Clause::connector(TRUNCATED_CONNECTOR_ARN)],
            accepted_target: templates_target(&connector),
        },
        ArnCase {
            case: "CreateServicePrincipalName, a word for the connector ARN alone",
            method: "POST",
            target: service_principal_name_target(&registration, A_WORD),
            body: Some(json!({})),
            clauses: vec![Clause::connector(A_WORD)],
            accepted_target: accepted_name.clone(),
        },
        ArnCase {
            case: "GetServicePrincipalName, a bare directory id for the registration ARN alone",
            method: "GET",
            target: service_principal_name_target(BARE_DIRECTORY_ID, &connector),
            body: None,
            clauses: vec![Clause::directory_registration(BARE_DIRECTORY_ID)],
            accepted_target: accepted_name.clone(),
        },
        ArnCase {
            case: "CreateServicePrincipalName, a word for both ARNs",
            method: "POST",
            target: service_principal_name_target(A_WORD, A_WORD),
            body: Some(json!({})),
            clauses: vec![
                Clause::directory_registration(A_WORD),
                Clause::connector(A_WORD),
            ],
            accepted_target: accepted_name.clone(),
        },
        ArnCase {
            case: "GetServicePrincipalName, the two ARNs swapped, so each misses the other's \
                   expression",
            method: "GET",
            target: service_principal_name_target(&connector, &registration),
            body: None,
            clauses: vec![
                Clause::directory_registration(&connector),
                Clause::connector(&registration),
            ],
            accepted_target: accepted_name.clone(),
        },
        ArnCase {
            case: "DeleteServicePrincipalName, a word for both ARNs",
            method: "DELETE",
            target: service_principal_name_target(A_WORD, A_WORD),
            body: None,
            clauses: vec![
                Clause::directory_registration(A_WORD),
                Clause::connector(A_WORD),
            ],
            accepted_target: accepted_name.clone(),
        },
        ArnCase {
            case: "DeleteServicePrincipalName, another vendor's ARN for both",
            method: "DELETE",
            target: service_principal_name_target(ANOTHER_VENDORS_ARN, ANOTHER_VENDORS_ARN),
            body: None,
            clauses: vec![
                Clause::directory_registration(ANOTHER_VENDORS_ARN),
                Clause::connector(ANOTHER_VENDORS_ARN),
            ],
            accepted_target: accepted_name,
        },
    ]
}

#[tokio::test]
async fn an_arn_pattern_refusal_states_every_clause_with_the_expression_unanchored() {
    let router = router();
    for row in arn_cases() {
        let (status, code, parsed) =
            call(&router, row.method, &row.target, row.body.as_ref()).await;
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
        let expected = row.expected_message();
        assert_eq!(
            parsed["message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected message {expected:?}, got {parsed}",
            row.case
        );
    }
}

/// The anchors go from the RENDERING: however many members a request breaks, no stated
/// expression is the model's text and none carries an anchor at either end.
#[tokio::test]
async fn no_clause_of_an_arn_refusal_states_the_modelled_text_or_either_anchor() {
    let router = router();
    for row in arn_cases() {
        let (_, _, parsed) = call(&router, row.method, &row.target, row.body.as_ref()).await;
        let message = parsed["message"].as_str().unwrap_or_default().to_string();
        let quoted: Vec<&str> = message
            .split(PATTERN_REQUIREMENT)
            .skip(1)
            .map(|tail| tail.split("; ").next().unwrap_or_default())
            .collect();
        assert_eq!(
            quoted.len(),
            row.clauses.len(),
            "case {:?}: expected {} stated expression(s), got {message:?}",
            row.case,
            row.clauses.len()
        );
        for (expression, clause) in quoted.iter().zip(&row.clauses) {
            assert!(
                !expression.is_empty()
                    && !expression.starts_with('^')
                    && !expression.ends_with('$'),
                "case {:?}: the expression stated for {:?} must carry neither anchor, \
                 got {message:?}",
                row.case,
                clause.member
            );
            assert_ne!(
                *expression, clause.pattern,
                "case {:?}: the clause for {:?} must not quote the modelled text verbatim, \
                 got {message:?}",
                row.case, clause.member
            );
        }
    }
}

/// Only the wording moved: the CHECK still runs the model's pattern as written, so a
/// request whose ARNs their expressions admit is never refused for those patterns.
#[tokio::test]
async fn arns_their_expressions_admit_are_never_refused_for_the_pattern() {
    let router = router();
    for row in arn_cases() {
        let (status, code, parsed) =
            call(&router, row.method, &row.accepted_target, row.body.as_ref()).await;
        let message = parsed["message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.contains(PATTERN_REQUIREMENT),
            "case {:?}: ARNs the patterns admit must not earn a pattern refusal, \
             got {status} / {code:?} / {message:?}",
            row.case
        );
    }
}
