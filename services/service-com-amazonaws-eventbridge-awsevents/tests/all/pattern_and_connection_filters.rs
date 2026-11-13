// SPDX-License-Identifier: CC0-1.0

//! `TestEventPattern` and the connection filters, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * `TestEventPattern` answers the event-pattern LANGUAGE applied to the
//!   envelope — the same interpreter rule matching uses — so a pattern holds
//!   exactly when every one of its fields holds, a term list is a disjunction,
//!   and each documented operator decides its own comparison;
//! * a pattern the language does not admit, and an `Event` that is not an event
//!   envelope ("the following fields are mandatory: id account source time region
//!   resources detail-type", `TestEventPattern$Event`), are both
//!   `InvalidEventPatternException` — the operation's only modelled client error;
//! * `ListConnections` reports the stored connections in name order, narrowed by
//!   at most ONE filter, paged by `Limit`/`NextToken`;
//! * `UpdateConnection` writes the members the request carries and leaves the
//!   rest, refuses parameters that do not belong to the type the connection will
//!   have, and reports no credential back.
//!
//! The operator table's rows come from the EventBridge event-pattern reference —
//! the operators `crate::state::event_pattern` interprets: exact values,
//! `prefix`, `suffix`, `equals-ignore-case`, `wildcard`, `anything-but`,
//! `numeric`, `cidr`, `exists`, `$or` and nested objects — and each is tested on
//! a value the issue that asked for this operation did not cite.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "ap-southeast-2";

/// How long a test waits before reading a connection whose next state the clock
/// owes it.
fn past_the_transition() -> Duration {
    state::CONNECTION_TRANSITION + Duration::from_millis(250)
}

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

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The envelope every pattern row below is tested against: every field
/// `TestEventPattern$Event` makes mandatory, and a `detail` with a string, a
/// number, an address, an array and a nested object to reach into.
fn base_event() -> Value {
    json!({
        "version": "0",
        "id": "7bf73129-1428-4cd3-a780-95db273d1602",
        "account": engine::emulator_account_id(),
        "source": "com.demo.orders",
        "time": "2026-01-01T12:00:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": "Order Created",
        "detail": {
            "status": "FULFILLED",
            "amount": 42,
            "client-ip": "10.0.1.7",
            "labels": ["red", "blue"],
            "shipping": { "country": "DE" },
        },
    })
}

/// The base envelope with `member` replaced by `value`, or removed when `value`
/// is `None`.
fn event_with(member: &str, value: Option<Value>) -> Value {
    let mut event = base_event();
    let object = event
        .as_object_mut()
        .expect("the base envelope is an object");
    match value {
        Some(value) => {
            object.insert(member.to_string(), value);
        }
        None => {
            object.remove(member);
        }
    }
    event
}

