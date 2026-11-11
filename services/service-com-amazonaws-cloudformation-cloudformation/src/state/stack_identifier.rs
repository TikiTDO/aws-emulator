// SPDX-License-Identifier: CC0-1.0

//! The constraints CloudFormation holds a stack IDENTIFIER to, applied before the store is
//! read.
//!
//! ## The rule
//!
//! A member that identifies a stack by NAME OR ID carries two constraints — at least one
//! character, and the name-or-ARN expression — and CloudFormation checks them on the INPUT,
//! ahead of resolving the stack: a request whose identifier no stack could ever carry is told
//! that its request is malformed rather than that the stack is missing, and EVERY constraint
//! the value breaks is reported in one envelope, the expression first.
//!
//! The expression is the whole of what CloudFormation admits: a stack NAME, or a stack ARN.
//! The unique id a stack id ENDS in is no identifier of a stack on its own — it is
//! hexadecimal, so it falls outside the name alternative whenever it starts with a digit, and
//! a request carrying one is answered as the malformed request it is rather than as a missing
//! stack, which is what the recorded refusal of a mis-spelled stack id shows. A stack is
//! addressed by its name and by its WHOLE stack id (`arn:…:stack/<name>/<unique-id>`), and a
//! request carrying either of those is RESOLVED, so a value in one of them that names no
//! stack is answered as the missing stack it is ([`super::stack_does_not_exist`] and the forms
//! beside it).
//!
//! ## Why the check lives here and not at the boundary
//!
//! Most of this service's identifier members target a model shape that declares NEITHER
//! constraint (`StackName`: `smithy_query(query="shape", shape="StackName")` carries no
//! `@length` and no `@pattern`), so the framework's constraint walk has nothing to check for
//! them and the operation is where the check belongs. A member whose model shape DOES carry
//! them (`StackNameOrId`) is already refused at the boundary, in the framework's own
//! envelope, and is not re-checked here.
//!
//! The clause, the envelope and the requirement wording are the framework's
//! ([`::aws_smithy_schema_server::envelope`]), so a refusal stated here cannot drift from one
//! the boundary states; what is this service's own is the EXPRESSION its validator states for
//! an identifier and the member path each operation states it at.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

/// The expression CloudFormation's own input validation states for a stack identifier: a
/// stack NAME ("A stack name can contain only alphanumeric characters (case sensitive) and
/// hyphens. It must start with an alphabetical character", which is
/// [`super::STACK_NAME_PATTERN`]) or a stack ARN.
///
/// Quoted as the SERVICE states it, which is not the anchored spelling the model writes for
/// the members that do carry a `@pattern`
/// (`smithy_query(query="shape", shape="StackNameOrId")`): a requirement reaches a message as
/// the layer that refuses states it.
pub const STACK_IDENTIFIER_PATTERN: &str = "[a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*";

/// The least a stack identifier may hold: `@length(min: 1)`, which the model states for the
/// identifier shape that carries its constraints
/// (`smithy_query(query="shape", shape="StackNameOrId")`) and which CloudFormation applies to
/// every member that takes one.
pub const STACK_IDENTIFIER_MIN_LENGTH: usize = 1;

/// The member path a refusal states a stack identifier at for an operation whose validator
/// names that member `StackName` — the lowerCamel spelling every violation path takes.
pub const STACK_NAME_PATH: &str = "stackName";

/// The member path a refusal states a stack identifier at for an operation whose validator
/// names that member `StackNameOrId` instead (`ListStackResources`).
///
/// CloudFormation does not name the member identically in every operation, so the path is
/// the operation's own: it is what tells a caller which parameter of the call it sent was
/// refused.
pub const STACK_NAME_OR_ID_PATH: &str = "stackNameOrId";

/// [`STACK_IDENTIFIER_PATTERN`] as the expression a whole identifier has to BE, compiled
/// once for the process.
///
/// Anchored at both ends before it is applied: the constraint states the shape of the whole
/// value, not of a part of it. `None` only if the expression itself could not be compiled, in
/// which case nothing is refused by it rather than everything.
fn whole_identifier() -> Option<&'static ::regex::Regex> {
    static COMPILED: ::std::sync::OnceLock<Option<::regex::Regex>> =
        ::std::sync::OnceLock::new();
    COMPILED
        .get_or_init(|| ::regex::Regex::new(&format!("^(?:{STACK_IDENTIFIER_PATTERN})$")).ok())
        .as_ref()
}

/// Whether CloudFormation admits `identifier` as naming a stack at all: a value the
/// expression it states for an identifier matches ([`STACK_IDENTIFIER_PATTERN`]) — a stack
/// NAME or a stack ARN, and nothing else. The unique id a stack id ENDS in is hexadecimal and
/// so falls outside the name alternative whenever it starts with a digit, which is why a
/// request carrying one is refused rather than resolved.
fn is_an_addressable_form(identifier: &str) -> bool {
    whole_identifier().is_none_or(|expression| expression.is_match(identifier))
}

