// SPDX-License-Identifier: CC0-1.0

//! How an `OAUTH_CLIENT_CREDENTIALS` connection becomes usable, and what the
//! invocations it authorizes then carry.
//!
//! The connection is written over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.CreateConnection`) through the real
//! `MultiServiceRouter`, so what the exchange is made from and what the
//! invocation carries are what a client's own `CreateConnection` stored.
//!
//! The RULES asserted here:
//!
//! * the token request such a connection makes at its `AuthorizationEndpoint` is
//!   the OAuth 2.0 client-credentials grant (RFC 6749 §4.4.2): the connection's
//!   `HttpMethod` request, its `OAuthHttpParameters$QueryStringParameters` on the
//!   query string, that block's `HeaderParameters` as request headers, and an
//!   `application/x-www-form-urlencoded` body whose fields are `client_id`,
//!   `client_secret` — from `ClientParameters`, as submitted — and every
//!   `BodyParameters` entry beside them;
//! * the connection holds the `access_token` that request answered with
//!   (RFC 6749 §5.1) and reaches `AUTHORIZED`, which makes an API destination
//!   invoking through it `ACTIVE`; an answer that issues no token leaves it
//!   `DEAUTHORIZED` and the destination `INACTIVE`, so nothing is reported as
//!   authorized that no token was obtained for;
//! * every invocation through an authorized connection carries that token as
//!   `Authorization: Bearer <token>` (RFC 6750 §2.1), and carries neither the
//!   client secret nor the token anywhere a read of the connection can see.
//!
//! The tables span the class rather than one case: the rows of the grant table
//! differ along the connection's `HttpMethod` and along which `OAuthHttpParameters`
//! it carries (none, a header, a query-string parameter, a body parameter, a
//! secret-flagged value, all of them), and the rows of the outcome table differ
//! along what the authorization endpoint ANSWERS — a token, a token response
//! carrying none, an empty one, a body that is not a token response at all, a
//! non-2xx status, and an endpoint that answers nothing.

use std::sync::{Arc, Mutex};

use aws_smithy_runtime_api::client::http::{
    HttpConnector, HttpConnectorFuture, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::ops::types::{
    ConnectionBodyParameter, ConnectionHeaderParameter, ConnectionHttpParameters,
    ConnectionQueryStringParameter,
};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::state::invocation::{
    api_destination_invocation, ConnectionSecret,
};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-1";

/// The endpoint the token request of every row is made against. Nothing of this
/// test reaches a network: the grant is handed the stub transport below, and the
/// connection written over the wire is handed the in-process emulator transport,
/// which serves nothing at this URL.
const AUTHORIZATION_ENDPOINT: &str = "https://authorization.example.com/oauth/token";

/// The client credentials every row submits. They are what the grant has to
/// carry and what no read of the connection may report.
const CLIENT_ID: &str = "the-client";
const CLIENT_SECRET: &str = "sekrit-client-secret";

/// The content type the client-credentials grant is sent as.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// One request the stub endpoint received, as the assertions read it.
#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    /// The value of the header `name`, or `None` when the request carried none.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The form fields the body carries, in the order it wrote them.
    fn form(&self) -> Vec<(String, String)> {
        form_urlencoded::parse(self.body.as_bytes())
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }
}

/// What the stub endpoint answers.
#[derive(Debug, Clone)]
enum Answer {
    /// This status and body.
    Http(u16, String),
    /// Nothing at all: the endpoint could not be reached.
    Unreachable,
}

/// An authorization endpoint that records every request it receives and answers
/// what the row configured — the stand-in for the one an account runs, so the
/// grant is asserted on the request it actually made.
#[derive(Debug, Clone)]
struct Endpoint {
    received: Arc<Mutex<Vec<Recorded>>>,
    answer: Answer,
}

impl Endpoint {
    fn new(answer: Answer) -> Self {
        Self {
            received: Arc::new(Mutex::new(Vec::new())),
            answer,
        }
    }

