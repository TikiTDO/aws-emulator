// SPDX-License-Identifier: CC0-1.0

//! Which sentence a `UserNotFoundException` carries, over the wire.
//!
//! The rule under test: an operation that resolves the user from the `Username`
//! and the `ClientId` of the request TOGETHER — the public sign-up and
//! password-recovery operations, whose input models a required `Username` beside
//! a required `ClientId` and no `UserPoolId`
//! (`smithy_query(query="operation",operation="ConfirmSignUp,ConfirmForgotPassword,ForgotPassword,ResendConfirmationCode")`)
//! — reports a username that resolves to no user as that COMBINATION not being
//! found, while an operation that resolves the user against a pool the request
//! names, or against an access token, reports that the user does not exist. Both
//! sentences are fixed: neither quotes the username, the client id or the pool,
//! so the refusal is not an answer about one particular identity.
//!
//! The rows differ along the username asked for (spanning what `UsernameType`
//! admits), the operation asked, and the app client's
//! `PreventUserExistenceErrors` mode, which changes the error CODE and leaves the
//! sentence alone. The served path of each of the four operations is exercised
//! too, so the sentence is the answer to an absent user and not to every call.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target`
/// prefix a client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Another password satisfying it, for the reset half of the recovery flow.
const RESET_PASSWORD: &str = "Res3tPass!2024";

