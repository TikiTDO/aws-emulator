// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The service contract: the vended traits one emulated service implements.
//!
//! - [`Service`] — one emulated service: the routing predicate that claims a
//!   request and the optional override seams an author edits. Its model-derived
//!   half — identity, wire protocol, routing table, and the two protocol-correct
//!   framework-error renderers — is reached (not edited) through one accessor,
//!   [`Service::definition`], which hands back a [`ErasedServiceDefinition`].
//! - [`ErasedServiceDefinition`] — the engine-erased, object-safe view of a service's
//!   generated `ServiceDefinition`, exposing its identity, wire protocol, routing table,
//!   and default error renderers so the router reads model data off a
//!   `&dyn Service` without naming the concrete engine the entry is generic over.
//! - [`ErasedOperation`] — the type-erased, object-safe form of one operation, so
//!   a service can hold a heterogeneous map of operations and the framework can
//!   run any of them without naming its concrete type.
//! - [`ServiceIdentity`] — the two model-derived names (shape id + signing name)
//!   carried into every service crate.
//!
//! These traits name both [`ServerProtocol`](aws_smithy_schema_server::ServerProtocol)
//! and [`OperationContext`](crate::OperationContext), so they belong in this
//! crate: `emulator-server` sits below it in the dependency graph and
//! cannot name either type. That lower crate holds only the HTTP transport
//! plumbing (`serve_*`, panic containment, the `EmulatorService` bridge,
//! the route-key helpers). The multi-service router that drives
//! these traits lives in [`crate::multi_service_router`].

use crate::OperationContext;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ServerProtocol;
use std::future::Future;
use std::pin::Pin;

/// A boxed, lifetime-bound future — the object-safe return of
/// [`ErasedOperation::run`]. The future borrows the operation, the service, the
/// request, and the context for `'r`, which is sound because the router awaits it
/// in-scope while all four are alive.
pub type BoxFuture<'r, T> = Pin<Box<dyn Future<Output = T> + Send + 'r>>;

/// The runtime-API server protocol every emulated service speaks: HTTP, with
/// `aws_smithy_runtime_api::http::{Request, Response}` as the message types.
///
/// A `&dyn` of this is what [`WireProtocol::protocol`](crate::WireProtocol::protocol)
/// hands back; the centralized sequence reads it from the service it is running the
/// operation for, so the router never names a concrete protocol type.
pub trait ServerHttpProtocol: ServerProtocol<Request = RtRequest, Response = RtResponse> {}
impl<P> ServerHttpProtocol for P where P: ServerProtocol<Request = RtRequest, Response = RtResponse> {}

/// The identity of one emulated service, vended into every service crate.
///
/// Two fields, both supplied at codegen by `#[smithy_service]` from the model:
/// the service's Smithy `shape_id` and its SigV4 `signing_name`. The signing
/// name is NOT derivable from the shape id — it comes from the service's
/// `aws.auth#sigv4` trait — so it is stored, not computed. Distinct from the
/// host-side service-identity type (which lives in the generator, carries only
/// the shape id plus the scaffold's name-derivation methods, and never ships to
/// a tenant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceIdentity {
    shape_id: &'static str,
    signing_name: &'static str,
    virtual_host_addressing: Option<crate::VirtualHostAddressing>,
}

impl ServiceIdentity {
    /// Build an identity from the two model-derived names. Generated code calls
    /// this with string literals baked at codegen.
    pub const fn new(shape_id: &'static str, signing_name: &'static str) -> Self {
        Self {
            shape_id,
            signing_name,
            virtual_host_addressing: None,
        }
    }

    /// This identity, with the virtual-hosted addresses the service's model declares.
    /// `#[smithy_service]` calls this with the host templates it derived from the model's
    /// endpoint rule set, for a service that declares any.
    pub const fn with_virtual_host_addressing(
        mut self,
        addressing: crate::VirtualHostAddressing,
    ) -> Self {
        self.virtual_host_addressing = Some(addressing);
        self
    }

