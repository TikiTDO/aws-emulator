// SPDX-License-Identifier: CC0-1.0

//! `AdminRespondToAuthChallenge` over the wire, driven exactly as an SDK drives
//! it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a challenge response resumes the STORED challenge
//! session its `Session` token names — for the same user pool and app client —
//! applies the state change the challenge it is waiting on owes, ends that
//! session, and completes the sign-in with the token triple issued from stored
//! state. For `NEW_PASSWORD_REQUIRED` the state change is the user's new
//! password, stored as permanent, with the user moved to `CONFIRMED` and the
//! `userAttributes.<name>` values the response supplies written to the stored
//! user; a response that does not satisfy the challenge is refused with the
//! modelled error and writes nothing.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The temporary password every fixture user is created with, which satisfies a
/// default pool's password policy.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2024";

/// The challenge an administrator-created user has to answer.
const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

/// A `Session` token this emulator never handed out, long enough to satisfy the
/// modelled `SessionType` length of 20..4096.
const UNKNOWN_SESSION: &str = "0123456789abcdefghijklmnopqrstuv";

/// The name every fixture pool is created under (`PoolName`'s modelled pattern
/// admits no apostrophe, so a case name is not one).
const POOL_NAME: &str = "challenge-pool";

/// The shared engine, so a test can read the state its calls wrote.
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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A created pool with one app client that allows the admin password flow.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool named `name` — declaring `email` a required attribute when
/// `required_email`, and setting `UsernameConfiguration.CaseSensitive` when
/// `case_sensitive` is given — with one app client that allows
/// `ADMIN_USER_PASSWORD_AUTH`.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    required_email: bool,
    case_sensitive: Option<bool>,
) -> Fixture {
    let mut pool_body = Map::new();
    pool_body.insert("PoolName".to_string(), json!(name));
    if required_email {
        pool_body.insert(
            "Schema".to_string(),
            json!([{
                "Name": "email",
                "AttributeDataType": "String",
                "Mutable": true,
                "Required": true,
            }]),
        );
    }
    if let Some(case_sensitive) = case_sensitive {
        pool_body.insert(
            "UsernameConfiguration".to_string(),
            json!({ "CaseSensitive": case_sensitive }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(pool_body)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "challenge-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Create `username` in `user_pool_id` with the temporary password, so it owes a
/// password change at its next sign-in.
async fn create_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "TemporaryPassword": TEMPORARY_PASSWORD,
        }),
    )
    .await;
}

/// Sign `username` in with `password` through the admin password flow and return
/// the answer.
async fn admin_initiate_auth(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    username: &str,
    password: &str,
) -> (u16, Value) {
    call(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
    )
    .await
}

/// Start the `NEW_PASSWORD_REQUIRED` challenge for `username` and return the
/// `Session` token it was handed.
async fn challenge_session(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    username: &str,
) -> String {
    let (status, payload) = admin_initiate_auth(router, fixture, username, TEMPORARY_PASSWORD).await;
    assert_eq!(
        status, 200,
        "starting the challenge for {username} must succeed, got {status} with {payload}"
    );
    assert_eq!(
        payload["ChallengeName"], NEW_PASSWORD_REQUIRED,
        "an administrator-created user owes a password change, got {payload}"
    );
    payload["Session"]
        .as_str()
        .expect("a persisted challenge session")
        .to_string()
}

/// The `UserStatus` and attribute values `AdminGetUser` renders for `username`.
async fn stored_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Value {
    ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// The value `AdminGetUser` renders for the attribute `name`, or `None`.
fn attribute(user: &Value, name: &str) -> Option<String> {
    user["UserAttributes"]
        .as_array()?
        .iter()
        .find(|attribute| attribute["Name"] == name)
        .and_then(|attribute| attribute["Value"].as_str())
        .map(str::to_string)
}

/// An `AdminRespondToAuthChallenge` request body.
fn respond_body(
    user_pool_id: &str,
    client_id: &str,
    challenge_name: &str,
    session: Option<&str>,
    responses: &Map<String, Value>,
) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientId".to_string(), json!(client_id));
    body.insert("ChallengeName".to_string(), json!(challenge_name));
    if let Some(session) = session {
        body.insert("Session".to_string(), json!(session));
    }
    body.insert(
        "ChallengeResponses".to_string(),
        Value::Object(responses.clone()),
    );
    Value::Object(body)
}