    /// The requests it received, in order.
    fn received(&self) -> Vec<Recorded> {
        self.received
            .lock()
            .expect("the recorder is not poisoned")
            .clone()
    }

    /// It as the transport a token request is handed.
    fn transport(&self) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

impl HttpConnector for Endpoint {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let recorded = Recorded {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            headers: request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: String::from_utf8_lossy(request.body().bytes().unwrap_or_default()).to_string(),
        };
        self.received
            .lock()
            .expect("the recorder is not poisoned")
            .push(recorded);
        let answer = self.answer.clone();
        HttpConnectorFuture::new(Box::pin(async move {
            match answer {
                Answer::Unreachable => Err(ConnectorError::io(Box::new(std::io::Error::other(
                    "nothing answers at that endpoint",
                )))),
                Answer::Http(status, body) => {
                    let response = http::Response::builder()
                        .status(status)
                        .body(aws_smithy_types::body::SdkBody::from(body))
                        .expect("the stub response is well formed");
                    HttpResponse::try_from(response)
                        .map_err(|error| ConnectorError::other(Box::new(error), None))
                }
            }
        }))
    }
}

/// A token response carrying `token`, as RFC 6749 §5.1 defines one.
fn token_response(token: &str) -> String {
    json!({ "access_token": token, "token_type": "Bearer", "expires_in": 3600 }).to_string()
}

/// The credential half of an OAuth connection's parameters, as the store holds
/// it: the client credentials, the endpoint, the method and the exchange's own
/// HTTP parameters.
fn credential(
    http_method: &str,
    http_parameters: Option<ConnectionHttpParameters>,
) -> state::oauth::OAuthCredential {
    state::oauth::OAuthCredential {
        client_id: CLIENT_ID.to_string(),
        client_secret: CLIENT_SECRET.to_string(),
        authorization_endpoint: AUTHORIZATION_ENDPOINT.to_string(),
        http_method: http_method.to_string(),
        http_parameters,
    }
}

/// `OAuthHttpParameters` carrying the header, query-string and body parameters a
/// row names, each with the `IsValueSecret` flag the row states.
fn oauth_http_parameters(
    headers: &[(&str, &str, bool)],
    query: &[(&str, &str)],
    body: &[(&str, &str)],
) -> ConnectionHttpParameters {
    ConnectionHttpParameters {
        header_parameters: (!headers.is_empty()).then(|| {
            headers
                .iter()
                .map(|(key, value, secret)| ConnectionHeaderParameter {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                    is_value_secret: *secret,
                })
                .collect()
        }),
        query_string_parameters: (!query.is_empty()).then(|| {
            query
                .iter()
                .map(|(key, value)| ConnectionQueryStringParameter {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                    is_value_secret: false,
                })
                .collect()
        }),
        body_parameters: (!body.is_empty()).then(|| {
            body.iter()
                .map(|(key, value)| ConnectionBodyParameter {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                    is_value_secret: false,
                })
                .collect()
        }),
    }
}

/// One row of the grant table: the case, the credential the connection holds, and
/// the request the grant is to make — the method, the URI, the headers it must
/// carry beside the content type, and the form fields in the order they are
/// written.
struct GrantRow {
    case: &'static str,
    credential: state::oauth::OAuthCredential,
    method: &'static str,
    uri: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    form: Vec<(&'static str, &'static str)>,
}

