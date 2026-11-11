// SPDX-License-Identifier: CC0-1.0

//! What a CONFIDENTIAL app client's challenge response has to carry, and how a
//! password refusal of the challenge path is worded.
//!
//! The rules under test:
//!
//! * An app client created with a secret authenticates every challenge response
//!   with that secret: "you must provide a `SECRET_HASH` parameter in all
//!   challenge responses to an app client that has a client secret"
//!   (`smithy_query(query="doc",shape="AdminRespondToAuthChallengeRequest",member="ChallengeResponses")`).
//!   A response that carries no `SECRET_HASH` entry is refused with
//!   `NotAuthorizedException` reporting that the client is configured with a
//!   secret and no hash arrived; one that carries a value the client's secret
//!   does not produce over the response's username and the client id is refused
//!   with `NotAuthorizedException` reporting that the hash cannot be verified.
//!   Either refusal is made BEFORE the response is applied, so the user still
//!   owes its password change and the session is still answerable. A PUBLIC app
//!   client holds no secret, so it verifies nothing and accepts a response with
//!   or without the entry.
//! * A password a pool's policy refuses is reported with the clause for the
//!   requirement it missed, behind the sentence the ANSWERING API uses: the
//!   challenge-response path answers "Password does not conform to policy: ",
//!   the self-service `ChangePassword` answers "Password did not conform with
//!   policy: ". The clause is the same in both.
//! * `ChangePassword` reports a missing previous password as the parameter AWS
//!   names in that refusal (`OldPassword`), not as the wire member it travels
//!   in, and reports it before it judges the proposed password.
//!
//! Each table row names the case it asserts and the value it saw.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The temporary password every fixture user is created with.
const TEMPORARY_PASSWORD: &str = "Temp0rary!2024";

/// A password that satisfies a default pool's policy, which is what a challenge
/// response offers as the new one.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The challenge an administrator-created user has to answer.
const NEW_PASSWORD_REQUIRED: &str = "NEW_PASSWORD_REQUIRED";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
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

/// The `SecretHash` a request for `username` to the app client `client_id`
/// carries when that client's secret is `client_secret`: base64 of HMAC-SHA256,
/// keyed with the secret, over the username followed by the client id
/// (`smithy_query(query="doc",shape="SignUpRequest",member="SecretHash")`).
fn secret_hash(client_secret: &str, username: &str, client_id: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, client_secret.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{username}{client_id}").as_bytes());
    aws_smithy_types::base64::encode(tag.as_ref())
}

/// A pool with one app client that allows the admin password flow.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    /// The client's secret, for a client created with one.
    client_secret: Option<String>,
}

/// Create a pool named `name` with one app client that allows
/// `ADMIN_USER_PASSWORD_AUTH`, with a generated secret when `with_secret`.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    with_secret: bool,
) -> Fixture {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("challenge-client"));
    body.insert("GenerateSecret".to_string(), json!(with_secret));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!(["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_USER_PASSWORD_AUTH"]),
    );
    let client = ok(router, "CreateUserPoolClient", &Value::Object(body)).await["UserPoolClient"]
        .clone();
    let client_secret = client["ClientSecret"].as_str().map(str::to_string);
    assert_eq!(
        client_secret.is_some(),
        with_secret,
        "GenerateSecret={with_secret} must decide whether a secret is minted, got {client}"
    );
    Fixture {
        user_pool_id,
        client_id: client["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        client_secret,
    }
}

/// Create `username` with the temporary password, so it owes a password change,
/// and return the `Session` of the challenge its sign-in is answered with.
async fn user_owing_a_password_change(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    username: &str,
) -> String {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": username,
            "TemporaryPassword": TEMPORARY_PASSWORD,
            "MessageAction": "SUPPRESS",
        }),
    )
    .await;
    let mut parameters = Map::new();
    parameters.insert("USERNAME".to_string(), json!(username));
    parameters.insert("PASSWORD".to_string(), json!(TEMPORARY_PASSWORD));
    if let Some(secret) = fixture.client_secret.as_deref() {
        parameters.insert(
            "SECRET_HASH".to_string(),
            json!(secret_hash(secret, username, &fixture.client_id)),
        );
    }
    let started = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": Value::Object(parameters),
        }),
    )
    .await;
    assert_eq!(
        started["ChallengeName"], NEW_PASSWORD_REQUIRED,
        "an administrator-created user owes a password change, got {started}"
    );
    started["Session"]
        .as_str()
        .expect("a challenge carries a session")
        .to_string()
}

