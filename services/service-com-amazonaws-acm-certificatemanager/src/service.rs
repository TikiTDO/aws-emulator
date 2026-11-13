// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-acm-certificatemanager` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   also claims a request addressed to this service's ACME surface (see "The ACME HTTP
//!   surface" below).
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: a request to a published ACME URL that presents a SigV4
//!   credential is named as an operation, so the router verifies that credential before
//!   anything is served. Every other request resolves through the generated routing
//!   table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. EDITED HERE: that is what an unsigned ACME client's
//!   request to a published ACME URL is, by construction, and this is where it is
//!   served. Anything else keeps the protocol-correct default (awsJson -> 404).
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
//! ## The ACME HTTP surface
//!
//! ACM publishes two kinds of hostname. The CONTROL plane is
//! `acm.<region>.amazonaws.com`, whose first label names the service, and that is what
//! the framework's route key resolves. Beside it, every managed ACME endpoint is given
//! a URL on the Region's ACME ENROLLMENT host,
//! `acm-acme-enroll.<region>.amazonaws.com` ([`crate::state::acme_endpoint_url`]),
//! which is the URL `AcmeEndpoint$EndpointUrl` hands an ACME client and the address
//! that client is configured with. A request to it has to reach THIS service rather
//! than the control-plane resolver, which cannot name a service from the enrollment
//! label and answers `404 UnknownService` — for a URL this service handed out.
//!
//! A caller that reached this emulator at its OWN endpoint resolves neither of those
//! hostnames, so the same ACME surface is published to it on the endpoint it is already
//! talking to, with the plane, the Region and the endpoint's id in the PATH
//! ([`crate::state::acme_local_endpoint_base_url`]) — the addressing an ACME client that
//! dials the advertised URL verbatim reaches.
//!
//! Three seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request addressed to that surface
//!   ([`acme_http::addresses_acme_surface_of_parts`]), which is what routes it here at
//!   all.
//! - [`Service::override_routing_error`] serves the request that presents NO
//!   credential. An ACME client holds none, and an ACME interaction names no modelled
//!   operation by construction, so the router reaches exactly this seam for it — BEFORE
//!   authentication, which is what lets an unsigned ACME client be answered at all.
//! - [`Service::override_operation`] serves the request that DOES present a SigV4
//!   credential, through the same [`acme_http::AcmeHttpSurface`]. The router
//!   authenticates whatever it names an operation for, so naming one here is what puts
//!   the emulator's own signature verification in front of a credentialed request to a
//!   published URL — the same verification a control-plane request gets, rather than a
//!   second signature check written into this service.
//!
//! What is served on that surface — the RFC 8555 directory, a fresh nonce, the
//! `newAccount` registration and an account's own resource, and a `501` problem document
//! for the JWS interactions this emulator cannot perform — is documented in
//! [`acme_http`]; the envelope those interactions arrive in is read by [`acme_jws`].

pub mod acme_http;
pub mod acme_jws;
pub mod acme_role_trust;
pub mod authorization;
mod enum_value_sets;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use ::aws_smithy_schema_server::constraints::{
    ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};

/// The error code this service answers an input-constraint violation with: the
/// modeled `ValidationException` (`@httpError 400`, `com.amazonaws.acm#ValidationException`),
/// which is also the code the framework's own refusal renderer picks. Restated
/// here because `override_refusal` replaces the code and the message together,
/// and a message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How ACM names the subject of a constraint clause: the POSITION in the input that
/// failed, and never the value submitted there.
///
/// The phrase is FIXED text — the member path is the only part of a clause that comes
/// from the request — so a refusal reflects no request content back at a caller or into
/// its logs. It is the subject of every recorded refusal of this class, at every
/// position: a malformed resource identifier, a request token outside its pattern, a tag
/// key or value outside its charset, a collection past its own bound, a list element
/// outside its element constraint set, and a filter value outside its enum. The one
/// position that reads differently is a member the request never sent
/// ([`ABSENT_SUBJECT`]), where there is no submitted value to withhold in the first
/// place.
const SUBJECT: &str = "Value of the input at";

/// How the subject reads when the member was ABSENT, which is the cross-service
/// rule `services/common/src/validation.rs` states ("an ABSENT member keeps
/// `Value null at '<path>' …`"): there is no submitted value to withhold, and the
/// `null` is a fact of the request's SHAPE rather than a rendering of anything the
/// caller sent.
const ABSENT_SUBJECT: &str = "Value null at";

