// SPDX-License-Identifier: CC0-1.0

//! How this service's `awsJson1_0` responses serialize a TIMESTAMP and a NESTED structure.
//!
//! Both are properties of this service's WIRE rather than of any one operation, so both
//! live at the one seam every response passes through — [`B2biJsonProtocol`], which
//! `service.rs` hands the dispatch sequence through `override_protocol`. No handler and no
//! shape declaration mentions either rule.
//!
//! # Timestamps
//!
//! Every timestamp this model declares targets `CreatedDate` or `ModifiedDate`, both of
//! which carry `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="CreatedDate,ModifiedDate")`), so every `createdAt`
//! and `modifiedAt` reaches the wire as an RFC 3339 / ISO 8601 date-time STRING rather than
//! as the epoch-seconds NUMBER an `awsJson` protocol defaults to.
//!
//! The JSON codec resolves a timestamp's wire form from the MEMBER schema alone
//! (`vendor/smithy-schema-server/src/json.rs`'s `write_timestamp` reads
//! `Schema::timestamp_format()`) and falls back to the protocol default when that schema
//! reports no format. The schemas built from a model carry no member-level format at all
//! (grep `vendor/smithy-macros`: it emits no `with_timestamp_format`), and `vendor/` is
//! read-only from inside the emulator — so the declared format is supplied here, on the way
//! out, for a member that reports none. A member that DOES declare one keeps it.
//!
//! Only the way out: no input member of this model targets a timestamp (every
//! `::aws_smithy_types::DateTime` field in `src/ops/types/` is an output member), so the
//! read side has nothing to correct and is the framework's own.
//!
//! # Nested structures
//!
//! `awsJson` lets a structure value on the wire carry a `__type` member naming the shape it
//! is an instance of, and the recorded responses of this API carry one on the structures
//! AWS serializes out of the service's shared internal model, in the namespace
//! [`COMMON_MODEL_NAMESPACE`] — never on the response's own top-level body (the operation
//! names that shape) and never on this service's own list summaries. The shapes the
//! recordings show it on are [`DISCRIMINATED_SHAPES`].
//!
//! The framework's serializer writes a structure member from the MEMBER schema (the
//! target's schema never reaches it), so a member is resolved to the shape it targets
//! through [`DISCRIMINATED_MEMBERS`], a transcription of this crate's Smithy model that
//! `tests/all/wire_timestamps_and_discriminators.rs` re-derives from the model itself.
//! A collection ELEMENT is written against its target's own schema, so it resolves by shape
//! name with no catalog row.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::json::JsonCodec;
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::rpc::HttpRpcServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The namespace the discriminator names one of this service's nested structures in.
///
/// The public model is `com.amazonaws.b2bi`, but the wire discriminator carries the id of
/// the shape in the shared internal model AWS serves this API from — the same lineage as
/// the `com.amazonaws.visby#…` an error's own `__type` carries
/// (`crate::scaffolded::wire_shape_id`). It is a constant of the shape, no part of it
/// coming from a request.
pub const COMMON_MODEL_NAMESPACE: &str = "com.amazonaws.visby.common";

/// The structures a response body serializes with a discriminator, by shape name.
///
/// Source: the recorded response bodies of this API, which show `__type` on a transformer's
/// `inputConversion`, `outputConversion` and `mapping`
/// (`com.amazonaws.visby.common#InputConversion` / `#OutputConversion` / `#Mapping`) and on
/// every `Tag` object a `ListTagsForResource` reports — and on nothing else: not on a
/// `ListProfiles` / `ListTransformers` summary, whose recorded entries carry their own
/// members only.
pub const DISCRIMINATED_SHAPES: &[&str] = &[
    "InputConversion",
    "Mapping",
    "OutputConversion",
    "Tag",
];

/// Every member of this service's model that targets one of [`DISCRIMINATED_SHAPES`], as
/// `(member name, target shape name)`.
///
/// Source: the Smithy model
/// (`smithy_query(query="shape",shape="CreateTransformerResponse,UpdateTransformerResponse,
/// GetTransformerResponse")`). The serializer is handed a member's schema rather than its
/// target's, so this is how a member is resolved to the shape the discriminator names;
/// `tests/all/wire_timestamps_and_discriminators.rs` re-derives it from the model and
/// fails if a row is wrong or a member is missing. `Tag` needs no row: it occupies a list
/// element, which is written against its own shape's schema.
pub const DISCRIMINATED_MEMBERS: &[(&str, &str)] = &[
    ("inputConversion", "InputConversion"),
    ("mapping", "Mapping"),
    ("outputConversion", "OutputConversion"),
];

/// The `__type` value for one shape name: the shape id in the shared internal model's
/// namespace.
#[must_use]
pub fn discriminator(shape_name: &str) -> String {
    format!("{COMMON_MODEL_NAMESPACE}#{shape_name}")
}

