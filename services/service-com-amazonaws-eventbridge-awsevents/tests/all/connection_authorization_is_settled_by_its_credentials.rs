// SPDX-License-Identifier: CC0-1.0

//! When a connection holds its authorization, and what every reader of it then
//! reports — end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * a connection whose authorization type's credentials ARE the authorization
//!   (`API_KEY`, `BASIC`) is settled by the call that supplies them: that call
//!   answers the terminal `AUTHORIZED` state and stamps the authorization at its
//!   own instant, with no transient state left for a later reader to see. Only a
//!   type that must first obtain a token from an authorization endpoint
//!   (`OAUTH_CLIENT_CREDENTIALS`) is answered unauthorized, and such a connection
//!   reports no authorization time until an exchange has issued one;
//! * once stamped, `LastAuthorizedTime` is retained for the life of the
//!   connection — through a deauthorization, which does not make a connection
//!   that WAS authorized look like one that never was, and into the envelope the
//!   delete answers;
//! * a connection whose authorization parameters `DeauthorizeConnection` took
//!   away reports why it holds none, as its `StateReason`.
//!
//! The table spans the class rather than one recorded case: its rows differ along
//! the authorization type, along whether the credentials are themselves the
//! authorization or have to be exchanged for a token, and along the parameter
//! block that carries them. Each row then walks the whole lifecycle, so every
//! reader of the connection (the create, the read, the listing, the update, the
//! deauthorize and the delete) is asserted on the same connection.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-2";

/// The authorization endpoint of the OAuth row: an `https` URL this emulator
/// serves nothing at, so the token exchange made against it obtains nothing.
/// Requests go through the in-process transport only.
const UNREACHABLE_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// Why a deauthorized connection holds no authorization, as `DeauthorizeConnection`
/// states it: fixed text with no variable part, spelled out here rather than
/// imported from the implementation.
const DEAUTHORIZED_REASON: &str = "The authorization parameters were removed from this connection \
                                   by a DeauthorizeConnection operation.";

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One row: the case, the `AuthorizationType`, the `AuthParameters` block that
/// carries its credentials, and whether those credentials ARE the authorization —
/// so the call that supplies them settles it.
struct Row {
    case: &'static str,
    kind: &'static str,
    parameters: Value,
    credentials_are_the_authorization: bool,
}

/// Every `ConnectionAuthorizationType` the model admits, with the member of
/// `CreateConnectionAuthRequestParameters` that carries its credentials. Source:
/// `smithy_query(query="operation", operation="CreateConnection")` —
/// `AuthorizationType` is one of `API_KEY | BASIC | OAUTH_CLIENT_CREDENTIALS`.
/// `OAUTH_CLIENT_CREDENTIALS` is the one type whose credentials are exchanged for
/// a token before anything is authorized ("OAUTH tokens are refreshed when a 401
/// or 407 response is returned").
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an api key, which IS the authorization",
            kind: "API_KEY",
            parameters: json!({
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit-key" },
            }),
            credentials_are_the_authorization: true,
        },
        Row {
            case: "basic credentials, which ARE the authorization",
            kind: "BASIC",
            parameters: json!({
                "BasicAuthParameters": { "Username": "the-user", "Password": "sekrit-password" },
            }),
            credentials_are_the_authorization: true,
        },
        Row {
            case: "oauth client credentials, which are exchanged for a token first",
            kind: "OAUTH_CLIENT_CREDENTIALS",
            parameters: json!({
                "OAuthParameters": {
                    "ClientParameters": { "ClientID": "the-client", "ClientSecret": "sekrit" },
                    "AuthorizationEndpoint": UNREACHABLE_ENDPOINT,
                    "HttpMethod": "POST",
                },
            }),
            credentials_are_the_authorization: false,
        },
    ]
}

/// The entry the listing carries for the connection called `name`.
async fn listed(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, listing) = events(router, "ListConnections", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnections answered {status} {listing}"
    );
    listing["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Connections in {listing}"))
        .iter()
        .find(|entry| entry["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: no entry called {name:?} in {listing}"))
        .clone()
}

