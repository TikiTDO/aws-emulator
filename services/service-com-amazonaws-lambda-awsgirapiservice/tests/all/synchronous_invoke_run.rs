// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SYNCHRONOUS invoke data plane: `Invoke` with
//! `InvocationType=RequestResponse` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) and `InvokeWithResponseStream`.
//!
//! The rule under test: a synchronous invocation RUNS the function, and the answer is what
//! the run produced. The handler of the deployment package the store holds for the resolved
//! version is called with the event and the context; its returned value is the response
//! payload, an exception it raised is still an HTTP 200 carrying the error document and the
//! `X-Amz-Function-Error: Unhandled` signal, `X-Amz-Executed-Version` names the version the
//! qualifier resolved to, and `X-Amz-Log-Result` carries the base64 log tail when
//! `LogType=Tail` asked for one. `InvokeWithResponseStream` answers the same run as an
//! `application/vnd.amazon.eventstream` body: the response as a `PayloadChunk`, then the
//! terminating `InvokeComplete`.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: what the handler computes (a literal, a value computed from
//! the event, one read from the function's environment, a value from the standard library),
//! how it ends (a return, a raise), which qualifier the invocation names (the unpublished
//! version, a published version, an alias), whether a log tail was asked for, and what the
//! function's runtime is.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

