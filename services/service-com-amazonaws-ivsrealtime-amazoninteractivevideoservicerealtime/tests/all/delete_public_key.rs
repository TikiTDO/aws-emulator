// SPDX-License-Identifier: CC0-1.0

//! `DeletePublicKey` over the wire.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! a handler called directly. The import and read surface itself is covered by
//! `tests/all/public_keys.rs`; this suite is about the delete.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A DELETE REMOVES THE PERSISTED KEY. "Deletes the specified public key used to sign
//!    stage participant tokens": the delete answers the modelled empty success (HTTP 200),
//!    after which the ARN names nothing — `GetPublicKey` answers the modelled
//!    `ResourceNotFoundException` (HTTP 404), `ListPublicKeys` no longer reports it and the
//!    `/tags/{resourceArn}` surface no longer resolves it — and every OTHER key of the
//!    account is left exactly as it was.
//! 2. A DELETE OF AN ARN THIS REGION'S VIEW HOLDS NO KEY FOR IS REFUSED with the modelled
//!    `ResourceNotFoundException` (HTTP 404), stating `Resource: <the ARN the caller sent>
//!    not found`: one never imported, one already deleted (so a delete is not idempotently
//!    accepted a second time), and one imported through ANOTHER region, which stays
//!    readable and deletable through the region that holds it.
//! 3. THE DELETE RELEASES THE KEY MATERIAL. A key is held under the fingerprint of its
//!    material, so once it is deleted the very same material — whose re-import was refused
//!    as a conflict while the key was held — imports again, under a fresh ARN.

use std::sync::Arc;

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every case that does not vary it signs for.
const REGION: &str = "us-east-1";

/// A second region, for the rule that a key belongs to the region it was imported
/// through.
const OTHER_REGION: &str = "eu-west-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published maximum length of `PublicKeyName`, from
/// `smithy_query(query="shape",shape="PublicKeyName")` (`@length(0..128)`).
const MAX_NAME_LENGTH: usize = 128;

