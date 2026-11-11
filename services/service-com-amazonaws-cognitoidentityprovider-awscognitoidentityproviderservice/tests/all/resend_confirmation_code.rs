// SPDX-License-Identifier: CC0-1.0

//! `ResendConfirmationCode` over the wire: the operation that re-sends the code which
//! confirms a new user's sign-up.
//!
//! The rules under test:
//!
//! * the message is addressed to the FIRST attribute the pool auto-verifies that the
//!   user holds a value for, and the reply reports that attribute, the medium the
//!   attribute is reached over and the stored value masked; a pool that auto-verifies
//!   nothing the user holds a value for has nowhere to send a code, so the call is
//!   refused;
//! * the code the call sends becomes the ONE code outstanding for that sign-up, so the
//!   code sent before it stops being accepted and the fresh one is;
//! * sending a code confirms nothing: the user it was sent for stays unconfirmed until
//!   a confirmation confirms it;
//! * a request naming no stored app client, naming no stored user, naming a user whose
//!   sign-up is already confirmed or a user that is disabled, or carrying no proof of
//!   the secret its app client holds, is refused with that refusal's own status, error
//!   code and message, and sends nothing.
//!
//! The code itself is delivered out of band, so no response carries it: a test learns
//! the delivered code the way a mailbox would — from the emulator's own state.
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

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The one sentence a `UserNotFoundException` says for this operation, which resolves
/// the user from the `Username` and the `ClientId` of the request together.
const USER_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// What a resend says when the pool auto-verifies nothing the user holds a value for,
/// so the code has no address to go to.
const NO_DESTINATION_MESSAGE: &str =
    "Cannot resend codes. No registered/verified email or phone_number";

/// A `ClientIdType` value (len 1..128, pattern `^[\w+]+$`) that no pool of these tests
/// holds, so it resolves to no app client at all.
const ABSENT_CLIENT_ID: &str = "4bs3ntcl13ntid";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations, this one among them, are called.
    Anonymous,
}

