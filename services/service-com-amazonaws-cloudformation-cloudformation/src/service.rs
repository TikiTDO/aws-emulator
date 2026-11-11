// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudformation-cloudformation` service: how it claims requests and how it words the
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
//!
//! Two of those seams are taken for the one reason named in
//! [`query_error_envelope`]: an awsQuery error response carries a `<RequestId>` element
//! in its `<ErrorResponse>` envelope, which the framework's renderers omit.
//! `override_operation` wraps the operation the routing table names, so the element
//! reaches a handler-returned error and a refusal raised while serving it, and the
//! [`CloudFormationDefinition`] this file hands back from `definition` adds it to a
//! routing failure, a transport failure and a pre-dispatch refusal. Everything else about
//! those paths — status, `<Type>`, `<Code>`, `<Message>` — stays with the framework.
//!
//! `override_refusal` is taken for three classes as well: a refusal of the member that names a
//! PUBLIC EXTENSION BY ARN, which CloudFormation's registry parses itself, is stated as the
//! registry states it rather than as a generic request-validation failure — see
//! [`registry_refusal_of_a_public_extension_arn`] — a value outside an enum whose value
//! set this service STATES differently from the model's declaration of it (in another order,
//! or over more values) is stated as the service states it, see
//! [`enum_value_set_as_the_service_states_it`], and every other refusal the framework
//! describes carries the one CODE this service publishes for a refused request, see
//! [`the_services_own_refusal_code`].
//!
//! `override_protocol` is taken for one rendering: a floating-point member is written as a
//! double, with its fractional part, which is how CloudFormation's envelope writes one —
//! see [`double_members`].
//!
//! The definition this file hands back is also where this front door's AUDIT leg hangs: the
//! generated entry is reached through the shared management-event recorder
//! ([`management_events`]), so every call this service serves — accepted and refused alike —
//! is recorded in the account's event history where CloudTrail's `LookupEvents` reports it,
//! through one code path for every operation rather than a recording written into each
//! handler.

pub mod custom_resource_plane;
pub mod double_members;
pub mod list_element_refusals;
pub mod management_events;
pub mod query_error_envelope;

