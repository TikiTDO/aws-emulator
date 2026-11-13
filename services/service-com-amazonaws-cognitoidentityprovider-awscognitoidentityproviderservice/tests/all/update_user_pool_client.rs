// SPDX-License-Identifier: CC0-1.0

//! `UpdateUserPoolClient` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`), and read back through
//! `DescribeUserPoolClient`.
//!
//! The rule under test: `UpdateUserPoolClient` REPLACES the configuration of the
//! app client it names — a member the request carries is stored as sent, and a
//! member the request leaves out goes back to the service default instead of
//! keeping the value the client held ("If you don't provide a value for an
//! attribute, Amazon Cognito sets it to its default value", this operation's
//! documentation in the service model). The app client's identity (`ClientId`,
//! `ClientSecret`, `UserPoolId`, `CreationDate`) survives the update, and
//! `LastModifiedDate` advances.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The name every case's app client is created with.
const CREATED_CLIENT_NAME: &str = "update-rule-client";

/// A password that satisfies a new pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// A created user pool's id.
async fn created_pool(router: &MultiServiceRouter) -> String {
    ok(router, "CreateUserPool", &json!({ "PoolName": "update-client-pool" })).await["UserPool"]
        ["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// A `CreateUserPoolClient` request that sets every member `UpdateUserPoolClient`
/// can reset, so each case can observe whether the update kept it or returned it
/// to its default. `GenerateSecret` is NOT an `UpdateUserPoolClient` input member,
/// so the minted secret is here to be checked for survival.
fn rich_create_request(user_pool_id: &str) -> Value {
    json!({
        "UserPoolId": user_pool_id,
        "ClientName": CREATED_CLIENT_NAME,
        "GenerateSecret": true,
        "RefreshTokenValidity": 10,
        "AccessTokenValidity": 5,
        "IdTokenValidity": 6,
        "TokenValidityUnits": { "AccessToken": "minutes", "IdToken": "minutes", "RefreshToken": "days" },
        "ReadAttributes": ["email"],
        "WriteAttributes": ["email"],
        "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        "SupportedIdentityProviders": ["COGNITO"],
        "CallbackURLs": ["https://example.com/callback"],
        "LogoutURLs": ["https://example.com/logout"],
        "DefaultRedirectURI": "https://example.com/callback",
        "AllowedOAuthFlows": ["code"],
        "AllowedOAuthScopes": ["openid"],
        "AllowedOAuthFlowsUserPoolClient": true,
        "PreventUserExistenceErrors": "ENABLED",
        "EnableTokenRevocation": false,
        "EnablePropagateAdditionalUserContextData": true,
        "AuthSessionValidity": 15,
        "RefreshTokenRotation": { "Feature": "ENABLED", "RetryGracePeriodSeconds": 30 }
    })
}

/// Create a pool and a rich app client on it, and return
/// `(user_pool_id, created client)`.
async fn pool_with_rich_client(router: &MultiServiceRouter) -> (String, Value) {
    let user_pool_id = created_pool(router).await;
    let created = ok(
        router,
        "CreateUserPoolClient",
        &rich_create_request(&user_pool_id),
    )
    .await["UserPoolClient"]
        .clone();
    (user_pool_id, created)
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

/// The value at a dotted path inside `value`, or `None` when any step is absent.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for step in path.split('.') {
        current = current.get(step)?;
    }
    Some(current)
}

/// Assert the rendered app client carries `expected` at `path`. A `Value::Null`
/// expectation means the member must be ABSENT.
fn assert_member(case: &str, client: &Value, path: &str, expected: &Value) {
    let actual = at(client, path);
    if expected.is_null() {
        assert!(
            actual.is_none_or(Value::is_null),
            "case {case:?}: {path} must be absent after the update, got {actual:?}"
        );
    } else {
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {path} must be {expected}, got {actual:?}"
        );
    }
}

/// `client` without the one member an update is expected to move, so two renders
/// of the same configuration can be compared.
fn without_last_modified(mut client: Value) -> Value {
    if let Some(members) = client.as_object_mut() {
        members.remove("LastModifiedDate");
    }
    client
}

