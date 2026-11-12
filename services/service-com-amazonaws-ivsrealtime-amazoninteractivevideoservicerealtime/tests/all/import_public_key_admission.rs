// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `ImportPublicKey` ADMITS, beside the round trip
//! `tests/all/public_keys.rs` pins.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws ivs-realtime` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The public-key store this operation grows is BOUNDED: while it holds as many keys as
//!    the service admits, a further import is refused with the modelled
//!    `ServiceQuotaExceededException` (HTTP 402) stating the resource the quota is on and
//!    the limit reached, reports no key, and persists nothing — so the keys already held
//!    are exactly as they were and the refused material is still unheld.
//! 2. An import's `tags` map defers to the AWS-wide tagging restrictions
//!    (`smithy_query(query="doc", shape="ImportPublicKeyRequest", member="tags")`), which
//!    reserve the `aws:` key namespace for AWS's own tags: an import naming a key in that
//!    namespace is refused with the modelled `ValidationException` (HTTP 400) in this
//!    service's member-refusal construction and persists nothing, while a key that merely
//!    RESEMBLES the namespace is an ordinary key — stored, reported and read back.

use std::sync::Arc;

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state::MAX_PUBLIC_KEYS;
use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every case here is signed for.
const REGION: &str = "us-east-1";

/// The resource a public-key quota refusal names, as this model's `PublicKeyArn` spells
/// the resource type (`arn:aws:ivs:<region>:<account>:public-key/<id>`, from
/// `smithy_query(query="shape", shape="PublicKeyArn")`).
const PUBLIC_KEY_RESOURCE: &str = "public-key";

/// The tag-key prefix AWS reserves for its own tags, which the tagging restrictions the
/// `tags` member defers to put out of a caller's reach.
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
///
/// Each suite under `tests/all/` is its own private module, so this is the per-suite copy
/// of that two-line harness rather than a shared one.
struct RealTime {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1 `__type`
/// discriminator), the stated message and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    message: String,
    body: Value,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK does: `POST /<operation>`, signed for [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code: text(&parsed, "__type"),
            message: text(&parsed, "message"),
            body: parsed,
        }
    }

    /// Call `operation` and require it to have succeeded.
    async fn ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} ({operation}): expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }
}

/// A string member, read as the empty string when absent — which is what a client sees for
/// a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A freshly generated public key, as its DER `SubjectPublicKeyInfo`.
///
/// Generated rather than hard-coded, so what the cases import is a real public key of the
/// named algorithm.
fn public_key_der(algorithm: &'static rcgen::SignatureAlgorithm) -> Vec<u8> {
    KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der()
}

/// A fresh key's material as the PEM public-key block `openssl ... -pubout` writes.
fn fresh_material(algorithm: &'static rcgen::SignatureAlgorithm) -> String {
    pem::encode(&pem::Pem::new(
        "PUBLIC KEY",
        public_key_der(algorithm),
    ))
    .replace("\r\n", "\n")
}

/// How this service states a quota refusal: it names the resource the quota is on and the
/// limit that was reached.
fn quota_refusal(resource: &str, limit: usize) -> String {
    format!("Resource: {resource} quota exceeded limit: {limit}")
}

/// How this service states a request naming tag keys in the reserved namespace: the
/// construction every other member refusal of this service uses — the operation, the
/// member the keys arrived in, the requirement, then the offending keys in sorted order.
fn reserved_refusal(operation: &str, member: &str, keys: &[&str]) -> String {
    format!(
        "{operation}.{member} is required to name no key in the reserved \
         {RESERVED_TAG_KEY_PREFIX} namespace, found {}",
        keys.join(", ")
    )
}