/// The wire error code this service's tagging surface answers for an ARN naming no
/// resource — the code those three operations were recorded answering, which is not the
/// `ResourceNotFoundException` the resource-specific reads answer.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
///
/// Each suite under `tests/all/` is its own private module, so this is this suite's copy
/// of that two-line harness rather than a shared one.
struct RealTime {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1 `__type`
/// discriminator) and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = text(&parsed, "__type");
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// Call `operation` as an SDK does: `POST /<operation>`, signed for `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> Reply {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] against [`REGION`], where most cases live.
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.call_in(REGION, operation, body).await
    }

    /// Import a key through `region`'s endpoint and answer the `publicKey` it reported.
    async fn import_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let reply = self.call_in(region, "ImportPublicKey", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ImportPublicKey in {region} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["publicKey"].clone()
    }

    /// `GetPublicKey` through `region`'s endpoint, without asserting the answer: this
    /// suite reads a key both while it is held and after it is gone.
    async fn read_in(&self, region: &str, arn: &str) -> Reply {
        self.call_in(region, "GetPublicKey", &json!({ "arn": arn }))
            .await
    }

    /// `DeletePublicKey` through `region`'s endpoint, without asserting the answer.
    async fn delete_in(&self, region: &str, arn: &str) -> Reply {
        self.call_in(region, "DeletePublicKey", &json!({ "arn": arn }))
            .await
    }

    /// Every ARN `ListPublicKeys` reports for `region`, in page order.
    async fn listed_in(&self, case: &str, region: &str) -> Vec<String> {
        let reply = self.call_in(region, "ListPublicKeys", &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListPublicKeys in {region} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["publicKeys"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| text(summary, "arn"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `ListTagsForResource` for `arn`, carrying the ARN in the URI the way a client does:
    /// percent-encoded into one path segment.
    async fn tags_of(&self, region: &str, arn: &str) -> Reply {
        self.send(
            region,
            "GET",
            &format!("/tags/{}", encode_path_label(arn)),
            None,
        )
        .await
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The not-found sentence this service states for an identifier it holds no resource for.
fn not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// A well-formed `PublicKeyArn` naming no imported key, so the request reaches the handler
/// rather than being refused by the modelled pattern.
fn unused_public_key_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:public-key/{id}")
}

/// A freshly generated public key as its DER `SubjectPublicKeyInfo`, so what the cases
/// import is a real public key of the named algorithm.
fn public_key_der(algorithm: &'static rcgen::SignatureAlgorithm) -> Vec<u8> {
    KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der()
}

/// `der` as the PEM `PUBLIC KEY` block `openssl ... -pubout` writes.
fn pem_of(der: &[u8]) -> String {
    pem::encode(&pem::Pem::new("PUBLIC KEY", der.to_vec())).replace("\r\n", "\n")
}

/// An import request for a fresh key of `algorithm`, with `extra`'s members beside the
/// material.
fn import_request(algorithm: &'static rcgen::SignatureAlgorithm, extra: &Value) -> Value {
    let mut request = extra.clone();
    request["publicKeyMaterial"] = json!(pem_of(&public_key_der(algorithm)));
    request
}

/// Assert `reply` is the modelled failure `status`/`code`, stating `message`.
fn assert_refusal(case: &str, what: &str, status: u16, code: &str, message: &str, reply: &Reply) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: {what} must answer HTTP {status}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code, code,
        "case {case:?}: {what} must be the modelled {code}, got {}",
        reply.body
    );
    assert_eq!(
        text(&reply.body, "message"),
        message,
        "case {case:?}: {what} must state {message:?}, got {}",
        reply.body
    );
}

/// RULE 1: the delete removes the key the request names, from every projection of it, and
/// leaves every other key of the account exactly as it was.
///
/// One row per KIND of key the import surface admits — the algorithm of the key, whether a
/// name is sent and how long it is, and how many tags accompany it — since the delete is
/// keyed by the ARN alone and so must not depend on any of them.
#[tokio::test]
async fn a_public_key_this_region_holds_is_deleted() {
    let cases: Vec<(&str, &'static rcgen::SignatureAlgorithm, Value)> = vec![
        (
            "ecdsa p-256, every optional member omitted",
            &PKCS_ECDSA_P256_SHA256,
            json!({}),
        ),
        (
            "ecdsa p-384, named, one tag",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "token-signing-key", "tags": { "env": "test" } }),
        ),
        (
            "ecdsa p-256, named, several tags",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "stage-key", "tags": { "env": "prod", "team": "video" } }),
        ),
        (
            "an explicitly empty name, which the modelled length 0..128 allows",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "" }),
        ),
        (
            "a name at the published maximum length",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "k".repeat(MAX_NAME_LENGTH) }),
        ),
        (
            "an explicitly empty tag map",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "tags": {} }),
        ),
    ];

    for (case, algorithm, extra) in cases {
        // One emulator per row, so each row decides against a store holding only its own
        // keys.
        let realtime = RealTime::new();
        let target = realtime
            .import_in(case, REGION, &import_request(algorithm, &extra))
            .await;
        let arn = text(&target, "arn");
        let other = realtime
            .import_in(
                case,
                REGION,
                &import_request(&PKCS_ECDSA_P256_SHA256, &json!({ "name": "left-alone" })),
            )
            .await;
        let other_arn = text(&other, "arn");

        let deleted = realtime.delete_in(REGION, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting {arn} must answer HTTP 200, got {} {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeletePublicKeyResponse declares no members, so the answer must \
             carry none, got {}",
            deleted.body
        );

        // The ARN now names nothing, on every surface that reported the key.
        let read = realtime.read_in(REGION, &arn).await;
        assert_refusal(
            case,
            "a read of the deleted key",
            404,
            "ResourceNotFoundException",
            &not_found(&arn),
            &read,
        );
        let listed = realtime.listed_in(case, REGION).await;
        assert_eq!(
            listed,
            vec![other_arn.clone()],
            "case {case:?}: the listing must report only the key that was left, got {listed:?}"
        );
        let tags = realtime.tags_of(REGION, &arn).await;
        assert_eq!(
            (tags.status, tags.error_code.as_str()),
            (404, TAGGING_NOT_FOUND_CODE),
            "case {case:?}: the tagging surface must resolve the deleted key to nothing, \
             got {} {}",
            tags.status,
            tags.body
        );

        // The other key of the account is untouched, member for member.
        let other_after = realtime.read_in(REGION, &other_arn).await;
        assert_eq!(
            other_after.status, 200,
            "case {case:?}: the other key must still read back, got {} {}",
            other_after.status, other_after.body
        );
        assert_eq!(
            other_after.body["publicKey"], other,
            "case {case:?}: the delete must leave {other_arn} exactly as it was, got {}",
            other_after.body
        );
    }
}

