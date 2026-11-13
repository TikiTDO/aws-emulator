// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator` service: how it claims requests and how it words the
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
//! Six opt-in seams are yours to change; every one defaults to the framework's
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
//! - `override_protocol` — the codec the dispatch sequence reads and answers one
//!   request with. The default returns `None`, keeping the model-derived protocol,
//!   which is what this service serves.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's refusal dialect
//!
//! `override_refusal` below is edited: a modeled-constraint violation keeps the
//! framework's code and status (`ValidationException`, HTTP 400) but is WORDED the way
//! this service words it — one clause per violation inside the AWS envelope
//! (`N validation error(s) detected: …`), each naming the member path and the violated
//! constraint and nothing of the value that was rejected, and quoting a `@pattern` as
//! the expression a value has to satisfy without the `^` / `$` anchors a model writes
//! to make that match total. The framework's default clause does the opposite on both
//! points (it interpolates the rejected value and quotes the pattern verbatim), so the
//! choice is made here, once, for every operation and every constrained member of this
//! service rather than at a call site. The suite
//! `tests/all/validation_refusal_framing.rs` pins both halves.
//!
//! That is the ENVELOPE's clause. A `fieldList` ENTRY's own message is the other half of
//! the same refusal and is worded differently, which is recorded behaviour: the entry
//! already identifies its member in its own `name`, so its message identifies nothing a
//! second time and is a sentence about the VALUE the entry refused and the requirement
//! that value failed — `Value [redacted] failed to satisfy constraint: <requirement>`,
//! where the value stands as the fixed [`WITHHELD_VALUE`] marker because this service
//! echoes no submitted value. The requirement is the one the envelope's clause states, so
//! the two halves cannot word one constraint two ways; only the subject differs
//! ([`field_clause`], pinned by `tests/all/field_list_value_subject.rs`).
//!
//! ## The media type this service announces
//!
//! TWO media types, because the recorded wire shows two announcers: the dispatcher that
//! fronts the workload-estimate and bill-planning operations announces
//! `application/x-amz-json-1.1` on what it returns — its successes, its errors about stored
//! state and the account-class gate's refusal of a well-formed request — while a refusal
//! reporting the REQUEST as invalid, every response of a resource-tagging operation and
//! everything rendered in
//! front of the service announce the `application/x-amz-json-1.0` the model's
//! `aws.protocols#awsJson1_0` trait implies — which is what the framework's serializers and
//! pre-dispatch renderers already stamp. The two are told apart by the GROUND a refusal
//! states and not by the shape it is worded in, since both grounds are the one modelled
//! `ValidationException`. Which announcer owns which responses, and the pair
//! itself, are stated in [`response_media_type`]; the rule and its mechanism live in
//! [`service_common::response_media_type`].
//!
//! ONE seam carries it, and only for the dispatcher's half: [`ModelledValidation`] (reached
//! through `override_operation`, which this service already takes for its modelled
//! refusals) serves a dispatcher-fronted request through the shared
//! [`ResponseMediaTypeOperation`], which runs the same routing-table operation and
//! re-announces the responses that dispatcher produced. The modelled refusal this file
//! serializes itself, every response of a tagging operation, and the unroutable request,
//! transport failure and authentication refusal the router renders are left exactly as
//! their own renderer wrote them. Routing, status, codes, messages and bodies all stay with
//! the framework.
//!
//! STILL TO MIGRATE, carrying the note `services/common/src/validation.rs` asks every
//! new copy to carry: the requirement half, the envelope's no-value subject half and the
//! unanchored `@pattern` below are NOT a dialect — they are the same three rules that
//! module's header stages for itself, already inlined in ~24 service crates
//! (`grep -rln 'fn unanchored' services/*/src`), each pinned by its own recordings and
//! tests. This crate inlines them a further time because moving them is one change of
//! its own rather than a side effect of this service's issue; the fix that owns that
//! migration moves the rules into `services/common` and calls them from here. What
//! stays with this service either way is how the clauses are counted, ordered and
//! joined into the envelope, and the fact that this service echoes no submitted value.
//!
//! ## Why the refusal is answered in the MODELLED shape
//!
//! `ValidationException` models `reason` and `fieldList` beside `message`, and a client
//! reads both: `reason` is what tells a field-validation failure apart from a parse
//! failure or an account-class refusal, and `fieldList` is what points the user at the
//! member that was wrong. [`Refused`] — all `override_refusal` can return — is a code
//! and a message, so a refusal rendered through it carries neither member.
//!
//! [`ModelledValidation`] is where that is closed: every operation is served behind it
//! (through `override_operation`), it collects the violations the framework's own walk
//! would collect, and answers them as the modelled `ValidationException` with
//! [`REASON_FIELD_VALIDATION_FAILED`] and one `fieldList` entry per violation. The
//! `message` is the SAME envelope over the SAME clauses `override_refusal` words, and each
//! entry states the SAME requirement about the value it refused, so the two halves of the
//! body cannot state one violation two ways and the wording above is unchanged. A request
//! the model accepts is handed to the generated walk untouched.

