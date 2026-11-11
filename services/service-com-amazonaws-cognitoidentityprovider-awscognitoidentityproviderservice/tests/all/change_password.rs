// SPDX-License-Identifier: CC0-1.0

//! `ChangePassword` over the wire, driven exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.ChangePassword`).
//!
//! The rule under test: the password a signed-in user signs in with is REPLACED
//! by the `ProposedPassword` once the caller has presented the password that
//! user holds as `PreviousPassword` — so the new password authenticates the
//! user from then on and the old one no longer does. A proposed password that
//! misses the pool's password policy is refused 400 `InvalidPasswordException`,
//! one the pool's `PasswordHistorySize` restricts the user from reusing 400
//! `PasswordHistoryPolicyViolationException`, a wrong `PreviousPassword` 400
//! `NotAuthorizedException`, an omitted one for a user that holds a password 400
//! `InvalidParameterException`, and a token that names no signed-in user 400
//! `NotAuthorizedException`; every refusal leaves the stored password standing,
//! which each row proves by signing in with it again.
//!
//! The pools, app clients, users and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `AdminCreateUser`,
//! `AdminSetUserPassword` and `InitiateAuth`, and every password change is read
//! back by signing in through `InitiateAuth`, the only reader of a stored
//! password this API has.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// Every refusal of the caller this API answers is an HTTP 400, whatever status
/// the error shape models (`src/service/error_wire.rs`).
const REFUSAL_STATUS: u16 = 400;

/// The prefix a password-policy refusal carries where the USER supplied the
/// password (`ChangePassword`, `ConfirmForgotPassword`, a new password given in
/// a challenge response).
const USER_PASSWORD_POLICY_PREFIX: &str = "Password did not conform with policy: ";

/// The sentence a password-history refusal carries. It is NOT one of the
/// password-policy clauses, so it carries no policy prefix.
const PASSWORD_REUSED_MESSAGE: &str = "Password has previously been used";

/// One emulator with just this service mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls `ChangePassword` — the
/// model marks the operation `@optionalAuth`.
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
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
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

/// A pool whose password policy is `policy` (left at the pool default when
/// `None`) and whose usernames are compared case-insensitively when asked.
/// Returns the minted `UserPoolId`.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    policy: Option<Value>,
    case_sensitive: bool,
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(policy) = policy {
        body.insert(
            "Policies".to_string(),
            json!({ "PasswordPolicy": policy }),
        );
    }
    if !case_sensitive {
        body.insert(
            "UsernameConfiguration".to_string(),
            json!({ "CaseSensitive": false }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// An app client of `user_pool_id` that allows the password sign-in this suite
/// reads a stored password back through. Returns its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "change-password-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A pool with one app client and one signed-in user holding a known password.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    username: String,
    access_token: String,
    id_token: String,
}

/// A sign-in of `username` on `client_id` with `password`: the status and body
/// `InitiateAuth` answered.
async fn sign_in(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    password: &str,
) -> (u16, Value) {
    dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
        false,
    )
    .await
}

/// Create the pool, its client and `username` holding `password` as a permanent
/// password, sign that user in as `signed_in_as` and return the fixture.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    policy: Option<Value>,
    case_sensitive: bool,
    username: &str,
    signed_in_as: &str,
    password: &str,
) -> Fixture {
    let user_pool_id = pool(router, name, policy, case_sensitive).await;
    let client_id = client(router, &user_pool_id).await;
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": password,
            "Permanent": true,
        }),
    )
    .await;
    let (status, payload) = sign_in(router, &client_id, signed_in_as, password).await;
    assert_eq!(
        status, 200,
        "signing {signed_in_as} in must succeed, got {status} with {payload}"
    );
    let result = &payload["AuthenticationResult"];
    Fixture {
        user_pool_id,
        client_id,
        username: username.to_string(),
        access_token: result["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string(),
        id_token: result["IdToken"].as_str().expect("an id token").to_string(),
    }
}

