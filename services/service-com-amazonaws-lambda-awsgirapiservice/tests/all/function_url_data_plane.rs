// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FUNCTION URL DATA PLANE: a request to the endpoint
//! `CreateFunctionUrlConfig` hands out is served by the function the URL is attached to.
//!
//! The rule under test: a function URL endpoint INVOKES its function with the documented
//! payload-format-2.0 event and answers the response the handler's result describes — the
//! `statusCode`, `headers`, `cookies` and `body` it names, the `200 application/json`
//! Lambda infers for a result that names none, and the `502` it answers for every result
//! that is no response at all (a handler that raised, a status outside the HTTP range, a
//! body past the synchronous payload ceiling) — while `AuthType` decides which callers are
//! admitted: `NONE` serves a public endpoint and `AWS_IAM` serves only a request whose
//! SigV4 signature the emulator verified, reporting that caller in the event.
//!
//! Every row drives real HTTP requests through the router, against a function whose
//! deployment package RUNS, so a response this plane made up rather than read off the run
//! fails here. The rows differ along what the rule generalises over: what the handler
//! returns (a described response, a described response carrying cookies or a base64 body, a
//! document, a string, no body at all, a status outside the range, an exception, a body
//! past the ceiling), what the request carries (a path, a query, repeated headers, cookies,
//! a textual body, a form body, a binary body), which of the three addressings reaches the
//! endpoint, which qualifier the URL is attached to, and the `AuthType` the URL enforces.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a control-plane request here. Nothing in it is verified unless a
/// row turns request authentication on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The control-plane endpoint of this service, which the control-plane calls below address.
const SERVICE_HOST: &str = "lambda.us-east-1.amazonaws.com";

/// The emulator's own endpoint, which a client that resolves no AWS hostname reaches.
const OWN_ENDPOINT: &str = "127.0.0.1:9297";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    pub(crate) fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!("expected a JSON body, got {error}: {}", self.text())
        })
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// Every value of a header the answer may carry more than once.
    fn header_values(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_string)
            .collect()
    }

    /// The status and the body, for a failure message.
    pub(crate) fn summary(&self) -> String {
        format!("{} {}", self.status, self.text())
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
pub(crate) fn router_on(engine: &std::sync::Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(std::sync::Arc::clone(engine)));
    router
}

/// Dispatch one request to `host`, with the headers `headers` names.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One CONTROL-plane call, signed the way the harness signs them.
pub(crate) async fn control(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    send(
        router,
        method,
        SERVICE_HOST,
        uri,
        &payload,
        &[
            ("authorization", AUTHORIZATION),
            ("content-type", "application/json"),
        ],
    )
    .await
}

/// Create a function whose package RUNS `source`, and wait out the provisioning window.
pub(crate) async fn given_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let created = control(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 20,
            "MemorySize": 512,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {}",
        created.summary()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `CreateFunctionUrlConfig` on `function`, answering the endpoint it minted.
///
/// A `NONE` URL is also granted to the public, which is what makes such an endpoint public:
/// `AuthType=NONE` bypasses IAM authentication, and the function's resource-based policy is
/// what admits the anonymous caller. `given_url_without_grant` is the same create with no
/// grant, for the rows that assert the refusal.
async fn given_url(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
    auth_type: &str,
) -> Endpoint {
    let endpoint = given_url_without_grant(router, function, qualifier, auth_type).await;
    if auth_type == "NONE" {
        grant(
            router,
            function,
            qualifier,
            &json!({
                "StatementId": "public-url",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "*",
                "FunctionUrlAuthType": "NONE",
            }),
        )
        .await;
    }
    endpoint
}

/// `AddPermission` on the function `function` names, at `qualifier`.
pub(crate) async fn grant(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
    statement: &Value,
) {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let granted = control(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/policy{query}"),
        statement,
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "AddPermission on {function:?} must succeed, got {}",
        granted.summary()
    );
}

/// `CreateFunctionUrlConfig` alone, granting nothing and configuring no CORS.
async fn given_url_without_grant(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
    auth_type: &str,
) -> Endpoint {
    given_configured_url(router, function, qualifier, auth_type, &Value::Null).await
}

/// `CreateFunctionUrlConfig` alone, with the `Cors` settings `cors` names — absent for
/// `Value::Null`, which is a URL configured with none.
pub(crate) async fn given_configured_url(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
    auth_type: &str,
    cors: &Value,
) -> Endpoint {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let mut body = json!({ "AuthType": auth_type });
    if !cors.is_null() {
        body["Cors"] = cors.clone();
    }
    let created = control(
        router,
        "POST",
        &format!("/2021-10-31/functions/{function}/url{query}"),
        &body,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunctionUrlConfig on {function:?} must succeed, got {}",
        created.summary()
    );
    let url = created
        .json()
        .get("FunctionUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the create must answer a FunctionUrl, got {}", created.text()));
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .expect("the minted URL carries a host")
        .to_string();
    let url_id = host
        .split('.')
        .next()
        .expect("the host carries the url id")
        .to_string();
    Endpoint { host, url_id }
}