pub mod response_media_type;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use service_common::response_media_type::ResponseMediaTypeOperation;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused,
    ServerHttpProtocol, Service, Violation, ViolationKind,
};

use crate::ops::types::{ValidationException, ValidationExceptionField};
use response_media_type::{
    fronted_by_the_dispatcher, produced_by_the_dispatcher, DISPATCHER_RESPONSE_MEDIA_TYPE,
};

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which this service declares as a common error on EVERY
/// operation (smithy_query `query="service_info"`: `common_errors` includes
/// `ValidationException`, `errorKind` client, `@httpError 400`). Restated here because
/// [`Service::override_refusal`] replaces the code and the message together, and it is
/// the same code the framework would have chosen.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status a constraint refusal is served on: `ValidationException`'s own
/// `@httpError(400)` (smithy_query `query="shape"`, `shape="ValidationException"`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// `ValidationExceptionReason`'s value for a NAMED MEMBER of the request whose value the
/// service does not accept (smithy_query: `shape="ValidationExceptionReason"` →
/// `unknownOperation | cannotParse | fieldValidationFailed | invalidRequestFromMember |
/// disallowedRate | other`).
///
/// The MODEL's own spelling, which is the spelling the recorded refusals of a member
/// outside its declared constraint carry — unlike the account-class gate, which sits in
/// front of this layer and renders the value it names in SCREAMING_SNAKE_CASE
/// (`crate::state::gate_reason`). Two producers, two recorded spellings.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// How many violations one refusal states. Each is rendered TWICE into the body — as a
/// clause of `message` and as a `fieldList` entry — so an unbounded count would turn a
/// small request into an arbitrarily large refusal. The bound is the shared one, which is
/// the boundary validator's own
/// ([`service_common::validation::MAX_VIOLATIONS`]), so the two refusal paths report
/// alike.
const MAX_VIOLATIONS: usize = service_common::validation::MAX_VIOLATIONS;

/// The wrapper a request for a DISPATCHER-FRONTED operation is served through once the
/// model has accepted it: it runs the same routing-table operation the generated table
/// holds and announces the dispatcher's media type on the responses that dispatcher
/// produced (see [`response_media_type`], and
/// [`service_common::response_media_type`] for the rule and the mechanism).
///
/// A `const` rather than a field: the wrapper is zero-sized state (a media-type pair and a
/// function pointer) and `Copy`, so each [`ModelledValidation`] carries its own copy and
/// nothing has to be borrowed from the service.
const DISPATCHER: ResponseMediaTypeOperation = ResponseMediaTypeOperation::announcing(
    DISPATCHER_RESPONSE_MEDIA_TYPE,
    produced_by_the_dispatcher,
);

/// What a violated constraint REQUIRED, as this service words it — the half both halves
/// of a refusal body share, so an envelope clause and a `fieldList` entry cannot describe
/// one constraint two ways.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    let requirement = match kind {
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
        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: the service states the pattern a value has to satisfy,
        // not the anchoring the model uses to make that match total. That is RECORDED
        // behaviour for this service — the recorded AWS refusal of a pattern-violating
        // name quotes `[a-zA-Z0-9-]+` where the model declares `^[a-zA-Z0-9-]+$` — not
        // a local preference, so a client that recompiles the quoted expression gets
        // the same regex object the real service quoted. A client re-validating with
        // it still has to anchor the match itself, exactly as it would against AWS.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(requirement)
}