/// Call `ChangePassword` unsigned, as a signed-in user's client does. A row that
/// submits no `PreviousPassword` leaves the member out of the request entirely.
async fn change_password(
    router: &MultiServiceRouter,
    access_token: &str,
    previous: Option<&str>,
    proposed: &str,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("AccessToken".to_string(), json!(access_token));
    if let Some(previous) = previous {
        body.insert("PreviousPassword".to_string(), json!(previous));
    }
    body.insert("ProposedPassword".to_string(), json!(proposed));
    dispatch(router, "ChangePassword", &Value::Object(body), false).await
}

/// Assert that `password` is the one `username` signs in with on `client_id`.
async fn signs_in(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    password: &str,
    case: &str,
    what: &str,
) {
    let (status, payload) = sign_in(router, &fixture.client_id, &fixture.username, password).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {what} ({password:?}) must sign the user in, got {status} with {payload}"
    );
    assert!(
        payload["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some(),
        "case {case:?}: {what} ({password:?}) must mint tokens, got {payload}"
    );
}

/// Assert that `password` does NOT sign `username` in: the stored credential is
/// something else, so the sign-in is refused `NotAuthorizedException`.
async fn does_not_sign_in(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    password: &str,
    case: &str,
    what: &str,
) {
    let (status, payload) = sign_in(router, &fixture.client_id, &fixture.username, password).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: {what} ({password:?}) must no longer sign the user in, got {status} with \
         {payload}"
    );
    let code = error_code(&payload);
    assert!(
        code.ends_with("NotAuthorizedException"),
        "case {case:?}: {what} ({password:?}) must be refused NotAuthorizedException, got \
         {code:?} with {payload}"
    );
}

/// One row of the accepted-change table.
#[derive(Clone, Copy, Debug)]
struct ChangeCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the pool compares usernames case-sensitively.
    case_sensitive: bool,
    /// The username `AdminCreateUser` creates.
    username: &'static str,
    /// The username the sign-in names, which a case-insensitive pool may case
    /// differently from the stored one.
    signed_in_as: &'static str,
    /// The password the user holds before the change.
    previous: &'static str,
    /// The passwords the row proposes, in order: each change replaces the one
    /// the change before it set.
    proposed: &'static [&'static str],
}

/// The rows differ along the FORM of the proposed password (length, which
/// character classes it carries, a non-ASCII one), the COUNT of changes made in
/// a row (one, three in sequence), and whether the sign-in that minted the token
/// cased the username as the store did. Every password satisfies the default
/// pool policy — at least 8 characters with an uppercase letter, a lowercase
/// letter, a number and a symbol (Amazon Cognito Developer Guide, "Password
/// policies").
const CHANGE_CASES: &[ChangeCase] = &[
    ChangeCase {
        case: "a password of the same form replaces the one the user holds",
        case_sensitive: true,
        username: "ada",
        signed_in_as: "ada",
        previous: "Passw0rd!2024",
        proposed: &["Passw0rd!2025"],
    },
    ChangeCase {
        case: "a long password carrying every class is stored as submitted",
        case_sensitive: true,
        username: "grace",
        signed_in_as: "grace",
        previous: "Passw0rd!2024",
        proposed: &["Gr4ce-Hopper-Compiles-1952!"],
    },
    ChangeCase {
        case: "a non-ASCII password is stored and read back unchanged",
        case_sensitive: true,
        username: "edsger",
        signed_in_as: "edsger",
        previous: "Passw0rd!2024",
        proposed: &["Dijkstr4-Köln!"],
    },
    ChangeCase {
        case: "three changes in sequence each replace the password the one before set",
        case_sensitive: true,
        username: "alan",
        signed_in_as: "alan",
        previous: "Passw0rd!2024",
        proposed: &["Turing-1!a", "Turing-2!b", "Turing-3!c"],
    },
    ChangeCase {
        case: "the change lands on the stored user when the sign-in cased the username \
               differently",
        case_sensitive: false,
        username: "Barbara",
        signed_in_as: "barbara",
        previous: "Passw0rd!2024",
        proposed: &["Liskov-Sub5!"],
    },
];

