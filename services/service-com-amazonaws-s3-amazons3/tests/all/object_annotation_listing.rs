// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListObjectAnnotations`: the LISTING of the annotations one stored
//! object version carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the object
//! writer and the annotation writer and reader are the ones [`super::bucket_lifecycle`],
//! [`super::object_and_tagging_roundtrip`], [`super::object_annotations`] and
//! [`super::object_versioning`] already own), so what is pinned is the wire behaviour —
//! status, bound headers, body, error code and message — rather than any handler's
//! internals.
//!
//! The rules under test, with their sources:
//!
//! * "Lists the annotations attached to an Amazon S3 object" (the operation's own
//!   documentation): the response reports exactly the annotations the writes recorded on THAT
//!   object version, each with the name it was written under and the size, entity tag and
//!   last-modified instant of the payload stored under it, in ascending name order. An object
//!   carrying none reports an empty listing.
//! * "Use the AnnotationPrefix parameter to filter the results by name prefix" (same
//!   documentation, and `ListObjectAnnotationsRequest$AnnotationPrefix`: "filter results to
//!   annotations whose name begins with the specified prefix"), and the response echoes "the
//!   prefix used to filter the response". A prefix no acceptable annotation name could begin
//!   with — past the 512-byte name maximum
//!   (`PutObjectAnnotationRequest$AnnotationName`), or under a prefix S3 reserves
//!   (`DeleteObjectAnnotationRequest$AnnotationName`: names "cannot start with `aws` or `s3`
//!   (case-insensitive)") — is the modelled `InvalidPrefix` (HTTP 400), whose `<Message>` is
//!   the sentence S3 sends on the wire for that code.
//! * "Results are paginated, with a maximum of 1,000 annotations per object" (same
//!   documentation): `MaxAnnotationResults` caps a page, the pages partition the selected
//!   annotations in name order, each page echoes "the continuation token used in this
//!   request", and `NextContinuationToken` is present exactly while annotations remain. A
//!   token the service did not issue is `InvalidArgument` (HTTP 400).
//! * A listing is a REPORT: two listings of an unchanged object report the same thing, and
//!   neither consumes nor hides what a later read sees.
//! * The listing is addressed by bucket, key and (optionally) `versionId`, so a missing link
//!   of that chain answers its own failure: `NoSuchBucket`, `NoSuchKey` or `NoSuchVersion`,
//!   each HTTP 404, and a `versionId` that is not shaped like one is `InvalidArgument` (HTTP
//!   400). The annotations of a version are that version's alone.
//! * An `x-amz-expected-bucket-owner` that is not this emulator's account is denied (403).

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    url_encoded_key, INVALID_ARGUMENT_CODE, INVALID_VERSION_ID_MESSAGE,
    MAX_ANNOTATION_NAME_BYTES, NO_SUCH_BUCKET_CODE, NO_SUCH_KEY_CODE, NO_SUCH_VERSION_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{bucket, expected_etag, put_object, text};
use super::object_annotations::{get_annotation, put_annotation, put_annotation_at};
use super::object_versioning::set_versioning;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has.
const NOT_THE_OWNER: &str = "123456789012";

/// The `<Message>` S3 sends for a prefix this namespace has no names under, as it reads on
/// the wire: "The annotation prefix you have provided is invalid."
const INVALID_PREFIX_MESSAGE: &str = "The annotation prefix you have provided is invalid.";

/// The page size a listing applies when the request names none: "Results are paginated, with
/// a maximum of 1,000 annotations per object" (`ListObjectAnnotations`'s own documentation).
const DEFAULT_MAX_ANNOTATION_RESULTS: i64 = 1_000;

/// A version id of the SHAPE S3 issues that this store never minted, so it names no stored
/// version of any object (32 characters drawn from the alphanumerics, the published width).
const UNISSUED_VERSION_ID: &str = "0123456789abcdef0123456789abcdef";