/// The endpoint one function URL publishes.
pub(crate) struct Endpoint {
    /// The host the URL is addressed at.
    pub(crate) host: String,
    /// The url id that host's first label carries.
    pub(crate) url_id: String,
}

/// One request to the function URL, addressed HOST-STYLE the way AWS publishes it.
async fn get(router: &MultiServiceRouter, endpoint: &Endpoint, path: &str) -> Answer {
    send(router, "GET", &endpoint.host, path, &[], &[]).await
}

/// The handler every row of the response-mapping suite runs: it answers from the event's
/// own `rawPath`, so one function serves every row and the answer is the RUN's.
fn responding_handler() -> String {
    r#"
import base64
import json


def lambda_handler(event, context):
    path = event["rawPath"]
    if path == "/described":
        return {
            "statusCode": 201,
            "headers": {"Content-Type": "text/plain", "X-Made": "yes"},
            "body": "made",
        }
    if path == "/cookies":
        return {"statusCode": 200, "cookies": ["a=1", "b=2; Path=/"], "body": "ok"}
    if path == "/accepted":
        return {"statusCode": 202}
    if path == "/binary":
        return {
            "statusCode": 200,
            "headers": {"content-type": "application/octet-stream"},
            "body": base64.b64encode(bytes([0, 1, 2, 255])).decode(),
            "isBase64Encoded": True,
        }
    if path == "/document":
        return {"ok": True}
    if path == "/string":
        return "plain"
    if path == "/list":
        return [1, 2]
    if path == "/bad-status":
        return {"statusCode": 999, "body": "never served"}
    if path == "/bad-header":
        return {
            "statusCode": 200,
            "headers": {"x-trace": "one\r\nx-injected: two"},
            "body": "never served",
        }
    if path == "/raise":
        raise ValueError("the handler could not answer")
    if path == "/sized":
        size = int(event["queryStringParameters"]["bytes"])
        return {"statusCode": 200, "body": "x" * size}
    if path.startswith("/echo"):
        return {
            "statusCode": 200,
            "headers": {"content-type": "application/json"},
            "body": json.dumps(
                {
                    "version": event["version"],
                    "rawPath": event["rawPath"],
                    "rawQueryString": event["rawQueryString"],
                    "headers": event["headers"],
                    "cookies": event.get("cookies"),
                    "queryStringParameters": event.get("queryStringParameters"),
                    "body": event.get("body"),
                    "isBase64Encoded": event["isBase64Encoded"],
                    "method": event["requestContext"]["http"]["method"],
                    "domainName": event["requestContext"]["domainName"],
                    "accountId": event["requestContext"]["accountId"],
                    "authorizer": event["requestContext"].get("authorizer"),
                }
            ),
        }
    return {"statusCode": 404, "body": "no such path: " + path}
"#
    .to_string()
}

