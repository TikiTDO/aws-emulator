// SPDX-License-Identifier: CC0-1.0

//! Streaming an image layer into an upload, end to end over the wire.
//!
//! The rule under test: `UploadLayerPart` appends a part's bytes to the upload
//! `InitiateLayerUpload` opened exactly when the part's declared
//! `[partFirstByte, partLastByte]` range spans as many bytes as its `layerPartBlob`
//! carries AND begins where that upload's received bytes end — so an upload's content is
//! the concatenation of its accepted parts, byte for byte, and the answer reports the
//! last byte the registry now holds. A part that satisfies neither condition, or that
//! names an upload this registry did not issue for the repository in the request,
//! changes nothing.
//!
//! The shared harness installs real finite B2 state. Intermediate ACK assertions
//! read authoritative checkpoint offset/SHA; refusals also preserve physical
//! retained-byte accounting. Completed cases seal and stream-read the actual
//! bytes independently, not a fake Vec projection or checkpoint-only byte oracle.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, ok, refused, REGISTRY_ID};

/// The upload `upload_id`, as the store holds it.
async fn stored_upload(engine: &Arc<engine::InMemoryEngine>, upload_id: &str) -> state::LayerUpload {
    let mutation = state::payloads::mutation_guard(&**engine).await.unwrap();
    let mut projection = state::service_state(&**engine).unwrap().lock().uploads.get(upload_id).cloned()
        .unwrap_or_else(|| panic!("upload {upload_id:?} is not in the current projection"));
    projection.snapshot = crate::wire::payload_store(engine).lookup_upload(&state::payloads::registry_owner().unwrap(),projection.snapshot.id.as_str())
        .await.unwrap().expect("authoritative upload checkpoint");
    drop(mutation);
    projection
}

/// `len` layer bytes that depend on `seed`: a deterministic spread over the whole byte
/// range, so a part carries `0x00` and `0xff` like real compressed layer content and two
/// parts of one upload do not carry the same bytes.
fn layer_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 97 + 13) % 256).unwrap_or_default())
        .collect()
}

/// A `layerPartBlob` member: a blob travels base64-encoded on the awsJson1_1 wire, with
/// the codec `aws_smithy_types::base64` the wire serializer itself uses.
fn blob(bytes: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(bytes))
}

/// An `UploadLayerPart` request body.
fn part_request(
    repository_name: &str,
    upload_id: &str,
    part_first_byte: i64,
    part_last_byte: i64,
    bytes: &[u8],
) -> Value {
    json!({
        "repositoryName": repository_name,
        "uploadId": upload_id,
        "partFirstByte": part_first_byte,
        "partLastByte": part_last_byte,
        "layerPartBlob": blob(bytes),
    })
}

/// Create the repository `name` and open an upload against it, answering the upload id.
async fn given_upload(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
    let opened = ok(
        router,
        case,
        "InitiateLayerUpload",
        &json!({"repositoryName": name}),
    )
    .await;
    opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId"))
        .to_string()
}

