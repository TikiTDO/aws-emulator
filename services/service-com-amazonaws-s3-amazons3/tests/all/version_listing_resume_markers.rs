// SPDX-License-Identifier: CC0-1.0

//! What a truncated `ListObjectVersions` page hands a client to resume from, and how its
//! key-name elements are written.
//!
//! RULE: under `encoding-type=url` a listing writes each of its KEY-NAME elements — here the
//! entries' `Key` and the `NextKeyMarker` a truncated page hands back ("if you specify the
//! `encoding-type` request parameter, Amazon S3 includes this element in the response, and
//! returns encoded key name values in the following response elements: `KeyMarker,
//! NextKeyMarker, Prefix, Key`, and `Delimiter`",
//! `ListObjectVersionsOutput$EncodingType`) — with every character outside RFC 3986's
//! unreserved set as `%XX` per UTF-8 code value and a space as `+`, while the `/` that
//! separates the elements of a key prefix is left LITERAL, so the element decodes back to the
//! stored key and the marker names a key the client can hand straight back. A marker that is
//! NOT a key name is reported as the service minted it: `NextVersionIdMarker` "specifies the
//! object version you want to start listing from", and a version id is no key name, so the
//! same walk yields the same ids in either rendering.
//!
//! Resuming is what the pair is for — "if your results were truncated, you can make a
//! follow-up paginated request by using the `NextKeyMarker` and `NextVersionIdMarker` response
//! parameters as a starting place in another request"
//! (`ListObjectVersionsOutput$IsTruncated`) — which is why over-escaping the separator is a
//! defect rather than a cosmetic difference: a page whose marker spells the separator as an
//! escape names a key that was never stored, and the walk either loses entries or never ends.
//!
//! The rows differ along what the encoder has to do to a key that CARRIES the separator: one
//! separator and several, a space beside it, the parentheses of the model's own example,
//! multi-byte UTF-8, and a key set with no separator at all. Every row is walked end to end
//! one entry per page, twice — once in each rendering — and the two walks are compared with
//! each other and with what was stored, so an escape applied to the separator, a marker that
//! does not resume, and an encoding applied to the version-id marker each fail here.
//!
//! [`super::listing_resume_markers`] walks the same property for `ListObjects` and
//! `ListMultipartUploads`, whose resume position is a key name plus an upload id — and owns
//! the two helpers this walk shares with it, the `Rendering` of one listing and the predicate
//! for whether the encoder has anything to do to a key;
//! [`super::listing_encoding_type`] pins the written form of each byte value by value. This
//! suite is the version listing's own walk, where the position is a key name plus a version
//! id and the keys carry the separator.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::router;
use super::listing_resume_markers::{touched_by_the_encoder, Rendering};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_listing_encoding::{decoded, in_path};
use super::object_versioning::{list_object_versions, set_versioning, version_listing_of};

/// `(case, keys)`: the key sets every walk below is run over, each row differing in what
/// `encoding-type=url` has to do to a key name — and so to the marker that names one. The
/// first key of each row is written TWICE, so one page boundary falls INSIDE a key and the
/// version-id marker is exercised as well as the key marker.
const ROWS: &[(&str, &[&str])] = &[
    (
        "a key prefix of several separators",
        &[
            "reports/2026/q1.csv",
            "reports/2026/q2.csv",
            "reports/summary.txt",
        ],
    ),
    (
        "a space beside the separator",
        &["my docs/april note.txt", "my docs/may note.txt"],
    ),
    (
        "the documented parentheses under a prefix",
        &["img/test_file(1).png", "img/test_file(2).png"],
    ),
    (
        "multi-byte UTF-8 under a prefix",
        &["記録/データ.txt", "記録/概要.txt"],
    ),
    ("no separator at all", &["alpha.bin", "beta~1"]),
];

/// The most pages one walk may take before the test gives up: a page carries one entry, so a
/// walk that has read more pages than any row has entries is not making progress, and a loop
/// driven by markers the service hands back must never be unbounded.
const PAGE_CEILING: usize = 16;

