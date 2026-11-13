// SPDX-License-Identifier: CC0-1.0

//! The EBS direct-API write loop a client drives THROUGH a refused block write, and the
//! snapshot's completion as Amazon EC2 reports it.
//!
//! RULE: the EBS direct APIs and EC2 describe ONE snapshot resource, and the EBS API models
//! no read of a snapshot's state at all, so `ec2:DescribeSnapshots` is the read through which
//! a client observes the lifecycle `ebs:StartSnapshot` begins: the id that call returns
//! resolves immediately as `pending`, it resolves as `completed` with `Progress` `100%` once
//! `CompleteSnapshot` seals it, `ec2:DeleteSnapshot` removes it, and only then is its id an
//! id the account holds no snapshot for (`InvalidSnapshot.NotFound`). A `PutSnapshotBlock`
//! whose payload is not the payload its `x-amz-Checksum` names fails the request's integrity
//! and stores NOTHING, so the sealed snapshot holds exactly the blocks whose writes were
//! accepted — which is what makes the write loop resumable: a client retries the refused
//! index, or writes another, and the snapshot it then completes is the one its accepted
//! writes built.
//!
//! This suite lives beside the router because the rule spans two services: the EBS crate's
//! own tests see no EC2 at all (so the snapshot's state is unobservable in them), and the
//! refusal, the completion and the delete are each decided by a different one of the two.
//! Both services are mounted on ONE engine here, as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: the volume size declared, WHICH
//! block index the refused write targets (before the accepted writes, between them, after
//! them, the same index one of them later writes, and no refused write at all), how MANY
//! indices are written and in which order, and the FORM the integrity failure takes (the
//! checksum of other bytes of the same length, a payload truncated in flight, and no payload
//! at all against a full block's checksum).

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The only block size the EBS direct APIs support: `PutSnapshotBlockRequest$DataLength`
/// documents "Currently, the only supported size is `524288` bytes".
const BLOCK_SIZE: i32 = 524_288;

/// The checksum algorithm every write below presents, the one value the modelled
/// `ChecksumAlgorithm` enum declares.
const CHECKSUM_ALGORITHM: &str = "SHA256";

/// A snapshot still being written: the `pending` member of EC2's `SnapshotState`.
const STATE_PENDING: &str = "pending";

/// A snapshot whose content is final: the `completed` member of EC2's `SnapshotState`.
const STATE_COMPLETED: &str = "completed";

/// `Snapshot$Progress` for a snapshot that has completed, as a percentage.
const PROGRESS_COMPLETED: &str = "100%";

/// EC2's error code for a snapshot id the account holds no snapshot for.
const SNAPSHOT_NOT_FOUND_CODE: &str = "InvalidSnapshot.NotFound";

/// The error code the EBS direct APIs answer a payload that is not the one its checksum
/// names with: the request's INTEGRITY failed, which is the signing class.
const INTEGRITY_ERROR_CODE: &str = "InvalidSignatureException";

/// The class the EBS direct APIs answer a read of a snapshot they hold none of with.
const EBS_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// How the integrity refusal states the two checksums: the one computed over the payload
/// that arrived, then the one the request presented for it. Written out here, so the wording
/// is pinned by this suite rather than by the code under test.
fn mismatch_message(computed: &str, supplied: &str) -> String {
    format!(
        "The value passed in as x-amz-Checksum does not match the computed checksum. \
         Computed checksum: {computed} expected checksum: {supplied}"
    )
}

/// EC2's own sentence for a snapshot id the account holds no snapshot for.
fn snapshot_absent_message(snapshot_id: &str) -> String {
    format!("The snapshot '{snapshot_id}' does not exist.")
}

/// Amazon EC2 and the EBS direct APIs over ONE shared engine, which is what makes the two
/// sides of a snapshot the same snapshot.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope,
    // which is what picks the service for a request.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ebs_ebs::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One EBS direct-API answer, reduced to what this suite asserts on.