/// Which user, and through which app client, one refusal row addresses.
#[derive(Clone, Copy, Debug)]
enum Subject {
    /// An app client id no stored pool holds.
    UnknownClient,
    /// A username that never signed up, through a client that reports user existence.
    UnknownUser,
    /// The same, through a client whose `PreventUserExistenceErrors` is `ENABLED`.
    UnknownUserHidden,
    /// A signed-up user whose sign-up a confirmation already confirmed.
    Confirmed,
    /// A signed-up user that is disabled.
    Disabled,
    /// A signed-up user of a client holding a secret, asked for with no `SecretHash`.
    SecretHashMissing,
    /// The same, asked for with the hash that client's secret expects.
    SecretHashCorrect,
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

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
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

/// Create a pool with `extra` merged into its configuration and return its minted id.
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

/// Create an app client of `user_pool_id` with `extra` merged into its configuration,
/// and return its minted id beside the secret the pool minted for it, if any.
async fn app_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    extra: Value,
) -> (String, Option<String>) {
    let mut body = json!({ "UserPoolId": user_pool_id, "ClientName": client_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPoolClient", &body).await;
    let client = &created["UserPoolClient"];
    let client_id = client["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string();
    let client_secret = client["ClientSecret"].as_str().map(str::to_string);
    (client_id, client_secret)
}

/// A `SignUp` request body for `username` with `attributes`, carrying `secret_hash`
/// when the case presents one.
fn sign_up_body(
    client_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
    secret_hash: Option<&str>,
) -> Value {
    let attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    let mut body = json!({
        "ClientId": client_id,
        "Username": username,
        "Password": GOOD_PASSWORD,
        "UserAttributes": attributes,
    });
    if let Some(secret_hash) = secret_hash {
        body["SecretHash"] = json!(secret_hash);
    }
    body
}

/// The `SecretHash` a request for `username` through `client_id` carries when that
/// client's secret is `client_secret`: base64 of HMAC-SHA256, keyed with the secret,
/// over the username followed by the client id — computed here independently of the
/// handler.
fn secret_hash(client_secret: &str, username: &str, client_id: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, client_secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{username}{client_id}").as_bytes());
    aws_smithy_types::base64::encode(tag.as_ref())
}

/// The sign-up confirmation code outstanding for `username`, read out of the
/// emulator's own state: the real service delivers a code out of band, so no response
/// carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&state::CodePurpose::SignUpConfirmation))
        .map(|code| code.code.clone())
}

/// Leave `username` disabled in the emulator's own state, which is the state a
/// disabling leaves behind: the operation that performs one is still a scaffold, so
/// this row writes the state rather than calling it.
fn disable_user(engine: &Arc<engine::InMemoryEngine>, user_pool_id: &str, username: &str) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let user = guard
        .pool_mut(user_pool_id)
        .and_then(|pool| pool.user_mut(username))
        .expect("the signed-up user is stored");
    user.enabled = false;
}

/// The `UserStatus` `AdminGetUser` reports for `username`.
async fn stored_status(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> String {
    let read_back = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read_back["UserStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a resend addresses the code to the first attribute the pool auto-verifies that
/// the user holds a value for, reports it masked with that attribute's medium, and the
/// fresh code supersedes the one the sign-up sent. Rows differ along which attributes
/// the pool auto-verifies, in which order, and which of them the user holds.
#[tokio::test]
async fn a_resend_addresses_the_pools_auto_verified_attribute_and_supersedes_the_code() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    // The attribute names are the `VerifiedAttributeType` values `AutoVerifiedAttributes`
    // is a list of, and the mediums the `DeliveryMediumType` values a delivery reports
    // (this service's model:
    // smithy_query(query="shape",shape="VerifiedAttributeType,DeliveryMediumType")).
    //
    // (case, what the pool auto-verifies, the user's attributes,
    //  the (AttributeName, DeliveryMedium, masked Destination) expected or None for a
    //  pool with nowhere to send the code)
    type Row<'a> = (
        &'a str,
        Vec<&'a str>,
        Vec<(&'a str, &'a str)>,
        Option<(&'a str, &'a str, &'a str)>,
    );
    let rows: Vec<Row<'_>> = vec![
        (
            "the one attribute the pool auto-verifies, held by the user",
            vec!["email"],
            vec![("email", "ada@example.com")],
            Some(("email", "EMAIL", "a***@e***")),
        ),
        (
            "a pool that auto-verifies the phone number texts the stored number",
            vec!["phone_number"],
            vec![("phone_number", "+15551230001")],
            Some(("phone_number", "SMS", "+*******0001")),
        ),
        (
            "both auto-verified and both held: the first the pool declares",
            vec!["phone_number", "email"],
            vec![("email", "bob@example.org"), ("phone_number", "+15551230002")],
            Some(("phone_number", "SMS", "+*******0002")),
        ),
        (
            "the first auto-verified attribute has no value, so the next one carries it",
            vec!["email", "phone_number"],
            vec![("phone_number", "+15551230003")],
            Some(("phone_number", "SMS", "+*******0003")),
        ),
        (
            "the user holds no value for what the pool auto-verifies",
            vec!["email"],
            vec![("phone_number", "+15551230004")],
            None,
        ),
        (
            "a pool that auto-verifies nothing",
            vec![],
            vec![("email", "eve@example.com")],
            None,
        ),
    ];

    for (index, (case, auto_verified, attributes, expected)) in rows.iter().enumerate() {
        // A pool that auto-verifies `phone_number` sends that verification over
        // SMS, so it needs the Amazon SNS caller SMS is sent through:
        // `CreateUserPool` refuses the pairing without it.
        let mut configuration = json!({ "AutoVerifiedAttributes": auto_verified });
        if auto_verified.contains(&"phone_number") {
            configuration["SmsConfiguration"] = json!({
                "SnsCallerArn": "arn:aws:iam::000000000000:role/sms-caller"
            });
        }
        let user_pool_id = pool(
            &setup,
            &format!("resend-destination-{index}"),
            configuration,
        )
        .await;
        let (client_id, _) = app_client(&setup, &user_pool_id, "app", json!({})).await;
        let username = format!("resend{index}");

        ok_anonymous(
            &emulator,
            "SignUp",
            &sign_up_body(&client_id, &username, attributes, None),
        )
        .await;
        // The sign-up sends its code to the same destination a resend of it would, so a
        // pool with nowhere to send one leaves nothing outstanding to supersede.
        let first = outstanding_code(&engine, &user_pool_id, &username);
        assert_eq!(
            first.is_some(),
            expected.is_some(),
            "case {case:?}: the sign-up must leave a code outstanding exactly when the \
             pool has a destination for it, got {first:?}"
        );

        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ResendConfirmationCode",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;

        match expected {
            Some((attribute, medium, destination)) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                let details = &payload["CodeDeliveryDetails"];
                assert_eq!(
                    details["AttributeName"],
                    json!(attribute),
                    "case {case:?}: expected AttributeName {attribute}, got {payload}"
                );
                assert_eq!(
                    details["DeliveryMedium"],
                    json!(medium),
                    "case {case:?}: expected DeliveryMedium {medium}, got {payload}"
                );
                assert_eq!(
                    details["Destination"],
                    json!(destination),
                    "case {case:?}: expected the stored value masked as {destination}, \
                     got {payload}"
                );

                let superseded = first.clone().unwrap_or_default();
                let outstanding = outstanding_code(&engine, &user_pool_id, &username)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the resend must leave a code outstanding")
                    });
                assert_ne!(
                    outstanding, superseded,
                    "case {case:?}: a resend mints a fresh code, got {outstanding:?} twice"
                );
                // Sending a code reports where it went and confirms nothing.
                let status_now = stored_status(&setup, &user_pool_id, &username).await;
                assert_eq!(
                    status_now, "UNCONFIRMED",
                    "case {case:?}: a resend leaves the sign-up unconfirmed, got {status_now}"
                );

                // Only the latest code is the one a confirmation accepts.
                let (stale_status, stale) = dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "ConfirmSignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "ConfirmationCode": superseded,
                    }),
                )
                .await;
                assert_eq!(
                    stale_status, 400,
                    "case {case:?}: the superseded code is refused, got {stale_status} \
                     with {stale}"
                );
                assert_eq!(
                    error_code(&stale),
                    "CodeMismatchException",
                    "case {case:?}: expected CodeMismatchException, got {stale}"
                );
                ok_anonymous(
                    &emulator,
                    "ConfirmSignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "ConfirmationCode": outstanding,
                    }),
                )
                .await;
                let status_now = stored_status(&setup, &user_pool_id, &username).await;
                assert_eq!(
                    status_now, "CONFIRMED",
                    "case {case:?}: the re-sent code confirms the sign-up, got {status_now}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    NO_DESTINATION_MESSAGE,
                    "case {case:?}: expected the no-destination wording, got {payload}"
                );
                assert!(
                    outstanding_code(&engine, &user_pool_id, &username).is_none(),
                    "case {case:?}: a refused resend sends nothing, so nothing is outstanding"
                );
                let status_now = stored_status(&setup, &user_pool_id, &username).await;
                assert_eq!(
                    status_now, "UNCONFIRMED",
                    "case {case:?}: a refused resend leaves the user as it was, got \
                     {status_now}"
                );
            }
        }
    }
}