/// `client`'s `LastModifiedDate`, as the seconds-since-epoch number the modelled
/// timestamp serialises to.
fn last_modified(case: &str, client: &Value) -> f64 {
    client["LastModifiedDate"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: an app client must carry a LastModifiedDate: {client}")
    })
}

/// One row of the `UpdateUserPoolClient` table: the members the update carries
/// beyond `UserPoolId` and `ClientId`, and what the app client must render
/// afterwards. Rows differ along what the rule generalises over — which members
/// are present, how many entries a list member carries, and the form of each
/// value.
struct UpdateCase {
    case: &'static str,
    request: Value,
    expected: Vec<(&'static str, Value)>,
}

/// The members every row that sets none of them expects back at their defaults.
/// The defaults are the model's documented ones, which `src/state.rs` owns:
/// `PreventUserExistenceErrors` "Defaults to LEGACY",
/// `EnableTokenRevocation` is "automatically activated",
/// `AllowedOAuthFlowsUserPoolClient` and
/// `EnablePropagateAdditionalUserContextData` default to `false`,
/// `RefreshTokenValidity` "can't" be the modelled `0` the deserializer applies —
/// "Amazon Cognito overrides the value with the default value of 30 days" —
/// `AuthSessionValidity` is the minimum of its modelled 3..15 range, and an app
/// client always carries a `TokenValidityUnits` block, empty when no unit was
/// named. `ExplicitAuthFlows` has
/// a documented default for a CREATE — the set a NEW app client starts with —
/// so an update that names no flows leaves the app client with NONE of them
/// rather than putting that create-time set back.
fn reset_to_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("AccessTokenValidity", Value::Null),
        ("IdTokenValidity", Value::Null),
        ("TokenValidityUnits", json!({})),
        ("ReadAttributes", Value::Null),
        ("WriteAttributes", Value::Null),
        ("SupportedIdentityProviders", Value::Null),
        ("CallbackURLs", Value::Null),
        ("LogoutURLs", Value::Null),
        ("DefaultRedirectURI", Value::Null),
        ("AllowedOAuthFlows", Value::Null),
        ("AllowedOAuthScopes", Value::Null),
        ("AnalyticsConfiguration", Value::Null),
        ("AuthSessionValidity", json!(3)),
        ("RefreshTokenRotation", Value::Null),
        ("RefreshTokenValidity", json!(30)),
        ("AllowedOAuthFlowsUserPoolClient", json!(false)),
        ("EnablePropagateAdditionalUserContextData", json!(false)),
        ("EnableTokenRevocation", json!(true)),
        ("PreventUserExistenceErrors", json!("LEGACY")),
        ("ExplicitAuthFlows", json!([])),
        // `ClientName` has no documented default and an app client must carry a
        // name, so an update that omits it keeps the stored one.
        ("ClientName", json!(CREATED_CLIENT_NAME)),
    ]
}

/// The defaults with the entry for each `path` this row sets replaced by its
/// expectation. An override of a nested path (`A.B`) also drops the default
/// expectation for the member that holds it (`A`), which the row is replacing.
fn overriding(overrides: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    let mut expected = reset_to_defaults();
    for (path, value) in overrides {
        expected
            .retain(|(existing, _)| *existing != path && !path.starts_with(&format!("{existing}.")));
        expected.push((path, value));
    }
    expected
}

