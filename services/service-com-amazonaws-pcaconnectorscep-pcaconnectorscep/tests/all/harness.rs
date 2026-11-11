// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate drives the service through.
//!
//! The suites of `tests/all.rs` compile into ONE test crate, so each of the rules below can
//! live here exactly once. They are rules, not repeated literals — which bytes of a path
//! segment an SDK escapes, how a request reaches this service, how to move the instant the
//! clock owes a connector its pending transition — so a private copy of one in a suite is a
//! copy that can drift away from the others:
//!
//! * [`new_router`], [`call`] / [`call_with_scope`] and [`Answer`]: one HTTP request per call
//!   through the same `MultiServiceRouter` the emulator boots, carrying the SigV4 credential
//!   scope a handler reads the caller's Region out of, and what came back.
//! * [`label`]: the `@httpLabel` percent-encoding rule.
//! * [`assert_tagging_not_found`]: the refusal the tagging endpoints answer an ARN naming no
//!   resource with, which is the tagging layer's own error class and not this service's.
//! * [`set_pending_due`] and the four helpers over it: the clock move, so a case about a
//!   transient status neither sleeps for the transition span nor races it.
//!
//! The clock move is ONE rule with two intents a case can want. [`set_pending_due`] states
//! the rule and answers whether anything was owed; [`release_pending`] and [`hold_pending`]
//! require a transition to be owed, and the `_if_owed` pair tolerates there being none and
//! reports it, for a case whose record may already have settled or been torn down.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// How far in the past a released transition's due instant is put: any past instant is due,
/// so the next call settles it.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// How far in the future a held transition's due instant is put: longer than any case takes,
/// so every call in it observes the transient status.
const NOT_DUE_FOR: Duration = Duration::from_secs(3600);

/// What one call answered: the HTTP status, the modelled error code (empty on success), the
/// raw body bytes and the parsed body (`Value::Null` when there were none).
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub error_type: String,
    pub raw: Vec<u8>,
    pub body: Value,
}

impl Answer {
    /// One member of the response body, as a string.
    pub fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }

    /// The returned `Tags` map, which an implemented listing always carries.
    pub fn tags(&self) -> &Map<String, Value> {
        self.body["Tags"]
            .as_object()
            .unwrap_or_else(|| panic!("expected a `Tags` map in {:?}", self.body))
    }

    /// How many entries a listing's item member carries (absent counts as none).
    pub fn count(&self, member: &str) -> usize {
        self.body[member].as_array().map_or(0, Vec::len)
    }

    /// The `Message` an error body carries, or the empty string when it carries none.
    pub fn message(&self) -> &str {
        self.body["Message"].as_str().unwrap_or_default()
    }

    /// One member of the returned `Connector`, or `Value::Null` when it carries none.
    pub fn connector(&self, name: &str) -> &Value {
        &self.body["Connector"][name]
    }

    /// One member of the returned `Challenge`, or `Value::Null` when it carries none.
    pub fn challenge(&self, name: &str) -> &Value {
        &self.body["Challenge"][name]
    }
}

/// The router, with the shared engine it was built from, so a case that needs a clock-driven
/// transition to be in a known position can put it there instead of racing it.
pub fn new_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Percent-encode a value for use in a single `@httpLabel` path segment or a query-string
/// value, the way an AWS SDK does: every byte outside the unreserved set is escaped, so the
/// `:` and `/` inside an ARN cannot read as path structure and the `=` padding of a
/// `NextToken` cannot read as a parameter separator. One rule covers both places.
pub fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// One request, addressed at `region`'s endpoint and signed for the same Region.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    call_with_scope(router, region, region, method, path, body).await
}

/// One request addressed at `host_region`'s endpoint but carrying `scope_region` in its SigV4
/// credential scope — which is how a handler learns the Region the caller is addressing — so a
/// case can send a scope Region no endpoint of this service has.
pub async fn call_with_scope(
    router: &MultiServiceRouter,
    host_region: &str,
    scope_region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://pca-connector-scep.{host_region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{scope_region}/pca-connector-scep/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let raw = response.into_body().to_vec();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        raw,
        body,
    }
}