/// Consecutive parts are appended in order, each answer reports the last byte the upload
/// now holds, and the upload's bytes end up being exactly the concatenation of the parts.
///
/// The rows differ along how the same layer is cut up — one part, many equal parts, parts
/// of unequal sizes, a single-byte part, a part of tens of kilobytes — which is what the
/// rule generalises over: the part boundaries a client picks are not part of the content.
#[tokio::test]
async fn consecutive_parts_concatenate_into_the_upload() {
    let cases: Vec<(&str, &str, Vec<usize>)> = vec![
        ("one single-byte part", "single-byte", vec![1]),
        ("one part carrying the whole layer", "one-part", vec![4096]),
        ("three equal parts", "equal-parts", vec![1024, 1024, 1024]),
        ("parts of unequal sizes", "uneven-parts", vec![7, 1, 4095, 13]),
        ("many small parts", "many-parts", vec![3; 32]),
        ("a part of tens of kilobytes", "big-part", vec![65_536, 17]),
    ];

    let (router, engine) = emulator_with_engine();
    for (case, repository_name, sizes) in cases {
        let upload_id = given_upload(&router, case, repository_name).await;
        let part_size = stored_upload(&engine, &upload_id).await.part_size;

        let mut expected: Vec<u8> = Vec::new();
        for (index, size) in sizes.iter().copied().enumerate() {
            let bytes = layer_bytes(index, size);
            let first = i64::try_from(expected.len()).expect("the test layer fits in an i64");
            let last = first + i64::try_from(size).expect("a test part fits in an i64") - 1;
            let answer = ok(
                &router,
                case,
                "UploadLayerPart",
                &part_request(repository_name, &upload_id, first, last, &bytes),
            )
            .await;
            expected.extend_from_slice(&bytes);

            assert_eq!(
                answer["lastByteReceived"].as_i64(),
                Some(last),
                "case {case:?} part {index}: lastByteReceived is the last byte of the \
                 accepted part, got {:?}",
                answer["lastByteReceived"]
            );
            assert_eq!(
                answer["registryId"].as_str(),
                Some(REGISTRY_ID),
                "case {case:?} part {index}: the answer names this registry, got {:?}",
                answer["registryId"]
            );
            assert_eq!(
                answer["repositoryName"].as_str(),
                Some(repository_name),
                "case {case:?} part {index}: the answer names the upload's repository, \
                 got {:?}",
                answer["repositoryName"]
            );
            assert_eq!(
                answer["uploadId"].as_str(),
                Some(upload_id.as_str()),
                "case {case:?} part {index}: the answer names the upload addressed, got {:?}",
                answer["uploadId"]
            );

            let stored = stored_upload(&engine, &upload_id).await;
            assert_eq!(stored.snapshot.checkpoint.offset() as usize,expected.len());
            assert_eq!(
                stored.snapshot.checkpoint.sha256().to_string(), state::content_digest(&expected),
                "case {case:?} part {index}: the upload holds the concatenation of its \
                 accepted parts, got {} bytes",
                stored.snapshot.checkpoint.offset() as usize
            );
            assert_eq!(
                stored.last_byte_received(),
                Some(last),
                "case {case:?} part {index}: the stored upload's last byte is the one \
                 reported, got {:?}",
                stored.last_byte_received()
            );
        }

        verify_physical_prefix(&engine,&upload_id,&expected).await;
        let stored = stored_upload(&engine, &upload_id).await;
        assert_eq!(
            stored.repository_name, repository_name,
            "case {case:?}: uploading parts does not move the upload to another \
             repository, got {:?}",
            stored.repository_name
        );
        assert_eq!(
            stored.part_size, part_size,
            "case {case:?}: uploading parts does not change the part size the client was \
             told, got {}",
            stored.part_size
        );
    }
}

/// A part that does not begin where the upload's received bytes end is refused with the
/// modelled `InvalidLayerPartException`, reports the last byte that IS valid, and leaves
/// the upload exactly as the last accepted part left it.
///
/// The rows differ along how the offset misses: a gap ahead, a replay of bytes already
/// received, a one-byte overlap, and an offset far past the end.
#[tokio::test]
async fn a_part_that_is_not_consecutive_is_refused() {
    let (router, engine) = emulator_with_engine();
    let repository_name = "noncontiguous";
    let upload_id = given_upload(&router, "non-consecutive parts", repository_name).await;

    // One accepted part, so the upload holds bytes 0..=9 and the next part must start at
    // byte 10.
    let accepted = layer_bytes(1, 10);
    ok(
        &router,
        "non-consecutive parts",
        "UploadLayerPart",
        &part_request(repository_name, &upload_id, 0, 9, &accepted),
    )
    .await;

    let cases: Vec<(&str, i64, i64, usize)> = vec![
        ("a gap of one byte", 11, 12, 2),
        ("a gap of many bytes", 1_000, 1_003, 4),
        ("a replay of the part already received", 0, 9, 10),
        ("a one-byte overlap", 9, 20, 12),
        ("a part starting one byte early", 5, 9, 5),
    ];

    for (case, first, last, size) in cases {
        let held_before = crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes;
        let body = refused(
            &router,
            case,
            "UploadLayerPart",
            &part_request(repository_name, &upload_id, first, last, &layer_bytes(2, size)),
            "InvalidLayerPartException",
        )
        .await;
        assert_eq!(
            body["lastValidByteReceived"].as_i64(),
            Some(9),
            "case {case:?}: the refusal reports the last byte that is still valid, got {:?}",
            body["lastValidByteReceived"]
        );
        assert_eq!(crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes,held_before,"refusal must not leave an unacknowledged tail");
        let stored = stored_upload(&engine, &upload_id).await;
        assert_eq!(stored.snapshot.checkpoint.offset() as usize,accepted.len());
        assert_eq!(
            stored.snapshot.checkpoint.sha256().to_string(), state::content_digest(&accepted),
            "case {case:?}: a refused part changes nothing, got {} bytes",
            stored.snapshot.checkpoint.offset() as usize
        );
    }

    // The same rule at the start of an upload: the first part must begin at byte 0, and
    // an upload that has received nothing has no last valid byte to report.
    let fresh = given_upload(&router, "first part offset", "firstpart").await;
    let held_before = crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes;
    let body = refused(
        &router,
        "the first part does not start at byte 0",
        "UploadLayerPart",
        &part_request("firstpart", &fresh, 1, 4, &layer_bytes(3, 4)),
        "InvalidLayerPartException",
    )
    .await;
    assert_eq!(crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes,held_before,"first-part refusal leaves no physical tail");
    assert_eq!(
        body["lastValidByteReceived"],
        Value::Null,
        "case {:?}: an upload that received nothing reports no last valid byte, got {:?}",
        "the first part does not start at byte 0",
        body["lastValidByteReceived"]
    );
    assert!(
        stored_upload(&engine, &fresh).await.snapshot.checkpoint.offset() == 0,
        "case {:?}: a refused first part leaves the upload empty",
        "the first part does not start at byte 0"
    );
    verify_physical_prefix(&engine,&upload_id,&accepted).await;
    verify_physical_prefix(&engine,&fresh,&[]).await;
}

