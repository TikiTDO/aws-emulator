// SPDX-License-Identifier: CC0-1.0

//! The fixture every suite in this crate sets its campaigns up with, and the request
//! plumbing the refusal suite drives its cases through.
//!
//! `CreateCampaign` mints a campaign only for an Amazon Connect instance the account
//! holds, and that registry is Amazon Connect's own store — not this service's — so a
//! suite that wants a campaign gives the account the instance first. One copy of that
//! step lives here, so a suite adds cases rather than another variant of it; it is the
//! one thing every suite of this crate takes from here. `campaign_lifecycle.rs` keeps
//! the host-style exchange harness it was written around and takes only the fixture.

use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_connect_amazonconnectservice::state as connect_state;
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState;
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A KMS key ARN, the form `EncryptionConfig$keyArn` takes (an `EncryptionKey`, len
/// 0..500, from `smithy_query(query="shape", shape="EncryptionConfig")`) — what a case
/// onboarding an instance with a customer key names.
pub const KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// A SigV4 `Authorization` header scoped to this service's signing name, which is what
/// routes a request to it, and to `region` — the region its ARNs are rooted in.
pub fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/connect-campaigns/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// The router with this service mounted, over a fresh engine, so each case starts from
/// an empty store and an empty instance registry.
pub fn router() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern, not this service's: these cases
    // assert handler behaviour, and the header above is a routing signal only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

/// The router under test together with the engine it was built over.
///
/// It DEREFS to the router, so a case drives its requests through it exactly as it
/// drives them through a bare router; keeping the engine beside it is what lets a case
/// reach a store in that engine which this service does not own — the Amazon Connect
/// instance registry every campaign create is resolved against.
pub struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Deref for Harness {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

impl Harness {
    /// [`register_connect_instance`] against the engine this harness was built over.
    pub fn register_connect_instance(&self, region: &str, instance: &str) {
        register_connect_instance(&self.engine, region, instance);
    }

    /// [`deregister_connect_instance`] against the engine this harness was built over.
    pub fn deregister_connect_instance(&self, instance: &str) -> bool {
        deregister_connect_instance(&self.engine, instance)
    }

    /// The engine this harness was built over, so a case can read the store a handler
    /// wrote — the only way to see a persisted value no implemented operation reports
    /// yet.
    pub fn engine(&self) -> &engine::InMemoryEngine {
        &self.engine
    }
}

/// One answer from the emulator: its status, the media type it announces, the wire
/// error code it carries when it is a refusal, and its parsed JSON body.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub media_type: String,
    pub error_code: String,
    pub body: Value,
}

impl Answer {
    /// The `message` member of a refusal, or `"<no message>"` when it carries none.
    pub fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// The `connectInstanceOnboardingJobStatus$status` an onboarding answer carries, or a
/// stand-in when it carries none — the one member every onboarding suite reads a job's
/// status out of.
pub fn job_status(answer: &Answer) -> &str {
    answer
        .body
        .pointer("/connectInstanceOnboardingJobStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("<no status>")
}

/// Put the clock past the instant `instance`'s onboarding job is due to have finished,
/// without waiting for it: the record carries that instant, so moving it is the same as
/// letting the clock reach it. Panics when the instance holds no record, because a case
/// that reaches for a point of a job it never started is a broken case.
pub fn reach_the_due_instant(harness: &Harness, instance: &str) {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let mut store = state.lock();
    let record = store
        .instance_onboarding
        .get_mut(instance)
        .unwrap_or_else(|| panic!("case {instance:?}: the start call stored no record"));
    record.succeeds_at = SystemTime::now() - Duration::from_secs(1);
}

/// Drive one request through the router and report what came back. A `body` of
/// `Value::Null` sends no body at all.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(serde_json::to_vec(body).expect("a test body always serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("host", format!("connect-campaigns.{region}.amazonaws.com"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    let error_code = header_code
        .or_else(|| {
            parsed
                .get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    Answer {
        status,
        media_type,
        error_code,
        body: parsed,
    }
}

/// Give the account a Connect instance with id `instance` in `region`, in Amazon
/// Connect's own instance registry inside `engine` — the registry a campaign create is
/// resolved against, so this is the fixture step that makes `instance` something a
/// campaign can be created for.
///
/// The record is written into Connect's store rather than created over `CreateInstance`,
/// because that call mints the id it stores and these cases name their own instances; it
/// is the record that call writes, already past the clock-owed activation instant so the
/// instance is active. Registering an id the registry already holds leaves the stored
/// record alone, so the step is cheap to repeat.
///
/// The write itself is the registration seam the Connect crate exposes
/// (`connect_state::register_instance`), the one statement of the record a registration
/// writes — so this suite and every other that registers an instance cannot drift on its
/// shape.
pub fn register_connect_instance(engine: &engine::InMemoryEngine, region: &str, instance: &str) {
    assert!(
        connect_state::register_instance(engine, region, instance),
        "instance {instance:?} in {region}: Amazon Connect's registry took no record"
    );
}

/// Take instance `instance` back out of the registry inside `engine`, reporting whether
/// a record was removed — the effect Connect's `DeleteInstance` has on the registry the
/// campaign create is gated on, which is how a case reaches an id the account no longer
/// holds.
pub fn deregister_connect_instance(engine: &engine::InMemoryEngine, instance: &str) -> bool {
    connect_state::deregister_instance(engine, instance)
}
