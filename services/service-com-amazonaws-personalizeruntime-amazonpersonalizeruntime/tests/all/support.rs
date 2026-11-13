// SPDX-License-Identifier: CC0-1.0

//! Shared harness for this crate's suites: one copy of the router, the request
//! builder, the response readers and the seeded registry.
//!
//! Declared in `tests/all.rs` like any other suite (the layout rule requires every
//! file under `tests/all/` to be declared), so the sibling suites reach it as
//! `crate::support`. It holds no tests of its own.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] — the same entry the
//! production router serves — as a `restJson1` `POST` to the operation's modelled
//! route, so these tests exercise routing, deserialization, the handler and
//! serialization together.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime as runtime;
use runtime::state::{Action, Item};

/// The endpoint host an Amazon Personalize Runtime caller addresses
/// (`endpointPrefix: personalize-runtime`).
pub const HOST: &str = "personalize-runtime.us-east-1.amazonaws.com";

/// The account every suite's campaigns, recommenders and filters live in: the one this
/// emulator serves.
///
/// A resource ARN of any OTHER account is a CROSS-ACCOUNT reference, which the
/// authorization stage refuses with HTTP 403 `AccessDeniedException` before anything is
/// resolved (see `identity_and_account_authorization.rs`, which owns that rule) — so a suite
/// about resolution, scoring, filtering or refusal wording addresses this account.
pub const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`personalize`) — the routing signal the router reads first. Request
/// authentication is off in tests, so the signature itself is never verified.
const SIGV4_HEADER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/personalize/aws4_request, \
     SignedHeaders=host, Signature=0";

/// `GetRecommendations`: `POST /recommendations`.
pub const RECOMMENDATIONS: &str = "/recommendations";
/// `GetPersonalizedRanking`: `POST /personalize-ranking`.
pub const PERSONALIZE_RANKING: &str = "/personalize-ranking";
/// `GetActionRecommendations`: `POST /action-recommendations`.
pub const ACTION_RECOMMENDATIONS: &str = "/action-recommendations";

/// The Items-dataset column names the seeded catalogs carry, spelled the way a
/// Personalize Items dataset spells a column.
pub const COLUMN_GENRE: &str = "GENRE";
/// The second seeded column.
pub const COLUMN_YEAR: &str = "YEAR";

/// The `GENRE` values the seeded Items catalog cycles through.
pub const GENRES: [&str; 4] = ["Comedy", "Drama", "Action", "Documentary"];

/// The `CHANNEL` values the seeded Actions catalog cycles through.
pub const CHANNELS: [&str; 2] = ["App", "Email"];

/// One emulator: a single shared engine with this one service mounted on it.
pub struct Emulator {
    pub engine: Arc<InMemoryEngine>,
}

impl Emulator {
    /// A fresh emulator whose registry holds nothing at all.
    pub fn new() -> Self {
        Self {
            engine: Arc::new(InMemoryEngine::new()),
        }
    }

    /// A router serving only this service over the shared engine. Built per call on
    /// purpose: every router shares the one engine, so anything a later call observes
    /// came from engine state and not from a handle held across calls.
    fn router(&self) -> MultiServiceRouter {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(runtime::EmulatedService::new(Arc::clone(&self.engine)));
        router
    }

