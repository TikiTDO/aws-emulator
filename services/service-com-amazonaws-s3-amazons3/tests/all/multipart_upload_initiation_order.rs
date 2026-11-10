// SPDX-License-Identifier: CC0-1.0

//! The order `ListMultipartUploads` reports the uploads of ONE key in.
//!
//! RULE: a general purpose bucket's listing of in-progress multipart uploads is a two-level
//! order — "Key-based sorting - Multipart uploads are initially sorted in ascending order
//! based on their object keys" and "Time-based sorting - For uploads that share the same
//! object key, they are further sorted in ascending order based on the upload initiation
//! time. Among uploads with the same key, the one that was initiated first will appear
//! before the ones that were initiated later" (`ListMultipartUploads`'s own documentation,
//! "Sorting of multipart uploads in response"). An upload id is an opaque token the service
//! mints, so it is NOT what orders two uploads of one key; and a walk that resumes from the
//! `NextKeyMarker`/`NextUploadIdMarker` of a truncated page reports every upload exactly
//! once in that same order, "the multipart upload after which listing should begin"
//! (`ListMultipartUploadsRequest$UploadIdMarker`) being the entry the previous page ended
//! on.
//!
//! The rows differ along what the second level has to order: how many uploads share one key
//! (two, five), how many keys carry several, how the initiations of different keys are
//! interleaved, whether a sharing key is a prefix of another key, and whether it is ASCII.
//! Where [`super::multipart_upload_listing`] spans the whole listing contract over one
//! initiation table, this suite spans the WITHIN-ONE-KEY order, including the case that
//! tells the two candidate orders apart: a pair whose later initiation minted the
//! lexicographically smaller id.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_creation_date_views::instant_of;
use super::bucket_lifecycle::router;
use super::multipart_upload_listing::{initiate, listing};
use super::object_and_tagging_roundtrip::bucket;
use super::object_listing_encoding::in_path;

/// `(case, the keys to initiate an upload for, in the order the initiations happen)`. A key
/// appearing more than once is a key carrying that many uploads in progress.
const ROWS: &[(&str, &[&str])] = &[
    ("two uploads of one key", &["same.bin", "same.bin"]),
    (
        "two uploads of one key, after a key that sorts later",
        &["z-later.bin", "same.bin", "same.bin"],
    ),
    (
        "five uploads of one key",
        &["five.bin", "five.bin", "five.bin", "five.bin", "five.bin"],
    ),
    (
        "three keys each carrying uploads, interleaved",
        &["c.bin", "a.bin", "b.bin", "a.bin", "c.bin", "b.bin", "a.bin"],
    ),
    (
        "a key that is a prefix of another, each carrying two",
        &["p/x", "p/x/y", "p/x", "p/x/y"],
    ),
    (
        "a non-ASCII key twice, around an ASCII one",
        &["über.bin", "ascii.bin", "über.bin"],
    ),
];

/// Every upload of `name`, collected by walking the listing `size` entries per page and
/// resuming from the markers each truncated page hands back.
///
/// `total` is how many uploads the bucket holds, which bounds the walk: a page that makes no
/// progress fails the case instead of looping forever.
async fn walked(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
    size: usize,
    total: usize,
) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut query = format!("&max-uploads={size}");
    let mut pages_left = total + 1;
    loop {
        assert!(
            pages_left > 0,
            "case {case:?}: a walk of {total} uploads at {size} per page must end within {} \
             pages, got {seen:?}",
            total + 1
        );
        pages_left -= 1;

        let page = listing(router, name, case, &query).await;
        assert!(
            page.entries().len() <= size,
            "case {case:?}: a page of max-uploads={size} carries at most that many entries, \
             got {:?}",
            page.entries()
        );
        seen.extend(page.entries());
        if page.field("IsTruncated") != "true" {
            break;
        }
        let (key_marker, id_marker) = (
            page.field("NextKeyMarker"),
            page.field("NextUploadIdMarker"),
        );
        assert_eq!(
            (key_marker.as_str(), id_marker.as_str()),
            (
                seen.last().map(|(key, _)| key.as_str()).unwrap_or_default(),
                seen.last().map(|(_, id)| id.as_str()).unwrap_or_default()
            ),
            "case {case:?}: the markers of a truncated page must name the entry it ended on, \
             got {key_marker:?} and {id_marker:?}"
        );
        query = format!(
            "&max-uploads={size}&key-marker={}&upload-id-marker={}",
            in_path(&key_marker),
            in_path(&id_marker)
        );
    }
    seen
}

/// Initiate one upload per entry of `keys` in `name`, in that order, and hand back
/// `(key, upload id)` per initiation — the initiation sequence itself.
async fn initiate_in_order(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[&str],
) -> Vec<(String, String)> {
    let mut initiated = Vec::new();
    for key in keys {
        let upload_id = initiate(router, name, key, &[]).await;
        initiated.push(((*key).to_string(), upload_id));
    }
    initiated
}