/// The token request is the client-credentials grant, over a table of connections.
#[tokio::test]
async fn the_token_request_is_the_client_credentials_grant() {
    let rows = vec![
        GrantRow {
            case: "a connection carrying no OAuth HTTP parameters",
            credential: credential("POST", None),
            method: "POST",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            // A connection that names no method at all posts its credentials,
            // which is what the grant is defined as.
            case: "a connection naming no HTTP method",
            credential: credential("", None),
            method: "POST",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            case: "a connection whose exchange uses another method the model admits",
            credential: credential("PUT", None),
            method: "PUT",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            case: "a connection carrying one query-string parameter",
            credential: credential(
                "POST",
                Some(oauth_http_parameters(&[], &[("aq", "a1")], &[])),
            ),
            method: "POST",
            uri: "https://authorization.example.com/oauth/token?aq=a1",
            headers: vec![],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            case: "a connection carrying one header parameter",
            credential: credential(
                "POST",
                Some(oauth_http_parameters(&[("x-oauth-h", "oh", false)], &[], &[])),
            ),
            method: "POST",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![("x-oauth-h", "oh")],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            case: "a connection carrying one body parameter",
            credential: credential(
                "POST",
                Some(oauth_http_parameters(&[], &[], &[("scope", "orders/write")])),
            ),
            method: "POST",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![],
            form: vec![
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("scope", "orders/write"),
            ],
        },
        GrantRow {
            // A value the write flagged SECRET is one the endpoint is sent: the
            // flag decides what a READ reports, never what the request carries.
            case: "a connection whose header parameter is flagged secret",
            credential: credential(
                "POST",
                Some(oauth_http_parameters(
                    &[("x-oauth-secret", "sekrit-oauth", true)],
                    &[],
                    &[],
                )),
            ),
            method: "POST",
            uri: AUTHORIZATION_ENDPOINT,
            headers: vec![("x-oauth-secret", "sekrit-oauth")],
            form: vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)],
        },
        GrantRow {
            case: "a connection carrying a parameter of every kind",
            credential: credential(
                "POST",
                Some(oauth_http_parameters(
                    &[("x-oauth-h", "oh", false)],
                    &[("aq", "a1")],
                    &[("scope", "orders/write")],
                )),
            ),
            method: "POST",
            uri: "https://authorization.example.com/oauth/token?aq=a1",
            headers: vec![("x-oauth-h", "oh")],
            form: vec![
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("scope", "orders/write"),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let endpoint = Endpoint::new(Answer::Http(200, token_response("tok-1")));
        let answered =
            state::oauth::client_credentials_token(&endpoint.transport(), &row.credential).await;
        assert_eq!(
            answered.as_deref(),
            Ok("tok-1"),
            "case {case:?}: expected the token the endpoint issued, got {answered:?}"
        );

        let received = endpoint.received();
        assert_eq!(
            received.len(),
            1,
            "case {case:?}: expected exactly one token request, got {received:?}"
        );
        let request = &received[0];
        assert_eq!(
            request.method, row.method,
            "case {case:?}: expected the connection's own method, got {request:?}"
        );
        assert_eq!(
            request.uri, row.uri,
            "case {case:?}: expected the authorization endpoint and its query, got {request:?}"
        );
        assert_eq!(
            request.header("content-type"),
            Some(FORM_CONTENT_TYPE),
            "case {case:?}: the grant is a form body, got {request:?}"
        );
        for (name, value) in &row.headers {
            assert_eq!(
                request.header(name),
                Some(*value),
                "case {case:?}: expected the header {name}: {value}, got {request:?}"
            );
        }
        let form: Vec<(String, String)> = row
            .form
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            request.form(),
            form,
            "case {case:?}: expected exactly the grant's fields {form:?}, got {:?}",
            request.body
        );
    }
}

/// One row of the outcome table: the case, what the authorization endpoint
/// answers, and what the exchange comes to — the token, or the `StateReason` the
/// connection then reports.
struct OutcomeRow {
    case: &'static str,
    answer: Answer,
    outcome: Result<&'static str, String>,
}

