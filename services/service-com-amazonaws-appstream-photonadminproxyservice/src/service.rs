// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appstream-photonadminproxyservice` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: the default
//!   matches the request's resolved route key against this service's signing name,
//!   and this service also claims the streaming-session addresses a
//!   `CreateStreamingURL` points a client at ([`streaming_session`]), one of which
//!   is fetched by a browser that signs nothing and so resolves to no route key.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable, Refusal, Refused,
    ServerHttpProtocol, ServiceIdentity, Service, WireProtocol,
};
use std::sync::Arc;

pub mod streaming_session;

use streaming_session::{Serves, StreamingSessionRoute};

/// The media type a request of this service's JSON protocol carries, which is how a
/// request NAMES that protocol when it carries no `X-Amz-Target`
/// ([`smithy_server_runtime::WireProtocolKind::detect`]).
///
/// This service declares two protocols (`awsJson1_1` and `rpcv2Cbor`), so a request that
/// names neither is refused before any route is asked to serve it. The streaming-session
/// addresses are fetched by a browser, which names no protocol at all, so this service
/// answers them in its JSON protocol — see [`EmulatedService::wire_protocol`].
const JSON_PROTOCOL_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The `com-amazonaws-appstream-photonadminproxyservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The route that resolves a streaming URL this service signed, and the route that
    /// serves the session address it redirects to. Held rather than built per request
    /// because [`Service::override_operation`] hands the router a BORROW of one of them,
    /// and separate because each is reached by a different kind of caller (see
    /// [`ErasedOperation::auth_schemes`] on them).
    issued_streaming_url: StreamingSessionRoute<Eng>,
    streaming_session_endpoint: StreamingSessionRoute<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            issued_streaming_url: StreamingSessionRoute::new(
                Arc::clone(&engine),
                Serves::IssuedUrl,
            ),
            streaming_session_endpoint: StreamingSessionRoute::new(
                Arc::clone(&engine),
                Serves::SessionEndpoint,
            ),
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// The streaming-session route that serves `req`, if either does.
    ///
    /// The one place that decision is made, so `can_handle` claims exactly the requests
    /// `override_operation` then serves.
    fn streaming_session_route(&self, method: &str, uri: &str) -> Option<&dyn ErasedOperation> {
        [
            &self.issued_streaming_url,
            &self.streaming_session_endpoint,
        ]
        .into_iter()
        .find(|route| route.claims(method, uri))
        .map(|route| route as &dyn ErasedOperation)
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry, which
        // the definition below forwards every one of them to. It is implemented HERE
        // rather than handed back bare for ONE reason: the protocol a streaming-session
        // address is served in, which no request to those addresses can name (see
        // [`EmulatedService::wire_protocol`]).
        self
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // A streaming session's two addresses are claimed by their path: the session
        // address a resolved streaming URL redirects to is fetched by a browser, which
        // signs nothing and names no route key, so nothing else would claim it.
        if self
            .streaming_session_route(parts.method.as_str(), &parts.uri.to_string())
            .is_some()
        {
            return true;
        }
        // Otherwise claim a request whose resolved route key is this service's signing
        // name.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The model binds no operation to the streaming-session addresses — they are what
        // a `CreateStreamingURL` points a client at rather than API calls — so they are
        // served here; every other request resolves through the generated routing table.
        self.streaming_session_route(req.method(), req.uri())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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

/// This service's model-derived half, forwarded to the generated entry — except for the
/// ONE thing the entry cannot decide: the protocol a streaming-session address is served
/// in.
///
/// This service declares `awsJson1_1` AND `rpcv2Cbor`, so the framework picks the protocol
/// of a request from the marker the request carries, and refuses one that names neither
/// before any route is asked to serve it. The streaming-session addresses
/// ([`streaming_session`]) carry no marker at all — a signed URL is opened by a browser,
/// which sends no `X-Amz-Target` and no AWS media type — so this definition names the JSON
/// protocol for them, the same one every operation of this service is served in. Nothing
/// else about the request changes: it still goes through the router's own authentication
/// and through [`Service::override_operation`], which is what serves it.
impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for EmulatedService<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.entry.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.entry.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        self.entry.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        if self
            .streaming_session_route(req.method(), req.uri())
            .is_none()
        {
            return self.entry.wire_protocol(req);
        }
        // The entry decides from the request, so it is asked about a request that names
        // this service's JSON protocol: the answer is the protocol the entry declares for
        // that marker, borrowed from the entry rather than from this probe.
        let mut probe = RtRequest::new(SdkBody::empty());
        if probe.set_method(req.method()).is_err() || probe.set_uri(req.uri()).is_err() {
            return self.entry.wire_protocol(req);
        }
        probe
            .headers_mut()
            .insert("content-type", JSON_PROTOCOL_MEDIA_TYPE);
        self.entry.wire_protocol(&probe)
    }
}
