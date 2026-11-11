// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONDITIONS an object read states: `If-Match`,
//! `If-Unmodified-Since`, `If-None-Match` and `If-Modified-Since` on `GetObject` and
//! `HeadObject`.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire behaviour — status, the
//! validator headers and whether a body is carried — rather than any handler's internals.
//!
//! The rules under test, with their sources, all from the four members' own documentation on
//! `GetObjectRequest` (and `HeadObject` "has the same options as a GET operation on an
//! object"):
//!
//! * "Return the object only if its entity tag (ETag) is the same as the one specified in
//!   this header; otherwise, return a 412 Precondition Failed error" (`$IfMatch`), and the
//!   same 412 for a read whose object "has not been modified since the specified time"
//!   condition does not hold (`$IfUnmodifiedSince`).
//! * "Return the object only if its entity tag (ETag) is different from the one specified in
//!   this header; otherwise, return a 304 Not Modified error" (`$IfNoneMatch`), and the same
//!   304 when the object "has been modified since the specified time" condition does not
//!   hold (`$IfModifiedSince`).
//! * The PRECEDENCE within each pair: "If-Match condition evaluates to true, and;
//!   If-Unmodified-Since condition evaluates to false; then, S3 returns 200 OK and the data
//!   requested", and "If-None-Match condition evaluates to false, and; If-Modified-Since
//!   condition evaluates to true; then, S3 returns 304 Not Modified" — so the entity-tag
//!   condition of each pair decides the read on its own.
//! * The dates are HTTP-dates (the members' `timestampFormat` is `http-date`), compared at
//!   the one-second resolution such a date carries (RFC 9110 §5.6.7).
//! * A 304 carries no content and does carry the validators of the representation the caller
//!   already holds: "the server generating a 304 response MUST generate … ETag,
//!   Last-Modified" and "MUST NOT generate content" (RFC 9110 §15.4.5).

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The bytes every read in this suite is of.
const BODY: &[u8] = b"conditional-read-payload";

/// `HEAD /<bucket>/<key><query>` — `HeadObject` with the conditional headers the case
/// carries.
///
/// `pub(crate)`, like [`http_date`] below: the suite that pins which VERSION a 304 names
/// ([`super::not_modified_version_headers`]) drives its HEADs and builds its dates through
/// these rather than keeping a second copy.
pub(crate) async fn head_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}{query}"), headers, b"").await
}

/// `instant` as the HTTP-date a conditional header carries.
pub(crate) fn http_date(instant: SystemTime) -> String {
    DateTime::from(instant)
        .fmt(Format::HttpDate)
        .expect("an instant this test built must render as an HTTP date")
}

/// The instant an HTTP-date names.
fn instant_of(rendered: &str) -> SystemTime {
    let date = DateTime::from_str(rendered, Format::HttpDate)
        .expect("the response's Last-Modified must be an HTTP date");
    SystemTime::UNIX_EPOCH + Duration::from_secs(date.secs().unsigned_abs())
}

/// What a conditional read must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answered {
    /// 200, with the object's bytes (a GET) and its metadata.
    Served,
    /// 304, with the validators of the representation and no content at all.
    NotModified,
    /// 412 `PreconditionFailed`.
    Refused,
}

/// One row of the condition table: the case, the headers the read carries, and the answer.
struct ConditionRow {
    case: &'static str,
    headers: Vec<(String, String)>,
    answered: Answered,
}

/// The conditions a read states, over the object whose entity tag is `etag` and whose
/// modification instant is `last_modified`.
///
/// The rows differ along which condition is stated, the FORM of the entity-tag value (the
/// exact tag, a tag that names another representation, the `*` wildcard, a comma-separated
/// list and the weak marker), where the date falls relative to the object's own instant
/// (before, exactly on, and after it), and which pair of conditions is stated together —
/// which is what the precedence rule is about.
fn condition_rows(etag: &str, last_modified: SystemTime) -> Vec<ConditionRow> {
    let other_etag = "\"00000000000000000000000000000000\"";
    let before = http_date(last_modified - Duration::from_secs(1));
    let exactly = http_date(last_modified);
    let after = http_date(last_modified + Duration::from_secs(60));
    let row = |case: &'static str, headers: Vec<(&str, String)>, answered: Answered| ConditionRow {
        case,
        headers: headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        answered,
    };
    vec![
        row("no condition at all", Vec::new(), Answered::Served),
        row(
            "If-None-Match naming the object's own tag",
            vec![("if-none-match", etag.to_string())],
            Answered::NotModified,
        ),
        row(
            "If-None-Match naming another representation",
            vec![("if-none-match", other_etag.to_string())],
            Answered::Served,
        ),
        row(
            "If-None-Match as the wildcard, which names any existing representation",
            vec![("if-none-match", "*".to_string())],
            Answered::NotModified,
        ),
        row(
            "If-None-Match as a list holding the object's tag",
            vec![("if-none-match", format!("{other_etag}, {etag}"))],
            Answered::NotModified,
        ),
        row(
            "If-None-Match with the weak marker on the object's tag",
            vec![("if-none-match", format!("W/{etag}"))],
            Answered::NotModified,
        ),
        row(
            "If-Modified-Since one second before the object was written",
            vec![("if-modified-since", before.clone())],
            Answered::Served,
        ),
        row(
            "If-Modified-Since exactly when the object was written",
            vec![("if-modified-since", exactly.clone())],
            Answered::NotModified,
        ),
        row(
            "If-Modified-Since after the object was written",
            vec![("if-modified-since", after.clone())],
            Answered::NotModified,
        ),
        row(
            "If-Match naming the object's own tag",
            vec![("if-match", etag.to_string())],
            Answered::Served,
        ),
        row(
            "If-Match naming another representation",
            vec![("if-match", other_etag.to_string())],
            Answered::Refused,
        ),
        row(
            "If-Unmodified-Since one second before the object was written",
            vec![("if-unmodified-since", before.clone())],
            Answered::Refused,
        ),
        row(
            "If-Unmodified-Since after the object was written",
            vec![("if-unmodified-since", after.clone())],
            Answered::Served,
        ),
        // The precedence inside each pair.
        row(
            "If-Match holding beside an If-Unmodified-Since that does not",
            vec![
                ("if-match", etag.to_string()),
                ("if-unmodified-since", before.clone()),
            ],
            Answered::Served,
        ),
        row(
            "If-None-Match matching beside an If-Modified-Since that holds",
            vec![
                ("if-none-match", etag.to_string()),
                ("if-modified-since", before.clone()),
            ],
            Answered::NotModified,
        ),
        row(
            "If-None-Match not matching beside an If-Modified-Since that does not hold",
            vec![
                ("if-none-match", other_etag.to_string()),
                ("if-modified-since", after.clone()),
            ],
            Answered::Served,
        ),
        // Across the pairs: the 412 the first pair earns is answered whatever the caller's
        // cached copy is.
        row(
            "an If-Match that does not hold beside a matching If-None-Match",
            vec![
                ("if-match", other_etag.to_string()),
                ("if-none-match", etag.to_string()),
            ],
            Answered::Refused,
        ),
        row(
            "an If-Match that holds beside a matching If-None-Match",
            vec![
                ("if-match", etag.to_string()),
                ("if-none-match", etag.to_string()),
            ],
            Answered::NotModified,
        ),
    ]
}

