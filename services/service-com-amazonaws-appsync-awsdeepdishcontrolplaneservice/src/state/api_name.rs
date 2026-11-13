// SPDX-License-Identifier: CC0-1.0

//! The `name` an `Api` — an Event API — carries: the form the model declares for it, and
//! the refusal a request outside that form takes.
//!
//! Two operations carry that member, and only those two: `CreateApi` (POST /v2/apis) and
//! `UpdateApi` (POST /v2/apis/{apiId}), each taking `name` @required -> `ApiName`
//! (`smithy_query(query="operation", operation="CreateApi,UpdateApi")`), a string
//! `@length` 1..50 with `@pattern "^[A-Za-z0-9_\-\ ]+$"`
//! (`smithy_query(query="shape", shape="ApiName")`).
//!
//! THE REFUSAL IS THE FRAMEWORK'S, not this service's. A value outside that form is
//! answered with the framework-level constraint error — HTTP 400 with the error code
//! `ValidationException` and the framework's own constraint report — where every OTHER
//! member of this service answers a broken `@length` or `@pattern` with the service's own
//! modelled `BadRequestException` (`crate::service`). The recorded refusals of this
//! member show the first; the recorded refusals of the other operations show the second,
//! so the choice is per operation and not a property of the constraint class.
//!
//! That is why the check runs HERE rather than at the request boundary (`skip_validation`
//! on `name` in `src/ops/types/`, the same split [`crate::state::description_refusal`] is
//! there for): the seam that words a boundary refusal
//! (`smithy_server_runtime::Service::override_refusal`) is handed the broken clauses
//! alone, with no operation to key on, and `name` is also the member of other operations
//! of this service whose target shape declares the very same bounds (`Namespace`, `@length`
//! 1..50) — so the operation's own handler is the one place the code can be chosen for the
//! operation that carries this shape.
//!
//! The REPORT is the tree's one statement of the framework's own
//! (`service_common::validation_message`): the clause names the member and the requirement
//! it broke, the `@pattern` is stated as the expression the model wraps rather than the
//! whole-value wrapping it writes around it, and the submitted value is not quoted back —
//! which is the sentence the recorded refusal of this member carries.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::smithy_server_runtime::Refusal;

/// `ApiName`'s modelled minimum length: a name is at least one character long.
pub const API_NAME_MIN_LEN: usize = 1;

/// `ApiName`'s modelled maximum length.
pub const API_NAME_MAX_LEN: usize = 50;

/// `ApiName`'s modelled `@pattern`, as the model writes it — anchored at both ends, so the
/// expression has to admit the WHOLE name. The clause a refusal states quotes the
/// expression those anchors wrap rather than this text
/// (`service_common::validation::unanchored`).
pub const API_NAME_PATTERN: &str = r"^[A-Za-z0-9_\-\ ]+$";

/// The member both operations carry the name on, spelled as the framework's own validating
/// walk spells a member of an operation's input, because that is the member path the
/// clause reports.
pub const API_NAME_MEMBER: &str = "name";

/// [`API_NAME_PATTERN`] compiled, which is how it is EVALUATED: the expression is a
/// constant of the model, so it compiles once for the process and never per request, and
/// the engine is the one the request boundary's own constraint walk evaluates a `@pattern`
/// with — so this check and the boundary's cannot read one expression two ways.
///
/// `None` for an expression that does not compile, which [`matches_api_name_pattern`]
/// reads as matching, exactly as the framework's own walk reads one: no value is refused
/// because of an unreadable model pattern.
static API_NAME_REGEX: ::std::sync::LazyLock<Option<::regex::Regex>> =
    ::std::sync::LazyLock::new(|| ::regex::Regex::new(API_NAME_PATTERN).ok());

/// Whether `name` satisfies [`API_NAME_PATTERN`]. A Smithy `@pattern` is a FIND rather
/// than a total match, which is what [`::regex::Regex::is_match`] is; this expression
/// anchors both ends itself, so the find it describes is the whole value.
fn matches_api_name_pattern(name: &str) -> bool {
    API_NAME_REGEX
        .as_ref()
        .is_none_or(|expression| expression.is_match(name))
}

