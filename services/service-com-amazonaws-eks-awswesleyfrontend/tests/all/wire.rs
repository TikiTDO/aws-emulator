// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through: one
//! shared engine behind the same `MultiServiceRouter` the production binary
//! mounts, plus the request/response plumbing an AWS SDK would do.
//!
//! It lives here rather than in each suite so there is ONE copy of it: a second
//! copy is a second place where "what a client sees" can drift.

use std::sync::Arc;

use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator, reduced to what a client branches on.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    /// The restJson1 error envelope's `__type`, absent on success.
    pub error_type: Option<String>,
    pub body: serde_json::Value,
}

impl Answer {
    /// The error `message` an answer carries, whichever casing the envelope spells it
    /// with. Empty on a success answer, which carries none.
    ///
    /// Here rather than in a suite, beside [`error_code`], so the suites that assert
    /// refusal WORDING all read the member the same way.
    pub fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }

    pub fn cluster(&self) -> &serde_json::Value {
        &self.body["cluster"]
    }

    pub fn association(&self) -> &serde_json::Value {
        &self.body["association"]
    }

    pub fn addon(&self) -> &serde_json::Value {
        &self.body["addon"]
    }
}

/// The emulator under test: one shared engine, one service, authentication off so a
/// test signs nothing (the credential scope below is still parsed for its region).
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with a handle on the engine behind it — what a suite needs when
/// it has to read or write the store directly (to stand up a precondition only an
/// operation that is still a 501 scaffold could otherwise write).
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Percent-encode a value that travels in ONE path segment, the way an AWS SDK
/// encodes an `@httpLabel`: `/` must not look like a segment separator.
///
/// Written out rather than pulled from `percent-encoding`, because a new
/// dependency means a new `Cargo.lock` entry and these tests deliberately leave
/// that file — the one every concurrent fix also edits — untouched. The rule is
/// the one an SDK applies to a non-greedy label: keep the unreserved set
/// (`A-Za-z0-9`, which is a subset of RFC 3986's, so this is a superset of what
/// must be escaped) and percent-encode every other byte; the server
/// percent-decodes the segment either way.
pub fn encode_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The error code a client matches on, with the `shape#` prefix a restJson1
/// envelope may carry stripped. Empty on a success answer, which carries no
/// error type at all.
pub fn error_code(answer: &Answer) -> String {
    answer
        .error_type
        .as_deref()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The access key id [`call`] signs with: a key neither STS nor IAM holds a record for,
/// which is the account's own published credential.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The emulator under test beside the two services a decision about WHO is calling reads
/// from: IAM (the user, the role and their identity policies) and STS (the session a role
/// is assumed into), all over the SAME shared engine.
///
/// Here rather than in a suite for the same reason the rest of this module is: a second
/// copy is a second place where "what a client sees" can drift.
pub fn emulator_with_identity() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// The text of the first `<name>…</name>` element of a response body — what reads one
/// value out of the awsQuery answer an IAM or STS setup call gives.
pub fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Answer {
    call_as(router, DEFAULT_ACCESS_KEY_ID, region, method, uri, body).await
}

/// [`call`], signed with `access_key_id` in the SigV4 credential scope — which is where
/// this emulator reads WHICH principal a request speaks for. One request builder for both,
/// so a suite that drives a particular caller cannot send a differently shaped request
/// from every other suite.
pub async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/eks/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.body();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
    };
    let error_type = error_type.or_else(|| {
        body.get("__type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    Answer {
        status,
        error_type,
        body,
    }
}
