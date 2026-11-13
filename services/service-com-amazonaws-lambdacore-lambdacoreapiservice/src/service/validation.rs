// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-constraint refusal: how the structured
//! [`Violation`]s the framework's boundary walk produces become the `message` the
//! `400 ValidationException` carries.
//!
//! [`crate::service`] owns the SEAMS (which requests are stated, and how the violations and
//! the submitted values are collected) and this module owns the sentence, so the clause
//! rules live in ONE place rather than in each operation that trips them. WHAT is enforced
//! is always the model's: every violation handed to [`refusal`] comes from
//! `aws_smithy_schema_server::constraints`, which reads the `@length`, `@range`,
//! `@pattern`, `@enum` and `@uniqueItems` traits codegen captured off this service's Smithy
//! model. Only the WORDS are chosen here, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report happened to cite:
//!
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `, in the order the walk found them
//!    ([`envelope`]);
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the requirement
//!    it failed, in the framework's own words ([`framework_requirement`], read off
//!    [`Violation::message`] rather than re-spelled, so a class the framework words
//!    differently — or one it gains — cannot drift out of step with it);
//! 3. a reported `@pattern` is the expression WITHOUT the `^` / `$` anchors a model writes
//!    to demand a whole-string match (`service_common::validation::unanchored`): a caller
//!    that lifts the advertised expression out of the message and re-applies it locally
//!    must get the character grammar the service publishes, not the model's way of making
//!    the match total. The CHECK still runs the model's pattern as written;
//! 4. a constraint broken by an ELEMENT of a list is stated at the LIST MEMBER — no element
//!    index and no `member` step — with the element's requirement nested inside
//!    `Member must satisfy constraint: [ … ]` and the value the list member HELD as the
//!    echo. The nested set states every requirement that element had to satisfy: the
//!    constraint it broke, and — because none of this model's lists is `@sparse`, so no
//!    element of one may be null — that a member must not be null. A constraint broken by
//!    the list ITSELF (its own `@length`, its `@uniqueItems`) is not nested, and neither is
//!    one broken by a member INSIDE a structure element, which keeps its own full path;
//! 5. an enum requirement states the value set the SERVICE publishes for that enum, in the
//!    order it publishes it ([`PUBLISHED_VALUE_SETS`]), which is not the order the on-disk
//!    model happens to declare its members in and, for an enum the service has since
//!    widened, is not the same set either. The CHECK still runs the model's value set as
//!    written, so only the advertised list is the service's.
//!
//! Rule 4 needs two things no [`Violation`] carries — the collection a failing element
//! belongs to, and the whole value that collection held — and both come from
//! `service_common::validation`: [`list_of_element`] owns the path spelling and
//! `submitted_lists` owns the rendering, read from the same built input the walk validated.
//! When the request's lists are not in hand (a refusal that reaches `override_refusal`,
//! which is handed the violations alone) the clause falls back to the REFUSED elements of
//! that collection ([`refused_elements`]), which is as much of the member's value as the
//! violations themselves know.
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this `restJson1` service, restated rather
//! than changed: a chosen [`Refused`] replaces the code and the message together.

use std::collections::{HashMap, HashSet};

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use service_common::validation::{list_of_element, unanchored};
use smithy_server_runtime::{Refusal, Refused};

/// The error code a constraint failure is named by, unchanged from the one the framework's
/// refusal channel picks for this `restJson1` service. Restated here because a chosen
/// [`Refused`] carries both halves: a message without its matching code is worse than
/// leaving both alone.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text separating a clause's subject from its requirement, in the framework's own
/// wording ([`Violation::message`]). Split on, never written — see
/// [`framework_requirement`].
const CLAUSE_MARKER: &str = "failed to satisfy constraint: ";

/// What a clause states around the requirement of a collection's ELEMENT (rule 4).
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// What separates the requirements inside one element's nested set (rule 4).
const ELEMENT_SET_JOIN: &str = ", ";

/// The value set this service PUBLISHES for one of its enums, paired with the set the
/// on-disk model declares for it (rule 5).
///
/// A clause advertises the accepted values to the caller, so the list it states is the
/// service's own — both its membership and its order. Where the model's declaration says
/// something else, the published list is recorded here, keyed by the model's set so EVERY
/// member targeting that enum states one list, in create, in update and in a query
/// parameter alike; an enum absent from here is stated exactly as the model declares it.
///
/// The keys are the model's sets
/// (`smithy_query(query="shape",shape="ComputeResourceType,NetworkConnectorState")`), the
/// values the lists the service publishes for them. Only the ADVERTISED list is the
/// service's: what a member ACCEPTS stays the model's set as codegen captured it, which is
/// what the framework's constraint walk runs, so a value named only by the published list is
/// still refused here even though the message lists it.
const PUBLISHED_VALUE_SETS: &[(&[&str], &[&str])] = &[
    // `ComputeResourceType`: the model declares two values, while the service accepts and
    // advertises a third.
    (
        &["MicroVm", "WebFunction"],
        &["WebFunction", "MicroVm", "AppSync"],
    ),
    // `NetworkConnectorState`: the same six values, in the service's order rather than the
    // model's lifecycle-ordered declaration.
    (
        &["PENDING", "ACTIVE", "INACTIVE", "FAILED", "DELETING", "DELETE_FAILED"],
        &["ACTIVE", "DELETING", "INACTIVE", "DELETE_FAILED", "PENDING", "FAILED"],
    ),
];

/// The deepest collection nesting one clause walks out of.
///
/// A step out of a collection is spelled `<position>.member`, and the step a MAP entry
/// carries is the key the CALLER chose, so a key spelled `<digits>.member` is
/// indistinguishable from a list index by the path alone: without a ceiling the nesting a
/// request could ask for would grow with the length of such a key. Every list this model
/// declares (`NetworkConnectorSubnetIds`, `NetworkConnectorSecurityGroupIds`,
/// `AssociatedComputeResourceTypesList`) holds SCALARS, so one step is all this service's
/// own inputs can produce; the ceiling is one past that, enough to state a list of lists
/// were the model to grow one.
const MAX_DEPTH: usize = 2;

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) or a violation class this module has no wording for
/// ([`ViolationKind`] is `#[non_exhaustive]`), which keeps the framework's own message.
///
/// `lists` yields every list of scalar elements the request carried, keyed by the member
/// path the walk reports that list at and already rendered
/// (`service_common::validation::submitted_lists`). It is a closure because it is only read
/// for rule 4 — a violation found on a list ELEMENT, whose clause states the whole list —
/// so a refusal that has none never walks the request again.
pub(crate) fn refusal(
    violations: &[Violation],
    lists: impl FnOnce() -> HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let lists = if violations
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        lists()
    } else {
        HashMap::new()
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: message(violations, &lists)?,
    })
}

