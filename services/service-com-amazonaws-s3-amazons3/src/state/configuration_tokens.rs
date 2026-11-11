// SPDX-License-Identifier: CC0-1.0

//! The ONE continuation token every PAGED listing of this service hands back, and the one
//! reading of a token a caller hands in.
//!
//! (The file keeps the name it was written under, when the four listings of a bucket's
//! id-addressed configurations were the only pagers that minted one; nothing but the module
//! path depends on it.)
//!
//! Each of those listings pages, and the token that resumes a truncated page is the
//! service's to mint and the caller's to hand back unread: "the continuation token is an
//! opaque value that Amazon S3 understands"
//! (`ListBucketMetricsConfigurationsOutput$NextContinuationToken`), "the token is
//! obfuscated and is not a usable value"
//! (`ListBucketAnalyticsConfigurationsOutput$NextContinuationToken`) and "ContinuationToken
//! is obfuscated and is not a real bucket name"
//! (`ListDirectoryBucketsRequest$ContinuationToken`). Two things follow, and neither is
//! served by rendering the cursor itself:
//!
//! * a token must not be a plain rendering of the position it resumes at — a value whose
//!   text IS a stored id invites a caller to compute the next one rather than round-trip
//!   what it was handed; and
//! * a value the caller did not receive from this service names no page of the listing, so
//!   it is refused ([`super::MALFORMED_CONTINUATION_TOKEN_CODE`]) rather than read as a
//!   cursor into a position the caller chose.
//!
//! So a token carries an AUTHENTICATOR over its cursor, computed under a secret the store
//! draws from the system's randomness and never reveals
//! ([`::service_common::page_tokens::PageTokenKey`], the workspace's one reading of that):
//! no caller can produce the authenticator for a cursor of its own, and a token minted by
//! another store — another process, a restarted one — names no page here either. The
//! cursor is SCOPED to the listing it was issued for, so a token from one listing is not a
//! cursor into another. Opaque, NOT confidential, exactly as that module
//! puts it: what a caller cannot do is make a token of its own.
//!
//! No page is kept on the side: the cursor is the first thing the page did not report — a
//! configuration id, a bucket name — which is why every one of these listings walks what it
//! reports in ascending order.

use ::service_common::jwt::{decode_base64url, encode_base64url};

pub use ::service_common::page_tokens::PageTokenKey;

/// The listing a continuation token was issued for.
///
/// Named inside the token so a `NextContinuationToken` from one listing names no
/// page of another's: the four families a bucket holds per-id configurations of, each bound
/// at its own subresource (`?analytics`, `?metrics`, `?inventory`,
/// `?intelligent-tiering`), and the account's DIRECTORY buckets, which pages by bucket name
/// on its own endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedListing {
    /// `ListBucketAnalyticsConfigurations`.
    Analytics,
    /// `ListDirectoryBuckets`.
    DirectoryBuckets,
    /// `ListBucketIntelligentTieringConfigurations`.
    IntelligentTiering,
    /// `ListBucketInventoryConfigurations`.
    Inventory,
    /// `ListBucketMetricsConfigurations`.
    Metrics,
}

impl PagedListing {
    /// The name this listing is authenticated under, inside the token. One name per listing
    /// and no part of any response — the subresource a configuration listing is bound at,
    /// and the resource the bucket listing reports.
    fn label(self) -> &'static str {
        match self {
            Self::Analytics => "analytics",
            Self::DirectoryBuckets => "directory-buckets",
            Self::IntelligentTiering => "intelligent-tiering",
            Self::Inventory => "inventory",
            Self::Metrics => "metrics",
        }
    }
}

