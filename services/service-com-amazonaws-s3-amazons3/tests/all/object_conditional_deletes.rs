// SPDX-License-Identifier: CC0-1.0

//! A DELETE made CONDITIONAL on the object version it addresses: carried out when every
//! condition the request carries holds, refused with HTTP 412 when one does not, and taking
//! nothing away when it does not.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, error code, message) and what the key holds afterwards.
//!
//! The rule under test, from the three members that carry a condition:
//!
//! * "Deletes the object if the ETag (entity tag) value provided during the delete operation
//!   matches the ETag of the object in S3. If the ETag values do not match, the operation
//!   returns a 412 Precondition Failed error", and "`If-Match` does accept a string value of
//!   an '*' (asterisk) character to denote a match of any ETag"
//!   (`DeleteObjectRequest$IfMatch`) — a tag is the tag OF an object, so a request that
//!   addresses no live object has none to compare and is refused `NoSuchKey` (404): what the
//!   condition named is not there at all;
//! * "the object is deleted only if its modification times matches the provided `Timestamp`.
//!   If the `Timestamp` values do not match, the operation returns a 412 Precondition Failed
//!   error. If the `Timestamp` matches or if the object doesn't exist, the operation returns
//!   a 204 Success (No Content) response" (`$IfMatchLastModifiedTime`);
//! * the same, over the object's size in bytes (`$IfMatchSize`).
//!
//! "You can use the `If-Match`, `x-amz-if-match-last-modified-time` and
//! `x-amz-if-match-size` conditional headers in conjunction with each-other or individually"
//! (`$IfMatchSize`): every member the request carries has to hold.
//!
//! WHERE each is supported differs, which is what decides the bucket a row is sent to: "the
//! `If-Match` header is supported for both general purpose and directory buckets.
//! `IfMatchLastModifiedTime` and `IfMatchSize` is only supported for directory buckets"
//! (`DeleteObject`'s own documentation) — so the rows carrying a time or a size are sent to a
//! directory bucket, and [`super::directory_bucket_conditional_deletes`] is what pins the
//! refusal of those two members on a general purpose bucket.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the condition
//! and how many of them at once, whether each one holds, how the entity tag is QUOTED (a
//! client may send it with or without the double quotes HTTP's quoted-string syntax wraps it
//! in) and whether it is the wildcard, whether the key holds an object at all, and how large
//! that object is.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::{
    NO_SUCH_KEY_CODE, NO_SUCH_KEY_MESSAGE, PRECONDITION_FAILED_CODE, PRECONDITION_FAILED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::seeded_payload;

/// A modification time that is not the one any object in this suite was written at — the
/// clock is read at the write, so a time in the past can match none of them.
///
/// In the spelling the framework's header binding reads a timestamp in, which is the
/// spelling the copy-source precondition suite beside this one sends its `-since` members
/// in: a conditional member's VALUE is a timestamp either way, and what this suite is about
/// is which object version it is compared against.
const ANOTHER_TIME: &str = "2015-01-01T00:00:00Z";

/// An entity tag that is well formed and is no object's — the MD5 digest can never be all
/// zeroes for the payloads here.
const ANOTHER_TAG: &str = "\"00000000000000000000000000000000\"";

