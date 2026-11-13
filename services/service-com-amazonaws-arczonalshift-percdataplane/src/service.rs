// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-arczonalshift-percdataplane` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   constraint is stated the way ARC states it: the framework's own envelope, clause and
//!   requirement, with the clause's subject decided by where in the input shape the
//!   refused member sits, a `@pattern` reported without the anchors a model writes to
//!   demand a whole-string match, and an enumerated member's admissible set stated as the
//!   set the service advertises. See "How this service words a refusal" below.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! # How this service words a refusal
//!
//! The envelope, the clause and the requirement are the framework's — `N validation
//! error(s) detected: <subject> at '<member path>' failed to satisfy constraint:
//! <requirement>`, rendered by
//! [`aws_smithy_schema_server::constraints::Violation::message`] — so a constraint class
//! the framework gains later cannot arrive with a sentence of this service's own.
//!
//! Three parts of the clause are ARC's, and each is a rule rather than one recorded string:
//!
//! * the SUBJECT is the member's POSITION ALONE for a member of the operation's own input
//!   shape, and the value the request carried for a member reached through another shape
//!   ([`quotes_the_value`]); an absent `@required` member has no value at all and keeps
//!   the framework's `Value null at '<member>'`;
//!
//! * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the
//!   `^`/`$` anchors the model writes to make the match total ([`unanchored`]): that
//!   anchoring is how Smithy turns a find into a whole-string match, not part of the
//!   expression a client shows a user or matches on;
//! * an enumerated member's admissible set is the set the SERVICE names, which the
//!   public model reproduces for all but two of this service's enums
//!   ([`NAMED_BY_THE_SERVICE`]): ARC names the zonal-shift statuses in an order of its
//!   own, and its alarm-source enumeration carries a source the public model does not
//!   publish, and the refusal states each as the service has it — so a client that reads
//!   the admissible values out of the message sees what the service accepts there rather
//!   than a narrower or differently ordered set.
//!
//! All three are properties of a SHAPE — the shape a member sits in, or the shape it
//! targets — so they hold for every member that reaches it and every operation that takes
//! one, and none of them depends on the request. Every other enum's set is the
//! framework's: the model's values in the model's declaration order.
//!
//! The `@pattern` rule above is the SAME rule ~24 other service crates in this tree carry
//! inlined (`grep -rln 'fn unanchored' services/*/src`), and
//! `services/common/src/validation.rs` ("Still to migrate here") is where it belongs.
//! Migrating all of them is one change of its own rather than a side effect of this
//! service's issue, so it is inlined here with this note, exactly as that module asks.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.arczonalshift#ValidationException`, which every operation of this
/// service declares and whose `@httpError` is 400.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// Only a `^…$` PAIR bracketing the whole expression is dropped: `^` alone, or `$`
/// alone, is an expression anchored at one end — a different language, not an anchored
/// spelling of this one — and an anchor left INSIDE after the pair is removed means the
/// pair bracketed one branch of an alternation (`^a$|^b$`) rather than the whole
/// expression, so such a pattern is reported exactly as the model spells it. A `$`
/// escaped by a backslash is a literal dollar sign, not the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(body) = without_end_anchor(body) else {
        return pattern;
    };
    if body.contains('^') || body.contains('$') {
        return pattern;
    }
    body
}

/// `pattern` without its end anchor, or `None` when it carries none. A trailing `$`
/// preceded by an ODD number of backslashes is escaped — a literal dollar sign inside
/// the expression — so it is not an anchor.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('$')?;
    let escapes = body.len() - body.trim_end_matches('\\').len();
    (escapes % 2 == 0).then_some(body)
}

