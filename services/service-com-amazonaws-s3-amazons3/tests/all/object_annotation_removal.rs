// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteObjectAnnotation`: the removal of ONE named annotation from
//! the object version a request addresses.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the object
//! writer and the annotation writer and reader are the ones [`super::bucket_lifecycle`],
//! [`super::object_and_tagging_roundtrip`], [`super::object_annotations`] and
//! [`super::object_versioning`] already own), so what is pinned is the wire behaviour —
//! status, bound headers, error code and message — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A delete takes away the ONE annotation the request names, permanently: "Deleting an
//!   annotation is permanent. Annotations are not independently versioned, so there is no
//!   delete marker or way to recover a deleted annotation"
//!   (`DeleteObjectAnnotation`'s own documentation). The success is the modelled 204 with an
//!   empty body; the object and every annotation the request did not name are left exactly
//!   as they were.
//! * The annotation is addressed by bucket, key and name, so the first missing link of that
//!   chain is what the request answers: `NoSuchBucket`, `NoSuchKey` or `NoSuchAnnotation`,
//!   each HTTP 404 with the `<Message>` S3's error document carries for that code. A name
//!   outside the naming rules — past the published 512-byte bound, or under a prefix S3
//!   reserves — is judged BEFORE that chain and refused for the rule it breaks
//!   (`AnnotationNameTooLong` / `InvalidAnnotation`, both 400), the same refusal the write
//!   of that name answers.
//! * "Use the x-amz-object-if-match header to perform a conditional delete that only
//!   succeeds if the object's ETag matches the provided value" (same documentation), so a
//!   condition that does not hold is `PreconditionFailed` (412) and removes nothing.
//! * `versionId` addresses one stored version, and the answer names it: "The version ID of
//!   the object that the annotation was deleted from"
//!   (`DeleteObjectAnnotationOutput$ObjectVersionId`). The annotations of a version the
//!   request did not name stay where they are.
//! * An `x-amz-expected-bucket-owner` that is not this emulator's account is denied (403)
//!   and removes nothing; the account it does serve is accepted.

use service_com_amazonaws_s3_amazons3::state::{
    url_encoded_key, MAX_ANNOTATION_NAME_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_annotations::{get_annotation, get_annotation_at, put_annotation_at};
use super::object_versioning::set_versioning;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// The `<Message>` S3's error document carries for each code of the address chain, as the
/// service publishes them (the codes' entries in S3's *Error responses* list, and the error
/// shapes' own documentation in this service's model for the annotation code).
const NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist";
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";
const NO_SUCH_ANNOTATION_MESSAGE: &str = "The specified annotation does not exist.";
/// The `<Message>` each NAME-RULE refusal carries: the fixed sentence of its own class, the
/// same text the write of that name answers.
const ANNOTATION_NAME_TOO_LONG_MESSAGE: &str = "Your annotation name is too long.";
const RESERVED_ANNOTATION_PREFIX_MESSAGE: &str =
    "Your annotation name cannot start with 'aws' or 's3'.";

/// `DELETE /<bucket>/<key>?annotation&annotationName=<name>` — `DeleteObjectAnnotation`,
/// with whatever extra query parameters and bound headers the case under test carries.
///
/// `pub(crate)`: the resource-naming suite ([`super::refusal_resource_naming`]) drives the
/// same removal through this helper rather than standing up a second copy of it.
pub(crate) async fn delete_annotation(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    name: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = format!(
        "/{bucket}/{key}?annotation&annotationName={}{query}",
        url_encoded_key(name)
    );
    send_bytes(router, "DELETE", &uri, headers, b"").await
}

/// An object to hang annotations on, written the way the annotation round-trip suite writes
/// one, failing the test when the service did not store it.
async fn object(router: &MultiServiceRouter, bucket_name: &str, key: &str, body: &[u8]) -> Answer {
    let written = put_object(router, bucket_name, key, body, &[]).await;
    assert_eq!(
        written.status,
        200,
        "the object {key:?} must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    written
}

/// One annotation written on an object, failing the test when the write was not taken — the
/// precondition of every removal case below.
async fn annotation(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    name: &str,
    payload: &[u8],
    query: &str,
) {
    let put = put_annotation_at(router, bucket_name, key, name, query, payload, &[]).await;
    assert_eq!(
        put.status,
        200,
        "the annotation {name:?} must be written first, got {} with body {:?}",
        put.status,
        text(&put)
    );
}

/// A name of exactly `bytes` ASCII bytes, for the rows that sit on the published bound.
fn name_of(bytes: usize) -> String {
    "a".repeat(bytes)
}

/// `length` bytes of valid UTF-8 text, cycling the printable ASCII range from a fixed start,
/// so a large payload is varied without anything random — and still text, which is what an
/// annotation payload has to be.
fn text_of(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| b'!' + u8::try_from(index % 90).unwrap_or(0))
        .collect()
}

/// One row of the removal table: the case name, the annotation name, and its payload.
type RemovalRow<'a> = (&'a str, String, Vec<u8>);