/// The `ChallengeResponses` map a row sends: the new password when it has one,
/// the `USERNAME` when it names one, and one `userAttributes.<name>` entry per
/// attribute value.
fn challenge_responses(
    new_password: Option<&str>,
    username: Option<&str>,
    attributes: &[(&str, &str)],
) -> Map<String, Value> {
    let mut responses = Map::new();
    if let Some(new_password) = new_password {
        responses.insert("NEW_PASSWORD".to_string(), json!(new_password));
    }
    if let Some(username) = username {
        responses.insert("USERNAME".to_string(), json!(username));
    }
    for (name, value) in attributes {
        responses.insert(format!("userAttributes.{name}"), json!(value));
    }
    responses
}

/// One answered challenge: how the response is shaped, and what the stored user
/// must look like afterwards.
struct Answered {
    case: &'static str,
    /// Whether the pool declares `email` a required attribute.
    required_email: bool,
    /// The pool's `UsernameConfiguration.CaseSensitive`, when it sets one.
    case_sensitive: Option<bool>,
    /// The username the user is created under.
    username: &'static str,
    /// The `USERNAME` the response names, when it names one at all.
    responded_username: Option<&'static str>,
    new_password: &'static str,
    /// The `userAttributes.<name>` entries the response carries.
    attributes: &'static [(&'static str, &'static str)],
    /// The attribute values the stored user must hold afterwards.
    expected_attributes: &'static [(&'static str, &'static str)],
}