/// `GET /<bucket>/<key>?annotation&x-id=ListObjectAnnotations` — `ListObjectAnnotations`,
/// with whatever extra query parameters and bound headers the case under test carries.
///
/// The `x-id` literal is what the model binds to separate this listing from
/// `GetObjectAnnotation`, which is bound to the same method, path and `?annotation`.
async fn list_annotations(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = format!("/{bucket}/{key}?annotation&x-id=ListObjectAnnotations{query}");
    send_bytes(router, "GET", &uri, headers, b"").await
}

/// One annotation a listing reports: the members of an `AnnotationEntry` that name a stored
/// fact about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Entry {
    name: String,
    size: i64,
    etag: String,
    last_modified: String,
}

/// EVERY `AnnotationEntry` a listing body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// configuration suites in this crate read a container, rather than by slicing the body. The
/// member a value belongs to is its PATH, so the `Key` and `ETag` of the response's own
/// envelope are not read as an entry's.
fn reported_entries(answer: &Answer) -> Vec<Entry> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);
    let mut entries: Vec<Entry> = Vec::new();
    let mut inside_entry = false;
    let mut element = String::new();
    let mut current = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
                // An entry's OPENING is what declares one present, so an entry whose
                // members are all empty is still reported.
                if element == "AnnotationEntry" {
                    inside_entry = true;
                    entries.push(Entry::default());
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                current.push_str(&unescaped(&decoded));
            }
            // An entity reference arrives as its own event, so the two halves of a quoted
            // entity tag arrive around it rather than as one run of text.
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                current.push_str(&unescaped(&format!("&{name};")));
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                let value = std::mem::take(&mut current);
                if closing == "AnnotationEntry" {
                    inside_entry = false;
                } else if inside_entry && closing == element {
                    // The member a value belongs to is the element that just closed INSIDE
                    // an entry, so the `Key` and `ETag` of the response's own envelope are
                    // not read as an entry's.
                    if let Some(entry) = entries.last_mut() {
                        match closing.as_str() {
                            "AnnotationName" => entry.name = value,
                            "ETag" => entry.etag = value,
                            "LastModified" => entry.last_modified = value,
                            "Size" => {
                                entry.size = value.parse().unwrap_or_else(|error| {
                                    panic!(
                                        "an entry's Size must be an integer, got {value:?}: \
                                         {error}"
                                    )
                                });
                            }
                            _ => {}
                        }
                    }
                }
                element.clear();
                current.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    entries
}

/// The names a listing reports, in the order it reported them.
fn reported_names(answer: &Answer) -> Vec<String> {
    reported_entries(answer)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
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
/// precondition of every listing case below.
async fn annotation(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    name: &str,
    payload: &[u8],
) {
    let put = put_annotation(router, bucket_name, key, name, payload, &[]).await;
    assert_eq!(
        put.status,
        200,
        "the annotation {name:?} must be written first, got {} with body {:?}",
        put.status,
        text(&put)
    );
}

/// A name of exactly `bytes` ASCII bytes, for the rows that sit on a published bound.
fn name_of(bytes: usize) -> String {
    "p".repeat(bytes)
}

/// One row of the listing table: the case, and the annotations the object carries as
/// `(name, payload)`.
type ListingRow<'a> = (&'a str, Vec<(String, Vec<u8>)>);

