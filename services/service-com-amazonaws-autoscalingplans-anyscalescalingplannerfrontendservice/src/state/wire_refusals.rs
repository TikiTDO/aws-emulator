// SPDX-License-Identifier: CC0-1.0

//! The refusal this service answers a mutation that overlaps a plan's in-flight
//! transition with, and the error channel that carries it.
//!
//! Two things about an answer are decided by the MODEL for a modelled error: the shape
//! (and with it the `__type` a client branches on) and the `@httpError` status. This
//! service answers the overlapping-mutation refusal under the modelled shape
//! `ConcurrentUpdateException` — the code is the model's — but NOT at the status the
//! model classifies that shape with: the live service answers it as a client error, at
//! 400, and a recorded refusal is the evidence that settles a model-versus-service
//! disagreement. It therefore cannot be constructed as a plain variant of an operation's
//! `#[operation_error]` enum, which would inherit the modelled status.
//!
//! The status is part of the answer, not a detail of it: a 5xx is the retriable
//! server-fault class, which SDK retry policies retry with backoff and surface as a
//! service-side failure, while a 4xx is handed back to the caller at once as a terminal
//! client error. A caller that serialises its own plan mutations branches on that class.
//!
//! The refusal lives here, once, rather than in each mutating operation's file: what a
//! refusal looks like on the wire is a property of the service, and the operations that
//! answer the same condition must not word it two ways. An operation carries it by
//! declaring its error channel [`Refusable`], which widens the operation's own modelled
//! channel without changing how any modelled error is rendered.

use ::aws_smithy_schema::serde::SerializableStruct;
use ::aws_smithy_schema::Schema;
use ::smithy_server_runtime::OperationError;

use crate::ops::types::ConcurrentUpdateException;

/// The status a mutation that overlaps a plan's in-flight transition is refused with:
/// the 4xx of a client error, rather than the 500 the modelled shape's `@httpError` and
/// `@error("server")` classification declare.
///
/// RECORDED behaviour, which is the only evidence there is for it and which overrides the
/// model here: the live service answers this refusal at 400. Contention between a
/// caller's own mutations is something the caller caused and can resolve by re-reading
/// the plan's status and writing again, so it is terminal for the request rather than a
/// fault to retry blindly.
pub const CONCURRENT_UPDATE_STATUS: u16 = 400;

/// The `__type` this refusal is named by, which is the modelled shape's own name: only
/// the STATUS of the refusal differs from the model's declaration, and a client branches
/// on the code.
pub const CONCURRENT_UPDATE_CODE: &str = "ConcurrentUpdateException";

/// AWS's sentence for a mutation that overlaps a plan's in-flight transition.
///
/// The WHOLE sentence is fixed text and interpolates nothing: not the plan the request
/// named, not the status the stored plan is in, not what the request was going to do to
/// it. `pub` and read by every mutating operation, so the one refusal cannot drift into
/// two spellings.
pub const CONCURRENT_UPDATE_MESSAGE: &str = "You already have a pending update to the scaling plan";

/// A refusal this service words itself, carrying the ONE mapping from the refusal to the
/// wire code, the HTTP status and the body it answers with.
#[derive(Debug)]
pub enum WireRefusal {
    /// A mutation of a plan that still has a lifecycle transition in flight — "You
    /// cannot update a scaling plan if it is in the process of being created, updated, or
    /// deleted" — whichever mutating operation was called.
    ConcurrentUpdate(ConcurrentUpdateException),
}

impl WireRefusal {
    /// The refusal of a mutation that overlaps the addressed plan's in-flight transition.
    pub fn concurrent_update() -> Self {
        Self::ConcurrentUpdate(ConcurrentUpdateException {
            message: Some(CONCURRENT_UPDATE_MESSAGE.to_string()),
        })
    }

    /// The body this refusal serializes: the modelled shape itself, so its own schema
    /// decides which members the answer carries.
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::ConcurrentUpdate(e) => e,
        }
    }

    /// The schema the answer is written against.
    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::ConcurrentUpdate(_) => &crate::ops::types::SCHEMA_CONCURRENT_UPDATE_EXCEPTION,
        }
    }

    /// The code on the wire, which the framework reads from an operation's `shape_name`
    /// rather than from the schema.
    fn error_code(&self) -> &'static str {
        match self {
            Self::ConcurrentUpdate(_) => CONCURRENT_UPDATE_CODE,
        }
    }

    /// The HTTP status this refusal answers with.
    fn http_status(&self) -> u16 {
        match self {
            Self::ConcurrentUpdate(_) => CONCURRENT_UPDATE_STATUS,
        }
    }

    /// The sentence the body states, for the [`std::fmt::Display`] rendering below.
    fn message(&self) -> &str {
        match self {
            Self::ConcurrentUpdate(e) => e.message.as_deref().unwrap_or_default(),
        }
    }
}

impl ::std::fmt::Display for WireRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.message())
    }
}

impl ::std::error::Error for WireRefusal {}

/// One operation's error channel, widened by the refusals above.
///
/// `Channel` is the operation's own channel — its modelled errors, the scaffold's 501 and
/// the framework's synthetic `ValidationException` — rendered exactly as it is rendered
/// without this wrapper. `Refused` is one of the service's own refusals, rendered by
/// [`WireRefusal`]. An operation declares `type Error = Refusable<{Op}ErrorModeled>` and
/// keeps writing `Err(SomeModeledError.into())`.
#[derive(Debug)]
pub enum Refusable<E> {
    /// The operation's own error channel.
    Channel(crate::scaffolded::Scaffolded<E>),
    /// A refusal this service words itself.
    Refused(WireRefusal),
}

impl<E> Refusable<E> {
    /// Refuse the request as invalid, through the operation's own channel — the same
    /// synthetic HTTP 400 `ValidationException` envelope
    /// [`crate::scaffolded::Scaffolded::validation`] answers, so a constraint the service
    /// enforces itself reads exactly as one the framework refuses at the boundary.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Channel(crate::scaffolded::Scaffolded::validation(message))
    }
}

impl<E> From<crate::scaffolded::Scaffolded<E>> for Refusable<E> {
    fn from(channel: crate::scaffolded::Scaffolded<E>) -> Self {
        Self::Channel(channel)
    }
}

impl<E> From<WireRefusal> for Refusable<E> {
    fn from(refusal: WireRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for Refusable<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Channel(e) => write!(f, "{e}"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

/// `source()` forwards to the wrapped failure rather than taking the default `None`.
impl<E: ::std::error::Error + 'static> ::std::error::Error for Refusable<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Channel(e) => e.source(),
            Self::Refused(refusal) => refusal.source(),
        }
    }
}

/// Every method delegates: the operation's own channel renders a modelled error exactly
/// as it would without this wrapper, and a refusal renders as [`WireRefusal`] states it —
/// so an operation that carries this channel answers either condition exactly as its
/// siblings do.
impl<E: OperationError> OperationError for Refusable<E> {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::Channel(e) => e.as_serializable(),
            Self::Refused(refusal) => refusal.as_serializable(),
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::Channel(e) => e.variant_schema(),
            Self::Refused(refusal) => refusal.variant_schema(),
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Channel(e) => e.shape_name(),
            Self::Refused(refusal) => refusal.error_code(),
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Channel(e) => e.http_status(),
            Self::Refused(refusal) => refusal.http_status(),
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Channel(e) => e.query_error_code(),
            // The modelled shape's `awsQueryError` trait is inert on this service, which
            // speaks `awsJson1_1`: there is no query envelope to carry a code in.
            Self::Refused(_) => None,
        }
    }
}