/// What the exchange comes to follows from what the endpoint answered, over a
/// table of answers.
#[tokio::test]
async fn the_exchange_holds_a_token_only_where_the_endpoint_issued_one() {
    let rows = vec![
        OutcomeRow {
            case: "a token response",
            answer: Answer::Http(200, token_response("tok-9")),
            outcome: Ok("tok-9"),
        },
        OutcomeRow {
            case: "a 2xx carrying no access token",
            answer: Answer::Http(200, json!({ "token_type": "Bearer" }).to_string()),
            outcome: Err(state::oauth::NO_TOKEN_ISSUED.to_string()),
        },
        OutcomeRow {
            case: "a 2xx carrying an empty access token",
            answer: Answer::Http(200, json!({ "access_token": "" }).to_string()),
            outcome: Err(state::oauth::NO_TOKEN_ISSUED.to_string()),
        },
        OutcomeRow {
            case: "a 2xx whose body is not a token response at all",
            answer: Answer::Http(200, "not a token response".to_string()),
            outcome: Err(state::oauth::NO_TOKEN_ISSUED.to_string()),
        },
        OutcomeRow {
            case: "the endpoint refusing the credentials",
            answer: Answer::Http(401, json!({ "error": "invalid_client" }).to_string()),
            outcome: Err(
                "The authorization endpoint answered HTTP 401 instead of issuing a token."
                    .to_string(),
            ),
        },
        OutcomeRow {
            case: "the endpoint faulting",
            answer: Answer::Http(500, String::new()),
            outcome: Err(
                "The authorization endpoint answered HTTP 500 instead of issuing a token."
                    .to_string(),
            ),
        },
        OutcomeRow {
            case: "an endpoint nothing answers at",
            answer: Answer::Unreachable,
            outcome: Err(state::ENDPOINT_NOT_REACHED.to_string()),
        },
    ];

    for row in rows {
        let case = row.case;
        let endpoint = Endpoint::new(row.answer.clone());
        let answered =
            state::oauth::client_credentials_token(&endpoint.transport(), &credential("POST", None))
                .await;
        let expected: Result<String, String> = match &row.outcome {
            Ok(token) => Ok((*token).to_string()),
            Err(reason) => Err(reason.clone()),
        };
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {expected:?} from the exchange, got {answered:?}"
        );
    }

    // A connection holding no credential at all — one `DeauthorizeConnection`
    // stripped — has nothing to obtain a token from, and says so.
    let endpoint = Endpoint::new(Answer::Http(200, token_response("tok-0")));
    let stripped = state::authorization_exchange(&endpoint.transport(), None).await;
    assert_eq!(
        stripped,
        Err(state::NO_AUTHORIZATION_ENDPOINT.to_string()),
        "a connection holding no authorization parameters obtains no token, got {stripped:?}"
    );
    assert!(
        endpoint.received().is_empty(),
        "no request is made for a connection holding no credential, got {:?}",
        endpoint.received()
    );
}

/// The emulator: one router carrying this service over one shared engine, and the
/// engine itself, because what an invocation carries is what the STORE holds.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
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
    let response = emulator.router.dispatch(request).await;
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

/// The credential half the store holds for the connection `name`.
fn stored_secret(emulator: &Emulator, name: &str) -> ConnectionSecret {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .connection(name)
        .unwrap_or_else(|| panic!("the connection {name} is in the store"))
        .secret
        .clone()
}

/// One row of the bearer table: the case, what the exchange came to, the state
/// the connection then settles in, its destination's state, and the
/// `Authorization` header an invocation through it carries.
struct BearerRow {
    case: &'static str,
    exchanged: Result<String, String>,
    settled_state: &'static str,
    destination_state: &'static str,
    authorization: Option<&'static str>,
}

