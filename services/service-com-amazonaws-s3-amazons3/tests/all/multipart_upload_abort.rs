// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DISCARD of the multipart upload loop: what
//! `AbortMultipartUpload` takes away stops being readable, and nothing else moves.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] already owns), so what is pinned is the wire behaviour —
//! status, body, error code and message — rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. An abort discards the ONE in-progress multipart upload its `(Bucket, Key, UploadId)`
//!    addresses, with every part it held — "After a multipart upload is aborted, no
//!    additional parts can be uploaded using that upload ID. The storage consumed by any
//!    previously uploaded parts will be freed" (`AbortMultipartUpload`'s own
//!    documentation) — and answers the 204 the model binds
//!    (`smithy_query(query="operation",operation="AbortMultipartUpload")` reports
//!    `http {method: DELETE, code: 204}`) with no body.
//! 2. The discard is SCOPED to that upload: every other upload the bucket has in progress
//!    stays listed by `ListMultipartUploads` and keeps its own parts, including another
//!    upload of the very same key, and an upload of the same key in another bucket.
//! 3. An id the discarded upload was addressed by names no upload in progress afterwards: a
//!    `ListParts` of it is refused 404 `NoSuchUpload` — "An in-progress multipart upload is a
//!    multipart upload that has been initiated by the CreateMultipartUpload request, but has
//!    not yet been completed or aborted" (`ListMultipartUploads`'s own documentation) — while
//!    a SECOND abort of that id answers the same 204 the first did.
//! 4. An abort is IDEMPOTENT at an address the bucket HELD an upload under: an id an earlier
//!    abort discarded, and an id a completion took away, are NO-OPs answered 204 with no
//!    body, changing nothing — the state such an abort asks for already holds.
//! 5. An id the bucket never had an upload in progress under addresses no upload of it at
//!    all and is refused 404 `NoSuchUpload`: an id nothing ever issued, an id issued for
//!    another key, and an id issued in another bucket. A bucket this emulator does not hold
//!    is 404 `NoSuchBucket`. The `NoSuchUpload` message is the FIXED sentence S3 sends for
//!    that error class, with no value from the request (the key, the id) spliced into it.
//! 6. A request this emulator cannot carry out as asked is refused rather than answered
//!    with a 204 that would claim the upload was discarded: a Requester Pays confirmation
//!    and the directory-bucket-only `x-amz-if-match-initiated-time` conditional with 501,
//!    and `x-amz-expected-bucket-owner` naming another account with 403 — and the upload
//!    each of those named is still in progress afterwards.
//!
//! The upload id is a value the SERVICE mints per initiation (`MultipartUploadId` is an
//! unconstrained string in the model), so nothing here asserts a literal id: what is
//! asserted is what the id the initiation answered addresses, and stops addressing.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, text};

/// The sentence S3 sends for the `NoSuchUpload` error class, in full. Fixed text: nothing
/// from the request or the store is substituted into it, which is rule 5 above.
const NO_SUCH_UPLOAD_MESSAGE: &str = "The specified upload does not exist. The upload ID \
                                      may be invalid, or the upload may have been aborted \
                                      or completed.";

