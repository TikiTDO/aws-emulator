// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCOUNT's CloudWatch role: which `cloudwatchRoleArn` an
//! `UpdateAccount` patch stores, and which one it refuses.
//!
//! THE RULE UNDER TEST: `cloudwatchRoleArn` is "the ARN of an Amazon CloudWatch role for the
//! current Account" (`Account$cloudwatchRoleArn`), and what API Gateway DOES with it is
//! ASSUME that role to write a stage's logs. So a value is stored only where the account's
//! IAM holds a role under the name it carries AND the trust policy that role carries lets
//! `apigateway.amazonaws.com` perform `sts:AssumeRole`; every other value names a role the
//! service could never use, and is refused `BadRequestException` (HTTP 400). A role ARN of
//! the documented form whose role DOES trust API Gateway is accepted and read back by
//! `GetAccount` — the inverse half of the same rule.
//!
//! Nothing is refused on evidence this emulator does not have: a value naming a role of
//! ANOTHER account is not this account's IAM to answer for, so it is stored.
//!
//! The roles are created through AWS IAM's OWN `CreateRole`, mounted on the same engine, so
//! the trust documents under test are the ones `iam:CreateRole` stores and the resolution
//! under test is the `GetRole` API Gateway makes. The rows span what the rule generalises
//! over: the trust document (one service principal, a list of them, another service's, an
//! explicit deny, another action entirely, none that can be read), the ARN's own form (the
//! documented form, a role under a path, another partition, another account, a non-role ARN,
//! text that is no ARN at all), and the patch itself (a value that clears the member).

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{assert_failure, gateway_with_iam, string_member, Gateway};

/// The account id every emulator ARN carries, which is the account whose roles this emulator
/// answers for.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The pointer an `UpdateAccount` patch names the CloudWatch role by.
pub(crate) const ROLE_POINTER: &str = "/cloudwatchRoleArn";

/// The sentence API Gateway refuses a role it cannot assume with.
pub(crate) const UNUSABLE_ROLE: &str = "The role ARN does not have required permissions configured. \
                             Please grant trust permission for API Gateway and add the \
                             required role policy.";

/// The sentence this service refuses a value that is no IAM role ARN with.
const INVALID_ARN: &str = "Invalid ARN specified in the request";

/// The error shape a refused `UpdateAccount` carries.
pub(crate) const BAD_REQUEST: &str = "BadRequestException";

/// A trust policy admitting `principal` to assume the role, as a service-principal trust
/// document is written.
pub(crate) fn trusting(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// `iam:CreateRole` for `name` under `path`, carrying `trust` as its
/// `AssumeRolePolicyDocument`.
pub(crate) async fn create_role(gateway: &Gateway, name: &str, path: &str, trust: &str) {
    let encoded = |value: &str| {
        ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
            .to_string()
    };
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&Path={}&AssumeRolePolicyDocument={}",
        encoded(name),
        encoded(path),
        encoded(trust)
    );
    let host = "iam.amazonaws.com";
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/iam/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the CreateRole request is well formed");
    let answer = gateway.send(request).await;
    assert_eq!(
        answer.status().as_u16(),
        200,
        "case CreateRole({name:?}): expected 200, got {} with {:?}",
        answer.status(),
        String::from_utf8_lossy(answer.body())
    );
}

/// `UpdateAccount` replacing the CloudWatch role with `value`.
pub(crate) async fn replace_role(
    gateway: &Gateway,
    value: &str,
) -> crate::rest_api_lifecycle::Answer {
    gateway
        .call(
            "PATCH",
            "/account",
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": ROLE_POINTER, "value": value },
                ],
            })),
        )
        .await
}

