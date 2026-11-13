// SPDX-License-Identifier: CC0-1.0

//! `DeleteUserPoolClientSecret` over the wire: one secret taken off an app client.
//!
//! The rule under test. The operation "deletes a specific client secret from a user pool
//! app client. You cannot delete the last remaining secret for an app client" (this
//! operation in the service model). So one call drops the ONE stored secret whose
//! `ClientSecretId` the request names from the named app client of the named pool, leaves
//! every other secret that client holds stored and active, answers a response with no
//! members, refuses an id the named client holds no secret for as absent, and refuses the
//! delete outright while the named secret is the only one the client still holds.
//!
//! The rows below differ along what that rule generalises over: how many secrets the app
//! client holds when the delete arrives (one, the two an app client may hold at once),
//! where each one came from (the create's `GenerateSecret`, a value the create supplied, a
//! generated add, an add supplying a value on each published bound of `ClientSecretType`,
//! 24..64), and which of the held secrets the delete names (the oldest or the newest).
//!
//! Every delete is read back through the operations that report and consume the stored
//! set: `ListUserPoolClientSecrets`, which reports every secret the client holds,
//! `DescribeUserPoolClient`, which renders the app client's own `ClientSecret`,
//! `GetClientToken`, whose machine-to-machine grant is authorized by each stored secret,
//! and `AddUserPoolClientSecret`, which may store one further secret once a delete has
//! made room for it.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for.
const DEFAULT_REGION: &str = "us-east-1";

/// The operation under test.
const DELETE: &str = "DeleteUserPoolClientSecret";

/// The operation that stores one further secret on an app client.
const ADD: &str = "AddUserPoolClientSecret";

/// The operation that reports the secrets an app client holds.
const LIST: &str = "ListUserPoolClientSecrets";

/// The `ExplicitAuthFlows` value that configures an app client for machine-to-machine
/// authorization, which is the grant a stored secret authorizes.
const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// How many secrets one app client holds at a time: `AddUserPoolClientSecret` "supports
/// up to 2 active secrets per app client" (that operation in the service model).
const MAX_CLIENT_SECRETS: usize = 2;

/// A `ClientSecretType`-shaped value (pattern `^[\w+]+$`) of the modelled MINIMUM length,
/// 24 (`smithy_query(query="shape",shape="ClientSecretType")`).
const SHORTEST_SECRET: &str = "eeeeeeeeeeeeeeeeeeeeeeee";

/// A `ClientSecretType`-shaped value of the modelled MAXIMUM length, 64.
const LONGEST_SECRET: &str = "f123456789_123456789_123456789_123456789_123456789_123456789_12+";

/// A `ClientSecretType`-shaped value no app client of the suite holds.
const UNHELD_SECRET: &str = "nosuchclientsecret000000000";

/// A `UserPoolIdType`-shaped id (pattern `^[\w-]+_[0-9a-zA-Z]+$`) no create minted.
const UNKNOWN_POOL_ID: &str = "us-east-1_nosuchpool";

/// A `ClientIdType`-shaped id (pattern `^[\w+]+$`) no create minted.
const UNKNOWN_CLIENT_ID: &str = "nosuchappclient0000";

/// A `ClientSecretIdType`-shaped id (length 1..128) no add minted.
const UNKNOWN_SECRET_ID: &str = "nosuchappclient0000--1700000000000";

/// The one sentence a `ClientSecretId` lookup answers when the app client holds secrets
/// and none of them is the one asked for: fixed text, quoting neither the id asked for
/// nor the app client, and with no trailing period.
const SECRET_NOT_FOUND_MESSAGE: &str = "Client secret does not exist";

/// The one sentence that lookup answers when the app client holds no secret at all, so
/// the app client's secret itself is what does not exist: fixed text as well.
const NO_CLIENT_SECRET_MESSAGE: &str = "User pool client secret does not exist.";

/// The one sentence the refusal to take an app client's only secret away carries:
/// fixed text, quoting neither the app client, the pool nor the `ClientSecretId`
/// the request named.
const LAST_SECRET_MESSAGE: &str = "Cannot delete the only client secret.";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{DEFAULT_REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
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

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
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

/// Call `operation` expecting a refusal, assert its status and wire error code, and hand
/// the body back so a caller can read the message too.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    payload
}

