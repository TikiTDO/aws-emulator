// SPDX-License-Identifier: CC0-1.0

//! The request preconditions a user-pool write and the app-client listing hold
//! callers to, over the wire, driven through the router exactly as an SDK drives
//! it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`) and
//! read back through the operations that report the stored configuration.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what
//! the rule generalises over rather than on one recorded request:
//!
//! * a pool that selects `phone_number` for auto-verification needs the SMS
//!   delivery configuration that verification is sent through, wherever that
//!   pairing is written (`CreateUserPool`, `UpdateUserPool`);
//! * an `AccountRecoverySetting` may not repeat a `Priority` or a mechanism
//!   `Name`, since a priority is a mechanism's rank among the pool's mechanisms;
//! * turning a pool's MFA on takes one of the three factors a challenge is
//!   presented over — SMS message, email message or a time-based one-time
//!   password — to be enabled by the same request (`SetUserPoolMfaConfig`);
//! * MFA is turned on when a pool is created or through `SetUserPoolMfaConfig`,
//!   not by an update of a created pool;
//! * a self-service password reset applies to the password the user OWNS, so a
//!   user that owns none is refused rather than sent a code (`ForgotPassword`);
//! * a pool that reserves user creation to administrators registers nobody
//!   through `SignUp`;
//! * the app-client listing is not held to the modelled MINIMUM of its page size:
//!   it serves such a request, with its `UserPoolClients` member present — empty
//!   when the page has nothing left to return.
//!
//! Every refused request leaves the pool exactly as it was, which each table
//! checks by reading the configuration back. Each row names the case it asserts
//! and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Amazon SNS caller a pool's SMS messages are sent through
/// (`SmsConfigurationType.SnsCallerArn`).
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/sms-caller";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The sentence a user-pool write is refused with when it selects `phone_number`
/// for auto-verification with no SMS delivery configuration.
const SMS_CONFIGURATION_REQUIRED: &str =
    "SMS configuration is required when phone_number is selected for auto verification";

/// The sentence a user-pool write is refused with when its
/// `AccountRecoverySetting` repeats a priority or a mechanism.
const DUPLICATE_RECOVERY_MECHANISMS: &str =
    "Invalid account recovery setting parameter. Account Recovery Setting cannot have \
     duplicate priorities or recovery mechanisms.";

/// The sentence `SetUserPoolMfaConfig` is refused with when it turns MFA on with
/// no factor enabled.
const MFA_METHOD_REQUIRED: &str =
    "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled.";

/// The sentence an update that would turn a created pool's MFA on is refused with.
const MFA_NOT_TURNED_ON_BY_UPDATE: &str =
    "Cannot turn MFA functionality ON, once the user pool has been created.";

/// The sentence a self-service reset is refused with for a user that owns no
/// password for a reset to apply to.
const PASSWORD_NOT_RESETTABLE: &str = "User password cannot be reset in the current state.";

/// The sentence a self-service registration is refused with on a pool that
/// reserves user creation to administrators.
const SIGN_UP_NOT_PERMITTED: &str = "SignUp is not permitted for this user pool";

/// `QueryLimit`'s modelled range maximum, the most app clients one page serves
/// (`smithy_query(query="operation",operation="ListUserPoolClients")`).
const MAX_RESULTS: usize = 60;

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
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Assert `payload` is the refusal `(code, message)` names, answered with
/// `status`.
fn assert_refusal(case: &str, status: u16, payload: &Value, code: &str, message: &str) {
    assert_eq!(
        (status, error_code(payload), error_message(payload)),
        (400, code, message),
        "case {case:?}: expected a 400 {code} carrying {message:?}, got {status} with {payload}"
    );
}

/// Assert the pool `stored` renders auto-verifies exactly `expected`. A pool that
/// auto-verifies nothing renders no member at all, which is that empty set.
fn assert_auto_verified(case: &str, stored: &Value, expected: &[&str]) {
    let rendered: Vec<String> = stored
        .get("AutoVerifiedAttributes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        rendered, expected,
        "case {case:?}: the pool must auto-verify {expected:?}, got {stored}"
    );
}