/// The bytes a REQUEST path leaves literal: RFC 3986's unreserved set plus `/`, the segment
/// separator. A key reaches the URI through this (a raw space is not a legal URI
/// character) and the framework percent-decodes a path label, so what the handler sees is
/// the key itself — the same encoding [`super::multipart_upload_listing`] builds its paths
/// with.
const PATH_LITERAL: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `key` as a request path carries it.
fn in_path(key: &str) -> String {
    utf8_percent_encode(key, PATH_LITERAL).to_string()
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(router: &MultiServiceRouter, name: &str, key: &str) -> String {
    let initiated = send(
        router,
        "POST",
        &format!("/{name}/{}?uploads", in_path(key)),
        &[],
        "",
    )
    .await;
    assert_eq!(
        initiated.status,
        200,
        "the upload of {key:?} must be initiated first, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    initiated
        .element("UploadId")
        .unwrap_or_else(|| panic!("an initiation of {key:?} must report an UploadId"))
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`, so an aborted upload
/// has parts to free.
async fn upload_part(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    body: &str,
) {
    let stored = send(
        router,
        "PUT",
        &format!(
            "/{name}/{}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPart",
            in_path(key)
        ),
        &[],
        body,
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "part {part_number} of {key:?} must be recorded first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
}

/// `DELETE /<bucket>/<key>?uploadId=<id>&x-id=AbortMultipartUpload` —
/// `AbortMultipartUpload`. The `x-id` literal is the one the model binds this operation
/// under (`smithy_query(query="operation",operation="AbortMultipartUpload")` reports
/// `uri: /{Bucket}/{Key+}?x-id=AbortMultipartUpload`), which is what distinguishes it from
/// a delete of the object at the same address.
async fn abort(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!(
            "/{name}/{}?uploadId={upload_id}&x-id=AbortMultipartUpload",
            in_path(key)
        ),
        headers,
        "",
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`, the read-back of ONE
/// upload.
async fn list_parts(router: &MultiServiceRouter, name: &str, key: &str, upload_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/{name}/{}?uploadId={upload_id}&x-id=ListParts",
            in_path(key)
        ),
        &[],
        "",
    )
    .await
}

/// Every `(Key, UploadId)` pair `ListMultipartUploads` reports for the bucket, in the order
/// the listing reports them.
async fn uploads_in_progress(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let listed = send(router, "GET", &format!("/{name}?uploads"), &[], "").await;
    assert_eq!(
        listed.status,
        200,
        "the listing of {name:?} must succeed, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    let body = text(&listed);
    body.split("<Upload>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</Upload>").next())
        .map(|chunk| {
            (
                element_of(chunk, "Key").unwrap_or_default(),
                element_of(chunk, "UploadId").unwrap_or_default(),
            )
        })
        .collect()
}

/// How many `<Part>` elements a `ListParts` answer carries.
fn reported_parts(answer: &Answer) -> usize {
    text(answer).matches("<Part>").count()
}

/// The text of the first `<name>` element in `document`.
///
/// `pub(crate)`: the multipart notification suite
/// ([`super::multipart_upload_event_notifications`]) reads an initiation's upload id out of
/// its body through this one reader rather than slicing the document a second way.
pub(crate) fn element_of(document: &str, name: &str) -> Option<String> {
    document
        .split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(value, _)| value.to_string())
}