/// What the create stored on the app client, which is the first of the secrets a delete
/// can take away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatedWith {
    /// `GenerateSecret: true`, so the create minted the client's first secret.
    Generated,
    /// The create supplied this `ClientSecret` value.
    Supplied(&'static str),
    /// Neither, so the app client is public and holds no secret at all.
    Public,
}

/// Which of the secrets an app client holds the delete names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The secret stored first, which is the one a read renders as the client's
    /// `ClientSecret`.
    Oldest,
    /// The secret stored last.
    Newest,
}

/// A created pool and the app client of it whose secrets are deleted.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    /// Every secret VALUE the app client holds, in the order it was stored.
    held: Vec<String>,
}

/// Create a pool named `pool_name` and one app client of it, configured for the
/// machine-to-machine grant so a stored secret can be read back through it.
async fn fixture(
    router: &MultiServiceRouter,
    case: &str,
    pool_name: &str,
    created_with: CreatedWith,
) -> Fixture {
    let created = ok(
        router,
        case,
        "CreateUserPool",
        &json!({ "PoolName": pool_name }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("deleting-client"));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([CLIENT_TOKEN_AUTH_FLOW]),
    );
    match created_with {
        CreatedWith::Generated => {
            body.insert("GenerateSecret".to_string(), json!(true));
        }
        CreatedWith::Supplied(secret) => {
            body.insert("ClientSecret".to_string(), json!(secret));
        }
        CreatedWith::Public => {}
    }
    let client = ok(router, case, "CreateUserPoolClient", &Value::Object(body)).await;

    Fixture {
        user_pool_id,
        client_id: client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        held: client["UserPoolClient"]["ClientSecret"]
            .as_str()
            .map(str::to_string)
            .into_iter()
            .collect(),
    }
}

/// Store one further secret on the fixture's app client — the value `supplied`, or a
/// generated one — and record what it stored.
async fn add_secret(
    router: &MultiServiceRouter,
    case: &str,
    fixture: &mut Fixture,
    supplied: Option<&str>,
) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    if let Some(secret) = supplied {
        body.insert("ClientSecret".to_string(), json!(secret));
    }
    let payload = ok(router, case, ADD, &Value::Object(body)).await;
    // "The ClientSecretValue is only returned when Amazon Cognito generates the secret"
    // (`AddUserPoolClientSecret.ClientSecretDescriptor`), so a generated value is only
    // knowable from the add that minted it.
    let stored = match supplied {
        Some(secret) => secret.to_string(),
        None => payload["ClientSecretDescriptor"]["ClientSecretValue"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a generated secret comes back in ClientSecretValue, got {payload}")
            })
            .to_string(),
    };
    fixture.held.push(stored);
}

/// The `ClientSecretId`s the listing reports for `fixture`'s app client, oldest first.
async fn listed_ids(router: &MultiServiceRouter, case: &str, fixture: &Fixture) -> Vec<String> {
    let payload = ok(
        router,
        case,
        LIST,
        &json!({ "UserPoolId": fixture.user_pool_id, "ClientId": fixture.client_id }),
    )
    .await;
    payload
        .get("ClientSecrets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must answer a ClientSecrets list, got {payload}")
        })
        .iter()
        .map(|descriptor| {
            descriptor["ClientSecretId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed secret must carry a ClientSecretId, got {descriptor}")
                })
                .to_string()
        })
        .collect()
}

/// Whether the machine-to-machine grant of `client_id` accepts `secret`: the grant "must
/// configure the app client with a client secret" and is authorized by the STORED one
/// (`GetClientToken` in the service model).
async fn grant_accepts(router: &MultiServiceRouter, client_id: &str, secret: &str) -> bool {
    let (status, payload) = dispatch(
        router,
        "GetClientToken",
        &json!({ "ClientId": client_id, "Secret": secret }),
    )
    .await;
    if status == 200 {
        return true;
    }
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "a refused grant is a 400 NotAuthorizedException, got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    false
}