    /// The virtual-hosted addresses this service's model declares: hosts that name a
    /// request parameter (S3's bucket) in front of the endpoint instead of in the path.
    /// `None` when the model declares none, and for an identity built without them, whose
    /// requests are then routed exactly as they arrive.
    ///
    /// It is carried on the identity, not on the definition, because a tenant that wraps
    /// the generated definition (to word errors, audit, or refuse callers) forwards
    /// `identity()` to it: the addressing reaches the router through every such wrapper.
    pub fn virtual_host_addressing(&self) -> Option<&crate::VirtualHostAddressing> {
        self.virtual_host_addressing.as_ref()
    }

    /// The service's Smithy shape id, e.g. `com.amazonaws.s3#AmazonS3`.
    pub fn shape_id(&self) -> &str {
        self.shape_id
    }

    /// The service's SigV4 signing name, e.g. `s3`.
    pub fn signing_name(&self) -> &str {
        self.signing_name
    }

    /// The signing name as a `'static` string, for consumers (e.g. telemetry
    /// dimensions) that need a `&'static str` key rather than a borrow.
    pub fn signing_name_static(&self) -> &'static str {
        self.signing_name
    }
}

/// One modeled operation, type-erased so a service can hold a heterogeneous map
/// of them and the router can run any of them without naming its concrete type.
///
/// The per-service generated code implements this for each operation. The `run`
/// body is a one-line shim that calls the centralized
/// [`run_operation_http`](crate::run_operation_http) with its concrete `Self`:
/// that call is the monomorphization site that recovers a concrete `Operation`
/// from this erased surface, so the whole deserialize/validate/handle/serialize
/// sequence — which names the per-operation `Schema` constants this vended trait
/// cannot — lives in the vended crate rather than being emitted per operation.
/// `run` is object-safe: it returns a boxed future and names only framework
/// types.
pub trait ErasedOperation: Send + Sync {
    /// Run this operation end to end: deserialize the request body, validate,
    /// call the typed handler, and serialize the output or error into a
    /// response. The router awaits the returned future in-scope.
    ///
    /// `svc` is the service the router looked this operation up from, passed
    /// back through so the centralized sequence can read the service's wire protocol
    /// ([`ErasedServiceDefinition::wire_protocol`], reached via [`Service::definition`])
    /// and reach the service's own refusal wording
    /// ([`Service::override_refusal`]) on a framework-level failure. The protocol
    /// is read off the service's `ServiceDefinition` — the macro-generated carrier the
    /// service holds — through the engine-erased [`ErasedServiceDefinition`] so this
    /// crate names no concrete engine type. The borrow is sound because the
    /// returned [`BoxFuture`] is tied to `'r`, which the router holds alive while
    /// it awaits.
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse>;