/// How this service states `refusal` when only the violations are in hand, or `None` to
/// keep the framework's own wording.
///
/// Only an input-constraint refusal is reworded: an authentication refusal is a different
/// failure with a different status and code, and nothing about it is this service's to
/// word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => self::refusal(violations, HashMap::new),
        _ => None,
    }
}

/// The `message` for a set of violations: the count, then one clause per failing
/// (member, requirement) in the order the walk found them.
///
/// A violation found on a list element is stated at the LIST member with its requirement
/// nested, which is why two bad elements of one list are ONE clause rather than two, and
/// why a member that broke two different rules still states both.
fn message(violations: &[Violation], lists: &HashMap<String, String>) -> Option<String> {
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        // A class with no wording here would otherwise be stated as a clause with no
        // requirement at all; the framework's whole message stands instead.
        let mut requirement = requirement_of(&violation.kind)?;
        for _ in 0..depth {
            requirement = nested_requirement(&requirement)?;
        }
        if !stated.insert((path.clone(), requirement.clone())) {
            continue;
        }
        let held = (depth > 0)
            .then(|| collection_value(violations, &path, lists))
            .flatten();
        clauses.push(format!(
            "{} at '{path}' failed to satisfy constraint: {requirement}",
            subject(violation, held.as_deref())
        ));
    }
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
///
/// The wording is `aws_smithy_schema_server::envelope`'s, which cannot be CALLED here: it
/// renders each clause from a [`Violation`], and rule 4's nested requirement is not a
/// [`ViolationKind`] any violation can carry. The two must stay identical, which
/// [`tests::the_envelope_counts_and_joins_clauses_the_way_the_framework_does`] pins against
/// that function itself.
fn envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The member path a clause names, and how many collection steps it walked out of to get
/// there (rule 4): `subnetIds.1.member` is stated at `subnetIds`, one step out.
///
/// Bounded by [`MAX_DEPTH`], so a path that keeps looking like an element step cannot nest
/// a requirement without end.
fn collapsed(path: &str) -> (String, usize) {
    let mut at = path;
    let mut depth = 0;
    while depth < MAX_DEPTH {
        match list_of_element(at) {
            Some(collection) => {
                at = collection;
                depth += 1;
            }
            None => break,
        }
    }
    (at.to_string(), depth)
}