/// Each documented operator decides its own comparison, and a pattern holds only
/// when every field of it holds.
#[tokio::test]
async fn test_event_pattern_answers_the_pattern_language_over_the_envelope() {
    let router = emulator();
    // (case, the pattern, the event, whether the pattern matches it)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "an exact value on an envelope field",
            json!({ "source": ["com.demo.orders"] }),
            base_event(),
            true,
        ),
        (
            "an exact value that differs in case only",
            json!({ "source": ["COM.DEMO.ORDERS"] }),
            base_event(),
            false,
        ),
        (
            "a term list is a disjunction",
            json!({ "source": ["com.demo.shipments", "com.demo.orders"] }),
            base_event(),
            true,
        ),
        (
            "two fields are a conjunction, and one of them fails",
            json!({ "source": ["com.demo.orders"], "detail-type": ["Order Shipped"] }),
            base_event(),
            false,
        ),
        (
            "prefix on a nested detail field",
            json!({ "detail": { "status": [{ "prefix": "FUL" }] } }),
            base_event(),
            true,
        ),
        (
            "suffix that does not hold",
            json!({ "detail": { "status": [{ "suffix": "PENDING" }] } }),
            base_event(),
            false,
        ),
        (
            "equals-ignore-case folds the comparison",
            json!({ "detail": { "status": [{ "equals-ignore-case": "fulfilled" }] } }),
            base_event(),
            true,
        ),
        (
            "wildcard spans a run of characters",
            json!({ "detail-type": [{ "wildcard": "Order*" }] }),
            base_event(),
            true,
        ),
        (
            "anything-but excludes the value the event carries",
            json!({ "detail": { "status": [{ "anything-but": "FULFILLED" }] } }),
            base_event(),
            false,
        ),
        (
            "anything-but admits a value outside its list",
            json!({ "detail": { "status": [{ "anything-but": ["PENDING", "CANCELLED"] }] } }),
            base_event(),
            true,
        ),
        (
            "numeric compares within a range",
            json!({ "detail": { "amount": [{ "numeric": [">", 40, "<=", 42] }] } }),
            base_event(),
            true,
        ),
        (
            "numeric outside its range",
            json!({ "detail": { "amount": [{ "numeric": ["<", 40] }] } }),
            base_event(),
            false,
        ),
        (
            "cidr covers the address",
            json!({ "detail": { "client-ip": [{ "cidr": "10.0.0.0/8" }] } }),
            base_event(),
            true,
        ),
        (
            "cidr that does not cover the address",
            json!({ "detail": { "client-ip": [{ "cidr": "192.168.0.0/16" }] } }),
            base_event(),
            false,
        ),
        (
            // The narrowest block the language admits: a mask as wide as the
            // family names one address rather than a block, which the service
            // refuses (see `pattern_refusal_wording`).
            "cidr at the narrow bound, covering two addresses",
            json!({ "detail": { "client-ip": [{ "cidr": "10.0.1.6/31" }] } }),
            base_event(),
            true,
        ),
        (
            "cidr at the wide bound, covering every address",
            json!({ "detail": { "client-ip": [{ "cidr": "0.0.0.0/0" }] } }),
            base_event(),
            true,
        ),
        (
            "exists true on a field the envelope carries",
            json!({ "detail": { "shipping": { "country": [{ "exists": true }] } } }),
            base_event(),
            true,
        ),
        (
            "exists false on a field the envelope does not carry",
            json!({ "detail": { "tracking": [{ "exists": false }] } }),
            base_event(),
            true,
        ),
        (
            "a term matches any member of an array-valued field",
            json!({ "detail": { "labels": ["blue"] } }),
            base_event(),
            true,
        ),
        (
            "$or holds when one branch holds",
            json!({
                "$or": [
                    { "source": ["com.demo.shipments"] },
                    { "detail": { "amount": [{ "numeric": ["=", 42] }] } },
                ],
            }),
            base_event(),
            true,
        ),
        (
            "$or fails when no branch holds",
            json!({
                "$or": [
                    { "source": ["com.demo.shipments"] },
                    { "detail": { "status": ["PENDING"] } },
                ],
            }),
            base_event(),
            false,
        ),
        (
            "a field of the event the pattern reaches that is absent entirely",
            json!({ "detail": { "status": ["FULFILLED"] } }),
            event_with("detail", Some(json!({}))),
            false,
        ),
        (
            "exists false holds when an intermediate parent is missing",
            json!({ "detail": { "shipping": { "tracking": [{ "exists": false }] } } }),
            event_with("detail", Some(json!({ "status": "FULFILLED" }))),
            true,
        ),
        (
            "exists true fails when an intermediate parent is missing",
            json!({ "detail": { "shipping": { "country": [{ "exists": true }] } } }),
            event_with("detail", Some(json!({ "status": "FULFILLED" }))),
            false,
        ),
        (
            "exists sees no leaf in an object-valued field",
            json!({ "detail": { "shipping": [{ "exists": true }] } }),
            base_event(),
            false,
        ),
        (
            "exists sees no leaf in an empty array",
            json!({ "detail": { "labels": [{ "exists": true }] } }),
            event_with(
                "detail",
                Some(json!({ "status": "FULFILLED", "labels": [] })),
            ),
            false,
        ),
        (
            "exists sees a leaf in an empty string, which is not null",
            json!({ "detail": { "status": [{ "exists": true }] } }),
            event_with("detail", Some(json!({ "status": "" }))),
            true,
        ),
        (
            "exists sees a leaf in a null-valued field, because null is a value",
            json!({ "detail": { "status": [{ "exists": true }] } }),
            event_with("detail", Some(json!({ "status": null }))),
            true,
        ),
        (
            "anything-but over a nested string operator",
            json!({ "detail": { "status": [{ "anything-but": { "prefix": "PEND" } }] } }),
            base_event(),
            true,
        ),
        (
            "anything-but over a LIST of nested string operators, one of which excludes",
            json!({
                "detail": {
                    "status": [{ "anything-but": { "prefix": ["PEND", "FUL"] } }],
                },
            }),
            base_event(),
            false,
        ),
        (
            "anything-but over an array-valued field admits any escaping element",
            json!({ "detail": { "labels": [{ "anything-but": ["red"] }] } }),
            base_event(),
            true,
        ),
        (
            "a resources ARN the envelope lists",
            json!({ "resources": ["arn:aws:s3:::demo-bucket"] }),
            event_with("resources", Some(json!(["arn:aws:s3:::demo-bucket"]))),
            true,
        ),
    ];

    for (case, pattern, event, expected) in rows {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern.to_string(), "Event": event.to_string() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["Result"],
            json!(expected),
            "case {case:?}: expected Result {expected}, got {body}"
        );
    }
}