/// The authorization a connection holds is decided by the call that supplies the
/// credentials, and the time it was last authorized is then retained by every
/// later reader of the connection.
#[tokio::test]
async fn the_call_that_supplies_the_credentials_decides_the_authorization() {
    for row in rows() {
        let Row {
            case,
            kind,
            parameters,
            credentials_are_the_authorization: settled_by_the_call,
        } = row;
        let router = emulator();
        let name = "the-connection";

        let (status, created) = events(
            &router,
            "CreateConnection",
            &json!({ "Name": name, "AuthorizationType": kind, "AuthParameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        let expected_created_state = if settled_by_the_call {
            json!(state::CONNECTION_STATE_AUTHORIZED)
        } else {
            json!(state::CONNECTION_STATE_AUTHORIZING)
        };
        assert_eq!(
            created["ConnectionState"], expected_created_state,
            "case {case:?}: expected the create to answer {expected_created_state}, got {created}"
        );

        // The read taken straight after the create: the connection holds the
        // authorization the create accepted, stamped at the create's own
        // instant, or holds none at all while a token is still to be obtained.
        let (status, described) =
            events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeConnection answered {status} {described}"
        );
        let authorized_at = described["LastAuthorizedTime"].clone();
        if settled_by_the_call {
            assert_eq!(
                described["ConnectionState"],
                json!(state::CONNECTION_STATE_AUTHORIZED),
                "case {case:?}: expected a read to report AUTHORIZED, got {described}"
            );
            assert_eq!(
                authorized_at, described["CreationTime"],
                "case {case:?}: the connection was authorized as it was created, got {described}"
            );
            assert!(
                authorized_at.is_number(),
                "case {case:?}: expected a LastAuthorizedTime, got {described}"
            );
            assert_eq!(
                described["StateReason"],
                Value::Null,
                "case {case:?}: nothing has taken this connection's authorization away, \
                 got {described}"
            );
        } else {
            assert_ne!(
                described["ConnectionState"],
                json!(state::CONNECTION_STATE_AUTHORIZED),
                "case {case:?}: a connection still to obtain a token is not authorized, \
                 got {described}"
            );
            assert_eq!(
                authorized_at,
                Value::Null,
                "case {case:?}: a connection never authorized reports no LastAuthorizedTime, \
                 got {described}"
            );
        }

        // The listing reads the same record, so it cannot disagree with it.
        // (`ConnectionState` is left out: the OAuth row's connection is still in
        // flight here, and which side of its transition each call falls on is
        // not what this asserts.)
        let entry = listed(&router, case, name).await;
        assert_eq!(
            entry["LastAuthorizedTime"], described["LastAuthorizedTime"],
            "case {case:?}: a listing and a read must agree about LastAuthorizedTime, got {entry}"
        );

        // An update that re-states the credentials supplies the authorization
        // again, so it decides it again: the same rule, on the other writer.
        let (status, updated) = events(
            &router,
            "UpdateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": kind,
                "AuthParameters": parameters,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateConnection answered {status} {updated}"
        );
        if settled_by_the_call {
            assert_eq!(
                updated["ConnectionState"],
                json!(state::CONNECTION_STATE_AUTHORIZED),
                "case {case:?}: expected the update to answer AUTHORIZED, got {updated}"
            );
            assert_eq!(
                updated["LastAuthorizedTime"], updated["LastModifiedTime"],
                "case {case:?}: the update that accepted the credentials is when the connection \
                 was last authorized, got {updated}"
            );
        } else {
            assert_ne!(
                updated["ConnectionState"],
                json!(state::CONNECTION_STATE_AUTHORIZED),
                "case {case:?}: an update that obtained no token authorizes nothing, got {updated}"
            );
            assert_eq!(
                updated["LastAuthorizedTime"],
                Value::Null,
                "case {case:?}: a connection never authorized reports no LastAuthorizedTime, \
                 got {updated}"
            );
        }
        // Whatever the update decided, it is what the read reports.
        let (_, read_back) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            read_back["LastAuthorizedTime"], updated["LastAuthorizedTime"],
            "case {case:?}: a read and the update that wrote it must agree, got {read_back}"
        );
        let authorized_at = updated["LastAuthorizedTime"].clone();

        // The deauthorize takes the authorization away and says why, and keeps
        // WHEN the connection was last authorized: a connection that was
        // authorized and then revoked is not one that never was.
        let (status, deauthorized) =
            events(&router, "DeauthorizeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeauthorizeConnection answered {status} {deauthorized}"
        );
        assert_eq!(
            deauthorized["LastAuthorizedTime"], authorized_at,
            "case {case:?}: expected the retained {authorized_at}, got {deauthorized}"
        );
        let (status, after) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeConnection answered {status} {after}"
        );
        assert_eq!(
            after["LastAuthorizedTime"], authorized_at,
            "case {case:?}: expected the retained {authorized_at}, got {after}"
        );
        assert_eq!(
            after["StateReason"],
            json!(DEAUTHORIZED_REASON),
            "case {case:?}: expected the documented reason, got {after}"
        );
        assert_eq!(
            listed(&router, case, name).await["StateReason"],
            json!(DEAUTHORIZED_REASON),
            "case {case:?}: a listing reports the same reason as a read"
        );

        // The delete is the last observation of the connection, and it carries
        // the same stamp.
        let (status, deleted) = events(&router, "DeleteConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteConnection answered {status} {deleted}"
        );
        assert_eq!(
            deleted["LastAuthorizedTime"], authorized_at,
            "case {case:?}: expected the retained {authorized_at}, got {deleted}"
        );
    }
}

/// A connection the create settled holds its authorization because nothing is
/// pending on it: reading it any number of times moves nothing, and the stamps it
/// reports do not drift.
#[tokio::test]
async fn reading_a_settled_connection_moves_nothing_about_it() {
    let router = emulator();
    let name = "settled-connection";
    let (status, created) = events(
        &router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {created}");

    let (_, first) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
    for attempt in 1..=3 {
        let (status, again) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: DescribeConnection answered {status} {again}"
        );
        assert_eq!(
            again, first,
            "attempt {attempt}: a read must change nothing about the connection, got {again}"
        );
    }
    // The create's own envelope and the read agree about the state and the
    // timestamps they share.
    for member in ["ConnectionArn", "ConnectionState", "CreationTime", "LastModifiedTime"] {
        assert_eq!(
            created[member], first[member],
            "expected the create and the read to agree about {member}, got {created} and {first}"
        );
    }
}