/// The `UserStatus` the stored user reports.
async fn user_status(router: &MultiServiceRouter, fixture: &Fixture, username: &str) -> String {
    ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": username }),
    )
    .await["UserStatus"]
        .as_str()
        .expect("a stored user reports its status")
        .to_string()
}

/// Answer the `NEW_PASSWORD_REQUIRED` challenge `session` with `responses`.
async fn respond(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    session: &str,
    responses: Value,
) -> (u16, Value) {
    call(
        router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": session,
            "ChallengeResponses": responses,
        }),
    )
    .await
}

/// Which `SECRET_HASH` entry a row presents.
#[derive(Debug, Clone, Copy)]
enum Presented {
    /// No entry at all.
    None,
    /// The hash this client's own secret produces for this username.
    OwnSecret,
    /// A hash another app client's secret produces.
    AnotherSecret,
    /// This client's secret over ANOTHER username.
    AnotherUsername,
    /// This client's secret over ANOTHER client id.
    AnotherClient,
    /// A value that is not a base64 tag at all.
    NotATag,
}

/// A confidential app client verifies the `SECRET_HASH` of every challenge
/// response, and a response that does not hold the secret applies nothing.
#[tokio::test]
async fn a_confidential_app_client_verifies_the_secret_hash_of_a_challenge_response() {
    let router = router();
    let confidential = pool_with_client(&router, "secret-challenge-pool", true).await;
    let secret = confidential
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");
    // A second confidential client, whose secret is the "another secret" of the
    // rows below.
    let other = pool_with_client(&router, "other-secret-pool", true).await;
    let other_secret = other
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");

    // The rows differ along WHAT the response presents for the secret: nothing,
    // the right tag, and each way a tag can be the wrong one (another secret,
    // another username, another client, no tag at all).
    let rows: &[(&str, Presented, Option<&str>)] = &[
        (
            "no SECRET_HASH entry at all",
            Presented::None,
            Some("not received"),
        ),
        ("the client's own hash", Presented::OwnSecret, None),
        (
            "a hash another app client's secret produces",
            Presented::AnotherSecret,
            Some("unverified"),
        ),
        (
            "this client's secret over another username",
            Presented::AnotherUsername,
            Some("unverified"),
        ),
        (
            "this client's secret over another client id",
            Presented::AnotherClient,
            Some("unverified"),
        ),
        (
            "a value that is not a base64 tag",
            Presented::NotATag,
            Some("unverified"),
        ),
    ];

    for (index, (case, presented, refused)) in rows.iter().enumerate() {
        let username = format!("confidential-user-{index}");
        let session = user_owing_a_password_change(&router, &confidential, &username).await;
        let hash = match presented {
            Presented::None => None,
            Presented::OwnSecret => Some(secret_hash(&secret, &username, &confidential.client_id)),
            Presented::AnotherSecret => Some(secret_hash(
                &other_secret,
                &username,
                &confidential.client_id,
            )),
            Presented::AnotherUsername => Some(secret_hash(
                &secret,
                "somebody-else",
                &confidential.client_id,
            )),
            Presented::AnotherClient => {
                Some(secret_hash(&secret, &username, &other.client_id))
            }
            // Outside base64's alphabet, so it decodes to no tag at all.
            Presented::NotATag => Some("$$not-base64$$".to_string()),
        };
        let mut responses = Map::new();
        responses.insert("USERNAME".to_string(), json!(username));
        responses.insert("NEW_PASSWORD".to_string(), json!(GOOD_PASSWORD));
        if let Some(hash) = hash {
            responses.insert("SECRET_HASH".to_string(), json!(hash));
        }

        let (status, payload) =
            respond(&router, &confidential, &session, Value::Object(responses)).await;
        let Some(refused) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a response holding the secret completes the challenge, \
                 got {status} with {payload}"
            );
            assert!(
                payload["AuthenticationResult"]["AccessToken"].is_string(),
                "case {case:?}: a completed challenge issues the token triple, got {payload}"
            );
            let status = user_status(&router, &confidential, &username).await;
            assert_eq!(
                status, "CONFIRMED",
                "case {case:?}: the answered challenge moves the user to CONFIRMED, got {status}"
            );
            continue;
        };

        let expected = if *refused == "not received" {
            format!(
                "Client {} is configured with secret but SECRET_HASH was not received",
                confidential.client_id
            )
        } else {
            format!(
                "Unable to verify secret hash for client {}",
                confidential.client_id
            )
        };
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: expected a NotAuthorizedException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The refusal applied nothing: the user still owes its password change,
        // and the password the refused response proposed signs nobody in.
        let stored = user_status(&router, &confidential, &username).await;
        assert_eq!(
            stored, "FORCE_CHANGE_PASSWORD",
            "case {case:?}: a refused response must leave the user owing its password change, \
             got {stored}"
        );
        // The session was not spent either: the same session, answered WITH the
        // secret, still completes the challenge.
        let (status, payload) = respond(
            &router,
            &confidential,
            &session,
            json!({
                "USERNAME": username,
                "NEW_PASSWORD": GOOD_PASSWORD,
                "SECRET_HASH": secret_hash(&secret, &username, &confidential.client_id),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused response must leave the session answerable, \
             got {status} with {payload}"
        );
    }
}

