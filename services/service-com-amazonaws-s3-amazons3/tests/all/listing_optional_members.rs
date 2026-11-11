// SPDX-License-Identifier: CC0-1.0

//! What `ListObjectsV2` does with the OPTIONAL members of its request.
//!
//! RULE: an implemented listing answers the 200 `ListBucketResult` for every request whose
//! input satisfies the model's constraints — an optional member that only changes how the
//! same enumeration is rendered (`EncodingType`, "encoding type used by Amazon S3 to encode
//! object key names in the XML response") or which already-recorded field each entry
//! carries (`FetchOwner`, "the owner field is not present in `ListObjectsV2` by default. If
//! you want to return the owner field with each key in the result, then set the
//! `FetchOwner` field to `true`") is APPLIED, never answered with a refusal. `Object$Owner`
//! is "the owner of the object", and this emulator serves one account, so the owner a
//! listing fetches is the account every other owner this service reports names.
//!
//! The rows below differ along WHICH optional members one request carries, and in what
//! combination: none, each on its own, both together, and both together with the selection
//! members a paginating client adds (`Prefix`, `StartAfter`, `MaxKeys`) — which is how an
//! SDK lists a bucket, because botocore puts `encoding-type=url` on every listing it sends
//! and percent-decodes every key-name element of the page it gets back.
//!
//! Every call goes over the wire through the harnesses the other object suites own, so what
//! is pinned is the wire behaviour rather than any handler's internals, and every assertion
//! names its case and the value it actually saw.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send};
use super::object_and_tagging_roundtrip::{bucket, text};
// The percent-decode a client applies to every key-name element of a page, owned by the
// suite that pins the encoding itself. No key in [`KEYS`] carries a `%`, so it is the
// identity on a listing that asked for no encoding.
use super::object_listing_encoding::decoded;
use super::object_storage::{list_objects_v2, listing_of, store_keys, Listing};

/// The keys every case below lists, in the ascending byte order a listing reports them in.
///
/// One key under a prefix, a second under the same prefix, one with no prefix at all, and
/// the parentheses of the `EncodingType` documentation's own example — so a row that asks
/// for an encoding type has something to encode and a row that does not has something to
/// report verbatim. None carries a `%`, which is what makes [`decoded`] the identity on a
/// listing that asked for no encoding.
const KEYS: &[&str] = &[
    "notes/april.txt",
    "notes/may.txt",
    "plain.txt",
    "test_file(3).png",
];

/// One request's optional members, and what the listing it answers must carry.
struct Case {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The query parameters this row adds to `GET /<bucket>?list-type=2`.
    query: &'static str,
    /// The keys the page must report, decoded, in response order.
    keys: &'static [&'static str],
    /// Whether the response must echo `EncodingType` and write its key names encoded.
    encoded: bool,
    /// Whether every `Contents` entry must carry an `Owner`.
    owner: bool,
}

/// The rows: the optional members of a listing, alone and in combination.
const CASES: &[Case] = &[
    Case {
        case: "no optional member",
        query: "",
        keys: KEYS,
        encoded: false,
        owner: false,
    },
    Case {
        case: "encoding-type=url, which every SDK listing carries",
        query: "&encoding-type=url",
        keys: KEYS,
        encoded: true,
        owner: false,
    },
    Case {
        case: "fetch-owner=true",
        query: "&fetch-owner=true",
        keys: KEYS,
        encoded: false,
        owner: true,
    },
    Case {
        case: "fetch-owner=false, which is the default",
        query: "&fetch-owner=false",
        keys: KEYS,
        encoded: false,
        owner: false,
    },
    Case {
        case: "both optional members at once",
        query: "&encoding-type=url&fetch-owner=true",
        keys: KEYS,
        encoded: true,
        owner: true,
    },
    Case {
        case: "both, with a prefix selecting part of the bucket",
        query: "&encoding-type=url&fetch-owner=true&prefix=notes/",
        keys: &["notes/april.txt", "notes/may.txt"],
        encoded: true,
        owner: true,
    },
    Case {
        case: "both, starting after a key",
        query: "&encoding-type=url&fetch-owner=true&start-after=notes/may.txt",
        keys: &["plain.txt", "test_file(3).png"],
        encoded: true,
        owner: true,
    },
    Case {
        case: "both, bounded by max-keys",
        query: "&encoding-type=url&fetch-owner=true&max-keys=2",
        keys: &["notes/april.txt", "notes/may.txt"],
        encoded: true,
        owner: true,
    },
];