/// One row of the discard table: the case name, the key the upload is initiated for, and
/// the part numbers (with their bodies) uploaded into it before it is aborted.
struct AbortCase {
    case: &'static str,
    key: &'static str,
    parts: Vec<(i32, &'static str)>,
}

/// The discard table. The rows differ along what the rule generalises over — the FORM of
/// the key (a plain name, a nested path, a name with a space and one with non-ASCII
/// characters, a name that is a single character), how many parts the upload holds when it
/// is aborted (none, one, several, parts at both published bounds of the 1..=10,000 part
/// number range), and the SIZE of those parts (empty, a few bytes, a few kilobytes) — since
/// an abort frees whatever the upload held, whatever that was.
fn abort_cases() -> Vec<AbortCase> {
    vec![
        AbortCase {
            case: "an upload that holds no part at all",
            key: "empty-upload.bin",
            parts: Vec::new(),
        },
        AbortCase {
            case: "an upload holding one part",
            key: "one-part.bin",
            parts: vec![(1, "the only part")],
        },
        AbortCase {
            case: "an upload holding several parts, uploaded out of order",
            key: "nested/path/to/many-parts.bin",
            parts: vec![(7, "seventh"), (2, "second"), (4, "")],
        },
        AbortCase {
            case: "an upload holding parts at both published part-number bounds",
            key: "key with a space.bin",
            parts: vec![(1, "first"), (10_000, "last")],
        },
        AbortCase {
            case: "an upload of a non-ASCII key holding a kilobyte-sized part",
            key: "ключ/数据.bin",
            parts: vec![(3, "abcdefghij")],
        },
        AbortCase {
            case: "an upload of a single-character key",
            key: "k",
            parts: vec![(1, "x"), (2, "y")],
        },
    ]
}

#[tokio::test]
async fn an_abort_discards_the_upload_it_names_and_only_that_upload() {
    let router = router();
    let name = "multipart-abort-discard";
    bucket(&router, name).await;

    // One upload per row, plus a WITNESS upload that no row aborts: whatever each abort
    // takes away, the witness must still be listed with the part it holds, which is what
    // makes "only that upload" an assertion rather than an assumption.
    let witness_key = "witness/untouched.bin";
    let witness = initiate(&router, name, witness_key).await;
    upload_part(&router, name, witness_key, &witness, 1, "witness part").await;

    let cases = abort_cases();
    for case in &cases {
        let AbortCase { case, key, parts } = case;
        let upload_id = initiate(&router, name, key).await;
        for (part_number, body) in parts {
            upload_part(&router, name, key, &upload_id, *part_number, body).await;
        }

        // The upload is in progress before the abort, holding exactly the parts uploaded
        // into it — so what the abort takes away is known.
        let listed = uploads_in_progress(&router, name).await;
        assert!(
            listed.contains(&((*key).to_string(), upload_id.clone())),
            "case {case:?}: the upload must be listed before the abort, got {listed:?}"
        );
        let before = list_parts(&router, name, key, &upload_id).await;
        assert_eq!(
            reported_parts(&before),
            parts.len(),
            "case {case:?}: the upload must hold {} parts before the abort, got {:?}",
            parts.len(),
            text(&before)
        );

        // The abort itself: the modelled 204, with no body at all.
        let aborted = abort(&router, name, key, &upload_id, &[]).await;
        assert_eq!(
            aborted.status,
            204,
            "case {case:?}: expected HTTP 204, got {} with body {:?}",
            aborted.status,
            text(&aborted)
        );
        assert!(
            aborted.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&aborted)
        );

        // The upload is gone from the bucket-wide listing and from the read of itself, and
        // the id addresses nothing a second time.
        let listed = uploads_in_progress(&router, name).await;
        assert!(
            !listed.iter().any(|(_, id)| *id == upload_id),
            "case {case:?}: the aborted upload must not be listed, got {listed:?}"
        );
        let read_back = list_parts(&router, name, key, &upload_id).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a read of the aborted upload must be refused 404, got {} with \
             body {:?}",
            read_back.status,
            text(&read_back)
        );
        assert_eq!(
            read_back.error_code(),
            "NoSuchUpload",
            "case {case:?}: expected the wire error code \"NoSuchUpload\", got {:?}",
            read_back.error_code()
        );
        // The `NoSuchUpload` class carries the FIXED sentence S3 sends for it: the same text
        // whatever was read, with no value from the request (the key, the id) spliced in.
        assert_eq!(
            read_back.error_message(),
            NO_SUCH_UPLOAD_MESSAGE,
            "case {case:?}: expected the fixed NoSuchUpload sentence, got {:?}",
            read_back.error_message()
        );
        let again = abort(&router, name, key, &upload_id, &[]).await;
        assert_eq!(
            again.status, 204,
            "case {case:?}: a second abort of the same id is a no-op answered 204, got {} \
             with body {:?}",
            again.status,
            text(&again)
        );
        assert!(
            again.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&again)
        );

        // The witness upload is untouched by every row's abort: still listed, still
        // holding its own part.
        let listed = uploads_in_progress(&router, name).await;
        assert!(
            listed.contains(&(witness_key.to_string(), witness.clone())),
            "case {case:?}: the upload no row aborted must stay listed, got {listed:?}"
        );
        let witness_parts = list_parts(&router, name, witness_key, &witness).await;
        assert_eq!(
            witness_parts.status, 200,
            "case {case:?}: the witness upload must still read back, got {} with body {:?}",
            witness_parts.status,
            text(&witness_parts)
        );
        assert_eq!(
            reported_parts(&witness_parts),
            1,
            "case {case:?}: the witness upload must still hold its one part, got {:?}",
            text(&witness_parts)
        );
    }

    // Every row's upload is gone and the witness is the only upload left in progress.
    let listed = uploads_in_progress(&router, name).await;
    assert_eq!(
        listed,
        vec![(witness_key.to_string(), witness.clone())],
        "after aborting every row's upload, only the witness may be left, got {listed:?}"
    );
}