#[tokio::test]
async fn a_delete_takes_one_annotation_and_leaves_the_object_and_its_other_annotations() {
    let router = router();
    bucket(&router, "annotation-removal-bucket").await;
    let body = b"region,total\nwest,12\n";
    object(&router, "annotation-removal-bucket", "reports/q1.csv", body).await;

    // The rows differ along what the rule generalises over: the NAME's size (one byte, a
    // plain word, the full 512-byte maximum), its form (a dotted key, a name that merely
    // contains a reserved prefix, UTF-8 text that is more bytes than characters) and the
    // PAYLOAD's size (one byte, a sentence, 64 KiB) — the removal is of the one name,
    // whatever the name and whatever it holds.
    let rows: [RemovalRow<'_>; 6] = [
        ("a one-byte name", "x".to_string(), b"1".to_vec()),
        (
            "a plain word",
            "reviewer".to_string(),
            b"approved by the data team".to_vec(),
        ),
        (
            "a dotted, namespaced name",
            "com.example.pipeline.stage".to_string(),
            b"ingest".to_vec(),
        ),
        (
            "a name that contains a reserved prefix without starting with it",
            "my-aws-notes".to_string(),
            b"not reserved".to_vec(),
        ),
        (
            "a name of UTF-8 text, more bytes than characters",
            "données-été".to_string(),
            "valeur — oui".as_bytes().to_vec(),
        ),
        (
            "a large payload",
            "bulk".to_string(),
            text_of(64 * 1024),
        ),
    ];

    // Every row is written first, so each delete has to take away ITS name alone out of a
    // set the object really carries.
    for (_, name, payload) in &rows {
        annotation(
            &router,
            "annotation-removal-bucket",
            "reports/q1.csv",
            name,
            payload,
            "",
        )
        .await;
    }
    let names: Vec<String> = rows.iter().map(|(_, name, _)| name.clone()).collect();

    for (index, (case, name, _)) in rows.iter().enumerate() {
        let deleted = delete_annotation(
            &router,
            "annotation-removal-bucket",
            "reports/q1.csv",
            name,
            "",
            &[],
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: expected HTTP 204 from DeleteObjectAnnotation, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert!(
            deleted.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&deleted)
        );
        // The bucket has no versioning state, so its objects carry no version id of their
        // own and none is reported rather than one being invented.
        assert_eq!(
            deleted.header("x-amz-object-version-id"),
            None,
            "case {case:?}: an object with no version id reports none, got {:?}",
            deleted.header("x-amz-object-version-id")
        );

        // The name is gone, permanently: the read of it answers the third link of the
        // address chain.
        let gone = get_annotation(
            &router,
            "annotation-removal-bucket",
            "reports/q1.csv",
            name,
        )
        .await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deleted annotation must be gone, got {} with body {:?}",
            gone.status,
            text(&gone)
        );
        assert_eq!(
            gone.error_code(),
            "NoSuchAnnotation",
            "case {case:?}: expected NoSuchAnnotation after the delete, got {:?}",
            gone.error_code()
        );

        // Every name this delete did not address is still there, with its own payload.
        for (other_index, other) in names.iter().enumerate() {
            if other_index <= index {
                continue;
            }
            let still = get_annotation(
                &router,
                "annotation-removal-bucket",
                "reports/q1.csv",
                other,
            )
            .await;
            assert_eq!(
                still.status,
                200,
                "case {case:?}: the annotation {other:?} was not named, so it must still be \
                 readable, got {} with body {:?}",
                still.status,
                text(&still)
            );
            assert_eq!(
                still.body.as_ref(),
                rows[other_index].2.as_slice(),
                "case {case:?}: expected {other:?} to keep its own {} payload bytes, got {}",
                rows[other_index].2.len(),
                still.body.len()
            );
        }

        // And the object itself is untouched by the removal of one of its annotations.
        let object_read = get_object(
            &router,
            "annotation-removal-bucket",
            "reports/q1.csv",
            "",
            &[],
        )
        .await;
        assert_eq!(
            object_read.status,
            200,
            "case {case:?}: the object must survive the removal, got {} with body {:?}",
            object_read.status,
            text(&object_read)
        );
        assert_eq!(
            object_read.body.as_ref(),
            body.as_slice(),
            "case {case:?}: expected the object's own bytes back, got {:?}",
            String::from_utf8_lossy(&object_read.body)
        );
        assert_eq!(
            object_read.header("etag").as_deref(),
            Some(expected_etag(body).as_str()),
            "case {case:?}: expected the object to keep its entity tag, got {:?}",
            object_read.header("etag")
        );
    }
}