/// The `Owner$ID` of each `Contents` entry, in response order, empty where an entry carries
/// no `Owner` at all.
fn owner_ids(listing: &Listing) -> Vec<String> {
    listing
        .contents
        .iter()
        .map(|entry| Listing::entry_field(entry, "ID"))
        .collect()
}

/// The canonical user id this service reports as the owner of what the account holds, read
/// out of `ListBuckets` — "the owner of the buckets listed" (`ListBucketsOutput$Owner`) — so
/// a suite checking an owner checks it against ANOTHER operation's answer rather than against
/// the handler under test, and asserts it has the shape a canonical user id has: "a 64-digit
/// hexadecimal encoded, 256-bit value" (AWS General Reference, *AWS account identifiers*).
///
/// `pub(crate)`: the version listing's owner suite
/// ([`super::version_listing_owner`]) reads the same id through this one helper, so the shape
/// check cannot drift between the two.
pub(crate) async fn account_owner(router: &MultiServiceRouter) -> String {
    let buckets = send(router, "GET", "/?x-id=ListBuckets", &[], "").await;
    assert_eq!(
        buckets.status,
        200,
        "ListBuckets must answer 200, got {} with body {:?}",
        buckets.status,
        text(&buckets)
    );
    let account = buckets.element("ID").unwrap_or_default();
    assert!(
        account.len() == 64
            && account
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
        "a canonical user id is 64 lowercase hex digits, got {account:?}"
    );
    account
}

#[tokio::test]
async fn every_optional_member_a_listing_models_is_served_not_refused() {
    let router = router();
    let bucket_name = "listing-optional-members";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    for Case {
        case,
        query,
        keys,
        encoded,
        owner,
    } in CASES
    {
        let answer = list_objects_v2(&router, bucket_name, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a listing of a bucket that exists answers 200 whatever optional \
             members the request carries, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);

        // The entries are the ones the selection members name, whatever the rendering.
        let reported: Vec<String> = listing.keys().iter().map(|key| decoded(key)).collect();
        let expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the page to report {expected:?}, got {reported:?}"
        );
        assert_eq!(
            listing.field("KeyCount"),
            keys.len().to_string(),
            "case {case:?}: expected KeyCount {}, got {:?}",
            keys.len(),
            listing.field("KeyCount")
        );

        // `EncodingType` is echoed exactly when the request asked for one, and the key
        // names are then genuinely written encoded — a verbatim `(` would mean the
        // response claimed an encoding it did not apply.
        assert_eq!(
            listing.field("EncodingType"),
            if *encoded { "url" } else { "" },
            "case {case:?}: expected the echoed EncodingType to be {:?}, got {:?}",
            if *encoded { "url" } else { "" },
            listing.field("EncodingType")
        );
        for key in listing.keys() {
            if *encoded {
                // The encoding leaves literal only RFC 3986's unreserved set, the `/` that
                // separates the elements of a key prefix, the `+` it writes a space as, and
                // the `%` of each escape: a `(` left in a key name would mean the response
                // claimed an encoding it did not apply.
                let literal = key
                    .chars()
                    .find(|character| {
                        !(character.is_ascii_alphanumeric() || "-_.~%/+".contains(*character))
                    })
                    .map(String::from);
                assert_eq!(
                    literal, None,
                    "case {case:?}: the reported key {key:?} must carry no unencoded \
                     character, found {literal:?}"
                );
            } else {
                assert!(
                    expected.contains(&key),
                    "case {case:?}: a listing that asked for no encoding reports the stored \
                     key verbatim, got {key:?} against {expected:?}"
                );
            }
        }

        // `Owner` is on every entry or on none of them, as the request asked.
        let ids = owner_ids(&listing);
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(
                !id.is_empty(),
                *owner,
                "case {case:?}: entry {index} must {}carry an Owner, got id {id:?}",
                if *owner { "" } else { "not " }
            );
        }
        // "The owner of the object" is an account id; no `DisplayName`, which is an
        // account's friendly name and nothing here records one.
        assert!(
            !text(&answer).contains("DisplayName"),
            "case {case:?}: no DisplayName may be reported, got body {:?}",
            text(&answer)
        );
    }
}