#[tokio::test]
async fn a_listing_reports_every_annotation_the_writes_recorded() {
    let router = router();
    bucket(&router, "annotation-listing-bucket").await;

    // The rows differ along what the rule generalises over: HOW MANY annotations the object
    // carries (none, one, a handful, more than the default page would ever need), the FORM
    // of their names (one byte, a dotted namespace, multi-byte UTF-8, the 512-byte maximum)
    // and the SIZE of their payloads (one byte up to a sentence) — and the write order is
    // deliberately not the order a listing reports.
    let many: Vec<(String, Vec<u8>)> = (0..12)
        .map(|index| (format!("batch-{index:02}"), format!("row {index}").into_bytes()))
        .collect();
    let rows: [ListingRow<'_>; 5] = [
        ("an object carrying no annotation at all", Vec::new()),
        (
            "one annotation of one byte",
            vec![("x".to_string(), b"1".to_vec())],
        ),
        (
            "three written out of name order",
            vec![
                ("zeta".to_string(), b"last by name".to_vec()),
                ("alpha".to_string(), b"first by name".to_vec()),
                ("mid.point".to_string(), b"between the two".to_vec()),
            ],
        ),
        (
            "names of varied form, including the published maximum",
            vec![
                ("com.example.pipeline.stage".to_string(), b"ingest".to_vec()),
                (
                    "données-été".to_string(),
                    "valeur — oui".as_bytes().to_vec(),
                ),
                ("my-aws-notes".to_string(), b"not reserved".to_vec()),
                (name_of(MAX_ANNOTATION_NAME_BYTES), b"on the bound".to_vec()),
            ],
        ),
        ("twelve annotations", many),
    ];

    for (case, annotations) in rows {
        let key = format!("listing/{}.csv", annotations.len());
        object(&router, "annotation-listing-bucket", &key, b"region,total\n").await;
        for (name, payload) in &annotations {
            annotation(
                &router,
                "annotation-listing-bucket",
                &key,
                name,
                payload,
            )
            .await;
        }

        let listed = list_annotations(&router, "annotation-listing-bucket", &key, "", &[]).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected HTTP 200 from ListObjectAnnotations, got {} with body {:?}",
            listed.status,
            text(&listed)
        );

        // Ascending name order, which is the order S3 reports a listing in and NOT the
        // order these annotations were written in.
        let mut expected = annotations.clone();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        let entries = reported_entries(&listed);
        assert_eq!(
            entries.len(),
            expected.len(),
            "case {case:?}: expected {} entries, got {:?} from body {:?}",
            expected.len(),
            entries,
            text(&listed)
        );
        for (entry, (name, payload)) in entries.iter().zip(expected.iter()) {
            assert_eq!(
                &entry.name, name,
                "case {case:?}: expected the entry {name:?} in name order, got {:?}",
                entry.name
            );
            assert_eq!(
                entry.size,
                i64::try_from(payload.len()).unwrap_or(i64::MAX),
                "case {case:?}: expected the stored payload's size {} for {name:?}, got {}",
                payload.len(),
                entry.size
            );
            assert_eq!(
                entry.etag,
                expected_etag(payload),
                "case {case:?}: expected the stored payload's entity tag {:?} for {name:?}, \
                 got {:?}",
                expected_etag(payload),
                entry.etag
            );
            assert!(
                !entry.last_modified.is_empty(),
                "case {case:?}: a stored annotation reports when it was written, got none for \
                 {name:?}"
            );
        }

        // The envelope reports the address the listing was taken from, how many entries it
        // carries and the page size it applied, and nothing is left for a next page.
        assert_eq!(
            listed.element("Bucket").as_deref(),
            Some("annotation-listing-bucket"),
            "case {case:?}: expected the listing to name its bucket, got {:?}",
            listed.element("Bucket")
        );
        assert_eq!(
            listed.element("Key").as_deref(),
            Some(key.as_str()),
            "case {case:?}: expected the listing to name its key {key:?}, got {:?}",
            listed.element("Key")
        );
        assert_eq!(
            listed.element("AnnotationCount"),
            Some(expected.len().to_string()),
            "case {case:?}: expected AnnotationCount {}, got {:?}",
            expected.len(),
            listed.element("AnnotationCount")
        );
        assert_eq!(
            listed.element("MaxAnnotationResults"),
            Some(DEFAULT_MAX_ANNOTATION_RESULTS.to_string()),
            "case {case:?}: expected the published page size {DEFAULT_MAX_ANNOTATION_RESULTS} \
             when the request named none, got {:?}",
            listed.element("MaxAnnotationResults")
        );
        assert_eq!(
            listed.element("NextContinuationToken"),
            None,
            "case {case:?}: a listing that reported every annotation hands back no \
             continuation token, got {:?}",
            listed.element("NextContinuationToken")
        );
        // The bucket has no versioning state, so its objects carry no version id of their
        // own and none is reported rather than one being invented.
        assert_eq!(
            listed.header("x-amz-object-version-id"),
            None,
            "case {case:?}: an object with no version id reports none, got {:?}",
            listed.header("x-amz-object-version-id")
        );
        assert_eq!(
            listed.element("AnnotationPrefix"),
            None,
            "case {case:?}: a listing the request did not filter reports no prefix, got {:?}",
            listed.element("AnnotationPrefix")
        );
    }
}

