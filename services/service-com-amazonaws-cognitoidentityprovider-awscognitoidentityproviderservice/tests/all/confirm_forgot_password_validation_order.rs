// SPDX-License-Identifier: CC0-1.0

//! The order `ConfirmForgotPassword` decides a request in, and what each
//! refusal discloses.
//!
//! The rule under test: the reset confirmation is judged in a fixed order — the
//! app client's secret hash, then the user, then the code outstanding for a
//! reset of that user, and only then the proposed password against the pool's
//! policy — so a caller learns nothing it has not already proved. A user that
//! exists with NO outstanding reset code is refused with
//! `ExpiredCodeException`, whose remediation is to ask for a code, whatever the
//! proposed password would be; and an app client that suppresses
//! user-existence errors answers an unknown username with that SAME refusal, so
//! no answer tells a caller whether the account exists. Under the documented
//! `LEGACY` default the unknown username is reported as
//! `UserNotFoundException` with the unresolved-pair sentence.
//!
//! `ExpiredCodeException`, `CodeMismatchException`, `InvalidPasswordException`,
//! `NotAuthorizedException` and `UserNotFoundException` are this operation's own
//! modelled errors
//! (`smithy_query(query="operation",operation="ConfirmForgotPassword")`).
//!
//! Each table row names the case it asserts and the value it saw.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes a SIGNED call to this
/// service; the reset confirmation itself is called anonymously, as a client
/// with no credentials calls it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's policy.
const GOOD_PASSWORD: &str = "Res3tPass!2024";

/// A `ConfirmationCodeType` value (pattern `^[\S]+$`) that no code this
/// emulator minted can be.
const UNISSUED_CODE: &str = "000000";

/// The sentence a confirmation whose code nothing outstanding matches is
/// refused with.
const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// The sentence an operation that resolves the user from `Username` and
/// `ClientId` together answers for a pair it cannot resolve.
const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// The shared engine, so a test can read the code a reset request delivered.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body`, signed or not, and return its status and body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let request = request
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

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body, true).await;
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

/// The `SecretHash` a request for `username` to `client_id` carries when that
/// client's secret is `client_secret`
/// (`smithy_query(query="doc",shape="ConfirmForgotPasswordRequest",member="SecretHash")`).
fn secret_hash(client_secret: &str, username: &str, client_id: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, client_secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{username}{client_id}").as_bytes());
    aws_smithy_types::base64::encode(tag.as_ref())
}

/// An app client of the fixture pool.
struct AppClient {
    client_id: String,
    client_secret: Option<String>,
}

/// Create an app client of `user_pool_id`, with a generated secret when
/// `with_secret`, and the `PreventUserExistenceErrors` setting `mode` names.
async fn app_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    with_secret: bool,
    mode: Option<&str>,
) -> AppClient {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    body.insert("GenerateSecret".to_string(), json!(with_secret));
    if let Some(mode) = mode {
        body.insert("PreventUserExistenceErrors".to_string(), json!(mode));
    }
    let client = ok(router, "CreateUserPoolClient", &Value::Object(body)).await["UserPoolClient"]
        .clone();
    AppClient {
        client_id: client["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret: client["ClientSecret"].as_str().map(str::to_string),
    }
}

/// A `ConfirmForgotPassword` request body.
fn confirm(
    client: &AppClient,
    username: &str,
    code: &str,
    password: &str,
    hash: Option<&str>,
) -> Value {
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(client.client_id));
    body.insert("Username".to_string(), json!(username));
    body.insert("ConfirmationCode".to_string(), json!(code));
    body.insert("Password".to_string(), json!(password));
    if let Some(hash) = hash {
        body.insert("SecretHash".to_string(), json!(hash));
    }
    Value::Object(body)
}