/// A PUBLIC app client holds no secret, so it verifies none: a response with or
/// without a `SECRET_HASH` entry completes the challenge.
#[tokio::test]
async fn a_public_app_client_verifies_no_secret_hash() {
    let router = router();
    let public = pool_with_client(&router, "public-challenge-pool", false).await;

    // The rows differ along whether the response carries an entry the client has
    // nothing to check: absent, and a stray value of each form.
    let rows: &[(&str, Option<&str>)] = &[
        ("no SECRET_HASH entry", None),
        ("a stray base64 tag", Some("c3RyYXktdGFn")),
        ("a stray value that is not base64", Some("$$stray$$")),
    ];

    for (index, (case, hash)) in rows.iter().enumerate() {
        let username = format!("public-user-{index}");
        let session = user_owing_a_password_change(&router, &public, &username).await;
        let mut responses = Map::new();
        responses.insert("USERNAME".to_string(), json!(username));
        responses.insert("NEW_PASSWORD".to_string(), json!(GOOD_PASSWORD));
        if let Some(hash) = hash {
            responses.insert("SECRET_HASH".to_string(), json!(hash));
        }
        let (status, payload) = respond(&router, &public, &session, Value::Object(responses)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a public client's challenge response completes, \
             got {status} with {payload}"
        );
        let stored = user_status(&router, &public, &username).await;
        assert_eq!(
            stored, "CONFIRMED",
            "case {case:?}: the answered challenge moves the user to CONFIRMED, got {stored}"
        );
    }
}

/// The clause a default pool's policy refuses each password with, and the
/// password that misses exactly that requirement.
///
/// The requirements are `PasswordPolicyType`'s own
/// (`smithy_query(query="shape",shape="PasswordPolicyType")`: `MinimumLength`,
/// `RequireUppercase`, `RequireLowercase`, `RequireNumbers`, `RequireSymbols`),
/// every one of which a pool created with no policy of its own enforces.
const POLICY_ROWS: &[(&str, &str, &str)] = &[
    ("shorter than the minimum length", "Ab1!", "Password not long enough"),
    (
        "no uppercase character",
        "passw0rd!2024",
        "Password must have uppercase characters",
    ),
    (
        "no lowercase character",
        "PASSW0RD!2024",
        "Password must have lowercase characters",
    ),
    (
        "no numeric character",
        "PasswordOnly!",
        "Password must have numeric characters",
    ),
    (
        "no symbol character",
        "Passw0rd2024",
        "Password must have symbol characters",
    ),
];

