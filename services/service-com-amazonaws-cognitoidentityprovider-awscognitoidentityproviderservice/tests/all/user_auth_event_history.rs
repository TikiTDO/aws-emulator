// SPDX-License-Identifier: CC0-1.0

//! The threat-protection event history of a user pool, over the wire: the events a
//! pool with its add-ons switched on LOGS about what was attempted against it, the
//! history `AdminListUserAuthEvents` reports, and the feedback the two feedback
//! operations leave on one event.
//!
//! The rules under test:
//!
//! * a pool whose threat protection is active logs ONE authentication event per call
//!   that attempts an authentication, a registration, a password change, a password
//!   recovery or a code resend against it, carrying the kind of attempt it was
//!   (`EventType`), the outcome that call reached (`EventResponse`: it completed,
//!   it was refused, or it answered with a challenge that is still outstanding), the
//!   challenge it checked and the risk the pool evaluated; a call that CONTINUES an
//!   attempt finishes that attempt's own event instead of logging a second one, and a
//!   refresh-token exchange presents a credential of a session already evaluated, so
//!   it is no event of its own;
//! * `AdminListUserAuthEvents` reports a user's logged events MOST RECENT FIRST, in
//!   pages of `MaxResults` (60 where the request asks for none or for zero) resumed by
//!   `NextToken`, and reports the same history however the request names the user;
//! * `AdminUpdateAuthEventFeedback` and `UpdateAuthEventFeedback` record a submitter's
//!   verdict on ONE logged event, which the listing then renders as that event's
//!   `EventFeedback` — the administrative operation as `Admin`, the user's own
//!   one-click operation as the user, and only when it presents the `FeedbackToken`
//!   Amazon Cognito minted for that event;
//! * a pool whose add-ons are off runs no threat protection and logs no history, so
//!   every one of the three operations is refused on it, as is one naming a pool, a
//!   user or an event that does not exist.
//!
//! Every pool, app client, user, token and event here is real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `AdminCreateUser`,
//! `AdminSetUserPassword`, `SignUp`, `InitiateAuth`, `AdminInitiateAuth`,
//! `ForgotPassword`, `ResendConfirmationCode` and `ChangePassword`, and the history is
//! read back only through `AdminListUserAuthEvents`.
//!
//! Each table row names the case it asserts.

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

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// Every refusal of the caller this API answers is an HTTP 400, whatever status the
/// error shape models (`src/service/error_wire.rs`).
const REFUSAL_STATUS: u16 = 400;

/// Passwords satisfying a default pool's password policy, and one no user holds.
const GOOD_PASSWORD: &str = "Passw0rd!2024";
const NEXT_PASSWORD: &str = "N3xtPassw0rd!";
const WRONG_PASSWORD: &str = "N0tTheP4ssword!";

/// The administrator-created user every row attempts against, and the one a
/// self-service registration creates.
const USERNAME: &str = "history-user";
const SIGNUP_USERNAME: &str = "signup-user";

/// A `UserPoolIdType` value (pattern `^[\w-]+_[0-9a-zA-Z]+$`) that no pool of these
/// tests carries, and a username no pool holds.
const ABSENT_USER_POOL_ID: &str = "us-east-1_n0suchpool";
const ABSENT_USERNAME: &str = "nobody-at-all";

/// An `EventIdType` value (pattern `^[\w+-]+$`) that names no logged event.
const ABSENT_EVENT_ID: &str = "n0-such-event";

/// A `TokenModelType` value (pattern `^[A-Za-z0-9-_=.]+$`) that this emulator minted
/// for no event, so it authorizes feedback on none.
const FOREIGN_FEEDBACK_TOKEN: &str = "n0t-a-feedback-token";

/// The `AdvancedSecurityModeType` values threat protection is ON under, and the one it
/// is off under (`smithy_query(query="shape",shape="AdvancedSecurityModeType")`).
const ENFORCED: &str = "ENFORCED";
const AUDIT: &str = "AUDIT";
const OFF: &str = "OFF";

/// The messages the refusals of these operations carry.
const USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE: &str = "This is an add on feature. Please update \
     AdvancedSecurityMode for your user pool to access this API.";
const FEEDBACK_TOKEN_INVALID_MESSAGE: &str = "Invalid session provided";
const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";
const PAGINATION_TOKEN_INVALID_MESSAGE: &str = "Invalid pagination token given.";

/// `QueryLimitType`'s modelled maximum, which is the page a request that asks for no
/// `MaxResults` — or for zero — is served.
const MAX_QUERY_LIMIT: usize = 60;

/// One emulator with just this service mounted, beside the engine it is mounted on —
/// which is where a test reads a delivered one-time password from, because no response
/// carries it.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// One emulator with just this service mounted.
fn emulator() -> MultiServiceRouter {
    router_and_engine().0
}

/// Dispatch `operation` with `body` and return its status and JSON response body. A
/// `signed` call carries a credential scope, as the SDK sends every administrative
/// operation; an unsigned one is addressed to the emulator's own address, which is how
/// a user's own client calls the self-service operations.
async fn dispatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if signed {
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call the unsigned `operation`, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, false).await;
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