/// One row of the prefix table: the case, the prefix the request carries (when any), and the
/// annotation names it must select.
type PrefixRow<'a> = (&'a str, Option<&'a str>, Vec<&'a str>);

/// The annotation names every prefix row below filters over, written on one object.
const FILTERED_NAMES: [&str; 7] = [
    "alpha",
    "alpha.beta",
    "beta",
    "données-été",
    "report/2024",
    "report/2025",
    "zeta",
];

#[tokio::test]
async fn a_listing_filters_by_annotation_prefix() {
    let router = router();
    bucket(&router, "annotation-prefix-bucket").await;
    object(&router, "annotation-prefix-bucket", "filtered.txt", b"body").await;
    for name in FILTERED_NAMES {
        annotation(
            &router,
            "annotation-prefix-bucket",
            "filtered.txt",
            name,
            name.as_bytes(),
        )
        .await;
    }

    // The rows differ along what the rule generalises over: no prefix at all, the empty
    // prefix (which filters nothing out), a prefix that is itself a whole name, one that is
    // a strict extension of a name, a path-like prefix selecting several, a multi-byte UTF-8
    // prefix, one that selects nothing, and one at the published 512-byte name maximum.
    let at_the_bound = name_of(MAX_ANNOTATION_NAME_BYTES);
    let rows: [PrefixRow<'_>; 8] = [
        ("no prefix at all", None, FILTERED_NAMES.to_vec()),
        (
            "the empty prefix, which filters nothing out",
            Some(""),
            FILTERED_NAMES.to_vec(),
        ),
        (
            "a prefix that is itself a name",
            Some("alpha"),
            vec!["alpha", "alpha.beta"],
        ),
        (
            "a strict extension of that name",
            Some("alpha."),
            vec!["alpha.beta"],
        ),
        (
            "a path-like prefix",
            Some("report/"),
            vec!["report/2024", "report/2025"],
        ),
        (
            "a prefix of multi-byte UTF-8 text",
            Some("donné"),
            vec!["données-été"],
        ),
        ("a prefix nothing is named under", Some("omega"), Vec::new()),
        (
            "a prefix at the published name maximum",
            Some(at_the_bound.as_str()),
            Vec::new(),
        ),
    ];

    for (case, prefix, expected) in rows {
        let query = prefix
            .map(|prefix| format!("&annotation-prefix={}", url_encoded_key(prefix)))
            .unwrap_or_default();
        let listed = list_annotations(
            &router,
            "annotation-prefix-bucket",
            "filtered.txt",
            &query,
            &[],
        )
        .await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            reported_names(&listed),
            expected,
            "case {case:?}: expected exactly the names beginning with {prefix:?}, got {:?} \
             from body {:?}",
            reported_names(&listed),
            text(&listed)
        );
        assert_eq!(
            listed.element("AnnotationCount"),
            Some(expected.len().to_string()),
            "case {case:?}: expected AnnotationCount {}, got {:?}",
            expected.len(),
            listed.element("AnnotationCount")
        );
        // "The prefix used to filter the response": the one the request carried, and
        // nothing when it carried none. An empty prefix filtered nothing, so it is reported
        // as the empty element it was sent as or not at all — what matters is that no OTHER
        // prefix is claimed.
        let reported_prefix = listed.element("AnnotationPrefix");
        let expected_prefix = prefix.filter(|prefix| !prefix.is_empty());
        assert_eq!(
            reported_prefix.as_deref().filter(|value| !value.is_empty()),
            expected_prefix,
            "case {case:?}: expected the filtering prefix {expected_prefix:?} echoed, got \
             {reported_prefix:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_through_the_annotations_in_name_order() {
    let router = router();
    bucket(&router, "annotation-paging-bucket").await;
    object(&router, "annotation-paging-bucket", "paged.txt", b"body").await;
    // Seven annotations, so the page sizes below divide the set evenly, unevenly and not at
    // all.
    let names: Vec<String> = (1..=7).map(|index| format!("note-{index}")).collect();
    for name in &names {
        annotation(
            &router,
            "annotation-paging-bucket",
            "paged.txt",
            name,
            name.as_bytes(),
        )
        .await;
    }

    // (case, the page size the request names, the number of pages it must take). Each row is
    // a different relation between the page size and the seven annotations: one at a time,
    // an uneven split, an even split, one short of the whole set, exactly the whole set, and
    // the published maximum.
    let rows: [(&str, Option<i32>, usize); 7] = [
        ("one annotation per page", Some(1), 7),
        ("two per page, the last page short", Some(2), 4),
        ("three per page, the last page short", Some(3), 3),
        ("six per page, one left over", Some(6), 2),
        ("exactly the whole set", Some(7), 1),
        ("the published maximum", Some(1_000), 1),
        ("no page size at all", None, 1),
    ];

    for (case, page_size, expected_pages) in rows {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = page_size
                .map(|size| format!("&max-annotation-results={size}"))
                .unwrap_or_default();
            if let Some(token) = &token {
                query.push_str(&format!("&continuation-token={}", url_encoded_key(token)));
            }
            let listed = list_annotations(
                &router,
                "annotation-paging-bucket",
                "paged.txt",
                &query,
                &[],
            )
            .await;
            assert_eq!(
                listed.status,
                200,
                "case {case:?}: expected HTTP 200 on page {pages}, got {} with body {:?}",
                listed.status,
                text(&listed)
            );
            let page = reported_names(&listed);
            let limit = page_size.unwrap_or(1_000);
            assert!(
                page.len() <= usize::try_from(limit).unwrap_or(0),
                "case {case:?}: a page carries at most {limit} entries, got {} ({page:?})",
                page.len()
            );
            assert_eq!(
                listed.element("AnnotationCount"),
                Some(page.len().to_string()),
                "case {case:?}: expected AnnotationCount {} on page {pages}, got {:?}",
                page.len(),
                listed.element("AnnotationCount")
            );
            // "The continuation token used in this request", reported back exactly as sent.
            assert_eq!(
                listed.element("ContinuationToken"),
                token.clone(),
                "case {case:?}: expected the request's own token {token:?} echoed on page \
                 {pages}, got {:?}",
                listed.element("ContinuationToken")
            );
            assert_eq!(
                listed.element("MaxAnnotationResults"),
                Some(limit.to_string()),
                "case {case:?}: expected the applied page size {limit} on page {pages}, got \
                 {:?}",
                listed.element("MaxAnnotationResults")
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: the pages must make progress, got {pages} of them for {} \
                 annotations",
                names.len()
            );
            match listed.element("NextContinuationToken") {
                Some(next) => {
                    assert!(
                        collected.len() < names.len(),
                        "case {case:?}: a listing that reported every annotation must hand \
                         back no token, got {next:?}"
                    );
                    token = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) over {} annotations, got {pages}",
            names.len()
        );
        assert_eq!(
            collected, names,
            "case {case:?}: the pages must partition the annotations in name order, got {collected:?}"
        );
    }

    // A page size and a prefix together: the filter chooses the set, the page size cuts it.
    let listed = list_annotations(
        &router,
        "annotation-paging-bucket",
        "paged.txt",
        "&annotation-prefix=note-&max-annotation-results=2",
        &[],
    )
    .await;
    assert_eq!(
        reported_names(&listed),
        vec!["note-1".to_string(), "note-2".to_string()],
        "case \"a prefix and a page size together\": expected the first two of the filtered \
         set, got {:?}",
        reported_names(&listed)
    );
    assert!(
        listed.element("NextContinuationToken").is_some(),
        "case \"a prefix and a page size together\": five annotations are left, so the page \
         hands back a token, got none from body {:?}",
        text(&listed)
    );
}

#[tokio::test]
async fn a_listing_reports_state_and_changes_nothing() {
    let router = router();
    bucket(&router, "annotation-report-bucket").await;
    object(&router, "annotation-report-bucket", "report.txt", b"body").await;
    for name in ["first", "second"] {
        annotation(
            &router,
            "annotation-report-bucket",
            "report.txt",
            name,
            name.as_bytes(),
        )
        .await;
    }

    let first = list_annotations(&router, "annotation-report-bucket", "report.txt", "", &[]).await;
    let second = list_annotations(&router, "annotation-report-bucket", "report.txt", "", &[]).await;
    assert_eq!(
        reported_entries(&first),
        reported_entries(&second),
        "case \"two listings of an unchanged object\": expected the same report twice, got \
         {:?} then {:?}",
        reported_entries(&first),
        reported_entries(&second)
    );

    // A listing neither consumes nor hides what a later read sees.
    let read = get_annotation(&router, "annotation-report-bucket", "report.txt", "first").await;
    assert_eq!(
        read.status,
        200,
        "case \"a read after a listing\": expected HTTP 200, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.body.as_ref(),
        b"first",
        "case \"a read after a listing\": expected the stored payload back, got {:?}",
        text(&read)
    );

    // And it tracks the state: an annotation written after the first listing is reported by
    // the next one, under the entity tag of ITS payload.
    annotation(
        &router,
        "annotation-report-bucket",
        "report.txt",
        "third",
        b"a later write",
    )
    .await;
    let third = list_annotations(&router, "annotation-report-bucket", "report.txt", "", &[]).await;
    assert_eq!(
        reported_names(&third),
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "case \"a write between two listings\": expected the new annotation reported, got {:?}",
        reported_names(&third)
    );
    let entry = reported_entries(&third)
        .into_iter()
        .find(|entry| entry.name == "third")
        .unwrap_or_else(|| panic!("the listing must carry the annotation just written"));
    assert_eq!(
        entry.etag,
        expected_etag(b"a later write"),
        "case \"a write between two listings\": expected the new payload's entity tag {:?}, \
         got {:?}",
        expected_etag(b"a later write"),
        entry.etag
    );
}

#[tokio::test]
async fn annotations_are_listed_per_object_version() {
    let router = router();
    bucket(&router, "annotation-listing-version-bucket").await;
    set_versioning(&router, "annotation-listing-version-bucket", "Enabled").await;

    // Two versions of one key, each carrying annotations of its own: the annotations of a
    // version live with that version, so a listing addressed at one reports only its own.
    let mut versions: Vec<String> = Vec::new();
    for (body, names) in [
        (&b"first revision"[..], ["only-on-first", "shared"]),
        (&b"second revision"[..], ["only-on-second", "shared"]),
    ] {
        let written = object(
            &router,
            "annotation-listing-version-bucket",
            "versioned.txt",
            body,
        )
        .await;
        let version = written.header("x-amz-version-id").unwrap_or_default();
        assert!(
            !version.is_empty(),
            "a write into a versioning-enabled bucket reports a version id, got none"
        );
        for name in names {
            let put = put_annotation_at(
                &router,
                "annotation-listing-version-bucket",
                "versioned.txt",
                name,
                &format!("&versionId={}", url_encoded_key(&version)),
                body,
                &[],
            )
            .await;
            assert_eq!(
                put.status,
                200,
                "the annotation {name:?} must be written first, got {} with body {:?}",
                put.status,
                text(&put)
            );
        }
        versions.push(version);
    }
    let [noncurrent, current]: [String; 2] = versions
        .try_into()
        .unwrap_or_else(|held: Vec<String>| panic!("two versions were written, got {held:?}"));

    // (case, the versionId the listing names, the names it must report, the version it must
    // report).
    let rows: [(&str, Option<&str>, Vec<&str>, &str); 3] = [
        (
            "the noncurrent version, named",
            Some(noncurrent.as_str()),
            vec!["only-on-first", "shared"],
            noncurrent.as_str(),
        ),
        (
            "the current version, named",
            Some(current.as_str()),
            vec!["only-on-second", "shared"],
            current.as_str(),
        ),
        (
            "no version at all, which is the current one",
            None,
            vec!["only-on-second", "shared"],
            current.as_str(),
        ),
    ];

    for (case, named, expected, reported_version) in rows {
        let query = named
            .map(|version| format!("&versionId={}", url_encoded_key(version)))
            .unwrap_or_default();
        let listed = list_annotations(
            &router,
            "annotation-listing-version-bucket",
            "versioned.txt",
            &query,
            &[],
        )
        .await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            reported_names(&listed),
            expected,
            "case {case:?}: expected the annotations of that version alone, got {:?}",
            reported_names(&listed)
        );
        assert_eq!(
            listed.header("x-amz-object-version-id").as_deref(),
            Some(reported_version),
            "case {case:?}: expected the listed version {reported_version:?}, got {:?}",
            listed.header("x-amz-object-version-id")
        );
    }
}