/// The rule half of a clause: what the member had to satisfy, with the one rendering this
/// service does not share with the framework applied first (rule 3).
fn requirement_of(kind: &ViolationKind) -> Option<String> {
    framework_requirement(&as_reported(kind))
}

/// A violated constraint as this service reports it: a `@pattern` without its whole-value
/// anchors (rule 3), an enum's value set as the service publishes it (rule 5), every other
/// class exactly as the framework describes it.
fn as_reported(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern)),
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(published(values)),
        other => other.clone(),
    }
}

/// The value set a clause advertises for the enum whose modelled set is `declared`
/// (rule 5): the list this service publishes for it, or `declared` itself for an enum whose
/// published list is the model's.
///
/// Matched on the set rather than on its order, so a model that reorders an enum's members
/// still resolves to the same published list.
fn published(declared: &[String]) -> Vec<String> {
    let mut sorted: Vec<&str> = declared.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    for (modelled, service) in PUBLISHED_VALUE_SETS {
        let mut key: Vec<&str> = modelled.to_vec();
        key.sort_unstable();
        if key == sorted {
            return service.iter().map(|value| (*value).to_string()).collect();
        }
    }
    declared.to_vec()
}

/// `requirement` as it reads when what broke it was an ELEMENT of a collection (rule 4):
/// nested inside the clause that names the collection member, beside the requirement every
/// element of a non-`@sparse` collection carries — that it is not null, in the framework's
/// own wording for that requirement rather than a second spelling of it.
///
/// `None` when the framework has no wording for a null member, which leaves the framework's
/// whole message standing, as [`framework_requirement`] does.
fn nested_requirement(requirement: &str) -> Option<String> {
    let not_null = framework_requirement(&ViolationKind::MissingRequiredMember)?;
    Some(format!(
        "{ELEMENT_SET_OPEN}{requirement}{ELEMENT_SET_JOIN}{not_null}]"
    ))
}