/// A created pool, its app client and its administrator-created user.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool whose `UserPoolAddOns` run threat protection in `mode` (and none at
/// all where `mode` is `None`), one app client, and one `CONFIRMED` user holding
/// [`GOOD_PASSWORD`] and a verified email address.
///
/// `UserPoolAddOns` is a Plus-tier feature of a pool, so a pool configuring it is
/// created on that tier.
async fn given_pool(router: &MultiServiceRouter, mode: Option<&str>) -> Fixture {
    let mut pool = Map::new();
    pool.insert("PoolName".to_string(), json!("auth-event-pool"));
    pool.insert("AutoVerifiedAttributes".to_string(), json!(["email"]));
    if let Some(mode) = mode {
        pool.insert("UserPoolTier".to_string(), json!("PLUS"));
        pool.insert(
            "UserPoolAddOns".to_string(),
            json!({ "AdvancedSecurityMode": mode }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(pool)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "auth-event-client",
            "GenerateSecret": false,
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
            ],
        }),
    )
    .await;
    let client_id = created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": USERNAME,
            "MessageAction": "SUPPRESS",
            "UserAttributes": [
                { "Name": "email", "Value": "history-user@example.com" },
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
            "Username": USERNAME,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// One call a row drives against the fixture.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// `InitiateAuth` `USER_PASSWORD_AUTH` with the stored password: a sign-in that
    /// completes, whose tokens the later steps of the row use.
    SignIn,
    /// The same with a password the user does not hold.
    WrongPassword,
    /// `AdminInitiateAuth` `ADMIN_USER_PASSWORD_AUTH`, with the stored password and
    /// with a wrong one: the administrative surface of the same attempt.
    AdminSignIn,
    AdminWrongPassword,
    /// `InitiateAuth` `REFRESH_TOKEN_AUTH` presenting the token the sign-in issued.
    Refresh,
    /// `SignUp` of [`SIGNUP_USERNAME`], and `ResendConfirmationCode` for it.
    SignUp,
    ResendCode,
    /// `ForgotPassword` for the administrator-created user.
    ForgotPassword,
    /// `ChangePassword` with the access token the sign-in issued, and the same with a
    /// previous password the user does not hold.
    ChangePassword,
    WrongPreviousPassword,
}

/// Drive `steps` against `fixture`, asserting each call was answered the way its own
/// step says — so a row can only pass while the traffic it claims to have driven was
/// really served or really refused.
async fn drive(router: &MultiServiceRouter, fixture: &Fixture, case: &str, steps: &[Step]) {
    let mut access_token = String::new();
    let mut refresh_token = String::new();
    for step in steps {
        match step {
            Step::SignIn => {
                let answered = ok_anonymous(
                    router,
                    "InitiateAuth",
                    &json!({
                        "ClientId": fixture.client_id,
                        "AuthFlow": "USER_PASSWORD_AUTH",
                        "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": GOOD_PASSWORD },
                    }),
                )
                .await;
                access_token = answered["AuthenticationResult"]["AccessToken"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                refresh_token = answered["AuthenticationResult"]["RefreshToken"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !access_token.is_empty() && !refresh_token.is_empty(),
                    "case {case:?}: a completed sign-in must issue tokens, got {answered}"
                );
            }
            Step::WrongPassword => {
                refused(
                    router,
                    case,
                    "InitiateAuth",
                    &json!({
                        "ClientId": fixture.client_id,
                        "AuthFlow": "USER_PASSWORD_AUTH",
                        "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": WRONG_PASSWORD },
                    }),
                    false,
                )
                .await;
            }
            Step::AdminSignIn => {
                ok(router, "AdminInitiateAuth", &admin_sign_in(fixture, GOOD_PASSWORD)).await;
            }
            Step::AdminWrongPassword => {
                refused(
                    router,
                    case,
                    "AdminInitiateAuth",
                    &admin_sign_in(fixture, WRONG_PASSWORD),
                    true,
                )
                .await;
            }
            Step::Refresh => {
                ok_anonymous(
                    router,
                    "InitiateAuth",
                    &json!({
                        "ClientId": fixture.client_id,
                        "AuthFlow": "REFRESH_TOKEN_AUTH",
                        "AuthParameters": { "REFRESH_TOKEN": refresh_token },
                    }),
                )
                .await;
            }
            Step::SignUp => {
                ok_anonymous(
                    router,
                    "SignUp",
                    &json!({
                        "ClientId": fixture.client_id,
                        "Username": SIGNUP_USERNAME,
                        "Password": GOOD_PASSWORD,
                        "UserAttributes": [
                            { "Name": "email", "Value": "signup-user@example.com" },
                        ],
                    }),
                )
                .await;
            }
            Step::ResendCode => {
                ok_anonymous(
                    router,
                    "ResendConfirmationCode",
                    &json!({ "ClientId": fixture.client_id, "Username": SIGNUP_USERNAME }),
                )
                .await;
            }
            Step::ForgotPassword => {
                ok_anonymous(
                    router,
                    "ForgotPassword",
                    &json!({ "ClientId": fixture.client_id, "Username": USERNAME }),
                )
                .await;
            }
            Step::ChangePassword => {
                ok_anonymous(
                    router,
                    "ChangePassword",
                    &json!({
                        "AccessToken": access_token,
                        "PreviousPassword": GOOD_PASSWORD,
                        "ProposedPassword": NEXT_PASSWORD,
                    }),
                )
                .await;
            }
            Step::WrongPreviousPassword => {
                refused(
                    router,
                    case,
                    "ChangePassword",
                    &json!({
                        "AccessToken": access_token,
                        "PreviousPassword": WRONG_PASSWORD,
                        "ProposedPassword": NEXT_PASSWORD,
                    }),
                    false,
                )
                .await;
            }
        }
    }
}