/// A part whose declared range does not describe the blob it carries is refused with the
/// modelled `InvalidLayerPartException` and nothing is kept.
///
/// The rows differ along how range and payload disagree: a range longer than the blob, a
/// range shorter than it, an empty blob under a one-byte range, and a range that ends
/// before it begins.
#[tokio::test]
async fn a_part_whose_range_does_not_describe_its_blob_is_refused() {
    let (router, engine) = emulator_with_engine();
    let repository_name = "mismatched";
    let upload_id = given_upload(&router, "range mismatch", repository_name).await;

    let cases: Vec<(&str, i64, i64, usize)> = vec![
        ("a range longer than the blob", 0, 9, 5),
        ("a range shorter than the blob", 0, 3, 10),
        ("an empty blob under a one-byte range", 0, 0, 0),
        ("a two-byte range carrying three bytes", 0, 1, 3),
        ("a range that ends before it begins", 5, 4, 0),
    ];

    for (case, first, last, size) in cases {
        let held_before = crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes;
        refused(
            &router,
            case,
            "UploadLayerPart",
            &part_request(repository_name, &upload_id, first, last, &layer_bytes(4, size)),
            "InvalidLayerPartException",
        )
        .await;
        assert_eq!(crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes,held_before,"refusal must not leave an unacknowledged tail");
        let stored = stored_upload(&engine, &upload_id).await;
        assert!(
            stored.snapshot.checkpoint.offset() == 0,
            "case {case:?}: a refused part keeps no byte, got {} bytes",
            stored.snapshot.checkpoint.offset() as usize
        );
    }

    // And the part the mismatched ones describe correctly is accepted, so the refusals
    // above are about the disagreement and not about the upload being unusable.
    let answer = ok(
        &router,
        "the matching part",
        "UploadLayerPart",
        &part_request(repository_name, &upload_id, 0, 9, &layer_bytes(4, 10)),
    )
    .await;
    assert_eq!(
        answer["lastByteReceived"].as_i64(),
        Some(9),
        "case {:?}: the matching part is accepted, got {:?}",
        "the matching part",
        answer["lastByteReceived"]
    );
    verify_physical_prefix(&engine,&upload_id,&layer_bytes(4,10)).await;
}