/// One row of the address table: the case name, the bucket, the key, the annotation name,
/// the status that address answers, and the error code and `<Message>` it carries.
type AddressRow<'a> = (&'a str, &'a str, &'a str, String, u16, &'a str, &'a str);

#[tokio::test]
async fn a_delete_is_addressed_by_bucket_key_and_name() {
    let router = router();
    bucket(&router, "annotation-address-bucket").await;
    object(&router, "annotation-address-bucket", "present.txt", b"here").await;
    annotation(
        &router,
        "annotation-address-bucket",
        "present.txt",
        "note",
        b"attached",
        "",
    )
    .await;

    // The rows walk the chain bucket → object → annotation: each link absent in turn, a key
    // that is only a PREFIX of a stored one, two names no write could ever have recorded
    // (one byte past the published 512-byte bound, and a reserved prefix), and the address
    // that is there. The two unrecordable names are the rule's edge: the name is judged
    // before the chain is walked, so the delete answers the same name-rule refusal the write
    // of that name does.
    let rows: [AddressRow<'_>; 6] = [
        (
            "a bucket this emulator does not hold",
            "no-such-bucket",
            "present.txt",
            "note".to_string(),
            404,
            "NoSuchBucket",
            NO_SUCH_BUCKET_MESSAGE,
        ),
        (
            "a key the bucket does not hold",
            "annotation-address-bucket",
            "absent.txt",
            "note".to_string(),
            404,
            "NoSuchKey",
            NO_SUCH_KEY_MESSAGE,
        ),
        (
            "a key that is a prefix of the stored one",
            "annotation-address-bucket",
            "present",
            "note".to_string(),
            404,
            "NoSuchKey",
            NO_SUCH_KEY_MESSAGE,
        ),
        (
            "a name the object does not carry",
            "annotation-address-bucket",
            "present.txt",
            "never-written".to_string(),
            404,
            "NoSuchAnnotation",
            NO_SUCH_ANNOTATION_MESSAGE,
        ),
        (
            "a name one byte past the published maximum, which no write could record",
            "annotation-address-bucket",
            "present.txt",
            name_of(MAX_ANNOTATION_NAME_BYTES + 1),
            400,
            "AnnotationNameTooLong",
            ANNOTATION_NAME_TOO_LONG_MESSAGE,
        ),
        (
            "a name under a prefix S3 reserves, which no write could record",
            "annotation-address-bucket",
            "present.txt",
            "aws-owned".to_string(),
            400,
            "InvalidAnnotation",
            RESERVED_ANNOTATION_PREFIX_MESSAGE,
        ),
    ];

    for (case, bucket_name, key, name, status, code, message) in rows {
        let deleted = delete_annotation(&router, bucket_name, key, &name, "", &[]).await;
        assert_eq!(
            deleted.status, status,
            "case {case:?}: expected HTTP {status} from DeleteObjectAnnotation, got {} with \
             body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.error_code(),
            code,
            "case {case:?}: expected the error code {code:?}, got {:?}",
            deleted.error_code()
        );
        assert_eq!(
            deleted.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            deleted.error_message()
        );
        // No refusal took the annotation that IS there away.
        let still =
            get_annotation(&router, "annotation-address-bucket", "present.txt", "note").await;
        assert_eq!(
            still.status,
            200,
            "case {case:?}: a refused delete removes nothing, so the stored annotation must \
             still read back, got {} with body {:?}",
            still.status,
            text(&still)
        );
    }

    // The address that IS there is a 204, and a second delete of the same name finds
    // nothing: "Deleting an annotation is permanent … no delete marker or way to recover a
    // deleted annotation", so the delete is not idempotent in its answer.
    for (attempt, status, code) in [(1u8, 204u16, ""), (2, 404, "NoSuchAnnotation")] {
        let deleted = delete_annotation(
            &router,
            "annotation-address-bucket",
            "present.txt",
            "note",
            "",
            &[],
        )
        .await;
        assert_eq!(
            deleted.status, status,
            "case \"delete attempt {attempt}\": expected HTTP {status}, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.error_code(),
            code,
            "case \"delete attempt {attempt}\": expected the error code {code:?}, got {:?}",
            deleted.error_code()
        );
    }
}

