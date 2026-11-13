// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InvokeAsync`: the event a caller submits is validated against
//! the function the store holds, queued against the version it was addressed to, and
//! acknowledged with the documented "202 Accepted", which "indicates that the request is
//! queued for invocation" and says nothing about the run.
//!
//! The suite asserts that rule over a TABLE: every documented `FunctionName` form, every
//! shape of JSON event, the payload ceiling at its published bound and one byte past it,
//! and each condition the operation refuses. No modelled operation reads an accepted
//! asynchronous invocation back out — Lambda exposes none — so the round trip is through
//! the service's own state, which is where the queued event lands.
//!
//! The clock-driven half (a queued event whose delivery attempt has come due) is
//! exercised by moving the stored due instant into the past, so nothing here sleeps. What
//! becomes of a delivered event is
//! [`super::async_invocation_delivery`]'s subject, not this suite's.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, AsyncInvocationRecord, ComAmazonawsLambdaAwsgirapiserviceState,
    ASYNC_NOT_DELIVERED_REASON, MAX_QUEUED_ASYNC_INVOCATIONS,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off, exactly as
/// the emulator runs it unless started with authentication on, so nothing here is
/// verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The payload ceiling the operation's own documentation states: "The payload limit is
/// 256KB. For larger payloads, for up to 1MB, use Invoke."
const PAYLOAD_LIMIT: usize = 256 * 1024;

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The wire error code (`__type`) of an error response, or the body when it carries
    /// none.
    fn error_code(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| {
                body.get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }
}

/// Dispatch one request with a raw body.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// A JSON control-plane request.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    send(router, method, uri, "application/json", payload).await
}

/// `InvokeAsync` (`POST /2014-11-13/functions/{FunctionName}/invoke-async`).
async fn invoke_async(router: &MultiServiceRouter, reference: &str, event: Vec<u8>) -> Answer {
    send(
        router,
        "POST",
        &format!("/2014-11-13/functions/{reference}/invoke-async"),
        "application/octet-stream",
        event,
    )
    .await
}

/// `GetFunctionConfiguration`, used only to drive one more request through the service
/// so whatever the clock has made due is materialised.
async fn get_function_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        &Value::Null,
    )
    .await
}

/// Deterministic deployment-package bytes of `length`.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// A JSON string literal of exactly `total` bytes: the two quotes plus the filler.
fn json_payload_of(total: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(total);
    payload.push(b'"');
    payload.extend(std::iter::repeat_n(b'a', total - 2));
    payload.push(b'"');
    payload
}

/// Create one function, publishing version 1 so a qualified reference resolves too.
async fn create_function(router: &MultiServiceRouter, name: &str, publish: bool) {
    let answer = json_call(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package_bytes(64)),
            },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "creating {name} must succeed, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Move every stored function's activation instant into the past, so the next request
/// materialises the `Pending` -> `Active` move the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Hold one event for `name` that NO delivery attempt ever claimed, with its attempt
/// instant already in the past: what an event accepted where nothing could deliver it looks
/// like by the time the clock has made its attempt due.
fn hold_undelivered_event(engine: &Arc<engine::InMemoryEngine>, name: &str, payload: &[u8]) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let function = store
        .functions
        .get_mut(name)
        .expect("the function must be held");
    function.async_invocations.push_back(AsyncInvocationRecord {
        version: "$LATEST".to_string(),
        request_id: "undelivered-event".to_string(),
        payload: payload.to_vec(),
        received: "2026-01-01T00:00:00.000Z".to_string(),
        attempt_due: SystemTime::now() - Duration::from_secs(1),
        attempts: 0,
        // Nothing is delivering it: this is the entry a process with no asynchronous
        // runtime to deliver on leaves behind, which is what the clock settles.
        attempt_pending: false,
        outcome: AsyncInvocationOutcome::Queued,
    });
}

/// Create a function and bring it to `Active`, which is the state an invocation needs.
async fn create_active_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    publish: bool,
) {
    create_function(router, name, publish).await;
    make_activation_due(engine);
}

