// SPDX-License-Identifier: CC0-1.0

//! The error CODE a running handler's own AWS SDK call reads back for a refusal, end to end
//! over the wire: Lambda, Amazon SQS and AWS Secrets Manager on ONE engine, and a
//! `python3.x` function whose code does `import boto3` and reports what the refusal it
//! caught named itself.
//!
//! The rule pinned here: an SDK client names a refusal by the code the called service puts
//! ON THE WIRE for it. A service whose model carries the `awsQueryCompatible` protocol
//! modifier kept the error vocabulary of the query protocol it moved off, so its answer
//! carries a LEGACY code beside the modelled shape and every client of such a service asks
//! for that vocabulary and reports that code; a service without the modifier has only the
//! shape, which is what its client reports. The modelled shape's own identifier is never
//! what a client reports for a query-compatible service.
//!
//! The rows differ along what the rule generalises over: whether the called service is
//! query-compatible at all; whether the model states a legacy code for the class and
//! whether that code is spelled with the service's `AWS.<service>.` prefix, is a different
//! unprefixed word, or is absent (so the shape's own name stands); and which operation of
//! which service is refused — a write, a create, a batch write and two reads.
//!
//! Every row is held to what the SAME request answers on the wire, so the expectation is
//! the service's own answer rather than anything this suite renders: the literal each row
//! carries is the `awsQueryError` code the called service's model states for the class
//! (`smithy_query(query="shape", shape="<the error shape>")`), and it is asserted to be
//! both what the handler read and what the wire carried.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The header a client of a query-compatible service opts into the legacy error vocabulary
/// with, and the header such a service answers that vocabulary's code in — as
/// `<code>;<fault>`.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The header an AWS answer names the error SHAPE of a refusal in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// How one called service is reached on the wire, and whether its model carries the
/// `awsQueryCompatible` modifier.
///
/// Both entries are read from that service's own model with
/// `smithy_query(query="service_info")`, so this suite states no wire detail of its own.
struct Wire {
    /// Its SigV4 signing name, which is what routes the request inside the emulator.
    signing_name: &'static str,
    /// The leading label of its endpoint host.
    endpoint_prefix: &'static str,
    /// The `X-Amz-Target` prefix a client names an operation under.
    target_prefix: &'static str,
    /// The AWS JSON protocol version, which names the media type of the request body.
    json_version: &'static str,
    /// Whether `protocol_modifiers` carries `awsQueryCompatible`.
    query_compatible: bool,
}

/// Amazon SQS: awsJson1_0, sigv4Name `sqs`, endpointPrefix `sqs`, service shape
/// `AmazonSQS`, `protocol_modifiers: ["awsQueryCompatible"]`.
const SQS_WIRE: Wire = Wire {
    signing_name: "sqs",
    endpoint_prefix: "sqs",
    target_prefix: "AmazonSQS",
    json_version: "1.0",
    query_compatible: true,
};

/// AWS Secrets Manager: awsJson1_1, sigv4Name `secretsmanager`, endpointPrefix
/// `secretsmanager`, service shape `secretsmanager`, and NO protocol modifier — the
/// contrast row, where a refusal is known by its shape alone.
const SECRETS_WIRE: Wire = Wire {
    signing_name: "secretsmanager",
    endpoint_prefix: "secretsmanager",
    target_prefix: "secretsmanager",
    json_version: "1.1",
    query_compatible: false,
};