#[tokio::test]
async fn an_abort_leaves_another_upload_of_the_same_key_and_other_buckets_alone() {
    // Two uploads of ONE key are two uploads, each with its own id — "You specify this
    // upload ID in each of your subsequent upload part requests" — so an abort of one must
    // leave the other in progress with its own parts; and an id is scoped to the bucket it
    // was minted in, so the same key's upload in a second bucket is untouched too.
    let router = router();
    let first_bucket = "multipart-abort-scope-one";
    let second_bucket = "multipart-abort-scope-two";
    bucket(&router, first_bucket).await;
    bucket(&router, second_bucket).await;

    let key = "shared/key.bin";
    let aborted_upload = initiate(&router, first_bucket, key).await;
    let kept_upload = initiate(&router, first_bucket, key).await;
    let other_bucket_upload = initiate(&router, second_bucket, key).await;
    upload_part(&router, first_bucket, key, &aborted_upload, 1, "going").await;
    upload_part(&router, first_bucket, key, &kept_upload, 1, "staying").await;
    upload_part(&router, first_bucket, key, &kept_upload, 2, "also staying").await;
    upload_part(&router, second_bucket, key, &other_bucket_upload, 1, "peer").await;

    let discarded = abort(&router, first_bucket, key, &aborted_upload, &[]).await;
    assert_eq!(
        discarded.status,
        204,
        "case \"one of two uploads of one key\": expected HTTP 204, got {} with body {:?}",
        discarded.status,
        text(&discarded)
    );

    let listed = uploads_in_progress(&router, first_bucket).await;
    assert_eq!(
        listed,
        vec![(key.to_string(), kept_upload.clone())],
        "case \"one of two uploads of one key\": only the other upload may be left, got \
         {listed:?}"
    );
    let kept = list_parts(&router, first_bucket, key, &kept_upload).await;
    assert_eq!(
        kept.status, 200,
        "case \"one of two uploads of one key\": the other upload must still read back, \
         got {} with body {:?}",
        kept.status,
        text(&kept)
    );
    assert_eq!(
        reported_parts(&kept),
        2,
        "case \"one of two uploads of one key\": the other upload must keep both parts, \
         got {:?}",
        text(&kept)
    );

    let peer = list_parts(&router, second_bucket, key, &other_bucket_upload).await;
    assert_eq!(
        peer.status, 200,
        "case \"the same key's upload in another bucket\": it must still read back, got {} \
         with body {:?}",
        peer.status,
        text(&peer)
    );
    assert_eq!(
        reported_parts(&peer),
        1,
        "case \"the same key's upload in another bucket\": it must keep its part, got {:?}",
        text(&peer)
    );

    // The id minted in the first bucket addresses no upload the second one ever held, so an
    // abort across buckets is refused and removes nothing.
    let crossed = abort(&router, second_bucket, key, &kept_upload, &[]).await;
    assert_eq!(
        crossed.status, 404,
        "case \"an id named against another bucket\": expected HTTP 404, got {} with body \
         {:?}",
        crossed.status,
        text(&crossed)
    );
    assert_eq!(
        crossed.error_code(),
        "NoSuchUpload",
        "case \"an id named against another bucket\": expected \"NoSuchUpload\", got {:?}",
        crossed.error_code()
    );
    let still_there = list_parts(&router, first_bucket, key, &kept_upload).await;
    assert_eq!(
        still_there.status, 200,
        "case \"an id named against another bucket\": the upload it named must be \
         untouched, got {} with body {:?}",
        still_there.status,
        text(&still_there)
    );
}

/// One row of the no-op table: the case name and the address the abort carries, for an
/// address the bucket HELD an upload under that is already gone.
struct NoOpCase {
    case: &'static str,
    bucket: String,
    key: String,
    upload_id: String,
}