#[tokio::test]
async fn a_conditional_delete_only_succeeds_on_a_matching_object_etag() {
    let router = router();
    bucket(&router, "annotation-if-match-bucket").await;
    let body = b"the object this annotation hangs on";
    object(&router, "annotation-if-match-bucket", "conditional.txt", body).await;
    let etag = expected_etag(body);

    // (case, the `x-amz-object-if-match` value, the status it answers, its code). The
    // matching tag is last, so the two refusals are judged against an annotation that is
    // still there.
    let rows: [(&str, String, u16, &str); 3] = [
        (
            "the entity tag of other bytes",
            expected_etag(b"something else"),
            412,
            "PreconditionFailed",
        ),
        (
            "a value that is no entity tag at all",
            "\"not-a-digest\"".to_string(),
            412,
            "PreconditionFailed",
        ),
        ("the object's own entity tag", etag.clone(), 204, ""),
    ];

    for (case, condition, status, code) in rows {
        annotation(
            &router,
            "annotation-if-match-bucket",
            "conditional.txt",
            "conditional-note",
            b"only deleted on a match",
            "",
        )
        .await;
        let deleted = delete_annotation(
            &router,
            "annotation-if-match-bucket",
            "conditional.txt",
            "conditional-note",
            "",
            &[("x-amz-object-if-match", condition.as_str())],
        )
        .await;
        assert_eq!(
            deleted.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.error_code(),
            code,
            "case {case:?}: expected the error code {code:?}, got {:?}",
            deleted.error_code()
        );

        let read = get_annotation(
            &router,
            "annotation-if-match-bucket",
            "conditional.txt",
            "conditional-note",
        )
        .await;
        let expected_read = if status == 204 { 404 } else { 200 };
        assert_eq!(
            read.status, expected_read,
            "case {case:?}: expected HTTP {expected_read} reading the annotation afterwards, \
             got {} with body {:?}",
            read.status,
            text(&read)
        );
    }
}