/// One row of the refusal table: the case, the bucket and key the request names, the extra
/// query it carries, its extra headers, the status it must answer and the wire error code.
type RefusalRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    String,
    Vec<(&'a str, &'a str)>,
    u16,
    &'a str,
);

#[tokio::test]
async fn a_listing_outside_its_published_rules_is_refused() {
    let router = router();
    bucket(&router, "annotation-refusal-bucket").await;
    object(&router, "annotation-refusal-bucket", "present.txt", b"body").await;
    annotation(
        &router,
        "annotation-refusal-bucket",
        "present.txt",
        "kept",
        b"kept",
    )
    .await;

    // A prefix past the published 512-byte name maximum, in ASCII and in multi-byte UTF-8
    // where the two units differ: no acceptable name can begin with either.
    let past_the_bound = name_of(MAX_ANNOTATION_NAME_BYTES + 1);
    let past_the_bound_in_bytes = "é".repeat(MAX_ANNOTATION_NAME_BYTES / 2 + 1);
    assert_eq!(
        past_the_bound_in_bytes.len(),
        MAX_ANNOTATION_NAME_BYTES + 2,
        "the test's own multi-byte prefix must be past the bound in BYTES, got {} bytes",
        past_the_bound_in_bytes.len()
    );
    let rows: [RefusalRow<'_>; 11] = [
        (
            "a bucket this emulator does not hold",
            "annotation-refusal-absent",
            "present.txt",
            String::new(),
            Vec::new(),
            404,
            NO_SUCH_BUCKET_CODE,
        ),
        (
            "a key the bucket does not hold",
            "annotation-refusal-bucket",
            "absent.txt",
            String::new(),
            Vec::new(),
            404,
            NO_SUCH_KEY_CODE,
        ),
        (
            "a well-formed version id that names no stored version",
            "annotation-refusal-bucket",
            "present.txt",
            format!("&versionId={UNISSUED_VERSION_ID}"),
            Vec::new(),
            404,
            NO_SUCH_VERSION_CODE,
        ),
        (
            "a version id that is not shaped like one",
            "annotation-refusal-bucket",
            "present.txt",
            "&versionId=not+a+version+id".to_string(),
            Vec::new(),
            400,
            INVALID_ARGUMENT_CODE,
        ),
        (
            "a continuation token this service never issued",
            "annotation-refusal-bucket",
            "present.txt",
            "&continuation-token=notatoken".to_string(),
            Vec::new(),
            400,
            INVALID_ARGUMENT_CODE,
        ),
        (
            "a prefix one byte past the published name maximum",
            "annotation-refusal-bucket",
            "present.txt",
            format!("&annotation-prefix={past_the_bound}"),
            Vec::new(),
            400,
            "InvalidPrefix",
        ),
        (
            "a prefix within the character count but past the byte count",
            "annotation-refusal-bucket",
            "present.txt",
            format!(
                "&annotation-prefix={}",
                url_encoded_key(&past_the_bound_in_bytes)
            ),
            Vec::new(),
            400,
            "InvalidPrefix",
        ),
        (
            "a prefix under a reserved namespace",
            "annotation-refusal-bucket",
            "present.txt",
            "&annotation-prefix=aws-owned".to_string(),
            Vec::new(),
            400,
            "InvalidPrefix",
        ),
        (
            "a prefix that is exactly a reserved namespace",
            "annotation-refusal-bucket",
            "present.txt",
            "&annotation-prefix=s3".to_string(),
            Vec::new(),
            400,
            "InvalidPrefix",
        ),
        (
            "that reserved namespace in another case",
            "annotation-refusal-bucket",
            "present.txt",
            "&annotation-prefix=S3Internal".to_string(),
            Vec::new(),
            400,
            "InvalidPrefix",
        ),
        (
            "an expected bucket owner that is not this account",
            "annotation-refusal-bucket",
            "present.txt",
            String::new(),
            vec![("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
            403,
            "AccessDenied",
        ),
    ];

    for (case, bucket_name, key, query, headers, status, code) in rows {
        let listed = list_annotations(&router, bucket_name, key, &query, &headers).await;
        assert_eq!(
            listed.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed.error_code(),
            code,
            "case {case:?}: expected the error code {code:?}, got {:?}",
            listed.error_code()
        );
        assert!(
            !listed.error_message().is_empty(),
            "case {case:?}: an S3 error document carries a message beside its code, got none"
        );
        assert!(
            reported_entries(&listed).is_empty(),
            "case {case:?}: a refused listing reports no annotation, got {:?}",
            reported_entries(&listed)
        );
    }

    // The two refusals whose `<Message>` the service publishes verbatim.
    let malformed = list_annotations(
        &router,
        "annotation-refusal-bucket",
        "present.txt",
        "&versionId=not+a+version+id",
        &[],
    )
    .await;
    assert_eq!(
        malformed.error_message(),
        INVALID_VERSION_ID_MESSAGE,
        "case \"a version id that is not shaped like one\": expected the published message \
         {INVALID_VERSION_ID_MESSAGE:?}, got {:?}",
        malformed.error_message()
    );
    let invalid_prefix = list_annotations(
        &router,
        "annotation-refusal-bucket",
        "present.txt",
        "&annotation-prefix=aws-owned",
        &[],
    )
    .await;
    assert_eq!(
        invalid_prefix.error_message(),
        INVALID_PREFIX_MESSAGE,
        "case \"a prefix under a reserved namespace\": expected the published message \
         {INVALID_PREFIX_MESSAGE:?}, got {:?}",
        invalid_prefix.error_message()
    );

    // `MaxAnnotationResults` is `range 1..1000`, so each published bound is served and each
    // value one step past it is refused before the handler runs. (case, the page size, the
    // status it must answer.)
    let page_sizes: [(&str, i32, u16); 4] = [
        ("the published minimum", 1, 200),
        ("the published maximum", 1_000, 200),
        ("one below the minimum", 0, 400),
        ("one past the maximum", 1_001, 400),
    ];
    for (case, page_size, status) in page_sizes {
        let listed = list_annotations(
            &router,
            "annotation-refusal-bucket",
            "present.txt",
            &format!("&max-annotation-results={page_size}"),
            &[],
        )
        .await;
        assert_eq!(
            listed.status, status,
            "case {case:?}: expected HTTP {status} for max-annotation-results={page_size}, got \
             {} with body {:?}",
            listed.status,
            text(&listed)
        );
    }
}