/// A reset confirmation for a user that holds no outstanding code is refused
/// with the code refusal, ahead of the password policy, and the refusal of an
/// unknown username is the one the app client's own
/// `PreventUserExistenceErrors` mode allows.
#[tokio::test]
async fn a_reset_confirmation_is_refused_before_any_code_exists() {
    let engine = engine();
    let router = router(&engine);
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "reset-order-pool", "AutoVerifiedAttributes": ["email"] }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    // The documented default of `PreventUserExistenceErrors` is `LEGACY`, so a
    // client that names none is the legacy mode; the other two rows name each
    // mode explicitly.
    let legacy = app_client(&router, &user_pool_id, "legacy", false, None).await;
    let hiding = app_client(&router, &user_pool_id, "hiding", false, Some("ENABLED")).await;
    let declared_legacy =
        app_client(&router, &user_pool_id, "declared-legacy", false, Some("LEGACY")).await;

    // A user that exists, created with no verified address, so no reset code can
    // have been delivered for it.
    let present = "present-user";
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": present,
            "TemporaryPassword": "Temp0rary!2024",
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;

    // The rows differ along the two facts the refusal depends on: whether the
    // username names a stored user, and what the proposed password is — a
    // policy-conformant one and one that misses each requirement, since the code
    // is judged BEFORE the policy. The username forms span what `UsernameType`
    // admits (length 1..128, pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`).
    let absent_names = [
        ("a plain name", "absent-user".to_string()),
        ("the one-character length minimum", "z".to_string()),
        ("the 128-character length maximum", "u".repeat(128)),
        ("a non-ASCII name", "ユーザー-不在①".to_string()),
        ("an email-shaped name", "absent@example.test".to_string()),
    ];
    let passwords = [
        ("a policy-conformant password", GOOD_PASSWORD),
        ("a password below the minimum length", "Ab1!"),
        ("a password with no uppercase character", "res3tpass!2024"),
        ("a password with no symbol character", "Res3tPass2024"),
    ];

    for (password_case, password) in passwords {
        // The user EXISTS and holds no outstanding code: the code refusal, not
        // the password policy and not a user-existence refusal, whichever mode
        // the app client is in.
        for (client_case, client) in [
            ("the legacy default client", &legacy),
            ("the client that declares LEGACY", &declared_legacy),
            ("the client that suppresses existence errors", &hiding),
        ] {
            let case = format!("{client_case}, a stored user with no outstanding code, {password_case}");
            let (status, payload) = call(
                &router,
                "ConfirmForgotPassword",
                &confirm(client, present, UNISSUED_CODE, password, None),
                false,
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ExpiredCodeException"),
                "case {case:?}: expected a 400 ExpiredCodeException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, EXPIRED_CODE_MESSAGE,
                "case {case:?}: expected {EXPIRED_CODE_MESSAGE:?}, got {message:?}"
            );
        }

        // A username no user carries: the legacy modes report the unresolved
        // pair, and the suppressing mode answers the SAME refusal a stored user
        // with no code gets.
        for (name_case, username) in &absent_names {
            for (client_case, client, code, message) in [
                (
                    "the legacy default client",
                    &legacy,
                    "UserNotFoundException",
                    USERNAME_CLIENT_NOT_FOUND_MESSAGE,
                ),
                (
                    "the client that declares LEGACY",
                    &declared_legacy,
                    "UserNotFoundException",
                    USERNAME_CLIENT_NOT_FOUND_MESSAGE,
                ),
                (
                    "the client that suppresses existence errors",
                    &hiding,
                    "ExpiredCodeException",
                    EXPIRED_CODE_MESSAGE,
                ),
            ] {
                let case = format!("{client_case}, {name_case} that no user carries, {password_case}");
                let (status, payload) = call(
                    &router,
                    "ConfirmForgotPassword",
                    &confirm(client, username, UNISSUED_CODE, password, None),
                    false,
                )
                .await;
                assert_eq!(
                    (status, error_code(&payload)),
                    (400, code),
                    "case {case:?}: expected a 400 {code}, got {status} with {payload}"
                );
                let answered = error_message(&payload);
                assert_eq!(
                    answered, message,
                    "case {case:?}: expected {message:?}, got {answered:?}"
                );
                assert!(
                    !answered.contains(username.as_str())
                        && !answered.contains(client.client_id.as_str()),
                    "case {case:?}: the refusal must quote neither the username nor the \
                     client id, got {answered:?}"
                );
            }
        }
    }
}

