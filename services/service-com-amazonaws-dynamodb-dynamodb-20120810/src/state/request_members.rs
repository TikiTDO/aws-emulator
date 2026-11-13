// SPDX-License-Identifier: CC0-1.0

//! How this service states that a PARAMETER its own request code requires is not in the
//! request at all.
//!
//! A fact of the request's own shape rather than of the table and item store, so it lives
//! beside [`super`] rather than inside it; `super` re-exports it, so every caller still
//! names it `crate::state::…`.

/// The sentence a request that carries no value for such a parameter earns.
///
/// Not a clause and not inside the `<N> validation error(s) detected: ` envelope: the
/// absence is found by the service's own request code before any constraint is walked, so
/// the whole message is this one sentence, with the member named as the model spells it on
/// the wire and nothing of the operation that was called.
///
/// Which operations answer an absent member this way, and which keep the validating walk's
/// `Member must not be null` clause, follows the OPERATION served and is recorded in
/// `src/service.rs` beside the other refusal wordings that do.
pub fn missing_parameter_message(member: &str) -> String {
    format!("The parameter '{member}' is required but was not present in the request")
}