/// The `ClientSecret` `DescribeUserPoolClient` renders for the fixture's app client.
async fn described_secret(
    router: &MultiServiceRouter,
    case: &str,
    fixture: &Fixture,
) -> Option<String> {
    ok(
        router,
        case,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": fixture.user_pool_id, "ClientId": fixture.client_id }),
    )
    .await["UserPoolClient"]["ClientSecret"]
        .as_str()
        .map(str::to_string)
}

/// The delete's request body.
fn delete_body(user_pool_id: &str, client_id: &str, client_secret_id: &str) -> Value {
    json!({
        "UserPoolId": user_pool_id,
        "ClientId": client_id,
        "ClientSecretId": client_secret_id,
    })
}

/// Assert a successful delete answered a response with no members, which is what
/// `DeleteUserPoolClientSecretResponse` models.
fn assert_empty_response(case: &str, payload: &Value) {
    let empty = match payload {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: the delete answers a response with no members, got {payload}"
    );
}

/// One row of the delete table: what the app client holds and which secret of it the
/// delete names.
struct DeleteCase {
    case: &'static str,
    created_with: CreatedWith,
    /// One entry per add before the delete, each the `ClientSecret` it supplies or `None`
    /// for a generated one.
    adds: &'static [Option<&'static str>],
    target: Target,
}