/// The two enumerations ARC NAMES differently from the enum its public model publishes,
/// as (what the model declares, what the service names) pairs.
///
/// The declared set is what identifies the shape here: the walk hands this seam an enum
/// violation's value SET, not the shape that declared it, and no other enum of this
/// service declares either set below
/// (`smithy_query(query="shape", shape="ZonalShiftStatus,AutoshiftExecutionStatus,\
/// ZonalAutoshiftStatus,AutoshiftObserverNotificationStatus,ControlConditionType,\
/// AppliedStatus,AutoshiftAppliedStatus,ShiftType,PracticeRunOutcome,\
/// ConflictExceptionReason,ValidationExceptionReason")`). Each pair is a property of the
/// SERVICE's own enumeration rather than of one request, so it is the same for every
/// member that reaches the shape and for every operation that takes one:
///
/// * the zonal-shift status — ARC names it expired-first, not in the order the model
///   declares, and a refusal about any member of that shape (`ListZonalShifts.status`
///   and the status of a shift a read reports) names it that way;
/// * the practice-run control-condition type — ARC's enumeration of alarm sources
///   carries one the public model does not publish, and a refusal about a control
///   condition's `type` (the outcome alarms and the blocking alarms of either
///   practice-run configuration operation, both reaching `ControlCondition.type`) names
///   the whole of it, so a client reading the admissible values out of the message is not
///   told that source is inadmissible.
const NAMED_BY_THE_SERVICE: [(&[&str], &[&str]); 2] = [
    (
        &["ACTIVE", "EXPIRED", "CANCELED"],
        &["EXPIRED", "ACTIVE", "CANCELED"],
    ),
    (&["CLOUDWATCH"], &["CARNAVAL", "CLOUDWATCH"]),
];

/// The admissible set a refusal states for an enumerated member whose model-declared set
/// is `declared`: the set ARC names for it, which for every enum but the two in
/// [`NAMED_BY_THE_SERVICE`] is the model's own set in its declaration order.
fn advertised(declared: &[String]) -> Vec<String> {
    for (model_names, service_names) in NAMED_BY_THE_SERVICE {
        if declared
            .iter()
            .map(String::as_str)
            .eq(model_names.iter().copied())
        {
            return service_names.iter().map(|value| value.to_string()).collect();
        }
    }
    declared.to_vec()
}

/// Whether a clause about `member_path` states the value the request carried there.
///
/// ARC states two subjects, and which one it states is decided by WHERE in the input the
/// refused member sits — a property of the input shape, so it is the same for every
/// request that reaches that member and every operation that takes one:
///
/// * a member of the operation's own input shape — bound to a URI label, to a query
///   parameter, or read from the top level of the body — is stated by POSITION ALONE,
///   `Value at '<member>'`, and the message carries nothing of what the caller sent
///   (`zonalShiftId` and `expiresIn` against their `@pattern`, `status` against an enum's
///   value set, `maxResults` against its `@range`);
/// * a member reached THROUGH ANOTHER SHAPE — an element of a collection, or a member of a
///   structure inside the input — is stated with that value quoted back,
///   `Value '<submitted>' at '<path>'`, which is what names the one element among several
///   that was refused (a control condition's `type` inside either practice-run
///   configuration operation's alarm lists).
///
/// The dotted path the constraint walk reports is what separates the two: a member of the
/// input shape is a single segment, and a member reached through a collection or a nested
/// structure names every step taken to it (`outcomeAlarms.<index>.member.type`).
fn quotes_the_value(member_path: &str) -> bool {
    member_path.contains('.')
}

/// The same violation with its subject decided by the refused member's position, a
/// `@pattern` reported unanchored and an enum's admissible set stated as the set the
/// service names; every other class is untouched, so the requirement each one words stays
/// the framework's.
fn as_stated(violation: &Violation) -> Violation {
    let kind = match &violation.kind {
        ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern).to_string()),
        ViolationKind::EnumValue(declared) => ViolationKind::EnumValue(advertised(declared)),
        other => other.clone(),
    };
    Violation {
        path: violation.path.clone(),
        kind,
        value: violation.value.clone(),
        // `Value at '<member>'` — the subject that names the position and nothing the
        // member held — is the form the framework renders for a value it is told not to
        // echo, so a violation ARC states by position alone is restated as one. A member
        // the request OMITTED carries no value at all and keeps `Value null at …`, which
        // is the absence rather than a withheld value.
        sensitive: violation.sensitive
            || (violation.value.is_some() && !quotes_the_value(&violation.path)),
    }
}

/// One violation's clause as ARC states it: the framework's own clause — which states the
/// position alone or quotes the submitted value back between `Value ` and ` at ` as
/// [`quotes_the_value`] decides, and states `Value null` for a member the request left out
/// — over the violation this service restates ([`as_stated`]).
fn clause(violation: &Violation) -> String {
    as_stated(violation).message()
}

