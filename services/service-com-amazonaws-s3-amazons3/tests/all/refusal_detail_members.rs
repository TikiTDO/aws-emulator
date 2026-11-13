// SPDX-License-Identifier: CC0-1.0

//! A refusal carries, in its error document, the members of ITS error class.
//!
//! The rule under test: beside the fixed `<Code>` and `<Message>` of the error class, S3's
//! `<Error>` document carries that class's own contextual members, each filled from the
//! REQUEST or from the emulator's STATE — and the message itself stays the class's fixed
//! text, never extended with one of those values. The message says what KIND of refusal it
//! is; the members say which request it was, and are the only machine-readable part a client
//! can act on:
//!
//! * an upload-scoped refusal names the `<UploadId>` the request addressed;
//! * a refusal about one PART names the `<UploadId>`, `<PartNumber>` and `<ETag>` a caller
//!   re-uploads it under — the tag as its own value, without the quotes HTTP's
//!   quoted-string syntax wraps the `ETag` HEADER in — and one refused on SIZE the
//!   `<ProposedSize>` it has beside the `<MinSizeAllowed>` it was judged against;
//! * a method refused against a kind of resource names the `<Method>` and the
//!   `<ResourceType>`;
//! * an unsatisfiable range names the `<RangeRequested>` as the request spelled it and the
//!   `<ActualObjectSize>` it was judged against;
//! * a precondition that did not hold names the conditional HEADER that failed in its
//!   `<Condition>`.
//!
//! The tables span what the rule generalises over rather than one recorded request:
//!
//! * the precondition table over the CONDITION (the two entity-tag headers, the two date
//!   headers, the two S3-defined delete headers, the four copy-source headers and the
//!   annotation operations' own), over the OPERATION that judges it (a write, a read, a
//!   delete, a copy, a part copy, a completion, an annotation write and an annotation
//!   delete) and over the kind of bucket it is judged on;
//! * the multipart table over the REFUSAL CLASS (an upload that is not there, a part that is
//!   not there, a tag that is not the part's, a list out of order, a part below the floor)
//!   and over the CALL that answers the upload-scoped one (the completion, the abort, the
//!   listing and a part upload);
//! * the range table over the FORM of the `Range` header (a span past the end, an open-ended
//!   start past the end, a zero-length suffix) and over the SIZE of the object it is judged
//!   against, including an empty one;
//! * the method table over the RESOURCE TYPE refused (the `?torrent` subresource and a
//!   delete marker).
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! document.

use service_com_amazonaws_s3_amazons3::state::{
    DELETE_MARKER_RESOURCE_TYPE, MIN_MULTIPART_PART_BYTES, PRECONDITION_FAILED_MESSAGE,
    TORRENT_RESOURCE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::multipart_upload_completion::completion_payload;
use super::object_annotation_removal::delete_annotation;
use super::object_annotations::put_annotation;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object as versioned_delete, set_versioning};

/// A time before every write this suite makes, so a `*-if-unmodified-since` judged
/// against it cannot hold and a `*-if-modified-since` judged against it can.
///
/// In the spelling the framework's timestamp binding reads a bound date member in, which is
/// the spelling the copy suite beside this one sends its own date members in.
const BEFORE_EVERY_WRITE: &str = "2015-01-01T00:00:00Z";

/// A time after every write this suite makes — the other side of the same pair.
const AFTER_EVERY_WRITE: &str = "2035-01-01T00:00:00Z";

/// An entity tag no object this suite writes carries, for a row whose condition must fail.
const ANOTHER_TAG: &str = "\"00000000000000000000000000000000\"";

/// The bytes every object in the precondition table holds.
const PAYLOAD: &[u8] = b"detail-member-payload";

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id.
async fn initiate(router: &MultiServiceRouter, bucket_name: &str, key: &str) -> String {
    let initiated = send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploads"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        initiated.status,
        200,
        "an initiation must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId")
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`, answering the part's
/// entity tag.
async fn upload_part(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    body: &[u8],
) -> String {
    let uploaded = send_bytes(
        router,
        "PUT",
        &format!(
            "/{bucket_name}/{key}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPart"
        ),
        &[],
        body,
    )
    .await;
    assert_eq!(
        uploaded.status,
        200,
        "uploading part {part_number} must answer HTTP 200, got {} with body {:?}",
        uploaded.status,
        text(&uploaded)
    );
    uploaded
        .header("etag")
        .expect("a part upload must report an ETag header")
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`.
async fn complete(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploadId={upload_id}"),
        headers,
        payload,
    )
    .await
}