/// RULE 2: an ARN this region's view holds no key for is the modelled not-found, whichever
/// way it came to hold none.
#[tokio::test]
async fn a_delete_of_a_public_key_this_region_holds_none_of_is_refused() {
    // A key never imported at all, in a store that holds another key — so the refusal is
    // about the ARN, not about the store being empty.
    let realtime = RealTime::new();
    let held = realtime
        .import_in(
            "never imported",
            REGION,
            &import_request(&PKCS_ECDSA_P384_SHA384, &json!({ "name": "held" })),
        )
        .await;
    for (case, id) in [
        ("never imported", "neverImported"),
        ("an id of the maximum admitted alphabet", "aZ9-key"),
    ] {
        let absent = unused_public_key_arn(REGION, id);
        let reply = realtime.delete_in(REGION, &absent).await;
        assert_refusal(
            case,
            "the delete",
            404,
            "ResourceNotFoundException",
            &not_found(&absent),
            &reply,
        );
    }
    // The held key is still exactly what the store answers, so a refused delete removed
    // nothing.
    let still_there = realtime.read_in(REGION, &text(&held, "arn")).await;
    assert_eq!(
        still_there.body["publicKey"], held,
        "case \"never imported\": a refused delete must leave every key untouched, got {}",
        still_there.body
    );

    // A key already deleted: the second delete of the same ARN is refused, not accepted
    // again.
    let realtime = RealTime::new();
    let arn = text(
        &realtime
            .import_in(
                "already deleted",
                REGION,
                &import_request(&PKCS_ECDSA_P256_SHA256, &json!({})),
            )
            .await,
        "arn",
    );
    let first = realtime.delete_in(REGION, &arn).await;
    assert_eq!(
        first.status, 200,
        "case \"already deleted\": the first delete must answer HTTP 200, got {} {}",
        first.status, first.body
    );
    let second = realtime.delete_in(REGION, &arn).await;
    assert_refusal(
        "already deleted",
        "the second delete",
        404,
        "ResourceNotFoundException",
        &not_found(&arn),
        &second,
    );

    // A key of ANOTHER region: this region's endpoint holds none, and the key stays
    // readable — and deletable — through the endpoint that does hold it.
    let realtime = RealTime::new();
    let elsewhere = realtime
        .import_in(
            "another region",
            OTHER_REGION,
            &import_request(&PKCS_ECDSA_P384_SHA384, &json!({ "name": "elsewhere" })),
        )
        .await;
    let elsewhere_arn = text(&elsewhere, "arn");
    let refused = realtime.delete_in(REGION, &elsewhere_arn).await;
    assert_refusal(
        "another region",
        "a delete through the wrong region's endpoint",
        404,
        "ResourceNotFoundException",
        &not_found(&elsewhere_arn),
        &refused,
    );
    let read = realtime.read_in(OTHER_REGION, &elsewhere_arn).await;
    assert_eq!(
        read.body["publicKey"], elsewhere,
        "case \"another region\": the key must stay readable through the region that holds \
         it, got {}",
        read.body
    );
    let owned = realtime.delete_in(OTHER_REGION, &elsewhere_arn).await;
    assert_eq!(
        owned.status, 200,
        "case \"another region\": the region that holds the key must delete it, got {} {}",
        owned.status, owned.body
    );
    let gone = realtime.read_in(OTHER_REGION, &elsewhere_arn).await;
    assert_refusal(
        "another region",
        "a read after the owning region's delete",
        404,
        "ResourceNotFoundException",
        &not_found(&elsewhere_arn),
        &gone,
    );
}

/// RULE 3: the delete releases the key MATERIAL, so the same material imports again
/// afterwards — which it cannot do while the key is held.
///
/// One row per algorithm, since a key is held under the fingerprint of its material
/// whatever curve it is on.
#[tokio::test]
async fn deleting_a_public_key_releases_its_material_for_reimport() {
    for (case, algorithm) in [
        ("ecdsa p-256", &PKCS_ECDSA_P256_SHA256),
        ("ecdsa p-384", &PKCS_ECDSA_P384_SHA384),
    ] {
        let realtime = RealTime::new();
        let material = pem_of(&public_key_der(algorithm));
        let request = json!({ "publicKeyMaterial": material, "name": "reimported" });
        let first = realtime.import_in(case, REGION, &request).await;
        let arn = text(&first, "arn");
        let fingerprint = text(&first, "fingerprint");

        // While the key is held the same material is a conflict.
        let conflict = realtime.call("ImportPublicKey", &request).await;
        assert_eq!(
            (conflict.status, conflict.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: re-importing held material must be refused, got {} {}",
            conflict.status,
            conflict.body
        );

        let deleted = realtime.delete_in(REGION, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer HTTP 200, got {} {}",
            deleted.status, deleted.body
        );

        // With the key gone the material is free again, and the re-import is a NEW
        // resource: a fresh ARN carrying the same fingerprint of the same key.
        let second = realtime.import_in(case, REGION, &request).await;
        let second_arn = text(&second, "arn");
        assert_ne!(
            second_arn, arn,
            "case {case:?}: the re-import must mint a fresh ARN, got {second_arn}"
        );
        assert_eq!(
            text(&second, "fingerprint"),
            fingerprint,
            "case {case:?}: the same key must fingerprint to the same value, got {second}"
        );
        let listed = realtime.listed_in(case, REGION).await;
        assert_eq!(
            listed,
            vec![second_arn.clone()],
            "case {case:?}: the listing must report only the re-imported key, got {listed:?}"
        );
    }
}