/// The module the handler of every package below lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn error_code(&self) -> String {
        self.header("x-amzn-errortype")
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    fn message(&self) -> String {
        let document = self.json();
        document
            .get("Message")
            .or_else(|| document.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
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

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
///
/// Shared with the suites that upload a package whose handler RUNS, so there is one
/// writer of a deployment archive in this target.
pub(crate) fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a function whose code is `source`, publish version 1, point the alias `live` at
/// it, and wait out the provisioning window the create documents.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    runtime: &str,
    variables: Value,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": HANDLER,
            "Publish": true,
            "Timeout": 5,
            "Environment": { "Variables": variables },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    let alias = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
            .expect("the alias body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias on {name:?} must succeed, got {} {}",
        alias.status,
        alias.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` on the function `reference` names.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    payload: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        payload,
        headers,
    )
    .await
}

/// The handler's own value is the response payload — one row per kind of value a handler
/// computes, so the answer is the RUN's and not a shape this emulator fills in.
#[tokio::test]
async fn the_response_payload_is_what_the_handler_returned() {
    let cases: &[(&str, &str, Value, Value)] = &[
        (
            "a literal document",
            "def lambda_handler(event, context):\n    return {'ok': True}\n",
            json!({}),
            json!({"ok": true}),
        ),
        (
            "a value computed from the event",
            "def lambda_handler(event, context):\n    return {'doubled': event['n'] * 2}\n",
            json!({"n": 21}),
            json!({"doubled": 42}),
        ),
        (
            "a value read from the environment",
            "import os\n\
             def lambda_handler(event, context):\n    return {'env': os.environ['SECRET']}\n",
            json!({}),
            json!({"env": "shhh"}),
        ),
        (
            "a value from the standard library",
            "import base64\n\
             def lambda_handler(event, context):\n    \
                 return {'encoded': base64.b64encode(b'lambda').decode()}\n",
            json!({}),
            json!({"encoded": "bGFtYmRh"}),
        ),
        (
            "the context the runtime reports",
            "def lambda_handler(event, context):\n    \
                 return {'name': context.function_name, 'version': context.function_version}\n",
            json!({}),
            json!({"name": "probe-run", "version": "$LATEST"}),
        ),
        (
            "a list",
            "def lambda_handler(event, context):\n    return sorted(event['items'])\n",
            json!({"items": [3, 1, 2]}),
            json!([1, 2, 3]),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, source, event, expected) in cases {
        // One function per row, created and removed in place: `UpdateFunctionCode` is a
        // different operation's rule, so each row gets its own function.
        let unique = "probe-run";
        create_function(
            &router,
            unique,
            source,
            "python3.12",
            json!({"SECRET": "shhh"}),
        )
        .await;
        let answered = invoke(
            &router,
            unique,
            &serde_json::to_vec(event).expect("the event serializes"),
            &[],
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json(),
            *expected,
            "case {case}: expected {expected}, got {}",
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-executed-version").as_deref(),
            Some("$LATEST"),
            "case {case}: expected the executed version, got {:?}",
            answered.header("x-amz-executed-version")
        );
        assert_eq!(
            answered.header("x-amz-function-error"),
            None,
            "case {case}: a handler that returned reports no function error"
        );
        // The next row needs the name free again.
        let deleted = call(
            &router,
            "DELETE",
            &format!("{FUNCTIONS_URI}/{unique}"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}

/// An exception the handler raised is still an HTTP 200, carrying the error document and
/// the `Unhandled` signal — one row per way a handler fails, and per phase of the run the
/// failure was raised in: a failure the invocation was bound to names that invocation,
/// while one raised while the handler was still being resolved names no invocation at all.
#[tokio::test]
async fn an_unhandled_handler_error_is_a_two_hundred_with_the_error_document() {
    // The case, the module source, the error class, the words of the message, and whether
    // an invocation was bound to the run when it failed.
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "a raised exception",
            "def lambda_handler(event, context):\n    raise ValueError('boom')\n",
            "ValueError",
            "boom",
            true,
        ),
        (
            "a failed computation",
            "def lambda_handler(event, context):\n    return 1 / 0\n",
            "ZeroDivisionError",
            "division by zero",
            true,
        ),
        (
            "a handler the module does not define",
            "def other(event, context):\n    return 1\n",
            "Runtime.HandlerNotFound",
            "lambda_handler",
            false,
        ),
        (
            "a module that does not import",
            "import no_such_module_at_all\n",
            "Runtime.ImportModuleError",
            "Unable to import module",
            false,
        ),
        (
            "a value that cannot be marshalled",
            "def lambda_handler(event, context):\n    return {'s': set()}\n",
            "Runtime.MarshalError",
            "Unable to marshal response",
            true,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, source, error_type, expected, bound) in cases {
        let name = "probe-raise";
        create_function(&router, name, source, "python3.12", json!({})).await;
        let answered = invoke(&router, name, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: an unhandled error is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case}: expected the Unhandled signal, got {:?}",
            answered.header("x-amz-function-error")
        );
        let document = answered.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(*error_type),
            "case {case}: expected the error type {error_type}, got {document}"
        );
        let message = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(expected),
            "case {case}: expected {expected:?} in the message, got {document}"
        );
        assert!(
            document
                .get("requestId")
                .and_then(Value::as_str)
                .is_some_and(|id| if *bound { id.len() == 36 } else { id.is_empty() }),
            "case {case}: expected {}, got {document}",
            if *bound {
                "the invocation's request id"
            } else {
                "no request id, for a failure raised before one was bound"
            }
        );
        assert!(
            document.get("stackTrace").is_some_and(Value::is_array),
            "case {case}: expected the handler's stack, got {document}"
        );
        let deleted = call(&router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}

/// The log tail is reported ONLY when `LogType=Tail` asked for it, and what it carries is
/// the log the run wrote — one row per `LogType` value the model declares.
#[tokio::test]
async fn the_log_tail_is_reported_only_when_it_is_asked_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "probe-log",
        "def lambda_handler(event, context):\n    \
             print('handler says hello')\n    \
             return {'ok': True}\n",
        "python3.12",
        json!({}),
    )
    .await;
    let cases: &[(&str, Option<&str>, bool)] = &[
        ("no LogType", None, false),
        ("LogType=None", Some("None"), false),
        ("LogType=Tail", Some("Tail"), true),
    ];
    for (case, log_type, reported) in cases {
        let headers: Vec<(&str, &str)> = log_type
            .map(|value| vec![("x-amz-log-type", value)])
            .unwrap_or_default();
        let answered = invoke(&router, "probe-log", b"{}", &headers).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        let tail = answered.header("x-amz-log-result");
        assert_eq!(
            tail.is_some(),
            *reported,
            "case {case}: expected a log tail {reported}, got {tail:?}"
        );
        if let Some(tail) = tail {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(tail)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                .unwrap_or_else(|error| panic!("case {case}: the tail is base64: {error}"));
            for expected in ["START RequestId:", "handler says hello", "END RequestId:", "REPORT"]
            {
                assert!(
                    decoded.contains(expected),
                    "case {case}: expected {expected:?} in the log, got {decoded:?}"
                );
            }
        }
    }
}

/// `X-Amz-Executed-Version` names the version the QUALIFIER resolved to — one row per
/// qualifier form, so an alias reports the version it points at rather than its own name.
#[tokio::test]
async fn the_executed_version_is_the_version_the_qualifier_resolved_to() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "probe-version",
        "def lambda_handler(event, context):\n    return {'ok': True}\n",
        "python3.12",
        json!({}),
    )
    .await;
    let cases: &[(&str, &str, &str)] = &[
        ("the unqualified name", "probe-version", "$LATEST"),
        ("the unpublished version", "probe-version:$LATEST", "$LATEST"),
        ("a published version", "probe-version:1", "1"),
        ("an alias", "probe-version:live", "1"),
    ];
    for (case, reference, expected) in cases {
        let answered = invoke(&router, reference, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-executed-version").as_deref(),
            Some(*expected),
            "case {case}: expected the executed version {expected}, got {:?}",
            answered.header("x-amz-executed-version")
        );
    }
}

/// An invocation that resolves to nothing names the QUALIFIED resource it addressed, and
/// the class of its fault — one row per way the pair fails to resolve.
#[tokio::test]
async fn an_invocation_that_resolves_to_nothing_names_the_qualified_resource() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "probe-missing",
        "def lambda_handler(event, context):\n    return {'ok': True}\n",
        "python3.12",
        json!({}),
    )
    .await;
    // The third column is the whole expected message: a pair that resolved an ARN is
    // refused naming that ARN, and a name of another Region resolves none — the Region is
    // settled first — so it is refused on the two Regions that disagree.
    let cases: &[(&str, &str, String)] = &[
        (
            "a function that does not exist",
            "absent-function",
            format!(
                "Function not found: arn:aws:lambda:us-east-1:{ACCOUNT}:function:\
                 absent-function:$LATEST"
            ),
        ),
        (
            "a function of another Region",
            "arn:aws:lambda:us-west-2:000000000000:function:probe-missing",
            "Functions from 'us-west-2' are not reachable in this region ('us-east-1')"
                .to_string(),
        ),
        (
            "a version that does not exist",
            "probe-missing:7",
            format!(
                "Function not found: arn:aws:lambda:us-east-1:{ACCOUNT}:function:probe-missing:7"
            ),
        ),
        (
            "an alias that does not exist",
            "probe-missing:staging",
            format!(
                "Function not found: arn:aws:lambda:us-east-1:{ACCOUNT}:function:probe-missing:\
                 staging"
            ),
        ),
    ];
    for (case, reference, expected) in cases {
        let answered = invoke(&router, reference, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            404,
            "case {case}: expected 404, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.message(),
            *expected,
            "case {case}: expected {expected:?}, got {}",
            answered.text()
        );
        assert_eq!(
            answered.json().get("Type").and_then(Value::as_str),
            Some("User"),
            "case {case}: expected the class of the fault, got {}",
            answered.text()
        );
    }
}

/// A payload past the quota of its invocation TYPE is refused with the sentence AWS gives
/// that type — one row per invocation type, at and over its own quota.
#[tokio::test]
async fn a_payload_past_its_invocation_types_quota_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "probe-quota",
        "def lambda_handler(event, context):\n    return {'ok': True}\n",
        "python3.12",
        json!({}),
    )
    .await;
    /// The asynchronous quota the model documents ("For asynchronous invocations, the
    /// maximum payload size is 1 MB").
    const ASYNCHRONOUS: usize = 1024 * 1024;
    let over = ASYNCHRONOUS + 1;
    // A JSON string of exactly `over` bytes, so the body is both over the quota and a
    // valid event.
    let payload = format!("\"{}\"", "x".repeat(over - 2));
    let answered = invoke(
        &router,
        "probe-quota",
        payload.as_bytes(),
        &[("x-amz-invocation-type", "Event")],
    )
    .await;
    assert_eq!(
        answered.status,
        413,
        "expected the 413 the model binds, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.error_code(),
        "RequestEntityTooLargeException",
        "expected the error code AWS answers, got {}",
        answered.error_code()
    );
    let expected = format!(
        "{over} byte payload is too large for the Event invocation type (limit \
         {ASYNCHRONOUS} bytes)"
    );
    assert_eq!(
        answered.message(),
        expected,
        "expected {expected:?}, got {}",
        answered.text()
    );
    // The same payload is WITHIN the synchronous quota, so it runs.
    let served = invoke(&router, "probe-quota", payload.as_bytes(), &[]).await;
    assert_eq!(
        served.status,
        200,
        "the same payload is within the synchronous quota, got {} {}",
        served.status,
        served.text()
    );
}

/// `InvokeWithResponseStream` answers the same run as an event stream: the response as a
/// `PayloadChunk`, then the `InvokeComplete` that ends it — one row per way the handler
/// ends. The chunk's `Payload` is the event's `@eventPayload` blob, so the frame's own body
/// is exactly what the handler produced.
#[tokio::test]
async fn a_response_stream_invocation_streams_what_the_handler_produced() {
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "a handler that returned",
            "def lambda_handler(event, context):\n    return {'streamed': True}\n",
            "{\"streamed\": true}",
            false,
        ),
        (
            "a handler that raised",
            "def lambda_handler(event, context):\n    raise ValueError('mid-stream')\n",
            "mid-stream",
            true,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, source, expected, raised) in cases {
        let name = "probe-stream";
        create_function(&router, name, source, "python3.12", json!({})).await;
        let answered = call(
            &router,
            "POST",
            &format!("/2021-11-15/functions/{name}/response-streaming-invocations"),
            b"{}",
            &[],
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("content-type").as_deref(),
            Some("application/vnd.amazon.eventstream"),
            "case {case}: expected the event-stream content type, got {:?}",
            answered.header("content-type")
        );
        assert_eq!(
            answered.header("x-amz-executed-version").as_deref(),
            Some("$LATEST"),
            "case {case}: expected the executed version, got {:?}",
            answered.header("x-amz-executed-version")
        );
        let framed = answered.text();
        for marker in ["PayloadChunk", "InvokeComplete"] {
            assert!(
                framed.contains(marker),
                "case {case}: expected the {marker} event in the stream, got {framed:?}"
            );
        }
        assert!(
            framed.contains(expected),
            "case {case}: expected {expected:?} in the stream, got {framed:?}"
        );
        if *raised {
            assert!(
                framed.contains("Unhandled"),
                "case {case}: expected the completion to name the error, got {framed:?}"
            );
        }
        let deleted = call(&router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}

/// A package this emulator embeds no engine for is REFUSED, never answered with a payload
/// no function produced — one row per runtime class.
#[tokio::test]
async fn a_runtime_with_no_engine_is_refused_rather_than_answered() {
    let cases: &[(&str, &str)] = &[
        ("a Java runtime", "java21"),
        ("a custom runtime", "provided.al2023"),
        ("a .NET runtime", "dotnet8"),
        ("a Ruby runtime", "ruby3.3"),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, runtime) in cases {
        let name = "probe-runtime";
        create_function(
            &router,
            name,
            "def lambda_handler(event, context):\n    return {'ok': True}\n",
            runtime,
            json!({}),
        )
        .await;
        let answered = invoke(&router, name, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            502,
            "case {case}: expected the modelled 502, got {} {}",
            answered.status,
            answered.text()
        );
        assert!(
            answered.error_code().contains("InvalidRuntimeException"),
            "case {case}: expected InvalidRuntimeException, got {}",
            answered.error_code()
        );
        assert!(
            answered.message().contains(runtime),
            "case {case}: expected the refusal to name the runtime, got {}",
            answered.text()
        );
        let deleted = call(&router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}