/// The one sentence an operation that resolves the user from `Username` and
/// `ClientId` together answers for a pair it cannot resolve.
const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// The one sentence an operation that resolves the user against a pool it names,
/// or against an access token, answers for a username it cannot resolve.
const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// The sentence a confirmation carrying a code nothing outstanding matches is
/// refused with: the remediation is to ask for a code.
const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// A `ConfirmationCodeType` value (len 1..2048, pattern `^[\S]+$`) that no code
/// this emulator minted can be, so a confirmation carrying it resolves nothing.
const UNISSUED_CODE: &str = "000000";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the public
    /// operations under test are called.
    Anonymous,
}

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return
/// the status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match addressing {
        Addressing::Signed => ENDPOINT_HOST,
        Addressing::Anonymous => EMULATOR_HOST,
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if matches!(addressing, Addressing::Signed) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Signed, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation` unsigned, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Anonymous, operation, body).await;
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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool with `extra` merged into its configuration and return its id.
async fn pool(router: &MultiServiceRouter, pool_name: &str, extra: Value) -> String {
    let mut body = json!({ "PoolName": pool_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// Create an app client of `user_pool_id` with `extra` merged into its
/// configuration, and return its minted id.
async fn app_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    extra: Value,
) -> String {
    let mut body = json!({ "UserPoolId": user_pool_id, "ClientName": client_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPoolClient", &body).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// The code outstanding for `purpose` on `username`, read out of the emulator's
/// own state: the real service delivers a code out of band, so no response
/// carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: state::CodePurpose,
) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&purpose))
        .map(|code| code.code.clone())
        .unwrap_or_else(|| panic!("a code must be outstanding for {username}"))
}

/// The four public operations under test, each asked about `username` through
/// `client_id`: (operation, request body).
fn public_requests(client_id: &str, username: &str) -> [(&'static str, Value); 4] {
    [
        (
            "ConfirmSignUp",
            json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": UNISSUED_CODE,
            }),
        ),
        (
            "ResendConfirmationCode",
            json!({ "ClientId": client_id, "Username": username }),
        ),
        (
            "ForgotPassword",
            json!({ "ClientId": client_id, "Username": username }),
        ),
        (
            "ConfirmForgotPassword",
            json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": UNISSUED_CODE,
                "Password": GOOD_PASSWORD,
            }),
        ),
    ]
}

#[tokio::test]
async fn an_absent_user_of_a_public_operation_is_refused_as_an_unresolved_pair() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(
        &setup,
        "public-absent-user",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    // The app client leaves `PreventUserExistenceErrors` unset, so it keeps the
    // documented `LEGACY` default: the mode that answers `UserNotFoundException`
    // for a user nobody registered. `ALLOW_USER_PASSWORD_AUTH` is what lets the
    // contrast row below reach `InitiateAuth`'s own user lookup.
    let open_client = app_client(
        &setup,
        &user_pool_id,
        "open",
        json!({ "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"] }),
    )
    .await;
    let hiding_client = app_client(
        &setup,
        &user_pool_id,
        "hiding",
        json!({
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"],
            "PreventUserExistenceErrors": "ENABLED",
        }),
    )
    .await;

    // A user that DOES exist, so every lookup below fails on the name it is
    // given rather than on an empty pool.
    ok_anonymous(
        &emulator,
        "SignUp",
        &json!({
            "ClientId": open_client,
            "Username": "present",
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": "present@example.test" }],
        }),
    )
    .await;

    // The names span what `UsernameType` admits (length 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
    // `smithy_query(query="shape",shape="UsernameType")`): both published length
    // bounds, a non-ASCII name, an email-shaped name, a name shaped like the
    // generated `sub` that `Username` also accepts, and the stored name re-cased,
    // which names a different user in a pool that compares case-sensitively.
    let longest = "u".repeat(128);
    let usernames = [
        ("a plain name", "absent-user".to_string()),
        ("the 128-character length maximum", longest),
        ("the one-character length minimum", "z".to_string()),
        ("a non-ASCII name", "ユーザー-不在①".to_string()),
        ("an email-shaped name", "absent@example.test".to_string()),
        (
            "a name shaped like a generated sub",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("the stored name re-cased", "PRESENT".to_string()),
    ];

    for (name_case, username) in &usernames {
        for (operation, body) in public_requests(&open_client, username) {
            let case = format!("{operation} with {name_case}");
            let (status, payload) =
                dispatch(&emulator, Addressing::Anonymous, operation, &body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: expected a 400 UserNotFoundException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, USERNAME_CLIENT_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {USERNAME_CLIENT_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(username.as_str()) && !message.contains(open_client.as_str()),
                "case {case:?}: the refusal must quote neither the username nor the client id, \
                 got {message:?}"
            );
        }

        // The same pair through a client that suppresses user-existence errors:
        // an operation that REPORTS A DELIVERY answers one, to a simulated
        // destination, so nothing about the answer tells the caller whether the
        // account exists; for the operations that report no delivery the refusal
        // is the one a user that DOES exist earns at the same point of that
        // operation's own validation order, so it discloses nothing either.
        for (operation, body) in public_requests(&hiding_client, username) {
            let case = format!("{operation} with {name_case}, through the hiding client");
            let (status, payload) =
                dispatch(&emulator, Addressing::Anonymous, operation, &body).await;
            if matches!(operation, "ResendConfirmationCode" | "ForgotPassword") {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a reported delivery, got {status} with {payload}"
                );
                assert_eq!(
                    payload["CodeDeliveryDetails"]["Destination"],
                    json!("a***@b***"),
                    "case {case:?}: expected a simulated destination, got {payload}"
                );
                continue;
            }
            // `ConfirmForgotPassword` checks the user and then the outstanding
            // reset code, and a user that exists with no code outstanding is
            // refused with the code refusal — so the masked refusal of an
            // unknown username is that same refusal, not one of its own class.
            let (expected_code, expected_message) = if operation == "ConfirmForgotPassword" {
                ("ExpiredCodeException", EXPIRED_CODE_MESSAGE)
            } else {
                ("NotAuthorizedException", USERNAME_CLIENT_NOT_FOUND_MESSAGE)
            };
            assert_eq!(
                (status, error_code(&payload)),
                (400, expected_code),
                "case {case:?}: expected a 400 {expected_code}, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
            assert!(
                !message.contains(username.as_str()) && !message.contains(hiding_client.as_str()),
                "case {case:?}: the refusal must quote neither the username nor the client id, \
                 got {message:?}"
            );
        }

        // The CONTRAST: an operation that resolves the user against the pool it
        // names, and one that resolves it from `AuthParameters`, keep the
        // sentence that reports the user alone.
        let pool_scoped: [(&str, &MultiServiceRouter, Addressing, Value); 2] = [
            (
                "AdminGetUser",
                &setup,
                Addressing::Signed,
                json!({ "UserPoolId": user_pool_id, "Username": username }),
            ),
            (
                "InitiateAuth",
                &emulator,
                Addressing::Anonymous,
                json!({
                    "ClientId": open_client,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            ),
        ];
        for (operation, target, addressing, body) in &pool_scoped {
            let case = format!("{operation} with {name_case}");
            let (status, payload) = dispatch(target, *addressing, operation, body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: expected a 400 UserNotFoundException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, USER_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {USER_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_public_operation_still_serves_the_user_the_pool_holds() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(
        &setup,
        "public-served",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let client_id = app_client(&setup, &user_pool_id, "open", json!({})).await;

    // Two registered users: one is confirmed through the code it was sent, the
    // other stays unconfirmed so the resend has a sign-up to re-send for.
    for username in ["confirming", "resending"] {
        ok_anonymous(
            &emulator,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "UserAttributes": [{ "Name": "email", "Value": format!("{username}@example.test") }],
            }),
        )
        .await;
    }

    // ConfirmSignUp, served: the code the sign-up sent confirms the account, and
    // the confirmation is read back through an implemented reader.
    let code = outstanding_code(
        &engine,
        &user_pool_id,
        "confirming",
        state::CodePurpose::SignUpConfirmation,
    );
    ok_anonymous(
        &emulator,
        "ConfirmSignUp",
        &json!({
            "ClientId": client_id,
            "Username": "confirming",
            "ConfirmationCode": code,
        }),
    )
    .await;
    let read_back = ok(
        &setup,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "confirming" }),
    )
    .await;
    assert_eq!(
        read_back["UserStatus"],
        json!("CONFIRMED"),
        "a confirmed sign-up reads back as CONFIRMED, got {read_back}"
    );

    // ResendConfirmationCode, served, for the sign-up still awaiting one.
    let resent = ok_anonymous(
        &emulator,
        "ResendConfirmationCode",
        &json!({ "ClientId": client_id, "Username": "resending" }),
    )
    .await;
    assert_eq!(
        resent["CodeDeliveryDetails"]["AttributeName"],
        json!("email"),
        "a resend reports the attribute it addressed, got {resent}"
    );

    // ForgotPassword and ConfirmForgotPassword, served, for the confirmed user.
    let requested = ok_anonymous(
        &emulator,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": "confirming" }),
    )
    .await;
    assert_eq!(
        requested["CodeDeliveryDetails"]["AttributeName"],
        json!("email"),
        "a reset request reports the attribute it addressed, got {requested}"
    );
    let reset_code = outstanding_code(
        &engine,
        &user_pool_id,
        "confirming",
        state::CodePurpose::PasswordReset,
    );
    ok_anonymous(
        &emulator,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": "confirming",
            "ConfirmationCode": reset_code,
            "Password": RESET_PASSWORD,
        }),
    )
    .await;
}
