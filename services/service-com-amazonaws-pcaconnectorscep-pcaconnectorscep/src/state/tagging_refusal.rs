// SPDX-License-Identifier: CC0-1.0

//! The refusals the resource-tagging endpoints answer with, which are the tagging layer's own
//! error classes rather than any error of this service.
//!
//! The three tagging operations share ONE route (`/tags/{ResourceArn}`, by method) and are
//! answered by the resource-tagging layer rather than by this service's own resource
//! operations. That layer answers its two failures with its OWN error classes, neither of
//! which is one this service's resource operations answer:
//!
//! * a `ResourceArn` it cannot PARSE as an ARN — [`TAG_BAD_REQUEST`], the error code
//!   [`TAG_BAD_REQUEST_ERROR_CODE`] with the sentence [`TAG_BAD_REQUEST_MESSAGE`]; and
//! * an ARN it parsed and resolved to no resource — [`TAG_NOT_FOUND`], the error code
//!   [`TAG_NOT_FOUND_ERROR_CODE`] with the sentence [`TAG_NOT_FOUND_MESSAGE`].
//!
//! Both carry their sentence under the single lower-case `message` member, and both sentences
//! are fixed text — the same whichever value was presented, so neither refusal reports a value
//! the caller sent and neither carries the service error's `ResourceId` / `ResourceType`
//! members.
//!
//! Neither shape is in ANY model — the tagging layer's errors are bound to no operation of
//! this service — so they are spelled here once, in the `smithy.framework` namespace the
//! scaffold reserves for a shape no model declares, and every tagging operation's error
//! channel is [`TaggingError`] over its own modelled errors.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::smithy_server_runtime::OperationError;

use crate::scaffolded::Scaffolded;

/// The sentence the tagging layer's not-found refusal carries: fixed text, naming nothing of
/// the request.
pub const TAG_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code that refusal is identified by (`__type`, and the `x-amzn-errortype`
/// header restJson1 resolves an error shape from). A client branches on this string, so it is
/// the tagging layer's own class name and not the service's `ResourceNotFoundException`.
pub const TAG_NOT_FOUND_ERROR_CODE: &str = "NotFoundException";

/// The HTTP status that refusal carries: the tagging layer states an ARN it cannot resolve as
/// a bad request about the value it was handed, not as a missing resource of this service.
pub const TAG_NOT_FOUND_STATUS: u16 = 400;

/// The sentence the tagging layer's refusal of an unparseable `ResourceArn` carries: fixed
/// text, naming neither the value, the member nor any expression.
pub const TAG_BAD_REQUEST_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The wire error code that refusal is identified by. The tagging layer's own class again,
/// distinct from the not-found one above, so a client can tell "this string is not an ARN"
/// from "that ARN names nothing".
pub const TAG_BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// The HTTP status that refusal carries: a request the layer could not parse is a client
/// error.
pub const TAG_BAD_REQUEST_STATUS: u16 = 400;

/// The `message` member of the tagging layer's not-found shape — lower-case on the wire,
/// which is the name a client reads the sentence out of.
static NOT_FOUND_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#NotFoundException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
);

/// Schema for the tagging layer's not-found shape.
///
/// Deliberately in the `smithy.framework` namespace: this shape is in no service's model, so
/// it must not collide with a modelled one. The wire error code comes from
/// [`OperationError::shape_name`] below, not from this schema.
pub static TAG_NOT_FOUND_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#NotFoundException",
        "smithy.framework",
        "NotFoundException",
    ),
    ShapeType::Structure,
    &[&NOT_FOUND_MESSAGE_MEMBER],
);

/// The `message` member of the tagging layer's bad-request shape — lower-case on the wire for
/// the same reason as the not-found one.
static BAD_REQUEST_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#BadRequestException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
);

/// Schema for the tagging layer's bad-request shape.
///
/// In the `smithy.framework` namespace as well: this service's model does declare a
/// `BadRequestException`, but with a required UPPER-case `Message` member and bound to none of
/// the tagging operations, so the tagging layer's refusal is not that shape and must not
/// collide with it.
pub static TAG_BAD_REQUEST_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#BadRequestException",
        "smithy.framework",
        "BadRequestException",
    ),
    ShapeType::Structure,
    &[&BAD_REQUEST_MESSAGE_MEMBER],
);

/// One refusal class of the resource-tagging layer: everything that distinguishes it on the
/// wire, in one place, so the two classes differ only in these values and share every line of
/// rendering below.
pub struct TaggingRefusalClass {
    /// The wire error code a client branches on.
    pub error_code: &'static str,
    /// The HTTP status the refusal carries.
    pub http_status: u16,
    /// The fixed sentence, which is the whole of what the body says.
    pub message: &'static str,
    /// The shape the body is written through.
    schema: &'static Schema,
    /// That shape's single lower-case `message` member.
    message_member: &'static Schema,
}

