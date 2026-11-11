// SPDX-License-Identifier: CC0-1.0

//! What a listing does with a query value that reached it PERCENT-ENCODED.
//!
//! RULE: a value bound from a request's query string is the value the parameter carries once
//! the URI's percent-encoding is undone, so a listing selects by that value and echoes that
//! value — a client may escape any byte of a query value (`/` as `%2F`, a space as `%20`, the
//! escape character itself as `%25`), and an SDK that signs the request escapes every byte
//! outside RFC 3986's unreserved set, so the wire form of one value is not unique while the
//! value it denotes is. The elements this is about are the ones each listing publishes as the
//! request it answers: "keys that begin with the indicated prefix"
//! (`ListObjectsV2Output$Prefix`), the `Delimiter` that "causes keys that contain the same
//! string between the prefix and the first occurrence of the delimiter to be rolled up"
//! (`ListObjectsV2Output$Delimiter`), the `StartAfter` that "indicates where in the bucket to
//! begin listing" (`ListObjectsV2Output$StartAfter`) and the `Marker` that "indicates where in
//! the bucket listing begins" (`ListObjectsOutput$Marker`) — a client pages by reading them out
//! of the response, and one that round-trips an element it cannot resolve against the keys it
//! was given pages nowhere.
//!
//! WHAT THIS SUITE ADDS, beside the suites that already drive an escaped value one at a time:
//! the version listing's own echo is pinned over escaped prefixes by
//! [`super::version_listing_echoed_members`], a directory bucket's by
//! [`super::directory_bucket_listings`], and a subresource's delimiter and start token by
//! [`super::bucket_subresource_document_rules`]. Here the same rule is held over a CLASS of
//! escape forms, on the two listings of a general purpose bucket's contents (`ListObjectsV2`
//! and the revision before it), in BOTH address forms — a virtual-hosted-style request binds
//! its members over the path-style address this service rewrites it to, which is its own seam
//! — and with the written form of the echoed element spelled out per row, so an encoder that
//! escapes the separator of a key prefix fails here on the element a client pages by rather
//! than only on a key name. The in-progress uploads of `ListMultipartUploads`, which the same
//! parameter selects, are driven over one row of the same table.
//!
//! The rendering `encoding-type=url` asks for is the one rule that applies ON TOP of this: the
//! element then carries the key-name encoding of the SAME value
//! ([`super::listing_encoding_type`] pins that form byte by byte), which decodes back to it.
//!
//! The rows differ along WHAT the client escaped: the separator of a key prefix, several of
//! them, a space, the escape character itself, punctuation a query string would otherwise
//! swallow, two-byte and three-byte UTF-8 — plus a control row carrying nothing the escaping
//! touches, so no row can pass by the response merely echoing the wire bytes. Every call goes
//! over the wire through the harnesses the other object suites own.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes_to_host, Answer};
use super::multipart_upload_listing::{initiate, list_uploads, upload_listing_of};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_listing_encoding::{decoded, in_path, in_query};
use super::object_listing_v1::list_objects;
use super::object_storage::{list_objects_v2, listing_of};

/// `(case, prefix, echoed_under_encoding_type)`: the prefix each row asks a listing to select
/// by, written as the value the caller means rather than as the bytes it travels in, and the
/// form the `Prefix` element carries when the request asked for `encoding-type=url`.
///
/// That third column is written out rather than computed, so no row can agree with a wrong
/// encoder by construction: it is the form the `EncodingType` enum documents — every character
/// outside RFC 3986's unreserved set written `%XX` per UTF-8 code value ("the object
/// `test_file(3).png` will appear as `test_file%283%29.png`") — with the two exceptions S3's
/// own form carries, a SPACE written `+` and the `/` separating the elements of a key prefix
/// left as it is.
const PREFIXES: &[(&str, &str, &str)] = &[
    ("nothing the escaping touches", "notes", "notes"),
    (
        "the separator of a key prefix",
        "model-card-exports/",
        "model-card-exports/",
    ),
    (
        "several separators",
        "reports/2026/q1/",
        "reports/2026/q1/",
    ),
    ("a space between words", "model data/", "model+data/"),
    ("the escape character itself", "100%-done/", "100%25-done/"),
    (
        "punctuation a query string swallows",
        "a+b=c&d/",
        "a%2Bb%3Dc%26d/",
    ),
    ("two-byte UTF-8", "données/", "donn%C3%A9es/"),
    ("three-byte UTF-8", "記録/", "%E8%A8%98%E9%8C%B2/"),
];