/// A `NEW_PASSWORD_REQUIRED` response completes the sign-in from stored state:
/// the chosen password becomes the stored permanent password, the user becomes
/// `CONFIRMED`, the supplied attribute values are written (and an attribute that
/// cannot be changed keeps its stored value), the session is spent, and the old
/// temporary password no longer signs in.
#[tokio::test]
async fn a_new_password_response_completes_the_sign_in_and_is_read_back() {
    let rows = [
        Answered {
            case: "the response names the user",
            required_email: false,
            case_sensitive: None,
            username: "alice",
            responded_username: Some("alice"),
            new_password: "Passw0rd!2024",
            attributes: &[],
            expected_attributes: &[],
        },
        Answered {
            case: "a password of other characters, named for the user it answers for",
            required_email: false,
            case_sensitive: None,
            username: "bob",
            responded_username: Some("bob"),
            new_password: "An0ther-Passw0rd",
            attributes: &[],
            expected_attributes: &[],
        },
        Answered {
            case: "a long password of other characters",
            required_email: false,
            case_sensitive: None,
            username: "dana",
            responded_username: Some("dana"),
            new_password: "Zx9#qwertyuiopASDFghjkl=2026",
            attributes: &[],
            expected_attributes: &[],
        },
        Answered {
            case: "a case-insensitive pool answered in another case",
            required_email: false,
            case_sensitive: Some(false),
            username: "Carol",
            responded_username: Some("carol"),
            new_password: "Passw0rd!2024",
            attributes: &[],
            expected_attributes: &[],
        },
        Answered {
            case: "the required attribute the challenge asked for",
            required_email: true,
            case_sensitive: None,
            username: "erin",
            responded_username: Some("erin"),
            new_password: "Passw0rd!2024",
            attributes: &[("email", "erin@example.com")],
            expected_attributes: &[("email", "erin@example.com")],
        },
        Answered {
            case: "a writable attribute the pool does not require",
            required_email: false,
            case_sensitive: None,
            username: "frank",
            responded_username: Some("frank"),
            new_password: "Passw0rd!2024",
            attributes: &[("given_name", "Frank"), ("locale", "en-GB")],
            expected_attributes: &[("given_name", "Frank"), ("locale", "en-GB")],
        },
    ];

    for row in rows {
        let case = row.case;
        let engine = engine();
        let router = router(&engine);
        let fixture = pool_with_client(&router, POOL_NAME, row.required_email, row.case_sensitive).await;
        create_user(&router, &fixture.user_pool_id, row.username).await;
        let session = challenge_session(&router, &fixture, row.username).await;
        let before = stored_user(&router, &fixture.user_pool_id, row.username).await;
        let stored_sub = attribute(&before, "sub").expect("the minted sub");

        let responses = challenge_responses(
            Some(row.new_password),
            row.responded_username,
            row.attributes,
        );
        let (status, payload) = call(
            &router,
            "AdminRespondToAuthChallenge",
            &respond_body(
                &fixture.user_pool_id,
                &fixture.client_id,
                NEW_PASSWORD_REQUIRED,
                Some(&session),
                &responses,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: answering the challenge must succeed, got {status} with code {:?}: \
             {payload}",
            error_code(&payload)
        );
        let result = &payload["AuthenticationResult"];
        for member in ["AccessToken", "IdToken", "RefreshToken"] {
            assert!(
                result[member].as_str().is_some_and(|token| !token.is_empty()),
                "case {case:?}: a completed sign-in returns {member}, got {payload}"
            );
        }
        assert_eq!(
            result["TokenType"], "Bearer",
            "case {case:?}: the token type is Bearer, got {payload}"
        );
        assert!(
            payload["ChallengeName"].is_null() && payload["Session"].is_null(),
            "case {case:?}: a completed sign-in carries no next challenge, got {payload}"
        );

        // The STORED user is what the response changed, read back through
        // `AdminGetUser`.
        let after = stored_user(&router, &fixture.user_pool_id, row.username).await;
        assert_eq!(
            after["UserStatus"], "CONFIRMED",
            "case {case:?}: the answered challenge confirms the user, got {after}"
        );
        for (name, expected) in row.expected_attributes {
            let actual = attribute(&after, name);
            assert_eq!(
                actual.as_deref(),
                Some(*expected),
                "case {case:?}: the stored {name} must be {expected:?}, got {actual:?} in {after}"
            );
        }
        assert_eq!(
            attribute(&after, "sub").as_deref(),
            Some(stored_sub.as_str()),
            "case {case:?}: the pool's own sub is not something a response changes, got {after}"
        );

        // The stored password is the one the user chose: it signs in without a
        // challenge, and the temporary password it replaced does not sign in.
        let (status, payload) =
            admin_initiate_auth(&router, &fixture, row.username, row.new_password).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the new password must sign in, got {status} with {payload}"
        );
        assert!(
            payload["ChallengeName"].is_null()
                && payload["AuthenticationResult"]["AccessToken"].is_string(),
            "case {case:?}: the user owes no further password change, got {payload}"
        );
        let (status, payload) =
            admin_initiate_auth(&router, &fixture, row.username, TEMPORARY_PASSWORD).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: the replaced temporary password must not sign in, got {status} with \
             {payload}"
        );

        // The session answered its challenge, so it is spent: the record stays
        // stored, marked spent, and answering with it again is refused as the
        // replay it is.
        {
            let service_state = state::service_state(&*engine).expect("this service's state");
            let guard = service_state.lock();
            assert!(
                guard
                    .auth_sessions
                    .get(&session)
                    .is_some_and(|stored| stored.spent),
                "case {case:?}: the answered session must be stored as spent, {} are stored",
                guard.auth_sessions.len()
            );
        }
        let (status, payload) = call(
            &router,
            "AdminRespondToAuthChallenge",
            &respond_body(
                &fixture.user_pool_id,
                &fixture.client_id,
                NEW_PASSWORD_REQUIRED,
                Some(&session),
                &responses,
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "NotAuthorizedException",
                "Invalid session for the user, session can only be used once."
            ),
            "case {case:?}: a spent session answers no second response, got {status} with \
             {payload}"
        );
    }
}

