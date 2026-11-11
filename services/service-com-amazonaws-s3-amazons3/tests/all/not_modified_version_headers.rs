// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH REPRESENTATION a `304 Not Modified` answer names: the
//! version-bound header beside the two validators RFC 9110 requires.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`],
//! [`super::object_conditional_reads`] — whose HEAD driver and HTTP-date builder this suite
//! reuses — and [`super::object_versioning`] already own), so what is pinned is the wire
//! behaviour — status, bound headers and whether a body is carried — rather than any
//! handler's internals. `HeadObject` "has the same options as a GET operation on an
//! object", so every row is driven through both reads.
//!
//! The rule under test, asserted over a SET of inputs rather than one request:
//!
//! > A read whose conditions say the caller already holds the current representation is
//! > answered about ONE resolved version, so it reports that version's id in
//! > `x-amz-version-id` exactly as a served read of the same address does: the key's
//! > current version when the request addressed no version, the addressed version when it
//! > did, and no such header at all for an object of a bucket that mints no version ids.
//!
//! Its sources: `GetObjectOutput$VersionId` and `HeadObjectOutput$VersionId` are bound to
//! the `x-amz-version-id` header (`smithy_query(query="operation",operation="GetObject")`),
//! and that output is what both the served and the conditional-failure answer are
//! serialized from; "Return the object only if its entity tag (ETag) is different from the
//! one specified in this header; otherwise, return a 304 Not Modified error"
//! (`GetObjectRequest$IfNoneMatch`, and `$IfModifiedSince` names the same status);
//! "Enabled—Enables versioning for the objects in the bucket. All objects added to the
//! bucket receive a unique version ID. Suspended—Disables versioning for the objects in the
//! bucket" (`PutBucketVersioning`); and a 304 "MUST generate … ETag, Last-Modified" and
//! "MUST NOT generate content" (RFC 9110 §15.4.5), which every row also pins.
//!
//! The ids themselves are values the SERVICE mints per write, so nothing here asserts a
//! literal one: each row's expectation is the id a write reported or the id a served read of
//! the same address reports.

use std::time::{Duration, SystemTime};

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_conditional_reads::{head_object, http_date};
use super::object_versioning::set_versioning;

/// The header S3 names the version an answer is about in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// The bytes of the two representations the conditions below are judged against.
const OLDER_BODY: &[u8] = b"older-representation";
const NEWER_BODY: &[u8] = b"newer-representation";

/// A header of an answer, failing the test when the answer carries none.
fn header_of(answer: &Answer, name: &str, what: &str) -> String {
    answer
        .header(name)
        .unwrap_or_else(|| panic!("{what} must report {name}, got the body {:?}", text(answer)))
}

/// One row of the table: the case, the address read, the conditional headers it carries, the
/// entity tag of the representation it is about, and the version id the 304 must name
/// (`None` where the object carries no version id at all).
struct Row {
    case: &'static str,
    bucket: &'static str,
    key: &'static str,
    query: String,
    headers: Vec<(String, String)>,
    etag: String,
    expected: Option<String>,
}