/// The message CloudFormation answers for an identifier that breaks its constraints, stated
/// at `member_path`, or `None` for one it admits.
///
/// Both constraints are reported, the expression first — the expression is the shape of the
/// value and the length is a bound on it — so a caller sees everything it has to fix rather
/// than one finding per round trip. The value is quoted through the shared rendering
/// ([`::service_common::validation::quoted_echo`]), so a message is never sized by the
/// request, and the CHECK itself is made against the value as sent.
pub fn malformed_identifier(member_path: &str, identifier: &str) -> Option<String> {
    let clause = |kind: ViolationKind| Violation {
        path: member_path.to_string(),
        kind,
        value: Some(::service_common::validation::quoted_echo(identifier).into_owned()),
        sensitive: false,
    };
    let mut violations: Vec<Violation> = Vec::new();
    if !is_an_addressable_form(identifier) {
        // The expression the clause STATES is the service's own, whichever addressable form
        // admitted the values it does admit: a requirement reaches a message as the layer
        // that refuses states it, and what CloudFormation states for an identifier it will
        // not take is the name-or-ARN expression.
        violations.push(clause(ViolationKind::Pattern(
            STACK_IDENTIFIER_PATTERN.to_string(),
        )));
    }
    if identifier.chars().count() < STACK_IDENTIFIER_MIN_LENGTH {
        violations.push(clause(ViolationKind::MinLength(
            // The requirement is stated from the bound itself, so the sentence and the check
            // cannot name two different numbers.
            STACK_IDENTIFIER_MIN_LENGTH as i64,
        )));
    }
    ::aws_smithy_schema_server::envelope(&violations)
}

/// Refuse the call when the stack identifier a request carries breaks CloudFormation's
/// constraints for it, so a malformed identifier is never answered as a stack that is not
/// there.
///
/// Called at the TOP of a handler, before any lookup: that order is part of the contract —
/// an identifier no stack could carry is a malformed request, and a caller can only tell the
/// two apart if the constraint refusal comes first. `identifier` is `None` for an operation
/// whose member is conditional and the request did not carry it, which constrains nothing.
///
/// Generic over the operation's error enum, so every operation refuses it the same way: HTTP
/// 400 with the wire code `ValidationError`.
pub fn refuse_malformed_identifier<E>(
    member_path: &str,
    identifier: Option<&str>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    match identifier.and_then(|identifier| malformed_identifier(member_path, identifier)) {
        Some(message) => Err(crate::scaffolded::Scaffolded::validation_error(message)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the identifiers CloudFormation admits are the ones the expression it states for
    /// an identifier matches — a stack name and a stack ARN; every other value is refused,
    /// the unique id a stack id ends in included, and a value breaking BOTH constraints is
    /// reported with both clauses, the expression first.
    #[test]
    fn an_identifier_is_admitted_by_the_services_own_expression() {
        for admitted in [
            "a",
            "a-stack",
            "Stack9",
            "arn:aws:cloudformation:us-east-1:123456789012:stack/a/0",
            "arn:aws-cn:cloudformation:cn-north-1:123456789012:stack/a/0",
            // A value in the 8-4-4-4-12 form that happens to START with a letter: the name
            // alternative admits it, since what the expression reads is the characters and
            // their positions rather than what the value was minted as.
            "ae81d159-090a-7b61-9856-04d0999cb465",
        ] {
            assert_eq!(
                malformed_identifier(STACK_NAME_PATH, admitted),
                None,
                "case {admitted:?}: expected the identifier to be admitted"
            );
        }
        for (case, refused) in [
            ("a name with an underscore", "a_stack"),
            ("a name with a dot", "a.stack"),
            ("a name starting with a digit", "9stack"),
            ("a name starting with a hyphen", "-stack"),
            // The unique id a stack id ends in, in either case of hexadecimal: hexadecimal
            // starting with a digit, which the name alternative does not admit.
            ("a stack's unique id", "0e81d159-090a-7b61-9856-04d0999cb465"),
            (
                "a stack's unique id in upper-case hexadecimal",
                "0E81D159-090A-7B61-9856-04D0999CB465",
            ),
            // A value that is nearly a unique id is no identifier either: the groups and
            // their lengths are what the form IS, and neither form is admitted.
            ("a unique id a group short", "0e81d159-090a-7b61-04d0999cb465"),
            (
                "a unique id with a short last group",
                "0e81d159-090a-7b61-9856-04d0999cb46",
            ),
            (
                "a unique id with a group too many",
                "0e81d159-090a-7b61-9856-04d0999cb465-9856",
            ),
            (
                "a unique id with a non-hexadecimal digit",
                "0g81d159-090a-7b61-9856-04d0999cb465",
            ),
            (
                "a unique id with no groups at all",
                "0e81d159090a7b61985604d0999cb465",
            ),
        ] {
            let expected = Some(format!(
                "1 validation error detected: Value '{refused}' at 'stackName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {STACK_IDENTIFIER_PATTERN}"
            ));
            assert_eq!(
                malformed_identifier(STACK_NAME_PATH, refused),
                expected,
                "case {case:?}: expected the expression clause alone"
            );
        }
        let expected = Some(format!(
            "2 validation errors detected: Value '' at 'stackNameOrId' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             {STACK_IDENTIFIER_PATTERN}; Value '' at 'stackNameOrId' failed to satisfy \
             constraint: Member must have length greater than or equal to \
             {STACK_IDENTIFIER_MIN_LENGTH}"
        ));
        assert_eq!(
            malformed_identifier(STACK_NAME_OR_ID_PATH, ""),
            expected,
            "case \"an empty identifier\": expected both clauses, the expression first"
        );
    }
}