/// Render one constraint violation as a clause of the `message` ENVELOPE: the member path
/// and the violated constraint, and NOTHING of the value that was rejected.
///
/// `None` for a violation class this service has no wording for — see [`requirement`].
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    // An absent member has no submitted value at all, so it is reported as the null it
    // was: that is a fact of the request's SHAPE, not a rendering of a value the caller
    // sent. Every other violation names the member only — the prose half of the body
    // states what the request had to satisfy and not what it carried.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The token a `fieldList` entry's message states where the value it refused would go.
///
/// A fixed text, the same for every member and every value and never derived from a
/// request: this service echoes no submitted value (see this module's docs), and an
/// entry's message is a sentence ABOUT the value it refused, so this marker is what
/// stands at the head of that sentence — saying the value was withheld rather than that
/// there was none, which is what a support tool reads it as.
const WITHHELD_VALUE: &str = "[redacted]";

/// Render one constraint violation as the `message` of the `fieldList` ENTRY that reports
/// it: the VALUE the entry refused and the requirement that value failed, and no member
/// path — the entry names its member in its own `name` member, so its message identifies
/// it nowhere a second time.
///
/// `None` for a violation class this service has no wording for — see [`requirement`].
///
/// The subject is about the value rather than about the member, which is the one place
/// the two halves of this service's refusal body differ: a value the request DID carry
/// stands as the [`WITHHELD_VALUE`] marker, and a member the request did not carry keeps
/// the `null` its absence is — there was no value to withhold.
fn field_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let value = match &violation.kind {
        ViolationKind::MissingRequiredMember => "null",
        _ => WITHHELD_VALUE,
    };
    Some(format!(
        "Value {value} failed to satisfy constraint: {requirement}"
    ))
}

/// A `@pattern` expression without the pair of anchors a model spells a whole-string
/// match with. Only a BOTH-ends anchored expression is unwrapped: a model that anchored
/// one end alone states a prefix or suffix rule, and dropping that anchor would widen
/// the language the expression describes. A `$` escaped by a backslash is a literal
/// dollar sign rather than the end anchor, so it is not one of the pair.
///
/// Stripping is positional, so it is only safe while the anchors it removes are the
/// WHOLE expression's: an alternation whose branches are individually anchored
/// (`^a$|^b$`) would otherwise become a different regex (`a$|^b`). An expression left
/// holding an anchor of its own after the strip is therefore returned as the model
/// spells it, rather than rewritten into something that matches a different language; a
/// `^` inside a character class is a negation, not an anchor, and does not trigger that.
/// No member of this service's model has the alternation shape today; the guard is what
/// keeps the helper correct for the next one.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(trimmed) = body.strip_suffix('$').filter(|rest| unescaped_tail(rest)) else {
        return pattern;
    };
    if has_interior_anchor(trimmed) {
        pattern
    } else {
        trimmed
    }
}

/// Whether the character following `head` was NOT escaped by a backslash: an even run of
/// trailing backslashes is a run of escaped backslashes, so the next character stands on
/// its own.
fn unescaped_tail(head: &str) -> bool {
    (head.len() - head.trim_end_matches('\\').len()).is_multiple_of(2)
}

/// Whether `expression` still carries an anchor of its own — an `^` or `$` that is
/// neither escaped nor inside a character class — after the outer pair was stripped.
///
/// A class is skipped because `^` is a NEGATION there (`[^a-z]`) and `$` an ordinary
/// character, so neither anchors anything and an expression built from one must still
/// have its outer anchors removed.
fn has_interior_anchor(expression: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    for ch in expression.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '^' | '$' if !in_class => return true,
            _ => {}
        }
    }
    false
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, joined with
/// `; ` in the order the validating walk found them.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
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