#[tokio::test]
async fn an_abort_of_an_upload_that_is_already_gone_is_a_no_op() {
    // Rule 4: the abort's own post-condition — that the address holds no upload in progress
    // — already holds for every row here, so each is answered with the operation's success
    // status rather than a refusal, and the cleanup a caller runs after a completion or in a
    // teardown path never has to special-case an upload that is already gone.
    let router = router();
    let name = "multipart-abort-idempotent";
    bucket(&router, name).await;

    // A WITNESS upload no row names: whatever each no-op answers, nothing may be removed.
    let witness_key = "live/upload.bin";
    let witness = initiate(&router, name, witness_key).await;
    upload_part(&router, name, witness_key, &witness, 1, "still here").await;
    let other_key = "live/other.bin";
    let other = initiate(&router, name, other_key).await;

    // An id an abort already discarded, and the same again for a key whose upload held no
    // part at all, so the rows span both forms of an upload that was there.
    let aborted_key = "dead/upload.bin";
    let aborted = initiate(&router, name, aborted_key).await;
    upload_part(&router, name, aborted_key, &aborted, 1, "going").await;
    let empty_key = "dead/empty.bin";
    let empty = initiate(&router, name, empty_key).await;
    for (key, upload_id) in [(aborted_key, &aborted), (empty_key, &empty)] {
        let discarded = abort(&router, name, key, upload_id, &[]).await;
        assert_eq!(
            discarded.status,
            204,
            "the row's upload must be aborted once first, got {} with body {:?}",
            discarded.status,
            text(&discarded)
        );
    }

    // And an id a COMPLETION took away, which is the id a cleanup path aborts after the
    // upload it was looping over finished.
    let completed_key = "completed/upload.bin";
    let completed_upload = initiate(&router, name, completed_key).await;
    let part = send(
        &router,
        "PUT",
        &format!(
            "/{name}/{}?partNumber=1&uploadId={completed_upload}&x-id=UploadPart",
            in_path(completed_key)
        ),
        &[],
        "assembled",
    )
    .await;
    assert_eq!(
        part.status,
        200,
        "the part of the completed upload must be recorded, got {} with body {:?}",
        part.status,
        text(&part)
    );
    let etag = part
        .header("etag")
        .expect("a part upload must report an ETag header");
    let completion = send(
        &router,
        "POST",
        &format!(
            "/{name}/{}?uploadId={completed_upload}",
            in_path(completed_key)
        ),
        &[],
        &format!("<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag></Part></CompleteMultipartUpload>"),
    )
    .await;
    assert_eq!(
        completion.status,
        200,
        "the upload must be completed first, got {} with body {:?}",
        completion.status,
        text(&completion)
    );

    // The rows differ along WHAT took the upload away (an earlier abort, a completion), what
    // the upload held when it went (one part, no part at all), and how many times the abort
    // is repeated — a no-op stays a no-op however often it is made.
    let rows = vec![
        NoOpCase {
            case: "an upload id an earlier abort already discarded",
            bucket: name.to_string(),
            key: aborted_key.to_string(),
            upload_id: aborted.clone(),
        },
        NoOpCase {
            case: "an upload that held no part when an earlier abort discarded it",
            bucket: name.to_string(),
            key: empty_key.to_string(),
            upload_id: empty.clone(),
        },
        NoOpCase {
            case: "an upload id a completion already took away",
            bucket: name.to_string(),
            key: completed_key.to_string(),
            upload_id: completed_upload.clone(),
        },
    ];

    for row in &rows {
        let NoOpCase {
            case,
            bucket: row_bucket,
            key: row_key,
            upload_id: row_upload,
        } = row;
        // Twice, so what is pinned is idempotence rather than one repeat.
        for attempt in 1..=2 {
            let answered = abort(&router, row_bucket, row_key, row_upload, &[]).await;
            assert_eq!(
                answered.status, 204,
                "case {case:?} (attempt {attempt}): expected the no-op's HTTP 204, got {} \
                 with body {:?}",
                answered.status,
                text(&answered)
            );
            assert!(
                answered.body.is_empty(),
                "case {case:?} (attempt {attempt}): a 204 carries no body, got {:?}",
                text(&answered)
            );
        }
    }

    // Nothing above was removed: the two uploads that were in progress still are, and the
    // one no row named still holds its part.
    let listed = uploads_in_progress(&router, name).await;
    assert_eq!(
        listed,
        vec![
            (other_key.to_string(), other.clone()),
            (witness_key.to_string(), witness.clone()),
        ],
        "a no-op abort removes nothing, so both uploads must still be listed, got {listed:?}"
    );
    let survivor = list_parts(&router, name, witness_key, &witness).await;
    assert_eq!(
        survivor.status, 200,
        "the witness upload must still read back, got {} with body {:?}",
        survivor.status,
        text(&survivor)
    );
    assert_eq!(
        reported_parts(&survivor),
        1,
        "the witness upload must still hold its part, got {:?}",
        text(&survivor)
    );
}

