// SPDX-License-Identifier: CC0-1.0

//! How this service NORMALIZES what it stores, over the wire, driven exactly as
//! an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rules under test, each asserted over a table whose rows differ along what
//! the rule generalises over rather than over one recorded request:
//!
//! * EMPTY IS ABSENT. A value submitted as the empty string is stored as no
//!   value at all, so a later read omits the member: a user attribute written
//!   empty is listed for no user, and a group created or updated with an empty
//!   `Description` carries none. The rows span the writers (`AdminCreateUser`,
//!   `SignUp`, `AdminUpdateUserAttributes`, `CreateGroup`, `UpdateGroup`), the
//!   members (a custom attribute, a standard one, a group description) and the
//!   value (absent, empty, held).
//! * THE POOL MINTS THE USERNAME ITS SIGN-IN CONFIGURATION IMPLIES. A pool whose
//!   `UsernameAttributes` make an attribute the sign-in name mints an opaque
//!   username and stores the submitted value under that attribute; a pool whose
//!   `UsernameConfiguration.CaseSensitive` is false lower-cases the submitted
//!   name; any other pool stores it as sent.
//! * ONE USER, EVERY NAME IT ANSWERS TO. `Username` "is typically your user's
//!   username, but it can be any of their alias attributes. If username isn't an
//!   alias attribute in your user pool, this value must be the sub of a local
//!   user or the username of a user from a third-party IdP"
//!   (`smithy_query(query="doc",shape="AdminGetUserRequest",member="Username")`),
//!   so the stored username, the `sub`, an active alias value and the federated
//!   username of a linked identity all resolve the same stored user.
//! * A POOL ALWAYS APPLIES AN ACCOUNT-RECOVERY CONFIGURATION, so a create that
//!   carries none is described with the service default.
//! * A DEVICE IS LISTED WHILE THE USER REMEMBERS IT, so a device whose
//!   remembered status is cleared leaves the self-service listing, which
//!   answers for the devices the user remembers, while the admin listing keeps
//!   reporting it as registered; a forgotten device is gone from both.
//! * `ConfirmDevice` ACCEPTS ONLY AN UNCONFIRMED KEY OF THE CALLER, so a key
//!   nothing minted, one already confirmed and one another user holds are all
//!   invalid parameters of the request.
//! * A PASSWORD RESET NEEDS A USER THAT OWNS A PASSWORD, so a user that has not
//!   confirmed its sign-up and one still holding the temporary password it was
//!   created with are refused.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeMap;
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

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The device credentials a confirm REGISTERS: "a Secure Remote Password (SRP)
/// value that your application generates when you register a user's device"
/// (`smithy_query(query="shape",shape="DeviceSecretVerifierConfigType")`), which
/// `ConfirmDevice` is served only with.
fn device_credentials() -> Value {
    json!({
        "PasswordVerifier": "ZGV2aWNlLXBhc3N3b3JkLXZlcmlmaWVy",
        "Salt": "ZGV2aWNlLXNhbHQ",
    })
}

/// The provider a `ProviderUserIdentifierType` names for a user of the pool
/// itself: "The `ProviderName` should be set to `Cognito` for users in Cognito
/// user pools"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="DestinationUser")`).
const NATIVE_PROVIDER: &str = "Cognito";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
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

/// Create a pool carrying `members` beside its name, and return its minted
/// `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, members: Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(members) = members.as_object() {
        for (member, value) in members {
            body.insert(member.clone(), value.clone());
        }
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client on `user_pool_id` that allows the password flows these
/// tests drive, and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "normalization-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// `AdminCreateUser` with `attributes`, returning the rendered `User`.
async fn create_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> Value {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attribute_list(attributes),
        }),
    )
    .await;
    created["User"].clone()
}

/// A created user holding a PERMANENT password, so it is `CONFIRMED` and can
/// sign in. Returns its stored username.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> String {
    let created = create_user(router, user_pool_id, username, attributes).await;
    let stored = created["Username"]
        .as_str()
        .expect("a stored username")
        .to_string();
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": stored,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    stored
}