/// The `cloudwatchRoleArn` `GetAccount` reports, or `None` when it reports none.
pub(crate) async fn reported_role(gateway: &Gateway) -> Option<String> {
    let answer = gateway.call("GET", "/account", None).await;
    assert_eq!(
        answer.status, 200,
        "case GetAccount: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .get("cloudwatchRoleArn")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The ARN of a role of THIS account, under `path`.
pub(crate) fn role_arn(path: &str, name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role{path}{name}")
}

/// One row of [`only_a_role_api_gateway_can_assume_is_stored`]: a role to create (when the
/// case has one), the value the patch carries, and what the call earns.
struct RoleRow {
    case: &'static str,
    /// `(role name, path, trust document)` to create before the patch, or `None` for a case
    /// whose value names no role this emulator holds.
    created: Option<(&'static str, &'static str, String)>,
    /// The value the patch stores, built from the created role when there is one.
    value: String,
    /// The sentence the call is refused with, or `None` for a value that is stored.
    refused_with: Option<&'static str>,
}

/// RULE: a `cloudwatchRoleArn` is stored only where IAM holds that role of this account and
/// its trust policy lets `apigateway.amazonaws.com` assume it; every other value is
/// `BadRequestException`.
#[tokio::test]
async fn only_a_role_api_gateway_can_assume_is_stored() {
    let rows = vec![
        RoleRow {
            case: "a role whose trust policy names apigateway.amazonaws.com",
            created: Some((
                "apigw-logs",
                "/",
                trusting("apigateway.amazonaws.com"),
            )),
            value: role_arn("/", "apigw-logs"),
            refused_with: None,
        },
        RoleRow {
            case: "a role trusting apigateway among several service principals",
            created: Some((
                "apigw-shared",
                "/",
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {
                            "Service": ["lambda.amazonaws.com", "apigateway.amazonaws.com"],
                        },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            )),
            value: role_arn("/", "apigw-shared"),
            refused_with: None,
        },
        RoleRow {
            case: "a role under a path, which IAM holds under its own name",
            created: Some((
                "apigw-pathed",
                "/service-role/",
                trusting("apigateway.amazonaws.com"),
            )),
            value: role_arn("/service-role/", "apigw-pathed"),
            refused_with: None,
        },
        RoleRow {
            case: "the same role named in another partition, which names the same role",
            created: Some((
                "apigw-partitioned",
                "/",
                trusting("apigateway.amazonaws.com"),
            )),
            value: format!("arn:aws-cn:iam::{ACCOUNT}:role/apigw-partitioned"),
            refused_with: None,
        },
        RoleRow {
            case: "a role whose trust policy names another service entirely",
            created: Some(("lambda-only", "/", trusting("lambda.amazonaws.com"))),
            value: role_arn("/", "lambda-only"),
            refused_with: Some(UNUSABLE_ROLE),
        },
        RoleRow {
            case: "a role whose trust policy DENIES apigateway the assume",
            created: Some((
                "apigw-denied",
                "/",
                trusting("apigateway.amazonaws.com").replace("Allow", "Deny"),
            )),
            value: role_arn("/", "apigw-denied"),
            refused_with: Some(UNUSABLE_ROLE),
        },
        RoleRow {
            case: "a role whose trust policy grants apigateway another action",
            created: Some((
                "apigw-tagging",
                "/",
                trusting("apigateway.amazonaws.com").replace("sts:AssumeRole", "sts:TagSession"),
            )),
            value: role_arn("/", "apigw-tagging"),
            refused_with: Some(UNUSABLE_ROLE),
        },
        RoleRow {
            case: "an ARN naming a role this account's IAM holds nothing under",
            created: None,
            value: role_arn("/", "never-created"),
            refused_with: Some(UNUSABLE_ROLE),
        },
        RoleRow {
            case: "a role of ANOTHER account, which this emulator's IAM cannot answer for",
            created: None,
            value: "arn:aws:iam::210987654321:role/partner-logs".to_string(),
            refused_with: None,
        },
        RoleRow {
            case: "an ARN that names no IAM role at all",
            created: None,
            value: "arn:aws:s3:::access-logs".to_string(),
            refused_with: Some(INVALID_ARN),
        },
        RoleRow {
            case: "an IAM ARN naming another entity type",
            created: None,
            value: format!("arn:aws:iam::{ACCOUNT}:user/operator"),
            refused_with: Some(INVALID_ARN),
        },
        RoleRow {
            case: "text that is no ARN at all",
            created: None,
            value: "apigw-logs".to_string(),
            refused_with: Some(INVALID_ARN),
        },
    ];

    for row in &rows {
        // One emulator per row, so what a row stores cannot decide the next row's answer.
        let gateway = gateway_with_iam();
        if let Some((name, path, trust)) = &row.created {
            create_role(&gateway, name, path, trust).await;
        }
        let case = row.case;
        let answer = replace_role(&gateway, &row.value).await;
        match row.refused_with {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the role to be stored, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    string_member(&answer.body, "cloudwatchRoleArn", case),
                    row.value,
                    "case {case:?}: expected the stored role to be reported"
                );
                // The round trip: what the update stored is what the account reads back.
                assert_eq!(
                    reported_role(&gateway).await.as_deref(),
                    Some(row.value.as_str()),
                    "case {case:?}: expected GetAccount to report the stored role"
                );
            }
            Some(sentence) => {
                assert_failure(&answer, 400, BAD_REQUEST, case);
                assert_eq!(
                    answer
                        .body
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    sentence,
                    "case {case:?}: expected the refusal {sentence:?}, got {:?}",
                    answer.body
                );
                // A refused patch leaves the account exactly as it was.
                assert_eq!(
                    reported_role(&gateway).await,
                    None,
                    "case {case:?}: a refused patch must store nothing"
                );
            }
        }
    }
}

/// RULE: an EMPTY value names no role at all — it clears the member — so it is stored
/// whatever IAM holds, and the account then reports no `cloudwatchRoleArn`.
#[tokio::test]
async fn an_empty_value_clears_the_account_role() {
    let gateway = gateway_with_iam();
    create_role(
        &gateway,
        "apigw-cleared",
        "/",
        &trusting("apigateway.amazonaws.com"),
    )
    .await;
    let arn = role_arn("/", "apigw-cleared");
    let stored = replace_role(&gateway, &arn).await;
    assert_eq!(
        stored.status, 200,
        "expected the role to be stored, got {} with {:?}",
        stored.status, stored.body
    );
    assert_eq!(
        reported_role(&gateway).await.as_deref(),
        Some(arn.as_str()),
        "expected the stored role to be read back before it is cleared"
    );

    let cleared = replace_role(&gateway, "").await;
    assert_eq!(
        cleared.status, 200,
        "expected the empty value to clear the role, got {} with {:?}",
        cleared.status, cleared.body
    );
    assert_eq!(
        reported_role(&gateway).await,
        None,
        "expected the account to report no cloudwatchRoleArn once it is cleared"
    );
}
