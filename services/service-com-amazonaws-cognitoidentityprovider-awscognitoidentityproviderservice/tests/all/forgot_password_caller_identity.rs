// SPDX-License-Identifier: CC0-1.0

//! Who `ForgotPassword` answers, over the wire.
//!
//! RULE under test: `ForgotPassword` models no `UserPoolId`
//! (`smithy_query(query="operation",operation="ForgotPassword")`), so the app client the
//! request names is what resolves the pool whose user is reset — a `ClientId` no pool
//! holds is `ResourceNotFoundException`, and a username only ANOTHER pool holds is no
//! user at all. A client that holds a client secret serves the request only when it
//! carries the `SecretHash` that secret produces, and that proof is required before any
//! user is resolved; a client that holds NO secret has no such value to verify, so a
//! request to it is served either way. A user the pool will not reset is reported as that
//! client's `PreventUserExistenceErrors` setting decides: `ENABLED` "suppresses messages that
//! might indicate a valid user exists"
//! (`smithy_query(query="doc",shape="CreateUserPoolClientRequest",member="PreventUserExistenceErrors")`),
//! so it answers the generic authentication error, while the documented default `LEGACY`
//! answers `UserNotFoundException`; a disabled user is refused either way. Every refusal
//! sends nothing: no code is left outstanding for any user of either pool.
//!
//! The table's rows differ along the app client the request names (one no pool holds,
//! two secretless clients of different pools, one that suppresses existence errors and
//! one that holds a secret), where the subject user is registered (the client's own
//! pool, the other pool, nowhere), whether that user is enabled, and what the presented
//! `SecretHash` is derived from. Each served row is round-tripped: the delivery it
//! reports is a code `ConfirmForgotPassword` accepts, after which the user signs in with
//! the new password.
//!
//! Each row names its own user, so one row's answer cannot explain another's.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
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

/// This service's real regional endpoint host, which a signed call addresses.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// Another password satisfying it, for a reset to settle on.
const RESET_PASSWORD: &str = "Res3tPassw0rd!";

/// A value of the modelled `ClientIdType` shape (pattern `^[\w+]+$`) that no
/// `CreateUserPoolClient` in this test minted.
const UNKNOWN_CLIENT_ID: &str = "noclientofanypool";

/// The one sentence a `UserNotFoundException` says for this operation, which resolves
/// the user from the `Username` and the `ClientId` of the request TOGETHER (it models no
/// `UserPoolId`), so the refusal reports that COMBINATION as unresolved rather than
/// making a statement about the username alone.
const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// A username no request in this test registers, for the `SecretHash` of another
/// identity.
const OTHER_USERNAME: &str = "someoneelse";

/// The `Destination` a delivery to a SIMULATED destination reports — the answer owed
/// to a request naming a user nobody registered, by an app client whose
/// `PreventUserExistenceErrors` is `ENABLED`. It identifies nobody, and it is the
/// same value whatever the request named, so no two usernames can be told apart by
/// the answer they get.
const SIMULATED_DESTINATION: &str = "a***@b***";

/// The email addresses subject users are created with, as `(local part, domain, the
/// `Destination` Amazon Cognito reports for that address)`
/// (`smithy_query(query="doc",shape="CodeDeliveryDetailsType",member="Destination")`:
/// "The email address or phone number destination where Amazon Cognito sent the code").
/// Amazon Cognito reports it MASKED, as the Developer Guide's account-recovery and
/// sign-up messages show — only the first character of the local part and the first
/// character of the domain stay readable, so a caller learns which address was used
/// without being told an address it does not hold. The entries differ along the
/// local part's length, the domain's label count and its last label, so what a served row
/// asserts is the masking RULE rather than one rendering of it. Each row takes the entry
/// its index names and appends that index to the local part, which leaves the first
/// character — and so the reported destination — as written here.
const ADDRESSES: [(&str, &str, &str); 3] = [
    ("caller", "example.net", "c***@e***"),
    ("a.very.long.local.part", "mail.example.co.uk", "a***@m***"),
    ("x", "q.io", "x***@q***"),
];

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how `ForgotPassword` is called.
    Anonymous,
}