/// Rule 1: the store is bounded, and an import past the bound is the modelled quota
/// refusal that persists nothing.
///
/// The rows past the bound differ along what the rule generalises over — the algorithm of
/// the key offered and which optional members accompany it — because none of that changes
/// the answer once the store is full.
#[tokio::test]
async fn importing_past_the_store_bound_is_the_modeled_quota_refusal() {
    let realtime = RealTime::new();

    // Fill the store to the bound, cycling the shapes an import can carry so the bound is
    // reached with keys of every form rather than one request repeated. Three of the keys
    // are kept, with what the import reported, to check the refusals below leave them be.
    let samples = [0usize, MAX_PUBLIC_KEYS / 2, MAX_PUBLIC_KEYS - 1];
    let mut held: Vec<(usize, Value, String)> = Vec::with_capacity(samples.len());
    for index in 0..MAX_PUBLIC_KEYS {
        let material = fresh_material(&PKCS_ECDSA_P256_SHA256);
        let mut request = json!({ "publicKeyMaterial": material });
        match index % 3 {
            0 => {}
            1 => request["name"] = json!(format!("fill-{index}")),
            _ => {
                request["name"] = json!(format!("fill-{index}"));
                request["tags"] = json!({ "index": index.to_string() });
            }
        }
        let case = format!("key {index} of {MAX_PUBLIC_KEYS}");
        let imported = realtime.ok(&case, "ImportPublicKey", request.clone()).await;
        if samples.contains(&index) {
            held.push((
                index,
                imported["publicKey"].clone(),
                text(&request, "publicKeyMaterial"),
            ));
        }
    }

    let expected = quota_refusal(PUBLIC_KEY_RESOURCE, MAX_PUBLIC_KEYS);
    let rows: [(&str, &'static rcgen::SignatureAlgorithm, Value); 4] = [
        (
            "only the required material",
            &PKCS_ECDSA_P256_SHA256,
            json!({}),
        ),
        (
            "a named import",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "past-the-bound" }),
        ),
        (
            "a named import carrying tags",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "past-the-bound", "tags": { "env": "test" } }),
        ),
        (
            "an import of a key of another curve, with every optional member omitted",
            &PKCS_ECDSA_P384_SHA384,
            json!({}),
        ),
    ];

    for (case, algorithm, base) in rows {
        let mut request = base.clone();
        request["publicKeyMaterial"] = json!(fresh_material(algorithm));

        let reply = realtime.call("ImportPublicKey", &request).await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.message.as_str()
            ),
            (402, "ServiceQuotaExceededException", expected.as_str()),
            "case {case:?}: an import past the bound of {MAX_PUBLIC_KEYS} must state \
             {expected:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert!(
            reply.body.get("publicKey").is_none(),
            "case {case:?}: a refused import must report no key, got {}",
            reply.body
        );

        // The refusal persisted NOTHING: the same material offered again meets the bound
        // again, where material the refused call had stored would be a conflict instead.
        let again = realtime.call("ImportPublicKey", &request).await;
        assert_eq!(
            (
                again.status,
                again.error_code.as_str(),
                again.message.as_str()
            ),
            (402, "ServiceQuotaExceededException", expected.as_str()),
            "case {case:?}: the refused material must still be unheld, so a second offer \
             of it must state {expected:?}, got {} {} {}",
            again.status,
            again.error_code,
            again.body
        );
    }

    // And every key the fill persisted is exactly as the import reported it, so the
    // refusals changed nothing the store already held.
    for (index, public_key, material) in &held {
        let case = format!("key {index} after the refusals");
        let arn = text(public_key, "arn");
        let read = realtime
            .ok(&case, "GetPublicKey", json!({ "arn": arn }))
            .await;
        assert_eq!(
            &read["publicKey"], public_key,
            "case {case:?}: the read must answer the key as imported, expected \
             {public_key}, got {read}"
        );

        // A key the store still holds is still a conflicting import — which is the answer
        // reached before the bound is even consulted, so a held key is held.
        let conflict = realtime
            .call(
                "ImportPublicKey",
                &json!({ "publicKeyMaterial": material.clone() }),
            )
            .await;
        assert_eq!(
            (conflict.status, conflict.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: re-importing held material must be the conflict, got {} {}",
            conflict.status,
            conflict.body
        );
    }
}

