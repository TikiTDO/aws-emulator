// SPDX-License-Identifier: CC0-1.0

//! The token-validity configuration an app client carries, over the wire, driven
//! exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: an app client is stored with the server-applied defaults
//! of every token-validity member the model documents one for, so
//! `CreateUserPoolClient`, `UpdateUserPoolClient` and `DescribeUserPoolClient`
//! all render those members whether or not the request named them.
//!
//! * `RefreshTokenValidity` is modelled `default 0`, `range 0..315360000`, and
//!   "You can't set `RefreshTokenValidity` to 0. If you do, Amazon Cognito
//!   overrides the value with the default value of 30 days ... If you don't
//!   specify otherwise in the configuration of your app client, your refresh
//!   tokens are valid for 30 days"
//!   (`smithy_query(query="doc",shape="UserPoolClientType",member="RefreshTokenValidity")`),
//!   so a stored app client never holds `0` and an unset one holds 30.
//! * `AuthSessionValidity` is "the duration, in minutes, of that session token"
//!   with `range 3..15`
//!   (`smithy_query(query="doc",shape="UserPoolClientType",member="AuthSessionValidity")`),
//!   so an app client that names none holds the minimum of that range.
//! * `TokenValidityUnits` holds "a separate token validity unit to each type of
//!   token" (`smithy_query(query="shape",shape="TokenValidityUnitsType")`: the
//!   members `AccessToken`, `IdToken` and `RefreshToken`, each a `TimeUnitsType`
//!   of `seconds`, `minutes`, `hours`, `days`), and each validity member has a
//!   documented default unit of its own, so every app client carries the block —
//!   empty when the request overrode no unit.
//!
//! Each table row names the case it asserts, the rows differ along which members
//! the request carries, which values they carry (including both published bounds
//! of each range) and how many units the block names, and every assertion
//! reports the actual value.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The `RefreshTokenValidity` every app client that asked for none holds.
const DEFAULT_REFRESH_TOKEN_VALIDITY: i64 = 30;

/// The `AuthSessionValidity`, in minutes, every app client that asked for none
/// holds: the minimum of the member's modelled 3..15 range.
const DEFAULT_AUTH_SESSION_VALIDITY: i64 = 3;

/// The largest `RefreshTokenValidity` the member's modelled range admits, which
/// the member documents as a range "displayed below in seconds".
const MAX_REFRESH_TOKEN_VALIDITY: i64 = 315_360_000;

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
async fn created_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// `members` beside the ids an app-client request needs.
fn request_body(members: &Value, ids: &[(&str, &str)]) -> Value {
    let mut body: Map<String, Value> = members
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("a case's members are a JSON object: {members}"));
    for (member, value) in ids {
        body.insert((*member).to_string(), json!(value));
    }
    Value::Object(body)
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

/// One row: the token-validity members a request carries, and what every read of
/// the app client must then render.
struct ValidityCase {
    case: &'static str,
    members: Value,
    refresh_token_validity: i64,
    auth_session_validity: i64,
    token_validity_units: Value,
}