/// The separator between the listing a token was issued for, the resource it was issued
/// AGAINST and the position in it: a unit separator, which no listing's label, no bucket name
/// and no cursor carries (no configuration id ([`super::CONFIGURATION_ID_PATTERN`]) and no
/// bucket name admits it), so the three parts of a cursor cannot run together.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The `NextContinuationToken` that resumes a listing at `first_not_returned` (inclusive):
/// the authenticated bytes of that position, rendered in base64url.
///
/// Base64 is the spelling an AWS continuation token has, in the URL-safe alphabet (RFC 4648
/// §5) so the value survives the `continuation-token` query parameter of the next request
/// whatever encodes it — through the workspace's one base64url codec
/// ([`::service_common::jwt::encode_base64url`], declared beside the JOSE reader that needed
/// it first), so no second alphabet swap lives here. The authenticator inside is what makes a
/// token this store did not mint detectable, which is what lets the next page refuse it
/// instead of serving from a position the caller chose.
///
/// `scope` is the resource the listing was taken OVER — the bucket a configuration listing is
/// bound to, and the empty string for a listing of the account itself — so a token issued for
/// one bucket names no page of the same listing on another.
pub fn encode_continuation_token(
    key: &PageTokenKey,
    listing: PagedListing,
    scope: &str,
    first_not_returned: &str,
) -> String {
    encode_base64url(&key.mint(&format!(
        "{}{CURSOR_SEPARATOR}{scope}{CURSOR_SEPARATOR}{first_not_returned}",
        listing.label()
    )))
}

/// The position `token` resumes `listing` over `scope` at (inclusive), or `None` for any
/// value this store did not mint for that listing and that scope: one that is not base64url,
/// one whose authenticator does not match the cursor it travels with, one issued for another
/// listing, one issued for another BUCKET, and one carrying no cursor at all.
pub fn decode_continuation_token(
    key: &PageTokenKey,
    listing: PagedListing,
    scope: &str,
    token: &str,
) -> Option<String> {
    match read_continuation_token(key, listing, scope, token) {
        ContinuationTokenReading::Position(resume_at) => Some(resume_at),
        _ => None,
    }
}

/// What `token` is, read against the listing and the scope the request addresses.
///
/// Three outcomes, because S3 answers them differently: a token that names a position here
/// resumes the listing, a token this service issued over ANOTHER resource names a position
/// the listing cannot be resumed from, and a value this service never issued names no page at
/// all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationTokenReading {
    /// The position the listing resumes at (inclusive).
    Position(String),
    /// A token this store minted for this listing, but over another scope — another bucket's
    /// listing of the same family.
    OtherScope,
    /// Not a token this store minted for this listing at all: not base64url, authenticated
    /// under another key, issued for another listing, or carrying no cursor.
    Unknown,
}

impl ContinuationTokenReading {
    /// The refusal a bucket-configuration listing answers an UNUSABLE reading with, as
    /// `(code, status, message)` — and the not-a-token refusal for a reading that names a
    /// position, which a caller matching `Position` first never reaches.
    ///
    /// Recorded behavior, and the two readings are answered DIFFERENTLY. A token the caller
    /// FORGED — one S3 never issued — names no page of the listing, which S3 reports against
    /// the token itself ([`super::MALFORMED_CONTINUATION_TOKEN_CODE`], HTTP 400, the input
    /// error a paginating client branches on). A token S3 DID issue, for another bucket's
    /// listing of the same family, is a cursor into a listing this request does not address:
    /// the recorded answer to one is [`super::INTERNAL_ERROR_CODE`] and its own published
    /// sentence, rather than the addressed bucket's first page or a page resolved against
    /// the addressed bucket's contents. The issue this was filed under reads the second case
    /// as the first; the recording is what is followed here.
    pub fn refusal(&self) -> (&'static str, u16, &'static str) {
        match self {
            Self::OtherScope => (
                super::INTERNAL_ERROR_CODE,
                super::INTERNAL_ERROR_STATUS,
                super::INTERNAL_ERROR_MESSAGE,
            ),
            Self::Position(_) | Self::Unknown => (
                super::MALFORMED_CONTINUATION_TOKEN_CODE,
                super::MALFORMED_CONTINUATION_TOKEN_STATUS,
                super::MALFORMED_CONTINUATION_TOKEN_MESSAGE,
            ),
        }
    }
}