/// Every constraint of `ApiName` that `name` breaks, in the order the framework's own
/// constraint walk checks them: the `@length` bound first — the one bound that was crossed,
/// as that walk reports it — and then the `@pattern`.
///
/// The length is counted in CHARACTERS, which is what Smithy's `@length` bounds on a
/// string, so a multi-byte name is measured the way the model measures it.
fn api_name_violations(name: &str) -> Vec<Violation> {
    let length = name.chars().count();
    let bound = if length < API_NAME_MIN_LEN {
        Some(ViolationKind::MinLength(API_NAME_MIN_LEN as i64))
    } else if length > API_NAME_MAX_LEN {
        Some(ViolationKind::MaxLength(API_NAME_MAX_LEN as i64))
    } else {
        None
    };
    let pattern = (!matches_api_name_pattern(name))
        // Carried as the model WRITES it: the rendering that drops the whole-value
        // anchors belongs to the report, not to the constraint.
        .then(|| ViolationKind::Pattern(API_NAME_PATTERN.to_string()));
    bound
        .into_iter()
        .chain(pattern)
        .map(|kind| Violation {
            path: API_NAME_MEMBER.to_string(),
            kind,
            // No value: the report this is rendered through states none for this member
            // (see the module docs), so there is nothing to quote and nothing of a
            // caller's bytes is copied to be dropped again. The CHECKS above were made
            // against the whole submitted value.
            value: None,
            sensitive: false,
        })
        .collect()
}

/// The constraint report for a `name` outside the form `ApiName` declares, or `None` for
/// one inside it.
///
/// Carried by the framework-level constraint error — HTTP 400 `ValidationException`,
/// `crate::scaffolded::Scaffolded::validation` — which is the answer this surface's
/// recordings show for this member, and the reason both operations that take it check it
/// in their own handlers (see the module docs).
pub fn api_name_refusal(name: &str) -> Option<String> {
    let violations = api_name_violations(name);
    if violations.is_empty() {
        return None;
    }
    // The clauses and the envelope around them are the framework's, stated through the
    // tree's one copy of that rendering, with the value half dropped from every clause of
    // this member. `None` only for no violation at all, which was answered above.
    ::service_common::validation_message::aws_validation_refusal_withholding_violations(
        &Refusal::Invalid(violations),
        |_| true,
    )
    .map(|refused| refused.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clause a refusal of this member states for `requirement`.
    fn expected(requirement: &str) -> String {
        format!(
            "1 validation error detected: Value at '{API_NAME_MEMBER}' failed to satisfy \
             constraint: {requirement}"
        )
    }

    #[test]
    fn a_name_inside_the_declared_form_is_not_refused() {
        for (case, name) in [
            ("the shortest name the bound admits", "E".repeat(API_NAME_MIN_LEN)),
            ("every character class the expression admits", "Ev3nt_api-name with spaces".to_string()),
            ("the longest name the bound admits", "E".repeat(API_NAME_MAX_LEN)),
            ("a name of only spaces, which the class admits", " ".repeat(3)),
        ] {
            let refusal = api_name_refusal(&name);
            assert!(
                refusal.is_none(),
                "case {case:?}: expected no refusal for {name:?}, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_name_outside_the_expression_states_it_without_the_anchors_and_without_the_value() {
        for (case, name) in [
            ("an at sign", "bad@name"),
            ("a hash", "name#1"),
            ("an exclamation mark", "Events!"),
            ("a dot, which the class does not admit", "Events.v2"),
            ("a character outside ASCII", "Événements"),
        ] {
            let refusal = api_name_refusal(name);
            let requirement =
                r"Member must satisfy regular expression pattern: [A-Za-z0-9_\-\ ]+";
            assert_eq!(
                refusal.as_deref(),
                Some(expected(requirement).as_str()),
                "case {case:?}: expected the unanchored expression and no value, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_name_outside_the_declared_length_states_the_bound_it_crossed() {
        let over_long = "E".repeat(API_NAME_MAX_LEN + 1);
        let refusal = api_name_refusal(&over_long);
        let requirement = format!("Member must have length less than or equal to {API_NAME_MAX_LEN}");
        assert_eq!(
            refusal.as_deref(),
            Some(expected(&requirement).as_str()),
            "case \"a name one past the maximum\": expected the crossed bound, got {refusal:?}"
        );

        // An empty name crosses the minimum AND satisfies no `[…]+` expression, so the
        // report carries both clauses, in the order the walk checks them.
        let refusal = api_name_refusal("");
        let message = refusal.clone().unwrap_or_default();
        assert!(
            message.starts_with("2 validation errors detected: ")
                && message.contains(&format!(
                    "Member must have length greater than or equal to {API_NAME_MIN_LEN}"
                ))
                && message.contains("Member must satisfy regular expression pattern: "),
            "case \"an empty name\": expected both clauses the walk finds, got {refusal:?}"
        );
    }

    #[test]
    fn no_clause_of_this_member_quotes_the_submitted_value() {
        for name in ["bad@name", "name#1", &"E".repeat(API_NAME_MAX_LEN + 1), ""] {
            let message = api_name_refusal(name).unwrap_or_default();
            // Neither half the framework writes for a value: the quoted submission, and
            // the `null` it states for a member that was not there at all.
            assert!(
                !message.contains(&format!("Value '{name}'")) && !message.contains("Value null"),
                "case {name:?}: expected no value quoted back, got {message:?}"
            );
        }
    }
}