#[tokio::test]
async fn the_owner_a_listing_fetches_is_the_account_the_service_reports_elsewhere() {
    let router = router();
    let bucket_name = "listing-owner-identity";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // The account that owns what this store holds, read from a DIFFERENT operation
    // (`ListBuckets`, whose `Owner` is "the owner of the buckets listed"), so the id below
    // is checked against the service's own answer rather than against the handler's.
    let account = account_owner(&router).await;

    let fetched = list_objects_v2(&router, bucket_name, "&fetch-owner=true").await;
    assert_eq!(
        fetched.status,
        200,
        "case \"fetch-owner=true\": the listing answers 200, got {} with body {:?}",
        fetched.status,
        text(&fetched)
    );
    let fetched = listing_of(&fetched);
    assert_eq!(
        owner_ids(&fetched),
        vec![account.clone(); KEYS.len()],
        "case \"fetch-owner=true\": every entry's Owner is the account this service reports, \
         got {:?}",
        owner_ids(&fetched)
    );

    // The member ADDS a field to the same listing: without it the same request reports the
    // same entries, with the same bytes and entity tags, and no owner at all.
    let plain = listing_of(&list_objects_v2(&router, bucket_name, "").await);
    assert_eq!(
        plain.keys(),
        fetched.keys(),
        "case \"fetch-owner\": the member must not change which keys are reported, got {:?} \
         against {:?}",
        plain.keys(),
        fetched.keys()
    );
    for (index, (bare, with_owner)) in plain.contents.iter().zip(&fetched.contents).enumerate() {
        for element in ["Size", "ETag", "StorageClass", "LastModified"] {
            assert_eq!(
                Listing::entry_field(bare, element),
                Listing::entry_field(with_owner, element),
                "case \"fetch-owner\": entry {index}'s {element} must be the same either way, \
                 got {:?} against {:?}",
                Listing::entry_field(bare, element),
                Listing::entry_field(with_owner, element)
            );
        }
    }
    assert_eq!(
        owner_ids(&plain),
        vec![String::new(); KEYS.len()],
        "case \"no fetch-owner\": the owner field is not present by default, got {:?}",
        owner_ids(&plain)
    );
}

#[tokio::test]
async fn a_paginating_client_sees_every_key_with_the_optional_members_set() {
    let router = router();
    let bucket_name = "listing-optional-paging";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // One key per page, with both optional members set on every request — the shape of the
    // listing an SDK's paginator sends — and the pages followed through the continuation
    // token each truncated page hands back.
    let mut seen: Vec<String> = Vec::new();
    let mut token = String::new();
    for page in 0..=KEYS.len() {
        let query = if token.is_empty() {
            "&encoding-type=url&fetch-owner=true&max-keys=1".to_string()
        } else {
            format!("&encoding-type=url&fetch-owner=true&max-keys=1&continuation-token={token}")
        };
        let answer = list_objects_v2(&router, bucket_name, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case \"page {page}\": every page answers 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);
        for (index, id) in owner_ids(&listing).iter().enumerate() {
            assert!(
                !id.is_empty(),
                "case \"page {page}\": entry {index} must carry an Owner, got id {id:?}"
            );
        }
        seen.extend(listing.keys().iter().map(|key| decoded(key)));
        if listing.field("IsTruncated") != "true" {
            break;
        }
        token = listing.field("NextContinuationToken");
        assert!(
            !token.is_empty(),
            "case \"page {page}\": a truncated page must hand back a continuation token, got \
             body {:?}",
            text(&answer)
        );
    }

    let expected: Vec<String> = KEYS.iter().map(|key| (*key).to_string()).collect();
    assert_eq!(
        seen, expected,
        "paging with both optional members set must report every stored key exactly once, in \
         key order: expected {expected:?}, got {seen:?}"
    );
}