/// RULE: a resend is served only for an unconfirmed, enabled user of a stored app
/// client whose secret the request proves it holds; every other request is refused with
/// that refusal's own status, error code and message, and sends no code. Rows differ
/// along which of those the request fails.
#[tokio::test]
async fn a_resend_is_refused_for_a_request_it_cannot_serve() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);
    let user_pool_id = pool(
        &setup,
        "resend-refusals",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let (open_client, _) = app_client(&setup, &user_pool_id, "open", json!({})).await;
    let (hiding_client, _) = app_client(
        &setup,
        &user_pool_id,
        "hiding",
        json!({ "PreventUserExistenceErrors": "ENABLED" }),
    )
    .await;
    let (confidential_client, confidential_secret) = app_client(
        &setup,
        &user_pool_id,
        "confidential",
        json!({ "GenerateSecret": true }),
    )
    .await;
    let confidential_secret =
        confidential_secret.expect("GenerateSecret=true mints a client secret");

    // (case, what the request names, expected status, expected error code — empty for a
    //  served request —, expected message)
    let rows: Vec<(&str, Subject, u16, &str, String)> = vec![
        (
            "an app client id no stored pool holds",
            Subject::UnknownClient,
            400,
            "ResourceNotFoundException",
            format!("User pool client {ABSENT_CLIENT_ID} does not exist."),
        ),
        (
            "a username that never signed up",
            Subject::UnknownUser,
            400,
            "UserNotFoundException",
            USER_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the same, through a client that suppresses user-existence errors",
            Subject::UnknownUserHidden,
            200,
            "",
            String::new(),
        ),
        (
            "a user whose sign-up is already confirmed",
            Subject::Confirmed,
            400,
            "InvalidParameterException",
            "User is already confirmed.".to_string(),
        ),
        (
            "a disabled user",
            Subject::Disabled,
            400,
            "NotAuthorizedException",
            "User is disabled.".to_string(),
        ),
        (
            "no SecretHash, through a client that holds a secret",
            Subject::SecretHashMissing,
            400,
            "NotAuthorizedException",
            format!(
                "Client {confidential_client} is configured with secret but SECRET_HASH was \
                 not received"
            ),
        ),
        (
            "the SecretHash that client's secret expects",
            Subject::SecretHashCorrect,
            200,
            "",
            String::new(),
        ),
    ];

    for (index, (case, subject, expected_status, expected_error, expected_message)) in
        rows.iter().enumerate()
    {
        let username = format!("subject{index}");
        let address = format!("{username}@example.com");
        let client_id = match subject {
            Subject::UnknownClient => ABSENT_CLIENT_ID.to_string(),
            Subject::UnknownUserHidden => hiding_client.clone(),
            Subject::SecretHashMissing | Subject::SecretHashCorrect => {
                confidential_client.clone()
            }
            Subject::UnknownUser | Subject::Confirmed | Subject::Disabled => open_client.clone(),
        };
        let hash = secret_hash(&confidential_secret, &username, &confidential_client);

        // Each row's user is brought into exactly the state the row is about.
        match subject {
            Subject::UnknownClient | Subject::UnknownUser | Subject::UnknownUserHidden => {}
            Subject::Confirmed => {
                ok_anonymous(
                    &emulator,
                    "SignUp",
                    &sign_up_body(&client_id, &username, &[("email", &address)], None),
                )
                .await;
                let code = outstanding_code(&engine, &user_pool_id, &username)
                    .unwrap_or_else(|| panic!("case {case:?}: the sign-up sends a code"));
                ok_anonymous(
                    &emulator,
                    "ConfirmSignUp",
                    &json!({
                        "ClientId": client_id,
                        "Username": username,
                        "ConfirmationCode": code,
                    }),
                )
                .await;
            }
            Subject::Disabled => {
                ok_anonymous(
                    &emulator,
                    "SignUp",
                    &sign_up_body(&client_id, &username, &[("email", &address)], None),
                )
                .await;
                disable_user(&engine, &user_pool_id, &username);
            }
            Subject::SecretHashMissing | Subject::SecretHashCorrect => {
                ok_anonymous(
                    &emulator,
                    "SignUp",
                    &sign_up_body(&client_id, &username, &[("email", &address)], Some(&hash)),
                )
                .await;
            }
        }

        let before = outstanding_code(&engine, &user_pool_id, &username);
        let mut body = json!({ "ClientId": client_id, "Username": username });
        if matches!(subject, Subject::SecretHashCorrect) {
            body["SecretHash"] = json!(hash);
        }
        let (status, payload) =
            dispatch(&emulator, Addressing::Anonymous, "ResendConfirmationCode", &body).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        let after = outstanding_code(&engine, &user_pool_id, &username);

        if expected_error.is_empty() {
            if matches!(subject, Subject::UnknownUserHidden) {
                // A user nobody registered, asked for through a client that
                // suppresses user-existence errors: the delivery reported is to a
                // SIMULATED destination, and nothing is stored for a user that
                // does not exist.
                assert_eq!(
                    payload["CodeDeliveryDetails"]["Destination"],
                    json!("a***@b***"),
                    "case {case:?}: expected a simulated destination, got {payload}"
                );
                assert!(
                    after.is_none(),
                    "case {case:?}: no code is minted for a user that does not exist, got \
                     {after:?}"
                );
                continue;
            }
            // The one served row: the code goes to the stored address, masked.
            assert_eq!(
                payload["CodeDeliveryDetails"]["Destination"],
                json!("s***@e***"),
                "case {case:?}: expected the stored address reported masked, got {payload}"
            );
            assert!(
                after.is_some() && after != before,
                "case {case:?}: a served resend mints a fresh code, got {before:?} then \
                 {after:?}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                *expected_error,
                "case {case:?}: expected error code {expected_error}, got {payload}"
            );
            assert_eq!(
                error_message(&payload),
                expected_message.as_str(),
                "case {case:?}: expected the wording of that refusal, got {payload}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused resend sends nothing, so the outstanding code \
                 is untouched, got {after:?}"
            );
        }
    }
}