/// The order the listing owes for `initiated`, which is in initiation order: a STABLE sort
/// by key leaves the uploads of one key in the order they were initiated in.
fn owed(initiated: &[(String, String)]) -> Vec<(String, String)> {
    let mut expected = initiated.to_vec();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    expected
}

#[tokio::test]
async fn uploads_sharing_a_key_read_back_in_initiation_order() {
    for (at, (case, keys)) in ROWS.iter().enumerate() {
        let router = router();
        let name = &format!("multipart-initiation-order-{at}");
        bucket(&router, name).await;
        let initiated = initiate_in_order(&router, name, keys).await;
        let expected = owed(&initiated);

        let page = listing(&router, name, case, "").await;
        assert_eq!(
            page.entries(),
            expected,
            "case {case:?}: expected the uploads by key ascending and, within one key, by \
             initiation time ascending, got {:?}",
            page.entries()
        );

        // The reported `Initiated` instants agree with that order: within one key they never
        // go backwards, which is the member the second level sorts on
        // (`MultipartUpload$Initiated`, "Date and time at which the multipart upload was
        // initiated").
        let dated = page.dated_entries();
        for pair in dated.windows(2) {
            let [(before_key, before_id, before_at), (after_key, after_id, after_at)] = pair else {
                continue;
            };
            if before_key != after_key {
                continue;
            }
            assert!(
                instant_of(before_at) <= instant_of(after_at),
                "case {case:?}: {before_id:?} is reported before {after_id:?} under key \
                 {before_key:?}, so its Initiated must not be later, got {before_at:?} then \
                 {after_at:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_paged_walk_reports_that_same_order_exactly_once() {
    for (at, (case, keys)) in ROWS.iter().enumerate() {
        let router = router();
        let name = &format!("multipart-initiation-paging-{at}");
        bucket(&router, name).await;
        let initiated = initiate_in_order(&router, name, keys).await;
        let expected = owed(&initiated);

        // The page sizes differ along what a cut can fall between: every entry its own page,
        // a size that splits a key's uploads, and one that does not divide the total.
        for size in [1_usize, 2, 3] {
            let paged = format!("{case} at {size} per page");
            let seen = walked(&router, name, &paged, size, expected.len()).await;
            assert_eq!(
                seen, expected,
                "case {paged:?}: a walk must report every upload exactly once in key then \
                 initiation order, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_earlier_upload_of_a_key_leads_even_when_its_id_sorts_last() {
    // An upload id is an opaque token the service mints, so which of two initiations gets
    // the lexicographically smaller id cannot be predicted: pairs are drawn under fresh keys
    // until one comes out INVERTED (the later initiation holding the smaller id), which is
    // the only case where ordering by id and ordering by initiation time disagree. The rule
    // is asserted over EVERY pair drawn, so a minter that happened to hand out ids in
    // initiation order throughout still pins the order it reports; the inverted pair is what
    // makes the assertion tell the two candidate orders apart, and the draws stop at the
    // first one.
    const ATTEMPTS: usize = 32;
    let case = "a same-key pair whose later initiation minted the smaller id";
    let router = router();
    let name = "multipart-initiation-order-inverted";
    bucket(&router, name).await;

    // `(key, the id initiated first, the id initiated second)` per draw.
    let mut pairs: Vec<(String, String, String)> = Vec::new();
    for attempt in 0..ATTEMPTS {
        let key = format!("pair-{attempt:02}.bin");
        let first = initiate(&router, name, &key, &[]).await;
        let second = initiate(&router, name, &key, &[]).await;
        assert_ne!(
            first, second,
            "case {case:?}: two initiations of {key:?} must mint two ids, got {first:?} twice"
        );
        let inverted = second < first;
        pairs.push((key, first, second));
        if inverted {
            break;
        }
    }
    let drawn = pairs.len() * 2;

    let page = listing(&router, name, case, "").await;
    for (key, first, second) in &pairs {
        let of_the_key: Vec<String> = page
            .entries()
            .into_iter()
            .filter(|(reported, _)| reported == key)
            .map(|(_, id)| id)
            .collect();
        assert_eq!(
            of_the_key,
            vec![first.clone(), second.clone()],
            "case {case:?}: the upload initiated first must lead under key {key:?}, whichever \
             way its id {first:?} sorts against {second:?}, got {of_the_key:?}"
        );
    }

    // The first level is untouched by the second: the keys of the whole listing are still
    // ascending, each key's entries contiguous.
    let keys: Vec<String> = page.entries().into_iter().map(|(key, _)| key).collect();
    let mut ascending = keys.clone();
    ascending.sort();
    assert_eq!(
        keys, ascending,
        "case {case:?}: the keys of a listing are reported in ascending order, got {keys:?}"
    );

    // And a pair survives a walk that cuts between its two entries: a page resumes after the
    // entry it ended on, which is that pair's FIRST upload, not every id below it.
    let seen = walked(&router, name, case, 1, drawn).await;
    assert_eq!(
        seen,
        page.entries(),
        "case {case:?}: a walk at one entry per page must report what the single page did, \
         got {seen:?}"
    );
}