/// The table: each row is one `UpdateUserPoolClient` request against an app
/// client created from [`rich_create_request`].
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "nothing but the ids: every modelled member goes back to its default",
            request: json!({}),
            expected: reset_to_defaults(),
        },
        UpdateCase {
            case: "ClientName only: the name is the member that changes",
            request: json!({ "ClientName": "renamed client" }),
            expected: overriding(vec![("ClientName", json!("renamed client"))]),
        },
        UpdateCase {
            case: "one auth flow, as a selection-based sign-in client configures it",
            request: json!({ "ExplicitAuthFlows": ["ALLOW_USER_AUTH"] }),
            expected: overriding(vec![("ExplicitAuthFlows", json!(["ALLOW_USER_AUTH"]))]),
        },
        UpdateCase {
            case: "three auth flows and two identity providers: a list member is stored whole",
            request: json!({
                "ExplicitAuthFlows": [
                    "ALLOW_USER_AUTH",
                    "ALLOW_REFRESH_TOKEN_AUTH",
                    "ALLOW_USER_PASSWORD_AUTH"
                ],
                "SupportedIdentityProviders": ["COGNITO", "Google"]
            }),
            expected: overriding(vec![
                (
                    "ExplicitAuthFlows",
                    json!([
                        "ALLOW_USER_AUTH",
                        "ALLOW_REFRESH_TOKEN_AUTH",
                        "ALLOW_USER_PASSWORD_AUTH"
                    ]),
                ),
                (
                    "SupportedIdentityProviders",
                    json!(["COGNITO", "Google"]),
                ),
            ]),
        },
        UpdateCase {
            case: "token lifetimes at the low end of each documented window, with units",
            request: json!({
                "AccessTokenValidity": 5,
                "IdTokenValidity": 5,
                "RefreshTokenValidity": 1,
                "AuthSessionValidity": 3,
                "TokenValidityUnits": {
                    "AccessToken": "minutes",
                    "IdToken": "minutes",
                    "RefreshToken": "hours"
                }
            }),
            expected: overriding(vec![
                ("AccessTokenValidity", json!(5)),
                ("IdTokenValidity", json!(5)),
                ("RefreshTokenValidity", json!(1)),
                ("AuthSessionValidity", json!(3)),
                (
                    "TokenValidityUnits",
                    json!({
                        "AccessToken": "minutes",
                        "IdToken": "minutes",
                        "RefreshToken": "hours"
                    }),
                ),
            ]),
        },
        UpdateCase {
            case: "token lifetimes at the high end of each modelled range",
            request: json!({
                "AccessTokenValidity": 86400,
                "IdTokenValidity": 86400,
                "RefreshTokenValidity": 315360000,
                "AuthSessionValidity": 15,
                "TokenValidityUnits": { "AccessToken": "seconds", "IdToken": "seconds", "RefreshToken": "seconds" }
            }),
            expected: overriding(vec![
                ("AccessTokenValidity", json!(86400)),
                ("IdTokenValidity", json!(86400)),
                ("RefreshTokenValidity", json!(315360000)),
                ("AuthSessionValidity", json!(15)),
                (
                    "TokenValidityUnits",
                    json!({ "AccessToken": "seconds", "IdToken": "seconds", "RefreshToken": "seconds" }),
                ),
            ]),
        },
        UpdateCase {
            case: "the OAuth configuration, with two callbacks and two scopes",
            request: json!({
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthFlows": ["code", "implicit"],
                "AllowedOAuthScopes": ["openid", "email"],
                "CallbackURLs": ["https://one.example.com/cb", "https://two.example.com/cb"],
                "LogoutURLs": ["https://one.example.com/out"],
                "DefaultRedirectURI": "https://two.example.com/cb"
            }),
            expected: overriding(vec![
                ("AllowedOAuthFlowsUserPoolClient", json!(true)),
                ("AllowedOAuthFlows", json!(["code", "implicit"])),
                ("AllowedOAuthScopes", json!(["openid", "email"])),
                (
                    "CallbackURLs",
                    json!(["https://one.example.com/cb", "https://two.example.com/cb"]),
                ),
                ("LogoutURLs", json!(["https://one.example.com/out"])),
                ("DefaultRedirectURI", json!("https://two.example.com/cb")),
            ]),
        },
        UpdateCase {
            case: "an empty OAuth flow list: an empty list is a value, not an absent member",
            request: json!({ "AllowedOAuthFlows": [], "CallbackURLs": [] }),
            expected: overriding(vec![
                ("AllowedOAuthFlows", json!([])),
                ("CallbackURLs", json!([])),
            ]),
        },
        UpdateCase {
            case: "the attribute permissions and the error-suppression mode",
            request: json!({
                "ReadAttributes": ["email", "phone_number", "custom:department"],
                "WriteAttributes": ["email"],
                "PreventUserExistenceErrors": "ENABLED",
                "EnableTokenRevocation": false
            }),
            expected: overriding(vec![
                (
                    "ReadAttributes",
                    json!(["email", "phone_number", "custom:department"]),
                ),
                ("WriteAttributes", json!(["email"])),
                ("PreventUserExistenceErrors", json!("ENABLED")),
                ("EnableTokenRevocation", json!(false)),
            ]),
        },
        UpdateCase {
            case: "refresh token rotation and additional user context data",
            request: json!({
                "RefreshTokenRotation": { "Feature": "ENABLED", "RetryGracePeriodSeconds": 0 },
                "EnablePropagateAdditionalUserContextData": true
            }),
            expected: overriding(vec![
                (
                    "RefreshTokenRotation",
                    json!({ "Feature": "ENABLED", "RetryGracePeriodSeconds": 0 }),
                ),
                ("EnablePropagateAdditionalUserContextData", json!(true)),
            ]),
        },
        UpdateCase {
            case: "an analytics configuration",
            request: json!({
                "AnalyticsConfiguration": {
                    "ApplicationId": "0123456789abcdef",
                    "RoleArn": "arn:aws:iam::000000000000:role/pinpoint",
                    "ExternalId": "external-id",
                    "UserDataShared": true
                }
            }),
            expected: overriding(vec![(
                "AnalyticsConfiguration",
                json!({
                    "ApplicationId": "0123456789abcdef",
                    "RoleArn": "arn:aws:iam::000000000000:role/pinpoint",
                    "ExternalId": "external-id",
                    "UserDataShared": true
                }),
            )]),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_every_member_this_operation_models() {
    let router = router();
    for case in update_cases() {
        let (user_pool_id, created) = pool_with_rich_client(&router).await;
        let client_id = created["ClientId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a minted client id: {created}", case.case))
            .to_string();

        // The create really did set what this case expects to be replaced or reset.
        assert_eq!(
            created.get("AccessTokenValidity"),
            Some(&json!(5)),
            "case {:?}: the created client must carry the lifetime the create set, got {created}",
            case.case
        );
        assert_eq!(
            created.get("ExplicitAuthFlows"),
            Some(&json!(["ALLOW_ADMIN_USER_PASSWORD_AUTH"])),
            "case {:?}: the created client must carry the flows the create set, got {created}",
            case.case
        );

        let mut request = case.request.clone();
        request["UserPoolId"] = json!(user_pool_id);
        request["ClientId"] = json!(client_id);
        let answered = ok(&router, "UpdateUserPoolClient", &request).await["UserPoolClient"].clone();

        let updated = described_client(&router, &user_pool_id, &client_id).await;
        for (path, expected) in &case.expected {
            assert_member(case.case, &updated, path, expected);
            assert_member(case.case, &answered, path, expected);
        }

        // The response is the STORED client: a later read renders the same thing.
        assert_eq!(
            answered, updated,
            "case {:?}: the update's response must be what DescribeUserPoolClient renders, got {answered} and {updated}",
            case.case
        );

        // The app client's identity survives the update: these are not members of
        // this operation's input.
        for member in ["ClientId", "ClientSecret", "UserPoolId", "CreationDate"] {
            assert_eq!(
                updated.get(member),
                created.get(member),
                "case {:?}: {member} must survive an update, got {:?}",
                case.case,
                updated.get(member)
            );
        }

        // Replacing is idempotent: the same request again renders the same client.
        ok(&router, "UpdateUserPoolClient", &request).await;
        let again = described_client(&router, &user_pool_id, &client_id).await;
        assert_eq!(
            without_last_modified(again.clone()),
            without_last_modified(updated.clone()),
            "case {:?}: the same update applied twice must render the same client, got {again}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_advances_the_clients_last_modified_instant() {
    let router = router();
    let (user_pool_id, created) = pool_with_rich_client(&router).await;
    let client_id = created["ClientId"].as_str().expect("a client id").to_string();
    let case = "last modified advances";

    let before = last_modified(case, &created);
    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "UpdateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id, "ClientName": "advanced" }),
    )
    .await;
    let first = last_modified(
        case,
        &described_client(&router, &user_pool_id, &client_id).await,
    );
    assert!(
        first > before,
        "case {case:?}: an update must advance LastModifiedDate past {before}, got {first}"
    );

    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "UpdateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id, "ClientName": "advanced" }),
    )
    .await;
    let second = last_modified(
        case,
        &described_client(&router, &user_pool_id, &client_id).await,
    );
    assert!(
        second > first,
        "case {case:?}: a second update must advance LastModifiedDate past {first}, got {second}"
    );

    // A read does not move it.
    let read_again = last_modified(
        case,
        &described_client(&router, &user_pool_id, &client_id).await,
    );
    assert_eq!(
        read_again, second,
        "case {case:?}: DescribeUserPoolClient must not move LastModifiedDate, got {read_again}"
    );
}