/// The app client's secret hash is judged FIRST: a confidential client's
/// confirmation that does not hold the secret is refused on the secret, for a
/// stored user and an unknown one alike.
#[tokio::test]
async fn a_reset_confirmation_is_refused_on_the_secret_before_the_user() {
    let engine = engine();
    let router = router(&engine);
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "reset-secret-pool", "AutoVerifiedAttributes": ["email"] }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let confidential = app_client(&router, &user_pool_id, "confidential", true, None).await;
    let secret = confidential
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");

    let present = "secret-present-user";
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": present,
            "TemporaryPassword": "Temp0rary!2024",
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;

    // The rows differ along the hash presented and whether the username names a
    // stored user: a request that does not hold the secret is refused on the
    // secret either way, so the refusal discloses nothing about the user. The
    // two defects of the hash are worded apart — nothing presented at all, and a
    // tag the secret does not produce.
    for (username_case, username) in [
        ("a stored user", present),
        ("a username no user carries", "secret-absent-user"),
    ] {
        for (hash_case, hash, expected) in [
            (
                "no SecretHash at all",
                None,
                format!(
                    "Client {} is configured with secret but SECRET_HASH was not received",
                    confidential.client_id
                ),
            ),
            (
                "a hash another username earns",
                Some(secret_hash(
                    &secret,
                    "somebody-else",
                    &confidential.client_id,
                )),
                format!(
                    "Unable to verify secret hash for client {}",
                    confidential.client_id
                ),
            ),
            (
                "a tag no secret of this client produces",
                Some(aws_smithy_types::base64::encode([0_u8; 32])),
                format!(
                    "Unable to verify secret hash for client {}",
                    confidential.client_id
                ),
            ),
        ] {
            let case = format!("{hash_case}, for {username_case}");
            let (status, payload) = call(
                &router,
                "ConfirmForgotPassword",
                &confirm(
                    &confidential,
                    username,
                    UNISSUED_CODE,
                    GOOD_PASSWORD,
                    hash.as_deref(),
                ),
                false,
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "NotAuthorizedException"),
                "case {case:?}: expected a 400 NotAuthorizedException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// With a code OUTSTANDING, the same operation tells the two code refusals
/// apart, and still judges the code before the password: a wrong code earns
/// `CodeMismatchException` and the right code with a refused password earns
/// `InvalidPasswordException` without spending the code.
#[tokio::test]
async fn an_outstanding_code_is_judged_before_the_password() {
    let engine = engine();
    let router = router(&engine);
    let user_pool_id = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "reset-code-pool", "AutoVerifiedAttributes": ["email"] }),
    )
    .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = app_client(&router, &user_pool_id, "plain", false, None).await;
    let username = "code-holder";
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [
                { "Name": "email", "Value": "code-holder@example.test" },
                { "Name": "email_verified", "Value": "true" },
            ],
            "TemporaryPassword": "Temp0rary!2024",
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;
    // The reset flow is open to a CONFIRMED user, so the temporary password the
    // create set is replaced by a permanent one first.
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": "Perman3nt!2024",
            "Permanent": true,
        }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "ForgotPassword",
        &json!({ "ClientId": client.client_id, "Username": username }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "ForgotPassword must report a delivery, got {status} with {payload}"
    );
    // The real service delivers the code out of band, so it is read from the
    // emulator's own state rather than from the response.
    let delivered = {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let guard = service_state.lock();
        guard
            .pool(&user_pool_id)
            .and_then(|pool| pool.user(username))
            .and_then(|user| user.outstanding_code(&state::CodePurpose::PasswordReset))
            .map(|code| code.code.clone())
            .expect("a reset code is outstanding")
    };

    // A code that is not the outstanding one: the mismatch, which asks for the
    // code to be presented again rather than for a new one.
    let (status, payload) = call(
        &router,
        "ConfirmForgotPassword",
        &confirm(&client, username, UNISSUED_CODE, GOOD_PASSWORD, None),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "CodeMismatchException"),
        "case \"a code that is not the outstanding one\": expected a 400 CodeMismatchException, \
         got {status} with {payload}"
    );

    // The right code with a password the policy refuses: the password refusal,
    // and the code is not spent by it.
    let (status, payload) = call(
        &router,
        "ConfirmForgotPassword",
        &confirm(&client, username, &delivered, "Ab1!", None),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidPasswordException"),
        "case \"the right code with a refused password\": expected a 400 \
         InvalidPasswordException, got {status} with {payload}"
    );

    // The same code with a password the policy accepts completes the reset, so
    // the refusals above spent nothing.
    let (status, payload) = call(
        &router,
        "ConfirmForgotPassword",
        &confirm(&client, username, &delivered, GOOD_PASSWORD, None),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the right code with an accepted password\": the reset must complete, \
         got {status} with {payload}"
    );

    // The code is spent now, so presenting it again finds nothing outstanding —
    // the same refusal a user that never asked for a code gets.
    let (status, payload) = call(
        &router,
        "ConfirmForgotPassword",
        &confirm(&client, username, &delivered, GOOD_PASSWORD, None),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ExpiredCodeException"),
        "case \"a spent code presented again\": expected a 400 ExpiredCodeException, \
         got {status} with {payload}"
    );
    let message = error_message(&payload);
    assert_eq!(
        message, EXPIRED_CODE_MESSAGE,
        "case \"a spent code presented again\": expected {EXPIRED_CODE_MESSAGE:?}, got {message:?}"
    );
}