/// A connection whose exchange ISSUED a token reaches `AUTHORIZED`, its
/// destination becomes `ACTIVE`, and the invocation it authorizes carries the
/// token as a bearer credential — while one whose exchange issued none stays
/// `DEAUTHORIZED` with its destination `INACTIVE` and authorizes nothing.
#[tokio::test]
async fn an_authorized_connection_sends_its_token_as_the_bearer_credential() {
    let rows = [
        BearerRow {
            case: "an exchange that issued a token",
            exchanged: Ok("tok-42".to_string()),
            settled_state: state::CONNECTION_STATE_AUTHORIZED,
            destination_state: state::API_DESTINATION_STATE_ACTIVE,
            authorization: Some("Bearer tok-42"),
        },
        BearerRow {
            case: "an exchange that issued none",
            exchanged: Err(state::ENDPOINT_NOT_REACHED.to_string()),
            settled_state: state::CONNECTION_STATE_DEAUTHORIZED,
            destination_state: state::API_DESTINATION_STATE_INACTIVE,
            authorization: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let BearerRow {
            exchanged,
            settled_state,
            destination_state,
            authorization,
            ..
        } = row;
        let emulator = emulator();
        let name = "oauth-connection";
        let (status, created) = events(
            &emulator,
            "CreateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "OAUTH_CLIENT_CREDENTIALS",
                "AuthParameters": {
                    "OAuthParameters": {
                        "ClientParameters": {
                            "ClientID": CLIENT_ID,
                            "ClientSecret": CLIENT_SECRET,
                        },
                        "AuthorizationEndpoint": AUTHORIZATION_ENDPOINT,
                        "HttpMethod": "POST",
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection answered {status} {created}"
        );
        let connection_arn = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {created}"))
            .to_string();
        let (status, destination) = events(
            &emulator,
            "CreateApiDestination",
            &json!({
                "Name": "oauth-destination",
                "ConnectionArn": connection_arn,
                "InvocationEndpoint": "https://destination.example.com/events",
                "HttpMethod": "POST",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApiDestination answered {status} {destination}"
        );

        // What the exchange came to, recorded the way the create records it once
        // the authorization endpoint has answered.
        let state = emulator
            .engine
            .state::<ComAmazonawsEventbridgeAwseventsState>();
        state::record_authorization(&state, name, exchanged.clone());
        // The transition the exchange owes the connection falls due at once, so a
        // read taken after it reports the settled state.
        tokio::time::sleep(state::CONNECTION_TRANSITION + std::time::Duration::from_millis(250))
            .await;

        let (_, read) = events(&emulator, "DescribeConnection", &json!({ "Name": name })).await;
        assert_eq!(
            read["ConnectionState"],
            json!(settled_state),
            "case {case:?}: expected the connection to settle {settled_state}, got {read}"
        );
        assert_eq!(
            read["StateReason"],
            exchanged
                .clone()
                .err()
                .map_or(Value::Null, |reason| json!(reason)),
            "case {case:?}: expected the state's own reason, got {read}"
        );
        if exchanged.is_ok() {
            assert!(
                read["LastAuthorizedTime"].is_number(),
                "case {case:?}: an authorized connection reports when it was, got {read}"
            );
        }
        let (_, destination) = events(
            &emulator,
            "DescribeApiDestination",
            &json!({ "Name": "oauth-destination" }),
        )
        .await;
        assert_eq!(
            destination["ApiDestinationState"],
            json!(destination_state),
            "case {case:?}: expected the destination to be {destination_state}, got {destination}"
        );

        // What an invocation through the connection CARRIES, built from the
        // credential half the store holds for it.
        let secret = stored_secret(&emulator, name);
        let invocation = api_destination_invocation(
            "https://destination.example.com/events",
            &secret,
            None,
            &json!({ "detail": { "order": "A-1" } }).to_string(),
            "application/json; charset=utf-8",
            "Amazon/EventBridge/ApiDestinations",
        );
        let sent = invocation
            .headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone());
        assert_eq!(
            sent.as_deref(),
            authorization,
            "case {case:?}: expected the Authorization header {authorization:?}, got \
             {:?}",
            invocation.headers
        );
        // The client secret is never sent to the destination, and neither is the
        // token reported by a read of the connection.
        let rendered = format!("{invocation:?}");
        assert!(
            !rendered.contains(CLIENT_SECRET),
            "case {case:?}: the invocation must carry no client secret, got {rendered}"
        );
        let (_, listed) = events(&emulator, "ListConnections", &json!({})).await;
        let reads = format!("{read}{listed}");
        for withheld in [CLIENT_SECRET, "tok-42"] {
            assert!(
                !reads.contains(withheld),
                "case {case:?}: no read may report {withheld:?}, got {reads}"
            );
        }
    }
}