/// The header a versioned write reports the id it stored under.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// What one walk reported: every entry as `(key as STORED, version id)` in response order,
/// every entry's `Key` as WRITTEN on the wire, and the `(NextKeyMarker, NextVersionIdMarker)`
/// each truncated page handed back, also as written.
#[derive(Debug, Default)]
struct Walk {
    entries: Vec<(String, String)>,
    written_keys: Vec<String>,
    markers: Vec<(String, String)>,
}

/// Walk `bucket`'s versions one entry per page in `rendering`, resuming each page from the
/// markers the previous one handed back — which is exactly what a paginating client does.
async fn walk(router: &MultiServiceRouter, name: &str, rendering: Rendering) -> Walk {
    let mut walked = Walk::default();
    let mut resume = String::new();
    for page in 0..PAGE_CEILING {
        let query = format!("&max-keys=1{}{resume}", rendering.query());
        let answer = list_object_versions(router, name, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}, page {page}: a version listing of a bucket that exists answers 200, got \
             {} with body {:?}",
            rendering.label(),
            answer.status,
            text(&answer)
        );
        let listing = version_listing_of(&answer);
        for (written, version_id) in listing.entries() {
            walked
                .entries
                .push((rendering.as_stored(&written), version_id));
            walked.written_keys.push(written);
        }

        if listing.field("IsTruncated") != "true" {
            // The page that ends the walk says so and hands back no position at all: "if
            // your results were truncated, you can make a follow-up paginated request by
            // using the NextKeyMarker and NextVersionIdMarker response parameters as a
            // starting place in another request" — there is nothing to follow up on here, so
            // a client that keeps paging while a marker is present stops.
            assert_eq!(
                listing.field("IsTruncated"),
                "false",
                "case {:?}, page {page}: the last page reports IsTruncated false, got {:?}",
                rendering.label(),
                listing.field("IsTruncated")
            );
            for element in ["NextKeyMarker", "NextVersionIdMarker"] {
                assert_eq!(
                    listing.field(element),
                    "",
                    "case {:?}, page {page}: an untruncated page hands back no {element}, got \
                     {:?}",
                    rendering.label(),
                    listing.field(element)
                );
            }
            return walked;
        }
        let next_key = listing.field("NextKeyMarker");
        let next_version = listing.field("NextVersionIdMarker");
        assert!(
            !next_key.is_empty() && !next_version.is_empty(),
            "case {:?}, page {page}: a truncated page must hand back both markers, got \
             {next_key:?} and {next_version:?}",
            rendering.label()
        );
        walked.markers.push((next_key.clone(), next_version.clone()));
        // The client hands the position back as the key it names, escaped the way a query
        // value carries it.
        resume = format!(
            "&key-marker={}&version-id-marker={}",
            in_path(&rendering.as_stored(&next_key)),
            in_path(&next_version)
        );
    }
    panic!(
        "case {:?}: the walk read {PAGE_CEILING} pages without reaching the end, so the markers \
         are not making progress: {:?}",
        rendering.label(),
        walked.entries
    );
}

/// Write each key of `keys` once, and the FIRST of them twice, into a versioning-enabled
/// bucket; hand back every stored version id per key, NEWEST FIRST — the order a version
/// listing reports them in.
async fn stored_versions(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[&str],
) -> BTreeMap<String, Vec<String>> {
    bucket(router, name).await;
    set_versioning(router, name, "Enabled").await;

    let mut stored: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let revisions = keys.iter().copied().chain(keys.iter().copied().take(1));
    for key in revisions {
        // The key travels as a request path, so what reaches the store is the key itself.
        let written = put_object(router, name, &in_path(key), b"body", &[]).await;
        assert_eq!(
            written.status,
            200,
            "the key {key:?} must be stored first, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let id = written.header(VERSION_ID_HEADER).unwrap_or_else(|| {
            panic!("a write into a versioned bucket must report the id it stored {key:?} under")
        });
        stored.entry(key.to_string()).or_default().insert(0, id);
    }
    stored
}