    /// The model's auth scheme shape ids for this operation, in priority order; empty means it
    /// is served unsigned (`@optionalAuth` or an empty `@auth`). `None` (an author-written
    /// operation) defers to the modeled operation the request's routing key names.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// The model-derived routing table of one emulated service: every modeled
/// operation this service wires, keyed by the routing key the protocol resolves
/// from a request.
///
/// Owned, not borrowed: each operation moves into the table as a
/// `Box<dyn ErasedOperation>`, so the table can be stored on the service's
/// `ServiceDefinition` without the self-referential borrow a
/// `&`-holding table would force. [`ErasedOperation`] is object-safe (`run`
/// returns a [`BoxFuture`] and names only framework types), so a boxed operation
/// is a sound table value.
///
/// The macro builds one per service in `ServiceDefinition::new` — one
/// [`add_operation`](Self::add_operation) call per modeled operation — and the
/// centralized dispatch reads it through [`get`](Self::get) after the protocol
/// names the operation. The backing store is a `HashMap` and is deliberately
/// hidden: the three-method surface (`new`/`add_operation`/`get`) is all a
/// caller touches, so the representation can change without a caller change. A
/// zero-operation service holds an empty table, which answers `None` for every
/// key.
pub struct OperationRoutingTable {
    operations: std::collections::HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl OperationRoutingTable {
    /// An empty table. Populate it with [`add_operation`](Self::add_operation).
    pub fn new() -> Self {
        Self {
            operations: std::collections::HashMap::new(),
        }
    }

    /// Wire one modeled operation under the `routing_key` the protocol resolves
    /// for it (the operation's shape name, e.g. `CreateBucket`). The key is a
    /// `'static str` because it is a model-derived literal baked at codegen.
    pub fn add_operation(
        &mut self,
        routing_key: &'static str,
        operation: Box<dyn ErasedOperation>,
    ) {
        self.operations.insert(routing_key, operation);
    }

    /// The operation wired under `routing_key`, or `None` when the key names no
    /// modeled operation. A pure lookup — the router computes the key from the
    /// request's protocol and asks the table, so a key that resolves but matches
    /// no modeled operation is the framework's "unknown operation" signal.
    pub fn get(&self, routing_key: &str) -> Option<&dyn ErasedOperation> {
        self.operations.get(routing_key).map(|b| b.as_ref())
    }
}

impl Default for OperationRoutingTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The engine-erased view of one service's generated `ServiceDefinition`: the
/// model-derived half of a service, reached through [`Service::definition`].
///
/// A service holds a `ServiceDefinition<Eng>` generic over its engine, but the router
/// drives services as `Box<dyn Service>` and must read their model data without
/// naming that engine. This object-safe trait is the bridge: the
/// `#[smithy_service]` expansion implements it for `ServiceDefinition<Eng>` for every
/// `Eng`, and [`Service::definition`] hands back a `&dyn ErasedServiceDefinition`, so
/// the router reads identity, wire protocol, routing table, and the
/// protocol-correct error renderers off a `&dyn Service` through one engine-free
/// surface.
///
/// Every method here is model-derived and NOT an author seam: the values are
/// baked at codegen from the Smithy model, and the two error renderers reproduce
/// the service's own protocol envelope. An author who wants to reword a
/// framework or routing error does so through the opt-in
/// [`Service::override_framework_error`] / [`Service::override_routing_error`]
/// seams, which fall back to the renderers here.
pub trait ErasedServiceDefinition {
    /// This service's vended identity: its Smithy shape id and SigV4 signing name.
    fn identity(&self) -> &ServiceIdentity;

    /// Legacy accessor for a codec independent of any request: the codec of the
    /// service's first declared protocol, in the generated order awsJson1_0,
    /// awsJson1_1, rpcv2Cbor, awsQuery, ec2Query, restXml, restJson1.
    ///
    /// For a service that declares one protocol this is the codec every request
    /// is served in. For a service that declares several it is the first one, NOT
    /// the protocol a given request is served in. The framework never calls it;
    /// it exists so code written against it keeps compiling. Code that serves a
    /// request calls [`wire_protocol(req)`](Self::wire_protocol)`.protocol()`.
    ///
    /// It is deliberately not marked `#[deprecated]`: a tenant's release gate
    /// runs clippy with warnings denied, so a deprecation warning would stop the
    /// code this accessor exists for from building.
    ///
    /// Required, so a definition that wraps the generated entry forwards it like
    /// every other method here.
    fn protocol(&self) -> &dyn ServerHttpProtocol;

    /// This service's model-derived operation routing table, built once in the
    /// entry's constructor and returned by reference (never rebuilt per call).
    fn routing_table(&self) -> &OperationRoutingTable;

    /// Render the framework-level routing failure — the request reached the one
    /// claiming service but named no modeled operation — in THIS service's own
    /// protocol envelope. The default rendering the router uses when a service
    /// does not override it (see [`Service::override_routing_error`]).
    ///
    /// It recovers the protocol's own routing failure (unknown or absent key)
    /// and renders it through the SAME pre-dispatch path a matched operation uses
    /// for a framework error, so an unmatched op and a matched-but-failing op
    /// cannot answer in different envelopes (query -> 400 InvalidAction text/xml,
    /// awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
    fn routing_error_response(&self, req: &RtRequest) -> RtResponse;