/// An `AttributeListType` for `attributes`.
fn attribute_list(attributes: &[(&str, &str)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// The attributes `AdminGetUser` reports for `username`, keyed by name.
async fn stored_attributes(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> BTreeMap<String, String> {
    let read = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    read["UserAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminGetUser must report an attribute list, got {read}"))
        .iter()
        .map(|attribute| {
            (
                attribute["Name"]
                    .as_str()
                    .expect("an attribute name")
                    .to_string(),
                attribute["Value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Sign `username` in and return its `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    device_key: Option<&str>,
) -> Value {
    let mut parameters = Map::new();
    parameters.insert("USERNAME".to_string(), json!(username));
    parameters.insert("PASSWORD".to_string(), json!(GOOD_PASSWORD));
    if let Some(device_key) = device_key {
        parameters.insert("DEVICE_KEY".to_string(), json!(device_key));
    }
    let payload = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": Value::Object(parameters),
        }),
    )
    .await;
    payload["AuthenticationResult"].clone()
}

/// The device keys a listing reports, in the order it served them.
fn listed_keys(payload: &Value) -> Vec<String> {
    payload["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("a device listing must carry Devices, got {payload}"))
        .iter()
        .map(|device| {
            device["DeviceKey"]
                .as_str()
                .expect("a listed device key")
                .to_string()
        })
        .collect()
}

/// One row of the empty-value table: what a writer submits for a member, and
/// what the stored resource then carries.
struct EmptyRow {
    case: &'static str,
    /// The value submitted for the member, or `None` to leave it out.
    submitted: Option<&'static str>,
    /// What a read must report: `None` where the member must be ABSENT.
    stored: Option<&'static str>,
}

/// A value submitted empty is stored as no value at all, so the member is absent
/// from every later read — exactly as if the request had left it out — while a
/// non-empty value is stored as sent.
///
/// The rows span the value (left out, empty, non-empty) and the table is driven
/// through every writer of a user attribute and of a group description.
#[tokio::test]
async fn an_empty_value_is_stored_as_no_value_at_all() {
    let rows = [
        EmptyRow {
            case: "the member left out",
            submitted: None,
            stored: None,
        },
        EmptyRow {
            case: "the member submitted empty",
            submitted: Some(""),
            stored: None,
        },
        EmptyRow {
            case: "the member submitted with a value",
            submitted: Some("a value"),
            stored: Some("a value"),
        },
        EmptyRow {
            case: "the member submitted with a value of one character",
            submitted: Some("x"),
            stored: Some("x"),
        },
    ];

    let router = router();
    let user_pool_id = pool(&router, "empty-pool", json!({})).await;
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [{ "Name": "note", "AttributeDataType": "String" }],
        }),
    )
    .await;
    let client_id = client(&router, &user_pool_id).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let submitted: Vec<(&str, &str)> = row
            .submitted
            .map(|value| vec![("custom:note", value), ("given_name", value)])
            .unwrap_or_default();

        // A CREATE writes the submitted attributes.
        let created_name = format!("created-{index}");
        create_user(&router, &user_pool_id, &created_name, &submitted).await;
        let stored = stored_attributes(&router, &user_pool_id, &created_name).await;
        for attribute in ["custom:note", "given_name"] {
            assert_eq!(
                stored.get(attribute).map(String::as_str),
                row.stored,
                "case {case:?}: AdminCreateUser must store {attribute:?} as {:?}, got {stored:?}",
                row.stored
            );
        }

        // A SELF-SERVICE REGISTRATION writes them the same way.
        let signed_up_name = format!("signed-up-{index}");
        ok(
            &router,
            "SignUp",
            &json!({
                "ClientId": client_id,
                "Username": signed_up_name,
                "Password": GOOD_PASSWORD,
                "UserAttributes": attribute_list(&submitted),
            }),
        )
        .await;
        let stored = stored_attributes(&router, &user_pool_id, &signed_up_name).await;
        assert_eq!(
            stored.get("custom:note").map(String::as_str),
            row.stored,
            "case {case:?}: SignUp must store the custom attribute as {:?}, got {stored:?}",
            row.stored
        );

        // An UPDATE of a user that already holds a value writes them the same
        // way, so an empty value takes the held value away.
        let updated_name = format!("updated-{index}");
        create_user(
            &router,
            &user_pool_id,
            &updated_name,
            &[("custom:note", "held"), ("given_name", "Held")],
        )
        .await;
        if !submitted.is_empty() {
            ok(
                &router,
                "AdminUpdateUserAttributes",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": updated_name,
                    "UserAttributes": attribute_list(&submitted),
                }),
            )
            .await;
        }
        let stored = stored_attributes(&router, &user_pool_id, &updated_name).await;
        let expected = match row.submitted {
            None => Some("held"),
            Some(_) => row.stored,
        };
        assert_eq!(
            stored.get("custom:note").map(String::as_str),
            expected,
            "case {case:?}: AdminUpdateUserAttributes must leave {expected:?} stored, got {stored:?}"
        );

        // A GROUP's `Description` is the same member kind: its modelled length
        // starts at 0, so an empty one is accepted and materialised as none.
        let group_name = format!("group-{index}");
        let mut create = Map::new();
        create.insert("UserPoolId".to_string(), json!(user_pool_id));
        create.insert("GroupName".to_string(), json!(group_name));
        if let Some(submitted) = row.submitted {
            create.insert("Description".to_string(), json!(submitted));
        }
        let created = ok(&router, "CreateGroup", &Value::Object(create)).await;
        assert_eq!(
            created["Group"].get("Description").and_then(Value::as_str),
            row.stored,
            "case {case:?}: CreateGroup must store the description as {:?}, got {created}",
            row.stored
        );
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;
        assert_eq!(
            read["Group"].get("Description").and_then(Value::as_str),
            row.stored,
            "case {case:?}: GetGroup must render the stored description, got {read}"
        );

        // And an UPDATE of a group that already has one answers the same way.
        let updated_group = format!("updated-group-{index}");
        ok(
            &router,
            "CreateGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "GroupName": updated_group,
                "Description": "held",
            }),
        )
        .await;
        let mut update = Map::new();
        update.insert("UserPoolId".to_string(), json!(user_pool_id));
        update.insert("GroupName".to_string(), json!(updated_group));
        if let Some(submitted) = row.submitted {
            update.insert("Description".to_string(), json!(submitted));
        }
        let updated = ok(&router, "UpdateGroup", &Value::Object(update)).await;
        assert_eq!(
            updated["Group"].get("Description").and_then(Value::as_str),
            expected,
            "case {case:?}: UpdateGroup must leave {expected:?} stored, got {updated}"
        );
    }
}

