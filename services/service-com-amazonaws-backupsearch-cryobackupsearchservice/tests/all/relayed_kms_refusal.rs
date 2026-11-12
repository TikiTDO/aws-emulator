// SPDX-License-Identifier: CC0-1.0

//! How `StartSearchJob` reports the refusal it RELAYED from KMS when an `EncryptionKeyArn`
//! resolves to no key, driven over HTTP exactly as a client drives it.
//!
//! THE RULE UNDER TEST: a refusal this service answers because the service it relayed the
//! call to refused reports that service's own reason verbatim, followed by the envelope the
//! relaying SDK layer states around a failed call — `(Service: <the service that refused>,
//! Status Code: <the status it answered with>, Request ID: <that call's own id>) (SDK
//! Attempt Count: 1)`. So the reason is not re-worded, the service and the status a client
//! parses back out of the text are the ones that actually answered, and the request id is
//! per call.
//!
//! What the rule generalises over is the FORM of the unresolvable reference, because each
//! form draws a different sentence out of KMS: a value that is no key identifier at all,
//! each of the two key-id forms KMS documents, a key ARN of a key nothing created, a held
//! key's ARN at another Region, an alias ARN naming no alias, and an ARN of another
//! service's resource. Every row is judged against the sentence KMS's OWN endpoint answers
//! for the SAME identifier (`kms_refusal`, which drives `kms:ListResourceTags` — the
//! read-only operation of that service that reports a failed key lookup the same way the
//! relayed resolution does), so this suite restates none of KMS's wording and cannot drift
//! from it.
//!
//! The HTTP harness (`emulator`, `call`, `create_kms_key`) is the search-job suite's, and
//! the request is built by the `EncryptionKeyArn` suite's `start_under_key`: every suite
//! drives the same emulator the same way.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::encryption_key_resolution::start_under_key;
use crate::search_jobs::{call, create_kms_key, emulator};

/// The Region every call below is signed for, which is the endpoint the key references are
/// resolved at.
const REGION: &str = "us-west-2";

/// The literal segment that closes a relayed refusal: the attempts the relayed call took.
const ATTEMPT_SUFFIX: &str = ") (SDK Attempt Count: 1)";

/// What KMS ITSELF answers for the key identifier `key_id`: the HTTP status and the message
/// of the refusal, read from that service's own endpoint over the same emulator.
///
/// `ListResourceTags` is the operation asked, because it only READS the key the identifier
/// names and reports a lookup that found nothing — an ARN it cannot read as a key or alias
/// ARN included — exactly as the relayed key resolution does. That makes its answer the
/// oracle for both halves of the envelope: the sentence the relay must carry and the status
/// it must state.
async fn kms_refusal(emulator: &MultiServiceRouter, region: &str, key_id: &str) -> (u16, String) {
    let body = json!({"KeyId": key_id}).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "kms.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "TrentService.ListResourceTags")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/kms/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(body.into_bytes()))
        .expect("a well-formed request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        status, 200,
        "oracle: KMS must refuse the key identifier {key_id:?}, got HTTP 200 ({payload:?})"
    );
    assert!(
        !message.is_empty(),
        "oracle: KMS's refusal of {key_id:?} must carry a message, got {payload:?}"
    );
    (status, message)
}

/// The request id a relayed refusal states, taken out of `message` by the two literal
/// segments that bracket it, or `None` when `message` is not in the relayed form.
fn relayed_request_id(message: &str, reason: &str, status: u16) -> Option<String> {
    let prefix = format!("{reason} (Service: Kms, Status Code: {status}, Request ID: ");
    let id = message
        .strip_prefix(prefix.as_str())?
        .strip_suffix(ATTEMPT_SUFFIX)?;
    Some(id.to_string())
}

