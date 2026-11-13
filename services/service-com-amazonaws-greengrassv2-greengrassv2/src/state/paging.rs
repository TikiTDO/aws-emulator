// SPDX-License-Identifier: CC0-1.0

//! What every paginated reader of this service shares: how big a page is, where one
//! starts, and how a cursor this service did not issue is refused.
//!
//! Stated once here rather than per operation, so two listings cannot page differently or
//! word the same bad cursor two ways. The cursor itself is the opaque token
//! [`super::encode_cursor`] mints from the key a page resumes at.

use super::*;
use crate::ops::types::{ValidationException, ValidationExceptionField};

/// How many items one page of a list reports.
///
/// The model's `DefaultMaxResults` is `range 1..100` and declares no `@default`, so a
/// request that names no `maxResults` is served the whole-collection page
/// ([`MAX_PAGE_SIZE`]). A value outside the modelled range is refused before any handler
/// runs, so the clamp here only keeps the conversion total.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)).unwrap_or(1)
}

/// Where a page starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    /// The request carried no `nextToken`, so the page starts at the first item.
    Start,
    /// The request carried a `nextToken` this service issued, which resumes at this key.
    ResumeAt(String),
    /// The request carried a `nextToken` this service did not issue.
    Unreadable,
}

/// Where the page a `nextToken` names starts.
pub fn cursor(next_token: Option<&str>) -> Cursor {
    match next_token {
        None => Cursor::Start,
        Some(token) => match decode_cursor(token) {
            Some(key) => Cursor::ResumeAt(key),
            None => Cursor::Unreadable,
        },
    }
}

/// The refusal a paginated reader answers a `nextToken` it did not issue: a cursor is this
/// service's own opaque token, so one it cannot read names no page and the request is bad
/// input rather than an empty page.
pub fn unreadable_next_token() -> ValidationException {
    ValidationException {
        message: "nextToken is not a token this service issued.".to_string(),
        reason: Some("FIELD_VALIDATION_FAILED".to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "nextToken".to_string(),
            message: "the cursor could not be decoded".to_string(),
        }]),
    }
}