#[tokio::test]
async fn a_replaced_auth_flow_list_is_observable_through_admin_initiate_auth() {
    let router = router();
    let user_pool_id = created_pool(&router).await;
    // An app client that starts with NO admin password flow.
    let client_id = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": CREATED_CLIENT_NAME,
            "ExplicitAuthFlows": ["ALLOW_REFRESH_TOKEN_AUTH"]
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a client id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true
        }),
    )
    .await;

    let auth = json!({
        "UserPoolId": user_pool_id,
        "ClientId": client_id,
        "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD }
    });

    // Before the update the flow is not enabled, so the sign-in is refused.
    let (status, payload) = call(&router, "AdminInitiateAuth", &auth).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"flow not enabled\": got {status} with {payload}"
    );

    // An update that enables it is honoured by the next sign-in.
    ok(
        &router,
        "UpdateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"]
        }),
    )
    .await;
    let signed_in = ok(&router, "AdminInitiateAuth", &auth).await;
    assert!(
        at(&signed_in, "AuthenticationResult.AccessToken").is_some(),
        "case \"flow enabled by the update\": a sign-in must issue tokens, got {signed_in}"
    );

    // And an update that omits `ExplicitAuthFlows` leaves the app client with
    // none of them, so the same sign-in is refused again.
    ok(
        &router,
        "UpdateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    let (status, payload) = call(&router, "AdminInitiateAuth", &auth).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"flow reset to the default\": got {status} with {payload}"
    );
}