/// One row of the username table: the pool's sign-in configuration, the name a
/// create submits, and the username the pool stores for it.
struct UsernameRow {
    case: &'static str,
    /// The `CreateUserPool` members that configure sign-in.
    configuration: Value,
    submitted: &'static str,
    /// The attribute the submitted value is stored under, where the pool makes
    /// an attribute value the sign-in name.
    sign_in_attribute: Option<&'static str>,
    /// What the stored username must be: the submitted value, its lower-cased
    /// form, or an opaque minted id.
    expected: ExpectedUsername,
}

/// What a row expects the stored username to be.
#[derive(Debug, PartialEq, Eq)]
enum ExpectedUsername {
    /// The submitted value, as sent.
    AsSubmitted,
    /// The submitted value, lower-cased.
    LowerCased,
    /// An id the pool minted, which is NOT the submitted value.
    Minted,
}

/// A pool stores the username its own sign-in configuration implies: an opaque
/// minted id where an attribute value is the sign-in name, the lower-cased name
/// where usernames are case-insensitive, and the submitted name otherwise. A
/// pool that minted the username still resolves the user by the sign-in value it
/// was given.
///
/// Every row submits a name such a pool admits: a value of no configured
/// sign-in attribute's shape names no user of one at all and is refused rather
/// than stored, which `tests/all/request_validation_gates.rs` covers.
#[tokio::test]
async fn a_pool_stores_the_username_its_sign_in_configuration_implies() {
    let rows = [
        UsernameRow {
            case: "a pool that configures nothing stores the name as sent",
            configuration: json!({}),
            submitted: "Barbara",
            sign_in_attribute: None,
            expected: ExpectedUsername::AsSubmitted,
        },
        UsernameRow {
            case: "a case-sensitive pool stores the name as sent",
            configuration: json!({ "UsernameConfiguration": { "CaseSensitive": true } }),
            submitted: "Grace.Hopper",
            sign_in_attribute: None,
            expected: ExpectedUsername::AsSubmitted,
        },
        UsernameRow {
            case: "a case-insensitive pool lower-cases the name on the way in",
            configuration: json!({ "UsernameConfiguration": { "CaseSensitive": false } }),
            submitted: "Ada-LOVELACE",
            sign_in_attribute: None,
            expected: ExpectedUsername::LowerCased,
        },
        UsernameRow {
            case: "an email sign-in pool mints the username and stores the address",
            configuration: json!({ "UsernameAttributes": ["email"] }),
            submitted: "Katherine.Johnson@example.com",
            sign_in_attribute: Some("email"),
            expected: ExpectedUsername::Minted,
        },
        UsernameRow {
            case: "a phone-number sign-in pool mints the username and stores the number",
            configuration: json!({ "UsernameAttributes": ["phone_number"] }),
            submitted: "+15555550123",
            sign_in_attribute: Some("phone_number"),
            expected: ExpectedUsername::Minted,
        },
        UsernameRow {
            case: "a pool admitting both sign-in attributes stores the one the value is",
            configuration: json!({ "UsernameAttributes": ["email", "phone_number"] }),
            submitted: "+15555550124",
            sign_in_attribute: Some("phone_number"),
            expected: ExpectedUsername::Minted,
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let user_pool_id = pool(
            &router,
            &format!("username-pool-{index}"),
            row.configuration.clone(),
        )
        .await;

        let created = create_user(&router, &user_pool_id, row.submitted, &[]).await;
        let stored_username = created["Username"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created user carries a username, got {created}"))
            .to_string();
        match row.expected {
            ExpectedUsername::AsSubmitted => assert_eq!(
                stored_username, row.submitted,
                "case {case:?}: expected the submitted name {:?}, got {stored_username:?}",
                row.submitted
            ),
            ExpectedUsername::LowerCased => assert_eq!(
                stored_username,
                row.submitted.to_lowercase(),
                "case {case:?}: expected the lower-cased name, got {stored_username:?}"
            ),
            ExpectedUsername::Minted => assert_ne!(
                stored_username, row.submitted,
                "case {case:?}: a minted username is never the submitted sign-in value, \
                 got {stored_username:?}"
            ),
        }

        // The submitted sign-in value is stored under the attribute the pool
        // signs in with, and nowhere else.
        let stored = stored_attributes(&router, &user_pool_id, &stored_username).await;
        match row.sign_in_attribute {
            Some(attribute) => assert_eq!(
                stored.get(attribute).map(String::as_str),
                Some(row.submitted),
                "case {case:?}: expected {attribute:?} to hold the submitted value, got {stored:?}"
            ),
            // A pool whose users sign in with a username of their own stores no
            // sign-in attribute at all, so the created user holds only the id
            // the pool minted for it.
            None => assert_eq!(
                stored.keys().map(String::as_str).collect::<Vec<&str>>(),
                vec!["sub"],
                "case {case:?}: expected only the minted sub, got {stored:?}"
            ),
        }

        // However the pool stored the username, the user answers to the value
        // the create submitted.
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": row.submitted }),
        )
        .await;
        assert_eq!(
            read["Username"].as_str(),
            Some(stored_username.as_str()),
            "case {case:?}: the submitted name must resolve the stored user, got {read}"
        );
    }
}

/// A stored user answers to every name that names it: its stored username, its
/// `sub`, an active alias value of the pool, and the federated username of an
/// identity linked to it.
#[tokio::test]
async fn a_stored_user_answers_to_every_name_that_names_it() {
    let router = router();
    let user_pool_id = pool(
        &router,
        "resolution-pool",
        json!({ "AliasAttributes": ["email", "preferred_username"] }),
    )
    .await;
    let provider_name = "MySAML";
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": provider_name,
            "ProviderType": "SAML",
            "ProviderDetails": { "client_id": "1example23456789" },
        }),
    )
    .await;
    let external_id = "external-user-1";
    let created = create_user(
        &router,
        &user_pool_id,
        "margaret",
        &[
            ("email", "margaret@example.com"),
            ("email_verified", "true"),
            ("preferred_username", "hamilton"),
        ],
    )
    .await;
    let sub = created["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a minted sub")
        .to_string();
    ok(
        &router,
        "AdminLinkProviderForUser",
        &json!({
            "UserPoolId": user_pool_id,
            "DestinationUser": {
                "ProviderName": NATIVE_PROVIDER,
                "ProviderAttributeName": "Cognito_Subject",
                "ProviderAttributeValue": "margaret",
            },
            "SourceUser": {
                "ProviderName": provider_name,
                "ProviderAttributeName": "Subject",
                "ProviderAttributeValue": external_id,
            },
        }),
    )
    .await;

    for (case, name) in [
        ("the stored username", "margaret".to_string()),
        ("the user's own sub", sub.clone()),
        (
            "a verified email alias of the pool",
            "margaret@example.com".to_string(),
        ),
        (
            "a preferred_username alias of the pool",
            "hamilton".to_string(),
        ),
        (
            "the federated username of a linked identity",
            format!("{provider_name}_{external_id}"),
        ),
    ] {
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": name }),
        )
        .await;
        assert_eq!(
            read["Username"].as_str(),
            Some("margaret"),
            "case {case:?}: {name:?} must resolve the stored user, got {read}"
        );
    }

    // A name no stored user answers to still resolves nothing.
    let (status, payload) = call(
        &router,
        "AdminGetUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": format!("{provider_name}_unlinked-identity"),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UserNotFoundException"),
        "case {:?}: expected a 400 UserNotFoundException, got {status} with {payload}",
        "a federated username nothing is linked under"
    );
}