/// Read `token` against `listing` and `scope`. See [`ContinuationTokenReading`].
pub fn read_continuation_token(
    key: &PageTokenKey,
    listing: PagedListing,
    scope: &str,
    token: &str,
) -> ContinuationTokenReading {
    let Some(cursor) = decode_base64url(token).and_then(|bytes| key.cursor_of(&bytes)) else {
        return ContinuationTokenReading::Unknown;
    };
    let Some((label, rest)) = cursor.split_once(CURSOR_SEPARATOR) else {
        return ContinuationTokenReading::Unknown;
    };
    let Some((issued_for, resume_at)) = rest.split_once(CURSOR_SEPARATOR) else {
        return ContinuationTokenReading::Unknown;
    };
    if label != listing.label() {
        return ContinuationTokenReading::Unknown;
    }
    match issued_for == scope {
        true => ContinuationTokenReading::Position(resume_at.to_string()),
        false => ContinuationTokenReading::OtherScope,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MAX_CONFIGURATION_ID_CHARS;

    /// Every listing's token names its position under the key that minted it, for the scope
    /// it was issued against, and nothing else does: another store's key, another family's
    /// listing, another bucket, or a value a caller wrote.
    #[test]
    fn a_token_names_a_position_only_for_the_store_and_listing_that_issued_it() {
        let key = PageTokenKey::new();
        let other_store = PageTokenKey::new();
        let listings = [
            PagedListing::Analytics,
            PagedListing::DirectoryBuckets,
            PagedListing::IntelligentTiering,
            PagedListing::Inventory,
            PagedListing::Metrics,
        ];
        let scope = "a-bucket";
        for listing in listings {
            // Cursors spanning what one can be: one character, every character class a
            // configuration id allows, one at the published ceiling, and a directory
            // bucket's name.
            for id in [
                "a".to_string(),
                "Report.one-2_3".to_string(),
                "z".repeat(MAX_CONFIGURATION_ID_CHARS),
                "base--usw2-az1--x-s3".to_string(),
            ] {
                let token = encode_continuation_token(&key, listing, scope, &id);
                assert_eq!(
                    decode_continuation_token(&key, listing, scope, &token).as_deref(),
                    Some(id.as_str()),
                    "case {listing:?}/{id:?}: a minted token must name its position, got \
                     {token:?}"
                );
                assert_eq!(
                    decode_continuation_token(&other_store, listing, scope, &token),
                    None,
                    "case {listing:?}/{id:?}: another store's key must name no position"
                );
                assert_eq!(
                    decode_continuation_token(&key, listing, "another-bucket", &token),
                    None,
                    "case {listing:?}/{id:?}: another scope must name no position"
                );
                for elsewhere in listings.iter().filter(|other| **other != listing) {
                    assert_eq!(
                        decode_continuation_token(&key, *elsewhere, scope, &token),
                        None,
                        "case {listing:?}/{id:?}: must name no position in {elsewhere:?}"
                    );
                }
                // The token is not a RENDERING of the position it resumes at: neither the
                // id itself nor any value a caller could compute from it without the key.
                for (spelling, computed) in [
                    ("the id itself", id.clone()),
                    ("hex of the id", ::hex::encode(&id)),
                    (
                        "base64 of the id",
                        ::aws_smithy_types::base64::encode(id.as_str()),
                    ),
                ] {
                    assert_ne!(
                        token, computed,
                        "case {listing:?}/{id:?}: the token must not be {spelling}, got \
                         {token:?}"
                    );
                }
                assert!(
                    token
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_')),
                    "case {listing:?}/{id:?}: a token is URL-safe base64, got {token:?}"
                );
            }
        }

        // Values a caller could write rather than round-trip, including one with the SHAPE
        // of a minted token, which only the authenticator can refuse.
        let shaped =
            encode_base64url(&[0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES + 1]);
        for (case, token) in [
            ("an id", "kept".to_string()),
            ("empty", String::new()),
            ("not base64url", "not a token".to_string()),
            ("base64url but too short", "a2VwdA".to_string()),
            ("a minted token's shape", shaped),
        ] {
            assert_eq!(
                decode_continuation_token(&key, PagedListing::Metrics, scope, &token),
                None,
                "case {case}: expected no position for {token:?}"
            );
        }
    }
}