/// The tagging layer's refusal of an ARN it resolved to no resource.
pub static TAG_NOT_FOUND: TaggingRefusalClass = TaggingRefusalClass {
    error_code: TAG_NOT_FOUND_ERROR_CODE,
    http_status: TAG_NOT_FOUND_STATUS,
    message: TAG_NOT_FOUND_MESSAGE,
    schema: &TAG_NOT_FOUND_SCHEMA,
    message_member: &NOT_FOUND_MESSAGE_MEMBER,
};

/// The tagging layer's refusal of a `ResourceArn` it could not parse as an ARN.
pub static TAG_BAD_REQUEST: TaggingRefusalClass = TaggingRefusalClass {
    error_code: TAG_BAD_REQUEST_ERROR_CODE,
    http_status: TAG_BAD_REQUEST_STATUS,
    message: TAG_BAD_REQUEST_MESSAGE,
    schema: &TAG_BAD_REQUEST_SCHEMA,
    message_member: &BAD_REQUEST_MESSAGE_MEMBER,
};

/// The payload of one of those refusals: the class it belongs to, and the one fixed sentence
/// that class states, as its single member.
#[derive(Clone)]
pub struct TaggingRefusal {
    /// Which of the layer's classes this is.
    class: &'static TaggingRefusalClass,
    /// The sentence, always the class's own [`TaggingRefusalClass::message`].
    message: String,
}

impl TaggingRefusal {
    /// The refusal of `class`, as the tagging layer words it.
    pub fn new(class: &'static TaggingRefusalClass) -> Self {
        Self {
            class,
            message: class.message.to_string(),
        }
    }
}

impl ::std::fmt::Debug for TaggingRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("TaggingRefusal")
            .field("error_code", &self.class.error_code)
            .field("http_status", &self.class.http_status)
            .field("message", &self.message)
            .finish()
    }
}

impl ::std::fmt::Display for TaggingRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.class.error_code, self.message)
    }
}

impl ::std::error::Error for TaggingRefusal {}

impl SerializableStruct for TaggingRefusal {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(self.class.message_member, self.message.as_str())?;
        Ok(())
    }
}

/// The error channel of a tagging operation: two channels, not one.
///
/// [`Self::Scaffolded`] is the usual one — the operation's own modelled errors, plus the
/// `ValidationException` a handler needs for a refusal its model cannot express — and
/// [`Self::Tagging`] is the resource-tagging layer's own refusal, which the model declares on
/// no shape. Generic over the operation's modelled error enum, so all three tagging operations
/// share one spelling of the second channel.
#[derive(Debug)]
pub enum TaggingError<E> {
    /// A modelled error, or a refusal the scaffold channel words.
    Scaffolded(Scaffolded<E>),
    /// One of the resource-tagging layer's own refusals.
    Tagging(TaggingRefusal),
}

impl<E> TaggingError<E> {
    /// The tagging layer's refusal of an ARN naming no resource of this service.
    pub fn not_found() -> Self {
        Self::Tagging(TaggingRefusal::new(&TAG_NOT_FOUND))
    }

    /// The tagging layer's refusal of a `ResourceArn` it could not parse as an ARN, which it
    /// answers before any resource is looked up.
    pub fn malformed_resource_arn() -> Self {
        Self::Tagging(TaggingRefusal::new(&TAG_BAD_REQUEST))
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for TaggingError<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Scaffolded(error) => write!(f, "{error}"),
            Self::Tagging(error) => write!(f, "{error}"),
        }
    }
}

impl<E: ::std::error::Error + 'static> ::std::error::Error for TaggingError<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Scaffolded(error) => error.source(),
            Self::Tagging(error) => error.source(),
        }
    }
}

/// Every method delegates to the channel the active variant belongs to, so the scaffold
/// wrapper keeps rendering exactly as it does for every other operation and only the tagging
/// layer's own variant answers its own code and status.
impl<E: OperationError> OperationError for TaggingError<E> {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::Scaffolded(error) => error.as_serializable(),
            Self::Tagging(error) => error,
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::Scaffolded(error) => error.variant_schema(),
            Self::Tagging(error) => error.class.schema,
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Scaffolded(error) => error.shape_name(),
            Self::Tagging(error) => error.class.error_code,
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Scaffolded(error) => error.http_status(),
            Self::Tagging(error) => error.class.http_status,
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Scaffolded(error) => error.query_error_code(),
            // No `awsQueryError` trait to honour: neither shape is in a model, and this
            // service speaks restJson1 in any case.
            Self::Tagging(_) => None,
        }
    }
}

/// Lets a refusal the scaffold channel words (`Scaffolded::validation`) lift into this
/// channel, so a handler keeps writing `?` and `.into()`.
impl<E> From<Scaffolded<E>> for TaggingError<E> {
    fn from(scaffolded: Scaffolded<E>) -> Self {
        Self::Scaffolded(scaffolded)
    }
}
