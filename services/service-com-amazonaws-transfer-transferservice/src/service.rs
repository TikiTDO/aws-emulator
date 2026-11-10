// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-transfer-transferservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! Transfer's `ValidationException` message is a TEMPLATE over the request's
//! structure, never over its values: every clause names the member path and the
//! modelled constraint the member failed, and no clause quotes the value that failed
//! it (a `@pattern` is quoted as the model's expression with the anchors a Smithy
//! pattern writes around it removed). [`constraint_envelope`] is the one place that
//! sentence is built.
//!
//! The one part of a clause that is neither the request's nor the model's is the ORDER an
//! enum clause lists the enum's admitted values in: the membership is the model's, the
//! order is this service's own listing of that enum ([`VALUE_LISTINGS`], [`listed`]).
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
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with.
///
/// Transfer's model declares no `ValidationException` of its own, so a refusal the
/// framework makes before any handler runs renders the framework's synthetic one —
/// which is also the code the real service answers these refusals with. It is
/// restated here because `override_refusal` replaces code and message together: only
/// the message's CONSTRUCTION changes below.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this service answers a malformed ARN with.
///
/// Transfer validates an ARN-valued member ITSELF — "Invalid Role ARN" is the
/// refusal the service answers a role ARN it will not accept with — and surfaces it
/// as its own modelled client error, which every operation that takes such a member
/// declares (`InvalidRequestException`, `errorKind` client, `httpError` 400). The
/// framework's synthetic `ValidationException` is NOT in any Transfer operation's
/// modelled error list, so a client branching on this refusal through the service's
/// own exception would never catch it.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// Whether `pattern` is one that requires the value to be an ARN.
///
/// The model states these as `^arn:...` (`Role`: `^arn:.*role/\S+$`) or as an
/// optional ARN (`NullableRole`: `^(|arn:.*role/\S+)$`), so both forms are
/// recognised — and no member name is listed here: the rule follows the CONSTRAINT,
/// so it covers every member of this service whose pattern demands an ARN.
fn is_arn_pattern(pattern: &str) -> bool {
    pattern.starts_with("^arn:") || pattern.starts_with("^(|arn:")
}

/// The member name an ARN refusal names, from the violation's dotted path: its last
/// segment, in the PascalCase the model spells the member with (the path carries the
/// lowerCamel form, e.g. `role` -> `Role`, `loggingRole` -> `LoggingRole`).
fn member_name(path: &str) -> String {
    let segment = path.rsplit('.').next().unwrap_or(path);
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Render ONE ARN-pattern violation the way Transfer states it: `Invalid <Member>
/// ARN` — the member whose ARN was rejected, and nothing of the value that was
/// rejected.
///
/// `None` for every other violation class, including a pattern on a member that is
/// not an ARN: those keep this service's generic constraint wording below.
fn arn_constraint_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    if !is_arn_pattern(pattern) {
        return None;
    }
    Some(format!("Invalid {} ARN", member_name(&violation.path)))
}

/// The refusal for a set of violations that are ALL ARN-pattern violations: one
/// clause per violation, so the single-violation case — the only one a request with
/// one bad ARN produces — is exactly `Invalid <Member> ARN`.
///
/// `None` declines the whole refusal, including a refusal that mixes an ARN
/// violation with another class: one consistent wording per message rather than two
/// half-rendered ones.
fn arn_constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(arn_constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    (!clauses.is_empty()).then(|| clauses.join("; "))
}

/// The MODEL's pattern as a message states it: the expression without the anchors a
/// Smithy `@pattern` writes around it.
///
/// Smithy `@pattern` semantics are a *find* — the expression must match SOMEWHERE in
/// the value — so a model's leading `^` and trailing `$` are redundant notation
/// rather than part of the requirement, and the service quotes the expression
/// without them. A `$` the expression ESCAPES (`\$`) is a literal dollar sign, not an
/// anchor, so it stays.
///
/// The strip is POSITIONAL, so it handles the one anchored expression Transfer's model
/// writes (`^…$`) and would garble an alternation that anchors each branch
/// (`^a$|^b$`); the model carries none. The same rule is staged, unwired, in
/// `services/common/src/validation.rs` and inlined in ~25 service crates — this copy
/// is one of them, and should move when that module is wired up.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let Some(trimmed) = body.strip_suffix('$') else {
        return body;
    };
    // An ODD number of backslashes before the `$` escapes it, making it a literal.
    let escaped = trimmed.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1;
    if escaped {
        body
    } else {
        trimmed
    }
}