use crate::state::provisioning::custom_resource;
use crate::state::registry;
use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol, Violation, ViolationKind};
use double_members::WithDoublesAsDoubles;
use query_error_envelope::{CloudFormationDefinition, RequestIdEnvelopeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The dotted path the validating walk names the member that identifies a public extension
/// BY ARN at — `ActivateTypeInput$PublicTypeArn`, the one member of this service's model
/// targeting `ThirdPartyTypeArn`
/// (`smithy_query(query="operation", operation="ActivateType")`), in the lowerCamel spelling
/// every violation path takes.
const PUBLIC_EXTENSION_ARN_PATH: &str = "publicTypeArn";

/// The error code of the registry's own modelled refusal, `CFNRegistryException` — the wire
/// code its `awsQueryError` trait declares, which is what a client activating a third-party
/// extension branches on.
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// The wire error code this service refuses a request on input grounds with — the code a
/// CloudFormation client branches on, and the one the framework's query renderer already
/// names for a refused value, restated here because a reworded refusal must carry it too
/// (see [`crate::scaffolded::Scaffolded::ValidationError`]).
const VALIDATION_REFUSAL_CODE: &str = "ValidationError";

/// `RegistrationStatus`, in the order CloudFormation's OWN input validation lists its value
/// set: the registration still running first, then the two statuses it concludes in.
///
/// That is the order a registration passes through, and it is not the order the model
/// declares the enum in (`smithy_query(query="shape", shape="RegistrationStatus")` gives
/// `[COMPLETE, IN_PROGRESS, FAILED]`) — so the refusal is stated from this order rather than
/// from the declaration.
const REGISTRATION_STATUS_VALUE_SET: &[&str] = &[
    registry::registration_status::IN_PROGRESS,
    registry::registration_status::COMPLETE,
    registry::registration_status::FAILED,
];

/// `TemplateStage`, in the order CloudFormation's OWN input validation lists its value set:
/// the stage CloudFormation DEPLOYS first, then the one the template was submitted as.
///
/// That is not the order the model declares the enum in
/// (`smithy_query(query="shape", shape="TemplateStage")` gives `[Original, Processed]`) — so a
/// value outside the set is refused from this order rather than from the declaration.
const TEMPLATE_STAGE_VALUE_SET: &[&str] = &[
    crate::ops::get_template::TEMPLATE_STAGE_PROCESSED,
    crate::ops::get_template::TEMPLATE_STAGE_ORIGINAL,
];

/// `StackStatus`, in the order CloudFormation's OWN input validation lists its value set.
///
/// It is not the order the model declares the enum in
/// (`smithy_query(query="shape", shape="StackStatus")` gives the lifecycle order,
/// `[CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, …]`) and it is neither sorted nor
/// grouped by lifecycle — so a value outside the set is refused from THIS order rather than
/// from the declaration. The set itself is the model's, value for value: these are the 23
/// values that `smithy_query` lists, re-ordered and not added to.
const STACK_STATUS_VALUE_SET: &[&str] = &[
    "REVIEW_IN_PROGRESS",
    "CREATE_FAILED",
    "UPDATE_ROLLBACK_FAILED",
    "UPDATE_ROLLBACK_IN_PROGRESS",
    "CREATE_IN_PROGRESS",
    "IMPORT_ROLLBACK_IN_PROGRESS",
    "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS",
    "ROLLBACK_IN_PROGRESS",
    "IMPORT_IN_PROGRESS",
    "DELETE_COMPLETE",
    "UPDATE_COMPLETE",
    "UPDATE_IN_PROGRESS",
    "DELETE_FAILED",
    "IMPORT_COMPLETE",
    "UPDATE_FAILED",
    "DELETE_IN_PROGRESS",
    "ROLLBACK_COMPLETE",
    "ROLLBACK_FAILED",
    "IMPORT_ROLLBACK_COMPLETE",
    "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS",
    "CREATE_COMPLETE",
    "IMPORT_ROLLBACK_FAILED",
    "UPDATE_ROLLBACK_COMPLETE",
];

/// `OperationStatus`, as the value set CloudFormation's OWN input validation STATES for it —
/// which is not the set the model declares.
///
/// The model carries four values (`crate::ops::types::OPERATION_STATUS_VALUES`), and the
/// service's refusal of a value outside the set reports SIX: the cancellation statuses a
/// handler reports beside them are accepted too, and the clause is the list a caller fixes
/// its request from, so it states them. Neither the model's order nor a sort of the values —
/// this is the order the refusal lists them in.
///
/// Only the SENTENCE states those two: a request carrying one is still refused, because the
/// member's wire contract is the model's enum and the validating walk that admits a value is
/// built from that model — which this crate does not author (`smithy-model/` is the service's
/// shipped model and `src/lib.rs` is generated from it), so there is no seam here that could
/// widen the set a member ACCEPTS without changing the model. Stating the wider set is what
/// the clause is for, and it is deliberate that the two differ until the model carries them.
const OPERATION_STATUS_VALUE_SET: &[&str] = &[
    "CANCEL_IN_PROGRESS",
    "SUCCESS",
    "IN_PROGRESS",
    "PENDING",
    "FAILED",
    "CANCEL_COMPLETE",
];

/// One enum of this service whose own refusals do not state its value set the way the MODEL
/// declares it — in another order, or over more values than the model's set.
///
/// Keyed by the values a violation of that enum ARRIVES carrying (the model's declaration,
/// compared as a SET) rather than by an operation or a member, so each entry governs every
/// member that targets that enum and one refusal cannot drift from another's.
struct StatedEnumValueSet {
    /// The model's own values for the enum, which is what the entry is matched on.
    modelled: &'static [&'static str],
    /// The value set the SERVICE states, in the order it states it. The same values as
    /// `modelled` where only the order is the service's; a SUPERSET where the service
    /// reports values the model does not declare.
    stated: &'static [&'static str],
}

/// Every enum of this service whose refusals state a value set the model's declaration does
/// not give — one entry per enum.
const STATED_ENUM_VALUE_SETS: &[StatedEnumValueSet] = &[
    StatedEnumValueSet {
        modelled: crate::ops::types::REGISTRATION_STATUS_VALUES,
        stated: REGISTRATION_STATUS_VALUE_SET,
    },
    StatedEnumValueSet {
        modelled: crate::ops::types::TEMPLATE_STAGE_VALUES,
        stated: TEMPLATE_STAGE_VALUE_SET,
    },
    StatedEnumValueSet {
        modelled: crate::ops::types::STACK_STATUS_VALUES,
        stated: STACK_STATUS_VALUE_SET,
    },
    StatedEnumValueSet {
        modelled: crate::ops::types::OPERATION_STATUS_VALUES,
        stated: OPERATION_STATUS_VALUE_SET,
    },
];