/// One row of the account-recovery table: what a create carries, and the
/// mechanisms the described pool then reports.
struct RecoveryRow {
    case: &'static str,
    submitted: Option<Value>,
    expected: Value,
}

/// A pool always applies an account-recovery configuration, so a create that
/// carries none is described with the service default — the verified email
/// first, the verified phone number second — while one that carries its own is
/// described with exactly that.
#[tokio::test]
async fn an_omitted_account_recovery_setting_is_described_as_the_service_default() {
    let rows = [
        RecoveryRow {
            case: "no AccountRecoverySetting at all",
            submitted: None,
            expected: json!([
                { "Priority": 1, "Name": "verified_email" },
                { "Priority": 2, "Name": "verified_phone_number" },
            ]),
        },
        RecoveryRow {
            case: "one mechanism",
            submitted: Some(json!({
                "RecoveryMechanisms": [{ "Priority": 1, "Name": "verified_phone_number" }],
            })),
            expected: json!([{ "Priority": 1, "Name": "verified_phone_number" }]),
        },
        RecoveryRow {
            case: "two mechanisms, the phone number first",
            submitted: Some(json!({
                "RecoveryMechanisms": [
                    { "Priority": 1, "Name": "verified_phone_number" },
                    { "Priority": 2, "Name": "verified_email" },
                ],
            })),
            expected: json!([
                { "Priority": 1, "Name": "verified_phone_number" },
                { "Priority": 2, "Name": "verified_email" },
            ]),
        },
        RecoveryRow {
            // "The `admin_only` option prevents self-service account recovery"
            // (`smithy_query(query="shape",shape="RecoveryOptionType")`), which
            // is a configuration like any other.
            case: "the admin_only mechanism",
            submitted: Some(json!({
                "RecoveryMechanisms": [{ "Priority": 1, "Name": "admin_only" }],
            })),
            expected: json!([{ "Priority": 1, "Name": "admin_only" }]),
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let configuration = match &row.submitted {
            Some(setting) => json!({ "AccountRecoverySetting": setting }),
            None => json!({}),
        };
        let user_pool_id = pool(
            &router,
            &format!("recovery-pool-{index}"),
            configuration,
        )
        .await;
        let described = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            described["UserPool"]["AccountRecoverySetting"]["RecoveryMechanisms"], row.expected,
            "case {case:?}: expected the mechanisms {}, got {described}",
            row.expected
        );
    }
}

