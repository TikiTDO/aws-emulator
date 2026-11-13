// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal a client-secret lookup that resolves nothing answers,
//! over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.DeleteUserPoolClientSecret`).
//!
//! The rule under test: a `ClientSecretId` the named app client holds no active secret
//! for is refused with `ResourceNotFoundException` at HTTP 400 carrying ONE fixed
//! sentence — the same text for every id, quoting neither the id asked for, the app
//! client (which the model marks `@sensitive`) nor the pool. Which sentence it is
//! reports WHAT the lookup failed to resolve: one secret among those the app client
//! holds, or the app client's secret altogether, for a client that holds none. So a
//! caller that classifies refusals by message sees one not-found class per fact rather
//! than one error per request, and the refusal never echoes the caller's secret
//! identifier.
//!
//! The rows below differ along what that rule generalises over: the FORM of the id
//! (hex, UUID, the `<client-id>--<epoch-millis>` form an add mints, each published
//! bound of `ClientSecretIdType`'s length, 1..128), and WHY it resolves nothing (never
//! minted, minted then deleted, minted for another app client, minted for an app client
//! of another pool, asked of a public app client that holds no secret at all). Each row
//! names the case it asserts and every assertion reports the value it saw.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The signature
/// is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The one sentence a `ClientSecretId` lookup answers when the app client holds secrets
/// and none of them is the one asked for.
const SECRET_NOT_FOUND_MESSAGE: &str = "Client secret does not exist";

/// The one sentence a `ClientSecretId` lookup answers when the app client holds no secret
/// at all, so the app client's secret itself is what does not exist.
const NO_CLIENT_SECRET_MESSAGE: &str = "User pool client secret does not exist.";