/// Every class of handler result maps onto the response it describes, and a result that
/// describes no response at all is the `502` Lambda answers for it.
#[tokio::test]
async fn a_function_url_answers_the_response_its_handler_described() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-responses", &responding_handler()).await;
    let endpoint = given_url(&router, "url-responses", None, "NONE").await;

    // (case, the path the handler answers from, the status, the body, the content type)
    let rows: &[(&str, &str, u16, &[u8], &str)] = &[
        (
            "a described response",
            "/described",
            201,
            b"made",
            "text/plain",
        ),
        (
            "a described response with no body and no content type",
            "/accepted",
            202,
            b"",
            "application/json",
        ),
        (
            "a described base64 body",
            "/binary",
            200,
            &[0, 1, 2, 255],
            "application/octet-stream",
        ),
        (
            "an inferred response from a document",
            "/document",
            200,
            br#"{"ok":true}"#,
            "application/json",
        ),
        (
            "an inferred response from a string, served as bare text",
            "/string",
            200,
            b"plain",
            "application/json",
        ),
        (
            "an inferred response from a list",
            "/list",
            200,
            b"[1,2]",
            "application/json",
        ),
        (
            "a status outside the HTTP range",
            "/bad-status",
            502,
            br#"{"message":"Internal Server Error"}"#,
            "application/json",
        ),
        (
            "a header value no response can carry",
            "/bad-header",
            502,
            br#"{"message":"Internal Server Error"}"#,
            "application/json",
        ),
        (
            "a handler that raised",
            "/raise",
            502,
            br#"{"message":"Internal Server Error"}"#,
            "application/json",
        ),
    ];
    for (case, path, status, body, content_type) in rows {
        let answer = get(&router, &endpoint, path).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.body.as_ref(),
            *body,
            "case {case:?}: expected the body {:?}, got {:?}",
            String::from_utf8_lossy(body),
            answer.text()
        );
        assert_eq!(
            answer.header("content-type").as_deref(),
            Some(*content_type),
            "case {case:?}: expected the content type {content_type:?}, got {:?}",
            answer.header("content-type")
        );
    }

    // Every cookie the result named is answered as its own `set-cookie` header.
    let cookies = get(&router, &endpoint, "/cookies").await;
    assert_eq!(
        cookies.header_values("set-cookie"),
        vec!["a=1".to_string(), "b=2; Path=/".to_string()],
        "case \"a described response carrying cookies\": expected one set-cookie per cookie, \
         got {:?}",
        cookies.header_values("set-cookie")
    );
}

/// The response quota bounds what the URL serves: a body under it is served and one past it
/// is the `502` class, at each side of the published bound.
#[tokio::test]
async fn a_response_is_served_up_to_the_synchronous_payload_ceiling() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-sizes", &responding_handler()).await;
    let endpoint = given_url(&router, "url-sizes", None, "NONE").await;

    // The quota on a handler's RESPONSE, which Lambda states in its own refusal for a
    // response past it (`Response payload size exceeded maximum allowed payload size (<n>
    // bytes).`) — the bound the run holds a result to, so a body under it is served and one
    // past it never leaves the run.
    const CEILING: usize = 6_291_556;
    // (case, the body size the handler answers, the status)
    let rows: &[(&str, usize, u16)] = &[
        ("a small body", 1024, 200),
        ("a one mebibyte body", 1024 * 1024, 200),
        ("a body just under the ceiling", CEILING - 4096, 200),
        ("a body one byte past the ceiling", CEILING + 1, 502),
        ("a body far past the ceiling", CEILING * 2, 502),
    ];
    for (case, size, status) in rows {
        let answer = get(&router, &endpoint, &format!("/sized?bytes={size}")).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status} for a {size}-byte body, got status {}",
            answer.status
        );
        if *status == 200 {
            assert_eq!(
                answer.body.len(),
                *size,
                "case {case:?}: expected {size} bytes, got {}",
                answer.body.len()
            );
        }
    }

    // The REQUEST half of the quota, which is the plane's own: "the maximum payload size is
    // 6 MB for synchronous invocations" (the model's own `Invoke` documentation) bounds the
    // event the request becomes, so a body past it is refused before the event is built.
    const REQUEST_CEILING: usize = 6 * 1024 * 1024;
    // (case, the body size the request carries, the status)
    let requests: &[(&str, usize, u16)] = &[
        ("a request body under the ceiling", 1024 * 1024, 200),
        ("a request body past the ceiling", REQUEST_CEILING + 1, 413),
    ];
    for (case, size, status) in requests {
        let answer = send(
            &router,
            "POST",
            &endpoint.host,
            "/echo",
            &vec![b'x'; *size],
            &[("content-type", "text/plain")],
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status} for a {size}-byte request, got status {}",
            answer.status
        );
    }
}