#[tokio::test]
async fn change_password_replaces_the_password_the_user_signs_in_with() {
    let router = emulator();
    for (index, row) in CHANGE_CASES.iter().enumerate() {
        let case = row.case;
        let user = fixture(
            &router,
            &format!("accepted-pool-{index}"),
            None,
            row.case_sensitive,
            row.username,
            row.signed_in_as,
            row.previous,
        )
        .await;

        let mut held = row.previous;
        for proposed in row.proposed {
            let (status, payload) =
                change_password(&router, &user.access_token, Some(held), proposed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: changing {held:?} to {proposed:?} must succeed, got {status} with \
                 {payload}"
            );
            assert!(
                payload.is_null() || payload.as_object().is_some_and(Map::is_empty),
                "case {case:?}: ChangePassword models no output member, got {payload:?}"
            );
            // The stored credential is the proposed password, which is what a
            // sign-in — the only reader of a stored password — now needs.
            signs_in(&router, &user, proposed, case, "the proposed password").await;
            does_not_sign_in(&router, &user, held, case, "the replaced password").await;
            held = proposed;
        }
    }
}

/// One row of the refused-change table: a call that must leave the stored
/// password standing.
#[derive(Clone, Copy, Debug)]
struct RefusedCase {
    /// What this row asserts.
    case: &'static str,
    /// The `PreviousPassword` the call submits, or `None` to omit the member.
    previous: Option<&'static str>,
    /// The `ProposedPassword` the call submits.
    proposed: &'static str,
    /// The error code the refusal must carry.
    code: &'static str,
    /// The message the refusal must carry.
    message: &'static str,
}

/// The password every row of the refusal tables starts from.
const HELD_PASSWORD: &str = "Passw0rd!2024";

/// The rows differ along WHY the call is refused — the presented previous
/// password is another user's-style value, it is the proposed one, it is absent
/// — and each one is answered without the proposed password ever being stored.
const PREVIOUS_PASSWORD_CASES: &[RefusedCase] = &[
    RefusedCase {
        case: "a previous password that is not the one the user holds",
        previous: Some("Wr0ngPassword!"),
        proposed: "Passw0rd!2025",
        code: "NotAuthorizedException",
        message: "Incorrect username or password.",
    },
    RefusedCase {
        case: "a previous password that differs from the stored one only in case",
        previous: Some("passw0rd!2024"),
        proposed: "Passw0rd!2025",
        code: "NotAuthorizedException",
        message: "Incorrect username or password.",
    },
    RefusedCase {
        case: "the proposed password presented as the previous one",
        previous: Some("Passw0rd!2025"),
        proposed: "Passw0rd!2025",
        code: "NotAuthorizedException",
        message: "Incorrect username or password.",
    },
    RefusedCase {
        case: "no previous password at all, for a user that holds one",
        previous: None,
        proposed: "Passw0rd!2025",
        code: "InvalidParameterException",
        // Amazon Cognito names the parameter `OldPassword` in this refusal,
        // which is not the wire member it travels in
        // (`smithy_query(query="operation",operation="ChangePassword")`:
        // `PreviousPassword`).
        message: "Missing required parameter OldPassword",
    },
];