/// One row of the device-listing table: what the caller does to a confirmed
/// device, and whether each listing still reports it.
struct DeviceRow {
    case: &'static str,
    /// The operation the row drives, beside the pool, the username, the access
    /// token and the device key.
    operation: &'static str,
    remembered_status: Option<&'static str>,
    /// Whether the SELF-SERVICE listing still reports the device.
    listed_by_user: bool,
    /// Whether the ADMIN listing still reports it, which is while the device is
    /// registered at all.
    listed_by_admin: bool,
}

/// The self-service device surface answers for the devices the user REMEMBERS,
/// so a device whose remembered status is cleared leaves `ListDevices` while it
/// stays in `AdminListDevices`, which reports every device registered to the
/// user; a forgotten device is gone from both.
#[tokio::test]
async fn a_device_is_listed_while_the_user_remembers_it() {
    let rows = [
        DeviceRow {
            case: "remembered through UpdateDeviceStatus",
            operation: "UpdateDeviceStatus",
            remembered_status: Some("remembered"),
            listed_by_user: true,
            listed_by_admin: true,
        },
        DeviceRow {
            case: "not_remembered through UpdateDeviceStatus",
            operation: "UpdateDeviceStatus",
            remembered_status: Some("not_remembered"),
            listed_by_user: false,
            listed_by_admin: true,
        },
        DeviceRow {
            case: "not_remembered through AdminUpdateDeviceStatus",
            operation: "AdminUpdateDeviceStatus",
            remembered_status: Some("not_remembered"),
            listed_by_user: false,
            listed_by_admin: true,
        },
        DeviceRow {
            case: "no status at all, which changes nothing",
            operation: "UpdateDeviceStatus",
            remembered_status: None,
            listed_by_user: true,
            listed_by_admin: true,
        },
        DeviceRow {
            case: "forgotten through ForgetDevice",
            operation: "ForgetDevice",
            remembered_status: None,
            listed_by_user: false,
            listed_by_admin: false,
        },
        DeviceRow {
            case: "forgotten through AdminForgetDevice",
            operation: "AdminForgetDevice",
            remembered_status: None,
            listed_by_user: false,
            listed_by_admin: false,
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let user_pool_id = pool(
            &router,
            &format!("device-pool-{index}"),
            json!({ "DeviceConfiguration": { "DeviceOnlyRememberedOnUserPrompt": false } }),
        )
        .await;
        let client_id = client(&router, &user_pool_id).await;
        let username = confirmed_user(&router, &user_pool_id, "alice", &[]).await;
        let result = sign_in(&router, &client_id, &username, None).await;
        let access_token = result["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string();
        let device_key = result["NewDeviceMetadata"]["DeviceKey"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a sign-in must mint a device key, got {result}"))
            .to_string();
        ok(
            &router,
            "ConfirmDevice",
            &json!({
                "AccessToken": access_token,
                "DeviceKey": device_key,
                "DeviceName": "probe-device",
                "DeviceSecretVerifierConfig": device_credentials(),
            }),
        )
        .await;

        // A confirmed, remembered device is in both listings before the row's
        // own call.
        for (listing, body) in listings(&user_pool_id, &username, &access_token) {
            let listed = listed_keys(&ok(&router, listing, &body).await);
            assert_eq!(
                listed,
                vec![device_key.clone()],
                "case {case:?}: {listing} must list the confirmed device first, got {listed:?}"
            );
        }

        let mut body = Map::new();
        if row.operation.starts_with("Admin") {
            body.insert("UserPoolId".to_string(), json!(user_pool_id));
            body.insert("Username".to_string(), json!(username));
        } else {
            body.insert("AccessToken".to_string(), json!(access_token));
        }
        body.insert("DeviceKey".to_string(), json!(device_key));
        if let Some(status) = row.remembered_status {
            body.insert("DeviceRememberedStatus".to_string(), json!(status));
        }
        ok(&router, row.operation, &Value::Object(body)).await;

        for (listing, body) in listings(&user_pool_id, &username, &access_token) {
            let still_listed = if listing == "ListDevices" {
                row.listed_by_user
            } else {
                row.listed_by_admin
            };
            let expected: Vec<String> = if still_listed {
                vec![device_key.clone()]
            } else {
                Vec::new()
            };
            let listed = listed_keys(&ok(&router, listing, &body).await);
            assert_eq!(
                listed, expected,
                "case {case:?}: expected {listing} to report {expected:?}, got {listed:?}"
            );
        }
    }
}

/// The two device listings, each with the body that asks it for this user's
/// devices.
fn listings(user_pool_id: &str, username: &str, access_token: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListDevices", json!({ "AccessToken": access_token })),
        (
            "AdminListDevices",
            json!({ "UserPoolId": user_pool_id, "Username": username }),
        ),
    ]
}

/// `ConfirmDevice` confirms only a key a sign-in of the SIGNED-IN user minted and
/// nothing has confirmed yet, so a key nothing minted, a key already confirmed
/// and a key another user of the pool holds are each an invalid parameter of the
/// request — one refusal, whatever the key.
#[tokio::test]
async fn confirm_device_refuses_every_key_that_is_not_an_unconfirmed_key_of_the_caller() {
    let router = router();
    let user_pool_id = pool(
        &router,
        "confirm-pool",
        json!({ "DeviceConfiguration": { "DeviceOnlyRememberedOnUserPrompt": false } }),
    )
    .await;
    let client_id = client(&router, &user_pool_id).await;

    confirmed_user(&router, &user_pool_id, "alice", &[]).await;
    confirmed_user(&router, &user_pool_id, "bob", &[]).await;
    let alice = sign_in(&router, &client_id, "alice", None).await;
    let alice_token = alice["AccessToken"].as_str().expect("an access token");
    let alice_key = alice["NewDeviceMetadata"]["DeviceKey"]
        .as_str()
        .expect("a minted device key")
        .to_string();
    let bob = sign_in(&router, &client_id, "bob", None).await;
    let bob_token = bob["AccessToken"].as_str().expect("an access token");
    let bob_key = bob["NewDeviceMetadata"]["DeviceKey"]
        .as_str()
        .expect("a minted device key")
        .to_string();
    ok(
        &router,
        "ConfirmDevice",
        &json!({
            "AccessToken": alice_token,
            "DeviceKey": alice_key,
            "DeviceSecretVerifierConfig": device_credentials(),
        }),
    )
    .await;

    for (case, token, device_key) in [
        (
            "a device key nothing minted",
            alice_token,
            "us-east-1_00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "a device key this user has already confirmed",
            alice_token,
            alice_key.clone(),
        ),
        (
            "a device key another user of the pool holds",
            alice_token,
            bob_key.clone(),
        ),
        (
            "a device key the OTHER user holds, presented by its owner's peer",
            bob_token,
            alice_key.clone(),
        ),
    ] {
        let (status, payload) = call(
            &router,
            "ConfirmDevice",
            &json!({
                "AccessToken": token,
                "DeviceKey": device_key,
                "DeviceSecretVerifierConfig": device_credentials(),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", "Invalid device key given."),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
    }

    // Bob's own unconfirmed key is still his to confirm: the refusals above
    // changed nothing.
    ok(
        &router,
        "ConfirmDevice",
        &json!({
            "AccessToken": bob_token,
            "DeviceKey": bob_key,
            "DeviceSecretVerifierConfig": device_credentials(),
        }),
    )
    .await;
}

/// One row of the reset-password table: the state the user is in, and whether a
/// reset applies to it.
struct ResetRow {
    case: &'static str,
    /// Whether the user is given a permanent password of its own, which is what
    /// completes an administrator-created account.
    owns_password: bool,
    /// Whether the user registered itself and never confirmed the sign-up.
    unconfirmed: bool,
    expected_status: &'static str,
}

/// `AdminResetUserPassword` resets the password the user OWNS, so a user that
/// owns none — one that never confirmed its sign-up, and one still holding the
/// temporary password an administrator created it with — is refused, while a
/// user whose registration is complete is moved to `RESET_REQUIRED`.
#[tokio::test]
async fn a_password_reset_needs_a_user_that_owns_a_password() {
    let rows = [
        ResetRow {
            case: "a user holding a permanent password of its own",
            owns_password: true,
            unconfirmed: false,
            expected_status: "RESET_REQUIRED",
        },
        ResetRow {
            case: "an administrator-created user that has not signed in yet",
            owns_password: false,
            unconfirmed: false,
            expected_status: "FORCE_CHANGE_PASSWORD",
        },
        ResetRow {
            case: "a self-registered user that never confirmed its sign-up",
            owns_password: false,
            unconfirmed: true,
            expected_status: "UNCONFIRMED",
        },
    ];

    let router = router();
    let user_pool_id = pool(
        &router,
        "reset-pool",
        json!({ "AutoVerifiedAttributes": ["email"] }),
    )
    .await;
    let client_id = client(&router, &user_pool_id).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let username = format!("user-{index}");
        // Every user holds a VERIFIED email, so where a reset applies at all it
        // has a destination to send its code to.
        let attributes = [
            ("email", "resettable@example.com"),
            ("email_verified", "true"),
        ];
        if row.unconfirmed {
            ok(
                &router,
                "SignUp",
                &json!({
                    "ClientId": client_id,
                    "Username": username,
                    "Password": GOOD_PASSWORD,
                    "UserAttributes": attribute_list(&attributes),
                }),
            )
            .await;
            ok(
                &router,
                "AdminUpdateUserAttributes",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "UserAttributes": attribute_list(&[("email_verified", "true")]),
                }),
            )
            .await;
        } else if row.owns_password {
            confirmed_user(&router, &user_pool_id, &username, &attributes).await;
        } else {
            create_user(&router, &user_pool_id, &username, &attributes).await;
        }

        let (status, payload) = call(
            &router,
            "AdminResetUserPassword",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        if row.owns_password {
            assert_eq!(
                status, 200,
                "case {case:?}: a reset of an owned password must succeed, got {status} with {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (
                    400,
                    "NotAuthorizedException",
                    "User password cannot be reset in the current state."
                ),
                "case {case:?}: expected a 400 NotAuthorizedException, got {status} with {payload}"
            );
        }

        // The user is in the state the call left it in: a refused reset moved
        // nothing.
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            read["UserStatus"].as_str(),
            Some(row.expected_status),
            "case {case:?}: expected the status {:?}, got {read}",
            row.expected_status
        );
    }
}
