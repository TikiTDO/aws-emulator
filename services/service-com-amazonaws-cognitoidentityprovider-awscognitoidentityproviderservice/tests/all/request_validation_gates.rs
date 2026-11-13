// SPDX-License-Identifier: CC0-1.0

//! The request-validation and precondition gates a write of this service passes
//! before it stores anything, over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! Four rules, each asserted over a SET of inputs rather than on one recorded
//! request:
//!
//! * In a pool that signs its users in WITH an attribute value
//!   (`UsernameAttributes` "specifies whether a user can use an email address or
//!   phone number as a username when they sign up", this service's model), the
//!   submitted `Username` IS that attribute's value, so a creation under a value
//!   of no configured sign-in attribute's shape is refused with 400
//!   `InvalidParameterException` naming the value the pool expects. A pool whose
//!   users sign in with a username of their own takes the name as sent.
//! * A `phone_number` value has to be "in E.164 format"
//!   (`smithy_query(query="doc",shape="AdminCreateUserRequest",member="UserAttributes")`),
//!   a `+` and the number's digits, wherever it is submitted — on a creation or
//!   on an attribute update, by an administrator or by the user itself — and one
//!   that is not is refused with 400 `InvalidParameterException`.
//! * A token-validity member and its time unit together name a DURATION, and
//!   what each member admits is a window of durations: an access or an ID
//!   token's "duration can range from five minutes to one day" and a refresh
//!   token's "from 60 minutes to 10 years"
//!   (`smithy_query(query="shape",shape="TokenValidityUnitsType")`), whose upper
//!   bounds are each member's modelled range maximum read in seconds — the
//!   ranges being "displayed below in seconds"
//!   (`smithy_query(query="doc",shape="UpdateUserPoolClientRequest",member="AccessTokenValidity")`)
//!   while the number is counted in the unit the request chose. A
//!   configuration naming a lifetime outside its window is refused with 400
//!   `InvalidParameterException` and stores nothing.
//! * Starting a user import job whose CSV was never uploaded is refused with 400
//!   `PreconditionNotMetException` naming the job the request addressed.
//!
//! Each table row names the case it asserts, the rows differ along what each
//! rule generalises over (the pool's sign-in configuration, the submitted value,
//! the validity's unit and number, the operation that submits it), and every
//! assertion reports the actual value. The pools, app clients, users and access
//! tokens are all real, and every accepted write is read back through
//! `AdminGetUser` or `DescribeUserPoolClient`.

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
/// routing signal left. That is how a signed-in user's own client calls the
/// self-service operations, which the model marks `@optionalAuth`.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The role a user import job writes its log stream with.
const IMPORT_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// The error code every refusal of a bad parameter carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as an SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a user's own client calls a self-service operation.
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

/// Call the signed `operation` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    dispatch(router, operation, body, true).await
}

/// Call the signed `operation`, requiring success, and return its response body.
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

/// The `message` of an error body, under either casing the protocol admits.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A pool named `pool_name` carrying `configuration`, and its minted id.
async fn pool(router: &MultiServiceRouter, pool_name: &str, configuration: &Value) -> String {
    let mut body: Map<String, Value> = configuration
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("a pool configuration is a JSON object: {configuration}"));
    body.insert("PoolName".to_string(), json!(pool_name));
    ok(router, "CreateUserPool", &Value::Object(body)).await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// An app client of `user_pool_id` that allows the password sign-in this suite
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