/// The sentence the CHALLENGE-RESPONSE path words a policy refusal with.
const CHALLENGE_PREFIX: &str = "Password does not conform to policy: ";

/// The sentence the self-service `ChangePassword` words the same refusal with.
const CHANGE_PASSWORD_PREFIX: &str = "Password did not conform with policy: ";

/// A `NEW_PASSWORD` the pool's policy refuses earns
/// `InvalidPasswordException` with the challenge path's own sentence, and the
/// same password earns `ChangePassword`'s own sentence there — the clause for
/// the requirement missed being the same in both.
#[tokio::test]
async fn a_policy_refusal_is_worded_by_the_api_that_answers_it() {
    let router = router();
    let fixture = pool_with_client(&router, "policy-wording-pool", false).await;

    for (index, (case, password, clause)) in POLICY_ROWS.iter().enumerate() {
        // The challenge path: the user owes a password change and offers this
        // password as the new one.
        let username = format!("challenge-user-{index}");
        let session = user_owing_a_password_change(&router, &fixture, &username).await;
        let (status, payload) = respond(
            &router,
            &fixture,
            &session,
            json!({ "USERNAME": username, "NEW_PASSWORD": password }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidPasswordException"),
            "case {case:?}: expected an InvalidPasswordException, got {status} with {payload}"
        );
        let expected = format!("{CHALLENGE_PREFIX}{clause}");
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?} on the challenge path, got {message:?}"
        );
        // Nothing was applied, so the user still owes its password change.
        let stored = user_status(&router, &fixture, &username).await;
        assert_eq!(
            stored, "FORCE_CHANGE_PASSWORD",
            "case {case:?}: a refused response writes nothing, got {stored}"
        );

        // The self-service path: a signed-in user proposes the same password.
        let access_token = signed_in_token(&router, &fixture, index).await;
        let (status, payload) = call(
            &router,
            "ChangePassword",
            &json!({
                "AccessToken": access_token,
                "PreviousPassword": GOOD_PASSWORD,
                "ProposedPassword": password,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidPasswordException"),
            "case {case:?}: expected an InvalidPasswordException, got {status} with {payload}"
        );
        let expected = format!("{CHANGE_PASSWORD_PREFIX}{clause}");
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?} on the self-service path, got {message:?}"
        );
    }
}