/// One row of the refusal table: the case name, the address and headers the abort carries,
/// and the status and wire error code it must be refused with.
struct RefusalCase {
    case: &'static str,
    bucket: String,
    key: String,
    upload_id: String,
    headers: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn an_address_or_a_feature_this_emulator_cannot_serve_is_refused() {
    let router = router();
    let name = "multipart-abort-refusals";
    let elsewhere = "multipart-abort-refusals-peer";
    bucket(&router, name).await;
    bucket(&router, elsewhere).await;

    let key = "live/upload.bin";
    let live = initiate(&router, name, key).await;
    upload_part(&router, name, key, &live, 1, "still here").await;
    let other_key = "live/other.bin";
    let other = initiate(&router, name, other_key).await;

    // The rows differ along what the rule generalises over: WHY the id names no upload of
    // this bucket (never minted, minted for another key, minted in another bucket) and on a
    // key that has an upload in progress or one that never had any, the ADDRESS an abort
    // cannot reach at all (a bucket this emulator does not hold), and the two headers whose
    // behaviour this emulator cannot carry out beside the owner assertion that does not hold.
    let rows = vec![
        RefusalCase {
            case: "an upload id the store never minted",
            bucket: name.to_string(),
            key: key.to_string(),
            upload_id: "sim-fabricated-upload-id-0000".to_string(),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "an upload id the store never minted, on a key nothing was uploaded for",
            bucket: name.to_string(),
            key: "never-uploaded.bin".to_string(),
            upload_id: "another-fabricated-id".to_string(),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "a real upload id named under another key",
            bucket: name.to_string(),
            key: other_key.to_string(),
            upload_id: live.clone(),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "a real upload id named against another bucket",
            bucket: elsewhere.to_string(),
            key: key.to_string(),
            upload_id: live.clone(),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "a bucket this emulator does not hold",
            bucket: "multipart-abort-absent-bucket".to_string(),
            key: key.to_string(),
            upload_id: live.clone(),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalCase {
            case: "a Requester Pays confirmation",
            bucket: name.to_string(),
            key: key.to_string(),
            upload_id: live.clone(),
            headers: vec![("x-amz-request-payer", "requester")],
            status: 501,
            code: "NotImplemented",
        },
        RefusalCase {
            case: "the directory-bucket-only initiated-time conditional",
            bucket: name.to_string(),
            key: key.to_string(),
            upload_id: live.clone(),
            // The instant is spelled the way the framework's timestamp binding accepts a
            // header value; what the row is about is that the CONDITIONAL is refused, not
            // how the instant is spelled.
            headers: vec![("x-amz-if-match-initiated-time", "2026-10-21T07:28:00Z")],
            status: 501,
            code: "NotImplemented",
        },
        RefusalCase {
            case: "x-amz-expected-bucket-owner naming another account",
            bucket: name.to_string(),
            key: key.to_string(),
            upload_id: live.clone(),
            headers: vec![("x-amz-expected-bucket-owner", "123456789012")],
            status: 403,
            code: "AccessDenied",
        },
    ];

    for row in &rows {
        let RefusalCase {
            case,
            bucket: row_bucket,
            key: row_key,
            upload_id: row_upload,
            headers,
            status,
            code,
        } = row;
        let refused = abort(&router, row_bucket, row_key, row_upload, headers).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the wire error code {code:?}, got {:?}",
            refused.error_code()
        );
        // Every refusal carries a message, and the `NoSuchUpload` class carries the FIXED
        // sentence S3 sends for it — the same text for every row, with nothing from the
        // request spliced in.
        let message = refused.error_message();
        assert!(
            !message.trim().is_empty(),
            "case {case:?}: an S3 error document carries a Message, got {:?}",
            text(&refused)
        );
        if *code == "NoSuchUpload" {
            assert_eq!(
                message, NO_SUCH_UPLOAD_MESSAGE,
                "case {case:?}: expected the fixed NoSuchUpload sentence, got {message:?}"
            );
            assert!(
                !message.contains(row_upload.as_str()) && !message.contains(row_key.as_str()),
                "case {case:?}: the message must interpolate no request value, got \
                 {message:?}"
            );
        }
    }

    // Nothing above was discarded: the two uploads that were in progress still are, and
    // the one the rows refused still holds its part.
    let listed = uploads_in_progress(&router, name).await;
    assert_eq!(
        listed,
        vec![
            (other_key.to_string(), other.clone()),
            (key.to_string(), live.clone()),
        ],
        "a refused abort removes nothing, so both uploads must still be listed, got \
         {listed:?}"
    );
    let survivor = list_parts(&router, name, key, &live).await;
    assert_eq!(
        survivor.status, 200,
        "the refused upload must still read back, got {} with body {:?}",
        survivor.status,
        text(&survivor)
    );
    assert_eq!(
        reported_parts(&survivor),
        1,
        "the refused upload must still hold its part, got {:?}",
        text(&survivor)
    );
}