/// `attributes` as the modelled `AttributeListType` an SDK sends.
fn attribute_list(attributes: &[(&str, &str)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// The stored user `username` names, as `AdminGetUser` renders it.
async fn admin_get_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Value {
    ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// The app client `client_id` names, as `DescribeUserPoolClient` renders it.
async fn described_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
) -> Value {
    ok(
        router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await["UserPoolClient"]
        .clone()
}

// ---------------------------------------------------------------------------
// The sign-in name a pool admits
// ---------------------------------------------------------------------------

/// One row: how the pool signs its users in, the value a creation submits as the
/// `Username`, and the sentence the creation is refused with — `None` for a
/// value the pool admits.
struct SignInNameCase {
    case: &'static str,
    configuration: Value,
    submitted: &'static str,
    refusal: Option<&'static str>,
}

/// The table. The rows span each `UsernameAttributes` configuration a pool can
/// hold (neither attribute, each one alone, both) against a submitted value of
/// each shape — an address, a number, a plain name, and a value that has the
/// shape of a phone number without being one — plus a pool that takes the name
/// as sent.
fn sign_in_name_cases() -> Vec<SignInNameCase> {
    vec![
        SignInNameCase {
            case: "a pool with no sign-in attribute takes a plain name",
            configuration: json!({}),
            submitted: "ada",
            refusal: None,
        },
        SignInNameCase {
            case: "a pool whose EMAIL is an alias still takes a plain name",
            configuration: json!({ "AliasAttributes": ["email"] }),
            submitted: "ada",
            refusal: None,
        },
        SignInNameCase {
            case: "an email sign-in pool takes an address",
            configuration: json!({ "UsernameAttributes": ["email"] }),
            submitted: "ada@lovelace.example",
            refusal: None,
        },
        SignInNameCase {
            case: "an email sign-in pool and a plain name",
            configuration: json!({ "UsernameAttributes": ["email"] }),
            submitted: "ada",
            refusal: Some("Username should be an email."),
        },
        SignInNameCase {
            case: "an email sign-in pool and a phone number",
            configuration: json!({ "UsernameAttributes": ["email"] }),
            submitted: "+15550100",
            refusal: Some("Username should be an email."),
        },
        SignInNameCase {
            case: "a phone-number sign-in pool takes a number",
            configuration: json!({ "UsernameAttributes": ["phone_number"] }),
            submitted: "+15550101",
            refusal: None,
        },
        SignInNameCase {
            case: "a phone-number sign-in pool and a plain name",
            configuration: json!({ "UsernameAttributes": ["phone_number"] }),
            submitted: "ada",
            refusal: Some("Username should be a phone number."),
        },
        SignInNameCase {
            case: "a phone-number sign-in pool and an address",
            configuration: json!({ "UsernameAttributes": ["phone_number"] }),
            submitted: "ada@lovelace.example",
            refusal: Some("Username should be a phone number."),
        },
        SignInNameCase {
            // The value has the shape of a phone number, so it IS this pool's
            // sign-in attribute's value — and is refused for the form that
            // attribute's values take rather than for naming the wrong one.
            case: "a phone-number sign-in pool and a number that is not in E.164 form",
            configuration: json!({ "UsernameAttributes": ["phone_number"] }),
            submitted: "+1555-0102",
            refusal: Some("Invalid phone number format."),
        },
        SignInNameCase {
            case: "a pool admitting both sign-in attributes takes an address",
            configuration: json!({ "UsernameAttributes": ["email", "phone_number"] }),
            submitted: "grace@hopper.example",
            refusal: None,
        },
        SignInNameCase {
            case: "a pool admitting both sign-in attributes takes a number",
            configuration: json!({ "UsernameAttributes": ["email", "phone_number"] }),
            submitted: "+15550103",
            refusal: None,
        },
        SignInNameCase {
            case: "a pool admitting both sign-in attributes and a plain name",
            configuration: json!({ "UsernameAttributes": ["email", "phone_number"] }),
            submitted: "ada",
            refusal: Some("Username should be either an email or a phone number."),
        },
    ]
}

#[tokio::test]
async fn a_creation_names_a_user_the_pools_sign_in_configuration_admits() {
    let router = router();

    for (index, row) in sign_in_name_cases().iter().enumerate() {
        let case = row.case;
        // A pool per row and per operation, so an accepted creation is never
        // the duplicate of the one the other operation made.
        for (operation, suffix) in [("AdminCreateUser", "admin"), ("SignUp", "self")] {
            let user_pool_id = pool(
                &router,
                &format!("sign-in-name-{index}-{suffix}"),
                &row.configuration,
            )
            .await;
            let client_id = client(&router, &user_pool_id, &format!("app-{index}-{suffix}")).await;

            let (status, payload) = if operation == "AdminCreateUser" {
                call(
                    &router,
                    operation,
                    &json!({ "UserPoolId": user_pool_id, "Username": row.submitted }),
                )
                .await
            } else {
                dispatch(
                    &router,
                    operation,
                    &json!({
                        "ClientId": client_id,
                        "Username": row.submitted,
                        "Password": GOOD_PASSWORD,
                    }),
                    false,
                )
                .await
            };

            match row.refusal {
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&payload), error_message(&payload)),
                        (400, INVALID_PARAMETER, message),
                        "case {case:?}: {operation} must refuse {:?}, got {status} with {payload}",
                        row.submitted
                    );
                    // A refused creation stored no user, so the pool holds none
                    // the submitted name resolves to.
                    let (status, payload) = call(
                        &router,
                        "AdminGetUser",
                        &json!({ "UserPoolId": user_pool_id, "Username": row.submitted }),
                    )
                    .await;
                    assert_eq!(
                        (status, error_code(&payload)),
                        (400, "UserNotFoundException"),
                        "case {case:?}: a refused {operation} must store no user, got {status} \
                         with {payload}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: {operation} must accept {:?}, got {status} with {payload}",
                        row.submitted
                    );
                    // The submitted name resolves to the user that was stored,
                    // whatever username the pool minted for it.
                    let read = admin_get_user(&router, &user_pool_id, row.submitted).await;
                    assert!(
                        read.get("Username").and_then(Value::as_str).is_some(),
                        "case {case:?}: the stored user must read back under the submitted \
                         name, got {read}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The form a phone number has to have
// ---------------------------------------------------------------------------

/// The sentence a `phone_number` that is not a phone number is refused with.
const INVALID_PHONE_NUMBER: &str = "Invalid phone number format.";

/// One row: a submitted `phone_number` value and whether it is one in E.164
/// form, so every operation that writes it must store it.
struct PhoneNumberCase {
    case: &'static str,
    value: &'static str,
    stored: bool,
}

/// The table. The rows span the shortest and the longest number E.164 admits, a
/// number of ordinary length, and the ways a value misses the form: no `+` at
/// all, a `+` with nothing after it, separators and spaces inside the digits, a
/// letter among them, prose, and one digit too many.
fn phone_number_cases() -> Vec<PhoneNumberCase> {
    vec![
        PhoneNumberCase {
            case: "an ordinary number",
            value: "+15550100",
            stored: true,
        },
        PhoneNumberCase {
            case: "the shortest number E.164 admits",
            value: "+1",
            stored: true,
        },
        PhoneNumberCase {
            // E.164 numbers carry at most 15 digits, country calling code
            // included.
            case: "the longest number E.164 admits",
            value: "+123456789012345",
            stored: true,
        },
        PhoneNumberCase {
            case: "one digit past the longest number E.164 admits",
            value: "+1234567890123456",
            stored: false,
        },
        PhoneNumberCase {
            case: "digits with no leading plus",
            value: "15550100",
            stored: false,
        },
        PhoneNumberCase {
            case: "a plus with no digits",
            value: "+",
            stored: false,
        },
        PhoneNumberCase {
            case: "a number carrying separators",
            value: "+1-555-0100",
            stored: false,
        },
        PhoneNumberCase {
            case: "a number carrying spaces",
            value: "+1 555 0100",
            stored: false,
        },
        PhoneNumberCase {
            case: "a letter among the digits",
            value: "+1555010a",
            stored: false,
        },
        PhoneNumberCase {
            case: "prose instead of a number",
            value: "call me",
            stored: false,
        },
    ]
}

/// Sign `username` in through an unsigned `InitiateAuth` and return its access
/// token.
async fn access_token(router: &MultiServiceRouter, client_id: &str, username: &str) -> String {
    let (status, payload) = dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a signed-in user holds an access token: {payload}"))
        .to_string()
}

/// The value `name` holds on the user `read` renders, or `None` when the user
/// holds no value for it.
fn stored_attribute(read: &Value, name: &str) -> Option<String> {
    read.get("UserAttributes")
        .and_then(Value::as_array)?
        .iter()
        .find(|attribute| attribute.get("Name").and_then(Value::as_str) == Some(name))
        .and_then(|attribute| attribute.get("Value").and_then(Value::as_str))
        .map(ToString::to_string)
}

#[tokio::test]
async fn a_phone_number_is_stored_only_in_e164_form_whichever_operation_submits_it() {
    let router = router();
    let user_pool_id = pool(&router, "phone-format-pool", &json!({})).await;
    let client_id = client(&router, &user_pool_id, "phone-format-app").await;

    for (index, row) in phone_number_cases().iter().enumerate() {
        let case = row.case;

        // A creation that submits the value as an attribute.
        let created_name = format!("created-{index}");
        let (status, payload) = call(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": created_name,
                "UserAttributes": attribute_list(&[("phone_number", row.value)]),
            }),
        )
        .await;
        if row.stored {
            assert_eq!(
                status, 200,
                "case {case:?}: AdminCreateUser must store {:?}, got {status} with {payload}",
                row.value
            );
            let read = admin_get_user(&router, &user_pool_id, &created_name).await;
            assert_eq!(
                stored_attribute(&read, "phone_number"),
                Some(row.value.to_string()),
                "case {case:?}: the created user must hold the submitted number, got {read}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, INVALID_PARAMETER, INVALID_PHONE_NUMBER),
                "case {case:?}: AdminCreateUser must refuse {:?}, got {status} with {payload}",
                row.value
            );
            let (status, payload) = call(
                &router,
                "AdminGetUser",
                &json!({ "UserPoolId": user_pool_id, "Username": created_name }),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: a refused AdminCreateUser must store no user, got {status} \
                 with {payload}"
            );
        }

        // A self-service registration that submits the same value.
        let registered_name = format!("registered-{index}");
        let (status, payload) = dispatch(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": registered_name,
                "Password": GOOD_PASSWORD,
                "UserAttributes": attribute_list(&[("phone_number", row.value)]),
            }),
            false,
        )
        .await;
        assert_eq!(
            (status == 200, status),
            (row.stored, status),
            "case {case:?}: SignUp owes the same answer for {:?}, got {status} with {payload}",
            row.value
        );
        if !row.stored {
            assert_eq!(
                (error_code(&payload), error_message(&payload)),
                (INVALID_PARAMETER, INVALID_PHONE_NUMBER),
                "case {case:?}: SignUp must word the refusal the same way, got {payload}"
            );
        }

        // An update of a STORED user's own value, by an administrator and by
        // the user itself: both write the same attribute, so both are held to
        // the same form.
        let standing = format!("standing-{index}");
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": standing }),
        )
        .await;
        ok(
            &router,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": standing,
                "Password": GOOD_PASSWORD,
                "Permanent": true,
            }),
        )
        .await;
        let token = access_token(&router, &client_id, &standing).await;

        let (status, payload) = call(
            &router,
            "AdminUpdateUserAttributes",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": standing,
                "UserAttributes": attribute_list(&[("phone_number", row.value)]),
            }),
        )
        .await;
        assert_eq!(
            (status == 200, status),
            (row.stored, status),
            "case {case:?}: AdminUpdateUserAttributes owes the same answer for {:?}, got \
             {status} with {payload}",
            row.value
        );
        if !row.stored {
            assert_eq!(
                (error_code(&payload), error_message(&payload)),
                (INVALID_PARAMETER, INVALID_PHONE_NUMBER),
                "case {case:?}: AdminUpdateUserAttributes must word the refusal the same way, \
                 got {payload}"
            );
        }

        let (status, payload) = dispatch(
            &router,
            "UpdateUserAttributes",
            &json!({
                "AccessToken": token,
                "UserAttributes": attribute_list(&[("phone_number", row.value)]),
            }),
            false,
        )
        .await;
        assert_eq!(
            (status == 200, status),
            (row.stored, status),
            "case {case:?}: UpdateUserAttributes owes the same answer for {:?}, got {status} \
             with {payload}",
            row.value
        );
        let read = admin_get_user(&router, &user_pool_id, &standing).await;
        if row.stored {
            assert_eq!(
                stored_attribute(&read, "phone_number"),
                Some(row.value.to_string()),
                "case {case:?}: the updated user must hold the submitted number, got {read}"
            );
        } else {
            assert_eq!(
                (error_code(&payload), error_message(&payload)),
                (INVALID_PARAMETER, INVALID_PHONE_NUMBER),
                "case {case:?}: UpdateUserAttributes must word the refusal the same way, got \
                 {payload}"
            );
            assert_eq!(
                stored_attribute(&read, "phone_number"),
                None,
                "case {case:?}: a refused update must write nothing, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The lifetime a token validity may name
// ---------------------------------------------------------------------------

/// The sentence a token validity outside its range is refused with.
const INVALID_TOKEN_VALIDITY: &str = "Invalid range for token validity.";

/// One row: the token-validity members an app-client request carries and whether
/// the lifetime they name is one an app client may hold.
struct ValidityCase {
    case: &'static str,
    members: Value,
    stored: bool,
}

/// The table. For each validity member the rows walk its window in several time
/// units — each documented bound expressed in its own unit and in a larger one,
/// and one step past each of them — plus a member the request leaves out, whose
/// documented default unit then decides the lifetime.
fn validity_cases() -> Vec<ValidityCase> {
    vec![
        ValidityCase {
            case: "no validity member at all",
            members: json!({}),
            stored: true,
        },
        ValidityCase {
            case: "an access token at the top of its range, in seconds",
            members: json!({
                "AccessTokenValidity": 86400,
                "TokenValidityUnits": { "AccessToken": "seconds" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "an access token at the bottom of its window, in minutes",
            members: json!({
                "AccessTokenValidity": 5,
                "TokenValidityUnits": { "AccessToken": "minutes" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "an access token at the bottom of its window, in seconds",
            members: json!({
                "AccessTokenValidity": 300,
                "TokenValidityUnits": { "AccessToken": "seconds" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "an access token one second under the bottom of its window",
            members: json!({
                "AccessTokenValidity": 299,
                "TokenValidityUnits": { "AccessToken": "seconds" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "an access token of four minutes",
            members: json!({
                "AccessTokenValidity": 4,
                "TokenValidityUnits": { "AccessToken": "minutes" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "an ID token one second under the bottom of its window",
            members: json!({
                "IdTokenValidity": 299,
                "TokenValidityUnits": { "IdToken": "seconds" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "a refresh token at the bottom of its window, in minutes",
            members: json!({
                "RefreshTokenValidity": 60,
                "TokenValidityUnits": { "RefreshToken": "minutes" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "a refresh token one minute under the bottom of its window",
            members: json!({
                "RefreshTokenValidity": 59,
                "TokenValidityUnits": { "RefreshToken": "minutes" }
            }),
            stored: false,
        },
        ValidityCase {
            // A NUMBER past the member's modelled maximum is refused by that
            // modelled constraint before any handler runs, in the constraint
            // report `app_client_validity_defaults.rs` covers; every row here
            // carries a number INSIDE the range and varies the unit, which is
            // what decides the lifetime it names.
            case: "an access token of 86400 minutes, a number inside the range",
            members: json!({
                "AccessTokenValidity": 86400,
                "TokenValidityUnits": { "AccessToken": "minutes" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "an access token of one day, named in days",
            members: json!({
                "AccessTokenValidity": 1,
                "TokenValidityUnits": { "AccessToken": "days" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "an access token of two days",
            members: json!({
                "AccessTokenValidity": 2,
                "TokenValidityUnits": { "AccessToken": "days" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "an access token of twenty-five hours",
            members: json!({
                "AccessTokenValidity": 25,
                "TokenValidityUnits": { "AccessToken": "hours" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "an access token of one more minute than a day",
            members: json!({
                "AccessTokenValidity": 1441,
                "TokenValidityUnits": { "AccessToken": "minutes" }
            }),
            stored: false,
        },
        ValidityCase {
            // "The default time unit for AccessTokenValidity in an API request
            // is hours", so 24 of them is the top of the range.
            case: "an access token of twenty-four units, the default unit being hours",
            members: json!({ "AccessTokenValidity": 24 }),
            stored: true,
        },
        ValidityCase {
            case: "an access token of twenty-five units in the default unit",
            members: json!({ "AccessTokenValidity": 25 }),
            stored: false,
        },
        ValidityCase {
            case: "an ID token of one day, named in hours",
            members: json!({
                "IdTokenValidity": 24,
                "TokenValidityUnits": { "IdToken": "hours" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "an ID token of two days",
            members: json!({
                "IdTokenValidity": 2,
                "TokenValidityUnits": { "IdToken": "days" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "a refresh token of ten years, named in days",
            members: json!({
                "RefreshTokenValidity": 3650,
                "TokenValidityUnits": { "RefreshToken": "days" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "a refresh token one day past ten years",
            members: json!({
                "RefreshTokenValidity": 3651,
                "TokenValidityUnits": { "RefreshToken": "days" }
            }),
            stored: false,
        },
        ValidityCase {
            case: "a refresh token of ten years, named in seconds",
            members: json!({
                "RefreshTokenValidity": 315360000,
                "TokenValidityUnits": { "RefreshToken": "seconds" }
            }),
            stored: true,
        },
        ValidityCase {
            // "The default time unit for RefreshTokenValidity in an API request
            // is days", so the top of the range named in the default unit is
            // the ten years above and anything larger leaves it.
            case: "a refresh token of a hundred thousand units in the default unit",
            members: json!({ "RefreshTokenValidity": 100000 }),
            stored: false,
        },
        ValidityCase {
            case: "a refresh token of ten years, named in hours",
            members: json!({
                "RefreshTokenValidity": 87600,
                "TokenValidityUnits": { "RefreshToken": "hours" }
            }),
            stored: true,
        },
        ValidityCase {
            case: "every validity at once, each inside its range",
            members: json!({
                "AccessTokenValidity": 3600,
                "IdTokenValidity": 3600,
                "RefreshTokenValidity": 30,
                "TokenValidityUnits": {
                    "AccessToken": "seconds",
                    "IdToken": "seconds",
                    "RefreshToken": "days"
                }
            }),
            stored: true,
        },
        ValidityCase {
            case: "every validity at once, one of them outside its range",
            members: json!({
                "AccessTokenValidity": 3600,
                "IdTokenValidity": 2,
                "RefreshTokenValidity": 30,
                "TokenValidityUnits": {
                    "AccessToken": "seconds",
                    "IdToken": "days",
                    "RefreshToken": "days"
                }
            }),
            stored: false,
        },
    ]
}

/// `members` beside the ids an app-client request needs.
fn client_request(members: &Value, ids: &[(&str, &str)]) -> Value {
    let mut body: Map<String, Value> = members
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("a case's members are a JSON object: {members}"));
    for (member, value) in ids {
        body.insert((*member).to_string(), json!(value));
    }
    Value::Object(body)
}

#[tokio::test]
async fn a_token_lifetime_outside_its_range_is_refused_by_both_app_client_writes() {
    let router = router();
    let user_pool_id = pool(&router, "token-validity-pool", &json!({})).await;
    let standing = client(&router, &user_pool_id, "standing-app").await;

    for (index, row) in validity_cases().iter().enumerate() {
        let case = row.case;
        let was = described_client(&router, &user_pool_id, &standing).await;

        let (status, payload) = call(
            &router,
            "CreateUserPoolClient",
            &client_request(
                &row.members,
                &[
                    ("UserPoolId", &user_pool_id),
                    ("ClientName", &format!("validity-app-{index}")),
                ],
            ),
        )
        .await;
        if row.stored {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateUserPoolClient must store the lifetime, got {status} \
                 with {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, INVALID_PARAMETER, INVALID_TOKEN_VALIDITY),
                "case {case:?}: CreateUserPoolClient must refuse the lifetime, got {status} \
                 with {payload}"
            );
        }

        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &client_request(
                &row.members,
                &[("UserPoolId", &user_pool_id), ("ClientId", &standing)],
            ),
        )
        .await;
        if row.stored {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateUserPoolClient must store the lifetime, got {status} \
                 with {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, INVALID_PARAMETER, INVALID_TOKEN_VALIDITY),
                "case {case:?}: UpdateUserPoolClient must refuse the lifetime, got {status} \
                 with {payload}"
            );
            let now = described_client(&router, &user_pool_id, &standing).await;
            assert_eq!(
                now, was,
                "case {case:?}: a refused update must leave the app client alone, got {now}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The CSV a user import job needs before it starts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn starting_an_import_job_whose_csv_never_arrived_names_that_job() {
    let router = router();
    let user_pool_id = pool(&router, "import-precondition-pool", &json!({})).await;

    // Two jobs of the same pool, so the sentence is shown to carry the job the
    // REQUEST addressed rather than a fixed value.
    for name in ["first-import", "second-import"] {
        let created = ok(
            &router,
            "CreateUserImportJob",
            &json!({
                "UserPoolId": user_pool_id,
                "JobName": name,
                "CloudWatchLogsRoleArn": IMPORT_ROLE,
            }),
        )
        .await["UserImportJob"]
            .clone();
        let job_id = created["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: a minted job id: {created}"))
            .to_string();

        let (status, payload) = call(
            &router,
            "StartUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "PreconditionNotMetException",
                format!("No csv file was uploaded for {job_id}").as_str()
            ),
            "case {name:?}: a job with no CSV must be refused by name, got {status} with \
             {payload}"
        );

        // The refusal moved nothing: the job is still waiting to be started.
        let read = ok(
            &router,
            "DescribeUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await["UserImportJob"]
            .clone();
        assert_eq!(
            read["Status"],
            json!("Created"),
            "case {name:?}: a refused start must leave the job Created, got {read}"
        );
    }
}