/// The `AdminInitiateAuth` body of an administrative password sign-in with
/// `password`.
fn admin_sign_in(fixture: &Fixture, password: &str) -> Value {
    json!({
        "UserPoolId": fixture.user_pool_id,
        "ClientId": fixture.client_id,
        "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": USERNAME, "PASSWORD": password },
    })
}

/// Call `operation` requiring it to be REFUSED, and return the refusal body.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    signed: bool,
) -> Value {
    let (status, payload) = dispatch(router, operation, body, signed).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: {operation} must be refused with HTTP {REFUSAL_STATUS}, got \
         {status} with {payload}"
    );
    payload
}

/// The history `AdminListUserAuthEvents` reports for `username`, as the listing serves
/// it, with `max_results` and `next_token` as the request carries them.
async fn history(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    let answered = ok(
        router,
        "AdminListUserAuthEvents",
        &Value::Object(body),
    )
    .await;
    let events = answered
        .get("AuthEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry an AuthEvents member: {answered}"));
    let next = answered
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (events, next)
}

/// One challenge a logged event records: the `ChallengeName` it checked and the
/// `ChallengeResponse` it got.
type Challenge = (&'static str, &'static str);

/// One logged event as a row asserts it: its `EventType`, its `EventResponse` and the
/// challenges it records.
type Expected = (&'static str, &'static str, &'static [Challenge]);

/// One row of the logging table: what it asserts, the calls it drives, the user whose
/// history it reads, and the history it expects newest first.
type Row = (&'static str, &'static [Step], &'static str, &'static [Expected]);

/// One logged event as the assertions compare it.
type Summary = (String, String, Vec<(String, String)>);

/// One logged event as a row asserts it: its `EventType`, its `EventResponse` and the
/// `ChallengeResponses` it carries, in order.
fn summarised(event: &Value) -> Summary {
    let challenges = event
        .get("ChallengeResponses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|challenge| {
            (
                challenge["ChallengeName"].as_str().unwrap_or_default().to_string(),
                challenge["ChallengeResponse"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    (
        event["EventType"].as_str().unwrap_or_default().to_string(),
        event["EventResponse"].as_str().unwrap_or_default().to_string(),
        challenges,
    )
}

/// RULE: a pool with threat protection active logs one event per call that attempts an
/// authentication, a registration, a password change, a recovery or a code resend,
/// carrying what the call attempted and the outcome it reached; a refresh-token
/// exchange is no event of its own.
///
/// The rows span what the rule generalises over rather than one recorded request:
/// WHICH kind of attempt was made (every `EventType` member,
/// `smithy_query(query="shape",shape="EventType")`), WHICH operation made it (the
/// self-service and the administrative surface), HOW it ended (completed, refused on
/// the credential) and HOW MANY calls were made (none, one, several).
#[tokio::test]
async fn every_attempted_call_is_logged_as_the_event_it_attempted() {
    // The `EventType` and `ChallengeName` / `ChallengeResponse` values are the model's
    // (`smithy_query(query="shape",shape="EventType")`,
    // `smithy_query(query="shape",shape="ChallengeName")`).
    let rows: &[Row] = &[
        (
            "nothing attempted",
            &[],
            USERNAME,
            &[],
        ),
        (
            "a password sign-in that completed",
            &[Step::SignIn],
            USERNAME,
            &[("SignIn", "Pass", &[("Password", "Success")])],
        ),
        (
            "a password sign-in refused on the credential",
            &[Step::WrongPassword],
            USERNAME,
            &[("SignIn", "Fail", &[("Password", "Failure")])],
        ),
        (
            "the administrative surface of the same sign-in",
            &[Step::AdminSignIn],
            USERNAME,
            &[("SignIn", "Pass", &[("Password", "Success")])],
        ),
        (
            "the administrative sign-in refused on the credential",
            &[Step::AdminWrongPassword],
            USERNAME,
            &[("SignIn", "Fail", &[("Password", "Failure")])],
        ),
        (
            "several sign-ins, newest first",
            &[Step::WrongPassword, Step::SignIn],
            USERNAME,
            &[
                ("SignIn", "Pass", &[("Password", "Success")]),
                ("SignIn", "Fail", &[("Password", "Failure")]),
            ],
        ),
        (
            "a self-service registration",
            &[Step::SignUp],
            SIGNUP_USERNAME,
            &[("SignUp", "Pass", &[])],
        ),
        (
            "a registration whose code was sent again",
            &[Step::SignUp, Step::ResendCode],
            SIGNUP_USERNAME,
            &[("ResendCode", "Pass", &[]), ("SignUp", "Pass", &[])],
        ),
        (
            "a password recovery",
            &[Step::ForgotPassword],
            USERNAME,
            &[("ForgotPassword", "Pass", &[])],
        ),
        (
            "a password change by the signed-in user",
            &[Step::SignIn, Step::ChangePassword],
            USERNAME,
            &[
                ("PasswordChange", "Pass", &[("Password", "Success")]),
                ("SignIn", "Pass", &[("Password", "Success")]),
            ],
        ),
        (
            "a password change refused on the previous password",
            &[Step::SignIn, Step::WrongPreviousPassword],
            USERNAME,
            &[
                ("PasswordChange", "Fail", &[("Password", "Failure")]),
                ("SignIn", "Pass", &[("Password", "Success")]),
            ],
        ),
        (
            "a refresh-token exchange, which is no event of its own",
            &[Step::SignIn, Step::Refresh],
            USERNAME,
            &[("SignIn", "Pass", &[("Password", "Success")])],
        ),
    ];

    for (case, steps, username, expected) in rows {
        let router = emulator();
        let fixture = given_pool(&router, Some(ENFORCED)).await;
        drive(&router, &fixture, case, steps).await;

        let (events, next) = history(&router, &fixture.user_pool_id, username, None, None).await;
        assert!(
            next.is_none(),
            "case {case:?}: a history inside one page must carry no NextToken, got {next:?}"
        );
        let found: Vec<Summary> = events.iter().map(summarised).collect();
        let wanted: Vec<Summary> = expected
            .iter()
            .map(|(event_type, response, challenges)| {
                (
                    (*event_type).to_string(),
                    (*response).to_string(),
                    challenges
                        .iter()
                        .map(|(name, result)| ((*name).to_string(), (*result).to_string()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the history {wanted:?}, got {found:?} from {events:?}"
        );

        // Every logged event carries the pool's own evaluation of it and an id of its
        // own, whatever the attempt was.
        for event in &events {
            let event_id = event["EventId"].as_str().unwrap_or_default();
            assert!(
                !event_id.is_empty(),
                "case {case:?}: a logged event must carry an EventId, got {event}"
            );
            assert!(
                event["CreationDate"].is_number(),
                "case {case:?}: a logged event must carry a CreationDate, got {event}"
            );
            assert_eq!(
                event["EventRisk"]["RiskDecision"],
                json!("NoRisk"),
                "case {case:?}: expected the pool's evaluation of this event, got {event}"
            );
            assert_eq!(
                event["EventRisk"]["CompromisedCredentialsDetected"],
                json!(false),
                "case {case:?}: expected no compromised credentials, got {event}"
            );
            assert!(
                event.get("EventFeedback").is_none(),
                "case {case:?}: an event nobody left feedback on must carry none, got {event}"
            );
        }
        let ids: Vec<&str> = events
            .iter()
            .map(|event| event["EventId"].as_str().unwrap_or_default())
            .collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: every logged event must carry its own id, got {ids:?}"
        );
    }
}

/// RULE: the history is served MOST RECENT FIRST in pages of `MaxResults`, resumed by
/// `NextToken`, and a request that asks for no page size — or for zero — is served the
/// modelled maximum.
///
/// The rows span the page sizes the rule generalises over: each published bound of
/// `QueryLimitType`'s range 0..60 (`smithy_query(query="operation",
/// operation="AdminListUserAuthEvents")`), a size inside it, and no size at all.
#[tokio::test]
async fn the_history_is_paginated_newest_first() {
    let router = emulator();
    let fixture = given_pool(&router, Some(AUDIT)).await;
    // Six sign-ins of the same user: more than the smallest page this suite asks for,
    // so a page boundary falls inside the history.
    let attempts = 6;
    for _ in 0..attempts {
        drive(&router, &fixture, "paged history", &[Step::SignIn]).await;
    }

    let (all, next) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
    assert_eq!(
        all.len(),
        attempts,
        "case \"the whole history\": expected {attempts} events, got {} from {all:?}",
        all.len()
    );
    assert!(
        next.is_none(),
        "case \"the whole history\": a history inside one page must carry no NextToken, \
         got {next:?}"
    );
    let order: Vec<f64> = all
        .iter()
        .map(|event| event["CreationDate"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] >= pair[1]),
        "case \"the whole history\": expected the events most recent first, got the \
         creation dates {order:?}"
    );
    let ordered_ids: Vec<String> = all
        .iter()
        .map(|event| event["EventId"].as_str().unwrap_or_default().to_string())
        .collect();

    for (case, page_size) in [
        ("one event per page", Some(1)),
        ("two events per page", Some(2)),
        ("the modelled maximum", Some(MAX_QUERY_LIMIT as i32)),
        ("a page size of zero", Some(0)),
        ("no page size at all", None),
    ] {
        let expected_page = page_size
            .filter(|size| *size > 0)
            .map_or(MAX_QUERY_LIMIT, |size| size as usize)
            .min(attempts);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let (events, next) = history(
                &router,
                &fixture.user_pool_id,
                USERNAME,
                page_size,
                token.as_deref(),
            )
            .await;
            pages += 1;
            assert!(
                events.len() <= expected_page,
                "case {case:?}: a page must hold at most {expected_page} events, got {}",
                events.len()
            );
            walked.extend(
                events
                    .iter()
                    .map(|event| event["EventId"].as_str().unwrap_or_default().to_string()),
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= attempts,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            walked, ordered_ids,
            "case {case:?}: walking the pages must serve the whole history once, in \
             order; expected {ordered_ids:?}, got {walked:?}"
        );
    }

    // The same history, named by the user's own `sub` instead of its username: a
    // `Username` "must be the sub of a local user" as well
    // (`AdminListUserAuthEvents.Username`).
    let user = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": USERNAME }),
    )
    .await;
    let sub = user["UserAttributes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|attribute| attribute["Name"] == json!("sub"))
        .and_then(|attribute| attribute["Value"].as_str().map(str::to_string))
        .expect("a stored user carries its sub");
    let (by_sub, _) = history(&router, &fixture.user_pool_id, &sub, None, None).await;
    let ids_by_sub: Vec<String> = by_sub
        .iter()
        .map(|event| event["EventId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids_by_sub, ordered_ids,
        "case \"named by sub\": expected the same history {ordered_ids:?}, got {ids_by_sub:?}"
    );

    // A token this listing did not mint resumes nothing.
    let payload = refused(
        &router,
        "a pagination token of no page",
        "AdminListUserAuthEvents",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": USERNAME,
            "NextToken": "bm90LWEtdG9rZW4=",
        }),
        true,
    )
    .await;
    assert_eq!(
        error_code(&payload),
        "InvalidParameterException",
        "case \"a pagination token of no page\": expected InvalidParameterException, got {payload}"
    );
    assert_eq!(
        error_message(&payload),
        PAGINATION_TOKEN_INVALID_MESSAGE,
        "case \"a pagination token of no page\": expected the pagination refusal \
         sentence, got {payload}"
    );
}

/// RULE: feedback is recorded on ONE logged event and the listing renders it as that
/// event's `EventFeedback` — submitted by the administrative operation as `Admin`, and
/// by the user's own operation only when it presents the `FeedbackToken` minted for
/// that event; the latest verdict is the one the event carries.
///
/// The rows span both `FeedbackValueType` values
/// (`smithy_query(query="shape",shape="FeedbackValueType")`) and both orders of
/// submitting them, so no row depends on one recorded verdict.
#[tokio::test]
async fn feedback_is_recorded_on_the_event_it_names() {
    for (case, verdicts) in [
        ("a session evaluated valid", vec!["Valid"]),
        ("a session evaluated invalid", vec!["Invalid"]),
        ("a verdict replaced by its opposite", vec!["Valid", "Invalid"]),
        ("the opposite replaced again", vec!["Invalid", "Valid"]),
    ] {
        let router = emulator();
        let fixture = given_pool(&router, Some(ENFORCED)).await;
        drive(&router, &fixture, case, &[Step::SignIn, Step::WrongPassword]).await;
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        assert_eq!(
            events.len(),
            2,
            "case {case:?}: the two sign-ins must have logged two events, got {events:?}"
        );
        let subject = events[0]["EventId"]
            .as_str()
            .expect("a logged event carries an id")
            .to_string();
        let untouched = events[1]["EventId"]
            .as_str()
            .expect("a logged event carries an id")
            .to_string();

        for verdict in &verdicts {
            ok(
                &router,
                "AdminUpdateAuthEventFeedback",
                &json!({
                    "UserPoolId": fixture.user_pool_id,
                    "Username": USERNAME,
                    "EventId": subject,
                    "FeedbackValue": verdict,
                }),
            )
            .await;
        }

        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        let feedback = &events[0]["EventFeedback"];
        assert_eq!(
            events[0]["EventId"],
            json!(subject),
            "case {case:?}: the history must still be served newest first, got {events:?}"
        );
        assert_eq!(
            feedback["FeedbackValue"],
            json!(verdicts.last().copied().unwrap_or_default()),
            "case {case:?}: expected the latest verdict on the event, got {feedback}"
        );
        assert_eq!(
            feedback["Provider"],
            json!("Admin"),
            "case {case:?}: expected the administrative submitter, got {feedback}"
        );
        assert!(
            feedback["FeedbackDate"].is_number(),
            "case {case:?}: submitted feedback must carry its date, got {feedback}"
        );
        // Feedback is left on ONE event: the other event of the same user carries
        // none.
        let other = events
            .iter()
            .find(|event| event["EventId"] == json!(untouched))
            .unwrap_or_else(|| panic!("case {case:?}: the other event must still be there"));
        assert!(
            other.get("EventFeedback").is_none(),
            "case {case:?}: feedback on one event must leave the others alone, got {other}"
        );

        // The user's own one-click operation is authorized by the `FeedbackToken`
        // Amazon Cognito minted for the event, which this emulator delivers to nobody:
        // a token it minted for no event authorizes feedback on none, and the stored
        // verdict stands.
        let payload = dispatch(
            &router,
            "UpdateAuthEventFeedback",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "Username": USERNAME,
                "EventId": subject,
                "FeedbackToken": FOREIGN_FEEDBACK_TOKEN,
                "FeedbackValue": "Valid",
            }),
            false,
        )
        .await;
        assert_eq!(
            payload.0, REFUSAL_STATUS,
            "case {case:?}: a foreign feedback token must be refused, got {payload:?}"
        );
        assert_eq!(
            error_code(&payload.1),
            "NotAuthorizedException",
            "case {case:?}: expected NotAuthorizedException, got {:?}",
            payload.1
        );
        assert_eq!(
            error_message(&payload.1),
            FEEDBACK_TOKEN_INVALID_MESSAGE,
            "case {case:?}: expected the invalid-token refusal sentence, got {:?}",
            payload.1
        );
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        assert_eq!(
            events[0]["EventFeedback"]["FeedbackValue"],
            json!(verdicts.last().copied().unwrap_or_default()),
            "case {case:?}: a refused submission must leave the stored verdict standing, \
             got {:?}",
            events[0]
        );
    }
}

/// RULE: a pool that runs no threat protection logs no history, so each of the three
/// operations is refused on it; so is one naming a pool, a user or an event that does
/// not exist.
///
/// The rows span the resource that is missing and the operation it is missing for,
/// including the add-on modes threat protection is on and off under
/// (`smithy_query(query="shape",shape="AdvancedSecurityModeType")`).
#[tokio::test]
async fn the_history_operations_are_refused_where_there_is_none() {
    for (case, mode) in [
        ("a pool that configures no add-ons at all", None),
        ("a pool whose add-ons are off", Some(OFF)),
    ] {
        let router = emulator();
        let fixture = given_pool(&router, mode).await;
        // Traffic served while threat protection is off logs nothing.
        drive(&router, &fixture, case, &[Step::SignIn, Step::WrongPassword]).await;

        for operation in [
            "AdminListUserAuthEvents",
            "AdminUpdateAuthEventFeedback",
            "UpdateAuthEventFeedback",
        ] {
            let payload = refused(
                &router,
                case,
                operation,
                &request_for(operation, &fixture.user_pool_id, USERNAME, ABSENT_EVENT_ID),
                operation != "UpdateAuthEventFeedback",
            )
            .await;
            assert_eq!(
                error_code(&payload),
                "UserPoolAddOnNotEnabledException",
                "case {case:?}: {operation} on a pool without threat protection must \
                 answer UserPoolAddOnNotEnabledException, got {payload}"
            );
            assert_eq!(
                error_message(&payload),
                USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE,
                "case {case:?}: expected the add-ons refusal sentence, got {payload}"
            );
        }

        // Switching threat protection ON reports an EMPTY history: the traffic served
        // while it was off logged nothing to find.
        ok(
            &router,
            "UpdateUserPool",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "UserPoolTier": "PLUS",
                "UserPoolAddOns": { "AdvancedSecurityMode": ENFORCED },
            }),
        )
        .await;
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        assert!(
            events.is_empty(),
            "case {case:?}: a pool that logged nothing while its add-ons were off must \
             report an empty history, got {events:?}"
        );
    }

    let router = emulator();
    let fixture = given_pool(&router, Some(ENFORCED)).await;
    drive(&router, &fixture, "the missing-resource rows", &[Step::SignIn]).await;
    let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
    let event_id = events[0]["EventId"]
        .as_str()
        .expect("a logged event carries an id")
        .to_string();

    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "a pool nothing created",
            "AdminListUserAuthEvents",
            ABSENT_USER_POOL_ID,
            USERNAME,
            "ResourceNotFoundException",
        ),
        (
            "a user the pool does not hold",
            "AdminListUserAuthEvents",
            "",
            ABSENT_USERNAME,
            "UserNotFoundException",
        ),
        (
            "feedback for a pool nothing created",
            "AdminUpdateAuthEventFeedback",
            ABSENT_USER_POOL_ID,
            USERNAME,
            "ResourceNotFoundException",
        ),
        (
            "feedback for a user the pool does not hold",
            "AdminUpdateAuthEventFeedback",
            "",
            ABSENT_USERNAME,
            "UserNotFoundException",
        ),
        (
            "the user's own feedback for a user the pool does not hold",
            "UpdateAuthEventFeedback",
            "",
            ABSENT_USERNAME,
            "UserNotFoundException",
        ),
    ];

    for (case, operation, user_pool_id, username, expected) in rows {
        let user_pool_id = if user_pool_id.is_empty() {
            fixture.user_pool_id.as_str()
        } else {
            user_pool_id
        };
        let payload = refused(
            &router,
            case,
            operation,
            &request_for(operation, user_pool_id, username, &event_id),
            *operation != "UpdateAuthEventFeedback",
        )
        .await;
        assert_eq!(
            error_code(&payload),
            *expected,
            "case {case:?}: expected {expected}, got {payload}"
        );
        if *expected == "UserNotFoundException" {
            assert_eq!(
                error_message(&payload),
                USER_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the user refusal sentence, got {payload}"
            );
        }
    }

    // An event id that names no event of this user's history: the resource the
    // feedback would be left on does not exist.
    for operation in ["AdminUpdateAuthEventFeedback", "UpdateAuthEventFeedback"] {
        let payload = refused(
            &router,
            "an event nothing logged",
            operation,
            &request_for(
                operation,
                &fixture.user_pool_id,
                USERNAME,
                ABSENT_EVENT_ID,
            ),
            operation != "UpdateAuthEventFeedback",
        )
        .await;
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case \"an event nothing logged\": {operation} must answer \
             ResourceNotFoundException, got {payload}"
        );
    }
}