/// The order this service LISTS an enum's admitted values in, for the enums it is
/// observed to publish a listing for.
///
/// An enum clause states a MEMBERSHIP and an ORDER. The membership is the model's,
/// always. The order is not: this service lists `WorkflowStepType` as `DELETE, DECRYPT,
/// …` while the model declares it `COPY, CUSTOM, TAG, DELETE, DECRYPT`
/// (`smithy_query(query="shape",shape="com.amazonaws.transfer#WorkflowStepType")`), and no
/// order derivable from the model — declaration order, sorted, reversed, by length, by the
/// bucket order of a hashed set — produces it. It is service metadata the public model does
/// not carry, so the only source for it is what this service's refusals are observed
/// stating, and it is recorded per enum HERE, beside the one renderer every
/// enum-constrained member of every operation goes through, rather than in a handler.
///
/// A listing is identified by the SET it orders — which is all a violation carries, the
/// clause having no shape id — and is applied ONLY to an enum whose admitted set it is a
/// permutation of. A listing can therefore neither add a value this model does not declare
/// nor drop one it does, and an enum whose membership changes in the model stops matching a
/// stale listing and states the model's order rather than an outdated set. An enum this
/// service publishes no listing for keeps the order the member schema carries, which is the
/// model's.
///
/// The order is a property of the ENUM, not of the member or the operation that reaches it,
/// so one enum states one ordered set wherever it is reached — at the top level of an
/// input, inside a nested structure, inside a structure in a list, and beside other clauses
/// of the same refusal.
const VALUE_LISTINGS: &[&[&str]] = &[
    // `WorkflowStepType`, reached by `CreateWorkflow.Steps[].Type` and
    // `OnExceptionSteps[].Type`. It is the only enum of this model declaring these five
    // values, so this membership identifies it.
    &["DELETE", "DECRYPT", "CUSTOM", "COPY", "TAG"],
    // `ProfileType`, reached by `CreateProfile.ProfileType` and
    // `ListProfiles.ProfileType`. This service lists it `PARTNER, LOCAL` while the model
    // declares it `LOCAL, PARTNER`
    // (`smithy_query(query="shape",shape="com.amazonaws.transfer#ProfileType")`), which is
    // the same kind of service-side listing `WorkflowStepType` carries above: no order
    // derivable from the model produces it. It is the only enum of this model declaring
    // these two values, so this membership identifies it.
    &["PARTNER", "LOCAL"],
];

/// Whether two value lists hold the same values, in whatever order each holds them.
fn orders_the_same_set(one: &[&str], other: &[&str]) -> bool {
    if one.len() != other.len() {
        return false;
    }
    let mut one: Vec<&str> = one.to_vec();
    let mut other: Vec<&str> = other.to_vec();
    one.sort_unstable();
    other.sort_unstable();
    one == other
}

/// `values` in the order this service lists them, or in the order they arrived — the
/// model's, off the member schema — when this service publishes no listing for that enum.
fn listed(values: &[String]) -> Vec<&str> {
    let arrived: Vec<&str> = values.iter().map(String::as_str).collect();
    match VALUE_LISTINGS
        .iter()
        .find(|listing| orders_the_same_set(listing, &arrived))
    {
        Some(listing) => listing.to_vec(),
        None => arrived,
    }
}

/// The REQUIREMENT one violation states, as Transfer words it — the modelled
/// constraint the member failed, with nothing of the value that failed it.
///
/// `None` for a violation class this service has no wording for (the framework's
/// `ViolationKind` is `#[non_exhaustive]`, so one can be added), which declines the
/// rewording and keeps the framework's own.
///
/// The sentences are the ones `services/common/src/validation.rs` stages for every
/// service; that module is not wired into any crate yet, so this table is a copy, and
/// it should move when it is.
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        // The bracketed set states the MEMBERSHIP the model declares, in the order this
        // service lists that enum's values ([`listed`]).
        ViolationKind::EnumValue(allowed) => format!(
            "Member must satisfy enum value set: [{}]",
            listed(allowed).join(", ")
        ),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// Render ONE constraint violation the way Transfer states it: the member PATH and
