// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PHASE of a run a function's `Timeout` bounds: `CreateFunction`
//! then `Invoke` (`POST /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The RULE asserted over the first table, in one sentence: a function's `Timeout` — "the
//! amount of time that Lambda allows a function to run before stopping it" — bounds the
//! INVOCATION of the handler and not the initialization of the execution environment that
//! handler runs in, so a handler that returns at once is answered with its own value however
//! long that initialization took, and the time the runtime tells the handler it has left is
//! what is left of the `Timeout` itself.
//!
//! Every invocation this emulator serves initializes an environment of its own — the engine
//! is built, this runtime is installed and the handler's own module is imported, all before
//! the handler is reached — and none of that is the handler's run. A `Timeout` charged with
//! it is a `Timeout` the handler never gets: the invocation is answered as a timed-out or
//! abandoned run though its handler was never late.
//!
//! The rows differ along what the rule generalises over: the `Timeout` configured (the
//! smallest the model allows, and spans up to the longest run this emulator waits for), the
//! runtime whose environment is initialized (both engines), and how much work the handler's
//! own module does while it is being imported — which is the initialization the rule is
//! about.
//!
//! The second table is the other half of the same rule, so the first cannot be satisfied by
//! never stopping a run at all: the `Timeout` still bounds the HANDLER's own work, and one
//! that outlasts it is stopped and reports the sandbox timeout class.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::HOST_WAIT_CEILING;
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
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

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How much of its `Timeout` a handler may already have spent by the time it reads what it
/// has left, in milliseconds.
///
/// Not an allowance for the environment's setup — that is the whole point of the rule — but
/// for the few statements of the runtime BETWEEN the instant the invoke phase begins and the
/// handler's own reading of the remaining time: the context object is built and the handler
/// is called. It is generous enough that a loaded host cannot turn those statements into a
/// failure, and far smaller than the initialization it must not cover: a `Timeout` charged
/// with the environment's setup leaves a handler the smallest `Timeout` nothing at all.
const ENTRY_SLACK_MS: i64 = 500;

/// The `errorType` a run the sandbox ended at the function's own `Timeout` reports.
const SANDBOX_TIMEDOUT: &str = "Sandbox.Timedout";

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

/// One row of either table: the case, the runtime the function runs, the package entries, the
/// handler it names and the `Timeout` it is created with.
struct Row {
    case: &'static str,
    runtime: &'static str,
    entries: &'static [(&'static str, &'static str)],
    handler: &'static str,
    timeout: i64,
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
            "Timeout": row.timeout,
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

/// `Invoke` on `name`, re-driven while the answer reports one of this process's own HOST
/// BOUNDS instead of a run of the handler ([`is_host_bound`]): the run slots every suite of
/// this target shares, and the memory bound read from the whole process. Neither says
/// anything about the rule.
async fn invoke(router: &MultiServiceRouter, name: &str) -> Answer {
    let mut answered = invoke_once(router, name).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answered.status, &answered.text()) {
            break;
        }
        tokio::time::sleep(SLACK).await;
        answered = invoke_once(router, name).await;
    }
    answered
}

/// One `Invoke` request on the function `name` names.
async fn invoke_once(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        b"{}",
    )
    .await
}

/// A python handler that reports what the runtime tells it it has left.
const PYTHON_REPORTS_WHAT_IT_HAS_LEFT: &str =
    "def lambda_handler(event, context):\n    \
         return {'left': context.get_remaining_time_in_millis()}\n";

/// The same handler behind a module whose IMPORT does real work: the initialization of an
/// execution environment is the runtime's own setup AND the handler module's, so a module
/// that imports several standard-library modules and computes at import time is the row that
/// spends the most before the handler is reached.
const PYTHON_WORKS_WHILE_IT_IMPORTS: &str = "import base64\n\
     import datetime\n\
     import hashlib\n\
     \n\
     READY = base64.b64encode(hashlib.sha256(b'ready').digest()).decode()\n\
     STAMPED = datetime.datetime(2026, 1, 1).isoformat()\n\
     \n\
     def lambda_handler(event, context):\n    \
         return {'left': context.get_remaining_time_in_millis(),\n            \
                 'ready': bool(READY and STAMPED)}\n";