/// Lambda, Amazon SQS and AWS Secrets Manager over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the refusal it catches is the one this suite reads off the wire.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One AWS JSON call to `wire`'s service, asked for the way a client of it asks: a
/// query-compatible service is asked for the legacy error vocabulary, exactly as every SDK
/// client of one does.
///
/// Answers the status, the legacy code the answer named (`None` where it named none) and the
/// error shape it named, plus the parsed body.
async fn called(
    router: &MultiServiceRouter,
    wire: &Wire,
    operation: &str,
    body: &Value,
) -> (u16, Option<String>, Option<String>, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{}.{REGION}.amazonaws.com", wire.endpoint_prefix),
        )
        .header("authorization", authorization(wire.signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{}", wire.json_version),
        )
        .header(
            "x-amz-target",
            format!("{}.{operation}", wire.target_prefix),
        );
    if wire.query_compatible {
        request = request.header(QUERY_MODE_HEADER, "true");
    }
    let request = request
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    // `<code>;<fault>`: the code is the part a client reports.
    let legacy = header(QUERY_ERROR_HEADER).and_then(|carried| {
        carried
            .split(';')
            .next()
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(str::to_string)
    });
    // An error shape travels as `<shape>` or `<namespace>#<shape>`.
    let shape = header(ERROR_TYPE_HEADER)
        .or_else(|| {
            parsed
                .get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|named| {
            named
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .split(':')
                .next()
                .unwrap_or_default()
                .to_string()
        });
    (parts.status.as_u16(), legacy, shape, parsed)
}

/// One Amazon SQS call that must answer 200, for the setup each row needs.
async fn served(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, _legacy, _shape, parsed) = called(router, &SQS_WIRE, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
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

/// The handler every row runs: it makes the ONE call the event names and reports what the
/// refusal it caught named itself, so the invocation's payload IS what the SDK gave the
/// guest.
///
/// The class the event names is resolved from `client.exceptions` BEFORE the call, which is
/// how a handler written against the service catches one class of refusal: a modelled shape
/// whose reported code is the legacy one has to be caught by it all the same.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client(event["service"])
    expected = getattr(client.exceptions, event["shape"])
    method = getattr(client, event["method"])
    try:
        answered = method(**event["parameters"])
    except expected as error:
        return _refusal(error, True)
    except botocore.exceptions.ClientError as error:
        return _refusal(error, False)
    return {"ok": True, "answered": sorted(answered)}


def _refusal(error, by_shape):
    return {
        "ok": False,
        "code": error.response["Error"]["Code"],
        "message": error.response["Error"]["Message"],
        "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        "caught_by_shape": by_shape,
    }
"#;

/// What one row refuses, and the code the called service's model states for the class.
struct Row {
    case: &'static str,
    /// The service token the handler builds its client with.
    service: &'static str,
    /// How that service is reached on the wire.
    wire: &'static Wire,
    /// The client method the handler calls.
    method: &'static str,
    /// The operation that method names, which the wire request below addresses.
    operation: &'static str,
    /// The error shape the refusal is modelled as.
    shape: &'static str,
    /// The code a client of this service reports for it: the shape's `awsQueryError` code
    /// where its model states one, and the shape's own name where it does not.
    code: &'static str,
}

/// The rows, each naming the `smithy_query` reading its expected code comes from.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // `QueueDoesNotExist`: awsQueryError {code: "AWS.SimpleQueueService.
            // NonExistentQueue"} — a legacy code carrying the service's own prefix, which
            // no part of the shape's name appears in.
            case: "a send to a queue that is not there",
            service: "sqs",
            wire: &SQS_WIRE,
            method: "send_message",
            operation: "SendMessage",
            shape: "QueueDoesNotExist",
            code: "AWS.SimpleQueueService.NonExistentQueue",
        },
        Row {
            // `QueueNameExists`: awsQueryError {code: "QueueAlreadyExists"} — a legacy code
            // that is a different unprefixed word from the shape's own name.
            case: "a create of a held name with different attributes",
            service: "sqs",
            wire: &SQS_WIRE,
            method: "create_queue",
            operation: "CreateQueue",
            shape: "QueueNameExists",
            code: "QueueAlreadyExists",
        },
        Row {
            // `EmptyBatchRequest`: awsQueryError {code: "AWS.SimpleQueueService.
            // EmptyBatchRequest"} — the prefixed form of the shape's own name.
            case: "a batch send carrying no entries",
            service: "sqs",
            wire: &SQS_WIRE,
            method: "send_message_batch",
            operation: "SendMessageBatch",
            shape: "EmptyBatchRequest",
            code: "AWS.SimpleQueueService.EmptyBatchRequest",
        },
        Row {
            // `InvalidAttributeName`: the model states no `awsQueryError` on this class,
            // so what a query-era client knows it by is the shape's own name.
            case: "a read of an attribute the model does not define",
            service: "sqs",
            wire: &SQS_WIRE,
            method: "get_queue_attributes",
            operation: "GetQueueAttributes",
            shape: "InvalidAttributeName",
            code: "InvalidAttributeName",
        },
        Row {
            // A service with no `awsQueryCompatible` modifier: there is no legacy
            // vocabulary, so the shape IS the code its client reports.
            case: "a read of a secret that is not there, from a service that is not \
                   query-compatible",
            service: "secretsmanager",
            wire: &SECRETS_WIRE,
            method: "get_secret_value",
            operation: "GetSecretValue",
            shape: "ResourceNotFoundException",
            code: "ResourceNotFoundException",
        },
    ]
}

