// SPDX-License-Identifier: CC0-1.0

//! A pool's MFA posture is re-writable in BOTH directions: `SetUserPoolMfaConfig`
//! stores whatever legal combination of `MfaConfiguration` and the per-factor
//! configuration blocks it is given, and `GetUserPoolMfaConfig` projects the last
//! stored one.
//!
//! The rule under test: the pool's MFA mode and its per-factor configuration are
//! independent except where the mode makes a factor indispensable — only
//! `MfaConfiguration` = `ON`, under which "all users must set up MFA before they
//! can sign in" (`MfaConfiguration` in this service's model), takes at least one
//! enabled factor, because such a pool would otherwise demand a challenge it could
//! never present. `OPTIONAL` leaves the determination to the application and `OFF`
//! asks for no challenge at all, so for either of them every combination — each
//! factor absent, or configured and de-activated — is accepted, stored as sent and
//! echoed back by both the write and the following read.
//!
//! Each row names the case it asserts and every assertion reports the actual value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Amazon SNS caller role an SMS MFA factor is delivered through.
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/sms-caller";

/// The refusal a pool that REQUIRES MFA with no factor enabled answers.
const MFA_METHOD_REQUIRED: &str =
    "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled.";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, expected 200, got {status} with {payload}"
    );
    payload
}

/// A freshly created pool with no MFA and no SMS settings of its own, and its id.
async fn pool(router: &MultiServiceRouter, case: &str) -> String {
    let created = ok(
        router,
        case,
        "CreateUserPool",
        &json!({ "PoolName": "mfa-posture-pool" }),
    )
    .await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {created}"))
        .to_string()
}

/// `members` with `user_pool_id` added, as a `SetUserPoolMfaConfig` request.
fn set_request(user_pool_id: &str, members: &Value) -> Value {
    let mut request = members.clone();
    request["UserPoolId"] = json!(user_pool_id);
    request
}

/// The configuration `GetUserPoolMfaConfig` reports for `user_pool_id`.
async fn read_back(router: &MultiServiceRouter, case: &str, user_pool_id: &str) -> Value {
    ok(
        router,
        case,
        "GetUserPoolMfaConfig",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await
}

/// One row of the rewrite table: a configuration to apply first, the rewrite that
/// follows it, and the configuration the pool must report afterwards.
struct RewriteCase {
    case: &'static str,
    first: Value,
    rewrite: Value,
    expected: Value,
}

/// The table. Rows differ along WHICH factor is de-activated, HOW it is
/// de-activated (switched off in place, or left out of the rewrite), how many
/// factors the pool held before it, and the mode the rewrite leaves the pool in.
fn rewrite_cases() -> Vec<RewriteCase> {
    vec![
        RewriteCase {
            // The de-activation `SoftwareTokenMfaConfigType.Enabled` exists for:
            // it is "the activation state of TOTP MFA" (this service's model), so
            // false is as storable a value as true.
            case: "TOTP switched off in place while MFA stays optional",
            first: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            rewrite: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
        },
        RewriteCase {
            case: "TOTP left out of the rewrite, so the pool offers no factor at all",
            first: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            rewrite: json!({ "MfaConfiguration": "OPTIONAL" }),
            expected: json!({ "MfaConfiguration": "OPTIONAL" }),
        },
        RewriteCase {
            case: "email OTP replaced by a de-activated TOTP configuration",
            first: json!({
                "MfaConfiguration": "OPTIONAL",
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
            rewrite: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
        },
        RewriteCase {
            // Three factors down to none in one call. The pool's own SMS settings
            // are not MFA-only — `SmsConfiguration` is settable "in
            // `CreateUserPool` and `UpdateUserPool`, or in `SetUserPoolMfaConfig`"
            // (this service's model) — so the stored SNS caller survives a rewrite
            // that says nothing about it, and the SMS view still renders it.
            case: "every factor de-activated at once while MFA stays optional",
            first: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": true },
                "EmailMfaConfiguration": {
                    "Message": "Your code is {####}",
                    "Subject": "Your sign-in code"
                },
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
                }
            }),
            rewrite: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            expected: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false },
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
                }
            }),
        },
        RewriteCase {
            // The mode moves too: `UserPoolMfaType` admits OFF | ON | OPTIONAL
            // (this service's model) and a rewrite may land on any of them.
            case: "TOTP switched off and the mode turned off with it",
            first: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            rewrite: json!({
                "MfaConfiguration": "OFF",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            expected: json!({
                "MfaConfiguration": "OFF",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
        },
        RewriteCase {
            case: "a required pool relaxed to optional with its only factor switched off",
            first: json!({
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
            rewrite: json!({ "MfaConfiguration": "OPTIONAL" }),
            expected: json!({ "MfaConfiguration": "OPTIONAL" }),
        },
    ]
}

#[tokio::test]
async fn a_factor_can_be_de_activated_and_the_read_follows() {
    let router = router();
    for case in rewrite_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, label).await;

        ok(
            &router,
            label,
            "SetUserPoolMfaConfig",
            &set_request(&user_pool_id, &case.first),
        )
        .await;

        let (status, answered) = call(
            &router,
            "SetUserPoolMfaConfig",
            &set_request(&user_pool_id, &case.rewrite),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the rewrite must be accepted, expected 200, got {status} with \
             {answered}"
        );
        assert_eq!(
            answered, case.expected,
            "case {label:?}: expected the write to echo {}, got {answered}",
            case.expected
        );

        let read = read_back(&router, label, &user_pool_id).await;
        assert_eq!(
            read, case.expected,
            "case {label:?}: expected the read to project {}, got {read}",
            case.expected
        );
    }
}

#[tokio::test]
async fn the_activation_state_of_a_factor_round_trips_both_ways() {
    let router = router();
    let case = "TOTP toggled on, off, on and off again";
    let user_pool_id = pool(&router, case).await;

    // The activation state is a value stored as sent, so the pool follows every
    // step of a sequence rather than latching on the first one that switched it on.
    for enabled in [true, false, true, false, false, true] {
        let answered = ok(
            &router,
            case,
            "SetUserPoolMfaConfig",
            &set_request(
                &user_pool_id,
                &json!({
                    "MfaConfiguration": "OPTIONAL",
                    "SoftwareTokenMfaConfiguration": { "Enabled": enabled }
                }),
            ),
        )
        .await;
        let expected = json!({
            "MfaConfiguration": "OPTIONAL",
            "SoftwareTokenMfaConfiguration": { "Enabled": enabled }
        });
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the write to echo {expected}, got {answered}"
        );
        let read = read_back(&router, case, &user_pool_id).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected the read to project {expected}, got {read}"
        );
    }
}

