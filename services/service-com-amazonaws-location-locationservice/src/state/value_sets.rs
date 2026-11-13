// SPDX-License-Identifier: CC0-1.0

//! The one place this service words a refusal about a member whose value has to
//! come from a documented set.
//!
//! Several members of this API are plain strings in the model (
//! `CreateRouteCalculatorRequest$DataSource`, `MapConfiguration$Style`, and the
//! other members whose doc lists "valid values") rather than enums, so the request
//! boundary accepts any text and WHICH values exist is the service's own check. All
//! of those refusals read as one sentence, so the sentence is written once here and
//! every such check builds it with [`documented_value_set_refusal`].
//!
//! The name says WHICH sentence this words: the framework's generic
//! constraint-violation clause for an enumerated type is a different sentence, worded
//! by the shared validation-message helpers, and the two must not be confused.

/// The sentence a member carrying a value outside its documented set is refused
/// with: the member name as the model spells it in single quotes, then what the
/// member has to satisfy — the allowed values in the order the documentation lists
/// them, separated by ", " — then the period that ends the sentence. The value the
/// request sent is never quoted back, so the sentence names only the member and the
/// set a caller may choose from.
///
/// `member` is the modelled member name and `allowed` the documented set; those are
/// the only parts that vary between the members this service checks this way.
pub fn documented_value_set_refusal<I>(member: &str, allowed: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut sentence = format!("'{member}' must be one of ");
    for (index, value) in allowed.into_iter().enumerate() {
        if index > 0 {
            sentence.push_str(", ");
        }
        sentence.push_str(value.as_ref());
    }
    sentence.push('.');
    sentence
}
