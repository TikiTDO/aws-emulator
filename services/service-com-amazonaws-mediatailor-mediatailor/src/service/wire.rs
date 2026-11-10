// SPDX-License-Identifier: CC0-1.0

//! How this service FRAMES an answer that carries no content.
//!
//! RULE: a success at the no-content status carries no body and states the length it has —
//! `content-length: 0` — so a client, a proxy or a contract check reading the framing finds
//! the length of the body that is not there rather than nothing at all.
//!
//! Two of this service's operations answer that status: both tagging writes model
//! `smithy.api#Unit` as their output and a success code of 204, as the `TagResource` and
//! `UntagResource` entries in this crate's `smithy-model/` document state.
//! The framework's renderer already writes no body for such an output, but states no length
//! for it.
//!
//! [`NoContentFramedRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`] — the one seam every
//! response a modelled operation produces passes through — so the framing belongs to the
//! service rather than being restated by each operation that answers at that status. The
//! correction itself is `service_common`'s, written once for every service that answers it.
//! Everything else — routing, request deserialization, every response that CARRIES content,
//! and every modelled failure — is the framework's own `restJson1` protocol, delegated to
//! unchanged.

use ::aws_smithy_runtime_api::http::{Request, Response};
use ::aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use ::aws_smithy_schema::{Schema, ShapeId};
use ::aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use ::aws_smithy_schema_server::RestJsonServerProtocol;

/// This service's wire protocol: `restJson1` exactly as the model declares it, with an
/// answer that carries no content framed as one.
#[derive(Debug)]
pub struct NoContentFramedRestJson {
    inner: RestJsonServerProtocol,
}

impl NoContentFramedRestJson {
    /// Build the protocol once, at service construction: `override_protocol` hands back a
    /// borrow of it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated `ServiceDefinition`'s own protocol (whose table is the
    /// model's); an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for NoContentFramedRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for NoContentFramedRestJson {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner.deserialize_input(request, input_schema)
    }

    /// The modelled success, framed as an answer that carries no content when it is one.
    /// Every answer that carries content is the renderer's, byte for byte.
    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let rendered =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        Ok(::service_common::no_content_responses::without_content(
            rendered,
        ))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}