/// The 400 `ValidationException` that states `violations` in the shape's own modelled
/// members, or `None` to decline the refusal — see [`requirement`], whose wording both
/// halves of the body are built from.
///
/// One rendering for every refused member of every operation: `message` is the envelope
/// [`constraint_envelope`] builds over [`constraint_clause`], `reason` is
/// [`REASON_FIELD_VALIDATION_FAILED`] — a named member of the request carries a value the
/// service does not accept — and `fieldList` carries one entry per violation, `name` the
/// member path the walk reports it at and `message` that violation's own
/// [`field_clause`], naming the value it refused and the constraint that value had to
/// satisfy. A client can therefore branch on `reason` and report WHICH member it got
/// wrong and with which value, instead of parsing the prose.
///
/// Nothing here is keyed to an operation, a member or a value: the rule is applied to
/// whatever [`Violation`] the walk produces, so it holds for every constrained member of
/// every operation — a `@pattern` on a name or an identifier, a `@length`, a `@range`, an
/// absent `@required` member, a member nested in a usage modification or in a tag map
/// alike.
fn field_validation_failed(violations: &[Violation]) -> Option<ValidationException> {
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
    let message = constraint_envelope(violations)?;
    let field_list = violations
        .iter()
        .map(|violation| {
            Some(ValidationExceptionField {
                name: violation.path.clone(),
                // The same REQUIREMENT the envelope states for this member, about the
                // value this entry refused rather than about the member path the
                // envelope names: the entry's own `name` carries that path already.
                message: field_clause(violation)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ValidationException {
        message,
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(field_list),
    })
}

/// One modelled operation, served so that a request refused BEFORE its handler runs is
/// answered in the modelled `ValidationException` — with `reason` and `fieldList` — rather
/// than in the framework's generic refusal body, which carries `message` alone.
///
/// Holds no engine and no operation instance. It carries the operation TYPE, which is all
/// it needs — the input shape to read the request into and the input `Schema` to validate
/// it against — and hands a request the model accepts to the one operation instance the
/// generated routing table built from the shared engine. There is therefore no second
/// instance of any operation and no second store.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
    /// `Some` for an operation the dispatcher fronts, and then the request the model
    /// accepts is served through [`DISPATCHER`], which announces that dispatcher's media
    /// type on the responses it produced; `None` for a resource-tagging operation, whose
    /// responses keep the media type the framework stamps. See [`response_media_type`].
    dispatcher: Option<ResponseMediaTypeOperation>,
}

impl<Op> ModelledValidation<Op> {
    const fn new(dispatcher: Option<ResponseMediaTypeOperation>) -> Self {
        Self {
            operation: PhantomData,
            dispatcher,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = effective_protocol(svc);
                // The framework's OWN machinery — the protocol's deserializer and
                // `constraints::validate` over the built input — so this cannot find a
                // different violation set from the walk that follows it.
                let violations =
                    service_common::validation::refusing_violations::<Op>(protocol, req);
                match field_validation_failed(&violations) {
                    Some(modelled) => match protocol.serialize_error(
                        &modelled,
                        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION_STATUS,
                    ) {
                        Ok(response) => Some(response),
                        // A refusal that will not serialize leaves the request to the
                        // framework, which refuses it in the generic envelope — the very
                        // divergence this wrapper removes, so it is logged rather than
                        // passed over silently.
                        Err(error) => {
                            tracing::warn!(
                                operation = Op::NAME,
                                %error,
                                "this service's constraint refusal would not serialize; it \
                                 falls back to the framework's generic envelope"
                            );
                            None
                        }
                    },
                    None => None,
                }
            };
            if let Some(response) = refused {
                // The row the framework sequence would have recorded for this request,
                // so a refused one is not a hole in the telemetry series.
                record_refusal::<Op>(ctx, started.elapsed());
                // Answered by the request validator in front of the dispatcher, so it
                // keeps the protocol-implied media type the serializer above stamped —
                // whichever operation it refuses. See [`response_media_type`].
                return response;
            }
            // Nothing to state: the request is served by the generated operation, which
            // runs the whole framework sequence (deserialize, validate, handle,
            // serialize) plus its telemetry over the same bytes. Reached through the
            // routing table rather than through `override_operation`, which would route
            // straight back here. A refusal this wrapper declined — a violation class
            // `requirement` has no wording for, or an error that would not
            // serialize — is refused there instead, in the framework's own envelope.
            //
            // A dispatcher-fronted operation goes through [`DISPATCHER`], which runs that
            // same routing-table entry and then announces the dispatcher's media type on
            // what it produced; a tagging operation is run directly, so its responses keep
            // what the framework stamped.
            match &self.dispatcher {
                Some(dispatcher) => dispatcher.run(svc, req, ctx).await,
                None => match svc.definition().routing_table().get(Op::NAME) {
                    Some(operation) => operation.run(svc, req, ctx).await,
                    // Unreachable in practice: this wrapper is only reachable under the key
                    // the generated table registered. Answer the framework's own
                    // no-such-operation response rather than inventing one.
                    None => svc.definition().routing_error_response(req),
                },
            }
        })
    }
}

