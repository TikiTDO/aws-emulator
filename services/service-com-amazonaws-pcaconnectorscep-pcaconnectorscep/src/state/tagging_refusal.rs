// SPDX-License-Identifier: CC0-1.0

//! The refusal the resource-tagging endpoints answer an ARN naming no resource with.
//!
//! The three tagging operations share ONE route (`/tags/{ResourceArn}`, by method) and are
//! answered by the resource-tagging layer rather than by this service's own resource
//! operations. That layer reports an ARN it resolves to no resource with its OWN error
//! class, which is not the `ResourceNotFoundException` the service's resource operations
//! answer: the status is [`TAG_NOT_FOUND_STATUS`], the wire error code is
//! [`TAG_NOT_FOUND_ERROR_CODE`], and the body carries the single lower-case `message` member
//! [`TAG_NOT_FOUND_MESSAGE`] — fixed text, the same whichever ARN was presented, so the refusal
//! reports no value the caller sent and carries none of the service error's `ResourceId` /
//! `ResourceType` members.
//!
//! The shape is in NO model — the tagging layer's error is not bound to any operation of
//! this service — so it is spelled here once, in the `smithy.framework` namespace the
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

/// The payload of that refusal: the one fixed sentence, as its single member.
#[derive(Debug, Clone)]
pub struct TaggingNotFound {
    /// The sentence, always [`TAG_NOT_FOUND_MESSAGE`].
    pub message: String,
}

impl TaggingNotFound {
    /// The refusal, as the tagging layer words it.
    pub fn new() -> Self {
        Self {
            message: TAG_NOT_FOUND_MESSAGE.to_string(),
        }
    }
}

impl Default for TaggingNotFound {
    fn default() -> Self {
        Self::new()
    }
}

impl ::std::fmt::Display for TaggingNotFound {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{TAG_NOT_FOUND_ERROR_CODE}: {}", self.message)
    }
}

impl ::std::error::Error for TaggingNotFound {}

impl SerializableStruct for TaggingNotFound {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(&NOT_FOUND_MESSAGE_MEMBER, self.message.as_str())?;
        Ok(())
    }
}

/// The error channel of a tagging operation: two channels, not one.
///
/// [`Self::Scaffolded`] is the usual one — the operation's own modelled errors, plus the
/// `ValidationException` a handler needs for a refusal its model cannot express — and
/// [`Self::NotFound`] is the tagging layer's own refusal, which the model declares on no
/// shape. Generic over the operation's modelled error enum, so all three tagging operations
/// share one spelling of the second channel.
#[derive(Debug)]
pub enum TaggingError<E> {
    /// A modelled error, or a refusal the scaffold channel words.
    Scaffolded(Scaffolded<E>),
    /// The tagging layer's refusal of an ARN that names no resource.
    NotFound(TaggingNotFound),
}

impl<E> TaggingError<E> {
    /// The tagging layer's refusal of an ARN naming no resource of this service.
    pub fn not_found() -> Self {
        Self::NotFound(TaggingNotFound::new())
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for TaggingError<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Scaffolded(error) => write!(f, "{error}"),
            Self::NotFound(error) => write!(f, "{error}"),
        }
    }
}

impl<E: ::std::error::Error + 'static> ::std::error::Error for TaggingError<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Scaffolded(error) => error.source(),
            Self::NotFound(error) => error.source(),
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
            Self::NotFound(error) => error,
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::Scaffolded(error) => error.variant_schema(),
            Self::NotFound(_) => &TAG_NOT_FOUND_SCHEMA,
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Scaffolded(error) => error.shape_name(),
            Self::NotFound(_) => TAG_NOT_FOUND_ERROR_CODE,
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Scaffolded(error) => error.http_status(),
            Self::NotFound(_) => TAG_NOT_FOUND_STATUS,
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Scaffolded(error) => error.query_error_code(),
            // No `awsQueryError` trait to honour: the shape is in no model, and this service
            // speaks restJson1 in any case.
            Self::NotFound(_) => None,
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