/// The parameters a row's call carries, built against the queue the row runs over.
fn parameters(case: &str, queue_url: &str, queue_name: &str) -> Value {
    match case {
        "a send to a queue that is not there" => json!({
            "QueueUrl": format!("{queue_url}-absent"),
            "MessageBody": "nowhere",
        }),
        "a create of a held name with different attributes" => json!({
            "QueueName": queue_name,
            "Attributes": { "VisibilityTimeout": "60" },
        }),
        "a batch send carrying no entries" => {
            json!({ "QueueUrl": queue_url, "Entries": [] })
        }
        "a read of an attribute the model does not define" => {
            json!({ "QueueUrl": queue_url, "AttributeNames": ["NotAnAttributeName"] })
        }
        _ => json!({ "SecretId": "no-such-secret-here" }),
    }
}

/// RULE: the code a handler's SDK reads back for a refusal is the one the called service put
/// on the wire — the legacy `awsQueryError` code for a query-compatible service, the error
/// shape's own name for every other — and never the modelled shape identifier where the two
/// differ.
#[tokio::test]
async fn a_handlers_sdk_reports_the_code_the_called_service_puts_on_the_wire() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router();
        let queue_name = format!("query-code-probe-{index}");
        let created = served(
            &router,
            case,
            "CreateQueue",
            &json!({
                "QueueName": queue_name,
                "Attributes": { "VisibilityTimeout": "30" },
            }),
        )
        .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();
        let input = parameters(case, &queue_url, &queue_name);

        // What the SAME request answers on the wire, asked for as a client of that service
        // asks: the legacy code where the service carries one, and the shape it named.
        let (wire_status, legacy, shape, wire_body) =
            called(&router, row.wire, row.operation, &input).await;
        assert!(
            (400..500).contains(&wire_status),
            "case {case:?}: expected the wire answer to refuse the call, got {wire_status} \
             with {wire_body}"
        );
        assert_eq!(
            shape.as_deref(),
            Some(row.shape),
            "case {case:?}: expected the wire answer to name the shape {:?}, got {shape:?} \
             with {wire_body}",
            row.shape
        );
        let on_the_wire = match row.wire.query_compatible {
            true => legacy.clone(),
            // No legacy vocabulary at all: a service without the modifier answers none, and
            // the shape is the code.
            false => {
                assert_eq!(
                    legacy, None,
                    "case {case:?}: expected a service that is not query-compatible to carry \
                     no legacy code, got {legacy:?}"
                );
                shape.clone()
            }
        };
        assert_eq!(
            on_the_wire.as_deref(),
            Some(row.code),
            "case {case:?}: expected the wire answer to carry the code {:?}, got \
             {on_the_wire:?} with {wire_body}",
            row.code
        );

        let function = format!("query-code-caller-{index}");
        let (status, _headers, response) = lambda(
            &router,
            "POST",
            FUNCTIONS_URI,
            &serde_json::to_vec(&json!({
                "FunctionName": function,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": HANDLER,
                "Timeout": 10,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD
                        .encode(package(HANDLER_SOURCE)),
                },
            }))
            .expect("the create body serializes"),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({response})"
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/invocations"),
            json!({
                "service": row.service,
                "method": row.method,
                "shape": row.shape,
                "parameters": input,
            })
            .to_string()
            .as_bytes(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
        );
        assert_eq!(
            payload["ok"],
            json!(false),
            "case {case:?}: expected the call to be refused, got {payload}"
        );
        assert_eq!(
            payload["code"].as_str(),
            Some(row.code),
            "case {case:?}: expected the handler to read the code {:?}, got {payload}",
            row.code
        );
        // And the shape identifier is not what it read, wherever the two differ — the
        // divergence this rule is about.
        if row.code != row.shape {
            assert_ne!(
                payload["code"].as_str(),
                Some(row.shape),
                "case {case:?}: expected the handler never to read the shape identifier {:?}, \
                 got {payload}",
                row.shape
            );
        }
        // The class named after the modelled shape, resolved before the call, still catches
        // the refusal: the legacy code and the shape name one class, as an SDK whose model
        // keys that class by the legacy code has it.
        assert_eq!(
            payload["caught_by_shape"],
            json!(true),
            "case {case:?}: expected `except client.exceptions.{}` to catch the refusal, got \
             {payload}",
            row.shape
        );
        // The rest of the answer is the service's own too, so nothing about the refusal is
        // the runtime's.
        assert_eq!(
            payload["message"].as_str(),
            wire_body
                .get("message")
                .or_else(|| wire_body.get("Message"))
                .and_then(Value::as_str),
            "case {case:?}: expected the handler to read the wire answer's own message, got \
             {payload} against {wire_body}"
        );
        assert_eq!(
            payload["status"].as_u64(),
            Some(u64::from(wire_status)),
            "case {case:?}: expected the handler to read the answer's status {wire_status}, \
             got {payload}"
        );
    }
}