/// An attribute whose stored value this challenge response cannot change keeps
/// it: `sub` is the pool's own id for the user (`Mutable=false`), and a required
/// attribute that already holds a value is not modified here.
#[tokio::test]
async fn an_unchangeable_attribute_keeps_its_stored_value() {
    let engine = engine();
    let router = router(&engine);
    let fixture = pool_with_client(&router, POOL_NAME, true, None).await;
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "grace",
            "TemporaryPassword": TEMPORARY_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": "grace@example.com" }],
        }),
    )
    .await;
    let session = challenge_session(&router, &fixture, "grace").await;
    let before = stored_user(&router, &fixture.user_pool_id, "grace").await;
    let stored_sub = attribute(&before, "sub").expect("the minted sub");

    let responses = challenge_responses(
        Some("Passw0rd!2024"),
        Some("grace"),
        &[
            ("sub", "0000000000000000"),
            ("email", "someone-else@example.com"),
            ("given_name", "Grace"),
        ],
    );
    let (status, payload) = call(
        &router,
        "AdminRespondToAuthChallenge",
        &respond_body(
            &fixture.user_pool_id,
            &fixture.client_id,
            NEW_PASSWORD_REQUIRED,
            Some(&session),
            &responses,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "answering the challenge must succeed, got {status} with code {:?}: {payload}",
        error_code(&payload)
    );

    let after = stored_user(&router, &fixture.user_pool_id, "grace").await;
    for (case, name, expected) in [
        ("the pool's own id for the user", "sub", stored_sub.as_str()),
        (
            "a required attribute that already has a value",
            "email",
            "grace@example.com",
        ),
        ("a writable attribute with no value yet", "given_name", "Grace"),
    ] {
        let actual = attribute(&after, name);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: the stored {name} must be {expected:?}, got {actual:?} in {after}"
        );
    }
}

/// Where a refusal row's `Session` comes from.
#[derive(Clone, Copy, Debug)]
enum SessionSource {
    /// The request carries no `Session` at all.
    Omitted,
    /// The session this row's own challenge was started with.
    Stored,
    /// A token this emulator never handed out.
    Unknown,
    /// A session started on ANOTHER app client of the same pool.
    OtherClient,
    /// A session started in ANOTHER user pool.
    OtherPool,
}

/// One refused response: how it is shaped, and the answer it owes.
struct Refused {
    case: &'static str,
    /// Whether the pool declares `email` a required attribute.
    required_email: bool,
    session: SessionSource,
    challenge_name: &'static str,
    new_password: Option<&'static str>,
    responded_username: Option<&'static str>,
    attributes: &'static [(&'static str, &'static str)],
    /// A `UserPoolId` to send instead of this row's own pool.
    user_pool_id: Option<&'static str>,
    /// A `ClientId` to send instead of this row's own app client.
    client_id: Option<&'static str>,
    status: u16,
    code: &'static str,
}

/// A row that answers correctly, which every other row varies from.
const CORRECT: Refused = Refused {
    case: "a correct response",
    required_email: false,
    session: SessionSource::Stored,
    challenge_name: NEW_PASSWORD_REQUIRED,
    new_password: Some("Passw0rd!2024"),
    responded_username: Some("alice"),
    attributes: &[],
    user_pool_id: None,
    client_id: None,
    status: 200,
    code: "",
};

