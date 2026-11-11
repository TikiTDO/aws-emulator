// SPDX-License-Identifier: CC0-1.0

//! How this service states a value that fell outside a `@pattern` its model declares.
//!
//! The rule under test: a request refused by one of the model's own `@pattern` constraints is
//! answered HTTP 400 `ValidationException` whose lower-case `message` is one bracketed list
//! of the validation events the request produced — each event naming the expression the value
//! had to match and the string the request sent, and nothing else. No member name, no count,
//! and none of the "N validation error(s) detected" framing other AWS services use.
//!
//! The event text is fixed; its two variable parts come from the model and from the request,
//! so the rows below vary both: six members across five operations, each with its own
//! published pattern, each sent its own malformed value, in a path label, a body member and a
//! nested union member. The recorded `CreateConnector` refusal is ONE row.
//!
//! Every pattern below is quoted from the model (smithy_query shape=ConnectorArn,ChallengeArn,
//! CertificateAuthorityArn,VpcEndpointId,AzureApplicationId,AzureDomain) and every value is
//! inside that member's own `@length` bounds, so the one constraint each row breaks is the
//! pattern.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// The Region these cases are signed for.
const REGION: &str = "eu-west-2";

/// `ConnectorArn`'s published pattern (length 5..200).
const CONNECTOR_ARN_PATTERN: &str = r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// `ChallengeArn`'s published pattern (length 5..200).
const CHALLENGE_ARN_PATTERN: &str = r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// `CertificateAuthorityArn`'s published pattern (length 5..200).
const CERTIFICATE_AUTHORITY_ARN_PATTERN: &str = r"^arn:aws(-[a-z]+)*:acm-pca:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:certificate-authority\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// `VpcEndpointId`'s published pattern (length 13..22).
const VPC_ENDPOINT_ID_PATTERN: &str = r"^vpce-[0-9a-f]{8}([0-9a-f]{9})?$";

/// `AzureApplicationId`'s published pattern (length 15..100).
const AZURE_APPLICATION_ID_PATTERN: &str = r"^[a-zA-Z0-9]{2,15}-[a-zA-Z0-9]{2,15}-[a-zA-Z0-9]{2,15}-[a-zA-Z0-9]{2,15}-[a-zA-Z0-9]{2,15}$";

/// `AzureDomain`'s published pattern (length 1..256).
const AZURE_DOMAIN_PATTERN: &str = r"^[a-zA-Z0-9._-]+$";

/// A well-formed private-CA ARN, for the rows whose refusal is about another member.
const CA_ARN: &str =
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// A well-formed Azure application id, likewise.
const AZURE_APPLICATION_ID: &str = "0123456789ab-cdef-0123-4567-89abcdef0123";

/// The sentence the rule requires, spelled here rather than read back out of the service, so
/// a change to either side is a failure rather than a tautology.
fn expected_message(pattern: &str, value: &str) -> String {
    format!("[ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"]")
}

/// One row: what is malformed, the request that carries it, the published pattern it breaks
/// and the value it sends.
struct Row {
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    pattern: &'static str,
    value: &'static str,
}