/// The node handler that reports the same.
const NODE_REPORTS_WHAT_IT_HAS_LEFT: &str =
    "exports.handler = async (event, context) => ({\n    \
         left: context.getRemainingTimeInMillis(),\n\
     });\n";

/// The node counterpart of [`PYTHON_WORKS_WHILE_IT_IMPORTS`]: a module that requires core
/// modules and computes while it is being loaded.
const NODE_WORKS_WHILE_IT_LOADS: &str = "const crypto = require('crypto');\n\
     const os = require('os');\n\
     const READY = crypto.createHash('sha256').update('ready').digest('hex');\n\
     const PLATFORM = os.platform();\n\
     exports.handler = async (event, context) => ({\n    \
         left: context.getRemainingTimeInMillis(),\n    \
         ready: Boolean(READY && PLATFORM),\n\
     });\n";

/// A python handler that outlasts any `Timeout` a row gives it.
const PYTHON_OUTLASTS_ITS_TIMEOUT: &str = "import time\n\
     def lambda_handler(event, context):\n    \
         time.sleep(600)\n    \
         return {'slept': True}\n";

/// A node handler that does the same, on the runtime's own timer queue.
const NODE_OUTLASTS_ITS_TIMEOUT: &str = "exports.handler = async () => {\n    \
         await new Promise((resolve) => setTimeout(resolve, 600000));\n    \
         return { slept: true };\n\
     };\n";

/// The whole rule, driven END TO END as one test: every row is a real `CreateFunction` and a
/// real `Invoke` over the router.
///
/// The two tables are one test, and one stream of invocations, for the reason the sibling
/// suites that run handlers are ([`one_invocation_stream_at_a_time`]): this emulator serves a
/// bounded number of runs at once for the whole process, and this target runs its suites in
/// parallel.
#[tokio::test]
async fn the_function_timeout_bounds_the_handlers_run_and_not_its_environments_setup() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let read = a_handler_is_given_the_whole_timeout_whatever_its_setup_cost(&router).await;
    assert!(
        read > 0,
        "no row of this table was answered by a run at all: every one was ended by this \
         process's own host bounds, so the rule was never observed"
    );
    let stopped = a_handler_that_outlasts_its_timeout_is_still_stopped(&router).await;
    assert!(
        stopped > 0,
        "no row of the timeout table was answered by a run at all, so the other half of the \
         rule was never observed"
    );
}