/// A response that does not satisfy the challenge is refused with the modelled
/// error and writes NOTHING: the stored user still owes its password change and
/// its session is still answerable afterwards.
#[tokio::test]
async fn a_response_that_does_not_satisfy_the_challenge_is_refused_and_writes_nothing() {
    let rows = [
        CORRECT,
        Refused {
            case: "no Session at all",
            session: SessionSource::Omitted,
            status: 400,
            code: "InvalidParameterException",
            ..CORRECT
        },
        Refused {
            // A `Session` that resolves to no stored challenge resolves no
            // challenge THIS request can answer, which is the same fact about
            // the request as a session started elsewhere — and a different one
            // from the replay of a session this emulator DID hand out.
            case: "a Session this emulator never handed out",
            session: SessionSource::Unknown,
            status: 400,
            code: "NotAuthorizedException",
            ..CORRECT
        },
        Refused {
            case: "a Session started on another app client",
            session: SessionSource::OtherClient,
            status: 400,
            code: "NotAuthorizedException",
            ..CORRECT
        },
        Refused {
            case: "a Session started in another user pool",
            session: SessionSource::OtherPool,
            status: 400,
            code: "NotAuthorizedException",
            ..CORRECT
        },
        Refused {
            // `ChallengeName` names the challenge being answered, so one that is
            // not the challenge the session is waiting on answers none: the state
            // the response carries is not the state the server holds, which is
            // what `CodeMismatchException` reports.
            case: "a challenge name the session is not waiting for",
            challenge_name: "SMS_MFA",
            status: 400,
            code: "CodeMismatchException",
            ..CORRECT
        },
        Refused {
            // "All challenges require USERNAME" (`ChallengeResponses` in this
            // service's model), so a response that names no user answers none.
            case: "no USERNAME in the challenge responses",
            responded_username: None,
            status: 400,
            code: "InvalidParameterException",
            ..CORRECT
        },
        Refused {
            case: "no NEW_PASSWORD in the challenge responses",
            new_password: None,
            status: 400,
            code: "InvalidParameterException",
            ..CORRECT
        },
        Refused {
            case: "a password shorter than the policy minimum",
            new_password: Some("Ab1!"),
            status: 400,
            code: "InvalidPasswordException",
            ..CORRECT
        },
        Refused {
            case: "a password with no digit",
            new_password: Some("Password!!"),
            status: 400,
            code: "InvalidPasswordException",
            ..CORRECT
        },
        Refused {
            case: "a password with no symbol",
            new_password: Some("Password12"),
            status: 400,
            code: "InvalidPasswordException",
            ..CORRECT
        },
        Refused {
            case: "a password with no uppercase letter",
            new_password: Some("passw0rd!!"),
            status: 400,
            code: "InvalidPasswordException",
            ..CORRECT
        },
        Refused {
            case: "a password with no lowercase letter",
            new_password: Some("PASSW0RD!!"),
            status: 400,
            code: "InvalidPasswordException",
            ..CORRECT
        },
        Refused {
            case: "a USERNAME the session was not started for",
            responded_username: Some("mallory"),
            status: 400,
            code: "NotAuthorizedException",
            ..CORRECT
        },
        Refused {
            case: "an attribute the pool's schema does not declare",
            attributes: &[("not_declared", "value")],
            status: 400,
            code: "InvalidParameterException",
            ..CORRECT
        },
        Refused {
            case: "a required attribute the response leaves unset",
            required_email: true,
            status: 400,
            code: "InvalidParameterException",
            ..CORRECT
        },
        Refused {
            case: "a user pool this emulator does not hold",
            user_pool_id: Some("us-east-1_absent"),
            status: 400,
            code: "ResourceNotFoundException",
            ..CORRECT
        },
        Refused {
            case: "an app client the pool does not hold",
            client_id: Some("absentclient"),
            status: 400,
            code: "ResourceNotFoundException",
            ..CORRECT
        },
    ];

    for row in rows {
        let case = row.case;
        let engine = engine();
        let router = router(&engine);
        let fixture = pool_with_client(&router, POOL_NAME, row.required_email, None).await;
        create_user(&router, &fixture.user_pool_id, "alice").await;
        create_user(&router, &fixture.user_pool_id, "mallory").await;
        let stored_session = challenge_session(&router, &fixture, "alice").await;

        let session = match row.session {
            SessionSource::Omitted => None,
            SessionSource::Stored => Some(stored_session.clone()),
            SessionSource::Unknown => Some(UNKNOWN_SESSION.to_string()),
            SessionSource::OtherClient => {
                let client = ok(
                    &router,
                    "CreateUserPoolClient",
                    &json!({
                        "UserPoolId": fixture.user_pool_id,
                        "ClientName": "other-client",
                        "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
                    }),
                )
                .await;
                let other = Fixture {
                    user_pool_id: fixture.user_pool_id.clone(),
                    client_id: client["UserPoolClient"]["ClientId"]
                        .as_str()
                        .expect("a minted client id")
                        .to_string(),
                };
                Some(challenge_session(&router, &other, "alice").await)
            }
            SessionSource::OtherPool => {
                let other = pool_with_client(&router, "other-pool", false, None).await;
                create_user(&router, &other.user_pool_id, "alice").await;
                Some(challenge_session(&router, &other, "alice").await)
            }
        };

        let body = respond_body(
            row.user_pool_id.unwrap_or(&fixture.user_pool_id),
            row.client_id.unwrap_or(&fixture.client_id),
            row.challenge_name,
            session.as_deref(),
            &challenge_responses(row.new_password, row.responded_username, row.attributes),
        );
        let (status, payload) = call(&router, "AdminRespondToAuthChallenge", &body).await;
        if row.status == 200 {
            assert_eq!(
                status, 200,
                "case {case:?}: must succeed, got {status} with code {:?}: {payload}",
                error_code(&payload)
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&payload)),
            (row.status, row.code),
            "case {case:?}: expected ({}, {:?}), got ({status}, {:?}) with {payload}",
            row.status,
            row.code,
            error_code(&payload)
        );

        // Nothing was written: the user still owes the password change its
        // challenge was started for, and its session is still answerable.
        let after = stored_user(&router, &fixture.user_pool_id, "alice").await;
        assert_eq!(
            after["UserStatus"], "FORCE_CHANGE_PASSWORD",
            "case {case:?}: a refused response changes no stored user, got {after}"
        );
        let mut responses = challenge_responses(Some("Corrected!2024"), Some("alice"), &[]);
        if row.required_email {
            responses.insert(
                "userAttributes.email".to_string(),
                json!("alice@example.com"),
            );
        }
        let (status, payload) = call(
            &router,
            "AdminRespondToAuthChallenge",
            &respond_body(
                &fixture.user_pool_id,
                &fixture.client_id,
                NEW_PASSWORD_REQUIRED,
                Some(&stored_session),
                &responses,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the session must still answer a corrected response, got {status} \
             with code {:?}: {payload}",
            error_code(&payload)
        );
    }
}

