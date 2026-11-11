// SPDX-License-Identifier: CC0-1.0

//! The INVARIANT `encoding-type=url` owes a client, over a table of key bytes.
//!
//! `tests/all/listing_encoding_type.rs` pins what the encoding WRITES, value by value
//! (`a+b=c&d` → `a%2Bb%3Dc%26d`, and so on). This suite pins the property that
//! makes those values usable rather than merely stable, which no literal expectation can
//! state: **decoding what an `encoding-type=url` listing reports yields exactly the
//! key that was stored, and the listing reports the same keys, in the same order, as the
//! same request without the parameter** — so a client that decodes every key-name element
//! (which is what botocore's `decode_list_object_v2` handler does on every page, through the
//! plus-aware `unquote_plus`) sees the objects it stored, and `encoding-type` is a rendering
//! of one listing rather than a different listing. The two halves are complementary: a
//! wrong-but-reversible encoder is caught there, and a right-looking-but-lossy one is caught
//! here.
//!
//! The rows are keys the literal table does not carry, chosen along what the encoder
//! generalises over: how many separators a key has, the ASCII punctuation a URI reserves,
//! a percent sign (the escape character itself, where a lossy encoder double-encodes or
//! fails to decode), a space, and multi-byte UTF-8 of two and three bytes per character.
//! Each assertion names its case and the actual value.
//!
//! Every call goes over the wire through the harnesses the other object suites own, so the
//! encoder under test is the one a client reaches.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::bucket_lifecycle::router;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_storage::{list_objects_v2, listing_of};

/// The bytes this suite leaves literal when it builds a REQUEST path: RFC 3986's
/// unreserved set plus `/`, the segment separator. A key goes into the request URI through
/// this (a raw space or `%` is not a legal URI character) and the framework percent-decodes
/// a path label, so what reaches the store is the key itself.
const PATH_LITERAL: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `key` as a request path carries it.
///
/// `pub(crate)`: the set this leaves literal — RFC 3986's unreserved characters plus `/` —
/// is legal in a query VALUE too, so the suites that hand a key back as a query parameter
/// ([`super::listing_resume_markers`]) escape it through this one helper.
pub(crate) fn in_path(key: &str) -> String {
    utf8_percent_encode(key, PATH_LITERAL).to_string()
}

/// The bytes a signing SDK leaves literal in a query VALUE: RFC 3986's unreserved set alone,
/// [`PATH_LITERAL`] without the `/` exemption — a query value is escaped byte by byte, which
/// is why the separator of a key prefix reaches the service as `%2F`.
const QUERY_VALUE_LITERAL: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// `value` as an SDK that signs the request puts it in a query string.
///
/// `pub(crate)`, beside [`in_path`] and for the same reason: a suite that hands a value to a
/// listing as a query parameter escapes it through one helper. Use [`in_path`] where the `/`
/// may stay literal and this one where the point is that it does not have to.
pub(crate) fn in_query(value: &str) -> String {
    utf8_percent_encode(value, QUERY_VALUE_LITERAL).to_string()
}

/// What a client does with every key-name element of an `encoding-type=url` page: the
/// plus-aware unquoting botocore applies to each of them, where `+` is a space and `%XX` is
/// the byte it names.
///
/// `pub(crate)`: the optional-member suite ([`super::listing_optional_members`]) decodes
/// the pages it reads through this one helper rather than keeping a second copy.
pub(crate) fn decoded(written: &str) -> String {
    let plus_decoded = written.replace('+', " ");
    percent_decode_str(&plus_decoded)
        .decode_utf8()
        .unwrap_or_else(|error| panic!("the response must carry valid UTF-8: {error}"))
        .into_owned()
}

/// `(case, key)`: keys the literal table in `listing_encoding_type.rs` does not carry.
const KEYS: &[(&str, &str)] = &[
    ("no character the encoder touches", "plain.txt"),
    ("the unreserved set only", "a~b-c_d.9"),
    ("several separators", "reports/2026/q1/summary.txt"),
    ("punctuation a URI reserves", "semi;colon:at@sign,comma"),
    ("the escape character itself", "100%-done.csv"),
    ("a space between words", "two words.txt"),
    ("two-byte UTF-8", "über-größe.txt"),
    ("three-byte UTF-8", "記録/データ.txt"),
];

#[tokio::test]
async fn decoding_an_encoded_listing_yields_the_keys_that_were_stored() {
    let router = router();
    let bucket_name = "listing-encoding-roundtrip";
    bucket(&router, bucket_name).await;

    for (case, key) in KEYS {
        let stored = put_object(&router, bucket_name, &in_path(key), b"body", &[]).await;
        assert_eq!(
            stored.status,
            200,
            "case {case:?}: the key {key:?} must be stored first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }

    // The same listing, rendered both ways.
    let plain = list_objects_v2(&router, bucket_name, "").await;
    assert_eq!(
        plain.status,
        200,
        "a listing with no encoding type must answer 200, got {} with body {:?}",
        plain.status,
        text(&plain)
    );
    let encoded = list_objects_v2(&router, bucket_name, "&encoding-type=url").await;
    assert_eq!(
        encoded.status,
        200,
        "a listing with encoding-type=url must answer 200, got {} with body {:?}",
        encoded.status,
        text(&encoded)
    );

    let verbatim_keys = listing_of(&plain).keys();
    let written_keys = listing_of(&encoded).keys();

    // The parameter changes the RENDERING, not the listing: same count, same order.
    assert_eq!(
        written_keys.len(),
        KEYS.len(),
        "the encoded listing must report every stored key, expected {} entries, got {written_keys:?}",
        KEYS.len()
    );
    assert_eq!(
        written_keys.len(),
        verbatim_keys.len(),
        "encoding-type must not change how many entries are reported: {verbatim_keys:?} against \
         {written_keys:?}"
    );

    // Row by row, in the ascending byte order a listing reports keys in, so each case's
    // own expectation is checked rather than the set as a whole.
    let mut expected: Vec<&str> = KEYS.iter().map(|(_, key)| *key).collect();
    expected.sort_unstable();
    for (index, key) in expected.iter().enumerate() {
        let case = KEYS
            .iter()
            .find(|(_, stored)| stored == key)
            .map(|(case, _)| *case)
            .unwrap_or_default();
        assert_eq!(
            &verbatim_keys[index], key,
            "case {case:?}: a listing with no encoding type must report the key verbatim, got {:?}",
            verbatim_keys[index]
        );
        // The invariant: what the encoded listing carries decodes back to the stored key.
        assert_eq!(
            &decoded(&written_keys[index]),
            key,
            "case {case:?}: decoding {:?} must yield the stored key",
            written_keys[index]
        );
        // And it is genuinely ENCODED where the key carries a character the listing does not
        // leave literal — anything outside the unreserved set and the `/` of a key prefix —
        // so the row cannot pass by the response being verbatim.
        let needs_encoding = key
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "-_.~/".contains(character)));
        assert_eq!(
            written_keys[index] != *key,
            needs_encoding,
            "case {case:?}: expected the reported key {:?} to {}be escaped",
            written_keys[index],
            if needs_encoding { "" } else { "not " }
        );
    }
}
