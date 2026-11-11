// SPDX-License-Identifier: CC0-1.0

//! One `Owner` container per entry, whatever SHAPE a `ListObjectVersions` page has.
//!
//! RULE: "specifies the owner of the object" (`ObjectVersion$Owner`) and "the account that
//! created the delete marker" (`DeleteMarkerEntry$Owner`) are projected on EVERY entry a
//! version listing reports, in both containers, and nothing in the request turns the member
//! on or off — the operation's input declares no such member
//! (`smithy_query(query="operation",operation="ListObjectVersions")` reports `Delimiter`,
//! `EncodingType`, `KeyMarker`, `MaxKeys`, `Prefix`, `VersionIdMarker`, `ExpectedBucketOwner`,
//! `RequestPayer` and `OptionalObjectAttributes`, and no `FetchOwner`, which is the switch
//! `ListObjectsV2` alone carries. The id is the account this service reports as the owner
//! everywhere else, and no `DisplayName` is recorded for it.
//!
//! [`super::version_listing_echoed_members`] asserts that rule per SELECTION, beside the
//! echoed elements and the checksum members. This suite's rows vary the SHAPE of the page
//! instead — a page that rolls keys up into common prefixes, one bounded by `max-keys`, one
//! resumed after a key marker, over a bucket holding a directory-style key, a key with two
//! versions and a key whose newest version is a delete marker — and COUNT the `Owner`
//! containers the document carries, so a page that owned some of its entries only fails here
//! whatever the entries themselves say. The expected id is read out of a DIFFERENT operation's
//! answer, through the one helper that reads it (`ListBuckets`'s `Owner`, "the owner of the
//! buckets listed"), so the expectation is the service's own id rather than this handler's.
//!
//! Every call goes over the wire through the harnesses the versioning and object suites own,
//! so the projection under test is the one a client reaches, and every assertion names its
//! case and the actual value.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::router;
use super::listing_optional_members::account_owner;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_versioning::{
    delete_object, list_object_versions, set_versioning, version_listing_of, VersionListing,
};

/// The bucket the rows below are read from.
const BUCKET: &str = "version-listing-owner";

/// One row: a version-listing query and how many `Version` plus `DeleteMarker` entries it
/// must carry over the bucket this suite builds.
struct Case {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The query appended to `GET /<bucket>?versions`.
    query: &'static str,
    /// The entries the page reports, counting both containers.
    entries: usize,
}

/// The rows, over the bucket [`stored_bucket`] builds: six entries in all — one version of
/// `archive/`, two of `documents/april.txt`, one of `documents/may.txt`, and `notes.txt`'s
/// delete marker and the version under it.
const CASES: &[Case] = &[
    Case {
        case: "no selection members",
        query: "",
        entries: 6,
    },
    Case {
        case: "a prefix",
        query: "&prefix=documents/",
        entries: 3,
    },
    Case {
        case: "a delimiter rolling keys up",
        query: "&delimiter=/",
        entries: 2,
    },
    Case {
        case: "a prefix and a delimiter",
        query: "&prefix=documents/&delimiter=/",
        entries: 3,
    },
    Case {
        case: "a page bounded by max-keys",
        query: "&max-keys=2",
        entries: 2,
    },
    Case {
        case: "a page resumed after a key marker",
        query: "&key-marker=documents/may.txt",
        entries: 2,
    },
    Case {
        case: "the url encoding type",
        query: "&encoding-type=url",
        entries: 6,
    },
    Case {
        case: "an encoded page under a delimiter",
        query: "&encoding-type=url&delimiter=/",
        entries: 2,
    },
];

/// A versioning-enabled bucket holding the keys the rows read: a directory-style key, a key
/// with two versions, a key with one, and a key whose newest version is a delete marker.
async fn stored_bucket(router: &MultiServiceRouter) {
    bucket(router, BUCKET).await;
    set_versioning(router, BUCKET, "Enabled").await;

    for (key, body) in [
        ("archive/", &b""[..]),
        ("documents/april.txt", &b"april"[..]),
        ("documents/april.txt", &b"april, revised"[..]),
        ("documents/may.txt", &b"may"[..]),
        ("notes.txt", &b"notes"[..]),
    ] {
        let written = put_object(router, BUCKET, key, body, &[]).await;
        assert_eq!(
            written.status,
            200,
            "the key {key:?} must be stored first, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }

    // A delete that names no version lays a marker over the key, which the listing reports in
    // its own container.
    let deleted = delete_object(router, BUCKET, "notes.txt", "").await;
    assert_eq!(
        deleted.status,
        204,
        "the delete that lays a marker must answer 204, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
}

#[tokio::test]
async fn every_entry_of_a_version_listing_carries_the_accounts_owner() {
    let router = router();
    stored_bucket(&router).await;
    let account = account_owner(&router).await;

    for Case {
        case,
        query,
        entries,
    } in CASES
    {
        let answer = list_object_versions(&router, BUCKET, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a version listing of a bucket that exists answers 200, got {} with \
             body {:?}",
            answer.status,
            text(&answer)
        );
        let body = text(&answer);
        let listing = version_listing_of(&answer);
        let reported = listing.entries();
        assert_eq!(
            reported.len(),
            *entries,
            "case {case:?}: expected the page to carry {entries} entries, got {reported:?}"
        );

        // Every entry the page reported names the account as its owner, whichever container
        // carried it.
        for (index, entry) in listing.listed().iter().enumerate() {
            let key = VersionListing::entry(entry, "Key");
            assert_eq!(
                VersionListing::entry(entry, "Owner.ID"),
                account,
                "case {case:?}: entry {index} ({key:?}) must carry the account's Owner/ID, got \
                 {:?}",
                VersionListing::entry(entry, "Owner.ID")
            );
        }

        // One `Owner` container per entry, counted through the reader rather than over the
        // bytes: a page that reported the member on some of its entries only would satisfy
        // nothing a client can rely on. The sibling suite
        // ([`super::version_listing_echoed_members`]) owns the per-entry assertion that the
        // member carries no display name.
        let owned = listing
            .listed()
            .iter()
            .filter(|entry| entry.keys().any(|element| element.starts_with("Owner.")))
            .count();
        assert_eq!(
            owned, *entries,
            "case {case:?}: expected {entries} entries carrying an Owner container, one per \
             entry, got {owned} in body {body:?}"
        );
    }
}