#[tokio::test]
async fn change_password_refuses_a_previous_password_the_user_does_not_hold() {
    let router = emulator();
    let user = fixture(
        &router,
        "previous-password-pool",
        None,
        true,
        "ada",
        "ada",
        HELD_PASSWORD,
    )
    .await;

    for row in PREVIOUS_PASSWORD_CASES {
        let case = row.case;
        let (status, payload) =
            change_password(&router, &user.access_token, row.previous, row.proposed).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: the refusal is a {REFUSAL_STATUS}, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with(row.code),
            "case {case:?}: expected {:?}, got {code:?} with {payload}",
            row.code
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the refusal to report {:?}, got {message:?}",
            row.message
        );
        // Nothing was written: the user still signs in with the password it
        // held, and not with the one the refused call proposed.
        signs_in(&router, &user, HELD_PASSWORD, case, "the held password").await;
        does_not_sign_in(&router, &user, row.proposed, case, "the proposed password").await;
    }
}

/// The policy the password-policy rows are judged against: every requirement
/// `PasswordPolicyType` carries, with a minimum length above the pool default
/// so the length row is this policy's and not the default's.
fn strict_policy() -> Value {
    json!({
        "MinimumLength": 12,
        "RequireUppercase": true,
        "RequireLowercase": true,
        "RequireNumbers": true,
        "RequireSymbols": true,
    })
}

/// One row of the password-policy table: the requirement a proposed password
/// misses, and the clause the refusal reports for it.
#[derive(Clone, Copy, Debug)]
struct PolicyCase {
    /// What this row asserts.
    case: &'static str,
    /// The proposed password.
    proposed: &'static str,
    /// The clause the refusal's message carries behind the user-supplied
    /// password prefix.
    clause: &'static str,
}

/// One row per requirement `PasswordPolicyType` declares
/// (`smithy_query(query="shape",shape="PasswordPolicyType")`): the minimum
/// length and the four character classes. The length rows sit on the policy's
/// own bound (11 characters, one short of the configured 12) rather than on the
/// pool default's.
const POLICY_CASES: &[PolicyCase] = &[
    PolicyCase {
        case: "one character short of MinimumLength",
        proposed: "Sh0rt-Pass!",
        clause: "Password not long enough",
    },
    PolicyCase {
        case: "a password carrying no uppercase letter",
        proposed: "all-l0wercase!",
        clause: "Password must have uppercase characters",
    },
    PolicyCase {
        case: "a password carrying no lowercase letter",
        proposed: "ALL-UPPERC4SE!",
        clause: "Password must have lowercase characters",
    },
    PolicyCase {
        case: "a password carrying no number",
        proposed: "NoNumbersHere!",
        clause: "Password must have numeric characters",
    },
    PolicyCase {
        case: "a password carrying no symbol",
        proposed: "N0SymbolsHere1",
        clause: "Password must have symbol characters",
    },
];

#[tokio::test]
async fn change_password_refuses_a_proposed_password_outside_the_pool_policy() {
    let router = emulator();
    let held = "P0licy-Compliant!";
    let user = fixture(
        &router,
        "policy-pool",
        Some(strict_policy()),
        true,
        "ada",
        "ada",
        held,
    )
    .await;

    for row in POLICY_CASES {
        let case = row.case;
        let (status, payload) =
            change_password(&router, &user.access_token, Some(held), row.proposed).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: the refusal is a {REFUSAL_STATUS}, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("InvalidPasswordException"),
            "case {case:?}: expected InvalidPasswordException, got {code:?} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message,
            format!("{USER_PASSWORD_POLICY_PREFIX}{}", row.clause),
            "case {case:?}: expected the policy clause for the missed requirement, got \
             {message:?}"
        );
        signs_in(&router, &user, held, case, "the held password").await;
    }

    // A password that satisfies every requirement of the same policy is stored,
    // so the rows above are refused for the requirement they miss and not for
    // the policy being configured at all.
    let (status, payload) =
        change_password(&router, &user.access_token, Some(held), "Str1ct-Enough!").await;
    assert_eq!(
        status, 200,
        "a password satisfying the whole policy must be accepted, got {status} with {payload}"
    );
    signs_in(
        &router,
        &user,
        "Str1ct-Enough!",
        "a password satisfying the whole policy",
        "the proposed password",
    )
    .await;
}