/// The value set this service STATES for the enum whose modelled values are `allowed`, when
/// it is one of the enums [`STATED_ENUM_VALUE_SETS`] names — matched as a SET, so the order
/// the values reached the clause in does not enter into it.
fn stated_value_set(allowed: &[String]) -> Option<&'static [&'static str]> {
    STATED_ENUM_VALUE_SETS
        .iter()
        .find(|entry| {
            entry.modelled.len() == allowed.len()
                && allowed
                    .iter()
                    .all(|value| entry.modelled.contains(&value.as_str()))
        })
        .map(|entry| entry.stated)
}

/// This service's wording for a value outside an enum's declared set: the same refusal the
/// framework states, with the value set as the SERVICE states it
/// ([`STATED_ENUM_VALUE_SETS`]).
///
/// The message is a requirement of this error class — a client matching the service's
/// refusal text matches the set and its order too — and only that list changes: the status,
/// the code and every other word stay the framework's, which is why the sentence is rebuilt
/// through the framework's own envelope rather than spelled again here. `None` when no
/// violation of this request names an enum this service states differently, which keeps
/// every other refusal exactly as the framework worded it.
fn enum_value_set_as_the_service_states_it(violations: &[Violation]) -> Option<Refused> {
    let mut restated = violations.to_vec();
    let mut reworded = false;
    for violation in &mut restated {
        let ViolationKind::EnumValue(allowed) = &mut violation.kind else {
            continue;
        };
        let Some(stated) = stated_value_set(allowed) else {
            continue;
        };
        let as_the_service_states_it: Vec<String> =
            stated.iter().map(|value| (*value).to_string()).collect();
        if *allowed != as_the_service_states_it {
            *allowed = as_the_service_states_it;
            reworded = true;
        }
    }
    if !reworded {
        return None;
    }
    Some(Refused {
        code: VALIDATION_REFUSAL_CODE.to_string(),
        message: aws_smithy_schema_server::envelope(&restated)?,
    })
}

/// The registry's own refusal for a public-extension ARN this service's registry parses and
/// resolves nothing from, in place of the generic request-validation wording.
///
/// CloudFormation reads the extension's kind, publisher and NAME out of that ARN itself and
/// refuses what the lookup cannot resolve with the registry's modelled error — the ARN is not
/// gated on syntax ahead of the registry, so the modelled `ThirdPartyTypeArn` pattern (which
/// is narrower than the ARNs CloudFormation parses) must not pre-empt that refusal with a
/// different error class. This restates exactly the two refusals the registry owns: a value
/// pinning a public version number past the extension's name
/// ([`registry::PublicTypeArnParts::public_version_number`]), and one whose name segment is no
/// extension type name ([`registry::is_type_name`]) — the same refusals, in the same order,
/// that `ActivateType`'s handler answers for an ARN the pattern let through. Every other
/// refusal of that member, and every other member's, keeps the framework's wording: this
/// service has no evidence that CloudFormation words those as registry failures.
fn registry_refusal_of_a_public_extension_arn(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violation.path != PUBLIC_EXTENSION_ARN_PATH
            || !matches!(violation.kind, ViolationKind::Pattern(_))
        {
            return None;
        }
        let arn = violation.value.as_deref()?;
        let parts = registry::split_public_type_arn(arn);
        let message = match &parts {
            Some(parts) if parts.public_version_number.is_some() => {
                registry::PUBLIC_VERSION_NUMBER_IN_ARN
            }
            Some(parts) if registry::is_type_name(&parts.type_name) => return None,
            _ => registry::INVALID_TYPE_NAME,
        };
        Some(Refused {
            code: REGISTRY_REFUSAL_CODE.to_string(),
            message: message.to_string(),
        })
    })
}

/// This service's CODE for a request the framework refused before any handler ran.
///
/// CloudFormation's Query API publishes exactly one code for a refused request —
/// `ValidationError`, the code its own documentation names and the one an SDK's error class is
/// keyed on ([`VALIDATION_REFUSAL_CODE`]) — and `MalformedInput`, which the framework's query
/// renderer names for a body it could not READ (an absent required member reaches it that
/// way, because absence is found while reading), is not a code CloudFormation answers at all.
/// So every refusal the framework can describe structurally is stated under the one code,
/// whichever layer found it.
///
/// Only the code is this service's: the MESSAGE is rebuilt through the framework's own
/// envelope over the same violations, so an absent member and a value outside a declared
/// bound keep the exact sentence they had.
fn the_services_own_refusal_code(violations: &[Violation]) -> Option<Refused> {
    Some(Refused {
        code: VALIDATION_REFUSAL_CODE.to_string(),
        message: aws_smithy_schema_server::envelope(violations)?,
    })
}