#[tokio::test]
async fn a_delete_names_the_object_version_it_removed_the_annotation_from() {
    let router = router();
    bucket(&router, "annotation-version-bucket").await;
    set_versioning(&router, "annotation-version-bucket", "Enabled").await;

    // Two versions of one key, each carrying an annotation of the same name: the annotations
    // of a version live with that version, so a delete addressed at one leaves the other's
    // alone.
    let mut versions: Vec<String> = Vec::new();
    for body in [&b"first revision"[..], &b"second revision"[..]] {
        let written = object(&router, "annotation-version-bucket", "versioned.txt", body).await;
        let version = written.header("x-amz-version-id").unwrap_or_default();
        assert!(
            !version.is_empty(),
            "a write into a versioning-enabled bucket reports a version id, got none"
        );
        annotation(
            &router,
            "annotation-version-bucket",
            "versioned.txt",
            "stage",
            body,
            &format!("&versionId={}", url_encoded_key(&version)),
        )
        .await;
        versions.push(version);
    }
    let [noncurrent, current]: [String; 2] = versions
        .try_into()
        .unwrap_or_else(|held: Vec<String>| panic!("two versions were written, got {held:?}"));

    // (case, the versionId the delete names, the version the answer must report).
    let rows: [(&str, Option<&str>, &str); 2] = [
        (
            "the noncurrent version, named",
            Some(noncurrent.as_str()),
            noncurrent.as_str(),
        ),
        (
            "no version at all, which is the current one",
            None,
            current.as_str(),
        ),
    ];

    for (case, named, reported) in rows {
        let query = named
            .map(|version| format!("&versionId={}", url_encoded_key(version)))
            .unwrap_or_default();
        let deleted = delete_annotation(
            &router,
            "annotation-version-bucket",
            "versioned.txt",
            "stage",
            &query,
            &[],
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: expected HTTP 204, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.header("x-amz-object-version-id").as_deref(),
            Some(reported),
            "case {case:?}: expected the deleted-from version {reported:?}, got {:?}",
            deleted.header("x-amz-object-version-id")
        );

        // The version this delete named no longer carries the name, and the OTHER version
        // still does when it has not been deleted yet.
        let gone = get_annotation_at(
            &router,
            "annotation-version-bucket",
            "versioned.txt",
            "stage",
            &query,
        )
        .await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the named version's annotation must be gone, got {} with body {:?}",
            gone.status,
            text(&gone)
        );
        assert_eq!(
            gone.error_code(),
            "NoSuchAnnotation",
            "case {case:?}: expected NoSuchAnnotation, got {:?}",
            gone.error_code()
        );
        if named.is_some() {
            let other = get_annotation(
                &router,
                "annotation-version-bucket",
                "versioned.txt",
                "stage",
            )
            .await;
            assert_eq!(
                other.status,
                200,
                "case {case:?}: the version the delete did not name keeps its annotation, got \
                 {} with body {:?}",
                other.status,
                text(&other)
            );
        }
    }
}

#[tokio::test]
async fn a_delete_for_an_owner_this_emulator_does_not_serve_is_denied_and_removes_nothing() {
    let router = router();
    bucket(&router, "annotation-owner-bucket").await;
    object(&router, "annotation-owner-bucket", "owned.txt", b"owned bytes").await;
    annotation(
        &router,
        "annotation-owner-bucket",
        "owned.txt",
        "owner",
        b"data team",
        "",
    )
    .await;

    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the test's non-owner account id must not be the account this emulator serves"
    );
    let denied = delete_annotation(
        &router,
        "annotation-owner-bucket",
        "owned.txt",
        "owner",
        "",
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
    )
    .await;
    assert_eq!(
        denied.status,
        403,
        "case \"an owner this emulator does not serve\": expected HTTP 403, got {} with body {:?}",
        denied.status,
        text(&denied)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "case \"an owner this emulator does not serve\": expected AccessDenied, got {:?}",
        denied.error_code()
    );
    let still = get_annotation(&router, "annotation-owner-bucket", "owned.txt", "owner").await;
    assert_eq!(
        still.status,
        200,
        "case \"an owner this emulator does not serve\": a denied delete removes nothing, got \
         {} with body {:?}",
        still.status,
        text(&still)
    );

    let allowed = delete_annotation(
        &router,
        "annotation-owner-bucket",
        "owned.txt",
        "owner",
        "",
        &[(
            "x-amz-expected-bucket-owner",
            engine::emulator_account_id(),
        )],
    )
    .await;
    assert_eq!(
        allowed.status,
        204,
        "case \"the owner this emulator serves\": expected HTTP 204, got {} with body {:?}",
        allowed.status,
        text(&allowed)
    );
    let gone = get_annotation(&router, "annotation-owner-bucket", "owned.txt", "owner").await;
    assert_eq!(
        gone.status, 404,
        "case \"the owner this emulator serves\": the annotation must be gone, got {} with \
         body {:?}",
        gone.status,
        text(&gone)
    );
}