/// One row of the password-history table.
#[derive(Clone, Copy, Debug)]
struct HistoryCase {
    /// What this row asserts.
    case: &'static str,
    /// The pool's `PasswordHistorySize`, or `None` for a pool that configures
    /// none.
    history_size: Option<i64>,
    /// The passwords the row sets in order, starting from the one the user is
    /// created with: each one is changed to the next.
    set_in_order: &'static [&'static str],
    /// The passwords the policy must then restrict the user from setting again,
    /// the one it holds now included (it is the last password that was set).
    restricted: &'static [&'static str],
    /// The passwords the policy must leave the user free to set again.
    reusable: &'static [&'static str],
}

/// `PasswordHistorySize` "is the number of previous passwords that you want
/// Amazon Cognito to restrict each user from reusing. Users can't set a password
/// that matches any of `n` previous passwords, where `n` is the value of
/// `PasswordHistorySize`" (`PasswordPolicyType.PasswordHistorySize` in this
/// service's model) — so the restricted set is the `n` passwords the user set
/// most recently, the one it holds now being the last of them, and a password
/// set further back than the window reaches can be set again. The rows span the
/// modelled range of that member, whose maximum is 24
/// (`smithy_query(query="shape",shape="PasswordHistorySizeType")`): its lower
/// bound 0, the sizes 1, 2 and 3, its upper bound 24, and a pool that configures
/// no size at all. Each row differs along how many passwords the user has had
/// and how far back the window reaches.
const HISTORY_CASES: &[HistoryCase] = &[
    HistoryCase {
        case: "a pool that configures no history size restricts no password",
        history_size: None,
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!"],
        restricted: &[],
        reusable: &["Hist0ry-A!", "Hist0ry-B!"],
    },
    HistoryCase {
        case: "the range's lower bound restricts no password",
        history_size: Some(0),
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!"],
        restricted: &[],
        reusable: &["Hist0ry-A!", "Hist0ry-B!"],
    },
    HistoryCase {
        case: "a size of one restricts the password in use and nothing before it",
        history_size: Some(1),
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!"],
        restricted: &["Hist0ry-B!"],
        reusable: &["Hist0ry-A!"],
    },
    HistoryCase {
        case: "a size of two restricts the password in use and the one before it",
        history_size: Some(2),
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!", "Hist0ry-C!"],
        restricted: &["Hist0ry-C!", "Hist0ry-B!"],
        reusable: &["Hist0ry-A!"],
    },
    HistoryCase {
        case: "a size of three reaches one password further back",
        history_size: Some(3),
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!", "Hist0ry-C!", "Hist0ry-D!"],
        restricted: &["Hist0ry-D!", "Hist0ry-C!", "Hist0ry-B!"],
        reusable: &["Hist0ry-A!"],
    },
    HistoryCase {
        case: "the range's upper bound restricts every password this user has had",
        history_size: Some(24),
        set_in_order: &["Hist0ry-A!", "Hist0ry-B!", "Hist0ry-C!"],
        restricted: &["Hist0ry-C!", "Hist0ry-B!", "Hist0ry-A!"],
        reusable: &[],
    },
];