/// The event carries the request as payload format 2.0 states it, over a set of requests
/// that differ in what they carry.
#[tokio::test]
async fn the_event_carries_the_request_as_payload_format_two_states_it() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-events", &responding_handler()).await;
    let endpoint = given_url(&router, "url-events", None, "NONE").await;

    let echoed = send(
        &router,
        "GET",
        &endpoint.host,
        "/echo/one?q=a%20b&q=c&flag",
        &[],
        &[("x-trace", "one"), ("cookie", "a=1; b=2")],
    )
    .await;
    assert_eq!(
        echoed.status,
        200,
        "case \"an echoed GET\": the echo must be served, got {}",
        echoed.summary()
    );
    let event = echoed.json();
    // (case, the member the event carries, the value it must carry)
    let members: &[(&str, &str, Value)] = &[
        ("the format version", "version", json!("2.0")),
        ("the raw path", "rawPath", json!("/echo/one")),
        (
            "the raw query string",
            "rawQueryString",
            json!("q=a%20b&q=c&flag"),
        ),
        ("the cookies array", "cookies", json!(["a=1", "b=2"])),
        (
            "the decoded query parameters",
            "queryStringParameters",
            json!({"q": "a b,c", "flag": ""}),
        ),
        ("the method", "method", json!("GET")),
        ("the account", "accountId", json!(ACCOUNT)),
        ("the published host", "domainName", json!(endpoint.host)),
        (
            "no authorizer on a public endpoint",
            "authorizer",
            Value::Null,
        ),
        ("a request with no body", "isBase64Encoded", json!(false)),
    ];
    for (case, member, expected) in members {
        assert_eq!(
            event.get(*member).unwrap_or(&Value::Null),
            expected,
            "case {case:?}: expected {member} to be {expected}, got {:?}",
            event.get(*member)
        );
    }
    let headers = event
        .get("headers")
        .and_then(Value::as_object)
        .expect("the event carries headers");
    assert_eq!(
        headers.get("x-trace"),
        Some(&json!("one")),
        "case \"a header the request carried\": expected it lower-cased, got {headers:?}"
    );
    assert_eq!(
        headers.get("cookie"),
        Some(&json!("a=1; b=2")),
        "case \"the cookie header\": the event reports the request's headers whole, so the \
         `Cookie` header is among them beside the cookies array, got {headers:?}"
    );

    // The body travels verbatim under a textual media type and base64 under every other
    // one. The rows span what the rule generalises over: text, JSON, a form body, and
    // bytes no text can carry.
    let binary: Vec<u8> = vec![0, 1, 2, 250, 255];
    let encoded_binary = base64::engine::general_purpose::STANDARD.encode(&binary);
    let encoded_form = base64::engine::general_purpose::STANDARD.encode(b"a=1&b=2");
    // (case, the content type, the body sent, the body the event carries, isBase64Encoded)
    let bodies: &[(&str, &str, &[u8], &str, bool)] = &[
        ("a text body", "text/plain", b"hello", "hello", false),
        (
            "a JSON body",
            "application/json",
            br#"{"a":1}"#,
            r#"{"a":1}"#,
            false,
        ),
        (
            "a form body",
            "application/x-www-form-urlencoded",
            b"a=1&b=2",
            encoded_form.as_str(),
            true,
        ),
        (
            "a binary body",
            "application/octet-stream",
            &binary,
            encoded_binary.as_str(),
            true,
        ),
    ];
    for (case, content_type, sent, carried, base64) in bodies {
        let answer = send(
            &router,
            "POST",
            &endpoint.host,
            "/echo",
            sent,
            &[("content-type", content_type)],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the POST must be served, got {}",
            answer.summary()
        );
        let event = answer.json();
        assert_eq!(
            event.get("body").and_then(Value::as_str),
            Some(*carried),
            "case {case:?}: expected the body {carried:?}, got {:?}",
            event.get("body")
        );
        assert_eq!(
            event.get("isBase64Encoded"),
            Some(&json!(base64)),
            "case {case:?}: expected isBase64Encoded {base64}, got {:?}",
            event.get("isBase64Encoded")
        );
        assert_eq!(
            event.get("method").and_then(Value::as_str),
            Some("POST"),
            "case {case:?}: expected the method POST, got {:?}",
            event.get("method")
        );
    }
}

