// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk every paginated read of this service shares: the slice of a
//! collection one call answers, and the `NextMarker` that resumes after it.
//!
//! Every one of these reads is modelled `@paginated(inputToken: Marker, outputToken:
//! NextMarker)`, and the model types both members as one string shape (`Marker`): the
//! value is the service's to mint and the caller's to hand back unread. A caller that
//! stores it, compares it or validates it treats it as opaque, so the position it names
//! travels INSIDE the token rather than being the token — a bare ordinal would both
//! invite a caller to compute its own and stop naming the same place as soon as the
//! collection changes.
//!
//! So a marker carries an authenticator over the position it names, computed under a
//! secret this store draws from the system's randomness and never reveals
//! ([`PageTokenKey`], the workspace's one reading of that), rendered as hex. That is what
//! lets a read tell a marker it ISSUED from one a caller wrote: a value this store did
//! not mint names no position at all and is refused, which is what the reads' own
//! refusal text already promises. The position is scoped to the COLLECTION it was issued
//! for, so a marker from one collection does not name a position in another.
//!
//! No page is kept on the side: the cursor is the offset into the collection, which is
//! why every paginated read here walks its collection in a stable order.

pub use ::service_common::page_tokens::PageTokenKey;

/// Why a page request could not be served.
#[derive(Debug)]
pub enum PageError {
    /// The `Marker` is not one this store issued for this collection.
    InvalidMarker,
}

/// The collection a `Marker` names a position in: the `items` member of the operation's
/// `@paginated` trait, which is the collection that read walks.
///
/// Named so a marker is bound to the collection it was issued for — a `NextMarker` from a
/// page of rules names no position in the listeners — and so the refusal every read
/// states ("a NextMarker from a previous page of this collection") is the refusal it
/// performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedCollection {
    AccountLimits,
    Certificates,
    Listeners,
    LoadBalancers,
    Rules,
    SslPolicies,
    TargetGroups,
    TrustStoreAssociations,
    TrustStores,
}

impl PagedCollection {
    /// The name this collection is authenticated under, inside the marker.
    fn label(self) -> &'static str {
        match self {
            Self::AccountLimits => "Limits",
            Self::Certificates => "Certificates",
            Self::Listeners => "Listeners",
            Self::LoadBalancers => "LoadBalancers",
            Self::Rules => "Rules",
            Self::SslPolicies => "SslPolicies",
            Self::TargetGroups => "TargetGroups",
            Self::TrustStoreAssociations => "TrustStoreAssociations",
            Self::TrustStores => "TrustStores",
        }
    }
}

/// The separator between the collection a marker was issued for and the position in it.
/// A unit separator, which no collection label carries, so the two parts of a cursor
/// cannot run together.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// One page of `items`, plus the `NextMarker` that resumes after it.
///
/// A read is handed everything the request selected and answers at most `page_size` of
/// it: with no `PageSize` the whole selection is one page, which is the answer an
/// operation whose `PageSize` the caller left out gives — a non-paginating client sees
/// every item rather than one page of them, and no `NextMarker` comes back with it.
///
/// `marker` resumes a previous walk: it is the token this store minted for the position
/// after the page it came with ([`encode_marker`]), and any other value is
/// [`PageError::InvalidMarker`].
pub fn paginate<T>(
    items: Vec<T>,
    marker: Option<&str>,
    page_size: Option<i32>,
    key: &PageTokenKey,
    collection: PagedCollection,
) -> Result<(Vec<T>, Option<String>), PageError> {
    let start = match marker {
        None => 0usize,
        Some(marker) => decode_marker(key, collection, marker).ok_or(PageError::InvalidMarker)?,
    };
    if start > items.len() {
        return Err(PageError::InvalidMarker);
    }
    let mut page: Vec<T> = items.into_iter().skip(start).collect();
    let next = match page_size.and_then(|size| usize::try_from(size).ok()) {
        Some(size) if page.len() > size => {
            page.truncate(size);
            Some(encode_marker(key, collection, start + size))
        }
        _ => None,
    };
    Ok((page, next))
}

/// The `NextMarker` that resumes at `offset` of `collection`: the authenticated bytes of
/// that position, hex-encoded.
///
/// Hex is this service's spelling for an opaque marker — it is what makes the value one a
/// caller round-trips rather than computes — while the authenticator inside makes a value
/// this store did not mint detectable, which is what lets a read refuse it instead of
/// serving a page from a position the caller chose.
pub fn encode_marker(key: &PageTokenKey, collection: PagedCollection, offset: usize) -> String {
    ::hex::encode(key.mint(&format!(
        "{}{CURSOR_SEPARATOR}{offset}",
        collection.label()
    )))
}