/// `violations` as this service states them, or `None` when there is nothing to state.
///
/// The envelope is the framework's own
/// ([`aws_smithy_schema_server::constraints::envelope`]): the violation count, then the
/// clauses joined by `"; "`. It is rebuilt here rather than post-processed because the
/// CLAUSES are this service's, and because rewriting a whole rendered envelope once per
/// violation would cost the product of the two — both of which a request controls
/// through the number of members it puts outside their constraints. Each clause is
/// touched exactly once, so the wording is linear in what the walk collected (which the
/// framework caps at 100 violations and 256 echoed characters each).
fn stated(violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(format!("1 validation error detected: {}", clause(only))),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.iter().map(clause).collect::<Vec<_>>().join("; ")
        )),
    }
}

/// The `com-amazonaws-arczonalshift-percdataplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. ARC states such a refusal in the framework's own envelope and clause
        // — with the clause's subject decided by the refused member's position, a
        // `@pattern` reported without its anchors and an enum's admissible set stated as
        // the set the service names; see "How this service words a refusal" in the module
        // docs. The code
        // stays `ValidationException`, which is both what the framework would answer and
        // the shape this service's model declares (`@httpError` 400), because a client
        // branches on the code and `Refused` replaces the code and the message together.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: stated(violations)?,
            }),
            // Authentication is the framework's to word: the request never reached an
            // operation, so there is nothing of this service's own to say about it.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{advertised, clause, quotes_the_value, stated, unanchored};
    use smithy_server_runtime::{Violation, ViolationKind};

    /// The anchoring a model spells a `@pattern` with is dropped only when the `^…$`
    /// pair brackets the WHOLE expression; every other form is the expression itself and
    /// is reported as the model writes it. The two anchored rows are this service's own
    /// patterns (`ZonalShiftId`, `ExpiresIn`); the rest are the forms the rule has to
    /// leave alone, which this service's model does not happen to contain.
    #[test]
    fn a_pattern_loses_only_a_whole_expression_anchor_pair() {
        let cases = [
            ("an anchored character class", "^[A-Za-z0-9-]+$", "[A-Za-z0-9-]+"),
            (
                "an anchored group sequence",
                "^([1-9][0-9]*)(m|h)$",
                "([1-9][0-9]*)(m|h)",
            ),
            ("anchored at the start only", "^arn:", "^arn:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("an escaped trailing dollar", r"^cost\$", r"^cost\$"),
            (
                "an escaped dollar before the anchor",
                r"^cost\\$",
                r"cost\\",
            ),
            ("the empty expression", "", ""),
        ];
        for (case, modelled, expected) in cases {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A violation about a member of the input shape states its position alone; one about
    /// a member reached through a collection quotes that element's value; one about a
    /// member the request omitted keeps the framework's `Value null at …`. The count and
    /// the joining are the framework's.
    #[test]
    fn a_clause_states_the_value_only_where_the_member_is_reached_through_a_shape() {
        let outside_pattern = Violation {
            path: "zonalShiftId".to_string(),
            kind: ViolationKind::Pattern("^[A-Za-z0-9-]+$".to_string()),
            value: Some("bad_id_1".to_string()),
            sensitive: false,
        };
        let inside_a_collection = Violation {
            path: "outcomeAlarms.1.member.type".to_string(),
            kind: ViolationKind::EnumValue(vec!["CLOUDWATCH".to_string()]),
            value: Some("BOGUS_TYPE".to_string()),
            sensitive: false,
        };
        let absent = Violation {
            path: "comment".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        assert_eq!(
            clause(&outside_pattern),
            "Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: [A-Za-z0-9-]+",
            "a member of the input shape is stated by position alone, with the pattern \
             unanchored"
        );
        assert_eq!(
            clause(&inside_a_collection),
            "Value 'BOGUS_TYPE' at 'outcomeAlarms.1.member.type' failed to satisfy \
             constraint: Member must satisfy enum value set: [CARNAVAL, CLOUDWATCH]",
            "an element's value names which element was refused"
        );
        assert_eq!(
            clause(&absent),
            "Value null at 'comment' failed to satisfy constraint: Member must not be null",
            "an absent member carried no value at all"
        );
        assert_eq!(stated(&[]), None, "nothing to state");
        assert_eq!(
            stated(std::slice::from_ref(&outside_pattern)).as_deref(),
            Some(
                "1 validation error detected: Value at 'zonalShiftId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+"
            ),
            "one violation takes the singular envelope"
        );
        assert_eq!(
            stated(&[outside_pattern, absent]).as_deref(),
            Some(
                "2 validation errors detected: Value at 'zonalShiftId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 [A-Za-z0-9-]+; Value null at 'comment' failed to satisfy constraint: Member \
                 must not be null"
            ),
            "several violations are counted and joined"
        );
    }

    /// The subject a clause states is decided by the refused member's POSITION in the
    /// input, over every form of path this service's walk reports: the members of each
    /// operation's input shape, bound to a URI label, a query parameter or the body, and
    /// the `type` of a control condition inside either practice-run configuration
    /// operation's alarm lists.
    #[test]
    fn the_subject_follows_the_refused_members_position() {
        let cases: [(&str, &str, bool); 8] = [
            ("a URI label", "zonalShiftId", false),
            ("a query parameter", "status", false),
            ("a paging query parameter", "maxResults", false),
            ("a top-level body member", "expiresIn", false),
            ("another top-level body member", "resourceIdentifier", false),
            (
                "a member of a collection element",
                "outcomeAlarms.1.member.type",
                true,
            ),
            (
                "a member of a later collection element",
                "blockingAlarms.10.member.alarmIdentifier",
                true,
            ),
            ("the collection member itself", "outcomeAlarms", false),
        ];
        for (case, path, expected) in cases {
            let actual = quotes_the_value(path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({path}): expected quotes_the_value {expected}, got {actual}"
            );
        }
    }

    /// Every enum this service's model declares, as a refusal states its set: the model's
    /// values in the model's declaration order, except the zonal-shift status, which ARC
    /// names expired-first, and the control-condition type, whose set also names the alarm
    /// source the model does not publish. The declared rows are quoted from the model
    /// (`smithy_query(query="shape", shape="ZonalShiftStatus,AutoshiftExecutionStatus,\
    /// ZonalAutoshiftStatus,AutoshiftObserverNotificationStatus,ControlConditionType,\
    /// AppliedStatus,ShiftType,PracticeRunOutcome")`), and cover enums no refusal recorded
    /// from this service has named.
    #[test]
    fn an_enum_set_is_the_models_except_the_two_the_service_names_itself() {
        let cases: [(&str, &[&str], &[&str]); 8] = [
            (
                "ZonalShiftStatus",
                &["ACTIVE", "EXPIRED", "CANCELED"],
                &["EXPIRED", "ACTIVE", "CANCELED"],
            ),
            (
                "ControlConditionType",
                &["CLOUDWATCH"],
                &["CARNAVAL", "CLOUDWATCH"],
            ),
            (
                "AutoshiftExecutionStatus",
                &["ACTIVE", "COMPLETED"],
                &["ACTIVE", "COMPLETED"],
            ),
            (
                "ZonalAutoshiftStatus",
                &["ENABLED", "DISABLED"],
                &["ENABLED", "DISABLED"],
            ),
            (
                "AutoshiftObserverNotificationStatus",
                &["ENABLED", "DISABLED"],
                &["ENABLED", "DISABLED"],
            ),
            (
                "AppliedStatus",
                &["APPLIED", "NOT_APPLIED"],
                &["APPLIED", "NOT_APPLIED"],
            ),
            (
                "ShiftType",
                &[
                    "ZONAL_SHIFT",
                    "PRACTICE_RUN",
                    "FIS_EXPERIMENT",
                    "ZONAL_AUTOSHIFT",
                ],
                &[
                    "ZONAL_SHIFT",
                    "PRACTICE_RUN",
                    "FIS_EXPERIMENT",
                    "ZONAL_AUTOSHIFT",
                ],
            ),
            (
                "PracticeRunOutcome",
                &[
                    "FAILED",
                    "INTERRUPTED",
                    "PENDING",
                    "SUCCEEDED",
                    "CAPACITY_CHECK_FAILED",
                ],
                &[
                    "FAILED",
                    "INTERRUPTED",
                    "PENDING",
                    "SUCCEEDED",
                    "CAPACITY_CHECK_FAILED",
                ],
            ),
        ];
        for (case, declared, expected) in cases {
            let declared: Vec<String> = declared.iter().map(|v| v.to_string()).collect();
            let actual = advertised(&declared);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