/// Assert `written` is `stored` as an `encoding-type=url` listing writes a key-name element:
/// it decodes back to the stored key, every separator is still a separator, and anything the
/// encoder touches was genuinely escaped.
fn assert_written_as_encoded(case: &str, element: &str, written: &str, stored: &str) {
    assert_eq!(
        decoded(written),
        stored,
        "case {case:?}: decoding the {element} {written:?} must yield the stored key {stored:?}"
    );
    assert!(
        !written.to_ascii_lowercase().contains("%2f"),
        "case {case:?}: the {element} {written:?} must leave the key separator literal rather \
         than escaping it"
    );
    assert_eq!(
        written.matches('/').count(),
        stored.matches('/').count(),
        "case {case:?}: the {element} {written:?} must carry the {} separator(s) of {stored:?} \
         literally",
        stored.matches('/').count()
    );
    assert_eq!(
        written != stored,
        touched_by_the_encoder(stored),
        "case {case:?}: expected the {element} {written:?} to {}be escaped",
        match touched_by_the_encoder(stored) {
            true => "",
            false => "not ",
        }
    );
}

#[tokio::test]
async fn a_version_listing_resumes_from_its_markers_in_either_rendering() {
    let router = router();

    for (index, (case, keys)) in ROWS.iter().enumerate() {
        let name = format!("version-resume-markers-{index}");
        let stored = stored_versions(&router, &name, keys).await;

        // What the walks must report: every stored version, key by key in ascending byte
        // order, newest version of a key first — all of it read back from what the writes
        // minted rather than written down here.
        let expected: Vec<(String, String)> = stored
            .iter()
            .flat_map(|(key, ids)| ids.iter().map(|id| (key.clone(), id.clone())))
            .collect();

        let verbatim = walk(&router, &name, Rendering::Verbatim).await;
        let encoded = walk(&router, &name, Rendering::Encoded).await;

        for (rendering, walked) in [
            (Rendering::Verbatim, &verbatim),
            (Rendering::Encoded, &encoded),
        ] {
            // Lossless: the marker-driven walk reports every stored version exactly once, in
            // order, whichever rendering it asked for.
            assert_eq!(
                walked.entries,
                expected,
                "case {case:?} ({}): the walk must report every stored version exactly once in \
                 order, expected {expected:?}, got {:?}",
                rendering.label(),
                walked.entries
            );
            // One entry per page, so every entry but the last ended a truncated page, and the
            // markers name the entry that page reported.
            assert_eq!(
                walked.markers.len(),
                expected.len().saturating_sub(1),
                "case {case:?} ({}): expected {} truncated page(s), got markers {:?}",
                rendering.label(),
                expected.len().saturating_sub(1),
                walked.markers
            );
            for (position, (marker_key, marker_version)) in walked.markers.iter().enumerate() {
                let (key, version_id) = &expected[position];
                assert_eq!(
                    &rendering.as_stored(marker_key),
                    key,
                    "case {case:?} ({}): the NextKeyMarker of page {position} must name the key \
                     {key:?} that page reported, got {marker_key:?}",
                    rendering.label()
                );
                // A version id is no key name, so it is handed back exactly as minted in
                // either rendering.
                assert_eq!(
                    marker_version, version_id,
                    "case {case:?} ({}): the NextVersionIdMarker of page {position} must be the \
                     minted id {version_id:?}, got {marker_version:?}",
                    rendering.label()
                );
            }
        }

        // The ENCODED walk wrote its key names in the form S3 answers with: decodable back to
        // the stored key, with the separator left alone.
        for (position, written) in encoded.written_keys.iter().enumerate() {
            assert_written_as_encoded(case, "Key", written, &expected[position].0);
        }
        for (position, (marker_key, _)) in encoded.markers.iter().enumerate() {
            assert_written_as_encoded(case, "NextKeyMarker", marker_key, &expected[position].0);
        }

        // And the renderings are not the same bytes: a row whose keys the encoder touches
        // must have been written differently in the two walks, so neither walk can pass by
        // both being wrong in the same way.
        assert_eq!(
            encoded.written_keys != verbatim.written_keys,
            keys.iter().any(|key| touched_by_the_encoder(key)),
            "case {case:?}: expected the two renderings to differ exactly where the encoder has \
             something to do, got {:?} against {:?}",
            encoded.written_keys,
            verbatim.written_keys
        );
    }
}