#[tokio::test]
async fn change_password_refuses_a_password_the_history_policy_restricts() {
    let router = emulator();
    for (index, row) in HISTORY_CASES.iter().enumerate() {
        let case = row.case;
        let policy = row.history_size.map(|size| {
            json!({
                "MinimumLength": 8,
                "RequireUppercase": true,
                "RequireLowercase": true,
                "RequireNumbers": true,
                "RequireSymbols": true,
                "PasswordHistorySize": size,
            })
        });
        let (first, changes) = row
            .set_in_order
            .split_first()
            .expect("a row sets at least the password the user is created with");
        let user = fixture(
            &router,
            &format!("history-pool-{index}"),
            policy,
            true,
            "ada",
            "ada",
            first,
        )
        .await;

        let mut held = *first;
        for proposed in changes {
            let (status, payload) =
                change_password(&router, &user.access_token, Some(held), proposed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: changing {held:?} to {proposed:?} must succeed, got {status} with \
                 {payload}"
            );
            held = proposed;
        }

        for restricted in row.restricted {
            let (status, payload) =
                change_password(&router, &user.access_token, Some(held), restricted).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: setting {restricted:?} again is a {REFUSAL_STATUS}, got {status} \
                 with {payload}"
            );
            let code = error_code(&payload);
            assert!(
                code.ends_with("PasswordHistoryPolicyViolationException"),
                "case {case:?}: setting {restricted:?} again expects \
                 PasswordHistoryPolicyViolationException, got {code:?} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, PASSWORD_REUSED_MESSAGE,
                "case {case:?}: setting {restricted:?} again must report \
                 {PASSWORD_REUSED_MESSAGE:?}, got {message:?}"
            );
            // The refusal leaves the password the user holds standing.
            signs_in(&router, &user, held, case, "the held password").await;
        }

        for reusable in row.reusable {
            let (status, payload) =
                change_password(&router, &user.access_token, Some(held), reusable).await;
            assert_eq!(
                status, 200,
                "case {case:?}: setting {reusable:?} again must succeed, got {status} with \
                 {payload}"
            );
            signs_in(&router, &user, reusable, case, "the reused password").await;
            held = reusable;
        }
    }
}

/// The history window is the POOL's, not one operation's: a password an
/// administrator sets is judged against the same `PasswordHistorySize` window,
/// and the password it replaces joins the same history a later self-service
/// change is judged against. `AdminSetUserPassword` declares
/// `PasswordHistoryPolicyViolationException` for exactly that
/// (`smithy_query(query="operation",operation="AdminSetUserPassword")`).
#[tokio::test]
async fn the_history_window_is_read_whichever_operation_sets_the_password() {
    let router = emulator();
    let policy = json!({
        "MinimumLength": 8,
        "RequireUppercase": true,
        "RequireLowercase": true,
        "RequireNumbers": true,
        "RequireSymbols": true,
        "PasswordHistorySize": 2,
    });
    let first = "Shar3d-A!";
    let user = fixture(
        &router,
        "shared-window-pool",
        Some(policy),
        true,
        "ada",
        "ada",
        first,
    )
    .await;

    // The user changes its own password, so the window now holds both.
    let second = "Shar3d-B!";
    let (status, payload) = change_password(&router, &user.access_token, Some(first), second).await;
    assert_eq!(
        status, 200,
        "the self-service change must succeed, got {status} with {payload}"
    );

    /// One administrative set, and what the shared window owes it.
    struct AdminCase {
        case: &'static str,
        password: &'static str,
        restricted: bool,
    }
    // The rows differ along where the submitted password sits relative to the
    // window: the password in use, the one before it (both inside a window of
    // two), and one the user has never had.
    let cases = [
        AdminCase {
            case: "the password the user holds now",
            password: second,
            restricted: true,
        },
        AdminCase {
            case: "the password it held before that, still inside the window",
            password: first,
            restricted: true,
        },
        AdminCase {
            case: "a password this user has never had",
            password: "Shar3d-C!",
            restricted: false,
        },
    ];
    for row in cases {
        let case = row.case;
        let (status, payload) = dispatch(
            &router,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": user.user_pool_id,
                "Username": user.username,
                "Password": row.password,
                "Permanent": true,
            }),
            true,
        )
        .await;
        if row.restricted {
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: setting {:?} administratively is a {REFUSAL_STATUS}, got {status} \
                 with {payload}",
                row.password
            );
            let code = error_code(&payload);
            assert!(
                code.ends_with("PasswordHistoryPolicyViolationException"),
                "case {case:?}: expected PasswordHistoryPolicyViolationException, got {code:?} \
                 with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, PASSWORD_REUSED_MESSAGE,
                "case {case:?}: expected the refusal to report {PASSWORD_REUSED_MESSAGE:?}, got \
                 {message:?}"
            );
            // The refused call wrote nothing: the user still holds the password
            // its own change set.
            signs_in(&router, &user, second, case, "the held password").await;
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: setting {:?} administratively must succeed, got {status} with \
                 {payload}",
                row.password
            );
            signs_in(&router, &user, row.password, case, "the password set").await;
        }
    }

    // The administrative set pushed the password it replaced into the same
    // history, so the user's own change is judged against a window of
    // "Shar3d-C!" (in use) and "Shar3d-B!" (one back), and the password two
    // changes back is free again.
    let (status, payload) =
        change_password(&router, &user.access_token, Some("Shar3d-C!"), second).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "the password the administrative set replaced is still inside the window, got {status} \
         with {payload}"
    );
    let code = error_code(&payload);
    assert!(
        code.ends_with("PasswordHistoryPolicyViolationException"),
        "expected PasswordHistoryPolicyViolationException, got {code:?} with {payload}"
    );
    let (status, payload) =
        change_password(&router, &user.access_token, Some("Shar3d-C!"), first).await;
    assert_eq!(
        status, 200,
        "the password outside the window is free again, got {status} with {payload}"
    );
    signs_in(
        &router,
        &user,
        first,
        "the password outside the window",
        "the reused password",
    )
    .await;
}