/// A session the clock has expired answers nothing: the refusal is the modelled
/// `NotAuthorizedException`, and the stored user is left owing its password
/// change. The window is minutes long (`AuthSessionValidity`), so the stored
/// expiry instant is rewound rather than waited out.
#[tokio::test]
async fn an_expired_session_answers_no_challenge() {
    let now = SystemTime::now();
    // Each row: case name, where the stored expiry instant is moved to, and
    // whether the response must be served.
    let rows = [
        ("expired an hour ago", now - Duration::from_secs(3600), false),
        ("expired a second ago", now - Duration::from_secs(1), false),
        ("live for another minute", now + Duration::from_secs(60), true),
    ];

    for (case, expires_at, served) in rows {
        let engine = engine();
        let router = router(&engine);
        let fixture = pool_with_client(&router, POOL_NAME, false, None).await;
        create_user(&router, &fixture.user_pool_id, "alice").await;
        let session = challenge_session(&router, &fixture, "alice").await;
        {
            let service_state = state::service_state(&*engine).expect("this service's state");
            let mut guard = service_state.lock();
            guard
                .auth_sessions
                .get_mut(&session)
                .expect("the stored session")
                .expires_at = expires_at;
        }

        let (status, payload) = call(
            &router,
            "AdminRespondToAuthChallenge",
            &respond_body(
                &fixture.user_pool_id,
                &fixture.client_id,
                NEW_PASSWORD_REQUIRED,
                Some(&session),
                &challenge_responses(Some("Passw0rd!2024"), Some("alice"), &[]),
            ),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: a live session must answer, got {status} with code {:?}: {payload}",
                error_code(&payload)
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: an expired session must be refused, got {status} with {payload}"
        );
        let after = stored_user(&router, &fixture.user_pool_id, "alice").await;
        assert_eq!(
            after["UserStatus"], "FORCE_CHANGE_PASSWORD",
            "case {case:?}: an expired session changes no stored user, got {after}"
        );
    }
}