/// The one-time password a challenge session delivered, read out of the emulator's own
/// state because nothing in the response carries it.
fn session_code(engine: &Arc<engine::InMemoryEngine>, session: &str) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .auth_sessions
        .get(session)
        .and_then(|stored| stored.code.as_ref())
        .map(|code| code.code.clone())
        .expect("the challenge delivered a code")
}

/// The `FeedbackToken` the pool minted for the logged event `event_id`, read out of the
/// emulator's own state the way a user reads it out of the notification email: it is a
/// secret of that event, so no response carries it.
fn feedback_token(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    event_id: &str,
) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| {
            user.auth_events
                .iter()
                .find(|event| event.event_id == event_id)
        })
        .map(|event| event.feedback_token.clone())
        .expect("a logged event carries the feedback token minted for it")
}

/// RULE: a user's own one-click feedback is recorded on the event whose
/// `FeedbackToken` it presents, and is reported as submitted by the user.
///
/// The rows span both `FeedbackValueType` values and an event that already carries an
/// administrator's verdict, which the user's own submission replaces.
#[tokio::test]
async fn the_user_s_own_feedback_is_recorded_against_its_token() {
    for (case, submitted_first, verdict) in [
        ("a user trusting the session", None, "Valid"),
        ("a user distrusting the session", None, "Invalid"),
        (
            "a user replacing the administrator's verdict",
            Some("Valid"),
            "Invalid",
        ),
    ] {
        let (router, engine) = router_and_engine();
        let fixture = given_pool(&router, Some(ENFORCED)).await;
        drive(&router, &fixture, case, &[Step::SignIn]).await;
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        let event_id = events[0]["EventId"]
            .as_str()
            .expect("a logged event carries an id")
            .to_string();

        if let Some(verdict) = submitted_first {
            ok(
                &router,
                "AdminUpdateAuthEventFeedback",
                &json!({
                    "UserPoolId": fixture.user_pool_id,
                    "Username": USERNAME,
                    "EventId": event_id,
                    "FeedbackValue": verdict,
                }),
            )
            .await;
        }

        let token = feedback_token(&engine, &fixture.user_pool_id, USERNAME, &event_id);
        ok_anonymous(
            &router,
            "UpdateAuthEventFeedback",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "Username": USERNAME,
                "EventId": event_id,
                "FeedbackToken": token,
                "FeedbackValue": verdict,
            }),
        )
        .await;

        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        let feedback = &events[0]["EventFeedback"];
        assert_eq!(
            feedback["FeedbackValue"],
            json!(verdict),
            "case {case:?}: expected the user's verdict on the event, got {feedback}"
        );
        assert_eq!(
            feedback["Provider"],
            json!("User"),
            "case {case:?}: expected the user as the submitter, got {feedback}"
        );
        assert!(
            feedback["FeedbackDate"].is_number(),
            "case {case:?}: submitted feedback must carry its date, got {feedback}"
        );
    }
}