/// the modelled constraint it failed, and NOTHING of the value that was submitted.
///
/// A member that was absent has no submitted value at all, and `null` is the
/// service's own word for that absence rather than caller data, so that one clause
/// keeps its `Value null at '…'` subject.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let subject = match violation.kind {
        ViolationKind::MissingRequiredMember => "Value null at",
        _ => "Value at",
    };
    Some(format!(
        "{subject} '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The refusal message for a set of constraint violations: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the same
/// shape the framework's own envelope uses.
///
/// `None` declines the whole refusal — including one that mixes a class this service
/// words with a class it does not — which keeps one consistent wording for the
/// message rather than two half-rendered ones.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-transfer-transferservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Two classes are reworded, and both for the same reason — the framework's
        // default would answer a code or a message this service does not:
        //
        // * a value that fails a member's ARN-shaped pattern is Transfer's OWN
        //   client error, `InvalidRequestException` (HTTP 400), worded `Invalid
        //   <Member> ARN`: the service validates an ARN itself, and
        //   `ValidationException` is not in any of its operations' modelled error
        //   lists, so a client cannot catch that refusal through the service's
        //   exception.
        // * every other constraint violation keeps the framework's
        //   `ValidationException` but is stated WITHOUT the value that was
        //   submitted: the message names the member path and the modelled
        //   constraint only, so caller-controlled input is never echoed back into a
        //   client's logs.
        //
        // An authentication refusal, and a violation class this service has no
        // wording for, keep the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => match arn_constraint_envelope(violations) {
                Some(message) => Some(Refused {
                    code: INVALID_REQUEST_EXCEPTION.to_string(),
                    message,
                }),
                None => Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraint_envelope(violations)?,
                }),
            },
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
    use super::*;
    use crate::ops::types as t;

    /// Every enum value set this service's model declares, as the generated catalog in
    /// `ops/types/shared_types.rs` carries it — one entry per enum shape of the model,
    /// REFERENCED rather than copied, so a membership the model changes reaches these
    /// assertions by a rebuild.
    const DECLARED_ENUM_SETS: &[&[&str]] = &[
        t::AGREEMENT_STATUS_TYPE_VALUES,
        t::AS2_TRANSPORT_VALUES,
        t::CERTIFICATE_STATUS_TYPE_VALUES,
        t::CERTIFICATE_TYPE_VALUES,
        t::CERTIFICATE_USAGE_TYPE_VALUES,
        t::COMMUNICATION_MODE_VALUES,
        t::COMPRESSION_ENUM_VALUES,
        t::CONNECTOR_EGRESS_TYPE_VALUES,
        t::CONNECTOR_STATUS_VALUES,
        t::CONNECTORS_IP_ADDRESS_TYPE_VALUES,
        t::CUSTOM_STEP_STATUS_VALUES,
        t::DIRECTORY_LISTING_OPTIMIZATION_VALUES,
        t::DOMAIN_VALUES,
        t::ENCRYPTION_ALG_VALUES,
        t::ENCRYPTION_TYPE_VALUES,
        t::ENDPOINT_TYPE_VALUES,
        t::ENFORCE_MESSAGE_SIGNING_TYPE_VALUES,
        t::EXECUTION_ERROR_TYPE_VALUES,
        t::EXECUTION_STATUS_VALUES,
        t::HOME_DIRECTORY_TYPE_VALUES,
        t::HOST_KEY_STORAGE_TYPE_VALUES,
        t::IDENTITY_PROVIDER_TYPE_VALUES,
        t::IP_ADDRESS_TYPE_VALUES,
        t::MAP_TYPE_VALUES,
        t::MDN_RESPONSE_VALUES,
        t::MDN_SIGNING_ALG_VALUES,
        t::OVERWRITE_EXISTING_VALUES,
        t::PRESERVE_CONTENT_TYPE_VALUES,
        t::PRESERVE_FILENAME_TYPE_VALUES,
        t::PROFILE_TYPE_VALUES,
        t::PROTOCOL_VALUES,
        t::PROXY_MODE_VALUES,
        t::SECURITY_POLICY_PROTOCOL_VALUES,
        t::SECURITY_POLICY_RESOURCE_TYPE_VALUES,
        t::SET_STAT_OPTION_VALUES,
        t::SFTP_AUTHENTICATION_METHODS_VALUES,
        t::SIGNING_ALG_VALUES,
        t::STATE_VALUES,
        t::TLS_SESSION_RESUMPTION_MODE_VALUES,
        t::TRANSFER_TABLE_STATUS_VALUES,
        t::WEB_APP_ENDPOINT_POLICY_VALUES,
        t::WEB_APP_ENDPOINT_TYPE_VALUES,
        t::WEB_APP_VPC_ENDPOINT_IP_ADDRESS_TYPE_VALUES,
        t::WORKFLOW_STEP_TYPE_VALUES,
    ];

    /// `values` as a violation carries them: the owned strings the framework's walk builds
    /// its `EnumValue` kind from.
    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// A listing orders a value set this MODEL declares, and EXACTLY one of them: it can
    /// neither add a value the model does not admit nor drop one it does, and since a
    /// listing is resolved by its membership alone (a clause carries no shape id), a
    /// membership two enums declared differently could not be told apart. A listing
    /// recorded for such a membership fails here rather than silently re-ordering the other
    /// enum's clause.
    #[test]
    fn every_listing_orders_exactly_one_value_set_this_model_declares() {
        for listing in VALUE_LISTINGS {
            let sharing: Vec<&&[&str]> = DECLARED_ENUM_SETS
                .iter()
                .filter(|declared| orders_the_same_set(listing, declared))
                .collect();
            assert_eq!(
                sharing.len(),
                1,
                "case {listing:?}: a listing must order the values of exactly one enum this \
                 model declares, got {sharing:?}"
            );
        }
    }

    /// A listing is resolved by the SET it orders, whatever order that set arrives in, and
    /// an enum this service publishes no listing for keeps the order it arrived in — the
    /// model's.
    #[test]
    fn a_listing_is_applied_only_to_the_enum_whose_set_it_orders() {
        let step_types = ["DELETE", "DECRYPT", "CUSTOM", "COPY", "TAG"];
        // (case, the set as it arrives off a member schema, the order the clause states)
        let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
            (
                "the listed enum, arriving in the model's declaration order",
                vec!["COPY", "CUSTOM", "TAG", "DELETE", "DECRYPT"],
                step_types.to_vec(),
            ),
            (
                "the listed enum, arriving sorted",
                t::WORKFLOW_STEP_TYPE_VALUES.to_vec(),
                step_types.to_vec(),
            ),
            (
                "the listed enum, arriving already listed",
                step_types.to_vec(),
                step_types.to_vec(),
            ),
            (
                "another enum of the same size with another membership",
                t::SIGNING_ALG_VALUES.to_vec(),
                t::SIGNING_ALG_VALUES.to_vec(),
            ),
            (
                "a subset of a listed set: not that enum",
                vec!["COPY", "CUSTOM", "TAG", "DELETE"],
                vec!["COPY", "CUSTOM", "TAG", "DELETE"],
            ),
            (
                "a listed set with one value more: not that enum either",
                vec!["COPY", "CUSTOM", "TAG", "DELETE", "DECRYPT", "ENCRYPT"],
                vec!["COPY", "CUSTOM", "TAG", "DELETE", "DECRYPT", "ENCRYPT"],
            ),
            (
                "an enum with no listing",
                t::ENDPOINT_TYPE_VALUES.to_vec(),
                t::ENDPOINT_TYPE_VALUES.to_vec(),
            ),
            (
                "a single-valued enum",
                t::ENCRYPTION_TYPE_VALUES.to_vec(),
                t::ENCRYPTION_TYPE_VALUES.to_vec(),
            ),
        ];
        for (case, arriving, expected) in rows {
            let arriving = owned(&arriving);
            let actual = listed(&arriving);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            let stated = requirement(&ViolationKind::EnumValue(arriving.clone()));
            let wanted = Some(format!(
                "Member must satisfy enum value set: [{}]",
                expected.join(", ")
            ));
            assert_eq!(
                stated, wanted,
                "case {case:?}: expected the clause to state {wanted:?}, got {stated:?}"
            );
        }
    }

    /// The order lives in the renderer every enum-constrained member goes through, so the
    /// whole refusal states it — including a refusal that states a second clause beside the
    /// enum's.
    #[test]
    fn the_envelope_states_the_listing_beside_another_clause() {
        let violations = vec![
            Violation {
                path: "steps.1.member.type".to_string(),
                kind: ViolationKind::EnumValue(owned(t::WORKFLOW_STEP_TYPE_VALUES)),
                value: None,
                sensitive: false,
            },
            Violation {
                path: "description".to_string(),
                kind: ViolationKind::MaxLength(200),
                value: None,
                sensitive: false,
            },
        ];
        let actual = constraint_envelope(&violations);
        let expected = Some(
            "2 validation errors detected: Value at 'steps.1.member.type' failed to satisfy \
             constraint: Member must satisfy enum value set: [DELETE, DECRYPT, CUSTOM, COPY, \
             TAG]; Value at 'description' failed to satisfy constraint: Member must have \
             length less than or equal to 200"
                .to_string(),
        );
        assert_eq!(
            actual, expected,
            "case \"an enum clause beside a length clause\": expected {expected:?}, got \
             {actual:?}"
        );
    }
}