/// The protocol the dispatch sequence serializes a response for `svc` with: the service's
/// own [`Service::override_protocol`] when it takes that seam, else the model-derived one.
///
/// The same selection the framework's centralized walk makes (`resolve_protocol` in
/// `vendor/smithy-server-runtime/src/operation.rs`, which is private to it), restated here
/// so a refusal this crate serializes itself cannot be framed in a different codec from
/// the responses the walk serializes beside it.
fn effective_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a request refused by the wrapper is not a hole in the telemetry
/// series: the same classification (from the refusal's status and error shape), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Everything is gated on a client collecting,
/// exactly as the framework gates it.
fn record_refusal<Op: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// A constraint is a property of the INPUT SHAPE and this service declares
/// `ValidationException` as a common error on every operation (smithy_query
/// `query="service_info"`), so the wrapper goes in front of all of them — implemented or
/// still scaffolded. A stub answers the modelled 400 for a request no operation could
/// serve and its 501 for one it could, which is the order the real service evaluates
/// them in.
///
/// Each wrapper also carries which announcer answers for its operation
/// ([`fronted_by_the_dispatcher`], read off the model's own operation name), so one table
/// states both halves of what `override_operation` serves.
fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                {
                    let name = <crate::ops::$module::$operation<Eng> as Operation>::NAME;
                    let dispatcher = fronted_by_the_dispatcher(name).then_some(DISPATCHER);
                    refusals.insert(
                        name,
                        Box::new(ModelledValidation::<
                            crate::ops::$module::$operation<Eng>,
                        >::new(dispatcher)) as Box<dyn ErasedOperation>,
                    );
                }
            )+
        };
    }
    register!(
        batch_create_bill_scenario_commitment_modification::BatchCreateBillScenarioCommitmentModification,
        batch_create_bill_scenario_usage_modification::BatchCreateBillScenarioUsageModification,
        batch_create_workload_estimate_usage::BatchCreateWorkloadEstimateUsage,
        batch_delete_bill_scenario_commitment_modification::BatchDeleteBillScenarioCommitmentModification,
        batch_delete_bill_scenario_usage_modification::BatchDeleteBillScenarioUsageModification,
        batch_delete_workload_estimate_usage::BatchDeleteWorkloadEstimateUsage,
        batch_update_bill_scenario_commitment_modification::BatchUpdateBillScenarioCommitmentModification,
        batch_update_bill_scenario_usage_modification::BatchUpdateBillScenarioUsageModification,
        batch_update_workload_estimate_usage::BatchUpdateWorkloadEstimateUsage,
        create_bill_estimate::CreateBillEstimate,
        create_bill_scenario::CreateBillScenario,
        create_workload_estimate::CreateWorkloadEstimate,
        delete_bill_estimate::DeleteBillEstimate,
        delete_bill_scenario::DeleteBillScenario,
        delete_workload_estimate::DeleteWorkloadEstimate,
        get_bill_estimate::GetBillEstimate,
        get_bill_scenario::GetBillScenario,
        get_preferences::GetPreferences,
        get_workload_estimate::GetWorkloadEstimate,
        list_bill_estimate_commitments::ListBillEstimateCommitments,
        list_bill_estimate_input_commitment_modifications::ListBillEstimateInputCommitmentModifications,
        list_bill_estimate_input_usage_modifications::ListBillEstimateInputUsageModifications,
        list_bill_estimate_line_items::ListBillEstimateLineItems,
        list_bill_estimates::ListBillEstimates,
        list_bill_scenario_commitment_modifications::ListBillScenarioCommitmentModifications,
        list_bill_scenario_usage_modifications::ListBillScenarioUsageModifications,
        list_bill_scenarios::ListBillScenarios,
        list_tags_for_resource::ListTagsForResource,
        list_workload_estimate_usage::ListWorkloadEstimateUsage,
        list_workload_estimates::ListWorkloadEstimates,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_bill_estimate::UpdateBillEstimate,
        update_bill_scenario::UpdateBillScenario,
        update_preferences::UpdatePreferences,
        update_workload_estimate::UpdateWorkloadEstimate,
    );
    refusals
}