/// The requirement one violation states, as ACM phrases it.
///
/// The requirement half and the unanchored `@pattern` below are two of the rules
/// `services/common/src/validation.rs` lists as still to migrate into it (its
/// "Still to migrate here (staged, not yet wired)" section): that module is
/// declared, but it owns only which value a clause echoes, not these, so they are
/// inlined here as they are in the other service crates that word their own
/// refusals. The migration that moves them there owns this copy too. (Which value a
/// clause echoes is not a question this service has: it echoes none — see
/// [`SUBJECT`].)
///
/// Returns `None` for a violation class this service has no wording for, which
/// lets [`constraint_envelope`] decline the whole refusal and keep the
/// framework's wording rather than invent a sentence for it ([`ViolationKind`]
/// is `#[non_exhaustive]`, so a class can be added upstream).
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
        // The requirement reported is the expression a value must match, without
        // the `^` / `$` the model spells it with: the anchoring makes the model's
        // match total, it is not part of the expression the service advertises,
        // so a client that re-validates locally with the pattern it read out of
        // the message uses the one ACM documents.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        // A "value set" is just that: the requirement is WHICH values the model
        // allows. The ORDER they are listed in is the service's own rather than
        // the model's — see [`enum_value_set`].
        ViolationKind::EnumValue(allowed) => enum_value_set(allowed),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The requirement an enum violation states: WHICH values the model allows, listed in
/// the order ACM states that enum's set in.
///
/// That order is a property of the SERVICE and not of the model — it is neither the
/// model's declaration order nor a sort of the values — so an enum this service
/// publishes an order for ([`enum_value_sets::published_order`]) is stated in it,
/// whichever member carried the value and whichever seam built the clause. A set no
/// order is published for is stated as it was presented, which is the only order the
/// emulator has a source for.
fn enum_value_set<S: AsRef<str>>(allowed: &[S]) -> String {
    let presented: Vec<&str> = allowed.iter().map(AsRef::as_ref).collect();
    let values: &[&str] = match enum_value_sets::published_order(&presented) {
        Some(published) => published,
        None => &presented,
    };
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The whole `ValidationException` message for an enum violation a HANDLER found
/// itself, at a member the framework's validating walk does not reach — a value
/// sitting directly on a union member, which that walk does not descend into.
///
/// Built from the same subject, the same requirement and the same envelope as a
/// refusal the framework raised, so one violation class reads as one message
/// whichever of the two refused the request: the position alone ([`SUBJECT`]), and the
/// value set in the order this service states it in ([`enum_value_set`]).
///
/// `path` is the member path of the position that failed, in the wire spelling a clause
/// names it with ([`lower_camel`] per step, dot-joined from the input root), which is
/// the spelling the framework's own walk writes.
pub(crate) fn enum_value_refusal(path: &str, allowed: &[&str]) -> String {
    one_error_envelope(&format!(
        "{} failed to satisfy constraint: {}",
        clause_subject(path, true),
        enum_value_set(allowed)
    ))
}

/// The `ValidationException` message for an ARN input member that names a resource of
/// ANOTHER account.
///
/// A cross-account ARN is a fact about the REQUEST rather than about the store, so ACM
/// states it as a verdict on the ARN the request carried: an ARN of the caller's own
/// account was expected and one of another account arrived. ACM states that verdict in
/// the envelope it words every identifier-validation refusal in — a resource identifier
/// of some KIND failed to validate, then the verdict on it — so the sentence is
/// member-scoped: the one part that comes from the request is the resource TYPE the
/// offending ARN carries ([`crate::state::acm_arn_resource_noun`]), which is what says
/// WHICH identifier member of a multi-ARN request was rejected. Nothing else from the
/// request or the store appears, so a refusal cannot echo a caller's own ARN or account
/// back, or say whether a record with that id exists.
///
/// Rendered here once, so every operation that holds an ARN member to the caller's
/// account answers the same sentence about its own member.
pub(crate) fn cross_account_arn_refusal(arn: &str) -> String {
    format!(
        "Resource identifier for {} has a validation error: Same account ARN expected but got \
         cross-account ARN",
        crate::state::acm_arn_resource_noun(arn)
    )
}

/// The subject a clause about `path` opens with: the position that failed
/// ([`SUBJECT`]) when the request submitted a value there, and the absent-member form
/// ([`ABSENT_SUBJECT`]) when it sent none.
///
/// What the caller submitted is NOT part of the subject either way, so the walk's echo
/// of it is read for its presence alone — which is the one thing the two subjects
/// differ on.
fn clause_subject(path: &str, submitted: bool) -> String {
    if submitted {
        format!("{SUBJECT} '{path}'")
    } else {
        format!("{ABSENT_SUBJECT} '{path}'")
    }
}

/// Render one constraint violation the way this service states it: the input position
/// that failed and the requirement it failed, and never the value submitted there
/// ([`clause_subject`]).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    // A failure of the collection's own ELEMENT is reported against the
    // collection member, with the element's constraint SET nested — the position a
    // caller fixes is the collection it sent, the index inside it is not part of the
    // requirement the model declares on the element, and the requirement stated is
    // everything that element has to satisfy rather than only the clause this one
    // value tripped over. An element set the model does not describe for that member
    // (a list nested inside a structure, an element with no constraints of its own)
    // keeps the single violated requirement, which is never wrong.
    let (subject, requirement) = match collection_element(&violation.path) {
        Some(collection) => (
            clause_subject(collection, violation.value.is_some()),
            format!(
                "Member must satisfy constraint: [{}]",
                element_constraint_set(collection).unwrap_or(requirement)
            ),
        ),
        None => (
            clause_subject(&violation.path, violation.value.is_some()),
            requirement,
        ),
    };
    Some(format!(
        "{subject} failed to satisfy constraint: {requirement}"
    ))
}