#[tokio::test]
async fn a_not_modified_read_names_the_version_it_evaluated() {
    let router = router();

    // A bucket that MINTS version ids, holding two versions of one key.
    bucket(&router, "revalidate-versioned").await;
    set_versioning(&router, "revalidate-versioned", "Enabled").await;
    let older = put_object(&router, "revalidate-versioned", "held.bin", OLDER_BODY, &[]).await;
    let newer = put_object(&router, "revalidate-versioned", "held.bin", NEWER_BODY, &[]).await;
    for (what, written) in [("the older write", &older), ("the newer write", &newer)] {
        assert_eq!(
            written.status,
            200,
            "case {what:?}: the version the conditions are judged against must be stored, got \
             {} with body {:?}",
            written.status,
            text(written)
        );
    }
    let older_version = header_of(&older, VERSION_ID_HEADER, "a write to a versioned bucket");
    let newer_version = header_of(&newer, VERSION_ID_HEADER, "a write to a versioned bucket");
    assert_ne!(
        older_version, newer_version,
        "case two writes of one key: each version must carry its own id, got \
         {older_version:?} twice"
    );

    // A bucket that mints NONE, and one whose versioning was SUSPENDED before the write:
    // neither object carries a version id, so neither answer names one.
    bucket(&router, "revalidate-plain").await;
    let plain = put_object(&router, "revalidate-plain", "held.bin", OLDER_BODY, &[]).await;
    bucket(&router, "revalidate-suspended").await;
    set_versioning(&router, "revalidate-suspended", "Enabled").await;
    set_versioning(&router, "revalidate-suspended", "Suspended").await;
    let suspended = put_object(&router, "revalidate-suspended", "held.bin", OLDER_BODY, &[]).await;
    for (what, written) in [
        ("a bucket that was never versioned", &plain),
        ("a bucket whose versioning is suspended", &suspended),
    ] {
        assert_eq!(
            written.status,
            200,
            "case {what:?}: the object must be stored, got {} with body {:?}",
            written.status,
            text(written)
        );
        assert_eq!(
            written.header(VERSION_ID_HEADER),
            None,
            "case {what:?}: such a write mints no version id, got {:?}",
            written.header(VERSION_ID_HEADER)
        );
    }

    let older_etag = expected_etag(OLDER_BODY);
    let newer_etag = expected_etag(NEWER_BODY);
    let future = http_date(SystemTime::now() + Duration::from_secs(3_600));
    let row = |case: &'static str,
               bucket: &'static str,
               key: &'static str,
               query: String,
               headers: Vec<(&str, String)>,
               etag: &str,
               expected: Option<&String>| Row {
        case,
        bucket,
        key,
        query,
        headers: headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        etag: etag.to_string(),
        expected: expected.cloned(),
    };

    // The rows differ along the condition that is stated and the FORM of its value, the
    // versioning status of the bucket, and whether the read addresses a version by id or
    // takes the key's current one.
    let rows = vec![
        row(
            "the current version, If-None-Match naming its own tag",
            "revalidate-versioned",
            "held.bin",
            String::new(),
            vec![("if-none-match", newer_etag.clone())],
            &newer_etag,
            Some(&newer_version),
        ),
        row(
            "the current version, If-None-Match as the wildcard",
            "revalidate-versioned",
            "held.bin",
            String::new(),
            vec![("if-none-match", "*".to_string())],
            &newer_etag,
            Some(&newer_version),
        ),
        row(
            "the current version, If-None-Match as a list holding its tag",
            "revalidate-versioned",
            "held.bin",
            String::new(),
            vec![(
                "if-none-match",
                format!("\"00000000000000000000000000000000\", {newer_etag}"),
            )],
            &newer_etag,
            Some(&newer_version),
        ),
        row(
            "the current version, If-None-Match with the weak marker on its tag",
            "revalidate-versioned",
            "held.bin",
            String::new(),
            vec![("if-none-match", format!("W/{newer_etag}"))],
            &newer_etag,
            Some(&newer_version),
        ),
        row(
            "the current version, If-Modified-Since after it was written",
            "revalidate-versioned",
            "held.bin",
            String::new(),
            vec![("if-modified-since", future.clone())],
            &newer_etag,
            Some(&newer_version),
        ),
        row(
            "a noncurrent version addressed by id, If-None-Match naming its tag",
            "revalidate-versioned",
            "held.bin",
            format!("?versionId={older_version}"),
            vec![("if-none-match", older_etag.clone())],
            &older_etag,
            Some(&older_version),
        ),
        row(
            "a noncurrent version addressed by id, If-Modified-Since after it was written",
            "revalidate-versioned",
            "held.bin",
            format!("?versionId={older_version}"),
            vec![("if-modified-since", future.clone())],
            &older_etag,
            Some(&older_version),
        ),
        row(
            "an object of a bucket that mints no version ids",
            "revalidate-plain",
            "held.bin",
            String::new(),
            vec![("if-none-match", older_etag.clone())],
            &older_etag,
            None,
        ),
        row(
            "an object written while versioning was suspended",
            "revalidate-suspended",
            "held.bin",
            String::new(),
            vec![("if-none-match", "*".to_string())],
            &older_etag,
            None,
        ),
    ];

    for row in rows {
        let case = row.case;
        let headers: Vec<(&str, &str)> = row
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let read = get_object(&router, row.bucket, row.key, &row.query, &headers).await;
        let headed = head_object(&router, row.bucket, row.key, &row.query, &headers).await;

        // What the SAME address answers with no condition at all: the version a served read
        // names is the version the 304 has to name.
        let served = get_object(&router, row.bucket, row.key, &row.query, &[]).await;
        assert_eq!(
            served.header(VERSION_ID_HEADER),
            row.expected,
            "case {case:?}: a served read of this address must name the version {:?}, got {:?}",
            row.expected,
            served.header(VERSION_ID_HEADER)
        );

        for (operation, answer) in [("GetObject", &read), ("HeadObject", &headed)] {
            assert_eq!(
                answer.status,
                304,
                "case {case:?} on {operation}: expected HTTP 304, got {} with body {:?}",
                answer.status,
                text(answer)
            );
            assert!(
                answer.body.is_empty(),
                "case {case:?} on {operation}: a 304 carries no content, got {:?}",
                text(answer)
            );
            assert_eq!(
                answer.header("etag").as_deref(),
                Some(row.etag.as_str()),
                "case {case:?} on {operation}: a 304 reports the representation's entity tag \
                 {:?}, got {:?}",
                row.etag,
                answer.header("etag")
            );
            assert!(
                answer.header("last-modified").is_some(),
                "case {case:?} on {operation}: a 304 reports when the representation was \
                 written, got {:?}",
                answer.header("last-modified")
            );
            assert_eq!(
                answer.header(VERSION_ID_HEADER),
                row.expected,
                "case {case:?} on {operation}: a 304 must name the version {:?}, got {:?}",
                row.expected,
                answer.header(VERSION_ID_HEADER)
            );
        }
    }
}