    /// Dispatch one request and return the raw HTTP response, headers included.
    async fn dispatch(&self, route: &str, body: &Value) -> ::http::Response<Bytes> {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}{route}"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("authorization", SIGV4_HEADER)
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        self.router().dispatch(request).await
    }

    /// Dispatch one request and return `(http status, parsed body)`.
    pub async fn call(&self, route: &str, body: &Value) -> (u16, Value) {
        let response = self.dispatch(route, body).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Dispatch one request that must be refused, and return everything a client keys
    /// its error handling off: the status, the `x-amzn-errortype` header restJson1
    /// resolves the error SHAPE from, the body's `__type` and its `message`.
    pub async fn refusal(&self, route: &str, body: Value) -> Refusal {
        let response = self.dispatch(route, &body).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            // AWS appends a Coral namespace after a colon; a client reads the shape
            // name in front of it.
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_ne!(
            status, 200,
            "case {route} {body}: expected a failure, got HTTP 200 with {parsed}"
        );
        Refusal {
            status,
            errortype,
            code: error_code(&parsed),
            message: error_message(&parsed),
        }
    }

    /// Dispatch one request that must succeed, and return its body.
    pub async fn ok(&self, route: &str, body: Value) -> Value {
        let (status, parsed) = self.call(route, &body).await;
        assert_eq!(
            status, 200,
            "case {route} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Dispatch one request that must fail, and return `(status, error code)`.
    pub async fn err(&self, route: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.call(route, &body).await;
        assert_ne!(
            status, 200,
            "case {route} {body}: expected a failure, got HTTP 200 with {parsed}"
        );
        (status, error_code(&parsed))
    }
}

/// The `__type` discriminator of a restJson1 error body (`{"__type":"<Code>", …}`),
/// without its namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a restJson1 error body (the modelled member of every
/// exception this service declares).
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// One refused request, as a client sees it.
#[derive(Debug)]
pub struct Refusal {
    /// The HTTP status.
    pub status: u16,
    /// The error shape named by `x-amzn-errortype`, without any Coral namespace.
    pub errortype: String,
    /// The error shape named by the body's `__type`, without its namespace.
    pub code: String,
    /// The body's `message`.
    pub message: String,
}

/// A deterministic Items catalog of `size` entries.
///
/// `base_score` decreases with the index, so the stored popularity order is known to
/// the test; `GENRE` and `YEAR` cycle, so a filter expression over either selects a
/// known subset.
pub fn items(size: usize) -> Vec<Item> {
    (0..size)
        .map(|index| {
            Item::new(format!("item-{index}"), (size - index) as f64)
                .with_attribute(COLUMN_GENRE, GENRES[index % GENRES.len()])
                .with_attribute(COLUMN_YEAR, (1990 + (index % 30)).to_string())
        })
        .collect()
}

/// A deterministic Actions catalog of `size` entries.
pub fn actions(size: usize) -> Vec<Action> {
    (0..size)
        .map(|index| {
            Action::new(format!("action-{index}"), (size - index) as f64)
                .with_attribute("CHANNEL", CHANNELS[index % CHANNELS.len()])
        })
        .collect()
}

/// The `itemId`s of an `itemList` / `personalizedRanking`, in response order.
pub fn ids(response: &Value, member: &str, id_member: &str) -> Vec<String> {
    response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response {response} carries no {member}"))
        .iter()
        .map(|entry| {
            entry
                .get(id_member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("entry {entry} carries no {id_member}"))
                .to_string()
        })
        .collect()
}

/// The `score`s of an `itemList` / `personalizedRanking` / `actionList`, in response
/// order.
pub fn scores(response: &Value, member: &str) -> Vec<f64> {
    response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response {response} carries no {member}"))
        .iter()
        .map(|entry| {
            entry
                .get("score")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("entry {entry} carries no score"))
        })
        .collect()
}

/// The `score` each entry of a list member reports, keyed by the entry's id — what a
/// caller comparing two responses over the same candidate set reads.
pub fn scores_by_id(response: &Value, member: &str, id_member: &str) -> BTreeMap<String, f64> {
    ids(response, member, id_member)
        .into_iter()
        .zip(scores(response, member))
        .collect()
}

/// One entry of a list member, as JSON.
pub fn entries(response: &Value, member: &str) -> Vec<Value> {
    response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response {response} carries no {member}"))
        .clone()
}

/// Assert the two properties every one of these operations' lists carries: the scores
/// never increase, and each is a share of the certainty spread over the WHOLE scored
/// candidate set, so each lies in `[0.0, 1.0]` and the reported list sums to at most
/// 1.0 (exactly 1.0 only when the response reports every candidate — see
/// [`assert_sums_to_one`]).
pub fn assert_descending_and_bounded(case: &str, scores: &[f64]) {
    for pair in scores.windows(2) {
        assert!(
            pair[0] >= pair[1],
            "case {case}: scores must not increase, got {scores:?}"
        );
    }
    for score in scores {
        assert!(
            (0.0..=1.0).contains(score),
            "case {case}: every score is a share of 1.0, got {score} in {scores:?}"
        );
    }
    let total: f64 = scores.iter().sum();
    assert!(
        total <= 1.0 + 1e-9,
        "case {case}: scores must sum to at most 1.0, got {total} from {scores:?}"
    );
}

/// [`assert_descending_and_bounded`], plus the sum a response that reports EVERY
/// scored candidate carries: exactly 1.0.
pub fn assert_sums_to_one(case: &str, scores: &[f64]) {
    assert_descending_and_bounded(case, scores);
    let total: f64 = scores.iter().sum();
    assert!(
        (total - 1.0).abs() < 1e-9,
        "case {case}: a response reporting every candidate must sum to 1.0, got {total} \
         from {scores:?}"
    );
}

/// The `recommendationId` of a response, which must never be empty.
pub fn recommendation_id(case: &str, response: &Value) -> String {
    let id = response
        .get("recommendationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case}: expected a non-empty recommendationId, got {response}"
    );
    id
}