/// Create a pool from `body`, requiring success, and return its minted
/// `UserPoolId`.
async fn pool(router: &MultiServiceRouter, body: &Value) -> String {
    ok(router, "CreateUserPool", body).await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// A `CreateUserPool` body naming `pool_name` with `members` merged into it.
fn create_request(pool_name: &str, members: &Value) -> Value {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(pool_name));
    if let Some(extra) = members.as_object() {
        for (name, value) in extra {
            body.insert(name.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// An `UpdateUserPool` body for `user_pool_id` with `members` merged into it.
fn update_request(user_pool_id: &str, members: &Value) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(extra) = members.as_object() {
        for (name, value) in extra {
            body.insert(name.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// The pool `user_pool_id` names, as `DescribeUserPool` renders it.
async fn described(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// The MFA configuration `user_pool_id` holds, as `GetUserPoolMfaConfig` reports
/// it.
async fn mfa_config(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "GetUserPoolMfaConfig",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await
}

/// An app client of `user_pool_id` that permits the password sign-in this suite
/// uses, and its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

// ---------------------------------------------------------------------------
// Auto-verifying a phone number takes an SMS delivery configuration
// ---------------------------------------------------------------------------

/// One row: the `AutoVerifiedAttributes` a write asks for, whether it carries an
/// `SmsConfiguration`, and whether the write is served.
struct AutoVerifyCase {
    case: &'static str,
    /// `VerifiedAttributeType` values, which are `email` and `phone_number`
    /// (`smithy_query(query="shape",shape="VerifiedAttributeType")`).
    auto_verified: Vec<&'static str>,
    sms_configured: bool,
    served: bool,
}

/// The table. The rows differ along WHICH attributes are auto-verified (neither,
/// each one alone, both), the COUNT of them, and whether the SMS delivery
/// configuration the phone-number verification needs is part of the same write.
fn auto_verify_cases() -> Vec<AutoVerifyCase> {
    vec![
        AutoVerifyCase {
            case: "nothing auto-verified, no SMS configuration",
            auto_verified: Vec::new(),
            sms_configured: false,
            served: true,
        },
        AutoVerifyCase {
            case: "email alone, no SMS configuration",
            auto_verified: vec!["email"],
            sms_configured: false,
            served: true,
        },
        AutoVerifyCase {
            case: "phone_number alone, no SMS configuration",
            auto_verified: vec!["phone_number"],
            sms_configured: false,
            served: false,
        },
        AutoVerifyCase {
            case: "phone_number alone, with an SMS configuration",
            auto_verified: vec!["phone_number"],
            sms_configured: true,
            served: true,
        },
        AutoVerifyCase {
            case: "both attributes, no SMS configuration",
            auto_verified: vec!["email", "phone_number"],
            sms_configured: false,
            served: false,
        },
        AutoVerifyCase {
            case: "both attributes, with an SMS configuration",
            auto_verified: vec!["email", "phone_number"],
            sms_configured: true,
            served: true,
        },
        AutoVerifyCase {
            case: "an empty auto-verified list, no SMS configuration",
            auto_verified: Vec::new(),
            sms_configured: true,
            served: true,
        },
    ]
}

impl AutoVerifyCase {
    /// The members this row's write carries beyond the pool it names.
    fn members(&self) -> Value {
        let mut members = Map::new();
        members.insert(
            "AutoVerifiedAttributes".to_string(),
            json!(self.auto_verified),
        );
        if self.sms_configured {
            members.insert(
                "SmsConfiguration".to_string(),
                json!({ "SnsCallerArn": SNS_CALLER_ARN }),
            );
        }
        Value::Object(members)
    }
}

#[tokio::test]
async fn auto_verifying_a_phone_number_takes_an_sms_configuration_on_every_pool_write() {
    let router = router();

    for (index, case) in auto_verify_cases().iter().enumerate() {
        let label = case.case;
        let members = case.members();

        // The CREATE path: a refused create stores no pool at all.
        let pool_name = format!("auto-verify-create-{index}");
        let (status, payload) =
            call(&router, "CreateUserPool", &create_request(&pool_name, &members)).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: CreateUserPool must serve this configuration, got {status} with \
                 {payload}"
            );
            let user_pool_id = payload["UserPool"]["Id"].as_str().expect("a minted id");
            let stored = described(&router, user_pool_id).await;
            assert_auto_verified(label, &stored, &case.auto_verified);
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                SMS_CONFIGURATION_REQUIRED,
            );
            let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            let named = listed["UserPools"]
                .as_array()
                .expect("a listing of pools")
                .iter()
                .any(|entry| entry["Name"] == json!(pool_name));
            assert!(
                !named,
                "case {label:?}: a refused create stores no pool, got {listed}"
            );
        }

        // The UPDATE path, against a pool that auto-verifies the email address
        // alone and holds no SMS configuration: a refused update leaves exactly
        // that.
        let user_pool_id = pool(
            &router,
            &create_request(
                &format!("auto-verify-update-{index}"),
                &json!({ "AutoVerifiedAttributes": ["email"] }),
            ),
        )
        .await;
        let (status, payload) =
            call(&router, "UpdateUserPool", &update_request(&user_pool_id, &members)).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: UpdateUserPool must serve this configuration, got {status} with \
                 {payload}"
            );
            let stored = described(&router, &user_pool_id).await;
            assert_auto_verified(label, &stored, &case.auto_verified);
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                SMS_CONFIGURATION_REQUIRED,
            );
            let stored = described(&router, &user_pool_id).await;
            assert_auto_verified(label, &stored, &["email"]);
            assert!(
                stored.get("SmsConfiguration").is_none_or(Value::is_null),
                "case {label:?}: a refused update stores no SMS configuration, got {stored}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// An account-recovery setting may not repeat a priority or a mechanism
// ---------------------------------------------------------------------------

/// One row: the `RecoveryMechanisms` a write asks for, and whether it is served.
struct RecoveryCase {
    case: &'static str,
    mechanisms: Value,
    served: bool,
}

/// The table. The mechanism names are `RecoveryOptionNameType`'s three values
/// (`verified_email`, `verified_phone_number`, `admin_only`, from
/// `smithy_query(query="shape",shape="RecoveryOptionType")`) and the priorities
/// are inside `PriorityType`'s modelled range 1..2; the rows differ along which
/// half repeats — the priority, the name, or both — and along the orderings and
/// counts the modelled `@length` 1..2 admits, including `admin_only`, which the
/// recorded cases do not cite.
fn recovery_cases() -> Vec<RecoveryCase> {
    vec![
        RecoveryCase {
            case: "email first, phone second",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 2 }
            ]),
            served: true,
        },
        RecoveryCase {
            case: "phone first, email second",
            mechanisms: json!([
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 }
            ]),
            served: true,
        },
        RecoveryCase {
            case: "one mechanism alone, the modelled length minimum",
            mechanisms: json!([{ "Name": "verified_email", "Priority": 1 }]),
            served: true,
        },
        RecoveryCase {
            case: "admin-only recovery alone",
            mechanisms: json!([{ "Name": "admin_only", "Priority": 1 }]),
            served: true,
        },
        RecoveryCase {
            case: "two distinct mechanisms both at priority 1",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 1 }
            ]),
            served: false,
        },
        RecoveryCase {
            case: "two distinct mechanisms both at priority 2",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 2 },
                { "Name": "verified_phone_number", "Priority": 2 }
            ]),
            served: false,
        },
        RecoveryCase {
            case: "one mechanism named twice, at two priorities",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 }
            ]),
            served: false,
        },
        RecoveryCase {
            case: "one mechanism named twice at one priority",
            mechanisms: json!([
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 1 }
            ]),
            served: false,
        },
        RecoveryCase {
            case: "admin-only named twice",
            mechanisms: json!([
                { "Name": "admin_only", "Priority": 1 },
                { "Name": "admin_only", "Priority": 2 }
            ]),
            served: false,
        },
    ]
}