/// The highest `PasswordHistorySize` a pool can configure, and the first value
/// past it: `PasswordHistorySizeType` is modelled with a range of 0..24
/// (`smithy_query(query="shape",shape="PasswordHistorySizeType")`), so 25 is
/// outside the member's own constraint and no pool can be created with it.
#[tokio::test]
async fn a_history_size_past_the_modelled_maximum_configures_no_pool() {
    let router = emulator();
    let (status, payload) = dispatch(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": "history-size-past-the-maximum",
            "Policies": { "PasswordPolicy": { "PasswordHistorySize": 25 } },
        }),
        true,
    )
    .await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "a PasswordHistorySize of 25 is outside the modelled range, got {status} with {payload}"
    );
}

/// One row of the token table: a value submitted as the `AccessToken` that names
/// no signed-in user.
#[derive(Clone, Copy, Debug)]
struct TokenCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the row submits the sign-in's id token; otherwise it submits a
    /// value this emulator never issued at all.
    id_token: bool,
    /// The message the refusal must carry.
    message: &'static str,
}

/// The rows differ along WHY the token names no signed-in user: it was never
/// issued at all, or it was issued as a token of another kind.
const TOKEN_CASES: &[TokenCase] = &[
    TokenCase {
        case: "a token this emulator never issued",
        id_token: false,
        message: "Invalid Access Token",
    },
    TokenCase {
        case: "the sign-in's id token, which is not the access token the member asks for",
        id_token: true,
        message: "Invalid Access Token",
    },
];

#[tokio::test]
async fn change_password_refuses_a_token_that_names_no_signed_in_user() {
    let router = emulator();
    let user = fixture(
        &router,
        "token-pool",
        None,
        true,
        "ada",
        "ada",
        HELD_PASSWORD,
    )
    .await;

    for row in TOKEN_CASES {
        let case = row.case;
        let token = if row.id_token {
            user.id_token.clone()
        } else {
            // Satisfies `TokenModelType`'s pattern, so the request reaches the
            // handler and the refusal is the handler's own.
            "not-a-token-this-emulator-issued".to_string()
        };
        let (status, payload) =
            change_password(&router, &token, Some(HELD_PASSWORD), "Passw0rd!2025").await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: the refusal is a {REFUSAL_STATUS}, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {code:?} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the refusal to report {:?}, got {message:?}",
            row.message
        );
        // The user whose password it would have changed still holds the one it
        // had.
        signs_in(&router, &user, HELD_PASSWORD, case, "the held password").await;
    }
}