/// The endpoint is reached by each of the three addressings a client can write it as, and
/// every one of them hands the handler the same path.
#[tokio::test]
async fn the_endpoint_is_reached_by_every_addressing_a_client_can_write() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-addressing", &responding_handler()).await;
    let endpoint = given_url(&router, "url-addressing", None, "NONE").await;

    let local = format!("/_aws/lambda-url/{}/echo", endpoint.url_id);
    let local_host = format!("/_aws/{}/echo", endpoint.host);
    let path_style = format!("/{}/echo", endpoint.url_id);
    // (case, the host the request is sent to, the path it carries)
    let rows: &[(&str, &str, &str)] = &[
        ("host-style, as AWS publishes it", &endpoint.host, "/echo"),
        (
            "the endpoint label in the path, on the emulator's own endpoint",
            OWN_ENDPOINT,
            &local,
        ),
        (
            "the endpoint host in the path, on the emulator's own endpoint",
            OWN_ENDPOINT,
            &local_host,
        ),
        (
            "the url id as the first path segment, on this service's endpoint",
            SERVICE_HOST,
            &path_style,
        ),
        (
            "the url id as the first path segment, on the emulator's own endpoint",
            OWN_ENDPOINT,
            &path_style,
        ),
    ];
    for (case, host, path) in rows {
        let answer = send(&router, "GET", host, path, &[], &[]).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the request must be served, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.json().get("rawPath").and_then(Value::as_str),
            Some("/echo"),
            "case {case:?}: expected the path within the URL, got {}",
            answer.text()
        );
    }

    // The control plane keeps its own path space on every one of those hosts.
    let listed = control(&router, "GET", FUNCTIONS_URI, &Value::Null).await;
    assert_eq!(
        listed.status,
        200,
        "case \"the control plane beside the data plane\": ListFunctions must still be served, \
         got {}",
        listed.summary()
    );
}