/// Assert `answer` is the refusal the resource-tagging endpoints (`/tags/{ResourceArn}`, by
/// method) answer an ARN that names no resource with.
///
/// Those three operations are served by the resource-tagging layer, which reports an ARN it
/// resolves to nothing with its OWN error class rather than with this service's
/// `ResourceNotFoundException`: HTTP 400, the `NotFoundException` error code, and a single
/// lower-case `message` member carrying one fixed sentence — no ARN, and none of the service
/// error's `Message` / `ResourceId` / `ResourceType` members. The three literals are the
/// recorded wire values of that refusal, spelled here rather than read back out of the
/// implementation.
pub fn assert_tagging_not_found(answer: &Answer, case: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "NotFoundException"),
        "case {case:?}: expected the tagging layer's 400 NotFoundException, got {} / {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some("Resource was not found"),
        "case {case:?}: expected the fixed lower-case `message` of the tagging layer's \
         refusal, got {:?}",
        answer.body
    );
    for member in ["Message", "ResourceId", "ResourceType"] {
        assert!(
            answer.body.get(member).is_none(),
            "case {case:?}: the tagging layer's refusal carries no {member:?} member, got {:?}",
            answer.body
        );
    }
}

/// Move the instant the clock owes `arn` its pending transition to `due_at`, through the
/// service's own state.
///
/// Strictly a clock move: it rewrites only the pending transition's due instant, never the
/// status, the timestamps or the outcome — the handlers still compute those from this same
/// record by settling it. Answers whether a transition was there to move: `false` once the
/// record has settled, its teardown has completed, or nothing was ever created under `arn`.
pub fn set_pending_due(engine: &engine::InMemoryEngine, arn: &str, due_at: SystemTime) -> bool {
    let shared = scep::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    match store
        .connectors
        .get_mut(arn)
        .and_then(|connector| connector.pending.as_mut())
    {
        Some(pending) => {
            pending.due_at = due_at;
            true
        }
        None => false,
    }
}

/// Make the transition the clock owes `arn` due, so the next call materialises it: `CREATING`
/// into `ACTIVE`, or a `DELETING` connector's teardown into the record being gone.
///
/// Requires a transition to be owed; use [`release_pending_if_owed`] where a case tolerates
/// there being none.
pub fn release_pending(engine: &engine::InMemoryEngine, arn: &str) {
    assert!(
        release_pending_if_owed(engine, arn),
        "expected a transition the clock owes {arn:?}: either no connector is stored under \
         that ARN or it has already settled"
    );
}

/// Keep the transition the clock owes `arn` undue for the rest of the case, so every call in
/// it observes the transient status.
///
/// Requires a transition to be owed; use [`hold_pending_if_owed`] where a case tolerates
/// there being none.
pub fn hold_pending(engine: &engine::InMemoryEngine, arn: &str) {
    assert!(
        hold_pending_if_owed(engine, arn),
        "expected a transition the clock owes {arn:?}: either no connector is stored under \
         that ARN or it has already settled"
    );
}

/// [`release_pending`] for a case that tolerates nothing being owed any more, answering
/// whether a transition was moved — so such a case can skip an assertion about a transient
/// state instead of racing the service's own transition span.
pub fn release_pending_if_owed(engine: &engine::InMemoryEngine, arn: &str) -> bool {
    set_pending_due(engine, arn, SystemTime::now() - ALREADY_DUE)
}

/// [`hold_pending`] for a case that tolerates nothing being owed any more, answering whether
/// a transition was moved.
pub fn hold_pending_if_owed(engine: &engine::InMemoryEngine, arn: &str) -> bool {
    set_pending_due(engine, arn, SystemTime::now() + NOT_DUE_FOR)
}
