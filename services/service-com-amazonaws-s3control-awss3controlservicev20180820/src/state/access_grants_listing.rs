// SPDX-License-Identifier: CC0-1.0

//! The PAGING every S3 Access Grants listing reads: how big a page a request asked for, and
//! where in the scan a continuation token says to resume.
//!
//! The three listings of the Access Grants surface — `ListAccessGrants`,
//! `ListAccessGrantsLocations` and `ListCallerAccessGrants` — are `@paginated` with the same
//! two members (`MaxResults`, `NextToken` / `inputToken`), and each scans a `BTreeMap` slice
//! of its instance's records in key order, so a token names the LAST key the previous page
//! reported and the next page resumes strictly after it. Stated once here so the three
//! cannot read the same two members differently.
//!
//! The token itself is this service's shared opaque form ([`crate::state::encode_page_token`]
//! / [`crate::state::decode_page_token`]), the one `ListAccessPoints` hands back, so nothing
//! about a token's contents is part of the contract.

/// The largest page any of these listings reports at once.
///
/// `MaxResults` is modelled `@range(min: 0, max: 1000)`
/// (`smithy_query(query="operation",operation="ListAccessGrants")`), so 1000 is the member's
/// own maximum and the page a request that names NO size gets.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// How many records the page a request asked for holds.
///
/// `MaxResults` carries `@default: 0`, and 0 is "the caller named no page size": the page is
/// then the whole remaining result set, up to [`MAX_PAGE_SIZE`]. A value outside the modelled
/// range cannot arrive here — the boundary validator refuses it before a handler runs — but
/// if one ever does it is refused rather than silently widened or clamped.
pub fn page_size<E>(max_results: i32) -> Result<usize, crate::scaffolded::Scaffolded<E>> {
    match usize::try_from(max_results) {
        Ok(0) => Ok(MAX_PAGE_SIZE),
        Ok(requested) if requested <= MAX_PAGE_SIZE => Ok(requested),
        Ok(_) | Err(_) => Err(crate::scaffolded::Scaffolded::validation(format!(
            "MaxResults must be between 0 and {MAX_PAGE_SIZE}"
        ))),
    }
}

/// The key the scan resumes strictly after, or `None` for a request that carries no token.
///
/// A token this service did not issue names no position in the scan, so it is refused rather
/// than read as a cursor into a position the caller chose.
pub fn resume_after<E>(
    next_token: Option<&str>,
) -> Result<Option<String>, crate::scaffolded::Scaffolded<E>> {
    match next_token {
        None => Ok(None),
        Some(token) => match crate::state::decode_page_token(token) {
            Some(key) => Ok(Some(key)),
            None => Err(crate::scaffolded::Scaffolded::validation(
                "NextToken is not a continuation token this service issued",
            )),
        },
    }
}
