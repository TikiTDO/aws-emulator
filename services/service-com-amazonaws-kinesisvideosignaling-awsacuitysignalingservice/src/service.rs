// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// How this data plane words a `ChannelARN` it cannot serve — the one wording both
/// routes and this file's boundary refusal state.
pub mod channel_refusal;
/// The socket a signaling channel's peers meet on: the channel's own WebSocket data
/// plane, which no buffered request-and-response seam can carry.
pub mod wss;

use channel_refusal::{blames_the_channel_arn, INVALID_ARGUMENT, INVALID_CHANNEL_ARN};

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// routing key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality.
///
/// That path conjunct is what separates services sharing one SigV4 signing name. FIVE
/// emulated services sign as `kinesisvideo` and are addressed by that one endpoint
/// prefix — this signaling endpoint, the Kinesis Video control plane, Media, Archived
/// Media and WebRTC Storage — so the routing key alone matches all five and the
/// router, which refuses rather than guesses, answers `500 AmbiguousService` for ALL
/// of them: a code no AWS model carries. Their modelled path spaces are DISJOINT at
/// the first segment (`v1` here; `createStream`/`listTagsForStream`/… in the control
/// plane; `getMedia` in Media; `getClip`/`getHLSStreamingSessionURL`/`listFragments`
/// in Archived Media; `joinStorageSession` in WebRTC Storage), which is exactly the
/// case this framework discriminator exists for.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]: this
        // service's signing name (`kinesisvideo`) is shared with the Kinesis Video
        // control plane, Media, Archived Media and WebRTC Storage, so the claim is the
        // routing key AND the model-derived first URI path segment. Matching on the
        // signing name alone would have five services claim every `kinesisvideo`
        // request, which the router answers with 500 `AmbiguousService` instead of
        // serving any of them.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service models no `ValidationException`, which is the code the framework
        // would otherwise name for a value that breaks a modelled constraint. Its own
        // error for that is `InvalidArgumentException` — "The value for this input
        // parameter is invalid" — which both operations declare, and which is what a
        // handler answers for the same class of bad input it checks itself (an ARN that
        // is not a signaling-channel ARN). Both the code and the message are replaced,
        // so a client branching on the code sees one code for one logical failure.
        //
        // A value that breaks the modelled `ChannelARN` constraints is worded by this
        // service's OWN sentence for that one member ([`INVALID_CHANNEL_ARN`]) rather than
        // by the framework's constraint report, which would echo the caller's value and
        // the model's regular expression back; see [`blames_the_channel_arn`].
        //
        // An authentication refusal keeps the framework's wording: it is refused before
        // this service is reached, and the modelled `NotAuthorizedException` is the
        // authorization answer, not the authentication one.
        match refusal {
            Refusal::Invalid(violations) if blames_the_channel_arn(violations) => Some(Refused {
                code: INVALID_ARGUMENT.to_string(),
                message: INVALID_CHANNEL_ARN.to_string(),
            }),
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_ARGUMENT.to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "The value for this input parameter is invalid".to_string()),
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
