// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the time an invocation is told it has LEFT: `CreateFunction` (and
//! `UpdateFunctionConfiguration`) then `Invoke`
//! (`POST /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The RULE asserted here, in one sentence: the remaining time a runtime reports to a handler
//! starts, at handler entry, at the function's CURRENTLY configured `Timeout` — "the amount of
//! time that Lambda allows a function to run before stopping it", `@range(min: 1)` on
//! `smithy_query(query="shape", shape="Timeout")` — and from there counts DOWN with the
//! wall-clock time that handler itself has spent, each invocation measured from its own start.
//!
//! So the value is neither a constant the runtime reports once nor a span shared by the
//! environment: a handler that pauses must be told less afterwards, by about what it paused
//! for, and a later invocation of the same function must again be told it has the whole
//! `Timeout`.
//!
//! The rows differ along what the rule generalises over: the `Timeout` configured, where that
//! configuration came from (the create, or an update that raised or lowered it afterwards),
//! how long the handler pauses between its two readings, and the runtime whose context object
//! answers them (both engines).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::HOST_WAIT_CEILING;
use service_com_amazonaws_lambda_awsgirapiservice::state::{ACTIVATION_DELAY, UPDATE_DELAY};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` each engine is reached by.
const PYTHON: &str = "python3.12";
/// See [`PYTHON`].
const NODE: &str = "nodejs22.x";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How much of its `Timeout` a handler may already have spent by the time it takes its FIRST
/// reading, in milliseconds: the few statements of the runtime between the instant the invoke
/// phase begins and the handler's own first line — the context object is built and the handler
/// is called — and nothing else. It is generous enough that a loaded host cannot turn those
/// statements into a failure, and far smaller than the environment's own setup, which the rule
/// says is not charged at all.
const ENTRY_SLACK_MS: i64 = 500;

/// How much less than the pause the countdown may come to, in milliseconds: both runtimes
/// report whole milliseconds, so a reading taken either side of a pause can lose the fraction
/// at each end.
const CLOCK_SLACK_MS: i64 = 50;

/// How much MORE than the pause the countdown may come to, in milliseconds: whatever the host
/// spends on this run's thread beyond the pause the handler asked for. Far below any row's
/// `Timeout`, so a countdown that tracked nothing — a constant, or a value measured from the
/// environment's setup — is still a failure.
const COUNTDOWN_SLACK_MS: i64 = 1_500;

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
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
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
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
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

/// One row: the case, the runtime the function runs, the package entries, the handler it
/// names, the `Timeout` it is created with, the `Timeout` an update moves it to (when the row
/// updates one) and how long its handler pauses between its two readings.
struct Row {
    case: &'static str,
    runtime: &'static str,
    entries: &'static [(&'static str, &'static str)],
    handler: &'static str,
    created_timeout: i64,
    updated_timeout: Option<i64>,
    pause_ms: i64,
}

impl Row {
    /// The `Timeout` the function is configured with when it is invoked, which is what the
    /// rule measures against.
    fn configured_timeout(&self) -> i64 {
        self.updated_timeout.unwrap_or(self.created_timeout)
    }

    /// That `Timeout` in the unit the runtime reports the remaining time in, bounded by the
    /// longest run this emulator waits for at all — the HOST's own ceiling, which the sibling
    /// suite bounds its expectation by too. Every row of this table is far inside it, so the
    /// expectation stays derived from the configuration rather than from the table.
    fn budget_ms(&self) -> i64 {
        let ceiling = i64::try_from(HOST_WAIT_CEILING.as_millis()).unwrap_or(i64::MAX);
        self.configured_timeout().saturating_mul(1_000).min(ceiling)
    }
}

/// Create the function `row` describes and wait out the provisioning window the create
/// documents.
async fn create_function(router: &MultiServiceRouter, name: &str, row: &Row) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": row.runtime,
            "Handler": row.handler,
            "Timeout": row.created_timeout,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(row.entries)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {:?}: CreateFunction must succeed, got {} {}",
        row.case,
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Move the function's `Timeout` to `timeout` and wait out the update window.
async fn update_timeout(router: &MultiServiceRouter, case: &str, name: &str, timeout: i64) {
    let updated = call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &serde_json::to_vec(&json!({ "Timeout": timeout })).expect("the update body serializes"),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "case {case:?}: UpdateFunctionConfiguration must succeed, got {} {}",
        updated.status,
        updated.text()
    );
    tokio::time::sleep(UPDATE_DELAY + SLACK).await;
}

/// Remove the function `name`, so the next row creates its own.
async fn delete_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let deleted = call(router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[]).await;
    assert_eq!(
        deleted.status, 204,
        "case {case:?}: the function must be removable between rows, got {} {}",
        deleted.status,
        deleted.text()
    );
}

/// How many times a row re-drives its invocation while the answer reports one of this test
/// binary's own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 4;

/// `Invoke` on `name` with the pause the handler is to take, re-driven while the answer reports
/// one of this process's own HOST BOUNDS instead of a run of the handler ([`is_host_bound`]):
/// the run slots every suite of this target shares, and the memory bound read from the whole
/// process. Neither says anything about the rule.
async fn invoke(router: &MultiServiceRouter, name: &str, pause_ms: i64) -> Answer {
    let mut answered = invoke_once(router, name, pause_ms).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answered.status, &answered.text()) {
            break;
        }
        tokio::time::sleep(SLACK).await;
        answered = invoke_once(router, name, pause_ms).await;
    }
    answered
}

/// One `Invoke` request on the function `name` names.
async fn invoke_once(router: &MultiServiceRouter, name: &str, pause_ms: i64) -> Answer {
    let event = serde_json::to_vec(&json!({ "pauseMs": pause_ms })).expect("the event serializes");
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &event,
    )
    .await
}

/// A python handler that reads what it has left, pauses for as long as the event asks, and
/// reads again.
const PYTHON_READS_WHAT_IT_HAS_LEFT_TWICE: &str = "import time\n\
     \n\
     def lambda_handler(event, context):\n    \
         first = context.get_remaining_time_in_millis()\n    \
         time.sleep(float(event['pauseMs']) / 1000.0)\n    \
         second = context.get_remaining_time_in_millis()\n    \
         return {'first': first, 'second': second}\n";

/// The node counterpart, reading the same two values around a wait on the runtime's own timer
/// queue.
const NODE_READS_WHAT_IT_HAS_LEFT_TWICE: &str = "exports.handler = async (event, context) => {\n    \
         const first = context.getRemainingTimeInMillis();\n    \
         await new Promise((resolve) => setTimeout(resolve, event.pauseMs));\n    \
         const second = context.getRemainingTimeInMillis();\n    \
         return { first, second };\n\
     };\n";

/// The whole rule, driven END TO END as one test: every row is a real `CreateFunction` — and,
/// where the row says so, a real `UpdateFunctionConfiguration` — followed by two real `Invoke`
/// requests over the router.
///
/// One test, and one stream of invocations, for the reason the sibling suites that run
/// handlers are ([`one_invocation_stream_at_a_time`]): this emulator serves a bounded number
/// of runs at once for the whole process, and this target runs its suites in parallel.
#[tokio::test]
async fn the_remaining_time_starts_at_the_configured_timeout_and_counts_down() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let rows: &[Row] = &[
        // A short span on each engine, so the reading is not one `Timeout`'s, and a pause well
        // inside it.
        Row {
            case: "a python handler pausing briefly under a three-second Timeout",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "lambda_function.lambda_handler",
            created_timeout: 3,
            updated_timeout: None,
            pause_ms: 400,
        },
        // A longer span, and a pause long enough that a countdown reporting anything but
        // elapsed time cannot pass.
        Row {
            case: "a python handler pausing for over a second under a five-second Timeout",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "lambda_function.lambda_handler",
            created_timeout: 5,
            updated_timeout: None,
            pause_ms: 1_500,
        },
        // The `Timeout` the invocation is measured against is the CURRENT one: a function
        // created with the smallest the model allows and updated upwards is bounded by the
        // value it now carries.
        Row {
            case: "a python handler whose Timeout an update raised from the model's minimum",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "lambda_function.lambda_handler",
            created_timeout: 1,
            updated_timeout: Some(4),
            pause_ms: 500,
        },
        Row {
            case: "a node handler pausing briefly under a three-second Timeout",
            runtime: NODE,
            entries: &[("index.js", NODE_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "index.handler",
            created_timeout: 3,
            updated_timeout: None,
            pause_ms: 400,
        },
        Row {
            case: "a node handler pausing for over a second under a five-second Timeout",
            runtime: NODE,
            entries: &[("index.js", NODE_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "index.handler",
            created_timeout: 5,
            updated_timeout: None,
            pause_ms: 1_500,
        },
        // The same rule the other way round: an update that LOWERS the `Timeout` lowers what
        // the next invocation is told it has.
        Row {
            case: "a node handler whose Timeout an update lowered",
            runtime: NODE,
            entries: &[("index.js", NODE_READS_WHAT_IT_HAS_LEFT_TWICE)],
            handler: "index.handler",
            created_timeout: 5,
            updated_timeout: Some(3),
            pause_ms: 400,
        },
    ];
    let name = "probe-remaining-time";
    let mut read = 0_usize;
    for row in rows {
        let case = row.case;
        create_function(&router, name, row).await;
        if let Some(timeout) = row.updated_timeout {
            update_timeout(&router, case, name, timeout).await;
        }
        // Two invocations of the same function: each is measured from its own start, so the
        // second must be told it has the whole `Timeout` again.
        for attempt in ["first invocation", "second invocation"] {
            let answered = invoke(&router, name, row.pause_ms).await;
            if is_host_bound(answered.status, &answered.text()) {
                continue;
            }
            read += 1;
            assert_remaining_time(row, attempt, &answered);
        }
        delete_function(&router, case, name).await;
    }
    assert!(
        read > 0,
        "no invocation of this table was answered by a run at all: every one was ended by this \
         process's own host bounds, so the rule was never observed"
    );
}

/// The rule, read off one invocation's answer: the handler returned its own two readings, the
/// first is the whole configured `Timeout`, and the second is that first reading less the time
/// the handler spent between them.
fn assert_remaining_time(row: &Row, attempt: &str, answered: &Answer) {
    let case = format!("{} ({attempt})", row.case);
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: a handler that returns must be answered, got {} {}",
        answered.status,
        answered.text()
    );
    let failure = answered.header("x-amz-function-error");
    assert_eq!(
        failure, None,
        "case {case:?}: the handler returns of its own accord, so the invocation carries no \
         function error, got {failure:?} with {}",
        answered.text()
    );
    let document = answered.json();
    let first = document.get("first").and_then(Value::as_i64);
    let second = document.get("second").and_then(Value::as_i64);
    let budget = row.budget_ms();
    assert!(
        first.is_some_and(|first| first > budget - ENTRY_SLACK_MS && first <= budget),
        "case {case:?}: expected the handler to be told at entry that it has between {} and \
         {budget} ms left, got {first:?} in {document}",
        budget - ENTRY_SLACK_MS
    );
    let (Some(first), Some(second)) = (first, second) else {
        panic!("case {case:?}: expected both readings in the answer, got {document}");
    };
    assert!(
        second < first,
        "case {case:?}: expected the second reading to be below the first {first}, got {second} \
         in {document}"
    );
    assert!(
        second > 0,
        "case {case:?}: the handler paused for {} ms of a {budget} ms Timeout, so it still has \
         time left, got {second} in {document}",
        row.pause_ms
    );
    let counted_down = first - second;
    assert!(
        counted_down >= row.pause_ms - CLOCK_SLACK_MS
            && counted_down <= row.pause_ms + COUNTDOWN_SLACK_MS,
        "case {case:?}: the handler paused for {} ms, so the remaining time must have counted \
         down by between {} and {} ms, got {counted_down} ms (from {first} to {second}) in \
         {document}",
        row.pause_ms,
        row.pause_ms - CLOCK_SLACK_MS,
        row.pause_ms + COUNTDOWN_SLACK_MS
    );
}