fn rows() -> Vec<Row> {
    let bad_connector_arn = "not-an-arn";
    let bad_challenge_arn = "not-a-challenge-arn";
    vec![
        // The recorded row: a create whose referenced CA ARN is not an ARN.
        Row {
            name: "CreateConnector's CertificateAuthorityArn",
            method: "POST",
            path: "/connectors".to_string(),
            body: Some(json!({ "CertificateAuthorityArn": "not-an-arn" })),
            pattern: CERTIFICATE_AUTHORITY_ARN_PATTERN,
            value: "not-an-arn",
        },
        // A patterned member in the body of the same create, alongside a well-formed CA.
        Row {
            name: "CreateConnector's VpcEndpointId",
            method: "POST",
            path: "/connectors".to_string(),
            body: Some(json!({
                "CertificateAuthorityArn": CA_ARN,
                "VpcEndpointId": "vpce-zzzzzzzzz",
            })),
            pattern: VPC_ENDPOINT_ID_PATTERN,
            value: "vpce-zzzzzzzzz",
        },
        // A patterned member nested inside a union member of that create.
        Row {
            name: "CreateConnector's Intune AzureApplicationId",
            method: "POST",
            path: "/connectors".to_string(),
            body: Some(json!({
                "CertificateAuthorityArn": CA_ARN,
                "MobileDeviceManagement": {
                    "Intune": {
                        "AzureApplicationId": "not-an-application-id!!",
                        "Domain": "example.onmicrosoft.com",
                    }
                },
            })),
            pattern: AZURE_APPLICATION_ID_PATTERN,
            value: "not-an-application-id!!",
        },
        Row {
            name: "CreateConnector's Intune Domain",
            method: "POST",
            path: "/connectors".to_string(),
            body: Some(json!({
                "CertificateAuthorityArn": CA_ARN,
                "MobileDeviceManagement": {
                    "Intune": {
                        "AzureApplicationId": AZURE_APPLICATION_ID,
                        "Domain": "not a domain",
                    }
                },
            })),
            pattern: AZURE_DOMAIN_PATTERN,
            value: "not a domain",
        },
        // A path label rather than a body member, on each of the two ARN shapes.
        Row {
            name: "DeleteConnector's ConnectorArn label",
            method: "DELETE",
            path: format!("/connectors/{}", label(bad_connector_arn)),
            body: None,
            pattern: CONNECTOR_ARN_PATTERN,
            value: bad_connector_arn,
        },
        Row {
            name: "GetChallengePassword's ChallengeArn label",
            method: "GET",
            path: format!("/challengePasswords/{}", label(bad_challenge_arn)),
            body: None,
            pattern: CHALLENGE_ARN_PATTERN,
            value: bad_challenge_arn,
        },
        // And a body member of another operation, so the rule is not one operation's.
        Row {
            name: "CreateChallenge's ConnectorArn",
            method: "POST",
            path: "/challenges".to_string(),
            body: Some(json!({ "ConnectorArn": "not-an-arn" })),
            pattern: CONNECTOR_ARN_PATTERN,
            value: "not-an-arn",
        },
    ]
}

async fn send(router: &MultiServiceRouter, row: &Row) -> Answer {
    harness::call(router, REGION, row.method, &row.path, row.body.clone()).await
}

/// Every member the model gives a `@pattern`, in every position one appears in, is refused
/// with the one sentence the rule requires.
#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_the_way_this_service_words_it() {
    let (router, _engine) = new_router();
    for row in rows() {
        let case = row.name;
        let answer = send(&router, &row).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} / {:?}",
            answer.status,
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            expected_message(row.pattern, row.value),
            "case {case:?}: expected this service's own pattern refusal, got {message:?}"
        );
        // The two framings this service does NOT use: the count-and-member envelope other
        // AWS services answer with, and any mention of the member's own name.
        assert!(
            !message.contains("validation error"),
            "case {case:?}: the refusal must not use the count envelope, got {message:?}"
        );
        assert!(
            !message.to_lowercase().contains("arn'") && !message.contains("at '"),
            "case {case:?}: the refusal names no member, got {message:?}"
        );
    }
}

/// The refusal reports the value the REQUEST sent, so two requests breaking the same pattern
/// are worded differently — nothing here is a value this emulator remembered.
#[tokio::test]
async fn the_refusal_states_the_value_the_request_sent() {
    let (router, _engine) = new_router();
    // Every value is inside `CertificateAuthorityArn`'s own `@length(5, 200)`, so the one
    // constraint each breaks is the pattern: a value that also falls outside a length bound
    // is a refusal about two constraints, which this service's one-event wording does not
    // cover and the framework states instead.
    for value in [
        "xxxxx",
        "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/not-a-uuid",
        "ARN:AWS:ACM-PCA",
        "_____",
    ] {
        let answer = harness::call(
            &router,
            REGION,
            "POST",
            "/connectors",
            Some(json!({ "CertificateAuthorityArn": value })),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {value:?}: expected 400, got {} / {:?}",
            answer.status, answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            expected_message(CERTIFICATE_AUTHORITY_ARN_PATTERN, value),
            "case {value:?}: the refusal must state the value the request sent, got {message:?}"
        );
    }
}