    /// Render an arbitrary framework-level
    /// [`ProtocolError`](aws_smithy_schema_server::ProtocolError) for `req` in
    /// the envelope of the protocol `req` is served in
    /// ([`wire_protocol`](Self::wire_protocol)) — for a failure the router
    /// detects before (or instead of) naming an operation, such as a malformed
    /// `aws-chunked` body stripped once up front. The default rendering the
    /// router uses when a service does not override it (see
    /// [`Service::override_framework_error`]).
    ///
    /// Rendered through the SAME envelope
    /// [`routing_error_response`](Self::routing_error_response) uses for `req`,
    /// so a transport-level refusal and a routing refusal cannot answer in
    /// different envelopes (a malformed `aws-chunked` body on a `restXml` S3
    /// service is a `400 InvalidRequest` as `application/xml`).
    ///
    /// The router calls this method rather than the envelope's renderer; see
    /// [`WireProtocol`](crate::WireProtocol) for why.
    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse;

    /// [`Self::framework_error_response`] worded as the service chose through
    /// [`Service::override_refusal`].
    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<crate::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        let _ = chosen;
        self.framework_error_response(error, req)
    }

    /// The protocol `req` is served in: the codec and error envelope every step of
    /// the request uses. The [`wire_protocol`](crate::wire_protocol) module says how
    /// it is selected, including for a request that names none of the service's
    /// protocols; the three error renderers above give the router's answer for
    /// such a request, so a tenant that calls them directly sees it too.
    ///
    /// The three error renderers above read it for the `req` they are given, so
    /// they are the only place an error's envelope is chosen.
    fn wire_protocol(&self, req: &RtRequest) -> crate::WireProtocol<'_>;
}

/// One emulated service: the predicate that claims a request plus the optional
/// seams an author edits. Its model-derived half is reached, not edited, through
/// one accessor.
///
/// The service's identity, protocol, operation routing table, and
/// protocol-correct framework-error renderers are model-derived and carried on
/// its `ServiceDefinition`, the macro-generated value the service holds. They are not
/// author seams: the scaffolded `impl Service` exposes them through a single
/// [`definition`](Service::definition) accessor returning a
/// [`ErasedServiceDefinition`], so the router can read them off a `&dyn Service` without
/// this crate naming the concrete engine type the entry is generic over.
///
/// The author owns the behavioral seams in the editable `service.rs`:
/// [`can_handle`](Service::can_handle) (which request this service claims) and
/// four opt-in override hooks — [`override_operation`](Service::override_operation),
/// [`override_refusal`](Service::override_refusal),
/// [`override_routing_error`](Service::override_routing_error), and
/// [`override_framework_error`](Service::override_framework_error) — each
/// defaulting to "let the framework do the default." No deserialization, no
/// telemetry, and no response construction happens here.
pub trait Service: Send + Sync + 'static {
    /// Optional raw payload route. Selection confers no authentication or commit authority.
    /// Ambiguous and body-routed requests remain on the existing buffered path.
    fn payload_route(&self, _parts: &http::request::Parts) -> Option<crate::transport::PayloadRoute> {
        None
    }

    /// This service's model-derived half (identity, wire protocol, routing table, and
    /// the protocol-correct error renderers), as an engine-erased
    /// [`ErasedServiceDefinition`].
    ///
    /// Not an author seam: the scaffolded impl returns a reference to the
    /// service's generated `ServiceDefinition`, whose values are baked at codegen from
    /// the model. The router reads identity, wire protocol, and the routing table
    /// through this one accessor rather than through per-datum trait methods, so
    /// adding or changing a model-derived datum never grows the author-facing
    /// trait surface.
    fn definition(&self) -> &dyn ErasedServiceDefinition;

    /// Whether this service claims the request. Seeded from the service's
    /// signing name and editable by the author for anything the model does not
    /// express (shared signing names, AKID-based routing).
    fn can_handle(&self, parts: &http::request::Parts) -> bool;