/// The same pool as [`given_pool`] with threat protection enforced, configured for the
/// CHOICE-BASED sign-in: it delivers a one-time password and leaves the challenge
/// outstanding, which is what makes one attempt span two calls.
async fn given_choice_based_pool(router: &MultiServiceRouter) -> Fixture {
    let fixture = given_pool(router, Some(ENFORCED)).await;
    ok(
        router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "UserPoolTier": "PLUS",
            "UserPoolAddOns": { "AdvancedSecurityMode": ENFORCED },
            "Policies": {
                "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] }
            },
        }),
    )
    .await;
    ok(
        router,
        "UpdateUserPoolClient",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "ClientId": fixture.client_id,
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_USER_AUTH",
            ],
        }),
    )
    .await;
    fixture
}

/// Start one choice-based sign-in of [`USERNAME`] and return the `Session` its
/// outstanding challenge is waiting on.
async fn start_choice_based_sign_in(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    case: &str,
) -> String {
    let started = ok_anonymous(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": fixture.client_id,
            "AuthFlow": "USER_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "PREFERRED_CHALLENGE": "EMAIL_OTP" },
        }),
    )
    .await;
    started["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the sign-in must leave a challenge"))
        .to_string()
}

/// RULE: a call that CONTINUES a sign-in finishes that sign-in's own event — the
/// attempt stays one event, which carries the result of each challenge it checked and
/// is `InProgress` only while a challenge is outstanding.
///
/// The rows span how the challenge was answered: correctly, and with a one-time
/// password the challenge did not deliver.
#[tokio::test]
async fn a_challenge_response_finishes_the_sign_in_it_continues() {
    for (case, answer_correctly, response, result) in [
        ("the challenge answered", true, "Pass", "Success"),
        ("the challenge answered wrongly", false, "Fail", "Failure"),
    ] {
        let (router, engine) = router_and_engine();
        let fixture = given_choice_based_pool(&router).await;

        let started = start_choice_based_sign_in(&router, &fixture, case).await;
        let session = started;

        // While the challenge is outstanding the attempt is logged as in progress.
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        let found: Vec<Summary> = events.iter().map(summarised).collect();
        assert_eq!(
            found,
            vec![("SignIn".to_string(), "InProgress".to_string(), Vec::new())],
            "case {case:?}: expected one outstanding sign-in, got {found:?}"
        );
        let started_id = events[0]["EventId"]
            .as_str()
            .expect("a logged event carries an id")
            .to_string();

        let delivered = session_code(&engine, &session);
        let code = if answer_correctly {
            delivered.clone()
        } else {
            // A code of the same shape that is not the one delivered.
            delivered
                .chars()
                .map(|digit| if digit == '0' { '1' } else { '0' })
                .collect()
        };
        let answered = json!({
            "ClientId": fixture.client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": session,
            "ChallengeResponses": { "USERNAME": USERNAME, "EMAIL_OTP_CODE": code },
        });
        if answer_correctly {
            ok_anonymous(&router, "RespondToAuthChallenge", &answered).await;
        } else {
            refused(&router, case, "RespondToAuthChallenge", &answered, false).await;
        }

        // The response FINISHED the attempt the sign-in started: one event still, now
        // carrying the outcome and the challenge it checked.
        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        let found: Vec<Summary> = events.iter().map(summarised).collect();
        assert_eq!(
            found,
            vec![(
                "SignIn".to_string(),
                response.to_string(),
                vec![("Mfa".to_string(), result.to_string())],
            )],
            "case {case:?}: expected the sign-in's own event finished, got {found:?}"
        );
        assert_eq!(
            events[0]["EventId"],
            json!(started_id),
            "case {case:?}: the finished event must be the one the sign-in started, got \
             {:?}",
            events[0]
        );
    }
}