/// The position `marker` names in `collection`, or `None` for any value this store did not
/// mint for it: one that is not hex, one whose authenticator does not match the position it
/// travels with, one issued for another collection, and one whose position is no number.
pub fn decode_marker(
    key: &PageTokenKey,
    collection: PagedCollection,
    marker: &str,
) -> Option<usize> {
    let cursor = key.cursor_of(&::hex::decode(marker).ok()?)?;
    let (label, offset) = cursor.split_once(CURSOR_SEPARATOR)?;
    if label != collection.label() {
        return None;
    }
    offset.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The positions a marker is minted for round-trip under the key that minted them,
    /// and nothing else does: another store's key, another collection, a value a caller
    /// wrote.
    #[test]
    fn a_marker_names_a_position_only_for_the_store_and_collection_that_issued_it() {
        let key = PageTokenKey::new();
        let other_store = PageTokenKey::new();
        let collections = [
            PagedCollection::AccountLimits,
            PagedCollection::Certificates,
            PagedCollection::Listeners,
            PagedCollection::LoadBalancers,
            PagedCollection::Rules,
            PagedCollection::SslPolicies,
            PagedCollection::TargetGroups,
            PagedCollection::TrustStoreAssociations,
            PagedCollection::TrustStores,
        ];
        for collection in collections {
            // Positions spanning a walk: the first page's end, a middle page, and a
            // collection far larger than any one page.
            for offset in [0usize, 1, 2, 7, 400, 4_000] {
                let marker = encode_marker(&key, collection, offset);
                assert_eq!(
                    decode_marker(&key, collection, &marker),
                    Some(offset),
                    "case {collection:?}/{offset}: a minted marker must name its position, \
                     got {marker:?}"
                );
                assert_eq!(
                    decode_marker(&other_store, collection, &marker),
                    None,
                    "case {collection:?}/{offset}: another store's key must name no position"
                );
                for elsewhere in collections.iter().filter(|other| **other != collection) {
                    assert_eq!(
                        decode_marker(&key, *elsewhere, &marker),
                        None,
                        "case {collection:?}/{offset}: must name no position in {elsewhere:?}"
                    );
                }
                assert!(
                    marker.chars().all(|char| char.is_ascii_hexdigit()),
                    "case {collection:?}/{offset}: a marker is hex, got {marker:?}"
                );
                assert_ne!(
                    marker,
                    offset.to_string(),
                    "case {collection:?}/{offset}: a marker is not the ordinal itself"
                );
            }
        }

        // Values a caller could write rather than round-trip, including one with the
        // shape of a minted marker.
        let shaped = ::hex::encode(vec![0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES]);
        for (case, marker) in [
            ("an ordinal", "1".to_string()),
            ("empty", String::new()),
            ("not hex", "not-a-marker".to_string()),
            ("hex but too short", "ab".to_string()),
            ("a minted marker's shape", shaped),
        ] {
            assert_eq!(
                decode_marker(&key, PagedCollection::Rules, &marker),
                None,
                "case {case}: expected no position for {marker:?}"
            );
        }
    }

    /// A walk of one collection reports every item once, in order, and ends with no
    /// marker; a read handed no `PageSize` answers the whole selection in one page.
    #[test]
    fn a_walk_reports_every_item_once_and_ends() {
        let key = PageTokenKey::new();
        for count in [0usize, 1, 2, 5] {
            let items: Vec<usize> = (0..count).collect();
            let (page, next) =
                paginate(items.clone(), None, None, &key, PagedCollection::Rules).expect("a page");
            assert_eq!(
                (page, next.clone()),
                (items.clone(), None),
                "case {count} items with no PageSize: expected the whole collection and no marker"
            );

            for size in 1..=count + 1 {
                let mut walked: Vec<usize> = Vec::new();
                let mut marker: Option<String> = None;
                for _ in 0..count + 2 {
                    let (page, next) = paginate(
                        items.clone(),
                        marker.as_deref(),
                        Some(i32::try_from(size).expect("a page size")),
                        &key,
                        PagedCollection::Rules,
                    )
                    .expect("a page");
                    assert!(
                        page.len() <= size,
                        "case {count} items at PageSize {size}: a page holds at most {size}, \
                         got {page:?}"
                    );
                    walked.extend(page);
                    marker = next;
                    if marker.is_none() {
                        break;
                    }
                }
                assert_eq!(
                    marker, None,
                    "case {count} items at PageSize {size}: the walk must end with no marker"
                );
                assert_eq!(
                    walked, items,
                    "case {count} items at PageSize {size}: expected every item once in order"
                );
            }
        }
    }
}
