// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ivschat-amazoninteractivevideoservicechat` service: how it claims requests and how it words the
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
//! ## How an answer that says it has no content is framed
//!
//! A response whose status is `204 No Content` carries NO body — that is what the status
//! says, and what the recorded AWS answers carry. This service's two tag mutations answer
//! it ([`crate::state::NO_CONTENT_SUCCESS_STATUS`]) while declaring an output STRUCTURE
//! with no members rather than `smithy.api#Unit`, and the protocol serializes such a
//! structure as the empty JSON object `{}` — a document the status promises is absent. So
//! [`Service::override_protocol`] below serves the model's own protocol with one
//! correction: a response serialized with that status is answered with no body at all, and
//! states the length it has (`content-length: 0`), which is what an empty-bodied AWS answer
//! carries and what a client that requires a framing header on an empty response reads.
//! Applied here, at the single place every response of this service is serialized, it
//! holds for every operation rather than for the two that prompted it; an answer that
//! carries a body, and every other status, is left exactly as the model's protocol
//! renders it.
//!
//! ## The document a failure is answered in, and the request every response names
//!
//! A client of this service reads a failure out of the `Code`, `Message`, `Details` and
//! `RequestID` members of the response document, and every response names the request it
//! answered in the header the model publishes for it. Neither is reachable from a handler
//! (no modelled shape declares them) and the framework's `restJson1` renderers write
//! neither, so [`error_document::with_response_envelope`] completes the rendered response
//! at the one place every answer of an operation of this service passes through
//! — [`StatedEnvelope`]. The rule itself lives in [`error_document`].
//!
//! ## How this service words a refusal
//!
//! Amazon IVS Chat states a value outside a modelled constraint as `<Operation>.<member
//! path>` followed by the requirement it violated (`CreateRoom.name does not match the
//! required pattern ^[a-zA-Z0-9-_]*$`), and carries the modelled `ValidationException`'s
//! `reason` and `fieldList` with it — where the framework's own pre-dispatch envelope words
//! every service's refusal alike (`1 validation error detected: Value '…' at '…' failed to
//! satisfy constraint: …`), with no operation in it and no modelled member beyond the text.
//!
//! The operation is not reachable at the framework's refusal seam:
//! [`Service::override_refusal`] is handed the violations alone, and it can answer with a
//! code and a message but with no modelled member. So the operations are served through
//! [`StatedEnvelope`], which collects the refusal itself — from the framework's own
//! deserializer and constraint walk, so not one modelled constraint is restated here — and
//! words each violation from the OPERATION it refused, which a per-operation wrapper has in
//! scope as `O::NAME`. The sentences live in `crate::state::refusals`, beside the message
//! rules the handlers state their modelled failures with.
//!
//! Two costs of serving the operations this way are deliberate. The wrapper reads the
//! request a second time (`deserialize_input` + `constraints::validate`, which the generated
//! `run` then repeats), so a request the model accepts is parsed and validated twice; the
//! framework exposes no seam that hands a refusal to the service before rendering it, and
//! the correct refusal is worth one extra parse of a control-plane request. And the wrapped
//! set is spelled out in [`wrap_operations`]: an operation the model gains later is NOT
//! wrapped until it is listed there, and keeps the framework's generic sentence until it is
//! — the fallback is a worse message, never a wrong route, because `override_operation`
//! answers `None` for a key it does not hold and the generated table serves it unchanged.

mod error_document;

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{constraints, ProtocolError, ServerProtocol};