/// The `com-amazonaws-cloudformation-cloudformation` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: CloudFormationDefinition<Eng>,
    /// The envelope wrapper every routable request is served through. Held in a field,
    /// built once, because `override_operation` hands back a reference to it.
    envelope: RequestIdEnvelopeOperation<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the custom resource RESPONSE plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read and write the one store the operations use,
    /// not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            // The generated entry, routing through the AUDITED table: every modelled
            // operation, each constructed from this same shared engine and wrapped in the
            // management-event recorder (see [`management_events`]), so each call this
            // front door serves is recorded as a management event of
            // `cloudformation.amazonaws.com`.
            entry: CloudFormationDefinition::new(
                Arc::clone(&engine),
                management_events::audited_table(&engine),
            ),
            envelope: RequestIdEnvelopeOperation::new(Arc::clone(&engine)),
            engine,
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own, with every
/// floating-point member rendered as a double ([`double_members`]).
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model. Reading
/// and routing a request are the generated protocol's unchanged; so is every byte of the
/// envelope it writes.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_output(
            &WithDoublesAsDoubles::new(output),
            output_schema,
            operation_name,
            success_status,
        )
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // A modelled error of this service declares no floating-point member today, so the
        // wrapper changes nothing here; it is applied anyway so the rule is the protocol's
        // rather than one path's, and a modelled error that gains one cannot answer
        // differently from an output.
        self.entry.protocol().serialize_error(
            &WithDoublesAsDoubles::new(error),
            error_schema,
            error_code,
            http_status,
        )
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            &WithDoublesAsDoubles::new(event),
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own protocol, with the one rendering correction above. Held on `&self`
        // (it IS `&self`), so nothing is built per request.
        Some(self)
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        if match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        } {
            return true;
        }
        // ...and one whose PATH is one of the custom resource `ResponseURL`s this service
        // published: the path namespace a plane served at the emulator's own endpoint is
        // addressed under, carrying the STACK by its own ARN, so a provider following the URL
        // it was handed addresses this service's own target whatever endpoint name it reached
        // this emulator by. See [`custom_resource_plane`].
        custom_resource::parse_response_path(parts.uri.path())
            .is_some_and(|target| custom_resource::target_region(&target).is_some())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to that same table entry and then adds the
        // `<RequestId>` the awsQuery error envelope carries (see
        // [`query_error_envelope`]). The routing stays the model's: a request whose
        // `Action` names no modelled operation is declined here, so the framework answers
        // it as the unroutable request it is.
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Three classes are reworded here: a refusal of the member that
        // names a PUBLIC EXTENSION BY ARN, which the registry parses itself (see
        // [`registry_refusal_of_a_public_extension_arn`]), a value outside an enum whose
        // value set this service states differently from the model's declaration (see
        // [`enum_value_set_as_the_service_states_it`]), and — for every refusal neither of
        // those restates — the CODE this service publishes for a refused request, with the
        // framework's own sentence (see [`the_services_own_refusal_code`]).
        match refusal {
            Refusal::Invalid(violations) => {
                registry_refusal_of_a_public_extension_arn(violations)
                    .or_else(|| enum_value_set_as_the_service_states_it(violations))
                    .or_else(|| the_services_own_refusal_code(violations))
            }
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A write to the `ResponseURL` of a custom resource provider request is exactly that
        // case by construction — the path is not in the model — so this is where the
        // provider's documented answer is received. The PATH decides: a request to this
        // service's own endpoint that named no operation is a routing failure, whatever its
        // host looks like, and keeps the framework's default.
        let target =
            custom_resource::parse_response_path(::service_common::request_path(req.uri()))?;
        // The Region the answer is about: the one the stack's own ARN names, which is the
        // only thing the URL says about it — the write carries no credential of its own.
        let region = custom_resource::target_region(&target)?.to_string();
        let (answered, concluded) =
            custom_resource_plane::serve(&*self.engine, req, &region, &target);
        // An answer this plane RECORDED makes work due that only a handler can carry out:
        // the next provider request the operation behind it owes. Nothing of a client's is
        // going to drive that — the provider answered out of band — so the answer starts the
        // task that does ([`crate::state::provider_driver`]). Only an answer that concluded
        // something starts one, so a write to a URL whose request is already answered — or
        // to one nothing awaits — starts no work at all.
        if concluded {
            crate::state::provider_driver::drive_landed_answer(&self.engine, &region);
        }
        Some(answered)
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