/// A URL attached to an ALIAS invokes the version that alias names, and follows the alias
/// when it is pointed at another version.
#[tokio::test]
async fn a_url_on_an_alias_follows_the_version_the_alias_names() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let versioned = |answer: &str| {
        format!(
            "def lambda_handler(event, context):\n    return {{'version': '{answer}'}}\n"
        )
    };
    given_function(&router, "url-alias", &versioned("one")).await;
    let first = control(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/url-alias/versions"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        first.status,
        201,
        "PublishVersion must succeed, got {}",
        first.summary()
    );
    let alias = control(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/url-alias/aliases"),
        &json!({"Name": "live", "FunctionVersion": "1"}),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias must succeed, got {}",
        alias.summary()
    );
    let endpoint = given_url(&router, "url-alias", Some("live"), "NONE").await;

    let served = get(&router, &endpoint, "/").await;
    assert_eq!(
        (served.status, served.text()),
        (200, r#"{"version":"one"}"#.to_string()),
        "case \"the version the alias names\": expected the first version's answer, got {}",
        served.summary()
    );

    // A second version, and the alias pointed at it: the URL follows the alias.
    let updated = control(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/url-alias/code"),
        &json!({
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(&versioned("two"))),
            "Publish": true,
        }),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "UpdateFunctionCode must succeed, got {}",
        updated.summary()
    );
    let repointed = control(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/url-alias/aliases/live"),
        &json!({"FunctionVersion": "2"}),
    )
    .await;
    assert_eq!(
        repointed.status,
        200,
        "UpdateAlias must succeed, got {}",
        repointed.summary()
    );
    let after = get(&router, &endpoint, "/").await;
    assert_eq!(
        (after.status, after.text()),
        (200, r#"{"version":"two"}"#.to_string()),
        "case \"the alias repointed\": expected the second version's answer, got {}",
        after.summary()
    );
}

/// Deleting the configuration stops the URL serving: the endpoint no longer exists, so
/// nothing of the function is answered at it.
#[tokio::test]
async fn deleting_the_configuration_stops_the_url_serving() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-deleted", &responding_handler()).await;
    let endpoint = given_url(&router, "url-deleted", None, "NONE").await;

    let served = get(&router, &endpoint, "/document").await;
    assert_eq!(
        served.status,
        200,
        "case \"the positive control\": the URL must serve first, got {}",
        served.summary()
    );
    let deleted = control(
        &router,
        "DELETE",
        "/2021-10-31/functions/url-deleted/url",
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteFunctionUrlConfig must succeed, got {}",
        deleted.summary()
    );
    let after = get(&router, &endpoint, "/document").await;
    assert_ne!(
        after.status, 200,
        "case \"after the delete\": the endpoint must serve nothing, got {}",
        after.summary()
    );
}