/// `PUT /<bucket>/<key>?x-id=CopyObject` — `CopyObject`, with the source and the conditional
/// headers the row carries.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}?x-id=CopyObject"),
        &all,
        b"",
    )
    .await
}

/// An entity tag's own VALUE, without the double quotes HTTP's quoted-string syntax wraps
/// it in: the form an error document carries a tag in, so a row can name the tag the way a
/// part upload reported it and still assert what the envelope must say.
fn tag_value(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// What one precondition row refuses, and which header its `<Condition>` must name.
struct ConditionRow {
    case: &'static str,
    /// The conditional header the request carries, and the value that makes it fail.
    header: (&'static str, String),
    /// The header name the refusal's `<Condition>` must carry.
    condition: &'static str,
}

#[tokio::test]
async fn a_precondition_that_does_not_hold_names_the_header_in_its_condition() {
    let router = router();
    let general = "detail-conditions";
    let origin = "detail-conditions-origin";
    bucket(&router, general).await;
    bucket(&router, origin).await;

    let standing = put_object(&router, general, "standing.bin", PAYLOAD, &[]).await;
    assert_eq!(
        standing.status,
        200,
        "the standing object must be stored first, got {} with body {:?}",
        standing.status,
        text(&standing)
    );
    let held_tag = standing
        .header("etag")
        .expect("a write must report an ETag header");
    let source = put_object(&router, origin, "source.bin", PAYLOAD, &[]).await;
    assert_eq!(
        source.status,
        200,
        "the copy source must be stored first, got {} with body {:?}",
        source.status,
        text(&source)
    );
    let source_tag = source
        .header("etag")
        .expect("a write must report an ETag header");
    let copy_source = format!("{origin}/source.bin");

    // A WRITE's own two conditional members, judged against what the key holds.
    let write_rows = vec![
        ConditionRow {
            case: "a write whose If-None-Match asks for a key that holds nothing",
            header: ("if-none-match", "*".to_string()),
            condition: "If-None-Match",
        },
        ConditionRow {
            case: "a write whose If-Match names a tag the key does not hold",
            header: ("if-match", ANOTHER_TAG.to_string()),
            condition: "If-Match",
        },
        ConditionRow {
            case: "a write whose If-Match names the held tag unquoted but altered",
            header: ("if-match", "d41d8cd98f00b204e9800998ecf8427e".to_string()),
            condition: "If-Match",
        },
    ];
    for row in &write_rows {
        let ConditionRow {
            case,
            header,
            condition,
        } = row;
        let refused = put_object(
            &router,
            general,
            "standing.bin",
            b"replacement",
            &[(header.0, header.1.as_str())],
        )
        .await;
        assert_condition(case, "PutObject", &refused, condition);
        // A refused write left the object exactly as it was, so the refusal reported a
        // condition rather than performing the write and describing it.
        let read = get_object(&router, general, "standing.bin", "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            PAYLOAD,
            "case {case:?}: a refused write must leave the stored bytes alone"
        );
    }

    // A READ's two conditional members, and a DELETE's entity-tag one.
    let read_rows = vec![
        ConditionRow {
            case: "a read whose If-Match names a tag the object does not carry",
            header: ("if-match", ANOTHER_TAG.to_string()),
            condition: "If-Match",
        },
        ConditionRow {
            case: "a read whose If-Unmodified-Since is before the write",
            header: ("if-unmodified-since", BEFORE_EVERY_WRITE.to_string()),
            condition: "If-Unmodified-Since",
        },
    ];
    for row in &read_rows {
        let ConditionRow {
            case,
            header,
            condition,
        } = row;
        let refused = get_object(
            &router,
            general,
            "standing.bin",
            "",
            &[(header.0, header.1.as_str())],
        )
        .await;
        assert_condition(case, "GetObject", &refused, condition);
    }

    let refused = super::object_conditional_deletes::delete_object(
        &router,
        general,
        "standing.bin",
        &[("if-match", ANOTHER_TAG)],
    )
    .await;
    assert_condition(
        "a delete whose If-Match names a tag the object does not carry",
        "DeleteObject",
        &refused,
        "If-Match",
    );

    // The four COPY-SOURCE conditions, each in the combination that refuses the copy: the
    // entity-tag members decide on their own, and a date member is judged only when the
    // tag member beside it is absent.
    let copy_rows = [
        ConditionRow {
            case: "a copy whose source if-match names another tag",
            header: ("x-amz-copy-source-if-match", ANOTHER_TAG.to_string()),
            condition: "x-amz-copy-source-If-Match",
        },
        ConditionRow {
            case: "a copy whose source if-none-match names the source's own tag",
            header: ("x-amz-copy-source-if-none-match", source_tag.clone()),
            condition: "x-amz-copy-source-If-None-Match",
        },
        ConditionRow {
            case: "a copy whose source if-unmodified-since is before the write",
            header: (
                "x-amz-copy-source-if-unmodified-since",
                BEFORE_EVERY_WRITE.to_string(),
            ),
            condition: "x-amz-copy-source-If-Unmodified-Since",
        },
        ConditionRow {
            case: "a copy whose source if-modified-since is after the write",
            header: (
                "x-amz-copy-source-if-modified-since",
                AFTER_EVERY_WRITE.to_string(),
            ),
            condition: "x-amz-copy-source-If-Modified-Since",
        },
    ];
    for (index, row) in copy_rows.iter().enumerate() {
        let ConditionRow {
            case,
            header,
            condition,
        } = row;
        let key = format!("copied/{index}.bin");
        let refused = copy_object(
            &router,
            general,
            &key,
            &copy_source,
            &[(header.0, header.1.as_str())],
        )
        .await;
        assert_condition(case, "CopyObject", &refused, condition);
        let absent = get_object(&router, general, &key, "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused copy must write nothing, got GetObject {}",
            absent.status
        );

        // The same condition on a PART copy, which judges the same four members.
        let upload_id = initiate(&router, general, &format!("part-copied/{index}.bin")).await;
        let part_refused = send_bytes(
            &router,
            "PUT",
            &format!(
                "/{general}/part-copied/{index}.bin?partNumber=1&uploadId={upload_id}\
                 &x-id=UploadPartCopy"
            ),
            &[
                ("x-amz-copy-source", copy_source.as_str()),
                (header.0, header.1.as_str()),
            ],
            b"",
        )
        .await;
        assert_condition(case, "UploadPartCopy", &part_refused, condition);
    }

    // A COMPLETION is a conditional write too, judged against what the key holds when the
    // parts are assembled.
    let upload_id = initiate(&router, general, "standing.bin").await;
    let part_tag = upload_part(&router, general, "standing.bin", &upload_id, 1, b"assembled").await;
    let refused = complete(
        &router,
        general,
        "standing.bin",
        &upload_id,
        &completion_payload(&[(1, part_tag)]),
        &[("if-none-match", "*")],
    )
    .await;
    assert_condition(
        "a completion whose If-None-Match asks for a key that holds nothing",
        "CompleteMultipartUpload",
        &refused,
        "If-None-Match",
    );
    assert_eq!(
        held_tag,
        get_object(&router, general, "standing.bin", "", &[])
            .await
            .header("etag")
            .unwrap_or_default(),
        "a refused completion must leave the standing object exactly as it was"
    );

    // The ANNOTATION operations carry a conditional member of their own, bound to an
    // S3-defined header, and name it the same way.
    let annotated = put_annotation(
        &router,
        general,
        "standing.bin",
        "review",
        b"annotation payload",
        &[],
    )
    .await;
    assert_eq!(
        annotated.status,
        200,
        "the annotation must be stored first, got {} with body {:?}",
        annotated.status,
        text(&annotated)
    );
    let annotation_write = put_annotation(
        &router,
        general,
        "standing.bin",
        "review",
        b"replacement payload",
        &[("x-amz-object-if-match", ANOTHER_TAG)],
    )
    .await;
    assert_condition(
        "an annotation write whose object-if-match names another tag",
        "PutObjectAnnotation",
        &annotation_write,
        "x-amz-object-if-match",
    );
    let annotation_delete = delete_annotation(
        &router,
        general,
        "standing.bin",
        "review",
        "",
        &[("x-amz-object-if-match", ANOTHER_TAG)],
    )
    .await;
    assert_condition(
        "an annotation delete whose object-if-match names another tag",
        "DeleteObjectAnnotation",
        &annotation_delete,
        "x-amz-object-if-match",
    );

    // The two conditional members only a DIRECTORY bucket judges, on one: the rule is about
    // the condition that failed, not about the kind of bucket it was judged on.
    let directory = "detail-conditions--use1-az4--x-s3";
    directory_bucket(&router, directory, "use1-az4").await;
    let stored = put_object(&router, directory, "standing.bin", PAYLOAD, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the directory bucket's object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    for (case, header, condition) in [
        (
            "a delete whose size is not the object's",
            ("x-amz-if-match-size", "7"),
            "x-amz-if-match-size",
        ),
        (
            "a delete whose modification time is not the object's",
            (
                "x-amz-if-match-last-modified-time",
                "2015-01-01T00:00:00Z",
            ),
            "x-amz-if-match-last-modified-time",
        ),
    ] {
        let refused = super::object_conditional_deletes::delete_object(
            &router,
            directory,
            "standing.bin",
            &[header],
        )
        .await;
        assert_condition(case, "DeleteObject", &refused, condition);
    }
}

/// Every precondition refusal is the same 412 with the same code and the same FIXED
/// sentence, and differs only in the `<Condition>` naming the header that did not hold.
fn assert_condition(case: &str, call: &str, refused: &Answer, condition: &str) {
    assert_eq!(
        refused.status,
        412,
        "case {case:?}/{call}: expected HTTP 412, got {} with body {:?}",
        refused.status,
        text(refused)
    );
    assert_eq!(
        refused.error_code(),
        "PreconditionFailed",
        "case {case:?}/{call}: expected the PreconditionFailed code, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.error_message(),
        PRECONDITION_FAILED_MESSAGE,
        "case {case:?}/{call}: the sentence must be the code's own fixed text, got {:?}",
        refused.error_message()
    );
    assert_eq!(
        refused.element("Condition").as_deref(),
        Some(condition),
        "case {case:?}/{call}: expected the <Condition> to name the header that did not \
         hold, got {:?} in {:?}",
        refused.element("Condition"),
        text(refused)
    );
}

#[tokio::test]
async fn an_upload_scoped_refusal_names_the_upload_the_request_addressed() {
    let router = router();
    let name = "detail-upload-scope";
    bucket(&router, name).await;
    let key = "assembled.bin";

    // An upload that IS in progress throughout, so a row's envelope naming the wrong upload
    // would be caught, and so every refusal below is about the id the request carried.
    let standing_upload = initiate(&router, name, key).await;

    // Several ids no upload was ever minted under, spanning the forms a client may send:
    // another upload's shape, a short token, one spelled with dots, and one carrying
    // characters a query has to percent-encode and an XML document has to escape — each as
    // the request SPELLS it in the query, beside the value the envelope must report.
    let absent_ids = [
        (
            "ZmFicmljYXRlZC11cGxvYWQtaWQtdGhhdC1uZXZlci1leGlzdGVk",
            "ZmFicmljYXRlZC11cGxvYWQtaWQtdGhhdC1uZXZlci1leGlzdGVk",
        ),
        ("0", "0"),
        ("upload.id-with.dots", "upload.id-with.dots"),
        ("a%3Cb%3E%26c", "a<b>&c"),
    ];
    for (upload_id, reported) in absent_ids {
        // The completion of an upload that is not there, and the same id through the three
        // other calls that address an upload: one rule, every operation that answers it. Each
        // id here is one this bucket never had an upload in progress under, which is the
        // address `AbortMultipartUpload` refuses `NoSuchUpload` too — an abort of an upload
        // the bucket DID hold and has already lost is the no-op answered 204 instead
        // ([`super::multipart_upload_abort`]), and that one has no error document to name an
        // upload in.
        let completed = complete(
            &router,
            name,
            key,
            upload_id,
            &completion_payload(&[(1, "\"d41d8cd98f00b204e9800998ecf8427e\"".to_string())]),
            &[],
        )
        .await;
        let aborted = send_bytes(
            &router,
            "DELETE",
            &format!("/{name}/{key}?uploadId={upload_id}&x-id=AbortMultipartUpload"),
            &[],
            b"",
        )
        .await;
        let listed = send(
            &router,
            "GET",
            &format!("/{name}/{key}?uploadId={upload_id}&x-id=ListParts"),
            &[],
            "",
        )
        .await;
        let part = send_bytes(
            &router,
            "PUT",
            &format!("/{name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"),
            &[],
            b"part bytes",
        )
        .await;
        for (call, refused) in [
            ("CompleteMultipartUpload", &completed),
            ("AbortMultipartUpload", &aborted),
            ("ListParts", &listed),
            ("UploadPart", &part),
        ] {
            assert_eq!(
                refused.status,
                404,
                "case {upload_id:?}/{call}: expected HTTP 404, got {} with body {:?}",
                refused.status,
                text(refused)
            );
            assert_eq!(
                refused.error_code(),
                "NoSuchUpload",
                "case {upload_id:?}/{call}: expected the NoSuchUpload code, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.element("UploadId").as_deref(),
                Some(reported),
                "case {upload_id:?}/{call}: the error document must name the upload the \
                 request addressed, got {:?} in {:?}",
                refused.element("UploadId"),
                text(refused)
            );
            // A caller-influenced value reaches the envelope through the serializer, so a
            // name carrying XML metacharacters comes back escaped rather than breaking the
            // document.
            if reported.contains('<') {
                assert!(
                    text(refused).contains("a&lt;b&gt;&amp;c"),
                    "case {upload_id:?}/{call}: the named upload must be XML-escaped in the \
                     document, got {:?}",
                    text(refused)
                );
            }
        }
    }

    // The upload that was in progress all along is still in progress, so no refusal above
    // took anything away.
    let listed = send(
        &router,
        "GET",
        &format!("/{name}/{key}?uploadId={standing_upload}&x-id=ListParts"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        listed.status,
        200,
        "the standing upload must still be in progress, got {} with body {:?}",
        listed.status,
        text(&listed)
    );

    // A completion of a REAL upload whose parts list is not in ascending order is refused
    // about that upload too, so it names it.
    let tags = [
        upload_part(&router, name, key, &standing_upload, 1, b"first part").await,
        upload_part(&router, name, key, &standing_upload, 2, b"second part").await,
    ];
    let out_of_order = complete(
        &router,
        name,
        key,
        &standing_upload,
        &completion_payload(&[(2, tags[1].clone()), (1, tags[0].clone())]),
        &[],
    )
    .await;
    assert_eq!(
        out_of_order.status,
        400,
        "a parts list out of order must answer HTTP 400, got {} with body {:?}",
        out_of_order.status,
        text(&out_of_order)
    );
    assert_eq!(
        out_of_order.error_code(),
        "InvalidPartOrder",
        "expected the InvalidPartOrder code, got {:?}",
        out_of_order.error_code()
    );
    assert_eq!(
        out_of_order.element("UploadId").as_deref(),
        Some(standing_upload.as_str()),
        "the error document must name the upload the request addressed, got {:?} in {:?}",
        out_of_order.element("UploadId"),
        text(&out_of_order)
    );
}

/// One row of the part-refusal table: the parts list the completion names, and the class it
/// must be refused with.
struct PartRow {
    case: &'static str,
    /// The part number the completion names, and the entity tag it names it with.
    named: (i32, String),
    code: &'static str,
}

#[tokio::test]
async fn a_refusal_about_one_part_names_that_part_in_its_error_document() {
    let router = router();
    let name = "detail-part-scope";
    bucket(&router, name).await;
    let key = "parts.bin";
    let upload_id = initiate(&router, name, key).await;
    let held_tag = upload_part(&router, name, key, &upload_id, 1, b"the only part").await;

    let rows = vec![
        PartRow {
            case: "a part number the upload holds no part for",
            named: (7, held_tag.clone()),
            code: "InvalidPart",
        },
        PartRow {
            case: "a tag that is not the part's",
            named: (1, ANOTHER_TAG.to_string()),
            code: "InvalidPart",
        },
        PartRow {
            case: "a part number and a tag neither of which the upload holds",
            named: (10_000, ANOTHER_TAG.to_string()),
            code: "InvalidPart",
        },
    ];
    for row in &rows {
        let PartRow { case, named, code } = row;
        let refused = complete(
            &router,
            name,
            key,
            &upload_id,
            &completion_payload(std::slice::from_ref(named)),
            &[],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the {code:?} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.element("UploadId").as_deref(),
            Some(upload_id.as_str()),
            "case {case:?}: the document must name the upload the part belongs to, got {:?} \
             in {:?}",
            refused.element("UploadId"),
            text(&refused)
        );
        assert_eq!(
            refused.element("PartNumber").as_deref(),
            Some(named.0.to_string().as_str()),
            "case {case:?}: the document must name the part number the request named, got \
             {:?} in {:?}",
            refused.element("PartNumber"),
            text(&refused)
        );
        assert_eq!(
            refused.element("ETag").as_deref(),
            Some(tag_value(&named.1)),
            "case {case:?}: the document must name the entity tag the request named, got \
             {:?} in {:?}",
            refused.element("ETag"),
            text(&refused)
        );
    }

    // A part BELOW the published floor, which is only knowable once a list says it is not
    // the last one: the refusal names the size it has beside the size it was judged against,
    // which is what a caller resizes it by. The rows differ in how far below the floor the
    // offending part is, including one a single byte short of it.
    for (case, first_part_size) in [
        ("a part of a few bytes", 9_usize),
        ("an empty first part", 0),
        ("a part one byte short of the floor", MIN_MULTIPART_PART_BYTES - 1),
    ] {
        let too_small_key = format!("too-small/{first_part_size}.bin");
        let upload = initiate(&router, name, &too_small_key).await;
        let first = vec![b'a'; first_part_size];
        let first_tag = upload_part(&router, name, &too_small_key, &upload, 1, &first).await;
        let last_tag = upload_part(&router, name, &too_small_key, &upload, 2, b"last part").await;
        let refused = complete(
            &router,
            name,
            &too_small_key,
            &upload,
            &completion_payload(&[(1, first_tag.clone()), (2, last_tag)]),
            &[],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "EntityTooSmall",
            "case {case:?}: expected the EntityTooSmall code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.element("ProposedSize").as_deref(),
            Some(first_part_size.to_string().as_str()),
            "case {case:?}: the document must report the size the part really has, got {:?} \
             in {:?}",
            refused.element("ProposedSize"),
            text(&refused)
        );
        assert_eq!(
            refused.element("MinSizeAllowed").as_deref(),
            Some(MIN_MULTIPART_PART_BYTES.to_string().as_str()),
            "case {case:?}: the document must report the floor the part was judged against, \
             got {:?} in {:?}",
            refused.element("MinSizeAllowed"),
            text(&refused)
        );
        assert_eq!(
            refused.element("PartNumber").as_deref(),
            Some("1"),
            "case {case:?}: the document must name the offending part, got {:?} in {:?}",
            refused.element("PartNumber"),
            text(&refused)
        );
        assert_eq!(
            refused.element("ETag").as_deref(),
            Some(tag_value(&first_tag)),
            "case {case:?}: the document must name that part's entity tag, got {:?} in {:?}",
            refused.element("ETag"),
            text(&refused)
        );
    }
}

#[tokio::test]
async fn an_unsatisfiable_range_names_the_range_and_the_object_size() {
    let router = router();
    let name = "detail-ranges";
    bucket(&router, name).await;

    // Two sizes, including an EMPTY object, against which every range is unsatisfiable.
    for (key, payload) in [
        ("sized.bin", PAYLOAD.to_vec()),
        ("empty.bin", Vec::new()),
        ("one-byte.bin", vec![b'z']),
    ] {
        let stored = put_object(&router, name, key, &payload, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "the object {key:?} must be stored first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        // The three forms of the `bytes` unit that can ask for what the representation does
        // not have: a span starting past the end, an open-ended start past the end, and a
        // zero-length suffix.
        for range in [
            format!("bytes={}-{}", payload.len(), payload.len() + 10),
            format!("bytes={}-", payload.len()),
            "bytes=-0".to_string(),
        ] {
            let refused = get_object(&router, name, key, "", &[("range", range.as_str())]).await;
            assert_eq!(
                refused.status,
                416,
                "case {key:?}/{range:?}: expected HTTP 416, got {} with body {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                "InvalidRange",
                "case {key:?}/{range:?}: expected the InvalidRange code, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.element("RangeRequested").as_deref(),
                Some(range.as_str()),
                "case {key:?}/{range:?}: the document must echo the Range the request \
                 carried, got {:?} in {:?}",
                refused.element("RangeRequested"),
                text(&refused)
            );
            assert_eq!(
                refused.element("ActualObjectSize").as_deref(),
                Some(payload.len().to_string().as_str()),
                "case {key:?}/{range:?}: the document must report the object's own size, \
                 got {:?} in {:?}",
                refused.element("ActualObjectSize"),
                text(&refused)
            );
        }

        // A range the representation DOES have is served, so the refusal above is the range
        // being judged rather than every ranged read failing.
        if !payload.is_empty() {
            let served = get_object(&router, name, key, "", &[("range", "bytes=0-0")]).await;
            assert_eq!(
                served.status,
                206,
                "case {key:?}: a range the object has must be served, got {} with body {:?}",
                served.status,
                text(&served)
            );
        }
    }
}

#[tokio::test]
async fn a_method_refused_against_a_resource_type_names_both() {
    let router = router();
    let name = "detail-methods";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    // The `?torrent` subresource: refused whatever the key names, so the rows span a key
    // the bucket holds and one it does not.
    let stored = put_object(&router, name, "held.bin", PAYLOAD, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    for key in ["held.bin", "never/written.bin"] {
        let refused = get_object(&router, name, key, "?torrent", &[]).await;
        assert_eq!(
            refused.status,
            405,
            "case {key:?}: expected HTTP 405, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "MethodNotAllowed",
            "case {key:?}: expected the MethodNotAllowed code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.element("Method").as_deref(),
            Some("GET"),
            "case {key:?}: the document must name the method the request used, got {:?} in \
             {:?}",
            refused.element("Method"),
            text(&refused)
        );
        assert_eq!(
            refused.element("ResourceType").as_deref(),
            Some(TORRENT_RESOURCE_TYPE),
            "case {key:?}: the document must name the resource type the method was refused \
             against, got {:?} in {:?}",
            refused.element("ResourceType"),
            text(&refused)
        );
    }

    // A DELETE MARKER is the other resource type an object-level method is refused against,
    // and the document names the marker and the method that addressed it.
    let deleted = versioned_delete(&router, name, "held.bin", "").await;
    assert_eq!(
        deleted.status,
        204,
        "the delete must lay a marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let marker_version = deleted
        .header("x-amz-version-id")
        .expect("a delete on a versioned bucket must report the marker's version id");
    for (call, method, refused) in [
        (
            "GetObject",
            "GET",
            get_object(
                &router,
                name,
                "held.bin",
                &format!("?versionId={marker_version}"),
                &[],
            )
            .await,
        ),
        (
            "GetObjectAcl",
            "GET",
            get_object(
                &router,
                name,
                "held.bin",
                &format!("?acl&versionId={marker_version}"),
                &[],
            )
            .await,
        ),
        (
            "RestoreObject",
            "POST",
            send(
                &router,
                "POST",
                &format!("/{name}/held.bin?restore&versionId={marker_version}"),
                &[],
                "<RestoreRequest><Days>1</Days></RestoreRequest>",
            )
            .await,
        ),
    ] {
        assert_eq!(
            refused.status,
            405,
            "case {call}: expected HTTP 405, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.element("Method").as_deref(),
            Some(method),
            "case {call}: the document must name the method the request used, got {:?} in \
             {:?}",
            refused.element("Method"),
            text(&refused)
        );
        assert_eq!(
            refused.element("ResourceType").as_deref(),
            Some(DELETE_MARKER_RESOURCE_TYPE),
            "case {call}: the document must name the resource type the method was refused \
             against, got {:?} in {:?}",
            refused.element("ResourceType"),
            text(&refused)
        );
    }
}