#[tokio::test]
async fn a_deleted_client_secret_is_dropped_and_every_other_one_stays_active() {
    // The two supplied values below sit on the published bounds of `ClientSecretType`
    // (length 24..64), so a typo in either is a failure of this suite rather than a
    // surprise refusal by the framework's constraint checks.
    assert_eq!(
        (SHORTEST_SECRET.len(), LONGEST_SECRET.len()),
        (24, 64),
        "the bound rows must sit on the modelled bounds, got ({}, {})",
        SHORTEST_SECRET.len(),
        LONGEST_SECRET.len()
    );

    let rows = [
        DeleteCase {
            case: "the generated secret the create stored, beside a generated add",
            created_with: CreatedWith::Generated,
            adds: &[None],
            target: Target::Oldest,
        },
        DeleteCase {
            case: "the generated add, beside the secret the create generated",
            created_with: CreatedWith::Generated,
            adds: &[None],
            target: Target::Newest,
        },
        DeleteCase {
            case: "the secret the create supplied, beside the shortest modelled add",
            created_with: CreatedWith::Supplied("createsecret000000000000021"),
            adds: &[Some(SHORTEST_SECRET)],
            target: Target::Oldest,
        },
        DeleteCase {
            case: "the longest modelled add, beside the secret the create supplied",
            created_with: CreatedWith::Supplied("createsecret000000000000022"),
            adds: &[Some(LONGEST_SECRET)],
            target: Target::Newest,
        },
        DeleteCase {
            case: "the older of the two adds of a client created public",
            created_with: CreatedWith::Public,
            adds: &[None, None],
            target: Target::Oldest,
        },
        DeleteCase {
            case: "the newer of the two adds of a client created public",
            created_with: CreatedWith::Public,
            adds: &[Some("suppliedfirstsecret00000021"), None],
            target: Target::Newest,
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        let router = router();
        let case = row.case;
        let mut fixture =
            fixture(&router, case, &format!("deleting-pool-{at}"), row.created_with).await;
        for supplied in row.adds {
            add_secret(&router, case, &mut fixture, *supplied).await;
        }
        assert_eq!(
            fixture.held.len(),
            MAX_CLIENT_SECRETS,
            "case {case:?}: the row must bring the client to {MAX_CLIENT_SECRETS} secrets, got {}",
            fixture.held.len()
        );

        // The listing reports the stored secrets oldest first, so its ids line up with the
        // values the create and the adds stored.
        let ids = listed_ids(&router, case, &fixture).await;
        assert_eq!(
            ids.len(),
            fixture.held.len(),
            "case {case:?}: the listing must report every stored secret, got {ids:?}"
        );
        let deleted_at = match row.target {
            Target::Oldest => 0,
            Target::Newest => ids.len() - 1,
        };
        let deleted_id = ids[deleted_at].clone();
        let deleted_value = fixture.held[deleted_at].clone();
        let remaining_ids: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted_at)
            .map(|(_, id)| id.clone())
            .collect();
        let remaining_values: Vec<String> = fixture
            .held
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted_at)
            .map(|(_, value)| value.clone())
            .collect();

        let payload = ok(
            &router,
            case,
            DELETE,
            &delete_body(&fixture.user_pool_id, &fixture.client_id, &deleted_id),
        )
        .await;
        assert_empty_response(case, &payload);

        // The deleted secret is gone from the stored set, and every other one is still
        // there, in the order it was stored.
        assert_eq!(
            listed_ids(&router, case, &fixture).await,
            remaining_ids,
            "case {case:?}: after deleting {deleted_id:?} the listing must report {remaining_ids:?}"
        );
        assert!(
            !grant_accepts(&router, &fixture.client_id, &deleted_value).await,
            "case {case:?}: the deleted secret must no longer authenticate the app client"
        );
        for value in &remaining_values {
            assert!(
                grant_accepts(&router, &fixture.client_id, value).await,
                "case {case:?}: the held secret {value:?} must still authenticate the app client"
            );
        }

        // `UserPoolClientType.ClientSecret` is "the app client secret": the oldest secret
        // the client still holds.
        let described = described_secret(&router, case, &fixture).await;
        assert_eq!(
            described.as_deref(),
            remaining_values.first().map(String::as_str),
            "case {case:?}: DescribeUserPoolClient must render {:?}, got {described:?}",
            remaining_values.first()
        );

        // The delete took the secret away for good: the same id now names nothing.
        let repeated = refused(
            &router,
            case,
            DELETE,
            &delete_body(&fixture.user_pool_id, &fixture.client_id, &deleted_id),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            error_message(&repeated),
            SECRET_NOT_FOUND_MESSAGE,
            "case {case:?}: expected {SECRET_NOT_FOUND_MESSAGE:?}, got {:?}",
            error_message(&repeated)
        );
        assert!(
            !error_message(&repeated).contains(deleted_id.as_str()),
            "case {case:?}: the refusal must not quote the id the request asked for, \
             got {:?}",
            error_message(&repeated)
        );

        // "You cannot delete the last remaining secret for an app client": the one secret
        // the client now holds stays whatever its id is.
        let only_id = remaining_ids
            .first()
            .expect("the client still holds one secret");
        let last = refused(
            &router,
            case,
            DELETE,
            &delete_body(&fixture.user_pool_id, &fixture.client_id, only_id),
            400,
            "InvalidParameterException",
        )
        .await;
        assert_eq!(
            error_message(&last),
            LAST_SECRET_MESSAGE,
            "case {case:?}: expected {LAST_SECRET_MESSAGE:?}, got {:?}",
            error_message(&last)
        );
        assert!(
            !error_message(&last).contains(fixture.client_id.as_str()),
            "case {case:?}: the refusal must not quote the app client the request named, \
             got {:?}",
            error_message(&last)
        );
        assert_eq!(
            listed_ids(&router, case, &fixture).await,
            remaining_ids,
            "case {case:?}: the refused delete leaves the last secret stored"
        );

        // The delete freed the room the two-secret limit refuses an add in, so the client
        // takes one more secret and both it holds authenticate it.
        fixture.held = remaining_values.clone();
        add_secret(&router, case, &mut fixture, None).await;
        let after_add = listed_ids(&router, case, &fixture).await;
        assert_eq!(
            after_add.len(),
            MAX_CLIENT_SECRETS,
            "case {case:?}: an add after the delete brings the client back to {MAX_CLIENT_SECRETS} secrets, got {after_add:?}"
        );
        assert!(
            !after_add.contains(&deleted_id),
            "case {case:?}: the deleted id never comes back, got {after_add:?}"
        );
        for value in &fixture.held {
            assert!(
                grant_accepts(&router, &fixture.client_id, value).await,
                "case {case:?}: the secret {value:?} must authenticate the app client"
            );
        }
    }
}