/// A part is only accepted for an upload this registry issued for the repository the
/// request names: an unknown repository is `RepositoryNotFoundException`, an upload id
/// this registry never issued — or one issued for another repository — is
/// `UploadNotFoundException`, and another account's registry is
/// `InvalidParameterException`. None of them keeps a byte.
#[tokio::test]
async fn a_part_is_accepted_only_for_an_upload_of_the_named_repository() {
    let (router, engine) = emulator_with_engine();
    let mine = given_upload(&router, "upload scoping", "mine").await;
    let other = given_upload(&router, "upload scoping", "other").await;

    // A repository that was created and then deleted: its name resolves to nothing, like
    // one never created.
    let gone = given_upload(&router, "upload scoping", "goneupload").await;
    let gone_projection = stored_upload(&engine,&gone).await;
    ok(
        &router,
        "upload scoping",
        "DeleteRepository",
        &json!({"repositoryName": "goneupload"}),
    )
    .await;

    let bytes = layer_bytes(5, 8);
    // A well-formed `UploadId` (8-4-4-4-12 hex) this registry never handed out.
    let never_issued = "00000000-0000-0000-0000-000000000000";
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a repository that was never created",
            part_request("absent", never_issued, 0, 7, &bytes),
            400,
            "RepositoryNotFoundException",
        ),
        (
            "a repository that was deleted after its upload was opened",
            part_request("goneupload", &gone, 0, 7, &bytes),
            400,
            "RepositoryNotFoundException",
        ),
        (
            "an upload id this registry never issued",
            part_request("mine", never_issued, 0, 7, &bytes),
            400,
            "UploadNotFoundException",
        ),
        (
            "an upload id issued for another repository",
            part_request("mine", &other, 0, 7, &bytes),
            400,
            "UploadNotFoundException",
        ),
        (
            "another account's registry",
            json!({
                "registryId": "123456789012",
                "repositoryName": "mine",
                "uploadId": mine,
                "partFirstByte": 0,
                "partLastByte": 7,
                "layerPartBlob": blob(&bytes),
            }),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, request, status, code) in cases {
        let held_before = crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes;
        let answer = call(&router, "UploadLayerPart", &request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
        assert_eq!(crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes,held_before,"scope refusal leaves no physical tail");
        for (name, upload_id) in [("mine", &mine), ("other", &other), ("gone", &gone)] {
            let stored = if name == "gone" { gone_projection.clone() } else { stored_upload(&engine, upload_id).await };
            assert!(
                stored.snapshot.checkpoint.offset() == 0,
                "case {case:?}: a refused part keeps no byte in the {name:?} upload, got \
                 {} bytes",
                stored.snapshot.checkpoint.offset() as usize
            );
        }
    }

    // The part the refusals above were refused for is accepted when it addresses its own
    // upload in this registry.
    let answer = ok(
        &router,
        "the upload's own repository",
        "UploadLayerPart",
        &part_request("mine", &mine, 0, 7, &bytes),
    )
    .await;
    assert_eq!(
        answer["lastByteReceived"].as_i64(),
        Some(7),
        "case {:?}: the part is accepted for its own upload, got {:?}",
        "the upload's own repository",
        answer["lastByteReceived"]
    );
    assert_eq!(
        stored_upload(&engine, &mine).await.snapshot.checkpoint.sha256().to_string(),
        state::content_digest(&bytes),
        "case {:?}: the accepted part's bytes are the ones stored",
        "the upload's own repository"
    );
    verify_physical_prefix(&engine,&mine,&bytes).await;
}