/// The `Last-Modified` of a read, in the spelling [`ANOTHER_TIME`] is in: the instant the
/// header names, rendered by the AWS types crate's own formatter rather than by any date
/// arithmetic here.
fn as_timestamp(last_modified: &str) -> String {
    DateTime::from_str(last_modified, Format::HttpDate)
        .expect("a read's Last-Modified is an HTTP date")
        .fmt(Format::DateTime)
        .expect("an instant renders as a timestamp")
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`, with the conditional headers the row carries.
///
/// `pub(crate)`: the suite that pins which bucket kind supports which of those members
/// ([`super::directory_bucket_conditional_deletes`]) sends the same request.
pub(crate) async fn delete_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "DELETE", &format!("/{bucket}/{key}"), headers, b"").await
}

/// How a row names the entity tag it makes its delete conditional on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// The tag the key's object really carries, as a read reports it (with its quotes).
    Held,
    /// That same tag with the quotes stripped — the same tag by value.
    HeldUnquoted,
    /// The `*` that "denote[s] a match of any ETag".
    Wildcard,
    /// A well-formed tag that is not the held object's.
    Other,
    /// The row carries no `If-Match` at all.
    Unnamed,
}

/// How a row names the size, or the modification time, it makes its delete conditional on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fact {
    /// The value the stored object really has.
    Held,
    /// A well-formed value that is not the stored object's.
    Other,
    /// The row carries this member not at all.
    Unnamed,
}

/// One row: the case name, the key's payload (`None` for a key holding nothing), the three
/// conditional members and the status the delete must answer.
struct Row {
    case: &'static str,
    /// How many bytes the key holds, or `None` when it holds nothing at all.
    bytes: Option<usize>,
    tag: Tag,
    size: Fact,
    time: Fact,
    status: u16,
}

#[tokio::test]
async fn a_conditional_delete_is_carried_out_only_when_every_condition_holds() {
    let router = router();
    let name = "conditional-delete-bucket";
    bucket(&router, name).await;
    // The modification-time and size conditions are "only supported for directory buckets",
    // so the rows that carry one are sent here instead.
    let directory = "conditional-delete--use1-az4--x-s3";
    directory_bucket(&router, directory, "use1-az4").await;

    // The rows: each member on its own, holding and not holding; the two forms of a tag that
    // holds and the wildcard; a key holding nothing under each member; several members at
    // once; and payload sizes from empty to a few KiB.
    let rows: Vec<Row> = vec![
        Row {
            case: "If-Match naming the tag the object carries",
            bytes: Some(32),
            tag: Tag::Held,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "If-Match naming that tag unquoted",
            bytes: Some(4_096),
            tag: Tag::HeldUnquoted,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "If-Match with the wildcard, which matches any tag",
            bytes: Some(1),
            tag: Tag::Wildcard,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "If-Match naming the tag of an empty object",
            bytes: Some(0),
            tag: Tag::Held,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "If-Match naming a tag the object does not carry",
            bytes: Some(32),
            tag: Tag::Other,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 412,
        },
        Row {
            case: "If-Match against a key holding nothing, which has no tag to match",
            bytes: None,
            tag: Tag::Other,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 404,
        },
        Row {
            case: "If-Match with the wildcard against a key holding nothing",
            bytes: None,
            tag: Tag::Wildcard,
            size: Fact::Unnamed,
            time: Fact::Unnamed,
            status: 404,
        },
        Row {
            case: "the size the object really is",
            bytes: Some(1_024),
            tag: Tag::Unnamed,
            size: Fact::Held,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "a size the object is not",
            bytes: Some(1_024),
            tag: Tag::Unnamed,
            size: Fact::Other,
            time: Fact::Unnamed,
            status: 412,
        },
        Row {
            case: "a size named against a key holding nothing, which still succeeds",
            bytes: None,
            tag: Tag::Unnamed,
            size: Fact::Other,
            time: Fact::Unnamed,
            status: 204,
        },
        Row {
            case: "the modification time the object really has",
            bytes: Some(64),
            tag: Tag::Unnamed,
            size: Fact::Unnamed,
            time: Fact::Held,
            status: 204,
        },
        Row {
            case: "a modification time the object does not have",
            bytes: Some(64),
            tag: Tag::Unnamed,
            size: Fact::Unnamed,
            time: Fact::Other,
            status: 412,
        },
        Row {
            case: "a modification time named against a key holding nothing",
            bytes: None,
            tag: Tag::Unnamed,
            size: Fact::Unnamed,
            time: Fact::Other,
            status: 204,
        },
        Row {
            case: "every member at once, all of them holding",
            bytes: Some(512),
            tag: Tag::Held,
            size: Fact::Held,
            time: Fact::Held,
            status: 204,
        },
        Row {
            case: "a tag that holds beside a size that does not",
            bytes: Some(512),
            tag: Tag::Held,
            size: Fact::Other,
            time: Fact::Unnamed,
            status: 412,
        },
        Row {
            case: "a size that holds beside a tag that does not",
            bytes: Some(512),
            tag: Tag::Other,
            size: Fact::Held,
            time: Fact::Unnamed,
            status: 412,
        },
        Row {
            case: "a tag and a size that hold beside a time that does not",
            bytes: Some(512),
            tag: Tag::Held,
            size: Fact::Held,
            time: Fact::Other,
            status: 412,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = format!("locks/row-{index}.bin");
        // Where the row's conditions are supported: a time or a size names a condition only a
        // directory bucket serves, and an `If-Match` on its own is served by either kind.
        let name = if row.size == Fact::Unnamed && row.time == Fact::Unnamed {
            name
        } else {
            directory
        };

        // What the key holds, and the facts about it a read reports — the tag, the size and
        // the modification time every condition is judged against.
        let (held_tag, held_time, held_size) = match row.bytes {
            None => (String::new(), String::new(), 0usize),
            Some(bytes) => {
                let payload = seeded_payload(0x2024_1027, bytes);
                let stored = put_object(&router, name, &key, &payload, &[]).await;
                assert_eq!(
                    stored.status,
                    200,
                    "case {case:?}: the fixture write must succeed, got {} with body {:?}",
                    stored.status,
                    text(&stored)
                );
                let read = get_object(&router, name, &key, "", &[]).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the fixture must be readable, got {}",
                    read.status
                );
                (
                    read.header("etag")
                        .unwrap_or_else(|| panic!("case {case:?}: a read reports an ETag")),
                    read.header("last-modified").unwrap_or_else(|| {
                        panic!("case {case:?}: a read reports a Last-Modified")
                    }),
                    bytes,
                )
            }
        };

        let tag = match row.tag {
            Tag::Held => held_tag.clone(),
            Tag::HeldUnquoted => held_tag.trim_matches('"').to_string(),
            Tag::Wildcard => String::from("*"),
            Tag::Other => String::from(ANOTHER_TAG),
            Tag::Unnamed => String::new(),
        };
        let size = match row.size {
            Fact::Held => held_size.to_string(),
            // A size no object in this suite is, and never a negative one: the member is a
            // byte count.
            Fact::Other => (held_size + 7).to_string(),
            Fact::Unnamed => String::new(),
        };
        let time = match row.time {
            Fact::Held => as_timestamp(&held_time),
            Fact::Other => String::from(ANOTHER_TIME),
            Fact::Unnamed => String::new(),
        };
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if row.tag != Tag::Unnamed {
            headers.push(("if-match", tag.as_str()));
        }
        if row.size != Fact::Unnamed {
            headers.push(("x-amz-if-match-size", size.as_str()));
        }
        if row.time != Fact::Unnamed {
            headers.push(("x-amz-if-match-last-modified-time", time.as_str()));
        }

        let answer = delete_object(&router, name, &key, &headers).await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with body {:?}",
            row.status,
            answer.status,
            text(&answer)
        );
        if row.status == 412 {
            assert_eq!(
                answer.error_code(),
                PRECONDITION_FAILED_CODE,
                "case {case:?}: expected the published code for a precondition that did \
                 not hold, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                PRECONDITION_FAILED_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                answer.error_message()
            );
        }
        // An `If-Match` at a key holding no live object names the entity tag of nothing at
        // all, so the delete is refused about the KEY rather than about a precondition.
        if row.status == 404 {
            assert_eq!(
                answer.error_code(),
                NO_SUCH_KEY_CODE,
                "case {case:?}: expected the published code for a key that is not there, \
                 got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                NO_SUCH_KEY_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                answer.error_message()
            );
        }

        // What the key holds afterwards: a refused delete took NOTHING away, so the object
        // is still there with the tag it had; one that was carried out left the key empty.
        let after = get_object(&router, name, &key, "", &[]).await;
        match (row.bytes, row.status) {
            (Some(_), 412) => {
                assert_eq!(
                    after.status, 200,
                    "case {case:?}: a refused delete must leave the object exactly as it \
                     was, got {}",
                    after.status
                );
                assert_eq!(
                    after.header("etag"),
                    Some(held_tag.clone()),
                    "case {case:?}: the surviving object must carry the tag it had, got {:?}",
                    after.header("etag")
                );
                assert_eq!(
                    after.header("content-length"),
                    Some(held_size.to_string()),
                    "case {case:?}: the surviving object must be the size it was, got {:?}",
                    after.header("content-length")
                );
            }
            _ => {
                assert_eq!(
                    after.status, 404,
                    "case {case:?}: the key must hold nothing afterwards, got {} with body \
                     {:?}",
                    after.status,
                    text(&after)
                );
            }
        }
    }
}

#[tokio::test]
async fn compare_and_delete_is_what_releases_a_lock_only_its_holder_may_release() {
    let router = router();
    let name = "conditional-delete-lock-bucket";
    bucket(&router, name).await;
    let key = "locks/leader";

    // Acquire: a write conditional on the key holding nothing, which is the other half of
    // the pair ("uploads the object only if the object key name does not already exist in
    // the bucket specified", `PutObjectRequest$IfNoneMatch`).
    let acquired = put_object(&router, name, key, b"holder-a", &[("if-none-match", "*")]).await;
    assert_eq!(
        acquired.status,
        200,
        "the first acquire must succeed, got {} with body {:?}",
        acquired.status,
        text(&acquired)
    );
    let first_tag = acquired
        .header("etag")
        .expect("a write reports the object's ETag");

    // A second holder cannot acquire while the lock is held.
    let contended = put_object(&router, name, key, b"holder-b", &[("if-none-match", "*")]).await;
    assert_eq!(
        contended.status, 412,
        "a second acquire while the lock is held must be refused, got {}",
        contended.status
    );

    // The holder renews the lock with its own tag, which gives the object a new tag — so the
    // tag the first acquire returned is now STALE.
    let renewed = put_object(&router, name, key, b"holder-a-again", &[("if-match", &first_tag)])
        .await;
    assert_eq!(
        renewed.status,
        200,
        "a renew with the holder's own tag must succeed, got {} with body {:?}",
        renewed.status,
        text(&renewed)
    );
    let renewed_tag = renewed
        .header("etag")
        .expect("a write reports the object's ETag");

    // A stale holder's release is refused and the lock survives: this is the mutual
    // exclusion the pattern rests on.
    let stale = delete_object(&router, name, key, &[("if-match", &first_tag)]).await;
    assert_eq!(
        stale.status, 412,
        "a release naming a stale tag must be refused, got {} with body {:?}",
        stale.status,
        text(&stale)
    );
    assert_eq!(
        stale.error_code(),
        PRECONDITION_FAILED_CODE,
        "a stale release is refused with the published code, got {:?}",
        stale.error_code()
    );
    let still_held = get_object(&router, name, key, "", &[]).await;
    assert_eq!(
        still_held.header("etag"),
        Some(renewed_tag.clone()),
        "the lock must survive a refused release, got {:?}",
        still_held.header("etag")
    );

    // And the current holder's release is carried out, leaving the lock free to acquire
    // again.
    let released = delete_object(&router, name, key, &[("if-match", &renewed_tag)]).await;
    assert_eq!(
        released.status, 204,
        "a release naming the current tag must answer the modelled 204, got {} with body \
         {:?}",
        released.status,
        text(&released)
    );
    let reacquired = put_object(&router, name, key, b"holder-b", &[("if-none-match", "*")]).await;
    assert_eq!(
        reacquired.status, 200,
        "the lock must be acquirable again once released, got {}",
        reacquired.status
    );
}