/// How a request ADDRESSES the bucket it lists. Both forms are driven, because this service
/// resolves a virtual-hosted-style request's bucket out of the host and binds its members over
/// the path-style address it rewrites to, which is a different seam from the path-style one.
#[derive(Clone, Copy, Debug)]
enum Address {
    /// `GET /<bucket>?…` to the Regional endpoint.
    Path,
    /// `GET /?…` to `<bucket>.s3.<region>.amazonaws.com`, which is what every SDK's
    /// endpoint resolver writes.
    Host,
}

/// One `ListObjectsV2` of `bucket`, addressed either way, carrying `query` beside the
/// `list-type=2` discriminator.
async fn list_v2(
    router: &MultiServiceRouter,
    address: Address,
    bucket: &str,
    query: &str,
) -> Answer {
    match address {
        Address::Path => list_objects_v2(router, bucket, query).await,
        Address::Host => {
            send_bytes_to_host(
                router,
                "GET",
                &format!("{bucket}.s3.us-east-1.amazonaws.com"),
                &format!("/?list-type=2{query}"),
                &[],
                b"",
            )
            .await
        }
    }
}

/// The leaf each row's one matching key ends in.
const LEAF: &str = "report.pdf";

/// A key no row's prefix selects, held by every bucket below so a row that selected the whole
/// bucket fails.
const OUTSIDE: &str = "zz-outside.txt";

#[tokio::test]
async fn a_listing_selects_by_and_echoes_the_decoded_prefix() {
    let router = router();

    for (index, (case, prefix, echoed)) in PREFIXES.iter().enumerate() {
        // One bucket per row, so a row's prefix can never select another row's key.
        let bucket_name = format!("listing-query-prefix-{index}");
        bucket(&router, &bucket_name).await;
        let key = format!("{prefix}{LEAF}");
        for stored in [key.as_str(), OUTSIDE] {
            let written = put_object(&router, &bucket_name, &in_path(stored), b"body", &[]).await;
            assert_eq!(
                written.status,
                200,
                "case {case:?}: the key {stored:?} must be stored first, got {} with body {:?}",
                written.status,
                text(&written)
            );
        }

        // Both address forms, because a virtual-hosted-style request binds its members over
        // the path-style address this service rewrites it to, which is its own seam.
        for address in [Address::Path, Address::Host] {
            let case = format!("{case}, addressed {address:?}-style");
            let answer = list_v2(
                &router,
                address,
                &bucket_name,
                &format!("&prefix={}", in_query(prefix)),
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a listing of a bucket that exists answers 200, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
            let listing = listing_of(&answer);
            assert_eq!(
                listing.field("Prefix"),
                *prefix,
                "case {case:?}: the echoed Prefix must be the value the caller asked for, got \
                 {:?} in {:?}",
                listing.field("Prefix"),
                text(&answer)
            );
            assert_eq!(
                listing.keys(),
                vec![key.clone()],
                "case {case:?}: the page must carry the one key under {prefix:?}, got {:?}",
                listing.keys()
            );
            assert_eq!(
                listing.field("KeyCount"),
                "1",
                "case {case:?}: one entry was selected, got {:?} in {:?}",
                listing.field("KeyCount"),
                text(&answer)
            );

            // The same selection, in the rendering `encoding-type=url` asks for — which is
            // what every SDK's listing sends: the element then carries the key-name encoding
            // of that same value, which decodes back to it.
            let encoded = list_v2(
                &router,
                address,
                &bucket_name,
                &format!("&prefix={}&encoding-type=url", in_query(prefix)),
            )
            .await;
            assert_eq!(
                encoded.status,
                200,
                "case {case:?}: the same listing with an encoding type answers 200, got {} with \
                 body {:?}",
                encoded.status,
                text(&encoded)
            );
            let encoded = listing_of(&encoded);
            assert_eq!(
                encoded.field("Prefix"),
                *echoed,
                "case {case:?}: the encoded Prefix must be written {echoed:?}, got {:?}",
                encoded.field("Prefix")
            );
            assert_eq!(
                decoded(&encoded.field("Prefix")),
                *prefix,
                "case {case:?}: the encoded Prefix must decode back to the value asked for, got \
                 {:?}",
                encoded.field("Prefix")
            );
            assert_eq!(
                encoded
                    .keys()
                    .iter()
                    .map(|key| decoded(key))
                    .collect::<Vec<String>>(),
                vec![key.clone()],
                "case {case:?}: the encoding renders the same page, got {:?}",
                encoded.keys()
            );
        }

        // And the revision that calls the same member `Prefix` reads it the same way, so the
        // rule is about the binding and not about one operation.
        let v1 = list_objects(
            &router,
            &bucket_name,
            &format!("prefix={}", in_query(prefix)),
        )
        .await;
        assert_eq!(
            v1.status,
            200,
            "case {case:?}: ListObjects answers 200, got {} with body {:?}",
            v1.status,
            text(&v1)
        );
        let v1 = listing_of(&v1);
        assert_eq!(
            v1.field("Prefix"),
            *prefix,
            "case {case:?}: ListObjects must echo the decoded prefix too, got {:?}",
            v1.field("Prefix")
        );
        assert_eq!(
            v1.keys(),
            vec![key.clone()],
            "case {case:?}: ListObjects must select by the decoded prefix too, got {:?}",
            v1.keys()
        );
    }
}

/// `(case, delimiter, keys, rolled_up, entries)`: what a delimiter escaped on the wire has to
/// roll up. The rows differ along the delimiter's own bytes — the separator of a key prefix, a
/// space, and multi-byte UTF-8 — each with the keys it groups and the entries it leaves.
type DelimiterRow<'a> = (
    &'a str,
    &'a str,
    &'a [&'a str],
    &'a [&'a str],
    &'a [&'a str],
);