/// `ChangePassword` reports a missing previous password as the parameter AWS
/// names in that refusal, and reports it BEFORE it judges the proposed one.
#[tokio::test]
async fn change_password_names_the_parameter_aws_names_for_a_missing_previous_password() {
    let router = router();
    let fixture = pool_with_client(&router, "missing-parameter-pool", false).await;

    // The rows differ along the proposed password: one the policy accepts and
    // one it refuses for each requirement, so the missing parameter is shown to
    // be reported ahead of the policy whatever the proposal.
    let mut rows: Vec<(String, &str)> = vec![(
        "a proposed password the policy accepts".to_string(),
        "An0therPass!2025",
    )];
    rows.extend(
        POLICY_ROWS
            .iter()
            .map(|(case, password, _)| (format!("a proposed password with {case}"), *password)),
    );

    for (index, (case, proposed)) in rows.iter().enumerate() {
        let access_token = signed_in_token(&router, &fixture, index).await;
        let (status, payload) = call(
            &router,
            "ChangePassword",
            &json!({ "AccessToken": access_token, "ProposedPassword": proposed }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected an InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Missing required parameter OldPassword",
            "case {case:?}: expected the parameter AWS names, got {message:?}"
        );
    }
}

/// A `NEW_PASSWORD_REQUIRED` response that carries no `NEW_PASSWORD` entry
/// names the KEY the password has to be given under, in the same sentence the
/// challenge's other attribute refusals use.
#[tokio::test]
async fn a_challenge_response_without_the_new_password_names_the_key_it_belongs_under() {
    let router = router();
    let fixture = pool_with_client(&router, "missing-new-password-pool", false).await;

    // The rows differ along what the response DOES carry beside the missing
    // entry: nothing, the username alone, an attribute value, and a key that is
    // not the one the password travels under.
    let rows: &[(&str, &[(&str, &str)])] = &[
        ("an empty response map", &[]),
        ("the username alone", &[("USERNAME", "{username}")]),
        (
            "an attribute value beside the username",
            &[("USERNAME", "{username}"), ("userAttributes.email", "a@b.test")],
        ),
        (
            "a key that is not the one the password travels under",
            &[("USERNAME", "{username}"), ("NEWPASSWORD", GOOD_PASSWORD)],
        ),
    ];

    for (index, (case, entries)) in rows.iter().enumerate() {
        let username = format!("no-password-user-{index}");
        let session = user_owing_a_password_change(&router, &fixture, &username).await;
        let mut responses = Map::new();
        for (key, value) in entries.iter() {
            let value = if *value == "{username}" {
                username.clone()
            } else {
                (*value).to_string()
            };
            responses.insert((*key).to_string(), json!(value));
        }
        let (status, payload) = respond(&router, &fixture, &session, Value::Object(responses)).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected an InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Invalid attributes given, new password should be given with key NEW_PASSWORD",
            "case {case:?}: expected the refusal to name the key, got {message:?}"
        );
        let stored = user_status(&router, &fixture, &username).await;
        assert_eq!(
            stored, "FORCE_CHANGE_PASSWORD",
            "case {case:?}: a refused response writes nothing, got {stored}"
        );
    }
}