/// The framework's own sentence for `kind`, recovered from a violation with NO value and an
/// EMPTY path, so that neither a rejected value nor a caller-chosen map key in a member
/// path can reach the requirement half.
///
/// `None` when the marker is absent, which leaves the framework's whole message standing.
fn framework_requirement(kind: &ViolationKind) -> Option<String> {
    let stated = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
        sensitive: false,
    }
    .message();
    stated
        .rsplit_once(CLAUSE_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// The value half of a clause.
///
/// * an ABSENT member has no value at all and is reported as the null it was;
/// * a clause stated at a COLLECTION because one of its elements failed states what that
///   member held (`held`);
/// * everything else echoes the value carried on the violation.
fn subject(violation: &Violation, held: Option<&str>) -> String {
    match (&violation.value, held) {
        (None, _) => "Value null".to_string(),
        (Some(_), Some(rendered)) => format!("Value '{rendered}'"),
        (Some(shown), None) => format!("Value '{shown}'"),
    }
}

/// What the collection at `path` held, as a clause states it.
///
/// The request's own list when it is in hand (`service_common::validation::submitted_lists`
/// renders every element the caller sent); otherwise the elements of that collection the
/// walk REFUSED, which is as much of the member's value as the violations know. The
/// fallback understates a list of which only some elements were refused — closing that is
/// the seam that hands a refusal the built input, not a second rendering here.
fn collection_value(
    violations: &[Violation],
    path: &str,
    lists: &HashMap<String, String>,
) -> Option<String> {
    if let Some(rendered) = lists.get(path) {
        return Some(rendered.clone());
    }
    refused_elements(violations, path)
}

/// The values of every element of the collection at `path` the walk refused, rendered as a
/// list renders — the elements in walk order, comma-separated, in brackets.
fn refused_elements(violations: &[Violation], path: &str) -> Option<String> {
    let mut elements: Vec<&str> = Vec::new();
    for violation in violations {
        let (collection, depth) = collapsed(&violation.path);
        if depth == 0 || collection != path {
            continue;
        }
        if let Some(value) = violation.value.as_deref() {
            if !elements.contains(&value) {
                elements.push(value);
            }
        }
    }
    (!elements.is_empty()).then(|| format!("[{}]", elements.join(", ")))
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of violations rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/validation_message_wording.rs`.

    use super::*;

    /// A violation of `kind` at `path` holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// RULE 1: the count and the join are the framework's, for every clause count a refusal
    /// can carry — pinned against the framework's own renderer so the two cannot drift.
    #[test]
    fn the_envelope_counts_and_joins_clauses_the_way_the_framework_does() {
        for count in 0_usize..4 {
            let violations: Vec<Violation> = (0..count)
                .map(|index| {
                    violation(
                        &format!("member{index}"),
                        ViolationKind::MaxLength(8),
                        Some("too long a value"),
                    )
                })
                .collect();
            let framework = aws_smithy_schema_server::envelope(&violations);
            let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
            let mine = envelope(&clauses);
            assert_eq!(
                mine, framework,
                "case {count} clause(s): expected the framework's envelope {framework:?}, \
                 got {mine:?}"
            );
        }
    }

    /// RULE 4: an element's failure is stated at its collection, nested, whatever the
    /// constraint class — and a failure that is NOT on an element keeps its own path.
    #[test]
    fn an_elements_failure_is_stated_at_its_collection() {
        let subnet_pattern = r"^subnet-[0-9a-z]*$";
        let rows = [
            (
                "a list element outside its pattern",
                violation(
                    "configuration.vpcEgressConfiguration.subnetIds.1.member",
                    ViolationKind::Pattern(subnet_pattern.to_string()),
                    Some("not-a-subnet"),
                ),
                "1 validation error detected: Value '[not-a-subnet]' at \
                 'configuration.vpcEgressConfiguration.subnetIds' failed to satisfy \
                 constraint: Member must satisfy constraint: [Member must satisfy regular \
                 expression pattern: subnet-[0-9a-z]*, Member must not be null]",
            ),
            (
                "a list element outside its enum value set",
                violation(
                    "configuration.vpcEgressConfiguration.associatedComputeResourceTypes.1.member",
                    ViolationKind::EnumValue(vec![
                        "MicroVm".to_string(),
                        "WebFunction".to_string(),
                    ]),
                    Some("Sim2RealProbe"),
                ),
                "1 validation error detected: Value '[Sim2RealProbe]' at \
                 'configuration.vpcEgressConfiguration.associatedComputeResourceTypes' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy enum value set: [WebFunction, MicroVm, AppSync], Member must not \
                 be null]",
            ),
            (
                // The LIST's own constraint, not an element's: stated at the same path, and
                // NOT nested.
                "the list's own length",
                violation(
                    "configuration.vpcEgressConfiguration.subnetIds",
                    ViolationKind::MaxLength(16),
                    Some("17 items"),
                ),
                "1 validation error detected: Value '17 items' at \
                 'configuration.vpcEgressConfiguration.subnetIds' failed to satisfy \
                 constraint: Member must have length less than or equal to 16",
            ),
            (
                // A member INSIDE a structure element keeps its own full path.
                "a member of a structure element",
                violation(
                    "things.1.member.name",
                    ViolationKind::MinLength(1),
                    Some(""),
                ),
                "1 validation error detected: Value '' at 'things.1.member.name' failed to \
                 satisfy constraint: Member must have length greater than or equal to 1",
            ),
            (
                // A map entry's step carries the caller's key, not an index: not an element.
                "a map entry value",
                violation(
                    "tags.someKey.member",
                    ViolationKind::MaxLength(256),
                    Some("over long"),
                ),
                "1 validation error detected: Value 'over long' at 'tags.someKey.member' \
                 failed to satisfy constraint: Member must have length less than or equal \
                 to 256",
            ),
            (
                // An absent member is reported as the null it was, with no value echoed.
                "an absent required member",
                violation("name", ViolationKind::MissingRequiredMember, None),
                "1 validation error detected: Value null at 'name' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ];

        for (case, violation, expected) in rows {
            let stated = refusal(std::slice::from_ref(&violation), HashMap::new)
                .unwrap_or_else(|| panic!("case {case:?}: a constraint refusal is stated"));
            assert_eq!(
                stated.code, VALIDATION_EXCEPTION,
                "case {case:?}: expected the code {VALIDATION_EXCEPTION}, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE 4, the value: the clause states the list the caller SENT when the request's
    /// lists are in hand, and the refused elements when they are not.
    #[test]
    fn a_collection_clause_states_the_whole_list_when_it_is_in_hand() {
        let violations = vec![violation(
            "configuration.vpcEgressConfiguration.subnetIds.2.member",
            ViolationKind::Pattern(r"^subnet-[0-9a-z]*$".to_string()),
            Some("nope"),
        )];
        let lists = || {
            [(
                "configuration.vpcEgressConfiguration.subnetIds".to_string(),
                "[subnet-0123456789abcdef0, nope, subnet-0fedcba9876543210]".to_string(),
            )]
            .into_iter()
            .collect()
        };
        let with_lists = refusal(&violations, lists).expect("a constraint refusal is stated");
        assert!(
            with_lists
                .message
                .contains("Value '[subnet-0123456789abcdef0, nope, subnet-0fedcba9876543210]'"),
            "case the request's lists in hand: the clause states the whole list, got {:?}",
            with_lists.message
        );
        let without = refusal(&violations, HashMap::new).expect("a constraint refusal is stated");
        assert!(
            without.message.contains("Value '[nope]'"),
            "case the violations alone: the clause states the refused elements, got {:?}",
            without.message
        );
    }

    /// RULE 5: an enum clause advertises the value set the SERVICE publishes, in the order
    /// it publishes it, whatever order the model declares and whether or not the model's set
    /// is the whole of it — and an enum with no published list of its own keeps the model's.
    #[test]
    fn an_enum_clause_states_the_published_value_set() {
        // Each row: the set a violation carries (the model's, in any order) and the list the
        // clause has to advertise.
        let rows = [
            (
                "the compute resource types, as the model declares them",
                vec!["MicroVm", "WebFunction"],
                "[WebFunction, MicroVm, AppSync]",
            ),
            (
                "the same set in another order",
                vec!["WebFunction", "MicroVm"],
                "[WebFunction, MicroVm, AppSync]",
            ),
            (
                "the connector states, as the model declares them",
                vec![
                    "PENDING",
                    "ACTIVE",
                    "INACTIVE",
                    "FAILED",
                    "DELETING",
                    "DELETE_FAILED",
                ],
                "[ACTIVE, DELETING, INACTIVE, DELETE_FAILED, PENDING, FAILED]",
            ),
            (
                // An enum this service publishes no list of its own for: the model's.
                "an enum with no published list of its own",
                vec!["IPv4", "DualStack"],
                "[IPv4, DualStack]",
            ),
        ];

        for (case, declared, expected) in rows {
            let values: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let stated = refusal(
                &[violation(
                    "someMember",
                    ViolationKind::EnumValue(values),
                    Some("nope"),
                )],
                HashMap::new,
            )
            .unwrap_or_else(|| panic!("case {case:?}: a constraint refusal is stated"));
            let expected = format!(
                "1 validation error detected: Value 'nope' at 'someMember' failed to satisfy \
                 constraint: Member must satisfy enum value set: {expected}"
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE 4, bounded: a path that keeps looking like an element step nests at most
    /// [`MAX_DEPTH`] times.
    #[test]
    fn nesting_is_bounded() {
        let deep = format!("outer{}", ".1.member".repeat(8));
        let (path, depth) = collapsed(&deep);
        assert_eq!(
            depth, MAX_DEPTH,
            "case {deep:?}: expected at most {MAX_DEPTH} steps out, got {depth}"
        );
        assert!(
            path.starts_with("outer"),
            "case {deep:?}: expected the remaining path to start at the outermost member, \
             got {path:?}"
        );
        let stated = refusal(
            &[violation(&deep, ViolationKind::MinLength(1), Some(""))],
            HashMap::new,
        )
        .expect("a constraint refusal is stated");
        assert_eq!(
            stated.message.matches(ELEMENT_SET_OPEN).count(),
            MAX_DEPTH,
            "case {deep:?}: expected {MAX_DEPTH} nested requirements, got {:?}",
            stated.message
        );
    }

    /// RULE 3 reaches a clause nested inside a collection's as well as a plain one: the
    /// anchors the model writes never reach the wire.
    #[test]
    fn a_pattern_is_stated_without_the_models_anchors() {
        let anchored = r"^subnet-[0-9a-z]*$";
        for (case, path) in [
            ("a scalar member", "operatorRole"),
            (
                "an element of a list",
                "configuration.vpcEgressConfiguration.subnetIds.1.member",
            ),
        ] {
            let stated = refusal(
                &[violation(
                    path,
                    ViolationKind::Pattern(anchored.to_string()),
                    Some("bad"),
                )],
                HashMap::new,
            )
            .expect("a constraint refusal is stated");
            assert!(
                stated.message.contains("pattern: subnet-[0-9a-z]*"),
                "case {case:?}: expected the unanchored expression, got {:?}",
                stated.message
            );
            assert!(
                !stated.message.contains(anchored),
                "case {case:?}: the model's anchored spelling must never reach the wire, \
                 got {:?}",
                stated.message
            );
        }
    }

    /// A refusal this module has no wording for keeps the framework's whole message, and an
    /// authentication refusal is not this service's to word.
    #[test]
    fn only_a_constraint_refusal_is_reworded() {
        assert!(
            refused(&Refusal::Unauthenticated).is_none(),
            "an authentication refusal keeps the framework's wording"
        );
        assert!(
            refused(&Refusal::Invalid(Vec::new())).is_none(),
            "a refusal with no violations states nothing"
        );
    }
}