/// What an app client that holds exactly one secret got it from.
#[derive(Debug, Clone, Copy)]
struct LastSecretCase {
    created_with: CreatedWith,
    /// The add that stored the client's only secret, for a client created public.
    add: Option<Option<&'static str>>,
}

#[tokio::test]
async fn the_last_remaining_client_secret_of_an_app_client_is_never_deleted() {
    let rows = [
        LastSecretCase {
            created_with: CreatedWith::Generated,
            add: None,
        },
        LastSecretCase {
            created_with: CreatedWith::Supplied("createsecret000000000000031"),
            add: None,
        },
        LastSecretCase {
            created_with: CreatedWith::Public,
            add: Some(None),
        },
        LastSecretCase {
            created_with: CreatedWith::Public,
            add: Some(Some(SHORTEST_SECRET)),
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        let router = router();
        let case = format!("the only secret of a client created {:?}", row.created_with);
        let mut fixture =
            fixture(&router, &case, &format!("last-secret-pool-{at}"), row.created_with).await;
        if let Some(supplied) = row.add {
            add_secret(&router, &case, &mut fixture, supplied).await;
        }
        let ids = listed_ids(&router, &case, &fixture).await;
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: the row must leave the client holding one secret, got {ids:?}"
        );

        let payload = refused(
            &router,
            &case,
            DELETE,
            &delete_body(&fixture.user_pool_id, &fixture.client_id, &ids[0]),
            400,
            "InvalidParameterException",
        )
        .await;
        assert_eq!(
            error_message(&payload),
            LAST_SECRET_MESSAGE,
            "case {case:?}: expected {LAST_SECRET_MESSAGE:?}, got {:?}",
            error_message(&payload)
        );
        assert!(
            !error_message(&payload).contains(&ids[0]) &&
                !error_message(&payload).contains(fixture.client_id.as_str()),
            "case {case:?}: the refusal must quote neither the secret id nor the app client, \
             got {:?}",
            error_message(&payload)
        );

        // The refusal changed nothing: the secret is still stored, still the client's
        // `ClientSecret`, and still authenticates it.
        assert_eq!(
            listed_ids(&router, &case, &fixture).await,
            ids,
            "case {case:?}: the refused delete leaves the stored secret alone"
        );
        assert_eq!(
            described_secret(&router, &case, &fixture).await.as_deref(),
            fixture.held.first().map(String::as_str),
            "case {case:?}: the app client still renders its own secret"
        );
        assert!(
            grant_accepts(&router, &fixture.client_id, &fixture.held[0]).await,
            "case {case:?}: the secret the delete refused must still authenticate the client"
        );
    }
}

/// Which part of a delete's request names nothing the state holds.
#[derive(Debug, Clone, Copy)]
enum Absent {
    /// A `UserPoolId` no create minted.
    UserPool,
    /// A `ClientId` no create minted.
    AppClient,
    /// A `ClientId` that names an app client of ANOTHER user pool, which the named pool
    /// holds no app client for.
    AppClientOfAnotherPool,
    /// A `ClientSecretId` no add minted.
    ClientSecret,
    /// A `ClientSecretId` that names a secret of ANOTHER app client, which the named
    /// client holds no secret for.
    ClientSecretOfAnotherClient,
    /// Any `ClientSecretId` presented to an app client that holds no secret at all.
    ClientSecretOfAPublicClient,
}