/// The entries a challenge response has to carry, and the state it may claim:
/// a response that names no user answers none, one that names another challenge
/// than its session is waiting on carries a state the server does not hold, and
/// a session value this emulator never handed out is no session of the request.
#[tokio::test]
async fn a_challenge_response_answers_only_the_challenge_its_own_session_is_waiting_on() {
    let router = router();
    let fixture = pool_with_client(&router, "challenge-state-pool", false).await;

    // No USERNAME, with a password the policy accepts, so the row is refused on
    // the missing entry and not on the password. The rows vary what else the
    // response carries.
    let without_username: &[(&str, Value)] = &[
        (
            "the new password alone",
            json!({ "NEW_PASSWORD": GOOD_PASSWORD }),
        ),
        (
            "the new password beside an attribute value",
            json!({ "NEW_PASSWORD": GOOD_PASSWORD, "userAttributes.given_name": "Grace" }),
        ),
    ];
    for (index, (case, responses)) in without_username.iter().enumerate() {
        let username = format!("unnamed-user-{index}");
        let session = user_owing_a_password_change(&router, &fixture, &username).await;
        let (status, payload) = respond(&router, &fixture, &session, responses.clone()).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected an InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Missing required parameter USERNAME",
            "case {case:?}: expected the missing-parameter refusal, got {message:?}"
        );
    }

    // A `ChallengeName` the session is not waiting on. The rows span other
    // values of `ChallengeNameType`
    // (`smithy_query(query="shape",shape="ChallengeNameType")`).
    for (index, challenge_name) in ["SMS_MFA", "EMAIL_OTP", "CUSTOM_CHALLENGE", "MFA_SETUP"]
        .iter()
        .enumerate()
    {
        let username = format!("other-challenge-user-{index}");
        let session = user_owing_a_password_change(&router, &fixture, &username).await;
        let (status, payload) = call(
            &router,
            "AdminRespondToAuthChallenge",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "ClientId": fixture.client_id,
                "ChallengeName": challenge_name,
                "Session": session,
                "ChallengeResponses": { "USERNAME": username, "NEW_PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "CodeMismatchException"),
            "case {challenge_name:?}: expected a CodeMismatchException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Invalid code or auth state for the user.",
            "case {challenge_name:?}: expected the auth-state refusal, got {message:?}"
        );
        // The session is untouched, so it still answers the challenge it holds.
        let (status, _) = respond(
            &router,
            &fixture,
            &session,
            json!({ "USERNAME": username, "NEW_PASSWORD": GOOD_PASSWORD }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {challenge_name:?}: the refused response must leave the session answerable"
        );
    }

    // A `Session` value this emulator never handed out, across the modelled
    // `SessionType` length range (20..4096).
    let unknown_sessions = [
        ("a session at the modelled minimum length", "a".repeat(20)),
        ("a session of a shape a generator might produce", format!("AYABe{}", "Z".repeat(60))),
        ("a session at the modelled maximum length", "s".repeat(4096)),
    ];
    for (case, session) in unknown_sessions {
        let (status, payload) = respond(
            &router,
            &fixture,
            &session,
            json!({ "USERNAME": "nobody", "NEW_PASSWORD": GOOD_PASSWORD }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: expected a NotAuthorizedException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, "Invalid session for the user.",
            "case {case:?}: expected the invalid-session refusal, got {message:?}"
        );
        assert!(
            !message.contains(&session),
            "case {case:?}: the refusal must not quote the session, got {message:?}"
        );
    }
}

/// The client-side `RespondToAuthChallenge` applies the same gate: its app
/// client's secret is verified BEFORE the session it carries is resolved, so a
/// caller that does not hold the secret learns nothing about the challenge.
#[tokio::test]
async fn the_client_side_respond_applies_the_same_secret_gate() {
    let router = router();
    let confidential = pool_with_client(&router, "client-side-secret-pool", true).await;
    let secret = confidential
        .client_secret
        .clone()
        .expect("GenerateSecret=true mints a secret");
    let username = "client-side-user";
    // A `Session` value this emulator never handed out: the gate runs ahead of
    // the session, so the row that HOLDS the secret is refused on the session
    // and every row that does not is refused on the secret.
    let session = "s".repeat(32);

    // The rows differ along the hash presented, as the administrative table
    // does, and name the refusal each earns.
    let rows: Vec<(&str, Option<String>, &str, String)> = vec![
        (
            "no SECRET_HASH entry",
            None,
            "NotAuthorizedException",
            format!(
                "Client {} is configured with secret but SECRET_HASH was not received",
                confidential.client_id
            ),
        ),
        (
            "a hash another username earns",
            Some(secret_hash(&secret, "somebody-else", &confidential.client_id)),
            "NotAuthorizedException",
            format!(
                "Unable to verify secret hash for client {}",
                confidential.client_id
            ),
        ),
        (
            "the hash this client's secret produces",
            Some(secret_hash(&secret, username, &confidential.client_id)),
            "NotAuthorizedException",
            "Invalid session for the user.".to_string(),
        ),
    ];

    for (case, hash, code, message) in rows {
        let mut responses = Map::new();
        responses.insert("USERNAME".to_string(), json!(username));
        responses.insert("EMAIL_OTP_CODE".to_string(), json!("123456"));
        if let Some(hash) = hash {
            responses.insert("SECRET_HASH".to_string(), json!(hash));
        }
        let (status, payload) = call(
            &router,
            "RespondToAuthChallenge",
            &json!({
                "ClientId": confidential.client_id,
                "ChallengeName": "EMAIL_OTP",
                "Session": session,
                "ChallengeResponses": Value::Object(responses),
            }),
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
    }
}

/// A fresh user of `fixture` that has answered its password challenge, signed
/// in: the access token of a user holding [`GOOD_PASSWORD`].
async fn signed_in_token(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    index: usize,
) -> String {
    let username = format!("self-service-user-{index}");
    let session = user_owing_a_password_change(router, fixture, &username).await;
    let answered = ok(
        router,
        "AdminRespondToAuthChallenge",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ChallengeName": NEW_PASSWORD_REQUIRED,
            "Session": session,
            "ChallengeResponses": { "USERNAME": username, "NEW_PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    answered["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("a completed challenge issues an access token")
        .to_string()
}