/// The events queued for `name`, in queue order, read out of the service's own state
/// because no modelled operation reports them.
fn queued(engine: &Arc<engine::InMemoryEngine>, name: &str) -> Vec<AsyncInvocationRecord> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|function| function.async_invocations.iter().cloned().collect())
        .unwrap_or_default()
}

/// How long a test waits for a delivery attempt to settle the event it queued. The attempt
/// becomes due a quarter of a second after the accepting call, so this is many times the
/// span it waits for and still bounds the test.
const DELIVERY_BUDGET: Duration = Duration::from_secs(20);

/// The outcome the delivery attempt for `name`'s single queued event settled on, waited for
/// rather than slept for.
async fn settled_outcome(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> AsyncInvocationOutcome {
    let deadline = std::time::Instant::now() + DELIVERY_BUDGET;
    loop {
        let outcome = queued(engine, name).first().map(|event| event.outcome.clone());
        match outcome {
            Some(AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering) | None => {}
            Some(settled) => return settled,
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the delivery attempt for {name} never settled within {DELIVERY_BUDGET:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A short, printable rendering of a payload, so a failure names the actual value
/// without dumping 256 KB of it.
fn render(payload: &[u8]) -> String {
    format!(
        "{} bytes starting {:?}",
        payload.len(),
        String::from_utf8_lossy(&payload[..payload.len().min(32)])
    )
}

/// One accepted invocation: how it was addressed, what it carried, and the version it
/// must be queued against.
struct Accepted {
    case: &'static str,
    reference: String,
    event: Vec<u8>,
    version: &'static str,
}

/// The acceptance table: every documented `FunctionName` form from the member's own
/// documentation ("Function name – my-function", "Function ARN – arn:aws:lambda:…",
/// "Partial ARN – 123456789012:function:my-function", each optionally qualified), every
/// shape a JSON event takes, and the payload ceiling at its published bound.
fn accepted_cases() -> Vec<Accepted> {
    let arn = function_arn("queued");
    vec![
        Accepted {
            case: "an empty event",
            reference: "queued".to_string(),
            event: Vec::new(),
            version: "$LATEST",
        },
        Accepted {
            case: "an empty JSON object",
            reference: "queued".to_string(),
            event: b"{}".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a JSON object",
            reference: "queued".to_string(),
            event: br#"{"key":"value","n":1}"#.to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a JSON array",
            reference: "queued".to_string(),
            event: b"[1,2,3]".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a JSON string",
            reference: "queued".to_string(),
            event: br#""event""#.to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a JSON number",
            reference: "queued".to_string(),
            event: b"42".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "the bare function: prefix",
            reference: "function:queued".to_string(),
            event: b"{}".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a partial ARN",
            reference: format!("{ACCOUNT}:function:queued"),
            event: b"{}".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a function ARN",
            reference: arn.clone(),
            event: b"{}".to_vec(),
            version: "$LATEST",
        },
        Accepted {
            case: "a name qualified with a published version",
            reference: "queued:1".to_string(),
            event: br#"{"to":"version 1"}"#.to_vec(),
            version: "1",
        },
        Accepted {
            case: "an ARN qualified with a published version",
            reference: format!("{arn}:1"),
            event: br#"{"to":"version 1 by arn"}"#.to_vec(),
            version: "1",
        },
        Accepted {
            case: "a payload at the 256 KB limit",
            reference: "queued".to_string(),
            event: json_payload_of(PAYLOAD_LIMIT),
            version: "$LATEST",
        },
    ]
}

#[tokio::test]
async fn invoke_async_queues_every_event_it_accepts_and_answers_202() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "queued", true).await;

    for (index, case) in accepted_cases().into_iter().enumerate() {
        let answer = invoke_async(&router, &case.reference, case.event.clone()).await;
        assert_eq!(
            answer.status,
            202,
            "case {}: expected 202 Accepted, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // The `Status` member is bound to the response code, so an accepted invocation
        // reports it as the HTTP status and the body carries no member of its own.
        let reported: Value = if answer.body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&answer.body).unwrap_or_else(|error| {
                panic!(
                    "case {}: expected a JSON body, got {error}: {}",
                    case.case,
                    String::from_utf8_lossy(&answer.body)
                )
            })
        };
        assert_eq!(
            reported,
            json!({}),
            "case {}: expected a response carrying no member of its own, got {reported}",
            case.case
        );

        let events = queued(&engine, "queued");
        assert_eq!(
            events.len(),
            index + 1,
            "case {}: expected {} queued events, got {}",
            case.case,
            index + 1,
            events.len()
        );
        let newest = events
            .last()
            .unwrap_or_else(|| panic!("case {}: the queue must hold the event", case.case));
        assert!(
            newest.payload == case.event,
            "case {}: expected the queued payload to be {}, got {}",
            case.case,
            render(&case.event),
            render(&newest.payload)
        );
        assert_eq!(
            newest.version, case.version,
            "case {}: expected the event queued against version {}, got {}",
            case.case, case.version, newest.version
        );
        assert_eq!(
            newest.outcome,
            AsyncInvocationOutcome::Queued,
            "case {}: expected a freshly accepted event to be queued, got {:?}",
            case.case,
            newest.outcome
        );
        assert!(
            !newest.received.is_empty(),
            "case {}: expected the acceptance instant to be recorded, got {:?}",
            case.case,
            newest.received
        );
    }
}

/// One request the operation refuses, and the modelled failure it refuses it with.
struct Refusal {
    case: &'static str,
    reference: String,
    event: Vec<u8>,
    status: u16,
    code: &'static str,
}

/// The refusal table: a request naming no function, naming a version the function does
/// not have, carrying a body that is not "the JSON that you want to provide to your
/// Lambda function as input", carrying one byte past the published payload ceiling, and
/// arriving while the function is still being provisioned.
fn refusal_cases() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a function that does not exist",
            reference: "absent".to_string(),
            event: b"{}".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "an ARN of a function that does not exist",
            reference: function_arn("absent"),
            event: b"{}".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a version the function does not have",
            reference: "queued:7".to_string(),
            event: b"{}".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a truncated JSON object",
            reference: "queued".to_string(),
            event: br#"{"k":"#.to_vec(),
            status: 400,
            code: "InvalidRequestContentException",
        },
        Refusal {
            case: "plain text",
            reference: "queued".to_string(),
            event: b"not json".to_vec(),
            status: 400,
            code: "InvalidRequestContentException",
        },
        Refusal {
            case: "two JSON documents in one body",
            reference: "queued".to_string(),
            event: b"{} {}".to_vec(),
            status: 400,
            code: "InvalidRequestContentException",
        },
        Refusal {
            case: "one byte past the 256 KB payload limit",
            reference: "queued".to_string(),
            event: json_payload_of(PAYLOAD_LIMIT + 1),
            status: 402,
            code: "ServiceQuotaExceededException",
        },
        Refusal {
            case: "a function that is still being provisioned",
            reference: "provisioning".to_string(),
            event: b"{}".to_vec(),
            status: 409,
            code: "ResourceConflictException",
        },
    ]
}