/// Every constraint the element shape of the top-level list member `name` declares, in
/// the order ACM lists them, or `None` when this service's model describes no element
/// constraint set for it.
///
/// Read off the model-derived schema: codegen attaches a collection member's element
/// constraints to that member as an [`ElementConstraintsTrait`], which is where the
/// validating walk reads them too, so this cannot state a bound the walk does not
/// enforce. The order is the one ACM reports: the length maximum, the length minimum,
/// the numeric bounds, the pattern, the value set, and last that an element must not be
/// null — which every element shape requires and no trait records.
///
/// Resolved by member NAME across every operation input, because `override_refusal` is
/// handed the violations and not the operation they came from (`Refusal::Invalid`
/// carries no schema). A name two inputs declare with DIFFERENT element constraints is
/// declined here rather than guessed at, so no refusal can state a bound the operation
/// being called does not enforce.
fn element_constraint_set(name: &str) -> Option<String> {
    if name.contains('.') {
        // Only a TOP-LEVEL list member can be resolved against an operation's input
        // schema: a member schema carries its own traits and not the members of the
        // shape it targets, so a list nested inside a structure is not reachable here.
        return None;
    }
    let mut resolved: Option<String> = None;
    for input in input_schemas() {
        let Some(traits) = input
            .members()
            .iter()
            .find(|member| {
                matches!(member.shape_type(), aws_smithy_schema::ShapeType::List)
                    && member
                        .member_name()
                        .is_some_and(|declared| lower_camel(declared) == name)
            })
            .and_then(|member| member.traits())
            .and_then(|traits| traits.get(&ElementConstraintsTrait::TRAIT_ID))
            .and_then(|wrapper| wrapper.as_any().downcast_ref::<ElementConstraintsTrait>())
            .map(ElementConstraintsTrait::inner)
        else {
            continue;
        };

        let mut kinds: Vec<ViolationKind> = Vec::new();
        if let Some(length) = traits
            .get(&LengthTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
        {
            kinds.extend(length.max.map(ViolationKind::MaxLength));
            kinds.extend(length.min.map(ViolationKind::MinLength));
        }
        if let Some(range) = traits
            .get(&RangeTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<RangeTrait>())
        {
            kinds.extend(range.max.map(|max| ViolationKind::MaxValue(bound(max))));
            kinds.extend(range.min.map(|min| ViolationKind::MinValue(bound(min))));
        }
        if let Some(pattern) = traits
            .get(&PatternTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<PatternTrait>())
        {
            kinds.push(ViolationKind::Pattern(pattern.regex.to_string()));
        }
        if let Some(values) = traits
            .get(&EnumTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<EnumTrait>())
        {
            kinds.push(ViolationKind::EnumValue(
                values.values.iter().map(|value| (*value).to_string()).collect(),
            ));
        }
        // An element shape with no constraint of its own has no set to state: its
        // violations (a `@uniqueItems` failure, say, which bounds the LIST) keep their
        // own requirement.
        if kinds.is_empty() {
            continue;
        }
        kinds.push(ViolationKind::MissingRequiredMember);
        let stated: Vec<String> = kinds.iter().filter_map(requirement).collect();
        if stated.len() != kinds.len() {
            continue;
        }
        let stated = stated.join(", ");
        match &resolved {
            // Two inputs declaring this name with different element constraints: which
            // one is being validated is not knowable here, so neither is stated.
            Some(first) if *first != stated => return None,
            Some(_) => {}
            None => resolved = Some(stated),
        }
    }
    resolved
}

/// A numeric `@range` bound as a requirement states it: an integral bound without a
/// trailing `.0`, a fractional one with its decimals — the rendering the framework uses
/// for the same bound when it reports the violation itself.
fn bound(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// A member's name as a message path spells it, which is the spelling the validating
/// walk writes into a violation's path (`aws_smithy_schema_server::constraints::path`).
///
/// The one source of that spelling for this crate: a refusal a HANDLER words names its
/// position through here too, so a path a clause states is the WIRE path of the member
/// whichever seam refused the request, rather than the model's own casing.
pub(crate) fn lower_camel(member_name: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(member_name)
}

/// Every operation input shape of this service, as the schemas codegen derives from the
/// Smithy model — the only runtime source for what a member declares. One entry per
/// operation file in `src/ops/` that HAS an input shape (`GetAccountConfiguration`
/// takes `Unit`, so it declares no member and has none).
///
/// An operation left out of it loses nothing but the composite element clause for its
/// own list members, and the suite in `tests/all/validation_message_rendering.rs`
/// asserts that clause on members of several of them.
fn input_schemas() -> [&'static aws_smithy_schema::Schema; 39] {
    use crate::ops::types;

    [
        types::ADD_TAGS_TO_CERTIFICATE_INPUT_SCHEMA,
        types::CREATE_ACME_DOMAIN_VALIDATION_INPUT_SCHEMA,
        types::CREATE_ACME_ENDPOINT_INPUT_SCHEMA,
        types::CREATE_ACME_EXTERNAL_ACCOUNT_BINDING_INPUT_SCHEMA,
        types::DELETE_ACME_DOMAIN_VALIDATION_INPUT_SCHEMA,
        types::DELETE_ACME_ENDPOINT_INPUT_SCHEMA,
        types::DELETE_ACME_EXTERNAL_ACCOUNT_BINDING_INPUT_SCHEMA,
        types::DELETE_CERTIFICATE_INPUT_SCHEMA,
        types::DESCRIBE_ACME_ACCOUNT_INPUT_SCHEMA,
        types::DESCRIBE_ACME_DOMAIN_VALIDATION_INPUT_SCHEMA,
        types::DESCRIBE_ACME_ENDPOINT_INPUT_SCHEMA,
        types::DESCRIBE_ACME_EXTERNAL_ACCOUNT_BINDING_INPUT_SCHEMA,
        types::DESCRIBE_CERTIFICATE_INPUT_SCHEMA,
        types::EXPORT_CERTIFICATE_INPUT_SCHEMA,
        types::GET_ACME_EXTERNAL_ACCOUNT_BINDING_CREDENTIALS_INPUT_SCHEMA,
        types::GET_CERTIFICATE_INPUT_SCHEMA,
        types::IMPORT_CERTIFICATE_INPUT_SCHEMA,
        types::LIST_ACME_ACCOUNTS_INPUT_SCHEMA,
        types::LIST_ACME_DOMAIN_VALIDATIONS_INPUT_SCHEMA,
        types::LIST_ACME_ENDPOINTS_INPUT_SCHEMA,
        types::LIST_ACME_EXTERNAL_ACCOUNT_BINDINGS_INPUT_SCHEMA,
        types::LIST_CERTIFICATES_INPUT_SCHEMA,
        types::LIST_CERTIFICATE_DOMAIN_VALIDATIONS_INPUT_SCHEMA,
        types::LIST_TAGS_FOR_CERTIFICATE_INPUT_SCHEMA,
        types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        types::PUT_ACCOUNT_CONFIGURATION_INPUT_SCHEMA,
        types::REMOVE_TAGS_FROM_CERTIFICATE_INPUT_SCHEMA,
        types::RENEW_CERTIFICATE_INPUT_SCHEMA,
        types::REQUEST_CERTIFICATE_INPUT_SCHEMA,
        types::RESEND_VALIDATION_EMAIL_INPUT_SCHEMA,
        types::REVOKE_ACME_ACCOUNT_INPUT_SCHEMA,
        types::REVOKE_ACME_EXTERNAL_ACCOUNT_BINDING_INPUT_SCHEMA,
        types::REVOKE_CERTIFICATE_INPUT_SCHEMA,
        types::SEARCH_CERTIFICATES_INPUT_SCHEMA,
        types::TAG_RESOURCE_INPUT_SCHEMA,
        types::UNTAG_RESOURCE_INPUT_SCHEMA,
        types::UPDATE_ACME_DOMAIN_VALIDATION_INPUT_SCHEMA,
        types::UPDATE_ACME_ENDPOINT_INPUT_SCHEMA,
        types::UPDATE_CERTIFICATE_OPTIONS_INPUT_SCHEMA,
    ]
}

/// The collection member a violation belongs to, when the failing leaf IS one of
/// that collection's elements.
///
/// The validating walk names a list element `<collection>.<1-based index>.member`.
/// A path ENDING at that step is the element itself, so the clause is reported
/// against `<collection>`; a member reached THROUGH an element
/// (`tags.1.member.key`) is an input position of its own and is reported there.
///
/// The index must be an integer, which is what distinguishes a list step from a
/// map entry (`<member>.<caller-chosen key>.member` — see `path::entry` in
/// `vendor/smithy-schema-server/src/constraints.rs`). No ACM member targets a map
/// with a constrained value, so no caller key reaches here today; were one added,
/// a purely numeric key would be read as an index.
fn collection_element(path: &str) -> Option<&str> {
    let head = path.strip_suffix(".member")?;
    let (collection, index) = head.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit());
    (indexed && !collection.is_empty()).then_some(collection)
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not
/// the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    // See the note on `requirement`: one of the rules
    // `services/common/src/validation.rs` lists as still to migrate into it,
    // inlined until it owns them.
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The AWS envelope a single-clause refusal message carries.
fn one_error_envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the
/// order the validating walk found them.
///
/// Two violations on the ELEMENTS of one collection member state the SAME clause — the
/// member's whole element constraint set, reported at the member — so that clause is
/// stated once and the refusal counts one error, not one per element or per constraint
/// the elements tripped over.
///
/// `None` declines the whole refusal — see [`requirement`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let clause = constraint_clause(violation)?;
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    match clauses.len() {
        0 => None,
        1 => Some(one_error_envelope(&clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-acm-certificatemanager` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the ACME HTTP surface an ACME endpoint publishes — the wire its
    /// `EndpointUrl` advertises. Held on the service because
    /// [`Service::override_operation`] hands the router a BORROW of one of its own
    /// operations, and it owns the shared engine so every answer is computed from the
    /// same store the control-plane handlers write.
    acme_http: acme_http::AcmeHttpSurface<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            acme_http: acme_http::AcmeHttpSurface::new(Arc::clone(&engine)),
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
        // A request whose resolved route key is this service's signing name is the
        // control plane, exactly as the framework's default claims it.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to this service's ACME surface — the enrollment host
        // this service PUBLISHES in every ACME endpoint's `EndpointUrl`, whose first
        // label is not this service's signing name, or the reserved path that surface is
        // published at on the emulator's OWN endpoint. Neither can be named by a route
        // key. See the module docs: without this the router answers `404 UnknownService`
        // for a URL this service handed out.
        acme_http::addresses_acme_surface_of_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A request to a published ACME URL that PRESENTS a SigV4 credential is named as
        // an operation, so the router verifies that credential's signature before
        // anything is served — the router authenticates only what it names an operation
        // for. A request carrying NO credential is left unnamed on purpose: it is
        // answered by `override_routing_error` below, which the router reaches before
        // authentication, so an ACME client — which holds no AWS credential at all —
        // still gets this service's own answer rather than the emulator's "sign this
        // request".
        if acme_http::carries_sigv4_credential(req) && acme_http::addresses_acme_surface(req) {
            return Some(&self.acme_http);
        }
        // Everything else resolves through the generated routing table.
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // ACM states a member-constraint failure against the INPUT POSITION that
        // failed and the constraint it failed, and never against the value
        // submitted there ([`SUBJECT`]) — a member the request did not send reads
        // as absent instead ([`ABSENT_SUBJECT`]). A `@pattern`
        // requirement names the expression without the model's `^` / `$`
        // anchoring, and an enum's value set is listed in the order this service
        // states that enum's values in. The code stays the modeled
        // `ValidationException` (HTTP 400), which is what the framework would have
        // answered; only the wording changes.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated routing
        // table resolved it. An ACME interaction is exactly that case by construction:
        // it carries no `X-Amz-Target` and no modelled operation serves its path, and
        // an ACME client presents no credential (`override_operation` names an
        // operation only for the credentialed ones), so this is where an unsigned
        // client's fetch of a published ACME URL is served. Anything else keeps the
        // framework's protocol-correct default (awsJson -> 404).
        acme_http::addresses_acme_surface(req).then(|| self.acme_http.serve(req))
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