#[tokio::test]
async fn a_relayed_key_refusal_carries_the_downstream_reason_and_the_relay_envelope() {
    let emulator = emulator();
    let account = engine::emulator_account_id();
    // A key the store really holds, so the rows that name it at another Region, or name its
    // Region with another key id, are refused by resolution rather than by the text.
    let held = create_kms_key(&emulator, REGION).await;

    // (case, the value sent as EncryptionKeyArn). The rows differ in the FORM of the
    // reference, which is what decides the sentence KMS answers with: the two key-id forms
    // KMS documents ("Key ID: 1234abcd-12ab-34cd-56ef-1234567890ab",
    // `smithy_query(query="doc",shape="TagResourceRequest",member="KeyId")`, and
    // `mrk-1234abcd12ab34cd56ef1234567890ab`,
    // `smithy_query(query="doc",shape="ReplicateKeyRequest",member="KeyId")`), the ARN forms
    // of each, and values in no identifier form at all.
    let rows: Vec<(&str, String)> = vec![
        ("a value that is no key identifier at all", "not-an-arn-at-all".to_string()),
        (
            "a single-Region key id that names no key",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "a multi-Region key id that names no key",
            format!("mrk-{}", "a".repeat(32)),
        ),
        (
            "a well-formed key ARN of a key nothing created",
            format!("arn:aws:kms:{REGION}:{account}:key/11111111-2222-4333-8444-555555555555"),
        ),
        (
            // A KMS key is regional: the same key at another Region's endpoint is absent.
            "a held key's ARN at another Region",
            held.replace(REGION, "eu-west-1"),
        ),
        (
            "an alias ARN naming no alias",
            format!("arn:aws:kms:{REGION}:{account}:alias/nothing"),
        ),
        (
            "an ARN of another service's resource",
            "arn:aws:s3:::some-bucket".to_string(),
        ),
        (
            "a key id in the right shape but not hexadecimal",
            "1111111z-2222-3333-4444-555555555555".to_string(),
        ),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (case, key_arn) in &rows {
        // What KMS answers the same identifier with, which is what the relay must report.
        let (kms_status, kms_reason) = kms_refusal(&emulator, REGION, key_arn).await;

        let refused = start_under_key(&emulator, REGION, Some(key_arn)).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: a key reference that resolves to nothing must be 404, got {} \
             ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            refused.error_code
        );

        let message = refused.string("message");
        let id = relayed_request_id(&message, &kms_reason, kms_status);
        assert!(
            id.is_some(),
            "case {case:?}: expected KMS's own reason {kms_reason:?} followed by \
             \"(Service: Kms, Status Code: {kms_status}, Request ID: <id>){ATTEMPT_SUFFIX}\", \
             got {message:?}"
        );
        let id = id.unwrap_or_default();
        assert!(
            ::service_common::query_error_envelope::is_request_id(&id),
            "case {case:?}: the relayed call's request id must have the form AWS reports one \
             in, got {id:?} (message {message:?})"
        );
        // The reference the caller sent is still what the refusal names, unchanged.
        assert_eq!(
            refused.string("resourceId"),
            *key_arn,
            "case {case:?}: the refusal must name the submitted reference, got {:?}",
            refused.get("resourceId")
        );
        request_ids.push(id);
    }

    // Per CALL, not per value: no two of the refusals above named the same relayed call.
    let mut distinct = request_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "each relayed refusal states the request id of its own call, got {request_ids:?}"
    );
}

/// RULE, the other half: the envelope belongs to a RELAYED refusal only — the same call with
/// a key the store holds, and the same call naming no key at all, are accepted and report no
/// relay — and two refusals of the SAME reference differ in nothing but the request id.
#[tokio::test]
async fn only_a_relayed_refusal_carries_the_envelope_and_only_its_id_varies() {
    let emulator = emulator();
    let held = create_kms_key(&emulator, REGION).await;

    for (case, key_arn) in [("a held key", Some(held.as_str())), ("no key at all", None)] {
        let accepted = start_under_key(&emulator, REGION, key_arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the call must be accepted, got {} ({:?})",
            accepted.status, accepted.body
        );
        assert!(
            !accepted.string("SearchJobIdentifier").is_empty(),
            "case {case:?}: an accepted call must report the job it minted, got {:?}",
            accepted.body
        );
    }

    let unresolvable = "not-an-arn-at-all";
    let (kms_status, kms_reason) = kms_refusal(&emulator, REGION, unresolvable).await;
    let mut ids: Vec<String> = Vec::new();
    for attempt in 0..2 {
        let refused = start_under_key(&emulator, REGION, Some(unresolvable)).await;
        let message = refused.string("message");
        let id = relayed_request_id(&message, &kms_reason, kms_status);
        assert!(
            id.is_some(),
            "attempt {attempt}: expected the relayed form around {kms_reason:?}, got {message:?}"
        );
        ids.push(id.unwrap_or_default());
    }
    assert_ne!(
        ids.first(),
        ids.last(),
        "two refusals of the same reference are two relayed calls, so each states its own \
         request id, got {ids:?}"
    );

    // Nothing of the refused calls was created: the two accepted calls above are the only
    // jobs this emulator holds.
    let listed = call(&emulator, REGION, "GET", "/search-jobs", None).await;
    assert_eq!(
        listed.get("SearchJobs").as_array().map(Vec::len),
        Some(2),
        "a refused start must create no job, got {:?}",
        listed.body
    );
}