    /// An author override of operation routing, consulted BEFORE the model-derived
    /// routing table ([`ErasedServiceDefinition::routing_table`], reached via
    /// [`definition`](Service::definition)). Return `Some` to serve a
    /// request the model's routing key cannot name correctly; return `None` (the
    /// default) to let the routing table resolve it.
    ///
    /// Takes the whole runtime-API request, not just its parts, because a routing
    /// decision is not always in the headers or the URI: the AWS `query` family
    /// (`awsQuery` / `ec2Query`) carries its discriminator in the `Action` field of
    /// the form-urlencoded BODY. This is the request the protocol's own
    /// `routing_key` reads and the matched operation's [`ErasedOperation::run`]
    /// then deserializes, so an override sees exactly the bytes dispatch does.
    ///
    /// Defaults to `None`: a service with no routing it must override keeps the
    /// generated table as its only operation source, and the editable `service.rs`
    /// is where an author replaces this body when the model cannot express the
    /// routing a real service performs.
    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let _ = req;
        None
    }

    /// An author override of the framework-level routing failure. It is asked in
    /// two cases:
    ///
    /// - neither [`override_operation`](Self::override_operation) nor the
    ///   model-derived routing table names an operation, so the request reached the
    ///   one claiming service in one of its protocols but named no modeled
    ///   operation (an unknown or absent routing key);
    /// - the request is in none of the protocols the service declares. Then
    ///   `override_operation` is not asked, because such a request is never served.
    ///
    /// Return `Some` to word that failure in a custom envelope; `None` (the
    /// default) renders [`ErasedServiceDefinition::routing_error_response`], reached via
    /// [`definition`](Service::definition).
    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        let _ = req;
        None
    }

    /// An author override of an arbitrary framework-level
    /// [`ProtocolError`](aws_smithy_schema_server::ProtocolError) the router
    /// detects before (or instead of) naming an operation.
    /// Return `Some` to word that failure in a custom envelope; `None` (the
    /// default) renders [`ErasedServiceDefinition::framework_error_response`], reached
    /// via [`definition`](Service::definition).
    ///
    /// Takes the error by shared reference so the orchestrator can still move the
    /// owned error into that default renderer on `None`
    /// ([`ProtocolError`](aws_smithy_schema_server::ProtocolError) is not `Clone`).
    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<RtResponse> {
        let _ = error;
        None
    }

    /// Choose how this service states a request the framework refused before any
    /// handler ran (an absent required member, or a value outside a declared
    /// bound). Return `None` (the default) to keep this crate's default wording.
    ///
    /// Services disagree on both the error code and the prose for the same
    /// refusal, and a model records neither, so the words are a service concern,
    /// not a framework one. The centralized sequence
    /// ([`run_operation_http`](crate::run_operation_http)) calls this directly on
    /// the service it is running the operation for, so refusal wording is
    /// single-homed here rather than copied onto each operation. It defaults to
    /// `None` — the framework's conventional wording — and the editable
    /// `service.rs` is where an author replaces this body with a `match refusal`
    /// that returns `Some(Refused { .. })` for the cases it rewords.
    fn override_refusal(&self, refusal: &crate::Refusal) -> Option<crate::Refused> {
        let _ = refusal;
        None
    }

    /// An author override of this service's wire protocol for the dispatch
    /// sequence. Return `None` (the default) to serve the protocol the request is in,
    /// from [`ErasedServiceDefinition::wire_protocol`], reached via
    /// [`definition`](Service::definition); return `Some` to serve a different
    /// codec than the model declares — e.g. `rpcv2Cbor` selected from a request's
    /// content-type, or `awsQuery` kept for a legacy caller.
    ///
    /// The one returned protocol drives the modeled request together —
    /// `deserialize_input`, `serialize_output`, and the modeled-error
    /// `serialize_error` — so a request that reaches a modeled operation reads
    /// and answers in that one codec. It does NOT reach the framework-level
    /// failure tail: a refusal (an absent required member, a value outside a
    /// declared bound) and a pre-dispatch codec error still render in the
    /// model-derived envelope the generated shim carries, which an override
    /// cannot reword from here. A service that overrides its protocol and also
    /// needs its framework errors in that codec must additionally reword them
    /// through [`override_framework_error`](Service::override_framework_error).
    ///
    /// The reference is borrowed from `&self`, so an overriding service MUST hold
    /// its protocol in a field and return `&self.field` — build it once in the
    /// service's constructor, never per request.
    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        None
    }
}