/// One row of the mode table: the members a `SetUserPoolMfaConfig` request carries
/// beyond `UserPoolId` and whether the pool takes them.
struct ModeCase {
    case: &'static str,
    members: Value,
    served: bool,
}

/// The table. Rows differ along the mode (`UserPoolMfaType`'s OFF, ON and OPTIONAL)
/// and what the same request offers to challenge with: nothing, a factor switched
/// off, passkeys (which are not one of the three factors an MFA challenge is
/// presented over), and each of the three factors switched on.
fn mode_cases() -> Vec<ModeCase> {
    vec![
        ModeCase {
            case: "optional with nothing offered",
            members: json!({ "MfaConfiguration": "OPTIONAL" }),
            served: true,
        },
        ModeCase {
            case: "optional with TOTP switched off",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: true,
        },
        ModeCase {
            case: "optional with passkeys alone",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "WebAuthnConfiguration": { "RelyingPartyId": "auth.example.com" }
            }),
            served: true,
        },
        ModeCase {
            case: "optional with TOTP switched on",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            served: true,
        },
        ModeCase {
            case: "off with nothing offered",
            members: json!({ "MfaConfiguration": "OFF" }),
            served: true,
        },
        ModeCase {
            case: "off with TOTP switched off",
            members: json!({
                "MfaConfiguration": "OFF",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: true,
        },
        ModeCase {
            case: "required with nothing offered",
            members: json!({ "MfaConfiguration": "ON" }),
            served: false,
        },
        ModeCase {
            case: "required with TOTP switched off",
            members: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: false,
        },
        ModeCase {
            case: "required with passkeys alone",
            members: json!({
                "MfaConfiguration": "ON",
                "WebAuthnConfiguration": { "RelyingPartyId": "auth.example.com" }
            }),
            served: false,
        },
        ModeCase {
            case: "required with TOTP switched on",
            members: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            served: true,
        },
        ModeCase {
            case: "required with email OTP",
            members: json!({
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
            served: true,
        },
        ModeCase {
            case: "required with SMS message MFA",
            members: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": {
                    "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
                }
            }),
            served: true,
        },
    ]
}

#[tokio::test]
async fn only_a_pool_that_requires_mfa_takes_an_enabled_factor() {
    let router = router();
    for case in mode_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, label).await;
        let before = read_back(&router, label, &user_pool_id).await;

        let (status, payload) = call(
            &router,
            "SetUserPoolMfaConfig",
            &set_request(&user_pool_id, &case.members),
        )
        .await;

        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} with {payload}"
            );
            let expected_mode = case.members["MfaConfiguration"].clone();
            assert_eq!(
                payload["MfaConfiguration"], expected_mode,
                "case {label:?}: expected the stored mode {expected_mode}, got {payload}"
            );
            let read = read_back(&router, label, &user_pool_id).await;
            assert_eq!(
                read, payload,
                "case {label:?}: expected the read to project {payload}, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status} with {payload}"
            );
            assert_eq!(
                payload.get("__type").and_then(Value::as_str),
                Some("InvalidParameterException"),
                "case {label:?}: expected InvalidParameterException, got {payload}"
            );
            assert_eq!(
                payload
                    .get("message")
                    .or_else(|| payload.get("Message"))
                    .and_then(Value::as_str),
                Some(MFA_METHOD_REQUIRED),
                "case {label:?}: expected the refusal to name the three factors, got {payload}"
            );
            let after = read_back(&router, label, &user_pool_id).await;
            assert_eq!(
                after, before,
                "case {label:?}: a refused write stores nothing, expected {before}, got {after}"
            );
        }
    }
}