const DELIMITERS: &[DelimiterRow<'static>] = &[
    (
        "the separator of a key prefix",
        "/",
        &["one/a.txt", "one/b.txt", "two/c.txt", "flat.txt"],
        &["one/", "two/"],
        &["flat.txt"],
    ),
    (
        "a space",
        " ",
        &["model data.tar", "model notes.txt", "flat.txt"],
        &["model "],
        &["flat.txt"],
    ),
    (
        "multi-byte UTF-8",
        "é",
        &["donnéa.txt", "donnéb.txt", "flat.txt"],
        &["donné"],
        &["flat.txt"],
    ),
];

#[tokio::test]
async fn a_listing_rolls_up_by_and_echoes_the_decoded_delimiter() {
    let router = router();

    for (index, (case, delimiter, keys, rolled_up, entries)) in DELIMITERS.iter().enumerate() {
        let bucket_name = format!("listing-query-delimiter-{index}");
        bucket(&router, &bucket_name).await;
        for key in *keys {
            let written = put_object(&router, &bucket_name, &in_path(key), b"body", &[]).await;
            assert_eq!(
                written.status,
                200,
                "case {case:?}: the key {key:?} must be stored first, got {} with body {:?}",
                written.status,
                text(&written)
            );
        }

        let answer = list_objects_v2(
            &router,
            &bucket_name,
            &format!("&delimiter={}", in_query(delimiter)),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a listing with a delimiter answers 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        assert_eq!(
            listing.field("Delimiter"),
            *delimiter,
            "case {case:?}: the echoed Delimiter must be the value the caller asked for, got \
             {:?} in {:?}",
            listing.field("Delimiter"),
            text(&answer)
        );
        assert_eq!(
            listing.common_prefixes,
            rolled_up
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the keys sharing a group must roll up to {rolled_up:?}, got {:?}",
            listing.common_prefixes
        );
        assert_eq!(
            listing.keys(),
            entries
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the keys outside a group stay entries, got {:?}",
            listing.keys()
        );
    }
}

/// `(case, keys, start_after, remaining)`: where a listing begins when the token it begins
/// after reached it escaped. The rows differ along what the token carries.
const START_TOKENS: &[(&str, &[&str], &str, &[&str])] = &[
    (
        "nothing the escaping touches",
        &["alpha.txt", "beta.txt", "gamma.txt"],
        "beta.txt",
        &["gamma.txt"],
    ),
    (
        "the separator of a key prefix",
        &["notes/april.txt", "notes/may.txt", "plain.txt"],
        "notes/april.txt",
        &["notes/may.txt", "plain.txt"],
    ),
    (
        "a space between words",
        &["two words.txt", "zz last.txt"],
        "two words.txt",
        &["zz last.txt"],
    ),
    (
        "multi-byte UTF-8",
        &["données.txt", "zz.txt"],
        "données.txt",
        &["zz.txt"],
    ),
];

#[tokio::test]
async fn a_listing_begins_after_and_echoes_the_decoded_start_token() {
    let router = router();

    for (index, (case, keys, token, remaining)) in START_TOKENS.iter().enumerate() {
        let bucket_name = format!("listing-query-start-{index}");
        bucket(&router, &bucket_name).await;
        for key in *keys {
            let written = put_object(&router, &bucket_name, &in_path(key), b"body", &[]).await;
            assert_eq!(
                written.status,
                200,
                "case {case:?}: the key {key:?} must be stored first, got {} with body {:?}",
                written.status,
                text(&written)
            );
        }
        let expected: Vec<String> = remaining.iter().map(|key| (*key).to_string()).collect();

        // `ListObjectsV2` names the token `start-after`, and the revision before it `marker`;
        // both say where the listing begins, so both are read from the decoded value.
        let after = list_objects_v2(
            &router,
            &bucket_name,
            &format!("&start-after={}", in_query(token)),
        )
        .await;
        assert_eq!(
            after.status,
            200,
            "case {case:?}: a listing with a start token answers 200, got {} with body {:?}",
            after.status,
            text(&after)
        );
        let after = listing_of(&after);
        assert_eq!(
            after.field("StartAfter"),
            *token,
            "case {case:?}: the echoed StartAfter must be the value the caller asked for, got {:?}",
            after.field("StartAfter")
        );
        assert_eq!(
            after.keys(),
            expected,
            "case {case:?}: the page must begin after {token:?}, got {:?}",
            after.keys()
        );

        let marked = list_objects(
            &router,
            &bucket_name,
            &format!("marker={}", in_query(token)),
        )
        .await;
        assert_eq!(
            marked.status,
            200,
            "case {case:?}: a marked listing answers 200, got {} with body {:?}",
            marked.status,
            text(&marked)
        );
        let marked = listing_of(&marked);
        assert_eq!(
            marked.field("Marker"),
            *token,
            "case {case:?}: the echoed Marker must be the value the caller asked for, got {:?}",
            marked.field("Marker")
        );
        assert_eq!(
            marked.keys(),
            expected,
            "case {case:?}: the marked page must begin after {token:?}, got {:?}",
            marked.keys()
        );
    }
}

/// The uploads listing binds the same `prefix` parameter, and its entries are not objects, so
/// the rule is held over it too — over the same table, with the version listing's own echo left
/// to [`super::version_listing_echoed_members`], which already pins it over escaped prefixes.
#[tokio::test]
async fn the_upload_listing_reads_the_same_parameter_the_same_way() {
    let router = router();

    for (index, (case, prefix, _)) in PREFIXES.iter().enumerate() {
        let bucket_name = format!("listing-query-uploads-{index}");
        bucket(&router, &bucket_name).await;
        let key = format!("{prefix}{LEAF}");

        initiate(&router, &bucket_name, &key, &[]).await;
        initiate(&router, &bucket_name, OUTSIDE, &[]).await;
        let uploads = list_uploads(
            &router,
            &bucket_name,
            &format!("&prefix={}", in_query(prefix)),
            &[],
        )
        .await;
        assert_eq!(
            uploads.status,
            200,
            "case {case:?}: an upload listing answers 200, got {} with body {:?}",
            uploads.status,
            text(&uploads)
        );
        let uploads = upload_listing_of(&uploads);
        assert_eq!(
            uploads.field("Prefix"),
            *prefix,
            "case {case:?}: the upload listing must echo the decoded prefix, got {:?}",
            uploads.field("Prefix")
        );
        assert_eq!(
            uploads
                .entries()
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<String>>(),
            vec![key.clone()],
            "case {case:?}: the upload listing must select by the decoded prefix, got {:?}",
            uploads.entries()
        );
    }
}