/// Rule 2: an import naming a tag key in the reserved namespace is refused, and leaves no
/// key behind.
///
/// The rows differ along what the rule generalises over: the CASING of the prefix, whether
/// the key carries a qualifier after it (the form AWS's own tags take) or nothing at all,
/// how many reserved keys one request carries, and whether ordinary keys travel beside
/// them.
#[tokio::test]
async fn an_import_naming_a_reserved_tag_key_is_refused_and_persists_nothing() {
    let realtime = RealTime::new();

    // (case, the tags the import carries, the offending keys the refusal must name)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a reserved key",
            json!({ "aws:owner": "platform" }),
            vec!["aws:owner"],
        ),
        (
            "a qualified reserved key, the form AWS's own tags take",
            json!({ "aws:cloudformation:stack-name": "prod" }),
            vec!["aws:cloudformation:stack-name"],
        ),
        (
            "the reserved prefix and nothing after it",
            json!({ "aws:": "x" }),
            vec!["aws:"],
        ),
        (
            "the reserved prefix in upper case",
            json!({ "AWS:owner": "platform" }),
            vec!["AWS:owner"],
        ),
        (
            "the reserved prefix in mixed case",
            json!({ "Aws:owner": "platform" }),
            vec!["Aws:owner"],
        ),
        (
            "a reserved key beside an ordinary one",
            json!({ "aws:owner": "platform", "team": "video" }),
            vec!["aws:owner"],
        ),
        (
            "two reserved keys at once",
            json!({ "aws:b": "2", "aws:a": "1" }),
            vec!["aws:a", "aws:b"],
        ),
    ];

    for (case, tags, offending) in rows {
        // A fresh key per row, so what the row proves about the refusal is not confounded
        // with the conflict a repeated material would answer.
        let material = fresh_material(&PKCS_ECDSA_P256_SHA256);
        let mut request = json!({ "publicKeyMaterial": material, "name": "reserved-tags" });
        request["tags"] = tags;

        let reply = realtime.call("ImportPublicKey", &request).await;
        let expected = reserved_refusal("ImportPublicKey", "tags", &offending);
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.message.as_str()
            ),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected the stated refusal {expected:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert!(
            reply.body.get("publicKey").is_none(),
            "case {case:?}: a refused import must report no key, got {}",
            reply.body
        );

        // The refusal persisted nothing, so the same material is still importable — had
        // the refused call stored a key, this would be the conflict instead.
        let accepted = realtime
            .ok(
                case,
                "ImportPublicKey",
                json!({
                    "publicKeyMaterial": text(&request, "publicKeyMaterial"),
                    "name": "reserved-tags",
                    "tags": { "team": "video" },
                }),
            )
            .await;
        let arn = text(&accepted["publicKey"], "arn");
        let read = realtime
            .ok(case, "GetPublicKey", json!({ "arn": arn }))
            .await;
        assert_eq!(
            read["publicKey"], accepted["publicKey"],
            "case {case:?}: the key imported after the refusal must read back as imported, \
             expected {}, got {read}",
            accepted["publicKey"]
        );
        assert_eq!(
            read["publicKey"]["tags"],
            json!({ "team": "video" }),
            "case {case:?}: only the tags the accepted import carried may be stored, \
             got {read}"
        );
    }
}

/// The other side of rule 2: it is the NAMESPACE that is reserved, so a key that merely
/// resembles it is an ordinary key an import stores and a read answers.
///
/// The rows differ in HOW they resemble it: the letters with more after them, the letters
/// without the colon, the prefix after another segment, a key shorter than the prefix, and
/// a key whose first bytes are a multi-byte character (a `TagKey` carries `@length(1..128)`
/// and no `@pattern`, so it may be any UTF-8 text).
#[tokio::test]
async fn an_import_naming_a_key_that_only_resembles_the_reserved_namespace_is_stored() {
    let realtime = RealTime::new();

    for (case, key) in [
        ("the reserved letters followed by more", "awsome"),
        ("the reserved letters without the colon", "aws"),
        ("the reserved letters with another separator", "aws-owner"),
        ("the prefix after another segment", "team-aws:owner"),
        ("a key shorter than the prefix", "a"),
        ("a key starting with a multi-byte character", "ключ:owner"),
    ] {
        let material = fresh_material(&PKCS_ECDSA_P256_SHA256);
        let imported = realtime
            .ok(
                case,
                "ImportPublicKey",
                json!({
                    "publicKeyMaterial": material,
                    "tags": { key: "accepted" },
                }),
            )
            .await;
        let public_key = imported["publicKey"].clone();
        assert_eq!(
            public_key["tags"],
            json!({ key: "accepted" }),
            "case {case:?}: an ordinary key must be stored, got {public_key}"
        );

        let arn = text(&public_key, "arn");
        let read = realtime
            .ok(case, "GetPublicKey", json!({ "arn": arn }))
            .await;
        assert_eq!(
            read["publicKey"], public_key,
            "case {case:?}: the read must answer the key as imported, expected \
             {public_key}, got {read}"
        );
    }
}
