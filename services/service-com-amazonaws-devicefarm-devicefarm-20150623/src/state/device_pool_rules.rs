// SPDX-License-Identifier: CC0-1.0

//! The operand of a device-pool selection rule: the one judgement every operation that
//! writes a pool's `rules` applies before it writes.
//!
//! A `Rule` names a device attribute, an operator and the attribute value to compare
//! against. That value is carried as a STRINGIFIED attribute value, not as the bare text of
//! it: the model's own `Rule$attribute` documentation says to "specify the value as
//! `"\"abc\""`", i.e. a JSON-quoted string — and for the set operators, the JSON list of
//! quoted strings they compare against. So an operand that is not the JSON form of a string
//! value is an operand the service cannot parse, and the rule is a bad argument rather than
//! a selection that matches nothing.
//!
//! The judgement lives here, beside the store, because every operation that sets a pool's
//! rules (`CreateDevicePool` and `UpdateDevicePool`) applies the same one, and the sentence
//! it is refused with belongs to the rule rather than to the handler that happened to carry
//! it.

use crate::ops::types::Rule;

/// Why the first rule of `rules` whose operand the service cannot parse is refused, or
/// `None` when every operand is a stringified attribute value.
///
/// A rule carrying NO operand has nothing to parse, so it is not an unparseable one: the
/// member is optional in the model, and a rule that omits it is left to the pool exactly as
/// it was supplied.
///
/// The returned sentence is the whole `ArgumentException` message: Device Farm states what
/// it was judging, then the operand it could not parse, then the form an operand has to
/// take, with examples. Only the operand is variable, and it is echoed verbatim from the
/// request's own rule. The examples quote their own quotation marks — the sentence is about
/// the quoting, so each `"` it names is written as `\"` in the message text itself.
pub fn rule_operand_refusal(rules: &[Rule]) -> Option<String> {
    let operand = rules
        .iter()
        .filter_map(|rule| rule.value.as_deref())
        .find(|operand| !is_stringified_attribute_value(operand))?;
    Some(format!(
        "Invalid request, please check the arn and the device pool rule. Parse error for \
         attribute operand: {operand}. Rule requires a String with \\\" on both sides for \
         example: \\\"ANDROID\\\", \\\"4.4.2\\\", \\\"PHONE\\\", \\\"iPad 5th Gen\\\"."
    ))
}

/// Whether `operand` is the stringified form of an attribute value.
///
/// Two forms parse: a JSON string (the single value an `EQUALS`, `CONTAINS` or ordering
/// comparison is made against) and a JSON array of JSON strings (the set the `IN` and
/// `NOT_IN` operators select from, which the model documents for those operators). Anything
/// else — bare text, a bare number or boolean, a quote that is never closed, a JSON object,
/// a list carrying something that is not a string — is not a stringified attribute value.
/// The list form is accepted whatever the operator, because refusing it where the model
/// does not forbid it would refuse a call the service serves.
fn is_stringified_attribute_value(operand: &str) -> bool {
    match ::serde_json::from_str::<::serde_json::Value>(operand) {
        Ok(::serde_json::Value::String(_)) => true,
        Ok(::serde_json::Value::Array(values)) => {
            values.iter().all(::serde_json::Value::is_string)
        }
        _ => false,
    }
}