struct Reply {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    /// The body parsed as JSON, or `Null` when it is not JSON.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, falling back to
    /// the body's `__type` discriminator.
    fn error_code(&self) -> String {
        let header = self.header("x-amzn-errortype");
        if !header.is_empty() {
            return header;
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A response header, or the empty string when it is absent.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The `message` member of a refusal body. The integrity class is not one of the EBS
    /// service's modelled shapes, so its message travels in the lower-case member every AWS
    /// framework error carries; a modelled refusal carries `Message`.
    fn message(&self) -> String {
        let body = self.json();
        for member in ["message", "Message"] {
            if let Some(text) = body.get(member).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        String::new()
    }

    /// The body as text, for a report that has to quote whatever came back.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One EBS direct-API request on its restJson1 wire.
async fn ebs(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> Reply {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ebs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ebs"));
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("a well-formed EBS direct-API request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: body.to_vec(),
    }
}

/// One EC2 action on its ec2Query wire, answered with `(status, the XML body)`.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed EC2 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// `ec2:DescribeSnapshots` narrowed to one snapshot id.
async fn describe(router: &MultiServiceRouter, snapshot_id: &str) -> (u16, String) {
    ec2(
        router,
        "DescribeSnapshots",
        &[("SnapshotId.1".to_string(), snapshot_id.to_string())],
    )
    .await
}

/// Base64 SHA-256, the form `Checksum` carries on the wire. Computed here independently of
/// the service, so the service's value is compared against a second implementation.
fn checksum(data: &[u8]) -> String {
    aws_smithy_types::base64::encode(Sha256::digest(data))
}

/// `BLOCK_SIZE` bytes that differ for every `seed`, so a block read back from the wrong
/// index cannot pass as the right one.
fn block_bytes(seed: u8) -> Vec<u8> {
    let length = usize::try_from(BLOCK_SIZE).expect("the block size fits a usize");
    (0..length)
        .map(|offset| {
            let mixed = (u64::try_from(offset).unwrap_or_default())
                .wrapping_mul(2_654_435_761)
                .wrapping_add(u64::from(seed).wrapping_mul(1_099_511_628_211));
            u8::try_from(mixed % 251).expect("a value below 251 fits a u8")
        })
        .collect()
}

/// The bytes block `index` of a row's snapshot carries: one distinct payload per index.
fn payload_for(index: i32) -> Vec<u8> {
    block_bytes(u8::try_from(index.rem_euclid(200)).unwrap_or_default())
}

/// How a request's payload and the checksum it presents for it disagree — the classes a
/// corrupted upload arrives as.
#[derive(Debug, Clone, Copy)]
enum Mismatch {
    /// The checksum of DIFFERENT bytes of the same length.
    OtherBytes,
    /// The checksum of the whole block against a payload truncated in flight.
    TruncatedInFlight,
    /// The checksum of a full block against no payload at all.
    EmptyPayload,
}

/// The `(payload, the checksum presented for it)` one mismatch class sends for `index`.
fn mismatched_write(kind: Mismatch, index: i32) -> (Vec<u8>, String) {
    let block = payload_for(index);
    match kind {
        Mismatch::OtherBytes => {
            let other = checksum(&payload_for(index + 1));
            (block, other)
        }
        Mismatch::TruncatedInFlight => {
            let supplied = checksum(&block);
            (block[..4096].to_vec(), supplied)
        }
        Mismatch::EmptyPayload => (Vec::new(), checksum(&block)),
    }
}

/// `ebs:StartSnapshot`, answering the id it minted.
async fn start_snapshot(router: &MultiServiceRouter, case: &str, volume_size: i64) -> String {
    let reply = ebs(
        router,
        "POST",
        "/snapshots",
        &[("content-type", "application/json".to_string())],
        serde_json::to_vec(&json!({ "VolumeSize": volume_size }))
            .expect("a serializable StartSnapshot body"),
    )
    .await;
    assert_eq!(
        reply.status,
        201,
        "case {case:?}: expected HTTP 201 from ebs:StartSnapshot, got {} with {}",
        reply.status,
        reply.text()
    );
    reply
        .json()
        .get("SnapshotId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: StartSnapshot reported no SnapshotId, got {}",
                reply.text()
            )
        })
        .to_string()
}

/// `ebs:PutSnapshotBlock` of `payload` at `index`, presenting `supplied` as its checksum and
/// declaring the full block length — the length is what the request DECLARED, which is not
/// what decides the integrity refusal.
async fn put_block(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    index: i32,
    payload: Vec<u8>,
    supplied: &str,
) -> Reply {
    ebs(
        router,
        "PUT",
        &format!("/snapshots/{snapshot_id}/blocks/{index}"),
        &[
            ("content-type", "application/octet-stream".to_string()),
            ("x-amz-Data-Length", BLOCK_SIZE.to_string()),
            ("x-amz-Checksum", supplied.to_string()),
            ("x-amz-Checksum-Algorithm", CHECKSUM_ALGORITHM.to_string()),
        ],
        payload,
    )
    .await
}

/// `ebs:CompleteSnapshot`, declaring `changed_blocks_count` writes.
async fn complete_snapshot(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    changed_blocks_count: usize,
) -> Reply {
    ebs(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &[(
            "x-amz-ChangedBlocksCount",
            changed_blocks_count.to_string(),
        )],
        Vec::new(),
    )
    .await
}

/// `ebs:ListSnapshotBlocks`.
async fn list_blocks(router: &MultiServiceRouter, snapshot_id: &str) -> Reply {
    ebs(
        router,
        "GET",
        &format!("/snapshots/{snapshot_id}/blocks"),
        &[],
        Vec::new(),
    )
    .await
}

/// The `BlockIndex` -> `BlockToken` map a listing reported.
fn listed_blocks(reply: &Reply) -> BTreeMap<i32, String> {
    reply
        .json()
        .get("Blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    let index = block.get("BlockIndex").and_then(Value::as_i64)?;
                    let token = block.get("BlockToken").and_then(Value::as_str)?;
                    Some((i32::try_from(index).unwrap_or(-1), token.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `ebs:GetSnapshotBlock` of the block `token` names.
async fn get_block(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    index: i32,
    token: &str,
) -> Reply {
    ebs(
        router,
        "GET",
        &format!(
            "/snapshots/{snapshot_id}/blocks/{index}?blockToken={}",
            form_encode(token)
        ),
        &[],
        Vec::new(),
    )
    .await
}

/// The first `name` element's text of an ec2Query answer, with the serializer's escaping
/// undone, or the empty string when the answer carries it empty or not at all — `Progress` is
/// answered with no text at all for a snapshot nothing has been transferred for, which is a
/// different answer from a percentage.
fn member(body: &str, name: &str) -> String {
    elements(body, name).into_iter().next().unwrap_or_default()
}

/// One row: the snapshot's declared size, the write that is refused, and the indices whose
/// writes are accepted, in the order the client makes them.
struct Row {
    case: &'static str,
    volume_size: i64,
    /// The index the refused write targets and how its payload and checksum disagree, or
    /// `None` for the positive control that writes only accepted blocks.
    refused: Option<(i32, Mismatch)>,
    written: &'static [i32],
}

/// The rows, differing in the declared volume size, where the refused write sits relative to
/// the accepted ones, how many indices are written and in which order, and the form the
/// integrity failure takes.
const ROWS: &[Row] = &[
    Row {
        case: "the refusal at index 0, the accepted write at index 1",
        volume_size: 1,
        refused: Some((0, Mismatch::OtherBytes)),
        written: &[1],
    },
    Row {
        case: "the refusal between two accepted writes",
        volume_size: 2,
        refused: Some((1, Mismatch::TruncatedInFlight)),
        written: &[0, 2],
    },
    Row {
        case: "the refusal after the accepted writes, at a higher index",
        volume_size: 4,
        refused: Some((9, Mismatch::EmptyPayload)),
        written: &[4, 3],
    },
    Row {
        case: "the refused index written correctly afterwards",
        volume_size: 1,
        refused: Some((5, Mismatch::OtherBytes)),
        written: &[5],
    },
    Row {
        case: "no refused write at all",
        volume_size: 8,
        refused: None,
        written: &[0, 1, 2],
    },
];

/// RULE: a client drives the whole write loop over both wires — the refused write stores
/// nothing, the accepted ones persist, the seal moves the snapshot EC2 reports to
/// `completed` with `Progress` `100%`, and the delete leaves its id naming nothing on either
/// side.
#[tokio::test]
async fn a_refused_block_write_leaves_a_snapshot_ec2_reports_completed_from_its_accepted_writes() {
    for row in ROWS {
        let case = row.case;
        let router = emulator();
        let snapshot_id = start_snapshot(&router, case, row.volume_size).await;

        // The id StartSnapshot returned resolves on the EC2 side at once, as `pending`.
        let (status, body) = describe(&router, &snapshot_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ec2:DescribeSnapshots must report the snapshot \
             ebs:StartSnapshot minted, got {status} with {body}"
        );
        assert_eq!(
            member(&body, "snapshotId"),
            snapshot_id,
            "case {case:?}: expected the started snapshot {snapshot_id}, got {body}"
        );
        assert_eq!(
            member(&body, "status"),
            STATE_PENDING,
            "case {case:?}: expected a started snapshot to be {STATE_PENDING}, got {:?} in \
             {body}",
            member(&body, "status")
        );
        assert_eq!(
            member(&body, "progress"),
            "",
            "case {case:?}: expected no progress on a snapshot nothing has been transferred \
             for, got {:?}",
            member(&body, "progress")
        );
        assert_eq!(
            member(&body, "volumeSize"),
            row.volume_size.to_string(),
            "case {case:?}: expected the declared volume size {}, got {:?}",
            row.volume_size,
            member(&body, "volumeSize")
        );

        // The write whose payload is not the payload its checksum names fails the request's
        // integrity, and stores nothing.
        if let Some((index, kind)) = row.refused {
            let (payload, supplied) = mismatched_write(kind, index);
            let reply = put_block(&router, &snapshot_id, index, payload.clone(), &supplied).await;
            assert_eq!(
                reply.status,
                403,
                "case {case:?}: a payload the checksum does not name must be refused 403 at \
                 index {index}, got {} with {}",
                reply.status,
                reply.text()
            );
            assert!(
                reply.error_code().ends_with(INTEGRITY_ERROR_CODE),
                "case {case:?}: expected the {INTEGRITY_ERROR_CODE} error code, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                mismatch_message(&checksum(&payload), &supplied),
                "case {case:?}: the refusal must state the checksum computed over the bytes \
                 that arrived and the one supplied, got {}",
                reply.text()
            );
        }

        // The writes whose payload their checksum names are accepted, and each reports the
        // checksum of the bytes it stored.
        for index in row.written {
            let payload = payload_for(*index);
            let expected = checksum(&payload);
            let reply = put_block(&router, &snapshot_id, *index, payload, &expected).await;
            assert_eq!(
                reply.status,
                201,
                "case {case:?}: the accepted write at index {index} must answer 201, got {} \
                 with {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.header("x-amz-Checksum"),
                expected,
                "case {case:?}: the write at index {index} must report the checksum of the \
                 stored bytes, got {:?}",
                reply.header("x-amz-Checksum")
            );
        }

        // The seal is taken for the writes the client made.
        let sealed = complete_snapshot(&router, &snapshot_id, row.written.len()).await;
        assert_eq!(
            sealed.status,
            202,
            "case {case:?}: ebs:CompleteSnapshot must be accepted with 202, got {} with {}",
            sealed.status,
            sealed.text()
        );

        // Which is the move the EC2 read observes.
        let (status, body) = describe(&router, &snapshot_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the sealed snapshot must still be reported, got {status} with \
             {body}"
        );
        assert_eq!(
            member(&body, "status"),
            STATE_COMPLETED,
            "case {case:?}: expected a sealed snapshot to be {STATE_COMPLETED}, got {:?} in \
             {body}",
            member(&body, "status")
        );
        assert_eq!(
            member(&body, "progress"),
            PROGRESS_COMPLETED,
            "case {case:?}: expected progress {PROGRESS_COMPLETED} once the snapshot is \
             sealed, got {:?}",
            member(&body, "progress")
        );
        assert!(
            !member(&body, "completionTime").is_empty(),
            "case {case:?}: a sealed snapshot must report when it was completed, got {body}"
        );

        // The sealed snapshot holds exactly the blocks whose writes were accepted: the
        // refused one left no residue at the index it targeted.
        let listing = list_blocks(&router, &snapshot_id).await;
        assert_eq!(
            listing.status,
            200,
            "case {case:?}: the sealed snapshot must be listable, got {} with {}",
            listing.status,
            listing.text()
        );
        let listed = listed_blocks(&listing);
        let mut expected_indices: Vec<i32> = row.written.to_vec();
        expected_indices.sort_unstable();
        let reported_indices: Vec<i32> = listed.keys().copied().collect();
        assert_eq!(
            reported_indices, expected_indices,
            "case {case:?}: expected the accepted writes {expected_indices:?} and nothing \
             else, got {reported_indices:?}"
        );

        // And each of them holds the bytes that were written to it, which the refused write
        // did not disturb.
        for (index, token) in &listed {
            let fetched = get_block(&router, &snapshot_id, *index, token).await;
            assert_eq!(
                fetched.status,
                200,
                "case {case:?}: the block at index {index} must be readable back, got {} with \
                 {}",
                fetched.status,
                fetched.text()
            );
            assert_eq!(
                fetched.body,
                payload_for(*index),
                "case {case:?}: the block at index {index} must hold the bytes its accepted \
                 write stored, got {} byte(s) whose checksum is {:?}",
                fetched.body.len(),
                checksum(&fetched.body)
            );
        }

        // The delete removes the one record the snapshot has, so neither side holds it
        // afterwards.
        let (status, body) = ec2(
            &router,
            "DeleteSnapshot",
            &[("SnapshotId".to_string(), snapshot_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ec2:DeleteSnapshot must delete the direct-API snapshot, got \
             {status} with {body}"
        );
        assert_eq!(
            member(&body, "return"),
            "true",
            "case {case:?}: expected the delete to report success, got {body}"
        );

        let (status, body) = describe(&router, &snapshot_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted snapshot must be refused, got {status} with {body}"
        );
        assert_eq!(
            member(&body, "Code"),
            SNAPSHOT_NOT_FOUND_CODE,
            "case {case:?}: expected {SNAPSHOT_NOT_FOUND_CODE} after the delete, got {body}"
        );
        assert_eq!(
            member(&body, "Message"),
            snapshot_absent_message(&snapshot_id),
            "case {case:?}: expected EC2's own sentence for an absent snapshot, got {body}"
        );

        let listing = list_blocks(&router, &snapshot_id).await;
        assert_eq!(
            listing.status,
            404,
            "case {case:?}: the EBS side must hold no snapshot either once it is deleted, \
             got {} with {}",
            listing.status,
            listing.text()
        );
        assert!(
            listing.error_code().ends_with(EBS_NOT_FOUND_CODE),
            "case {case:?}: expected the {EBS_NOT_FOUND_CODE} class from the EBS side, got \
             {:?}",
            listing.error_code()
        );
    }
}
