// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-partnercentralaccount-partnercentralaccount` service: how it claims requests and how it words the
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
//! # How this service refuses a request on its input constraints
//!
//! PartnerCentral Account answers a member outside a modelled constraint with its
//! own `ValidationException` — `@httpError 400`, with the prose in the `Message`
//! member the model declares. The framework's pre-dispatch renderer gets the status
//! and the error code right, but writes the prose into a framework-chosen lowercase
//! `message` key that is the same for every awsJson service, so the body carries no
//! `Message` at all and an SDK surfaces a constraint refusal with no reason.
//!
//! [`InModelledEnvelope`] closes that gap: every modelled operation is served
//! wrapped in it, and a request the wrapper finds refusable is answered through the
//! `ValidationException` envelope below instead. The violations come from the
//! framework's OWN deserializer and constraint walk, so the wrapper cannot refuse a
//! different set of requests than the walk that follows it.
//!
//! The WORDS of that envelope are not this file's: [`constraint_envelope`] delegates
//! to the shared home for boundary-refusal wording
//! ([`service_common::validation_message`]) and so to the framework behind it, which
//! renders the counted envelope and one clause per violation — the member path, the
//! requirement each constraint class states, and a `@pattern` quoted without the
//! whole-value anchors the model brackets it with. Both refusal paths go through it,
//! so they cannot disagree.
//!
//! What this service chooses is what an ENUM clause states: no submitted value, and
//! the admissible values listed in the order this service lists a set in — the
//! [`enum_value_set`] module, which owns that rule for the class.

mod enum_value_set;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which every operation declares
/// (`smithy_query(query="shape", shape="ValidationException")`). It is also the code
/// the framework would have chosen, restated here because the envelope below and
/// `override_refusal` each name the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// This service's Smithy namespace, for the error shape the envelope below names.
const NAMESPACE: &str = "com.amazonaws.partnercentralaccount";

/// The `com-amazonaws-partnercentralaccount-partnercentralaccount` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is
    /// refused in this service's own `ValidationException`. Keyed by the routing key
    /// the protocol resolves — the key the generated table uses — and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            accept_connection_invitation::AcceptConnectionInvitation,
            associate_aws_training_certification_email_domain::AssociateAwsTrainingCertificationEmailDomain,
            cancel_connection::CancelConnection,
            cancel_connection_invitation::CancelConnectionInvitation,
            cancel_profile_update_task::CancelProfileUpdateTask,
            create_connection_invitation::CreateConnectionInvitation,
            create_partner::CreatePartner,
            disassociate_aws_training_certification_email_domain::DisassociateAwsTrainingCertificationEmailDomain,
            get_alliance_lead_contact::GetAllianceLeadContact,
            get_connection::GetConnection,
            get_connection_invitation::GetConnectionInvitation,
            get_connection_preferences::GetConnectionPreferences,
            get_partner::GetPartner,
            get_profile_update_task::GetProfileUpdateTask,
            get_profile_visibility::GetProfileVisibility,
            get_qualifications_association_details::GetQualificationsAssociationDetails,
            get_qualifications_association_task::GetQualificationsAssociationTask,
            get_qualifications_disassociation_task::GetQualificationsDisassociationTask,
            get_verification::GetVerification,
            list_connection_invitations::ListConnectionInvitations,
            list_connections::ListConnections,
            list_partners::ListPartners,
            list_tags_for_resource::ListTagsForResource,
            put_alliance_lead_contact::PutAllianceLeadContact,
            put_profile_visibility::PutProfileVisibility,
            reject_connection_invitation::RejectConnectionInvitation,
            send_email_verification_code::SendEmailVerificationCode,
            start_profile_update_task::StartProfileUpdateTask,
            start_qualifications_association_task::StartQualificationsAssociationTask,
            start_qualifications_disassociation_task::StartQualificationsDisassociationTask,
            start_verification::StartVerification,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_connection_preferences::UpdateConnectionPreferences,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// The `Message` member of this service's `ValidationException`, as the model spells
/// it (`ValidationException$Message`, `@required`). The awsJson codecs key a member
/// by its MEMBER NAME, so this is what puts `Message` — not the framework's lowercase
/// `message` — in the refusal body.
static VALIDATION_EXCEPTION_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.partnercentralaccount#ValidationException$Message",
        NAMESPACE,
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_xml_name("Message");

/// The shape a constraint refusal is serialized through: this service's
/// `ValidationException`, carrying the `Message` member alone.
///
/// `Reason` is deliberately absent rather than filled with a value this layer does
/// not know. It names a rule BEYOND the modelled constraints
/// (`REQUEST_VALIDATION_FAILED` / `BUSINESS_VALIDATION_FAILED`), and the recorded
/// frontend answers a member outside its declared constraint with the message alone;
/// a handler that does know the reason sets it itself through the modelled shape in
/// `ops::types`.
static VALIDATION_EXCEPTION_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.partnercentralaccount#ValidationException",
        NAMESPACE,
        "ValidationException",
    ),
    ShapeType::Structure,
    &[&VALIDATION_EXCEPTION_MESSAGE],
);

/// The body of a constraint refusal: why the request was refused, and nothing else.
struct ConstraintRefusal {
    message: String,
}

impl SerializableStruct for ConstraintRefusal {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&VALIDATION_EXCEPTION_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's
/// own `ValidationException` (see the module docs).
///
/// Nothing else is re-implemented: a request this wrapper does not refuse is handed
/// to the generated `ErasedOperation::run`, which owns deserialize -> validate ->
/// handle -> serialize, telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a declared constraint. Empty when the request is valid,
/// and empty for a failure that is NOT a refusal — a body the codec cannot read is a
/// serialization failure, which the framework's own walk renders.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set of violations from the walk that follows it.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking
        // what was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with the
/// prose in the `Message` member the model declares. `None` when there is nothing to
/// refuse — and also when the error cannot be serialized, which leaves the
/// framework's walk to refuse the request in its own envelope rather than answering a
/// 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = ConstraintRefusal {
        message: constraint_envelope(violations)?,
    };
    protocol
        .serialize_error(
            &refused,
            &VALIDATION_EXCEPTION_SCHEMA,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// The `Message` of a constraint refusal: the AWS validation envelope
/// (`N validation error(s) detected: `) over one clause per violation, each naming the
/// member path in lowerCamelCase, the constraint it failed and — for every class but
/// one — the rejected value. `None` when there is nothing to state.
///
/// The envelope, the clauses and the `@pattern` quoting are the shared home's and the
/// framework's, so a constraint class added upstream cannot arrive with a sentence of
/// its own. This service chooses one thing about one class: an enum clause states no
/// value and lists the admissible values in its own order ([`enum_value_set`]).
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    // The order is applied to each violation BEFORE the envelope words it, so only the
    // values inside an enum clause move and every other violation reaches the shared
    // renderer exactly as the framework's walk reported it.
    let stated = Refusal::Invalid(violations.iter().map(enum_value_set::as_stated).collect());
    Some(
        service_common::validation_message::aws_validation_refusal_withholding_violations(
            &stated,
            enum_value_set::states_no_value,
        )?
        .message,
    )
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is
        // refused in this service's `ValidationException` rather than in the
        // framework's pre-dispatch envelope, which names no `Message`. A key the
        // model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam words any refusal the FRAMEWORK still renders — the
        // fallback in `validation_exception`, and a refusal raised before an operation
        // is named at all. Wording it here from the same violations keeps the two
        // paths from disagreeing: a value outside a declared bound is this service's
        // `ValidationException` either way.
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