#[tokio::test]
async fn an_update_of_an_absent_pool_or_client_is_a_400() {
    let router = router();
    let (user_pool_id, created) = pool_with_rich_client(&router).await;
    let client_id = created["ClientId"].as_str().expect("a client id").to_string();

    // Pool ids no pool was created under, each satisfying the modelled
    // `UserPoolIdType` pattern `^[\w-]+_[0-9a-zA-Z]+$` in a different form.
    let absent_pools = [
        ("another region", "us-west-2_39e6f417e"),
        ("an upper-case suffix", "eu-central-1_ABCDEFGHI"),
        ("a single-character suffix", "us-east-1_0"),
        ("an underscore-only prefix", "pool_abc123"),
    ];
    for (case, absent_pool) in absent_pools {
        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({ "UserPoolId": absent_pool, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: an absent pool must answer 400, got {status} with {payload}"
        );
    }

    // Client ids no app client was created under, each satisfying the modelled
    // `ClientIdType` pattern `^[\w+]+$` in a different form.
    let absent_clients = [
        ("lower-case alphanumeric", "1h57kf5cpq17m0eml12eukdi1o"),
        ("an upper-case id", "ABCDEFGHIJ1234567890"),
        ("a single character", "a"),
        ("underscores and a plus", "client_id+1"),
    ];
    for (case, absent_client) in absent_clients {
        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": absent_client }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: an absent app client must answer 400, got {status} with {payload}"
        );
    }

    // The refused updates changed nothing about the client that does exist.
    let described = described_client(&router, &user_pool_id, &client_id).await;
    assert_eq!(
        without_last_modified(described.clone()),
        without_last_modified(created.clone()),
        "case \"live client untouched\": got {described}"
    );
}