/// The discriminator the structure about to be written at `schema` carries, or `None` for a
/// structure no recording shows one on.
///
/// `schema` is a MEMBER schema when the structure occupies a structure member (the
/// framework's serializer passes the member, not its target) and the target's own schema
/// when it is a collection element, so both are resolved: the first through
/// [`DISCRIMINATED_MEMBERS`], the second by its shape name.
fn discriminator_for(schema: &Schema) -> Option<String> {
    let shape_name = match schema.member_name() {
        Some(member) => DISCRIMINATED_MEMBERS
            .iter()
            .find(|(name, _)| *name == member)
            .map(|(_, target)| *target)?,
        None => DISCRIMINATED_SHAPES
            .iter()
            .copied()
            .find(|shape| *shape == schema.shape_id().shape_name())?,
    };
    Some(discriminator(shape_name))
}

/// Schema for the `__type` member itself.
///
/// The awsJson codec reads a member's wire key off its name, so the name given here IS the
/// key written. It is not a member of any modelled shape — which is exactly why the
/// discriminator cannot come from a shape declaration and is written here instead — so it
/// lives in the `smithy.api` namespace the framework's own error-envelope discriminator
/// uses rather than colliding with one of this service's shapes.
static TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// The date-time text an instant is reported as: RFC 3339 in UTC, which is what Smithy's
/// `date-time` format is and what [`Format::DateTime`] renders.
///
/// An instant with no date-time rendering at all (one outside the range the formatter can
/// express) keeps the protocol's own number rather than being given a fake string — see
/// [`WireSerializer::write_timestamp`].
fn date_time_text(value: &DateTime) -> Option<String> {
    value.fmt(Format::DateTime).ok()
}

/// A structure serialized with its own members only, every one of them written through the
/// two rules this module states.
///
/// This is what wraps an operation's output: a response's top-level body carries no
/// `__type` (the operation names the shape), only the structures inside it do.
struct Wired<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for Wired<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0
            .serialize_members(&mut WireSerializer { inner: serializer })
    }
}

/// A structure serialized as its discriminator followed by its own members.
struct Discriminated<'a> {
    /// The shape id written as `__type`.
    discriminator: String,
    /// The structure whose members follow it.
    inner: &'a dyn SerializableStruct,
}

impl SerializableStruct for Discriminated<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(&TYPE_MEMBER_SCHEMA, &self.discriminator)?;
        self.inner
            .serialize_members(&mut WireSerializer { inner: serializer })
    }
}

/// The serializer the members of an already-opened structure are written through: every
/// write is the wrapped serializer's, except that
///
/// * a timestamp whose member schema reports no format is written as the date-time text its
///   model declares, and
/// * a nested structure is written with its discriminator when it has one,
///
/// and a nested structure, list or map is descended into either way, so a value at any
/// depth is reached.
struct WireSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for WireSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        match discriminator_for(schema) {
            Some(discriminator) => self.inner.write_struct(
                schema,
                &Discriminated {
                    discriminator,
                    inner: value,
                },
            ),
            None => self.inner.write_struct(schema, &Wired(value)),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|elements| {
            write_elements(&mut WireSerializer { inner: elements })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|entries| {
            write_entries(&mut WireSerializer { inner: entries })
        })
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

    /// The member's own schema, carrying the date-time text of its instant. A member that
    /// declares a format, or an instant the formatter cannot express, falls through to the
    /// codec's own rendering, so a member is never dropped.
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        if schema.timestamp_format().is_some() {
            return self.inner.write_timestamp(schema, value);
        }
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

/// This service's wire protocol: `awsJson1_0` exactly as the model declares it, with a
/// response body's timestamps in the `date-time` form their model declares and its nested
/// structures carrying the type discriminator the recordings show on them.
///
/// Everything but the success body is the framework's own `awsJson1_0`, delegated to
/// unchanged — routing, request deserialization, and the error envelope (whose top-level
/// `__type` the framework already writes, and whose shapes carry no timestamp member).
#[derive(Debug)]
pub struct B2biJsonProtocol {
    inner: HttpRpcServerProtocol<JsonCodec>,
}

impl B2biJsonProtocol {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: aws_smithy_schema_server::AwsJson10(),
        }
    }
}

impl Default for B2biJsonProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for B2biJsonProtocol {
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

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_output(&Wired(output), output_schema, operation_name, success_status)
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

#[cfg(test)]
mod tests {
    use super::{date_time_text, discriminator, COMMON_MODEL_NAMESPACE};
    use aws_smithy_types::DateTime;

    /// RULE: an instant is reported as an RFC 3339 UTC date-time string. The rows span what
    /// the rendering generalises over: the epoch itself, a whole second, an instant
    /// carrying sub-second digits, and one before the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time() {
        let rows: [(f64, &str); 4] = [
            (0.0, "1970-01-01T00:00:00Z"),
            (1_704_067_200.0, "2024-01-01T00:00:00Z"),
            (1_704_067_200.25, "2024-01-01T00:00:00.25Z"),
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

    /// RULE: a discriminator is the shape's id in the shared internal model's namespace.
    #[test]
    fn a_discriminator_is_the_shape_id_in_the_shared_namespace() {
        for shape in super::DISCRIMINATED_SHAPES {
            let rendered = discriminator(shape);
            assert_eq!(
                rendered,
                format!("{COMMON_MODEL_NAMESPACE}#{shape}"),
                "case {shape:?}: expected the shared-namespace id, got {rendered:?}"
            );
        }
    }
}
