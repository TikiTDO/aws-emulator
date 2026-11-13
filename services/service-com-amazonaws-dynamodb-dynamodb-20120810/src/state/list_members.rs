// SPDX-License-Identifier: CC0-1.0

//! A list member a request CARRIES but states no entry in.
//!
//! A list the request leaves out and a list it sends empty are two different requests:
//! the first asks for nothing, while the second declares a list OF nothing — and a member
//! whose whole content is its entries cannot be satisfied by an empty one, so it is
//! refused rather than read as absent. The model does not state the rule for the index
//! lists a table declaration carries (`smithy_query(query="operation",
//! operation="CreateTable")` reports `GlobalSecondaryIndexes` and `LocalSecondaryIndexes`
//! with no `@length` minimum), so the service is what enforces it, with one fixed
//! parameter-value sentence naming the member ([`empty_list_message`]).
//!
//! It lives beside [`super`] rather than inside it because it is about the shape of the
//! REQUEST rather than about the table and item store; `super` re-exports it, so every
//! caller still names it `crate::state::…`.

use super::RequestError;

/// AWS's fixed text for a list member the request carries with no entries at all: the
/// sentence is fixed throughout and the member's own name, as the model spells it on the
/// wire, is the only substituted part — the fact is about the member rather than about the
/// resource the request addressed.
pub fn empty_list_message(member: &str) -> String {
    format!("One or more parameter values were invalid: List of {member} is empty")
}

/// Refuse the first member of `members` the request carries with no entries.
///
/// Each entry pairs a member's wire name with the number of entries the request stated for
/// it, and `None` is a member the request leaves out — which asks for nothing and is no
/// defect. The caller lists the members in the order it reads them, which is the order a
/// refusal names them in.
pub fn check_lists_not_empty<'m>(
    members: impl IntoIterator<Item = (&'m str, Option<usize>)>,
) -> Result<(), RequestError> {
    for (member, entries) in members {
        if entries == Some(0) {
            return Err(RequestError::Validation(empty_list_message(member)));
        }
    }
    Ok(())
}