/// Which app client one row's request names.
#[derive(Clone, Copy, Debug)]
enum Client {
    /// A well-formed `ClientId` no pool in this emulator holds.
    Unknown,
    /// A secretless client of the first pool.
    Open,
    /// A secretless client of the first pool that suppresses existence errors.
    Hiding,
    /// A client of the first pool that holds a client secret.
    Confidential,
    /// A secretless client of the SECOND pool.
    Neighbour,
}

/// Where the subject user of one row is registered, relative to the pool that holds the
/// app client the request names.
#[derive(Clone, Copy, Debug)]
enum Subject {
    /// Registered, enabled, with a verified email, in the client's own pool.
    OwnPool,
    /// Registered the same way, but in the other pool of this emulator.
    OtherPool,
    /// Registered in the client's own pool with a verified email, then disabled.
    Disabled,
    /// Registered in neither pool.
    Unregistered,
}

/// What the `SecretHash` one row presents is derived from.
#[derive(Clone, Copy, Debug)]
enum Hash {
    /// The request carries none.
    Absent,
    /// The hash the named client's own secret produces for the subject username.
    OwnSecret,
    /// The hash ANOTHER client's secret produces for the same username and client id.
    AnotherClientsSecret,
    /// The hash the right secret produces for a DIFFERENT username.
    AnotherUsername,
}

/// The message one row expects, which is fixed text or a template over the client id.
#[derive(Clone, Copy, Debug)]
enum Expected {
    /// The row is served, so there is no error message.
    Served,
    /// One fixed sentence, the same for every request.
    Fixed(&'static str),
    /// The refusal that names the `ClientId` no pool holds.
    ClientNotFound,
    /// The refusal that names the client whose secret the request did not prove.
    UnverifiedSecretHash,
    /// The refusal that names the client the request presented no hash for at
    /// all, which is a different fact about the request.
    SecretHashNotReceived,
}

/// One table row.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    client: Client,
    subject: Subject,
    hash: Hash,
    status: u16,
    /// The expected wire error code, or `None` when the row is served.
    error: Option<&'static str>,
    message: Expected,
}