use error_document::with_response_envelope;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.ivschat#ValidationException`, which this service declares as the common
/// error of every operation (`smithy_query(query="service_info")` — `common_errors`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-ivschat-amazoninteractivevideoservicechat` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedEnvelope`] so a constraint refusal is stated the way this service states it
    /// and every response carries the document and the request id this service answers in.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads and writes the
    /// one store.
    stated_refusals: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedEnvelope`] and keyed by the operation's own `Operation::NAME` — the
/// model-derived routing key `ServerHttpProtocol::routing_key` resolves and the generated
/// table is keyed by, so no key is spelled by hand here.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(StatedEnvelope::new(
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
        let mut stated_refusals = OperationRoutingTable::new();
        // EVERY operation the model binds. A declared constraint is a property of the INPUT
        // SHAPE, not of a handler, and the error document and the request id are properties
        // of the RESPONSE, so the wrapper goes in front of all of them — implemented or
        // still scaffolded: a request that violates the model is refused the same way
        // whoever would have served it, and a 501 names the request it answers like every
        // other response.
        wrap_operations!(
            stated_refusals,
            engine,
            create_chat_token::CreateChatToken,
            create_logging_configuration::CreateLoggingConfiguration,
            create_room::CreateRoom,
            delete_logging_configuration::DeleteLoggingConfiguration,
            delete_message::DeleteMessage,
            delete_room::DeleteRoom,
            disconnect_user::DisconnectUser,
            get_logging_configuration::GetLoggingConfiguration,
            get_room::GetRoom,
            list_logging_configurations::ListLoggingConfigurations,
            list_rooms::ListRooms,
            list_tags_for_resource::ListTagsForResource,
            send_event::SendEvent,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_logging_configuration::UpdateLoggingConfiguration,
            update_room::UpdateRoom,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated_refusals,
        }
    }
}

/// One modelled operation, served with the constraint refusals it owes stated the way this
/// service states them — the OPERATION, the dotted member path, and the requirement the
/// request violated — and every response carrying the document and the request id this
/// service answers in.
///
/// The refusal is built from the structured [`Violation`]s the framework's own walk
/// collects, never from a rendered response and never from a constraint restated here, so
/// the wrapper cannot find a different set of violations from the walk that follows it. What
/// the wrapper adds is the one thing that walk cannot know: which operation was refused,
/// which a per-operation wrapper has as `O::NAME`.
struct StatedEnvelope<O> {
    operation: O,
}

impl<O> StatedEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatedEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r WireRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, WireResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is not
            // `Sync`, and this future is. The protocol itself is.
            let refusal = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                let violations = refused_by::<O>(protocol, req);
                validation_exception(protocol, O::NAME, &violations)
            };
            if let Some(refusal) = refusal {
                return with_response_envelope(refusal);
            }
            with_response_envelope(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a constraint the model declares (`@length`, `@range`,
/// `@pattern`, an enum's value set, `@uniqueItems`). Empty when the request satisfies the
/// model, and empty for a failure that is NOT a refusal — a body the codec cannot read at
/// all is a serialization failure, which the framework words per protocol.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), which is the same pair the centralized
/// sequence uses, so no rule is restated here.
fn refused_by<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &WireRequest,
) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking what was
        // read; to a caller they are one refusal, so they are one here — exactly as
        // `smithy_server_runtime::refusal_of` classifies them.
        Err(SerdeError::MissingMember { member_name }) => {
            vec![crate::state::missing_member_violation(member_name)]
        }
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations` for `operation`: the modelled
/// `ValidationException` at the status its `@httpError` declares, carrying the sentence, the
/// `reason` and the `fieldList` entry per member
/// (`crate::state::stated_validation_exception`).
///
/// `None` when there is nothing to refuse — and also when the refusal cannot be serialized,
/// which leaves the request to the framework's walk rather than answering a 400 with no
/// reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    violations: &[Violation],
) -> Option<WireResponse> {
    let refused = crate::state::stated_validation_exception(operation, violations)?;
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A refusal that will not serialize leaves the request to the framework, which
        // refuses it in the generic envelope — the very divergence this wrapper removes, so
        // it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the stated ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// Answer a response whose status says it carries no content with no body, and state the
/// length that answer has.
///
/// Only that status is touched: the status itself, the body and every header of every other
/// answer are the protocol's. The length is read off the answer (it has none), never
/// invented — an empty-bodied success states `content-length: 0` rather than leaving the
/// framing to the transport, which is what a client that requires a framing header on an
/// empty response reads. Over HTTP/1.1 the header does not survive on this status: the HTTP
/// writer the vendored transport uses removes `content-length` from a 204, which is below
/// anything this crate decides. See the module docs.
fn answer_no_content(response: &mut WireResponse) {
    if response.status().as_u16() == crate::state::NO_CONTENT_SUCCESS_STATUS {
        *response.body_mut() = aws_smithy_types::body::SdkBody::empty();
        let _ = response.headers_mut().try_insert("content-length", "0");
    }
}

/// The protocol this service answers modeled requests in: the model's own, with the one
/// correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one
/// protocol instance, the generated one, and nothing model-derived is restated here.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        answer_no_content(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // A modelled failure of this service states its reason in a body, and no error it
        // declares carries the no-content status, so this path is a straight delegation.
        self.entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`StatedEnvelope`] so a
        // constraint refusal is stated the way this service states it (naming the operation
        // and the member path, with the modelled `reason` and `fieldList`) rather than in
        // the framework's pre-dispatch envelope, and so every response carries the document
        // and the request id this service answers in. A key the model does not name resolves
        // to `None` here exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated_refusals.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // [`StatedEnvelope`] states a constraint refusal itself, from the operation that was
        // refused, so this seam is only reached for a refusal NO operation owns — one the
        // router answers before a key is resolved (an unauthenticated caller), or the
        // serialization fallback in [`validation_exception`]. Neither carries an operation to
        // name, and this service's sentence is built from one, so this keeps the framework's
        // wording rather than inventing a sentence with the operation missing from it.
        let _ = refusal;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modeled operation of this service produces is serialized by the
        // protocol returned here: the model's own, with the one correction the module docs
        // state — an answer whose status says it carries no content is answered with no
        // body.
        Some(self)
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
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: no non-test build needs it.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-ivschat-amazoninteractivevideoservicechat.json"
    );

    /// The number of operations this model binds (`smithy_query(query="service_info")` —
    /// `operation_count`), so a model that lost operations fails here too.
    const MODELLED_OPERATIONS: usize = 17;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedEnvelope`], under the same key the generated routing table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering a refusal in the framework's wording with no request id — and a key
    /// spelled differently from the generated table's fails here instead of silently never
    /// being reached by `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_error_envelope() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            MODELLED_OPERATIONS,
            "case the model's surface: expected this service's whole operation list, got \
             {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated_refusals.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind StatedEnvelope; \
                 this one is missing from wrap_operations!, so it answers a refusal in the \
                 framework's wording and names no request"
            );
        }
    }
}