#[tokio::test]
async fn an_account_recovery_setting_may_not_repeat_a_priority_or_a_mechanism() {
    let router = router();
    // The configuration every update below starts from, so a refused update is
    // visible as the setting the pool keeps.
    let starting_mechanisms = json!([{ "Name": "verified_email", "Priority": 1 }]);

    for (index, case) in recovery_cases().iter().enumerate() {
        let label = case.case;
        let members = json!({ "AccountRecoverySetting": { "RecoveryMechanisms": case.mechanisms } });

        // The CREATE path.
        let pool_name = format!("recovery-create-{index}");
        let (status, payload) =
            call(&router, "CreateUserPool", &create_request(&pool_name, &members)).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: CreateUserPool must serve this setting, got {status} with \
                 {payload}"
            );
            let user_pool_id = payload["UserPool"]["Id"].as_str().expect("a minted id");
            let stored = described(&router, user_pool_id).await;
            assert_eq!(
                stored["AccountRecoverySetting"]["RecoveryMechanisms"], case.mechanisms,
                "case {label:?}: the stored pool carries the setting the create asked for, got \
                 {stored}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                DUPLICATE_RECOVERY_MECHANISMS,
            );
            let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            let named = listed["UserPools"]
                .as_array()
                .expect("a listing of pools")
                .iter()
                .any(|entry| entry["Name"] == json!(pool_name));
            assert!(
                !named,
                "case {label:?}: a refused create stores no pool, got {listed}"
            );
        }

        // The UPDATE path.
        let user_pool_id = pool(
            &router,
            &create_request(
                &format!("recovery-update-{index}"),
                &json!({
                    "AccountRecoverySetting": { "RecoveryMechanisms": starting_mechanisms }
                }),
            ),
        )
        .await;
        let (status, payload) =
            call(&router, "UpdateUserPool", &update_request(&user_pool_id, &members)).await;
        let stored = described(&router, &user_pool_id).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: UpdateUserPool must serve this setting, got {status} with \
                 {payload}"
            );
            assert_eq!(
                stored["AccountRecoverySetting"]["RecoveryMechanisms"], case.mechanisms,
                "case {label:?}: the updated pool carries the setting the update asked for, got \
                 {stored}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                DUPLICATE_RECOVERY_MECHANISMS,
            );
            assert_eq!(
                stored["AccountRecoverySetting"]["RecoveryMechanisms"], starting_mechanisms,
                "case {label:?}: a refused update leaves the pool's setting as it was, got \
                 {stored}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Turning MFA on takes a factor to challenge with
// ---------------------------------------------------------------------------

/// One row: the members a `SetUserPoolMfaConfig` request carries beyond
/// `UserPoolId`, and whether the pool takes them.
struct MfaCase {
    case: &'static str,
    members: Value,
    served: bool,
}

/// The table. The rows differ along the MFA mode (`UserPoolMfaType`'s OFF, ON and
/// OPTIONAL), WHICH factor the same request enables (none, each of the three, and
/// passkeys, which are not one of the three the refusal names), and whether a
/// configured factor is switched on — `SoftwareTokenMfaConfigType.Enabled` is
/// "the activation state of TOTP MFA" (this service's model), so a disabled TOTP
/// configuration enables nothing. Only `ON`, the mode under which "all users must
/// set up MFA before they can sign in" (`MfaConfiguration` in this service's
/// model), takes a factor: `OPTIONAL` leaves the determination to the application,
/// so it is served with no factor offered at all.
fn mfa_cases() -> Vec<MfaCase> {
    vec![
        MfaCase {
            case: "the pool id alone: MFA stays off",
            members: json!({}),
            served: true,
        },
        MfaCase {
            case: "MFA off with no factor",
            members: json!({ "MfaConfiguration": "OFF" }),
            served: true,
        },
        MfaCase {
            case: "MFA off with TOTP configured but disabled",
            members: json!({
                "MfaConfiguration": "OFF",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA required with no factor at all",
            members: json!({ "MfaConfiguration": "ON" }),
            served: false,
        },
        MfaCase {
            case: "MFA optional with no factor at all",
            members: json!({ "MfaConfiguration": "OPTIONAL" }),
            served: true,
        },
        MfaCase {
            case: "MFA required with TOTP configured but disabled",
            members: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: false,
        },
        MfaCase {
            case: "MFA optional with passkeys alone, which are not one of the three factors",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "WebAuthnConfiguration": {
                    "RelyingPartyId": "auth.example.com",
                    "UserVerification": "required",
                    "FactorConfiguration": "MULTI_FACTOR_WITH_USER_VERIFICATION"
                }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA optional with TOTP configured but disabled, the de-activating write",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "SoftwareTokenMfaConfiguration": { "Enabled": false }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA required with TOTP enabled",
            members: json!({
                "MfaConfiguration": "ON",
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA optional with email message MFA",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "EmailMfaConfiguration": { "Message": "Your code is {####}" }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA required with SMS message MFA",
            members: json!({
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": { "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN } }
            }),
            served: true,
        },
        MfaCase {
            case: "MFA optional with every factor the refusal names",
            members: json!({
                "MfaConfiguration": "OPTIONAL",
                "SmsMfaConfiguration": { "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN } },
                "EmailMfaConfiguration": { "Message": "Your code is {####}" },
                "SoftwareTokenMfaConfiguration": { "Enabled": true }
            }),
            served: true,
        },
    ]
}

#[tokio::test]
async fn turning_a_pools_mfa_on_takes_a_factor_to_challenge_with() {
    let router = router();

    for (index, case) in mfa_cases().iter().enumerate() {
        let label = case.case;
        let user_pool_id = pool(
            &router,
            &create_request(&format!("mfa-precondition-{index}"), &json!({})),
        )
        .await;
        // The pool's MFA configuration as the call found it, so a refusal is
        // visible as the configuration it left behind.
        let before = mfa_config(&router, &user_pool_id).await;

        let mut request = case.members.clone();
        request["UserPoolId"] = json!(user_pool_id);
        let (status, payload) = call(&router, "SetUserPoolMfaConfig", &request).await;

        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: SetUserPoolMfaConfig must serve this configuration, got {status} \
                 with {payload}"
            );
            let read = mfa_config(&router, &user_pool_id).await;
            assert_eq!(
                read, payload,
                "case {label:?}: the stored configuration must be what the call answered, got \
                 {read}"
            );
            let expected_mode = case
                .members
                .get("MfaConfiguration")
                .cloned()
                .unwrap_or_else(|| json!("OFF"));
            assert_eq!(
                read["MfaConfiguration"], expected_mode,
                "case {label:?}: the stored mode must be {expected_mode}, got {read}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                MFA_METHOD_REQUIRED,
            );
            let after = mfa_config(&router, &user_pool_id).await;
            assert_eq!(
                after, before,
                "case {label:?}: a refused set stores nothing, expected {before}, got {after}"
            );
            let described = described(&router, &user_pool_id).await;
            assert_eq!(
                described["MfaConfiguration"],
                json!("OFF"),
                "case {label:?}: the pool's own view keeps the mode it held, got {described}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// MFA is not turned on by an update of a created pool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_of_a_created_pool_cannot_turn_its_mfa_on() {
    let router = router();

    // The rows differ along the mode the pool was CREATED with and the mode the
    // update asks for, which is what decides whether the update would turn MFA on:
    // every `UserPoolMfaType` value is asked for from a pool that is off and from
    // one that is not.
    // (created mode, requested mode, served)
    let rows: &[(&str, Option<&str>, Option<&str>, bool)] = &[
        ("a pool that is off, asked to stay off", None, Some("OFF"), true),
        ("a pool that is off, asked for nothing", None, None, true),
        ("a pool that is off, asked for ON", None, Some("ON"), false),
        (
            "a pool that is off, asked for OPTIONAL",
            None,
            Some("OPTIONAL"),
            false,
        ),
        (
            "a pool created OPTIONAL, asked for OPTIONAL",
            Some("OPTIONAL"),
            Some("OPTIONAL"),
            true,
        ),
        (
            "a pool created OPTIONAL, asked for ON",
            Some("OPTIONAL"),
            Some("ON"),
            true,
        ),
        (
            "a pool created ON, asked for ON",
            Some("ON"),
            Some("ON"),
            true,
        ),
        (
            "a pool created ON, asked to turn MFA off",
            Some("ON"),
            Some("OFF"),
            true,
        ),
        (
            "a pool created ON, asked for nothing, which turns MFA off",
            Some("ON"),
            None,
            true,
        ),
    ];

    for (index, (label, created, requested, served)) in rows.iter().enumerate() {
        let mut create = Map::new();
        if let Some(mode) = created {
            create.insert("MfaConfiguration".to_string(), json!(mode));
        }
        let user_pool_id = pool(
            &router,
            &create_request(
                &format!("mfa-update-{index}"),
                &Value::Object(create),
            ),
        )
        .await;
        let mut members = Map::new();
        if let Some(mode) = requested {
            members.insert("MfaConfiguration".to_string(), json!(mode));
        }
        let (status, payload) = call(
            &router,
            "UpdateUserPool",
            &update_request(&user_pool_id, &Value::Object(members)),
        )
        .await;
        let stored = described(&router, &user_pool_id).await;

        if *served {
            assert_eq!(
                status, 200,
                "case {label:?}: the update must be served, got {status} with {payload}"
            );
            let expected = requested.unwrap_or("OFF");
            assert_eq!(
                stored["MfaConfiguration"],
                json!(expected),
                "case {label:?}: the pool must hold {expected:?} afterwards, got {stored}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                MFA_NOT_TURNED_ON_BY_UPDATE,
            );
            assert_eq!(
                stored["MfaConfiguration"],
                json!(created.unwrap_or("OFF")),
                "case {label:?}: a refused update leaves the pool's mode as it was, got {stored}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A self-service reset applies to a password the user owns
// ---------------------------------------------------------------------------

/// Which user state a `ForgotPassword` row asks a reset for.
#[derive(Clone, Copy, Debug)]
enum UserState {
    /// Created by an administrator and still holding the temporary password it
    /// must replace (`FORCE_CHANGE_PASSWORD`).
    ForceChangePassword,
    /// Registered through `SignUp` and not confirmed (`UNCONFIRMED`).
    Unconfirmed,
    /// Holding a password of its own (`CONFIRMED`).
    Confirmed,
    /// Holding a password of its own, with a reset already required of it
    /// (`RESET_REQUIRED`).
    ResetRequired,
}

#[tokio::test]
async fn a_self_service_reset_needs_a_password_the_user_owns() {
    let router = router();
    // The pool verifies the email address its users recover through and permits
    // self-service sign-up, so the UNCONFIRMED row can register itself.
    let user_pool_id = pool(
        &router,
        &create_request(
            "self-service-reset",
            &json!({
                "AutoVerifiedAttributes": ["email"],
                "AdminCreateUserConfig": { "AllowAdminCreateUserOnly": false }
            }),
        ),
    )
    .await;
    let client_id = client(&router, &user_pool_id, "reset-client").await;

    // (case, the user's state, the expected status for a reset of it)
    let rows: &[(&str, UserState, bool)] = &[
        (
            "a user still holding an administrator's temporary password",
            UserState::ForceChangePassword,
            false,
        ),
        (
            "a user that has not confirmed its own registration",
            UserState::Unconfirmed,
            false,
        ),
        (
            "a user holding a password of its own",
            UserState::Confirmed,
            true,
        ),
        (
            "a user a reset is already required of",
            UserState::ResetRequired,
            true,
        ),
    ];

    for (index, (label, state, resettable)) in rows.iter().enumerate() {
        let username = format!("reset-user-{index}");
        let address = format!("{username}@example.test");
        match state {
            UserState::Unconfirmed => {
                ok(
                    &router,
                    "SignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "Password": GOOD_PASSWORD,
                        "UserAttributes": [
                            { "Name": "email", "Value": address },
                            { "Name": "email_verified", "Value": "true" }
                        ]
                    }),
                )
                .await;
            }
            _ => {
                ok(
                    &router,
                    "AdminCreateUser",
                    &json!({
                        "UserPoolId": user_pool_id,
                        "Username": username,
                        "TemporaryPassword": GOOD_PASSWORD,
                        "UserAttributes": [
                            { "Name": "email", "Value": address },
                            { "Name": "email_verified", "Value": "true" }
                        ]
                    }),
                )
                .await;
            }
        }
        if matches!(state, UserState::Confirmed | UserState::ResetRequired) {
            ok(
                &router,
                "AdminSetUserPassword",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "Password": GOOD_PASSWORD,
                    "Permanent": true
                }),
            )
            .await;
        }
        if matches!(state, UserState::ResetRequired) {
            ok(
                &router,
                "AdminResetUserPassword",
                &json!({ "UserPoolId": user_pool_id, "Username": username }),
            )
            .await;
        }

        let (status, payload) = call(
            &router,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;

        if *resettable {
            assert_eq!(
                status, 200,
                "case {label:?}: the reset must be started, got {status} with {payload}"
            );
            assert_eq!(
                payload["CodeDeliveryDetails"]["AttributeName"],
                json!("email"),
                "case {label:?}: the code goes to the address the pool recovers through, got \
                 {payload}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "NotAuthorizedException",
                PASSWORD_NOT_RESETTABLE,
            );
        }

        // The refusal, and the start, leave the user's status to the operations
        // that own it: a refused reset moves nothing.
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        let expected_status = match state {
            UserState::ForceChangePassword => "FORCE_CHANGE_PASSWORD",
            UserState::Unconfirmed => "UNCONFIRMED",
            UserState::Confirmed => "CONFIRMED",
            UserState::ResetRequired => "RESET_REQUIRED",
        };
        assert_eq!(
            read["UserStatus"],
            json!(expected_status),
            "case {label:?}: the user must still be {expected_status}, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// SMS recovery does not qualify for a user with SMS MFA activated
// ---------------------------------------------------------------------------

/// RULE: "with this setting, SMS doesn't qualify for a valid password recovery
/// mechanism if the user also has SMS multi-factor authentication (MFA)
/// activated" (`AccountRecoverySettingType` in this service's model) — so the
/// mechanism that delivers over SMS is skipped for such a user and the pool falls
/// through to the next mechanism by priority, leaving a user with no other
/// qualifying mechanism with no destination at all.
///
/// The rows differ along which contact values the user holds verified and whether
/// SMS MFA is activated for it, over a pool that prefers the phone number.
#[tokio::test]
async fn sms_recovery_does_not_qualify_for_a_user_with_sms_mfa_activated() {
    let router = router();
    let user_pool_id = pool(
        &router,
        &create_request(
            "sms-mfa-recovery",
            &json!({
                // The phone number is the preferred mechanism, the email address
                // the fallback.
                "AccountRecoverySetting": {
                    "RecoveryMechanisms": [
                        { "Name": "verified_phone_number", "Priority": 1 },
                        { "Name": "verified_email", "Priority": 2 }
                    ]
                },
                "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN }
            }),
        ),
    )
    .await;
    let client_id = client(&router, &user_pool_id, "sms-mfa-recovery-client").await;

    // (case, holds a verified email, SMS MFA activated, the attribute a code goes
    // to, or None for a refusal)
    let rows: &[(&str, bool, bool, Option<&str>)] = &[
        (
            "a verified phone and email, no MFA: the preferred mechanism is used",
            true,
            false,
            Some("phone_number"),
        ),
        (
            "a verified phone and email, with SMS MFA: the reset falls to email",
            true,
            true,
            Some("email"),
        ),
        (
            "a verified phone alone, no MFA",
            false,
            false,
            Some("phone_number"),
        ),
        (
            "a verified phone alone, with SMS MFA: nothing qualifies",
            false,
            true,
            None,
        ),
    ];

    for (index, (label, verified_email, sms_mfa, expected)) in rows.iter().enumerate() {
        let username = format!("sms-mfa-user-{index}");
        let mut attributes = vec![
            json!({ "Name": "phone_number", "Value": format!("+1555010000{index}") }),
            json!({ "Name": "phone_number_verified", "Value": "true" }),
        ];
        if *verified_email {
            attributes.push(json!({ "Name": "email", "Value": format!("{username}@example.test") }));
            attributes.push(json!({ "Name": "email_verified", "Value": "true" }));
        }
        ok(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "TemporaryPassword": GOOD_PASSWORD,
                "UserAttributes": attributes
            }),
        )
        .await;
        ok(
            &router,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "Permanent": true
            }),
        )
        .await;
        if *sms_mfa {
            ok(
                &router,
                "AdminSetUserMFAPreference",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "SMSMfaSettings": { "Enabled": true, "PreferredMfa": true }
                }),
            )
            .await;
            let read = ok(
                &router,
                "AdminGetUser",
                &json!({ "UserPoolId": user_pool_id, "Username": username }),
            )
            .await;
            assert_eq!(
                read["UserMFASettingList"],
                json!(["SMS_MFA"]),
                "case {label:?}: the user must have SMS MFA activated, got {read}"
            );
        }

        let (status, payload) = call(
            &router,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;

        match expected {
            Some(attribute) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: the reset must be started, got {status} with {payload}"
                );
                assert_eq!(
                    payload["CodeDeliveryDetails"]["AttributeName"],
                    json!(attribute),
                    "case {label:?}: the code must go to {attribute:?}, got {payload}"
                );
            }
            None => assert_refusal(
                label,
                status,
                &payload,
                "InvalidParameterException",
                "Cannot reset password for the user as there is no registered/verified email or \
                 phone_number",
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// A pool that reserves user creation to administrators registers nobody
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pool_that_reserves_user_creation_to_administrators_registers_nobody() {
    let router = router();

    // (case, what the pool's AdminCreateUserConfig says, whether a sign-up registers)
    let rows: &[(&str, Value, bool)] = &[
        (
            "a pool that configures nothing, which permits self-service sign-up",
            json!({}),
            true,
        ),
        (
            "a pool that permits self-service sign-up explicitly",
            json!({ "AdminCreateUserConfig": { "AllowAdminCreateUserOnly": false } }),
            true,
        ),
        (
            "a pool that reserves user creation to administrators",
            json!({ "AdminCreateUserConfig": { "AllowAdminCreateUserOnly": true } }),
            false,
        ),
    ];

    for (index, (label, configuration, registers)) in rows.iter().enumerate() {
        let user_pool_id = pool(
            &router,
            &create_request(&format!("sign-up-gate-{index}"), configuration),
        )
        .await;
        let client_id = client(&router, &user_pool_id, "sign-up-client").await;
        let username = format!("registrant-{index}");

        let (status, payload) = call(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "UserAttributes": [{ "Name": "email", "Value": "registrant@example.test" }]
            }),
        )
        .await;

        let (read_status, read) = call(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;

        if *registers {
            assert_eq!(
                status, 200,
                "case {label:?}: the registration must be served, got {status} with {payload}"
            );
            assert_eq!(
                (read_status, read["Username"].clone()),
                (200, json!(username)),
                "case {label:?}: the registered user must be readable, got {read_status} with \
                 {read}"
            );
        } else {
            assert_refusal(
                label,
                status,
                &payload,
                "NotAuthorizedException",
                SIGN_UP_NOT_PERMITTED,
            );
            assert_eq!(
                (read_status, error_code(&read)),
                (400, "UserNotFoundException"),
                "case {label:?}: a refused registration stores no user, got {read_status} with \
                 {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The app-client listing serves a page size under the modelled minimum
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_app_client_listing_serves_a_page_size_under_the_modelled_minimum() {
    let router = router();
    let empty_pool = pool(&router, &create_request("list-clients-none", &json!({}))).await;
    let stocked_pool = pool(&router, &create_request("list-clients-three", &json!({}))).await;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..3 {
        stored.push(client(&router, &stocked_pool, &format!("app-{index}")).await);
    }
    stored.sort();

    // The rows span the values below `QueryLimit`'s modelled range minimum of 1 —
    // zero and a negative — against a pool that holds app clients and one that
    // holds none, beside the request that names no page size at all.
    for (label, user_pool_id, max_results, expected) in [
        (
            "no MaxResults at all, against a pool holding three clients",
            &stocked_pool,
            None,
            stored.clone(),
        ),
        (
            "MaxResults 0, one below the modelled minimum",
            &stocked_pool,
            Some(0),
            stored.clone(),
        ),
        (
            "MaxResults -1, below the modelled minimum",
            &stocked_pool,
            Some(-1),
            stored.clone(),
        ),
        (
            "MaxResults 0 against a pool holding no client",
            &empty_pool,
            Some(0),
            Vec::new(),
        ),
        (
            "no MaxResults against a pool holding no client",
            &empty_pool,
            None,
            Vec::new(),
        ),
    ] {
        let mut body = json!({ "UserPoolId": user_pool_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        let (status, payload) = call(&router, "ListUserPoolClients", &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the listing must be served, got {status} with {payload}"
        );
        let served = payload["UserPoolClients"].as_array().unwrap_or_else(|| {
            panic!("case {label:?}: UserPoolClients must be present, got {payload}")
        });
        assert!(
            served.len() <= MAX_RESULTS,
            "case {label:?}: a page holds at most {MAX_RESULTS} entries, got {}",
            served.len()
        );
        let ids: Vec<String> = served
            .iter()
            .map(|entry| {
                entry["ClientId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: an entry names a client: {entry}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids, expected,
            "case {label:?}: expected {expected:?}, got {ids:?}"
        );
    }

    // A page that has nothing left to return still SERIALIZES the member, empty,
    // and carries no token: a client walking the listing to exhaustion reads the
    // terminal page rather than a refusal.
    let first = ok(
        &router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": stocked_pool, "MaxResults": 3 }),
    )
    .await;
    assert_eq!(
        first["UserPoolClients"]
            .as_array()
            .expect("a listing serves its member")
            .len(),
        3,
        "the first page serves every stored client, got {first}"
    );
    let token = {
        // A token that resumes after the LAST stored client: the page it names
        // holds nothing, which is what a walk to exhaustion ends on.
        let page = ok(
            &router,
            "ListUserPoolClients",
            &json!({ "UserPoolId": stocked_pool, "MaxResults": 2 }),
        )
        .await;
        page["NextToken"]
            .as_str()
            .expect("a non-final page carries a NextToken")
            .to_string()
    };
    let last = ok(
        &router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": stocked_pool, "MaxResults": 2, "NextToken": token }),
    )
    .await;
    let remaining: Vec<String> = last["UserPoolClients"]
        .as_array()
        .expect("a resumed page serves its member")
        .iter()
        .map(|entry| entry["ClientId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec![stored[2].clone()],
        "the resumed page serves the one client left, got {last}"
    );
    assert!(
        last.get("NextToken").is_none_or(Value::is_null),
        "the final page carries no NextToken, got {last}"
    );

    // A page size ABOVE the modelled maximum is still refused: it asks for more
    // than one page of this listing holds.
    let (status, payload) = call(
        &router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": stocked_pool, "MaxResults": 61 }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "a page size past the modelled maximum is refused, got {status} with {payload}"
    );
}
