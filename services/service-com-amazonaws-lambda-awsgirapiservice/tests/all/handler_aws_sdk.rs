// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler calls another service of this emulator through, end to end
//! over the wire: Lambda and Amazon SQS on ONE engine, a `python3.x` function whose code
//! does `import boto3` and calls the queue, and the queue read back where a client reads it.
//!
//! The rule pinned here: AWS Lambda's managed runtimes ship the AWS SDK for the handler's
//! language on its import path, so a handler can call another AWS service with no dependency
//! of its own, and what the call answers is that service's own answer — a success carrying
//! the operation's output members, or the service's modelled refusal raised as
//! `botocore.exceptions.ClientError` with that service's error code and message.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the service is
//! called (a write, a batch write, a read, a delete, a read of the queue's own attributes),
//! whether the call succeeds or is refused, the service token the client is built with, and
//! whether the operation named exists at all. Each row asserts both halves of the rule — what
//! the handler read back, which is what it returns as the invocation's payload, and what the
//! QUEUE itself holds afterwards, read through its own API — so a call that answered without
//! doing the work, or did the work without answering, fails here.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the handler's SDK call is dispatched back into this same router, so the queue it writes
/// to is the queue this suite reads.
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

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
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
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&body).to_string())
        })
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

/// Create a function whose code is `source` and wait out the provisioning window.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str, source: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, answering the payload the run produced
/// and the `FunctionError` signal it carried, if any.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// The handler every row runs: it reports what the ONE SDK call it makes answered, so the
/// invocation's payload IS what the SDK gave the guest.
///
/// The call is named by the event, so one package serves every row: the rows differ in the
/// service token, the method and the parameters they ask for, which is what the rule
/// generalises over.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client(event["service"])
    try:
        method = getattr(client, event["method"])
    except AttributeError as error:
        return {"ok": False, "attribute_error": str(error)}
    try:
        answered = method(**event["parameters"])
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "sentence": str(error),
            "modelled": isinstance(error, client.exceptions.from_code(
                error.response["Error"]["Code"])),
        }
    except botocore.exceptions.UnknownServiceError as error:
        return {"ok": False, "unknown_service": str(error)}
    return {
        "ok": True,
        "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        "members": sorted(key for key in answered if key != "ResponseMetadata"),
        "answered": {
            key: value for key, value in answered.items() if key != "ResponseMetadata"
        },
    }
"#;

/// What one row asks the handler to do, and what must come of it.
struct Row {
    case: &'static str,
    /// The service token the handler builds its client with.
    service: &'static str,
    /// The client method it calls.
    method: &'static str,
    /// The operation that method names, which is what the call must address.
    operation: &'static str,
    /// Whether the call must succeed.
    expect_ok: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a write: SendMessage puts one message on the queue",
            service: "sqs",
            method: "send_message",
            operation: "SendMessage",
            expect_ok: true,
        },
        Row {
            case: "a batch write: SendMessageBatch puts three messages on the queue",
            service: "sqs",
            method: "send_message_batch",
            operation: "SendMessageBatch",
            expect_ok: true,
        },
        Row {
            case: "a read of the queue's own attributes: GetQueueAttributes",
            service: "sqs",
            method: "get_queue_attributes",
            operation: "GetQueueAttributes",
            expect_ok: true,
        },
        Row {
            case: "a read by URL: GetQueueUrl resolves the queue by name",
            service: "sqs",
            method: "get_queue_url",
            operation: "GetQueueUrl",
            expect_ok: true,
        },
        Row {
            case: "a refused write: SendMessage to a queue that is not there",
            service: "sqs",
            method: "send_message",
            operation: "SendMessage",
            expect_ok: false,
        },
        Row {
            // The method name is turned into the operation it names and the call is
            // addressed to it, so what the handler reads is the SERVICE's answer about
            // that operation rather than a verdict this runtime reached on its own.
            case: "a method naming an operation the service does not serve",
            service: "sqs",
            method: "frobnicate_queue",
            operation: "FrobnicateQueue",
            expect_ok: false,
        },
    ]
}

/// The parameters each row's call carries, built against the queue the row runs over.
fn parameters(case: &str, queue_url: &str, queue_name: &str) -> Value {
    match case {
        "a write: SendMessage puts one message on the queue" => {
            json!({ "QueueUrl": queue_url, "MessageBody": "from the handler" })
        }
        "a batch write: SendMessageBatch puts three messages on the queue" => json!({
            "QueueUrl": queue_url,
            "Entries": [
                { "Id": "one", "MessageBody": "first" },
                { "Id": "two", "MessageBody": "second" },
                { "Id": "three", "MessageBody": "third" },
            ],
        }),
        "a read of the queue's own attributes: GetQueueAttributes" => {
            json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] })
        }
        "a read by URL: GetQueueUrl resolves the queue by name" => {
            json!({ "QueueName": queue_name })
        }
        "a refused write: SendMessage to a queue that is not there" => json!({
            "QueueUrl": format!("{queue_url}-absent"),
            "MessageBody": "nowhere",
        }),
        _ => json!({ "QueueUrl": queue_url }),
    }
}