/// The length bounds of `ClientSecretIdType`
/// (`smithy_query(query="shape",shape="ClientSecretIdType")`: length 1..128), used to
/// build an id at each published bound.
const SECRET_ID_LENGTH_BOUNDS: [usize; 2] = [1, 128];

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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client of `user_pool_id` and return its minted `ClientId`.
/// `generate_secret` is `GenerateSecret`, so `false` makes a PUBLIC app client — one
/// that holds no client secret at all.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    generate_secret: bool,
) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "GenerateSecret": generate_secret,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Store one further generated secret on `client_id` and return its minted
/// `ClientSecretId`.
async fn add_secret(router: &MultiServiceRouter, user_pool_id: &str, client_id: &str) -> String {
    let added = ok(
        router,
        "AddUserPoolClientSecret",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    added["ClientSecretDescriptor"]["ClientSecretId"]
        .as_str()
        .expect("a minted ClientSecretId")
        .to_string()
}

/// The `ClientSecretId`s `client_id` holds, in the order they are reported.
async fn listed_ids(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
) -> Vec<String> {
    let listed = ok(
        router,
        "ListUserPoolClientSecrets",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    listed["ClientSecrets"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUserPoolClientSecrets must answer a list, got {listed}"))
        .iter()
        .map(|entry| {
            entry["ClientSecretId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the table: which app client the delete names, the id it asks for, and the
/// request values the refusal must quote none of.
struct NotFoundCase {
    case: String,
    user_pool_id: String,
    client_id: String,
    client_secret_id: String,
    expected: &'static str,
}

#[tokio::test]
async fn a_client_secret_lookup_that_resolves_nothing_answers_one_fixed_sentence() {
    let router = router();

    // The pool under test: one confidential app client that holds secrets, a second
    // one that holds its own, and a public app client that holds none.
    let user_pool_id = pool(&router, "client-secret-not-found-pool").await;
    let holder = client(&router, &user_pool_id, "holds-secrets", true).await;
    let sibling = client(&router, &user_pool_id, "holds-its-own", true).await;
    let public = client(&router, &user_pool_id, "holds-none", false).await;

    // Another pool, so an id that IS a secret id — of an app client of somewhere else —
    // resolves nothing here.
    let other_pool = pool(&router, "client-secret-not-found-other-pool").await;
    let elsewhere = client(&router, &other_pool, "other-pools-client", true).await;

    // A second secret makes room for a delete, so one genuinely issued id becomes an
    // id the client no longer holds.
    let added = add_secret(&router, &user_pool_id, &holder).await;
    let held_before = listed_ids(&router, &user_pool_id, &holder).await;
    assert!(
        held_before.contains(&added),
        "the added secret must be listed before the delete, got {held_before:?}"
    );
    let deleted = held_before
        .first()
        .expect("the app client holds secrets")
        .clone();
    ok(
        &router,
        "DeleteUserPoolClientSecret",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": holder,
            "ClientSecretId": deleted,
        }),
    )
    .await;

    let sibling_secret = listed_ids(&router, &user_pool_id, &sibling)
        .await
        .first()
        .expect("the sibling app client holds its own secret")
        .clone();
    let elsewhere_secret = listed_ids(&router, &other_pool, &elsewhere)
        .await
        .first()
        .expect("the other pool's app client holds its own secret")
        .clone();

    let mut cases = vec![
        NotFoundCase {
            case: "an id the delete already took away".to_string(),
            user_pool_id: user_pool_id.clone(),
            client_id: holder.clone(),
            client_secret_id: deleted.clone(),
            expected: SECRET_NOT_FOUND_MESSAGE,
        },
        NotFoundCase {
            case: "the id of a secret ANOTHER app client of the same pool holds".to_string(),
            user_pool_id: user_pool_id.clone(),
            client_id: holder.clone(),
            client_secret_id: sibling_secret.clone(),
            expected: SECRET_NOT_FOUND_MESSAGE,
        },
        NotFoundCase {
            case: "the id of a secret an app client of ANOTHER pool holds".to_string(),
            user_pool_id: user_pool_id.clone(),
            client_id: holder.clone(),
            client_secret_id: elsewhere_secret.clone(),
            expected: SECRET_NOT_FOUND_MESSAGE,
        },
    ];

    // Ids no add ever minted, in each form a caller writes one in: a 32-hex string, a
    // UUID, and the `<client-id>--<epoch-millis>` form `ClientSecretDescriptorType`
    // documents for a minted id.
    let never_minted = [
        (
            "a 32-hex id no add minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            "a UUID no add minted",
            "0e042c11-84c3-4b28-a91a-831ab2bbb450".to_string(),
        ),
        (
            "an id in the minted <client-id>--<epoch-millis> form that no add minted",
            format!("{holder}--1700000000000"),
        ),
    ];
    for (case, client_secret_id) in &never_minted {
        // The same id asked of the app client that holds secrets, and of the public app
        // client that holds none: both resolve nothing, and each reports what IT failed to
        // resolve — one secret of a client that has them, the client's secret altogether
        // for one that has none.
        for (who, asked_client, expected) in [
            (
                "a confidential app client",
                &holder,
                SECRET_NOT_FOUND_MESSAGE,
            ),
            ("a public app client", &public, NO_CLIENT_SECRET_MESSAGE),
        ] {
            cases.push(NotFoundCase {
                case: format!("{case}, asked of {who}"),
                user_pool_id: user_pool_id.clone(),
                client_id: asked_client.clone(),
                client_secret_id: client_secret_id.clone(),
                expected,
            });
        }
    }

    // And an id at each published bound of the id's length.
    for length in SECRET_ID_LENGTH_BOUNDS {
        cases.push(NotFoundCase {
            case: format!("an unminted id of the bound length {length}"),
            user_pool_id: user_pool_id.clone(),
            client_id: holder.clone(),
            client_secret_id: "z".repeat(length),
            expected: SECRET_NOT_FOUND_MESSAGE,
        });
    }

    for NotFoundCase {
        case,
        user_pool_id: asked_pool,
        client_id: asked_client,
        client_secret_id,
        expected,
    } in cases
    {
        let (status, payload) = call(
            &router,
            "DeleteUserPoolClientSecret",
            &json!({
                "UserPoolId": asked_pool,
                "ClientId": asked_client,
                "ClientSecretId": client_secret_id,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: must be a 400 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        for quoted_by_nothing in [&client_secret_id, &asked_client, &asked_pool] {
            assert!(
                !message.contains(quoted_by_nothing.as_str()),
                "case {case:?}: the refusal must quote nothing the request asked for, \
                 but {message:?} carries {quoted_by_nothing:?}"
            );
        }
    }

    // Not one of those refusals wrote anything: every app client still holds exactly the
    // secrets it held, and the deleted id is still gone from the only client that had it.
    let held_after = listed_ids(&router, &user_pool_id, &holder).await;
    assert_eq!(
        held_after,
        vec![added.clone()],
        "the refusals must leave the app client holding only the secret the delete spared, \
         got {held_after:?}"
    );
    assert_eq!(
        listed_ids(&router, &user_pool_id, &sibling).await,
        vec![sibling_secret.clone()],
        "the refusals must leave the sibling app client's secret alone"
    );
    assert_eq!(
        listed_ids(&router, &other_pool, &elsewhere).await,
        vec![elsewhere_secret.clone()],
        "the refusals must leave the other pool's app client's secret alone"
    );
    assert!(
        listed_ids(&router, &user_pool_id, &public).await.is_empty(),
        "a public app client holds no secret before or after the refusals, got {:?}",
        listed_ids(&router, &user_pool_id, &public).await
    );
}

/// The lookups that fail EARLIER are a different class and keep their own wording: a
/// pool no create minted, and an app client the named pool does not hold, must not be
/// flattened into the client-secret sentence.
#[tokio::test]
async fn an_unresolved_pool_or_app_client_keeps_its_own_wording() {
    let router = router();
    let user_pool_id = pool(&router, "client-secret-not-found-earlier-pool").await;
    let holder = client(&router, &user_pool_id, "holds-secrets", true).await;
    let secret_id = listed_ids(&router, &user_pool_id, &holder)
        .await
        .first()
        .expect("the app client holds its generated secret")
        .clone();

    // A `UserPoolIdType`-shaped id (pattern `^[\w-]+_[0-9a-zA-Z]+$`) and a
    // `ClientIdType`-shaped id (pattern `^[\w+]+$`) that no create minted.
    let unknown_pool = "us-east-1_nosuchpool";
    let unknown_client = "nosuchappclient0000";

    // The refusal names the resource that failed to resolve, so an absent pool and an
    // absent app client are each reported as themselves rather than as a missing client
    // secret. What follows that opening is each class's own wording, which these cases do
    // not pin.
    for (case, asked_pool, asked_client, names_the_resource) in [
        (
            "a pool no create minted",
            unknown_pool.to_string(),
            holder.clone(),
            "User pool ",
        ),
        (
            "an app client no create minted",
            user_pool_id.clone(),
            unknown_client.to_string(),
            "User pool client ",
        ),
    ] {
        let (status, payload) = call(
            &router,
            "DeleteUserPoolClientSecret",
            &json!({
                "UserPoolId": asked_pool,
                "ClientId": asked_client,
                "ClientSecretId": secret_id,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: must be a 400 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.starts_with(names_the_resource),
            "case {case:?}: the refusal must report {names_the_resource:?} as what did not \
             resolve, got {message:?}"
        );
        assert!(
            message != SECRET_NOT_FOUND_MESSAGE && message != NO_CLIENT_SECRET_MESSAGE,
            "case {case:?}: a lookup that fails before the secret must not answer either \
             client-secret sentence, got {message:?}"
        );
    }
}