/// The `com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The modelled-refusal wrapper every operation is served behind, keyed by the
    /// routing key the model names that operation by; see [`ModelledValidation`]. Built
    /// once here because [`Service::override_operation`] hands back a borrow of it.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusals: modelled_validation_table::<Eng>(),
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
        // Serve every modelled operation through its [`ModelledValidation`] wrapper, so
        // a request refused before the handler runs is stated in the MODELLED
        // `ValidationException` — with the `reason` and `fieldList` members a client
        // reads — instead of the framework's generic refusal body, which can carry only
        // a code and a message. See [`ModelledValidation`] for why that cannot be done
        // from `override_refusal` or from inside `handle`.
        //
        // The key comes from the protocol's own routing resolution and the wrappers are
        // registered under the model's own operation names, so a request naming no
        // modelled operation resolves to nothing here and is left to the generated
        // routing table (and then to the framework's routing error).
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        self.refusals
            .get(routing_key)
            .map(|wrapper| wrapper.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation WITHOUT the value that violated
        // it: the message names the member path and the constraint only, and quotes a
        // `@pattern` as the expression itself rather than the anchored form the model
        // writes. The code stays the modeled `ValidationException` (HTTP 400), which is
        // what the framework would have answered; only the wording changes.
        //
        // Reached only for a refusal [`ModelledValidation`] declined (a violation class
        // [`requirement`] has no wording for), because that wrapper answers every
        // other one in the modelled shape BEFORE the framework renders it — the words
        // are not the only problem there: the generic refusal body cannot carry the
        // modelled `reason`/`fieldList` members at all, and `Refused` is a code and a
        // message. The wording is the SAME envelope either way, so the two paths cannot
        // state one violation two ways.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(
                    &violations[..violations.len().min(MAX_VIOLATIONS)],
                )?,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `@pattern` this service's model declares, as the model spells it, paired
    /// with the expression a clause has to quote. Read with `smithy_query`
    /// (`query="shape"` on `WorkloadEstimateName`, `ResourceId`, `Arn`, `ClientToken`,
    /// `NextPageToken`, `Tags` — whose KEY is `^[\w\s:+=@/-]+$` and whose VALUE is the
    /// same class with `*` — and `AccountId`).
    #[test]
    fn a_modelled_pattern_is_quoted_without_its_outer_anchors() {
        let cases = [
            // `WorkloadEstimateName` / `BillScenarioName` / `BillEstimateName`.
            ("^[a-zA-Z0-9-]+$", "[a-zA-Z0-9-]+"),
            // `ResourceId`.
            (
                "^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$",
                "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}",
            ),
            // `Arn`.
            (
                "^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$",
                "arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+",
            ),
            // `ClientToken`.
            (r"^[\u0021-\u007E]+$", r"[\u0021-\u007E]+"),
            // `NextPageToken`.
            (r"^[\S\s]*$", r"[\S\s]*"),
            // `ResourceTagKey`, the `Tags` map's key.
            (r"^[\w\s:+=@/-]+$", r"[\w\s:+=@/-]+"),
            // `ResourceTagValue`, its value: the same class, `*` rather than `+`.
            (r"^[\w\s:+=@/-]*$", r"[\w\s:+=@/-]*"),
            // `AccountId`.
            (r"^\d{12}$", r"\d{12}"),
        ];
        for (declared, quoted) in cases {
            let actual = unanchored(declared);
            assert_eq!(
                actual, quoted,
                "case {declared:?}: expected {quoted:?}, got {actual:?}"
            );
        }
    }

    /// The cases the positional strip must NOT rewrite, and the ones it still must: an
    /// expression whose own anchors are not the outer pair, or that anchors one end
    /// only, keeps the model's spelling, while an escaped `$` and a negated character
    /// class are ordinary content.
    #[test]
    fn an_expression_whose_anchors_are_not_the_outer_pair_is_left_alone() {
        let cases = [
            // Individually anchored alternation: stripping would change the language.
            ("^a$|^b$", "^a$|^b$"),
            ("^(^a)$", "^(^a)$"),
            // A trailing escaped `$` is a literal dollar, so the expression anchors its
            // START only: a prefix rule, which dropping the `^` would widen.
            (r"^cost\$", r"^cost\$"),
            ("suffix$", "suffix$"),
            // `^` inside a class is a negation, not an anchor.
            ("^[^ ]+$", "[^ ]+"),
            (r"^[\]^]+$", r"[\]^]+"),
            // No anchors at all: quoted exactly as declared.
            ("[0-9]+", "[0-9]+"),
            // An escaped backslash before the end anchor: the `$` is still the anchor.
            (r"^a\\$", r"a\\"),
        ];
        for (declared, quoted) in cases {
            let actual = unanchored(declared);
            assert_eq!(
                actual, quoted,
                "case {declared:?}: expected {quoted:?}, got {actual:?}"
            );
        }
    }

    /// The envelope's counting and joining, which is this service's own dialect.
    #[test]
    fn the_envelope_counts_and_joins_its_clauses() {
        let violation = |path: &str, kind| Violation {
            path: path.to_string(),
            kind,
            // The framework attaches the rejected value; this service drops it, which is
            // what these cases pin.
            value: Some("submitted".to_string()),
            sensitive: false,
        };
        let one = [violation("name", ViolationKind::MaxLength(64))];
        assert_eq!(
            constraint_envelope(&one).as_deref(),
            Some(
                "1 validation error detected: Value at 'name' failed to satisfy \
                 constraint: Member must have length less than or equal to 64"
            ),
            "one violation: got {:?}",
            constraint_envelope(&one)
        );

        let two = [
            violation("name", ViolationKind::MaxLength(64)),
            Violation {
                path: "identifier".to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            },
        ];
        assert_eq!(
            constraint_envelope(&two).as_deref(),
            Some(
                "2 validation errors detected: Value at 'name' failed to satisfy \
                 constraint: Member must have length less than or equal to 64; \
                 Value null at 'identifier' failed to satisfy constraint: Member must \
                 not be null"
            ),
            "two violations: got {:?}",
            constraint_envelope(&two)
        );

        assert_eq!(
            constraint_envelope(&[]),
            None,
            "no violations: the framework's own wording stands"
        );
    }

    /// The two subjects of one refusal, over every violation class this service words: the
    /// envelope clause names the member path, the `fieldList` entry's message is about the
    /// value it refused and names no member path, and both state the same requirement.
    #[test]
    fn a_field_entry_is_about_the_value_and_the_envelope_names_the_member() {
        let cases = [
            (
                ViolationKind::Pattern("^[a-zA-Z0-9-]+$".to_string()),
                "Member must satisfy regular expression pattern: [a-zA-Z0-9-]+",
            ),
            (
                ViolationKind::MaxLength(64),
                "Member must have length less than or equal to 64",
            ),
            (
                ViolationKind::MinLength(36),
                "Member must have length greater than or equal to 36",
            ),
            (
                ViolationKind::MaxValue("25".to_string()),
                "Member must have value less than or equal to 25",
            ),
            (
                ViolationKind::MinValue("1".to_string()),
                "Member must have value greater than or equal to 1",
            ),
            (
                ViolationKind::EnumValue(vec!["one".to_string(), "two".to_string()]),
                "Member must satisfy enum value set: [one, two]",
            ),
            (ViolationKind::UniqueItems, "Member must have unique values"),
        ];
        for (kind, requirement) in cases {
            let violation = Violation {
                path: "name".to_string(),
                kind,
                value: Some("submitted".to_string()),
                sensitive: false,
            };
            let expected =
                format!("Value {WITHHELD_VALUE} failed to satisfy constraint: {requirement}");
            let actual = field_clause(&violation);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {:?}: expected {expected:?}, got {actual:?}",
                violation.kind
            );
            let envelope_clause =
                format!("Value at 'name' failed to satisfy constraint: {requirement}");
            let actual = constraint_clause(&violation);
            assert_eq!(
                actual.as_deref(),
                Some(envelope_clause.as_str()),
                "case {:?}: expected {envelope_clause:?}, got {actual:?}",
                violation.kind
            );
        }
    }

    /// The one case a field entry has no value to be about: an ABSENT member, which keeps
    /// the null its absence is in both halves of the body.
    #[test]
    fn an_absent_member_is_reported_as_the_null_it_was() {
        let absent = Violation {
            path: "name".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let actual = field_clause(&absent);
        assert_eq!(
            actual.as_deref(),
            Some("Value null failed to satisfy constraint: Member must not be null"),
            "case absent member, field entry: got {actual:?}"
        );
        let actual = constraint_clause(&absent);
        assert_eq!(
            actual.as_deref(),
            Some("Value null at 'name' failed to satisfy constraint: Member must not be null"),
            "case absent member, envelope clause: got {actual:?}"
        );
    }
}