/// How many messages a row's call must have left on the queue.
fn expected_on_queue(case: &str) -> usize {
    match case {
        "a write: SendMessage puts one message on the queue" => 1,
        "a batch write: SendMessageBatch puts three messages on the queue" => 3,
        _ => 0,
    }
}

/// One SDK call per row, asserted on both sides: what the guest read back, and what the
/// queue itself holds.
#[tokio::test]
async fn a_handler_calls_another_service_through_the_sdk_the_runtime_supplies() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router();
        let queue_name = format!("sdk-probe-{index}");
        let created = sqs(
            &router,
            case,
            "CreateQueue",
            &json!({ "QueueName": queue_name }),
        )
        .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();

        let function = format!("sdk-caller-{index}");
        create_function(&router, case, &function, HANDLER_SOURCE).await;
        let (signal, payload) = invoke(
            &router,
            case,
            &function,
            &json!({
                "service": row.service,
                "method": row.method,
                "parameters": parameters(case, &queue_url, &queue_name),
            }),
        )
        .await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        assert_eq!(
            payload["ok"],
            json!(row.expect_ok),
            "case {case:?}: expected ok={}, got {payload}",
            row.expect_ok
        );

        if row.expect_ok {
            assert_eq!(
                payload["status"],
                json!(200),
                "case {case:?}: expected the SDK to report HTTP 200, got {payload}"
            );
            assert!(
                payload["members"]
                    .as_array()
                    .is_some_and(|members| !members.is_empty()),
                "case {case:?}: expected the answer to carry the operation's own output \
                 members, got {payload}"
            );
        } else {
            let code = payload["code"].as_str().unwrap_or_default();
            assert!(
                !code.is_empty() && code != "Unknown",
                "case {case:?}: expected the service's own error code, got {payload}"
            );
            // The sentence botocore renders for a refusal, so a handler that logs the
            // exception logs what a real client logs — and it names the OPERATION the
            // method was turned into.
            let sentence = payload["sentence"].as_str().unwrap_or_default();
            let expected = format!(
                "An error occurred ({code}) when calling the {} operation: ",
                row.operation
            );
            assert!(
                sentence.starts_with(&expected),
                "case {case:?}: expected the sentence to open {expected:?}, got \
                 {sentence:?}"
            );
            assert_eq!(
                payload["modelled"],
                json!(true),
                "case {case:?}: expected the refusal to be the modelled error class for its \
                 code, got {payload}"
            );
        }

        // What the QUEUE holds: the work the call reported is the work that happened.
        let drained = sqs(
            &router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let held = drained["Messages"].as_array().map_or(0, Vec::len);
        assert_eq!(
            held,
            expected_on_queue(case),
            "case {case:?}: expected {} message(s) on the queue, got {held} ({drained})",
            expected_on_queue(case)
        );
    }
}

/// A service this runtime's SDK does not reach is reported as an unknown service, and
/// nothing is sent — the SDK never guesses at a protocol it does not implement.
#[tokio::test]
async fn a_service_the_runtimes_sdk_does_not_reach_is_an_unknown_service() {
    let router = router();
    let case = "a client built for a service the runtime's SDK does not reach";
    create_function(&router, case, "sdk-unknown", HANDLER_SOURCE).await;
    let (signal, payload) = invoke(
        &router,
        case,
        "sdk-unknown",
        &json!({
            // A real AWS service whose own protocol this runtime's SDK seam binds no
            // request for, so the client cannot be built for it at all.
            "service": "route53",
            "method": "list_hosted_zones",
            "parameters": {},
        }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
    );
    let reported = payload["unknown_service"].as_str().unwrap_or_default();
    assert!(
        reported.contains("Unknown service: 'route53'"),
        "case {case:?}: expected the unknown service to be named, got {payload}"
    );
}

/// The SDK the runtime supplies is on the import path BESIDE the package's own entries, so
/// a package that ships its own module of that name wins — the order a real runtime
/// resolves them in.
#[tokio::test]
async fn a_package_that_ships_its_own_sdk_module_wins_over_the_runtimes() {
    let router = router();
    let case = "a package shipping its own boto3 module";
    let source = r#"
import boto3


def lambda_handler(event, context):
    return {"shipped": getattr(boto3, "shipped_by_the_package", False)}
"#;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer.write_all(source.as_bytes()).expect("the bytes");
        writer
            .start_file("boto3.py", options)
            .expect("the entry starts");
        writer
            .write_all(b"shipped_by_the_package = True\n")
            .expect("the bytes");
        writer.finish().expect("a finished archive");
    }
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": "sdk-shadowed",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(buffer.into_inner()),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    let (signal, payload) = invoke(&router, case, "sdk-shadowed", &json!({})).await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
    );
    assert_eq!(
        payload["shipped"],
        json!(true),
        "case {case:?}: expected the package's own module to win, got {payload}"
    );
}