/// An app client as this test holds it: its pool, its id, and the secret it has.
#[derive(Debug)]
struct AppClient {
    user_pool_id: String,
    client_id: String,
    client_secret: Option<String>,
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

/// The `SecretHash` a request for `username` to the app client `client_id` carries when
/// that client's secret is `client_secret`: base64 of HMAC-SHA256, keyed with the secret,
/// over the username followed by the client id
/// (`smithy_query(query="doc",shape="ForgotPasswordRequest",member="SecretHash")`).
fn secret_hash(client_secret: &str, username: &str, client_id: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, client_secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{username}{client_id}").as_bytes());
    aws_smithy_types::base64::encode(tag.as_ref())
}

/// Create a pool and return its minted `UserPoolId`.
async fn create_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id`, with a generated secret when `with_secret`
/// and the given `PreventUserExistenceErrors` setting when one is named.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    with_secret: bool,
    prevent_user_existence_errors: Option<&str>,
) -> AppClient {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(client_name));
    body.insert("GenerateSecret".to_string(), json!(with_secret));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!(["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]),
    );
    if let Some(setting) = prevent_user_existence_errors {
        body.insert(
            "PreventUserExistenceErrors".to_string(),
            json!(setting),
        );
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    let client = &created["UserPoolClient"];
    let client_secret = client["ClientSecret"].as_str().map(str::to_string);
    assert_eq!(
        client_secret.is_some(),
        with_secret,
        "GenerateSecret={with_secret} must decide whether a secret is minted, got {client}"
    );
    AppClient {
        user_pool_id: user_pool_id.to_string(),
        client_id: client["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret,
    }
}

/// Create `username` in `user_pool_id` with a VERIFIED email and a permanent password,
/// which is what makes it a `CONFIRMED` user a reset message can be addressed to.
async fn admin_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    address: &str,
) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [
                { "Name": "email", "Value": address },
                { "Name": "email_verified", "Value": "true" },
            ],
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// The code outstanding for a password reset of `username`, read out of the emulator's
/// own state: the real service delivers the code out of band, so no response carries it.
fn outstanding_reset_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(&state::CodePurpose::PasswordReset))
        .map(|code| code.code.clone())
}

/// Sign `username` in with `password` through the admin password flow of a secretless
/// client of its pool, and return the access token that sign-in issued.
async fn access_token(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
    password: &str,
) -> String {
    let signed_in = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
    )
    .await;
    signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("a sign-in issues an access token")
        .to_string()
}

/// RULE: the app client named by a `ForgotPassword` request resolves the pool whose user
/// is reset, a client that holds a secret is served only on the `SecretHash` that secret
/// produces, and a user the pool will not reset is reported as that client's
/// `PreventUserExistenceErrors` setting decides — with no code sent in any refusal.
#[tokio::test]
async fn forgot_password_answers_only_the_pool_its_app_client_names() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    // Two pools, so a client of one is shown NOT to reach the users of the other.
    let first = create_pool(&setup, "reset-callers-first").await;
    let second = create_pool(&setup, "reset-callers-second").await;
    // `PreventUserExistenceErrors` is left unset on `open`, which takes the documented
    // `LEGACY` default.
    let open = create_client(&setup, &first, "open", false, None).await;
    let hiding = create_client(&setup, &first, "hiding", false, Some("ENABLED")).await;
    let confidential = create_client(&setup, &first, "confidential", true, None).await;
    let other = create_client(&setup, &first, "other-secret", true, None).await;
    let neighbour = create_client(&setup, &second, "neighbour", false, None).await;
    let secret = confidential
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");
    let other_secret = other
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");

    let rows: Vec<Row> = vec![
        Row {
            case: "a ClientId no pool holds",
            client: Client::Unknown,
            subject: Subject::OwnPool,
            hash: Hash::Absent,
            status: 400,
            error: Some("ResourceNotFoundException"),
            message: Expected::ClientNotFound,
        },
        Row {
            case: "a secretless client, for a user of its own pool",
            client: Client::Open,
            subject: Subject::OwnPool,
            hash: Hash::Absent,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a client of the second pool, for a user of ITS own pool",
            client: Client::Neighbour,
            subject: Subject::OwnPool,
            hash: Hash::Absent,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a client of the first pool, for a user only the second pool holds",
            client: Client::Open,
            subject: Subject::OtherPool,
            hash: Hash::Absent,
            status: 400,
            error: Some("UserNotFoundException"),
            message: Expected::Fixed(USERNAME_CLIENT_NOT_FOUND_MESSAGE),
        },
        Row {
            case: "a client of the second pool, for a user only the first pool holds",
            client: Client::Neighbour,
            subject: Subject::OtherPool,
            hash: Hash::Absent,
            status: 400,
            error: Some("UserNotFoundException"),
            message: Expected::Fixed(USERNAME_CLIENT_NOT_FOUND_MESSAGE),
        },
        Row {
            case: "a legacy client, for a user nobody registered",
            client: Client::Open,
            subject: Subject::Unregistered,
            hash: Hash::Absent,
            status: 400,
            error: Some("UserNotFoundException"),
            message: Expected::Fixed(USERNAME_CLIENT_NOT_FOUND_MESSAGE),
        },
        Row {
            case: "a client that suppresses existence errors, for a user nobody registered",
            client: Client::Hiding,
            subject: Subject::Unregistered,
            hash: Hash::Absent,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a client that suppresses existence errors, for a user of its own pool",
            client: Client::Hiding,
            subject: Subject::OwnPool,
            hash: Hash::Absent,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a client that holds a secret, with the hash its own secret produces",
            client: Client::Confidential,
            subject: Subject::OwnPool,
            hash: Hash::OwnSecret,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a client that holds a secret, with no hash at all",
            client: Client::Confidential,
            subject: Subject::OwnPool,
            hash: Hash::Absent,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::SecretHashNotReceived,
        },
        Row {
            case: "a client that holds a secret, with ANOTHER client's secret's hash",
            client: Client::Confidential,
            subject: Subject::OwnPool,
            hash: Hash::AnotherClientsSecret,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::UnverifiedSecretHash,
        },
        Row {
            case: "a client that holds a secret, with the hash of a different username",
            client: Client::Confidential,
            subject: Subject::OwnPool,
            hash: Hash::AnotherUsername,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::UnverifiedSecretHash,
        },
        Row {
            case: "a client that holds a secret, with no hash, for a user nobody registered",
            client: Client::Confidential,
            subject: Subject::Unregistered,
            hash: Hash::Absent,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::SecretHashNotReceived,
        },
        Row {
            case: "a client that holds a secret, with its hash, for a user nobody registered",
            client: Client::Confidential,
            subject: Subject::Unregistered,
            hash: Hash::OwnSecret,
            status: 400,
            error: Some("UserNotFoundException"),
            message: Expected::Fixed(USERNAME_CLIENT_NOT_FOUND_MESSAGE),
        },
        Row {
            case: "a secretless client, with a hash it has no secret to verify",
            client: Client::Open,
            subject: Subject::OwnPool,
            hash: Hash::AnotherClientsSecret,
            status: 200,
            error: None,
            message: Expected::Served,
        },
        Row {
            case: "a disabled user of the client's own pool",
            client: Client::Open,
            subject: Subject::Disabled,
            hash: Hash::Absent,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::Fixed("User is disabled."),
        },
        Row {
            case: "a disabled user, under a client that suppresses existence errors",
            client: Client::Hiding,
            subject: Subject::Disabled,
            hash: Hash::Absent,
            status: 400,
            error: Some("NotAuthorizedException"),
            message: Expected::Fixed("User is disabled."),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // The named client, the pool it belongs to, and the pool it does NOT.
        let (client_id, client_secret, home, away) = match row.client {
            Client::Unknown => (UNKNOWN_CLIENT_ID.to_string(), None, &first, &second),
            Client::Open => (open.client_id.clone(), None, &open.user_pool_id, &second),
            Client::Hiding => (
                hiding.client_id.clone(),
                None,
                &hiding.user_pool_id,
                &second,
            ),
            Client::Confidential => (
                confidential.client_id.clone(),
                Some(secret.clone()),
                &confidential.user_pool_id,
                &second,
            ),
            Client::Neighbour => (
                neighbour.client_id.clone(),
                None,
                &neighbour.user_pool_id,
                &first,
            ),
        };

        let username = format!("caller{index}");
        let (local, domain, masked) = ADDRESSES[index % ADDRESSES.len()];
        let address = format!("{local}{index}@{domain}");
        match row.subject {
            Subject::OwnPool => admin_user(&setup, home, &username, &address).await,
            Subject::OtherPool => admin_user(&setup, away, &username, &address).await,
            Subject::Disabled => {
                admin_user(&setup, home, &username, &address).await;
                ok(
                    &setup,
                    "AdminDisableUser",
                    &json!({ "UserPoolId": home, "Username": username }),
                )
                .await;
            }
            Subject::Unregistered => {}
        }

        let presented = match row.hash {
            Hash::Absent => None,
            Hash::OwnSecret => Some(secret_hash(
                client_secret.as_deref().expect("this row names a secret"),
                &username,
                &client_id,
            )),
            Hash::AnotherClientsSecret => {
                Some(secret_hash(&other_secret, &username, &client_id))
            }
            Hash::AnotherUsername => Some(secret_hash(
                client_secret.as_deref().expect("this row names a secret"),
                OTHER_USERNAME,
                &client_id,
            )),
        };
        let mut body = Map::new();
        body.insert("ClientId".to_string(), json!(client_id));
        body.insert("Username".to_string(), json!(username));
        if let Some(presented) = &presented {
            body.insert("SecretHash".to_string(), json!(presented));
        }

        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ForgotPassword",
            &Value::Object(body),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {payload}",
            row.status
        );

        let expected_message = match row.message {
            Expected::Served => String::new(),
            Expected::Fixed(text) => text.to_string(),
            Expected::ClientNotFound => {
                format!("User pool client {client_id} does not exist.")
            }
            Expected::UnverifiedSecretHash => {
                format!("Unable to verify secret hash for client {client_id}")
            }
            Expected::SecretHashNotReceived => format!(
                "Client {client_id} is configured with secret but SECRET_HASH was not received"
            ),
        };

        match row.error {
            Some(expected_error) => {
                assert_eq!(
                    error_code(&payload),
                    expected_error,
                    "case {case:?}: expected error code {expected_error}, got {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {payload}"
                );
                // A refusal sends no message, so no code is outstanding for this user in
                // EITHER pool: not the one the client names, and not the one it does not.
                for pool in [home, away] {
                    let sent = outstanding_reset_code(&engine, pool, &username);
                    assert!(
                        sent.is_none(),
                        "case {case:?}: a refused request sends no code, got {sent:?} \
                         outstanding in pool {pool}"
                    );
                }
            }
            None => {
                let details = &payload["CodeDeliveryDetails"];
                if matches!(row.subject, Subject::Unregistered) {
                    // A user nobody registered, through a client that suppresses
                    // user-existence errors: the delivery reported is to a
                    // SIMULATED destination — nothing distinguishes the answer
                    // from the one a registered user gets — and nothing was sent,
                    // so no code is outstanding in either pool.
                    assert_eq!(
                        details["DeliveryMedium"],
                        json!("EMAIL"),
                        "case {case:?}: a simulated delivery is reported over EMAIL, got \
                         {payload}"
                    );
                    assert_eq!(
                        details["AttributeName"],
                        json!("email"),
                        "case {case:?}: expected the attribute a simulated delivery names, \
                         got {payload}"
                    );
                    assert_eq!(
                        details["Destination"],
                        json!(SIMULATED_DESTINATION),
                        "case {case:?}: expected the simulated destination, got {payload}"
                    );
                    for pool in [home, away] {
                        let sent = outstanding_reset_code(&engine, pool, &username);
                        assert!(
                            sent.is_none(),
                            "case {case:?}: no code is minted for a user that does not exist, \
                             got {sent:?} outstanding in pool {pool}"
                        );
                    }
                    continue;
                }
                assert_eq!(
                    details["DeliveryMedium"],
                    json!("EMAIL"),
                    "case {case:?}: a verified email is reached over EMAIL, got {payload}"
                );
                assert_eq!(
                    details["AttributeName"],
                    json!("email"),
                    "case {case:?}: expected the attribute the code went to, got {payload}"
                );
                assert_eq!(
                    details["Destination"],
                    json!(masked),
                    "case {case:?}: expected {address} reported masked as {masked}, got \
                     {payload}"
                );

                // ROUND TRIP: the reported delivery is a code really minted for the pool
                // this client names — `ConfirmForgotPassword` accepts it, and the user
                // then signs in with the password that reset settled on.
                let code = outstanding_reset_code(&engine, home, &username)
                    .unwrap_or_else(|| panic!("case {case:?}: a served request sends a code"));
                let mut confirm = Map::new();
                confirm.insert("ClientId".to_string(), json!(client_id));
                confirm.insert("Username".to_string(), json!(username));
                confirm.insert("ConfirmationCode".to_string(), json!(code));
                confirm.insert("Password".to_string(), json!(RESET_PASSWORD));
                if let Some(presented) = &presented {
                    confirm.insert("SecretHash".to_string(), json!(presented));
                }
                let (confirmed, payload) = dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "ConfirmForgotPassword",
                    &Value::Object(confirm),
                )
                .await;
                assert_eq!(
                    confirmed, 200,
                    "case {case:?}: the code the delivery reported must complete the \
                     reset, got {confirmed} with {payload}"
                );
                // A secretless client of the same pool reads the stored password back.
                let sign_in_client = if home == &second {
                    &neighbour.client_id
                } else {
                    &open.client_id
                };
                let token =
                    access_token(&setup, home, sign_in_client, &username, RESET_PASSWORD).await;
                let (read_back, profile) = dispatch(
                    &emulator,
                    Addressing::Anonymous,
                    "GetUser",
                    &json!({ "AccessToken": token }),
                )
                .await;
                assert_eq!(
                    read_back, 200,
                    "case {case:?}: the sign-in with the reset password must name its \
                     user, got {read_back} with {profile}"
                );
                assert_eq!(
                    profile["Username"],
                    json!(username),
                    "case {case:?}: expected the reset user's own profile, got {profile}"
                );
            }
        }
    }
}