/// The table. The rows span a request that names no token-validity member at
/// all, one that names the modelled default `0`, each published bound of the two
/// ranges, a value between them, and unit blocks naming none, one and all three
/// units.
fn validity_cases() -> Vec<ValidityCase> {
    vec![
        ValidityCase {
            case: "no token-validity member at all: every default is server-applied",
            members: json!({}),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "RefreshTokenValidity sent as the modelled default 0, which is overridden",
            members: json!({ "RefreshTokenValidity": 0 }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "RefreshTokenValidity 1, the smallest value an app client can hold",
            members: json!({ "RefreshTokenValidity": 1 }),
            refresh_token_validity: 1,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "RefreshTokenValidity at the top of its modelled range, which is in seconds",
            members: json!({
                "RefreshTokenValidity": MAX_REFRESH_TOKEN_VALIDITY,
                "TokenValidityUnits": { "RefreshToken": "seconds" }
            }),
            refresh_token_validity: MAX_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({ "RefreshToken": "seconds" }),
        },
        ValidityCase {
            case: "AuthSessionValidity 3, the bottom of its modelled range",
            members: json!({ "AuthSessionValidity": 3 }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: 3,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "AuthSessionValidity 9, between the bounds of its modelled range",
            members: json!({ "AuthSessionValidity": 9 }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: 9,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "AuthSessionValidity 15, the top of its modelled range",
            members: json!({ "AuthSessionValidity": 15 }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: 15,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "an explicitly empty TokenValidityUnits block overrides no unit",
            members: json!({ "TokenValidityUnits": {} }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({}),
        },
        ValidityCase {
            case: "one unit named, for the one validity the request sets",
            members: json!({
                "AccessTokenValidity": 5,
                "TokenValidityUnits": { "AccessToken": "minutes" }
            }),
            refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({ "AccessToken": "minutes" }),
        },
        ValidityCase {
            case: "a unit for the refresh token only, with a validity to go with it",
            members: json!({
                "RefreshTokenValidity": 90,
                "TokenValidityUnits": { "RefreshToken": "days" }
            }),
            refresh_token_validity: 90,
            auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
            token_validity_units: json!({ "RefreshToken": "days" }),
        },
        ValidityCase {
            case: "all three units named, with every validity member set",
            members: json!({
                "AccessTokenValidity": 3600,
                "IdTokenValidity": 7200,
                "RefreshTokenValidity": 86400,
                "AuthSessionValidity": 10,
                "TokenValidityUnits": {
                    "AccessToken": "seconds",
                    "IdToken": "seconds",
                    "RefreshToken": "hours"
                }
            }),
            refresh_token_validity: 86400,
            auth_session_validity: 10,
            token_validity_units: json!({
                "AccessToken": "seconds",
                "IdToken": "seconds",
                "RefreshToken": "hours"
            }),
        },
    ]
}

/// Assert the three members this rule covers on `client`, rendered by `reader`.
fn assert_validity(case: &str, reader: &str, client: &Value, row: &ValidityCase) {
    let refresh = client.get("RefreshTokenValidity").cloned();
    assert_eq!(
        refresh,
        Some(json!(row.refresh_token_validity)),
        "case {case:?}: {reader} must render RefreshTokenValidity {}, got {refresh:?}",
        row.refresh_token_validity
    );
    let session = client.get("AuthSessionValidity").cloned();
    assert_eq!(
        session,
        Some(json!(row.auth_session_validity)),
        "case {case:?}: {reader} must render AuthSessionValidity {}, got {session:?}",
        row.auth_session_validity
    );
    let units = client.get("TokenValidityUnits").cloned();
    assert_eq!(
        units,
        Some(row.token_validity_units.clone()),
        "case {case:?}: {reader} must render TokenValidityUnits {}, got {units:?}",
        row.token_validity_units
    );
}

#[tokio::test]
async fn every_app_client_read_carries_the_server_applied_validity_configuration() {
    let router = router();
    let user_pool_id = created_pool(&router, "validity-defaults-pool").await;

    for (index, row) in validity_cases().iter().enumerate() {
        let client_name = format!("validity-client-{index}");
        let created = ok(
            &router,
            "CreateUserPoolClient",
            &request_body(
                &row.members,
                &[("UserPoolId", &user_pool_id), ("ClientName", &client_name)],
            ),
        )
        .await["UserPoolClient"]
            .clone();
        assert_validity(row.case, "CreateUserPoolClient", &created, row);

        let client_id = created["ClientId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a minted client id: {created}", row.case))
            .to_string();

        // The create answered the STORED app client, so a read renders the same
        // configuration, and a read leaves it where it is.
        let described = described_client(&router, &user_pool_id, &client_id).await;
        assert_validity(row.case, "DescribeUserPoolClient", &described, row);
        let again = described_client(&router, &user_pool_id, &client_id).await;
        assert_eq!(
            again, described,
            "case {:?}: reading an app client must not change it, got {again} then {described}",
            row.case
        );

        // The same members applied through an update are resolved the same way.
        let updated = ok(
            &router,
            "UpdateUserPoolClient",
            &request_body(
                &row.members,
                &[("UserPoolId", &user_pool_id), ("ClientId", &client_id)],
            ),
        )
        .await["UserPoolClient"]
            .clone();
        assert_validity(row.case, "UpdateUserPoolClient", &updated, row);
        let read_back = described_client(&router, &user_pool_id, &client_id).await;
        assert_validity(row.case, "the read after the update", &read_back, row);
    }
}

#[tokio::test]
async fn an_update_that_names_no_validity_member_restores_the_server_applied_defaults() {
    let router = router();
    let user_pool_id = created_pool(&router, "validity-reset-pool").await;
    // The defaults an app client that configures none of these members holds.
    let defaults = ValidityCase {
        case: "the server-applied defaults",
        members: json!({}),
        refresh_token_validity: DEFAULT_REFRESH_TOKEN_VALIDITY,
        auth_session_validity: DEFAULT_AUTH_SESSION_VALIDITY,
        token_validity_units: json!({}),
    };

    for (index, row) in validity_cases().iter().enumerate() {
        let client_name = format!("reset-client-{index}");
        let created = ok(
            &router,
            "CreateUserPoolClient",
            &request_body(
                &row.members,
                &[("UserPoolId", &user_pool_id), ("ClientName", &client_name)],
            ),
        )
        .await["UserPoolClient"]
            .clone();
        let client_id = created["ClientId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a minted client id: {created}", row.case))
            .to_string();

        // "If you don't provide a value for an attribute, Amazon Cognito sets it
        // to its default value" (`UpdateUserPoolClient`), so an update that names
        // none of these members puts every one of them back to its default.
        let updated = ok(
            &router,
            "UpdateUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await["UserPoolClient"]
            .clone();
        assert_validity(row.case, "the reset UpdateUserPoolClient", &updated, &defaults);
        let read_back = described_client(&router, &user_pool_id, &client_id).await;
        assert_validity(row.case, "the read after the reset", &read_back, &defaults);
    }
}

#[tokio::test]
async fn a_validity_outside_its_modelled_range_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = created_pool(&router, "validity-range-pool").await;
    let standing = ok(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "standing-client" }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    // One row per published bound crossed by one: `AuthSessionValidity` is
    // modelled 3..15 and `RefreshTokenValidity` 0..315360000.
    let rows = [
        (
            "AuthSessionValidity one below its minimum",
            json!({ "AuthSessionValidity": 2 }),
        ),
        (
            "AuthSessionValidity one above its maximum",
            json!({ "AuthSessionValidity": 16 }),
        ),
        (
            "RefreshTokenValidity one below its minimum",
            json!({ "RefreshTokenValidity": -1 }),
        ),
        (
            "RefreshTokenValidity one above its maximum",
            json!({ "RefreshTokenValidity": MAX_REFRESH_TOKEN_VALIDITY + 1 }),
        ),
    ];

    for (case, members) in rows {
        let was = described_client(&router, &user_pool_id, &standing).await;
        let (status, payload) = call(
            &router,
            "CreateUserPoolClient",
            &request_body(
                &members,
                &[("UserPoolId", &user_pool_id), ("ClientName", "refused")],
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: a value outside the modelled range is a 400 \
             InvalidParameterException, got {status} with {payload}"
        );

        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &request_body(
                &members,
                &[("UserPoolId", &user_pool_id), ("ClientId", &standing)],
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: the update owes the same refusal, got {status} with {payload}"
        );
        let now = described_client(&router, &user_pool_id, &standing).await;
        assert_eq!(
            now, was,
            "case {case:?}: a refused request must leave the app client alone, got {now}"
        );
    }
}