/// A pattern the language does not admit is the operation's modelled
/// `InvalidEventPatternException`; an `Event` that is not an envelope is a bad
/// request MEMBER, so it is the refusal every unacceptable member of this service
/// answers with (`ValidationException`) — the two members are two different things
/// for the caller to fix.
#[tokio::test]
async fn test_event_pattern_refuses_a_pattern_or_an_event_it_cannot_read() {
    let router = emulator();
    // (case, the pattern, the event, the error code the refusal carries)
    let rows: Vec<(&str, Value, Value, &str)> = vec![
        (
            "a leaf value not wrapped in an array",
            json!({ "source": "com.demo.orders" }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "a field listing no value at all",
            json!({ "source": [] }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "an operator the language does not have",
            json!({ "source": [{ "starts-with": "com." }] }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "numeric with an odd number of terms",
            json!({ "detail": { "amount": [{ "numeric": [">", 1, "<"] }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "exists with something that is not a boolean",
            json!({ "detail": { "status": [{ "exists": "yes" }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "numeric naming a range whose bottom is above its top",
            json!({ "detail": { "amount": [{ "numeric": [">", 100, "<", 10] }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "numeric naming a range whose ends meet but exclude each other",
            json!({ "detail": { "amount": [{ "numeric": [">", 42, "<=", 42] }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "cidr with a block that is not one",
            json!({ "detail": { "client-ip": [{ "cidr": "10.0.0.0/notanumber" }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "cidr naming a host inside a block rather than the block itself",
            json!({ "detail": { "client-ip": [{ "cidr": "10.0.1.7/24" }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "cidr with a prefix wider than the address family admits",
            json!({ "detail": { "client-ip": [{ "cidr": "10.0.0.0/33" }] } }),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "a pattern that is not a JSON object",
            json!(["com.demo.orders"]),
            base_event(),
            "InvalidEventPatternException",
        ),
        (
            "an event missing a mandatory field",
            json!({ "source": ["com.demo.orders"] }),
            event_with("detail-type", None),
            "ValidationException",
        ),
        (
            "an event whose time is not an instant that parses",
            json!({ "source": ["com.demo.orders"] }),
            event_with("time", Some(json!("the day before yesterday"))),
            "ValidationException",
        ),
    ];

    for (case, pattern, event, code) in rows {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern.to_string(), "Event": event.to_string() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // Text that is not JSON at all, in either member: the pattern is the invalid
    // pattern, the event the invalid member.
    for (case, request, code) in [
        (
            "a pattern that is not JSON",
            json!({ "EventPattern": "{", "Event": base_event().to_string() }),
            "InvalidEventPatternException",
        ),
        (
            "an event that is not JSON",
            json!({ "EventPattern": r#"{"source":["com.demo.orders"]}"#, "Event": "{" }),
            "ValidationException",
        ),
    ] {
        let (status, body) = events(&router, "TestEventPattern", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }
}

/// An API-key connection called `name`.
async fn given_connection(router: &MultiServiceRouter, name: &str) {
    let (status, body) = events(
        router,
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
    assert_eq!(status, 200, "CreateConnection {name} answered {status} {body}");
}

/// The `Name` of every connection a listing reported.
fn listed_names(body: &Value) -> Vec<String> {
    body["Connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| connection["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `ListConnections` reports the stored connections in name order, narrowed by at
/// most one filter and paged.
#[tokio::test]
async fn list_connections_reports_the_stored_connections_under_one_filter() {
    let router = emulator();
    for name in ["alpha.one", "alpha.two", "beta"] {
        given_connection(&router, name).await;
    }
    // One connection is moved off the state the others are in, so a state filter
    // discriminates.
    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = events(
        &router,
        "DeauthorizeConnection",
        &json!({ "Name": "beta" }),
    )
    .await;
    assert_eq!(status, 200, "DeauthorizeConnection answered {status} {body}");
    tokio::time::sleep(past_the_transition()).await;

    // (case, request, the names reported, whether the listing pages on)
    let rows: Vec<(&str, Value, Vec<&str>, bool)> = vec![
        (
            "no filter at all",
            json!({}),
            vec!["alpha.one", "alpha.two", "beta"],
            false,
        ),
        (
            "a name prefix",
            json!({ "NamePrefix": "alpha" }),
            vec!["alpha.one", "alpha.two"],
            false,
        ),
        (
            "a name prefix nothing starts with",
            json!({ "NamePrefix": "gamma" }),
            vec![],
            false,
        ),
        (
            "the state the authorized connections are in",
            json!({ "ConnectionState": state::CONNECTION_STATE_AUTHORIZED }),
            vec!["alpha.one", "alpha.two"],
            false,
        ),
        (
            "the state the deauthorized connection is in",
            json!({ "ConnectionState": state::CONNECTION_STATE_DEAUTHORIZED }),
            vec!["beta"],
            false,
        ),
        (
            "a page of two, which pages on",
            json!({ "Limit": 2 }),
            vec!["alpha.one", "alpha.two"],
            true,
        ),
    ];

    for (case, request, expected, pages_on) in rows {
        let (status, body) = events(&router, "ListConnections", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            listed_names(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            body["NextToken"].is_string(),
            pages_on,
            "case {case:?}: expected a NextToken present={pages_on}, got {body}"
        );
        if !pages_on {
            continue;
        }
        let token = body["NextToken"].as_str().unwrap_or_default().to_string();
        let (status, next) = events(
            &router,
            "ListConnections",
            &json!({ "Limit": 2, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the next page answered {status} {next}"
        );
        assert_eq!(
            listed_names(&next),
            vec!["beta".to_string()],
            "case {case:?}: expected the next page to carry the last connection, got {next}"
        );
        assert!(
            !next["NextToken"].is_string(),
            "case {case:?}: the last page carries no token, got {next}"
        );
    }

    // A listing reports every member of the summary shape off the stored record.
    let (_, body) = events(&router, "ListConnections", &json!({ "NamePrefix": "beta" })).await;
    let listed = &body["Connections"][0];
    for member in [
        "ConnectionArn",
        "Name",
        "ConnectionState",
        "AuthorizationType",
        "CreationTime",
        "LastModifiedTime",
        "LastAuthorizedTime",
    ] {
        assert!(
            !listed[member].is_null(),
            "expected the listing to carry {member}, got {body}"
        );
    }
    let (_, described) = events(&router, "DescribeConnection", &json!({ "Name": "beta" })).await;
    for member in ["ConnectionArn", "ConnectionState", "AuthorizationType"] {
        assert_eq!(
            listed[member], described[member],
            "a listing and a read must agree about {member}, got {body} and {described}"
        );
    }

    // The filters are alternatives, and a state the enum does not have is not a
    // filter at all.
    for (case, request) in [
        (
            "both filters at once",
            json!({ "NamePrefix": "alpha", "ConnectionState": state::CONNECTION_STATE_AUTHORIZED }),
        ),
        (
            "a connection state the enum does not have",
            json!({ "ConnectionState": "SOMEWHAT_AUTHORIZED" }),
        ),
        (
            "a page token this service did not issue",
            json!({ "NextToken": "not-a-token" }),
        ),
    ] {
        let (status, body) = events(&router, "ListConnections", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// One row of the update table: the case, the update, the authorization type the
/// connection then has, the `ConnectionState` the update ANSWERS, the one it
/// comes to rest in, the member path a read must report, and the value it must
/// report there.
type UpdateRow = (
    &'static str,
    Value,
    &'static str,
    &'static str,
    &'static str,
    Vec<&'static str>,
    Value,
);

/// `UpdateConnection` writes what the request carries, leaves the rest, and
/// reports no credential back.
#[tokio::test]
async fn update_connection_writes_the_members_it_is_given() {
    let rows: Vec<UpdateRow> = vec![
        (
            "a change of authorization type, with the new type's credentials",
            json!({
                "AuthorizationType": "BASIC",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "new-user", "Password": "sekrit-new" },
                },
            }),
            "BASIC",
            state::CONNECTION_STATE_AUTHORIZED,
            state::CONNECTION_STATE_AUTHORIZED,
            vec!["BasicAuthParameters", "Username"],
            json!("new-user"),
        ),
        (
            // OAuth client credentials are not themselves the authorization:
            // they are exchanged for a token at the connection's authorization
            // endpoint, and nothing answers at the endpoint this row names, so
            // the update leaves the connection DEAUTHORIZED.
            "a change to OAuth, with the client parameters",
            json!({
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": {
                    "OAuthParameters": {
                        "ClientParameters": {
                            "ClientID": "new-client",
                            "ClientSecret": "sekrit-new",
                        },
                        "AuthorizationEndpoint": "https://auth.example.com/token",
                        "HttpMethod": "POST",
                    },
                },
            }),
            "OAUTH_CLIENT_CREDENTIALS",
            state::CONNECTION_STATE_UPDATING,
            state::CONNECTION_STATE_DEAUTHORIZED,
            vec!["OAuthParameters", "ClientParameters", "ClientID"],
            json!("new-client"),
        ),
        (
            // The type the parameters belong to is named even where it is the
            // one the connection already has: `AuthorizationType` is what
            // selects the block that carries the credentials, so an update that
            // supplies one without it is refused (see the refusal table below).
            "new credentials of the type the connection already has",
            json!({
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "x-other-key", "ApiKeyValue": "sekrit-new" },
                },
            }),
            "API_KEY",
            state::CONNECTION_STATE_AUTHORIZED,
            state::CONNECTION_STATE_AUTHORIZED,
            vec!["ApiKeyAuthParameters", "ApiKeyName"],
            json!("x-other-key"),
        ),
        (
            "a description alone, which leaves the credentials alone",
            json!({ "Description": "the updated connection" }),
            "API_KEY",
            state::CONNECTION_STATE_AUTHORIZED,
            state::CONNECTION_STATE_AUTHORIZED,
            vec!["ApiKeyAuthParameters", "ApiKeyName"],
            json!("x-api-key"),
        ),
    ];

    for (case, update, expected_type, answered_state, expected_state, path, expected_value) in rows
    {
        let router = emulator();
        let name = "the-connection";
        given_connection(&router, name).await;
        tokio::time::sleep(past_the_transition()).await;
        let (_, before) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;

        let mut request = update.clone();
        request["Name"] = json!(name);
        let (status, updated) = events(&router, "UpdateConnection", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateConnection answered {status} {updated}"
        );
        assert_eq!(
            updated["ConnectionArn"], before["ConnectionArn"],
            "case {case:?}: an updated connection keeps its ARN, got {updated}"
        );
        assert_eq!(
            updated["ConnectionState"],
            json!(answered_state),
            "case {case:?}: an update is answered in {answered_state}, got {updated}"
        );
        assert_eq!(
            updated["CreationTime"], before["CreationTime"],
            "case {case:?}: an update does not move the creation time, got {updated}"
        );

        // The state the update answered is the state a caller reading straight
        // after it sees; a transient one then settles on its own.
        let (_, transient) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            transient["ConnectionState"],
            json!(answered_state),
            "case {case:?}: a read taken at once still reports {answered_state}, got {transient}"
        );
        tokio::time::sleep(past_the_transition()).await;
        let (status, settled) = events(&router, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeConnection answered {status} {settled}"
        );
        assert_eq!(
            settled["ConnectionState"],
            json!(expected_state),
            "case {case:?}: the clock owes the updated connection {expected_state}, got {settled}"
        );
        assert_eq!(
            settled["AuthorizationType"],
            json!(expected_type),
            "case {case:?}: expected the authorization type {expected_type}, got {settled}"
        );
        let mut cursor = &settled["AuthParameters"];
        for step in &path {
            cursor = &cursor[*step];
        }
        assert_eq!(
            *cursor, expected_value,
            "case {case:?}: expected AuthParameters{path:?} to be {expected_value}, \
             got {settled}"
        );
        if let Some(description) = update.get("Description") {
            assert_eq!(
                settled["Description"], *description,
                "case {case:?}: expected the updated description, got {settled}"
            );
        }
        let rendered = settled.to_string();
        for secret in ["sekrit", "sekrit-new"] {
            assert!(
                !rendered.contains(secret),
                "case {case:?}: a read must report no credential, but {secret:?} is in {rendered}"
            );
        }
    }
}

/// What `UpdateConnection` refuses, and with which modelled error.
#[tokio::test]
async fn update_connection_refuses_what_the_resource_rules_forbid() {
    let router = emulator();
    given_connection(&router, "live").await;
    tokio::time::sleep(past_the_transition()).await;

    // (case, request, expected status, expected wire code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an update of a connection that does not exist",
            json!({ "Name": "absent", "Description": "nowhere" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a type change with no credentials of the new type",
            json!({ "Name": "live", "AuthorizationType": "BASIC" }),
            400,
            "ValidationException",
        ),
        (
            "parameters of a type the connection will not have",
            json!({
                "Name": "live",
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "credentials with no authorization type to select them",
            json!({
                "Name": "live",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "credentials of the type the connection already has, with no type named",
            json!({
                "Name": "live",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "p" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "an authorization type the enum does not have",
            json!({ "Name": "live", "AuthorizationType": "MUTUAL_TLS" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, body) = events(&router, "UpdateConnection", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the wire code {expected_code}, got {body}"
        );
    }

    // A refused update left the connection exactly as it was.
    let (status, body) = events(&router, "DescribeConnection", &json!({ "Name": "live" })).await;
    assert_eq!(status, 200, "DescribeConnection answered {status} {body}");
    assert_eq!(
        body["AuthorizationType"],
        json!("API_KEY"),
        "a refused update must not change the authorization type, got {body}"
    );
    assert_eq!(
        body["ConnectionState"],
        json!(state::CONNECTION_STATE_AUTHORIZED),
        "a refused update must not move the connection's state, got {body}"
    );
}