#[tokio::test]
async fn a_conditional_read_is_served_refused_or_answered_not_modified() {
    let router = router();
    bucket(&router, "conditional-reads").await;
    let written = put_object(&router, "conditional-reads", "held.bin", BODY, &[]).await;
    assert_eq!(
        written.status,
        200,
        "the object the conditions are judged against must be written first, got {} with \
         body {:?}",
        written.status,
        text(&written)
    );
    let etag = written
        .header("etag")
        .expect("a stored object must report its entity tag");
    let probe = head_object(&router, "conditional-reads", "held.bin", "", &[]).await;
    let last_modified = instant_of(
        &probe
            .header("last-modified")
            .expect("a stored object must report when it was last modified"),
    );

    for row in condition_rows(&etag, last_modified) {
        let case = row.case;
        let headers: Vec<(&str, &str)> = row
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();

        // Both reads are judged by the same conditions: a HEAD "has the same options as a
        // GET operation on an object", so only the body differs.
        let read = get_object(&router, "conditional-reads", "held.bin", "", &headers).await;
        let headed = head_object(&router, "conditional-reads", "held.bin", "", &headers).await;
        let (expected_status, expected_code) = match row.answered {
            Answered::Served => (200, ""),
            Answered::NotModified => (304, ""),
            Answered::Refused => (412, "PreconditionFailed"),
        };
        for (operation, answer) in [("GetObject", &read), ("HeadObject", &headed)] {
            assert_eq!(
                answer.status, expected_status,
                "case {case:?} on {operation}: expected HTTP {expected_status}, got {} with \
                 body {:?}",
                answer.status,
                text(answer)
            );
        }
        if !expected_code.is_empty() {
            assert_eq!(
                read.error_code(),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {:?}",
                read.error_code()
            );
        }

        match row.answered {
            Answered::Served => {
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: a served read carries the object's bytes, got {:?}",
                    text(&read)
                );
                assert_eq!(
                    read.header("etag").as_deref(),
                    Some(etag.as_str()),
                    "case {case:?}: a served read reports the object's entity tag, got {:?}",
                    read.header("etag")
                );
            }
            Answered::NotModified => {
                // No content, and the validators of the representation the caller holds.
                assert!(
                    read.body.is_empty(),
                    "case {case:?}: a 304 carries no content, got {:?}",
                    text(&read)
                );
                assert_eq!(
                    read.header("etag").as_deref(),
                    Some(etag.as_str()),
                    "case {case:?}: a 304 reports the representation's entity tag, got {:?}",
                    read.header("etag")
                );
                assert_eq!(
                    read.header("last-modified").as_deref(),
                    Some(http_date(last_modified).as_str()),
                    "case {case:?}: a 304 reports when the representation was written, got \
                     {:?}",
                    read.header("last-modified")
                );
            }
            Answered::Refused => {
                assert!(
                    text(&read).contains("<Condition>"),
                    "case {case:?}: a 412 names the condition that did not hold, got {:?}",
                    text(&read)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_conditional_read_of_a_key_that_is_not_there_is_not_modified_for_nobody() {
    let router = router();
    bucket(&router, "conditional-absent").await;

    // The conditions are judged against a version the read RESOLVED, so a key the bucket
    // does not hold is answered by its absence whatever the request's conditions are.
    for (case, headers) in [
        ("If-None-Match as the wildcard", vec![("if-none-match", "*")]),
        (
            "If-Modified-Since in the past",
            vec![("if-modified-since", "Thu, 29 Oct 2026 20:56:50 GMT")],
        ),
        ("If-Match as the wildcard", vec![("if-match", "*")]),
    ] {
        let read = get_object(&router, "conditional-absent", "missing.bin", "", &headers).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: a key that is not there answers 404, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchKey",
            "case {case:?}: expected the NoSuchKey code, got {:?}",
            read.error_code()
        );
    }
}
