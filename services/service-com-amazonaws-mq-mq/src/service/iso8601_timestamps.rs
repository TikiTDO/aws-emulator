// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` responses are written: the TIMESTAMP form every
//! modelled body carries, and the FRAMING an answer that carries no content states.
//!
//! This is the one seam every response a modelled operation produces passes through
//! ([`smithy_server_runtime::Service::override_protocol`], wired in [`crate::service`]),
//! so both corrections live on it rather than being restated by each operation that
//! needs one. Everything else — routing, request deserialization, the body bytes of
//! every answer that carries content, and every modelled failure — is the framework's
//! own `restJson1` protocol, delegated to unchanged.
//!
//! # Timestamps
//!
//! RULE: every timestamp Amazon MQ answers with is an ISO-8601 / RFC-3339 date-time
//! STRING — `"2026-10-30T15:43:01.513Z"` — never the epoch-seconds number the
//! protocol defaults to.
//!
//! This model declares exactly ONE timestamp shape, `com.amazonaws.mq#__timestampIso8601`,
//! and it carries `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="__timestampIso8601",
//! model_path="services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json")`),
//! so EVERY timestamp member of EVERY shape of this service — a broker's `created`, a
//! configuration's `created`, a configuration revision's `created` — is a date-time
//! one. The rule is therefore the protocol's, not any one shape's.
//!
//! WHY IT HAS TO BE APPLIED HERE. The JSON codec resolves a timestamp's wire form from
//! the MEMBER schema alone (`vendor/smithy-schema-server/src/json.rs`'s
//! `write_timestamp` reads `Schema::timestamp_format()`) and falls back to the
//! protocol default of epoch seconds when the member schema reports no format. The
//! schema built from the model carries no member-level `@timestampFormat` at all (grep
//! `vendor/smithy-macros`: it emits no `with_timestamp_format`), so every timestamp of
//! this service went onto the wire as a bare number: a client parsing the member with
//! a date-time parser — which is what an SDK bound to this model does — fails outright.
//!
//! The structural fix belongs in that codegen, which a service crate cannot edit
//! (`vendor/` is read-only), and the alternative on the service side is to hand-write
//! the schema and codec of every structure that holds a timestamp (eight of them here).
//! So this module closes it once, at the protocol seam `service.rs` already owns: the
//! output is serialized with every timestamp written as the date-time TEXT of its
//! instant, and nothing else about the walk changes — the member's wire name, its
//! order and its schema stay the ones the model gave it, and no other member kind is
//! touched.
//!
//! Only the modelled SUCCESS body goes through it. No error shape of this model
//! carries a timestamp member, so an error document has nothing to re-render, and
//! request deserialization is untouched: no input member of this model is a timestamp.
//!
//! # An answer that carries no content
//!
//! RULE: a success at the no-content status carries no body and STATES the length it
//! has — `content-length: 0` — so a client, a proxy or a contract check reading the
//! framing of such an answer finds the length of the body that is not there rather than
//! no framing header at all.
//!
//! Every operation of this service whose success code is that status is one whose output
//! is `smithy.api#Unit`: the two tagging writes, `CreateTags` (POST /v1/tags/{ResourceArn},
//! code 204) and `DeleteTags` (DELETE /v1/tags/{ResourceArn}, code 204) —
//! `smithy_query(query="operation",operation="CreateTags,DeleteTags",
//! model_path="services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json")`.
//! The framework's renderer already writes no body for such an output, but states no
//! length for it. The correction itself is `service_common`'s, written once for every
//! service that answers at that status; all this module does is take it here.
//!
//! The framing header is the HTTP writer's last word rather than this crate's: over
//! HTTP/1.1 that writer leaves it off a no-content answer (RFC 9110 section 15.3.5
//! gives a 204 no body), which is below anything a service crate decides.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The date-time text an instant is reported as: RFC-3339 in UTC, which is what
/// Smithy's `date-time` format is and what `Format::DateTime` renders.
///
/// An instant with no date-time rendering at all (one outside the range the formatter
/// can express) keeps the protocol's own number rather than being given a fake
/// string — see [`IsoTimestampSerializer::write_timestamp`].
fn date_time_text(value: &DateTime) -> Option<String> {
    value.fmt(Format::DateTime).ok()
}

/// One output shape, serialized with every timestamp rendered as date-time text.
struct IsoTimestamps<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for IsoTimestamps<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut IsoTimestampSerializer { inner: s })
    }
}

/// The serializer the output is walked with: every call is the codec's own, except a
/// timestamp leaf, which is written as its date-time text. A nested structure, list
/// or map re-enters the wrapper, because the codec walks each level with its own
/// serializer and a timestamp is rarely the output's own member.
struct IsoTimestampSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for IsoTimestampSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &IsoTimestamps(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_list(schema, &|inner| write_elements(&mut IsoTimestampSerializer { inner }))
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_map(schema, &|inner| write_entries(&mut IsoTimestampSerializer { inner }))
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    /// The one rewrite: the member's own schema, carrying the date-time text of its
    /// instant. An instant the formatter cannot express falls through to the codec's
    /// own rendering, so a member is never dropped.
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        match date_time_text(value) {
            Some(text) => self.inner.write_string(schema, &text),
            None => self.inner.write_timestamp(schema, value),
        }
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with
/// every timestamp of a modelled response in the `date-time` form the model's one
/// timestamp shape declares, and an answer that carries no content framed as one.
#[derive(Debug)]
pub struct Iso8601TimestampRestJson {
    inner: RestJsonServerProtocol,
}

impl Iso8601TimestampRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it
    /// and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table
    /// is the model's); an author-overridden protocol drives only the modelled
    /// sequence — `deserialize_input`, `serialize_output`, `serialize_error`.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for Iso8601TimestampRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for Iso8601TimestampRestJson {
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
        // No input member of this model is a timestamp, so the request side is the
        // framework's own.
        self.inner.deserialize_input(request, input_schema)
    }

    /// The modelled success, with every timestamp in its date-time form and, when it is
    /// an answer that carries no content, framed as one. An answer that DOES carry
    /// content keeps the status, the headers and every body byte its renderer wrote.
    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let rendered = self.inner.serialize_output(
            &IsoTimestamps(output),
            output_schema,
            operation_name,
            success_status,
        )?;
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
        // No error shape of this model carries a timestamp member.
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

#[cfg(test)]
mod tests {
    use super::date_time_text;
    use aws_smithy_types::DateTime;

    /// RULE: an instant is reported as an RFC-3339 UTC date-time string. The rows span
    /// what the rendering generalises over: the epoch itself, a whole second, an
    /// instant carrying sub-second digits, and one before the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time() {
        let rows: [(f64, &str); 4] = [
            (0.0, "1970-01-01T00:00:00Z"),
            (1_793_374_981.0, "2026-10-30T15:43:01Z"),
            (1_793_374_981.5, "2026-10-30T15:43:01.5Z"),
            (-1.0, "1969-12-31T23:59:59Z"),
        ];
        for (seconds, expected) in rows {
            let rendered = date_time_text(&DateTime::from_secs_f64(seconds));
            assert_eq!(
                rendered.as_deref(),
                Some(expected),
                "case {seconds}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