/// A handler that returns at once is answered with its own value, and the time it is told it
/// has left is what is left of the function's `Timeout` — whatever the initialization of the
/// environment it runs in cost.
///
/// Answers how many rows a run answered.
async fn a_handler_is_given_the_whole_timeout_whatever_its_setup_cost(
    router: &MultiServiceRouter,
) -> usize {
    let rows: &[Row] = &[
        // The smallest `Timeout` the model allows (`@range(min: 1)`), which is the row an
        // environment's setup charged to the `Timeout` leaves nothing of.
        Row {
            case: "a python handler with the smallest Timeout the model allows",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_REPORTS_WHAT_IT_HAS_LEFT)],
            handler: "lambda_function.lambda_handler",
            timeout: 1,
        },
        // The same `Timeout`, with the most setup before the handler: the module's own
        // imports and its module-level computation.
        Row {
            case: "a python handler behind a module that works while it imports",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_WORKS_WHILE_IT_IMPORTS)],
            handler: "lambda_function.lambda_handler",
            timeout: 1,
        },
        // A longer span, so the reading is not one `Timeout`'s.
        Row {
            case: "a python handler with a three-second Timeout",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_REPORTS_WHAT_IT_HAS_LEFT)],
            handler: "lambda_function.lambda_handler",
            timeout: 3,
        },
        // The other engine, whose environment this emulator builds the same way.
        Row {
            case: "a node handler with the smallest Timeout the model allows",
            runtime: NODE,
            entries: &[("index.js", NODE_REPORTS_WHAT_IT_HAS_LEFT)],
            handler: "index.handler",
            timeout: 1,
        },
        Row {
            case: "a node handler behind a module that works while it loads",
            runtime: NODE,
            entries: &[("index.js", NODE_WORKS_WHILE_IT_LOADS)],
            handler: "index.handler",
            timeout: 1,
        },
        Row {
            case: "a node handler with a five-second Timeout",
            runtime: NODE,
            entries: &[("index.js", NODE_REPORTS_WHAT_IT_HAS_LEFT)],
            handler: "index.handler",
            timeout: 5,
        },
    ];
    let name = "probe-init-phase";
    let mut read = 0_usize;
    for row in rows {
        let case = row.case;
        create_function(router, name, row).await;
        let answered = invoke(router, name).await;
        if is_host_bound(answered.status, &answered.text()) {
            delete_function(router, case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: a handler that returns at once must be answered, got {} {}",
            answered.status,
            answered.text()
        );
        let failure = answered.header("x-amz-function-error");
        assert_eq!(
            failure, None,
            "case {case:?}: the handler returns at once, so the invocation carries no function \
             error, got {failure:?} with {}",
            answered.text()
        );
        // What the handler may be told it has left: its whole `Timeout`, bounded by the
        // longest run this emulator waits for at all, and no more than that.
        let allowed_ms = i64::try_from(HOST_WAIT_CEILING.as_millis())
            .unwrap_or(i64::MAX)
            .min(row.timeout.saturating_mul(1_000));
        let document = answered.json();
        let left = document.get("left").and_then(Value::as_i64);
        assert!(
            left.is_some_and(|left| left > allowed_ms - ENTRY_SLACK_MS && left <= allowed_ms),
            "case {case:?}: expected the handler to be told it has between {} and \
             {allowed_ms} ms of its own run left, got {left:?} in {document}",
            allowed_ms - ENTRY_SLACK_MS
        );
        delete_function(router, case, name).await;
    }
    read
}

/// The `Timeout` still bounds the handler's OWN work: a handler that outlasts it is stopped
/// and the invocation reports the sandbox timeout class, naming the `Timeout` it ran past.
///
/// Answers how many rows a run answered.
async fn a_handler_that_outlasts_its_timeout_is_still_stopped(
    router: &MultiServiceRouter,
) -> usize {
    let rows: &[Row] = &[
        Row {
            case: "a python handler that sleeps past a one-second Timeout",
            runtime: PYTHON,
            entries: &[("lambda_function.py", PYTHON_OUTLASTS_ITS_TIMEOUT)],
            handler: "lambda_function.lambda_handler",
            timeout: 1,
        },
        Row {
            case: "a node handler that waits past a one-second Timeout",
            runtime: NODE,
            entries: &[("index.js", NODE_OUTLASTS_ITS_TIMEOUT)],
            handler: "index.handler",
            timeout: 1,
        },
    ];
    let name = "probe-init-phase-timeout";
    let mut read = 0_usize;
    for row in rows {
        let case = row.case;
        create_function(router, name, row).await;
        let answered = invoke(router, name).await;
        if is_host_bound(answered.status, &answered.text()) {
            delete_function(router, case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: a timed-out run is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case:?}: expected the Unhandled signal, got {}",
            answered.text()
        );
        let document = answered.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(SANDBOX_TIMEDOUT),
            "case {case:?}: expected the sandbox timeout class, got {document}"
        );
        let stated = format!("Task timed out after {:.2} seconds", row.timeout as f64);
        let message = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.ends_with(&stated),
            "case {case:?}: expected the message to state {stated:?}, got {message:?}"
        );
        delete_function(router, case, name).await;
    }
    read
}