/// `AuthType` decides which callers are admitted: a public endpoint serves a request
/// carrying no credential at all, and an `AWS_IAM` endpoint serves only a request whose
/// signature the emulator verified — one scoped to another service, one carrying no
/// signature and one whose signature does not match are each refused `403`.
#[tokio::test]
async fn aws_iam_urls_admit_only_callers_the_emulator_verified() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-open", &responding_handler()).await;
    given_function(&router, "url-restricted", &responding_handler()).await;
    let open = given_url(&router, "url-open", None, "NONE").await;
    let restricted = given_url(&router, "url-restricted", None, "AWS_IAM").await;

    // With authentication off, the harness reaches both: the `AWS_IAM` endpoint reports
    // the caller its credential speaks for, which a public endpoint reports for nobody.
    let admitted = send(
        &router,
        "GET",
        &restricted.host,
        "/echo",
        &[],
        &[("authorization", AUTHORIZATION)],
    )
    .await;
    assert_eq!(
        admitted.status,
        200,
        "case \"a signed request to an AWS_IAM endpoint\": expected the handler's answer, got {}",
        admitted.summary()
    );
    let iam = admitted
        .json()
        .get("authorizer")
        .and_then(|authorizer| authorizer.get("iam").cloned())
        .unwrap_or(Value::Null);
    // (the member the identity carries, the value it must carry)
    let identity: &[(&str, Value)] = &[
        ("accessKey", json!("emulator-test-key")),
        ("accountId", json!(ACCOUNT)),
        (
            "userArn",
            json!(format!("arn:aws:iam::{ACCOUNT}:user/emulator")),
        ),
    ];
    for (member, expected) in identity {
        assert_eq!(
            iam.get(*member),
            Some(expected),
            "case \"the caller identity\": expected {member} to be {expected}, got {iam}"
        );
    }

    // From here on the SAME emulator verifies request signatures, as it does when it is
    // started with request authentication on.
    let mut authenticating = router_on(&engine);
    authenticating.set_request_authentication(true);

    let scoped_elsewhere = "AWS4-HMAC-SHA256 \
         Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
        .to_string();
    let scoped_here = "AWS4-HMAC-SHA256 \
         Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
        .to_string();
    // One row: how the request is credentialled, which endpoint it addresses, and the
    // status that earns it.
    struct AuthRow<'r> {
        case: &'r str,
        endpoint: &'r Endpoint,
        headers: &'r [(&'r str, &'r str)],
        status: u16,
    }
    let signed_elsewhere = [
        ("authorization", scoped_elsewhere.as_str()),
        ("x-amz-date", "20260101T000000Z"),
    ];
    let mismatched = [
        ("authorization", scoped_here.as_str()),
        ("x-amz-date", "20260101T000000Z"),
    ];
    let rows: Vec<AuthRow<'_>> = vec![
        AuthRow {
            case: "a public endpoint, carrying no credential at all",
            endpoint: &open,
            headers: &[],
            status: 200,
        },
        AuthRow {
            case: "an AWS_IAM endpoint, carrying no credential at all",
            endpoint: &restricted,
            headers: &[],
            status: 403,
        },
        AuthRow {
            case: "an AWS_IAM endpoint, signed for another service",
            endpoint: &restricted,
            headers: &signed_elsewhere,
            status: 403,
        },
        AuthRow {
            case: "an AWS_IAM endpoint, with a signature that does not match",
            endpoint: &restricted,
            headers: &mismatched,
            status: 403,
        },
    ];
    for row in &rows {
        let answer = send(
            &authenticating,
            "GET",
            &row.endpoint.host,
            "/document",
            &[],
            row.headers,
        )
        .await;
        assert_eq!(
            answer.status,
            row.status,
            "case {:?}: expected {}, got {}",
            row.case,
            row.status,
            answer.summary()
        );
    }
}

/// A `NONE` endpoint is public only while the function's resource-based policy grants the
/// invocation to EVERYONE: a grant to a named principal, and no grant at all, admit no
/// anonymous caller.
#[tokio::test]
async fn a_none_endpoint_admits_the_public_only_while_the_policy_grants_it() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // (case, the function, the statement its policy carries, the status an anonymous
    // request is answered)
    let rows: &[(&str, &str, Option<Value>, u16)] = &[
        (
            "no grant at all",
            "url-ungranted",
            None,
            403,
        ),
        (
            "a grant to a named principal",
            "url-named-principal",
            Some(json!({
                "StatementId": "named",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": format!("arn:aws:iam::{ACCOUNT}:user/someone"),
                "FunctionUrlAuthType": "NONE",
            })),
            403,
        ),
        (
            "a grant to a service principal",
            "url-service-principal",
            Some(json!({
                "StatementId": "service",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "events.amazonaws.com",
                "FunctionUrlAuthType": "NONE",
            })),
            403,
        ),
        (
            "a grant to everyone",
            "url-public",
            Some(json!({
                "StatementId": "public",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "*",
                "FunctionUrlAuthType": "NONE",
            })),
            200,
        ),
    ];
    for (case, function, statement, status) in rows {
        given_function(&router, function, &responding_handler()).await;
        let endpoint = given_url_without_grant(&router, function, None, "NONE").await;
        if let Some(statement) = statement {
            grant(&router, function, None, statement).await;
        }
        let answer = get(&router, &endpoint, "/document").await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {}",
            answer.summary()
        );
        if *status == 403 {
            assert_eq!(
                answer.json().get("Message").and_then(Value::as_str),
                Some("Forbidden"),
                "case {case:?}: expected the endpoint's refusal sentence, got {}",
                answer.text()
            );
        }
    }
}