#[tokio::test]
async fn a_delete_for_a_resource_no_app_client_holds_is_refused() {
    for (at, absent) in [
        Absent::UserPool,
        Absent::AppClient,
        Absent::AppClientOfAnotherPool,
        Absent::ClientSecret,
        Absent::ClientSecretOfAnotherClient,
        Absent::ClientSecretOfAPublicClient,
    ]
    .iter()
    .enumerate()
    {
        let router = router();
        let case = format!("{absent:?} named by the delete");

        // The named client holds the two secrets an app client may hold at once, so no row
        // below is refused for being the client's last secret.
        let mut named = fixture(
            &router,
            &case,
            &format!("not-found-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;
        add_secret(&router, &case, &mut named, None).await;
        let mut elsewhere = fixture(
            &router,
            &case,
            &format!("other-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;
        add_secret(&router, &case, &mut elsewhere, None).await;
        let public = fixture(
            &router,
            &case,
            &format!("public-pool-{at}"),
            CreatedWith::Public,
        )
        .await;

        let named_ids = listed_ids(&router, &case, &named).await;
        let elsewhere_ids = listed_ids(&router, &case, &elsewhere).await;

        let (user_pool_id, client_id, secret_id, expected) = match absent {
            Absent::UserPool => (
                UNKNOWN_POOL_ID.to_string(),
                named.client_id.clone(),
                named_ids[0].clone(),
                format!("User pool {UNKNOWN_POOL_ID} does not exist."),
            ),
            Absent::AppClient => (
                named.user_pool_id.clone(),
                UNKNOWN_CLIENT_ID.to_string(),
                named_ids[0].clone(),
                format!("User pool client {UNKNOWN_CLIENT_ID} does not exist."),
            ),
            Absent::AppClientOfAnotherPool => (
                named.user_pool_id.clone(),
                elsewhere.client_id.clone(),
                elsewhere_ids[0].clone(),
                format!(
                    "User pool client {} does not exist in user pool {}.",
                    elsewhere.client_id, named.user_pool_id
                ),
            ),
            Absent::ClientSecret => (
                named.user_pool_id.clone(),
                named.client_id.clone(),
                UNKNOWN_SECRET_ID.to_string(),
                SECRET_NOT_FOUND_MESSAGE.to_string(),
            ),
            Absent::ClientSecretOfAnotherClient => (
                named.user_pool_id.clone(),
                named.client_id.clone(),
                elsewhere_ids[0].clone(),
                SECRET_NOT_FOUND_MESSAGE.to_string(),
            ),
            Absent::ClientSecretOfAPublicClient => (
                public.user_pool_id.clone(),
                public.client_id.clone(),
                UNKNOWN_SECRET_ID.to_string(),
                NO_CLIENT_SECRET_MESSAGE.to_string(),
            ),
        };

        let payload = refused(
            &router,
            &case,
            DELETE,
            &delete_body(&user_pool_id, &client_id, &secret_id),
            400,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&payload)
        );

        // The refusal deleted nothing: every app client still holds exactly the secrets it
        // held, and each of them still authenticates its client.
        for (client, ids) in [(&named, &named_ids), (&elsewhere, &elsewhere_ids)] {
            assert_eq!(
                &listed_ids(&router, &case, client).await,
                ids,
                "case {case:?}: a refused delete leaves an app client's secrets alone"
            );
            for value in &client.held {
                assert!(
                    grant_accepts(&router, &client.client_id, value).await,
                    "case {case:?}: the held secret {value:?} must still authenticate its client"
                );
            }
        }
        assert!(
            listed_ids(&router, &case, &public).await.is_empty(),
            "case {case:?}: the public app client still holds no secret"
        );
        assert!(
            !grant_accepts(&router, &named.client_id, UNHELD_SECRET).await,
            "case {case:?}: the delete stores no secret of its own"
        );
    }
}

#[tokio::test]
async fn a_delete_that_breaks_a_modelled_constraint_is_refused_as_a_bad_request() {
    let router = router();
    let case = "a ClientSecretId shorter than the modelled minimum";
    let mut fixture = fixture(
        &router,
        case,
        "constraint-pool",
        CreatedWith::Generated,
    )
    .await;
    add_secret(&router, case, &mut fixture, None).await;
    let ids = listed_ids(&router, case, &fixture).await;

    // `ClientSecretId` is 1..128 characters, so an empty one is refused by the framework's
    // constraint checks — with a 4xx that is never the scaffold's 501.
    let (status, payload) = dispatch(
        &router,
        DELETE,
        &delete_body(&fixture.user_pool_id, &fixture.client_id, ""),
    )
    .await;
    assert!(
        (400..500).contains(&status),
        "case {case:?}: a bad request is refused with a 4xx, never the scaffold's 501, got {status} with {payload}"
    );
    assert_eq!(
        listed_ids(&router, case, &fixture).await,
        ids,
        "case {case:?}: the refused delete leaves the stored secrets alone"
    );
}