/// RULE: the attempt a response continues is the one WAITING ON the session it
/// answered, so a user with two sign-ins in flight has each response finish its own
/// attempt's event.
#[tokio::test]
async fn a_response_finishes_the_attempt_whose_session_it_answers() {
    let case = "two sign-ins in flight, the first one answered";
    let (router, engine) = router_and_engine();
    let fixture = given_choice_based_pool(&router).await;

    let first = start_choice_based_sign_in(&router, &fixture, case).await;
    let second = start_choice_based_sign_in(&router, &fixture, case).await;
    assert_ne!(
        first, second,
        "case {case:?}: two sign-ins must be waiting on sessions of their own"
    );
    let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
    assert_eq!(
        events.len(),
        2,
        "case {case:?}: two sign-ins must have logged two events, got {events:?}"
    );
    // Newest first, so the event of the FIRST sign-in is the last of the two.
    let first_event = events[1]["EventId"]
        .as_str()
        .expect("a logged event carries an id")
        .to_string();
    let second_event = events[0]["EventId"]
        .as_str()
        .expect("a logged event carries an id")
        .to_string();

    let code = session_code(&engine, &first);
    ok_anonymous(
        &router,
        "RespondToAuthChallenge",
        &json!({
            "ClientId": fixture.client_id,
            "ChallengeName": "EMAIL_OTP",
            "Session": first,
            "ChallengeResponses": { "USERNAME": USERNAME, "EMAIL_OTP_CODE": code },
        }),
    )
    .await;

    let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
    assert_eq!(
        events.len(),
        2,
        "case {case:?}: the response must finish an attempt rather than log another, \
         got {events:?}"
    );
    let outcome = |event_id: &str| -> Summary {
        events
            .iter()
            .find(|event| event["EventId"] == json!(event_id))
            .map(summarised)
            .unwrap_or_else(|| panic!("case {case:?}: {event_id} must still be in the history"))
    };
    assert_eq!(
        outcome(&first_event),
        (
            "SignIn".to_string(),
            "Pass".to_string(),
            vec![("Mfa".to_string(), "Success".to_string())],
        ),
        "case {case:?}: the answered sign-in's own event must be the one finished"
    );
    assert_eq!(
        outcome(&second_event),
        ("SignIn".to_string(), "InProgress".to_string(), Vec::new()),
        "case {case:?}: the sign-in nobody answered must still be outstanding"
    );
}