/// One upload holds at most [`state::MAX_LAYER_UPLOAD_BYTES`] bytes: the part that fills
/// the cap is still accepted, and the part past it is refused with the
/// `LimitExceededException` this operation models, keeping none of its bytes.
///
/// The cap is read from the constant, so the boundary tested is wherever the
/// implementation puts it. The bytes below it are seeded straight into the store (the
/// suite holds the engine): what is under test is the boundary, not a hundred megabytes
/// of round trips.
#[tokio::test]
async fn the_part_past_the_upload_byte_cap_is_refused() {
    let (router, engine) = emulator_with_engine();
    let repository_name = "capped";
    let upload_id = given_upload(&router, "the upload byte cap", repository_name).await;

    // One byte short of the cap, so the next wire call is the one that fills it. The
    // capacity is reserved up front so appending the last byte reallocates nothing.
    let seeded = state::MAX_LAYER_UPLOAD_BYTES - 1;
    {
        let mutation = state::payloads::mutation_guard(&*engine).await.unwrap();
        let payloads = crate::wire::payload_store(&engine);
        let scope = { let state = state::service_state(&*engine).unwrap(); let state = state.lock(); state::payloads::repository_owner(state.repository(repository_name).unwrap()).unwrap() };
        let bound = state::payload_upload::lookup(&payloads,&scope,&upload_id).await.unwrap().unwrap();
        let mut stage = payloads.resume(&bound.snapshot.id,bound.snapshot.checkpoint.clone()).await.unwrap();
        let zeros = bytes::Bytes::from(vec![0u8;64*1024]);
        let mut offset = 0;
        while offset < seeded as u64 {
            let count = (seeded as u64-offset).min(zeros.len() as u64) as usize;
            offset = stage.append(offset,zeros.slice(..count)).await.unwrap();
        }
        mutation.checkpoint(stage,bound.protocol.encode().unwrap()).await.unwrap();
        mutation.reconcile(&*engine).await.unwrap();
    }
    let seeded = i64::try_from(seeded).expect("the cap fits in an i64");

    let last = ok(
        &router,
        "the part that fills the cap",
        "UploadLayerPart",
        &part_request(repository_name, &upload_id, seeded, seeded, &[0xab]),
    )
    .await;
    assert_eq!(
        last["lastByteReceived"].as_i64(),
        Some(seeded),
        "the part that fills the cap is accepted, got {:?}",
        last
    );
    assert_eq!(
        stored_upload(&engine, &upload_id).await.snapshot.checkpoint.offset() as usize,
        state::MAX_LAYER_UPLOAD_BYTES,
        "the upload holds every byte accepted up to the cap"
    );

    let held_before = crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes;
    let over = call(
        &router,
        "UploadLayerPart",
        &part_request(repository_name, &upload_id, seeded + 1, seeded + 1, &[0xcd]),
    )
    .await;
    assert_eq!(crate::wire::payload_store(&engine).inventory().await.unwrap().retained_bytes,held_before,"over-cap refusal leaves no physical tail");
    assert_eq!(
        over.status, 400,
        "the part past the cap is refused, got {} with {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "LimitExceededException",
        "expected LimitExceededException past {} bytes, got {:?}",
        state::MAX_LAYER_UPLOAD_BYTES,
        over.body
    );
    assert_eq!(
        stored_upload(&engine, &upload_id).await.snapshot.checkpoint.offset() as usize,
        state::MAX_LAYER_UPLOAD_BYTES,
        "a refused part keeps no byte, so the upload stays at the cap"
    );
    let mut reader = sealed_prefix_reader(&engine,&upload_id).await;
    let mut observed = 0usize;
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        for (index,byte) in chunk.iter().enumerate() {
            let expected = if observed+index == state::MAX_LAYER_UPLOAD_BYTES-1 { 0xab } else { 0 };
            assert_eq!(*byte,expected,"independent physical cap-prefix byte at {}",observed+index);
        }
        observed += chunk.len();
    }
    assert_eq!(observed,state::MAX_LAYER_UPLOAD_BYTES);
}


async fn sealed_prefix_reader(engine: &Arc<engine::InMemoryEngine>, id: &str) -> service_common::payload_store::PayloadReader {
    let projection = stored_upload(engine,id).await;
    let mutation = state::payloads::mutation_guard(&**engine).await.unwrap();
    let store = crate::wire::payload_store(engine);
    let stage = store.resume(&projection.snapshot.id,projection.snapshot.checkpoint.clone()).await.unwrap();
    let handle = mutation.seal(stage,service_common::payload_store::IntegrityExpectation {
        length:Some(projection.snapshot.checkpoint.offset()),sha256:Some(projection.snapshot.checkpoint.sha256()),
    }).await.unwrap();
    mutation.reconcile(&**engine).await.unwrap();
    store.open(&handle,None).await.unwrap()
}
async fn verify_physical_prefix(engine: &Arc<engine::InMemoryEngine>, id: &str, expected: &[u8]) {
    let mut reader = sealed_prefix_reader(engine,id).await;
    let mut offset = 0;
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        assert_eq!(&chunk[..],&expected[offset..offset+chunk.len()],"physical sealed bytes, not merely cached checkpoint metadata");
        offset += chunk.len();
    }
    assert_eq!(offset,expected.len());
}
