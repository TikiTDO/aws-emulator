// SPDX-License-Identifier: CC0-1.0

//! The ONE JSON wire harness this crate's suites answer requests over: a router holding
//! this service, one `restJson1` call, the reply as a client reads it, and the
//! percent-encoding of a query-string component.
//!
//! Declared in `tests/all.rs` beside the suites, so each suite reaches it as
//! `crate::json_wire`. Here rather than in one suite because every suite that drives this
//! service over HTTP builds the same router, signs the same way and reads the same three
//! things out of a reply — a second copy of that is a second thing to keep in step with
//! the wire.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call this harness builds is signed for and addressed at.
pub const HOME: &str = "us-east-1";

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// The `x-amzn-errortype` header, when the answer carried one.
    pub error_type: Option<String>,
    /// The response body, parsed, or `Value::Null` when there was none.
    pub body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    pub fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The `message` member of the error body, or an empty string when there is none.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `NextToken` the answer handed back, if any.
    pub fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `member` of every row of the list member `list`, in the order the answer
    /// reported them — how a listing's page is read whatever the list is called.
    pub fn listed(&self, list: &str, member: &str) -> Vec<String> {
        self.body
            .get(list)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get(member)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router.
pub struct Ses {
    /// The router every call is dispatched through, so a suite can mount a peer service
    /// beside this one.
    pub router: MultiServiceRouter,
}

impl Ses {
    /// A router holding only this service, over a fresh engine.
    pub fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These suites exercise routing and handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// One call: `method` and `path` as a client addresses them, with `body` as the
    /// request's JSON payload when there is one.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{HOME}.amazonaws.com{path}"))
            .header("host", format!("email.{HOME}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

/// Percent-encode one query-string component the way an SDK sends it: everything outside
/// RFC 3986's unreserved set escaped, so a cursor's `+`, `/`, `=` and `!` reach the
/// handler exactly as it was minted or as a caller sent it.
///
/// The `percent-encoding` crate does the encoding; no suite writes the escaping itself.
pub fn query_component(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}