/// RULE: a call this emulator refuses because the flow is not written yet was refused
/// by the EMULATOR rather than evaluated by the pool, so the pool's history gains
/// nothing from it.
///
/// The rows span both sign-in surfaces, each asked for a flow neither serves.
#[tokio::test]
async fn a_flow_this_emulator_does_not_serve_logs_no_event() {
    for (case, operation, signed) in [
        ("the self-service surface", "InitiateAuth", false),
        ("the administrative surface", "AdminInitiateAuth", true),
    ] {
        let router = emulator();
        let fixture = given_pool(&router, Some(ENFORCED)).await;
        let mut body = json!({
            "ClientId": fixture.client_id,
            // A modelled `AuthFlowType` member this emulator has not implemented.
            "AuthFlow": "USER_SRP_AUTH",
            "AuthParameters": { "USERNAME": USERNAME, "SRP_A": "00" },
        });
        if signed {
            body["UserPoolId"] = json!(fixture.user_pool_id);
        }
        let (status, payload) = dispatch(&router, operation, &body, signed).await;
        assert_eq!(
            status, 501,
            "case {case:?}: {operation} with an unwritten flow must answer 501, got \
             {status} with {payload}"
        );

        let (events, _) = history(&router, &fixture.user_pool_id, USERNAME, None, None).await;
        assert!(
            events.is_empty(),
            "case {case:?}: a call the emulator refused as unimplemented must log no \
             event, got {events:?}"
        );
    }
}

/// The request body `operation` takes for the pool, user and event named.
fn request_for(operation: &str, user_pool_id: &str, username: &str, event_id: &str) -> Value {
    match operation {
        "AdminListUserAuthEvents" => json!({
            "UserPoolId": user_pool_id,
            "Username": username,
        }),
        "AdminUpdateAuthEventFeedback" => json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "EventId": event_id,
            "FeedbackValue": "Valid",
        }),
        _ => json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "EventId": event_id,
            "FeedbackToken": FOREIGN_FEEDBACK_TOKEN,
            "FeedbackValue": "Valid",
        }),
    }
}