#[tokio::test]
async fn invoke_async_refuses_what_it_cannot_accept_and_queues_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "queued", true).await;
    // Created last and left alone, so it is still inside the provisioning window the
    // model documents: "During this time, you can't invoke or modify the function."
    create_function(&router, "provisioning", false).await;

    for case in refusal_cases() {
        let answer = invoke_async(&router, &case.reference, case.event.clone()).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains(case.code),
            "case {}: expected {}, got {}",
            case.case,
            case.code,
            answer.error_code()
        );
        for name in ["queued", "provisioning"] {
            assert!(
                queued(&engine, name).is_empty(),
                "case {}: expected a refused invocation to queue nothing for {name}, got {} \
                 queued",
                case.case,
                queued(&engine, name).len()
            );
        }
    }
}

#[tokio::test]
async fn a_queued_event_is_delivered_to_its_function_and_the_entry_records_the_attempt() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "queued", false).await;

    let answer = invoke_async(&router, "queued", br#"{"event":true}"#.to_vec()).await;
    assert_eq!(
        answer.status,
        202,
        "expected 202 Accepted, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    let events = queued(&engine, "queued");
    assert_eq!(
        events.len(),
        1,
        "expected exactly one queued event, got {}",
        events.len()
    );
    assert_eq!(
        events[0].outcome,
        AsyncInvocationOutcome::Queued,
        "expected the event to be queued before its attempt is due, got {:?}",
        events[0].outcome
    );

    // The delivery attempt the queueing owes: the event is handed to the function and the
    // attempt's own outcome lands on the entry. This package is 64 deterministic bytes
    // rather than an archive, so the attempt could not read it — which is still an ATTEMPT
    // that was made, not an event nobody ever delivered.
    let settled = settled_outcome(&engine, "queued").await;
    let AsyncInvocationOutcome::Failed(reason) = &settled else {
        panic!("expected the attempt to report why the function could not run, got {settled:?}")
    };
    assert!(
        reason.contains("could not be read"),
        "expected the reason to name the unreadable deployment package, got {reason:?}"
    );

    let held = queued(&engine, "queued");
    assert_eq!(
        held.len(),
        1,
        "expected the one event to still be held, got {}",
        held.len()
    );
    assert!(
        held[0].payload == br#"{"event":true}"#.to_vec(),
        "expected the event to keep its payload, got {}",
        render(&held[0].payload)
    );
}

#[tokio::test]
async fn an_event_no_attempt_claimed_records_that_nothing_delivered_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "queued", false).await;

    // An event whose attempt instant the clock has already passed and which no delivery
    // attempt ever took: what an acceptance in a process with no asynchronous runtime
    // leaves behind. The next request materialises what the clock owes on it.
    hold_undelivered_event(&engine, "queued", br#"{"event":true}"#);
    let read = get_function_configuration(&router, "queued").await;
    assert_eq!(
        read.status,
        200,
        "the configuration read must succeed, got {} {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );

    let settled = queued(&engine, "queued");
    assert_eq!(
        settled.len(),
        1,
        "expected the one event to still be held, got {}",
        settled.len()
    );
    assert_eq!(
        settled[0].outcome,
        AsyncInvocationOutcome::Failed(ASYNC_NOT_DELIVERED_REASON.to_string()),
        "expected the due attempt to record why the event never ran, got {:?}",
        settled[0].outcome
    );
}

#[tokio::test]
async fn the_queue_holds_its_bound_and_drops_the_oldest_event() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "queued", false).await;

    // Three past the bound, so the oldest three must have made room for the newest
    // three while every accepted call still answered 202.
    let total = MAX_QUEUED_ASYNC_INVOCATIONS + 3;
    for index in 0..total {
        let event = format!(r#"{{"index":{index}}}"#).into_bytes();
        let answer = invoke_async(&router, "queued", event).await;
        assert_eq!(
            answer.status,
            202,
            "case event {index}: expected 202 Accepted, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let events = queued(&engine, "queued");
    assert_eq!(
        events.len(),
        MAX_QUEUED_ASYNC_INVOCATIONS,
        "expected the queue to hold its bound of {MAX_QUEUED_ASYNC_INVOCATIONS}, got {}",
        events.len()
    );
    for (position, event) in events.iter().enumerate() {
        let expected = format!(
            r#"{{"index":{}}}"#,
            position + total - MAX_QUEUED_ASYNC_INVOCATIONS
        );
        assert!(
            event.payload == expected.as_bytes(),
            "case position {position}: expected the queue to hold {expected}, got {}",
            render(&event.payload)
        );
    }
}
